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
//! # What is deliberately not here
//!
//! An *unmapped* arguments object. §10.4.4 makes one for a strict function, or one whose parameter
//! list is not simple, and praxis has neither: the interpreter tracks no strictness yet, and
//! default, rest and destructuring parameters are refused. Every arguments object it can build is
//! a mapped one, so there is one path here rather than two.

use crate::heap::{EnvironmentId, Heap, ObjectId, PropertyKey};
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
        // Index `n` is parameter `n`, because praxis gives a function's parameters the first slots
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

/// The index a key names, if it names one at all.
///
/// Not §10.4.2's array index: an arguments object is not an Array, and §10.4.4 joins the keys
/// `"0"`, `"1"` and so on and nothing else.
///
/// Written back and compared rather than merely parsed, because `"01"` parses as 1 and is not the
/// key `"1"` — a map found by parsing alone would make `arguments["01"]` an alias of the first
/// parameter, which is a property the object does not even have.
///
/// Two allocations per call, and it is called once per property an arguments object is asked for.
/// Reading the digits out of the UTF-16 units directly would need none, and is the sort of thing
/// M8 measures before it changes: the callers all check for a parameter map first, so no ordinary
/// object pays this.
pub(super) fn index_of(heap: &Heap, key: PropertyKey) -> Option<u32> {
    // A Symbol is no index and has no digits — `as_string` answering `None` is that.
    let units = heap.string(key.as_string()?)?;
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    let index: u32 = text.parse().ok()?; // a key that is not a number is not an index, and that is the answer
    (index.to_string() == text).then_some(index)
}
