//! §23.1 — `Array`, in the part that is the object rather than the fifty methods on it.
//!
//! The exotic behaviour is [`crate::heap`]'s, because it is a property rule and belongs where
//! properties live. What is here is the constructor, the prototype, and the one static that asks a
//! question nothing else can answer.

use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, ErrorKind, Value};
use crate::vm::Vm;

use super::{define_method, define_value, key};

/// §23.1.1.1 `Array(...)`.
///
/// # The argument that is not an element
///
/// One argument that is a Number is a **length**, not a contents: `Array(3)` is three holes and
/// `Array("3")` is one string. That is §23.1.1.1 steps 2 and 3, it is the reason `Array(3)` and
/// `[3]` differ, and it is why nobody uses the constructor to make a literal.
///
/// A length that is not an integer index is a RangeError rather than a rounding — `Array(1.5)`
/// throws where `a.length = 1.5` throws for the same reason.
pub fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §23.1.1.1 step 1 — `ArrayCreate(…, proto)` where the proto comes from new.target, so a
    // `class D extends Array {}` makes arrays that inherit `D.prototype`.
    let prototype = super::prototype_from(vm, heap, call, Realm::array_prototype)?;
    if call.arguments.len() == 1
        && let Value::Number(length) = call.argument(0)
    {
        let rounded = length as u32;
        if f64::from(rounded) != length {
            return Err(Abrupt::Raised(
                ErrorKind::Range,
                "an array length must be an integer index",
            ));
        }
        return Ok(Value::Object(heap.new_array(prototype, rounded)));
    }
    // Step 4 — every argument is an element, and that includes the zero-argument case.
    let array = heap.new_array(prototype, 0);
    for (at, value) in call.arguments.iter().enumerate() {
        let key = key(heap, &at.to_string());
        let descriptor = PropertyDescriptor {
            value: Some(*value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(array, key, &descriptor);
    }
    Ok(Value::Object(array))
}

/// A dense Array holding exactly these values — `CreateArrayFromList` (§7.3.18).
///
/// The array a built-in hands back when it has computed a list rather than been given one. Its
/// elements are ordinary in every way, which is what §7.3.18 means by an array "whose elements are
/// the elements of list".
pub(crate) fn from_values(vm: &Vm, heap: &mut Heap, values: &[Value]) -> Completion<Value> {
    let array = heap.new_array(vm.realm().array_prototype(), 0);
    for (at, value) in values.iter().enumerate() {
        let key = key(heap, &at.to_string());
        let descriptor = PropertyDescriptor {
            value: Some(*value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(array, key, &descriptor);
    }
    Ok(Value::Object(array))
}

/// §23.1.2.2 `Array.isArray`.
///
/// The only way to ask. `instanceof Array` answers a different question — it walks a prototype
/// chain, so it is false for an array from another realm and true for anything given
/// `Array.prototype`— and `typeof` says `"object"` for both. This asks what the object *is*.
pub fn is_array(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(heap.is_array_through(object)?))
}

/// Build `Array` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.array_prototype();
    let function = heap.new_native_constructor(realm.function_prototype(), construct, realm.id());
    super::define_function_metadata(heap, function, "Array", 1);

    // §23.1.4 — `Array.prototype` is not writable, not enumerable and not configurable, for the
    // same reason `Object.prototype` is not.
    let key = key(heap, "prototype");
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(prototype)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(function, key, &descriptor);
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, "Array", Value::Object(function));
    for (name, length, native) in [
        ("from", 1, from as crate::heap::Native),
        ("isArray", 1, is_array),
        ("of", 0, of),
    ] {
        define_method(heap, realm, function, name, length, native);
    }
    // §23.1.2.5 `get Array [@@species]` — an accessor answering `this`, which is what makes
    // `ArraySpeciesCreate` hand a subclass of Array its own kind back from `map` and `filter` and
    // `slice`. Without it every one of those answers a plain Array, and a subclass silently loses
    // its type on the first method call.
    super::buffer::define_species(heap, realm, function);
    define_unscopables(heap, prototype);
}

