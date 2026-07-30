//! §25.3 — `DataView`, which reads a buffer's bytes as numbers.
//!
//! # Why the endianness is an argument and not a setting
//!
//! Because a `DataView` exists for data that came from somewhere else — a file, a socket, a
//! format — and such data has an endianness the *format* chose, not the machine. So every read and
//! write takes `littleEndian` as its own argument, defaulting to **big**-endian, which is the
//! opposite of every machine praxis runs on and is deliberate: network byte order is big-endian,
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
use crate::heap::{Heap, Native, NativeCall, ObjectId, PropertyDescriptor, View};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The nine types §25.3 can read, with the width and the reading each implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Element {
    /// A signed byte.
    Int8,
    /// An unsigned byte.
    Uint8,
    /// A signed 16-bit integer.
    Int16,
    /// An unsigned 16-bit integer.
    Uint16,
    /// A signed 32-bit integer.
    Int32,
    /// An unsigned 32-bit integer.
    Uint32,
    /// An IEEE single.
    Float32,
    /// An IEEE double.
    Float64,
}

impl Element {
    /// How many bytes one of these takes — §25.3.1.1's element size.
    pub(super) fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    /// The name §25.3.4 gives the pair of methods that read and write it.
    fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8",
            Self::Uint8 => "Uint8",
            Self::Int16 => "Int16",
            Self::Uint16 => "Uint16",
            Self::Int32 => "Int32",
            Self::Uint32 => "Uint32",
            Self::Float32 => "Float32",
            Self::Float64 => "Float64",
        }
    }

    /// §25.3.1.3 `RawBytesToNumeric` — the bytes, already in reading order, as a Number.
    pub(super) fn read(self, bytes: &[u8]) -> f64 {
        let mut eight = [0_u8; 8];
        eight[..bytes.len()].copy_from_slice(bytes);
        match self {
            Self::Int8 => f64::from(eight[0] as i8),
            Self::Uint8 => f64::from(eight[0]),
            Self::Int16 => f64::from(i16::from_le_bytes([eight[0], eight[1]])),
            Self::Uint16 => f64::from(u16::from_le_bytes([eight[0], eight[1]])),
            Self::Int32 => f64::from(i32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            Self::Uint32 => f64::from(u32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            // §6.1.6.1's Number is a double, so a float32 widens on the way out. Every float32 is
            // exactly representable as a double, so nothing is lost — but the *value* is the
            // rounded one, which is why `v.setFloat32(0, 0.1); v.getFloat32(0)` is not `0.1`.
            Self::Float32 => {
                f64::from(f32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]]))
            }
            Self::Float64 => f64::from_le_bytes(eight),
        }
    }

    /// §25.3.1.5 `NumericToRawBytes` — a Number as bytes, in little-endian order.
    ///
    /// The integer conversions are `ToIntN`/`ToUintN` (§7.1.7 and following), which **wrap** rather
    /// than clamp or throw: `setUint8(0, 256)` writes 0, and `setInt8(0, 200)` writes -56. That is
    /// modular arithmetic and not saturation, and it is what makes a TypedArray of bytes behave
    /// like memory rather than like a checked container.
    pub(super) fn write(self, value: f64) -> Vec<u8> {
        match self {
            Self::Int8 | Self::Uint8 => vec![wrap(value, 8) as u8],
            Self::Int16 | Self::Uint16 => (wrap(value, 16) as u16).to_le_bytes().to_vec(),
            Self::Int32 | Self::Uint32 => (wrap(value, 32) as u32).to_le_bytes().to_vec(),
            Self::Float32 => (value as f32).to_le_bytes().to_vec(),
            Self::Float64 => value.to_le_bytes().to_vec(),
        }
    }
}

/// §7.1.7 `ToIntN`/`ToUintN` — a Number as `bits` bits, wrapping.
///
/// `NaN`, both infinities and every fractional part go to zero or are truncated first (§7.1.5), and
/// only then does the value wrap. So `setUint8(0, NaN)` writes 0 rather than refusing, which is
/// what makes writing to a buffer total.
fn wrap(value: f64, bits: u32) -> u64 {
    let truncated = value.trunc();
    let modulus = 2_f64.powi(bits as i32);
    // `rem_euclid` rather than `%`, because `%` keeps the sign of the left operand and this has to
    // answer a *non-negative* residue: -1 as a byte is 255 and not -1.
    let wrapped = truncated.rem_euclid(modulus);
    // `NaN` and both infinities arrive here as `NaN` — `inf.rem_euclid(256)` is `NaN` — and a
    // `f64 as u64` cast **saturates**: `NaN` becomes 0, as does anything negative. So the guard
    // §7.1.5 writes out is already in the cast, and written twice it was a branch nothing could
    // tell from its absence.
    wrapped as u64
}

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
    for (element, get, set) in READERS {
        define_method(
            heap,
            realm,
            prototype,
            &format!("get{}", element.name()),
            1,
            *get,
        );
        define_method(
            heap,
            realm,
            prototype,
            &format!("set{}", element.name()),
            2,
            *set,
        );
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

/// The eight pairs, each with the two natives that read and write it.
static READERS: &[(Element, Native, Native)] = &[
    (Element::Int8, get_int8, set_int8),
    (Element::Uint8, get_uint8, set_uint8),
    (Element::Int16, get_int16, set_int16),
    (Element::Uint16, get_uint16, set_uint16),
    (Element::Int32, get_int32, set_int32),
    (Element::Uint32, get_uint32, set_uint32),
    (Element::Float32, get_float32, set_float32),
    (Element::Float64, get_float64, set_float64),
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
    heap.object(object)
        .and_then(crate::heap::Object::view)
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
    // machine praxis runs on, and the reason every read says which it wants.
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
    Ok(Value::Number(element.read(&ordered)))
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
    let number = vm.to_number(call.argument(1), heap)?;
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
    let mut bytes = element.write(number);
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
