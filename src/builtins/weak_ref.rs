//! §26.1's `WeakRef` and §26.2's `FinalizationRegistry`.
//!
//! # What a program can actually observe
//!
//! Very little, and deliberately. `deref` answers the target until the collector has taken it, and
//! ViperJS collects only when its embedder says to — so within any one script it answers the same
//! thing every time, which is what §9.10.4's note requires of an engine that *does* collect
//! part-way through. The cleanup callback is never called at all; §26.2 permits that outright, and
//! the alternative needs a decision about when to collect that §9.10's note leaves to the
//! implementation and that ViperJS has not made. See [`crate::heap::Weak`] for the longer version.
//!
//! So what is left to get right is the surface: which values may be held, which arguments are
//! refused, and the brand checks.
//!
//! # Why the brand checks answer the slot rather than the object
//!
//! Both of §26's objects keep their state in one slot, so "does this have something weak on it"
//! does not tell them apart — and a `register` that passed such a check would find no cells and
//! quietly do nothing, which is worse than the TypeError §26.2.3.1 asks for, because nothing tells
//! the caller their registration went nowhere. That was a real bug here, caught by a row asking
//! for `FinalizationRegistry.prototype.register.call(new WeakRef({}), …)`.
//!
//! So each check answers the **thing**: [`reference`] answers a target, [`registry`] answers a
//! registry. A caller holding one cannot then ask a second time and have to say what it would do
//! if the answer had changed — which is where the unreachable arm would have been.

use super::collection::tag_with;
use super::{define_method, define_value};
use crate::heap::{Cell, Heap, Holdable, NativeCall, ObjectId, Registry, Weak};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §7.2.10 `CanBeHeldWeakly` — an Object, or a Symbol that is not in §20.4.2.2's registry.
///
/// Answers the value *as* a [`Holdable`] rather than a yes or no, so a caller that asked cannot
/// then go on to store something else. A registered Symbol is held for the life of the realm, so a
/// weak reference to one could never become stale — it would be an ordinary reference with a
/// misleading name, which is what §7.2.10 exists to prevent.
fn holdable(value: Value, heap: &Heap) -> Option<Holdable> {
    match value {
        Value::Object(id) => Some(Holdable::Object(id)),
        Value::Symbol(id) => heap
            .symbol_registry_key(id)
            .is_none()
            .then_some(Holdable::Symbol(id)),
        _ => None,
    }
}

/// The target `this` holds — §26.1.3.2's brand check, answering it rather than the object.
fn reference(heap: &Heap, this: Value, what: &'static str) -> Completion<Holdable> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error(what));
    };
    match heap.object(object).and_then(crate::heap::Object::weak) {
        Some(Weak::Ref(target)) => Ok(*target),
        // A `FinalizationRegistry`, an object with no weak slot at all, and a primitive all arrive
        // here, and all three are the same TypeError.
        _ => Err(Abrupt::type_error(what)),
    }
}

/// The registry `this` is — §26.2.3's brand check, answering it rather than the object.
fn registry<'a>(
    heap: &'a mut Heap,
    this: Value,
    what: &'static str,
) -> Completion<&'a mut Registry> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error(what));
    };
    match heap
        .object_mut(object)
        .and_then(crate::heap::Object::weak_mut)
    {
        Some(Weak::Registry(found)) => Ok(found),
        _ => Err(Abrupt::type_error(what)),
    }
}

/// §26.1.1.1 `WeakRef(target)`.
fn construct_ref(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error("WeakRef must be called with new"));
    }
    // Step 2 — checked before anything is made, so a refused target leaves nothing behind.
    let Some(target) = holdable(call.argument(0), heap) else {
        return Err(Abrupt::type_error(
            "a WeakRef target must be an object or an unregistered symbol",
        ));
    };
    let prototype = super::prototype_from(vm, heap, call, Realm::weak_ref_prototype)?;
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_weak(Weak::Ref(target));
    }
    Ok(Value::Object(object))
}

/// §26.1.3.2 `WeakRef.prototype.deref`.
fn deref(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = reference(
        heap,
        call.this_value,
        "deref was called on something that is not a WeakRef",
    )?;
    // Step 4 — the target if it is still there, and `undefined` once it is not. DR-0010 never
    // reuses a slot, so an empty one means collected and can never come to mean anything else.
    //
    // A Symbol is asked the same question in its own arena, and it is asked about the *Symbol*
    // rather than its description: one made with no description is alive and has none, so reading
    // absence as death would make `new WeakRef(Symbol()).deref()` answer `undefined`.
    let alive = match target {
        Holdable::Object(id) => heap.object(id).is_some(),
        Holdable::Symbol(id) => heap.symbol(id).is_some(),
    };
    Ok(match alive {
        true => target.as_value(),
        false => Value::Undefined,
    })
}

