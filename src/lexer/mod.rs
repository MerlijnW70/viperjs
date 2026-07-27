//! Source text to tokens — trivia, punctuators, names, and end of input.
//!
//! What is here is what every later slice stands on: a cursor that can never split a character
//! or read past the end, spans that tile the source exactly, the `newline_before` flag that
//! automatic semicolon insertion will need long before it is used, and identifiers over the
//! real Unicode `ID_Start`/`ID_Continue` sets rather than an ASCII approximation of them.
//!
//! # What is not here yet
//!
//! String literals, templates and regular expressions arrive in the following slices. Until
//! then a character that can only begin one of those — `"`, `'`, `` ` `` — is a
//! [`LexErrorKind::UnexpectedCharacter`], which is also the permanent answer for a character
//! with no token form at all (`@`, `€`, `\0`). Two deferrals remain, each pinned by a test so
//! that implementing it is a deliberate change and not an accident:
//!
//! - **Annex B.1.1 HTML-like comments.** `<!--` lexes as `<` `!` `--` today; `-->` would
//!   additionally need "nothing but trivia before it on this line" state and a Script-vs-Module
//!   goal flag.
//! - **`BigInt` values.** The `n` suffix produces a [`TokenKind::BigInt`], because lexing
//!   `123n` as `123` and the name `n` would be silently valid nonsense — but the value waits for
//!   the BigInt type at M7, and [`numeric_value`] answers `None` for such a span rather than
//!   handing back the nearest `f64`.
//!
//! # Names, and what the lexer refuses to decide
//!
//! An `IdentifierName` becomes a [`TokenKind::Keyword`] only for the 38 spellings §12.7.2
//! reserves unconditionally, and only when no `\u` escape contributed to it. Everything else —
//! `let`, `static`, `async`, `of`, `get`, `implements` — stays a [`TokenKind::Identifier`],
//! because whether those are keywords depends on grammatical context the lexer cannot see. That
//! line is the spec's, not a convenience: §12.7.2 enumerates `ReservedWord` lexically and then
//! spends four more clauses on the contextual cases.
//!
//! # The one property that matters
//!
//! Every token knows its exact extent, and the token spans plus the trivia gaps between them
//! reconstruct the source byte for byte. That is the oracle for this slice (see the module's
//! tests), and it is what keeps every later slice honest: a lexer that quietly loses a byte is
//! a parser that reports the wrong line for the next three years.
//!
//! # How this module is laid out
//!
//! - `token` — [`Token`], [`TokenKind`], and the punctuator table.
//! - `reserved` — [`ReservedWord`], the §12.7.2 list.
//! - `error` — [`LexError`] and its kinds.
//! - `trivia` — white space, comments, and the hashbang (§12.2 – §12.5).
//! - `name` — identifiers, `\u` escapes, and the keyword decision (§12.7).
//! - `number` — numeric literals and their values (§12.9.3), Annex B's legacy forms included.
//! - here — the cursor, and [`Lexer::next_token`]: the one place that decides which of the
//!   above a character belongs to.

mod error;
mod name;
mod number;
mod reserved;
#[cfg(test)]
mod test_support;
mod token;
mod trivia;

pub use self::error::{LexError, LexErrorKind};
pub use self::name::identifier_value;
pub use self::number::numeric_value;
pub use self::reserved::ReservedWord;
pub use self::token::{Token, TokenKind};

use self::token::PUNCTUATORS;
use crate::span::Span;
use crate::unicode_id::is_id_start;

