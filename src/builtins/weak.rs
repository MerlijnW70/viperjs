//! §24.3's `WeakMap` and §24.4's `WeakSet` — the collections that do not keep their keys.
//!
//! # What makes them a separate module rather than two more rows in `collection`
//!
//! Almost nothing is shared. A `WeakMap` has no `size`, no `clear`, no `forEach` and no iterator,
//! and §24.3.3's four methods are the whole of it — because every one of those would let a program
//! see when the collector ran, and a program that can see that can see a difference between two
//! runs of the same code. The list of methods *is* the design.
//!
//! # What a key may be
//!
//! §7.2.10 `CanBeHeldWeakly`: an Object, or a Symbol that is **not** in §20.4.2.2's registry.
//! Everything else is refused. The Symbol case is the interesting one and the rule behind it is
//! that a weak key must be able to *go away*: `Symbol.for("a")` is held by the registry for as
//! long as the process runs, so an entry keyed by one could never be collected and would be a leak
//! wearing a weak map's name. An ordinary `Symbol("a")` has no such holder and is allowed.
//!
//! # Where the weakness actually lives
//!
//! Not here. `set` and `get` are an ordinary insert and lookup; what makes the map weak is that
//! [`crate::heap::Heap::collect`] does not walk these entries and prunes the ones whose keys it
//! could not reach. A program cannot tell the difference by asking — only by not running out of
//! memory — which is why the tests for it are in the collector rather than beside these methods.

use super::collection::{collection_of, place};
use super::{define_method, define_value, key};
use crate::heap::{Collection, CollectionKind, Heap, Native, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §7.2.10 `CanBeHeldWeakly` — whether this value may be a weak key.
///
/// Answers a question rather than throwing, because its two callers want different things from
/// the answer: `set` and `add` throw when it is false, and `get`, `has` and `delete` simply say
/// "not there". That difference is §24.3.3's, and it is why a lookup with a number key is not an
/// error — nothing could ever have stored one, so nothing is there.
fn can_be_held_weakly(value: Value, heap: &Heap) -> bool {
    match value {
        Value::Object(_) => true,
        // A registered Symbol is held by the registry for the life of the realm, so an entry keyed
        // by one could never be collected — §7.2.10 keeps it out rather than letting a weak map
        // quietly become a strong one.
        Value::Symbol(symbol) => heap.symbol_registry_key(symbol).is_none(),
        _ => false,
    }
}

/// Both constructors — §24.3.1.1 and §24.4.1.1, which differ in the adder they call.
fn construct(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: CollectionKind,
) -> Completion<Value> {
    // Step 1 — a plain call has no `new.target` to take a prototype from, and §24.3.1.1 says so.
    if !call.constructing() {
        return Err(Abrupt::type_error(match kind.keyed() {
            true => "WeakMap must be called with new",
            false => "WeakSet must be called with new",
        }));
    }
    let prototype = super::prototype_from(vm, heap, call, |realm| match kind.keyed() {
        true => realm.weak_map_prototype(),
        false => realm.weak_set_prototype(),
    })?;
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_collection(Collection::new(kind));
    }
    // Steps 5 and 6 — `undefined` and **null** both mean "no iterable", and an empty collection is
    // the answer to either.
    let iterable = call.argument(0);
    if matches!(iterable, Value::Undefined | Value::Null) {
        return Ok(Value::Object(object));
    }
    // Step 7 — the adder is read *through the property*, once, so a subclass that overrode `set`
    // has its own called. That is observable, which is why this is not a direct insert.
    let name = key(heap, if kind.keyed() { "set" } else { "add" });
    let adder = vm.get_property_key(Value::Object(object), name, heap)?;
    if !heap.is_callable(adder) {
        return Err(Abrupt::type_error(
            "the collection's adder is not a function",
        ));
    }
    // §24.3.1.1 step 8 and §24.4.1.1 step 8 both defer to §24.1.1.2, and so does this: the loop
    // was a verbatim copy of `Map`'s and kept its bugs when that one was fixed.
    super::collection::add_entries_from_iterable(
        vm,
        heap,
        object,
        adder,
        iterable,
        kind.keyed(),
        "each entry of a WeakMap's iterable must be an object",
    )?;
    Ok(Value::Object(object))
}

/// §24.3.1.1 `WeakMap`.
fn construct_weak_map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    construct(vm, heap, call, CollectionKind::WeakMap)
}

/// §24.4.1.1 `WeakSet`.
fn construct_weak_set(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    construct(vm, heap, call, CollectionKind::WeakSet)
}

