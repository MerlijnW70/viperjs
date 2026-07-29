//! §20.1.2 — the `Object` statics that read or change what an object *is*, rather than one property.
//!
//! Split from [`super::object`], which holds the constructor, `Object.prototype` and the statics
//! that work on a single named property. These are the whole-object ones: sealing and freezing,
//! copying every property across, and listing what is there in three different shapes.
//!
//! # Why sealing and freezing are two levels and not two flags
//!
//! §7.3.14 `SetIntegrityLevel` does one thing twice over. Sealing makes every own property
//! non-configurable and prevents extensions; freezing does that *and* makes every data property
//! non-writable. Nothing is recorded on the object — `Object.isFrozen` re-derives the answer by
//! looking at every property, which is why freezing an object and then adding nothing can still be
//! undone in the only way it can be undone: not at all.
//!
//! That also means `Object.isFrozen({})` on a *non-extensible* empty object is `true`. There is no
//! property that could disagree, and §7.3.15 asks about the properties that are there rather than
//! about a promise that was made.

use super::object::{coerced, keys_of, own_property};
use super::{define_method, key};
use crate::heap::{Heap, NativeCall, ObjectId, Property, PropertyDescriptor, PropertyKind};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Add these statics to the `Object` constructor.
pub(super) fn install(heap: &mut Heap, realm: &Realm, function: ObjectId) {
    for (name, length, native) in [
        ("assign", 2, assign as crate::heap::Native),
        ("entries", 1, entries),
        ("freeze", 1, freeze),
        ("fromEntries", 1, from_entries),
        ("getOwnPropertyDescriptors", 1, get_own_property_descriptors),
        ("is", 2, is),
        ("isFrozen", 1, is_frozen),
        ("isSealed", 1, is_sealed),
        ("seal", 1, seal),
        ("setPrototypeOf", 2, set_prototype_of),
        ("getOwnPropertySymbols", 1, get_own_property_symbols),
        ("values", 1, values),
    ] {
        define_method(heap, realm, function, name, length, native);
    }
}

/// How thoroughly §7.3.14 `SetIntegrityLevel` is to shut an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Nothing may be added, removed or redefined — but values may still be assigned.
    Sealed,
    /// That, and no value may be assigned either.
    Frozen,
}

/// §7.3.14 `SetIntegrityLevel`.
///
/// Extensions are prevented **first**, and that ordering is observable: a getter run by one of the
/// defines below cannot add a property that then escapes being sealed.
fn shut(heap: &mut Heap, object: ObjectId, level: Level) {
    if let Some(found) = heap.object_mut(object) {
        found.prevent_extensions();
    }
    for key in heap.own_property_keys(object) {
        let Some(property) = own_property(heap, object, key) else {
            continue;
        };
        // Step 3.b.ii — an accessor keeps its functions and only loses its configurability, even
        // when freezing. There is no `[[Writable]]` on an accessor to take away.
        let writable = match (level, property.kind) {
            (Level::Frozen, PropertyKind::Data { .. }) => Some(false),
            _ => None,
        };
        let descriptor = PropertyDescriptor {
            writable,
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(object, key, &descriptor);
    }
}

/// §7.3.15 `TestIntegrityLevel` — whether an object is already shut to this level.
///
/// Derived rather than remembered. An object is frozen when *every* property says so and nothing
/// may be added, so an object that was never frozen but happens to satisfy both is frozen, and
/// answering otherwise would need a flag the specification does not have.
fn is_shut(heap: &mut Heap, object: ObjectId, level: Level) -> bool {
    if heap
        .object(object)
        .is_none_or(crate::heap::Object::is_extensible)
    {
        return false;
    }
    heap.own_property_keys(object)
        .into_iter()
        .filter_map(|key| own_property(heap, object, key))
        .all(|property| shut_enough(property, level))
}

/// Whether one property is as shut as this level requires.
fn shut_enough(property: Property, level: Level) -> bool {
    if property.configurable {
        return false;
    }
    match (level, property.kind) {
        (Level::Frozen, PropertyKind::Data { writable, .. }) => !writable,
        _ => true,
    }
}

/// §20.1.2.6 `Object.freeze`.
///
/// A primitive is handed straight back rather than refused — it has no properties to freeze, so
/// the request is already satisfied. That is step 1, and it is the same shrug `preventExtensions`
/// gives.
fn freeze(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if let Value::Object(object) = call.argument(0) {
        shut(heap, object, Level::Frozen);
    }
    Ok(call.argument(0))
}

/// §20.1.2.20 `Object.seal`.
fn seal(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if let Value::Object(object) = call.argument(0) {
        shut(heap, object, Level::Sealed);
    }
    Ok(call.argument(0))
}

/// §20.1.2.13 `Object.isFrozen`.
///
/// A primitive is **true**: it has no properties that could be changed, so it satisfies the
/// question. The asymmetry with `freeze` is only apparent — both answer "there is nothing here to
/// do", one by doing nothing and one by saying so.
fn is_frozen(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(true));
    };
    Ok(Value::Boolean(is_shut(heap, object, Level::Frozen)))
}

