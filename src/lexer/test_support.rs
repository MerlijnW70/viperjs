//! Helpers shared by the lexer's test modules.
//!
//! Only what more than one child module needs — the round-trip oracle stays beside the scanning
//! it checks, in `mod.rs`.

use super::{Goal, Lexer, Token, TokenKind, numeric_value};

/// The kinds of a source that lexes cleanly, EOF included.
pub(super) fn kinds(source: &str) -> Vec<TokenKind> {
    Lexer::new(source)
        .tokens(Goal::Div)
        .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // a test asserting clean lexing has nothing to say if lexing failed
        .iter()
        .map(|t| t.kind)
        .collect()
}

/// The single non-EOF token of a source, for tests about one token's flags.
pub(super) fn first(source: &str) -> Token {
    let mut lexer = Lexer::new(source);
    lexer
        .next_token(Goal::Div)
        .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // same
}

/// `Identifier { contains_escape: false }`, the overwhelmingly common case.
pub(super) const PLAIN: TokenKind = TokenKind::Identifier {
    contains_escape: false,
};

/// `Identifier { contains_escape: true }`.
pub(super) const ESCAPED: TokenKind = TokenKind::Identifier {
    contains_escape: true,
};

/// `Number { legacy: false }`, every literal the main grammar produces.
pub(super) const NUMBER: TokenKind = TokenKind::Number { legacy: false };

/// `Number { legacy: true }`, Annex B.1.1's two forms — a Syntax Error in strict code.
pub(super) const LEGACY: TokenKind = TokenKind::Number { legacy: true };

/// `String { legacy_escape: false }`, every literal that uses no Annex B escape.
pub(super) const STRING: TokenKind = TokenKind::String {
    legacy_escape: false,
};

/// `String { legacy_escape: true }` — a Syntax Error in strict code (§12.9.4.1).
pub(super) const LEGACY_STRING: TokenKind = TokenKind::String {
    legacy_escape: true,
};

/// The numeric value of the one literal in `source`.
pub(super) fn value(source: &str) -> f64 {
    let token = first(source);
    numeric_value(source, token.span)
        .unwrap_or_else(|| panic!("{source:?} should have a numeric value")) // a test about the value cannot proceed without one
}
