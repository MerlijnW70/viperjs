//! §28.1 — `Reflect`, which is every internal method with a name.
//!
//! # Why it exists when `Object` already has most of it
//!
//! Two differences, and both are about being *usable in a program that has to cope*.
//!
//! `Object.defineProperty` throws when it cannot do what it was asked and answers the object when
//! it can, so a program that wants to know has to wrap it in a `try`. `Reflect.defineProperty`
//! answers the **Boolean** the internal method actually returned. Five of these do that, and it is
//! the reason a `Proxy` handler can be written by hand: the handlers are specified to return
//! Booleans, so the operations they wrap have to as well.
//!
//! And `Object.keys` takes anything, converting a primitive on the way (§7.1.18). Every function
//! here **requires an object** and says so, because these are the internal methods and an internal
//! method has no meaning for a String.
//!
//! # `get` and `set` take a receiver
//!
//! The one thing here that no other clause offers. §10.1.8.1 hands a getter the object the read
//! went *through* rather than the one the property was found on; `Reflect.get` lets a program
//! choose that third thing directly, which is what makes a `Proxy`'s `get` trap able to forward to
//! its target without lying to the getter about who is asking.

use super::{define_method, define_value};
use crate::heap::{Heap, Native, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `Reflect` into `heap` as a property of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    // §28.1 — an ordinary object, not a constructor and not callable. `new Reflect` is a TypeError
    // for the dull reason that it has no `[[Construct]]`, exactly as `Math` is.
    let reflect = heap.new_object(Some(realm.object_prototype()));
    define_value(heap, global, "Reflect", Value::Object(reflect));

    for (name, length, native) in [
        ("apply", 3, apply as Native),
        ("construct", 2, construct),
        ("defineProperty", 3, define_property),
        ("deleteProperty", 2, delete_property),
        ("get", 2, get),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getPrototypeOf", 1, get_prototype_of),
        ("has", 2, has),
        ("isExtensible", 1, is_extensible),
        ("ownKeys", 1, own_keys),
        ("preventExtensions", 1, prevent_extensions),
        ("set", 3, set),
        ("setPrototypeOf", 2, set_prototype_of),
    ] {
        define_method(heap, realm, reflect, name, length, native);
    }
    super::collection::tag_with(heap, reflect, "Reflect");
}

/// The object `target` is, or the TypeError every one of these begins with.
///
/// §28.1 opens all thirteen the same way and it is the difference from `Object`: these are the
/// internal methods, and an internal method is something an *object* has. `Object.keys("ab")`
/// converts and answers; `Reflect.ownKeys("ab")` refuses, because a String has no `[[OwnPropertyKeys]]`.
fn target_of(heap: &Heap, value: Value, what: &'static str) -> Completion<ObjectId> {
    match value {
        Value::Object(object) if heap.object(object).is_some() => Ok(object),
        _ => Err(Abrupt::type_error(what)),
    }
}

/// §28.1.1 — `Reflect.apply(target, thisArgument, argumentsList)`.
fn apply(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = call.argument(0);
    if !heap.is_callable(target) {
        return Err(Abrupt::type_error("Reflect.apply needs a function"));
    }
    // §28.1.1 step 3 — `CreateListFromArrayLike`, which requires an object: `Reflect.apply(f, x)`
    // with no list is a TypeError where `f.apply(x)` is a call with no arguments.
    let arguments = super::function::list_from(
        vm,
        heap,
        call.argument(2),
        "the arguments given to Reflect.apply must be an object",
    )?;
    vm.call_value(target, call.argument(1), &arguments, heap)
}

/// §28.1.2 — `Reflect.construct(target, argumentsList, newTarget)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = call.argument(0);
    if !heap.is_constructor(target) {
        return Err(Abrupt::type_error("Reflect.construct needs a constructor"));
    }
    // Step 2 — an absent `newTarget` is the target itself, which is what `new` does. A *present*
    // one must also be a constructor, and it is what decides the prototype of the object made:
    // this is the only way in the language to build an X whose prototype came from a Y.
    let new_target = match call.arguments.len() {
        0..=2 => target,
        _ => {
            let given = call.argument(2);
            if !heap.is_constructor(given) {
                return Err(Abrupt::type_error(
                    "Reflect.construct needs a constructor as its new.target",
                ));
            }
            given
        }
    };
    let arguments = super::function::list_from(
        vm,
        heap,
        call.argument(1),
        "the arguments given to Reflect.construct must be an object",
    )?;
    vm.construct_with_target(target, new_target, &arguments, heap)
}

