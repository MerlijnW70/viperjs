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
/// such walk passes through, once per index, and because each pass interns a key — so a walk that
/// is going nowhere is also a walk that is spending the budget.
fn within_budget(heap: &Heap) -> Completion<()> {
    if heap.is_exhausted() {
        return Err(Abrupt::range_error(
            "the heap has grown past what this engine will allocate",
        ));
    }
    Ok(())
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

/// `this` as an object — §7.1.18, in the part that does not need a wrapper.
pub(super) fn this_object(call: &NativeCall<'_>) -> Completion<ObjectId> {
    match call.this_value {
        Value::Object(object) => Ok(object),
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
    let object = this_object(call)?;
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
    let object = this_object(call)?;
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
    let object = this_object(call)?;
    let mut length = length_of(vm, heap, object)?;
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
    let object = this_object(call)?;
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
    let object = this_object(call)?;
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
    let object = this_object(call)?;
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
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    let prototype = vm.realm().array_prototype();
    let mapped = heap.new_array(prototype, to_index(length));
    for index in 0..length {
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        let answer = vm.call_value(function, receiver, &arguments, heap)?;
        set_index(heap, mapped, index, answer);
    }
    Ok(Value::Object(mapped))
}

/// §23.1.3.13 `Array.prototype.filter`.
pub fn filter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    let prototype = vm.realm().array_prototype();
    let kept = heap.new_array(prototype, 0);
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
            set_index(heap, kept, at, element);
            at += 1;
        }
    }
    Ok(Value::Object(kept))
}

/// §23.1.3.25 `Array.prototype.slice`.
pub fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let from = start_index(vm, heap, call.argument(0), length)?;
    let to = match call.argument(1) {
        Value::Undefined => length,
        value => start_index(vm, heap, value, length)?,
    };
    let prototype = vm.realm().array_prototype();
    let taken = heap.new_array(prototype, 0);
    let mut at = 0_u64;
    for index in from..to.max(from) {
        // §23.1.3.25 step 9.b — a hole stays a hole, so `slice` is one of the few that can
        // answer with one.
        if has_index(vm, heap, object, index)? {
            let element = get_index(vm, heap, object, index)?;
            set_index(heap, taken, at, element);
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

/// A length as the count an Array's `length` property can hold.
///
/// `LengthOfArrayLike` allows up to `2^53 - 1` and an Array's `length` stops at `2^32 - 1`, so an
/// array-like longer than an Array can be is clamped rather than refused — the methods that build
/// a result would otherwise fail on an object no Array could have been.
pub(super) fn to_index(length: u64) -> u32 {
    u32::try_from(length).unwrap_or(u32::MAX - 1)
}

/// Build `Array.prototype`'s methods into `heap`.
pub fn install(heap: &mut Heap, realm: &crate::realm::Realm) {
    let prototype = realm.array_prototype();
    use super::{array_edit as edit, array_iterate as iterate};
    for (name, length, native) in [
        ("at", 1, edit::at as crate::heap::Native),
        ("concat", 1, edit::concat),
        ("every", 1, iterate::every),
        ("fill", 1, edit::fill),
        ("filter", 1, filter),
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
        ("splice", 2, edit::splice),
        ("toString", 0, to_string),
        ("unshift", 1, edit::unshift),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §23.1.3.38 — `[@@iterator]` **is** `values`, the same function object rather than a second
    // one that behaves alike. A script comparing them with `===` finds them equal, and that is
    // what the clause says.
    super::alias_to_symbol(heap, realm, prototype, "values", "iterator");
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
