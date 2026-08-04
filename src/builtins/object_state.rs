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

use super::define_method;
use super::object::{coerced, keys_of};
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
        ("groupBy", 2, group_by),
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
fn shut(vm: &mut Vm, heap: &mut Heap, object: ObjectId, level: Level) -> Completion<bool> {
    // Step 3 — `[[PreventExtensions]]` answering `false` stops the whole operation, and
    // `Object.freeze` then throws. Only a proxy can answer `false` here, which is why this used to
    // be a step with no branch.
    if !vm.prevent_through(object, heap)? {
        return Ok(false);
    }
    for key in vm.own_keys_through(object, heap)? {
        let Some(property) = vm.own_property_through(object, key, heap)? else {
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
        crate::builtins::object::defined(vm.define_through(object, key, &descriptor, heap)?)?;
    }
    Ok(true)
}

/// §7.3.15 `TestIntegrityLevel` — whether an object is already shut to this level.
///
/// Derived rather than remembered. An object is frozen when *every* property says so and nothing
/// may be added, so an object that was never frozen but happens to satisfy both is frozen, and
/// answering otherwise would need a flag the specification does not have.
fn is_shut(vm: &mut Vm, heap: &mut Heap, object: ObjectId, level: Level) -> Completion<bool> {
    // Step 3 — an extensible object is shut to no level at all, and the question is answered
    // without looking at a single property.
    if vm.extensible_through(object, heap)? {
        return Ok(false);
    }
    for key in vm.own_keys_through(object, heap)? {
        let Some(property) = vm.own_property_through(object, key, heap)? else {
            continue;
        };
        if !shut_enough(property, level) {
            return Ok(false);
        }
    }
    Ok(true)
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
fn freeze(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if let Value::Object(object) = call.argument(0)
        && !shut(vm, heap, object, Level::Frozen)?
    {
        // Step 3 — `SetIntegrityLevel` answering `false` is a TypeError here, unlike
        // `Reflect.preventExtensions`, which reports it.
        return Err(Abrupt::type_error(
            "Object.freeze could not freeze this object",
        ));
    }
    Ok(call.argument(0))
}

/// §20.1.2.20 `Object.seal`.
fn seal(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if let Value::Object(object) = call.argument(0)
        && !shut(vm, heap, object, Level::Sealed)?
    {
        return Err(Abrupt::type_error("Object.seal could not seal this object"));
    }
    Ok(call.argument(0))
}

/// §20.1.2.13 `Object.isFrozen`.
///
/// A primitive is **true**: it has no properties that could be changed, so it satisfies the
/// question. The asymmetry with `freeze` is only apparent — both answer "there is nothing here to
/// do", one by doing nothing and one by saying so.
fn is_frozen(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(true));
    };
    Ok(Value::Boolean(is_shut(vm, heap, object, Level::Frozen)?))
}

