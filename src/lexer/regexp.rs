//! Regular expression literals (ECMA-262 §12.9.5) — finding where one ends.
//!
//! §12.9.5 is unusually explicit about how little this is: "the productions below describe the
//! syntax for a regular expression literal and are used by the input element scanner **to find
//! the end** of the regular expression literal. The source text comprising the
//! RegularExpressionBody and the RegularExpressionFlags are subsequently parsed again using the
//! more stringent ECMAScript Regular Expression grammar (22.2.1)."
//!
//! So `/(?<=/` is a perfectly good *token* whose pattern is nonsense, and saying so is M4's job.
//! What this file owes the parser is the extent, plus the two static semantics that carve it up:
//! `BodyText` (§12.9.5.1) and `FlagText` (§12.9.5.2).
//!
//! # Two rules that are already someone else's problem
//!
//! `RegularExpressionFirstChar` excludes `*`, and Note 2 says the code unit sequence `//` starts
//! a comment rather than an empty literal. Both are enforced without a line of code here,
//! because [`super::trivia`] consumes `//` and `/*` before a `/` ever reaches this scanner — so
//! a body that arrives here is already non-empty and already does not start with `*`. Writing
//! the checks anyway would add two branches no input could reach.

use super::{LexError, LexErrorKind, Lexer, TokenKind};
use crate::span::Span;
use crate::unicode_id::is_id_continue;

/// The two halves of a regular expression literal, as §12.9.5's static semantics define them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegExpParts {
    /// `BodyText` (§12.9.5.1): the `RegularExpressionBody`, between the slashes and excluding
    /// both. Never empty — `//` is a comment.
    pub body: Span,
    /// `FlagText` (§12.9.5.2): the `RegularExpressionFlags` after the closing slash. Often
    /// empty, and never validated here — `RegularExpressionFlags` is any run of
    /// `IdentifierPartChar`, so `/a/xyz` is a well-formed *token* that §22.2 will reject.
    pub flags: Span,
}

/// Split a regular expression literal into its body and its flags, or `None` if `span` does not
/// cover one.
///
/// The split is found by re-running the same scan that delimited the token, so the two can never
/// disagree about where the closing slash was — which matters more than it sounds, since finding
/// it means knowing which slashes were escaped and which were inside a character class.
///
/// ```
/// use viperjs::lexer::{Goal, Lexer, TokenKind, regexp_parts};
///
/// let source = r"/ab\/[/]c/gi";
/// let token = Lexer::new(source).next_token(Goal::RegExp).expect("this lexes");
/// assert_eq!(token.kind, TokenKind::RegExp);
/// let parts = regexp_parts(source, token.span).expect("a regular expression literal");
/// // The escaped slash and the one inside the class both stay in the body.
/// assert_eq!(parts.body.slice(source), Some(r"ab\/[/]c"));
/// assert_eq!(parts.flags.slice(source), Some("gi"));
/// ```
pub fn regexp_parts(source: &str, span: Span) -> Option<RegExpParts> {
    let text = span.slice(source)?;
    if !text.starts_with('/') {
        return None;
    }
    let mut lexer = Lexer::new(text);
    lexer.scan_regexp_body().ok()?; // a span that does not hold a literal has no parts, which is what `None` says
    // `scan_regexp_body` stops just past the closing slash, so the body is what lies between the
    // two and the flags are the whole remainder — the scanner accepts any run of
    // `IdentifierPartChar` there, and so does this.
    let close = lexer.cursor.offset();
    let base = span.start;
    Some(RegExpParts {
        body: Span::new(base + 1, base + close - 1),
        flags: Span::new(base + close, span.end),
    })
}

