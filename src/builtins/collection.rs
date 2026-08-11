//! §24.1 and §24.2 — `Map` and `Set`, which are one implementation with two names.
//!
//! # Why they are together
//!
//! Every method of one has a twin in the other with the same body: `has` is `has`, `delete` is
//! `delete`, `forEach` differs only in what it passes as the second argument. §24.2's `Set` is
//! §24.1's `Map` whose value *is* its key, and writing them apart would be two chances to fix a bug
//! in one place. What genuinely differs — the adder the constructor uses, whether an iterable's
//! elements are entries or values, what an iterator step answers — is named at the call and is a
//! line each.
//!
//! # What a `Set` stores
//!
//! §24.2's `[[SetData]]` is a list of values, so the entry's key and value are the same value. That
//! is what makes `set.entries()` answer `[v, v]` — a shape that looks like a mistake and is
//! deliberate, because it lets a `Set` be walked by anything written for a `Map`.

use super::{define_method, define_value, key};
use crate::heap::{
    Collection, CollectionKind, Heap, Iterated, Iteration, Native, NativeCall, ObjectId,
    PropertyDescriptor, PropertyKey,
};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `Map` and `Set` into `heap` as properties of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    build(
        heap,
        realm,
        global,
        CollectionKind::Map,
        realm.map_prototype(),
        realm.map_iterator_prototype(),
    );
    build(
        heap,
        realm,
        global,
        CollectionKind::Set,
        realm.set_prototype(),
        realm.set_iterator_prototype(),
    );
}

/// One of the two, with the methods that differ named where they differ.
fn build(
    heap: &mut Heap,
    realm: &Realm,
    global: ObjectId,
    kind: CollectionKind,
    prototype: ObjectId,
    iterator_prototype: ObjectId,
) {
    let map = kind == CollectionKind::Map;
    let name = if map { "Map" } else { "Set" };
    let constructor = heap.new_native_constructor(
        realm.function_prototype(),
        if map { construct_map } else { construct_set },
        realm.id(),
    );
    super::define_function_metadata(heap, constructor, name, 0);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, global, name, Value::Object(constructor));
    define_value(heap, prototype, "constructor", Value::Object(constructor));
    // §24.1.4.2 and §24.2.4.2 — `get Map[@@species]` and `get Set[@@species]`, both of which
    // answer the receiver. Neither `Map` nor `Set` has a method that *uses* one, which is why this
    // could be missing without anything noticing: the accessor exists for a subclass that wants to
    // be asked, and §24.3 and §24.4 deliberately give `WeakMap` and `WeakSet` none at all.
    super::buffer::define_species(heap, realm, constructor);
    // §24.1.2.1 — `Map.groupBy`, and only on `Map`: §24.2.2 gives `Set` no such static, because a
    // Set has no value to hold the group in.
    if map {
        define_method(heap, realm, constructor, "groupBy", 2, group_by);
    }

    // §24.1.3 and §24.2.3, each in the order its clause lists them. `length` is what the clause
    // writes, which for `forEach` is 1 even though it reads two arguments.
    let shared: &[(&str, u32, Native)] = match map {
        true => &[
            ("clear", 0, map_clear),
            ("delete", 1, map_delete),
            ("forEach", 1, map_for_each),
            ("has", 1, map_has),
        ],
        false => &[
            ("clear", 0, set_clear),
            ("delete", 1, set_delete),
            ("forEach", 1, set_for_each),
            ("has", 1, set_has),
        ],
    };
    for (name, length, native) in shared {
        define_method(heap, realm, prototype, name, *length, *native);
    }
    let own: &[(&str, u32, Native)] = match map {
        true => &[
            ("get", 1, get),
            ("set", 2, set),
            ("entries", 0, map_entries),
        ],
        false => &[("add", 1, add), ("entries", 0, set_entries)],
    };
    for (name, length, native) in own {
        define_method(heap, realm, prototype, name, *length, *native);
    }
    // §24.1.3.8 and §24.2.3.8 — `keys` and `values`. For a `Set` they are the **same function
    // object**, which a program can see: `Set.prototype.keys === Set.prototype.values`. §24.2.3.8
    // says so outright, and it follows from a Set's key being its value.
    define_method(
        heap,
        realm,
        prototype,
        "values",
        0,
        if map { map_values } else { set_values },
    );
    match map {
        true => define_method(heap, realm, prototype, "keys", 0, map_keys),
        false => {
            alias(heap, prototype, "keys", "values");
            // §24.2.4's seven, which only a `Set` has.
            super::set_ops::install(heap, realm, prototype);
        }
    }
    // §24.1.3.12 and §24.2.3.11 — `[@@iterator]` is `entries` for a `Map` and `values` for a
    // `Set`, and is the same function object as the one it names.
    alias_symbol(
        heap,
        prototype,
        "iterator",
        if map { "entries" } else { "values" },
    );

    // §24.1.3.10 and §24.2.3.9 — `size` is an **accessor**, not a data property, so it cannot be
    // assigned and reads whatever the collection currently holds.
    let getter = heap.new_native_function(
        realm.function_prototype(),
        if map { map_size } else { set_size },
        realm.id(),
    );
    super::define_function_metadata(heap, getter, "get size", 0);
    let name_key = key(heap, "size");
    let _ = heap.define_own_property(
        prototype,
        name_key,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
    super::tag_with(heap, prototype, name);

    // §24.1.5 and §24.2.5 — the iterator prototypes, which inherit from %IteratorPrototype% and so
    // get `[@@iterator]` from it.
    define_method(heap, realm, iterator_prototype, "next", 0, next);
    super::tag_with(
        heap,
        iterator_prototype,
        if map { "Map Iterator" } else { "Set Iterator" },
    );
}

