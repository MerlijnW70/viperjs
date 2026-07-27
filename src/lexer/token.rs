//! What a token *is*: the kinds and the punctuator table.
//!
//! Data and its spellings, with no scanning logic — which is why the test here can afford to
//! write every punctuator out a second time by hand. Two independent spellings of the same list
//! catch a drifting table; one spelling proves nothing.

use super::ReservedWord;
use crate::span::Span;

/// One lexical token: what it is, where it is, and whether a line break preceded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// Which token this is.
    pub kind: TokenKind,
    /// The bytes the token itself covers — never the trivia around it.
    pub span: Span,
    /// Whether at least one line terminator was crossed since the previous token.
    ///
    /// Recorded here rather than recomputed later because automatic semicolon insertion
    /// (ECMA-262 §12.10) is defined in terms of it, and by the time the parser asks, the trivia
    /// is gone. A block comment containing a line terminator sets this too — §12.4 says such a
    /// comment *is* a line terminator for the syntactic grammar, and that is exactly the rule
    /// that decides whether `a = b /*\n*/ ++c` is one statement or two.
    ///
    /// True for the first token of a source that begins with a line terminator. Nothing
    /// consults it there, and the alternative is a special case that earns nothing.
    pub newline_before: bool,
}

/// Every token form this slice can produce: the punctuators of ECMA-262 §12.8, plus end of
/// input.
///
/// End of input is a token, not `None`. A parser that has to handle "no more tokens" separately
/// from "wrong token" grows a second error path for every construct; giving EOF a kind and an
/// empty span at the end of the source collapses the two.
///
/// Every variant except [`TokenKind::Eof`] must also appear in the `PUNCTUATORS` table — the
/// tests cross-check the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// End of input. Empty span at the end of the source; repeats forever once reached.
    Eof,

    /// An `IdentifierName` (§12.7) that is not one of the unconditionally reserved words.
    ///
    /// The span covers the spelling as written, escapes included; [`identifier_value`] turns it
    /// into the sequence of code points the spec calls its `IdentifierCodePoints`.
    Identifier {
        /// Whether a `\u` escape contributed a code point to the spelling.
        ///
        /// The parser needs this and cannot recover it later without re-lexing. §12.7.2 Note 1:
        /// a keyword matches a literal sequence of source characters, so `els\u{65}` is an
        /// `IdentifierName` and **not** the keyword `else` — while the early errors of §13.1.1
        /// separately forbid using it as a binding. Both rules need to know an escape was used.
        contains_escape: bool,
    },

    /// `# IdentifierName` (§12.7) — a private class member name.
    ///
    /// The span includes the `#`, as does the value: the spec's `StringValue` of a
    /// `PrivateIdentifier` is the number sign concatenated with the name's.
    PrivateIdentifier {
        /// Whether a `\u` escape contributed a code point to the name.
        contains_escape: bool,
    },

    /// One of the 38 spellings §12.7.2 reserves unconditionally, written without escapes.
    Keyword(ReservedWord),

    /// A `NumericLiteral` (§12.9.3) denoting a Number. [`crate::lexer::numeric_value`] reads it.
    Number {
        /// Whether this is one of Annex B.1.1's two legacy forms — a `LegacyOctalIntegerLiteral`
        /// like `0123`, or a `NonOctalDecimalIntegerLiteral` like `08`.
        ///
        /// §12.9.3.1 makes both a Syntax Error in strict code and legal outside it. The lexer
        /// cannot know which it is reading, so it records the fact and leaves the verdict to the
        /// parser — the same division of labour as `contains_escape` on an identifier.
        legacy: bool,
    },

    /// A `StringLiteral` (§12.9.4). [`crate::lexer::string_value`] reads its code units.
    String {
        /// Whether it used one of Annex B's legacy escapes — `LegacyOctalEscapeSequence` like
        /// `\7`, or `NonOctalDecimalEscapeSequence`, which is `\8` and `\9`.
        ///
        /// §12.9.4.1 makes both a Syntax Error in strict code. Its Note 2 explains why the lexer
        /// cannot settle it: a literal may *precede* the directive that makes its own code
        /// strict, as in `function invalid() { "\7"; "use strict"; }`.
        legacy_escape: bool,
    },

    /// A `NumericLiteral` carrying the `n` of `BigIntLiteralSuffix` (§12.9.3).
    ///
    /// Recognised now although BigInt values arrive at M7, because the alternative is worse than
    /// waiting: without the suffix, `123n` lexes as `123` followed by the name `n`, which is
    /// nonsense that parses.
    BigInt,

    /// `{`
    LBrace,
    /// `}` — the spec's `RightBracePunctuator`, split out because the goal symbol decides
    /// whether it closes a block or resumes a template. That distinction arrives with templates.
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,

    /// `.`
    Dot,
    /// `...`
    DotDotDot,
    /// `;`
    Semicolon,
    /// `,`
    Comma,
    /// `:`
    Colon,
    /// `=>`
    Arrow,
    /// `?.` — only when the next code point is not a decimal digit (§12.8).
    QuestionDot,
    /// `?`
    Question,

    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `===`
    EqEqEq,
    /// `!==`
    BangEqEq,

    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/` — the spec's `DivPunctuator`, split out because the goal symbol decides whether it
    /// opens a regular expression. That disambiguation arrives with regex literals.
    Slash,
    /// `%`
    Percent,
    /// `**`
    StarStar,
    /// `++`
    PlusPlus,
    /// `--`
    MinusMinus,

    /// `<<`
    LtLt,
    /// `>>`
    GtGt,
    /// `>>>`
    GtGtGt,

    /// `&`
    Amp,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `!`
    Bang,
    /// `~`
    Tilde,
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `??`
    QuestionQuestion,

    /// `=`
    Eq,
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,
    /// `*=`
    StarEq,
    /// `/=`
    SlashEq,
    /// `%=`
    PercentEq,
    /// `**=`
    StarStarEq,
    /// `<<=`
    LtLtEq,
    /// `>>=`
    GtGtEq,
    /// `>>>=`
    GtGtGtEq,
    /// `&=`
    AmpEq,
    /// `|=`
    PipeEq,
    /// `^=`
    CaretEq,
    /// `&&=`
    AmpAmpEq,
    /// `||=`
    PipePipeEq,
    /// `??=`
    QuestionQuestionEq,
}

impl TokenKind {
    /// The source text every token of this kind has, or `None` when the text varies.
    ///
    /// Punctuators and keywords have exactly one spelling; [`TokenKind::Eof`] has the empty one.
    /// Identifiers do not, so they answer `None` rather than a lie — a caller that wants their
    /// text asks the span, and a caller that wants their *value* asks [`identifier_value`].
    ///
    /// Written as a match rather than a lookup in `PUNCTUATORS` on purpose: two independent
    /// spellings of the same fact let the tests catch a table row that drifted, which a
    /// self-consistent lookup never could.
    pub fn as_str(&self) -> Option<&'static str> {
        Some(match self {
            Self::Eof => "",

            Self::Identifier { .. }
            | Self::PrivateIdentifier { .. }
            | Self::Number { .. }
            | Self::BigInt
            | Self::String { .. } => return None,
            Self::Keyword(word) => word.as_str(),

            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBracket => "[",
            Self::RBracket => "]",

            Self::Dot => ".",
            Self::DotDotDot => "...",
            Self::Semicolon => ";",
            Self::Comma => ",",
            Self::Colon => ":",
            Self::Arrow => "=>",
            Self::QuestionDot => "?.",
            Self::Question => "?",

            Self::Lt => "<",
            Self::Gt => ">",
            Self::LtEq => "<=",
            Self::GtEq => ">=",
            Self::EqEq => "==",
            Self::BangEq => "!=",
            Self::EqEqEq => "===",
            Self::BangEqEq => "!==",

            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::Percent => "%",
            Self::StarStar => "**",
            Self::PlusPlus => "++",
            Self::MinusMinus => "--",

            Self::LtLt => "<<",
            Self::GtGt => ">>",
            Self::GtGtGt => ">>>",

            Self::Amp => "&",
            Self::Pipe => "|",
            Self::Caret => "^",
            Self::Bang => "!",
            Self::Tilde => "~",
            Self::AmpAmp => "&&",
            Self::PipePipe => "||",
            Self::QuestionQuestion => "??",

            Self::Eq => "=",
            Self::PlusEq => "+=",
            Self::MinusEq => "-=",
            Self::StarEq => "*=",
            Self::SlashEq => "/=",
            Self::PercentEq => "%=",
            Self::StarStarEq => "**=",
            Self::LtLtEq => "<<=",
            Self::GtGtEq => ">>=",
            Self::GtGtGtEq => ">>>=",
            Self::AmpEq => "&=",
            Self::PipeEq => "|=",
            Self::CaretEq => "^=",
            Self::AmpAmpEq => "&&=",
            Self::PipePipeEq => "||=",
            Self::QuestionQuestionEq => "??=",
        })
    }
}

/// Every punctuator, **longest first**.
///
/// "A token is always as long as possible" (§12.4 states the rule while explaining comments; it
/// governs the whole lexical grammar), so `>>>=` must be tried before `>>>`, `>>` and `>`. The
/// ordering is the entire correctness argument for the match loop, so a test asserts it rather
/// than trusting the next person to insert a row in the right place.
pub(super) const PUNCTUATORS: &[(&str, TokenKind)] = &[
    // 4 bytes.
    (">>>=", TokenKind::GtGtGtEq),
    // 3 bytes.
    ("...", TokenKind::DotDotDot),
    ("===", TokenKind::EqEqEq),
    ("!==", TokenKind::BangEqEq),
    ("**=", TokenKind::StarStarEq),
    ("<<=", TokenKind::LtLtEq),
    (">>=", TokenKind::GtGtEq),
    (">>>", TokenKind::GtGtGt),
    ("&&=", TokenKind::AmpAmpEq),
    ("||=", TokenKind::PipePipeEq),
    ("??=", TokenKind::QuestionQuestionEq),
    // 2 bytes.
    ("=>", TokenKind::Arrow),
    ("==", TokenKind::EqEq),
    ("!=", TokenKind::BangEq),
    ("<=", TokenKind::LtEq),
    (">=", TokenKind::GtEq),
    ("+=", TokenKind::PlusEq),
    ("-=", TokenKind::MinusEq),
    ("*=", TokenKind::StarEq),
    ("/=", TokenKind::SlashEq),
    ("%=", TokenKind::PercentEq),
    ("&=", TokenKind::AmpEq),
    ("|=", TokenKind::PipeEq),
    ("^=", TokenKind::CaretEq),
    ("**", TokenKind::StarStar),
    ("++", TokenKind::PlusPlus),
    ("--", TokenKind::MinusMinus),
    ("<<", TokenKind::LtLt),
    (">>", TokenKind::GtGt),
    ("&&", TokenKind::AmpAmp),
    ("||", TokenKind::PipePipe),
    ("??", TokenKind::QuestionQuestion),
    ("?.", TokenKind::QuestionDot),
    // 1 byte.
    ("{", TokenKind::LBrace),
    ("}", TokenKind::RBrace),
    ("(", TokenKind::LParen),
    (")", TokenKind::RParen),
    ("[", TokenKind::LBracket),
    ("]", TokenKind::RBracket),
    (".", TokenKind::Dot),
    (";", TokenKind::Semicolon),
    (",", TokenKind::Comma),
    (":", TokenKind::Colon),
    ("?", TokenKind::Question),
    ("<", TokenKind::Lt),
    (">", TokenKind::Gt),
    ("+", TokenKind::Plus),
    ("-", TokenKind::Minus),
    ("*", TokenKind::Star),
    ("/", TokenKind::Slash),
    ("%", TokenKind::Percent),
    ("&", TokenKind::Amp),
    ("|", TokenKind::Pipe),
    ("^", TokenKind::Caret),
    ("!", TokenKind::Bang),
    ("~", TokenKind::Tilde),
    ("=", TokenKind::Eq),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::test_support::*;
    #[test]
    fn every_punctuator_lexes_as_itself_and_the_table_is_ordered_longest_first() {
        // Longest-first ordering is the whole correctness argument for the match loop, and it
        // is a property of the table's *order* — nothing else in the file would notice a row
        // inserted in the wrong place.
        for pair in PUNCTUATORS.windows(2) {
            let [(before, _), (after, _)] = pair else {
                continue;
            };
            assert!(
                before.len() >= after.len(),
                "{before:?} must not precede the longer {after:?}"
            );
        }
        // The table and `as_str` are written independently; each row must agree with its kind,
        // and each kind must appear exactly once.
        let mut seen = std::collections::HashSet::new();
        for &(text, kind) in PUNCTUATORS {
            assert_eq!(
                kind.as_str(),
                Some(text),
                "table row {text:?} disagrees with as_str"
            );
            assert!(seen.insert(kind), "{text:?} appears twice in the table");
            // …and every one of them actually lexes, in isolation, to exactly itself. `?.` is
            // the one exception to "text in, kind out" and has its own test.
            if kind != TokenKind::QuestionDot {
                assert_eq!(kinds(text), [kind, TokenKind::Eof], "lexing {text:?}");
            }
        }
        assert_eq!(seen.len(), 57, "ECMA-262 §12.8 has 57 punctuators");
        // Eof's fixed text is the empty one, which is what makes the span/kind cross-check in
        // `retile` work uniformly for it. Identifiers have no fixed text at all — the
        // difference between `Some("")` and `None` is "always empty" versus "ask the source".
        assert_eq!(TokenKind::Eof.as_str(), Some(""));
        assert_eq!(
            TokenKind::Identifier {
                contains_escape: false
            }
            .as_str(),
            None
        );
        assert_eq!(
            TokenKind::PrivateIdentifier {
                contains_escape: false
            }
            .as_str(),
            None
        );
    }
}
