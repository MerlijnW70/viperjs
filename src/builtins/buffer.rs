//! §25.1 — `ArrayBuffer`, which is a block of bytes and nothing else.
//!
//! # What it is for
//!
//! Nothing, by itself. A buffer has no elements, no length in anything but bytes, and no way to
//! read what is in it: every one of those comes from a *view* — a TypedArray or a `DataView` — laid
//! over it. That separation is the whole design, and it is why two views can share one buffer and
//! see each other's writes.
//!
//! # Detaching, which has no syntax
//!
//! A buffer can lose its bytes while the object that held them stays reachable (§25.1.3.3). No
//! operator does it: the host does, and `transfer` does. Everything that reads a buffer therefore
//! has to ask whether it is still there *first*, and the answer can change between two statements
//! of a program that never mentions the buffer. A detached buffer answers `0` for `byteLength`,
//! which makes it indistinguishable from an empty one by that question alone — deliberately, since
//! §25.1.5.1 says so.

use super::{define_method, define_value, key};
use crate::heap::{Buffer, Heap, Native, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `ArrayBuffer` into `heap` as a property of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.array_buffer_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "ArrayBuffer", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, global, "ArrayBuffer", Value::Object(constructor));
    define_value(heap, prototype, "constructor", Value::Object(constructor));

    // §25.1.4.1 — `isView`, which asks about the *view* and not about the buffer. It is on the
    // constructor rather than the prototype because it takes anything at all, including things
    // that are not buffers, and answers `false` rather than throwing.
    define_method(heap, realm, constructor, "isView", 1, is_view);
    define_method(heap, realm, prototype, "slice", 2, slice);
    // §25.1.5.5 — `transfer`, the one operation in the language that detaches a buffer. Without it
    // the state exists and nothing can reach it, which makes every check for it untestable.
    define_method(heap, realm, prototype, "transfer", 0, transfer);

    // §25.1.5.1 and §25.1.5.3 — both accessors, so a program cannot assign either, and both read
    // the buffer each time rather than remembering a number that detaching would make wrong.
    for (name, native) in [
        ("byteLength", byte_length as Native),
        ("detached", detached),
    ] {
        let getter = heap.new_native_function(realm.function_prototype(), native);
        super::define_function_metadata(heap, getter, &format!("get {name}"), 0);
        let key = key(heap, name);
        let _ = heap.define_own_property(
            prototype,
            key,
            &PropertyDescriptor {
                getter: Some(Value::Object(getter)),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    // §25.1.4.3 — `get ArrayBuffer[@@species]` answers the receiver, so `slice` on a subclass makes
    // another of that subclass.
    species(heap, realm, constructor);
    super::collection::tag_with(heap, realm, prototype, "ArrayBuffer");
}

/// §25.1.4.3 — the species accessor, which answers whatever it is read on.
fn species(heap: &mut Heap, realm: &Realm, constructor: ObjectId) {
    let Some(symbol) = realm.well_known(super::well_known_at("species")) else {
        return;
    };
    let getter = heap.new_native_function(realm.function_prototype(), receiver);
    super::define_function_metadata(heap, getter, "get [Symbol.species]", 0);
    let _ = heap.define_own_property(
        constructor,
        PropertyKey::from_symbol(symbol),
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// A getter that answers `this` — §25.1.4.3 and every other `@@species`.
fn receiver(_: &mut Vm, _: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(call.this_value)
}

/// §25.1.3.1 — `new ArrayBuffer(length)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error("ArrayBuffer must be called with new"));
    }
    // §25.1.3.1 step 2 — `ToIndex`, which is not `ToLength`: a negative length or one past
    // 2^53 - 1 is a **RangeError** rather than being clamped, because a buffer that quietly became
    // a different size than asked for is a bug that surfaces somewhere else entirely.
    let length = to_index(vm, heap, call.argument(0))?;
    let prototype = super::prototype_from(heap, call, vm.realm().array_buffer_prototype());
    allocate(heap, prototype, length)
}

/// §25.1.3.1 `AllocateArrayBuffer` — the object and its zeroed bytes.
///
/// The heap's own budget is what refuses an absurd length, rather than a limit written here:
/// DR-0013 says an engine that cannot allocate says so as a RangeError, and a buffer is the
/// easiest thing in the language to ask for too much of.
fn allocate(heap: &mut Heap, prototype: ObjectId, length: usize) -> Completion<Value> {
    // DR-0013 — the budget is what refuses an absurd length rather than a limit written here, and
    // a buffer is the easiest thing in the language to ask too much of. Checked *before* the
    // allocation, because the point is not to make it.
    super::array_methods::within_budget(heap)?;
    // `checked_sub` rather than a comparison, because the boundary of a comparison here is a number
    // no test can reach: the allowance depends on what the heap already holds. Subtracting says the
    // same thing with nothing to be off by one about.
    if heap.allowance().checked_sub(length).is_none() {
        return Err(Abrupt::range_error(
            "this ArrayBuffer is larger than this engine will allocate",
        ));
    }
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_buffer(Buffer::new(length));
    }
    // The bytes count against DR-0013's footprint from here on, so a *second* buffer is measured
    // against a heap that already knows about the first. Without this the check above would let
    // every buffer through, each one measured against an allowance that never moved.
    heap.charge_buffer(length);
    Ok(Value::Object(object))
}

/// §7.1.22 `ToIndex` — a length that must be a non-negative integer inside the safe range.
pub(super) fn to_index(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<usize> {
    // §7.1.22 step 1 gives `undefined` its own line and it needs none: `ToNumber(undefined)` is
    // `NaN`, and `ToIntegerOrInfinity(NaN)` is 0, which is the answer that line asks for. Written
    // out as a case it was a branch no test could tell from its absence.
    let number = vm.to_number(value, heap)?;
    let integer = super::string::to_integer_or_infinity(number);
    // A fractional value is *truncated* and then checked, so `1.5` is 1 and `-0.5` is 0 rather
    // than either being refused. Only the range is a RangeError.
    if !(0.0..=9_007_199_254_740_991.0).contains(&integer) {
        return Err(Abrupt::range_error("this length is not a valid index"));
    }
    Ok(integer as usize)
}

/// The buffer `this` is, or the TypeError §25.1.5 asks for.
fn buffer_of(heap: &Heap, this: Value) -> Completion<ObjectId> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not an ArrayBuffer"));
    };
    match heap.object(object).and_then(crate::heap::Object::buffer) {
        Some(_) => Ok(object),
        None => Err(Abrupt::type_error("this is not an ArrayBuffer")),
    }
}