/// Give `prototype` a second name for a method it already has — the *same* function object.
fn alias(heap: &mut Heap, prototype: ObjectId, name: &str, existing: &str) {
    let Some(value) = super::own_value(heap, prototype, existing) else {
        return;
    };
    define_value(heap, prototype, name, value);
}

/// The same, under a well-known Symbol.
fn alias_symbol(heap: &mut Heap, prototype: ObjectId, symbol: &str, existing: &str) {
    let (Some(found), Some(value)) = (
        heap.well_known(super::well_known_at(symbol)),
        super::own_value(heap, prototype, existing),
    ) else {
        return;
    };
    let _ = heap.define_own_property(
        prototype,
        PropertyKey::from_symbol(found),
        &PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §24.1.1.1 — `new Map(iterable)`.
fn construct_map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    construct(vm, heap, call, CollectionKind::Map)
}

/// §24.2.1.1 — `new Set(iterable)`.
fn construct_set(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    construct(vm, heap, call, CollectionKind::Set)
}

/// Both constructors, which differ in the method they call for each element and nothing else.
fn construct(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: CollectionKind,
) -> Completion<Value> {
    let map = kind == CollectionKind::Map;
    // Step 1 — a plain call is a TypeError, because there would be no `new.target` to take a
    // prototype from and because §24 says so.
    if !call.constructing() {
        return Err(Abrupt::type_error(match map {
            true => "Map must be called with new",
            false => "Set must be called with new",
        }));
    }
    let prototype = super::prototype_from(vm, heap, call, |realm| match map {
        true => realm.map_prototype(),
        false => realm.set_prototype(),
    })?;
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_collection(Collection::new(kind));
    }
    // Steps 5 and 6 — `undefined` **and null** both mean "no iterable", and an empty collection is
    // the answer for either. Anything else is iterated.
    let iterable = call.argument(0);
    if matches!(iterable, Value::Undefined | Value::Null) {
        return Ok(Value::Object(object));
    }
    // Step 7 — the *adder* is read from the object, once, and called for each element. Read
    // through the property, so a subclass that overrode `set` has its own called — which is
    // observable and is why this is not a direct call to the internals.
    let name = key(heap, if map { "set" } else { "add" });
    let adder = vm.get_property_key(Value::Object(object), name, heap)?;
    if !heap.is_callable(adder) {
        return Err(Abrupt::type_error(
            "the collection's adder is not a function",
        ));
    }
    add_entries_from_iterable(
        vm,
        heap,
        object,
        adder,
        iterable,
        map,
        "each entry of a Map's iterable must be an object",
    )?;
    Ok(Value::Object(object))
}

