//! Function definitions (ECMAScript §15.2).
//!
//! One type for both forms, because they differ in where they may stand and in almost nothing
//! else. A `FunctionDeclaration` is a `Declaration`, so it belongs to a `StatementList` and never
//! to a `Statement` — `while (x) function f() {}` has no derivation — and it must be named. A
//! `FunctionExpression` may stand wherever a value may, and its name is optional and visible only
//! to itself.
//!
//! # Where a function's name goes is not where it is written
//!
//! At the top level of a script or of another function body, a `FunctionDeclaration` is
//! *var-scoped*; inside a block it is *lexically* scoped. §8.2.10 and §8.2.12 are what say so —
//! the `TopLevel` variants of the two name operations put a `HoistableDeclaration` on the opposite
//! side from the plain ones — and the difference is observable:
//!
//! ```text
//! function f() {} function f() {}       fine at the top level, both being var-like
//! let f; function f() {}                a collision, the function being var-like
//! { let f; function f() {} }            a collision, both being lexical
//! ```
//!
//! # A function body is where several walks stop
//!
//! §8.3's label operations and `VarDeclaredNames` are all defined to stop at a
//! `FunctionStatementList`, so `while (1) { function f() { break; } }` is a Syntax Error: the
//! `break` cannot see the loop it looks like it is in. Every rule those operations serve is
//! therefore asked again, from scratch, of each function body.

use super::{Binding, BindingElement, BindingName, Expr, Stmt};
use crate::span::Span;

/// A `FunctionDeclaration` or `FunctionExpression` (§15.2), a `GeneratorDeclaration` or
/// `GeneratorExpression` (§15.5), or the function half of a `MethodDefinition` (§15.4).
///
/// One type for four productions because they differ in exactly one bit of syntax — the `*` — and
/// in what that bit does to the grammar parameters. Everything the tree holds is the same.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The name. Required of a declaration and optional of an expression, where it names the
    /// function to its own body and to nothing outside. A method has none: its name is its key's.
    pub name: Option<BindingName>,
    /// What it takes.
    pub parameters: FormalParameters,
    /// The `FunctionStatementList`, which is a scope of its own and a boundary for several of
    /// the static semantics — see the module documentation.
    pub body: Box<[Stmt]>,
    /// Whether the `function` was preceded by `async` (§15.8).
    ///
    /// What it changes is the `[Await]` grammar parameter over the parameters and the body. It is
    /// independent of `is_generator`: all four combinations are productions of their own, the
    /// two together being §15.6's async generator.
    pub is_async: bool,
    /// Whether a `*` followed the `function`, making this a generator (§15.5).
    ///
    /// What it changes is the `[Yield]` grammar parameter over the parameters and the body: with
    /// it set, `yield` is a `YieldExpression` and is not an identifier anywhere within. It is one
    /// bit rather than a separate type because that is genuinely all the syntax difference there
    /// is; the runtime difference is enormous and is M3's problem.
    pub is_generator: bool,
    /// `function` through the closing brace.
    pub span: Span,
}

/// An `ArrowFunction` (§15.3).
///
/// Not a [`Function`]: it has no name, and its body may be a single expression rather than a
/// statement list. What it shares is the parameters, which are the same `BindingElement`s — but
/// `UniqueFormalParameters`, so unlike a plain function's they may never repeat.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowFunction {
    /// Whether `async` preceded it (§15.9).
    ///
    /// Unlike a [`Function`] there is no generator form: `async* () => {}` is not a production,
    /// an arrow having no `yield` of its own to suspend at.
    pub is_async: bool,
    /// What it takes.
    pub parameters: FormalParameters,
    /// What it does.
    pub body: ArrowBody,
    /// The parameters through the end of the body.
    pub span: Span,
}

/// `ConciseBody` (§15.3) — the two shapes an arrow's body may take.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrowBody {
    /// `a => b` — an `ExpressionBody`, whose value is returned.
    ///
    /// `ConciseBody : [lookahead ≠ {] ExpressionBody`, which is why `a => ({})` needs its
    /// parentheses: a `{` opens a block, so an object literal has to be told apart by the author.
    Expression(Box<Expr>),
    /// `a => { … }` — an ordinary `FunctionBody`, `return` and all.
    Block(Box<[Stmt]>),
}

/// A `ClassDeclaration` or `ClassExpression` (§15.7).
#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    /// The name. Required of a declaration, and optional of an expression.
    pub name: Option<BindingName>,
    /// `extends …`, whose presence is what decides whether `super(…)` may be called.
    pub heritage: Option<Box<Expr>>,
    /// The body, `;` elements dropped — they declare nothing and mean nothing.
    pub elements: Box<[ClassElement]>,
    /// `class` through the closing brace.
    pub span: Span,
}

