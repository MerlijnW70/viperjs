//! §25.3 — `DataView`, which reads a buffer's bytes as numbers.
//!
//! # Why the endianness is an argument and not a setting
//!
//! Because a `DataView` exists for data that came from somewhere else — a file, a socket, a
//! format — and such data has an endianness the *format* chose, not the machine. So every read and
//! write takes `littleEndian` as its own argument, defaulting to **big**-endian, which is the
//! opposite of every machine ViperJS runs on and is deliberate: network byte order is big-endian,
//! and a default that matched the machine would make a program correct on one and wrong on another.
//!
//! That is the one place §25.3 and §23.2 disagree. A TypedArray uses the *platform's* order and has
//! no way to ask for the other; a `DataView` asks every time.
//!
//! # Every access re-checks the buffer
//!
//! A buffer can be detached between two statements (§25.1.3.3), so `[[ViewedArrayBuffer]]` being
//! present is not enough — `GetViewValue` asks again on every single read. It is not defensive
//! coding; it is that the answer genuinely changes.

use super::{define_method, define_value, key};
use crate::heap::{Element, Heap, Native, NativeCall, ObjectId, PropertyDescriptor, View};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `DataView` into `heap` as a property of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.data_view_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "DataView", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, global, "DataView", Value::Object(constructor));
    define_value(heap, prototype, "constructor", Value::Object(constructor));

    // §25.3.4 — a `get` and a `set` per type, named after it. Their `length`s differ: a getter
    // takes the offset and the endianness, a setter takes the value between them.
    for (name, get, set) in READERS {
        define_method(heap, realm, prototype, &format!("get{name}"), 1, *get);
        define_method(heap, realm, prototype, &format!("set{name}"), 2, *set);
    }
    // §25.3.4.1 to §25.3.4.3 — three accessors, each of which throws for a detached buffer rather
    // than answering 0. That is where they differ from `ArrayBuffer.prototype.byteLength`, which
    // answers 0: a view onto nothing is not a view of length nothing, it is an error.
    for (name, native) in [
        ("buffer", buffer as Native),
        ("byteLength", byte_length),
        ("byteOffset", byte_offset),
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
    super::collection::tag_with(heap, realm, prototype, "DataView");
}

/// The ten pairs, each with the two natives that read and write it.
static READERS: &[(&str, Native, Native)] = &[
    ("Int8", get_int8, set_int8),
    ("Uint8", get_uint8, set_uint8),
    ("Int16", get_int16, set_int16),
    ("Uint16", get_uint16, set_uint16),
    ("Int32", get_int32, set_int32),
    ("Uint32", get_uint32, set_uint32),
    ("Float32", get_float32, set_float32),
    ("Float64", get_float64, set_float64),
    ("BigInt64", get_bigint64, set_big64),
    ("BigUint64", get_biguint64, set_big64),
];

/// §25.3.2.1 — `new DataView(buffer, byteOffset, byteLength)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error("DataView must be called with new"));
    }
    // Step 2 — the first argument must be an actual buffer, checked before anything is converted.
    let Value::Object(buffer) = call.argument(0) else {
        return Err(Abrupt::type_error("a DataView needs an ArrayBuffer"));
    };
    if heap
        .object(buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none()
    {
        return Err(Abrupt::type_error("a DataView needs an ArrayBuffer"));
    }
    let offset = super::buffer::to_index(vm, heap, call.argument(1))?;
    // Steps 5 and 6 — the offset is checked against the buffer *after* the conversion, because the
    // conversion can run a `valueOf` that detaches it.
    if detached(heap, buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let length = byte_length_of(heap, buffer);
    if offset > length {
        return Err(Abrupt::range_error(
            "this offset is past the end of the buffer",
        ));
    }
    // Step 8 — an absent length means "to the end", which is not the same as 0 and is why the
    // argument cannot simply be converted with the others.
    let tracking = matches!(call.argument(2), Value::Undefined)
        && heap
            .object(buffer)
            .and_then(crate::heap::Object::buffer)
            .is_some_and(|found| found.max_byte_length().is_some());
    let width = match call.argument(2) {
        Value::Undefined => length - offset,
        given => {
            let asked = super::buffer::to_index(vm, heap, given)?;
            if offset + asked > length {
                return Err(Abrupt::range_error(
                    "this DataView is longer than its buffer",
                ));
            }
            asked
        }
    };
    // Step 12 — checked *again*, because converting the length can also have detached it. Two
    // checks around one conversion is what the specification does and it is not redundant.
    if detached(heap, buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let prototype = super::prototype_from(heap, call, vm.realm().data_view_prototype());
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_view(View {
            buffer,
            offset,
            length: width,
            // A `DataView` has no type of its own — it asks for one at every access.
            element: None,
            // §25.3.2.1 step 15 — the same `auto` a TypedArray gets, and decided the same way: no
            // explicit length over a resizable buffer. A `DataView` keeps every byte rather than a
            // whole number of elements, because it has no element to be a whole number of.
            tracking,
        });
    }
    Ok(Value::Object(object))
}

/// Whether a buffer has been detached — §25.1.3.2, asked afresh on every access.
///
/// A missing buffer counts as detached, which is not a defensive default but the honest reading:
/// the question is "are the bytes there", and an object that never had any has none now.
fn detached(heap: &Heap, buffer: ObjectId) -> bool {
    heap.object(buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached)
}

/// How many bytes a buffer has — 0 for one that is detached or was never a buffer.
fn byte_length_of(heap: &Heap, buffer: ObjectId) -> usize {
    heap.object(buffer)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length)
}

