//! Expressions (ECMAScript §13).
//!
//! [`Expr`] carries one thing beyond its kind and its span: whether it was written inside
//! parentheses. That is not decoration — several rules turn on it, and they are named on
//! [`Expr::new`], which is the one place that decides what "not parenthesized" means.

use super::{
    AssignmentOperator, AssignmentTarget, BinaryOperator, LogicalOperator, UnaryOperator,
    UpdateOperator,
};
use crate::span::Span;

/// One element of an array literal (§13.2.4).
///
/// A hole is an element and not an absence: `[, 1]` has two of them, and `[1, ]` has one. The
/// difference is whether a comma had anything before it in its slot, which is the whole content
/// of `Elision` and the one thing about array literals that is easy to get wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElement {
    /// A comma with nothing before it — `[, 1]`, `[1, , 2]`. Reads as `undefined` and is not the
    /// same as one: the index is absent from the array rather than holding that value.
    Hole,
    /// An ordinary element.
    Value(Expr),
    /// `...a` — a `SpreadElement`, which contributes however many elements it turns out to have.
    Spread(Expr),
}

/// One entry of an object literal (§13.2.5).
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyDefinition {
    /// `a: 1` — `PropertyName : AssignmentExpression`, the only production the `__proto__` rule
    /// of §13.2.5.1 counts.
    KeyValue {
        /// What names the property.
        key: PropertyKey,
        /// What it is set to.
        value: Expr,
    },
    /// `{a}` — an `IdentifierReference`, which is narrower than the `IdentifierName` a key may
    /// be: `{if: 1}` is a property and `{if}` has no derivation.
    Shorthand {
        /// The name, which is both the key and the value.
        name: Box<str>,
        /// Where it was written.
        span: Span,
    },
    /// `{a = 1}` — a `CoverInitializedName`, which is **not** a legal object literal.
    ///
    /// §13.2.5.1 says it is always a Syntax Error where an object literal stays one. It is here
    /// because the cover grammar needs it: `({a = 1} = b)` is a pattern, and the `=` that says so
    /// arrives long after this has been parsed. A literal that still holds one when the
    /// expression around it is finished is that Syntax Error — see [`crate::parser`].
    ShorthandWithDefault {
        /// The name, which is both the key and the target.
        name: Box<str>,
        /// What to use when the value is `undefined`.
        default: Box<Expr>,
        /// Where the name was written.
        span: Span,
    },
    /// `a() {}`, `get a() {}`, `set a(v) {}` — a `MethodDefinition` (§15.4).
    Method {
        /// What names the property.
        key: PropertyKey,
        /// Which of the three it is.
        kind: MethodKind,
        /// The function, which is never named: a method's name is the property's.
        function: Box<super::Function>,
    },
    /// `...a`, which stands wherever any other property may.
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

/// A `TemplateLiteral` (§13.2.8) — its literal parts, and the expressions between them.
///
/// `quasis` is always one longer than `expressions`: a template begins and ends with a literal
/// part, even when that part is empty. `` `${a}` `` has two empty ones.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateLiteral {
    /// The literal components, in order.
    pub quasis: Box<[TemplateElement]>,
    /// The substitutions, in order. One fewer than the components.
    pub expressions: Box<[Expr]>,
}

/// One literal component of a template (§12.9.6).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateElement {
    /// `TV`, the cooked value — `None` when the component holds a `NotEscapeSequence`, which is
    /// what the specification means by "undefined" there. Only a tagged template may have one,
    /// and §13.2.8.1 is why.
    pub cooked: Option<Vec<u16>>,
    /// `TRV`, the raw value. Always present, escapes left exactly as written.
    pub raw: Vec<u16>,
    /// The component including its delimiters.
    pub span: Span,
}

/// Which kind of `MethodDefinition` (§15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MethodKind {
    /// `a() {}` — an ordinary method.
    Normal,
    /// `get a() {}`, which takes no parameters.
    Get,
    /// `set a(v) {}`, which takes exactly one.
    Set,
}

/// What names a property (§13.2.5).
///
/// The four source forms are kept apart rather than reduced to one string, because reducing them
/// needs `PropName`, and `PropName` of a `NumericLiteral` is `ToString` of its value — an abstract
/// operation this engine does not have yet. Inventing an approximation would be a bug that only
/// ever showed up in a property name.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKey {
    /// An `IdentifierName`, escapes resolved. Includes every reserved word.
    Identifier(Box<str>),
    /// A `StringLiteral`, as UTF-16 code units (DR-0004) — which may include a lone surrogate,
    /// and so is not a `str`.
    String(Box<[u16]>),
    /// A `NumericLiteral`, as its value.
    Number(f64),
    /// `[ AssignmentExpression ]`, whose name is not known until it runs.
    Computed(Box<Expr>),
}

impl PropertyKey {
    /// Whether this names `__proto__`, for §13.2.5.1.
    ///
    /// A computed key is not asked, the rule being about the other productions; and a numeric key
    /// cannot spell it, `PropName` of a number being the number written out.
    pub fn is_proto(&self) -> bool {
        match self {
            Self::Identifier(name) => &**name == "__proto__",
            Self::String(units) => units.iter().copied().eq("__proto__".encode_utf16()),
            Self::Number(_) | Self::Computed(_) => false,
        }
    }
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