/// §24.3.3.4 `WeakMap.prototype.set`.
fn set(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::WeakMap,
        "set was called on something that is not a WeakMap",
    )?;
    let entry_key = call.argument(0);
    // Step 4 — storing a key that cannot be held weakly is a TypeError, where *looking one up* is
    // merely a miss. The asymmetry is deliberate: a program that stores one has made a mistake it
    // wants to hear about, and a program that looks one up has asked a question with an answer.
    if !can_be_held_weakly(entry_key, heap) {
        return Err(Abrupt::type_error(
            "a WeakMap key must be an object or an unregistered symbol",
        ));
    }
    place(heap, object, entry_key, call.argument(1));
    Ok(call.this_value)
}

/// §24.4.3.1 `WeakSet.prototype.add`.
fn add(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::WeakSet,
        "add was called on something that is not a WeakSet",
    )?;
    let value = call.argument(0);
    if !can_be_held_weakly(value, heap) {
        return Err(Abrupt::type_error(
            "a WeakSet value must be an object or an unregistered symbol",
        ));
    }
    // The value is its own key, exactly as in a `Set` — which is what makes the collector's rule
    // ("keep the value while the key lives") do nothing for a weak set, and correctly so.
    place(heap, object, value, value);
    Ok(call.this_value)
}

/// §24.3.3.3 `WeakMap.prototype.get`.
fn get(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::WeakMap,
        "get was called on something that is not a WeakMap",
    )?;
    let found = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap));
    // Step 4 — a key that is not there answers `undefined`, and a key that could never have been
    // there is the same answer for the same reason.
    Ok(found
        .and_then(|at| {
            heap.object(object)
                .and_then(crate::heap::Object::collection)
                .and_then(|collection| collection.value_at(at))
        })
        .unwrap_or(Value::Undefined))
}

/// §24.3.3.2 and §24.4.3.3 — `has`, which is the same question in both.
fn has(heap: &Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        match kind.keyed() {
            true => "has was called on something that is not a WeakMap",
            false => "has was called on something that is not a WeakSet",
        },
    )?;
    let found = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap));
    Ok(Value::Boolean(found.is_some()))
}

/// §24.3.3.2 `WeakMap.prototype.has`.
fn map_has(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    has(heap, call, CollectionKind::WeakMap)
}

/// §24.4.3.3 `WeakSet.prototype.has`.
fn set_has(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    has(heap, call, CollectionKind::WeakSet)
}

/// §24.3.3.1 and §24.4.3.2 — `delete`, which answers whether there was anything to delete.
fn delete(heap: &mut Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        match kind.keyed() {
            true => "delete was called on something that is not a WeakMap",
            false => "delete was called on something that is not a WeakSet",
        },
    )?;
    let at = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap));
    let Some(at) = at else {
        return Ok(Value::Boolean(false));
    };
    if let Some(found) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::collection_mut)
    {
        found.delete_at(at);
    }
    Ok(Value::Boolean(true))
}

/// §24.3.3.1 `WeakMap.prototype.delete`.
fn map_delete(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    delete(heap, call, CollectionKind::WeakMap)
}

/// §24.4.3.2 `WeakSet.prototype.delete`.
fn set_delete(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    delete(heap, call, CollectionKind::WeakSet)
}

/// Build `WeakMap` and `WeakSet` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    for kind in [CollectionKind::WeakMap, CollectionKind::WeakSet] {
        let prototype = match kind.keyed() {
            true => realm.weak_map_prototype(),
            false => realm.weak_set_prototype(),
        };
        let constructor = heap.new_native_constructor(
            realm.function_prototype(),
            match kind.keyed() {
                true => construct_weak_map,
                false => construct_weak_set,
            },
            realm.id(),
        );
        super::define_function_metadata(heap, constructor, kind.name(), 0);
        super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
        define_value(heap, global, kind.name(), Value::Object(constructor));
        define_value(heap, prototype, "constructor", Value::Object(constructor));
        // §24.3.3 and §24.4.3 in the order their clauses list them. There is no `size`, no
        // `clear`, no `forEach` and no iterator, and that absence is the design rather than a gap
        // — each of them would tell a program when the collector had run.
        let methods: &[(&str, u32, Native)] = match kind.keyed() {
            true => &[
                ("delete", 1, map_delete as Native),
                ("get", 1, get),
                ("has", 1, map_has),
                ("set", 2, set),
            ],
            false => &[
                ("add", 1, add as Native),
                ("delete", 1, set_delete),
                ("has", 1, set_has),
            ],
        };
        for (name, length, native) in methods {
            define_method(heap, realm, prototype, name, *length, *native);
        }
        super::tag_with(heap, prototype, kind.name());
    }
}
