//! §23.1.3's methods that run a callback over the elements.
//!
//! # The two questions they disagree about
//!
//! **Does a hole get visited?** `every`, `some`, `reduce` and `reduceRight` skip one — the
//! callback is not run and, for `reduce`, a hole cannot be the initial value. `find` and its three
//! relatives do *not*: they were added long after the others and deliberately read every index, so
//! `[, 1].find(x => x === undefined)` answers `undefined` after visiting index 0 while
//! `[, 1].some(x => x === undefined)` answers `false` without visiting it at all. Neither is a
//! mistake; they are different generations of the same idea.
//!
//! **What does the callback receive?** `every`, `some` and the `find` family get
//! `(element, index, object)` and an optional receiver. `reduce` gets
//! `(accumulator, element, index, object)` and **no** receiver at all — §23.1.3.24 has no
//! `thisArg`, which surprises people who reach for one.

use crate::heap::{Heap, NativeCall};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

use super::array_methods::{callback, get_index, has_index, length_of, this_object};

/// Which end a search or a fold starts from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum From {
    /// Index zero upwards.
    Start,
    /// The last index downwards.
    End,
}

/// §23.1.3.24 `Array.prototype.reduce` and §23.1.3.25 `reduceRight`.
///
/// The two differ in direction and in nothing else, including the argument order handed to the
/// callback: `reduceRight` still calls it `(accumulator, element, …)` rather than swapping them.
fn fold(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, from: From) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let order: Vec<u64> = match from {
        From::Start => (0..length).collect(),
        From::End => (0..length).rev().collect(),
    };
    let mut walk = order.into_iter();
    // §23.1.3.24 step 6 — with no initial value the first *present* element becomes one, which is
    // why a leading hole is not it. With none at all the array is empty as far as this is
    // concerned, and step 7's TypeError is the only thing a fold can answer.
    let mut total = match call.arguments.len() > 1 {
        true => call.argument(1),
        false => {
            let mut found = None;
            for index in walk.by_ref() {
                if has_index(vm, heap, object, index)? {
                    found = Some(get_index(vm, heap, object, index)?);
                    break;
                }
            }
            match found {
                Some(value) => value,
                None => {
                    return Err(Abrupt::type_error(
                        "reduce of an empty array with no initial value",
                    ));
                }
            }
        }
    };
    for index in walk {
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [
            total,
            element,
            Value::Number(index as f64),
            Value::Object(object),
        ];
        // §23.1.3.24 step 8.c.ii — no receiver. `reduce` is the one callback method with no
        // `thisArg`, so a callback that needs one has to close over it.
        total = vm.call_value(function, Value::Undefined, &arguments, heap)?;
    }
    Ok(total)
}

/// §23.1.3.24 `Array.prototype.reduce`.
pub fn reduce(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    fold(vm, heap, call, From::Start)
}

/// §23.1.3.25 `Array.prototype.reduceRight`.
pub fn reduce_right(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    fold(vm, heap, call, From::End)
}

/// §23.1.3.6 `every` and §23.1.3.28 `some`, which are one algorithm and two answers.
///
/// `wanted` is what the callback must say for the search to stop. `every` stops on a falsy answer
/// and reports `false`; `some` stops on a truthy one and reports `true`. What is left when nothing
/// stopped it is the opposite — so `[].every(f)` is `true` and `[].some(f)` is `false`, which is
/// what "vacuously" means and what a program relying on either had better expect.
fn quantify(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    wanted: bool,
) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    for index in 0..length {
        if !has_index(vm, heap, object, index)? {
            continue;
        }
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        let answer = vm.call_value(function, receiver, &arguments, heap)?;
        if answer.to_boolean(heap) == wanted {
            return Ok(Value::Boolean(wanted));
        }
    }
    Ok(Value::Boolean(!wanted))
}

/// §23.1.3.6 `Array.prototype.every`.
pub fn every(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    quantify(vm, heap, call, false)
}

/// §23.1.3.28 `Array.prototype.some`.
pub fn some(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    quantify(vm, heap, call, true)
}

/// §23.1.3.9 to §23.1.3.12 — `find`, `findIndex`, `findLast` and `findLastIndex`.
///
/// One walk with two switches: which end it starts from, and whether it answers the element or its
/// index. Unlike everything above, a hole is **visited**: these read every index and hand the
/// callback `undefined` for one, which is why `[, 1].findIndex(x => x === undefined)` is 0.
fn search(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    from: From,
    want_index: bool,
) -> Completion<Value> {
    let object = this_object(call)?;
    let length = length_of(vm, heap, object)?;
    let function = callback(call, heap)?;
    let receiver = call.argument(1);
    let order: Vec<u64> = match from {
        From::Start => (0..length).collect(),
        From::End => (0..length).rev().collect(),
    };
    for index in order {
        let element = get_index(vm, heap, object, index)?;
        let arguments = [element, Value::Number(index as f64), Value::Object(object)];
        let answer = vm.call_value(function, receiver, &arguments, heap)?;
        if answer.to_boolean(heap) {
            return Ok(match want_index {
                true => Value::Number(index as f64),
                false => element,
            });
        }
    }
    // Nothing matched, and the two shapes say so differently: `-1` is an index that cannot exist
    // and `undefined` is an element that can, which is why `findIndex` is the safe one to test.
    Ok(match want_index {
        true => Value::Number(-1.0),
        false => Value::Undefined,
    })
}

/// §23.1.3.9 `Array.prototype.find`.
pub fn find(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, From::Start, false)
}

/// §23.1.3.10 `Array.prototype.findIndex`.
pub fn find_index(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, From::Start, true)
}

/// §23.1.3.11 `Array.prototype.findLast`.
pub fn find_last(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, From::End, false)
}

/// §23.1.3.12 `Array.prototype.findLastIndex`.
pub fn find_last_index(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, From::End, true)
}