/// §20.1.2.15 `Object.isSealed`.
fn is_sealed(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(true));
    };
    Ok(Value::Boolean(is_shut(heap, object, Level::Sealed)))
}

/// §20.1.2.14 `Object.is` — `SameValue` (§7.2.10), which is neither `==` nor `===`.
///
/// The two places it differs from `===` are the reason it exists: `NaN` is the same value as
/// itself, and `+0` is not the same value as `-0`.
fn is(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let same = call.argument(0).same_value(&call.argument(1), heap);
    Ok(Value::Boolean(same))
}

/// §20.1.2.22 `Object.setPrototypeOf`.
///
/// Refuses anything but an object or `null` as the new prototype, and answers its *first* argument
/// rather than the object it changed — which is what makes it chainable.
fn set_prototype_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = call.argument(0);
    if matches!(target, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "undefined and null have no prototype to set",
        ));
    }
    let prototype = match call.argument(1) {
        Value::Object(prototype) => Some(prototype),
        Value::Null => None,
        _ => {
            return Err(Abrupt::type_error("a prototype must be an object or null"));
        }
    };
    // Step 4 — a primitive target is handed back untouched, having no prototype slot of its own to
    // be disappointed about.
    let Value::Object(object) = target else {
        return Ok(target);
    };
    // §10.1.2's refusals — a non-extensible object, and a chain that would come back here — are
    // the heap's, because they are what every prototype walk in the engine depends on. Here they
    // become the TypeError step 5 asks for, which is the only part that belongs to this function.
    if !heap.set_prototype_of(object, prototype) {
        return Err(Abrupt::type_error(
            "this object's prototype may not be changed",
        ));
    }
    Ok(target)
}

/// §20.1.2.1 `Object.assign(target, ...sources)`.
///
/// Reads each source's own *enumerable* keys and **gets** each one, so a getter on a source runs
/// and its answer is what is copied. That is why this cannot be a descriptor copy: the result holds
/// values, never accessors, however the source held them.
fn assign(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let target = coerced(vm, heap, call.argument(0))?;
    for source in call.arguments.iter().skip(1) {
        // Step 4.a — `undefined` and `null` sources are skipped rather than refused, so
        // `Object.assign({}, null)` is an empty object and not a TypeError.
        if matches!(source, Value::Undefined | Value::Null) {
            continue;
        }
        let from = vm.object_for(*source, heap)?;
        let Value::Object(from) = from else {
            continue;
        };
        for key in heap.own_property_keys(from) {
            if !own_property(heap, from, key).is_some_and(|property| property.enumerable) {
                continue;
            }
            let value = vm.get_property_key(Value::Object(from), key, heap)?;
            // Step 4.c.ii.1 — `Set(to, key, value, true)`, so the target's own setters apply and
            // its *refusals* become errors. A read-only property on the target stops the copy with
            // a TypeError even though the same assignment written out would be silent.
            super::set_or_throw(vm, heap, target, key, value)?;
        }
    }
    Ok(Value::Object(target))
}

