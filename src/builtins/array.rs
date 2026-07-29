//! §23.1 — `Array`, in the part that is the object rather than the fifty methods on it.
//!
//! The exotic behaviour is [`crate::heap`]'s, because it is a property rule and belongs where
//! properties live. What is here is the constructor, the prototype, and the one static that asks a
//! question nothing else can answer.

use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, ErrorKind, Value};
use crate::vm::Vm;

use super::{define_method, define_value, key};

/// §23.1.1.1 `Array(...)`.
///
/// # The argument that is not an element
///
/// One argument that is a Number is a **length**, not a contents: `Array(3)` is three holes and
/// `Array("3")` is one string. That is §23.1.1.1 steps 2 and 3, it is the reason `Array(3)` and
/// `[3]` differ, and it is why nobody uses the constructor to make a literal.
///
/// A length that is not an integer index is a RangeError rather than a rounding — `Array(1.5)`
/// throws where `a.length = 1.5` throws for the same reason.
pub fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let prototype = vm.realm().array_prototype();
    if call.arguments.len() == 1
        && let Value::Number(length) = call.argument(0)
    {
        let rounded = length as u32;
        if f64::from(rounded) != length {
            return Err(Abrupt::Raised(
                ErrorKind::Range,
                "an array length must be an integer index",
            ));
        }
        return Ok(Value::Object(heap.new_array(prototype, rounded)));
    }
    // Step 4 — every argument is an element, and that includes the zero-argument case.
    let array = heap.new_array(prototype, 0);
    for (at, value) in call.arguments.iter().enumerate() {
        let key = key(heap, &at.to_string());
        let descriptor = PropertyDescriptor {
            value: Some(*value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(array, key, &descriptor);
    }
    Ok(Value::Object(array))
}

/// A dense Array holding exactly these values — `CreateArrayFromList` (§7.3.18).
///
/// The array a built-in hands back when it has computed a list rather than been given one. Its
/// elements are ordinary in every way, which is what §7.3.18 means by an array "whose elements are
/// the elements of list".
pub(super) fn from_values(vm: &Vm, heap: &mut Heap, values: &[Value]) -> Completion<Value> {
    let array = heap.new_array(vm.realm().array_prototype(), 0);
    for (at, value) in values.iter().enumerate() {
        let key = key(heap, &at.to_string());
        let descriptor = PropertyDescriptor {
            value: Some(*value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(array, key, &descriptor);
    }
    Ok(Value::Object(array))
}

/// §23.1.2.2 `Array.isArray`.
///
/// The only way to ask. `instanceof Array` answers a different question — it walks a prototype
/// chain, so it is false for an array from another realm and true for anything given
/// `Array.prototype`— and `typeof` says `"object"` for both. This asks what the object *is*.
pub fn is_array(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(false));
    };
    let answer = heap
        .object(object)
        .is_some_and(crate::heap::Object::is_array);
    Ok(Value::Boolean(answer))
}

/// Build `Array` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.array_prototype();
    let function = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, function, "Array", 1);

    // §23.1.4 — `Array.prototype` is not writable, not enumerable and not configurable, for the
    // same reason `Object.prototype` is not.
    let key = key(heap, "prototype");
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(prototype)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(function, key, &descriptor);
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, "Array", Value::Object(function));
    define_method(heap, realm, function, "isArray", 1, is_array);
}
