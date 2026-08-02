//! **praxis** — an embeddable JavaScript engine in safe Rust, with zero runtime dependencies.
//!
//! The public surface is deliberately tiny and will grow one milestone at a time (see
//! `AGENTS.md`). Nothing here is stable until the crate reaches 1.0.
//!
//! # The one invariant that outranks everything
//!
//! **No input may panic.** Every failure a script author can cause — a syntax error, a stack
//! overflow, a 100 MB string literal, a truncated UTF-16 surrogate — is a `Result`, never a
//! panic and never a process exit. An embedder runs untrusted source inside their own binary;
//! a panic there is our bug, categorically, and no amount of "well, that input is absurd"
//! makes it not our bug.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
pub mod builtins;
pub use crate::builtins::date::{local_offset, set_local_offset};
pub mod bigint;
pub mod compile;
pub mod heap;
pub mod lexer;
pub mod parser;
pub mod realm;
pub mod regexp;
pub mod span;
pub mod static_semantics;
mod unicode_id;
mod unicode_id_table;
mod unicode_property;
mod unicode_property_table;
pub mod value;
pub mod vm;

/// The engine version, as reported to embedders (`praxis::VERSION`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Two things about the comments that no compiler checks — see the module for why they are tests.
#[cfg(test)]
mod documentation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_manifest_version() {
        // Not a tautology at the level that matters: it pins that the constant is wired to
        // Cargo's metadata rather than hand-copied, which is how a version string goes stale.
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(!VERSION.is_empty());
    }
}
