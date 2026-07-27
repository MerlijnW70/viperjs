//! Destructuring binding patterns (ECMAScript §14.3.3).
//!
//! # The same shapes as an assignment pattern, and a different grammar
//!
//! `let [a] = b` and `[a] = b` look alike and are not related productions. A
//! [`super::Pattern`] is refined out of a literal by a cover grammar, and its targets are
//! `LeftHandSideExpression`s — anywhere a value can be put. A `BindingPattern` is parsed directly,
//! because a binding position expects one, and its targets are names being *created*:
//!
//! ```text
//! [a.b] = c        legal — a.b is somewhere to put a value
//! let [a.b] = c    not — there is no such thing as declaring `a.b`
//! ```
//!
//! So they are two types rather than one with a flag. Sharing them would mean a tree in which
//! `let [a.b]` is representable, and the parser being the only thing that knows it cannot happen.
//!
//! # A pattern always takes an initialiser
//!
//! `VariableDeclaration : BindingIdentifier Initializer_opt | BindingPattern Initializer` — the
//! `_opt` is on the first alternative only. `var a;` declares `a` as `undefined`; `var [a];` has
//! nothing to take apart, so it has no derivation at all. That holds for `var` and `let` as much
//! as for `const`, which is the one case where a *name* needs one too.
//!
//! # Where the rest elements differ, again
//!
//! `BindingRestElement : ... BindingIdentifier | ... BindingPattern`, and
//! `BindingRestProperty : ... BindingIdentifier`. The same asymmetry the assignment patterns
//! have, for the same reason: the remaining elements of an iterator can be taken apart, and the
//! remaining properties of an object are an object.

use super::{Expr, PropertyKey};
use crate::span::Span;

/// A name being declared, or a pattern of them (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    /// `a` — a `BindingIdentifier`.
    Identifier(BindingName),
    /// `[a]`, `{a}` — an `ArrayBindingPattern` or an `ObjectBindingPattern`.
    Pattern(BindingPattern),
}

impl Binding {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Self::Identifier(name) => name.span,
            Self::Pattern(pattern) => pattern.span(),
        }
    }
}

/// One declared name, and where it was written (§13.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingName {
    /// The name, with any `\u` escapes resolved — `BoundNames` is a `StringValue`.
    pub name: Box<str>,
    /// The name alone. Early errors about `BoundNames` point here.
    pub span: Span,
}

/// An `ArrayBindingPattern` or an `ObjectBindingPattern` (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub enum BindingPattern {
    /// `[a, b]`.
    Array(ArrayBindingPattern),
    /// `{a, b}`.
    Object(ObjectBindingPattern),
}

impl BindingPattern {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Self::Array(pattern) => pattern.span,
            Self::Object(pattern) => pattern.span,
        }
    }
}

/// `ArrayBindingPattern` (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayBindingPattern {
    /// The elements, in order. `None` is an elision — a position deliberately skipped, binding
    /// nothing, which is not the same as one that binds and gets `undefined`.
    pub elements: Box<[Option<BindingElement>]>,
    /// `...a` or `...[a]`, which is last or absent.
    pub rest: Option<Box<Binding>>,
    /// The whole pattern, brackets included.
    pub span: Span,
}

/// `ObjectBindingPattern` (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectBindingPattern {
    /// The properties, in order.
    pub properties: Box<[BindingProperty]>,
    /// `...a`, which is last or absent.
    ///
    /// A name and not a [`Binding`], because `BindingRestProperty : ... BindingIdentifier` — the
    /// remaining properties of an object are an object, and there is nothing to take apart.
    pub rest: Option<BindingName>,
    /// The whole pattern, braces included.
    pub span: Span,
}

/// One property of an object binding pattern (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct BindingProperty {
    /// What names the property being read.
    pub key: PropertyKey,
    /// What it binds, and what to use when it is `undefined`.
    ///
    /// For shorthand — `{a}` and `{a = 1}` — this binds the key's own name, which is what
    /// shorthand means. `SingleNameBinding` is a `BindingIdentifier`, narrower than the
    /// `IdentifierName` a key may be: `{if: a}` binds and `{if}` has no derivation.
    pub value: BindingElement,
}

/// A binding and the value to use when what arrives is `undefined` (§14.3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct BindingElement {
    /// What is bound.
    pub target: Binding,
    /// What to use instead when the value is `undefined`.
    pub default: Option<Box<Expr>>,
}
