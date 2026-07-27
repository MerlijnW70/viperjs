//! The syntax tree.
//!
//! Every node is a value that owns what it means and carries a [`Span`] saying where it came
//! from — see `decisions/DR-0005-ast-owns-its-data-and-carries-spans.md` for why those are two
//! separate decisions and why the span is never allowed to become the second copy of the data.
//!
//! The tree grows one grammar slice at a time, so what is here is what the parser can build
//! today: a `Script` of statements, and expressions down to `PrimaryExpression`.

use crate::span::Span;

/// A whole `Script` (§16.1) — what a source text parses to.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// The statements it contains, in order. Empty for an empty source.
    pub body: Box<[Stmt]>,
    /// The whole source text, so a diagnostic about the script itself has somewhere to point.
    pub span: Span,
}

/// A statement, with where it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    /// Which statement this is, and its contents.
    pub kind: StmtKind,
    /// The source it covers, including the semicolon when one was written — and not including
    /// one that was not, since an inserted semicolon has no source to point at.
    pub span: Span,
}

/// What a statement is.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// `{ … }` (§14.2) — a block, which is a scope as well as a grouping.
    Block(Box<[Stmt]>),
    /// `;` (§14.4). Not the same thing as an omitted semicolon: this one was written.
    Empty,
    /// An expression evaluated for its effect (§14.5).
    ///
    /// Boxed for the same reason [`ExprKind::RegExp`] is: an `Expr` inline would make `StmtKind`
    /// twice the size of any other variant, and statements nest — so every level of `{ { { … } } }`
    /// would carry it on the parser's stack.
    Expression(Box<Expr>),
    /// `debugger;` (§14.16).
    Debugger,
    /// `var`, `let` or `const` (§14.3).
    ///
    /// Boxed for the reason the others are: a declaration is a keyword and a list, and inline
    /// that is wider than every other statement — which every level of `{ { { … } } }` would
    /// carry on the parser's stack.
    Declaration(Box<Declaration>),
    /// `if (…) … else …` (§14.6). The alternate is absent when no `else` was written.
    If(Box<IfStatement>),
    /// `while (…) …` (§14.7.3).
    While(Box<WhileStatement>),
    /// `do … while (…);` (§14.7.2).
    DoWhile(Box<DoWhileStatement>),
    /// `throw …;` (§14.14). Always has a value — there is no argument-less form.
    Throw(Box<Expr>),
    /// `for (…; …; …) …` (§14.7.4).
    For(Box<ForStatement>),
    /// `for (… in …) …` and `for (… of …) …` (§14.7.5).
    ForInOf(Box<ForInOfStatement>),
    /// `switch (…) { case …: … }` (§14.12).
    Switch(Box<SwitchStatement>),
    /// `try { … } catch { … } finally { … }` (§14.15).
    Try(Box<TryStatement>),
    /// `break;` (§14.9), without a label.
    Break,
    /// `continue;` (§14.8), without a label.
    Continue,
}

/// `for ( … ; … ; … ) Statement` (§14.7.4) — the three-part loop.
///
/// The `for`-`in` and `for`-`of` forms are a different production (`ForInOfStatement`) and will be
/// a different variant: they share a keyword and nothing else, having one head clause where this
/// has three.
#[derive(Debug, Clone, PartialEq)]
pub struct ForStatement {
    /// Run once before the loop. Absent when the header begins with its first `;`.
    pub init: Option<ForInit>,
    /// Evaluated before each iteration. Absent means "always true" — `for (;;)` is the endless
    /// loop, and there is no test to be true rather than a test that is.
    pub test: Option<Expr>,
    /// Evaluated after each iteration.
    pub update: Option<Expr>,
    /// What runs each time round.
    pub body: Stmt,
}

/// The first clause of a three-part `for` header (§14.7.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    /// `for (a = 0; …)` — an expression, evaluated and discarded.
    Expression(Box<Expr>),
    /// `for (var a = 0; …)`, `for (let a = 0; …)`, `for (const a = 0; …)`.
    ///
    /// A lexical one is its own scope, which is why `let a; for (let a;;);` is not a
    /// redeclaration — and why §14.7.4.1 has to state separately that the body may not `var` the
    /// same name.
    Declaration(Box<Declaration>),
}

