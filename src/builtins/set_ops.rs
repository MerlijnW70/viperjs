//! §24.2.4's seven set operations, and the "set-like" they are willing to be given.
//!
//! # Why `other` need not be a Set
//!
//! §24.2.1.2 `GetSetRecord` asks an object for three things — a `size`, a `has` and a `keys` — and
//! that is the whole contract. Nothing checks for a `[[SetData]]`, so a hand-written object with
//! those three works, and so does a **`Map`**: `new Set([1]).union(new Map([[9, 'a']]))` answers
//! `{1, 9}`, because a Map has all three and its `keys` yields its keys. That is duck-typing on
//! purpose, and it is why these methods take a wider argument than their name suggests.
//!
//! The receiver is the other way about: its own `[[SetData]]` is read directly and its brand is
//! checked. Two different levels of access in one method, deliberately.
//!
//! # Why the size decides which way round the work goes
//!
//! Four of the seven branch on whether the receiver is smaller than `other`. That is not an
//! optimisation the specification left to implementations: the branch is **observable**, because
//! the two sides run different code. Walking the receiver calls `other.has` once per element;
//! walking `other` drives its `keys` iterator and calls nothing. A program can count either, and
//! the order of the result differs too — the receiver's order in one branch, `other`'s in the
//! other. Getting the comparison backwards is a real bug that answers the same *set* every time.
//!
//! # Why the iterator is stepped rather than drained
//!
//! `isSupersetOf` and `isDisjointFrom` stop at the first element that settles the question and
//! close the iterator. Collecting `other`'s keys into a list first would answer the same thing for
//! a finite iterator and would hang on one that never ends — and would call `next` more times than
//! the specification says, which a counting test sees.

