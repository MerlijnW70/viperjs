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
    let constructor =
        heap.new_native_constructor(realm.function_prototype(), construct, realm.id());
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
    // §25.1.6.4 — the only operation that changes a buffer's length in place. `grow` is §25.2's
    // spelling of it and each refuses the other's kind of buffer, which is why they are two names
    // for what is one operation underneath.
    define_method(heap, realm, prototype, "resize", 1, resize);
    // §25.1.5.6 — the same transfer with the resizability dropped, which is the only way to turn a
    // resizable buffer into a fixed one.
    define_method(
        heap,
        realm,
        prototype,
        "transferToFixedLength",
        0,
        transfer_to_fixed_length,
    );

    // §25.1.5.1 and §25.1.5.3 — both accessors, so a program cannot assign either, and both read
    // the buffer each time rather than remembering a number that detaching would make wrong.
    for (name, native) in [
        ("byteLength", byte_length as Native),
        ("detached", detached),
        // §25.1.6.2 and §25.1.6.3. Both answer about `[[ArrayBufferMaxByteLength]]`, and
        // `maxByteLength` answers for a *fixed* buffer too — its current length, because a buffer
        // that cannot be resized is already as long as it may ever be.
        ("resizable", resizable),
        ("maxByteLength", max_byte_length),
    ] {
        let getter = heap.new_native_function(realm.function_prototype(), native, realm.id());
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
    define_species(heap, realm, constructor);
    super::tag_with(heap, prototype, "ArrayBuffer");
}

