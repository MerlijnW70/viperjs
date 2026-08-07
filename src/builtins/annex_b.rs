//! §B.2.2 — what Annex B keeps on `Object.prototype`: one accessor and four methods.
//!
//! # Why these exist at all
//!
//! They predate §6.2.6's property descriptors. `__defineGetter__` was how an accessor was made
//! before `Object.defineProperty` existed, and Annex B records it as "normative optional" because
//! the web has too much code using it for a browser to remove. DR-0008 already takes the position
//! that ViperJS implements Annex B's *lexical* extensions; these are library ones, and they cost
//! four short functions.
//!
//! # What they are not
//!
//! Not `Object.defineProperty` with a shorter spelling. §B.2.2.2 defines the property as
//! **enumerable and configurable**, where a descriptor with those fields absent gets `false` for
//! both — so `o.__defineGetter__("x", f)` and `Object.defineProperty(o, "x", {get: f})` produce
//! properties that enumerate differently. That is what makes them worth their own rows.
//!
//! And the two `__lookup*__` methods walk the **prototype chain**, where
//! `getOwnPropertyDescriptor` does not: they answer about the accessor a program would actually
//! reach, which is what they were for.
//!
//! §B.2.2.1's `__proto__` is not `Object.getPrototypeOf` with a shorter spelling either, and the
//! setter is where they part: a value that is neither an Object nor `null` is *ignored* here and
//! is a TypeError there. The web depends on `o.__proto__ = undefined` doing nothing quietly.
//!
//! # None of this runs user code except through an internal method
//!
//! The four methods' key conversion is `ToPropertyKey` of a value already read, and every other
//! step of theirs is a heap operation. `__proto__`'s two halves are the exception and have to be:
//! they go through `[[GetPrototypeOf]]` and `[[SetPrototypeOf]]`, which a Proxy may trap, so they
//! take the interpreter and can raise whatever a trap raises. This paragraph said "nothing here
//! can run user code" before the accessor landed.

use super::define_method;
use super::object::{defined, this_object};
use crate::heap::{Heap, NativeCall, PropertyDescriptor, PropertyKind};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Which half of an accessor a method is about.
///
/// The four methods are two operations over this, and writing each once is what keeps the getter
/// pair and the setter pair from drifting.
#[derive(Clone, Copy)]
enum Half {
    /// `[[Get]]`.
    Getter,
    /// `[[Set]]`.
    Setter,
}

/// §B.2.2.1 and §B.2.2.2 — define an accessor with one half supplied.
fn define(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, half: Half) -> Completion<Value> {
    let object = this_object(call, "this method requires an object")?;
    let function = call.argument(1);
    // Step 2, and it comes **before** the key is converted — so
    // `o.__defineGetter__({toString() { throw 1; }}, 1)` is a TypeError about the getter rather
    // than whatever the key's `toString` would have thrown.
    if !heap.is_callable(function) {
        return Err(Abrupt::type_error(match half {
            Half::Getter => "the getter is not a function",
            Half::Setter => "the setter is not a function",
        }));
    }
    let key = vm.to_property_key(call.argument(0), heap)?;
    // Step 3 — **enumerable and configurable**, which is not what an absent field in a descriptor
    // means. This is the whole difference from `Object.defineProperty(o, k, {get: f})`.
    let (getter, setter) = match half {
        Half::Getter => (Some(function), None),
        Half::Setter => (None, Some(function)),
    };
    let descriptor = PropertyDescriptor {
        getter,
        setter,
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    // `DefinePropertyOrThrow`, so a property that cannot be redefined is a TypeError rather than a
    // silent nothing — and through `Vm::define_through`, which is §10.1.6 *or* §10.5.6: a Proxy's
    // `defineProperty` trap runs for `p.__defineGetter__('x', f)` exactly as it does for
    // `Object.defineProperty`. The heap's own define walks past a Proxy, so a trap that threw was
    // never called and one that refused was never heard.
    defined(vm.define_through(object, key, &descriptor, heap)?)?;
    Ok(Value::Undefined)
}

/// §B.2.2.3 and §B.2.2.4 — find the accessor a program would reach, along the whole chain.
fn look_up(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, half: Half) -> Completion<Value> {
    let object = this_object(call, "this method requires an object")?;
    let key = vm.to_property_key(call.argument(0), heap)?;
    let mut walk = object;
    // Step 3's loop, iteratively: a chain is as long as a program makes it (DR-0002).
    loop {
        // Step 3.b — the **first** object with this property answers, whatever kind it is. A data
        // property found part-way up answers `undefined` and stops the walk rather than being
        // stepped over, so the answer is "the accessor you would reach" and not "the nearest
        // accessor anywhere above you".
        // Steps 3.a and 3.c are `[[GetOwnProperty]]` and `[[GetPrototypeOf]]`, and both are `?` —
        // so a Proxy anywhere on the chain has its traps called and its throws reported. Reading
        // the heap directly walked **past** every one of them: a trap that threw was never called,
        // and one that answered a descriptor of its own was never asked.
        if let Some(property) = vm.own_property_through(walk, key, heap)? {
            return Ok(match (property.kind, half) {
                (PropertyKind::Accessor { getter, .. }, Half::Getter) => getter,
                (PropertyKind::Accessor { setter, .. }, Half::Setter) => setter,
                _ => Value::Undefined,
            });
        }
        let Some(next) = vm.prototype_through(walk, heap)? else {
            return Ok(Value::Undefined);
        };
        walk = next;
    }
}

/// §B.2.2.2 `Object.prototype.__defineGetter__`.
fn define_getter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    define(vm, heap, call, Half::Getter)
}

