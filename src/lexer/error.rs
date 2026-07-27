//! Why lexing stopped, and where.
//!
//! Errors are values with spans (`AGENTS.md`), so this is a plain data module: no recovery
//! policy, and no formatting decision beyond a message a host can render.

use crate::span::Span;
use std::fmt;

/// Why lexing stopped, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError {
    /// What went wrong.
    pub kind: LexErrorKind,
    /// The offending source text. For an unterminated comment this reaches to the end of the
    /// source, because that is genuinely how much of the file the comment swallowed.
    pub span: Span,
}

/// Every failure the lexer can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    /// A `/*` with no matching `*/` before the end of the source (§12.4 — comments do not nest,
    /// and there is no "unterminated at EOF is fine" allowance).
    UnterminatedComment,
    /// A code point that begins no token form. Note that while this slice is incomplete, this
    /// also covers the literals it has not learned yet — see the module documentation.
    UnexpectedCharacter,
    /// A `\` that is not followed by a well-formed `UnicodeEscapeSequence` (§12.9.4): not a `u`,
    /// fewer than four hex digits, or a `\u{…}` with no digits or no closing brace.
    InvalidUnicodeEscape,
    /// `\u{…}` whose value exceeds U+10FFFF — the spec's `NotCodePoint`.
    CodePointOutOfRange,
    /// An escape that is well-formed but contributes a code point which cannot appear where it
    /// was written (§12.7.1.1): `\u{20}` is a space, and a space is no part of a name.
    EscapedCodePointIsNotAnIdentifierCharacter,
    /// A `NumericLiteralSeparator` that is not between two digits (§12.9.3): `1__0`, `1_`, `0x_1`.
    MisplacedNumericSeparator,
    /// `0b`, `0o` or `0x` with no digit after it.
    ///
    /// The grammar arrives at the same verdict by another route — `0x` is not a
    /// `HexIntegerLiteral`, so it is the literal `0` followed by the identifier `x` — but both
    /// are a Syntax Error and nothing a script can observe tells them apart, so the lexer
    /// reports the one that says what to fix.
    MissingDigitsAfterRadixPrefix,
    /// A `StringLiteral` whose closing quote never arrived — because the line ended first
    /// (§12.9.4 forbids a literal `<LF>` or `<CR>` inside one) or because the source did.
    UnterminatedStringLiteral,
    /// `\x` not followed by exactly two hex digits (`HexEscapeSequence`, §12.9.4).
    InvalidHexEscape,
    /// §12.9.3: "The SourceCharacter immediately following a NumericLiteral must not be an
    /// IdentifierStart or DecimalDigit." The spec's own example is that `3in` is an error, and
    /// not the two input elements `3` and `in`.
    NumericLiteralFollowedByIdentifierOrDigit,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnterminatedComment => "unterminated block comment",
            Self::UnexpectedCharacter => "unexpected character",
            Self::InvalidUnicodeEscape => "malformed unicode escape sequence",
            Self::CodePointOutOfRange => "escaped value is not a unicode code point",
            Self::EscapedCodePointIsNotAnIdentifierCharacter => {
                "escaped code point is not valid in an identifier"
            }
            Self::UnterminatedStringLiteral => "unterminated string literal",
            Self::InvalidHexEscape => "malformed hexadecimal escape sequence",
            Self::MisplacedNumericSeparator => "numeric separator must sit between two digits",
            Self::MissingDigitsAfterRadixPrefix => "missing digits after the radix prefix",
            Self::NumericLiteralFollowedByIdentifierOrDigit => {
                "numeric literal is immediately followed by an identifier or a digit"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_kind_describes_itself_and_no_two_alike() {
        // An error a host cannot render is not an error value. Distinctness matters more than
        // the exact wording: two failures that print identically are one failure to whoever
        // reads them — and the three escape errors are precisely the ones worth telling apart,
        // being a typo, a misunderstanding about code points, and a rule about where a
        // character may appear.
        let kinds = [
            (LexErrorKind::UnterminatedComment, "comment"),
            (LexErrorKind::UnexpectedCharacter, "unexpected"),
            (LexErrorKind::InvalidUnicodeEscape, "escape"),
            (LexErrorKind::CodePointOutOfRange, "code point"),
            (
                LexErrorKind::EscapedCodePointIsNotAnIdentifierCharacter,
                "escaped code point",
            ),
            (
                LexErrorKind::UnterminatedStringLiteral,
                "unterminated string",
            ),
            (LexErrorKind::InvalidHexEscape, "hexadecimal"),
            (LexErrorKind::MisplacedNumericSeparator, "separator"),
            (LexErrorKind::MissingDigitsAfterRadixPrefix, "radix"),
            (
                LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
                "followed by",
            ),
        ];
        let mut messages: Vec<String> = Vec::new();
        for (kind, expected) in kinds {
            let message = kind.to_string();
            assert!(
                message.contains(expected),
                "{kind:?} renders as {message:?}, which never mentions {expected:?}"
            );
            assert!(!messages.contains(&message), "{message:?} is used twice");
            messages.push(message);
        }
        assert_eq!(
            messages.len(),
            10,
            "one message for each kind, and no kind missed"
        );
    }

    #[test]
    fn a_lex_error_carries_the_span_it_was_given() {
        // The span is the half of the error that a caret in a terminal is drawn from; an error
        // type that dropped it would be a message, not a diagnostic.
        let error = LexError {
            kind: LexErrorKind::CodePointOutOfRange,
            span: Span::new(3, 13),
        };
        assert_eq!(error.span.len(), 10);
        assert_eq!(error.span.slice("ab \\u{110000}"), Some("\\u{110000}"));
    }
}
