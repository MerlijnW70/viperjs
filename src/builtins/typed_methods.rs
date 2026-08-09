//! §23.2.3 — what a TypedArray can do, which is nearly what an Array can and never by the same
//! algorithm.
//!
//! # Why these are not `Array.prototype`'s
//!
//! Every one of §23.1.3's methods is *generic*: it reads a `length` property and then `Get`s each
//! index, so it works on anything array-like. §23.2.3's are not. They begin with
//! `ValidateTypedArray`, take the length from the internal slot, and read elements directly — which
//! shows in three places a program can see:
//!
//! - **`length` is not consulted.** Assigning `ta.length = 0` changes nothing, and a generic
//!   algorithm would then iterate nothing.
//! - **The answer is a TypedArray.** `map` and `filter` and `slice` make one of the same kind
//!   through `@@species`, not an Array.
//! - **A detached buffer is a TypeError**, checked at the start, where a generic walk would simply
//!   find every index absent and answer an array of `undefined`.
//!
//! # The two orderings
//!
//! `sort` without a comparator is **numeric** here and lexicographic on `Array.prototype`. That is
//! not a convenience: the elements are numbers and there is no reason to render them as strings
//! first, and a TypedArray sorted as strings would put 10 before 9.
//!
//! # Two of the eleven kinds hold BigInts, and it shows in every method that takes a value
//!
//! §23.2.1's `[[ContentType]]` decides which conversion a write runs — §7.1.13 `ToBigInt` for
//! `BigInt64Array` and `BigUint64Array`, §7.1.4 `ToNumber` for the other nine — and the two refuse
//! each other outright. So `fill`, `with`, `set`, `from`, `of` and `map` all ask the *destination*
//! what it holds rather than asking the value what it is, and `indexOf` finds nothing when handed
//! the other type because §7.2.15 makes values of different types unequal without comparing them.
//!
//! Reading is the mirror of it: an element is a [`Value`] and not an `f64`, because for two kinds
//! it is a BigInt. A walk that assumed otherwise would read a `BigInt64Array` as having no
//! elements at all.

use super::{define_method, key};
use crate::heap::{
    Element, Heap, Iterated, Iteration, Native, NativeCall, Numeric, ObjectId, PropertyKey, View,
};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;
use std::cmp::Ordering;