/// §25.1.5.1 — `get byteLength`, which is **0** for a detached buffer.
fn byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    Ok(Value::Number(length as f64))
}

/// §25.1.5.3 — `get detached`, which is the only way to ask.
///
/// `byteLength` cannot answer it: a detached buffer and an empty one both say 0, deliberately, so
/// this is the question that separates them.
fn detached(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let gone = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached);
    Ok(Value::Boolean(gone))
}

/// §25.1.5.5 — `transfer`, which moves the bytes to a new buffer and detaches this one.
///
/// The bytes are *moved*, not copied: the point is to hand ownership somewhere else without paying
/// for a copy, and the old buffer is left detached so that nothing can read what it no longer has.
/// Every view onto it starts throwing from here.
fn transfer(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let taken = heap
        .object_mut(object)
        .and_then(crate::heap::Object::buffer_mut)
        .and_then(|found| {
            let bytes = found.bytes()?.to_vec();
            found.detach();
            Some(bytes)
        });
    // A buffer that was *already* detached has nothing to transfer, and §25.1.5.5 step 3 says so
    // rather than answering an empty buffer.
    let Some(mut bytes) = taken else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    // Step 5 — an explicit length truncates or zero-extends, so `transfer` is also how a buffer is
    // resized. `undefined` keeps the length it had.
    if let Value::Undefined = call.argument(0) {
    } else {
        let asked = to_index(vm, heap, call.argument(0))?;
        bytes.resize(asked, 0);
    }
    let made = allocate(heap, vm.realm().array_buffer_prototype(), bytes.len())?;
    if let Value::Object(id) = made
        && let Some(target) = heap
            .object_mut(id)
            .and_then(crate::heap::Object::buffer_mut)
            .and_then(crate::heap::Buffer::bytes_mut)
    {
        target.copy_from_slice(&bytes);
    }
    Ok(made)
}

