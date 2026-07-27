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

use crate::ast::{
    AssignmentOperator, BinaryOperator, Expr, ExprKind, LogicalOperator, RegExpLiteral,
    UnaryOperator,
};
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
/// Measured in a debug build against a one-mebibyte stack — the smallest in common use — and
/// re-measured every time the grammar grows, because it falls every time the grammar grows:
///
/// | after | levels a mebibyte holds | cap |
/// | --- | --- | --- |
/// | primary expressions | 928 | 512 |
/// | prefix and binary operators | 304 | 128 |
/// | conditional, assignment, comma | 168 | 64 |
///
/// Each slice put another function between one bracket and the next. That is the trajectory to
/// expect, and it is why keeping the recursive path narrow counts as correctness work rather
/// than optimisation: every frame removed is nesting a real program is allowed to have.
///
/// `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` runs a full-depth parse in a thread
/// with exactly one mebibyte, and this cap leaves a factor of about two and a half in hand. That
/// test is the real specification of this constant: raise the cap, or make a level cost more
/// stack, and it fails.
///
/// # Why a count and not a stack measurement
///
/// Because a stack measurement would make which programs parse depend on how the engine was
/// compiled, and this project's whole premise is a conformance number that does not drift.
/// DR-0006 has the argument, including what it costs — a release build could afford several
/// times this and is not allowed to. The limit becomes an embedder-set value at M3, where
/// somebody knows how much stack there actually is; the default stays conservative.
pub const MAX_NESTING_DEPTH: u32 = 64;

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

/// A binary operator, with what it means and how tightly it binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Operator {
    /// What the operator does, which decides which node it builds.
    kind: OperatorKind,
    /// Binding power. Higher binds tighter; the numbers themselves mean nothing beyond order.
    precedence: u8,
    /// Whether `a op b op c` groups to the right. Only `**` does (§13.6), which is why
    /// `2 ** 3 ** 2` is 512 rather than 64.
    right_associative: bool,
}

/// Which kind of node an operator builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperatorKind {
    /// Both operands are always evaluated.
    Binary(BinaryOperator),
    /// The right operand may not be evaluated at all.
    Logical(LogicalOperator),
}

/// The prefix operators of §13.5, or `None` if this token starts no unary expression.
///
/// `await` is absent: it is `UnaryExpression`'s `[+Await]` alternative, and needs the parameter
/// that arrives with async functions.
fn unary_operator(kind: TokenKind) -> Option<UnaryOperator> {
    Some(match kind {
        TokenKind::Keyword(ReservedWord::Delete) => UnaryOperator::Delete,
        TokenKind::Keyword(ReservedWord::Void) => UnaryOperator::Void,
        TokenKind::Keyword(ReservedWord::Typeof) => UnaryOperator::Typeof,
        TokenKind::Plus => UnaryOperator::Plus,
        TokenKind::Minus => UnaryOperator::Minus,
        TokenKind::Tilde => UnaryOperator::BitwiseNot,
        TokenKind::Bang => UnaryOperator::LogicalNot,
        _ => return None,
    })
}

