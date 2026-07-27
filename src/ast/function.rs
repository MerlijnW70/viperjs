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

use super::{Binding, BindingElement, BindingName, Stmt};
use crate::span::Span;

/// A `FunctionDeclaration` or `FunctionExpression` (§15.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The name. Required of a declaration and optional of an expression, where it names the
    /// function to its own body and to nothing outside.
    pub name: Option<BindingName>,
    /// What it takes.
    pub parameters: FormalParameters,
    /// The `FunctionStatementList`, which is a scope of its own and a boundary for several of
    /// the static semantics — see the module documentation.
    pub body: Box<[Stmt]>,
    /// `function` through the closing brace.
    pub span: Span,
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