/// Put §23.2.3's methods on `%TypedArray%.prototype`.
pub(super) fn install(heap: &mut Heap, realm: &Realm, prototype: ObjectId, constructor: ObjectId) {
    for (name, length, native) in [
        ("at", 1, at as Native),
        ("copyWithin", 2, copy_within),
        ("entries", 0, entries),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("toLocaleString", 0, to_locale_string),
        ("keys", 0, keys),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("set", 1, set),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("subarray", 2, subarray),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("toString", 0, to_string),
        ("with", 2, with),
        ("values", 0, values),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §23.2.3.36 — `[@@iterator]` is the *same function object* as `values`, which a program can
    // see. It follows from a TypedArray's iteration being over its elements and nothing else.
    if let Some(symbol) = heap.well_known(super::well_known_at("iterator"))
        && let Some(found) = super::own_value(heap, prototype, "values")
    {
        let _ = heap.define_own_property(
            prototype,
            PropertyKey::from_symbol(symbol),
            &crate::heap::PropertyDescriptor {
                value: Some(found),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
    }
    // §23.2.2 — the two statics every kind inherits, and the species accessor `map` and `filter`
    // and `slice` read to decide what to answer with.
    for (name, length, native) in [("from", 1, from as Native), ("of", 0, of)] {
        define_method(heap, realm, constructor, name, length, native);
    }
    super::buffer::define_species(heap, realm, constructor);
}

/// §23.2.4.1 `ValidateTypedArray` — the view, or the TypeError every method here begins with.
///
/// Two questions in one: is this a TypedArray at all, and are its bytes still there. The second is
/// asked *first* in every method rather than at the first element, so an empty walk over a detached
/// buffer throws rather than quietly doing nothing.
pub(super) fn validate(heap: &Heap, this: Value) -> Completion<(ObjectId, View)> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not a TypedArray"));
    };
    let Some(view) = heap.typed_view(object) else {
        return Err(Abrupt::type_error("this is not a TypedArray"));
    };
    if heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached)
    {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    // §10.4.5.2 — and a window that no longer fits its buffer is refused on the same terms as a
    // detached one. `new Uint8Array(rab, 0, 4)` after `rab.resize(2)` still names four elements
    // and only two of them exist, so walking it would read past the end of the buffer.
    if heap.view_out_of_bounds(object) {
        return Err(Abrupt::type_error(
            "this TypedArray is outside the bounds of its buffer",
        ));
    }
    Ok((object, view))
}

/// The elements of a view, as JavaScript values, in order.
///
/// Taken once because every method that walks them may run a callback, and a callback can detach
/// the buffer — §23.2.3.7 and its neighbours are explicit that the walk carries on with what it
/// had. Reading each element afresh would make a detached buffer turn the rest of the walk into
/// `undefined`s, which is a different answer.
fn elements(heap: &mut Heap, view: View) -> Vec<Value> {
    (0..view.count())
        .filter_map(|at| heap.element_at(view, at))
        .collect()
}

/// The same, for a method that is going to write them straight into another buffer.
///
/// `slice`, `copyWithin`, `reverse` and `set` move elements without a program ever seeing one, so
/// there is nothing for a BigInt element to be allocated *as*. Reading them through [`elements`]
/// would allocate one per element and immediately read it back out.
fn numerics(heap: &Heap, view: View) -> Vec<Numeric> {
    (0..view.count())
        .filter_map(|at| heap.numeric_at(view, at))
        .collect()
}

/// Whether this array's elements are BigInts — §23.2.1's `[[ContentType]]`, asked of an object.
fn holds_big(heap: &Heap, object: ObjectId) -> bool {
    heap.typed_view(object)
        .and_then(|view| view.element)
        .is_some_and(Element::holds_big)
}

/// Write a run of values into an array, converting each by *its* content type — §10.4.5.16.
///
/// The destination decides, which is why this takes the values as [`Value`]s and not as numerics:
/// `map` and `filter` write into whatever `@@species` answered, and a species of a different
/// content type is a TypeError at the write rather than a silent reinterpretation of the bytes.
fn write_all(vm: &mut Vm, heap: &mut Heap, into: ObjectId, values: Vec<Value>) -> Completion<()> {
    for (index, value) in values.into_iter().enumerate() {
        let numeric = vm.to_numeric_of(into, value, heap)?;
        heap.write_element(into, index, &numeric);
    }
    Ok(())
}

/// §7.3.20 `SpeciesConstructor` applied to a TypedArray, and a new one of `count` elements.
fn species_array(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    count: usize,
) -> Completion<Value> {
    let default = kind_constructor(vm, heap, object)?;
    let species = super::promise::species_of(vm, heap, object, default)?;
    let made = vm.construct_value(Value::Object(species), &[Value::Number(count as f64)], heap)?;
    // §23.2.4.2 step 4 — what came back has to be a TypedArray, and a long enough one. A species
    // that answered something else would make every write below go nowhere in silence.
    let Value::Object(id) = made else {
        return Err(Abrupt::type_error("the species did not make a TypedArray"));
    };
    match heap.typed_view(id) {
        Some(view) if view.count() >= count => (),
        _ => return Err(Abrupt::type_error("the species did not make a TypedArray")),
    }
    // §23.2.4.2 step 4 — and of the **same content type** as the array it came from. A species
    // that answered an `Int8Array` for a `BigInt64Array` is refused here rather than at the first
    // write, which is what lets `slice`, `map` and `filter` copy elements across without each of
    // them asking again: the only species they can reach holds what they hold.
    if holds_big(heap, id) != holds_big(heap, object) {
        return Err(Abrupt::type_error(
            "the species made a TypedArray of the other content type",
        ));
    }
    Ok(made)
}

/// §23.2.4.2 step 1 — the **intrinsic** constructor for the kind `object` is.
///
/// The intrinsic and not the object's `constructor` property, which is what this used to read.
/// §7.3.22 consults the property itself at step 2; handing it in as the *default* as well meant a
/// species of `undefined` — step 5's "I have no opinion" — fell back to whatever the property said
/// rather than to the kind. `sample.constructor = {}` then decided what `map` built with, and
/// constructing a plain object is the TypeError 230 of §23.2's tests were reporting.
fn kind_constructor(vm: &mut Vm, heap: &mut Heap, object: ObjectId) -> Completion<ObjectId> {
    let element = heap
        .typed_view(object)
        .and_then(|view| view.element)
        .ok_or_else(|| Abrupt::type_error("this is not a TypedArray"))?;
    let clamped = heap
        .object(object)
        .is_some_and(crate::heap::Object::is_clamped);
    vm.realm()
        .typed_constructor(element, clamped)
        .ok_or_else(|| Abrupt::type_error("this TypedArray has no constructor"))
}

/// §23.2.3.1 — `at`, which counts back from the end for a negative index.
fn at(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, view) = validate(heap, call.this_value)?;
    let count = view.count() as f64;
    let asked = super::string::to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let index = if asked < 0.0 { count + asked } else { asked };
    // `try_from` rather than a comparison against the count: past the end `element_at` already
    // answers nothing, so a bound here would decide nothing — but a *negative* index has to be
    // caught, because `-6.0 as usize` is 0 in Rust and would answer the first element.
    let Ok(index) = usize::try_from(index as i64) else {
        return Ok(Value::Undefined);
    };
    Ok(heap.element_at(view, index).unwrap_or(Value::Undefined))
}

/// §23.2.3.9 — `fill`, which writes one value over a range.
fn fill(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    // §23.2.3.9 step 4 — the *value* is converted before the ends are, which a `valueOf` on any of
    // the three can observe, and by the array's content type: `new BigInt64Array(1).fill(0)` is a
    // TypeError where `.fill(0n)` fills.
    let value = vm.to_numeric_of(object, call.argument(0), heap)?;
    let from = relative(vm, heap, call.argument(1), count, 0.0)?;
    let to = relative(vm, heap, call.argument(2), count, count as f64)?;
    // Step 10 — asked **again**, because all three conversions above can run a `valueOf` and a
    // `valueOf` is a program: it can transfer the buffer out from under the fill that is still
    // reading its own arguments. Without this the writes are simply discarded and the program is
    // never told its data went away.
    validate(heap, call.this_value)?;
    for index in from..to {
        heap.write_element(object, index, &value);
    }
    Ok(call.this_value)
}

/// §23.2.3.26 — `slice`, which copies into a **new** TypedArray of the species' kind.
fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    let from = relative(vm, heap, call.argument(0), count, 0.0)?;
    let to = relative(vm, heap, call.argument(1), count, count as f64)?;
    let taken = to.saturating_sub(from);
    let made = species_array(vm, heap, object, taken)?;
    // §23.2.3.27 step 10.a — **after** the species has been made, and only when there is anything
    // to copy. Both halves are observable: `SpeciesConstructor` reads `constructor` and then
    // `@@species` off the receiver, so a getter on either can detach the buffer this is about to
    // read, and a `slice()` of nothing must still answer the empty array it just made rather than
    // throw. Read before the species instead, the copy would quietly hold what a detached buffer
    // used to say.
    if taken > 0 {
        validate(heap, call.this_value)?;
    }
    let values: Vec<Numeric> = (from..from + taken)
        .filter_map(|index| heap.numeric_at(view, index))
        .collect();
    if let Value::Object(id) = made {
        // §23.2.3.26 step 14 — the copy is `SetValueInBuffer` between two arrays of the *same*
        // element kind, so the numerics go straight across with no conversion. A species of a
        // different kind gets each value written through §10.4.5.16 instead, which is why
        // `write_element` is still the way in and not a byte copy.
        for (index, value) in values.into_iter().enumerate() {
            heap.write_element(id, index, &value);
        }
    }
    Ok(made)
}