/// The binary operators of §13.6 through §13.13, or `None` if this token is not one.
///
/// The precedences are the grammar's nesting read as numbers: §13.13's `CoalesceExpression`
/// contains a `BitwiseORExpression`, which contains a `BitwiseXORExpression`, and so on down to
/// §13.6's `ExponentiationExpression` — each layer binding tighter than the one that contains
/// it. Written as a table rather than as one function per layer because a function per layer
/// would put a dozen stack frames between one bracket and the next, and
/// [`MAX_NESTING_DEPTH`] is measured in exactly those frames.
fn binary_operator(kind: TokenKind) -> Option<Operator> {
    use BinaryOperator as B;
    use LogicalOperator as L;
    let (kind, precedence) = match kind {
        TokenKind::QuestionQuestion => (OperatorKind::Logical(L::NullishCoalescing), 1),
        TokenKind::PipePipe => (OperatorKind::Logical(L::Or), 2),
        TokenKind::AmpAmp => (OperatorKind::Logical(L::And), 3),
        TokenKind::Pipe => (OperatorKind::Binary(B::BitwiseOr), 4),
        TokenKind::Caret => (OperatorKind::Binary(B::BitwiseXor), 5),
        TokenKind::Amp => (OperatorKind::Binary(B::BitwiseAnd), 6),
        TokenKind::EqEq => (OperatorKind::Binary(B::Equal), 7),
        TokenKind::BangEq => (OperatorKind::Binary(B::NotEqual), 7),
        TokenKind::EqEqEq => (OperatorKind::Binary(B::StrictEqual), 7),
        TokenKind::BangEqEq => (OperatorKind::Binary(B::StrictNotEqual), 7),
        TokenKind::Lt => (OperatorKind::Binary(B::LessThan), 8),
        TokenKind::Gt => (OperatorKind::Binary(B::GreaterThan), 8),
        TokenKind::LtEq => (OperatorKind::Binary(B::LessThanOrEqual), 8),
        TokenKind::GtEq => (OperatorKind::Binary(B::GreaterThanOrEqual), 8),
        TokenKind::Keyword(ReservedWord::Instanceof) => (OperatorKind::Binary(B::Instanceof), 8),
        // `RelationalExpression` takes `in` only under `[+In]`, which a `for` head turns off.
        // Nothing turns it off yet, so the parameter arrives with `for` — adding it now would be
        // a flag no test could set.
        TokenKind::Keyword(ReservedWord::In) => (OperatorKind::Binary(B::In), 8),
        TokenKind::LtLt => (OperatorKind::Binary(B::ShiftLeft), 9),
        TokenKind::GtGt => (OperatorKind::Binary(B::ShiftRight), 9),
        TokenKind::GtGtGt => (OperatorKind::Binary(B::ShiftRightUnsigned), 9),
        TokenKind::Plus => (OperatorKind::Binary(B::Add), 10),
        TokenKind::Minus => (OperatorKind::Binary(B::Subtract), 10),
        TokenKind::Star => (OperatorKind::Binary(B::Multiply), 11),
        TokenKind::Slash => (OperatorKind::Binary(B::Divide), 11),
        TokenKind::Percent => (OperatorKind::Binary(B::Remainder), 11),
        TokenKind::StarStar => (OperatorKind::Binary(B::Exponent), 12),
        _ => return None,
    };
    Some(Operator {
        kind,
        precedence,
        right_associative: kind == OperatorKind::Binary(B::Exponent),
    })
}

/// The assignment operators of §13.15, or `None` if this token is not one.
fn assignment_operator(kind: TokenKind) -> Option<AssignmentOperator> {
    use AssignmentOperator as A;
    Some(match kind {
        TokenKind::Eq => A::Assign,
        TokenKind::PlusEq => A::Add,
        TokenKind::MinusEq => A::Subtract,
        TokenKind::StarEq => A::Multiply,
        TokenKind::SlashEq => A::Divide,
        TokenKind::PercentEq => A::Remainder,
        TokenKind::StarStarEq => A::Exponent,
        TokenKind::LtLtEq => A::ShiftLeft,
        TokenKind::GtGtEq => A::ShiftRight,
        TokenKind::GtGtGtEq => A::ShiftRightUnsigned,
        TokenKind::AmpEq => A::BitwiseAnd,
        TokenKind::CaretEq => A::BitwiseXor,
        TokenKind::PipeEq => A::BitwiseOr,
        TokenKind::AmpAmpEq => A::LogicalAnd,
        TokenKind::PipePipeEq => A::LogicalOr,
        TokenKind::QuestionQuestionEq => A::NullishCoalescing,
        _ => return None,
    })
}

/// §13.15.1's `AssignmentTargetType`, for the expressions this parser can build.
///
/// The specification defines it per production, and every one that answers "simple" is either an
/// `IdentifierReference` or a `MemberExpression` — so for now it is exactly "an identifier".
///
/// Parentheses need no mention, and that is not an oversight. Because a bracketed expression is
/// a flag on the node rather than a node of its own, `(a)` *is* the identifier `a` and answers
/// for itself, while `(a, b)` is a sequence and does not. The specification says the same thing
/// the long way round, by giving the parenthesized production the target type of what it covers.
fn is_simple_assignment_target(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::Identifier(_))
}

/// Whether `expr` is an unparenthesized `&&` or `||`, which §13.13 keeps out of a `??`.
fn is_bare_and_or(expr: &Expr) -> bool {
    !expr.parenthesized
        && matches!(
            expr.kind,
            ExprKind::Logical {
                operator: LogicalOperator::And | LogicalOperator::Or,
                ..
            }
        )
}

