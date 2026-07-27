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

//! # How this module is laid out
//!
//! - `error` — [`ParseError`] and its kinds.
//! - `operator` — precedence, associativity, and the pairs §13 keeps apart.
//! - `expression` — the grammar of §13, from `Expression` down to `PrimaryExpression`.
//! - `statement` — the grammar of §14, and automatic semicolon insertion (§12.10).
//! - `declaration` — `var`, `let` and `const` (§14.3), and the early errors on them.
//! - `control` — conditionals, loops, `throw`, `break` and `continue` (§14.6 – §14.14).
//! - here — the [`Parser`] itself: the token it is looking at, how it advances, and the count
//!   that bounds its recursion.

mod control;
mod declaration;
mod error;
mod expression;
mod operator;
mod statement;
#[cfg(test)]
mod test_support;

pub use self::error::{ParseError, ParseErrorKind};
pub use self::statement::parse_script;

use crate::ast::Expr;
use crate::lexer::{Goal, Lexer, Token, TokenKind};

/// How deeply the grammar may nest before the parser gives up.
///
/// ECMAScript sets no limit, so this is our refusal rather than the grammar's — which is the
/// point of giving it its own [`ParseErrorKind::TooDeeplyNested`] instead of dressing it up as a
/// syntax error.
///
/// # Where the number comes from
///
/// Measured in a debug build against a one-mebibyte stack — the smallest in common use — and
/// re-measured every time the grammar grows, because it falls every time the grammar grows:
///
/// | after | levels a mebibyte holds | cap |
/// | --- | --- | --- |
/// | primary expressions | 928 | 512 |
/// | prefix and binary operators | 304 | 128 |
/// | conditional, assignment, comma | 168 | 64 |
/// | member access, calls, `new`, update | 112 | 48 |
/// | conditionals and loops | 114 | 48 |
///
/// Each slice put another function between one bracket and the next. That is the trajectory to
/// expect, and it is why keeping the recursive path narrow counts as correctness work rather
/// than optimisation: every frame removed is nesting a real program is allowed to have. Two
/// slices have now bought depth back by moving locals out of a frame the recursion passes
/// through — the trick works because a debug build reuses no stack slots between match arms, so
/// an arm that cannot recurse is still paid for by every level that does.
///
/// The last row is the first slice not to cost anything, and the reason is worth keeping: the
/// count is one budget shared by every kind of nesting, so what bounds it is whichever kind
/// spends the most stack per level. Statements are cheap next to expressions — a level of `if`
/// is three frames where a level of `(` is the whole precedence ladder — and measured alone they
/// afford 339 levels, `while` 504, a block 469. So the expression path still sets the number,
/// and will keep setting it until a statement form recurses through an expression-sized descent.
///
/// `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` runs a full-depth parse of each
/// recursive path in a thread with exactly one mebibyte, and this cap leaves a factor of about
/// two in hand on the narrowest of them. That test is the real specification of this constant:
/// raise the cap, or make a level cost more stack, and it fails.
///
/// # Why a count and not a stack measurement
///
/// Because a stack measurement would make which programs parse depend on how the engine was
/// compiled, and this project's whole premise is a conformance number that does not drift.
/// DR-0006 has the argument, including what it costs — a release build could afford several
/// times this and is not allowed to. The limit becomes an embedder-set value at M3, where
/// somebody knows how much stack there actually is; the default stays conservative.
pub const MAX_NESTING_DEPTH: u32 = 48;

