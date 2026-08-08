//! §23.1.3 — the methods on `Array.prototype`.
//!
//! # Why almost none of them mention Arrays
//!
//! §23.1.3 is written against an *array-like*: an object with a `length` and some indices. Not one
//! of these methods asks whether it was given an Array, which is why
//! `Array.prototype.join.call({0: "a", 1: "b", length: 2})` works and why `arguments` and a DOM
//! node list have always been usable with them. The generic reading is the specified one; a check
//! for a real Array would be an engine inventing a restriction.
//!
//! So they read `length` through `LengthOfArrayLike` and the elements through ordinary `[[Get]]`,
//! and the exotic behaviour in [`crate::heap`] only shows up when the *result* is an Array.
//!
//! # Holes
//!
//! An index that is absent is not the same as one holding `undefined`, and the methods disagree
//! about which they care about. `join` reads a hole as the empty string, `forEach` and `map` skip
//! it entirely, `indexOf` never matches one. Each of those is a `HasProperty` the specification
//! writes explicitly, and each is a row in the tests.

use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

use super::{define_method, delete_or_throw, key, set_or_throw};

/// §7.3.18 `LengthOfArrayLike` — the `length` of anything, as a count.
///
/// `ToLength` clamps to `[0, 2^53 - 1]`: a negative length reads as zero and an enormous one as
/// the largest a Number can index exactly. That is why `{length: -1}` iterates nothing rather
/// than failing, and why these methods can be handed anything at all without checking first.
pub(super) fn length_of(vm: &mut Vm, heap: &mut Heap, object: ObjectId) -> Completion<u64> {
    let name = key(heap, "length");
    let value = vm.get_property_key(Value::Object(object), name, heap)?;
    let number = vm.to_number(value, heap)?;
    Ok(to_length(number))
}

/// The largest length an array-like may have — §7.1.20's clamp, and §23.1's TypeError.
///
/// Two rules meet at this number and they are not the same rule. `ToLength` **clamps** to it, so
/// an object claiming a `length` of `2 ** 60` is read as having this many. But a method that would
/// *grow* an array past it throws a **TypeError** instead of clamping, because the elements it
/// would have to move have nowhere to go.
pub(super) const MAX_LENGTH: u64 = 9_007_199_254_740_991;

/// Whether an array-like may be this long — §23.1.3.34 step 4.a and §23.1.3.28 step 8.
///
/// Separated from the methods that ask it so that the boundary can be *asked* at lengths that
/// cannot be *walked*. Proving from JavaScript that a length one short of the maximum may grow by
/// one means letting it grow and then walking 2^53 indices, which is not a test, it is a wait.
pub(super) const fn fits(length: u64) -> bool {
    length <= MAX_LENGTH
}

/// How long an array-like is after a splice — §23.1.3.28 step 8's arithmetic.
///
/// Its own function for the same reason as [`fits`]: what it computes is checked against the
/// maximum, and every case where the check is interesting is a case too large to reach by running
/// anything.
pub(super) const fn spliced_length(length: u64, removed: u64, inserted: u64) -> u64 {
    length - removed + inserted
}

/// §7.1.20 `ToLength`.
///
/// Written as two clamps rather than a guard and a clamp, because `f64::max` answers the *other*
/// operand when one is NaN — so `max(0.0)` turns NaN into zero and a negative into zero at once,
/// which is §7.1.20 steps 2 and 3 exactly. A guard in front of it would be a branch no input
/// could tell from its absence.
#[allow(clippy::manual_clamp)] // see the note below
pub(super) fn to_length(number: f64) -> u64 {
    const MAX: f64 = 9_007_199_254_740_991.0;
    // Not `clamp`: its own documentation says it answers NaN for a NaN input, and §7.1.20
    // step 2 says a NaN length is zero. `max` then `min` is the pair that gets that right,
    // because `f64::max` answers the *other* operand when one is NaN.
    number.max(0.0).min(MAX) as u64
}

/// The key an index is filed under — the decimal spelling, which is what a property key is.
pub(super) fn index_key(heap: &mut Heap, index: u64) -> PropertyKey {
    PropertyKey::from_units(heap, &index.to_string().encode_utf16().collect::<Vec<_>>())
}

