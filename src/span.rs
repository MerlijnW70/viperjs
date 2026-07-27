//! Source positions — byte spans, and the line/column translation used by every error message.
//!
//! This is the first module of the engine and it is deliberately small, because its real job is
//! to be the worked example of the bar every later module is held to: logic with observable
//! branches, tests that die when a branch flips, and comments that say *why* rather than *what*.

/// A half-open byte range `[start, end)` into a source text.
///
/// Byte offsets, not character indices: the lexer walks bytes, and every later stage (parser,
/// compiler, error reporter) needs to slice the original text cheaply. Translation to the
/// line/column pair a human reads happens once, at the moment an error is rendered
/// ([`line_col`]) — never on the hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Span {
    /// First byte of the range.
    pub start: u32,
    /// One past the last byte of the range.
    pub end: u32,
}

impl Span {
    /// A span covering `[start, end)`.
    ///
    /// A reversed range is corrected to the empty span at `start` rather than panicking or
    /// silently producing a range that later slicing would panic on: a position is diagnostic
    /// metadata, and a bad one must never be able to take down an embedder (see the crate-level
    /// no-panic invariant).
    ///
    /// Written as a `max` rather than an `if end < start`, deliberately. That branch has an
    /// **equivalent mutant**: at `end == start` both arms produce the same span, so `<` and `<=`
    /// are indistinguishable by any test, and mutation testing reports untestable logic. A
    /// branch no test can pin is a branch that should not exist — the arithmetic says the same
    /// thing with nothing left to guard.
    pub fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end: end.max(start),
        }
    }

    /// The empty span at `offset` — where a "expected X here" error points.
    pub fn empty_at(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Length in bytes.
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    /// Whether the span covers no bytes.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both — used to give a parent AST node the extent of its
    /// children (`a + b` spans from `a`'s start to `b`'s end).
    pub fn to(&self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// The text this span covers, or `None` if it does not land on character boundaries of
    /// `source` (a corrupt span must not panic a slice).
    pub fn slice<'a>(&self, source: &'a str) -> Option<&'a str> {
        source.get(self.start as usize..self.end as usize)
    }
}

/// A 1-based line and column, as an error message reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, counted in **UTF-16 code units**.
    pub column: u32,
}