/// §23.1.3.35 `Array.prototype [ %Symbol.unscopables% ]`.
///
/// # What it is for
///
/// §9.1.1.2.1 step 5 reads this off the object a `with` was opened on, and a name listed here is
/// one the `with` does **not** bind. That is what stops `with (array) { … }` from shadowing an
/// outer `values` or `keys` with a method the array only has because ES2015 added it — the whole
/// point being that code written before the method existed must go on meaning what it meant.
///
/// # Two things the list is not
///
/// **It is not "the methods added after ES5".** It is a fixed list in the specification, and the
/// membership of a given method is a decision TC39 took when that method landed rather than
/// anything derivable here. `Array.prototype.with` is the case that shows it: it is a
/// change-array-by-copy method exactly like `toReversed` and `toSorted`, it is **not** in the list,
/// and `built-ins/Array/prototype/Symbol.unscopables/change-array-by-copy.js` asserts its absence.
/// The reason is that `with` is a reserved word, so no code has ever referred to a binding by that
/// name and there is nothing for it to shadow.
///
/// **It is not conditioned on what ViperJS implements.** The list is what the clause says whether or
/// not the method beside it exists, because a script reads the object rather than calling through
/// it.
///
/// # The attributes differ between the two levels, and both are checked
///
/// Each entry is `CreateDataPropertyOrThrow`, so all three attributes are true — these are
/// properties a script may delete or overwrite, and `propertyHelper.js` verifies it. The property
/// holding them is the other set: not writable, not enumerable, **configurable**. One helper for
/// both would get exactly one of them wrong.
fn define_unscopables(heap: &mut Heap, prototype: ObjectId) {
    // Step 1 — `OrdinaryObjectCreate(null)`. A null prototype because the keys are ordinary method
    // names: with `Object.prototype` under it, a `with` over an array would find `toString` and
    // `valueOf` here and read them as blocked.
    let list = heap.new_object(None);
    for name in [
        "at",
        "copyWithin",
        "entries",
        "fill",
        "find",
        "findIndex",
        "findLast",
        "findLastIndex",
        "flat",
        "flatMap",
        "includes",
        "keys",
        "toReversed",
        "toSorted",
        "toSpliced",
        "values",
    ] {
        super::create_data_property(heap, list, name, Value::Boolean(true));
    }
    let Some(symbol) = heap.well_known(super::well_known_at("unscopables")) else {
        return;
    };
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(list)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(
        prototype,
        crate::heap::PropertyKey::from_symbol(symbol),
        &descriptor,
    );
}