/// Whether `object` or its prototypes have this index — §7.3.11 `HasProperty`.
///
/// The question that tells a hole from an `undefined`, and the one that makes `[, 1]` and
/// `[undefined, 1]` behave differently in half of §23.1.3.
pub(super) fn has_index(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    index: u64,
) -> Completion<bool> {
    within_budget(heap)?;
    let name = index_key(heap, index);
    let found = vm.has_property_key(Value::Object(object), name, heap)?;
    Ok(found)
}

/// The value at this index — §7.3.2 `Get`.
pub(super) fn get_index(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    index: u64,
) -> Completion<Value> {
    within_budget(heap)?;
    let name = index_key(heap, index);
    vm.get_property_key(Value::Object(object), name, heap)
}

/// Stop if DR-0013's budget has been spent — the check a built-in has to make for itself.
///
/// The interpreter asks between instructions, and a built-in walking a length never gets back to
/// it: `Array.prototype.reduceRight` over an object whose `length` is `2 ** 53 - 1` is a loop
/// inside Rust with no instruction boundary in it. Asked here because this is the one place every
/// such walk passes through, once per index.
///
/// **This used to end "and each pass interns a key — so a walk that is going nowhere is also a walk
/// that is spending the budget", and DR-0026 made that false.** An `Index` key allocates nothing,
/// so a walk that reads absent elements out of a huge array-like now spends no heap and this never
/// fires. That is *not* a hole to plug: `Array.prototype.indexOf.call({length: 2 ** 53 - 1}, x)` is
/// a program the specification says to loop through, and node does not return from it either —
/// measured, not assumed. The engine's answer to a program that will not stop is DR-0022's time
/// budget, which is the host's to set. What this still catches is the walk that *does* allocate:
/// `map` and `splice` build a result per index, and those are the ones a budget can be spent on.
///
/// One test was passing on the old behaviour and is worth naming, because the failure is the shape
/// this repository keeps meeting: `slice` asked `ArraySpeciesCreate` for a **zero**-length array
/// where §23.1.3.25 step 8 asks for `count`, so the RangeError that clause owes never came — and
/// the walk ran into the heap budget instead, which threw a RangeError of its own that
/// `assert.throws(RangeError, …)` could not tell apart.
pub(super) fn within_budget(heap: &Heap) -> Completion<()> {
    if heap.is_exhausted() {
        return Err(Abrupt::range_error(
            "the heap has grown past what this engine will allocate",
        ));
    }
    Ok(())
}

/// §7.3.23 `ArraySpeciesCreate` — the array a method that copies should answer with.
///
/// Not `ArrayCreate`. §23.1.3's copying methods ask the array they were given what *kind* of thing
/// to make: a subclass of Array gets its own kind back from `map` and `filter` and `slice`, which
/// is what `Symbol.species` is for. Only an Array is asked — step 2 answers a plain Array for any
/// other array-like, so `Array.prototype.map.call({length: 1})` is not affected by anything the
/// object's `constructor` says.
///
/// The realm check in step 4 is not written: ViperJS has one realm, so "the constructor came from
/// another realm" is a condition no program here can produce.
pub(super) fn array_species_create(
    vm: &mut Vm,
    heap: &mut Heap,
    original: ObjectId,
    length: u64,
) -> Completion<Value> {
    if !heap.is_array_through(original)? {
        return Ok(Value::Object(new_array_checked(vm, heap, length)?));
    }
    let name = key(heap, "constructor");
    let mut constructor = vm.get_property_key(Value::Object(original), name, heap)?;
    // Step 5 — the species is read off the constructor *only* when the constructor is an object.
    // A primitive one is left alone and refused by step 7, which is why `a.constructor = 1` is a
    // TypeError rather than quietly making a plain Array.
    if let Value::Object(_) = constructor {
        let Some(species) = heap.well_known(super::well_known_at("species")) else {
            return Ok(Value::Object(new_array_checked(vm, heap, length)?));
        };
        constructor = vm.get_property_key(constructor, PropertyKey::from_symbol(species), heap)?;
        // Step 5.b — `null` becomes `undefined`, so both spellings of "no opinion" reach step 6.
        if matches!(constructor, Value::Null) {
            constructor = Value::Undefined;
        }
    }
    if matches!(constructor, Value::Undefined) {
        return Ok(Value::Object(new_array_checked(vm, heap, length)?));
    }
    if !heap.is_constructor(constructor) {
        return Err(Abrupt::type_error(
            "the species of this array is not a constructor",
        ));
    }
    vm.construct_value(constructor, &[Value::Number(length as f64)], heap)
}