/// A position in the source that can only move forward, one whole code point at a time.
///
/// The point of the type is that it has no panicking path and no unreachable branch: the
/// remaining text is held as a slice rather than an index, so "advance" is
/// [`std::str::Chars::as_str`] and never a range expression that could land mid-character.
struct Cursor<'a> {
    source: &'a str,
    /// The not-yet-consumed tail of `source`. Always a suffix, always on a character boundary.
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            rest: source,
        }
    }

    /// Byte offset of the cursor within the whole source.
    ///
    /// `rest` is a suffix of `source`, so the subtraction cannot underflow. The `as u32` is the
    /// documented >4 GiB truncation — see [`Lexer::new`].
    fn offset(&self) -> u32 {
        (self.source.len() - self.rest.len()) as u32
    }

    fn is_eof(&self) -> bool {
        self.rest.is_empty()
    }

    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    /// The byte `n` positions ahead, if there is one.
    ///
    /// Safe to compare against ASCII: every UTF-8 continuation byte is `>= 0x80`, so a byte
    /// equal to an ASCII character can never be part of a multi-byte code point.
    fn peek_byte(&self, n: usize) -> Option<u8> {
        self.rest.as_bytes().get(n).copied()
    }

    fn starts_with(&self, text: &str) -> bool {
        self.rest.starts_with(text)
    }

    /// Consume one code point, if any.
    fn bump(&mut self) -> Option<char> {
        let mut chars = self.rest.chars();
        let ch = chars.next()?;
        self.rest = chars.as_str();
        Some(ch)
    }

    /// Consume `count` bytes of matched ASCII.
    ///
    /// One byte is one code point for ASCII, so this is `count` bumps — which keeps the
    /// "never split a character" property in exactly one place instead of two.
    fn advance_ascii(&mut self, count: usize) {
        for _ in 0..count {
            let _ = self.bump();
        }
    }
}