/// §24.1.1.2 `AddEntriesFromIterable`, for all four collections.
///
/// **One element at a time, and every failure closes the iterator.** Both halves used to be wrong
/// and both are visible to a program: this gathered the whole iterable into a list and then looped,
/// so `new Map([0, 1])` drew *every* value before refusing the first, and `return` was never called
/// at all. An **infinite** iterable made the difference louder still — the eager form ran until the
/// heap budget stopped it and reported a RangeError where the clause wants a TypeError on the first
/// element, which is what `iterator-items-are-not-object-close-iterator.js` asks about.
///
/// Shared rather than written per collection: `WeakMap` and `WeakSet` had a verbatim copy of this
/// loop, so fixing `Map` and `Set` left them exactly as they were. Four constructors differ in two
/// things — whether an element is an entry pair and what to call it in the message — and in nothing
/// else.
pub(super) fn add_entries_from_iterable(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    adder: Value,
    iterable: Value,
    keyed: bool,
    entries_must_be_objects: &'static str,
) -> Completion<()> {
    let walk = super::iterator::Walk::over(vm, heap, iterable)?;
    while let Some(element) = walk.step(vm, heap)? {
        // Closed with the *original* completion kept, which is what `IfAbruptCloseIterator` means
        // by `? IteratorClose(iteratorRecord, k)`: §7.4.9 step 4 discards whatever the `return`
        // itself does when it is closing because something already went wrong. `Walk::close` is
        // that form — `iterator-close-failure-after-set-failure.js` is the file that tells them
        // apart, and it wants the adder's error rather than the close's.
        if let Err(abrupt) = add_one(
            vm,
            heap,
            object,
            adder,
            element,
            keyed,
            entries_must_be_objects,
        ) {
            walk.close(vm, heap);
            return Err(abrupt);
        }
    }
    Ok(())
}

/// One element of §24.1.1.2's iterable, added — steps 3.c to 3.i.
///
/// Its own function so the caller has a single `Result` to close the iterator on. Written the other
/// way round, each of the four throwing steps needs its own close and its own early return, and the
/// one that gets forgotten is the one no test happens to reach.
fn add_one(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    adder: Value,
    element: Value,
    keyed: bool,
    entries_must_be_objects: &'static str,
) -> Completion<()> {
    if !keyed {
        vm.call_value(adder, Value::Object(object), &[element], heap)?;
        return Ok(());
    }
    // Step 3.c — each element of a keyed collection's iterable must itself be an object with a `0`
    // and a `1`. A primitive there is a TypeError rather than an entry with `undefined` in it, and
    // the check is *before* either `Get`.
    let Value::Object(_) = element else {
        return Err(Abrupt::type_error(entries_must_be_objects));
    };
    let first = key(heap, "0");
    let second = key(heap, "1");
    let entry_key = vm.get_property_key(element, first, heap)?;
    let entry_value = vm.get_property_key(element, second, heap)?;
    vm.call_value(
        adder,
        Value::Object(object),
        &[entry_key, entry_value],
        heap,
    )?;
    Ok(())
}

/// The collection `this` is, or the TypeError §24 asks for.
///
/// Every method starts here, and the check is about the *internal slot* rather than the prototype:
/// `Map.prototype.get.call({})` throws because the object is not a Map, not because it is missing a
/// method. That is what makes these methods safe to borrow onto a subclass and unsafe to fake.
pub(super) fn collection_of(
    heap: &Heap,
    this: Value,
    kind: CollectionKind,
    what: &'static str,
) -> Completion<ObjectId> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error(what));
    };
    match heap
        .object(object)
        .and_then(crate::heap::Object::collection)
    {
        // The *kind*, and not merely the presence. §24.1.3.7 requires `[[MapData]]` where
        // §24.2.3.7 requires `[[SetData]]`, so `Map.prototype.has.call(new Set())` is a
        // TypeError. The two read alike and are different functions with different
        // requirements; checking only that *some* collection is there lets each answer
        // questions about the other.
        Some(found) if found.kind() == kind => Ok(object),
        _ => Err(Abrupt::type_error(what)),
    }
}

/// §24.1.3.6 — `Map.prototype.get`.
fn get(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::Map,
        "get was called on something that is not a Map",
    )?;
    let found = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap))
        .and_then(|at| {
            heap.object(object)
                .and_then(crate::heap::Object::collection)
                .and_then(|found| found.value_at(at))
        });
    // §24.1.3.6 step 5 — a key that is not there answers `undefined`, which is deliberately the
    // same answer a key *mapped* to `undefined` gives. `has` is the question that tells them apart.
    Ok(found.unwrap_or(Value::Undefined))
}

/// §24.1.3.9 — `Map.prototype.set`.
fn set(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::Map,
        "set was called on something that is not a Map",
    )?;
    place(heap, object, call.argument(0), call.argument(1));
    // Step 8 — the *map* comes back, not the value, which is what makes `set` chainable.
    Ok(call.this_value)
}

/// §24.2.3.1 — `Set.prototype.add`, which is `set` with one argument used twice.
fn add(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        CollectionKind::Set,
        "add was called on something that is not a Set",
    )?;
    let value = call.argument(0);
    place(heap, object, value, value);
    Ok(call.this_value)
}

