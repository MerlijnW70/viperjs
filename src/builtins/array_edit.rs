//! §23.1.3's methods that move elements about, and the two searches that take no callback.
//!
//! # Why moving is not copying
//!
//! `shift`, `unshift`, `splice` and `reverse` change the object they were given. On a real Array
//! that is unremarkable; on an array-like it is the whole point, and it is why each of them writes
//! `length` explicitly rather than letting the exotic behaviour do it.
//!
//! And each of them **preserves holes**. Moving an element means a `Get` and a `Set` for an index
//! that is there, and a `Delete` for one that is not — so an array with a hole in the middle still
//! has one after `reverse`, in the mirrored place. Filling them in would be easier and would make
//! `1 in a` answer differently, which is exactly the difference a hole exists to record.

use super::array_methods::{
    fits, get_index, has_index, index_key, length_of, set_index, spliced_length, start_index,
    this_object,
};
use super::{delete_or_throw, key, set_or_throw};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Write `length` back, which every method here does for itself.
fn write_length(vm: &mut Vm, heap: &mut Heap, object: ObjectId, length: u64) -> Completion<()> {
    let name = key(heap, "length");
    let count = Value::Number(length as f64);
    set_or_throw(vm, heap, object, name, count)?;
    Ok(())
}

/// Move what is at `from` to `to`, or delete `to` when `from` is a hole — §23.1.3.26 step 6.b.
fn move_index(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    from: u64,
    to: u64,
) -> Completion<()> {
    let target = index_key(heap, to);
    match has_index(vm, heap, object, from)? {
        true => {
            let element = get_index(vm, heap, object, from)?;
            set_or_throw(vm, heap, object, target, element)?;
        }
        // The hole travels with the element, which is what keeps `1 in a` answering the same
        // thing before and after.
        false => {
            delete_or_throw(vm, heap, object, target)?;
        }
    }
    Ok(())
}

/// §23.1.3.26 `Array.prototype.shift`.
pub fn shift(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let Some(last) = length.checked_sub(1) else {
        write_length(vm, heap, object, 0)?;
        return Ok(Value::Undefined);
    };
    let first = get_index(vm, heap, object, 0)?;
    for index in 1..length {
        move_index(vm, heap, object, index, index - 1)?;
    }
    let end = index_key(heap, last);
    delete_or_throw(vm, heap, object, end)?;
    write_length(vm, heap, object, last)?;
    Ok(first)
}

/// §23.1.3.32 `Array.prototype.unshift`.
pub fn unshift(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let added = call.arguments.len() as u64;
    // §23.1.3.34 step 4 — *only* when there is something to insert. This used to say that a guard
    // here was one no input could tell from its absence, on the grounds that moving each index
    // onto itself changes nothing. It changes nothing and it takes for ever: an array-like whose
    // `length` is 2^53-1 spends the rest of the day doing it, and the specification skips the
    // step rather than performing it emptily.
    if added > 0 {
        // Step 4.a — the one length rule in §23.1 that throws instead of clamping. `ToLength`
        // has already clamped what was *read*; this is about what would be *written*, and there
        // is no index past this to write to.
        if !fits(length + added) {
            return Err(Abrupt::type_error(
                "the resulting array would be longer than 2^53 - 1",
            ));
        }
        // Downwards, because moving up would overwrite what has not been read yet — the same
        // reason `memmove` exists and `memcpy` would be wrong.
        for index in (0..length).rev() {
            move_index(vm, heap, object, index, index + added)?;
        }
        for (at, value) in call.arguments.iter().enumerate() {
            let name = index_key(heap, at as u64);
            set_or_throw(vm, heap, object, name, *value)?;
        }
    }
    let grown = length + added;
    write_length(vm, heap, object, grown)?;
    Ok(Value::Number(grown as f64))
}

/// §23.1.3.4 `Array.prototype.copyWithin`, in place.
///
/// The one method here that can read an index it has already written, because its source and its
/// destination are the same array. §23.1.3.4 step 8 is what stops it: when the destination starts
/// inside the source the walk runs backwards, so every index is read before the copy reaches it —
/// the same reason `memmove` exists and `memcpy` would be wrong.
pub fn copy_within(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let to = start_index(vm, heap, call.argument(0), length)?;
    let from = start_index(vm, heap, call.argument(1), length)?;
    // An absent third argument is the end of the array, which is not what `ToIntegerOrInfinity`
    // of `undefined` would give — that is zero, and would copy nothing at all.
    let end = match call.argument(2) {
        Value::Undefined => length,
        value => start_index(vm, heap, value, length)?,
    };
    // Step 7 — however much is asked for, it stops at whichever end comes first. `saturating_sub`
    // is the `max(final - from, 0)` half: a range that runs backwards copies nothing rather than
    // wrapping round to an enormous count.
    let count = end.saturating_sub(from).min(length - to);
    let backwards = from < to && to < from + count;
    for step in 0..count {
        let (source, target) = match backwards {
            true => (from + count - 1 - step, to + count - 1 - step),
            false => (from + step, to + step),
        };
        // Step 9.c — a hole is not copied as `undefined`, it is *deleted* at the destination, so
        // `copyWithin` moves holes the way `reverse` and `shift` do.
        move_index(vm, heap, object, source, target)?;
    }
    Ok(Value::Object(object))
}