use super::collection::collection_of;
use super::iterator::Walk;
use super::{define_method, key};
use crate::heap::{Collection, CollectionKind, Heap, Native, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §24.2.1.2's Set Record — what an argument has to offer to be treated as a set.
struct SetLike {
    /// The object itself, which `has` is called on.
    object: Value,
    /// How many elements it says it has, as a count. `None` is `+∞`, which a set-like may claim
    /// and which makes every "is the receiver smaller" comparison answer yes.
    size: Option<u64>,
    /// Its `has`, read once.
    has: Value,
    /// Its `keys`, read once and called only if the walk goes that way.
    keys: Value,
}

impl SetLike {
    /// §24.2.1.2 `GetSetRecord`.
    fn of(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<Self> {
        let Value::Object(_) = value else {
            return Err(Abrupt::type_error("this is not a set-like object"));
        };
        let name = key(heap, "size");
        let raw = vm.get_property_key(value, name, heap)?;
        let number = vm.to_number(raw, heap)?;
        // Step 4 — an absent `size` reads as `NaN` through `ToNumber`, and step 5 refuses that. So
        // "has no size" and "has a size of NaN" are one case and the note in §24.2.1.2 says so.
        if number.is_nan() {
            return Err(Abrupt::type_error("a set-like object must have a size"));
        }
        let integer = number.trunc();
        // Step 7 — a negative size is a **RangeError**, which is the one place in these seven
        // methods that is not a TypeError.
        if integer < 0.0 {
            return Err(Abrupt::range_error(
                "a set-like object's size cannot be negative",
            ));
        }
        let size = match integer == f64::INFINITY {
            true => None,
            false => Some(integer as u64),
        };
        let mut read = |name: &str| -> Completion<Value> {
            let property = key(heap, name);
            let found = vm.get_property_key(value, property, heap)?;
            match heap.is_callable(found) {
                true => Ok(found),
                false => Err(Abrupt::type_error(
                    "a set-like object must have has and keys",
                )),
            }
        };
        let has = read("has")?;
        let keys = read("keys")?;
        Ok(Self {
            object: value,
            size,
            has,
            keys,
        })
    }

    /// Whether the receiver, with this many elements, is no larger than this set-like.
    ///
    /// The comparison the four branching methods make. `None` is `+∞`, and nothing is larger than
    /// that — so a set-like claiming an infinite size always sends the walk over the receiver,
    /// which is the branch that never touches its `keys`.
    fn at_least(&self, count: u64) -> bool {
        self.size.is_none_or(|size| count <= size)
    }

    /// §24.2.1.2's `[[Has]]`, called on the set-like itself.
    fn has(&self, vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<bool> {
        let answer = vm.call_value(self.has, self.object, &[value], heap)?;
        Ok(answer.to_boolean(heap))
    }

    /// A walk over its keys — §24.2.1.2's `[[Keys]]`, called once.
    fn keys(&self, vm: &mut Vm, heap: &mut Heap) -> Completion<Walk> {
        Walk::from_method(vm, heap, self.object, self.keys)
    }
}

/// The receiver's elements, taken as a list before anything else runs.
///
/// Every one of these methods may run user code — `has`, `keys`, `next` — and that code may add to
/// or delete from the receiver. §24.2.4's algorithms re-read the length as they go and are written
/// against a list that is being mutated; taking a copy first answers about the set as it was, which
/// is what the tests about mutation during the walk are checking for.
fn elements(heap: &Heap, object: ObjectId) -> Vec<Value> {
    heap.object(object)
        .and_then(crate::heap::Object::collection)
        .map(|found| found.live_entries().map(|(key, _)| key).collect())
        .unwrap_or_default()
}

/// Whether the receiver holds this value, by §7.2.11 `SameValueZero`.
fn holds(heap: &Heap, object: ObjectId, value: Value) -> bool {
    heap.object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(value, heap))
        .is_some()
}

/// §6.1.6.1's `-0` normalised to `+0` — `CanonicalizeKeyedCollectionKey`.
///
/// Applied to everything that arrives from a set-like, because it is what a `Set` stores and the
/// result of these methods *is* a Set. Without it `new Set([0]).union(setLikeOf(-0))` would hold
/// two elements that `has` cannot tell apart.
fn canonical(value: Value) -> Value {
    match value {
        // Both halves of the guard, which is the same shape `Collection::push` uses: `-0` is the
        // one value that has to change, and saying so needs the sign as well as the magnitude.
        Value::Number(number) if number == 0.0 && number.is_sign_negative() => Value::Number(0.0),
        other => other,
    }
}

/// A `Set` holding these values, in this order — what six of the seven answer with.
fn set_of(vm: &mut Vm, heap: &mut Heap, values: Vec<Value>) -> Value {
    let object = heap.new_object(Some(vm.realm().set_prototype()));
    let mut collection = Collection::new(CollectionKind::Set);
    for value in values {
        collection.push(value, value);
    }
    if let Some(found) = heap.object_mut(object) {
        found.set_collection(collection);
    }
    Value::Object(object)
}

/// Append `value` unless the list already has it — the `SetDataHas` every builder does.
fn push_new(heap: &Heap, into: &mut Vec<Value>, value: Value) {
    if !into.iter().any(|held| held.same_value_zero(&value, heap)) {
        into.push(value);
    }
}

/// The receiver and the argument, which every one of the seven begins by reading.
fn both(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<(ObjectId, SetLike)> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::Set,
        "this is not a Set",
    )?;
    // The receiver's brand is checked *first*, so `Set.prototype.union.call(1, {})` complains about
    // the receiver rather than about the argument.
    let other = SetLike::of(vm, heap, call.argument(0))?;
    Ok((object, other))
}

/// §24.2.4.17 `Set.prototype.union`.
fn union(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mut result = elements(heap, object);
    let walk = other.keys(vm, heap)?;
    while let Some(value) = walk.step(vm, heap)? {
        push_new(heap, &mut result, canonical(value));
        super::array_methods::within_budget(vm, heap)?;
    }
    Ok(set_of(vm, heap, result))
}

/// §24.2.4.9 `Set.prototype.intersection`.
fn intersection(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mut result = Vec::new();
    let mine = elements(heap, object);
    match other.at_least(mine.len() as u64) {
        // The receiver is the smaller one, so ask the argument about each of its elements. The
        // answer keeps the *receiver's* order.
        true => {
            for value in mine {
                if other.has(vm, heap, value)? {
                    push_new(heap, &mut result, canonical(value));
                }
                super::array_methods::within_budget(vm, heap)?;
            }
        }
        // …and the other way about, which keeps the *argument's* order. Two orders from one
        // method, decided by a size a program controls.
        false => {
            let walk = other.keys(vm, heap)?;
            while let Some(value) = walk.step(vm, heap)? {
                let value = canonical(value);
                if holds(heap, object, value) {
                    push_new(heap, &mut result, value);
                }
                super::array_methods::within_budget(vm, heap)?;
            }
        }
    }
    Ok(set_of(vm, heap, result))
}

/// §24.2.4.5 `Set.prototype.difference`.
fn difference(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mut result = elements(heap, object);
    match other.at_least(result.len() as u64) {
        true => {
            let mine = result.clone();
            for value in mine {
                if other.has(vm, heap, value)? {
                    result.retain(|held| !held.same_value_zero(&value, heap));
                }
                super::array_methods::within_budget(vm, heap)?;
            }
        }
        false => {
            let walk = other.keys(vm, heap)?;
            while let Some(value) = walk.step(vm, heap)? {
                let value = canonical(value);
                result.retain(|held| !held.same_value_zero(&value, heap));
                super::array_methods::within_budget(vm, heap)?;
            }
        }
    }
    Ok(set_of(vm, heap, result))
}

/// §24.2.4.15 `Set.prototype.symmetricDifference`.
///
/// The one that does not branch on size: it has to see every element of the argument whichever is
/// bigger, because an element in neither set still belongs in the answer.
fn symmetric_difference(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mut result = elements(heap, object);
    let walk = other.keys(vm, heap)?;
    while let Some(value) = walk.step(vm, heap)? {
        let value = canonical(value);
        // Asked of the receiver as it *was*, not of the answer being built — so an element the
        // argument produces twice is removed once and not put back.
        match holds(heap, object, value) {
            true => result.retain(|held| !held.same_value_zero(&value, heap)),
            false => push_new(heap, &mut result, value),
        }
        super::array_methods::within_budget(vm, heap)?;
    }
    Ok(set_of(vm, heap, result))
}

/// §24.2.4.12 `Set.prototype.isSubsetOf`.
fn is_subset_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mine = elements(heap, object);
    // A set with more elements than another cannot be inside it, and the sizes settle that without
    // asking anything — which is why a larger receiver never calls `has` at all.
    if !other.at_least(mine.len() as u64) {
        return Ok(Value::Boolean(false));
    }
    for value in mine {
        if !other.has(vm, heap, value)? {
            return Ok(Value::Boolean(false));
        }
        super::array_methods::within_budget(vm, heap)?;
    }
    Ok(Value::Boolean(true))
}