/// §26.2.1.1 `FinalizationRegistry(cleanupCallback)`.
fn construct_registry(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error(
            "FinalizationRegistry must be called with new",
        ));
    }
    let cleanup = call.argument(0);
    // Step 2 — a registry with nothing to call is refused at construction rather than at the
    // moment it would have called it, which is a moment that may never come.
    if !heap.is_callable(cleanup) {
        return Err(Abrupt::type_error("the cleanup callback is not a function"));
    }
    let prototype = super::prototype_from(vm, heap, call, Realm::finalization_registry_prototype)?;
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_weak(Weak::Registry(Registry {
            cleanup,
            cells: Vec::new(),
        }));
    }
    Ok(Value::Object(object))
}

/// §26.2.3.1 `FinalizationRegistry.prototype.register`.
fn register(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let held = call.argument(1);
    let Some(target) = holdable(call.argument(0), heap) else {
        return Err(Abrupt::type_error(
            "a FinalizationRegistry target must be an object or an unregistered symbol",
        ));
    };
    // Step 5 — the held value may not *be* the target. It is held strongly, so a registration of
    // that shape would keep the target alive through its own cell and the callback could never be
    // reached. `SameValue`, and comparing the [`Holdable`] is exactly that: the general relation
    // needs the heap only to compare two Strings by their contents, and a String cannot be here.
    if holdable(held, heap) == Some(target) {
        return Err(Abrupt::type_error(
            "a FinalizationRegistry cannot hold its own target",
        ));
    }
    // Step 6 — an unregister token must be holdable too, and `undefined` means there is none.
    // Nothing else stands in for absence: `null` is a value that cannot be held, not an omission.
    let token = match call.argument(2) {
        Value::Undefined => None,
        given => match holdable(given, heap) {
            Some(found) => Some(found),
            None => {
                return Err(Abrupt::type_error(
                    "an unregister token must be an object or an unregistered symbol",
                ));
            }
        },
    };
    let found = registry(
        heap,
        call.this_value,
        "register was called on something that is not a FinalizationRegistry",
    )?;
    found.cells.push(Cell {
        target,
        held,
        token,
    });
    Ok(Value::Undefined)
}

/// §26.2.3.4 `FinalizationRegistry.prototype.unregister`.
fn unregister(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 3 — a token that could never have been stored is a TypeError rather than a miss, which
    // is the opposite of how §24.3.3.3's `get` treats one. The difference is that `unregister` is
    // being *told* something rather than asked, and a program passing a number has made a mistake
    // worth hearing about.
    let Some(token) = holdable(call.argument(0), heap) else {
        return Err(Abrupt::type_error(
            "an unregister token must be an object or an unregistered symbol",
        ));
    };
    let found = registry(
        heap,
        call.this_value,
        "unregister was called on something that is not a FinalizationRegistry",
    )?;
    Ok(Value::Boolean(found.unregister(token)))
}

/// Build `WeakRef` and `FinalizationRegistry` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let reference = realm.weak_ref_prototype();
    let constructor =
        heap.new_native_constructor(realm.function_prototype(), construct_ref, realm.id());
    super::define_function_metadata(heap, constructor, "WeakRef", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(reference));
    define_value(heap, global, "WeakRef", Value::Object(constructor));
    define_value(heap, reference, "constructor", Value::Object(constructor));
    define_method(heap, realm, reference, "deref", 0, deref);
    tag_with(heap, reference, "WeakRef");

    let registry = realm.finalization_registry_prototype();
    let constructor =
        heap.new_native_constructor(realm.function_prototype(), construct_registry, realm.id());
    super::define_function_metadata(heap, constructor, "FinalizationRegistry", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(registry));
    define_value(
        heap,
        global,
        "FinalizationRegistry",
        Value::Object(constructor),
    );
    define_value(heap, registry, "constructor", Value::Object(constructor));
    define_method(heap, realm, registry, "register", 2, register);
    define_method(heap, realm, registry, "unregister", 1, unregister);
    tag_with(heap, registry, "FinalizationRegistry");
}
