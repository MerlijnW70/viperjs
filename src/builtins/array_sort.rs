//! §23.1.3.30 `Array.prototype.sort` and §23.1.3.34 `toSorted`, and the ordering they share.
//!
//! # Why the default order is alphabetical
//!
//! `[1, 5, 10].sort()` answers `[1, 10, 5]`, and that is not a bug in anyone's engine — it is
//! §23.1.3.30.2 steps 5 to 11, which convert both elements with `ToString` and compare the
//! spellings. The default comparator has no idea what a number is. Every other order is a
//! comparator the caller supplies.
//!
//! # Why holes and `undefined` are handled before the comparator sees anything
//!
//! §23.1.3.30.2 steps 1 to 3 put `undefined` last, and §23.1.3.30.1 leaves holes out of the list
//! entirely — so a comparator is never called with `undefined`, and never called about a hole.
//! Both then reappear at the end of the result: the `undefined`s because the ordering put them
//! there, the holes because [`sort`] deletes the indices past what it wrote. That is why a
//! comparator may assume its two arguments are real elements, and why counting the calls a sort
//! makes is a test about which elements were gathered rather than about the algorithm.
//!
//! # Why the sort is stable
//!
//! §23.1.3.30 has required it since ES2019: two elements the comparator calls equal come out in
//! the order they went in. That rules out the obvious quicksort and it rules out the insertion
//! sort a smaller collection can get away with — an array-like may be millions of elements long,
//! and a quadratic sort on one is not slow, it is a hang. A bottom-up merge sort is stable by
//! construction, needs no recursion, and makes `n log n` comparisons whatever the input looks
//! like.

use std::cmp::Ordering;

use super::array_methods::{
    get_index, has_index, index_key, length_of, new_array_checked, set_index, this_object,
    within_budget,
};
use super::{delete_or_throw, key, set_or_throw};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Whether §23.1.3.30.1 gathers the missing indices or leaves them out.
///
/// The one difference between the two methods' lists, and it is why `sort` can answer with holes
/// and `toSorted` never does: `sort` skips them and then deletes the tail it did not write, while
/// `toSorted` reads each one as the `undefined` an absent property gets and sorts that.
#[derive(Clone, Copy)]
enum Holes {
    /// `skip-holes` — an absent index contributes nothing to the list.
    Skip,
    /// `read-through-holes` — an absent index reads as `undefined` and sorts with the rest.
    ReadThrough,
}

/// The comparator argument, refused here rather than where it would be called — §23.1.3.30 step 1.
///
/// Checked before `this` is even looked at, so `Array.prototype.sort.call(null, 1)` is a TypeError
/// about the comparator and not about `null`. An absent comparator is the default order; a present
/// one that is not callable is a mistake worth naming.
fn comparator(call: &NativeCall<'_>, heap: &Heap) -> Completion<Value> {
    let given = call.argument(0);
    if matches!(given, Value::Undefined) {
        return Ok(Value::Undefined);
    }
    let callable = matches!(given, Value::Object(object)
        if heap.object(object).is_some_and(|found| found.call().is_some()));
    match callable {
        true => Ok(given),
        false => Err(Abrupt::type_error("the comparator is not a function")),
    }
}