/// §B.2.2.3 `Object.prototype.__defineSetter__`.
fn define_setter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    define(vm, heap, call, Half::Setter)
}

/// §B.2.2.4 `Object.prototype.__lookupGetter__`.
fn lookup_getter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    look_up(vm, heap, call, Half::Getter)
}

/// §B.2.2.5 `Object.prototype.__lookupSetter__`.
fn lookup_setter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    look_up(vm, heap, call, Half::Setter)
}

/// §B.2.2.1.1 `get Object.prototype.__proto__`.
///
/// `ToObject` and then `[[GetPrototypeOf]]`, so a **primitive** answers about the wrapper it
/// stands for: `"a".__proto__` is `String.prototype`. Only `undefined` and `null` have no object
/// and are the TypeError.
fn get_proto(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = super::object::coerced(vm, heap, call.this_value)?;
    Ok(match vm.prototype_through(object, heap)? {
        Some(prototype) => Value::Object(prototype),
        None => Value::Null,
    })
}

/// §B.2.2.1.2 `set Object.prototype.__proto__`.
///
/// # Three ways to do nothing, and one to throw
///
/// The setter is deliberately lenient in a way the getter is not, and the asymmetry is the whole
/// of it. Step 1 is `RequireObjectCoercible`, so `undefined` and `null` are a TypeError — but a
/// value that is neither an Object nor `null` (step 2) and a **receiver** that is not an Object
/// (step 3) both answer `undefined` and change nothing at all. So `(1).__proto__ = {}` is silent,
/// and so is `o.__proto__ = 5`.
///
/// That is not `Object.setPrototypeOf`'s behaviour, which refuses a bad value with a TypeError.
/// Two spellings of one operation, and they disagree about what a mistake is: this one predates
/// the other and the web depends on it not throwing.
///
/// Step 5 is the one refusal, and it is the *outcome* of the internal method rather than a check
/// of its own — a cycle or a non-extensible object, which §10.1.2 already decides.
fn set_proto(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1.
    if matches!(call.this_value, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "undefined and null have no prototype to set",
        ));
    }
    // Step 2 — read before the receiver is judged, though neither order is observable: nothing
    // here runs a program.
    let prototype = match call.argument(0) {
        Value::Object(prototype) => Some(prototype),
        Value::Null => None,
        _ => return Ok(Value::Undefined),
    };
    // Step 3.
    let Value::Object(object) = call.this_value else {
        return Ok(Value::Undefined);
    };
    // Steps 4 and 5. Through the internal method, so a Proxy's `setPrototypeOf` trap is what
    // answers — and a trap that says `false` is this TypeError.
    if !vm.set_prototype_through(object, prototype, heap)? {
        return Err(Abrupt::type_error(
            "this object's prototype may not be changed",
        ));
    }
    Ok(Value::Undefined)
}

/// Build Annex B's additions to `Object.prototype` — §B.2.2.1's accessor and four methods.
pub fn install(heap: &mut Heap, realm: &Realm) {
    let prototype = realm.object_prototype();
    for (name, length, native) in [
        ("__defineGetter__", 2, define_getter as crate::heap::Native),
        ("__defineSetter__", 2, define_setter),
        ("__lookupGetter__", 1, lookup_getter),
        ("__lookupSetter__", 1, lookup_setter),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §B.2.2.1 — an **accessor**, and that is the point rather than an implementation detail. A
    // script reads the two halves off the descriptor and calls them on other receivers, which is
    // how `Object.getOwnPropertyDescriptor(Object.prototype, "__proto__").set.call(o, p)` works
    // and why a special case in the property lookup would not do.
    let getter = heap.new_native_function(realm.function_prototype(), get_proto, realm.id());
    super::define_function_metadata(heap, getter, "get __proto__", 0);
    let setter = heap.new_native_function(realm.function_prototype(), set_proto, realm.id());
    super::define_function_metadata(heap, setter, "set __proto__", 1);
    let slot = super::key(heap, "__proto__");
    let _ = heap.define_own_property(
        prototype,
        slot,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            setter: Some(Value::Object(setter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}