/// §25.1.4.3 — the species accessor, which answers whatever it is read on.
pub(super) fn define_species(heap: &mut Heap, realm: &Realm, constructor: ObjectId) {
    let Some(symbol) = heap.well_known(super::well_known_at("species")) else {
        return;
    };
    let getter = heap.new_native_function(realm.function_prototype(), receiver, realm.id());
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

/// `AllocateArrayBuffer` step 3.a — a buffer cannot start out longer than it may ever be.
///
/// A RangeError and not a clamp, for the reason `ToIndex` gives: a buffer that quietly became a
/// different size than asked for surfaces somewhere else entirely. Its own function because *where*
/// it runs is observable — see the call in `construct`.
fn refuse_longer_than_max(length: usize, max: Option<usize>) -> Completion<()> {
    if max.is_some_and(|max| length > max) {
        return Err(Abrupt::range_error(
            "this ArrayBuffer is longer than its maxByteLength allows",
        ));
    }
    Ok(())
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
    // §25.1.3.1 step 3 `GetArrayBufferMaxByteLengthOption`, read *after* the length, which a
    // `valueOf` on either can observe.
    let max = max_byte_length_option(vm, heap, call.argument(1))?;
    // `AllocateArrayBuffer` step 3.a, and its position is the whole of what it decides: the
    // RangeError comes **before** step 4's `OrdinaryCreateFromConstructor`, so a `new.target` whose
    // `prototype` getter throws never runs it. Unobservable while §10.1.13 read an own data
    // property, and a regression the moment it became a real `Get`.
    refuse_longer_than_max(length, max)?;
    let prototype = super::prototype_from(vm, heap, call, Realm::array_buffer_prototype)?;
    allocate(heap, prototype, length, max)
}

/// §25.1.3.7 `GetArrayBufferMaxByteLengthOption` — the `maxByteLength` an options bag asks for.
///
/// `None` for anything that is not an object and for an object without the property, which is the
/// difference between a fixed buffer and a resizable one of the same length. Deliberately *not* a
/// TypeError for a non-object: `new ArrayBuffer(8, null)` and `new ArrayBuffer(8)` are the same
/// buffer, and only a `maxByteLength` that is present says otherwise.
pub(super) fn max_byte_length_option(
    vm: &mut Vm,
    heap: &mut Heap,
    options: Value,
) -> Completion<Option<usize>> {
    let Value::Object(_) = options else {
        return Ok(None);
    };
    let name = key(heap, "maxByteLength");
    let found = vm.get_property_key(options, name, heap)?;
    // Step 3 — `undefined` is "no opinion" and not a length of zero. A bag written
    // `{ maxByteLength: undefined }` therefore makes the same fixed buffer as no bag at all, which
    // is what lets a caller pass an option through without deciding whether it has one.
    if matches!(found, Value::Undefined) {
        return Ok(None);
    }
    Ok(Some(to_index(vm, heap, found)?))
}

/// §25.1.3.1 `AllocateArrayBuffer` — the object and its zeroed bytes.
///
/// The heap's own budget is what refuses an absurd length, rather than a limit written here:
/// DR-0013 says an engine that cannot allocate says so as a RangeError, and a buffer is the
/// easiest thing in the language to ask for too much of.
fn allocate(
    heap: &mut Heap,
    prototype: ObjectId,
    length: usize,
    max: Option<usize>,
) -> Completion<Value> {
    // DR-0013 — the budget is what refuses an absurd length rather than a limit written here, and
    // a buffer is the easiest thing in the language to ask too much of. Checked *before* the
    // allocation, because the point is not to make it.
    super::array_methods::heap_within_budget(heap)?;
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
        let mut made = Buffer::new(length);
        if let Some(max) = max {
            made.allow_resizing_to(max);
        }
        found.set_buffer(made);
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
        // §25.1.5's methods each require an **unshared** buffer, and §25.2.4's require a shared
        // one — so neither family answers about the other, however alike the bytes underneath are.
        // Checking only "is there a buffer" would let `ArrayBuffer.prototype.slice` copy a
        // `SharedArrayBuffer` and hand back something of the wrong kind.
        Some(found) if !found.shared() => Ok(object),
        _ => Err(Abrupt::type_error("this is not an ArrayBuffer")),
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

/// §25.1.6.3 — `get resizable`, which is whether §25.1.3.1 was given a `maxByteLength`.
fn resizable(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let can = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_some_and(|buffer| buffer.max_byte_length().is_some());
    Ok(Value::Boolean(can))
}

/// §25.1.6.2 — `get maxByteLength`, which a fixed buffer answers too.
///
/// §25.1.6.2 step 5 gives a non-resizable buffer its *current* length rather than `undefined`: a
/// buffer that cannot be resized is already as long as it will ever be, so the two questions
/// "how long is it" and "how long may it get" have one answer. A detached buffer answers 0, on the
/// same grounds §25.1.5.1 gives for `byteLength`.
fn max_byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .filter(|buffer| !buffer.detached())
        .map_or(0, |buffer| {
            buffer
                .max_byte_length()
                .unwrap_or_else(|| buffer.byte_length())
        });
    Ok(Value::Number(length as f64))
}

/// §25.1.6.4 — `resize`, which changes a buffer's length without moving its bytes.
///
/// The order of the four refusals is the whole of what a test can see here, and it is not the
/// order the clause lists them in. §25.1.6.4's own steps put the detached check before the
/// argument, but `coerced-new-length-detach.js` is explicit that there is "one detach check
/// **after** argument coercion" — so a `valueOf` that detaches the buffer still runs, and a
/// `valueOf` on an already-detached buffer still runs too. Both then throw a TypeError, and the
/// difference from the naive order is invisible except through that side effect.
fn resize(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    // Step 2 — the slot, which is what "resizable" *is*. A fixed buffer has no `resize` to refuse
    // a length for; it has no maximum at all, so the question never gets as far as the length.
    let resizable = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_some_and(|buffer| buffer.max_byte_length().is_some());
    if !resizable {
        return Err(Abrupt::type_error("this ArrayBuffer is not resizable"));
    }
    // Step 3 — a shared buffer grows with §25.2.5.4 and is refused here, so that neither name
    // works on the other's kind of buffer and `typeof ab.resize` cannot be used to tell them apart.
    if heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_some_and(crate::heap::Buffer::shared)
    {
        return Err(Abrupt::type_error(
            "a SharedArrayBuffer grows rather than resizes",
        ));
    }
    let length = to_index(vm, heap, call.argument(0))?;
    let Some(buffer) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::buffer_mut)
    else {
        return Err(Abrupt::type_error("this is not an ArrayBuffer"));
    };
    if buffer.detached() {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let before = buffer.byte_length();
    if !buffer.resize(length) {
        return Err(Abrupt::range_error(
            "this length is past the ArrayBuffer's maxByteLength",
        ));
    }
    // DR-0013 counts what buffers hold, and a resize is the one place that number can go *down*.
    // Charged as the difference so that growing and shrinking in a loop does not read as a heap
    // that only ever grew — which is what a runaway looks like, and would refuse the program.
    heap.charge_buffer_delta(before, length);
    Ok(Value::Undefined)
}

/// §25.1.5.5 — `transfer`, which moves the bytes to a new buffer and detaches this one.
///
/// The bytes are *moved*, not copied: the point is to hand ownership somewhere else without paying
/// for a copy, and the old buffer is left detached so that nothing can read what it no longer has.
/// Every view onto it starts throwing from here.
///
/// §25.1.5.5 passes `preserve-resizability` to `ArrayBufferCopyAndDetach`, so a resizable buffer
/// transfers into another resizable one with the same maximum — the length may be chosen afresh
/// but the *ceiling* travels with the bytes. §25.1.5.6's `transferToFixedLength` is the same
/// operation with `fixed-length` instead, and [`transfer_kind`] is the one line between them.
fn transfer(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transfer_kind(vm, heap, call, true)
}

/// §25.1.5.6 — `transferToFixedLength`, which drops the resizability on the way across.
fn transfer_to_fixed_length(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    transfer_kind(vm, heap, call, false)
}

/// §25.1.5.4 `ArrayBufferCopyAndDetach`, whose last argument is the whole difference between the
/// two methods above.
fn transfer_kind(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    preserve_resizability: bool,
) -> Completion<Value> {
    let object = buffer_of(heap, call.this_value)?;
    // Read before the bytes are taken, because taking them detaches the buffer and a detached
    // buffer is asked nothing else.
    let max = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::max_byte_length)
        .filter(|_| preserve_resizability);
    let taken = heap
        .object_mut(object)
        .and_then(crate::heap::Object::buffer_mut)
        .and_then(|found| {
            let bytes = found.with_bytes(|bytes| Some(bytes?.to_vec()))?;
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
    // §25.1.5.4 step 6 — a transfer that names a length longer than the old maximum raises the
    // ceiling to it rather than refusing, because the new buffer is a new allocation and the old
    // maximum was a promise about the old one.
    let max = max.map(|max| max.max(bytes.len()));
    let made = allocate(heap, vm.realm().array_buffer_prototype(), bytes.len(), max)?;
    if let Value::Object(id) = made
        && let Some(buffer) = heap
            .object_mut(id)
            .and_then(crate::heap::Object::buffer_mut)
    {
        buffer.with_bytes_mut(|target| {
            if let Some(target) = target {
                target.copy_from_slice(&bytes);
            }
        });
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
    // Step 15, and it is one-sided: the new buffer may be **longer** than asked and never shorter.
    // Without it a species answering four bytes for a slice of eight left the copy below with
    // nowhere to put half of them, and `get_mut` declined in silence — a slice that answered a
    // buffer holding whatever the species had put there.
    let room = matches!(made, Value::Object(id)
        if heap.object(id).and_then(crate::heap::Object::buffer)
            .is_some_and(|found| found.byte_length() >= taken));
    if !room {
        return Err(Abrupt::type_error(
            "the species of this ArrayBuffer made one too small to hold the slice",
        ));
    }

    // Step 14 — detached is checked **after** the new buffer is made, because making it can run a
    // program that detaches this one. Copying then would read bytes that are not there.
    let Some(source) = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        // `from + taken` and not `to`, because §25.1.5.3 step 7's length is `max(final - first, 0)`
        // — the two ends are clamped independently, so `slice(4, 2)` is an *empty* slice and not a
        // backwards one. Written as a range from the two ends it is `bytes[4..2]`, which panics.
        .and_then(|buffer| {
            buffer.with_bytes(|bytes| {
                let bytes = bytes?;
                let start = from.min(bytes.len());
                Some(bytes[start..(start + taken).min(bytes.len())].to_vec())
            })
        })
    else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    if let Value::Object(id) = made
        && let Some(buffer) = heap
            .object_mut(id)
            .and_then(crate::heap::Object::buffer_mut)
    {
        buffer.with_bytes_mut(|bytes| {
            if let Some(target) = bytes.and_then(|bytes| bytes.get_mut(..source.len())) {
                target.copy_from_slice(&source);
            }
        });
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