/// The view `this` is, or the TypeError §25.3.4 asks for.
fn view_of(heap: &Heap, this: Value) -> Completion<View> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not a DataView"));
    };
    // §25.3.1.1 step 2 asks for a `[[DataView]]` slot, and a TypedArray has a view without one:
    // both are a window onto a buffer, and only a `DataView` has no element type. Accepting any
    // view let `DataView.prototype.getFloat64.call(new Int8Array())` past this check and refuse a
    // few steps later with the wrong error — a RangeError about the bounds rather than a TypeError
    // about the receiver.
    // `any_view` rather than the stored view, because a `DataView` over a resizable buffer with no
    // explicit length tracks that buffer — so its `[[ByteLength]]` is a question about the buffer
    // *now* and the stored number is stale from the first `resize`.
    // §25.3.1.2 — a `DataView` whose window no longer fits its buffer is refused on the same terms
    // as a detached one, exactly as §23.2's methods refuse an out-of-bounds TypedArray.
    if heap.view_out_of_bounds(object) {
        return Err(Abrupt::type_error(
            "this DataView is outside the bounds of its buffer",
        ));
    }
    heap.any_view(object)
        .filter(|view| view.element.is_none())
        .ok_or_else(|| Abrupt::type_error("this is not a DataView"))
}

/// §25.3.4.1 — `get buffer`.
fn buffer(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let view = view_of(heap, call.this_value)?;
    Ok(Value::Object(view.buffer))
}

/// §25.3.4.2 — `get byteLength`, which **throws** for a detached buffer.
fn byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let view = view_of(heap, call.this_value)?;
    if detached(heap, view.buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    Ok(Value::Number(view.length as f64))
}

