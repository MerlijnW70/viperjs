//! Helpers shared by the lexer's test modules.
//!
//! Only what more than one child module needs — the round-trip oracle stays beside the scanning
//! it checks, in `mod.rs`.

use super::{Lexer, Token, TokenKind};

/// The kinds of a source that lexes cleanly, EOF included.
pub(super) fn kinds(source: &str) -> Vec<TokenKind> {
    Lexer::new(source)
        .tokens()
        .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // a test asserting clean lexing has nothing to say if lexing failed
        .iter()
        .map(|t| t.kind)
        .collect()
}

/// The single non-EOF token of a source, for tests about one token's flags.
pub(super) fn first(source: &str) -> Token {
    let mut lexer = Lexer::new(source);
    lexer
        .next_token()
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