/// §7.3.5 `CreateDataPropertyOrThrow` — put a value at this index, or say why it could not go.
///
/// The half of [`set_index`] that a species result needs. When the target is a fresh Array the
/// define cannot be refused and the two are the same; when it is whatever a `Symbol.species`
/// handed back it may be non-extensible, or already have a non-configurable property at that
/// index, and §23.1.3 says to throw rather than to carry on writing into something that is not
/// listening.
pub(super) fn create_index(
    heap: &mut Heap,
    object: ObjectId,
    index: u64,
    value: Value,
) -> Completion<()> {
    let name = index_key(heap, index);
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    match heap.define_own_property(object, name, &descriptor) {
        true => Ok(()),
        false => Err(Abrupt::type_error(
            "this index could not be added to the array",
        )),
    }
}

/// Put a value at this index of an object being built — §7.3.5 `CreateDataPropertyOrThrow`.
pub(super) fn set_index(heap: &mut Heap, object: ObjectId, index: u64, value: Value) {
    let name = index_key(heap, index);
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(object, name, &descriptor);
}

/// Put a value under a name of an object being built — §7.3.5, for a key that is not an index.
pub(super) fn define_named(heap: &mut Heap, object: ObjectId, name: PropertyKey, value: Value) {
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(object, name, &descriptor);
}

/// `this` as an object — §7.1.18 `ToObject`, which wraps rather than refuses.
///
/// Every method in §23.1.3 opens with `Let O be ? ToObject(this value)`, and the difference
/// between that and requiring an object is a whole family of working programs:
/// `Array.prototype.join.call("ab")` reads a String object's own indices, and
/// `Array.prototype.sort.call(true)` sorts a Boolean wrapper — which has no indices, so it sorts
/// nothing and answers the wrapper. Refusing a primitive here would be the engine inventing a
/// restriction the specification does not have.
///
/// `undefined` and `null` are the two that genuinely have no object, and they are §7.1.18 steps 1
/// and 2's TypeError rather than this function's opinion.
pub(super) fn this_object(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<ObjectId> {
    match vm.object_for(call.this_value, heap)? {
        Value::Object(object) => Ok(object),
        // `object_for` answers an Object or throws; there is no third answer. Saying so costs a
        // line and keeps the promise that nothing here panics on a shape the types cannot rule out.
        _ => Err(Abrupt::type_error(
            "an Array.prototype method requires an object",
        )),
    }
}

/// The callback a method was given, which §23.1.3 requires to be callable *before* anything runs.
///
/// Checked up front rather than at the first element, because §23.1.3.15 step 3 says so: an empty
/// array with a callback that is not a function still throws, and a program that relied on the
/// check being skipped would be relying on the array being empty.
pub(super) fn callback(call: &NativeCall<'_>, heap: &Heap) -> Completion<Value> {
    let function = call.argument(0);
    let callable = matches!(function, Value::Object(object)
        if heap.object(object).is_some_and(|found| found.call().is_some()));
    match callable {
        true => Ok(function),
        false => Err(Abrupt::type_error("the callback is not a function")),
    }
}

/// §23.1.3.18 `Array.prototype.join`.
///
/// A hole and `undefined` and `null` all read as the empty string — step 4.b's
/// "If element is undefined or null, let next be the empty String" — which is why
/// `[1, , 3].join("-")` is `"1--3"` and not `"1-undefined-3"`.
pub fn join(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let separator = match call.argument(0) {
        Value::Undefined => ",".to_string(),
        value => {
            let id = vm.to_string(value, heap)?;
            String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
        }
    };
    let mut joined = String::new();
    for index in 0..length {
        if index > 0 {
            joined.push_str(&separator);
        }
        let element = get_index(vm, heap, object, index)?;
        if matches!(element, Value::Undefined | Value::Null) {
            continue;
        }
        let id = vm.to_string(element, heap)?;
        joined.push_str(&String::from_utf16_lossy(heap.string(id).unwrap_or(&[])));
    }
    Ok(super::text(heap, &joined))
}

/// §23.1.3.31 `Array.prototype.toString` — `join` with no separator, or the object's own `join`.
///
/// Step 3 calls whatever `join` the object has rather than the intrinsic one, so replacing
/// `Array.prototype.join` changes what an array prints. That is not a quirk to tidy away: it is
/// what makes `join` the single place an array's text is decided.
pub fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let name = key(heap, "join");
    let found = vm.get_property_key(Value::Object(object), name, heap)?;
    let callable = matches!(found, Value::Object(id)
        if heap.object(id).is_some_and(|it| it.call().is_some()));
    match callable {
        // Step 4 — an object whose `join` is not callable falls back to
        // `Object.prototype.toString`, which is how `[object Array]` can still be reached.
        false => super::object::to_string(vm, heap, call),
        true => vm.call_value(found, call.this_value, &[], heap),
    }
}

