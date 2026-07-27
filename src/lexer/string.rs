//! String literals (ECMA-262 §12.9.4): the escapes, and the code units they denote.
//!
//! # A string is not text
//!
//! [`string_value`] returns `Vec<u16>` and not `String`, because `"\uD800"` is a legal literal
//! whose value is one unpaired surrogate — a thing no Rust `String` can hold and no `char` can
//! name. DR-0004 has the argument; the short version is that every way of squeezing a JavaScript
//! string into UTF-8 is silently wrong for an input a script can write on purpose.
//!
//! # What may and may not appear literally
//!
//! §12.9.4 forbids exactly three code points inside a literal: the closing quote, `\`, and
//! LineTerminators — with `<LS>` and `<PS>` carved back out as explicit alternatives, so U+2028
//! and U+2029 *may* appear raw even though they end a line everywhere else. That exception is
//! what made ECMAScript a superset of JSON, and it is the one place in the lexer where the four
//! line terminators of §12.3 do not act alike.

use super::escape::{CodeUnits, hex_value, utf16_encode};
use super::{LexError, LexErrorKind, Lexer, TokenKind};
use crate::span::Span;

/// The code units a string literal denotes (§12.9.4's `SV`), or `None` if `span` does not cover
/// one.
///
/// Always allocates, and always UTF-16: see the module documentation and DR-0004. There is no
/// borrowed fast path because there is nothing to borrow — the source is UTF-8 and the value is
/// code units, so even `"abc"` is a conversion rather than a slice.
///
/// Returns `None` for a span off a character boundary, one not delimited by matching quotes, or
/// one containing an escape that the lexer would have rejected.
///
/// ```
/// use praxis::lexer::{Goal, Lexer, TokenKind, string_value};
///
/// let source = r#""café""#;
/// let token = Lexer::new(source).next_token(Goal::Div).expect("this lexes");
/// assert_eq!(token.kind, TokenKind::String { legacy_escape: false });
/// assert_eq!(string_value(source, token.span), Some(vec![0x63, 0x61, 0x66, 0xe9]));
/// ```
pub fn string_value(source: &str, span: Span) -> Option<Vec<u16>> {
    let text = span.slice(source)?;
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = text.strip_prefix(quote)?.strip_suffix(quote)?;

    // Re-read the body with the same escape readers the scan used, so a value can never disagree
    // with what was validated. `body` cannot itself be a `"` — `strip_suffix` would have taken
    // the same character `strip_prefix` did only if the text were one character long, and a
    // one-character text has no prefix left to strip.
    let mut lexer = Lexer::new(body);
    let mut units = Vec::with_capacity(body.len());
    while !lexer.cursor.is_eof() {
        if lexer.cursor.peek() == Some('\\') {
            match lexer.read_string_escape() {
                Ok(escape) => escape.units.push_onto(&mut units),
                Err(_) => return None,
            }
        } else {
            let ch = lexer.cursor.bump()?;
            utf16_encode(ch as u32).push_onto(&mut units);
        }
    }
    Some(units)
}

/// One decoded `\`-escape from inside a string literal.
pub(super) struct StringEscape {
    /// What it contributes to the value.
    units: CodeUnits,
    /// Whether it was one of Annex B's legacy forms — a `LegacyOctalEscapeSequence` like `\7`,
    /// or a `NonOctalDecimalEscapeSequence`, which is to say `\8` or `\9`.
    ///
    /// §12.9.4.1 makes both a Syntax Error in strict code, and its Note 2 is worth reading
    /// twice: a literal may *precede* the "use strict" directive that makes it strict, as in
    /// `function invalid() { "\7"; "use strict"; }`. So the lexer cannot decide this even in
    /// principle — it records the fact and the parser, which will have read the directive
    /// prologue by then, delivers the verdict.
    legacy: bool,
}