/// §25.3.4.3 — `get byteOffset`, which throws for the same reason.
fn byte_offset(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let view = view_of(heap, call.this_value)?;
    if detached(heap, view.buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    Ok(Value::Number(view.offset as f64))
}

/// §25.3.1.1 `GetViewValue` — one read, once the type is known.
fn get_value(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    element: Element,
) -> Completion<Value> {
    let view = view_of(heap, call.this_value)?;
    let at = super::buffer::to_index(vm, heap, call.argument(0))?;
    // §25.3.4's default is **big**-endian, so an absent argument is `false` — the opposite of the
    // machine ViperJS runs on, and the reason every read says which it wants.
    let little = call.argument(1).to_boolean(heap);
    // Step 5 — asked *now*, because converting the index above can have detached the buffer.
    if detached(heap, view.buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let width = element.width();
    if at + width > view.length {
        return Err(Abrupt::range_error(
            "this read is past the end of the DataView",
        ));
    }
    let from = view.offset + at;
    let Some(bytes) = heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::bytes)
        .map(|bytes| bytes[from..from + width].to_vec())
    else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    let ordered = match little {
        true => bytes,
        // Reversed into little-endian order, because that is the one the readers above are written
        // in. Doing it here rather than twice per type keeps the eight of them the same shape.
        false => bytes.into_iter().rev().collect(),
    };
    // §25.3.1.1 step 16 — the `BigInt64` pair answers with a BigInt where the other eight answer
    // with a Number. The same eight bytes are `-1n` read as signed and a very large positive read
    // as unsigned, which is the whole of the difference between the two.
    let numeric = element.read(&ordered);
    Ok(heap.numeric_value(numeric))
}

/// §25.3.1.2 `SetViewValue` — one write.
fn set_value(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    element: Element,
) -> Completion<Value> {
    let view = view_of(heap, call.this_value)?;
    let at = super::buffer::to_index(vm, heap, call.argument(0))?;
    // Step 4 — the *value* is converted before the endianness is read and before the bounds are
    // checked, so a `valueOf` that detaches the buffer is noticed by the check below rather than
    // by writing into bytes that have gone.
    //
    // …and *which* conversion depends on the slot: §25.3.1.2 step 4 is `ToBigInt` for the BigInt
    // pair and `ToNumber` for the rest, which is where a `DataView` refuses to mix the two numeric
    // types exactly as the operators do. Everything modulo 2^64 for the pair — a fixed-width slot
    // takes the low bits rather than refusing a value too large for it, which is §21.2.2.2's
    // `asUintN(64, …)`.
    let numeric = vm.to_numeric(element.holds_big(), call.argument(1), heap)?;
    let little = call.argument(2).to_boolean(heap);
    if detached(heap, view.buffer) {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let width = element.width();
    if at + width > view.length {
        return Err(Abrupt::range_error(
            "this write is past the end of the DataView",
        ));
    }
    // §25.3.1.5, and the `Option` cannot be an absence here: the conversion above was chosen by
    // this same kind, so the numeric it produced is the one this kind writes.
    let mut bytes = element
        .write_numeric(&numeric, false)
        .unwrap_or_else(|| vec![0; width]);
    if !little {
        bytes.reverse();
    }
    let from = view.offset + at;
    if let Some(target) = heap
        .object_mut(view.buffer)
        .and_then(crate::heap::Object::buffer_mut)
        .and_then(crate::heap::Buffer::bytes_mut)
    {
        target[from..from + width].copy_from_slice(&bytes);
    }
    // §25.3.1.2 step 15 — a write answers `undefined`, not the value written.
    Ok(Value::Undefined)
}

/// §25.3.4 — `getBigInt64`, where the top bit of the eight is a sign.
fn get_bigint64(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::BigInt64)
}

/// §25.3.4 — `setBigInt64` **and** `setBigUint64`, which are one function.
///
/// The two write the same eight bytes: a sign is something a *read* decides, and §25.3.1.2 has no
/// step that consults one. Two natives differing in a flag neither of them used said otherwise —
/// which is also why the kind named here is arbitrary between the pair.
fn set_big64(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::BigInt64)
}

/// §25.3.4 — `getBigUint64`, the same eight bytes with no sign among them.
fn get_biguint64(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::BigUint64)
}

/// §25.3.4 — `getInt8`.
fn get_int8(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Int8)
}

/// §25.3.4 — `setInt8`.
fn set_int8(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Int8)
}

/// §25.3.4 — `getUint8`.
fn get_uint8(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Uint8)
}

/// §25.3.4 — `setUint8`.
fn set_uint8(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Uint8)
}

/// §25.3.4 — `getInt16`.
fn get_int16(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Int16)
}

/// §25.3.4 — `setInt16`.
fn set_int16(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Int16)
}

/// §25.3.4 — `getUint16`.
fn get_uint16(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Uint16)
}

/// §25.3.4 — `setUint16`.
fn set_uint16(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Uint16)
}

/// §25.3.4 — `getInt32`.
fn get_int32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Int32)
}

/// §25.3.4 — `setInt32`.
fn set_int32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Int32)
}

/// §25.3.4 — `getUint32`.
fn get_uint32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Uint32)
}

/// §25.3.4 — `setUint32`.
fn set_uint32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Uint32)
}

/// §25.3.4 — `getFloat32`.
fn get_float32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Float32)
}

/// §25.3.4 — `setFloat32`.
fn set_float32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Float32)
}

/// §25.3.4 — `getFloat64`.
fn get_float64(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    get_value(vm, heap, call, Element::Float64)
}

/// §25.3.4 — `setFloat64`.
fn set_float64(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_value(vm, heap, call, Element::Float64)
}
