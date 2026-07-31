//! §23.1.3.13 `flat`, §23.1.3.14 `flatMap` and §23.1.3.32 `toLocaleString` — the rest of §23.1.3.
//!
//! # Why flattening is not recursive here
//!
//! §23.1.3.13.1 `FlattenIntoArray` calls itself, once per level of nesting, and the depth it is
//! asked for may be `+∞`. The nesting of the *data* then decides how deep the recursion goes —
//! `[[[[…]]]]` a million levels deep is a million frames — and DR-0002 does not allow a program to
//! end the process by handing over a deeply nested array. That is the same argument the collector's
//! mark phase makes, and it has the same answer: an explicit stack, which grows on the heap where
//! the budget can see it.
//!
//! So what the specification writes as a recursive call is a frame pushed onto a list here, and
//! "return targetIndex" is that list being popped. The order elements are visited in is identical,
//! which is what matters: every `Get` and every `HasProperty` happens where §23.1.3.13.1 says.
//!
//! # The one step that is not here
//!
//! §23.1.3.13.1 step 3.c.vi.1 throws a TypeError when the index being written reaches 2^53-1, and
//! that check is absent because no input reaches it. `written` counts *actual writes*, and every
//! one of them defines a property on the heap — so DR-0013's budget stops the walk at around a
//! million, six orders of magnitude short. A guard nothing can reach is a branch no test can tell
//! from its absence, and the sort in [`super::array_sort`] reached the same conclusion about a
//! bound of its own. What a program meets instead is the RangeError the budget raises.
//!
//! # Species
//!
//! §23.1.3.13 step 5 is `ArraySpeciesCreate`, and praxis makes an ordinary Array instead — as it
//! does in `map`, `filter` and `slice`. That is one unimplemented abstract operation rather than a
//! decision taken here, and it is what the suite's `Symbol.species` rows are about.

use super::array_methods::{
    array_species_create, callback, create_index, get_index, has_index, length_of, this_object,
    within_budget,
};
use super::{key, set_or_throw};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// One level of §23.1.3.13.1, part-way through.
///
/// What a recursive call would have kept in its frame: which array is being read, how far along it
/// is, how long it is, and how much further it may still flatten. `source` is the array-like the
/// level is walking — the *target* is the same one at every level, so it is not here.
struct Level {
    /// The array-like this level is reading.
    source: ObjectId,
    /// How far through it the level has got.
    at: u64,
    /// How many indices it has — read once, as §23.1.3.13.1 step 3.c.v.2 does.
    length: u64,
    /// How many more levels may be flattened.
    ///
    /// A plain count, and `+∞` is `u64::MAX`. Telling those apart would take 2^64 levels of
    /// nesting — far past what the heap budget allows, and past what any machine could hold — so a
    /// separate case for infinity is a branch no input can distinguish from the count. The version
    /// that had one produced exactly that: a mutant the whole suite could not kill.
    depth: u64,
}

impl Level {
    /// The depth the level below would get, or `None` when this level may not flatten at all.
    ///
    /// Step 3.c.iv and 3.c.v.1 in one subtraction: nought cannot go lower, and everything else
    /// goes one lower. `checked_sub` is both of those, which is why there is no comparison here to
    /// get the wrong way round.
    fn descend(&self) -> Option<u64> {
        self.depth.checked_sub(1)
    }
}

/// §23.1.3.13.1 `FlattenIntoArray`, with the recursion written as a stack.
///
/// `mapper` is the `flatMap` case: present, it is called for every element of the **outermost**
/// level only, because §23.1.3.14 passes a depth of 1 and the mapper is not handed down. Written
/// as one function rather than two because the two differ in exactly that one place, and a copy
/// would be a second walk to keep in step.
fn flatten(
    vm: &mut Vm,
    heap: &mut Heap,
    target: ObjectId,
    source: ObjectId,
    length: u64,
    depth: u64,
    mapper: Option<(Value, Value)>,
) -> Completion<u64> {
    let mut written = 0_u64;
    let mut levels = vec![Level {
        source,
        at: 0,
        length,
        depth,
    }];
    while let Some(level) = levels.last_mut() {
        if level.at >= level.length {
            levels.pop();
            continue;
        }
        let (reading, index) = (level.source, level.at);
        level.at += 1;
        within_budget(heap)?;
        // Step 3.b — a hole contributes nothing at all, and is not flattened into an `undefined`.
        if !has_index(vm, heap, reading, index)? {
            continue;
        }
        let mut element = get_index(vm, heap, reading, index)?;
        // Step 3.c.ii — the mapper runs at the top level only, which is the whole of the
        // difference between `flatMap` and `flat(1)`.
        if let Some((function, receiver)) = mapper
            && levels.len() == 1
        {
            let arguments = [element, Value::Number(index as f64), Value::Object(reading)];
            element = vm.call_value(function, receiver, &arguments, heap)?;
        }
        // Step 3.c.iv — only a real Array is flattened. An array-*like* is not, however many
        // indices it has, which is what makes `[{length: 2}].flat()` answer a one-element array.
        let deeper = levels
            .last()
            .and_then(Level::descend)
            .filter(|_| matches!(element, Value::Object(id) if heap.object(id).is_some_and(|found| found.is_array())));
        match (deeper, element) {
            (Some(depth), Value::Object(nested)) => {
                let length = length_of(vm, heap, nested)?;
                levels.push(Level {
                    source: nested,
                    at: 0,
                    length,
                    depth,
                });
            }
            _ => {
                create_index(heap, target, written, element)?;
                written += 1;
            }
        }
    }
    Ok(written)
}