/// §28.1.3 — `Reflect.defineProperty`, which **answers** where `Object.defineProperty` throws.
fn define_property(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.defineProperty needs an object",
    )?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    let descriptor = super::object::to_property_descriptor(vm, heap, call.argument(2))?;
    // §28.1.3 answers the refusal rather than throwing it, which is the whole difference from
    // `Object.defineProperty` — but a define that *throws* still throws: §10.4.2.1's RangeError
    // from an array's `length` and §10.4.5.16's TypeError from a TypedArray's content type are
    // both raised by the define itself rather than reported as a refusal.
    let outcome = vm.define_through(object, name, &descriptor, heap)?;
    super::object::define_answer(outcome)
}

/// §28.1.4 — `Reflect.deleteProperty`, which is `delete` without the operator's sloppiness.
fn delete_property(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.deleteProperty needs an object",
    )?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    vm.delete_property_key(Value::Object(object), name, heap)
}

/// §28.1.5 — `Reflect.get(target, key, receiver)`.
fn get(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(heap, call.argument(0), "Reflect.get needs an object")?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    // Step 3 — an absent receiver is the target, which is what an ordinary read does. Present, it
    // is what a getter sees as `this`, and it may be anything at all.
    let receiver = match call.arguments.len() {
        0..=2 => Value::Object(object),
        _ => call.argument(2),
    };
    vm.get_through(Value::Object(object), name, receiver, heap)
}

/// §28.1.9 — `Reflect.set(target, key, value, receiver)`.
fn set(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(heap, call.argument(0), "Reflect.set needs an object")?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    let receiver = match call.arguments.len() {
        0..=3 => Value::Object(object),
        _ => call.argument(3),
    };
    // §10.1.9's Boolean, which `Reflect.set` answers where an assignment discards it in sloppy
    // code and turns it into a throw in strict code.
    vm.set_through(
        Value::Object(object),
        name,
        call.argument(2),
        receiver,
        heap,
    )
}

/// §28.1.6 — `Reflect.getOwnPropertyDescriptor`.
fn get_own_property_descriptor(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.getOwnPropertyDescriptor needs an object",
    )?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    // §6.2.6.4 — a property that is not there is `undefined` and not an empty descriptor, which is
    // how a caller tells "absent" from "present and holding undefined".
    let Some(property) = vm.own_property_through(object, name, heap)? else {
        return Ok(Value::Undefined);
    };
    Ok(super::object::describe(heap, &vm.realm(), property))
}

/// §28.1.7 — `Reflect.getPrototypeOf`, which does **not** convert a primitive first.
fn get_prototype_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.getPrototypeOf needs an object",
    )?;
    let prototype = vm.prototype_through(object, heap)?;
    Ok(prototype.map_or(Value::Null, Value::Object))
}

/// §28.1.13 — `Reflect.setPrototypeOf`, which answers whether it was allowed.
fn set_prototype_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.setPrototypeOf needs an object",
    )?;
    // §28.1.13 step 2 — `null` is a prototype and anything else that is not an object is a
    // TypeError. `undefined` is *not* accepted, which is where this differs from a great many
    // other places that treat the two alike.
    let prototype = match call.argument(1) {
        Value::Null => None,
        Value::Object(id) => Some(id),
        _ => {
            return Err(Abrupt::type_error("a prototype must be an object or null"));
        }
    };
    Ok(Value::Boolean(
        vm.set_prototype_through(object, prototype, heap)?,
    ))
}

/// §28.1.8 — `Reflect.has`, which is `in` without requiring the operator's operand order.
fn has(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(heap, call.argument(0), "Reflect.has needs an object")?;
    let name = vm.to_property_key(call.argument(1), heap)?;
    let found = vm.has_property_key(Value::Object(object), name, heap)?;
    Ok(Value::Boolean(found))
}

/// §28.1.10 — `Reflect.isExtensible`.
fn is_extensible(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.isExtensible needs an object",
    )?;
    Ok(Value::Boolean(vm.extensible_through(object, heap)?))
}

/// §28.1.12 — `Reflect.preventExtensions`, which answers `true` rather than the object.
fn prevent_extensions(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(
        heap,
        call.argument(0),
        "Reflect.preventExtensions needs an object",
    )?;
    Ok(Value::Boolean(vm.prevent_through(object, heap)?))
}

/// §28.1.11 — `Reflect.ownKeys`, which answers **Symbols too**.
///
/// The one listing in the language that hides nothing: `Object.keys` gives enumerable String keys,
/// `Object.getOwnPropertyNames` gives every String key, and this gives every key there is. It is
/// what a `Proxy`'s `ownKeys` trap has to answer, so it has to be able to say everything.
fn own_keys(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = target_of(heap, call.argument(0), "Reflect.ownKeys needs an object")?;
    let keys = vm.own_keys_through(object, heap)?;
    let values: Vec<Value> = keys
        .into_iter()
        .map(|found| heap.key_value(found))
        .collect();
    super::array::from_values(vm, heap, &values)
}