/// Translate a byte offset into a 1-based line/column.
///
/// Two decisions worth stating, because both are places a "reasonable" implementation is wrong:
///
/// 1. **All four ECMAScript line terminators end a line**, not just `\n`: LF, CR, U+2028 (LINE
///    SEPARATOR) and U+2029 (PARAGRAPH SEPARATOR) — ECMA-262 §12.3. A CRLF pair counts once.
///    An engine that only splits on `\n` reports the wrong line for any file with old-Mac
///    endings or a stray U+2028, which is a real (and very confusing) failure for the user.
///
/// 2. **The column is counted in UTF-16 code units**, because that is what every JavaScript
///    tool — stack traces, source maps, editors — means by "column". An astral character
///    (emoji, rare CJK) is therefore two columns, matching what `String.prototype.length`
///    would say about the text before it.
///
/// An offset past the end of `source`, or one landing inside a character, is clamped to the
/// nearest earlier boundary instead of panicking.
pub fn line_col(source: &str, offset: u32) -> LineCol {
    let offset = offset as usize;
    let mut line = 1u32;
    let mut column = 1u32;
    let mut prev_was_cr = false;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if prev_was_cr && ch == '\n' {
            // The `\n` of a CRLF pair: the CR already ended the line, so this byte adds
            // nothing. Without this, every CRLF file reports its lines doubled.
            prev_was_cr = false;
            continue;
        }
        prev_was_cr = ch == '\r';
        if matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf16() as u32;
        }
    }

    LineCol { line, column }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reversed_span_collapses_instead_of_panicking() {
        // The guard's BOTH directions: a reversed range collapses to empty at `start`…
        let bad = Span::new(10, 4);
        assert_eq!(bad, Span { start: 10, end: 10 });
        assert!(bad.is_empty());
        // …and an ordinary range is passed through untouched. (Forcing the comparison either
        // way breaks exactly one of these two.)
        let good = Span::new(4, 10);
        assert_eq!(good, Span { start: 4, end: 10 });
        assert_eq!(good.len(), 6);
        // The boundary itself: start == end is legal, not "reversed".
        assert_eq!(Span::new(7, 7), Span::empty_at(7));
        assert_eq!(Span::empty_at(7).len(), 0);
    }

    #[test]
    fn to_covers_both_spans_in_either_order() {
        let a = Span::new(2, 5);
        let b = Span::new(9, 12);
        // Ordered and reversed must give the same hull — a parent node's extent cannot depend
        // on which child the parser happened to visit first.
        assert_eq!(a.to(b), Span::new(2, 12));
        assert_eq!(b.to(a), Span::new(2, 12));
        // A contained span does not shrink the hull (pins min/max, not first/last).
        assert_eq!(a.to(Span::new(3, 4)), a);
    }

    #[test]
    fn slice_returns_none_instead_of_panicking_on_a_bad_span() {
        let src = "let x = 1;";
        assert_eq!(Span::new(4, 5).slice(src), Some("x"));
        // Past the end: None, never a panic.
        assert_eq!(Span::new(4, 999).slice(src), None);
        // Mid-character: also None. `é` is two bytes, so offset 1 is not a boundary.
        assert_eq!(Span::new(0, 1).slice("é"), None);
    }

    #[test]
    fn line_col_counts_every_ecmascript_line_terminator() {
        // ECMA-262 §12.3 lists four. An implementation that only knows `\n` passes the first
        // of these and fails the rest — which is precisely why each is asserted separately.
        assert_eq!(line_col("a\nb", 2), LineCol { line: 2, column: 1 });
        assert_eq!(line_col("a\rb", 2), LineCol { line: 2, column: 1 });
        assert_eq!(line_col("a\u{2028}b", 4), LineCol { line: 2, column: 1 });
        assert_eq!(line_col("a\u{2029}b", 4), LineCol { line: 2, column: 1 });
    }

    #[test]
    fn a_source_starting_with_a_newline_is_not_mid_crlf() {
        // The initial value of `prev_was_cr`. Start it at `true` and the very first `\n` of a
        // file is swallowed as the tail of a CRLF pair that never happened — every line number
        // in the file off by one, for the single most ordinary input there is.
        assert_eq!(line_col("\nb", 1), LineCol { line: 2, column: 1 });
        assert_eq!(line_col("\n\nb", 2), LineCol { line: 3, column: 1 });
    }

    #[test]
    fn a_crlf_pair_ends_one_line_not_two() {
        // The `prev_was_cr` guard. Drop it and this file reads as line 3.
        assert_eq!(line_col("a\r\nb", 3), LineCol { line: 2, column: 1 });
        // The guard must RESET after it fires: a `\n` immediately following a complete CRLF
        // pair is its own line break, not the tail of another pair. Leave the flag set and
        // every blank line in a Windows file stops counting.
        assert_eq!(line_col("a\r\n\nb", 4), LineCol { line: 3, column: 1 });
        // …but a LONE `\n` following a non-CR still ends its own line, and two CRLF pairs are
        // two lines — the guard must not swallow every `\n`.
        assert_eq!(line_col("a\n\nb", 3), LineCol { line: 3, column: 1 });
        assert_eq!(line_col("a\r\nb\r\nc", 6), LineCol { line: 3, column: 1 });
        // A CR followed by something other than LF is still a line break of its own.
        assert_eq!(line_col("a\rb\nc", 4), LineCol { line: 3, column: 1 });
    }

    #[test]
    fn columns_are_utf16_code_units() {
        // `é` is one UTF-16 unit (2 bytes), the rocket is TWO units (4 bytes). A byte-counting
        // or char-counting implementation gets a different answer for exactly one of these.
        assert_eq!(line_col("é x", 3), LineCol { line: 1, column: 3 });
        assert_eq!(line_col("🚀 x", 5), LineCol { line: 1, column: 4 });
    }

    #[test]
    fn an_out_of_range_offset_clamps_to_the_end() {
        // Diagnostics must survive a wrong offset: the worst outcome is an imprecise caret.
        let src = "ab\ncd";
        assert_eq!(line_col(src, 99), LineCol { line: 2, column: 3 });
        assert_eq!(line_col("", 0), LineCol { line: 1, column: 1 });
        assert_eq!(line_col("", 5), LineCol { line: 1, column: 1 });
        // Offset 0 is the origin, and offset 1 is one column along — pins that the loop's
        // `i >= offset` break is not off by one.
        assert_eq!(line_col(src, 0), LineCol { line: 1, column: 1 });
        assert_eq!(line_col(src, 1), LineCol { line: 1, column: 2 });
    }
}
