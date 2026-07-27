//! Which operators exist, how tightly they bind, and which pairs the grammar keeps apart.
//!
//! Tables and predicates, with no parsing in them. §13.6 through §13.16 nest one layer inside
//! the next — a `CoalesceExpression` contains a `BitwiseORExpression`, which contains a
//! `BitwiseXORExpression`, and so on — and that nesting read as numbers is all a precedence
//! table is. Writing it this way rather than as one function per layer is not a shortcut: a
//! dozen layers would put a dozen stack frames between one bracket and the next, and
//! [`super::MAX_NESTING_DEPTH`] is measured in exactly those frames (DR-0006).

use super::{ParseError, ParseErrorKind};
use crate::ast::{
    AssignmentOperator, BinaryOperator, Expr, ExprKind, LogicalOperator, UnaryOperator,
};
use crate::lexer::{ReservedWord, TokenKind};

/// A binary operator, with what it means and how tightly it binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Operator {
    /// What the operator does, which decides which node it builds.
    pub(super) kind: OperatorKind,
    /// Binding power. Higher binds tighter; the numbers themselves mean nothing beyond order.
    pub(super) precedence: u8,
    /// Whether `a op b op c` groups to the right. Only `**` does (§13.6), which is why
    /// `2 ** 3 ** 2` is 512 rather than 64.
    pub(super) right_associative: bool,
}

/// Which kind of node an operator builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperatorKind {
    /// Both operands are always evaluated.
    Binary(BinaryOperator),
    /// The right operand may not be evaluated at all.
    Logical(LogicalOperator),
}

/// The prefix operators of §13.5, or `None` if this token starts no unary expression.
///
/// `await` is absent: it is `UnaryExpression`'s `[+Await]` alternative, and needs the parameter
/// that arrives with async functions.
pub(super) fn unary_operator(kind: TokenKind) -> Option<UnaryOperator> {
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
pub(super) fn binary_operator(kind: TokenKind) -> Option<Operator> {
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
pub(super) fn assignment_operator(kind: TokenKind) -> Option<AssignmentOperator> {
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
pub(super) fn is_simple_assignment_target(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::Identifier(_))
}

/// Whether `expr` is an unparenthesized `&&` or `||`, which §13.13 keeps out of a `??`.
pub(super) fn is_bare_and_or(expr: &Expr) -> bool {
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
pub(super) fn is_bare_coalesce(expr: &Expr) -> bool {
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
pub(super) fn combine(left: Expr, operator: Operator, right: Expr) -> Result<Expr, ParseError> {
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