/// One element of a class body (§15.7), other than the `;` that declares nothing.
///
/// Private names are the remaining `ClassElementName` alternative and are not here yet.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassElement {
    /// `m() {}`, `get a() {}`, `static *m() {}` — a `MethodDefinition`.
    Method(ClassMethod),
    /// `a;` or `a = 1;` — a `FieldDefinition`, which is a property of each *instance* rather
    /// than of the prototype, and so is not a method however it is written.
    Field(ClassField),
    /// `static { … }` — a `ClassStaticBlock`, which has no name at all.
    StaticBlock(ClassStaticBlock),
}

impl ClassElement {
    /// Whether this is the class's constructor (§15.7.3).
    ///
    /// Only a method can be: §15.7.1 refuses a field called `constructor` outright, and a static
    /// block has no name to be it with.
    pub fn is_constructor(&self) -> bool {
        match self {
            Self::Method(method) => {
                ClassMethod::names_the_constructor(&method.key, method.kind, method.is_static)
            }
            Self::Field(_) | Self::StaticBlock(_) => false,
        }
    }
}

/// One method of a class body (§15.7).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassMethod {
    /// What names it.
    pub key: super::PropertyKey,
    /// Whether it is a method, a getter or a setter.
    pub kind: super::MethodKind,
    /// The function itself, which is never named — a method's name is the key's.
    pub function: Box<Function>,
    /// Whether it was written with `static`.
    pub is_static: bool,
    /// Where the name was written, for the early errors about which name it is.
    pub key_span: Span,
}

impl ClassMethod {
    /// Whether a method with these parts is the class's constructor (§15.7.3).
    ///
    /// The parser needs the answer while it is still reading the body — `super(…)` is legal in
    /// there exactly when this is true of a derived class — and one definition of the rule is
    /// worth the slightly awkward signature.
    ///
    /// A static method is never the constructor, however it is named, and neither is an accessor:
    /// §15.7.1 refuses that outright rather than treating it as an ordinary method.
    pub fn names_the_constructor(
        key: &super::PropertyKey,
        kind: super::MethodKind,
        is_static: bool,
    ) -> bool {
        !is_static && kind == super::MethodKind::Normal && key_is(key, "constructor")
    }
}

/// One field of a class body (§15.7).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassField {
    /// What names it.
    pub key: super::PropertyKey,
    /// `= …`, if one was written. Absent leaves the property `undefined`, which is not the same
    /// as the field not existing.
    pub initializer: Option<Box<Expr>>,
    /// Whether it was written with `static`.
    pub is_static: bool,
    /// Where the name was written, for the early errors about which name it is.
    pub key_span: Span,
}

/// A `static { … }` block (§15.7).
///
/// Its body is a `StatementList[~Yield, +Await, ~Return]` — so no `return`, and `await` is a
/// keyword there purely so that §15.7.1 can forbid it outright. There is nothing to suspend into:
/// the block runs once, while the class is being defined.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassStaticBlock {
    /// What runs.
    pub body: Box<[Stmt]>,
    /// `static` through the closing brace.
    pub span: Span,
}

/// Whether `key`'s `PropName` (§15.7.3) is the literal `name`.
///
/// The rule is about `PropName`, not about how the name was written — so `"constructor"() {}` is
/// the constructor exactly as `constructor() {}` is. A computed key is not: `PropName` of one is
/// not known until it runs, which is how a class gets a method called `constructor` at all. A
/// numeric key has a `PropName` too, and it is never either of the two names this asks about.
pub(crate) fn key_is(key: &super::PropertyKey, name: &str) -> bool {
    match key {
        super::PropertyKey::Identifier(text) => &**text == name,
        super::PropertyKey::String(units) => units.iter().copied().eq(name.encode_utf16()),
        // A private name is not a property name at all, so it is never either of the two
        // names this asks about — `#constructor` has its own rule, on the name itself.
        super::PropertyKey::Number(_)
        | super::PropertyKey::Computed(_)
        | super::PropertyKey::Private(_) => false,
    }
}

/// `FormalParameters` (§15.1).
#[derive(Debug, Clone, PartialEq)]
pub struct FormalParameters {
    /// The parameters in order, each a `BindingElement` — so a pattern and a default are both
    /// ordinary here, and the same types serve as in a declaration.
    pub items: Box<[BindingElement]>,
    /// `...a` or `...[a]`, which is last or absent and takes no default.
    pub rest: Option<Box<Binding>>,
    /// The parentheses and everything between them.
    pub span: Span,
}

impl FormalParameters {
    /// `IsSimpleParameterList` (§15.1.4) — whether every parameter is a plain name.
    ///
    /// A rest parameter, a default, or a pattern all make it false, and the answer decides two
    /// rules that nothing else does: a non-simple list may not repeat a name (§15.1.1), and a
    /// function whose body opens with `"use strict"` may not have one at all (§15.2.1). Both
    /// exist for the same reason — the parameters of a non-simple list are initialised by running
    /// code, and running code cannot be told what it is being run for.
    pub fn is_simple(&self) -> bool {
        self.rest.is_none()
            && self
                .items
                .iter()
                .all(|item| item.default.is_none() && matches!(item.target, Binding::Identifier(_)))
    }
}
