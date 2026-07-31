//! §22.2.9's RegExp String Iterator — the state `matchAll` walks with.
//!
//! # Why this is state and not a closure over the pattern
//!
//! §22.2.9.1 makes an iterator over a *copy* of the regular expression, so a program that changes
//! the original's `lastIndex` half-way through a `for`-`of` does not disturb the walk — and one
//! that changes the copy's does. Both are observable, so the copy has to be a real object the
//! iterator holds rather than a pattern it remembers.
//!
//! # Why the flags are copied out
//!
//! `[[Global]]` and `[[Unicode]]` are read **once**, when the iterator is made. Re-reading them
//! would let a `flags` getter change what the walk does between steps, which §22.2.9.1 does not
//! allow: it takes them at step 8 and 9 and never asks again.

use crate::heap::{ObjectId, StringId};

/// §22.2.9.1's four slots, and the one that says the walk is over.
#[derive(Debug, Clone, Copy)]
pub struct Matches {
    /// `[[IteratingRegExp]]` — the copy, whose `lastIndex` the walk moves.
    pub regexp: ObjectId,
    /// `[[IteratedString]]`.
    pub subject: StringId,
    /// `[[Global]]`, read once when the iterator was made.
    pub global: bool,
    /// `[[Unicode]]`, likewise — which decides how far an empty match steps.
    pub unicode: bool,
    /// `[[Done]]`. Once set, every further `next` answers the same finished result without
    /// touching the regular expression again.
    pub done: bool,
}