/// §23.2.4.3 `TypedArrayCreateSameType` — a new array of the *same kind*, this many elements long.
///
/// The **intrinsic** constructor for that kind, and deliberately not `@@species`. §23.2.3.32,
/// §23.2.3.33 and §23.2.3.36 make their copy this way, so a subclass of `Uint8Array` gets a plain
/// `Uint8Array` back — while `map`, `filter` and `slice` sitting beside them *do* consult species
/// and answer the subclass. Two spellings for the same-looking thing, and the difference is
/// visible from one receiver.
fn same_kind(vm: &mut Vm, heap: &mut Heap, object: ObjectId, count: usize) -> Completion<ObjectId> {
    // Asked of the realm rather than looked up on the global by name. "Intrinsic" is the whole
    // claim this function makes, and `globalThis.Uint8Array = something` is a line a script may
    // write — after which a name lookup answers the something and `toSorted` on a `Uint8Array`
    // builds one of those. The realm took the nine before any script ran.
    let constructor = kind_constructor(vm, heap, object)?;
    let made = vm.construct_value(
        Value::Object(constructor),
        &[Value::Number(count as f64)],
        heap,
    )?;
    match made {
        Value::Object(id) => Ok(id),
        _ => Err(Abrupt::type_error(
            "that constructor did not make a TypedArray",
        )),
    }
}

/// §23.2.3.36 — `with`, one index replaced and everything else copied.
fn with(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    let asked = super::string::to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let index = if asked < 0.0 {
        count as f64 + asked
    } else {
        asked
    };
    // Step 6 before step 7 — the *value* is converted before the index is judged, so a `valueOf`
    // that throws is what a program sees even when the index was out of range as well. And by this
    // array's content type, which is step 5's `If O.[[ContentType]] is bigint`.
    let replacement = vm.to_numeric_of(object, call.argument(1), heap)?;
    let Ok(index) = usize::try_from(index as i64) else {
        return Err(Abrupt::range_error("that index is not in the TypedArray"));
    };
    if index >= count {
        return Err(Abrupt::range_error("that index is not in the TypedArray"));
    }
    let values = numerics(heap, view);
    let made = same_kind(vm, heap, object, count)?;
    for (at, value) in values.into_iter().enumerate() {
        let held = if at == index { &replacement } else { &value };
        heap.write_element(made, at, held);
    }
    Ok(Value::Object(made))
}

/// §23.2.3.32 — `toReversed`, which leaves the array it was given alone.
fn to_reversed(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    let values = numerics(heap, view);
    let made = same_kind(vm, heap, object, count)?;
    for (at, value) in values.into_iter().rev().enumerate() {
        heap.write_element(made, at, &value);
    }
    Ok(Value::Object(made))
}

/// §23.2.3.33 — `toSorted`, which is `sort` without the mutation.
fn to_sorted(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 comes **before** `ValidateTypedArray`, so a bad comparator is reported as one even
    // when `this` is not a TypedArray at all.
    let comparator = call.argument(0);
    if !matches!(comparator, Value::Undefined) && !heap.is_callable(comparator) {
        return Err(Abrupt::type_error("the comparator is not a function"));
    }
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    let sorted = ordered(vm, heap, view, comparator)?;
    let made = same_kind(vm, heap, object, count)?;
    for (at, value) in sorted.into_iter().enumerate() {
        heap.write_element(made, at, &value);
    }
    Ok(Value::Object(made))
}

/// §23.2.3.30 — `subarray`, which makes another **window onto the same buffer**.
///
/// The one method here that does not copy, and the difference from `slice` is the whole reason both
/// exist: writing through what `subarray` answered is visible through the original.
fn subarray(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.this_value else {
        return Err(Abrupt::type_error("this is not a TypedArray"));
    };
    let Some(view) = heap.typed_view(object) else {
        return Err(Abrupt::type_error("this is not a TypedArray"));
    };
    // Not `validate`: §23.2.3.30 does *not* check for a detached buffer, because it makes no
    // element access at all. A subarray of a detached array is an empty one.
    let count = view.count();
    let from = relative(vm, heap, call.argument(0), count, 0.0)?;
    let to = relative(vm, heap, call.argument(1), count, count as f64)?;
    let taken = to.saturating_sub(from);
    let width = view.element.map_or(1, crate::heap::Element::width);
    let default = kind_constructor(vm, heap, object)?;
    let species = super::promise::species_of(vm, heap, object, default)?;
    // §23.2.3.30 step 16 — a subarray of a **length-tracking** array with no explicit end is itself
    // length-tracking, and the way that is said is by constructing with *two* arguments instead of
    // three. A third argument, even the right number, would pin the window: the new array would
    // stop following the buffer the moment it was resized. The species can see the difference —
    // these tests count the arguments it was called with.
    let mut arguments = vec![
        Value::Object(view.buffer),
        Value::Number((view.offset + from * width) as f64),
    ];
    if !(view.tracking && matches!(call.argument(1), Value::Undefined)) {
        arguments.push(Value::Number(taken as f64));
    }
    let made = vm.construct_value(Value::Object(species), &arguments, heap)?;
    // §23.2.4.3 step 4 — a species may answer anything at all, and what it answered has to be a
    // TypedArray. One that is not would be handed back as though it were, and every later use of
    // it would fail somewhere else entirely.
    match made {
        Value::Object(id) if heap.typed_view(id).is_some() => Ok(made),
        _ => Err(Abrupt::type_error("the species did not make a TypedArray")),
    }
}

