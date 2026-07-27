//! Source text to tokens — trivia, punctuators, names, and end of input.
//!
//! What is here is what every later slice stands on: a cursor that can never split a character
//! or read past the end, spans that tile the source exactly, the `newline_before` flag that
//! automatic semicolon insertion will need long before it is used, and identifiers over the
//! real Unicode `ID_Start`/`ID_Continue` sets rather than an ASCII approximation of them.
//!
//! # What is not here yet
//!
//! Every token form §12 defines is here. A character with no token form at all — `@`, `€`,
//! `\0` — is a [`LexErrorKind::UnexpectedCharacter`], permanently. Two deferrals remain,
//! each pinned by a test so that implementing it is a deliberate change and not an accident:
//!
//! - **Annex B.1.1 HTML-like comments.** `<!--` lexes as `<` `!` `--` today; `-->` would
//!   additionally need "nothing but trivia before it on this line" state and a Script-vs-Module
//!   goal flag.
//! - **`BigInt` values.** The `n` suffix produces a [`TokenKind::BigInt`], and the parser has a
//!   production for it — but the *value* waits for the BigInt type at M7. [`numeric_value`]
//!   answers `None` for such a span rather than handing back the nearest `f64`;
//!   [`bigint_digits`] answers with the digits, which is everything that can be known without
//!   arbitrary-precision arithmetic.
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
//! - `number` — how far a numeric literal reaches (§12.9.3), Annex B's legacy forms included.
//! - `number_value` — what one denotes, correctly rounded (§12.9.3.3), and what a
//!   `BigIntLiteral` is made of.
//! - `string` — string literals and the code units they denote (§12.9.4).
//! - `regexp` — where a regular expression literal ends (§12.9.5).
//! - `template` — template components and their two values (§12.9.6).
//! - `escape` — `UnicodeEscapeSequence` and UTF-16 encoding, shared by `name` and `string`.
//! - here — the cursor, the [`Goal`] symbol, and [`Lexer::next_token`]: the one place that
//!   decides which of the above a character belongs to.

mod error;
mod escape;
mod name;
mod number;
mod number_value;
mod regexp;
mod reserved;
mod string;
mod template;
#[cfg(test)]
mod test_support;
mod token;
mod trivia;

pub use self::error::{LexError, LexErrorKind};
pub use self::name::identifier_value;
pub use self::number_value::{bigint_digits, numeric_value};
pub use self::regexp::{RegExpParts, regexp_parts};
pub use self::reserved::ReservedWord;
pub use self::string::string_value;
pub use self::template::{TemplatePart, TemplateValue, template_value};
pub use self::token::{Token, TokenKind};

use self::token::PUNCTUATORS;
use crate::span::Span;
use crate::unicode_id::is_id_start;

/// A position in the source that can only move forward, one whole code point at a time.
///
/// The point of the type is that it has no panicking path and no unreachable branch: the
/// remaining text is held as a slice rather than an index, so "advance" is
/// [`std::str::Chars::as_str`] and never a range expression that could land mid-character.
#[derive(Clone, Copy)]
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

/// Which of §12.6's goal symbols the next token is read under.
///
/// ECMAScript's lexical grammar is not context-free at the token level: the same characters are
/// different tokens depending on what the parser is expecting. `/` is division under
/// `InputElementDiv` and opens a regular expression literal under `InputElementRegExp`, so
/// `a /b/ g` is two divisions and `f(/b/g)` is one literal — from the same seven characters.
///
/// The lexer cannot choose. Nothing in the text distinguishes the two readings; only a parser
/// that knows whether an operand or an operator comes next does. Engines that guess from the
/// previous token get most programs right and a few famously wrong, so this is a parameter
/// rather than a heuristic, supplied at every call.
///
/// The four are a two-by-two: a goal answers "may a `/` open a literal here?" and "may a `}`
/// resume a template here?", and every combination of those answers is one of §12.6's names.
///
/// §12.6 lists five. `InputElementHashbangOrRegExp` is absent because it differs from
/// `InputElementRegExp` only in admitting a `HashbangComment`, which §12.5 already confines to
/// byte 0 of the source — [`super::trivia`] tests the position instead, which is the same rule
/// with nothing for a caller to get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Goal {
    /// `InputElementDiv`: a `/` here is the division operator, a `}` closes a block.
    Div,
    /// `InputElementRegExp`: a `/` here opens a `RegularExpressionLiteral`.
    RegExp,
    /// `InputElementTemplateTail`: a `}` here resumes a template — the goal a parser uses once
    /// it has finished the substitution expression inside one.
    TemplateTail,
    /// `InputElementRegExpOrTemplateTail`: both at once.
    RegExpOrTemplateTail,
}

