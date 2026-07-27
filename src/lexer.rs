//! Source text to tokens — trivia, punctuators, names, and end of input.
//!
//! What is here is what every later slice stands on: a cursor that can never split a character
//! or read past the end, spans that tile the source exactly, the `newline_before` flag that
//! automatic semicolon insertion will need long before it is used, and identifiers over the
//! real Unicode `ID_Start`/`ID_Continue` sets rather than an ASCII approximation of them.
//!
//! # What is not here yet
//!
//! Numeric literals, string literals, templates and regular expressions arrive in the following
//! slices. Until then a character that can only begin one of those — `1`, `"`, `` ` `` — is a
//! [`LexErrorKind::UnexpectedCharacter`], which is also the permanent answer for a character
//! with no token form at all (`@`, `€`, `\0`). One deferral remains, pinned by a test so that
//! implementing it is a deliberate change and not an accident: **Annex B.1.1 HTML-like
//! comments**, where `<!--` lexes as `<` `!` `--` today, and `-->` would additionally need
//! "nothing but trivia before it on this line" state and a Script-vs-Module goal flag.
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

use crate::span::Span;
use crate::unicode_id::{is_id_continue, is_id_start};
use std::borrow::Cow;
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

            Self::Identifier { .. } | Self::PrivateIdentifier { .. } => return None,
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

/// The 38 spellings of `ReservedWord` (ECMA-262 §12.7.2).
///
/// This is the whole list and nothing more. `await` and `yield` are here because the production
/// lists them, even though §12.7.2 marks them contextually allowed as identifiers — that
/// exception is expressed by parameterized productions in §13.1, which is the parser's problem.
/// The five categories §12.7.2 describes collapse, for a lexer, into exactly one question: is
/// this spelling in the `ReservedWord` production? Contextual keywords (`let`, `static`,
/// `async`, `of`, `get`, `set`, `from`, `as`, `target`, `meta`) and the strict-mode future
/// reserved words (`implements`, `interface`, `package`, `private`, `protected`, `public`) are
/// deliberately absent: they are ordinary `IdentifierName`s until a grammatical context says
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedWord {
    /// `await`
    Await,
    /// `break`
    Break,
    /// `case`
    Case,
    /// `catch`
    Catch,
    /// `class`
    Class,
    /// `const`
    Const,
    /// `continue`
    Continue,
    /// `debugger`
    Debugger,
    /// `default`
    Default,
    /// `delete`
    Delete,
    /// `do`
    Do,
    /// `else`
    Else,
    /// `enum` — reserved but unused; §12.7.2 Note 2 sets it aside for future extensions.
    Enum,
    /// `export`
    Export,
    /// `extends`
    Extends,
    /// `false`
    False,
    /// `finally`
    Finally,
    /// `for`
    For,
    /// `function`
    Function,
    /// `if`
    If,
    /// `import`
    Import,
    /// `in`
    In,
    /// `instanceof`
    Instanceof,
    /// `new`
    New,
    /// `null`
    Null,
    /// `return`
    Return,
    /// `super`
    Super,
    /// `switch`
    Switch,
    /// `this`
    This,
    /// `throw`
    Throw,
    /// `true`
    True,
    /// `try`
    Try,
    /// `typeof`
    Typeof,
    /// `var`
    Var,
    /// `void`
    Void,
    /// `while`
    While,
    /// `with`
    With,
    /// `yield`
    Yield,
}

