//! §27.1 — what an iterator remembers between two calls to `next`.
//!
//! # Why this is a field and not a property
//!
//! An Array Iterator's position is an internal slot (§23.1.5.1's `[[ArrayLikeNextIndex]]`), and
//! that is not a detail. A property could be read, written and deleted by any script that got hold
//! of the iterator — so `for (const x of a)` could be made to skip, repeat or never end by code
//! that never touched the array. The position lives here, where nothing in the language can reach
//! it, for the same reason an Array's `length` is exotic rather than ordinary.
//!
//! # One type for four iterators
//!
//! §23.1.5 makes three Array Iterators — over keys, over values, over both — and §22.1.5 makes one
//! over a String's *code points*. They differ in what each step answers and in nothing else: the
//! same position, the same "and now it is done", the same object shape coming back. So this is one
//! record with a [`Iterated`] saying which, rather than four that would drift.
//!
//! The String one is not an Array Iterator with a String in it. §22.1.5.1 walks code *points*, so
//! a surrogate pair is one step and `"😀"` iterates once where `.length` says two — which is the
//! whole reason `for`-`of` over a String is not the same as a `for` over its indices.

use crate::value::Value;

/// What an iterator answers at each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iterated {
    /// §23.1.5.2.1 with `key` — the index, as a Number.
    Keys,
    /// …with `value` — the element. What `for`-`of` over an Array uses.
    Values,
    /// …with `key+value` — a two-element Array of both.
    Entries,
    /// §22.1.5.1 — one code point of a String at a time, as a String.
    Characters,
}

/// Where an iterator has got to — §23.1.5.1 and §22.1.5.1's internal slots, together.
#[derive(Debug, Clone)]
pub struct Iteration {
    /// What is being walked: the array-like object, or the String.
    ///
    /// A `Value` rather than an `ObjectId`, because §22.1.5 iterates a String *primitive* and
    /// §23.1.5 iterates whatever `ToObject` made — the two are not the same kind of thing and
    /// neither is converted to the other.
    pub over: Value,
    /// The next position to read.
    pub at: u64,
    /// Which of the four this is.
    pub kind: Iterated,
    /// Whether it has already run out — §23.1.5.1's `[[ArrayLikeIterationKind]]` being cleared.
    ///
    /// Once set, every further `next` answers done again without looking at the target. That
    /// matters when the target has shrunk: an iterator that ran off the end of an array does not
    /// start finding elements again because the array grew back.
    pub done: bool,
}