/// §23.2.3.24 — `set`, which copies a source over this array starting at an offset.
fn set(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Steps 1 to 5 — the brand check, which is what makes this a method of a TypedArray rather
    // than of anything with indices. Its `View` is deliberately discarded: it describes the buffer
    // as it stands *now*, and step 6 is about to run a program.
    let (object, _) = validate(heap, call.this_value)?;
    // Step 6 — `ToIntegerOrInfinity`, and it can call a `valueOf`. That method may detach the
    // buffer, resize it, or both, which is why every fact taken before this line is stale after it.
    let offset = super::string::to_integer_or_infinity(vm.to_number(call.argument(1), heap)?);
    // Step 7, before the buffer is looked at again: a negative offset is a RangeError whatever
    // happened to the buffer meanwhile.
    if offset < 0.0 {
        return Err(Abrupt::range_error("the offset of set may not be negative"));
    }
    // Steps 8 and 9 — the buffer is read **again** and a detached one is a TypeError. Saturating
    // rather than casting: `ToIntegerOrInfinity` may answer infinity, and an offset that becomes
    // `usize::MAX` must fail the length check below rather than wrap around it.
    let offset = if offset.is_finite() {
        offset as usize
    } else {
        usize::MAX
    };
    let (_, view) = validate(heap, call.this_value)?;
    let source = call.argument(0);
    // Every element is read *before* any is written, which matters when the two overlap: a source
    // that is a view onto the same buffer would otherwise be read through what had just been
    // written over it.
    let values: Vec<Numeric> = match source {
        Value::Object(id) if heap.typed_view(id).is_some() => {
            // §23.2.3.24.1 step 5 — a source of the other content type is a **TypeError**, and it
            // is the only check standing between the two: elements move across here without any
            // conversion, so eight bytes of BigInt would otherwise land in a `Float64Array` as a
            // double made of the same bits.
            if holds_big(heap, id) != holds_big(heap, object) {
                return Err(Abrupt::type_error(
                    "a BigInt TypedArray and a Number one cannot be copied into each other",
                ));
            }
            // §23.2.3.26.1 step 4 — the *source* is asked the same question, and for the same
            // reason: the `valueOf` above could as easily have detached its buffer as the target's.
            let (_, other) = validate(heap, source)?;
            numerics(heap, other)
        }
        _ => {
            // §23.2.3.26.2 `SetTypedArrayFromArrayLike` — an **array-like**, and nothing else. This
            // read the iterable protocol first and fell back, which is not a clause `set` has at
            // all: steps 3 to 5 are `ToObject`, `LengthOfArrayLike` and a loop of `Get`, and there
            // is no `GetMethod(@@iterator)` anywhere in it. So `ta.set(src)` where `src` had both a
            // `length` and an `@@iterator` wrote what the *iterator* answered — a wrong value, and
            // one no error could have warned about.
            //
            // Each value is converted by *this* array's content type, so `bigOnes.set([1])` throws
            // where `bigOnes.set([1n])` writes. An Array has both readings and they agree, which is
            // why every ordinary use of this looked right.
            let taken = array_like(vm, heap, source)?;
            let holds_big = holds_big(heap, object);
            let mut numbers = Vec::with_capacity(taken.len());
            for value in taken {
                numbers.push(vm.to_numeric(holds_big, value, heap)?);
            }
            numbers
        }
    };
    if offset.saturating_add(values.len()) > view.count() {
        return Err(Abrupt::range_error(
            "this source is too long for this TypedArray",
        ));
    }
    for (index, value) in values.into_iter().enumerate() {
        heap.write_element(object, offset + index, &value);
    }
    Ok(Value::Undefined)
}

/// An object with a `length` and no iterator — §7.3.19.
fn array_like(vm: &mut Vm, heap: &mut Heap, source: Value) -> Completion<Vec<Value>> {
    let Value::Object(object) = source else {
        return Err(Abrupt::type_error("this is not something to copy from"));
    };
    let name = key(heap, "length");
    let length = vm.get_property_key(Value::Object(object), name, heap)?;
    let count = super::array_methods::to_length(vm.to_number(length, heap)?);
    let mut taken = Vec::new();
    for index in 0..count {
        let at = super::array_methods::index_key(heap, index);
        taken.push(vm.get_property_key(Value::Object(object), at, heap)?);
        super::array_methods::within_budget(heap)?;
    }
    Ok(taken)
}

/// §23.2.3.6 — `copyWithin`, which moves a run of elements inside one array.
fn copy_within(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let count = view.count();
    let target = relative(vm, heap, call.argument(0), count, 0.0)?;
    let from = relative(vm, heap, call.argument(1), count, 0.0)?;
    let to = relative(vm, heap, call.argument(2), count, count as f64)?;
    // §23.2.3.6 step 14.b and 14.c — the three coercions above may have detached the buffer or
    // shrunk it under this call, so `ValidateTypedArray` is run **again** and answers the same
    // TypeError it would have at the top. `fill` and `slice` throw here too; `indexOf` and its
    // neighbours do not and answer `-1` instead, and that per-method difference is the whole cost
    // of this area.
    let (_, view) = validate(heap, call.this_value)?;
    // Read first, then write — the two runs may overlap, and copying element by element would read
    // through what it had already written.
    let taken: Vec<Numeric> = (from..to)
        .filter_map(|index| heap.numeric_at(view, index))
        .take(count.saturating_sub(target))
        .collect();
    for (index, value) in taken.into_iter().enumerate() {
        heap.write_element(object, target + index, &value);
    }
    Ok(call.this_value)
}

/// §23.2.3.23 — `reverse`, in place.
fn reverse(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let mut values = numerics(heap, view);
    values.reverse();
    for (index, value) in values.into_iter().enumerate() {
        heap.write_element(object, index, &value);
    }
    Ok(call.this_value)
}

