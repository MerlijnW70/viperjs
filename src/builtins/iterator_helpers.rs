//! §27.1.3's `Iterator` and the methods of §27.1.4 that *consume* an iterator.
//!
//! # What `Iterator` is for, given it cannot be called
//!
//! §27.1.3.1 refuses both a plain call and `new Iterator()` — the constructor exists so that
//! `class MyIterator extends Iterator` works, and so that `%IteratorPrototype%` has a name a
//! program can reach. Everything useful hangs off the prototype.
//!
//! # `GetIteratorDirect`, and why these work on things that are not iterable
//!
//! §27.1.4's methods take the receiver *as* the iterator and read only its `next` — they never ask
//! for `[@@iterator]`. So `Iterator.prototype.toArray.call({next() { … }})` is meaningful, and an
//! object that is iterable but has no `next` of its own is not what these are for.
//!
//! # Why a callback that is not callable closes the iterator
//!
//! The check is written as "throw, then `IteratorClose`" rather than plain "throw", so
//! `iter.some(1)` calls `iter.return()` on the way out. That is observable and it is deliberate:
//! the method took possession of the iterator when it was called, so it has to give it back even
//! when the fault was the caller's.
//!
//! The methods that *make* an iterator rather than consuming one — `map`, `filter`, `take`, `drop`
//! and `flatMap` — need a helper object with state of its own, and it is
//! [`crate::heap::Helper`] in [`super::iterator_lazy`]. This said they "are not here yet"; all
//! eleven of §27.1.4's methods answer today, which a `typeof` over the prototype settles in a line.

