//! §25.2's `SharedArrayBuffer` and §25.4's `Atomics`.
//!
//! # What "shared" means to an engine with one agent
//!
//! Two things, and neither is about threads. A `SharedArrayBuffer` **cannot be detached** — §25.2
//! gives it no `[[ArrayBufferDetachKey]]` and no `transfer`, so its bytes are there for as long as
//! anything can name it. And it is a different **brand**: `ArrayBuffer.prototype.byteLength`
//! requires an unshared buffer and `SharedArrayBuffer.prototype.byteLength` a shared one, so
//! neither answers about the other however alike the bytes are.
//!
//! praxis runs one agent, so the memory model of §25.4.1 has nothing to order: every operation is
//! already the only one happening. That does **not** make `Atomics` decorative — the operations
//! have arithmetic and coercion of their own, they refuse the wrong element kinds, and they read
//! and write in ways `ta[i]` does not.
//!
//! # Why `Atomics` accepts an ordinary `ArrayBuffer`
//!
//! Since ES2020 every operation here works on an unshared buffer too — only `wait` requires a
//! shared one, and `wait` needs agents praxis has not got. §25.4.3's `ValidateIntegerTypedArray`
//! asks about the *element kind* rather than about sharing, which is the check these actually make.

use super::{define_method, define_value, key};
use crate::heap::{Element, Heap, Native, NativeCall, ObjectId, PropertyDescriptor, View};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The buffer `this` is, when it is shared — §25.2.4's brand check.
fn shared_buffer(heap: &Heap, this: Value) -> Completion<ObjectId> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
    };
    match heap.object(object).and_then(crate::heap::Object::buffer) {
        Some(found) if found.shared() => Ok(object),
        _ => Err(Abrupt::type_error("this is not a SharedArrayBuffer")),
    }
}

/// §25.2.2.1 `SharedArrayBuffer(length)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error(
            "SharedArrayBuffer must be called with new",
        ));
    }
    let asked = super::buffer::to_index(vm, heap, call.argument(0))?;
    super::array_methods::within_budget(heap)?;
    // DR-0013 — asked before the bytes are taken rather than noticed afterwards, because the
    // length is a number the program chose. `checked_sub` rather than a comparison for the reason
    // §25.1.3.1's version gives: the boundary of a comparison here is a number no test can reach,
    // since the allowance depends on what the heap already holds. Subtracting says the same thing
    // with nothing to be off by one about.
    if heap.allowance().checked_sub(asked).is_none() {
        return Err(Abrupt::range_error(
            "this SharedArrayBuffer is larger than this engine will allocate",
        ));
    }
    let prototype = super::prototype_from(heap, call, vm.realm().shared_buffer_prototype());
    let object = heap.new_object(Some(prototype));
    heap.charge_buffer(asked);
    if let Some(found) = heap.object_mut(object) {
        found.set_buffer(crate::heap::Buffer::new_shared(asked));
    }
    Ok(Value::Object(object))
}

/// §25.2.4.1 `get SharedArrayBuffer.prototype.byteLength`.
fn byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    Ok(Value::Number(length as f64))
}

/// §25.2.4.3 `SharedArrayBuffer.prototype.slice`.
///
/// The same arithmetic as §25.1.5.4's, and a different brand at each end: the receiver must be
/// shared, and so must what the species made.
fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    let start = super::array_methods::start_index(vm, heap, call.argument(0), length as u64)?;
    let end = match call.argument(1) {
        Value::Undefined => length as u64,
        given => super::array_methods::start_index(vm, heap, given, length as u64)?,
    };
    let taken = (end.saturating_sub(start)) as usize;
    let bytes = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::bytes)
        .map(|found| found[start as usize..start as usize + taken].to_vec())
        .unwrap_or_default();
    let prototype = vm.realm().shared_buffer_prototype();
    let made = heap.new_object(Some(prototype));
    heap.charge_buffer(taken);
    if let Some(found) = heap.object_mut(made) {
        let mut buffer = crate::heap::Buffer::new_shared(taken);
        if let Some(target) = buffer.bytes_mut() {
            target.copy_from_slice(&bytes);
        }
        found.set_buffer(buffer);
    }
    Ok(Value::Object(made))
}

/// Which arithmetic §25.4.3's read-modify-write operations do.
#[derive(Clone, Copy)]
enum Operation {
    /// §25.4.3.1 — `add`.
    Add,
    /// §25.4.3.2 — `and`.
    And,
    /// §25.4.3.11 — `or`.
    Or,
    /// §25.4.3.14 — `sub`.
    Sub,
    /// §25.4.3.15 — `xor`.
    Xor,
    /// §25.4.3.5 — `exchange`, which keeps the new value and answers the old.
    Exchange,
}

impl Operation {
    /// The new value, given what is there and what was asked for.
    ///
    /// The bitwise three are done on the *integer* form, because that is what they mean: `&` on a
    /// pair of doubles is not an operation, and every element kind these accept is an integer one.
    fn apply(self, held: f64, given: f64) -> f64 {
        let (left, right) = (held as i64, given as i64);
        match self {
            Self::Add => held + given,
            Self::And => (left & right) as f64,
            Self::Or => (left | right) as f64,
            Self::Sub => held - given,
            Self::Xor => (left ^ right) as f64,
            Self::Exchange => given,
        }
    }
}