/// §23.1.3.30.2 `CompareArrayElements`, which answers a number rather than an ordering.
///
/// A number because that is what a comparator answers and there is nothing to gain by narrowing
/// it: only the sign is ever read. `NaN` is folded to zero here rather than at the one place that
/// reads it, because step 4.b says the *comparison* is zero — a comparator answering nonsense
/// makes every element equal to every other, which terminates, rather than making the merge
/// depend on which side a `NaN` comparison fell.
fn compare_elements(
    vm: &mut Vm,
    heap: &mut Heap,
    left: Value,
    right: Value,
    comparator: Value,
) -> Completion<f64> {
    // Steps 1 to 3 — `undefined` sorts to the end, and it does so *before* the comparator is
    // consulted, so no comparator is ever called with one. Two of them are equal, which is what
    // keeps a run of them stable rather than reversed.
    match (left, right) {
        (Value::Undefined, Value::Undefined) => return Ok(0.0),
        (Value::Undefined, _) => return Ok(1.0),
        (_, Value::Undefined) => return Ok(-1.0),
        _ => {}
    }
    if !matches!(comparator, Value::Undefined) {
        // Step 4 — the comparator may run anything at all, including code that changes the array
        // being sorted. That is why the list was gathered first: what is being sorted is the list,
        // and what the comparator does to the object underneath it changes nothing here.
        let answer = vm.call_value(comparator, Value::Undefined, &[left, right], heap)?;
        let order = vm.to_number(answer, heap)?;
        return Ok(if order.is_nan() { 0.0 } else { order });
    }
    // Steps 5 to 11 — both spellings, then §7.2.12 step 3's comparison, which is by **code unit**.
    // Not by character and not by locale: a lone surrogate is a code unit like any other, and
    // ordering a pair of them the way their code *points* read would disagree with `<`.
    let left = vm.to_string(left, heap)?;
    let right = vm.to_string(right, heap)?;
    // A String this heap does not know is impossible here — both were made a line ago — and
    // reading it as empty is the answer that keeps the ordering total rather than a panic.
    let order = heap
        .string(left)
        .unwrap_or(&[])
        .cmp(heap.string(right).unwrap_or(&[]));
    Ok(match order {
        Ordering::Less => -1.0,
        Ordering::Greater => 1.0,
        Ordering::Equal => 0.0,
    })
}

/// How many merge passes a list of `count` elements needs — `ceil(log2(count))`.
///
/// Its own function so the bound can be *stated*. Written as a comparison in the loop it drives,
/// `while width < count` and `while width <= count` sort identically: the extra pass merges the
/// whole list with an empty range and copies it unchanged, so no input distinguishes them and no
/// test could. Here the answer is a number, and the table below pins every case where doubling
/// crosses a power of two — which is the same trick §23.1.3's length rules use, for the same
/// reason.
const fn passes(count: usize) -> u32 {
    match count {
        // Nothing to merge: a list this short is sorted, which is why `[].sort()` and `[1].sort()`
        // never call the comparator.
        0 | 1 => 0,
        // The number of doublings needed to reach `count`, counted off the leading zeros of the
        // last index rather than by looping — `count - 1` is what makes a power of two need one
        // pass fewer than the size above it.
        _ => usize::BITS - (count - 1).leading_zeros(),
    }
}

/// A stable sort whose comparator may throw — §23.1.3.30.1 step 4.
///
/// Bottom-up: runs of one are merged into runs of two, those into runs of four, and so on. Each
/// pass writes the whole list into the spare buffer and the two swap, so there is one extra list
/// alive rather than one per level. Nothing here recurses, so no input reaches it by depth.
///
/// "Stop before performing any further calls" is what `?` does — an abrupt comparator ends the
/// sort where it stood, and the half-merged buffer is dropped rather than written back.
fn merge_sort(
    vm: &mut Vm,
    heap: &mut Heap,
    mut items: Vec<Value>,
    comparator: Value,
) -> Completion<Vec<Value>> {
    let count = items.len();
    let mut buffer: Vec<Value> = Vec::with_capacity(count);
    let mut width = 1;
    for _ in 0..passes(count) {
        buffer.clear();
        let mut start = 0;
        while start < count {
            let middle = (start + width).min(count);
            let end = (start + width * 2).min(count);
            let (mut left, mut right) = (start, middle);
            while left < middle && right < end {
                let order = compare_elements(vm, heap, items[left], items[right], comparator)?;
                // `<= 0` rather than `< 0`, and that is the whole of stability: when the two are
                // equal the one that was already earlier goes first. Taking the right one here
                // would sort just as correctly and would reverse every run of equal elements.
                match order <= 0.0 {
                    true => {
                        buffer.push(items[left]);
                        left += 1;
                    }
                    false => {
                        buffer.push(items[right]);
                        right += 1;
                    }
                }
            }
            // Whichever side still has elements is already in order and already after everything
            // written — one of these two ranges is always empty.
            buffer.extend_from_slice(&items[left..middle]);
            buffer.extend_from_slice(&items[right..end]);
            start = end;
        }
        std::mem::swap(&mut items, &mut buffer);
        width *= 2;
        within_budget(heap)?;
    }

    Ok(items)
}