impl ReservedWord {
    /// The one spelling this word has.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Await => "await",
            Self::Break => "break",
            Self::Case => "case",
            Self::Catch => "catch",
            Self::Class => "class",
            Self::Const => "const",
            Self::Continue => "continue",
            Self::Debugger => "debugger",
            Self::Default => "default",
            Self::Delete => "delete",
            Self::Do => "do",
            Self::Else => "else",
            Self::Enum => "enum",
            Self::Export => "export",
            Self::Extends => "extends",
            Self::False => "false",
            Self::Finally => "finally",
            Self::For => "for",
            Self::Function => "function",
            Self::If => "if",
            Self::Import => "import",
            Self::In => "in",
            Self::Instanceof => "instanceof",
            Self::New => "new",
            Self::Null => "null",
            Self::Return => "return",
            Self::Super => "super",
            Self::Switch => "switch",
            Self::This => "this",
            Self::Throw => "throw",
            Self::True => "true",
            Self::Try => "try",
            Self::Typeof => "typeof",
            Self::Var => "var",
            Self::Void => "void",
            Self::While => "while",
            Self::With => "with",
            Self::Yield => "yield",
        }
    }

    /// The reserved word `text` spells exactly, if it spells one.
    ///
    /// Only ever called with an escape-free `IdentifierName`, which is what makes a plain string
    /// comparison correct: §12.7.2 Note 1 says a keyword matches literal source characters, so
    /// `els\u{65}` must not reach here and be told it is `else`.
    pub fn from_text(text: &str) -> Option<Self> {
        Some(match text {
            "await" => Self::Await,
            "break" => Self::Break,
            "case" => Self::Case,
            "catch" => Self::Catch,
            "class" => Self::Class,
            "const" => Self::Const,
            "continue" => Self::Continue,
            "debugger" => Self::Debugger,
            "default" => Self::Default,
            "delete" => Self::Delete,
            "do" => Self::Do,
            "else" => Self::Else,
            "enum" => Self::Enum,
            "export" => Self::Export,
            "extends" => Self::Extends,
            "false" => Self::False,
            "finally" => Self::Finally,
            "for" => Self::For,
            "function" => Self::Function,
            "if" => Self::If,
            "import" => Self::Import,
            "in" => Self::In,
            "instanceof" => Self::Instanceof,
            "new" => Self::New,
            "null" => Self::Null,
            "return" => Self::Return,
            "super" => Self::Super,
            "switch" => Self::Switch,
            "this" => Self::This,
            "throw" => Self::Throw,
            "true" => Self::True,
            "try" => Self::Try,
            "typeof" => Self::Typeof,
            "var" => Self::Var,
            "void" => Self::Void,
            "while" => Self::While,
            "with" => Self::With,
            "yield" => Self::Yield,
            _ => return None,
        })
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
    /// A `\` that is not followed by a well-formed `UnicodeEscapeSequence` (§12.9.4): not a `u`,
    /// fewer than four hex digits, or a `\u{…}` with no digits or no closing brace.
    InvalidUnicodeEscape,
    /// `\u{…}` whose value exceeds U+10FFFF — the spec's `NotCodePoint`.
    CodePointOutOfRange,
    /// An escape that is well-formed but contributes a code point which cannot appear where it
    /// was written (§12.7.1.1): `\u{20}` is a space, and a space is no part of a name.
    EscapedCodePointIsNotAnIdentifierCharacter,
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
        })
    }
}

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
/// use praxis::lexer::{identifier_value, Lexer, TokenKind};
///
/// // A raw string, so the source really does contain a backslash: this spells `abc` the
/// // long way round, and the value comes back as if it had been spelled plainly.
/// let source = r"\u0061bc";
/// let token = Lexer::new(source).next_token().expect("this lexes");
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