/// §23.1.3.23 `Array.prototype.push`.
pub fn push(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let mut length = length_of(vm, heap, object)?;
    // Step 4 — refused **before** anything is written, so an array-like at the limit is left
    // exactly as it was rather than part-way grown. `ToLength` has already clamped a `length` of
    // `2 ** 60` down to the maximum, which is why a nonsense length arrives here as a refusable
    // one rather than as arithmetic that overflows.
    //
    // `argCount` and not one: pushing **nothing** onto an array at the maximum is allowed, since
    // `len + 0` is not past it. That is the case an off-by-one gets wrong in the safe-looking
    // direction, and the one a test written with a single argument cannot see.
    if !fits(length.saturating_add(call.arguments.len() as u64)) {
        return Err(Abrupt::type_error(
            "this array-like is longer than this engine will allocate",
        ));
    }
    for value in call.arguments {
        let name = index_key(heap, length);
        set_or_throw(vm, heap, object, name, *value)?;
        length += 1;
    }
    // §23.1.3.23 step 5 sets `length` even on a plain object, which is what makes `push` work on
    // an array-like and leave a `length` behind that was not there before.
    let name = key(heap, "length");
    let count = Value::Number(length as f64);
    set_or_throw(vm, heap, object, name, count)?;
    Ok(count)
}

/// §23.1.3.22 `Array.prototype.pop`.
pub fn pop(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let name = key(heap, "length");
    // Step 3 — an empty array's `length` is still *written*, which matters for an array-like
    // whose `length` was a string: `pop` leaves it a number.
    let Some(last) = length.checked_sub(1) else {
        set_or_throw(vm, heap, object, name, Value::Number(0.0))?;
        return Ok(Value::Undefined);
    };
    let element = get_index(vm, heap, object, last)?;
    let index = index_key(heap, last);
    delete_or_throw(vm, heap, object, index)?;
    set_or_throw(vm, heap, object, name, Value::Number(last as f64))?;
    Ok(element)
}

