//! §28.2's `Proxy` and §28.2.2.1's `Proxy.revocable`.
//!
//! # What is here and what is not
//!
//! Only the two ways to make one. Every internal method a proxy overrides is in
//! [`crate::vm::Vm`], because a trap is JavaScript and answering `[[Get]]` therefore needs the
//! interpreter — see that module for why that is the one exotic object ViperJS could not put in the
//! heap's dispatch.
//!
//! # Why `Proxy` has no prototype property
//!
//! §28.2.2 gives the constructor none, and a proxy's prototype is its target's — asked through the
//! target rather than fixed at construction. So `new Proxy({}, {}) instanceof Object` is true
//! because the *target* is an Object, and `Proxy.prototype` is `undefined`. A constructor with no
//! prototype at all is unusual enough to be worth saying: it is the only one in the language.

use super::{define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId, Proxy};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §28.2.1.1 `ProxyCreate` — the target and handler both have to be objects.
fn create(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<ObjectId> {
    let (Value::Object(target), Value::Object(handler)) = (call.argument(0), call.argument(1))
    else {
        return Err(Abrupt::type_error(
            "a proxy's target and handler must both be objects",
        ));
    };
    // §10.5's proxy has **no prototype of its own**: `[[GetPrototypeOf]]` asks the target. So it is
    // made with none rather than with `Object.prototype`, and `instanceof` still works because the
    // walk goes through the target.
    let object = heap.new_object(None);
    if let Some(found) = heap.object_mut(object) {
        found.set_proxy(Proxy::new(target, handler));
    }
    // §10.5 — a proxy has a `[[Call]]` only if the *initial* target had one, and a `[[Construct]]`
    // only if the target was a constructor. Decided here and never revisited, which is why an
    // `apply` trap on a handler whose target is a plain object does nothing: there is no
    // `[[Call]]` for it to be the body of. It is also what makes `typeof` answer without asking
    // the handler anything.
    let callable = heap
        .object(target)
        .and_then(crate::heap::Object::call)
        .is_some();
    if callable {
        let constructs = heap
            .object(target)
            .is_some_and(crate::heap::Object::is_constructor);
        // §10.5 gives a proxy no `[[Realm]]`: `GetFunctionRealm` answers one by recursing into
        // its target, so the id recorded here is never read and the running realm is as honest a
        // placeholder as any.
        heap.make_callable(object, through, constructs, vm.realm().id());
    }
    let _ = vm;
    Ok(object)
}

/// §10.5.12 and §10.5.13 — a callable proxy's body.
///
/// One function for both, because a call and a construction differ only in `[[NewTarget]]` and
/// `NativeCall` already carries it. The work is in [`crate::vm::Vm`], where the trap can be called.
fn through(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if call.constructing() {
        return vm.proxy_construct(call.function, call.arguments, call.new_target, heap);
    }
    vm.proxy_call(call.function, call.this_value, call.arguments, heap)
}

/// §28.2.1.1 `Proxy(target, handler)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — a plain call is a TypeError. `Proxy` is the one constructor with no `prototype`
    // property, so there would be nothing for `new.target` to read even if it were allowed.
    if !call.constructing() {
        return Err(Abrupt::type_error("Proxy must be called with new"));
    }
    Ok(Value::Object(create(vm, heap, call)?))
}

/// §28.2.2.1 `Proxy.revocable(target, handler)`.
///
/// Answers an object with the proxy and a function that turns it off. The function is not a method
/// of the proxy: revocation has to be something you can hand out *without* handing out the ability
/// to use the proxy, and a method on the proxy would be neither.
fn revocable(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let proxy = create(vm, heap, call)?;
    let revoker =
        heap.new_native_function(vm.realm().function_prototype(), revoke, vm.realm().id());
    super::define_function_metadata(heap, revoker, "", 0);
    // §28.2.2.1.1's `[[RevocableProxy]]` — carried on the function object, because a built-in's
    // body is a bare function pointer holding no state. The same shape §27.2's resolve functions
    // use for their `[[Promise]]`.
    if let Some(found) = heap.object_mut(revoker) {
        found.set_role(crate::heap::Role::Revoke(proxy));
    }
    let answer = heap.new_object(Some(vm.realm().object_prototype()));
    define_value(heap, answer, "proxy", Value::Object(proxy));
    define_value(heap, answer, "revoke", Value::Object(revoker));
    Ok(Value::Object(answer))
}

/// §28.2.2.1.1 — the revocation function, which empties the pair and answers `undefined`.
fn revoke(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(function) = Value::Object(call.function) else {
        return Ok(Value::Undefined);
    };
    let held = match heap.object(function).and_then(crate::heap::Object::role) {
        Some(crate::heap::Role::Revoke(proxy)) => Some(*proxy),
        _ => None,
    };
    // Step 2 — a second call finds the slot already empty and does nothing, which is why revoking
    // twice is not an error.
    if let Some(proxy) = held
        && let Some(found) = heap
            .object_mut(proxy)
            .and_then(crate::heap::Object::proxy_mut)
    {
        found.revoke();
    }
    Ok(Value::Undefined)
}

/// Build `Proxy` onto the global.
pub(super) fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let constructor =
        heap.new_native_constructor(realm.function_prototype(), construct, realm.id());
    super::define_function_metadata(heap, constructor, "Proxy", 2);
    define_value(heap, global, "Proxy", Value::Object(constructor));
    define_method(heap, realm, constructor, "revocable", 2, revocable);
}
