//! §24.1 and §24.2 — what a `Map` and a `Set` hold, and why deleting leaves a hole.
//!
//! # One type for both
//!
//! §24.1's `[[MapData]]` is a List of Records with a `[[Key]]` and a `[[Value]]`; §24.2's
//! `[[SetData]]` is a List of values. They are the same list with the second half unused, and every
//! operation over them — insertion order, the equality, the hole a delete leaves — is the same
//! sentence in both clauses. One record with a [`CollectionKind`] keeps them from drifting.
//!
//! # Why a delete leaves a hole
//!
//! §24.1.3.3 step 3.b sets the entry's key and value to **empty** and does *not* remove the record.
//! That is not tidiness deferred: §24.1.5.1's iterator remembers a position in this list, so
//! removing an entry would move everything after it and the iterator would skip one. An entry
//! deleted while a `for`-`of` is running is passed over, and an entry *added* while one is running
//! is visited — both of which fall out of the list only ever growing.
//!
//! The holes are never compacted. A program that adds and deletes for ever grows this for ever,
//! which is what the specification describes, and the alternative is an iterator that can be made
//! to miss entries by code it never touched.
//!
//! # The equality is not `===`
//!
//! §24.1.3.9 uses `SameValueZero`, which differs from strict equality in exactly one place and from
//! `Object.is` in exactly one other: **`NaN` matches `NaN`**, and `+0` matches `-0`. So a map can
//! be keyed by `NaN` and found again, which `===` would make impossible, and `map.set(-0, 1)` is
//! read back by `map.get(0)`.
//!
//! [`Value::same_value_zero`] is that relation and it is not written again here. It needs the heap,
//! because two Strings are equal by their *contents* and a `Value::String` is an identifier for
//! them — an equality written in terms of that identifier answers `false` for two occurrences of
//! the same literal, which is a `Map` that cannot find a key it was just given.

use crate::heap::Heap;
use crate::value::Value;

/// Which of §24's four collections this is.
///
/// Four rather than two because the weak pair are a different *brand*, not a flag on the same one.
/// §24.3.3.3's `WeakMap.prototype.get` requires a `[[WeakMapData]]` and §24.1.3.6's `Map.prototype
/// .get` requires a `[[MapData]]`, so `Map.prototype.get.call(new WeakMap())` is a TypeError — and
/// making them separate kinds is what gets that right without a second check anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    /// §24.1 — keys with values.
    Map,
    /// §24.2 — values alone, each of which is also its own key.
    Set,
    /// §24.3 — keys with values, and holding a key does not keep it alive.
    WeakMap,
    /// §24.4 — values alone, and holding one does not keep it alive.
    WeakSet,
}

impl CollectionKind {
    /// Whether an entry has a value of its own, or is its own value.
    #[must_use]
    pub fn keyed(self) -> bool {
        matches!(self, Self::Map | Self::WeakMap)
    }

    /// Whether holding a key keeps it reachable.
    ///
    /// The one question the collector asks. A strong collection is *precisely* a thing that keeps
    /// values alive on purpose; a weak one is precisely a thing that does not, and the difference
    /// is unobservable to a program except by running out of memory — which is why it has to be
    /// right in the collector rather than checked by a test.
    #[must_use]
    pub fn weak(self) -> bool {
        matches!(self, Self::WeakMap | Self::WeakSet)
    }

    /// What a diagnostic calls it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Map => "Map",
            Self::Set => "Set",
            Self::WeakMap => "WeakMap",
            Self::WeakSet => "WeakSet",
        }
    }
}

/// `[[MapData]]` or `[[SetData]]`, and which of the two it is.
#[derive(Debug)]
pub struct Collection {
    /// The entries in insertion order, `None` where one was deleted — see the module documentation.
    entries: Vec<Option<(Value, Value)>>,
    /// How many are not holes, so that `size` costs nothing to answer.
    ///
    /// Kept rather than counted, because §24.1.3.10's `get size` is a property access and a program
    /// may read it in a loop. Counting would make that quadratic in a collection that has had
    /// deletions.
    live: usize,
    /// Which collection this is.
    kind: CollectionKind,
}

impl Collection {
    /// An empty one.
    #[must_use]
    pub fn new(kind: CollectionKind) -> Self {
        Self {
            entries: Vec::new(),
            live: 0,
            kind,
        }
    }

    /// Which of the two this is.
    #[must_use]
    pub fn kind(&self) -> CollectionKind {
        self.kind
    }

    /// How many entries it has — §24.1.3.10 and §24.2.3.9's `size`.
    #[must_use]
    pub fn size(&self) -> usize {
        self.live
    }