/// §23.1.2.1 `Array.from(items[, mapfn[, thisArg]])`.
///
/// Two different readings of one argument, chosen by whether it has an `@@iterator`. An iterable
/// is *iterated*; anything else is read as an array-like, by its `length` and its indices. That is
/// why `Array.from("ab")` is two characters and `Array.from({length: 2})` is two `undefined`s —
/// the first has an iterator and the second does not, and neither is a special case of the other.
fn from(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let items = call.argument(0);
    let mapper = match call.argument(1) {
        Value::Undefined => None,
        // Step 2 — checked before anything is read, so a bad `mapfn` is refused before the
        // iterable is asked for its iterator.
        value if is_callable(heap, value) => Some(value),
        _ => return Err(Abrupt::type_error("the mapping function must be callable")),
    };
    let receiver = call.argument(2);
    // Steps 5 and 7 both make the target **before** anything is read, and the two say different
    // things about the arguments: the iterator path constructs with none, because nobody has
    // counted the iterable yet, and the array-like path passes the `length` it just read.
    // Steps 5 and 7 both make the target **before** anything is read, and the two differ in what
    // they tell the constructor: the iterator path passes no arguments, because nobody has counted
    // the iterable yet, and the array-like path passes the `length` it just read.
    if let Some(method) = iterator_method_of(vm, heap, items)? {
        let walk = super::iterator::Walk::from_method(vm, heap, items, method)?;
        let target = collect_into(vm, heap, call.this_value, &[])?;
        let Value::Object(object) = target else {
            return Err(Abrupt::type_error("a constructor must answer an object"));
        };
        let mut at = 0_u64;
        // **The write is inside the walk**, which is step 6.e and not a tidiness: steps 6.e.vii and
        // 6.e.ix both close the iterator when they throw, and an iterator drained to `done` first
        // has nothing left to close. So a mapper that throws, or a target that refuses the index,
        // tells the iterator the walk stopped — which is what `closeCount` measures.
        while let Some(value) = walk.step(vm, heap)? {
            let mapped = match mapper {
                // Step 6.e.vii — the index goes with the value, which is what makes
                // `Array.from({length: 3}, (_, i) => i)` the idiom it is.
                Some(mapper) => {
                    match vm.call_value(mapper, receiver, &[value, Value::Number(at as f64)], heap)
                    {
                        Ok(mapped) => mapped,
                        Err(error) => {
                            walk.close(vm, heap);
                            return Err(error);
                        }
                    }
                }
                None => value,
            };
            if let Err(error) = super::array_methods::create_index(heap, object, at, mapped) {
                walk.close(vm, heap);
                return Err(error);
            }
            at += 1;
            // DR-0013 — an iterator that never says it is done would otherwise grow the target
            // until the process died, and every step allocates the object §7.4.13 wraps its answer
            // in. The same budget the Array methods watch is what notices.
            super::array_methods::within_budget(heap)?;
        }
        set_final_length(vm, heap, target, at)?;
        return Ok(target);
    }
    let taken = spread_array_like(vm, heap, items)?;
    let length = taken.len() as u64;
    let target = collect_into(vm, heap, call.this_value, &[Value::Number(length as f64)])?;
    let Value::Object(object) = target else {
        return Err(Abrupt::type_error("a constructor must answer an object"));
    };
    for (at, value) in taken.into_iter().enumerate() {
        let mapped = match mapper {
            Some(mapper) => {
                let index = Value::Number(at as f64);
                vm.call_value(mapper, receiver, &[value, index], heap)?
            }
            None => value,
        };
        super::array_methods::create_index(heap, object, at as u64, mapped)?;
    }
    set_final_length(vm, heap, target, length)?;
    Ok(target)
}

/// The iterator `items` hands out, or `None` if it has none — §7.4.2 with `GetMethod`.
///
/// No arm for `undefined` and `null`: reading a property of either is already the TypeError
/// §23.1.2.1 step 4 asks for, by way of the `ToObject` it would otherwise reach. A guard here
/// would answer the same thing one step earlier.
pub(super) fn iterator_of(vm: &mut Vm, heap: &mut Heap, items: Value) -> Completion<Option<Value>> {
    let Some(method) = iterator_method_of(vm, heap, items)? else {
        return Ok(None);
    };
    let iterator = vm.call_value(method, items, &[], heap)?;
    match iterator {
        Value::Object(_) => Ok(Some(iterator)),
        _ => Err(Abrupt::type_error("an iterator must be an object")),
    }
}

/// The `@@iterator` method itself, before it is called — §7.3.10 `GetMethod`.
///
/// The half [`iterator_of`] does first, kept apart because §23.1.2.1 needs the *method* rather than
/// the iterator: §7.4.4 `GetIteratorFromMethod` reads `next` off what the call answered and keeps
/// the pair as a record, and a walk that can be closed needs both halves. Calling the method here
/// and reading `next` again there would be the pair-of-lookups mistake §27.1.4.1 already records.
fn iterator_method_of(vm: &mut Vm, heap: &mut Heap, items: Value) -> Completion<Option<Value>> {
    let Some(symbol) = heap.well_known(super::well_known_at("iterator")) else {
        return Ok(None);
    };
    let key = crate::heap::PropertyKey::from_symbol(symbol);
    let method = vm.get_property_key(items, key, heap)?;
    // §7.3.10 `GetMethod` — `undefined` and `null` both mean "there is none", and anything else
    // that is not callable is a TypeError rather than a fall through to the array-like reading.
    if matches!(method, Value::Undefined | Value::Null) {
        return Ok(None);
    }
    // No callability check of its own: calling something that is not a function is already the
    // TypeError §7.3.10 asks for, raised one step later and by the machinery that knows what a
    // callable is. A guard here would be a second way to say the same thing.
    Ok(Some(method))
}