/// §23.1.3.24 `Array.prototype.reverse`, in place.
pub fn reverse(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    for lower in 0..length / 2 {
        let upper = length - lower - 1;
        // Both ends are read before either is written, and a hole on one side becomes a hole on
        // the other — §23.1.3.24 steps 6.f to 6.i, which are four cases rather than a swap.
        let (has_lower, has_upper) = (
            has_index(vm, heap, object, lower)?,
            has_index(vm, heap, object, upper)?,
        );
        let low = get_index(vm, heap, object, lower)?;
        let high = get_index(vm, heap, object, upper)?;
        let (low_key, high_key) = (index_key(heap, lower), index_key(heap, upper));
        match has_upper {
            true => vm.set_property_key(Value::Object(object), low_key, high, heap)?,
            false => vm.delete_property_key(Value::Object(object), low_key, heap)?,
        };
        match has_lower {
            true => vm.set_property_key(Value::Object(object), high_key, low, heap)?,
            false => vm.delete_property_key(Value::Object(object), high_key, heap)?,
        };
    }
    Ok(Value::Object(object))
}

/// §23.1.3.29 `Array.prototype.splice`.
///
/// The one method that removes and inserts at once, and the one whose *answer* is a different
/// array from the one it changed. An absent second argument means "everything from the start",
/// which is not the same as a second argument of `undefined` — that is `ToIntegerOrInfinity`
/// of NaN, which is zero.
pub fn splice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let start = start_index(vm, heap, call.argument(0), length)?;
    let removed_count = match call.arguments.len() {
        0 => 0,
        1 => length - start,
        _ => {
            let count = vm.to_number(call.argument(1), heap)?;
            let integer = if count.is_nan() { 0.0 } else { count.trunc() };
            (integer.max(0.0) as u64).min(length - start)
        }
    };
    // What was taken out, as an Array of its own — holes included, since it is built the same way
    // `slice` builds one.
    let prototype = vm.realm().array_prototype();
    let removed = heap.new_array(prototype, 0);
    for offset in 0..removed_count {
        if has_index(vm, heap, object, start + offset)? {
            let element = get_index(vm, heap, object, start + offset)?;
            set_index(heap, removed, offset, element);
        }
    }
    write_length(vm, heap, removed, removed_count)?;

    let inserted = call.arguments.len().saturating_sub(2) as u64;
    // §23.1.3.28 step 8 — the same rule as `unshift`'s, for the same reason: what is being asked
    // for is a length no index could reach. Checked after the removed elements are collected,
    // because the specification checks it there and the collection can run user code.
    if !fits(spliced_length(length, removed_count, inserted)) {
        return Err(Abrupt::type_error(
            "the resulting array would be longer than 2^53 - 1",
        ));
    }
    // The tail is read out **before** any of it is written back. Moving it in place would work
    // too, and would need the direction to depend on whether the array is growing or shrinking —
    // and the case where it is doing neither moves every element onto itself, so no input could
    // tell a wrong direction from a right one there. Reading first removes the question.
    //
    // `None` is a hole, and it stays one: a hole that travelled would otherwise arrive as
    // `undefined` and `1 in a` would start answering differently.
    //
    // …and only when there is a move to make. §23.1.3.28's steps 14 and 15 are an `if` and an
    // `else if` on whether more is going in than is coming out; when the two are equal *neither*
    // runs. Moving every element onto itself instead is unobservable only in the sense that the
    // values do not change: on an array-like whose `length` is 2^53 it is the rest of the day.
    if inserted != removed_count {
        let mut tail = Vec::new();
        for index in start + removed_count..length {
            tail.push(match has_index(vm, heap, object, index)? {
                true => Some(get_index(vm, heap, object, index)?),
                false => None,
            });
        }
        for (offset, element) in tail.into_iter().enumerate() {
            let name = index_key(heap, start + inserted + offset as u64);
            match element {
                Some(value) => vm.set_property_key(Value::Object(object), name, value, heap)?,
                None => vm.delete_property_key(Value::Object(object), name, heap)?,
            };
        }
    }
    for (at, value) in call.arguments.iter().skip(2).enumerate() {
        let name = index_key(heap, start + at as u64);
        set_or_throw(vm, heap, object, name, *value)?;
    }
    // A shortening leaves indices past the new end and a lengthening leaves none, so the range is
    // empty in the second case rather than needing to be guarded.
    let shortened = length - removed_count + inserted;
    for index in (shortened..length).rev() {
        let name = index_key(heap, index);
        delete_or_throw(vm, heap, object, name)?;
    }
    write_length(vm, heap, object, shortened)?;
    Ok(Value::Object(removed))
}