/// Turns source text into tokens.
///
/// ```
/// use praxis::lexer::{Lexer, TokenKind};
///
/// let tokens = Lexer::new("{ /* hi */ }").tokens().expect("this source lexes");
/// let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
/// assert_eq!(kinds, [TokenKind::LBrace, TokenKind::RBrace, TokenKind::Eof]);
/// ```
pub struct Lexer<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Lexer<'a> {
    /// A lexer over `source`.
    ///
    /// **Precondition:** `source` is at most `u32::MAX` bytes. [`Span`] holds `u32` offsets, so
    /// a larger source would report truncated positions. Nothing panics if it happens — a bad
    /// span slices to `None` and `line_col` clamps — but diagnostics would point at nonsense.
    /// The check belongs at the embedding boundary where source is accepted (M3's `api.rs`),
    /// not on the token loop, and it will arrive with a decision record.
    pub fn new(source: &'a str) -> Self {
        Self {
            cursor: Cursor::new(source),
        }
    }

    /// The next token, or the error that stopped lexing.
    ///
    /// Once end of input is reached this returns [`TokenKind::Eof`] forever: a parser recovering
    /// from an error will ask again, and it must not matter how many times it does.
    pub fn next_token(&mut self) -> Result<Token, LexError> {
        let newline_before = self.skip_trivia()?;
        let start = self.cursor.offset();

        let Some(first) = self.cursor.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: Span::empty_at(start),
                newline_before,
            });
        };

        // Names before punctuators: no `IdentifierStart` is a punctuator, so the order is a
        // readability choice rather than a correctness one — but `#` and `\` would otherwise
        // fall through to the "no token form" error, which is how they behaved last slice.
        if first == '#' {
            let _ = self.cursor.bump();
            let contains_escape = self.scan_identifier()?;
            return Ok(Token {
                kind: TokenKind::PrivateIdentifier { contains_escape },
                span: Span::new(start, self.cursor.offset()),
                newline_before,
            });
        }
        if first == '\\' || is_id_start(first as u32) {
            let contains_escape = self.scan_identifier()?;
            let span = Span::new(start, self.cursor.offset());
            return Ok(Token {
                kind: self.classify_name(span, contains_escape),
                span,
                newline_before,
            });
        }

        // Numbers must be tried before punctuators, and only for these two shapes: a decimal
        // digit, or a `.` with a digit behind it (§12.9.3's `. DecimalDigits` alternative). The
        // lookahead is what keeps `.` a punctuator and `...` a spread — neither is followed by a
        // digit, so neither reaches here.
        if first.is_ascii_digit()
            || (first == '.' && self.cursor.peek_byte(1).is_some_and(|b| b.is_ascii_digit()))
        {
            let kind = self.scan_number()?;
            return Ok(Token {
                kind,
                span: Span::new(start, self.cursor.offset()),
                newline_before,
            });
        }

        for &(text, kind) in PUNCTUATORS {
            if !self.cursor.starts_with(text) {
                continue;
            }
            // §12.8: `OptionalChainingPunctuator :: ?. [lookahead ∉ DecimalDigit]`. Without this
            // the conditional `a?.5:b` — legal since ES3 — lexes as `a` `?.` `5` and fails to
            // parse. `DecimalDigit` is ASCII 0-9 (§12.9.3), not any Unicode digit.
            if kind == TokenKind::QuestionDot
                && self.cursor.peek_byte(2).is_some_and(|b| b.is_ascii_digit())
            {
                continue;
            }
            self.cursor.advance_ascii(text.len());
            return Ok(Token {
                kind,
                span: Span::new(start, self.cursor.offset()),
                newline_before,
            });
        }

        // Consume the whole code point, not one byte: the error span must cover the character a
        // human sees, and the cursor must stay on a boundary so recovery can continue.
        let _ = self.cursor.bump();
        Err(LexError {
            kind: LexErrorKind::UnexpectedCharacter,
            span: Span::new(start, self.cursor.offset()),
        })
    }

    /// Every token including the final [`TokenKind::Eof`], or the first error.
    pub fn tokens(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token()?;
            let done = token.kind == TokenKind::Eof;
            tokens.push(token);
            if done {
                return Ok(tokens);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::test_support::*;
    /// Rebuild `source` from the token stream alone, and return how far lexing got.
    ///
    /// For each token this appends the trivia gap that preceded it, then the text the token's
    /// own span covers — so the result can only equal the source if the spans are ordered,
    /// non-overlapping, and leave nothing out. It also asserts each span covers the *right*
    /// bytes by cross-checking against [`TokenKind::as_str`]; tiling alone would be satisfied by
    /// spans that are contiguous but shifted.
    ///
    /// Placeholders rather than `unwrap` on a bad span: a panic here would be reported as a
    /// crash in the helper, while a placeholder shows up in the diff of the failing assertion.
    fn retile(source: &str) -> (String, usize) {
        let mut lexer = Lexer::new(source);
        let mut out = String::new();
        let mut at = 0usize;
        loop {
            match lexer.next_token() {
                Ok(token) => {
                    let start = token.span.start as usize;
                    out.push_str(source.get(at..start).unwrap_or("<GAP OUT OF ORDER>"));
                    let text = token.span.slice(source).unwrap_or("<SPAN OFF BOUNDARY>");
                    if let Some(fixed) = token.kind.as_str() {
                        assert_eq!(text, fixed, "span and kind disagree in {source:?}");
                    }
                    out.push_str(text);
                    at = token.span.end as usize;
                    if token.kind == TokenKind::Eof {
                        return (out, at);
                    }
                }
                Err(err) => {
                    let stop = err.span.start as usize;
                    out.push_str(source.get(at..stop).unwrap_or("<GAP OUT OF ORDER>"));
                    return (out, stop);
                }
            }
        }
    }

    #[test]
    fn the_token_spans_and_the_trivia_between_them_reconstruct_the_source_exactly() {
        // The oracle for this slice. Every input here has broken a real lexer at some point.
        let lexes_completely = [
            "",                            // empty file — EOF is still a token
            ";",                           // no trivia at all
            " ; ",                         // trivia on both sides, including trailing
            "\u{feff};",                   // a BOM, which is just white space (§12.2)…
            ";\u{feff};",                  // …anywhere, not only at the start
            "\r",                          // lone CR, old-Mac style
            "\r\n;",                       // CRLF
            "\n\r;",                       // LF then CR — two line breaks, not a pair
            "\u{2028};",                   // LINE SEPARATOR
            "\u{2029};",                   // PARAGRAPH SEPARATOR
            "//x",                         // line comment ended by EOF, not a newline
            "//x\n;",                      // …and one ended by a newline it does not own
            "//x\u{2028};",                // U+2028 ends a line comment too
            "/**/;",                       // the shortest block comment
            "/***/;",                      // an asterisk that is not the terminator
            "/*/*/;",                      // comments do not nest: this one closes
            "/* a\n b */;",                // a block comment spanning lines
            "<!--",                        // Annex B.1.1, deliberately not a comment yet
            ">>>=?.(){}[]...=>",           // longest-match punctuators, back to back
            "{}();,:",                     //
            "/ /=",                        // a slash that is neither comment form
            "\t\u{000b}\u{000c}\u{00a0};", // <TAB> <VT> <FF> and NO-BREAK SPACE
            "\u{1680}\u{2000}\u{200a};",   // exotic <USP> members
            "\u{202f}\u{205f}\u{3000};",   // …and the rest of them
            "a",                           // the shortest name there is
            "a b",                         // …two of them, and the trivia between
            "_$0",                         // both ECMAScript additions plus a digit
            "if else",                     // keywords, whose spans must also line up
            "caf\u{e9} \u{5d0} \u{3042}",  // names that are not ASCII
            "x\u{1d49c}",                  // …including one outside the BMP
            "#priv",                       // a private name, `#` included in the span
            "#!/usr/bin/env node\n;",      // §12.5 hashbang, only at byte 0
            "\\u0061",                     // a name spelled entirely as an escape
            "a\\u{62}c",                   // …and one spelled partly as one
            "\\u{61}\\u{62}",              // two escapes in a row
            "0",                           // the shortest literal there is
            "1_000.5e-3",                  // a decimal wearing everything at once
            ".5",                          // …and one with no integer part
            "?.5",                         // `? .5`, the conditional §12.8's lookahead protects
            "0x1F 0b1_0 0o7 0123 08",      // every radix, Annex B's two included
            "1n 0x2n",                     // BigInt, whose `n` is part of the span
            "1..toString",                 // `1.` then `.` then a name
        ];
        for source in lexes_completely {
            let (tiled, stopped) = retile(source);
            assert_eq!(tiled, source, "retiling {source:?}");
            assert_eq!(stopped, source.len(), "stopped early on {source:?}");
        }

        // Inputs that stop partway: the reconstruction must still be an exact prefix — the
        // lexer may refuse to continue, but it may not invent or lose a byte before it does.
        for source in [
            "/*",
            "/*/",
            "/* x",
            "@",
            "3in",
            "0x",
            "1__0",
            ";\u{200b}",
            "a\\x",
            "#5",
        ] {
            let (tiled, stopped) = retile(source);
            assert_eq!(source.get(..stopped), Some(tiled.as_str()), "on {source:?}");
            assert!(
                stopped < source.len(),
                "{source:?} should not lex completely"
            );
        }
    }

    #[test]
    fn eof_is_a_token_with_an_empty_span_at_the_end_and_repeats_forever() {
        let mut lexer = Lexer::new(" ");
        let eof = lexer.next_token().expect("whitespace only lexes"); // the assertion under test needs the token
        assert_eq!(eof.kind, TokenKind::Eof);
        assert_eq!(eof.span, Span::empty_at(1)); // at the END of the trivia, not the start
        // Asking again must not advance, wrap, or produce a different token: a recovering
        // parser will ask an unbounded number of times.
        for _ in 0..3 {
            assert_eq!(lexer.next_token(), Ok(eof));
        }
        // An empty source is the same story with nothing before it.
        assert_eq!(kinds(""), [TokenKind::Eof]);
        assert_eq!(first("").span, Span::empty_at(0));
    }

    #[test]
    fn punctuators_take_the_longest_match() {
        // Every family where a shorter punctuator is a prefix of a longer one. Each line is a
        // place a first-match-wins lexer produces two tokens where the source has one.
        let families: &[(&str, &[TokenKind])] = &[
            (">>>=", &[TokenKind::GtGtGtEq]),
            (">>>", &[TokenKind::GtGtGt]),
            (">>=", &[TokenKind::GtGtEq]),
            (">>", &[TokenKind::GtGt]),
            (">=", &[TokenKind::GtEq]),
            (">", &[TokenKind::Gt]),
            ("<<=", &[TokenKind::LtLtEq]),
            ("<<", &[TokenKind::LtLt]),
            ("<=", &[TokenKind::LtEq]),
            ("<", &[TokenKind::Lt]),
            ("...", &[TokenKind::DotDotDot]),
            ("..", &[TokenKind::Dot, TokenKind::Dot]),
            (".", &[TokenKind::Dot]),
            ("===", &[TokenKind::EqEqEq]),
            ("==", &[TokenKind::EqEq]),
            ("=>", &[TokenKind::Arrow]),
            ("=", &[TokenKind::Eq]),
            ("!==", &[TokenKind::BangEqEq]),
            ("!=", &[TokenKind::BangEq]),
            ("!", &[TokenKind::Bang]),
            ("**=", &[TokenKind::StarStarEq]),
            ("**", &[TokenKind::StarStar]),
            ("*=", &[TokenKind::StarEq]),
            ("*", &[TokenKind::Star]),
            ("&&=", &[TokenKind::AmpAmpEq]),
            ("&&", &[TokenKind::AmpAmp]),
            ("&=", &[TokenKind::AmpEq]),
            ("&", &[TokenKind::Amp]),
            ("||=", &[TokenKind::PipePipeEq]),
            ("||", &[TokenKind::PipePipe]),
            ("|=", &[TokenKind::PipeEq]),
            ("|", &[TokenKind::Pipe]),
            ("??=", &[TokenKind::QuestionQuestionEq]),
            ("??", &[TokenKind::QuestionQuestion]),
            ("?.", &[TokenKind::QuestionDot]),
            ("?", &[TokenKind::Question]),
            ("++", &[TokenKind::PlusPlus]),
            ("+=", &[TokenKind::PlusEq]),
            ("+", &[TokenKind::Plus]),
            ("--", &[TokenKind::MinusMinus]),
            ("-=", &[TokenKind::MinusEq]),
            ("-", &[TokenKind::Minus]),
            ("/=", &[TokenKind::SlashEq]),
            ("%=", &[TokenKind::PercentEq]),
            ("^=", &[TokenKind::CaretEq]),
            ("^", &[TokenKind::Caret]),
            ("~", &[TokenKind::Tilde]),
            // `>>>>` is a real hazard: the longest match takes three, leaving one.
            (">>>>", &[TokenKind::GtGtGt, TokenKind::Gt]),
            ("====", &[TokenKind::EqEqEq, TokenKind::Eq]),
        ];
        for (source, expected) in families {
            let mut want = expected.to_vec();
            want.push(TokenKind::Eof);
            assert_eq!(kinds(source), want, "lexing {source:?}");
        }
    }

    #[test]
    fn optional_chaining_yields_to_a_following_decimal_digit() {
        // §12.8: `?. [lookahead ∉ DecimalDigit]`. `a?.5:b` is a conditional expression that has
        // been legal since ES3 — the consequent is the numeric literal `.5` — and lexing `?.`
        // there breaks code older than optional chaining. Now that numbers exist, the whole
        // tokenization is visible: a question mark, then a number, and no `?.` anywhere.
        assert_eq!(kinds("?.5"), [TokenKind::Question, NUMBER, TokenKind::Eof]);
        // Every digit, not just one: a `is_ascii_digit` written as `== b'0'` passes the above.
        for digit in '0'..='9' {
            let source = format!("?.{digit}");
            let mut lexer = Lexer::new(&source);
            assert_eq!(
                lexer.next_token().map(|t| t.kind),
                Ok(TokenKind::Question),
                "?.{digit} must not be optional chaining"
            );
        }
        // Anything else after `?.` leaves it a single punctuator…
        assert_eq!(
            kinds("?.("),
            [TokenKind::QuestionDot, TokenKind::LParen, TokenKind::Eof]
        );
        assert_eq!(kinds("?."), [TokenKind::QuestionDot, TokenKind::Eof]);
        assert_eq!(
            kinds("?.["),
            [TokenKind::QuestionDot, TokenKind::LBracket, TokenKind::Eof]
        );
        // …including a non-ASCII digit, which `DecimalDigit` (§12.9.3) is not.
        assert_eq!(
            Lexer::new("?.٥").next_token().map(|t| t.kind),
            Ok(TokenKind::QuestionDot),
            "ARABIC-INDIC DIGIT FIVE is not a DecimalDigit"
        );
        // A space between them is not lookahead: `? .5` was always two tokens.
        assert_eq!(
            kinds("? ."),
            [TokenKind::Question, TokenKind::Dot, TokenKind::Eof]
        );
    }

    #[test]
    fn an_html_open_comment_lexes_as_three_punctuators_until_annex_b_arrives() {
        // Annex B.1.1 gives `<!--` and `-->` alternative comment definitions for web
        // compatibility. They are deliberately NOT implemented in this slice: `-->` needs
        // "only trivia before it on this line" state and a Script-vs-Module goal flag. This
        // test exists so that implementing Annex B changes it on purpose rather than by
        // accident — if it starts failing, that is the day, not a regression.
        assert_eq!(
            kinds("<!--"),
            [
                TokenKind::Lt,
                TokenKind::Bang,
                TokenKind::MinusMinus,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("-->"),
            [TokenKind::MinusMinus, TokenKind::Gt, TokenKind::Eof]
        );
    }

    #[test]
    fn a_character_with_no_token_form_yet_is_an_error_that_covers_the_whole_character() {
        // The error span must cover the character a human sees. Reporting one byte of a
        // multi-byte code point produces a caret pointing into the middle of an emoji — and,
        // worse, would leave the cursor off a boundary.
        let cases = [
            ("@", 1),        // never a token in any edition
            ("\"", 1),       // a string literal: a later slice
            ("`", 1),        // a template: a later slice
            ("\u{0000}", 1), // NUL is legal source text, just not a token start
            // Multi-byte code points that are not identifier characters — `é` and `א` would be
            // names now, so these are drawn from categories Unicode leaves out of ID_Start.
            ("\u{00a7}", 2), // SECTION SIGN, two bytes
            ("€", 3),        // three
            ("🚀", 4),       // four
        ];
        for (source, len) in cases {
            assert_eq!(
                Lexer::new(source).tokens(),
                Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span: Span::new(0, len),
                }),
                "on {source:?}"
            );
        }
        // The offending character is reported where it is, not where the token stream started.
        assert_eq!(
            Lexer::new("; @").tokens(),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(2, 3),
            })
        );
    }

    #[test]
    fn no_single_code_point_can_make_the_lexer_panic() {
        // DR-0002: no input may panic, and "that input is absurd" is not a defence. A sweep
        // rather than a fuzzer because the interesting boundaries are all reachable by hand:
        // every ASCII byte, both ends of every white-space and line-terminator range, and one
        // character from each UTF-8 length class.
        let mut probes: Vec<String> = (0u8..=0x7f).map(|b| (b as char).to_string()).collect();
        for ch in [
            '\u{0085}',
            '\u{00a0}',
            '\u{167f}',
            '\u{1680}',
            '\u{1681}',
            '\u{1fff}',
            '\u{2000}',
            '\u{200a}',
            '\u{200b}',
            '\u{2027}',
            '\u{2028}',
            '\u{2029}',
            '\u{202a}',
            '\u{202f}',
            '\u{205f}',
            '\u{3000}',
            '\u{feff}',
            '\u{ffff}',
            '\u{10000}',
            '\u{10ffff}',
        ] {
            probes.push(ch.to_string());
        }
        for probe in &probes {
            // Alone, after a slash (the trivia fork), and inside each comment form — the four
            // places a byte-oriented lexer can step off a character boundary.
            for source in [
                probe.clone(),
                format!("/{probe}"),
                format!("//{probe}"),
                format!("/*{probe}*/;"),
                format!("/*{probe}"),
            ] {
                // The result does not matter; not unwinding does. Retiling additionally proves
                // no byte was invented or lost on the way.
                let (tiled, stopped) = retile(&source);
                assert_eq!(source.get(..stopped), Some(tiled.as_str()), "on {source:?}");
            }
        }
    }

    #[test]
    fn tokens_collects_the_whole_stream_and_stops_at_the_first_error() {
        let tokens = Lexer::new(" ;\n; ").tokens().expect("this source lexes"); // the assertion under test needs the tokens
        assert_eq!(tokens.len(), 3, "two semicolons and EOF");
        assert_eq!(tokens[0].span, Span::new(1, 2));
        assert!(!tokens[0].newline_before);
        assert_eq!(tokens[1].span, Span::new(3, 4));
        assert!(tokens[1].newline_before);
        assert_eq!(tokens[2].kind, TokenKind::Eof);
        assert_eq!(
            tokens[2].span,
            Span::empty_at(5),
            "EOF sits past the trailing space"
        );
        // The first error wins, and the tokens before it are discarded — a caller that wants
        // them can drive `next_token` itself.
        assert_eq!(
            Lexer::new(";@;").tokens().map(|t| t.len()),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(1, 2),
            })
        );
    }
}