impl Goal {
    /// Whether a `/` opens a `RegularExpressionLiteral` rather than being division.
    fn slash_opens_regexp(self) -> bool {
        matches!(self, Self::RegExp | Self::RegExpOrTemplateTail)
    }

    /// Whether a `}` resumes a template rather than being the `RightBracePunctuator`.
    fn brace_resumes_template(self) -> bool {
        matches!(self, Self::TemplateTail | Self::RegExpOrTemplateTail)
    }
}

/// Turns source text into tokens.
///
/// ```
/// use praxis::lexer::{Goal, Lexer, TokenKind};
///
/// let tokens = Lexer::new("{ /* hi */ }").tokens(Goal::Div).expect("this source lexes");
/// let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
/// assert_eq!(kinds, [TokenKind::LBrace, TokenKind::RBrace, TokenKind::Eof]);
/// ```
///
/// `Copy`, and deliberately so: a lexer is two string slices and nothing else, so a caller that
/// needs to see the token *after* the one it is holding can copy the lexer and read from the
/// copy. That is a snapshot rather than a buffer — there is no state to invalidate, and no
/// question of which goal symbol a buffered token was read under, because the copy is read under
/// whichever goal the caller asks for.
#[derive(Clone, Copy)]
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

    /// A lexer positioned at `offset` bytes into `source`.
    ///
    /// For the one construct that has to read a token twice. `` `a${b}c` `` is four tokens, and
    /// which of them the `}` belongs to is not something the lexer can know — it depends on
    /// whether the expression before it has finished, which only the parser knows. So the parser
    /// reads it, discovers it wanted a template component, and asks again from the same place
    /// under [`Goal::TemplateTail`].
    ///
    /// Spans stay absolute, being measured against `source` rather than against the tail. An
    /// offset that is not a character boundary, or is past the end, gives a lexer positioned at
    /// the end — the same answer an exhausted one gives, and no panic (DR-0002).
    pub fn resume_at(source: &'a str, offset: u32) -> Self {
        let rest = source.get(offset as usize..).unwrap_or("");
        Self {
            cursor: Cursor { source, rest },
        }
    }

    /// The next token, or the error that stopped lexing.
    ///
    /// Once end of input is reached this returns [`TokenKind::Eof`] forever: a parser recovering
    /// from an error will ask again, and it must not matter how many times it does.
    pub fn next_token(&mut self, goal: Goal) -> Result<Token, LexError> {
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

        // A quote can only open a string literal, so this needs no lookahead at all.
        if first == '"' || first == '\'' {
            let kind = self.scan_string(first)?;
            return Ok(Token {
                kind,
                span: Span::new(start, self.cursor.offset()),
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

        // §12.9.6. A backtick opens a template under every goal — `Template` is a `CommonToken`
        // — while a `}` resumes one only where the parser says a substitution just ended. The
        // scanner needs no nesting counter: the goal carries that knowledge in from the
        // recursion that already has it.
        if first == '`' || (first == '}' && goal.brace_resumes_template()) {
            let kind = self.scan_template(first == '`')?;
            return Ok(Token {
                kind,
                span: Span::new(start, self.cursor.offset()),
                newline_before,
            });
        }

        // §12.6, and the one place the goal symbol changes what a character means. Trivia has
        // already taken `//` and `/*`, so a `/` arriving here starts a literal rather than a
        // comment — which is exactly why Note 2's "`//` is a comment, not an empty literal" and
        // `RegularExpressionFirstChar`'s exclusion of `*` need no code of their own.
        if first == '/' && goal.slash_opens_regexp() {
            let kind = self.scan_regexp()?;
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
    pub fn tokens(mut self, goal: Goal) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token(goal)?;
            let done = token.kind == TokenKind::Eof;
            tokens.push(token);
            if done {
                return Ok(tokens);
            }
        }
    }
}

#[cfg(test)]
mod tests;