/// §25.4.3.4 `ValidateIntegerTypedArray`, and the index — the opening every operation here shares.
///
/// Answers the view and the position, because getting one without the other is never useful. The
/// index is validated *after* the element kind, which is what §25.4.3.3 step 1 asks for: a
/// `Float64Array` is refused before its index is even looked at.
fn target(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<(View, usize)> {
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    let Some(view) = heap.typed_view(object) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    // §25.4.3.4 — the float kinds are refused. Atomics are about bit patterns a CPU can exchange,
    // and a double is not one of those however well it holds an integer.
    if matches!(
        view.element,
        Some(Element::Float32 | Element::Float64) | None
    ) {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    }
    if heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached)
    {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let asked = super::buffer::to_index(vm, heap, call.argument(1))?;
    // §25.4.3.3 step 3 — out of range is a **RangeError**, where an ordinary `ta[9]` is silently
    // `undefined`. These say so, because an atomic write that went nowhere is worse than an error.
    if asked >= view.count() {
        return Err(Abrupt::range_error("that index is outside the TypedArray"));
    }
    Ok((view, asked))
}

/// The five arithmetic operations and `exchange`, which differ only in [`Operation::apply`].
fn modify(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    what: Operation,
) -> Completion<Value> {
    let (view, at) = target(vm, heap, call)?;
    // The value is converted *after* the index is validated, and its conversion may run user code
    // that detaches the buffer — so the write is checked again below rather than assumed.
    let given = vm.to_number(call.argument(2), heap)?;
    let Some(held) = heap.element_at(view, at) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    heap.write_element(object, at, what.apply(held, given));
    // Every one of them answers the value that *was* there, which is what makes them atomic
    // read-modify-writes rather than writes.
    Ok(Value::Number(held))
}

/// §25.4.3.9 `Atomics.load`.
fn load(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, at) = target(vm, heap, call)?;
    Ok(heap
        .element_at(view, at)
        .map_or(Value::Undefined, Value::Number))
}

/// §25.4.3.13 `Atomics.store`.
///
/// The one that answers what it was *given* rather than what it wrote or what was there. The two
/// differ: storing 300 into a `Uint8Array` writes 44 and answers 300.
fn store(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, at) = target(vm, heap, call)?;
    let given = vm.to_number(call.argument(2), heap)?;
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    heap.write_element(object, at, given);
    Ok(Value::Number(super::string::to_integer_or_infinity(given)))
}

/// §25.4.3.3 `Atomics.compareExchange`.
fn compare_exchange(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, at) = target(vm, heap, call)?;
    let expected = vm.to_number(call.argument(1 + 1), heap)?;
    let replacement = vm.to_number(call.argument(3), heap)?;
    let Some(held) = heap.element_at(view, at) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    // The comparison is against the value **as the element kind stores it**, not against what was
    // handed over: expecting 300 of a `Uint8Array` holding 44 is a match, because 300 stored there
    // *is* 44. Comparing the raw arguments would never match and the write would never happen.
    if let Some(element) = view.element
        && element.read(&element.write(expected)) == held
    {
        heap.write_element(object, at, replacement);
    }
    Ok(Value::Number(held))
}

/// §25.4.3.8 `Atomics.isLockFree`.
///
/// Answers about a *width* rather than about a buffer. praxis has one agent, so every width it
/// supports is lock-free in the only sense the question has — but the answer must still be the
/// same for a given width every time it is asked, which §25.4.3.8's note requires.
fn is_lock_free(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let width = vm.to_number(call.argument(0), heap)?;
    Ok(Value::Boolean(matches!(width as i64, 1 | 2 | 4 | 8)))
}

/// Build §25.2's `SharedArrayBuffer` and §25.4's `Atomics` onto the global.
pub(super) fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.shared_buffer_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "SharedArrayBuffer", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(
        heap,
        global,
        "SharedArrayBuffer",
        Value::Object(constructor),
    );
    define_value(heap, prototype, "constructor", Value::Object(constructor));
    define_method(heap, realm, prototype, "slice", 2, slice);
    let getter = heap.new_native_function(realm.function_prototype(), byte_length);
    super::define_function_metadata(heap, getter, "get byteLength", 0);
    let name = key(heap, "byteLength");
    let _ = heap.define_own_property(
        prototype,
        name,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
    super::buffer::define_species(heap, realm, constructor);
    super::collection::tag_with(heap, realm, prototype, "SharedArrayBuffer");

    // §25.4 — an ordinary object rather than a constructor, like `Math` and `JSON`.
    let atomics = heap.new_object(Some(realm.object_prototype()));
    define_value(heap, global, "Atomics", Value::Object(atomics));
    for (name, length, native) in [
        ("add", 3, add as Native),
        ("and", 3, and),
        ("compareExchange", 4, compare_exchange),
        ("exchange", 3, exchange),
        ("isLockFree", 1, is_lock_free),
        ("load", 2, load),
        ("or", 3, or),
        ("store", 3, store),
        ("sub", 3, sub),
        ("xor", 3, xor),
    ] {
        define_method(heap, realm, atomics, name, length, native);
    }
    super::collection::tag_with(heap, realm, atomics, "Atomics");
}

/// §25.4.3.1 `Atomics.add`.
fn add(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Add)
}

/// §25.4.3.2 `Atomics.and`.
fn and(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::And)
}

/// §25.4.3.11 `Atomics.or`.
fn or(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Or)
}

/// §25.4.3.14 `Atomics.sub`.
fn sub(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Sub)
}

/// §25.4.3.15 `Atomics.xor`.
fn xor(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Xor)
}

/// §25.4.3.5 `Atomics.exchange`.
fn exchange(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Exchange)
}
