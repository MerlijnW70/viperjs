//! §22.2 — regular expressions, pattern and matcher both.
//!
//! # Why this is ours to write
//!
//! A regular expression engine is the one substantial algorithm praxis cannot borrow: the charter
//! forbids runtime dependencies, and ECMAScript's flavour is not any library's. Backreferences and
//! lookbehind put it outside what a finite automaton can decide, `lastIndex` is observable state a
//! program may write to mid-loop, and §22.2.2's semantics are specified as a *backtracking* matcher
//! with continuations — so the specification's own algorithm is the design, and following it is
//! cheaper than being clever.
//!
//! # What is in this module and what is not
//!
//! The pattern grammar (§22.2.1) becomes a [`Node`] tree here, and the matcher walks that tree.
//! Neither knows anything about objects: `RegExp` itself, `lastIndex`, the flags as properties and
//! the four `Symbol` methods are in [`crate::builtins`], because those are about the *object* and
//! this is about the *pattern*. The split is what lets the parser be tested with strings alone.
//!
//! # Two things deliberately not here yet
//!
//! `\p{…}` needs the Unicode property tables, which are generated data under DR-0003 and a slice of
//! their own. Annex B's extensions to the grammar — a lone `]`, a quantifier on an assertion,
//! `\8` as an identity escape — are refused for the same reason DR-0008 gives: Annex B's lexical
//! extensions are in and its syntactic ones are not.

mod matcher;
mod parser;

pub use self::matcher::{Capture, Match, Matcher};
pub use self::parser::{
    Assertion, ClassEscape, ClassItem, Error, Flags, GroupKind, Node, Pattern, parse,
};
