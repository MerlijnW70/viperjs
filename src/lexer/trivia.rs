//! Trivia: white space, line terminators, and the comment forms (ECMA-262 §12.2 – §12.5).
//!
//! Everything the lexer discards, plus the one fact it keeps from having done so: whether a line
//! terminator was crossed. §12.4 makes that subtler than it looks — a block comment containing a
//! newline counts as one, a line comment never consumes the newline that ends it, and §12.5's
//! hashbang is a comment at byte 0 of the source and a syntax error one byte later.

use super::{LexError, LexErrorKind, Lexer};
use crate::span::Span;

/// ECMA-262 §12.2 White Space, Table 31 — and *only* Table 31.
///
/// Not `char::is_whitespace`, which disagrees in both directions and would therefore be wrong
/// twice over: U+FEFF (`<ZWNBSP>`) is ECMAScript white space and Rust says it is not, while
/// U+0085 (NEL) is not and Rust says it is. §12.2 Note 2 makes the exclusion explicit — the
/// Unicode `White_Space` property is deliberately *not* the criterion.
fn is_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{0009}'      // <TAB>  CHARACTER TABULATION
        | '\u{000b}'    // <VT>   LINE TABULATION
        | '\u{000c}'    // <FF>   FORM FEED
        | '\u{feff}' // <ZWNBSP> ZERO WIDTH NO-BREAK SPACE — white space anywhere, not a
                     // "byte order mark" the lexer strips at position 0 only.
    ) || is_space_separator(ch)
}

/// The spec's `<USP>`: Unicode general category `Space_Separator` (Zs), spelled out.
///
/// Hardcoded because we have no Unicode tables and never will (`Cargo.toml`'s dependency table
/// stays empty). Zs is a closed, stable category — U+0020 and U+00A0 are members, which is why
/// §12.2's table stopped listing them separately. U+200B ZERO WIDTH SPACE is **not** a member:
/// it was reclassified out of Zs in Unicode 4.0, and an engine that still treats it as white
/// space silently accepts source every other engine rejects.
fn is_space_separator(ch: char) -> bool {
    matches!(
        ch,
        '\u{0020}'                  // SPACE
        | '\u{00a0}'                // NO-BREAK SPACE
        | '\u{1680}'                // OGHAM SPACE MARK
        | '\u{2000}'
            ..='\u{200a}'   // EN QUAD .. HAIR SPACE
        | '\u{202f}'                // NARROW NO-BREAK SPACE
        | '\u{205f}'                // MEDIUM MATHEMATICAL SPACE
        | '\u{3000}' // IDEOGRAPHIC SPACE
    )
}

/// ECMA-262 §12.3 Line Terminators, Table 32 — all four, the same set [`crate::span::line_col`]
/// counts lines by. The two agreeing is not optional: a token whose `newline_before` disagrees
/// with the line number in its own error message is a bug report nobody can act on.
fn is_line_terminator(ch: char) -> bool {
    matches!(ch, '\u{000a}' | '\u{000d}' | '\u{2028}' | '\u{2029}')
}

impl<'a> Lexer<'a> {
    /// Consume white space, line terminators and comments; report whether a line terminator was
    /// crossed (directly or inside a block comment).
    pub(super) fn skip_trivia(&mut self) -> Result<bool, LexError> {
        let mut newline = false;
        loop {
            match self.cursor.peek() {
                Some(ch) if is_line_terminator(ch) => {
                    newline = true;
                    let _ = self.cursor.bump();
                }
                Some(ch) if is_whitespace(ch) => {
                    let _ = self.cursor.bump();
                }
                // A `/` is only trivia when a second character says so; otherwise it is the
                // division punctuator and belongs to the caller.
                Some('/') => match self.cursor.peek_byte(1) {
                    Some(b'/') => self.skip_line_comment(),
                    Some(b'*') => newline |= self.skip_block_comment()?,
                    _ => return Ok(newline),
                },
                // §12.5: a hashbang comment is "location-sensitive" — `#!` is a comment only at
                // the very first byte of the source. One byte later the same two characters are
                // a private name that happens to be malformed, and `#!/usr/bin/env node` on
                // line 2 is a syntax error. Hence a position test, not a lookahead.
                Some('#')
                    if self.cursor.offset() == 0 && self.cursor.peek_byte(1) == Some(b'!') =>
                {
                    self.skip_line_comment();
                }
                _ => return Ok(newline),
            }
        }
    }

