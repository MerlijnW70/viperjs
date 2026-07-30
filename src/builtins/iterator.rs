//! §27.1, §23.1.5 and §22.1.5 — the iterator objects, and the prototype chain they share.
//!
//! # Three prototypes and why they are three
//!
//! %IteratorPrototype% has one method: `[@@iterator]() { return this }`. That is the whole of what
//! makes an iterator *iterable*, and it is why `for (const x of someIterator)` works — the loop
//! asks for an iterator and the iterator answers itself.
//!
//! %ArrayIteratorPrototype% and %StringIteratorPrototype% inherit from it and add a `next`. They
//! are separate objects rather than one because a script may replace either `next` without
//! touching the other, and because `@@toStringTag` names them differently. Nothing else about them
//! differs, which is why one private `step` serves both.
//!
//! # What `next` may be given
//!
//! Anything, and it refuses everything that is not one of its own iterators — §23.1.5.2.1 step 2
//! is a `RequireInternalSlot`. So `Array.prototype.values.call([]).next.call({})` is a TypeError
//! rather than an answer about an object that merely looks similar. The position an iterator keeps
//! is a slot for the same reason; see [`crate::heap::Iteration`].

use super::{define_method, key};
use crate::heap::{Heap, Iterated, Iteration, NativeCall, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build the three iterator prototypes into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm) {
    // §27.1.2.1 — `[@@iterator]` answers the receiver. An iterator is iterable, which is what lets
    // one be passed anywhere an iterable is wanted.
    let shared = realm.iterator_prototype();
    let itself = heap.new_native_function(realm.function_prototype(), same);
    super::define_function_metadata(heap, itself, "[Symbol.iterator]", 0);
    if let Some(symbol) = realm.well_known(super::well_known_at("iterator")) {
        let name = crate::heap::PropertyKey::from_symbol(symbol);
        let _ = heap.define_own_property(
            shared,
            name,
            &PropertyDescriptor {
                value: Some(Value::Object(itself)),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }

    for (prototype, tag) in [
        (realm.array_iterator_prototype(), "Array Iterator"),
        (realm.string_iterator_prototype(), "String Iterator"),
    ] {
        define_method(heap, realm, prototype, "next", 0, next);
        // §23.1.5.2.2 and §22.1.5.2.2 — the tag is what tells the two apart in a message, and it
        // is the only thing that does: they are otherwise the same shape.
        let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) else {
            continue;
        };
        let name = crate::heap::PropertyKey::from_symbol(symbol);
        let units: Vec<u16> = tag.encode_utf16().collect();
        let value = Value::String(heap.intern(&units));
        let _ = heap.define_own_property(
            prototype,
            name,
            &PropertyDescriptor {
                value: Some(value),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}

/// §27.1.2.1 `%IteratorPrototype%[@@iterator]` — the receiver, whatever it is.
///
/// Does not check what it was given. §27.1.2.1 is one step long and has nothing to check: it
/// answers `this`, and an object that is not an iterator gets itself back and fails later, where
/// the failure names the right thing.
fn same(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(call.this_value)
}

/// §23.1.5.2.1 and §22.1.5.2.1 — `next` for both kinds of iterator.
fn next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(iterator) = call.this_value else {
        return Err(Abrupt::type_error("this method requires an iterator"));
    };
    // Step 2's `RequireInternalSlot`. An object that merely has the right prototype is not one of
    // these, and answering for it would mean inventing a position it never had.
    let Some(state) = heap
        .object(iterator)
        .and_then(crate::heap::Object::iteration)
    else {
        return Err(Abrupt::type_error("this method requires an iterator"));
    };
    let state = state.clone();
    let Some((value, at)) = step(vm, heap, &state)? else {
        // Once done, done: a later `next` must not start finding things again because the target
        // grew back. §23.1.5.2.1 step 4.b sets the kind to empty, and this is that.
        if let Some(found) = heap
            .object_mut(iterator)
            .and_then(crate::heap::Object::iteration_mut)
        {
            found.done = true;
        }
        return result(vm, heap, Value::Undefined, true);
    };
    if let Some(found) = heap
        .object_mut(iterator)
        .and_then(crate::heap::Object::iteration_mut)
    {
        found.at = at;
    }
    result(vm, heap, value, false)
}

/// One step: what this iterator answers next, and where it has got to, or `None` if it is spent.
fn step(vm: &mut Vm, heap: &mut Heap, state: &Iteration) -> Completion<Option<(Value, u64)>> {
    if state.done {
        return Ok(None);
    }
    match state.kind {
        Iterated::Characters => Ok(character(heap, state)),
        kind => indexed(vm, heap, state, kind),
    }
}

/// §22.1.5.1 — the code point at this position, and the position after it.
///
/// Code *points*, so a surrogate pair is one step. That is what makes `for`-`of` over a String
/// different from a `for` over its indices, and it is why this cannot be an Array Iterator with a
/// String inside it.
fn character(heap: &mut Heap, state: &Iteration) -> Option<(Value, u64)> {
    let Value::String(data) = state.over else {
        return None;
    };
    let units = heap.string(data)?.to_vec();
    let at = usize::try_from(state.at).ok()?; // a position past `usize` is past the string, and that is the answer
    let first = *units.get(at)?;
    // A leading surrogate followed by a trailing one is one code point and two units; anything
    // else, a lone surrogate included, is one of each.
    let paired = (0xD800..0xDC00).contains(&first)
        && units
            .get(at + 1)
            .is_some_and(|next| (0xDC00..0xE000).contains(next));
    let width = 1 + usize::from(paired);
    let taken = units.get(at..at + width)?.to_vec();
    let id = heap.intern(&taken);
    Some((Value::String(id), state.at + width as u64))
}

/// §23.1.5.2.1 — the key, the value, or both, at this position.
fn indexed(
    vm: &mut Vm,
    heap: &mut Heap,
    state: &Iteration,
    kind: Iterated,
) -> Completion<Option<(Value, u64)>> {
    let name = key(heap, "length");
    let length = vm.get_property_key(state.over, name, heap)?;
    // §23.1.5.2.1 step 6 reads `length` *every* step, not once — so an array that shrinks while
    // being walked stops early, and one that grows keeps going. That is observable and is the
    // reason this is not a count taken at the start.
    let length = super::array_methods::to_length(vm.to_number(length, heap)?);
    if state.at >= length {
        return Ok(None);
    }
    let index = Value::Number(state.at as f64);
    let value = match kind {
        Iterated::Keys => index,
        _ => {
            let at = super::array_methods::index_key(heap, state.at);
            let element = vm.get_property_key(state.over, at, heap)?;
            match kind {
                Iterated::Entries => super::array::from_values(vm, heap, &[index, element])?,
                _ => element,
            }
        }
    };
    Ok(Some((value, state.at + 1)))
}

/// §7.4.13 `CreateIterResultObject` — `{value, done}`, ordinary in every way.
pub(super) fn result(vm: &mut Vm, heap: &mut Heap, value: Value, done: bool) -> Completion<Value> {
    let object = heap.new_object(Some(vm.realm().object_prototype()));
    for (name, held) in [("value", value), ("done", Value::Boolean(done))] {
        let name = key(heap, name);
        let _ = heap.define_own_property(
            object,
            name,
            &PropertyDescriptor {
                value: Some(held),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    Ok(Value::Object(object))
}

/// §23.1.5.1 `CreateArrayIterator`, for the three `Array.prototype` methods that make one.
pub(super) fn over_array(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: Iterated,
) -> Completion<Value> {
    // Step 1's `ToObject`, so `Array.prototype.values.call("ab")` walks a String object's
    // characters by index — which is not the same as a String's own iterator, and is right.
    let over = vm.object_for(call.this_value, heap)?;
    let prototype = vm.realm().array_iterator_prototype();
    Ok(Value::Object(heap.new_iterator(
        prototype,
        Iteration {
            over,
            at: 0,
            kind,
            done: false,
        },
    )))
}

/// §22.1.3.34 `String.prototype[@@iterator]` — `CreateStringIterator`.
pub(super) fn over_string(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    if matches!(call.this_value, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "this method cannot be called on undefined or null",
        ));
    }
    let data = vm.to_string(call.this_value, heap)?;
    let prototype = vm.realm().string_iterator_prototype();
    Ok(Value::Object(heap.new_iterator(
        prototype,
        Iteration {
            over: Value::String(data),
            at: 0,
            kind: Iterated::Characters,
            done: false,
        },
    )))
}