/// Whether `expr` is an unparenthesized `??`, which §13.13 keeps out of a `&&` or `||`.
fn is_bare_coalesce(expr: &Expr) -> bool {
    !expr.parenthesized
        && matches!(
            expr.kind,
            ExprKind::Logical {
                operator: LogicalOperator::NullishCoalescing,
                ..
            }
        )
}

/// Join two operands with an operator, enforcing §13.13's rule about which may sit together.
///
/// A free function rather than a method because it is called from the recursive loop and holds
/// several temporaries: keeping them out of [`Parser::parse_binary`]'s frame keeps them out of
/// every level of nesting.
fn combine(left: Expr, operator: Operator, right: Expr) -> Result<Expr, ParseError> {
    let span = left.span.to(right.span);
    let kind = match operator.kind {
        OperatorKind::Binary(operator) => ExprKind::Binary {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        },
        OperatorKind::Logical(operator) => {
            // §13.13 keeps `??` and the two boolean operators in separate families: a `??` may
            // not take a bare `&&` or `||` as either operand, and neither may take a bare `??`.
            // `&&` and `||` mix freely with each other, so this is not symmetric between them.
            let forbidden = if operator == LogicalOperator::NullishCoalescing {
                is_bare_and_or
            } else {
                is_bare_coalesce
            };
            if forbidden(&left) {
                return Err(ParseError {
                    kind: ParseErrorKind::MixedCoalesceAndLogical,
                    span: left.span,
                });
            }
            if forbidden(&right) {
                return Err(ParseError {
                    kind: ParseErrorKind::MixedCoalesceAndLogical,
                    span: right.span,
                });
            }
            ExprKind::Logical {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            }
        }
    };
    Ok(Expr {
        kind,
        span,
        parenthesized: false,
    })
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

    /// `Expression` (§13.16) — one or more `AssignmentExpression`s separated by commas.
    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_assignment()?;
        if self.current.kind != TokenKind::Comma {
            return Ok(first);
        }
        let mut parts = vec![first];
        while self.current.kind == TokenKind::Comma {
            self.advance(Goal::RegExp)?;
            parts.push(self.parse_assignment()?);
        }
        let span = match (parts.first(), parts.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            // `parts` was built from one element and only ever grows, so this cannot happen;
            // an empty span is the answer that costs nothing if it somehow does.
            _ => Span::empty_at(self.current.span.start),
        };
        Ok(Expr {
            kind: ExprKind::Sequence(parts),
            span,
            parenthesized: false,
        })
    }

    /// `AssignmentExpression` (§13.15) and `ConditionalExpression` (§13.14), in one frame.
    ///
    /// Merged deliberately. They are separate productions, but a `?` and an `=` cannot both
    /// follow the same operand — a conditional is not a `LeftHandSideExpression`, so it can only
    /// be assigned to in the sense that saying so is an error — and one function fewer on the
    /// recursion path is depth that [`MAX_NESTING_DEPTH`] does not have to give up.
    ///
    /// Falling through to the assignment check rather than returning early after a conditional
    /// is what makes `(a ? b : c) = d` say "this expression cannot be assigned to" instead of
    /// the far less helpful "expected end of input".
    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_binary(0)?;
        if self.current.kind == TokenKind::Question {
            left = self.parse_conditional_tail(left)?;
        }
        let Some(operator) = assignment_operator(self.current.kind) else {
            return Ok(left);
        };
        // §13.15.1. The check is here rather than in the AST because it is a *syntax* error:
        // `1 = 2` must be refused before anything runs, not when it runs.
        if !is_simple_assignment_target(&left) {
            return Err(ParseError {
                kind: ParseErrorKind::InvalidAssignmentTarget,
                span: left.span,
            });
        }
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // `LeftHandSideExpression = AssignmentExpression` — the recursion is on the right, so
        // `a = b = c` is `a = (b = c)`.
        let value = self.parse_assignment();
        self.leave();
        let value = value?;
        Ok(Expr {
            span: left.span.to(value.span),
            kind: ExprKind::Assignment {
                operator,
                target: Box::new(left),
                value: Box::new(value),
            },
            parenthesized: false,
        })
    }

    /// `? AssignmentExpression : AssignmentExpression` (§13.14), with `test` already parsed.
    ///
    /// Both arms are `AssignmentExpression`s, which §13.14's own Note explains: it lets an
    /// assignment be governed by either arm, and keeps a comma expression out of the middle.
    /// So `a ? b = 1 : c = 2` is two assignments, and `a ? b, c : d` is an error.
    fn parse_conditional_tail(&mut self, test: Expr) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let arms = self.parse_conditional_arms();
        self.leave();
        let (consequent, alternate) = arms?;
        Ok(Expr {
            span: test.span.to(alternate.span),
            kind: ExprKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
            parenthesized: false,
        })
    }

    /// The two arms of a conditional, split out so their locals do not sit in a frame that the
    /// bracket recursion also passes through.
    fn parse_conditional_arms(&mut self) -> Result<(Expr, Expr), ParseError> {
        let consequent = self.parse_assignment()?;
        self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
        let alternate = self.parse_assignment()?;
        Ok((consequent, alternate))
    }

    /// The operator layers of §13.6 – §13.13, by precedence climbing.
    ///
    /// `minimum` is the weakest binding power this call will accept; an operator weaker than it
    /// belongs to the caller. Left-associative operators recurse one level tighter so the next
    /// one of equal precedence is left for the loop, and `**` recurses at its own level so the
    /// next one is taken by the recursion instead — which is the whole of associativity.
    fn parse_binary(&mut self, minimum: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some(operator) = binary_operator(self.current.kind) {
            if operator.precedence < minimum {
                break;
            }
            // §13.6: the left operand of `**` is an `UpdateExpression`, and a prefix unary is
            // not one. Checked before the operator is consumed so the error can point at the
            // operand that is wrong rather than at the operator that noticed.
            if operator.kind == OperatorKind::Binary(BinaryOperator::Exponent)
                && matches!(left.kind, ExprKind::Unary { .. })
                && !left.parenthesized
            {
                return Err(ParseError {
                    kind: ParseErrorKind::ExponentiationOnUnary,
                    span: left.span,
                });
            }
            // An operator is followed by an operand, so the goal is `RegExp`: in `a / /b/`, the
            // first slash divides and the second opens a literal.
            self.advance(Goal::RegExp)?;
            let tighter = if operator.right_associative {
                operator.precedence
            } else {
                operator.precedence + 1
            };
            self.enter()?;
            let right = self.parse_binary(tighter);
            self.leave();
            let right = right?;
            left = combine(left, operator, right)?;
        }
        Ok(left)
    }

    /// `UnaryExpression` (§13.5), or whatever it falls through to.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let Some(operator) = unary_operator(self.current.kind) else {
            return self.parse_primary();
        };
        let token = self.advance(Goal::RegExp)?;
        self.enter()?;
        // `- UnaryExpression`, so the operators stack: `- - a` is two of them.
        let argument = self.parse_unary();
        self.leave();
        let argument = argument?;
        Ok(Expr {
            span: token.span.to(argument.span),
            kind: ExprKind::Unary {
                operator,
                argument: Box::new(argument),
            },
            parenthesized: false,
        })
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
                literal(ExprKind::RegExp(Box::new(RegExpLiteral {
                    body: text(parts.body),
                    flags: text(parts.flags),
                })))
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

    /// An expression rendered as a parenthesized prefix form.
    ///
    /// Precedence and associativity are claims about *shape*, and a shape is far easier to read
    /// as `(+ 1 (* 2 3))` than as three nested constructors — which matters, because a test
    /// nobody can read is a test nobody checks.
    fn render(expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::This => "this".to_string(),
            ExprKind::Null => "null".to_string(),
            ExprKind::Boolean(value) => value.to_string(),
            ExprKind::Number(value) => value.to_string(),
            ExprKind::Identifier(name) => name.clone(),
            ExprKind::String(units) => format!("{units:?}"),
            ExprKind::RegExp(literal) => format!("/{}/{}", literal.body, literal.flags),
            ExprKind::Unary { operator, argument } => {
                format!("({} {})", operator.as_str(), render(argument))
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => format!("({} {} {})", operator.as_str(), render(left), render(right)),
            ExprKind::Logical {
                operator,
                left,
                right,
            } => format!("({} {} {})", operator.as_str(), render(left), render(right)),
            ExprKind::Conditional {
                test,
                consequent,
                alternate,
            } => format!(
                "(? {} {} {})",
                render(test),
                render(consequent),
                render(alternate)
            ),
            ExprKind::Assignment {
                operator,
                target,
                value,
            } => format!(
                "({} {} {})",
                operator.as_str(),
                render(target),
                render(value)
            ),
            ExprKind::Sequence(parts) => {
                let rendered: Vec<String> = parts.iter().map(render).collect();
                format!("(, {})", rendered.join(" "))
            }
        }
    }

    /// The shape of the tree `source` parses to.
    fn shape(source: &str) -> String {
        render(&parse(source))
    }

    /// A regular expression node, spelled the way the tests want to read it.
    fn regexp(body: &str, flags: &str) -> ExprKind {
        ExprKind::RegExp(Box::new(RegExpLiteral {
            body: body.to_string(),
            flags: flags.to_string(),
        }))
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
        assert_eq!(parse("/ab+/gi").kind, regexp("ab+", "gi"));
        // The escaped slash and the character class stay in the body, since the lexer found the
        // real closing slash.
        assert_eq!(parse(r"/a\/[/]b/").kind, regexp(r"a\/[/]b", ""));
        // Empty flags are an empty string rather than a missing one.
        assert_eq!(parse("/x/").kind, regexp("x", ""));
        // …and inside parentheses, where an operand may also stand.
        assert!(matches!(parse("(/x/)").kind, ExprKind::RegExp(_)));
    }

    #[test]
    fn a_slash_after_an_operand_divides_and_the_next_one_may_still_open_a_literal() {
        // The other half of the goal invariant, now that there is a binary expression to parse
        // the division into. `a /b/ g` is two divisions, and it is the parser's choice of goal
        // that makes it so — a lexer guessing from the previous token would have to get this
        // right by luck.
        assert_eq!(shape("a / b"), "(/ a b)");
        assert_eq!(shape("a /b/ g"), "(/ (/ a b) g)");
        // An operator is followed by an operand, so the goal after one is `RegExp` again: this
        // really is `a` divided by a regular expression, which is legal and rare.
        assert_eq!(shape("a / /b/"), "(/ a /b/)");
        assert_eq!(shape("typeof /b/"), "(typeof /b/)");
        assert_eq!(shape("1 + /b/g"), "(+ 1 /b/g)");
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
    fn every_prefix_operator_the_grammar_has_today() {
        // §13.5. `await` is absent: it is the `[+Await]` alternative and needs a parameter that
        // arrives with async functions.
        assert_eq!(shape("-a"), "(- a)");
        assert_eq!(shape("+a"), "(+ a)");
        assert_eq!(shape("!a"), "(! a)");
        assert_eq!(shape("~a"), "(~ a)");
        assert_eq!(shape("typeof a"), "(typeof a)");
        assert_eq!(shape("void a"), "(void a)");
        assert_eq!(shape("delete a"), "(delete a)");
        // `- UnaryExpression`, so they stack — and `--` would be one token, which is why the
        // spaced form is the one that means two negations.
        assert_eq!(shape("- - a"), "(- (- a))");
        assert_eq!(shape("!!a"), "(! (! a))");
        assert_eq!(shape("typeof typeof a"), "(typeof (typeof a))");
        // A prefix operator binds tighter than any binary one.
        assert_eq!(shape("-a + b"), "(+ (- a) b)");
        assert_eq!(shape("-a * b"), "(* (- a) b)");
        assert_eq!(shape("typeof a === b"), "(=== (typeof a) b)");
        // The span runs from the operator to the end of its operand.
        assert_eq!(parse("- a").span, Span::new(0, 3));
    }

    #[test]
    fn the_precedence_ladder_is_the_grammars_nesting_read_as_numbers() {
        // Each pair is two adjacent layers of §13.6 – §13.13, checked in both orders so that a
        // table entry cannot be right by accident of which side it was written on.
        // (`??` against `||` is absent on purpose: §13.13 forbids that pair outright, and it
        // has its own test.)
        for (source, shaped) in [
            ("a || b && c", "(|| a (&& b c))"),
            ("a && b || c", "(|| (&& a b) c)"),
            ("a && b | c", "(&& a (| b c))"),
            ("a | b && c", "(&& (| a b) c)"),
            ("a | b ^ c", "(| a (^ b c))"),
            ("a ^ b | c", "(| (^ a b) c)"),
            ("a ^ b & c", "(^ a (& b c))"),
            ("a & b ^ c", "(^ (& a b) c)"),
            ("a & b == c", "(& a (== b c))"),
            ("a == b & c", "(& (== a b) c)"),
            ("a == b < c", "(== a (< b c))"),
            ("a < b == c", "(== (< a b) c)"),
            ("a < b << c", "(< a (<< b c))"),
            ("a << b < c", "(< (<< a b) c)"),
            ("a << b + c", "(<< a (+ b c))"),
            ("a + b << c", "(<< (+ a b) c)"),
            ("a + b * c", "(+ a (* b c))"),
            ("a * b + c", "(+ (* a b) c)"),
            ("a * b ** c", "(* a (** b c))"),
            ("a ** b * c", "(* (** a b) c)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // The relational layer holds the two word-shaped operators as well as the symbols.
        assert_eq!(shape("a instanceof b == c"), "(== (instanceof a b) c)");
        assert_eq!(shape("a in b == c"), "(== (in a b) c)");
        assert_eq!(shape("a + b instanceof c"), "(instanceof (+ a b) c)");
        // Every remaining operator, so no table entry goes unexercised.
        for (source, shaped) in [
            ("a - b", "(- a b)"),
            ("a / b", "(/ a b)"),
            ("a % b", "(% a b)"),
            ("a >> b", "(>> a b)"),
            ("a >>> b", "(>>> a b)"),
            ("a > b", "(> a b)"),
            ("a <= b", "(<= a b)"),
            ("a >= b", "(>= a b)"),
            ("a != b", "(!= a b)"),
            ("a === b", "(=== a b)"),
            ("a !== b", "(!== a b)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // Parentheses override all of it, which is the only reason precedence is bearable.
        assert_eq!(shape("(a + b) * c"), "(* (+ a b) c)");
    }

    #[test]
    fn everything_groups_to_the_left_except_exponentiation() {
        // `AdditiveExpression : AdditiveExpression + MultiplicativeExpression` — the recursion is
        // on the left, so equal precedence groups left.
        assert_eq!(shape("a - b - c"), "(- (- a b) c)");
        assert_eq!(shape("a / b / c"), "(/ (/ a b) c)");
        assert_eq!(shape("a < b < c"), "(< (< a b) c)");
        assert_eq!(shape("a && b && c"), "(&& (&& a b) c)");
        assert_eq!(shape("a ?? b ?? c"), "(?? (?? a b) c)");
        // `ExponentiationExpression : UpdateExpression ** ExponentiationExpression` — the
        // recursion is on the *right*, so `2 ** 3 ** 2` is 512 and not 64.
        assert_eq!(shape("a ** b ** c"), "(** a (** b c))");
        assert_eq!(shape("a ** b ** c ** d"), "(** a (** b (** c d)))");
        // A left-associative chain is a loop rather than a recursion, so its length is bounded by
        // memory rather than by MAX_NESTING_DEPTH.
        let long = vec!["1"; 5000].join(" + ");
        assert!(parse_expression(&long).is_ok());
    }

    #[test]
    fn a_prefix_unary_may_not_be_the_left_operand_of_exponentiation() {
        // §13.6: `ExponentiationExpression : UpdateExpression ** ExponentiationExpression`. A
        // prefix unary is not an `UpdateExpression`, so `-a ** b` has no derivation at all — the
        // rule exists because a reader cannot tell whether it would mean `(-a) ** b` or
        // `-(a ** b)`, and those differ.
        for source in [
            "-a ** b",
            "+a ** b",
            "!a ** b",
            "~a ** b",
            "typeof a ** b",
            "void a ** b",
            "delete a ** b",
        ] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::ExponentiationOnUnary,
                "on {source:?}"
            );
        }
        // The caret goes under the operand that is wrong, not the operator that noticed.
        assert_eq!(error("-a ** b").span, Span::new(0, 2));
        // Both ways of saying what you meant are fine.
        assert_eq!(shape("(-a) ** b"), "(** (- a) b)");
        assert_eq!(shape("-(a ** b)"), "(- (** a b))");
        // The restriction is on the *left* operand only: the right is an
        // `ExponentiationExpression`, which a `UnaryExpression` is.
        assert_eq!(shape("a ** -b"), "(** a (- b))");
        assert_eq!(shape("a ** typeof b"), "(** a (typeof b))");
        // …and only `**` is restricted. Every other operator takes a bare unary on the left.
        assert_eq!(shape("-a * b"), "(* (- a) b)");
        assert_eq!(shape("-a + b"), "(+ (- a) b)");
    }

    #[test]
    fn coalescing_may_not_be_mixed_with_the_boolean_operators_without_parentheses() {
        // §13.13: `CoalesceExpressionHead` admits a `CoalesceExpression` or a
        // `BitwiseORExpression` and nothing else, and `ShortCircuitExpression` keeps the two
        // families apart in the other direction. Both orders are errors, and for the same reason
        // as `**`: nobody would agree on what the unbracketed form meant.
        for source in ["a || b ?? c", "a ?? b || c", "a && b ?? c", "a ?? b && c"] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::MixedCoalesceAndLogical,
                "on {source:?}"
            );
        }
        // The caret goes under the operand from the wrong family.
        assert_eq!(error("a || b ?? c").span, Span::new(0, 6));
        assert_eq!(error("a ?? b || c").span, Span::new(5, 11));
        // Parentheses settle it, in either direction.
        assert_eq!(shape("(a || b) ?? c"), "(?? (|| a b) c)");
        assert_eq!(shape("a ?? (b || c)"), "(?? a (|| b c))");
        assert_eq!(shape("(a ?? b) || c"), "(|| (?? a b) c)");
        assert_eq!(shape("a || (b ?? c)"), "(|| a (?? b c))");
        // `&&` and `||` mix with each other freely — the rule is not symmetric, and a check that
        // rejected `a || b && c` would be rejecting ordinary JavaScript.
        assert_eq!(shape("a || b && c"), "(|| a (&& b c))");
        assert_eq!(shape("a && b || c"), "(|| (&& a b) c)");
        // …and `??` chains with itself, since `CoalesceExpressionHead` may be a
        // `CoalesceExpression`.
        assert_eq!(shape("a ?? b ?? c"), "(?? (?? a b) c)");
        // A `??` whose operand is an ordinary binary expression is fine: the boundary is the
        // boolean operators, not precedence in general.
        assert_eq!(shape("a ?? b + c"), "(?? a (+ b c))");
        assert_eq!(shape("a | b ?? c"), "(?? (| a b) c)");
    }

    #[test]
    fn a_conditional_takes_an_assignment_in_each_arm_and_a_comma_in_neither() {
        // §13.14: `ShortCircuitExpression ? AssignmentExpression : AssignmentExpression`. The
        // Note explains the choice — it lets an assignment be governed by either arm and keeps a
        // comma expression out of the middle, which C and Java do not.
        assert_eq!(shape("a ? b : c"), "(? a b c)");
        assert_eq!(shape("a ? b = 1 : c = 2"), "(? a (= b 1) (= c 2))");
        // The test is a ShortCircuitExpression, so everything below it binds tighter.
        assert_eq!(shape("a || b ? c : d"), "(? (|| a b) c d)");
        assert_eq!(shape("a + b ? c : d"), "(? (+ a b) c d)");
        // Nesting: the alternate is an AssignmentExpression, so a chain groups to the right —
        // which is what makes `a ? b : c ? d : e` mean what everyone assumes it means.
        assert_eq!(shape("a ? b : c ? d : e"), "(? a b (? c d e))");
        assert_eq!(shape("a ? b ? c : d : e"), "(? a (? b c d) e)");
        // A comma is not an AssignmentExpression, so it cannot appear in an arm unbracketed.
        assert_eq!(
            error("a ? b, c : d").kind,
            ParseErrorKind::Unexpected {
                expected: "`:`",
                found: TokenKind::Comma,
            }
        );
        assert_eq!(shape("a ? (b, c) : d"), "(? a (, b c) d)");
        assert_eq!(
            error("a ? b").kind,
            ParseErrorKind::Unexpected {
                expected: "`:`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(parse("a ? b : c").span, Span::new(0, 9));
        // The bracketing flag is part of what a node claims about itself, and nothing else
        // in this slice reads it — so it is asserted here rather than left to a later one
        // that will (arrow functions cover `(a, b)`, and `delete (x, y)` turns on it).
        assert!(!parse("a ? b : c").parenthesized);
        assert!(parse("(a ? b : c)").parenthesized);
    }

    #[test]
    fn assignment_groups_to_the_right_and_only_targets_what_can_be_assigned_to() {
        // §13.15: `LeftHandSideExpression = AssignmentExpression` — the recursion is on the
        // right, so a chain groups that way and `a = b = c` gives both `a` and `b` the value.
        assert_eq!(shape("a = b"), "(= a b)");
        assert_eq!(shape("a = b = c"), "(= a (= b c))");
        // Every operator §13.15 lists, including the three with their own productions.
        for (source, shaped) in [
            ("a += b", "(+= a b)"),
            ("a -= b", "(-= a b)"),
            ("a *= b", "(*= a b)"),
            ("a /= b", "(/= a b)"),
            ("a %= b", "(%= a b)"),
            ("a **= b", "(**= a b)"),
            ("a <<= b", "(<<= a b)"),
            ("a >>= b", "(>>= a b)"),
            ("a >>>= b", "(>>>= a b)"),
            ("a &= b", "(&= a b)"),
            ("a ^= b", "(^= a b)"),
            ("a |= b", "(|= a b)"),
            ("a &&= b", "(&&= a b)"),
            ("a ||= b", "(||= a b)"),
            ("a ??= b", "(??= a b)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // The value is a whole AssignmentExpression, so everything binds tighter than `=`.
        assert_eq!(shape("a = b + c"), "(= a (+ b c))");
        assert_eq!(shape("a += b ? c : d"), "(+= a (? b c d))");
        assert_eq!(shape("a = b || c"), "(= a (|| b c))");

        // §13.15.1: a Syntax Error if the AssignmentTargetType is not simple. Refused here
        // rather than at run time, which is the difference between a program that never starts
        // and one that fails halfway through.
        for source in [
            "1 = 2",
            "'a' = 2",
            "this = 1",
            "null = 1",
            "true = 1",
            "a + b = c",
            "-a = b",
            "(a, b) = 1",
            "(a ? b : c) = d",
            "a ?? b = c",
            "/x/ = 1",
            "1 += 2",
            "1 &&= 2",
        ] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::InvalidAssignmentTarget,
                "on {source:?}"
            );
        }
        // The caret goes under the thing that cannot be assigned to.
        assert_eq!(error("a + b = c").span, Span::new(0, 5));
        // Parentheses do not change the answer, and need no rule of their own: `(a)` *is* the
        // identifier, because bracketing is a flag rather than a node.
        assert_eq!(shape("(a) = 1"), "(= a 1)");
        assert_eq!(shape("((a)) = 1"), "(= a 1)");
        assert!(matches!(
            parse("(a) = 1").kind,
            ExprKind::Assignment { ref target, .. } if target.parenthesized
        ));
        assert_eq!(parse("a = b").span, Span::new(0, 5));
        assert!(!parse("a = b").parenthesized);
        assert!(parse("(a = b)").parenthesized);
        // `&&=`, `||=` and `??=` do not always assign — `a ||= b` leaves `a` alone when it is
        // truthy — which is why the operator says so rather than the compiler guessing.
        assert!(AssignmentOperator::LogicalOr.short_circuits());
        assert!(AssignmentOperator::LogicalAnd.short_circuits());
        assert!(AssignmentOperator::NullishCoalescing.short_circuits());
        assert!(!AssignmentOperator::Assign.short_circuits());
        assert!(!AssignmentOperator::Add.short_circuits());
    }

    #[test]
    fn the_comma_operator_is_the_loosest_thing_there_is_and_is_held_flat() {
        // §13.16: `Expression : Expression , AssignmentExpression`.
        assert_eq!(shape("a, b"), "(, a b)");
        // Flat rather than nested pairs: the grammar's recursion is on the left, so pairs would
        // nest once per comma for no gain — evaluation is left to right either way.
        assert_eq!(shape("a, b, c"), "(, a b c)");
        assert_eq!(shape("a, b, c, d"), "(, a b c d)");
        // …but a bracketed sequence is its own node, so flattening cannot cross parentheses.
        assert_eq!(shape("(a, b), c"), "(, (, a b) c)");
        assert_eq!(shape("a, (b, c)"), "(, a (, b c))");
        // Loosest of all: everything else binds tighter.
        assert_eq!(shape("a = 1, b = 2"), "(, (= a 1) (= b 2))");
        assert_eq!(shape("a ? b : c, d"), "(, (? a b c) d)");
        assert_eq!(shape("a + b, c * d"), "(, (+ a b) (* c d))");
        assert_eq!(parse("a, b").span, Span::new(0, 4));
        assert!(!parse("a, b").parenthesized);
        assert!(parse("(a, b)").parenthesized);
        // A long list is a loop rather than a recursion, so its length is bounded by memory.
        let long = vec!["a"; 5000].join(", ");
        assert!(parse_expression(&long).is_ok());
        // A trailing comma is not part of the operator — that is argument-list syntax.
        assert_eq!(
            error("a, b,").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Eof,
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
