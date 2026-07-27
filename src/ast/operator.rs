//! The operators, and how each is written (ECMAScript §13.4 – §13.15).
//!
//! Every one of these is a plain enum with an `as_str`, and the pairing matters more than it
//! looks: a diagnostic that names an operator, and a test that reads a tree as text, both go
//! through it — so an operator whose spelling here disagreed with the lexer's would be a bug
//! visible only in messages.

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

/// The update operators of §13.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateOperator {
    /// `++`
    Increment,
    /// `--`
    Decrement,
}

impl UpdateOperator {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Increment => "++",
            Self::Decrement => "--",
        }
    }
}

/// The assignment operators of §13.15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignmentOperator {
    /// `=`
    Assign,
    /// `+=`
    Add,
    /// `-=`
    Subtract,
    /// `*=`
    Multiply,
    /// `/=`
    Divide,
    /// `%=`
    Remainder,
    /// `**=`
    Exponent,
    /// `<<=`
    ShiftLeft,
    /// `>>=`
    ShiftRight,
    /// `>>>=`
    ShiftRightUnsigned,
    /// `&=`
    BitwiseAnd,
    /// `^=`
    BitwiseXor,
    /// `|=`
    BitwiseOr,
    /// `&&=`
    LogicalAnd,
    /// `||=`
    LogicalOr,
    /// `??=`
    NullishCoalescing,
}

impl AssignmentOperator {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Assign => "=",
            Self::Add => "+=",
            Self::Subtract => "-=",
            Self::Multiply => "*=",
            Self::Divide => "/=",
            Self::Remainder => "%=",
            Self::Exponent => "**=",
            Self::ShiftLeft => "<<=",
            Self::ShiftRight => ">>=",
            Self::ShiftRightUnsigned => ">>>=",
            Self::BitwiseAnd => "&=",
            Self::BitwiseXor => "^=",
            Self::BitwiseOr => "|=",
            Self::LogicalAnd => "&&=",
            Self::LogicalOr => "||=",
            Self::NullishCoalescing => "??=",
        }
    }

    /// Whether the value is evaluated only when the target does not already decide the answer.
    ///
    /// True for the three §13.15 gives their own productions — `&&=`, `||=` and `??=`. They also
    /// differ in a way the others do not: `a ||= b` does not assign at all when `a` is truthy,
    /// so it is not sugar for `a = a || b`.
    pub fn short_circuits(&self) -> bool {
        matches!(
            self,
            Self::LogicalAnd | Self::LogicalOr | Self::NullishCoalescing
        )
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
