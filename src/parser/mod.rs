//! Tokens to a syntax tree.
//!
//! # How the goal symbol is chosen
//!
//! The lexer refuses to guess whether a `/` is division or a regular expression, and hands the
//! question to whoever knows ([`Goal`]). This is that caller, and the rule it uses is a single
//! invariant, stated here once because every [`Parser::advance`] call depends on it:
//!
//! > **The goal is chosen when advancing *past* a token, by what may legally follow it.**
//!
//! A token that completes an operand is followed by an operator, so the parser advances past it
//! under [`Goal::Div`]. A token that demands an operand — an operator, an opening parenthesis,
//! the start of a statement — is followed by one, so the parser advances under [`Goal::RegExp`].
//! There is no lookahead buffer to invalidate and no rescanning, because a position is never
//! read twice: by the time the parser knows what a token is, it has already decided what may
//! come after it.
//!
//! # Recursion is bounded here, not by the operating system
//!
//! DR-0002 requires it, and requires it in the same commit as the recursion itself: a
//! recursive-descent parser handed `((((…` recurses once per bracket, and a stack overflow is
//! not a failure any `Result` can rescue — it takes the embedder's process with it. So every
//! recursive entry is counted, and refused past [`MAX_NESTING_DEPTH`]. The cap is a number we
//! chose rather than one the specification has an opinion about, and it is chosen from a
//! measurement of what a level of nesting actually costs — see the constant.

use crate::ast::{Expr, ExprKind};
use crate::lexer::{
    Goal, LexError, LexErrorKind, Lexer, ReservedWord, Token, TokenKind, identifier_value,
    numeric_value, regexp_parts, string_value,
};
use crate::span::Span;
use std::fmt;

/// How deeply the grammar may nest before the parser gives up.
///
/// ECMAScript sets no limit, so this is our refusal rather than the grammar's — which is the
/// point of giving it its own [`ParseErrorKind::TooDeeplyNested`] instead of dressing it up as a
/// syntax error.
///
/// # Where the number comes from
///
/// Measured, not guessed. A debug build spends roughly 1.1 KiB of stack per level of nesting, so
/// parsing at this cap needs a little over half a megabyte — which is why
/// `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` runs a full-depth parse inside a
/// thread with exactly one mebibyte and no more. That test is the real specification of this
/// constant: raise the cap, or make a level of nesting cost more stack, and it fails.
///
/// It will cost more. Every production the parser gains adds frames between one bracket and the
/// next, so the per-level cost grows as the grammar fills in and this number has to be re-earned
/// rather than assumed. A release build is several times cheaper, and the cap is set for the
/// debug one deliberately: `cargo test` must not be the configuration that crashes.
pub const MAX_NESTING_DEPTH: u32 = 512;

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

/// Parse `source` as a single expression, which must be all of it.
///
/// A placeholder entry point: the real one is `parse_script`, and it arrives with statements.
///
/// ```
/// use praxis::ast::ExprKind;
/// use praxis::parser::parse_expression;
///
/// let expr = parse_expression("(1)").expect("this parses");
/// assert_eq!(expr.kind, ExprKind::Number(1.0));
/// assert!(expr.parenthesized);
/// ```
pub fn parse_expression(source: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(source)?;
    let expr = parser.parse_expression()?;
    parser.expect_eof()?;
    Ok(expr)
}

/// A recursive-descent parser over one source text.
struct Parser<'a> {
    source: &'a str,
    lexer: Lexer<'a>,
    /// The token under consideration. Always already lexed — see the module documentation on how
    /// its goal was chosen.
    current: Token,
    /// How many recursive entries are open. See [`Parser::nested`].
    depth: u32,
}

impl<'a> Parser<'a> {
    /// A parser positioned on the first token of `source`.
    ///
    /// That token is read under [`Goal::RegExp`], because a program begins where an operand may
    /// stand: a leading `/` opens a regular expression and never divides.
    fn new(source: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token(Goal::RegExp)?;
        Ok(Self {
            source,
            lexer,
            current,
            depth: 0,
        })
    }

    /// Consume the current token and read the next one under `goal`.
    ///
    /// The returned token is the one just consumed, which is almost always the one the caller
    /// wanted to look at — so `let token = self.advance(…)?` reads as "take this and move on".
    fn advance(&mut self, goal: Goal) -> Result<Token, ParseError> {
        let consumed = self.current;
        self.current = self.lexer.next_token(goal)?;
        Ok(consumed)
    }

