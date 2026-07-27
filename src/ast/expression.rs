//! Expressions (ECMAScript §13).
//!
//! [`Expr`] carries one thing beyond its kind and its span: whether it was written inside
//! parentheses. That is not decoration — several rules turn on it, and they are named on
//! [`Expr::new`], which is the one place that decides what "not parenthesized" means.

use super::{ArrayElement, PropertyDefinition, RegExpLiteral, TemplateLiteral};
use super::{
    AssignmentOperator, AssignmentTarget, BinaryOperator, LogicalOperator, UnaryOperator,
    UpdateOperator,
};
use crate::span::Span;

/// One entry of an `ArgumentList` (§13.3).
///
/// Two alternatives and no third: unlike an `ArrayLiteral` an argument list has no elision, so
/// `f(,)` and `f(a,,b)` have no derivation where `[,]` and `[a,,b]` do. A trailing comma is
/// allowed and leaves nothing behind, `Arguments : ( ArgumentList , )` being its own production.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    /// An ordinary `AssignmentExpression`.
    Value(Expr),
    /// `...a` — a spread, which contributes however many arguments it turns out to have.
    Spread(Expr),
}

/// A `YieldExpression` (§15.5).
///
/// Three productions in one node, because two of them differ only by a flag and the third only by
/// the operand being absent:
///
/// ```text
/// YieldExpression[In, Await] :
///   yield
///   yield [no LineTerminator here] AssignmentExpression[?In, +Yield, ?Await]
///   yield [no LineTerminator here] * AssignmentExpression[?In, +Yield, ?Await]
/// ```
///
/// It is an `AssignmentExpression` and nothing tighter, which is the whole reason
/// `1 + yield` and `yield ? a : b` have no derivation: both operators want an operand narrower
/// than one.
#[derive(Debug, Clone, PartialEq)]
pub struct YieldExpression {
    /// What is yielded, or nothing for a bare `yield`.
    ///
    /// Absent when a line terminator followed the `yield`, or when the next token could not begin
    /// an `AssignmentExpression` — the two ways the grammar's first production wins.
    pub argument: Option<Box<Expr>>,
    /// Whether a `*` followed, making this `yield*` — delegation to another iterable.
    ///
    /// Never true without an argument: `yield*` with nothing after it has no derivation.
    pub delegate: bool,
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
    /// An expression that was not written inside parentheses.
    ///
    /// Every construction site that is not [`Expr::in_parentheses`] goes through here, so the one
    /// place that decides what "not parenthesized" means is this one. That matters more than the
    /// three lines it saves: whether an expression was bracketed changes what several rules do to
    /// it — §13.6 lets `(-a) ** b` through where `-a ** b` has no derivation, §13.13 lets
    /// `(a ?? b) || c` through, §13.15.1 lets `(a) = 1` through — so a site that set the flag
    /// wrongly would be a rule quietly failing somewhere else.
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            kind,
            span,
            parenthesized: false,
        }
    }

    /// The same expression, marked as having been written inside parentheses.
    ///
    /// The span grows to include them, because that is what a reader points at: in `((a + b))`,
    /// the construct a diagnostic should underline is the whole bracketed text.
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
        /// Whether the property was a `PrivateIdentifier` (§13.3).
        ///
        /// `a.#b`. `property` holds the name without its `#`, as everywhere else — the `#` is
        /// punctuation of the production and not part of the name.
        private: bool,
        /// Whether this link was written `?.` rather than `.` (§13.3).
        ///
        /// Only ever true inside an [`ExprKind::OptionalChain`], which is where the
        /// short-circuiting *stops*: `a?.b.c` short-circuits the whole chain and `(a?.b).c` does
        /// not, and the two are told apart by where the wrapper sits rather than by this flag.
        optional: bool,
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
        /// Whether this link was written `?.[` rather than `[` (§13.3).
        optional: bool,
        /// What is being accessed.
        object: Box<Expr>,
        /// The expression giving the property key.
        property: Box<Expr>,
    },
    /// `callee(arguments)` (§13.3).
    Call {
        /// Whether this link was written `?.(` rather than `(` (§13.3).
        optional: bool,
        /// What is called.
        callee: Box<Expr>,
        /// The arguments, in order.
        arguments: Box<[Argument]>,
    },
    /// An `OptionalExpression` (§13.3) — a chain containing at least one `?.`.
    ///
    /// The wrapper marks where short-circuiting ends, which is information the per-link flags do
    /// not carry: `a?.b.c` gives up on the whole thing when `a` is nullish, while `(a?.b).c` gives
    /// up only on the part inside the parentheses and then reads `.c` of `undefined`. It is also
    /// what makes `a?.b = c` and `new a?.b` refusals rather than special cases — an
    /// `OptionalExpression` is neither a `MemberExpression` nor an assignment target.
    OptionalChain(Box<Expr>),
    /// `#a in b` (§13.10) — the one place a private name stands on its own.
    ///
    /// `RelationalExpression : PrivateIdentifier in ShiftExpression`, which exists so that code
    /// can ask whether an object carries a private field without the access throwing. The name is
    /// not an expression: there is no production that lets it stand anywhere else, so it is part
    /// of this node rather than an operand of an ordinary `in`.
    PrivateIn {
        /// The name, without its `#`.
        name: Box<str>,
        /// What is being asked about.
        object: Box<Expr>,
    },
    /// `import(a)` and `import(a, b)` (§13.3) — an `ImportCall`.
    ///
    /// A `CallExpression` and not a `MemberExpression`, which is why `new import(a)` has no
    /// derivation: `new` takes the narrower one, and there is nothing here to construct.
    ImportCall {
        /// The `ModuleSpecifier`, which is an expression here rather than a string literal — that
        /// being the whole point of the form.
        specifier: Box<Expr>,
        /// The second argument, if one was written. The grammar allows exactly two.
        options: Option<Box<Expr>>,
    },
    /// `import.meta` (§13.3) — the other `MetaProperty`, and one only a module may write.
    ImportMeta,
    /// `new.target` (§13.3) — a `MetaProperty`, and not a member access of anything.
    ///
    /// `import.meta` is the other `MetaProperty` and needs the `Module` goal, which arrives with
    /// modules.
    NewTarget,
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
        arguments: Box<[Argument]>,
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
        /// What is assigned to, already checked against §13.15.1 — and already refined, if it
        /// was a literal covering a pattern.
        target: Box<AssignmentTarget>,
        /// What is assigned.
        value: Box<Expr>,
    },
    /// `a, b, c` (§13.16) — the comma operator.
    ///
    /// Held flat rather than as nested pairs. The grammar's recursion is on the left, so pairs
    /// would nest once per comma and a long list would nest deeply for no reason; evaluation is
    /// left to right with the last value as the result either way.
    Sequence(Box<[Expr]>),
    /// `[…]` (§13.2.4). Holes and spreads are elements, so the list is what `length` will be.
    Array(Box<[ArrayElement]>),
    /// `await a` (§15.8) — only inside an async function.
    ///
    /// A `UnaryExpression`, where a `YieldExpression` is an `AssignmentExpression`, so this binds
    /// tighter than nearly everything and that one binds looser than everything. Its operand is
    /// mandatory: §15.8's Note 2 says you must await something.
    Await(Box<Expr>),
    /// `yield`, `yield a` or `yield* a` (§15.5) — only inside a generator.
    Yield(Box<YieldExpression>),
    /// `class … { … }` (§15.7) — an expression, so its name is optional and its own.
    Class(Box<super::Class>),
    /// `super`, which is only ever the head of a `SuperProperty` or a `SuperCall` (§13.3).
    Super,
    /// `` `a${b}c` `` (§13.2.8).
    Template(Box<TemplateLiteral>),
    /// `` f`a` `` (§13.3) — a tagged template, which is a call written without parentheses.
    TaggedTemplate {
        /// What is called. A `MemberExpression` or a `CallExpression`, so tags chain.
        tag: Box<Expr>,
        /// What it is called with.
        quasi: Box<TemplateLiteral>,
    },
    /// `a => b` (§15.3). An `AssignmentExpression` and nothing tighter, so `x + (a) => b` has
    /// no derivation.
    Arrow(Box<super::ArrowFunction>),
    /// `function (…) { … }` (§15.2) — an expression, so its name is optional and its own.
    Function(Box<super::Function>),
    /// `{…}` (§13.2.5), where a `{` could not have begun a statement.
    Object(Box<[PropertyDefinition]>),
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