/// Parse `source` as a single expression, which must be all of it.
///
/// A convenience beside [`parse_script`], which is the entry point a program goes through.
/// Useful where an expression is all there is — and, more often, in tests about one.
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
    pub(super) source: &'a str,
    lexer: Lexer<'a>,
    /// The token under consideration. Always already lexed — see the module documentation on how
    /// its goal was chosen.
    pub(super) current: Token,
    /// How many recursive entries are open. See [`Parser::enter`].
    depth: u32,
    /// How many enclosing iteration statements there are, which is what §14.8.1 and §14.9.1 ask
    /// about when they refuse a `break` or `continue` that has nothing to leave.
    pub(super) iteration_depth: u32,
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
            iteration_depth: 0,
        })
    }

    /// Consume the current token and read the next one under `goal`.
    ///
    /// The returned token is the one just consumed, which is almost always the one the caller
    /// wanted to look at — so `let token = self.advance(…)?` reads as "take this and move on".
    pub(super) fn advance(&mut self, goal: Goal) -> Result<Token, ParseError> {
        let consumed = self.current;
        self.current = self.lexer.next_token(goal)?;
        Ok(consumed)
    }

    /// The token after the current one, read under `goal`.
    ///
    /// A copy of the lexer reads it, so nothing is buffered and nothing is invalidated: the
    /// lexer is two string slices, and lexing from a copy leaves the real one exactly where it
    /// was. The goal is a parameter for the same reason it is everywhere else — the caller is
    /// the one who knows what could legally stand there.
    ///
    /// Used sparingly, and only where the grammar genuinely needs two tokens to decide: `let`
    /// is a declaration or an identifier depending on what follows it, and nothing shorter than
    /// looking answers that.
    pub(super) fn peek(&self, goal: Goal) -> Result<Token, ParseError> {
        let mut lookahead = self.lexer;
        Ok(lookahead.next_token(goal)?)
    }

    /// Open one level of nesting, refusing rather than recursing past [`MAX_NESTING_DEPTH`].
    ///
    /// Paired with [`Parser::leave`] rather than wrapping a closure, because a closure costs two
    /// stack frames per level and the whole point of the count is to spend as few as possible.
    /// The pairing is checked by a test that a *failed* nested parse still leaves the count
    /// where it found it, since that is the case a stray `?` would break.
    pub(super) fn enter(&mut self) -> Result<(), ParseError> {
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
    pub(super) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// The error for "the grammar wanted `expected` here".
    pub(super) fn unexpected(&self, expected: &'static str) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected,
                found: self.current.kind,
            },
            span: self.current.span,
        }
    }

    /// Consume the current token if it is `kind`, reading the next under `goal`.
    pub(super) fn eat(
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
    pub(super) fn expect_eof(&self) -> Result<(), ParseError> {
        if self.current.kind != TokenKind::Eof {
            return Err(self.unexpected("end of input"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ExprKind;
    use crate::lexer::LexErrorKind;
    use crate::parser::test_support::*;
    use crate::span::Span;
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
        //
        // Every recursive path gets its own full-depth parse, because the count is one budget
        // shared between them and what bounds it is whichever spends the most stack per level.
        // Expressions do today by a wide margin; the rest are here so that the day one of them
        // overtakes, this fails rather than the cap quietly becoming a lie for one grammar form.
        let deep = MAX_NESTING_DEPTH as usize;
        let paths = [
            format!("{}1{}", "(".repeat(deep), ")".repeat(deep)),
            format!("{}{}", "{".repeat(deep), "}".repeat(deep)),
            format!("{}a;", "if (a) ".repeat(deep)),
            format!("{}a;", "if (a) b; else ".repeat(deep)),
            format!("{}a;", "while (a) ".repeat(deep)),
            format!("{}a;{}", "do ".repeat(deep), " while (b);".repeat(deep)),
            // One shallower, because `throw` counts the frame it holds while its value is
            // parsed — so `throw` plus a full-depth expression is one level past the cap, and
            // the deepest that parses has one bracket fewer.
            format!("throw {}1{};", "(".repeat(deep - 1), ")".repeat(deep - 1)),
        ];
        let worker = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                for source in &paths {
                    // A failure would be a bug in the test's sources, not a stack problem — the
                    // point of the run is that it returns at all.
                    assert!(parse_script(source).is_ok(), "{source:.32?} at full depth");
                }
                parse_expression(&format!("{}1{}", "(".repeat(deep), ")".repeat(deep)))
                    .map(|expr| expr.kind)
            })
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