/// §24.1.3.9 step 4 and §24.2.3.1 step 4 — look for the key, then put the value where it belongs.
///
/// Two statements rather than one, because the lookup asks the heap about String contents and the
/// change writes to the heap: holding the collection open across both would be borrowing the heap
/// to read while it is borrowed to write.
pub(super) fn place(heap: &mut Heap, object: ObjectId, key: Value, value: Value) {
    let at = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(key, heap));
    let Some(found) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::collection_mut)
    else {
        return;
    };
    match at {
        Some(at) => found.replace_at(at, value),
        None => found.push(key, value),
    }
}

/// §24.1.3.7 and §24.2.3.7 — `has`.
/// §24.1.3.7 — `Map.prototype.has`.
fn map_has(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    has(heap, call, CollectionKind::Map)
}

/// §24.2.3.7 — `Set.prototype.has`, which reads the same and requires the other slot.
fn set_has(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    has(heap, call, CollectionKind::Set)
}

/// The body both share — whether the key is there.
fn has(heap: &Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        "has was called on something that is not a Map or a Set",
    )?;
    let found = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap));
    Ok(Value::Boolean(found.is_some()))
}

/// §24.1.3.3 and §24.2.3.4 — `delete`, which answers whether there was anything to delete.
/// §24.1.3.3 — `Map.prototype.delete`.
fn map_delete(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    delete(heap, call, CollectionKind::Map)
}

/// §24.2.3.4 — `Set.prototype.delete`, which reads the same and requires the other slot.
fn set_delete(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    delete(heap, call, CollectionKind::Set)
}

/// The body both share — whether there was anything to delete.
fn delete(heap: &mut Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        "delete was called on something that is not a Map or a Set",
    )?;
    let at = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.position_of(call.argument(0), heap));
    if let Some(at) = at
        && let Some(found) = heap
            .object_mut(object)
            .and_then(crate::heap::Object::collection_mut)
    {
        found.delete_at(at);
    }
    Ok(Value::Boolean(at.is_some()))
}

/// §24.1.3.1 and §24.2.3.2 — `clear`, which answers `undefined` and not the collection.
/// §24.1.3.1 — `Map.prototype.clear`.
fn map_clear(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    clear(heap, call, CollectionKind::Map)
}

/// §24.2.3.2 — `Set.prototype.clear`, which reads the same and requires the other slot.
fn set_clear(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    clear(heap, call, CollectionKind::Set)
}

/// The body both share — `undefined`, and not the collection.
fn clear(heap: &mut Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        "clear was called on something that is not a Map or a Set",
    )?;
    if let Some(found) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::collection_mut)
    {
        found.clear();
    }
    Ok(Value::Undefined)
}

/// §24.1.3.10 and §24.2.3.9 — `get size`.
/// §24.1.3.10 — `get Map.prototype.size`.
fn map_size(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    size(heap, call, CollectionKind::Map)
}

/// §24.2.3.9 — `get Set.prototype.size`, which reads the same and requires the other slot.
fn set_size(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    size(heap, call, CollectionKind::Set)
}

/// The body both share — how many entries there are.
fn size(heap: &Heap, call: &NativeCall<'_>, kind: CollectionKind) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        "size was read on something that is not a Map or a Set",
    )?;
    let count = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .map_or(0, crate::heap::Collection::size);
    Ok(Value::Number(count as f64))
}

/// §24.1.3.5 and §24.2.3.6 — `forEach`.
/// §24.1.3.5 — `Map.prototype.forEach`.
fn map_for_each(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    for_each(vm, heap, call, CollectionKind::Map)
}

/// §24.2.3.6 — `Set.prototype.forEach`, which reads the same and requires the other slot.
fn set_for_each(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    for_each(vm, heap, call, CollectionKind::Set)
}

/// The body both share — the walk itself.
fn for_each(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: CollectionKind,
) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        kind,
        "forEach was called on something that is not a Map or a Set",
    )?;
    let callback = call.argument(0);
    if !heap.is_callable(callback) {
        return Err(Abrupt::type_error("the callback is not a function"));
    }
    let receiver = call.argument(1);
    // By *position* rather than over a copied list, because §24.1.3.5 step 4 walks the entries as
    // they are: an entry added while this is running is visited, and one deleted before it is
    // reached is not. A snapshot taken up front would get both backwards.
    let mut from = 0;
    while let Some((at, entry_key, entry_value)) = heap
        .object(object)
        .and_then(crate::heap::Object::collection)
        .and_then(|found| found.live_from(from))
    {
        from = at + 1;
        // §24.1.3.5 step 4.a.i — value, *then* key, then the collection. A Set passes its value
        // twice, which is what makes a Set walkable by a callback written for a Map.
        vm.call_value(
            callback,
            receiver,
            &[entry_value, entry_key, call.this_value],
            heap,
        )?;
    }
    Ok(Value::Undefined)
}