/// §23.2.3.29 — `sort`, whose default order is **numeric**.
///
/// Where `Array.prototype.sort` renders each element as a String first, this compares the numbers:
/// the elements *are* numbers and there is nothing to render. Sorting them as strings would put 10
/// before 9, which is right for an Array of anything and wrong for an array of numbers.
fn sort(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let comparator = call.argument(0);
    if !matches!(comparator, Value::Undefined) && !heap.is_callable(comparator) {
        return Err(Abrupt::type_error("the comparator is not a function"));
    }
    let (object, view) = validate(heap, call.this_value)?;
    let values = ordered(vm, heap, view, comparator)?;
    for (index, value) in values.into_iter().enumerate() {
        heap.write_element(object, index, &value);
    }
    Ok(call.this_value)
}

/// A view's elements in §23.2.3.29's order — the body `sort` and `toSorted` share.
///
/// Two different sorts, and which one runs is decided before anything is read: the default compares
/// the elements themselves, and a comparator is a program and has to be handed JavaScript values.
/// Both sort **outside the heap** and are written back afterwards, because a comparator that
/// detached the buffer would otherwise be writing into bytes that had gone.
fn ordered(
    vm: &mut Vm,
    heap: &mut Heap,
    view: View,
    comparator: Value,
) -> Completion<Vec<Numeric>> {
    if matches!(comparator, Value::Undefined) {
        let mut values = numerics(heap, view);
        values.sort_by(default_order);
        return Ok(values);
    }
    let taken = elements(heap, view);
    let values = sorted_by(vm, heap, taken, comparator)?;
    Ok(values
        .into_iter()
        .filter_map(|value| heap.as_numeric(value))
        .collect())
}

/// §23.2.3.29 `CompareTypedArrayElements` with no comparator — ascending, and total.
///
/// For Numbers: `NaN` last and `-0` before `+0`. `sort_by` with a partial comparison cannot say
/// either, so the key does it — every `NaN` sorts above every number, and `total_cmp` separates the
/// zeroes by sign. For BigInts neither question arises: there is no BigInt `NaN` and no negative
/// zero, so it is the plain comparison.
fn default_order(left: &Numeric, right: &Numeric) -> Ordering {
    match (left, right) {
        (Numeric::Number(left), Numeric::Number(right)) => {
            numeric_order(*left).total_cmp(&numeric_order(*right))
        }
        (Numeric::BigInt(left), Numeric::BigInt(right)) => left.compare(right),
        // A Number against a BigInt, which no array can present: every element of one array is of
        // one kind, so the two here are always the same variant. Called equal rather than given an
        // order, because there is no order between the two types to give.
        _ => Ordering::Equal,
    }
}

/// The key that puts `NaN` last and `-0` before `+0` — §23.2.3.29's ordering, as one number.
fn numeric_order(value: f64) -> f64 {
    match value {
        found if found.is_nan() => f64::INFINITY,
        // `total_cmp` already separates the zeroes by sign and in the right direction, so nothing
        // else is needed: this is only about lifting `NaN` above every number rather than leaving
        // it wherever an unordered comparison put it.
        found => found,
    }
}

/// An insertion sort driven by a program's comparator.
///
/// Insertion rather than anything cleverer because the comparator may lie — return a different
/// answer for the same pair each time — and a sort that assumed consistency could then read outside
/// its own slice. §23.2.3.29 requires only that the result be *some* permutation when that happens,
/// and this gives one for any comparator at all.
fn sorted_by(
    vm: &mut Vm,
    heap: &mut Heap,
    values: Vec<Value>,
    comparator: Value,
) -> Completion<Vec<Value>> {
    let mut sorted: Vec<Value> = Vec::with_capacity(values.len());
    for value in values {
        let mut at = 0;
        while at < sorted.len() {
            let answer = vm.call_value(comparator, Value::Undefined, &[sorted[at], value], heap)?;
            let order = vm.to_number(answer, heap)?;
            // §23.2.3.29 step 4 — `NaN` from a comparator is treated as 0, which is "these are
            // equal", so a comparator that answers nonsense still terminates.
            if order.is_nan() || order > 0.0 {
                break;
            }
            at += 1;
        }
        sorted.insert(at, value);
        super::array_methods::within_budget(heap)?;
    }
    Ok(sorted)
}

/// §23.2.3.29 `%TypedArray%.prototype.toLocaleString`.
///
/// §23.1.3.32's body with `ValidateTypedArray` in front of it, and that check is the whole reason
/// this exists as a separate function. The Array method opens with `ToObject`, which happily
/// answers about a plain object — so without one of its own, `%TypedArray%.prototype` would
/// inherit `Object.prototype`'s and a TypedArray method would work on things that are not
/// TypedArrays. That is exactly what happened when `Object.prototype.toLocaleString` arrived and
/// four tests stopped throwing.
fn to_locale_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    validate(heap, call.this_value)?;
    super::array_flat::to_locale_string(vm, heap, call)
}