/// §24.2.4.14 `Set.prototype.isSupersetOf`.
fn is_superset_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let count = elements(heap, object).len() as u64;
    // A set with fewer elements than another cannot contain it, and the sizes settle that without
    // driving the iterator at all. `None` is the `+∞` a set-like may claim, and nothing finite is
    // at least that — so an infinite argument answers `false` here and `keys` is never called.
    if !other.size.is_some_and(|size| count >= size) {
        return Ok(Value::Boolean(false));
    }
    let walk = other.keys(vm, heap)?;
    while let Some(value) = walk.step(vm, heap)? {
        if !holds(heap, object, canonical(value)) {
            // Stopped early, so the iterator is told — §7.4.9, and the reason this walks rather
            // than drains.
            walk.close(vm, heap);
            return Ok(Value::Boolean(false));
        }
        super::array_methods::within_budget(vm, heap)?;
    }
    Ok(Value::Boolean(true))
}

/// §24.2.4.11 `Set.prototype.isDisjointFrom`.
fn is_disjoint_from(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, other) = both(vm, heap, call)?;
    let mine = elements(heap, object);
    match other.at_least(mine.len() as u64) {
        true => {
            for value in mine {
                if other.has(vm, heap, value)? {
                    return Ok(Value::Boolean(false));
                }
                super::array_methods::within_budget(vm, heap)?;
            }
        }
        false => {
            let walk = other.keys(vm, heap)?;
            while let Some(value) = walk.step(vm, heap)? {
                if holds(heap, object, canonical(value)) {
                    walk.close(vm, heap);
                    return Ok(Value::Boolean(false));
                }
                super::array_methods::within_budget(vm, heap)?;
            }
        }
    }
    Ok(Value::Boolean(true))
}

/// Put §24.2.4's seven onto `Set.prototype`.
pub(super) fn install(heap: &mut Heap, realm: &Realm, prototype: ObjectId) {
    for (name, native) in [
        ("union", union as Native),
        ("intersection", intersection),
        ("difference", difference),
        ("symmetricDifference", symmetric_difference),
        ("isSubsetOf", is_subset_of),
        ("isSupersetOf", is_superset_of),
        ("isDisjointFrom", is_disjoint_from),
    ] {
        define_method(heap, realm, prototype, name, 1, native);
    }
}