/// The value of one `HexDigit` (§12.9.3), or `None` if `ch` is not one.
///
/// `char::to_digit` is exactly right here and rarely is: it accepts only `0-9`, `a-z` and `A-Z`,
/// so it agrees with `HexDigit` on the ASCII range and — importantly — rejects the Arabic-Indic
/// and fullwidth digits that a `is_numeric`-based check would wave through.
fn hex_value(ch: char) -> Option<u32> {
    ch.to_digit(16)
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

    /// Decide whether a just-scanned `IdentifierName` is a keyword.
    ///
    /// §12.7.2 Note 1: keywords match a literal sequence of source characters, so a spelling
    /// that used an escape is an `IdentifierName` and never a keyword — `els\u{65}` does not
    /// declare an `else`. It is not thereby a usable binding either, but that is §13.1.1's early
    /// error and needs the grammatical context only the parser has.
    fn classify_name(&self, span: Span, contains_escape: bool) -> TokenKind {
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
    fn scan_identifier(&mut self) -> Result<bool, LexError> {
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

    /// Consume `\ UnicodeEscapeSequence` (§12.9.4) and return the code point it denotes.
    ///
    /// Two forms: `\u` followed by exactly four hex digits, or `\u{` HexDigits `}` where the
    /// value must not exceed U+10FFFF (the spec's `CodePoint`, against `NotCodePoint`). The
    /// braced form takes `HexDigits[~Sep]` — **no numeric separators**, and any number of
    /// digits, so `\u{00000000000061}` is a perfectly ordinary `a`.
    ///
    /// The returned value is deliberately a `u32` and not a `char`: `\uD800` and `\u{10FFFF}`
    /// are both well-formed escapes whose acceptability depends on where they appear, and the
    /// caller is the one that knows.
    fn read_unicode_escape(&mut self) -> Result<u32, LexError> {
        let start = self.cursor.offset();
        // Every ill-formed exit reports the same span: from the backslash to wherever the
        // sequence stopped making sense.
        macro_rules! malformed {
            () => {
                LexError {
                    kind: LexErrorKind::InvalidUnicodeEscape,
                    span: Span::new(start, self.cursor.offset()),
                }
            };
        }

        self.cursor.advance_ascii(1); // the `\`
        if self.cursor.peek() != Some('u') {
            return Err(malformed!());
        }
        self.cursor.advance_ascii(1);

        if self.cursor.peek() == Some('{') {
            self.cursor.advance_ascii(1);
            let mut value: u32 = 0;
            let mut digits = 0usize;
            while let Some(digit) = self.cursor.peek().and_then(hex_value) {
                let _ = self.cursor.bump();
                digits += 1;
                // Saturating, not wrapping: the digit count is chosen by whoever wrote the
                // source, so `\u{FFFFFFFFFFFFFFFF}` is an input, and an input may not overflow
                // (DR-0002). Saturation lands far above U+10FFFF, which is the answer anyway.
                value = value.saturating_mul(16).saturating_add(digit);
            }
            if digits == 0 || self.cursor.peek() != Some('}') {
                return Err(malformed!());
            }
            self.cursor.advance_ascii(1);
            if value > 0x10ffff {
                return Err(LexError {
                    kind: LexErrorKind::CodePointOutOfRange,
                    span: Span::new(start, self.cursor.offset()),
                });
            }
            return Ok(value);
        }

        // `Hex4Digits :: HexDigit HexDigit HexDigit HexDigit` — exactly four. A fifth digit is
        // simply the next character of the name, which is what makes `a0` the name `a0`.
        let mut value: u32 = 0;
        for _ in 0..4 {
            let Some(digit) = self.cursor.peek().and_then(hex_value) else {
                return Err(malformed!());
            };
            let _ = self.cursor.bump();
            // Bounded by construction: four hex digits cannot exceed 0xFFFF.
            value = value * 16 + digit;
        }
        Ok(value)
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
                // §12.5: a hashbang comment is "location-sensitive" — `#!` is a comment only at
                // the very first byte of the source. One byte later the same two characters are
                // a private name that happens to be malformed, and `#!/usr/bin/env node` on
                // line 2 is a syntax error. Hence a position test, not a lookahead.
                Some('#')
                    if self.cursor.offset() == 0 && self.cursor.peek_byte(1) == Some(b'!') =>
                {
                    self.skip_line_comment();
                }
                _ => return Ok(newline),
            }
        }
    }

    /// Consume a two-character comment opener — `//` or §12.5's `#!` — and everything up to but
    /// **not including** the next line terminator.
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
            "?.5",
            "@",
            "1",
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
            ("1", 1),        // a numeric literal: a later slice
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

    /// The cooked value of the first token of `source`.
    fn name_of(source: &str) -> String {
        let token = first(source);
        identifier_value(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should have an identifier value")) // a test about the value cannot proceed without one
            .into_owned()
    }

    /// `Identifier { contains_escape: false }`, the overwhelmingly common case.
    const PLAIN: TokenKind = TokenKind::Identifier {
        contains_escape: false,
    };
    /// `Identifier { contains_escape: true }`.
    const ESCAPED: TokenKind = TokenKind::Identifier {
        contains_escape: true,
    };

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
        // A digit cannot start one — which is what keeps `1` available for the numeric literal
        // slice rather than making `1abc` a name.
        assert_eq!(
            Lexer::new("1").next_token().map(|t| t.kind),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(0, 1),
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
        assert!(Lexer::new("x\u{200b}y").tokens().is_err());
        // Symbols that look like they might qualify and do not.
        for source in ["\u{20ac}", "\u{1f680}", "\u{00a7}"] {
            assert!(Lexer::new(source).tokens().is_err(), "{source:?}");
        }
    }

    #[test]
    fn exactly_the_thirty_eight_reserved_words_lex_as_keywords() {
        use ReservedWord::*;
        // The §12.7.2 `ReservedWord` production, written out a third time — independently of
        // `as_str` and of `from_text`, which is what makes this a check rather than an echo.
        let all: &[(&str, ReservedWord)] = &[
            ("await", Await),
            ("break", Break),
            ("case", Case),
            ("catch", Catch),
            ("class", Class),
            ("const", Const),
            ("continue", Continue),
            ("debugger", Debugger),
            ("default", Default),
            ("delete", Delete),
            ("do", Do),
            ("else", Else),
            ("enum", Enum),
            ("export", Export),
            ("extends", Extends),
            ("false", False),
            ("finally", Finally),
            ("for", For),
            ("function", Function),
            ("if", If),
            ("import", Import),
            ("in", In),
            ("instanceof", Instanceof),
            ("new", New),
            ("null", Null),
            ("return", Return),
            ("super", Super),
            ("switch", Switch),
            ("this", This),
            ("throw", Throw),
            ("true", True),
            ("try", Try),
            ("typeof", Typeof),
            ("var", Var),
            ("void", Void),
            ("while", While),
            ("with", With),
            ("yield", Yield),
        ];
        assert_eq!(
            all.len(),
            38,
            "the ReservedWord production has 38 alternatives — recount before changing this"
        );
        let mut seen = std::collections::HashSet::new();
        for &(text, word) in all {
            assert_eq!(word.as_str(), text);
            assert_eq!(ReservedWord::from_text(text), Some(word));
            assert!(seen.insert(word), "{text:?} appears twice");
            assert_eq!(
                kinds(text),
                [TokenKind::Keyword(word), TokenKind::Eof],
                "lexing {text:?}"
            );
        }

        // Contextual keywords are NOT reserved words. Lexing them as keywords would make
        // `var let = 1` a syntax error, which it is not outside strict mode, and would take the
        // decision away from the parser that actually has the context (§12.7.2's five
        // categories). The strict-mode future reserved words belong to the parser too.
        for text in [
            "let",
            "static",
            "async",
            "of",
            "get",
            "set",
            "from",
            "as",
            "target",
            "meta",
            "implements",
            "interface",
            "package",
            "private",
            "protected",
            "public",
            "arguments",
            "eval",
            "undefined",
            "NaN",
        ] {
            assert_eq!(ReservedWord::from_text(text), None, "{text:?}");
            assert_eq!(kinds(text), [PLAIN, TokenKind::Eof], "lexing {text:?}");
        }
        // Near misses: a prefix, an extension, and a case change are all just names.
        for text in ["awai", "awaits", "Await", "iff", "i", "IF", "in_"] {
            assert_eq!(ReservedWord::from_text(text), None, "{text:?}");
            assert_eq!(kinds(text), [PLAIN, TokenKind::Eof], "lexing {text:?}");
        }
        // A keyword still ends where the name ends.
        assert_eq!(
            kinds("if(a)"),
            [
                TokenKind::Keyword(If),
                TokenKind::LParen,
                PLAIN,
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
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
                Lexer::new(source).tokens().map(|t| t.len()),
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
            Lexer::new("\\u0030").tokens(),
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
            Lexer::new("ab\\u0020").tokens(),
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
                Lexer::new(source).tokens().map(|t| t.len()),
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
                Lexer::new(source).tokens().map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::CodePointOutOfRange,
                    span: Span::new(0, source.len() as u32),
                }),
                "on {source:?}"
            );
        }
        // The boundary itself is in range — it is rejected later, and for a different reason.
        assert_eq!(
            Lexer::new("\\u{10FFFF}").tokens(),
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
                Lexer::new(source).tokens().map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span,
                }),
                "on {source:?}"
            );
        }
    }

    #[test]
    fn a_hashbang_is_a_comment_only_at_the_very_first_byte() {
        // §12.5: hashbang comments are "location-sensitive". At byte 0 this is how a script
        // becomes executable; anywhere else the same two characters are a malformed private
        // name, and treating them as a comment would silently delete a line of code.
        assert_eq!(
            kinds("#!/usr/bin/env node\n;"),
            [TokenKind::Semicolon, TokenKind::Eof]
        );
        assert!(first("#!/usr/bin/env node\n;").newline_before);
        // It runs to the line terminator and no further, and may end at EOF instead.
        assert_eq!(kinds("#!x"), [TokenKind::Eof]);
        assert_eq!(
            kinds("#!"),
            [TokenKind::Eof],
            "the shortest hashbang there is"
        );
        // A second one on the next line is NOT a comment — it is at byte 4.
        assert_eq!(
            Lexer::new("#!x\n#!y").tokens(),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(5, 6),
            })
        );
        // One byte later it is not a comment. Even a single space disqualifies it, because the
        // hashbang precedes everything — including white space.
        for source in [" #!x", "\n#!x", ";#!x"] {
            assert_eq!(
                Lexer::new(source).tokens().map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::UnexpectedCharacter,
                    span: Span::new(2, 3),
                }),
                "on {source:?}"
            );
        }
        // A `#` at byte 0 that is not followed by `!` is an ordinary private name.
        assert_eq!(
            kinds("#x"),
            [
                TokenKind::PrivateIdentifier {
                    contains_escape: false
                },
                TokenKind::Eof
            ]
        );
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
            .tokens()
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
            Lexer::new("a\\u{20}").tokens().is_err(),
            "…and the lexer does refuse it"
        );
        // An empty span is an empty value rather than a failure — `Span::empty_at` is what EOF
        // carries, and asking it for a name should not be exciting.
        assert_eq!(
            identifier_value("abc", Span::empty_at(1)).as_deref(),
            Some("")
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