impl Lexer<'_> {
    /// Scan a `RegularExpressionLiteral`, with the cursor on the opening `/`.
    ///
    /// Reached only under [`crate::lexer::Goal::RegExp`]: §12.6 gives `/` two readings and the lexer is not the
    /// one that can choose between them.
    pub(super) fn scan_regexp(&mut self) -> Result<TokenKind, LexError> {
        self.scan_regexp_body()?;
        // `RegularExpressionFlags :: [empty] | RegularExpressionFlags IdentifierPartChar`.
        // IdentifierPart**Char**, note — not `IdentifierPart` — so unlike a name, a flag list
        // admits no `\u` escapes at all. `/a/g` is not `/a/g`; it is a literal with no
        // flags followed by a stray backslash.
        while let Some(ch) = self.cursor.peek() {
            if !is_id_continue(ch as u32) {
                break;
            }
            let _ = self.cursor.bump();
        }
        Ok(TokenKind::RegExp)
    }

    /// Consume `/ RegularExpressionBody /`, leaving the cursor just past the closing slash.
    ///
    /// The whole difficulty is that three things change what a `/` means: a preceding backslash,
    /// an enclosing character class, and nothing else. Getting the class wrong is the classic
    /// bug — `/[/]/` is one literal matching a slash, and a scanner that stops at the first
    /// unescaped `/` reads it as `/[/` followed by garbage.
    fn scan_regexp_body(&mut self) -> Result<(), LexError> {
        let start = self.cursor.offset();
        let _ = self.cursor.bump(); // the opening `/`
        let mut in_class = false;
        loop {
            let Some(ch) = self.cursor.peek() else {
                return Err(self.unterminated_regexp(start));
            };
            // `RegularExpressionNonTerminator :: SourceCharacter but not LineTerminator`, and
            // every alternative bottoms out in one — so a line terminator ends the search
            // wherever it appears, inside a class and after a backslash included. A literal may
            // not span a line, full stop.
            if matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                return Err(self.unterminated_regexp(start));
            }
            let _ = self.cursor.bump();
            match ch {
                // `RegularExpressionBackslashSequence :: \ RegularExpressionNonTerminator`. The
                // escaped character is consumed without being looked at, which is what keeps
                // `/\//` from ending at its middle slash and `/\[/` from opening a class.
                '\\' => match self.cursor.peek() {
                    Some(next) if !matches!(next, '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                        let _ = self.cursor.bump();
                    }
                    _ => return Err(self.unterminated_regexp(start)),
                },
                // Classes do not nest: `RegularExpressionClassChar` excludes only `]` and `\`,
                // so a `[` inside a class is an ordinary character and `/[[]/` closes at the
                // first `]`.
                '[' if !in_class => in_class = true,
                ']' if in_class => in_class = false,
                '/' if !in_class => return Ok(()),
                _ => {}
            }
        }
    }

    /// The error a literal gets when its closing slash never arrives.
    fn unterminated_regexp(&self, start: u32) -> LexError {
        LexError {
            kind: LexErrorKind::UnterminatedRegExp,
            span: Span::new(start, self.cursor.offset()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Goal;
    use crate::lexer::test_support::*;

    /// The kinds of `source` read with a `/` treated as opening a literal.
    fn regexp_kinds(source: &str) -> Vec<TokenKind> {
        Lexer::new(source)
            .tokens(Goal::RegExp)
            .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // a test asserting clean lexing has nothing to say if lexing failed
            .iter()
            .map(|t| t.kind)
            .collect()
    }

    /// The body and flags of the one literal in `source`.
    fn parts(source: &str) -> (String, String) {
        let mut lexer = Lexer::new(source);
        let token = lexer
            .next_token(Goal::RegExp)
            .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)); // same
        let parts = regexp_parts(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should split into parts")); // same
        (
            parts.body.slice(source).unwrap_or("<BAD SPAN>").to_string(),
            parts
                .flags
                .slice(source)
                .unwrap_or("<BAD SPAN>")
                .to_string(),
        )
    }

    /// The error `source` fails with under [`Goal::RegExp`].
    fn error(source: &str) -> LexError {
        match Lexer::new(source).tokens(Goal::RegExp) {
            Err(err) => err,
            Ok(tokens) => panic!("{source:?} should not lex, got {tokens:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn a_slash_is_division_or_a_literal_according_to_the_goal_symbol_alone() {
        // §12.6 gives `/` two readings — `DivPunctuator` under `InputElementDiv`,
        // `RegularExpressionLiteral` under `InputElementRegExp` — and the same characters mean
        // different things under each. No amount of looking at the source resolves it; only the
        // parser knows whether it expects an operand or an operator.
        assert_eq!(
            kinds("/a/g"),
            [
                TokenKind::Slash,
                PLAIN,
                TokenKind::Slash,
                PLAIN,
                TokenKind::Eof
            ]
        );
        assert_eq!(regexp_kinds("/a/g"), [TokenKind::RegExp, TokenKind::Eof]);
        // `/=` is the starkest case: one token under `Div`, and under `RegExp` a literal whose
        // body is `=`.
        assert_eq!(
            kinds("/=/"),
            [TokenKind::SlashEq, TokenKind::Slash, TokenKind::Eof]
        );
        assert_eq!(regexp_kinds("/=/"), [TokenKind::RegExp, TokenKind::Eof]);
        assert_eq!(parts("/=/").0, "=");
        // A literal is one token however long, and the token after it is read normally.
        assert_eq!(
            regexp_kinds("/a/.test"),
            [TokenKind::RegExp, TokenKind::Dot, PLAIN, TokenKind::Eof]
        );
    }

    #[test]
    fn comments_win_over_literals_because_the_body_can_be_neither_empty_nor_start_with_a_star() {
        // Note 2: "instead of representing an empty regular expression literal, the code unit
        // sequence `//` starts a single-line comment". And `RegularExpressionFirstChar` excludes
        // `*`, so `/*` is always a comment opener. Both hold under the RegExp goal, where a
        // careless scanner would grab the slash first.
        assert_eq!(regexp_kinds("// not a literal"), [TokenKind::Eof]);
        assert_eq!(regexp_kinds("/* nor this */"), [TokenKind::Eof]);
        assert_eq!(
            Lexer::new("/*/").tokens(Goal::RegExp).map(|t| t.len()),
            Err(LexError {
                kind: LexErrorKind::UnterminatedComment,
                span: Span::new(0, 3),
            }),
            "`/*` opens a comment even where a literal could start, so this is an unclosed one"
        );
        // …and the empty literal really is written the way Note 2 says.
        assert_eq!(parts("/(?:)/"), ("(?:)".to_string(), String::new()));
    }

    #[test]
    fn a_character_class_makes_a_slash_ordinary() {
        // `RegularExpressionClassChar` excludes only `]` and `\`, so `/` inside `[...]` is an
        // ordinary character. A scanner that stops at the first unescaped `/` splits this
        // literal in half and everything after it is garbage.
        assert_eq!(parts("/[/]/"), ("[/]".to_string(), String::new()));
        assert_eq!(
            parts("/[abc/def]/g"),
            ("[abc/def]".to_string(), "g".to_string())
        );
        // Classes do not nest: a `[` inside one is ordinary, so this closes at the first `]`.
        assert_eq!(parts("/[[]/"), ("[[]".to_string(), String::new()));
        // A `]` outside a class is ordinary too — `RegularExpressionChar` excludes only `\`,
        // `/` and `[`.
        assert_eq!(parts("/]/"), ("]".to_string(), String::new()));
        assert_eq!(parts("/a]b/"), ("a]b".to_string(), String::new()));
        // An escaped `[` opens nothing, so the `/` after it still closes the literal.
        assert_eq!(parts(r"/\[/"), (r"\[".to_string(), String::new()));
        // An escaped `]` inside a class does not close it.
        assert_eq!(parts(r"/[\]/]/"), (r"[\]/]".to_string(), String::new()));
    }

    #[test]
    fn a_backslash_hides_whatever_follows_it() {
        // `RegularExpressionBackslashSequence :: \ RegularExpressionNonTerminator` — the escaped
        // character is consumed without being read, which is the whole point.
        assert_eq!(parts(r"/\//"), (r"\/".to_string(), String::new()));
        assert_eq!(parts(r"/a\/b/"), (r"a\/b".to_string(), String::new()));
        assert_eq!(parts(r"/\\/"), (r"\\".to_string(), String::new()));
        // A doubled backslash does not hide the slash after it: `/\\/` closes, so this is the
        // literal `\\` and then more source.
        assert_eq!(regexp_kinds(r"/\\/g"), [TokenKind::RegExp, TokenKind::Eof]);
        assert_eq!(parts(r"/\\/g"), (r"\\".to_string(), "g".to_string()));
    }

    #[test]
    fn flags_are_any_run_of_identifier_part_characters_and_are_not_checked_here() {
        // `RegularExpressionFlags :: [empty] | RegularExpressionFlags IdentifierPartChar`. The
        // lexer's job is the extent; §22.2 decides whether the flags mean anything.
        assert_eq!(parts("/a/"), ("a".to_string(), String::new()));
        assert_eq!(parts("/a/g"), ("a".to_string(), "g".to_string()));
        assert_eq!(parts("/a/gimsuy"), ("a".to_string(), "gimsuy".to_string()));
        assert_eq!(
            parts("/a/xyz"),
            ("a".to_string(), "xyz".to_string()),
            "nonsense, but a token"
        );
        assert_eq!(parts("/a/$_"), ("a".to_string(), "$_".to_string()));
        assert_eq!(parts("/a/\u{e9}"), ("a".to_string(), "\u{e9}".to_string()));
        // Flags stop at the first character that could not continue a name.
        assert_eq!(
            regexp_kinds("/a/g;"),
            [TokenKind::RegExp, TokenKind::Semicolon, TokenKind::Eof]
        );
        assert_eq!(parts("/a/g;"), ("a".to_string(), "g".to_string()));
        // `IdentifierPartChar`, not `IdentifierPart`: a flag list admits no escapes at all. So
        // this is a literal with NO flags, followed by an identifier that happens to spell `g` —
        // and emphatically not the `g` flag written the long way.
        assert_eq!(parts(r"/a/\u0067"), ("a".to_string(), String::new()));
        assert_eq!(
            regexp_kinds(r"/a/\u0067"),
            [TokenKind::RegExp, ESCAPED, TokenKind::Eof]
        );
    }

    #[test]
    fn a_literal_may_not_span_a_line_and_says_so_where_it_ran_out() {
        // Every alternative bottoms out in `RegularExpressionNonTerminator`, which excludes all
        // four line terminators — inside a class and after a backslash included.
        for source in [
            "/abc",        // no closing slash at all
            "/abc\ndef/",  // a newline before one
            "/abc\rdef/",  //
            "/a\u{2028}/", // <LS> is a terminator here, unlike inside a string literal
            "/a\u{2029}/",
            "/[abc",     // an unterminated class
            "/[abc\n]/", // …and one broken by a newline
            r"/a\",      // a backslash with nothing behind it
            "/a\\\n/",   // …and one with only a newline behind it
        ] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::UnterminatedRegExp,
                "on {source:?}"
            );
        }
        // The span runs from the opening slash to where the search gave up, so the caret covers
        // the literal rather than the whole rest of the file.
        assert_eq!(error("/abc\ndef/").span, Span::new(0, 4));
        assert_eq!(error("/abc").span, Span::new(0, 4));
    }

    #[test]
    fn regexp_parts_answers_rather_than_panicking_on_a_span_it_was_not_given() {
        assert_eq!(regexp_parts("/a/", Span::new(0, 99)), None);
        assert_eq!(regexp_parts("/\u{e9}/", Span::new(0, 2)), None);
        assert_eq!(regexp_parts("abc", Span::new(0, 3)), None);
        assert_eq!(regexp_parts("", Span::empty_at(0)), None);
        // Starts like a literal but never closes.
        assert_eq!(regexp_parts("/abc", Span::new(0, 4)), None);
        // Contains a slash but does not *open* with one. A split that only hunted for the
        // closing delimiter would report these as literals with an empty body, which is a thing
        // §12.9.5 says cannot exist.
        assert_eq!(regexp_parts("a/", Span::new(0, 2)), None);
        assert_eq!(regexp_parts("ab/cd", Span::new(0, 5)), None);
        assert_eq!(regexp_parts("=/", Span::new(0, 2)), None);
        // A valid span that does not begin at zero keeps its offsets absolute.
        let source = "x = /ab/g";
        let parts = regexp_parts(source, Span::new(4, 9)).expect("a literal"); // the assertion under test needs the parts
        assert_eq!(parts.body, Span::new(5, 7));
        assert_eq!(parts.flags, Span::new(8, 9));
        assert_eq!(parts.body.slice(source), Some("ab"));
        // An empty flag list is an empty span sitting just past the closing slash, not a missing
        // one — so a caller can point at "here is where flags would go".
        let parts = regexp_parts("/ab/", Span::new(0, 4)).expect("a literal"); // same
        assert_eq!(parts.flags, Span::new(4, 4));
        assert!(parts.flags.is_empty());
    }

    #[test]
    fn no_regular_expression_literal_however_odd_can_panic() {
        // DR-0002. Backslashes and brackets against the end of input are what a fuzzer finds
        // first here, and nesting is what a careless class tracker gets wrong.
        let cases = [
            "/".to_string(),
            "/[".to_string(),
            r"/\".to_string(),
            r"/[\".to_string(),
            "/]".to_string(),
            "/[]".to_string(),
            "/[]/".to_string(),
            format!("/{}/", "[".repeat(2000)),
            format!("/{}/", r"\\".repeat(2000)),
            format!("/{}/", "a".repeat(5000)),
            format!("/a/{}", "g".repeat(5000)),
        ];
        for source in &cases {
            // The verdict does not matter; not unwinding does. Where it lexes, the parts must
            // still be recoverable.
            if let Ok(tokens) = Lexer::new(source).tokens(Goal::RegExp) {
                assert!(
                    regexp_parts(source, tokens[0].span).is_some(),
                    "{:?} lexed but has no parts",
                    &source[..source.len().min(12)]
                );
            }
        }
        // `/[]/` is an empty class, not an empty literal — the body is `[]`.
        assert_eq!(parts("/[]/"), ("[]".to_string(), String::new()));
    }
}
