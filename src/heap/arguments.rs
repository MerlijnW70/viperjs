//! §10.4.4 — the arguments exotic object, and the mapping that makes it exotic.
//!
//! # What is exotic about it
//!
//! Nothing about its shape. It is an ordinary object with an ordinary prototype, a `length`, a
//! `callee`, and a property per argument. What is exotic is that some of those properties are
//! *the same variable* as the parameter of the same position: in
//!
//! ```text
//! function f(a) { arguments[0] = 2; return a; }
//! ```
//!
//! `f(1)` answers `2`. Nothing was copied — `arguments[0]` and `a` are one binding seen through
//! two names, and §10.4.4 calls the link between them the *parameter map*.
//!
//! # Why the map is per index, and one-way
//!
//! Because a program can break it, one index at a time. §10.4.4.2 removes an index's mapping when
//! it is redefined as an accessor or made non-writable, and §10.4.4.5 removes it when the property
//! is deleted. Afterwards the two names are two variables and nothing joins them again — so this
//! is a slot per index rather than a count of how many are still mapped.
//!
//! # The unmapped kind is here too
//!
//! §10.4.4 makes one for a strict function, or one whose parameter list is not simple, and both
//! arrive today: `(function (a) { 'use strict'; a = 99; return arguments[0] })(1)` is `1` where the
//! sloppy spelling is `99`, and a default, a rest or a destructuring parameter answers the same way
//! in either mode. A strict one's `callee` throws rather than answering.
//!
//! This section read "What is deliberately not here", on the grounds that "the interpreter tracks
//! no strictness yet, and default, rest and destructuring parameters are refused". All three
//! conditions passed several milestones ago. The *mapping* is what this file is about and the two
//! kinds differ only in whether it exists — which is why one file still serves both.

use crate::heap::{EnvironmentId, ObjectId};
use crate::value::Value;

/// What a call knows that its arguments object needs — §10.4.4.4 and §10.4.4.6's inputs.
///
/// A struct rather than six more parameters, because six of them in a row is a call whose
/// arguments can be silently swapped. Two are `ObjectId`s and two are `bool`s: the compiler could
/// not tell `callee` from `thrower`, nor `mapped` from anything else.
#[derive(Debug, Clone, Copy)]
pub struct Incoming<'a> {
    /// The call's environment, where the parameters live.
    pub environment: EnvironmentId,
    /// Every argument the call was given, in order.
    pub values: &'a [Value],
    /// How many named parameters the function has — how far the map can reach.
    pub parameters: usize,
    /// The function being called, which a mapped object's `callee` names.
    pub callee: ObjectId,
    /// %ThrowTypeError%, which an unmapped object's `callee` is poisoned with.
    pub thrower: ObjectId,
    /// Whether §15.1.4 calls the parameter list simple, and so whether to join the map.
    pub mapped: bool,
    /// `%Symbol.iterator%` and `%Array.prototype.values%` — §10.4.4.4 step 16.
    ///
    /// One field for the pair because the property cannot be made without both, and `None` for a
    /// realm whose well-known Symbols are not there. Given to a **mapped and an unmapped object
    /// alike**: §10.4.4.6 step 7 says the same thing, which is why `[...arguments]` works in a
    /// strict function and in one with a default parameter.
    pub iteration: Option<(crate::heap::SymbolId, ObjectId)>,
}

/// Which parameter each argument index is the same variable as — §10.4.4's parameter map.
#[derive(Debug, Clone)]
pub struct ArgumentsMap {
    /// The call's environment, where the parameters live.
    pub(super) environment: EnvironmentId,
    /// The slot each index is joined to, in index order. `None` is a link that has been broken.
    ///
    /// Shorter than the argument list when a call passed more arguments than the function
    /// declares: §10.4.4 maps only as far as there are parameters, so `f(1, 2)` on a
    /// one-parameter `f` leaves `arguments[1]` an ordinary property that writes through to
    /// nothing.
    slots: Vec<Option<u32>>,
}

impl ArgumentsMap {
    /// A map over the first `parameters` indices of a call in `environment`.
    pub(super) fn new(environment: EnvironmentId, parameters: usize) -> Self {
        // Index `n` is parameter `n`, because ViperJS gives a function's parameters the first slots
        // of its environment in order — so the map is an identity rather than a table of names.
        let slots = (0..parameters).map(|at| u32::try_from(at).ok()).collect();
        Self { environment, slots }
    }

    /// The call's environment, where the joined parameters live.
    pub(super) fn environment(&self) -> EnvironmentId {
        self.environment
    }

    /// The parameter slot this index is joined to, if it still is.
    pub(super) fn slot(&self, index: u32) -> Option<u32> {
        self.slots.get(index as usize).copied().flatten()
    }

    /// Break the link at this index — §10.4.4.2 step 5 and §10.4.4.5 step 4.
    pub(super) fn unmap(&mut self, index: u32) {
        if let Some(slot) = self.slots.get_mut(index as usize) {
            *slot = None;
        }
    }
}