use super::iterator::Walk;
use super::{define_method, key};
use crate::heap::{Heap, Native, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §27.1.4's opening: the receiver as an iterator, and the callback, in that order.
///
/// The two checks are not independent. §27.1.4.2 step 2 refuses a receiver that is not an object
/// **before** anything else; step 4 then refuses a callback that is not callable *and closes the
/// iterator on the way out*, because by then the method is holding one.
fn consuming(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    wants: &'static str,
) -> Completion<(Walk, Value)> {
    let Value::Object(_) = call.this_value else {
        return Err(Abrupt::type_error("this is not an iterator"));
    };
    let callback = call.argument(0);
    // The callback is judged *before* `GetIteratorDirect`, so a bad one closes the iterator with
    // `next` still unread — §27.1.4 makes the Iterator Record with an undefined `[[NextMethod]]`
    // for exactly this window.
    if !heap.is_callable(callback) {
        Walk::close_unread(vm, heap, call.this_value);
        return Err(Abrupt::type_error(wants));
    }
    let walk = Walk::direct(vm, heap, call.this_value)?;
    Ok((walk, callback))
}

/// What a walk does with each value, and what it answers when the values run out.
#[derive(Clone, Copy)]
enum Consume {
    /// §27.1.4.7 — every value, and the answer is `undefined`.
    ForEach,
    /// §27.1.4.11 — stop at the first the callback likes, and answer it.
    Find,
    /// §27.1.4.10 — stop at the first the callback likes, and answer `true`.
    Some,
    /// §27.1.4.6 — stop at the first it does *not*, and answer `false`.
    Every,
}

/// §27.1.4.6, §27.1.4.7, §27.1.4.10 and §27.1.4.11 — the four that walk with a predicate.
///
/// One walk with four answers, because they differ only in what stops them and what they say. Each
/// hands the callback the value **and its position**, which is what makes them different from the
/// Array methods they resemble: there is no third argument, because an iterator is not a
/// collection you can hand back.
fn consume(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    what: Consume,
) -> Completion<Value> {
    let wants = match what {
        Consume::ForEach => "the callback is not a function",
        _ => "the predicate is not a function",
    };
    let (walk, callback) = consuming(vm, heap, call, wants)?;
    let mut counter = 0_u64;
    while let Some(value) = walk.step(vm, heap)? {
        let arguments = [value, Value::Number(counter as f64)];
        // An abrupt callback closes the iterator and carries its own completion out — the walk
        // took possession, so it hands it back however it is leaving.
        let answer = match vm.call_value(callback, Value::Undefined, &arguments, heap) {
            Ok(answer) => answer,
            Err(raised) => {
                walk.close(vm, heap);
                return Err(raised);
            }
        };
        let liked = answer.to_boolean(heap);
        let stop = match what {
            Consume::ForEach => None,
            Consume::Find => liked.then_some(value),
            Consume::Some => liked.then_some(Value::Boolean(true)),
            Consume::Every => (!liked).then_some(Value::Boolean(false)),
        };
        if let Some(answer) = stop {
            walk.close(vm, heap);
            return Ok(answer);
        }
        counter += 1;
        super::array_methods::within_budget(heap)?;
    }
    // Nothing stopped it, so each says what an exhausted walk means for it. `every` over nothing is
    // `true` and `some` over nothing is `false` — vacuously, and for the same reason as the Array
    // methods of those names.
    Ok(match what {
        Consume::ForEach | Consume::Find => Value::Undefined,
        Consume::Some => Value::Boolean(false),
        Consume::Every => Value::Boolean(true),
    })
}

/// §27.1.4.7 `Iterator.prototype.forEach`.
fn for_each(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    consume(vm, heap, call, Consume::ForEach)
}

/// §27.1.4.11 `Iterator.prototype.find`.
fn find(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    consume(vm, heap, call, Consume::Find)
}

/// §27.1.4.10 `Iterator.prototype.some`.
fn some(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    consume(vm, heap, call, Consume::Some)
}

/// §27.1.4.6 `Iterator.prototype.every`.
fn every(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    consume(vm, heap, call, Consume::Every)
}

/// §27.1.4.9 `Iterator.prototype.reduce`.
fn reduce(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (walk, reducer) = consuming(vm, heap, call, "the reducer is not a function")?;
    // Step 5 — with no initial value the **first** value is one, and an empty iterator is then a
    // TypeError because there is nothing to answer. With one given, an empty iterator answers it.
    let (mut accumulator, mut counter) = match call.arguments.len() > 1 {
        true => (call.argument(1), 0_u64),
        false => match walk.step(vm, heap)? {
            Some(first) => (first, 1),
            None => {
                return Err(Abrupt::type_error(
                    "reduce of an empty iterator with no initial value",
                ));
            }
        },
    };
    while let Some(value) = walk.step(vm, heap)? {
        let arguments = [accumulator, value, Value::Number(counter as f64)];
        accumulator = match vm.call_value(reducer, Value::Undefined, &arguments, heap) {
            Ok(answer) => answer,
            Err(raised) => {
                walk.close(vm, heap);
                return Err(raised);
            }
        };
        counter += 1;
        super::array_methods::within_budget(heap)?;
    }
    Ok(accumulator)
}

/// §27.1.4.13 `Iterator.prototype.toArray`.
fn to_array(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(_) = call.this_value else {
        return Err(Abrupt::type_error("this is not an iterator"));
    };
    let walk = Walk::direct(vm, heap, call.this_value)?;
    let prototype = vm.realm().array_prototype();
    let array = heap.new_array(prototype, 0);
    let mut at = 0_u64;
    while let Some(value) = walk.step(vm, heap)? {
        super::array_methods::set_index(heap, array, at, value);
        at += 1;
        super::array_methods::within_budget(heap)?;
    }
    let name = key(heap, "length");
    super::set_or_throw(vm, heap, array, name, Value::Number(at as f64))?;
    Ok(Value::Object(array))
}

/// §27.1.3.1 `Iterator` — a constructor that refuses to construct anything.
///
/// Both refusals are the same rule read twice: it may not be *called*, and it may not be the thing
/// being constructed. So `new Iterator()` throws and `new (class extends Iterator {})()` does not,
/// because there the `new.target` is the subclass.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = call.new_target;
    let ours = super::global_object(heap, &vm.realm(), "Iterator");
    if !call.constructing() || matches!((target, ours), (Value::Object(a), Some(b)) if a == b) {
        return Err(Abrupt::type_error(
            "Iterator is abstract and cannot be constructed directly",
        ));
    }
    let prototype = super::prototype_from(heap, call, vm.realm().iterator_prototype());
    Ok(Value::Object(heap.new_object(Some(prototype))))
}