/// `for ( … in … ) Statement` or `for ( … of … ) Statement` (§14.7.5).
///
/// A different production from [`ForStatement`] rather than a variant of it: this has one head
/// clause where that has three, and the two share only a keyword.
#[derive(Debug, Clone, PartialEq)]
pub struct ForInOfStatement {
    /// Which of the two loops this is.
    pub kind: ForInOfKind,
    /// What each value is assigned to, or the name each value binds.
    pub left: ForInOfTarget,
    /// What is iterated. An `Expression` for `in` and an `AssignmentExpression` for `of`, which
    /// is why `for (a in b, c)` parses and `for (a of b, c)` does not.
    pub right: Expr,
    /// What runs for each value.
    pub body: Stmt,
}

/// Which of the two `ForInOfStatement` loops (§14.7.5).
///
/// They differ at runtime in more than iteration order — one walks enumerable string keys, the
/// other drives an iterator — so this is in the tree rather than being recovered from the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForInOfKind {
    /// `for (a in b)`, over enumerable property keys.
    In,
    /// `for (a of b)`, over an iterator.
    Of,
}

/// The left of a `for`-`in` or `for`-`of` header (§14.7.5).
#[derive(Debug, Clone, PartialEq)]
pub enum ForInOfTarget {
    /// `for (a.b of c)` — an existing target, assigned to on each iteration.
    Expression(Box<Expr>),
    /// `for (let a of b)` — a fresh binding, which for a lexical declaration is a fresh one each
    /// time round. Always exactly one declarator with no initialiser: `ForBinding` is singular
    /// and the grammar gives it no `Initializer`.
    Declaration(Box<Declaration>),
}

/// `switch ( Expression ) CaseBlock` (§14.12).
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchStatement {
    /// The value each `case` is compared against, with `===`.
    pub discriminant: Expr,
    /// The clauses, in source order, `default` among them wherever it was written.
    ///
    /// One flat list rather than the grammar's `CaseClauses_opt DefaultClause CaseClauses_opt`,
    /// because that shape exists to say "at most one `default`" and nothing else — the clauses
    /// run in source order regardless of which side of it they fall. What the flat list gives up
    /// is that guarantee, so the parser states it instead.
    pub cases: Box<[SwitchCase]>,
}

/// One `CaseClause` or the `DefaultClause` of a `switch` (§14.12).
///
/// A clause is **not** a scope. The whole `CaseBlock` is one, which is why
/// `case 1: let a; case 2: let a;` is a redeclaration and `case 1: { let a; } case 2: { let a; }`
/// is not — the braces in the second are doing the work.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    /// What this clause matches. `None` for `default`, which matches nothing and is jumped to
    /// only when no `case` matched.
    pub test: Option<Expr>,
    /// The clause's `StatementList`, which may be empty — a clause that falls straight through.
    pub body: Box<[Stmt]>,
    /// `case` or `default` through the last statement of the clause.
    pub span: Span,
}

/// `try Block Catch`, `try Block Finally`, or all three (§14.15).
///
/// The grammar has no `try Block` alone, so at least one of the two is always present — but which
/// one is not something the type can say, and a parser that forgot to check would build a
/// perfectly typed statement that no source could produce.
#[derive(Debug, Clone, PartialEq)]
pub struct TryStatement {
    /// The guarded `Block`, as its statement list. Its own scope, so it is kept as a list rather
    /// than as a nested [`StmtKind::Block`] that would add a second one.
    pub block: Box<[Stmt]>,
    /// The `Catch`, if there is one.
    pub handler: Option<CatchClause>,
    /// The `Finally` block, if there is one.
    pub finalizer: Option<Box<[Stmt]>>,
}