/// §23.1.3.1 `Array.prototype.concat`.
///
/// An Array argument is *spread* one level and anything else is appended whole, which is
/// §23.1.3.1.1's `IsConcatSpreadable` without the Symbol that can override it. One level: an array
/// inside an array stays an array, which is why `flat` had to be added later.
pub fn concat(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let prototype = vm.realm().array_prototype();
    let joined = heap.new_array(prototype, 0);
    let mut at = 0_u64;
    let mut sources = vec![Value::Object(object)];
    sources.extend_from_slice(call.arguments);
    for source in sources {
        let spreadable = matches!(source, Value::Object(id)
            if heap.object(id).is_some_and(crate::heap::Object::is_array));
        let Value::Object(id) = source else {
            set_index(heap, joined, at, source);
            at += 1;
            continue;
        };
        if !spreadable {
            set_index(heap, joined, at, source);
            at += 1;
            continue;
        }
        let length = length_of(vm, heap, id)?;
        for index in 0..length {
            // A hole in a spread source is a hole in the result, so `[1, , 2].concat(3)` keeps
            // its gap rather than filling it with `undefined`.
            if has_index(vm, heap, id, index)? {
                let element = get_index(vm, heap, id, index)?;
                set_index(heap, joined, at, element);
            }
            at += 1;
        }
    }
    write_length(vm, heap, joined, at)?;
    Ok(Value::Object(joined))
}

/// §23.1.3.8 `Array.prototype.fill`.
pub fn fill(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let value = call.argument(0);
    let from = start_index(vm, heap, call.argument(1), length)?;
    let to = match call.arguments.len() > 2 {
        true => start_index(vm, heap, call.argument(2), length)?,
        false => length,
    };
    for index in from..to.max(from) {
        let name = index_key(heap, index);
        set_or_throw(vm, heap, object, name, value)?;
    }
    Ok(Value::Object(object))
}

/// §23.1.3.19 `Array.prototype.lastIndexOf`.
///
/// Strict equality and backwards, and a hole is skipped rather than compared — the same two rules
/// `indexOf` follows, read from the other end.
pub fn last_index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let wanted = call.argument(0);
    // §23.1.3.19 step 5 — the default start is the *last* index, and a given one is clamped to
    // it rather than to the length. A negative counts from the end and may fall off it.
    let mut from = match call.arguments.len() > 1 {
        false => length,
        true => {
            let number = vm.to_number(call.argument(1), heap)?;
            let integer = if number.is_nan() { 0.0 } else { number.trunc() };
            match integer < 0.0 {
                true => {
                    let start = length as f64 + integer;
                    if start < 0.0 {
                        return Ok(Value::Number(-1.0));
                    }
                    start as u64 + 1
                }
                false => (integer as u64).min(length.saturating_sub(1)) + 1,
            }
        }
    };
    while from > 0 {
        from -= 1;
        if !has_index(vm, heap, object, from)? {
            continue;
        }
        let element = get_index(vm, heap, object, from)?;
        if element.is_strictly_equal(&wanted, heap) {
            return Ok(Value::Number(from as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

/// §23.1.3.16 `Array.prototype.includes`.
///
/// The reason it exists rather than `indexOf(x) !== -1`: it compares with `SameValueZero`, so
/// **NaN matches NaN**. And it does not skip a hole — it reads one as `undefined` — so
/// `[, 1].includes(undefined)` is `true` where `[, 1].indexOf(undefined)` is `-1`.
pub fn includes(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let wanted = call.argument(0);
    let from = start_index(vm, heap, call.argument(1), length)?;
    for index in from..length {
        let element = get_index(vm, heap, object, index)?;
        if same_value_zero(element, wanted, heap) {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

/// §7.2.11 `SameValueZero` — strict equality, except that NaN equals itself.
///
/// The signed zeroes still compare equal, which is the one thing that separates it from
/// `SameValue`: `Object.is(0, -0)` is `false` and `[0].includes(-0)` is `true`.
fn same_value_zero(left: Value, right: Value, heap: &Heap) -> bool {
    if let (Value::Number(left), Value::Number(right)) = (left, right)
        && left.is_nan()
        && right.is_nan()
    {
        return true;
    }
    left.is_strictly_equal(&right, heap)
}

/// §23.1.3.1 `Array.prototype.at`.
///
/// A negative index counts from the end, which is the whole reason it exists — `a[-1]` is a
/// property named `"-1"` and always has been.
pub fn at(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let number = vm.to_number(call.argument(0), heap)?;
    let integer = if number.is_nan() { 0.0 } else { number.trunc() };
    let index = match integer < 0.0 {
        true => length as f64 + integer,
        false => integer,
    };
    // Only the negative half needs a guard: an index past the end reads a property that is not
    // there, and `[[Get]]` already answers `undefined` for one.
    if index < 0.0 {
        return Ok(Value::Undefined);
    }
    get_index(vm, heap, object, index as u64)
}