/// §23.1.3.30.1 `SortIndexedProperties` — read the elements out, then sort the list.
///
/// The reading happens in full before the first comparison, which is what makes a comparator that
/// mutates the array harmless: it can change the object all it likes and the list it is being
/// asked about is already a copy.
fn sorted_list(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    length: u64,
    comparator: Value,
    holes: Holes,
) -> Completion<Vec<Value>> {
    // The list is engine memory rather than heap memory, so DR-0013 does not see it directly —
    // and it does not need to. Every index walked interns its decimal spelling as a String, which
    // costs the heap more per element than the list costs the engine, so a walk long enough to
    // exhaust one has already exhausted the other. `within_budget` is what notices, and it is the
    // same door every other walk in §23.1.3 goes through.
    let mut items = Vec::new();
    // Reading through holes gathers exactly `length` elements, so how much that will cost is known
    // before the first one is read — and asking now refuses an impossible length in no time at all
    // rather than after four million property lookups have proved it. Skipping them cannot be
    // asked in advance: how many of the indices are actually there is what the loop finds out.
    for index in 0..length {
        let read = match holes {
            Holes::Skip => has_index(vm, heap, object, index)?,
            Holes::ReadThrough => true,
        };
        if read {
            items.push(get_index(vm, heap, object, index)?);
        }
        within_budget(heap)?;
    }
    merge_sort(vm, heap, items, comparator)
}

/// §23.1.3.30 `Array.prototype.sort`, in place.
pub fn sort(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let comparator = comparator(call, heap)?;
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let sorted = sorted_list(vm, heap, object, length, comparator, Holes::Skip)?;
    let written = sorted.len() as u64;
    for (index, element) in sorted.into_iter().enumerate() {
        let name = index_key(heap, index as u64);
        set_or_throw(vm, heap, object, name, element)?;
    }
    // Steps 9 — the holes went nowhere, they were left out. The indices they used to occupy are
    // the ones past everything written, and deleting them is what puts the holes at the end.
    for index in written..length {
        let name = index_key(heap, index);
        delete_or_throw(vm, heap, object, name)?;
    }
    Ok(Value::Object(object))
}

/// §23.1.3.34 `Array.prototype.toSorted`, which leaves its argument alone.
pub fn to_sorted(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let comparator = comparator(call, heap)?;
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    // Step 4 is `ArrayCreate(len)`, which refuses a length no Array could have — see
    // [`new_array_checked`]. Made *before* the elements are gathered, because that is where the
    // specification puts it and because the refusal is the cheaper of the two.
    let copy = new_array_checked(vm, heap, length)?;
    // `read-through-holes`, so the list has exactly `length` elements and every index of the copy
    // is written — which is why `toSorted` answers a dense array however sparse it was given one.
    let sorted = sorted_list(vm, heap, object, length, comparator, Holes::ReadThrough)?;
    for (index, element) in sorted.into_iter().enumerate() {
        set_index(heap, copy, index as u64, element);
    }
    // The elements were defined rather than set, so nothing has moved `length` — it was given at
    // creation and says the same thing whether or not the last elements were `undefined`.
    let name = key(heap, "length");
    let count = Value::Number(length as f64);
    set_or_throw(vm, heap, copy, name, count)?;
    Ok(Value::Object(copy))
}

#[cfg(test)]
mod tests {
    use super::passes;

    #[test]
    fn a_list_needs_one_merge_pass_per_doubling_that_does_not_reach_it() {
        // Every case where the answer changes, and the two on either side of each. A power of two
        // needs one pass fewer than the size just above it — that is the `count - 1`, and it is
        // the only part of this that is easy to write the other way round.
        for (count, expected) in [
            (0, 0),
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 3),
            (7, 3),
            (8, 3),
            (9, 4),
            (16, 4),
            (17, 5),
        ] {
            assert_eq!(passes(count), expected, "passes({count})");
        }
        // Enough passes to halve the largest list there could be, and never more: a bound that
        // overshot would merge an already-sorted list again, and one that undershot would leave it
        // in runs. Neither is reachable by running anything, which is why it is asked here.
        assert_eq!(passes(usize::MAX), usize::BITS);
        assert_eq!(passes(usize::MAX / 2 + 1), usize::BITS - 1);
    }
}