/// §27.1.4.1's and §27.1.4.2's setter — `SetterThatIgnoresPrototypeProperties`.
///
/// The odd one. Writing to `Iterator.prototype`'s own `constructor` or `[@@toStringTag]` is a
/// **TypeError**, but writing to the same name on something that *inherits* it makes an own
/// property there. That is what lets a generator prototype give itself a tag without the
/// assignment silently going through an inherited accessor and changing what every other iterator
/// reports.
fn ignoring_setter(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    name: PropertyKey,
) -> Completion<Value> {
    let Value::Object(receiver) = call.this_value else {
        return Err(Abrupt::type_error("this is not an object"));
    };
    if receiver == vm.realm().iterator_prototype() {
        return Err(Abrupt::type_error(
            "this property may not be written on Iterator.prototype itself",
        ));
    }
    let value = call.argument(0);
    match super::object::own_property(heap, receiver, name)?.is_some() {
        // It already has one of its own, so write through it — which may itself be an accessor.
        true => {
            super::set_or_throw(vm, heap, receiver, name, value)?;
        }
        // It has none, so `CreateDataPropertyOrThrow` makes one rather than reaching the
        // inherited accessor a second time and looping.
        false => {
            let descriptor = PropertyDescriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            };
            if !heap.define_own_property(receiver, name, &descriptor) {
                return Err(Abrupt::type_error("this property could not be created"));
            }
        }
    }
    Ok(Value::Undefined)
}

/// The `constructor` half of that pair.
fn set_constructor(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let name = key(heap, "constructor");
    ignoring_setter(vm, heap, call, name)
}

/// The `[@@toStringTag]` half.
fn set_tag(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Some(symbol) = vm.realm().well_known(super::well_known_at("toStringTag")) else {
        return Ok(Value::Undefined);
    };
    ignoring_setter(vm, heap, call, PropertyKey::from_symbol(symbol))
}

/// `get Iterator.prototype.constructor` — the constructor itself.
fn get_constructor(vm: &mut Vm, heap: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    Ok(super::global_object(heap, &vm.realm(), "Iterator").map_or(Value::Undefined, Value::Object))
}

/// `get Iterator.prototype[@@toStringTag]` — the constant `"Iterator"`.
fn get_tag(_: &mut Vm, heap: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    Ok(super::text(heap, "Iterator"))
}

/// Define one of §27.1.4's two accessor properties, both halves at once.
fn define_pair(
    heap: &mut Heap,
    realm: &Realm,
    object: ObjectId,
    name: PropertyKey,
    label: &str,
    pair: (Native, Native),
) {
    let getter = heap.new_native_function(realm.function_prototype(), pair.0);
    super::define_function_metadata(heap, getter, &format!("get {label}"), 0);
    let setter = heap.new_native_function(realm.function_prototype(), pair.1);
    super::define_function_metadata(heap, setter, &format!("set {label}"), 1);
    let _ = heap.define_own_property(
        object,
        name,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            setter: Some(Value::Object(setter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// Build §27.1's `Iterator` onto the global, and §27.1.4's consumers onto its prototype.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.iterator_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "Iterator", 0);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    super::define_value(heap, global, "Iterator", Value::Object(constructor));
    // §27.1.4's five that *make* an iterator, and §27.1.3.2's `from`, which need the helper
    // object this module deliberately does not.
    super::iterator_lazy::install(heap, realm, constructor);

    for (name, length, native) in [
        ("every", 1, every as Native),
        ("find", 1, find),
        ("forEach", 1, for_each),
        ("reduce", 1, reduce),
        ("some", 1, some),
        ("toArray", 0, to_array),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §27.1.4.1 and §27.1.4.2 — accessors rather than data properties, and their setters are the
    // reason: an assignment through one has to land on the *receiver* rather than here.
    let name = key(heap, "constructor");
    define_pair(
        heap,
        realm,
        prototype,
        name,
        "Iterator.prototype.constructor",
        (get_constructor, set_constructor),
    );
    if let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) {
        define_pair(
            heap,
            realm,
            prototype,
            PropertyKey::from_symbol(symbol),
            "Iterator.prototype[Symbol.toStringTag]",
            (get_tag, set_tag),
        );
    }
}