/// §23.2.3.16 — `join`, whose elements become text by §7.1.17 and not by `number_to_string`.
///
/// The difference shows on two of the eleven kinds: `String(1n)` is `"1"` and `ToString` of a
/// BigInt is §21.2.3.3's digits, where a Number's rendering knows nothing about them.
/// # The length is taken before the separator and the elements after it
///
/// Steps 3 and 5 in that order, and the gap between them is where a program runs: `ToString` of the
/// separator may call a `toString` that resizes or detaches the buffer. The count stays what it was
/// — step 3 already read it — and the elements are read *afterwards*, so an index that no longer
/// exists contributes the empty string rather than being dropped.
///
/// That is what makes `new Int8Array(rab3).join(shrinksToZero)` answer `"--"`: three elements'
/// worth of separators with nothing between them. Reading the elements from the view captured
/// before the conversion answered `""` instead, and dropping them from the count answered `""` too
/// — two ways to lose the same thing.
fn join(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    // Step 3, before anything a program controls can run.
    let count = view.count();
    let separator = match call.argument(0) {
        Value::Undefined => ",".to_string(),
        given => {
            // Through the machine: §23.2.3.16 step 5 is `ToString`, and a separator is the one
            // argument here a program controls — `join({ toString() { … } })` refused before this.
            // The element loop below is the opposite case and stays heap-only, deliberately.
            let id = vm.to_string(given, heap)?;
            let units = heap.string(id).unwrap_or(&[]).to_vec();
            char::decode_utf16(units)
                .map(|found| found.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect()
        }
    };
    // Read again: the conversion may have left a different buffer behind the same object, and the
    // window captured above describes one that is gone.
    let now = heap
        .typed_view(object)
        .filter(|_| !heap.view_out_of_bounds(object));
    let mut joined: Vec<String> = Vec::with_capacity(count);
    for at in 0..count {
        // §23.2.3.16 step 6.b — an element that is no longer there is `undefined`, and step 6.c
        // makes `undefined` the empty string rather than the word.
        let Some(value) = now.and_then(|view| heap.element_at(view, at)) else {
            joined.push(String::new());
            continue;
        };
        // Neither a Number nor a BigInt can throw here, and neither can call a program: `?` is the
        // shape `to_string` has for the types that can, not a case this one reaches.
        let id = value.to_string(heap)?;
        joined.push(String::from_utf16_lossy(heap.string(id).unwrap_or(&[])));
    }
    Ok(super::text(heap, &joined.join(&separator)))
}

/// §23.2.3.31 — `toString`, which is `join` with the default separator and nothing else.
fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let name = key(heap, "join");
    let found = vm.get_property_key(call.this_value, name, heap)?;
    // Through the *property*, because §23.2.3.31 is `Array.prototype.toString` and that one calls
    // whatever `join` currently is — so replacing `join` changes what `toString` answers.
    if !heap.is_callable(found) {
        return Err(Abrupt::type_error("join is not a function"));
    }
    vm.call_value(found, call.this_value, &[], heap)
}

/// §23.2.3.14 — `indexOf`.
fn index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, Search::First)
}

/// §23.2.3.18 — `lastIndexOf`.
fn last_index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, Search::Last)
}

/// §23.2.3.13 — `includes`, which differs from `indexOf` in finding `NaN`.
fn includes(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    search(vm, heap, call, Search::Includes)
}

/// Which of the three searches this is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Search {
    /// `indexOf` — strict equality, forwards.
    First,
    /// `lastIndexOf` — strict equality, backwards.
    Last,
    /// `includes` — `SameValueZero`, forwards, so it finds `NaN`.
    Includes,
}

/// The body all three share.
fn search(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, how: Search) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    // §23.2.3.15 step 3 — `len` is taken *before* anything is coerced, and the negative-index
    // arithmetic below is against this number even if the buffer shrinks under it.
    let count = view.count();
    // §7.2.15 step 1 — values of different **types** are unequal without being compared, so
    // anything that is not a numeric at all can match no element: `ta.indexOf("1")` is -1, and so
    // is `bigOnes.indexOf(1)`. Taken as a numeric once rather than asked per element.
    let wanted = heap.as_numeric(call.argument(0));
    let from = match call.arguments.len() {
        0..=1 => None,
        _ => Some(super::string::to_integer_or_infinity(
            vm.to_number(call.argument(1), heap)?,
        )),
    };
    // §23.2.3.13 and §23.2.3.14 differ in exactly one thing and it is not the direction: `includes`
    // uses `SameValueZero`, which finds `NaN`, and the other two use strict equality, which cannot.
    // That distinction is about Numbers only — there is no BigInt `NaN` for either to disagree on.
    let matches = |found: &Numeric| match (&wanted, found) {
        (Some(Numeric::Number(number)), Numeric::Number(found)) => {
            (how == Search::Includes && found.is_nan() && number.is_nan()) || found == number
        }
        (Some(Numeric::BigInt(number)), Numeric::BigInt(found)) => found == number,
        _ => false,
    };
    // Step 11 reads each element **live**, after the coercion rather than before it. A buffer the
    // coercion detached answers `undefined` for every index, and `undefined` matches no numeric —
    // so the answer is `-1` or `false` rather than an error, and rather than the elements that were
    // there a moment ago. Snapshotting before the coercion searched a buffer that no longer exists.
    let values = match heap.typed_view(object) {
        Some(live) => numerics(heap, live),
        None => Vec::new(),
    };
    let start = |default: f64| -> f64 {
        let given = from.unwrap_or(default);
        if given < 0.0 {
            count as f64 + given
        } else {
            given
        }
    };
    let answer = match how {
        Search::Last => {
            // The last index, named once. Written out twice — once as the default and once as the
            // clamp — a change to either was hidden by the `min` of the two.
            let last = (count as f64) - 1.0;
            let end = start(last).min(last);
            (0..=end.max(-1.0) as isize).rev().find(|index| {
                // `count` is step 3's `len` and `values` is what is there *now*, so the second may
                // be shorter — a coercion that shrank the buffer leaves indices this loop still
                // visits and the elements no longer has. `get` is the whole of what step 11's
                // "read it and compare" means when the read finds nothing: no match, no error.
                *index >= 0
                    && usize::try_from(*index)
                        .ok()
                        .and_then(|at| values.get(at))
                        .is_some_and(matches)
            })
        }
        _ => {
            let begin = start(0.0).max(0.0) as usize;
            (begin..count)
                .find(|index| values.get(*index).is_some_and(matches))
                .map(|index| index as isize)
        }
    };
    Ok(match how {
        Search::Includes => Value::Boolean(answer.is_some()),
        _ => Value::Number(answer.map_or(-1.0, |index| index as f64)),
    })
}