/// §20.1.2.15 `Object.isSealed`.
fn is_sealed(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(true));
    };
    Ok(Value::Boolean(is_shut(vm, heap, object, Level::Sealed)?))
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
fn set_prototype_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
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
    if !vm.set_prototype_through(object, prototype, heap)? {
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
        for key in vm.own_keys_through(from, heap)? {
            if !vm
                .own_property_through(from, key, heap)?
                .is_some_and(|property| property.enumerable)
            {
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
    for key in keys_of(vm, heap, object)? {
        // §7.3.24 step 4 — String keys only, so a Symbol-keyed property is not among the values
        // either. Filtered before the `[[Get]]`, because that would run a getter for something
        // the answer will not hold.
        if key.as_string().is_none() {
            continue;
        }
        if !vm
            .own_property_through(object, key, heap)?
            .is_some_and(|property| property.enumerable)
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
    let found: Vec<Value> = keys_of(vm, heap, object)?
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

/// §20.1.2.7 `Object.fromEntries(iterable)`, over §7.1.5.1's `AddEntriesFromIterable`.
///
/// Takes an **iterable** and not an array-like, which is not a widening but a correction: reading
/// a `length` and the indices under it accepts an Array and answers `{}` for a `Map` — the very
/// thing this is most often pointed at. A doc comment here used to call the old reading "a
/// *narrower* input than the specification's, not a different answer for the same input", and
/// `new Map([["a", 1]])` is a counter-example to the second half.
///
/// The iterator is **closed** whenever a step of the loop goes wrong — a non-object entry, a `0`
/// or `1` whose getter throws — because the walk is abandoning something it asked to start. That
/// is §7.1.5.1 steps 3.c to 3.f, and it is the whole reason the entries are read through a helper
/// rather than in a plain `for`.
fn from_entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1's `RequireObjectCoercible`, which is *not* `ToObject`: a String is coercible and is
    // iterable, and the walk below is what refuses it — one entry at a time, because each of its
    // characters is a primitive rather than a pair.
    let source = call.argument(0);
    if matches!(source, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "undefined and null cannot be converted to an object",
        ));
    }
    let built = heap.new_object(Some(vm.realm().object_prototype()));
    let walk = super::iterator::Walk::over(vm, heap, source)?;
    loop {
        let Some(entry) = walk.step(vm, heap)? else {
            return Ok(Value::Object(built));
        };
        let outcome = entry_into(vm, heap, built, entry);
        if outcome.is_err() {
            walk.close(vm, heap);
            return outcome.map(|()| Value::Object(built));
        }
    }
}

/// One `[key, value]` pair of §7.1.5.1, defined on `built` — steps 3.c to 3.f.
///
/// Its own function so that every way it can fail leaves by one path, which is what the caller's
/// single `IteratorClose` is written against. Inlined, the three abrupt completions would each
/// need their own close and the third would be the one that got forgotten.
fn entry_into(vm: &mut Vm, heap: &mut Heap, built: ObjectId, entry: Value) -> Completion<()> {
    let Value::Object(_) = entry else {
        return Err(Abrupt::type_error("each entry must be an object"));
    };
    let (first, second) = (
        super::array_methods::index_key(heap, 0),
        super::array_methods::index_key(heap, 1),
    );
    let name = vm.get_property_key(entry, first, heap)?;
    let value = vm.get_property_key(entry, second, heap)?;
    // The key is converted **after** the value is read, which is observable: an entry whose `1`
    // getter throws never asks the key for its `toString`.
    let name = vm.to_property_key(name, heap)?;
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    // `CreateDataPropertyOrThrow` on an ordinary object made a moment ago, which nothing can
    // refuse: it is extensible and holds no property that is not configurable.
    let _ = heap.define_own_property(built, name, &descriptor);
    Ok(())
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
    for key in keys_of(vm, heap, object)? {
        let Some(property) = vm.own_property_through(object, key, heap)? else {
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

/// §20.1.2.13 `Object.groupBy ( items, callback )`.
///
/// The answer inherits from **null**, which is the point of it: the keys come from the program's
/// own data, so a group called `"toString"` or `"__proto__"` has to be an ordinary property rather
/// than something that collides with the prototype chain. `Object.create(null)` is what the clause
/// says and it is not an optimisation.
///
/// The properties are made in the order the keys were first seen — §7.3.35 keeps an ordered list
/// for exactly this — so `Object.keys` of the answer reports the order the callback discovered.
fn group_by(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let groups = super::iterator::group_by(
        vm,
        heap,
        call.argument(0),
        call.argument(1),
        super::iterator::Keying::Property,
    )?;
    let object = heap.new_object(None);
    for (found, elements) in groups {
        let array = super::array::from_values(vm, heap, &elements)?;
        // Already a String or a Symbol — §7.3.35 ran `ToPropertyKey` before grouping, which is
        // where a throwing `toString` would have closed the iterator. This conversion cannot fail.
        let name = vm.to_property_key(found, heap)?;
        heap.define_own_property(object, name, &crate::heap::PropertyDescriptor::data(array));
    }
    Ok(Value::Object(object))
}
