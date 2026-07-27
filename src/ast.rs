//! The syntax tree.
//!
//! Every node is a value that owns what it means and carries a [`Span`] saying where it came
//! from — see `decisions/DR-0005-ast-owns-its-data-and-carries-spans.md` for why those are two
//! separate decisions and why the span is never allowed to become the second copy of the data.
//!
//! The tree grows one grammar slice at a time, so what is here is what the parser can build
//! today: `PrimaryExpression`'s simplest forms, and the prefix and binary operators built
//! on them.

use crate::span::Span;

/// An expression, with where it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Which expression this is, and its contents.
    pub kind: ExprKind,
    /// The source it covers, parentheses included.
    pub span: Span,
    /// Whether it was written inside parentheses.
    ///
    /// Not a node of its own, because nothing evaluates differently for being bracketed — but not
    /// discardable either, because several early errors turn on it. `(a) = 1` is legal and
    /// `(a, b) = 1` is not; `delete (x)` is the same as `delete x` while `delete (x, y)` is not.
    /// A flag rather than a count, since no rule asks how *many* pairs of parentheses there were.
    pub parenthesized: bool,
}

impl Expr {
    /// The same expression, marked as having been written inside parentheses and re-spanned to
    /// include them.
    ///
    /// The span grows because that is what a reader points at: in `((a + b))`, the construct a
    /// diagnostic should underline is the whole bracketed text, not the `a + b` inside it.
    pub fn in_parentheses(self, span: Span) -> Self {
        Self {
            span,
            parenthesized: true,
            ..self
        }
    }
}

/// What an expression is.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// `this`.
    This,
    /// An `Identifier` — the name already has its `\u` escapes resolved (§12.7.1.2).
    Identifier(String),
    /// `null`.
    Null,
    /// `true` or `false`.
    Boolean(bool),
    /// A `NumericLiteral`, already correctly rounded (§12.9.3.3).
    ///
    /// Two literals that denote the same Number are indistinguishable here, which is right:
    /// `1e3` and `1000` are the same value written twice, and only the span remembers which.
    Number(f64),
    /// A `StringLiteral`, as the UTF-16 code units of its `SV` (§12.9.4.2) — possibly including
    /// unpaired surrogates, which is why this is not a `String` (DR-0004).
    String(Vec<u16>),
    /// A prefix `UnaryExpression` (§13.5).
    Unary {
        /// Which operator.
        operator: UnaryOperator,
        /// What it applies to.
        argument: Box<Expr>,
    },
    /// A binary operator that evaluates both operands (§13.6 – §13.12).
    Binary {
        /// Which operator.
        operator: BinaryOperator,
        /// The left operand.
        left: Box<Expr>,
        /// The right operand.
        right: Box<Expr>,
    },
    /// `&&`, `||` or `??` (§13.13), kept apart from [`ExprKind::Binary`] because they are apart
    /// in the grammar and in what they compile to: the right operand may never be evaluated, so
    /// there is a branch here where an arithmetic operator has none.
    Logical {
        /// Which operator.
        operator: LogicalOperator,
        /// The left operand, always evaluated.
        left: Box<Expr>,
        /// The right operand, evaluated only if the left does not decide the answer.
        right: Box<Expr>,
    },
    /// A `RegularExpressionLiteral` (§12.9.5).
    ///
    /// Boxed, and the only variant that is. Two `String`s inline would make it half again as
    /// large as any other variant, and an enum is as large as its largest — so the rarest node
    /// in the grammar would set the size of every expression the parser holds on its stack, and
    /// with it how deeply [`crate::parser::MAX_NESTING_DEPTH`] can afford to let anything nest.
    RegExp(Box<RegExpLiteral>),
}

/// The two halves of a regular expression literal, as written.
///
/// Neither is parsed here: §12.9.5 says both "are subsequently parsed again using the more
/// stringent ECMAScript Regular Expression grammar", which is the RegExp engine's work at M4. So
/// an unparsable pattern is a perfectly good node, and stops being one later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegExpLiteral {
    /// `BodyText` (§12.9.5.1) — everything between the slashes.
    pub body: String,
    /// `FlagText` (§12.9.5.2) — everything after the closing slash. Often empty.
    pub flags: String,
}

/// The prefix operators of §13.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    /// `delete`
    Delete,
    /// `void`
    Void,
    /// `typeof`
    Typeof,
    /// Unary `+`
    Plus,
    /// Unary `-`
    Minus,
    /// `~`
    BitwiseNot,
    /// `!`
    LogicalNot,
}