    /// Open one level of nesting, refusing rather than recursing past [`MAX_NESTING_DEPTH`].
    ///
    /// Paired with [`Parser::leave`] rather than wrapping a closure, because a closure costs two
    /// stack frames per level and the whole point of the count is to spend as few as possible.
    /// The pairing is checked by a test that a *failed* nested parse still leaves the count
    /// where it found it, since that is the case a stray `?` would break.
    fn enter(&mut self) -> Result<(), ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(ParseError {
                kind: ParseErrorKind::TooDeeplyNested,
                span: self.current.span,
            });
        }
        self.depth += 1;
        Ok(())
    }

    /// Close one level of nesting.
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// The error for "the grammar wanted `expected` here".
    fn unexpected(&self, expected: &'static str) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected,
                found: self.current.kind,
            },
            span: self.current.span,
        }
    }

    /// Consume the current token if it is `kind`, reading the next under `goal`.
    fn eat(
        &mut self,
        kind: TokenKind,
        goal: Goal,
        expected: &'static str,
    ) -> Result<Token, ParseError> {
        if self.current.kind != kind {
            return Err(self.unexpected(expected));
        }
        self.advance(goal)
    }

    /// Require that nothing follows.
    fn expect_eof(&self) -> Result<(), ParseError> {
        if self.current.kind != TokenKind::Eof {
            return Err(self.unexpected("end of input"));
        }
        Ok(())
    }

    /// `Expression`, for as much of it as the grammar reaches today.
    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_primary()
    }

    /// `PrimaryExpression` (§13.2), for the forms that need no other production.
    ///
    /// Only the one recursive production lives in this frame. Everything else is next door in
    /// [`Parser::parse_atom`], because a debug build gives every local in a function its own
    /// stack slot and does not reuse them between match arms — so an arm that never recurses
    /// still costs its slots once per level of nesting. Moving them out roughly halved the stack
    /// a deep parse needs, which is not a speed argument: it is how many legitimate programs
    /// [`MAX_NESTING_DEPTH`] can afford to accept.
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.current;
        if token.kind != TokenKind::LParen {
            return self.parse_atom(token);
        }
        // `( Expression )`. The `(` is advanced past under `Goal::RegExp` because an operand
        // follows it, and the `)` under `Goal::Div` because the bracketed expression is one.
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let inner = self.parse_expression();
        self.leave();
        // The inner failure is reported before the missing `)` is looked for, and the order
        // matters: whatever went wrong inside the brackets happened first and is what the reader
        // needs to see. Checking for the closing bracket first turns every error inside a
        // bracketed expression into "expected `)`" — including, absurdly, the depth cap.
        let inner = inner?;
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok(inner.in_parentheses(token.span.to(close.span)))
    }

    /// The `PrimaryExpression` forms that contain no other expression.
    fn parse_atom(&mut self, token: Token) -> Result<Expr, ParseError> {
        let literal = |kind| {
            Ok(Expr {
                kind,
                span: token.span,
                parenthesized: false,
            })
        };
        match token.kind {
            TokenKind::Keyword(ReservedWord::This) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::This)
            }
            TokenKind::Keyword(ReservedWord::Null) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Null)
            }
            TokenKind::Keyword(ReservedWord::True) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(true))
            }
            TokenKind::Keyword(ReservedWord::False) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(false))
            }
            // An `Identifier` is an `IdentifierName` that is not a `ReservedWord` — and the lexer
            // has already made that distinction, contextual keywords included.
            TokenKind::Identifier { .. } => {
                self.advance(Goal::Div)?;
                let name = identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::Identifier(name.into_owned()))
            }
            TokenKind::Number { .. } => {
                self.advance(Goal::Div)?;
                let value = numeric_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::Number(value))
            }
            TokenKind::String { .. } => {
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::String(value))
            }
            TokenKind::RegExp => {
                self.advance(Goal::Div)?;
                let parts = regexp_parts(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                let text = |span: Span| span.slice(self.source).unwrap_or_default().to_string();
                literal(ExprKind::RegExp {
                    body: text(parts.body),
                    flags: text(parts.flags),
                })
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    /// The error for a token whose value the lexer produced but this parser cannot read back.
    ///
    /// Unreachable in principle — the value functions accept every span the lexer hands out — but
    /// the types do not say so, and the alternative to an error here is an `unwrap` that DR-0002
    /// forbids. It reports the token as unexpected, which is what it has become.
    fn value_missing(&self, token: Token) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected: "a literal this parser can read",
                found: token.kind,
            },
            span: token.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parsed expression of `source`.
    fn parse(source: &str) -> Expr {
        parse_expression(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)) // a test about a tree cannot proceed without one
    }

    /// The error `source` fails with.
    fn error(source: &str) -> ParseError {
        match parse_expression(source) {
            Err(err) => err,
            Ok(expr) => panic!("{source:?} should not parse, got {expr:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn every_primary_expression_the_grammar_reaches_today() {
        assert_eq!(parse("this").kind, ExprKind::This);
        assert_eq!(parse("null").kind, ExprKind::Null);
        assert_eq!(parse("true").kind, ExprKind::Boolean(true));
        assert_eq!(parse("false").kind, ExprKind::Boolean(false));
        assert_eq!(parse("1").kind, ExprKind::Number(1.0));
        assert_eq!(parse("0x10").kind, ExprKind::Number(16.0));
        assert_eq!(parse("1e3").kind, ExprKind::Number(1000.0));
        assert_eq!(parse("'hi'").kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse(r#""hi""#).kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse("x").kind, ExprKind::Identifier("x".to_string()));
        // The value is the cooked one, so an escaped name and a plain one give the same node.
        assert_eq!(parse(r"x").kind, ExprKind::Identifier("x".to_string()));
        // Contextual keywords are identifiers, which is the whole reason the lexer refused to
        // decide: `let` and `of` are ordinary names until a grammatical context says otherwise.
        assert_eq!(parse("let").kind, ExprKind::Identifier("let".to_string()));
        assert_eq!(parse("of").kind, ExprKind::Identifier("of".to_string()));
        assert_eq!(
            parse("async").kind,
            ExprKind::Identifier("async".to_string())
        );
        // …while a genuine reserved word is not an expression at all.
        assert_eq!(
            error("var").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Keyword(ReservedWord::Var),
            }
        );
        // Spans cover exactly the construct.
        assert_eq!(parse("  1  ").span, Span::new(2, 3));
        assert_eq!(parse("this").span, Span::new(0, 4));
    }

    #[test]
    fn a_slash_at_the_start_of_an_expression_opens_a_literal() {
        // The goal symbol, from the other side of the handoff. `Parser::new` reads the first
        // token under `Goal::RegExp` because a program begins where an operand may stand — so
        // this is a regular expression and not the start of a division.
        assert_eq!(
            parse("/ab+/gi").kind,
            ExprKind::RegExp {
                body: "ab+".to_string(),
                flags: "gi".to_string(),
            }
        );
        // The escaped slash and the character class stay in the body, since the lexer found the
        // real closing slash.
        assert_eq!(
            parse(r"/a\/[/]b/").kind,
            ExprKind::RegExp {
                body: r"a\/[/]b".to_string(),
                flags: String::new(),
            }
        );
        // Empty flags are an empty string rather than a missing one.
        assert_eq!(
            parse("/x/").kind,
            ExprKind::RegExp {
                body: "x".to_string(),
                flags: String::new(),
            }
        );
        // …and inside parentheses, where an operand may also stand.
        assert!(matches!(parse("(/x/)").kind, ExprKind::RegExp { .. }));
    }

    #[test]
    fn a_slash_after_an_operand_is_division_and_so_is_not_an_expression_yet() {
        // The other half of the invariant: every primary advances under `Goal::Div`, so the
        // token after one is read as an operator. There is no binary expression to parse it into
        // yet, which is exactly why this reports a stray `/` rather than an unterminated literal
        // — the reading is already correct, and only the grammar is incomplete.
        assert_eq!(
            error("a / b").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::Slash,
            }
        );
        assert_eq!(error("a / b").span, Span::new(2, 3));
        // Had the goal been wrong here, `/ b /` would have lexed as a literal and this would
        // have complained about something else entirely — or not at all.
        assert_eq!(
            error("a /b/ g").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::Slash,
            }
        );
        // `/=` likewise: an operator after an operand, a literal before one.
        assert_eq!(
            error("a /= b").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::SlashEq,
            }
        );
        assert_eq!(
            parse("/=x/").kind,
            ExprKind::RegExp {
                body: "=x".to_string(),
                flags: String::new(),
            }
        );
    }

    #[test]
    fn parentheses_are_recorded_without_becoming_a_node() {
        let bracketed = parse("(1)");
        assert_eq!(bracketed.kind, ExprKind::Number(1.0));
        assert!(bracketed.parenthesized);
        assert_eq!(
            bracketed.span,
            Span::new(0, 3),
            "the span covers the brackets"
        );
        // …and the same expression without them is not marked.
        assert!(!parse("1").parenthesized);
        // Nesting them changes only the span: no rule counts brackets.
        let twice = parse("((1))");
        assert!(twice.parenthesized);
        assert_eq!(twice.kind, ExprKind::Number(1.0));
        assert_eq!(twice.span, Span::new(0, 5));
        assert_eq!(parse(" ( 1 ) ").span, Span::new(1, 6));
        // An empty pair is not an expression — `()` is only meaningful as an arrow parameter
        // list, which is a cover grammar this parser does not reach yet.
        assert_eq!(
            error("()").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::RParen,
            }
        );
        assert_eq!(
            error("(1").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(
            error("(1 2)").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Number { legacy: false },
            }
        );
    }

    #[test]
    fn nesting_is_bounded_by_the_parser_rather_than_by_the_stack() {
        // DR-0002: a stack overflow is not a failure any `Result` can rescue, and it takes the
        // embedder's process with it. So the cap is the parser's, it is explicit, and it is
        // reported as its own kind — the grammar has no depth limit, this refusal is ours.
        let at_the_cap = format!(
            "{}1{}",
            "(".repeat(MAX_NESTING_DEPTH as usize),
            ")".repeat(MAX_NESTING_DEPTH as usize)
        );
        assert_eq!(parse(&at_the_cap).kind, ExprKind::Number(1.0));

        let past_it = format!(
            "{}1{}",
            "(".repeat(MAX_NESTING_DEPTH as usize + 1),
            ")".repeat(MAX_NESTING_DEPTH as usize + 1)
        );
        assert_eq!(error(&past_it).kind, ParseErrorKind::TooDeeplyNested);

        // Far past it: the answer must still be an error rather than a crash, and must arrive
        // without parsing the other million brackets first.
        let absurd = "(".repeat(1_000_000);
        assert_eq!(error(&absurd).kind, ParseErrorKind::TooDeeplyNested);

        // The count unwinds. Two deep parses in a row must both succeed, which they cannot if a
        // failed one leaks its depth — and a failure inside brackets is the case that leaks,
        // since `enter` and `leave` are paired by hand rather than by a scope.
        assert!(parse_expression("((((1))))").is_ok());
        assert!(parse_expression("((((@))))").is_err());
        assert!(parse_expression("((((1))))").is_ok());
        assert!(parse_expression("((((1)").is_err());
        assert!(parse_expression("((((1))))").is_ok());
    }

    #[test]
    fn parsing_at_the_cap_fits_in_the_stack_it_claims_to_need() {
        // This is what makes MAX_NESTING_DEPTH a measurement rather than a hope. A cap that the
        // stack cannot afford is worse than no cap at all: the parse dies by overflow — which
        // DR-0002 says no `Result` can rescue and which takes the embedder's process with it —
        // one level before the check that was supposed to prevent exactly that.
        //
        // One mebibyte is the smallest thread stack in common use, and this runs in a debug
        // build, which is several times hungrier than a release one. If a future production adds
        // frames between one bracket and the next, this test is where it says so.
        let source = format!(
            "{}1{}",
            "(".repeat(MAX_NESTING_DEPTH as usize),
            ")".repeat(MAX_NESTING_DEPTH as usize)
        );
        let worker = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || parse_expression(&source).map(|expr| expr.kind))
            .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
        let parsed = worker
            .join()
            .unwrap_or_else(|_| panic!("a full-depth parse did not survive one mebibyte")); // the panic IS the assertion
        assert_eq!(parsed, Ok(ExprKind::Number(1.0)));
    }

    #[test]
    fn a_lexical_failure_arrives_as_a_parse_error_with_its_span_intact() {
        // The parser does not re-word what the lexer said; it carries it. A diagnostic that lost
        // the difference between "unterminated string" and "unexpected token" would be worse
        // than one that never had it.
        assert_eq!(
            error("'abc").kind,
            ParseErrorKind::Lexical(LexErrorKind::UnterminatedStringLiteral)
        );
        assert_eq!(error("'abc").span, Span::new(0, 4));
        assert_eq!(
            error("@").kind,
            ParseErrorKind::Lexical(LexErrorKind::UnexpectedCharacter)
        );
        assert_eq!(
            error("(1 @)").kind,
            ParseErrorKind::Lexical(LexErrorKind::UnexpectedCharacter),
            "a failure mid-parse is still the lexer's, reported where it happened"
        );
        assert_eq!(error("(1 @)").span, Span::new(3, 4));
        assert_eq!(
            error("3in").kind,
            ParseErrorKind::Lexical(LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit)
        );
    }

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
            error("(/a/ /b/)").kind.to_string(),
            "expected `)`, found `/`",
            "after an operand the goal is Div, so this second slash divides rather than opening"
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

    #[test]
    fn no_source_however_odd_can_make_the_parser_panic() {
        // DR-0002, at the level above the lexer's. Deep nesting is the one that matters here,
        // and the rest are the shapes that reach the parser's own error paths.
        let cases = [
            String::new(),
            "(".repeat(100_000),
            ")".repeat(100_000),
            "((((".to_string(),
            "'".to_string(),
            "/".to_string(),
            "`".to_string(),
            "0x".to_string(),
            format!("({})", "1 ".repeat(10_000)),
            format!("{}1", "(".repeat(500)),
        ];
        for source in &cases {
            // The verdict does not matter; not unwinding does.
            let _ = parse_expression(source);
        }
        // An empty source wants an expression and says so.
        assert_eq!(
            error("").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Eof,
            }
        );
    }
}