/// §24.1.3.4 and the Set's — `entries`.
fn map_entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Entries, CollectionKind::Map)
}

/// §24.2.3.5 — `Set.prototype.entries`, which answers `[v, v]` because a Set's key is its value.
fn set_entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Entries, CollectionKind::Set)
}

/// §24.1.3.8 — `keys`.
fn map_keys(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Keys, CollectionKind::Map)
}

/// §24.1.3.11 and §24.2.3.10 — `values`.
fn map_values(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Values, CollectionKind::Map)
}

/// §24.2.3.10 — `Set.prototype.values`, which is also its `keys` and its `[@@iterator]`.
fn set_values(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Values, CollectionKind::Set)
}

/// §24.1.5.1 — an iterator over this collection, remembering a position and nothing else.
fn iterator(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: Iterated,
    over: CollectionKind,
) -> Completion<Value> {
    let object = collection_of(
        heap,
        call.this_value,
        over,
        "an iterator was asked of something that is not a Map or a Set",
    )?;
    let prototype = match over == CollectionKind::Map {
        true => vm.realm().map_iterator_prototype(),
        false => vm.realm().set_iterator_prototype(),
    };
    let made = heap.new_iterator(
        prototype,
        Iteration {
            over: Value::Object(object),
            at: 0,
            kind,
            done: false,
        },
    );
    Ok(Value::Object(made))
}

/// §24.1.5.2.1 and §24.2.5.2.1 — one step.
fn next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(iterator) = call.this_value else {
        return Err(Abrupt::type_error(
            "next was called on something that is not an iterator",
        ));
    };
    let Some(state) = heap
        .object(iterator)
        .and_then(crate::heap::Object::iteration)
    else {
        return Err(Abrupt::type_error(
            "next was called on something that is not an iterator",
        ));
    };
    let (over, from, kind, done) = (state.over, state.at as usize, state.kind, state.done);
    // Once it has run out it stays run out, whatever the collection does afterwards — the same
    // rule an Array Iterator has, and for the same reason. An `over` that is not an object cannot
    // come from any program, and it lands in the same place a finished iterator does rather than
    // in a branch of its own that nothing could reach to test.
    let found = match (done, over) {
        (false, Value::Object(target)) => heap
            .object(target)
            .and_then(crate::heap::Object::collection)
            .and_then(|found| found.live_from(from)),
        _ => None,
    };
    let Some((at, entry_key, entry_value)) = found else {
        if let Some(state) = heap
            .object_mut(iterator)
            .and_then(crate::heap::Object::iteration_mut)
        {
            state.done = true;
        }
        return super::iterator::result(vm, heap, Value::Undefined, true);
    };
    if let Some(state) = heap
        .object_mut(iterator)
        .and_then(crate::heap::Object::iteration_mut)
    {
        state.at = at as u64 + 1;
    }
    let answer = match kind {
        Iterated::Keys => entry_key,
        Iterated::Values => entry_value,
        // §24.1.5.2.1 step 10 — a two-element Array, made fresh each step so that a program
        // keeping one does not have it change under it.
        _ => super::array::from_values(vm, heap, &[entry_key, entry_value])?,
    };
    super::iterator::result(vm, heap, answer, false)
}

/// §24.1.2.1 `Map.groupBy ( items, callback )`.
///
/// The keys are grouped by `SameValue` after §24.5.1 folds `-0` into `+0`, which is the difference
/// from §20.1.2.13: no conversion runs, so an object or a `NaN` is a key in its own right and two
/// callbacks answering equal-looking objects make two groups.
///
/// The `Map` is built directly rather than through `Construct(%Map%)` and `set`: the clause appends
/// to `[[MapData]]`, so a program that replaced `Map.prototype.set` does not see it called — which
/// is the opposite of what `new Map(iterable)` does two clauses away, and is deliberate in both.
fn group_by(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let groups = super::iterator::group_by(
        vm,
        heap,
        call.argument(0),
        call.argument(1),
        super::iterator::Keying::Zero,
    )?;
    let object = heap.new_object(Some(vm.realm().map_prototype()));
    if let Some(found) = heap.object_mut(object) {
        found.set_collection(Collection::new(CollectionKind::Map));
    }
    for (found, elements) in groups {
        let array = super::array::from_values(vm, heap, &elements)?;
        place(heap, object, found, array);
    }
    Ok(Value::Object(object))
}