/// §7.1.5 with a relative index — negative counts back from the end, and both ends clamp.
fn relative(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
    count: usize,
    default: f64,
) -> Completion<usize> {
    let number = match value {
        Value::Undefined => default,
        other => super::string::to_integer_or_infinity(vm.to_number(other, heap)?),
    };
    let at = if number < 0.0 {
        (count as f64 + number).max(0.0)
    } else {
        number.min(count as f64)
    };
    Ok(at as usize)
}

/// §23.2.3.20 — `keys`.
fn keys(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Keys)
}

/// §23.2.3.35 — `values`, which is also `[@@iterator]`.
fn values(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Values)
}

/// §23.2.3.8 — `entries`.
fn entries(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    iterator(vm, heap, call, Iterated::Entries)
}

/// An Array Iterator over this array — the same one §23.1.5 makes, because a TypedArray is
/// array-like and the iterator reads it by index like any other.
fn iterator(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: Iterated,
) -> Completion<Value> {
    validate(heap, call.this_value)?;
    let made = heap.new_iterator(
        vm.realm().array_iterator_prototype(),
        Iteration {
            over: call.this_value,
            at: 0,
            kind,
            done: false,
        },
    );
    Ok(Value::Object(made))
}

/// What a callback-driven walk is looking for — §23.2.3.7 and its eight neighbours.
///
/// All nine read the elements once, call the same callback with the same three arguments, and
/// differ only in what they do with the answers. Written as one walk with this saying which, rather
/// than nine walks that would drift apart at the first bug fixed in one of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// `every` — false at the first falsy answer.
    Every,
    /// `some` — true at the first truthy one.
    Any,
    /// `forEach` — nothing at all.
    Each,
    /// `map` — a new array of the answers.
    Map,
    /// `filter` — a new array of the elements whose answer was truthy.
    Filter,
    /// `find` — the first element whose answer was truthy.
    Find,
    /// `findIndex` — its index.
    FindIndex,
    /// `findLast` — the last, walking backwards.
    FindLast,
    /// `findLastIndex` — its index.
    FindLastIndex,
}

/// §23.2.3.7 — `every`.
fn every(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Every)
}

/// §23.2.3.28 — `some`.
fn some(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Any)
}

/// §23.2.3.12 — `forEach`.
fn for_each(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Each)
}

/// §23.2.3.21 — `map`, which answers a TypedArray of the species' kind rather than an Array.
fn map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Map)
}

/// §23.2.3.10 — `filter`, likewise.
fn filter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Filter)
}

/// §23.2.3.10 — `find`.
fn find(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::Find)
}

/// §23.2.3.11 — `findIndex`.
fn find_index(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::FindIndex)
}

/// §23.2.3.10 — `findLast`, which walks backwards.
fn find_last(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::FindLast)
}

/// §23.2.3.11 — `findLastIndex`.
fn find_last_index(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    walk(vm, heap, call, Walk::FindLastIndex)
}

/// The walk all nine share.
fn walk(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, how: Walk) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let callback = call.argument(0);
    if !heap.is_callable(callback) {
        return Err(Abrupt::type_error("the callback is not a function"));
    }
    let receiver = call.argument(1);
    // §23.2.3.7 step 3 caches the **length** and step 6.b re-reads each *element* with `Get(O, Pk)`
    // — the same two decisions `fold` below spells out, and they are not one. So a callback that
    // shrinks a resizable buffer still gets the number of turns the array had when the walk
    // started, and the turns past the new end are handed `undefined`.
    //
    // A snapshot of the elements was taken here instead, above a comment saying §23.2.3.7 "carries
    // on with what it had rather than turning the rest of the walk into `undefined`s" — which is
    // what a snapshot does and not what the clause says. It handed back what used to be there.
    let count = view.count();
    let backwards = matches!(how, Walk::FindLast | Walk::FindLastIndex);
    let order: Vec<usize> = match backwards {
        true => (0..count).rev().collect(),
        false => (0..count).collect(),
    };
    let mut kept: Vec<Value> = Vec::new();
    for index in order {
        let element = element_now(heap, object, index);
        let answer = vm.call_value(
            callback,
            receiver,
            &[element, Value::Number(index as f64), call.this_value],
            heap,
        )?;
        let truthy = answer.to_boolean(heap);
        match how {
            Walk::Every if !truthy => return Ok(Value::Boolean(false)),
            Walk::Any if truthy => return Ok(Value::Boolean(true)),
            Walk::Find | Walk::FindLast if truthy => return Ok(element),
            Walk::FindIndex | Walk::FindLastIndex if truthy => {
                return Ok(Value::Number(index as f64));
            }
            // §23.2.3.21 step 6.c — the *answer* is kept, in order, and the array to put them in
            // is made afterwards: `filter` knows its length only when the walk is over. Kept as a
            // value rather than converted here, because §10.4.5.16 asks the **destination** which
            // conversion to run and the destination does not exist yet.
            Walk::Map => kept.push(answer),
            // §23.2.3.10 step 6.d keeps `kValue` — the value this turn was *given*, not a second
            // read, which after a shrink would differ from what the callback saw.
            Walk::Filter if truthy => kept.push(element),
            _ => {}
        }
        super::array_methods::within_budget(heap)?;
    }
    match how {
        Walk::Every => Ok(Value::Boolean(true)),
        Walk::Any => Ok(Value::Boolean(false)),
        Walk::Find | Walk::FindLast => Ok(Value::Undefined),
        Walk::FindIndex | Walk::FindLastIndex => Ok(Value::Number(-1.0)),
        Walk::Each => Ok(Value::Undefined),
        Walk::Map | Walk::Filter => {
            let made = species_array(vm, heap, object, kept.len())?;
            if let Value::Object(id) = made {
                write_all(vm, heap, id, kept)?;
            }
            Ok(made)
        }
    }
}

/// §23.2.3.22 — `reduce`.
fn reduce(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    fold(vm, heap, call, false)
}