/// §23.1.3.17 `Array.prototype.indexOf`.
///
/// Strict equality, so `NaN` is never found — which is the whole reason `includes` exists and
/// uses `SameValueZero` instead.
pub fn index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let wanted = call.argument(0);
    let from = start_index(vm, heap, call.argument(1), length)?;
    for index in from..length {
        // Step 9.a — a hole is skipped rather than compared, so `[, 1].indexOf(undefined)` is -1
        // where `[undefined, 1].indexOf(undefined)` is 0.
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        if element.is_strictly_equal(&wanted, heap) {
            return Ok(Value::Number(index as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

/// Where a search starts, out of the argument §23.1.3.17 step 6 describes.
///
/// A negative index counts from the end, and one past the start is clamped to zero rather than
/// wrapping — `[1, 2].indexOf(1, -5)` searches the whole array.
pub(super) fn start_index(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
    length: u64,
) -> Completion<u64> {
    // No special case for an absent argument: `ToNumber(undefined)` is NaN, and §23.1.3.17 step 6
    // reads NaN as zero — so the ordinary path already answers what "from the beginning" means.
    let number = vm.to_number(value, heap)?;
    let integer = if number.is_nan() { 0.0 } else { number.trunc() };
    Ok(match integer < 0.0 {
        true => (length as f64 + integer).max(0.0) as u64,
        false => (integer as u64).min(length),
    })
}

/// §23.1.3.15 `Array.prototype.forEach`.
pub fn for_each(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    for index in 0..length {
        // Step 6.b — a hole is skipped, so the callback is not run for one and `length` is not
        // the number of times it is called.
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        vm.call_value(function, receiver, &arguments, heap)?;
    }
    Ok(Value::Undefined)
}

/// §23.1.3.21 `Array.prototype.map`.
///
/// The result has the same `length` as the original **including its holes**, and a hole in maps to
/// a hole out: the callback is not run and no property is made. `[, 1].map(f)` calls `f` once and
/// answers an array of length 2 whose first index is still absent.
pub fn map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    // §23.1.3.21 step 5 — the *species* decides what comes back, so a subclass of Array gets
    // its own kind from `map` rather than a plain one.
    let Value::Object(mapped) = array_species_create(vm, heap, object, length)? else {
        return Err(Abrupt::type_error(
            "the species of this array did not make an object",
        ));
    };
    for index in 0..length {
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        let answer = vm.call_value(function, receiver, &arguments, heap)?;
        create_index(heap, mapped, index, answer)?;
    }
    Ok(Value::Object(mapped))
}

/// §23.1.3.13 `Array.prototype.filter`.
pub fn filter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    let Value::Object(kept) = array_species_create(vm, heap, object, 0)? else {
        return Err(Abrupt::type_error(
            "the species of this array did not make an object",
        ));
    };
    let mut at = 0_u64;
    for index in 0..length {
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        let answer = vm.call_value(function, receiver, &arguments, heap)?;
        if answer.to_boolean(heap) {
            // The result is packed: the indices of what was kept are consecutive, whatever they
            // were in the original. That is why `filter` cannot answer with holes.
            create_index(heap, kept, at, element)?;
            at += 1;
        }
    }
    Ok(Value::Object(kept))
}

/// §23.1.3.25 `Array.prototype.slice`.
pub fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let from = start_index(vm, heap, call.argument(0), length)?;
    let to = match call.argument(1) {
        Value::Undefined => length,
        value => start_index(vm, heap, value, length)?,
    };
    // §23.1.3.25 step 8 — `count`, and it is **not** the zero every neighbour passes. `filter`,
    // `concat` and `flat` really do ask for an empty one because they cannot know how many they
    // will keep; `slice` knows, and the number is observable twice over: it is the argument a
    // `Symbol.species` constructor is called with, and it is what §10.4.2.2 step 1 refuses when it
    // is past 2^32-1. `{length: 2 ** 32}.slice(0, 2 ** 32)` is that RangeError.
    //
    // **It passed a zero here until DR-0026, and the tests for it were green.** Interning a key per
    // index used to spend DR-0013's heap budget, so a walk this long ended in a RangeError of a
    // different kind — and `assert.throws(RangeError, …)` cannot tell two RangeErrors apart. An
    // `Index` key allocates nothing, the budget stopped being spent, and the walk stopped ending.
    let count = to.max(from) - from;
    let Value::Object(taken) = array_species_create(vm, heap, object, count)? else {
        return Err(Abrupt::type_error(
            "the species of this array did not make an object",
        ));
    };
    let mut at = 0_u64;
    for index in from..to.max(from) {
        // §23.1.3.25 step 9.b — a hole stays a hole, so `slice` is one of the few that can
        // answer with one.
        if has_index(vm, heap, object, index)? {
            let element = get_index(vm, heap, object, index)?;
            create_index(heap, taken, at, element)?;
        }
        at += 1;
    }
    // The length is written even when the last elements were holes, which `set_index` alone
    // would not do.
    let name = key(heap, "length");
    let count = Value::Number(at as f64);
    set_or_throw(vm, heap, taken, name, count)?;
    Ok(Value::Object(taken))
}

/// §10.4.2.2 `ArrayCreate` — a new Array that long, or the RangeError that length earns.
///
/// `LengthOfArrayLike` **clamps** what is read to 2^53-1; an Array's own `length` stops at 2^32-1,
/// and §10.4.2.2 step 1 makes the gap between them a **RangeError** rather than a second clamp.
/// Every method that answers a copy of an array-like meets this, because the array-like it was
/// handed may be longer than any Array — so the copy it is asking for is not one that exists.
pub(super) fn new_array_checked(vm: &mut Vm, heap: &mut Heap, length: u64) -> Completion<ObjectId> {
    let Ok(size) = u32::try_from(length) else {
        return Err(Abrupt::range_error("an array cannot be that long"));
    };
    let prototype = vm.realm().array_prototype();
    Ok(heap.new_array(prototype, size))
}

/// Build `Array.prototype`'s methods into `heap`.
pub fn install(heap: &mut Heap, realm: &crate::realm::Realm) {
    let prototype = realm.array_prototype();
    use super::{array_edit as edit, array_iterate as iterate};
    for (name, length, native) in [
        ("at", 1, edit::at as crate::heap::Native),
        ("concat", 1, edit::concat),
        ("copyWithin", 2, edit::copy_within),
        ("every", 1, iterate::every),
        ("fill", 1, edit::fill),
        ("filter", 1, filter),
        ("flat", 0, super::array_flat::flat),
        ("flatMap", 1, super::array_flat::flat_map),
        ("find", 1, iterate::find),
        ("findIndex", 1, iterate::find_index),
        ("findLast", 1, iterate::find_last),
        ("findLastIndex", 1, iterate::find_last_index),
        ("forEach", 1, for_each),
        ("includes", 1, edit::includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("lastIndexOf", 1, edit::last_index_of),
        ("map", 1, map),
        ("pop", 0, pop),
        ("entries", 0, entries),
        ("keys", 0, keys),
        ("push", 1, push),
        ("values", 0, values),
        ("reduce", 1, iterate::reduce),
        ("reduceRight", 1, iterate::reduce_right),
        ("reverse", 0, edit::reverse),
        ("shift", 0, edit::shift),
        ("slice", 2, slice),
        ("some", 1, iterate::some),
        ("sort", 1, super::array_sort::sort),
        ("splice", 2, edit::splice),
        ("toReversed", 0, super::array_copy::to_reversed),
        ("toSorted", 1, super::array_sort::to_sorted),
        ("toLocaleString", 0, super::array_flat::to_locale_string),
        ("toSpliced", 2, super::array_copy::to_spliced),
        ("toString", 0, to_string),
        ("unshift", 1, edit::unshift),
        ("with", 2, super::array_copy::with),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §23.1.3.38 — `[@@iterator]` **is** `values`, the same function object rather than a second
    // one that behaves alike. A script comparing them with `===` finds them equal, and that is
    // what the clause says.
    super::alias_to_symbol(heap, prototype, "values", "iterator");
}

/// §23.1.3.34 `Array.prototype.values`, and `[@@iterator]` — which is the same function object.
///
/// §23.1.3.38 says so outright: `Array.prototype[@@iterator]` **is** `Array.prototype.values`, so
/// a script comparing them with `===` finds them equal. Installing two natives with the same body
/// would not satisfy that.
pub(super) fn values(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    super::iterator::over_array(vm, heap, call, crate::heap::Iterated::Values)
}

/// §23.1.3.17 `Array.prototype.keys`.
pub(super) fn keys(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    super::iterator::over_array(vm, heap, call, crate::heap::Iterated::Keys)
}

/// §23.1.3.7 `Array.prototype.entries`.
pub(super) fn entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    super::iterator::over_array(vm, heap, call, crate::heap::Iterated::Entries)
}

#[cfg(test)]
mod length_tests {
    use super::{MAX_LENGTH, fits, spliced_length};

    #[test]
    fn the_maximum_length_is_a_length_and_one_past_it_is_not() {
        // Both sides of the boundary, and the boundary itself. A comparison written one notch out
        // would agree with every array a test could actually build.
        assert!(fits(0));
        assert!(fits(MAX_LENGTH - 1));
        assert!(fits(MAX_LENGTH));
        assert!(!fits(MAX_LENGTH + 1));
        assert!(!fits(u64::MAX));
        // 2^53 - 1, written out: the number is what §7.1.20 clamps to and what §23.1 refuses to
        // grow past, and the two must be the same number.
        assert_eq!(MAX_LENGTH, 9_007_199_254_740_991);
    }

    #[test]
    fn a_splice_that_removes_as_much_as_it_inserts_leaves_the_length_alone() {
        assert_eq!(spliced_length(10, 0, 0), 10);
        assert_eq!(spliced_length(10, 3, 3), 10);
        assert_eq!(spliced_length(10, 3, 0), 7);
        assert_eq!(spliced_length(10, 0, 3), 13);
        // The rows that matter, at the top of the range: removing one and inserting one is
        // allowed where inserting one alone is not.
        assert!(fits(spliced_length(MAX_LENGTH, 1, 1)));
        assert!(!fits(spliced_length(MAX_LENGTH, 0, 1)));
        assert!(fits(spliced_length(MAX_LENGTH, 1, 0)));
    }
}
