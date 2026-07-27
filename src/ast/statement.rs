//! Statements, and the declarations that are not statements (ECMAScript §14).
//!
//! One type per production that carries anything, and a [`StmtKind`] variant naming it. The
//! compound ones are boxed into that enum rather than inlined, and always for the same reason:
//! statements nest, the parser holds one per level of nesting, and an enum is as wide as its
//! widest variant — so a fat variant is paid for by every `{ { { … } } }` in
//! [`crate::parser::MAX_NESTING_DEPTH`], in levels.

use super::Expr;
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
    /// `a: …` (§14.13).
    Labelled(Box<LabelledStatement>),
    /// `with (…) …` (§14.11).
    With(Box<WithStatement>),
    /// `break;` or `break a;` (§14.9).
    Break(Option<Box<Label>>),
    /// `continue;` or `continue a;` (§14.8).
    Continue(Option<Box<Label>>),
}

/// `LabelIdentifier : LabelledItem` (§14.13).
#[derive(Debug, Clone, PartialEq)]
pub struct LabelledStatement {
    /// The name given to the statement.
    pub label: Label,
    /// What was labelled. A `Statement`, so never a lexical declaration.
    pub body: Stmt,
}

/// A label, where it is declared or where a jump names it (§14.13, §14.8, §14.9).
///
/// Boxed into [`StmtKind::Break`] and [`StmtKind::Continue`] rather than inlined, because a name
/// and a span inline would make those two the widest variants of a statement — and a jump without
/// a label is much the commoner one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The name, with any `\u` escapes resolved. Nothing here is a terminal, so an escaped
    /// spelling names the same label.
    pub name: Box<str>,
    /// The name alone.
    pub span: Span,
}

/// `with ( Expression ) Statement` (§14.11).
///
/// A Syntax Error in strict code (§14.11.1), which this parser cannot yet tell apart — so it
/// parses everywhere for now, and the day strict mode exists that rule has somewhere to live.
#[derive(Debug, Clone, PartialEq)]
pub struct WithStatement {
    /// The object whose properties become bindings for the body.
    pub object: Expr,
    /// What runs with them in scope.
    pub body: Stmt,
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
