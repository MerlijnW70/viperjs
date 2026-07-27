//! Why parsing stopped, and where.
//!
//! A plain data module, like the lexer's: the parser decides what went wrong, and this decides
//! only how to say it. Messages are built without the source, so a host that has kept nothing
//! but the error can still render something a person can act on.

use crate::lexer::{LexError, LexErrorKind, TokenKind};
use crate::span::Span;
use std::fmt;

/// Why parsing stopped, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// What went wrong.
    pub kind: ParseErrorKind,
    /// The source it went wrong at. For an unexpected token this is that token, not the
    /// construct it interrupted — a caret under the surprise beats one under its context.
    pub span: Span,
}

/// Every failure the parser can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The lexer could not produce a token at all.
    Lexical(LexErrorKind),
    /// A token appeared where the grammar does not allow it.
    Unexpected {
        /// What the grammar wanted, phrased for a reader: `` "`)`" ``, `"an expression"`.
        expected: &'static str,
        /// What was actually there, so a message can be built without re-reading the source.
        found: TokenKind,
    },
    /// Nesting exceeded [`MAX_NESTING_DEPTH`].
    TooDeeplyNested,
    /// §13.6: `ExponentiationExpression : UpdateExpression ** ExponentiationExpression`.
    ///
    /// The left operand is an `UpdateExpression`, which a prefix unary is not — so `-a ** b` has
    /// no derivation and `(-a) ** b` does. The rule exists because the alternative reading is
    /// genuinely ambiguous to a reader: `-a ** b` could plausibly mean either `(-a) ** b` or
    /// `-(a ** b)`, and those differ.
    ExponentiationOnUnary,
    /// §13.15.1: the left of an assignment must be something that can be assigned to.
    ///
    /// The specification calls the test `AssignmentTargetType`, and for everything this parser
    /// can build so far the answer is "an identifier, however many parentheses are around it".
    /// `1 = 2` and `(a, b) = 3` are the shapes this rejects.
    InvalidAssignmentTarget,
    /// §13.13: `??` may not be mixed with `&&` or `||` without parentheses.
    ///
    /// `CoalesceExpressionHead` admits a `CoalesceExpression` or a `BitwiseORExpression` and
    /// nothing else, and `ShortCircuitExpression` keeps the two families apart in the other
    /// direction — so `a || b ?? c` and `a ?? b || c` are both errors, for the same reason as
    /// above: no reader would agree on what they meant.
    MixedCoalesceAndLogical,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexical(kind) => write!(f, "{kind}"),
            Self::Unexpected { expected, found } => {
                write!(f, "expected {expected}, found ")?;
                // A token with one spelling is quoted; one whose text varies is named by its
                // category, because "found `x`" is no help when the complaint is that an
                // identifier cannot stand there at all.
                match found {
                    TokenKind::Eof => f.write_str("end of input"),
                    TokenKind::Identifier { .. } => f.write_str("an identifier"),
                    TokenKind::PrivateIdentifier { .. } => f.write_str("a private name"),
                    TokenKind::Number { .. } => f.write_str("a number"),
                    TokenKind::BigInt => f.write_str("a bigint literal"),
                    TokenKind::String { .. } => f.write_str("a string"),
                    TokenKind::RegExp => f.write_str("a regular expression"),
                    TokenKind::Template { .. } => f.write_str("a template"),
                    // Everything left is a punctuator or a keyword, and every one of those has
                    // exactly one spelling — `as_str` cannot be `None` here, and asking for a
                    // default rather than testing for it keeps a branch out of the message path.
                    fixed => write!(f, "`{}`", fixed.as_str().unwrap_or_default()),
                }
            }
            Self::TooDeeplyNested => write!(f, "expression nests too deeply"),
            Self::InvalidAssignmentTarget => {
                write!(f, "this expression cannot be assigned to")
            }
            Self::ExponentiationOnUnary => write!(
                f,
                "the left operand of `**` may not be an unparenthesized unary expression"
            ),
            Self::MixedCoalesceAndLogical => write!(
                f,
                "`??` may not be mixed with `&&` or `||` without parentheses"
            ),
        }
    }
}

impl From<LexError> for ParseError {
    fn from(error: LexError) -> Self {
        Self {
            kind: ParseErrorKind::Lexical(error.kind),
            span: error.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::test_support::*;
    #[test]
    fn every_parse_error_says_what_it_wanted_and_what_it_found() {
        // "Errors carry spans and read like a good compiler's" (AGENTS.md). The message is built
        // without the source, so a host that has only the error can still render something a
        // person can act on.
        assert_eq!(
            error("(1").kind.to_string(),
            "expected `)`, found end of input"
        );
        assert_eq!(
            error("(1 2)").kind.to_string(),
            "expected `)`, found a number"
        );
        assert_eq!(
            error("var").kind.to_string(),
            "expected an expression, found `var`"
        );
        assert_eq!(
            error("1 2").kind.to_string(),
            "expected end of input, found a number"
        );
        assert_eq!(
            error("1 x").kind.to_string(),
            "expected end of input, found an identifier"
        );
        assert_eq!(
            error("1 )").kind.to_string(),
            "expected end of input, found `)`"
        );
        assert_eq!(
            error("1 'a'").kind.to_string(),
            "expected end of input, found a string"
        );
        assert_eq!(
            error("1 `a`").kind.to_string(),
            "expected end of input, found a template"
        );
        assert_eq!(
            error("1 #a").kind.to_string(),
            "expected end of input, found a private name"
        );
        assert_eq!(
            error("1 2n").kind.to_string(),
            "expected end of input, found a bigint literal"
        );
        assert_eq!(
            error("1 ]").kind.to_string(),
            "expected end of input, found `]`"
        );
        // A regular expression can only stand where an operand may, and an operand may stand
        // wherever this grammar reaches — so there is no source that puts one somewhere
        // unexpected, and the message for it is checked by building the error directly.
        assert_eq!(
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::RegExp,
            }
            .to_string(),
            "expected `)`, found a regular expression"
        );
        assert_eq!(
            error("'abc").kind.to_string(),
            "unterminated string literal",
            "a lexical failure keeps its own words"
        );
        assert_eq!(
            ParseErrorKind::TooDeeplyNested.to_string(),
            "expression nests too deeply"
        );
    }
}
