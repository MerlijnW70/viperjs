//! Syntax-directed operations over the tree (ECMAScript §8.2, §8.3).
//!
//! The specification defines its early errors in terms of operations like `VarDeclaredNames` and
//! `ContainsUndefinedContinueTarget`, each written piecewise over grammar productions. These are
//! those operations, and DR-0007 has the argument for computing them here rather than tracking
//! the answers in the parser: the specification is the oracle, and a function whose body can be
//! read next to its section is the only version of it that can be checked.
//!
//! - `names` — `LexicallyDeclaredNames` and `VarDeclaredNames` (§8.2.6, §8.2.8), and the
//!   asymmetry between them that most of the scope rules turn on.
//! - `labels` — `ContainsDuplicateLabels`, `ContainsUndefinedBreakTarget` and
//!   `ContainsUndefinedContinueTarget` (§8.3), and the two nesting rules that go with them.
//!
//! # Every walk here is iterative
//!
//! A `Stmt` already has one recursive path over it that no walk can avoid: its own destructor.
//! `Block(Box<[Stmt]>)` drops its children, which drop theirs, and measured in a debug build
//! against a mebibyte that runs out at about 3,500 levels. So a tree deeper than that cannot
//! safely exist at all, whoever built it — which puts a ceiling on what any argument about walks
//! can be worth, and is comfortably above the parser's cap of 48.
//!
//! What an iterative walk buys, then, is not a bigger number. It is that `Drop` stays the *only*
//! recursive path over the tree, so there is one limit to know rather than one per operation —
//! and these are public functions over a public tree, so each new one would otherwise add its
//! own. [`labels`] shows what it costs when the walk carries state: a set per node, held as
//! indices into one arena rather than as a set cloned down every branch.

mod labels;
mod names;

pub use self::labels::{LabelProblem, LabelProblemKind, first_label_problem};
pub use self::names::{lexically_declared_names, var_declared_names};

use crate::span::Span;

/// A name some construct declares, and where it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredName<'a> {
    /// The bound name, with any `\u` escapes already resolved — `BoundNames` is a `StringValue`,
    /// so two spellings of one name are one name.
    pub name: &'a str,
    /// The name alone, not the initialiser with it. Early errors about these names point here.
    pub span: Span,
}
