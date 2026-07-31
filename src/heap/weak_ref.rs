//! §26.1's `WeakRef` and §26.2's `FinalizationRegistry` — one reference the collector will not
//! follow, and a list of them.
//!
//! # What makes these different from §24.3's weak collections
//!
//! A `WeakMap` entry is an *ephemeron*: its value lives while its key does. These two hold a
//! reference that is simply not followed, and the observable consequence is a `deref` that starts
//! answering `undefined`. Nothing else in the heap has that shape, which is why it is a type of
//! its own rather than another [`crate::heap::CollectionKind`].
//!
//! # Why `deref` can be answered by asking the arena
//!
//! DR-0010 never reuses a slot: sweeping empties it and leaves the hole, and the arena only grows.
//! So an [`crate::heap::ObjectId`] whose slot is empty names something that *was* collected and
//! can never name anything else — and `heap.object(id).is_none()` is exactly §26.1.3.2's question,
//! with no extra bookkeeping and no chance of answering about the wrong object. A collector with a
//! free list would need a generation counter here the same day, which is the cost the module
//! documentation in [`crate::heap::collect`] already records.
//!
//! # Why the cleanup callback never runs
//!
//! §26.2 lets an implementation call it "at any time" and permits it never to be called at all.
//! Choosing a moment means deciding when to collect, which §9.10's note leaves entirely to the
//! implementation and which praxis has not decided — collection is driven by the embedder. So a
//! registry here holds its cells, drops the ones whose target has gone, and calls nothing. That is
//! a conforming choice rather than a gap, and it is the only one available until there is a
//! measured answer to when collection should happen.

use crate::heap::{ObjectId, SymbolId};
use crate::value::Value;

/// A value §7.2.10 `CanBeHeldWeakly` allows — an Object, or an unregistered Symbol.
///
/// A type rather than a checked [`Value`], because every one of §26's slots holds one and none of
/// them can hold anything else. Written as a `Value` instead, each read would have an arm for
/// "some other primitive" that no program could reach: `WeakRef`'s constructor refuses those and
/// so does `register`, so the arm would be a branch no test could distinguish from its absence.
/// Saying it in the type removes the question rather than answering it four times.
///
/// Equality is *identity*, which is what the derive gives: two of these are equal exactly when
/// they are the same handle. That is `SameValue` restricted to what may be held — the general
/// relation needs the heap only because two Strings are equal by their contents, and a String can
/// never be here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holdable {
    /// An ordinary object, which is what almost every weak reference names.
    Object(ObjectId),
    /// A Symbol that `Symbol.for` did not make — a registered one is held by §20.4.2.2's registry
    /// for the life of the realm, so it could never become stale and §7.2.10 keeps it out.
    Symbol(SymbolId),
}

impl Holdable {
    /// The value this stands for, for a caller that has to hand it back to a script.
    #[must_use]
    pub fn as_value(self) -> Value {
        match self {
            Self::Object(id) => Value::Object(id),
            Self::Symbol(id) => Value::Symbol(id),
        }
    }
}

/// One `register` call — §26.2.1.1's `[[Cells]]` entry.
#[derive(Debug, Clone)]
pub struct Cell {
    /// `[[WeakRefTarget]]` — held **weakly**, so registering something does not keep it alive.
    pub target: Holdable,
    /// `[[HeldValue]]` — held strongly, because it is what the callback would be handed.
    ///
    /// §26.2.3.1 step 5 refuses a held value that is the target itself, and this is why: holding
    /// it strongly would keep the target alive through its own registration, and the target could
    /// then never be collected. The check is the specification noticing the same thing.
    pub held: Value,
    /// `[[UnregisterToken]]` — held weakly too, and absent when `register` was given two arguments.
    pub token: Option<Holdable>,
}

/// §26.2's `[[CleanupCallback]]` and `[[Cells]]`.
#[derive(Debug)]
pub struct Registry {
    /// Held strongly and never called — see the module documentation.
    pub cleanup: Value,
    /// The cells, in registration order.
    pub cells: Vec<Cell>,
}

impl Registry {
    /// Drop every cell whose target the collector could not reach — §26.2's liveness rule.
    ///
    /// Answers nothing, because nothing can observe which cells went: a cell goes only when its
    /// target is unreachable, and a program that could still name the target is a program the cell
    /// was kept for. `unregister` with a token whose target has gone answers `false`, which is what
    /// it would have answered had the cell never been registered.
    pub fn retain_cells(&mut self, reachable: impl Fn(Holdable) -> bool) {
        self.cells.retain(|cell| reachable(cell.target));
    }

    /// Take out every cell registered under this token — §26.2.3.4 `unregister`.
    ///
    /// Answers whether any were, which is what the method returns. *Every* cell rather than the
    /// first: one token may be given to any number of `register` calls, and §26.2.3.4 step 5
    /// removes all of them.
    pub fn unregister(&mut self, token: Holdable) -> bool {
        let before = self.cells.len();
        self.cells.retain(|cell| cell.token != Some(token));
        self.cells.len() != before
    }
}

/// §26.1's `[[WeakRefTarget]]` or §26.2's `[[Cells]]`, and which of the two this is.
#[derive(Debug)]
pub enum Weak {
    /// §26.1 — one target, which `deref` answers for as long as it is there.
    Ref(Holdable),
    /// §26.2 — the callback and everything registered against it.
    Registry(Registry),
}
