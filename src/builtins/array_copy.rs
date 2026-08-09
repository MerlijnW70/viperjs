//! §23.1.3's change-copy methods — `with`, `toReversed` and `toSpliced`.
//!
//! # What "change-copy" means, and why there are four of them rather than three
//!
//! Each of these is an older method with the mutation taken out: `with` is an index assignment,
//! `toReversed` is `reverse`, `toSpliced` is `splice`, and `toSorted` — which lives beside `sort`
//! in [`super::array_sort`] — is `sort`. They were added together (ES2023) for the same reason,
//! which is that the four they mirror are the only methods on `Array.prototype` that change the
//! array they are called on.
//!
//! # Why the copy has no holes
//!
//! Every one of them reads with a plain `Get` and writes with `CreateDataPropertyOrThrow` — there
//! is no `HasProperty` anywhere in their algorithms. So a hole is read as the `undefined` it
//! evaluates to and *written* as a real property: `[, 1].toReversed()` has an element at index 1
//! where the original had a hole. That is not an oversight in the specification, it is the
//! difference between these and `slice`, which does ask and does preserve them.

use super::array_methods::{
    fits, get_index, length_of, new_array_checked, set_index, spliced_length, start_index,
    this_object, within_budget,
};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Copy `count` indices of `object` into `copy`, reading each through the given map.
///
/// The three methods differ only in *which* index of the original each index of the copy comes
/// from, so that is the one thing passed in. Written as a shared walk rather than three loops
/// because the budget check and the read-write pair are what would be copied three times, and one
/// of the three would eventually be the one that forgot.
fn fill_from(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    copy: ObjectId,
    range: std::ops::Range<u64>,
    source: impl Fn(u64) -> u64,
) -> Completion<()> {
    for index in range {
        let element = get_index(vm, heap, object, source(index))?;
        set_index(heap, copy, index, element);
        within_budget(vm, heap)?;
    }
    Ok(())
}

/// §23.1.3.39 `Array.prototype.with` — one index replaced, everything else copied.
pub fn with(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    // Steps 3 and 4 — a negative index counts back from the end, and this is the one relative
    // index in §23.1.3 that is **not** clamped. `[1, 2].with(5, 0)` is a RangeError where
    // `[1, 2].slice(5)` is empty, because there is no index 5 to put the value at. Both
    // infinities land outside the range and are refused by the same comparison.
    let relative = vm.to_number(call.argument(0), heap)?;
    let relative = if relative.is_nan() {
        0.0
    } else {
        relative.trunc()
    };
    let actual = match relative >= 0.0 {
        true => relative,
        false => length as f64 + relative,
    };
    if actual < 0.0 || actual >= length as f64 {
        return Err(Abrupt::range_error("that index is not in the array"));
    }
    let actual = actual as u64;
    let copy = new_array_checked(vm, heap, length)?;
    let replacement = call.argument(1);
    for index in 0..length {
        // Step 8.b — the replacement is used *instead of* reading, so the original is not asked
        // about that index at all. An implementation that read and then overwrote would agree
        // about the answer and disagree about which getters ran.
        let element = match index == actual {
            true => replacement,
            false => get_index(vm, heap, object, index)?,
        };
        set_index(heap, copy, index, element);
        within_budget(vm, heap)?;
    }
    Ok(Value::Object(copy))
}

/// §23.1.3.33 `Array.prototype.toReversed`.
pub fn to_reversed(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let copy = new_array_checked(vm, heap, length)?;
    // Step 5.a — index `k` of the copy is index `len - k - 1` of the original. Reading forwards
    // and writing backwards would answer the same array and would call the getters in the other
    // order, which §23.1.3.33 fixes by walking the destination.
    fill_from(vm, heap, object, copy, 0..length, |index| {
        length - index - 1
    })?;
    Ok(Value::Object(copy))
}

/// §23.1.3.35 `Array.prototype.toSpliced`.
pub fn to_spliced(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let start = start_index(vm, heap, call.argument(0), length)?;
    let inserted = call.arguments.len().saturating_sub(2) as u64;
    // Step 6 — three cases, and they are about how many arguments were *given* rather than what
    // they were. No arguments removes nothing; one removes everything from the start onwards; two
    // or more remove what the second one says. An explicit `undefined` is the third case and not
    // the first, because it was given: `ToIntegerOrInfinity(undefined)` is zero, so it removes
    // nothing, where an absent second argument would have removed the whole tail.
    let removed = match call.arguments.len() {
        0 => 0,
        1 => length - start,
        _ => {
            let count = vm.to_number(call.argument(1), heap)?;
            let integer = if count.is_nan() { 0.0 } else { count.trunc() };
            (integer.max(0.0) as u64).min(length - start)
        }
    };
    let grown = spliced_length(length, removed, inserted);
    // Step 8 is a **TypeError** and step 9's `ArrayCreate` is a RangeError, and they are checked
    // in that order — so an array-like asking for more than 2^53-1 elements is told it asked for
    // too many rather than that an Array cannot be that long.
    if !fits(grown) {
        return Err(Abrupt::type_error(
            "the resulting array would be longer than 2^53 - 1",
        ));
    }
    let copy = new_array_checked(vm, heap, grown)?;
    // Steps 11 to 13 — the head, then what is being inserted, then the tail from past what was
    // removed. The head and the tail are the same walk with a different offset; only the middle
    // comes from the arguments rather than from the array.
    fill_from(vm, heap, object, copy, 0..start, |index| index)?;
    for (offset, value) in call.arguments.iter().skip(2).enumerate() {
        set_index(heap, copy, start + offset as u64, *value);
    }
    let tail = start + inserted;
    fill_from(vm, heap, object, copy, tail..grown, |index| {
        index - tail + start + removed
    })?;
    Ok(Value::Object(copy))
}