/// §23.2.3.23 — `reduceRight`.
fn reduce_right(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    fold(vm, heap, call, true)
}

/// Both folds, which differ in direction and in nothing else.
fn fold(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, backwards: bool) -> Completion<Value> {
    let (object, view) = validate(heap, call.this_value)?;
    let callback = call.argument(0);
    if !heap.is_callable(callback) {
        return Err(Abrupt::type_error("the callback is not a function"));
    }
    // §23.2.3.22 step 3 caches the **length** and step 8 re-reads each *element*, and the two are
    // not the same decision. A callback that shrinks a resizable buffer therefore still gets the
    // number of turns the array had when the fold started, and the turns past the new end are
    // handed `undefined` — which a snapshot of the elements taken up front cannot express, because
    // it would hand back what used to be there.
    let count = view.count();
    let order: Vec<usize> = match backwards {
        true => (0..count).rev().collect(),
        false => (0..count).collect(),
    };
    let mut steps = order.into_iter();
    // §23.2.3.22 step 5 — with no initial value the *first element* is one, and an empty array with
    // no initial value is a TypeError rather than `undefined`: there is no answer to give.
    let mut total = match call.arguments.len() {
        0..=1 => match steps.next() {
            Some(index) => element_now(heap, object, index),
            None => {
                return Err(Abrupt::type_error(
                    "reduce of an empty TypedArray with no initial value",
                ));
            }
        },
        _ => call.argument(1),
    };
    for index in steps {
        let current = element_now(heap, object, index);
        total = vm.call_value(
            callback,
            Value::Undefined,
            &[total, current, Value::Number(index as f64), call.this_value],
            heap,
        )?;
        super::array_methods::within_budget(heap)?;
    }
    Ok(total)
}

/// The element at `index` **as the array is now** — §23.2.3.22 step 8's `Get(O, Pk)`.
///
/// The view is fetched afresh rather than passed in, because a length-tracking one is a different
/// window after every resize and a caller holding the old one would read past the end of the
/// buffer. `undefined` for an index the array no longer has, which is what `Get` answers for an
/// out-of-range canonical numeric index and is the whole observable effect of shrinking mid-walk.
fn element_now(heap: &mut Heap, object: ObjectId, index: usize) -> Value {
    let Some(view) = heap.typed_view(object) else {
        return Value::Undefined;
    };
    heap.element_at(view, index).unwrap_or(Value::Undefined)
}

/// §23.2.2.1 — `%TypedArray%.from`, which takes an iterable or an array-like and a mapper.
fn from(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(constructor) = call.this_value else {
        return Err(Abrupt::type_error("from must be called on a constructor"));
    };
    let mapper = call.argument(1);
    if !matches!(mapper, Value::Undefined) && !heap.is_callable(mapper) {
        return Err(Abrupt::type_error("the mapper is not a function"));
    }
    let source = call.argument(0);
    // §23.2.2.1 steps 4 and 5 — `GetMethod(source, @@iterator)`, and the branch is on whether there
    // **is** one. The same clause the constructor has, and this had the same fallback-on-failure:
    // every error the walk raised was caught and answered with an array-like reading instead, so a
    // `@@iterator` getter that threw built an array out of `length` and discarded the throw.
    let taken = match super::array::iterator_method_of(vm, heap, source)? {
        Some(method) => super::promise_group::iterable_to_list_with(vm, heap, source, method)?,
        None => array_like(vm, heap, source)?,
    };
    let made = vm.construct_value(
        Value::Object(constructor),
        &[Value::Number(taken.len() as f64)],
        heap,
    )?;
    let id = made_typed_array(heap, made, taken.len())?;
    // §23.2.2.1 step 8.d — each answer goes in through `Set`, so the conversion is §10.4.5.16's and
    // is chosen by the array the constructor made: `BigInt64Array.from([1])` is a TypeError and
    // `BigInt64Array.from([1n])` is not. The mapper runs first, and its answer is what is converted.
    for (index, value) in taken.into_iter().enumerate() {
        let mapped = match matches!(mapper, Value::Undefined) {
            true => value,
            false => vm.call_value(
                mapper,
                Value::Undefined,
                &[value, Value::Number(index as f64)],
                heap,
            )?,
        };
        let numeric = vm.to_numeric_of(id, mapped, heap)?;
        heap.write_element(id, index, &numeric);
    }
    Ok(made)
}

/// §23.2.2.2 — `%TypedArray%.of`, which is `from` for arguments already in hand.
fn of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(constructor) = call.this_value else {
        return Err(Abrupt::type_error("of must be called on a constructor"));
    };
    let made = vm.construct_value(
        Value::Object(constructor),
        &[Value::Number(call.arguments.len() as f64)],
        heap,
    )?;
    let id = made_typed_array(heap, made, call.arguments.len())?;
    write_all(vm, heap, id, call.arguments.to_vec())?;
    Ok(made)
}

/// §23.2.4.4 `TypedArrayCreateFromConstructor` step 3 — what a constructor answered, if it is one.
///
/// `from` and `of` call a constructor a program named, and it may answer anything: a plain object,
/// a number, a `DataView`. Checking that it is a TypedArray is what stops the writes below going
/// nowhere in silence and the caller receiving something that is not what it asked for.
fn made_typed_array(heap: &Heap, made: Value, count: usize) -> Completion<ObjectId> {
    match made {
        // Step 4 — and long enough. A constructor called with a length may answer a *shorter*
        // array, and the elements would then be written into indices it does not have, which
        // §10.4.5.5 discards in silence: the caller would receive a short array and no complaint.
        Value::Object(id)
            if heap
                .typed_view(id)
                .is_some_and(|view| view.count() >= count) =>
        {
            Ok(id)
        }
        _ => Err(Abrupt::type_error(
            "this constructor did not make a long enough TypedArray",
        )),
    }
}