/// `catch ( CatchParameter ) Block`, or `catch Block` (§14.15).
#[derive(Debug, Clone, PartialEq)]
pub struct CatchClause {
    /// The name the thrown value is bound to. `None` for the binding-less form of ES2019, which
    /// is what to write when the value is not wanted — not the same as binding an unused name,
    /// since no binding is created at all.
    pub parameter: Option<CatchParameter>,
    /// The handler's `Block`, as its statement list.
    pub body: Box<[Stmt]>,
    /// `catch` through the closing brace.
    pub span: Span,
}

/// The name a `catch` binds its thrown value to (§14.15).
///
/// A `BindingIdentifier` only. The `BindingPattern` form — `catch ([a, b])` — arrives with
/// destructuring, and brings with it the one early error of §14.15.1 that a single name cannot
/// break: that the bound names may not repeat.
#[derive(Debug, Clone, PartialEq)]
pub struct CatchParameter {
    /// The bound name, with any `\u` escapes resolved.
    pub name: Box<str>,
    /// The name alone.
    pub span: Span,
}

/// `if ( Expression ) Statement else Statement` (§14.6).
///
/// Boxed into [`StmtKind::If`] rather than inlined, as every compound statement here is: three
/// statement-sized fields inline would make `StmtKind` several times the width of its next
/// largest variant, and the parser holds one on the stack per level of nesting.
#[derive(Debug, Clone, PartialEq)]
pub struct IfStatement {
    /// What decides which branch runs.
    pub test: Expr,
    /// The branch taken when the test is truthy.
    pub consequent: Stmt,
    /// The branch taken otherwise. `None` when there is no `else` — which is not the same as an
    /// empty statement, though the two behave alike, because only one of them was written.
    pub alternate: Option<Stmt>,
}

/// `while ( Expression ) Statement` (§14.7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct WhileStatement {
    /// Evaluated before each iteration, including the first.
    pub test: Expr,
    /// What runs while it holds.
    pub body: Stmt,
}

/// `do Statement while ( Expression ) ;` (§14.7.2).
///
/// Distinct from [`WhileStatement`] rather than a flag on it: the body runs before the test is
/// ever evaluated, so the two differ in what they do and not merely in how they were written.
#[derive(Debug, Clone, PartialEq)]
pub struct DoWhileStatement {
    /// What runs at least once.
    pub body: Stmt,
    /// Evaluated after each iteration.
    pub test: Expr,
}

/// A `var`, `let` or `const` declaration (§14.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    /// Which of the three keywords introduced it.
    pub kind: DeclarationKind,
    /// The names it binds, in order. Never empty — the grammar needs at least one.
    pub declarators: Box<[Declarator]>,
}

/// Which keyword a declaration was written with.
///
/// The three differ in scope and in when their bindings become usable, none of which the parser
/// decides — but the early errors of §14.3.1.1 apply to two of them and not the third, which is
/// why the distinction is in the tree rather than left to the compiler to rediscover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclarationKind {
    /// `var` — function-scoped, and not a lexical declaration.
    Var,
    /// `let` — block-scoped.
    Let,
    /// `const` — block-scoped, and its bindings must be initialised where they are declared.
    Const,
}

impl DeclarationKind {
    /// How it is written.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Var => "var",
            Self::Let => "let",
            Self::Const => "const",
        }
    }

    /// Whether this is a `LexicalDeclaration` (§14.3.1) rather than a `VariableStatement`.
    ///
    /// The two early errors about `let` as a name and about duplicate names apply to the lexical
    /// forms only: `var a, a;` is perfectly legal and `let a, a;` is not.
    pub fn is_lexical(&self) -> bool {
        matches!(self, Self::Let | Self::Const)
    }
}