/// Which half of a key-and-value pair a listing wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    /// §20.1.2.23 `Object.values`.
    Values,
    /// §20.1.2.5 `Object.entries` — both, as a two-element array.
    Entries,
}

/// §7.3.24 `EnumerableOwnProperties` — own enumerable String keys, and what each is worth.
fn listed(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, half: Half) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let mut listed = Vec::new();
    for key in keys_of(heap, object) {
        // §7.3.24 step 4 — String keys only, so a Symbol-keyed property is not among the values
        // either. Filtered before the `[[Get]]`, because that would run a getter for something
        // the answer will not hold.
        if key.as_string().is_none()
            || !own_property(heap, object, key).is_some_and(|property| property.enumerable)
        {
            continue;
        }
        let value = vm.get_property_key(Value::Object(object), key, heap)?;
        listed.push(match half {
            Half::Values => value,
            Half::Entries => {
                let Some(name) = key.as_string() else {
                    continue;
                };
                super::array::from_values(vm, heap, &[Value::String(name), value])?
            }
        });
    }
    super::array::from_values(vm, heap, &listed)
}

/// §20.1.2.11 `Object.getOwnPropertySymbols`.
///
/// The other half of `getOwnPropertyNames`, and the reason there are two: a Symbol key is not
/// hidden, but it is not listed with the names either. Every operation in the language that walks
/// an object's keys picks one of these two lists, and none of them picks both.
fn get_own_property_symbols(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let found: Vec<Value> = keys_of(heap, object)
        .into_iter()
        .filter_map(|key| key.as_symbol().map(Value::Symbol))
        .collect();
    super::array::from_values(vm, heap, &found)
}

/// §20.1.2.23 `Object.values`.
fn values(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    listed(vm, heap, call, Half::Values)
}

/// §20.1.2.5 `Object.entries`.
fn entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    listed(vm, heap, call, Half::Entries)
}

/// §20.1.2.7 `Object.fromEntries(iterable)`.
///
/// Array-likes only for now: the iterator protocol is M6, and until it exists this reads a `length`
/// and the indices under it. That covers what `Object.entries` produces and what a hand-written
/// array of pairs is, which is nearly everything this is used for — and it is a *narrower* input
/// than the specification's, not a different answer for the same input.
fn from_entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let source = coerced(vm, heap, call.argument(0))?;
    let built = heap.new_object(Some(vm.realm().object_prototype()));
    let length_key = key(heap, "length");
    let length = vm.get_property_key(Value::Object(source), length_key, heap)?;
    let count = super::array_methods::to_length(vm.to_number(length, heap)?);
    for at in 0..count {
        let index = super::array_methods::index_key(heap, at);
        let pair = vm.get_property_key(Value::Object(source), index, heap)?;
        let Value::Object(_) = pair else {
            return Err(Abrupt::type_error("each entry must be an object"));
        };
        let (first, second) = (
            super::array_methods::index_key(heap, 0),
            super::array_methods::index_key(heap, 1),
        );
        let name = vm.get_property_key(pair, first, heap)?;
        let value = vm.get_property_key(pair, second, heap)?;
        let name = vm.to_property_key(name, heap)?;
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(built, name, &descriptor);
    }
    Ok(Value::Object(built))
}

/// §20.1.2.9 `Object.getOwnPropertyDescriptors` — every own key's descriptor, on one object.
///
/// *Every* own key and not only the enumerable ones, which is what makes this and
/// `Object.defineProperties` a pair that round-trips an object exactly.
fn get_own_property_descriptors(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let realm = vm.realm();
    let built = heap.new_object(Some(realm.object_prototype()));
    for key in keys_of(heap, object) {
        let Some(property) = own_property(heap, object, key) else {
            continue;
        };
        let described = super::object::describe(heap, &realm, property);
        let descriptor = PropertyDescriptor {
            value: Some(described),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(built, key, &descriptor);
    }
    Ok(Value::Object(built))
}
