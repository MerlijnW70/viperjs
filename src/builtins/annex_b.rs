//! §B.2.2 — the four accessor methods Annex B keeps on `Object.prototype`.
//!
//! # Why these exist at all
//!
//! They predate §6.2.6's property descriptors. `__defineGetter__` was how an accessor was made
//! before `Object.defineProperty` existed, and Annex B records it as "normative optional" because
//! the web has too much code using it for a browser to remove. DR-0008 already takes the position
//! that praxis implements Annex B's *lexical* extensions; these are library ones, and they cost
//! four short functions.
//!
//! # What they are not
//!
//! Not `Object.defineProperty` with a shorter spelling. §B.2.2.1 defines the property as
//! **enumerable and configurable**, where a descriptor with those fields absent gets `false` for
//! both — so `o.__defineGetter__("x", f)` and `Object.defineProperty(o, "x", {get: f})` produce
//! properties that enumerate differently. That is what makes them worth their own rows.
//!
//! And the two `__lookup*__` methods walk the **prototype chain**, where
//! `getOwnPropertyDescriptor` does not: they answer about the accessor a program would actually
//! reach, which is what they were for.
//!
//! Nothing here can run user code. The key conversion is `ToPropertyKey` of a value that has
//! already been read, and every other step is a heap operation — which is why none of these four
//! takes the interpreter.

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

/// §B.2.2.1 `Object.prototype.__defineGetter__`.
fn define_getter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    define(vm, heap, call, Half::Getter)
}

/// §B.2.2.2 `Object.prototype.__defineSetter__`.
fn define_setter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    define(vm, heap, call, Half::Setter)
}

/// §B.2.2.3 `Object.prototype.__lookupGetter__`.
fn lookup_getter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    look_up(vm, heap, call, Half::Getter)
}

/// §B.2.2.4 `Object.prototype.__lookupSetter__`.
fn lookup_setter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    look_up(vm, heap, call, Half::Setter)
}

/// Build Annex B's four methods onto `Object.prototype`.
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
}