/// §23.1.2.1 steps 7 and 8 — reading something that is not iterable by its `length`.
fn spread_array_like(vm: &mut Vm, heap: &mut Heap, items: Value) -> Completion<Vec<Value>> {
    let object = vm.object_for(items, heap)?;
    let name = key(heap, "length");
    let length = vm.get_property_key(object, name, heap)?;
    let length = super::array_methods::to_length(vm.to_number(length, heap)?);
    let mut taken = Vec::new();
    for at in 0..length {
        let index = super::array_methods::index_key(heap, at);
        taken.push(vm.get_property_key(object, index, heap)?);
        super::array_methods::within_budget(heap)?;
    }
    Ok(taken)
}

/// §23.1.2.1 step 5 and §23.1.2.3 step 4 — the object the values are collected into.
///
/// `this` is the constructor when it is one, which is the whole of why these two are `Array`'s
/// *static* methods and not free functions: `Array.of.call(MyArray, 1)` builds a `MyArray`, and a
/// subclass inherits both and gets itself. A `this` that is not a constructor — including the
/// `undefined` a plain call passes — falls back to `ArrayCreate`, so `Array.of(1)` is an ordinary
/// array and nothing about the common case changes.
///
/// `arguments` is what the two clauses differ by: §23.1.2.3 and §23.1.2.1's array-like path pass
/// the length, its iterator path passes nothing. That is not a detail — a constructor is free to
/// read its argument, and `Array.from(iterable)` must not tell it a length nobody has counted yet.
fn collect_into(
    vm: &mut Vm,
    heap: &mut Heap,
    this_value: Value,
    arguments: &[Value],
) -> Completion<Value> {
    let constructor = matches!(this_value, Value::Object(id)
        if heap.object(id).is_some_and(crate::heap::Object::is_constructor));
    if constructor {
        return vm.construct_value(this_value, arguments, heap);
    }
    Ok(Value::Object(
        heap.new_array(vm.realm().array_prototype(), 0),
    ))
}

/// §23.1.2.1 step 6.e.x and §23.1.2.3 step 8 — `Set(A, "length", len, true)`.
///
/// The `true` is `Throw`, and it is what the whole step is for: the target may be anything a
/// constructor answered with, so the write can be refused by a `length` that is not writable and
/// can throw outright from a setter. Discarding the answer — which is what a plain write does —
/// leaves a half-filled object being handed back as though it had worked.
fn set_final_length(vm: &mut Vm, heap: &mut Heap, target: Value, length: u64) -> Completion<()> {
    let name = key(heap, "length");
    let accepted = vm.set_property_key(target, name, Value::Number(length as f64), heap)?;
    match accepted.to_boolean(heap) {
        true => Ok(()),
        false => Err(Abrupt::type_error(
            "the length of what this was collected into could not be set",
        )),
    }
}

/// §23.1.2.3 `Array.of(...items)`.
///
/// The difference from the constructor, and the only reason it exists: `Array(3)` is three holes
/// and `Array.of(3)` is one element. One argument means one element here, whatever it is.
fn of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let length = call.arguments.len() as u64;
    let target = collect_into(vm, heap, call.this_value, &[Value::Number(length as f64)])?;
    for (at, value) in call.arguments.iter().enumerate() {
        // Step 7.c is `CreateDataPropertyOrThrow` and not a plain define: a target that is
        // non-extensible, or that already has a non-configurable property at this index, refuses —
        // and the clause says to stop rather than to carry on writing into something not listening.
        let Value::Object(object) = target else {
            return Err(Abrupt::type_error("a constructor must answer an object"));
        };
        super::array_methods::create_index(heap, object, at as u64, *value)?;
    }
    set_final_length(vm, heap, target, length)?;
    Ok(target)
}

/// Whether a value is something a call may reach — §7.2.3 `IsCallable`.
fn is_callable(heap: &Heap, value: Value) -> bool {
    let Value::Object(object) = value else {
        return false;
    };
    heap.object(object)
        .is_some_and(|found| found.call().is_some())
}
