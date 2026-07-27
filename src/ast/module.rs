//! Modules and the two declarations only they admit (ECMAScript §16.2).
//!
//! # A module is not a script with extra statements
//!
//! `ModuleItem : ImportDeclaration | ExportDeclaration | StatementListItem` — so two of the three
//! things a module body holds are not statements at all, which is why this is a list of items
//! rather than a `Box<[Stmt]>` with two more `StmtKind`s. It also keeps the walks honest: an
//! import declares names, and a walk over statements would never think to ask it.
//!
//! The other difference is quieter and catches people out. §16.2.1.1 asks for the
//! `LexicallyDeclaredNames` of a `ModuleItemList`, not the `TopLevelLexicallyDeclaredNames` a
//! `Script` gets — so a function declared at the top of a module is *lexically* scoped, and
//! `function f() {} function f() {}` is a redeclaration where the same text in a script is fine.

use super::{BindingName, Stmt};
use crate::span::Span;

/// A `Module` (§16.2), which is what [`crate::parser::parse_module`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The `ModuleItemList`, in source order.
    pub body: Box<[ModuleItem]>,
    /// The whole source text.
    pub span: Span,
}

/// One `ModuleItem` (§16.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleItem {
    /// `import …;`
    Import(ImportDeclaration),
    /// Anything a `Script` could also have held.
    Statement(Stmt),
}

/// An `ImportDeclaration` (§16.2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportDeclaration {
    /// What is bound, or nothing for `import "a";` — which imports no names and is written for
    /// the side effect of the module being evaluated.
    pub clause: Option<ImportClause>,
    /// The `ModuleSpecifier`, as its string value. Where it points is the host's business; §16.2
    /// says only that this text is handed to it.
    pub specifier: Box<[u16]>,
    /// A `WithClause`'s entries, empty when none was written.
    pub attributes: Box<[ImportAttribute]>,
    /// `import` through the `;`, inserted or not.
    pub span: Span,
}

/// The five shapes of an `ImportClause` (§16.2.2).
///
/// One variant per alternative rather than three optional fields, because three fields would admit
/// combinations the grammar does not have — a namespace import and a named list together, for one.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportClause {
    /// `import a from "b"`
    Default(BindingName),
    /// `import * as a from "b"`
    Namespace(BindingName),
    /// `import {a, b as c} from "d"`
    Named(Box<[ImportSpecifier]>),
    /// `import a, * as b from "c"`
    DefaultAndNamespace(BindingName, BindingName),
    /// `import a, {b} from "c"`
    DefaultAndNamed(BindingName, Box<[ImportSpecifier]>),
}

/// One entry of an `ImportsList` (§16.2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSpecifier {
    /// The name the other module exports, which is not an identifier in this one and so is not a
    /// binding — `import {if as a} from "b"` is ordinary, and so is `import {"a" as b} from "c"`.
    pub imported: ModuleExportName,
    /// The name this module binds.
    pub local: BindingName,
}

/// A `ModuleExportName` (§16.2.2) — the name as the *other* module spells it.
///
/// Two alternatives, and the string one exists so that a module may export a name no identifier
/// can spell. Neither is a binding here, which is why every reserved word is allowed.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleExportName {
    /// `IdentifierName`, escapes already resolved.
    Identifier(Box<str>),
    /// `StringLiteral`, as its value.
    String(Box<[u16]>),
}

/// One entry of a `WithClause` (§16.2.2) — an import attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportAttribute {
    /// `AttributeKey : IdentifierName | StringLiteral`.
    pub key: ModuleExportName,
    /// The value, which the grammar makes a `StringLiteral` and nothing else.
    pub value: Box<[u16]>,
}
