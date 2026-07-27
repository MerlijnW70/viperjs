//! Source text to tokens — the lexer's skeleton: trivia, punctuators, and end of input.
//!
//! This is the first slice of M1 and it deliberately stops short of the interesting literals.
//! What is here is what every later slice stands on: a cursor that can never split a character
//! or read past the end, spans that tile the source exactly, and the `newline_before` flag that
//! automatic semicolon insertion will need long before it is used.
//!
//! # What is not here yet
//!
//! Identifiers, numeric literals, string literals, templates and regular expressions arrive in
//! the following slices. Until then a character that can only begin one of those — `a`, `1`,
//! `"`, `` ` `` — is a [`LexErrorKind::UnexpectedCharacter`], which is also the permanent answer
//! for a character with no token form at all (`@`, `\0`). Two further deferrals, each pinned by
//! a test so that implementing them is a deliberate change and not an accident:
//!
//! - **Annex B.1.1 HTML-like comments.** `<!--` lexes as `<` `!` `--` today. `-->` additionally
//!   needs "nothing but trivia before it on this line" state and a Script-vs-Module goal flag,
//!   neither of which exists yet.
//! - **§12.5 hashbang comments.** `#!` is only a comment at byte 0 of the source, which is
//!   position state this slice has no other use for.
//!
//! # The one property that matters
//!
//! Every token knows its exact extent, and the token spans plus the trivia gaps between them
//! reconstruct the source byte for byte. That is the oracle for this slice (see the module's
//! tests), and it is what keeps every later slice honest: a lexer that quietly loses a byte is
//! a parser that reports the wrong line for the next three years.

use crate::span::Span;
use std::fmt;

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
    /// The exact source text of this token, or `""` for [`TokenKind::Eof`].
    ///
    /// Written as a match rather than a lookup in `PUNCTUATORS` on purpose: two independent
    /// spellings of the same fact let the tests catch a table row that drifted, which a
    /// self-consistent lookup never could.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eof => "",

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
        }
    }
}

/// Why lexing stopped, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError {
    /// What went wrong.
    pub kind: LexErrorKind,
    /// The offending source text. For an unterminated comment this reaches to the end of the
    /// source, because that is genuinely how much of the file the comment swallowed.
    pub span: Span,
}

/// The failures this slice's lexer can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    /// A `/*` with no matching `*/` before the end of the source (§12.4 — comments do not nest,
    /// and there is no "unterminated at EOF is fine" allowance).
    UnterminatedComment,
    /// A code point that begins no token form. Note that while this slice is incomplete, this
    /// also covers the literals it has not learned yet — see the module documentation.
    UnexpectedCharacter,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnterminatedComment => "unterminated block comment",
            Self::UnexpectedCharacter => "unexpected character",
        })
    }
}

/// Every punctuator, **longest first**.
///
/// "A token is always as long as possible" (§12.4 states the rule while explaining comments; it
/// governs the whole lexical grammar), so `>>>=` must be tried before `>>>`, `>>` and `>`. The
/// ordering is the entire correctness argument for the match loop, so a test asserts it rather
/// than trusting the next person to insert a row in the right place.
const PUNCTUATORS: &[(&str, TokenKind)] = &[
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

        if self.cursor.is_eof() {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: Span::empty_at(start),
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

    /// Consume white space, line terminators and comments; report whether a line terminator was
    /// crossed (directly or inside a block comment).
    fn skip_trivia(&mut self) -> Result<bool, LexError> {
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
                _ => return Ok(newline),
            }
        }
    }

    /// Consume `//` and everything up to — but **not including** — the next line terminator.
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
                    assert_eq!(
                        text,
                        token.kind.as_str(),
                        "span and kind disagree in {source:?}"
                    );
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

    /// The kinds of a source that lexes cleanly, EOF included.
    fn kinds(source: &str) -> Vec<TokenKind> {
        Lexer::new(source)
            .tokens()
            .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // a test asserting clean lexing has nothing to say if lexing failed
            .iter()
            .map(|t| t.kind)
            .collect()
    }

    /// The single non-EOF token of a source, for tests about one token's flags.
    fn first(source: &str) -> Token {
        let mut lexer = Lexer::new(source);
        lexer
            .next_token()
            .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // same
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
        ];
        for source in lexes_completely {
            let (tiled, stopped) = retile(source);
            assert_eq!(tiled, source, "retiling {source:?}");
            assert_eq!(stopped, source.len(), "stopped early on {source:?}");
        }

        // Inputs that stop partway: the reconstruction must still be an exact prefix — the
        // lexer may refuse to continue, but it may not invent or lose a byte before it does.
        for source in ["/*", "/*/", "/* x", "?.5", "@", "a", ";\u{200b}"] {
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
                text,
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
        // Eof is the only kind with no source text, which is what makes the span/kind
        // cross-check in `retile` work uniformly for it.
        assert_eq!(TokenKind::Eof.as_str(), "");
    }

    #[test]
    fn optional_chaining_yields_to_a_following_decimal_digit() {
        // §12.8: `?. [lookahead ∉ DecimalDigit]`. `a?.5:b` is a conditional expression that has
        // been legal since ES3; lexing `?.` there breaks code older than optional chaining.
        // Driven token by token because the `5` is a numeric literal, which this slice cannot
        // lex yet — what is under test is that the `?` and `.` came out separately.
        let mut lexer = Lexer::new("?.5");
        assert_eq!(lexer.next_token().map(|t| t.kind), Ok(TokenKind::Question));
        assert_eq!(lexer.next_token().map(|t| t.kind), Ok(TokenKind::Dot));
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
            ("a", 1),        // an identifier: a later slice
            ("1", 1),        // a numeric literal: a later slice
            ("\"", 1),       // a string literal: a later slice
            ("\u{0000}", 1), // NUL is legal source text, just not a token start
            ("é", 2),        // two bytes
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

    #[test]
    fn the_two_lex_errors_describe_themselves_differently() {
        // An error a host cannot render is not an error value. Distinctness matters more than
        // the exact wording: two failures that print the same are one failure to a user.
        let unterminated = LexErrorKind::UnterminatedComment.to_string();
        let unexpected = LexErrorKind::UnexpectedCharacter.to_string();
        assert!(unterminated.contains("comment"), "{unterminated:?}");
        assert!(unexpected.contains("character"), "{unexpected:?}");
        assert_ne!(unterminated, unexpected);
    }
}