    /// Consume a two-character comment opener — `//` or §12.5's `#!` — and everything up to but
    /// **not including** the next line terminator.
    ///
    /// §12.4 is emphatic about the exclusion: the terminator "is recognized separately by the
    /// lexical grammar", which is why the presence of a line comment cannot change automatic
    /// semicolon insertion. Swallow it here and `//x\n a` loses the newline that made `a` a new
    /// statement. Running to end of input without a terminator is fine, not an error.
    fn skip_line_comment(&mut self) {
        self.cursor.advance_ascii(2);
        while let Some(ch) = self.cursor.peek() {
            if is_line_terminator(ch) {
                return;
            }
            let _ = self.cursor.bump();
        }
    }

    /// Consume `/* … */`, reporting whether the comment contained a line terminator.
    ///
    /// That return value is the rule from §12.4: "if a MultiLineComment contains a line
    /// terminator code point, then the entire comment is considered to be a LineTerminator for
    /// purposes of parsing by the syntactic grammar". So `a = b /*\n*/ ++c` is two statements,
    /// while `a = b /**/ ++c` is one — a difference no test of the comment alone would reveal.
    fn skip_block_comment(&mut self) -> Result<bool, LexError> {
        let start = self.cursor.offset();
        self.cursor.advance_ascii(2);
        let mut newline = false;
        loop {
            if self.cursor.starts_with("*/") {
                self.cursor.advance_ascii(2);
                return Ok(newline);
            }
            match self.cursor.bump() {
                Some(ch) => newline |= is_line_terminator(ch),
                // §12.4 has no unterminated form. The span runs to the end of the source
                // because that is how much of the file the comment actually consumed — an
                // error pointing at just the `/*` tells the user nothing about the damage.
                None => {
                    return Err(LexError {
                        kind: LexErrorKind::UnterminatedComment,
                        span: Span::new(start, self.cursor.offset()),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::TokenKind;
    use crate::lexer::test_support::*;
    #[test]
    fn every_ecmascript_line_terminator_sets_newline_before() {
        // §12.3 lists four. A lexer that knows only `\n` passes the first and fails the rest,
        // so each is asserted separately rather than as a set.
        for terminator in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            let source = format!("{terminator};");
            assert!(
                first(&source).newline_before,
                "{terminator:?} should end a line"
            );
        }
        // CRLF is one break, but the flag only records "at least one", so what matters is that
        // it is set and that the `;` still lands where it should.
        let token = first("\r\n;");
        assert!(token.newline_before);
        assert_eq!(token.span, Span::new(2, 3));
    }

    #[test]
    fn plain_white_space_does_not_set_newline_before() {
        // The other half of the flag: without this, everything is "on a new line" and ASI
        // inserts semicolons everywhere.
        for space in [" ", "\t", "\u{000b}", "\u{000c}", "\u{00a0}", "\u{feff}"] {
            let source = format!("{space};");
            assert!(
                !first(&source).newline_before,
                "{space:?} is white space, not a line terminator"
            );
        }
        // Nor does the very first token of a source with no trivia at all.
        assert!(!first(";").newline_before);
    }

    #[test]
    fn the_white_space_set_is_the_spec_table_not_rusts_idea_of_white_space() {
        // §12.2 Note 2: ECMAScript white space is Table 31 plus general category Zs, and
        // *deliberately* not the Unicode White_Space property. These three are exactly where a
        // `char::is_whitespace` implementation goes wrong, in both directions.

        // U+FEFF is ECMAScript white space; Rust says it is not.
        assert!(!'\u{feff}'.is_whitespace());
        assert_eq!(kinds("\u{feff};"), [TokenKind::Semicolon, TokenKind::Eof]);

        // U+0085 NEL is not ECMAScript white space; Rust says it is.
        assert!('\u{0085}'.is_whitespace());
        assert_eq!(
            Lexer::new("\u{0085}").next_token().map(|t| t.kind),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(0, 2),
            })
        );

        // U+200B left category Zs in Unicode 4.0 and is not white space in any edition of
        // ECMA-262 — the classic "invisible character breaks the build" report.
        assert!(is_space_separator('\u{200a}')); // HAIR SPACE, the last of the 2000..200A run
        assert!(!is_space_separator('\u{200b}')); // ZERO WIDTH SPACE, one past it
        assert!(!is_space_separator('\u{1fff}')); // one before the run
        // Both ends of every remaining member, so no arm of the table can be dropped unnoticed.
        for space in [
            '\u{0020}', '\u{00a0}', '\u{1680}', '\u{2000}', '\u{2005}', '\u{202f}', '\u{205f}',
            '\u{3000}',
        ] {
            assert!(is_space_separator(space), "{space:?} is in Zs");
            assert!(is_whitespace(space), "{space:?} is <USP>");
        }
        // …and the Table 31 members that are not Zs at all.
        for space in ['\u{0009}', '\u{000b}', '\u{000c}', '\u{feff}'] {
            assert!(is_whitespace(space), "{space:?} is in Table 31");
            assert!(!is_space_separator(space), "{space:?} is not Zs");
        }
        // A line terminator is not white space and vice versa: the two sets are disjoint, and
        // conflating them loses `newline_before`.
        for terminator in ['\n', '\r', '\u{2028}', '\u{2029}'] {
            assert!(is_line_terminator(terminator));
            assert!(!is_whitespace(terminator));
        }
        assert!(!is_line_terminator('\u{2027}')); // one before U+2028
        assert!(!is_line_terminator('\u{202a}')); // one after U+2029
        assert!(!is_line_terminator(' '));
    }

    #[test]
    fn a_line_comment_stops_before_the_terminator_that_still_ends_the_line() {
        // §12.4: the terminator "is recognized separately… and becomes part of the stream of
        // input elements", which is precisely why line comments cannot affect ASI. If
        // `skip_line_comment` swallowed it, this `;` would not know it started a new line.
        let token = first("//comment\n;");
        assert_eq!(token.kind, TokenKind::Semicolon);
        assert!(token.newline_before);

        // Everything after `//` really is inside the comment, semicolons included.
        assert_eq!(kinds("//;;;"), [TokenKind::Eof]);
        // A line comment may end at EOF with no terminator at all — and then nothing precedes
        // EOF's line, so the flag stays false.
        assert!(!first("//comment").newline_before);
        // U+2028 ends a line comment as surely as `\n` does.
        assert!(first("//comment\u{2028};").newline_before);
        // Two slashes are needed. One is division; three are a comment starting with a slash.
        assert_eq!(kinds("/"), [TokenKind::Slash, TokenKind::Eof]);
        assert_eq!(
            kinds("/=;"),
            [TokenKind::SlashEq, TokenKind::Semicolon, TokenKind::Eof]
        );
        assert_eq!(kinds("///x"), [TokenKind::Eof]);
    }

    #[test]
    fn a_block_comment_spanning_lines_counts_as_a_line_terminator() {
        // §12.4: a MultiLineComment containing a line terminator *is* a LineTerminator for the
        // syntactic grammar. This one rule decides whether `a = b /*\n*/ ++c` is one statement
        // or two, and it is invisible to any test that only checks the comment was skipped.
        assert!(first("/*\n*/;").newline_before);
        assert!(first("/*\r*/;").newline_before);
        assert!(first("/*\u{2028}*/;").newline_before);
        assert!(first("/*\u{2029}*/;").newline_before);
        // …and a comment on one line does NOT set it. Without this assertion, "always true"
        // passes the four above.
        assert!(!first("/* no break here */;").newline_before);
        // The flag survives further trivia after the comment.
        assert!(first("/*\n*/ /* and more */ ;").newline_before);
        // It is also reached the other way round: a newline before a single-line comment.
        assert!(first("\n/* x */;").newline_before);
    }

    #[test]
    fn block_comments_end_at_the_first_close_and_do_not_nest() {
        // §12.4: "Multi-line comments cannot nest." The inner `/*` is ordinary comment text, so
        // the FIRST `*/` closes — an engine that counts openings would swallow the `;`.
        assert_eq!(kinds("/* /* */;"), [TokenKind::Semicolon, TokenKind::Eof]);
        // An asterisk that is not followed by a slash keeps the comment open.
        assert_eq!(kinds("/***/;"), [TokenKind::Semicolon, TokenKind::Eof]);
        assert_eq!(kinds("/* * */;"), [TokenKind::Semicolon, TokenKind::Eof]);
        // The empty comment, and one whose body starts with the slash of its own opener.
        assert_eq!(kinds("/**/;"), [TokenKind::Semicolon, TokenKind::Eof]);
        assert_eq!(kinds("/*/*/;"), [TokenKind::Semicolon, TokenKind::Eof]);
        // Multi-byte characters inside a comment must not be mistaken for `*` or `/` bytes.
        assert_eq!(kinds("/* 🚀 é */;"), [TokenKind::Semicolon, TokenKind::Eof]);
    }

    #[test]
    fn an_unterminated_block_comment_is_an_error_spanning_to_the_end_of_the_source() {
        // The span reaches the end because that is how much the comment consumed; pointing at
        // just the `/*` would understate it. `/*/` is the classic — it looks closed and is not.
        for source in ["/*", "/*/", "/* x", "/**", ";/* x\ny"] {
            let start = source.find("/*").unwrap_or(0) as u32; // the literal contains `/*` by construction
            assert_eq!(
                Lexer::new(source).tokens(),
                Err(LexError {
                    kind: LexErrorKind::UnterminatedComment,
                    span: Span::new(start, source.len() as u32),
                }),
                "on {source:?}"
            );
        }
        // The two-character close really is required: adding it makes each of these lex.
        assert_eq!(kinds("/*/ */;"), [TokenKind::Semicolon, TokenKind::Eof]);
    }

    #[test]
    fn a_hashbang_is_a_comment_only_at_the_very_first_byte() {
        // §12.5: hashbang comments are "location-sensitive". At byte 0 this is how a script
        // becomes executable; anywhere else the same two characters are a malformed private
        // name, and treating them as a comment would silently delete a line of code.
        assert_eq!(
            kinds("#!/usr/bin/env node\n;"),
            [TokenKind::Semicolon, TokenKind::Eof]
        );
        assert!(first("#!/usr/bin/env node\n;").newline_before);
        // It runs to the line terminator and no further, and may end at EOF instead.
        assert_eq!(kinds("#!x"), [TokenKind::Eof]);
        assert_eq!(
            kinds("#!"),
            [TokenKind::Eof],
            "the shortest hashbang there is"
        );
        // A second one on the next line is NOT a comment — it is at byte 4.
        assert_eq!(
            Lexer::new("#!x\n#!y").tokens(),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(5, 6),
            })
        );
        // One byte later it is not a comment. Even a single space disqualifies it, because the
        // hashbang precedes everything — including white space.
        for source in [" #!x", "\n#!x", ";#!x"] {
            assert_eq!(
                Lexer::new(source).tokens().map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span: Span::new(2, 3),
                }),
                "on {source:?}"
            );
        }
        // A `#` at byte 0 that is not followed by `!` is an ordinary private name.
        assert_eq!(
            kinds("#x"),
            [
                TokenKind::PrivateIdentifier {
                    contains_escape: false
                },
                TokenKind::Eof
            ]
        );
    }
}