/// The length a flattened result should be given, written back explicitly.
///
/// `set_index` defines properties and never moves `length`, so an empty result would otherwise
/// keep whatever the array was created with. Every method here creates it with nought, so this is
/// what makes `[].flat().length` answer nought rather than nothing at all.
fn finish(vm: &mut Vm, heap: &mut Heap, array: ObjectId, written: u64) -> Completion<Value> {
    let name = key(heap, "length");
    let count = Value::Number(written as f64);
    set_or_throw(vm, heap, array, name, count)?;
    Ok(Value::Object(array))
}

/// §23.1.3.13 `Array.prototype.flat`.
pub fn flat(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    // Step 4 — an absent depth is **one**, not zero, which is why `flat()` flattens a level. A
    // negative one is clamped to zero, and `+∞` is the case with no count at all.
    let depth = match call.argument(0) {
        Value::Undefined => 1,
        given => {
            let asked = vm.to_number(given, heap)?;
            let integer = if asked.is_nan() { 0.0 } else { asked.trunc() };
            // No clamp and no case for infinity: a Rust cast from `f64` to an integer *saturates*,
            // so a negative depth and `-∞` both become nought and `+∞` becomes the largest count
            // there is. Both of the guards that used to be here answered exactly what the cast
            // already answers, and the suite could kill neither.
            integer as u64
        }
    };
    let Value::Object(flattened) = array_species_create(vm, heap, object, 0)? else {
        return Err(Abrupt::type_error(
            "the species of this array did not make an object",
        ));
    };
    let written = flatten(vm, heap, flattened, object, length, depth, None)?;
    finish(vm, heap, flattened, written)
}

/// §23.1.3.14 `Array.prototype.flatMap`.
pub fn flat_map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    // Step 3 — checked before anything is read, so an empty array with a bad mapper still throws.
    let mapper = callback(call, heap)?;
    let Value::Object(flattened) = array_species_create(vm, heap, object, 0)? else {
        return Err(Abrupt::type_error(
            "the species of this array did not make an object",
        ));
    };
    // A depth of exactly one: `flatMap` maps and then flattens once, and never further, whatever
    // the mapper answered.
    let written = flatten(
        vm,
        heap,
        flattened,
        object,
        length,
        1,
        Some((mapper, call.argument(1))),
    )?;
    finish(vm, heap, flattened, written)
}

/// §23.1.3.32 `Array.prototype.toLocaleString`.
///
/// The core language's version, which ECMA-402 replaces when it is present. It differs from `join`
/// in two ways and they are both about §23.1.3.32 step 6.c: the separator is implementation-defined
/// rather than an argument, and each element is converted by **calling its `toLocaleString`**
/// rather than by `ToString` — so an object with one has it called, and one without gets the
/// TypeError that calling `undefined` earns.
pub fn to_locale_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(vm, heap, call)?;
    let length = length_of(vm, heap, object)?;
    let name = key(heap, "toLocaleString");
    let mut joined = String::new();
    for index in 0..length {
        if index > 0 {
            joined.push(',');
        }
        // Step 6.b is a plain `Get`, so a hole reads as `undefined` and step 6.c then skips it —
        // which is why a hole and an `undefined` both contribute nothing but still take a comma.
        let element = get_index(vm, heap, object, index)?;
        if matches!(element, Value::Undefined | Value::Null) {
            continue;
        }
        let method = vm.get_property_key(element, name, heap)?;
        let text = vm.call_value(method, element, &[], heap)?;
        let id = vm.to_string(text, heap)?;
        joined.push_str(&String::from_utf16_lossy(heap.string(id).unwrap_or(&[])));
        within_budget(heap)?;
    }
    Ok(super::text(heap, &joined))
}
