//! Destructuring assignment patterns (ECMAScript §13.15.5).
//!
//! # These are not literals, and they are written exactly like literals
//!
//! `[a, b] = c` begins with something that reads as an array literal and is not one: `a` and `b`
//! are places to put values rather than values to collect. The specification handles that with a
//! *cover grammar* — the source is parsed as an `ArrayLiteral`, and when an `=` turns up the tree
//! is refined into the `ArrayAssignmentPattern` "that is covered by" it (§13.15.5). This module is
//! what it is refined into, and [`crate::parser`] does the refining.
//!
//! Refining rather than reinterpreting matters because the two grammars are not the same shape:
//!
//! - `{a = 1}` is a pattern and is **not** a literal — §13.2.5.1 says a `CoverInitializedName` is
//!   always a Syntax Error where an object literal stays an object literal.
//! - `[...a, ]` is a literal and is **not** a pattern — an `AssignmentRestElement` is last, and
//!   nothing may follow it, not even a comma.
//!
//! So a tree that kept the literal and left the reading to whoever came later would be a tree
//! that means two things at once. These types mean one.
//!
//! # Where a pattern is stricter than an assignment
//!
//! §13.15.5.1 says a `DestructuringAssignmentTarget` that is not itself a pattern is a Syntax
//! Error unless its `AssignmentTargetType` is **simple** — where §13.15.1 refuses only *invalid*.
//! The difference is the `web-compat` case of §8.6.4, so `f() = 1` is a runtime error on a web
//! host and `[f()] = 1` is a Syntax Error on every host. praxis refuses both, being no web host,
//! but the two are refused for different reasons and the second would still be refused if it were.

use super::{Expr, PropertyKey};
use crate::span::Span;

/// The left of an assignment (§13.15).
///
/// Two cases because the specification has two: an ordinary `LeftHandSideExpression`, and a
/// pattern that was covered by a literal. Only `=` admits the second — `[a] += b` has no
/// derivation, the compound forms taking a `LeftHandSideExpression` and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignmentTarget {
    /// `a`, `a.b`, `a[0]` — something whose `AssignmentTargetType` is simple.
    Simple(Expr),
    /// `[a]`, `{a}` — refined from the literal that covered it.
    Pattern(Pattern),
}

impl AssignmentTarget {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Self::Simple(expr) => expr.span,
            Self::Pattern(pattern) => pattern.span(),
        }
    }
}

/// An `ArrayAssignmentPattern` or an `ObjectAssignmentPattern` (§13.15.5).
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `[a, b]`.
    Array(ArrayPattern),
    /// `{a, b}`.
    Object(ObjectPattern),
}

impl Pattern {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Self::Array(pattern) => pattern.span,
            Self::Object(pattern) => pattern.span,
        }
    }
}

/// `ArrayAssignmentPattern` (§13.15.5).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayPattern {
    /// The elements, in order. `None` is an elision — a position deliberately skipped, which is
    /// not the same as one bound to nothing.
    pub elements: Box<[Option<PatternElement>]>,
    /// `...a`, which is last or absent. Unlike an object's, this target may itself be a pattern:
    /// `[...[a]] = b` is legal where `({...[a]} = b)` is not.
    pub rest: Option<Box<AssignmentTarget>>,
    /// The whole pattern, brackets included.
    pub span: Span,
}

/// `ObjectAssignmentPattern` (§13.15.5).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPattern {
    /// The properties, in order.
    pub properties: Box<[PatternProperty]>,
    /// `...a`, which is last or absent.
    ///
    /// An `Expr` and not an [`AssignmentTarget`], because §13.15.5.1 makes it a Syntax Error for
    /// an `AssignmentRestProperty` target to be an array or object literal — so this one can
    /// never be a pattern, where an array's rest can.
    pub rest: Option<Box<Expr>>,
    /// The whole pattern, braces included.
    pub span: Span,
}

/// One property of an object pattern (§13.15.5).
#[derive(Debug, Clone, PartialEq)]
pub struct PatternProperty {
    /// What names the property being read.
    pub key: PropertyKey,
    /// Where its value goes, and what to use when it is `undefined`.
    ///
    /// For shorthand — `{a}` and `{a = 1}` — this is the key again, which is what shorthand
    /// means. Nothing records *that* it was written that way, because nothing needs to and the
    /// spans already say it: the key and the target were written in the same place.
    pub value: PatternElement,
}

/// A target and the value to use when what arrives is `undefined` (§13.15.5).
///
/// `AssignmentElement : DestructuringAssignmentTarget Initializer_opt`. The default is not a
/// fallback for a missing property but for an `undefined` one, which is a distinction that only
/// shows up at run time — and is why it is kept rather than folded into the target.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternElement {
    /// Where the value goes.
    pub target: AssignmentTarget,
    /// What to use instead when the value is `undefined`.
    pub default: Option<Box<Expr>>,
}