impl UnaryOperator {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Void => "void",
            Self::Typeof => "typeof",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::BitwiseNot => "~",
            Self::LogicalNot => "!",
        }
    }
}

/// The binary operators that always evaluate both operands (§13.6 – §13.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    /// `**`
    Exponent,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Remainder,
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `<<`
    ShiftLeft,
    /// `>>`
    ShiftRight,
    /// `>>>`
    ShiftRightUnsigned,
    /// `<`
    LessThan,
    /// `>`
    GreaterThan,
    /// `<=`
    LessThanOrEqual,
    /// `>=`
    GreaterThanOrEqual,
    /// `instanceof`
    Instanceof,
    /// `in`
    In,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `===`
    StrictEqual,
    /// `!==`
    StrictNotEqual,
    /// `&`
    BitwiseAnd,
    /// `^`
    BitwiseXor,
    /// `|`
    BitwiseOr,
}

impl BinaryOperator {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exponent => "**",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
            Self::Add => "+",
            Self::Subtract => "-",
            Self::ShiftLeft => "<<",
            Self::ShiftRight => ">>",
            Self::ShiftRightUnsigned => ">>>",
            Self::LessThan => "<",
            Self::GreaterThan => ">",
            Self::LessThanOrEqual => "<=",
            Self::GreaterThanOrEqual => ">=",
            Self::Instanceof => "instanceof",
            Self::In => "in",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::StrictEqual => "===",
            Self::StrictNotEqual => "!==",
            Self::BitwiseAnd => "&",
            Self::BitwiseXor => "^",
            Self::BitwiseOr => "|",
        }
    }
}

/// The short-circuiting operators of §13.13.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalOperator {
    /// `&&`
    And,
    /// `||`
    Or,
    /// `??`
    NullishCoalescing,
}

impl LogicalOperator {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::And => "&&",
            Self::Or => "||",
            Self::NullishCoalescing => "??",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(kind: ExprKind) -> Expr {
        Expr {
            kind,
            span: Span::new(0, 1),
            parenthesized: false,
        }
    }

    #[test]
    fn parenthesizing_marks_the_node_and_widens_its_span_without_touching_its_meaning() {
        // The flag exists for early errors that distinguish `(a) = 1` from `(a, b) = 1`, and the
        // widened span exists because that is the text a diagnostic should underline. Neither may
        // change what the expression *is*.
        let inner = expr(ExprKind::Identifier("a".to_string()));
        let outer = inner.clone().in_parentheses(Span::new(0, 3));
        assert_eq!(outer.kind, inner.kind);
        assert_eq!(outer.span, Span::new(0, 3));
        assert!(outer.parenthesized);
        assert!(!inner.parenthesized);
        // Doing it twice is idempotent in everything but the span: no rule counts the brackets.
        let twice = outer.clone().in_parentheses(Span::new(0, 5));
        assert!(twice.parenthesized);
        assert_eq!(twice.span, Span::new(0, 5));
        assert_eq!(twice.kind, inner.kind);
    }

    #[test]
    fn no_single_variant_is_allowed_to_set_the_size_of_every_expression() {
        // An enum is as large as its largest variant, and the parser holds `Expr` values on its
        // stack once per level of nesting — so a fat variant is paid for by
        // `MAX_NESTING_DEPTH`, in levels. The regular expression literal is the only one that
        // needed boxing; this is the assertion that says so, and that would fail if a later
        // slice added another.
        assert!(
            size_of::<ExprKind>() <= 32,
            "ExprKind grew to {} bytes — box the variant that did it",
            size_of::<ExprKind>()
        );
        assert!(
            size_of::<Expr>() <= 48,
            "Expr is {} bytes",
            size_of::<Expr>()
        );
    }

    #[test]
    fn two_literals_are_equal_when_they_denote_the_same_value_however_they_were_written() {
        // Equality is over meaning, not spelling — the span is the only record of how a value
        // was written, and DR-0005 forbids reading meaning back out of it.
        assert_eq!(ExprKind::Number(1000.0), ExprKind::Number(1e3));
        assert_ne!(ExprKind::Number(0.0), ExprKind::Number(1.0));
        assert_eq!(ExprKind::String(vec![0x61]), ExprKind::String(vec![0x61]));
        assert_ne!(ExprKind::Boolean(true), ExprKind::Boolean(false));
        assert_ne!(ExprKind::Null, ExprKind::This);
        // A string value may hold an unpaired surrogate, which is the whole reason it is not a
        // `String` (DR-0004).
        assert_eq!(
            ExprKind::String(vec![0xd800]),
            ExprKind::String(vec![0xd800])
        );
    }
}
