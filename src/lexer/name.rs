//! Identifiers: `IdentifierName`, its `\u` escapes, and the keyword decision (§12.7).
//!
//! The character classes live in [`crate::unicode_id`]; what is here is the scanning that walks
//! them, the escape forms of §12.9.4, and the two early errors of §12.7.1.1 that stop an escape
//! from smuggling in a code point nobody could have written directly.

use super::{LexError, LexErrorKind, Lexer, ReservedWord, TokenKind};
use crate::span::Span;
use crate::unicode_id::{is_id_continue, is_id_start};
use std::borrow::Cow;

/// The code points an identifier names, with every `\u` escape resolved (§12.7.1.2
/// `IdentifierCodePoints`).
///
/// Borrows when the spelling contained no escape, which is nearly always — `Cow` here is not
/// premature optimization but the difference between allocating for every name in a program and
/// allocating for the handful that are written oddly. For a [`TokenKind::PrivateIdentifier`] the
/// leading `#` is part of the value, matching the spec's `StringValue`.
///
/// Returns `None` if `span` does not land on character boundaries of `source`, or covers a
/// malformed escape — a caller passing a span the lexer did not hand it gets an answer, not a
/// panic.
///
/// It resolves escapes; it does **not** re-run §12.7.1.1. Over a span the lexer produced there
/// is nothing to re-check, and over any other span the useful answer is "here is what those
/// escapes denote" rather than a second opinion on validity: `a\u{20}` reads back as `a `, one
/// space and all, because that is what is written there. The lexer is where a name is judged.
///
/// ```
/// use praxis::lexer::{Goal, Lexer, TokenKind, identifier_value};
///
/// // A raw string, so the source really does contain a backslash: this spells `abc` the
/// // long way round, and the value comes back as if it had been spelled plainly.
/// let source = r"\u0061bc";
/// let token = Lexer::new(source).next_token(Goal::Div).expect("this lexes");
/// assert_eq!(token.kind, TokenKind::Identifier { contains_escape: true });
/// assert_eq!(identifier_value(source, token.span).as_deref(), Some("abc"));
/// ```
pub fn identifier_value<'a>(source: &'a str, span: Span) -> Option<Cow<'a, str>> {
    let text = span.slice(source)?;
    if !text.contains('\\') {
        return Some(Cow::Borrowed(text));
    }
    // Re-read the spelling with the same escape decoder the scan used, so the value can never
    // disagree with what was validated. Only the escapes need interpreting; every other byte is
    // already the code point it contributes.
    let mut lexer = Lexer::new(text);
    let mut value = String::with_capacity(text.len());
    while !lexer.cursor.is_eof() {
        if lexer.cursor.starts_with("\\") {
            match lexer.read_unicode_escape() {
                Ok(code_point) => value.push(char::from_u32(code_point)?),
                Err(_) => return None,
            }
        } else {
            value.push(lexer.cursor.bump()?);
        }
    }
    Some(Cow::Owned(value))
}

// `pub(super)` on exactly the two entry points `next_token` dispatches to, and no further:
// the escape machinery is reachable only through them, which is what keeps the early errors
// of §12.7.1.1 from being bypassable by some later caller in the parent module.
impl<'a> Lexer<'a> {
    /// Decide whether a just-scanned `IdentifierName` is a keyword.
    ///
    /// §12.7.2 Note 1: keywords match a literal sequence of source characters, so a spelling
    /// that used an escape is an `IdentifierName` and never a keyword — `els\u{65}` does not
    /// declare an `else`. It is not thereby a usable binding either, but that is §13.1.1's early
    /// error and needs the grammatical context only the parser has.
    pub(super) fn classify_name(&self, span: Span, contains_escape: bool) -> TokenKind {
        if contains_escape {
            return TokenKind::Identifier {
                contains_escape: true,
            };
        }
        match span
            .slice(self.cursor.source)
            .and_then(ReservedWord::from_text)
        {
            Some(word) => TokenKind::Keyword(word),
            None => TokenKind::Identifier {
                contains_escape: false,
            },
        }
    }