/// One name bound by a declaration, with what it is initialised to.
#[derive(Debug, Clone, PartialEq)]
pub struct Declarator {
    /// The bound name, with any `\u` escapes resolved.
    pub name: Box<str>,
    /// What it is initialised to, if anything. Absent is legal for `var` and `let`, and a
    /// Syntax Error for `const` (§14.3.1.1).
    pub initializer: Option<Box<Expr>>,
    /// The name alone. Kept apart from [`Declarator::span`] because the early errors of §14.2.1
    /// and §16.1.1 are about `BoundNames`, and a caret under `a` says more than one under
    /// `a = someLongExpression()`.
    pub name_span: Span,
    /// The name and the initialiser together.
    pub span: Span,
}

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
    /// `object.name` (§13.3).
    ///
    /// The name is an `IdentifierName`, not an `Identifier` — so `a.if` and `a.default` are
    /// ordinary property accesses, and a reserved word is only reserved where a *binding* could
    /// stand.
    Member {
        /// What is being accessed.
        object: Box<Expr>,
        /// The property name, with any `\u` escapes already resolved.
        ///
        /// `Box<str>` and not `String` because it is never appended to — and because the eight
        /// bytes are not free: `ExprKind` is as large as its largest variant, and that size is
        /// paid once per level of nesting on the parser's stack. The test below keeps it honest.
        property: Box<str>,
    },
    /// `object[property]` (§13.3), where the property is computed rather than written.
    ComputedMember {
        /// What is being accessed.
        object: Box<Expr>,
        /// The expression giving the property key.
        property: Box<Expr>,
    },
    /// `callee(arguments)` (§13.3).
    Call {
        /// What is called.
        callee: Box<Expr>,
        /// The arguments, in order.
        arguments: Box<[Expr]>,
    },
    /// `new callee(arguments)` (§13.3), including the argument-less `new callee`.
    ///
    /// The two spell the same node: §13.3 gives `new MemberExpression Arguments` and
    /// `new NewExpression`, and the second means the same as the first with none — which is why
    /// `new a` and `new a()` construct alike, while `new a.b()` gives the arguments to `new`
    /// rather than to `a.b`.
    New {
        /// What is constructed.
        callee: Box<Expr>,
        /// The arguments, in order. Empty when none were written.
        arguments: Box<[Expr]>,
    },
    /// `++a` or `a++` (§13.4).
    Update {
        /// Which operator.
        operator: UpdateOperator,
        /// Whether it was written before the operand rather than after it. The two differ in
        /// what they evaluate to, not in what they do.
        prefix: bool,
        /// What is incremented or decremented. §13.4.1 requires this to be a valid assignment
        /// target, which the parser has already checked.
        argument: Box<Expr>,
    },
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
    /// `test ? consequent : alternate` (§13.14).
    Conditional {
        /// The condition.
        test: Box<Expr>,
        /// Evaluated when the condition is truthy.
        consequent: Box<Expr>,
        /// Evaluated when it is not.
        alternate: Box<Expr>,
    },
    /// An assignment (§13.15).
    ///
    /// One node for all sixteen operators rather than a second node for the three that
    /// short-circuit: unlike [`ExprKind::Binary`] against [`ExprKind::Logical`], the shape is
    /// identical — a target and a value — and the difference lives entirely in the operator,
    /// which a compiler has to look at anyway. [`AssignmentOperator::short_circuits`] asks it.
    Assignment {
        /// Which operator.
        operator: AssignmentOperator,
        /// What is assigned to. §13.15.1 requires this to be a valid assignment target, which
        /// the parser has already checked.
        target: Box<Expr>,
        /// What is assigned.
        value: Box<Expr>,
    },
    /// `a, b, c` (§13.16) — the comma operator.
    ///
    /// Held flat rather than as nested pairs. The grammar's recursion is on the left, so pairs
    /// would nest once per comma and a long list would nest deeply for no reason; evaluation is
    /// left to right with the last value as the result either way.
    Sequence(Box<[Expr]>),
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
        // Statements nest too — `{ { { … } } }` recurses once per brace — so the same rule
        // applies to them, with more room to spare because there are fewer of them.
        assert!(
            size_of::<StmtKind>() <= 24,
            "StmtKind grew to {} bytes — box the variant that did it",
            size_of::<StmtKind>()
        );
        assert!(
            size_of::<Stmt>() <= 32,
            "Stmt is {} bytes",
            size_of::<Stmt>()
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
