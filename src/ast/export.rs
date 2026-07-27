//! `export` declarations (ECMAScript §16.2.3).
//!
//! # Two names for everything, and only sometimes the same one
//!
//! Every export has a *local* side and an *exported* side, and `export {a as b}` is the shape that
//! shows it: `a` is a binding in this module and `b` is the name another module asks for. The two
//! early errors of §16.2.1.1 are one about each side — the exported names must not repeat, and the
//! local names must actually be declared.
//!
//! With a `FromClause` there is no local side at all. `export {a} from "b"` re-exports someone
//! else's `a` without binding anything here, which is why `export {"a"} from "b"` is ordinary and
//! `export {"a"}` has nothing it could mean.

use super::{Expr, ImportAttribute, ModuleExportName, Stmt};
use crate::span::Span;

/// An `ExportDeclaration` (§16.2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDeclaration {
    /// Which of the six forms this is.
    pub kind: ExportKind,
    /// `export` through the `;`, inserted or not.
    pub span: Span,
}

/// The six shapes of an `ExportDeclaration` (§16.2.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ExportKind {
    /// `export * from "a"` and `export * as n from "a"`.
    ///
    /// Without a name this re-exports everything the other module has, which is why it
    /// contributes no exported names of its own: what they are is not known until link time.
    All {
        /// `as n`, if one was written.
        exported: Option<ModuleExportName>,
        /// The `ModuleSpecifier`, as its string value.
        specifier: Box<[u16]>,
        /// A `WithClause`'s entries, empty when none was written.
        attributes: Box<[ImportAttribute]>,
    },
    /// `export {a, b as c} from "d"` — a re-export, binding nothing here.
    NamedFrom {
        /// What is re-exported, and under what name.
        specifiers: Box<[ExportSpecifier]>,
        /// The `ModuleSpecifier`, as its string value.
        specifier: Box<[u16]>,
        /// A `WithClause`'s entries, empty when none was written.
        attributes: Box<[ImportAttribute]>,
    },
    /// `export {a, b as c}` — of names this module declares.
    Named(Box<[ExportSpecifier]>),
    /// `export var a;`, `export let a = 1;`, `export function f() {}`, `export class C {}`.
    ///
    /// The declaration is an ordinary one and declares its names in the module exactly as it
    /// would without the word in front — the `export` adds an exported name and takes nothing
    /// away.
    Declaration(Stmt),
    /// `export default …`.
    Default(ExportDefault),
}

/// What follows `export default` (§16.2.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ExportDefault {
    /// A `HoistableDeclaration` or a `ClassDeclaration`, either of which may be anonymous here —
    /// `[+Default]` is what makes `export default function () {}` a declaration rather than an
    /// expression statement that could never have been one.
    Declaration(Stmt),
    /// An `AssignmentExpression`, which the lookahead reaches only once the three declaration
    /// forms above have been ruled out.
    Expression(Box<Expr>),
}

/// One entry of an `ExportsList` (§16.2.3).
///
/// Both sides are `ModuleExportName`s, and with a `FromClause` that is all they ever are. Without
/// one, `local` has to name something this module declares — a rule about the finished list rather
/// than about this node, since the declaration may come later in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpecifier {
    /// The name on the left of the `as`, or the only name when there is none.
    pub local: ModuleExportName,
    /// The name another module asks for.
    pub exported: ModuleExportName,
}