impl Lexer<'_> {
    /// Scan a `StringLiteral` delimited by `quote`, which the cursor is sitting on.
    pub(super) fn scan_string(&mut self, quote: char) -> Result<TokenKind, LexError> {
        let start = self.cursor.offset();
        let _ = self.cursor.bump();
        let mut legacy_escape = false;
        loop {
            let Some(ch) = self.cursor.peek() else {
                return Err(self.unterminated_string(start));
            };
            match ch {
                _ if ch == quote => {
                    let _ = self.cursor.bump();
                    return Ok(TokenKind::String { legacy_escape });
                }
                '\\' => legacy_escape |= self.read_string_escape()?.legacy,
                // §12.9.4 admits `<LS>` and `<PS>` as literal characters but not `<LF>` or
                // `<CR>`: a string may not span a line break unless it is escaped into one.
                // Reporting here rather than running to the end of the file keeps a missing
                // quote from swallowing the rest of the program.
                '\n' | '\r' => return Err(self.unterminated_string(start)),
                _ => {
                    let _ = self.cursor.bump();
                }
            }
        }
    }

    /// The error a literal gets when its closing quote never arrives.
    ///
    /// Spans from the opening quote to wherever the search gave up — the end of the line, or the
    /// end of the source. Pointing at just the quote would understate how much text is affected,
    /// and pointing at the whole file would overstate it.
    fn unterminated_string(&self, start: u32) -> LexError {
        LexError {
            kind: LexErrorKind::UnterminatedStringLiteral,
            span: Span::new(start, self.cursor.offset()),
        }
    }

    /// Consume one `\` and the `EscapeSequence` or `LineTerminatorSequence` after it.
    ///
    /// The dispatch is §12.9.4's `EscapeSequence` in order of how much lookahead each form needs.
    /// Note the default: a `\` before anything not otherwise spoken for is a `NonEscapeCharacter`
    /// and simply *is* that character, so `\q` is `q` and `\🚀` is the rocket. That is a
    /// deliberate part of the grammar rather than lenient error recovery, which is why there is
    /// no "unknown escape" error to report.
    pub(super) fn read_string_escape(&mut self) -> Result<StringEscape, LexError> {
        let plain = |units| StringEscape {
            units,
            legacy: false,
        };
        // The `\` is consumed by every branch; `read_unicode_escape` wants to consume it itself.
        let Some(after) = self.cursor.peek_byte(1) else {
            // A backslash at the very end of the source: there is no escape here, and no
            // closing quote either.
            let start = self.cursor.offset();
            self.cursor.advance_ascii(1);
            return Err(LexError {
                kind: LexErrorKind::UnterminatedStringLiteral,
                span: Span::new(start, self.cursor.offset()),
            });
        };
        if after == b'u' {
            return Ok(plain(utf16_encode(self.read_unicode_escape()?)));
        }

        self.cursor.advance_ascii(1); // the `\`
        match after {
            // `SingleEscapeCharacter`, Table 33. Nine of them, and no more: `\a` is not 0x07.
            b'b' | b't' | b'n' | b'v' | b'f' | b'r' | b'"' | b'\'' | b'\\' => {
                self.cursor.advance_ascii(1);
                let unit = match after {
                    b'b' => 0x0008,
                    b't' => 0x0009,
                    b'n' => 0x000a,
                    b'v' => 0x000b,
                    b'f' => 0x000c,
                    b'r' => 0x000d,
                    b'"' => 0x0022,
                    b'\'' => 0x0027,
                    _ => 0x005c,
                };
                Ok(plain(CodeUnits::One(unit)))
            }
            b'x' => Ok(plain(CodeUnits::One(self.read_hex_escape()?))),
            b'0'..=b'9' => self.read_numeric_escape(u32::from(after - b'0')),
            _ => {
                // `LineContinuation :: \ LineTerminatorSequence`, contributing the empty String
                // — and a `LineTerminatorSequence` is CRLF *as one*, so the `\n` of a `\r\n`
                // must not be left behind to end the literal.
                let ch = self.cursor.peek();
                if matches!(ch, Some('\n' | '\r' | '\u{2028}' | '\u{2029}')) {
                    let _ = self.cursor.bump();
                    if ch == Some('\r') && self.cursor.peek() == Some('\n') {
                        let _ = self.cursor.bump();
                    }
                    return Ok(plain(CodeUnits::Nothing));
                }
                // `NonEscapeCharacter`: the code point stands for itself.
                match self.cursor.bump() {
                    Some(ch) => Ok(plain(utf16_encode(ch as u32))),
                    None => Ok(plain(CodeUnits::Nothing)),
                }
            }
        }
    }

    /// `HexEscapeSequence :: x HexDigit HexDigit` — exactly two digits, with the `x` consumed.
    fn read_hex_escape(&mut self) -> Result<u16, LexError> {
        let start = self.cursor.offset().saturating_sub(1); // include the `\` in the span
        self.cursor.advance_ascii(1); // the `x`
        let mut value: u16 = 0;
        for _ in 0..2 {
            let Some(digit) = self.cursor.peek().and_then(hex_value) else {
                return Err(LexError {
                    kind: LexErrorKind::InvalidHexEscape,
                    span: Span::new(start, self.cursor.offset()),
                });
            };
            let _ = self.cursor.bump();
            // Bounded by construction: two hex digits cannot exceed 0xFF.
            value = value * 16 + digit as u16;
        }
        Ok(value)
    }

    /// The escapes that begin with a digit, with the `\` consumed: `\0`, and Annex B's legacy
    /// forms.
    ///
    /// Only `\0` is not legacy, and only when nothing digit-shaped follows it —
    /// `EscapeSequence :: 0 [lookahead ∉ DecimalDigit]`. So `"\0"` is an ordinary NUL that
    /// strict code may use, while `"\08"` is a `LegacyOctalEscapeSequence` that it may not, even
    /// though both denote NUL and differ only in what comes next.
    ///
    /// The octal run's length depends on its first digit, because the value may not exceed 255:
    /// `ZeroToThree` takes two more digits (`\377` is 255) but `FourToSeven` takes only one
    /// (`\477` is `\47` followed by a literal `7`).
    ///
    /// `first` is the digit the caller already matched to get here, passed rather than read
    /// again: re-deriving it would need a "what if it is not a digit after all" arm that nothing
    /// can reach and therefore no test can pin.
    fn read_numeric_escape(&mut self, first: u32) -> Result<StringEscape, LexError> {
        let _ = self.cursor.bump();

        // `NonOctalDecimalEscapeSequence :: one of 8 9` — worth its own name in the grammar, and
        // it denotes the digit character itself: `"\8"` is `"8"`.
        if first > 7 {
            return Ok(StringEscape {
                units: CodeUnits::One(0x0030 + first as u16),
                legacy: true,
            });
        }

        let next_is_digit = self.cursor.peek().is_some_and(|ch| ch.is_ascii_digit());
        if first == 0 && !next_is_digit {
            return Ok(StringEscape {
                units: CodeUnits::One(0x0000),
                legacy: false,
            });
        }

        let mut value = first;
        let more = if first <= 3 { 2 } else { 1 };
        for _ in 0..more {
            let Some(digit) = self.cursor.peek().and_then(|ch| ch.to_digit(8)) else {
                break;
            };
            let _ = self.cursor.bump();
            // Bounded by the run lengths above: at most 0o377, which is 255.
            value = value * 8 + digit;
        }
        Ok(StringEscape {
            units: CodeUnits::One(value as u16),
            legacy: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Goal;
    use crate::lexer::test_support::*;

    /// The code units of the one string literal in `source`.
    fn value(source: &str) -> Vec<u16> {
        let token = first(source);
        string_value(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should have a string value")) // a test about the value cannot proceed without one
    }

    /// The value of `source`, as a Rust `String` — for the many cases where it is well-formed.
    fn text(source: &str) -> String {
        String::from_utf16(&value(source))
            .unwrap_or_else(|_| panic!("{source:?} should be well-formed UTF-16")) // same
    }

    /// The error `source` fails with, or a panic naming what it produced instead.
    fn error(source: &str) -> LexError {
        match Lexer::new(source).tokens(Goal::Div) {
            Err(err) => err,
            Ok(tokens) => panic!("{source:?} should not lex, got {tokens:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn a_string_literal_runs_from_one_quote_to_its_match() {
        for (source, expected) in [
            (r#""""#, ""),
            ("''", ""),
            (r#""abc""#, "abc"),
            ("'abc'", "abc"),
            // The other quote is an ordinary character inside a literal — which is the whole
            // reason there are two kinds.
            (r#""it's""#, "it's"),
            (r#"'say "hi"'"#, "say \"hi\""),
            // Non-ASCII passes through as itself, astral characters included.
            (r#""café""#, "café"),
            (r#""🚀""#, "🚀"),
        ] {
            assert_eq!(text(source), expected, "value of {source:?}");
            assert_eq!(kinds(source), [STRING, TokenKind::Eof], "{source:?}");
            assert_eq!(first(source).span, Span::new(0, source.len() as u32));
        }
        // Adjacent literals are separate tokens, and a quote does not end the wrong one.
        assert_eq!(kinds(r#""a" 'b'"#), [STRING, STRING, TokenKind::Eof]);
        assert_eq!(
            kinds(r#""a"+'b'"#),
            [STRING, TokenKind::Plus, STRING, TokenKind::Eof]
        );
        // An astral character is two code units, not one — the count a script would observe.
        assert_eq!(value(r#""🚀""#), vec![0xd83d, 0xde80]);
        assert_eq!(value(r#""a🚀b""#).len(), 4);
    }

    #[test]
    fn an_unterminated_literal_stops_at_the_line_rather_than_eating_the_file() {
        // §12.9.4: `<LF>` and `<CR>` may not appear literally, so a missing quote is caught at
        // the end of the line. Running on would turn one typo into a cascade of errors much
        // further down, all of them wrong.
        for source in [r#"""#, "'", r#""abc"#, "\"abc\nx = 1", "\"abc\rx", r#""\"#] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::UnterminatedStringLiteral,
                "on {source:?}"
            );
        }
        assert_eq!(
            error("\"abc\ndef\"").span,
            Span::new(0, 4),
            "the span stops at the newline, not at the quote on the next line"
        );
        // A quote of the other kind does not close it.
        assert_eq!(
            error(r#""abc'"#).kind,
            LexErrorKind::UnterminatedStringLiteral
        );
    }

    #[test]
    fn line_separator_and_paragraph_separator_may_appear_where_no_other_terminator_may() {
        // §12.9.4 lists `<LS>` and `<PS>` as their own alternatives, so unlike `<LF>` and `<CR>`
        // they are ordinary characters inside a literal. This is the exception that made
        // ECMAScript a superset of JSON, and it is the one place the four line terminators of
        // §12.3 behave differently from one another.
        assert_eq!(value("\"a\u{2028}b\""), vec![0x61, 0x2028, 0x62]);
        assert_eq!(value("\"a\u{2029}b\""), vec![0x61, 0x2029, 0x62]);
        assert_eq!(kinds("\"\u{2028}\""), [STRING, TokenKind::Eof]);
        // …and the newline flag of a token after such a literal is not set by a separator that
        // was inside it.
        let tokens = Lexer::new("\"\u{2028}\";")
            .tokens(Goal::Div)
            .unwrap_or_else(|err| panic!("should lex, got {}", err.kind)); // the assertion needs the tokens
        assert!(!tokens[1].newline_before);
    }

    #[test]
    fn the_nine_single_character_escapes_are_table_33_and_nothing_else() {
        // Each entry of §12.9.4's Table 33, by code unit value rather than by name, because the
        // names are where they get confused: `\b` is BACKSPACE and not a word boundary, `\v` is
        // LINE TABULATION, and `\f` is FORM FEED.
        for (source, unit) in [
            (r#""\b""#, 0x0008),
            (r#""\t""#, 0x0009),
            (r#""\n""#, 0x000a),
            (r#""\v""#, 0x000b),
            (r#""\f""#, 0x000c),
            (r#""\r""#, 0x000d),
            (r#""\"""#, 0x0022),
            (r#""\'""#, 0x0027),
            (r#""\\""#, 0x005c),
        ] {
            assert_eq!(value(source), vec![unit], "value of {source:?}");
        }
        // Anything else is a `NonEscapeCharacter` and stands for itself — not an error, and not
        // some other character. `\a` is `a`, and specifically not 0x07 as it is in C.
        for (source, expected) in [
            (r#""\a""#, "a"),
            (r#""\q""#, "q"),
            (r#""\-""#, "-"),
            (r#""\ ""#, " "),
            (r#""\é""#, "é"),
        ] {
            assert_eq!(text(source), expected, "value of {source:?}");
        }
        // An escaped astral character is still two code units.
        assert_eq!(value(r#""\🚀""#), vec![0xd83d, 0xde80]);
        // An escaped quote does not end the literal — the point of the escape.
        assert_eq!(kinds(r#""a\"b""#), [STRING, TokenKind::Eof]);
        assert_eq!(text(r#""a\"b""#), "a\"b");
    }

    #[test]
    fn a_line_continuation_contributes_nothing_and_swallows_a_crlf_whole() {
        // `LineContinuation :: \ LineTerminatorSequence`, whose SV is the empty String — the one
        // way a literal may span a line.
        assert_eq!(text("\"a\\\nb\""), "ab");
        assert_eq!(text("\"a\\\rb\""), "ab");
        assert_eq!(text("\"a\\\u{2028}b\""), "ab");
        assert_eq!(text("\"a\\\u{2029}b\""), "ab");
        // A `LineTerminatorSequence` is CRLF *as one* (§12.3). Consume only the `\r` and the
        // `\n` is left to end the literal, so this must lex rather than fail.
        assert_eq!(text("\"a\\\r\nb\""), "ab");
        assert_eq!(kinds("\"a\\\r\nb\""), [STRING, TokenKind::Eof]);
        // …and two continuations in a row still contribute nothing at all.
        assert_eq!(text("\"\\\n\\\n\""), "");
        // The continuation is invisible to the value but not to the span.
        assert_eq!(first("\"a\\\nb\"").span, Span::new(0, 6));
    }

    #[test]
    fn a_hex_escape_is_exactly_two_digits() {
        assert_eq!(value(r#""\x41""#), vec![0x41]);
        assert_eq!(value(r#""\x00""#), vec![0x00]);
        assert_eq!(value(r#""\xff""#), vec![0xff]);
        assert_eq!(value(r#""\xFF""#), vec![0xff]);
        // A third digit is an ordinary character, so `\x414` is "A4".
        assert_eq!(text(r#""\x414""#), "A4");
        // Fewer than two, or a non-digit, is an error rather than a shorter escape.
        for source in [r#""\x""#, r#""\x4""#, r#""\xg""#, r#""\x4g""#, r#""\x"#] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::InvalidHexEscape,
                "on {source:?}"
            );
        }
    }

    #[test]
    fn a_unicode_escape_may_denote_a_lone_surrogate() {
        // The reason [`string_value`] returns code units and not text (DR-0004). `\uD800` is a
        // well-formed literal whose value is one unpaired surrogate; there is no `char` for it
        // and no UTF-8 encoding of it.
        assert_eq!(value(r#""\ud800""#), vec![0xd800]);
        assert_eq!(value(r#""\uDFFF""#), vec![0xdfff]);
        assert!(String::from_utf16(&value(r#""\ud800""#)).is_err());
        // `\u{D800}` reaches the same place by the other form: `CodePoint` admits surrogates,
        // and UTF16EncodeCodePoint passes anything below 0x10000 through unchanged.
        assert_eq!(value(r#""\u{d800}""#), vec![0xd800]);
        // Two escapes that happen to pair up make an ordinary astral character — nothing
        // re-encodes them, they simply sit next to each other.
        assert_eq!(value(r#""🚀""#), vec![0xd83d, 0xde80]);
        assert_eq!(text(r#""🚀""#), "🚀");
        // The braced form encodes astral code points as a pair, unlike the four-digit form.
        assert_eq!(value(r#""\u{1f680}""#), vec![0xd83d, 0xde80]);
        assert_eq!(value(r#""\u{61}""#), vec![0x61]);
        assert_eq!(value(r#""\u{000000061}""#), vec![0x61]);
        // Malformed and out-of-range are the errors the identifier slice already defined, since
        // it is the same production being read by the same reader.
        for source in [
            r#""\u""#,
            r#""\u1""#,
            r#""\u123""#,
            r#""\u{}""#,
            r#""\u{1""#,
        ] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::InvalidUnicodeEscape,
                "on {source:?}"
            );
        }
        assert_eq!(
            error(r#""\u{110000}""#).kind,
            LexErrorKind::CodePointOutOfRange
        );
    }

    #[test]
    fn annex_b_legacy_escapes_are_decoded_and_flagged_for_the_parser() {
        // §12.9.4.1 makes `LegacyOctalEscapeSequence` and `NonOctalDecimalEscapeSequence` a
        // Syntax Error in strict code. Its Note 2 is why the lexer must not decide: a literal
        // can precede the directive that makes it strict, as in
        // `function invalid() { "\7"; "use strict"; }`.
        for (source, units) in [
            (r#""\1""#, vec![1]),
            (r#""\7""#, vec![7]),
            (r#""\12""#, vec![0o12]),
            (r#""\012""#, vec![0o12]),
            (r#""\377""#, vec![255]),    // the largest one there is
            (r#""\08""#, vec![0, 0x38]), // `\0` is legacy here, because a digit follows
            (r#""\8""#, vec![0x38]),     // NonOctalDecimalEscapeSequence: the digit itself
            (r#""\9""#, vec![0x39]),
        ] {
            assert_eq!(value(source), units, "value of {source:?}");
            assert_eq!(
                kinds(source),
                [LEGACY_STRING, TokenKind::Eof],
                "kinds of {source:?}"
            );
        }
        // The run length depends on the first digit, because the value may not exceed 255:
        // `ZeroToThree` takes three digits and `FourToSeven` only two, so the trailing `7` here
        // is a character rather than part of the escape.
        assert_eq!(value(r#""\477""#), vec![0o47, 0x37]);
        assert_eq!(value(r#""\3777""#), vec![255, 0x37]);
        // `\0` on its own is NOT legacy — it is `EscapeSequence :: 0` — and strict code may use
        // it. The lookahead is the only difference between it and `"\08"` above.
        assert_eq!(value(r#""\0""#), vec![0]);
        assert_eq!(kinds(r#""\0""#), [STRING, TokenKind::Eof]);
        assert_eq!(text(r#""\0a""#), "\u{0}a");
        assert_eq!(kinds(r#""\0a""#), [STRING, TokenKind::Eof]);
        // One legacy escape anywhere marks the whole literal.
        assert_eq!(kinds(r#""abc\7def""#), [LEGACY_STRING, TokenKind::Eof]);
        assert_eq!(kinds(r#""abc""#), [STRING, TokenKind::Eof]);
    }

    #[test]
    fn string_value_answers_rather_than_panicking_on_a_span_it_was_not_given() {
        assert_eq!(string_value(r#""a""#, Span::new(0, 99)), None);
        assert_eq!(string_value("\"é\"", Span::new(0, 2)), None);
        // Not delimited by quotes at all, or by mismatched ones. `aba` and `xx` are the ones
        // that matter: they open and close with the *same* character, so a check that only
        // matched the two ends against each other would read them as strings.
        assert_eq!(string_value("abc", Span::new(0, 3)), None);
        assert_eq!(string_value("aba", Span::new(0, 3)), None);
        assert_eq!(string_value("xx", Span::new(0, 2)), None);
        assert_eq!(string_value("``", Span::new(0, 2)), None);
        assert_eq!(string_value(r#""abc'"#, Span::new(0, 5)), None);
        // A single quote character is a prefix with no suffix left behind it.
        assert_eq!(string_value(r#"""#, Span::new(0, 1)), None);
        assert_eq!(string_value("''", Span::new(0, 2)), Some(vec![]));
        // An escape the lexer would have refused has no value either.
        assert_eq!(string_value(r#""\x4""#, Span::new(0, 5)), None);
        assert_eq!(string_value(r#""\u{110000}""#, Span::new(0, 12)), None);
        // A valid span that does not start at zero.
        assert_eq!(
            string_value(r#"x = "hi""#, Span::new(4, 8)),
            Some(vec![0x68, 0x69])
        );
    }

    #[test]
    fn no_string_literal_however_odd_can_panic() {
        // DR-0002. Every one of these is something a script author can type, and several are
        // things a fuzzer finds first: a backslash against the end of input, an escape cut in
        // half by the closing quote, and a literal made entirely of continuations.
        let cases = [
            r#"""#.to_string(),
            r#""\"#.to_string(),
            r#""\u"#.to_string(),
            r#""\u{"#.to_string(),
            r#""\x"#.to_string(),
            r#""\0"#.to_string(),
            "\"\\\r".to_string(),
            "\"\\\r\n\"".to_string(),
            format!("\"{}\"", "\\\n".repeat(2000)),
            format!("\"{}\"", "\\u0041".repeat(2000)),
            format!("\"{}\"", "\\377".repeat(2000)),
            format!("\"{}", "a".repeat(5000)),
            format!("\"{}\"", "\u{2028}".repeat(2000)),
        ];
        for source in &cases {
            // The verdict does not matter; not unwinding does.
            if let Ok(tokens) = Lexer::new(source).tokens(Goal::Div) {
                assert!(
                    string_value(source, tokens[0].span).is_some(),
                    "{:?} lexed but has no value",
                    &source[..source.len().min(16)]
                );
            }
        }
        assert_eq!(
            value(&format!("\"{}\"", "\\\n".repeat(2000))),
            Vec::<u16>::new()
        );
        assert_eq!(
            value(&format!("\"{}\"", "\\u0041".repeat(2000))).len(),
            2000
        );
    }
}
