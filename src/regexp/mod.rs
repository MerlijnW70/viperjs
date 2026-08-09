//! §22.2 — regular expressions, pattern and matcher both.
//!
//! # Why this is ours to write
//!
//! A regular expression engine is the one substantial algorithm ViperJS cannot borrow: the charter
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
//! # Annex B's grammar, and the one thing still refused
//!
//! §B.1.2 replaces several of §22.2.1's productions when the pattern carries neither `u` nor `v`,
//! and those replacements are what the web is written against: `/}/` is a brace, `/\1/` with no
//! group is a legacy octal escape, `/\8/` is an `8`. DR-0008's reversal covers them — an Annex B
//! rule is in when a *static* fact decides it, and the Unicode flag is read off the literal.
//!
//! **Which of them are built is not listed here on purpose.** Every one is a single production, and
//! a summary in this doc would be a claim about a dozen sites that nothing checks — which is how
//! the paragraph this replaced came to say `\p{…}` and a quantified lookahead were missing long
//! after both landed. `parser.rs` carries the rule at the site that implements it, and the failing
//! entries under `annexB/language/literals/regexp/` in the expectations file are what say the rest.
//!
//! What is left out is `\p{RGI_Emoji}` and its siblings — a property of *strings*, which needs the
//! UCD's emoji sequence tables and nothing else here wants them. It is refused by name rather than
//! as bad syntax, deliberately: it is a legal operand, so calling it a Syntax Error would pass
//! every test asserting that a malformed one must be rejected.

mod matcher;
mod parser;
mod syntax;

pub use self::matcher::{Capture, Match, Matcher};
pub use self::parser::parse;
pub use self::syntax::{
    Assertion, ClassEscape, ClassItem, Error, First, Flags, GroupKind, Node, Pattern,
};