/// §25.1.4.1 — `ArrayBuffer.isView`, which is about DataViews and TypedArrays.
///
/// Answers `false` for everything else including a buffer itself, and never throws: it is the
/// question "may I pass this where a view is wanted", and a wrong shape is an answer rather than
/// an error.
fn is_view(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let view = matches!(call.argument(0), Value::Object(id)
        if heap.object(id).is_some_and(|found| found.view().is_some()));
    Ok(Value::Boolean(view))
}

/// §25.1.5.3 — `slice`, which copies bytes into a **new** buffer.
fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    // Steps 4 and 6 — the two ends are relative, so a negative one counts back from the end and
    // anything past the end is clamped to it. The same rule `Array.prototype.slice` has.
    let from = relative(vm, heap, call.argument(0), length, 0.0)?;
    let to = relative(vm, heap, call.argument(1), length, length as f64)?;
    let taken = to.saturating_sub(from);

    // Step 9 — the *species* constructor, so a subclass gets one of its own back. Built by
    // *calling* it, which a subclass can observe, and before the bytes are copied because calling
    // it may run a program that detaches this buffer.
    let default = vm.realm().array_buffer_constructor();
    let species = super::promise::species_of(vm, heap, object, default)?;
    let made = vm.construct_value(Value::Object(species), &[Value::Number(taken as f64)], heap)?;
    // Steps 11 and 12 — what came back has to be a buffer, a *different* one, and not detached.
    // A species that answered `this` would make `slice` copy a buffer onto itself.
    let same = matches!(made, Value::Object(id) if id == object);
    let ok = matches!(made, Value::Object(id)
        if heap.object(id).and_then(crate::heap::Object::buffer).is_some_and(|found| !found.detached()));
    if same || !ok {
        return Err(Abrupt::type_error(
            "the species of this ArrayBuffer did not make a new one",
        ));
    }

    // Step 14 — detached is checked **after** the new buffer is made, because making it can run a
    // program that detaches this one. Copying then would read bytes that are not there.
    let Some(source) = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::bytes)
        // `from + taken` and not `to`, because §25.1.5.3 step 7's length is `max(final - first, 0)`
        // — the two ends are clamped independently, so `slice(4, 2)` is an *empty* slice and not a
        // backwards one. Written as a range from the two ends it is `bytes[4..2]`, which panics.
        .map(|bytes| {
            let start = from.min(bytes.len());
            bytes[start..(start + taken).min(bytes.len())].to_vec()
        })
    else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    if let Value::Object(id) = made
        && let Some(target) = heap
            .object_mut(id)
            .and_then(crate::heap::Object::buffer_mut)
            .and_then(crate::heap::Buffer::bytes_mut)
            .and_then(|bytes| bytes.get_mut(..source.len()))
    {
        target.copy_from_slice(&source);
    }
    Ok(made)
}

/// §7.1.5 with a relative index — negative counts back from the end, and both ends clamp.
fn relative(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
    length: usize,
    default: f64,
) -> Completion<usize> {
    let number = match value {
        Value::Undefined => default,
        other => super::string::to_integer_or_infinity(vm.to_number(other, heap)?),
    };
    let at = match number < 0.0 {
        true => (length as f64 + number).max(0.0),
        false => number.min(length as f64),
    };
    Ok(at as usize)
}