    /// The next live entry at or after `from`, with the position it was found at.
    ///
    /// What a walk actually wants, and the reason it is here rather than a bound the caller
    /// compares against: past the end and on a hole both answer `None`, so a caller written in
    /// terms of `positions()` would have a comparison that decides nothing — every off-by-one in
    /// it produces the same behaviour, which is what an untestable branch looks like.
    #[must_use]
    pub fn live_from(&self, from: usize) -> Option<(usize, Value, Value)> {
        self.entries
            .iter()
            .enumerate()
            .skip(from)
            .find_map(|(at, entry)| entry.map(|(key, value)| (at, key, value)))
    }

    /// Every live entry in insertion order, for the collector and for `forEach`.
    pub fn live_entries(&self) -> impl Iterator<Item = (Value, Value)> + '_ {
        self.entries.iter().filter_map(|entry| *entry)
    }

    /// Where `key` is, by §7.2.11 `SameValueZero`, or `None` if it is not there.
    ///
    /// Separate from the four operations that use it because it needs the heap and they need to
    /// *change* the heap: a lookup that answered while holding the collection open would be asking
    /// the heap a question with the heap already borrowed to be written. So every method here is a
    /// question first and a change second, which is also how the specification reads.
    ///
    /// A linear scan, which is what the property table started as and for the same reason: it is
    /// obviously right, and the shape that replaces it is an index beside the list rather than a
    /// different list. §24.1's note asks for sub-linear access "on average"; that is a benchmark's
    /// decision and this is the implementation to measure against.
    #[must_use]
    pub fn position_of(&self, wanted: Value, heap: &Heap) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_some_and(|(key, _)| key.same_value_zero(&wanted, heap)))
    }

    /// What is at `at`'s value, if it is a live entry.
    #[must_use]
    pub fn value_at(&self, at: usize) -> Option<Value> {
        self.entries
            .get(at)
            .copied()
            .flatten()
            .map(|(_, value)| value)
    }

    /// Put `value` at a position a lookup already found — §24.1.3.9 step 4.a.i.
    ///
    /// In place, so an existing key keeps its **position**: re-setting a key does not move it to
    /// the end, and iteration order is *first* insertion order.
    pub fn replace_at(&mut self, at: usize, value: Value) {
        if let Some(entry) = self.entries.get_mut(at)
            && let Some((key, _)) = *entry
        {
            *entry = Some((key, value));
        }
    }

    /// Append an entry a lookup did not find.
    pub fn push(&mut self, key: Value, value: Value) {
        // §24.1.3.9 step 6 — `-0` is normalised to `+0` before it is stored, so a map keyed by `-0`
        // hands back `+0` when iterated. The lookup already treats the two as one key; this is what
        // makes the *stored* key the one the specification names.
        let key = match key {
            Value::Number(number) if number == 0.0 && number.is_sign_negative() => {
                Value::Number(0.0)
            }
            other => other,
        };
        self.entries.push(Some((key, value)));
        self.live += 1;
    }

    /// Take out the entry at a position a lookup found — §24.1.3.3 and §24.2.3.4.
    pub fn delete_at(&mut self, at: usize) {
        if let Some(entry) = self.entries.get_mut(at)
            && entry.is_some()
        {
            *entry = None;
            self.live -= 1;
        }
    }

    /// Drop every entry whose key the collector could not reach — §24.3's liveness rule.
    ///
    /// Only ever called on a weak collection, and only by the sweep. A program cannot see this
    /// happen: an entry goes only when nothing else can name its key, so nothing is left that
    /// could ask about it. That is what makes the rule safe to apply and also what makes it
    /// impossible to test from JavaScript — the tests for it are in the collector, on a heap whose
    /// roots are named by hand.
    pub fn retain_keys(&mut self, reachable: impl Fn(Value) -> bool) {
        let mut dropped = 0;
        for entry in &mut self.entries {
            if let Some((key, _)) = *entry
                && !reachable(key)
            {
                *entry = None;
                dropped += 1;
            }
        }
        // Counted and subtracted once rather than decremented in the loop, because `self.live` and
        // `self.entries` are two fields and the loop is holding one of them.
        self.live -= dropped;
    }

    /// Empty it — §24.1.3.1 and §24.2.3.2.
    ///
    /// Every entry becomes a hole and the list keeps its length, for the reason a single delete
    /// does: an iterator part-way through a cleared collection must find nothing more, not start
    /// again at whatever is added next.
    pub fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = None;
        }
        self.live = 0;
    }
}