    /// Scan an `IdentifierName` from its first character, reporting whether any `\u` escape
    /// contributed a code point.
    ///
    /// The first character is checked here rather than trusted, because one caller — the `#` of
    /// a private name — has not looked at it yet. `#5` and a bare `#` must both be errors, not
    /// an empty name.
    pub(super) fn scan_identifier(&mut self) -> Result<bool, LexError> {
        let at = self.cursor.offset();
        let mut contains_escape = match self.cursor.peek() {
            Some('\\') => self.scan_escaped_identifier_char(true)?,
            Some(ch) if is_id_start(ch as u32) => {
                let _ = self.cursor.bump();
                false
            }
            _ => {
                let _ = self.cursor.bump();
                return Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span: Span::new(at, self.cursor.offset()),
                });
            }
        };
        loop {
            match self.cursor.peek() {
                // A `\` inside a name can only be an escape: `IdentifierPart` has no other
                // alternative that starts with one, so `a\x` is an error rather than the name
                // `a` followed by something else.
                Some('\\') => contains_escape |= self.scan_escaped_identifier_char(false)?,
                Some(ch) if is_id_continue(ch as u32) => {
                    let _ = self.cursor.bump();
                }
                _ => return Ok(contains_escape),
            }
        }
    }

    /// Read one `\ UnicodeEscapeSequence` and check the code point may appear where it was
    /// written. Always returns `true` — the value exists so the caller can write `|=`.
    ///
    /// §12.7.1.1: it is a Syntax Error if the escape's code point is not matched by
    /// `IdentifierStartChar` (at the start) or `IdentifierPartChar` (later). The rule is what
    /// stops `\u{20}` from smuggling a space into a name, and it is why these predicates take a
    /// `u32`: `\uD800` names a lone surrogate, which is a legal thing to *write* and an illegal
    /// thing to mean.
    fn scan_escaped_identifier_char(&mut self, is_start: bool) -> Result<bool, LexError> {
        let at = self.cursor.offset();
        let code_point = self.read_unicode_escape()?;
        let allowed = if is_start {
            is_id_start(code_point)
        } else {
            is_id_continue(code_point)
        };
        if !allowed {
            return Err(LexError {
                kind: LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                span: Span::new(at, self.cursor.offset()),
            });
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Goal;
    use crate::lexer::test_support::*;
    /// The cooked value of the first token of `source`.
    fn name_of(source: &str) -> String {
        let token = first(source);
        identifier_value(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should have an identifier value")) // a test about the value cannot proceed without one
            .into_owned()
    }

    #[test]
    fn a_name_runs_to_the_first_character_that_cannot_continue_it() {
        // `IdentifierName :: IdentifierStart | IdentifierName IdentifierPart` (§12.7). The
        // greediness is the point: a name that stopped early would silently split `abc` into
        // three bindings, and one that ran too far would swallow the `;`.
        assert_eq!(kinds("a"), [PLAIN, TokenKind::Eof]);
        assert_eq!(kinds("abc"), [PLAIN, TokenKind::Eof]);
        assert_eq!(first("abc;").span, Span::new(0, 3));
        assert_eq!(kinds("a b"), [PLAIN, PLAIN, TokenKind::Eof]);
        assert_eq!(
            kinds("a-b"),
            [PLAIN, TokenKind::Minus, PLAIN, TokenKind::Eof]
        );
        // The two ECMAScript additions start names; digits continue them but cannot start one.
        assert_eq!(kinds("_"), [PLAIN, TokenKind::Eof]);
        assert_eq!(kinds("$"), [PLAIN, TokenKind::Eof]);
        assert_eq!(kinds("$_0"), [PLAIN, TokenKind::Eof]);
        assert_eq!(first("a0b$_;").span, Span::new(0, 5));
        // A digit cannot start one: `1` is a numeric literal, and `1abc` is that literal
        // followed by a name — which §12.9.3 then rejects — rather than one strange identifier.
        assert_eq!(
            Lexer::new("1").next_token(Goal::Div).map(|t| t.kind),
            Ok(TokenKind::Number { legacy: false })
        );
        assert_eq!(
            Lexer::new("1abc").tokens(Goal::Div).map(|t| t.len()),
            Err(LexError {
                kind: LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
                span: Span::new(1, 2),
            })
        );
        // A name may sit against a punctuator with no space at all.
        assert_eq!(
            kinds("a=>b"),
            [PLAIN, TokenKind::Arrow, PLAIN, TokenKind::Eof]
        );
    }

    #[test]
    fn names_use_the_unicode_id_sets_and_not_an_ascii_approximation_of_them() {
        // Each of these is a valid JavaScript variable name in every shipping engine, and none
        // of them survives an `is_ascii_alphabetic` implementation.
        for source in [
            "caf\u{e9}",
            "\u{3a9}mega",
            "\u{5d0}",
            "\u{3042}",
            "\u{4e00}",
        ] {
            assert_eq!(kinds(source), [PLAIN, TokenKind::Eof], "lexing {source:?}");
            assert_eq!(first(source).span, Span::new(0, source.len() as u32));
        }
        // Astral: `char`-at-a-time scanning must advance four bytes, not one.
        assert_eq!(kinds("x\u{1d49c}"), [PLAIN, TokenKind::Eof]);
        assert_eq!(first("x\u{1d49c}").span, Span::new(0, 5));
        // Other_ID_Start — in ID_Start only because Unicode grandfathered it (§12.7 Note 3).
        assert_eq!(kinds("\u{2118}"), [PLAIN, TokenKind::Eof]);
        // Other_ID_Continue: MIDDLE DOT continues a name, so this is ONE identifier, not three
        // tokens. An engine that reached for `is_alphanumeric` breaks exactly here.
        assert_eq!(kinds("x\u{b7}y"), [PLAIN, TokenKind::Eof]);
        // ZERO WIDTH NON-JOINER is ID_Continue in Unicode 17, which is why §12.7 no longer
        // lists it separately — the table answers for it.
        assert_eq!(kinds("x\u{200c}y"), [PLAIN, TokenKind::Eof]);
        // …but the neighbouring ZERO WIDTH SPACE is not a name character or white space, and
        // must stay an error rather than being invisibly absorbed.
        assert!(Lexer::new("x\u{200b}y").tokens(Goal::Div).is_err());
        // Symbols that look like they might qualify and do not.
        for source in ["\u{20ac}", "\u{1f680}", "\u{00a7}"] {
            assert!(Lexer::new(source).tokens(Goal::Div).is_err(), "{source:?}");
        }
    }

    #[test]
    fn a_reserved_word_spelled_with_an_escape_is_a_name_and_not_a_keyword() {
        // §12.7.2 Note 1: "A code point in a keyword cannot be expressed by a \ Unicode-
        // EscapeSequence." So this is an IdentifierName whose value happens to be "else" —
        // which §13.1.1 then refuses as a binding, but that is the parser's rule and needs the
        // `contains_escape` flag this token carries.
        assert_eq!(kinds("els\\u{65}"), [ESCAPED, TokenKind::Eof]);
        assert_eq!(name_of("els\\u{65}"), "else");
        // …and the same for an escape at the very start.
        assert_eq!(kinds("\\u0069f"), [ESCAPED, TokenKind::Eof]);
        assert_eq!(name_of("\\u0069f"), "if");
        // Without the escape, both are keywords. The flag is the only difference, and it is
        // the whole difference.
        assert_eq!(
            kinds("else"),
            [TokenKind::Keyword(ReservedWord::Else), TokenKind::Eof]
        );
        assert_eq!(
            kinds("if"),
            [TokenKind::Keyword(ReservedWord::If), TokenKind::Eof]
        );
    }

    #[test]
    fn a_unicode_escape_contributes_a_code_point_to_the_name() {
        // §12.7.1.2 IdentifierCodePoints: the `\` contributes nothing and the escape
        // contributes exactly one code point, so an escaped spelling and a plain one name the
        // same thing (§12.7.1: "All interpretations… are based upon their actual code points").
        assert_eq!(name_of("\\u0061bc"), "abc");
        assert_eq!(name_of("a\\u{62}c"), "abc");
        assert_eq!(name_of("\\u{61}\\u{62}"), "ab");
        assert_eq!(kinds("\\u0061bc"), [ESCAPED, TokenKind::Eof]);
        // Both forms of the sequence, and both hex cases.
        assert_eq!(name_of("\\u00E9"), "\u{e9}");
        assert_eq!(name_of("\\u00e9"), "\u{e9}");
        assert_eq!(name_of("\\u{E9}"), "\u{e9}");
        // `CodePoint :: HexDigits[~Sep]` — any number of digits, so leading zeros are fine and
        // there is no four-digit limit inside the braces.
        assert_eq!(name_of("\\u{000000000000061}"), "a");
        // An astral escape is one code point, not a surrogate pair.
        assert_eq!(name_of("\\u{1D49C}"), "\u{1d49c}");
        // The span covers the spelling as written — escapes are longer than what they denote.
        assert_eq!(first("\\u{61}").span, Span::new(0, 6));
        // A name that merely contains a `u` is not an escape.
        assert_eq!(name_of("u0061"), "u0061");
    }

    #[test]
    fn an_escape_must_denote_a_character_that_could_have_been_written_directly() {
        // §12.7.1.1: it is a Syntax Error if the escape's code point is not matched by
        // IdentifierStartChar (first) or IdentifierPartChar (later). Put plainly: replacing the
        // escape with what it denotes must leave a valid name. Without this rule `\u{20}` is a
        // space inside an identifier and every downstream assumption breaks.
        let not_a_name_char = |source: &str| {
            assert_eq!(
                Lexer::new(source).tokens(Goal::Div).map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                    span: Span::new(0, source.len() as u32),
                }),
                "on {source:?}"
            );
        };
        not_a_name_char("\\u0020"); // a space
        not_a_name_char("\\u{2e}"); // a full stop
        not_a_name_char("\\uD800"); // a lone surrogate — well-formed, and not a character
        not_a_name_char("\\u{10FFFF}"); // in range, still not an identifier character

        // A digit cannot START a name even when escaped, but it can continue one — the two
        // halves of the early error use different predicates, and this is the pair that proves
        // the `is_start` flag is actually consulted.
        assert_eq!(
            Lexer::new("\\u0030").tokens(Goal::Div),
            Err(LexError {
                kind: LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                span: Span::new(0, 6),
            })
        );
        assert_eq!(name_of("a\\u0030"), "a0");
        // …and `_` the other way round: legal in both positions, but only because §12.7 adds it
        // explicitly at the start.
        assert_eq!(name_of("\\u005F"), "_");
        assert_eq!(name_of("a\\u005F"), "a_");
        // A bad escape in the middle reports where the escape is, not where the name began.
        assert_eq!(
            Lexer::new("ab\\u0020").tokens(Goal::Div),
            Err(LexError {
                kind: LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                span: Span::new(2, 8),
            })
        );
    }

    #[test]
    fn a_malformed_escape_is_an_error_rather_than_a_shorter_name() {
        // `IdentifierPart :: \ UnicodeEscapeSequence` is the only alternative beginning with a
        // backslash, so a `\` that is not a well-formed escape cannot fall back to "the name
        // ended here" — `a\x` is a syntax error, not the name `a`.
        // The span runs from the backslash to exactly where the sequence stopped being a
        // possible escape — so `\x` reports two characters' worth of nothing, while `\u12g4`
        // reports the four it did manage to read. That rule is uniform, and the expected ends
        // below are what pin it.
        for (source, start, end) in [
            ("\\", 0, 1),       // nothing at all after it
            ("\\x", 0, 1),      // not a `u`; the `x` is not part of any escape
            ("\\u", 0, 2),      // no digits
            ("\\u12", 0, 4),    // fewer than four
            ("\\u123", 0, 5),   // still fewer than four
            ("\\u12g4", 0, 4),  // a non-hex digit among the four
            ("\\u{", 0, 3),     // an unclosed brace
            ("\\u{}", 0, 3),    // no digits at all
            ("\\u{61", 0, 5),   // digits, but never closed
            ("\\u{zz}", 0, 3),  // no hex digits inside
            ("\\u{61 }", 0, 5), // `HexDigits[~Sep]` admits no spaces…
            ("\\u{6_1}", 0, 4), // …and no numeric separators either
            ("a\\x", 1, 2),     // and all the same, mid-name
            ("a\\u{}", 1, 4),   //
        ] {
            assert_eq!(
                Lexer::new(source).tokens(Goal::Div).map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::InvalidUnicodeEscape,
                    span: Span::new(start, end),
                }),
                "on {source:?}"
            );
        }

        // Exactly four, no more and no fewer: a fifth hex digit is simply the next character of
        // the name. Read five and `a0` becomes the single character U+610.
        assert_eq!(name_of("\\u00610"), "a0");
        assert_eq!(name_of("\\u0061a"), "aa");
    }

    #[test]
    fn an_escape_beyond_the_last_code_point_is_out_of_range_rather_than_malformed() {
        // `CodePoint :: HexDigits but only if the MV of HexDigits ≤ 0x10FFFF`; anything larger
        // is `NotCodePoint`. Distinguishing this from a malformed escape matters to whoever
        // reads the message — one is a typo, the other is a misunderstanding.
        for source in [
            "\\u{110000}",         // one past the last code point
            "\\u{FFFFFFFF}",       // fills a u32 exactly
            "\\u{FFFFFFFFFFFFFF}", // and one that would overflow it — saturation, not a panic
        ] {
            assert_eq!(
                Lexer::new(source).tokens(Goal::Div).map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::CodePointOutOfRange,
                    span: Span::new(0, source.len() as u32),
                }),
                "on {source:?}"
            );
        }
        // The boundary itself is in range — it is rejected later, and for a different reason.
        assert_eq!(
            Lexer::new("\\u{10FFFF}").tokens(Goal::Div),
            Err(LexError {
                kind: LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                span: Span::new(0, 10),
            })
        );
    }

    #[test]
    fn a_private_name_keeps_its_hash_in_both_the_span_and_the_value() {
        // `PrivateIdentifier :: # IdentifierName` (§12.7). The spec's StringValue is the number
        // sign concatenated with the name's, so the `#` is part of what the token means — two
        // different classes may each have a `#x`, and the `#` is what says so.
        let private = TokenKind::PrivateIdentifier {
            contains_escape: false,
        };
        assert_eq!(kinds("#x"), [private, TokenKind::Eof]);
        assert_eq!(first("#x").span, Span::new(0, 2));
        assert_eq!(name_of("#x"), "#x");
        assert_eq!(name_of("#count"), "#count");
        assert_eq!(
            kinds("this.#x"),
            [
                TokenKind::Keyword(ReservedWord::This),
                TokenKind::Dot,
                private,
                TokenKind::Eof
            ]
        );
        // Escapes work in a private name too, and the flag travels with it.
        assert_eq!(
            kinds("#\\u0078"),
            [
                TokenKind::PrivateIdentifier {
                    contains_escape: true
                },
                TokenKind::Eof
            ]
        );
        assert_eq!(name_of("#\\u0078"), "#x");
        // `#` is not a punctuator and there is no empty private name: what follows must start
        // one. The error points where the name was expected, just past the `#`.
        // (`#!` is absent deliberately: at byte 0 those two characters are a hashbang comment,
        // which is its own test.)
        for (source, span) in [
            ("#", Span::new(1, 1)),
            ("#5", Span::new(1, 2)),
            ("# x", Span::new(1, 2)),
            ("#\u{200b}", Span::new(1, 4)),
        ] {
            assert_eq!(
                Lexer::new(source).tokens(Goal::Div).map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span,
                }),
                "on {source:?}"
            );
        }
    }

    #[test]
    fn identifier_value_borrows_the_common_case_and_owns_only_what_it_must() {
        // Nearly every name in a program is written plainly; allocating a String for each would
        // be a cost paid on every identifier to serve the handful that are spelled oddly.
        let source = "plain \\u0061bc";
        let plain = first(source);
        assert!(matches!(
            identifier_value(source, plain.span),
            Some(Cow::Borrowed("plain"))
        ));
        let escaped = Lexer::new(source)
            .tokens(Goal::Div)
            .expect("this lexes") // the assertion under test needs the tokens
            [1];
        assert!(matches!(
            identifier_value(source, escaped.span),
            Some(Cow::Owned(ref value)) if value == "abc"
        ));
        // A span the lexer never produced gets an answer, not a panic: off the end, off a
        // character boundary, and over a malformed escape.
        assert_eq!(identifier_value("abc", Span::new(0, 99)), None);
        assert_eq!(identifier_value("\u{e9}", Span::new(0, 1)), None);
        assert_eq!(identifier_value("a\\u", Span::new(0, 3)), None);
        // A *well-formed* escape denoting something that could never appear in a name reads
        // back as what it says. Validity was settled when the lexer refused to produce this
        // token; asking again here would be a second, weaker opinion.
        assert_eq!(
            identifier_value("a\\u{20}", Span::new(0, 7)).as_deref(),
            Some("a ")
        );
        assert!(
            Lexer::new("a\\u{20}").tokens(Goal::Div).is_err(),
            "…and the lexer does refuse it"
        );
        // An empty span is an empty value rather than a failure — `Span::empty_at` is what EOF
        // carries, and asking it for a name should not be exciting.
        assert_eq!(
            identifier_value("abc", Span::empty_at(1)).as_deref(),
            Some("")
        );
    }
}
