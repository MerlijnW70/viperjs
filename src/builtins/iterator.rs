//! §27.1, §23.1.5 and §22.1.5 — the iterator objects, and the prototype chain they share.
//!
//! # Three prototypes and why they are three
//!
//! %IteratorPrototype% has one method: `[@@iterator]() { return this }`. That is the whole of what
//! makes an iterator *iterable*, and it is why `for (const x of someIterator)` works — the loop
//! asks for an iterator and the iterator answers itself.
//!
//! %ArrayIteratorPrototype% and %StringIteratorPrototype% inherit from it and add a `next`. They
//! are separate objects rather than one because a script may replace either `next` without
//! touching the other, and because `@@toStringTag` names them differently. Nothing else about them
//! differs, which is why one private `step` serves both.
//!
//! # What `next` may be given
//!
//! Anything, and it refuses everything that is not one of its own iterators — §23.1.5.2.1 step 2
//! is a `RequireInternalSlot`. So `Array.prototype.values.call([]).next.call({})` is a TypeError
//! rather than an answer about an object that merely looks similar. The position an iterator keeps
//! is a slot for the same reason; see [`crate::heap::Iteration`].

use super::{define_method, key};
use crate::heap::{Heap, Iterated, Iteration, NativeCall, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;
/// §7.4.2's Iterator Record, as much of it as a built-in walk needs.
///
/// Held rather than drained, so a walk can stop as soon as it knows the answer and tell the
/// iterator it did — §24.2.4's `isDisjointFrom` and §27.1.4's `some` both need that, and collecting
/// the values into a list first would call `next` more times than either specifies and would hang
/// on an iterator that never ends.
///
/// `next` is read **once** and kept, which is what §7.4.3 does: replacing it part-way through a
/// walk does not change the walk.
pub(crate) struct Walk {
    /// The iterator object, which is also the receiver `next` is called on.
    iterator: Value,
    /// Its `next` method, read once.
    next: Value,
}

impl Walk {
    /// §7.4.9 `IteratorClose` on an iterator whose `next` has **not** been read.
    ///
    /// §27.1.4's methods all check their argument *before* `GetIteratorDirect`, and close the
    /// iterator when it is wrong — so the close happens with an Iterator Record whose
    /// `[[NextMethod]]` is still undefined, and `next` is never touched. A test that logs property
    /// reads sees the difference between that and reading `next` first; six of them do.
    pub(crate) fn close_unread(vm: &mut Vm, heap: &mut Heap, iterator: Value) {
        Self {
            iterator,
            next: Value::Undefined,
        }
        .close(vm, heap);
    }

    /// A record for an iterator whose `next` was read earlier — what a stored helper holds.
    pub(super) fn of(iterator: Value, next: Value) -> Self {
        Self { iterator, next }
    }

    /// The `next` this record is holding, for a caller that has to store it.
    pub(super) fn next_method(&self) -> Value {
        self.next
    }

    /// §7.4.10 `GetIteratorDirect` — the object *is* the iterator, and only its `next` is read.
    ///
    /// What every method on `Iterator.prototype` uses. It does **not** ask for `[@@iterator]`, so a
    /// helper works on anything with a `next` and does not care whether it is iterable — which is
    /// why `Iterator.prototype.toArray.call({next() {…}})` is meaningful.
    pub(super) fn direct(vm: &mut Vm, heap: &mut Heap, iterator: Value) -> Completion<Self> {
        let name = key(heap, "next");
        let next = vm.get_property_key(iterator, name, heap)?;
        Ok(Self { iterator, next })
    }

    /// §7.4.2 `GetIterator(obj, sync)` — ask the object for its iterator, then read that one's
    /// `next`.
    ///
    /// The difference from [`Walk::direct`] is which object is walked. This one asks
    /// `[@@iterator]` and walks *what it answers*, so a `Map`, a `Set`, a string and a generator
    /// are all acceptable and none of them is its own iterator. `direct` walks the object it was
    /// handed and never asks — which is right for §27.1.4's helpers and wrong for everything that
    /// takes an "iterable".
    ///
    /// Reading a `length` and the indices under it is neither, and is the shape to watch for: it
    /// accepts an Array and answers `{}` for a `Map`, which is a wrong value rather than a
    /// refusal. `Object.fromEntries` did exactly that until this existed.
    pub(super) fn over(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<Self> {
        let method = match vm.realm().well_known(super::well_known_at("iterator")) {
            Some(symbol) => {
                vm.get_property_key(value, crate::heap::PropertyKey::from_symbol(symbol), heap)?
            }
            None => Value::Undefined,
        };
        // §7.4.2 step 3 — `GetMethod`, so absent means **not iterable** rather than "walk the
        // object itself". That is what separates this from §27.1.4.7's `flatMap`, whose step 2.b
        // deliberately falls back to the object, and the two would otherwise look alike.
        //
        // One test and not two: `GetMethod` separates `undefined`/`null` from a non-callable, and
        // §7.4.2 then throws for both — so asking about nullishness first is a branch whose two
        // sides reach the same throw with the same words. Mutation coverage said so.
        if !heap.is_callable(method) {
            return Err(Abrupt::type_error("this value is not iterable"));
        }
        Self::from_method(vm, heap, value, method)
    }

    /// §7.4.4 `GetIteratorFromMethod` — call the method, and keep what it answered.
    pub(super) fn from_method(
        vm: &mut Vm,
        heap: &mut Heap,
        object: Value,
        method: Value,
    ) -> Completion<Self> {
        let iterator = vm.call_value(method, object, &[], heap)?;
        let Value::Object(_) = iterator else {
            return Err(Abrupt::type_error("an iterator must be an object"));
        };
        let name = key(heap, "next");
        let next = vm.get_property_key(iterator, name, heap)?;
        Ok(Self { iterator, next })
    }

    /// §7.4.8 `IteratorStepValue` — the next value, or `None` once it is done.
    pub(super) fn step(&self, vm: &mut Vm, heap: &mut Heap) -> Completion<Option<Value>> {
        let step = vm.call_value(self.next, self.iterator, &[], heap)?;
        let Value::Object(_) = step else {
            return Err(Abrupt::type_error("an iterator must answer an object"));
        };
        let done = key(heap, "done");
        if vm.get_property_key(step, done, heap)?.to_boolean(heap) {
            return Ok(None);
        }
        let value = key(heap, "value");
        Ok(Some(vm.get_property_key(step, value, heap)?))
    }

    /// §7.4.9 `IteratorClose` — tell the iterator the walk stopped early.
    ///
    /// A `return` that throws is swallowed. Every caller here is already answering a question and
    /// has one of its own to report; §7.4.9 step 6 keeps the original completion when the walk was
    /// abandoned for a *value* rather than an error, and that is every use here.
    pub(super) fn close(&self, vm: &mut Vm, heap: &mut Heap) {
        let name = key(heap, "return");
        let Ok(method) = vm.get_property_key(self.iterator, name, heap) else {
            return;
        };
        // No guard for an absent `return`: calling `undefined` is already an error, and the error
        // is discarded here anyway — so the check and its absence do exactly the same thing, which
        // makes it a branch nothing can test. §7.4.9 wants both cases silent and both are.
        let _ = vm.call_value(method, self.iterator, &[], heap);
    }

    /// §7.4.9 `IteratorClose` again, with a **normal** completion — so what it finds is reported.
    ///
    /// The difference from [`Walk::close`] is step 4, and it is the clause's own: closing carries a
    /// completion, and step 4 keeps *that* one when it is a throw. Every caller of the swallowing
    /// form above is abandoning a walk because something already went wrong, so the close's own
    /// trouble is discarded by the clause rather than by convenience. §27.1.4's Iterator Helper
    /// `return` is the caller on the other side: it closes with `NormalCompletion(unused)`, so
    /// there is nothing for step 4 to keep and steps 5 and 6 are what the program sees.
    ///
    /// Three ways that is visible, and test262 has a file per helper for each: a `return` **getter**
    /// that throws (step 2), a `return` that throws when called (step 5), and one that answers a
    /// primitive (step 6, a TypeError of the clause's own making).
    pub(super) fn close_reporting(&self, vm: &mut Vm, heap: &mut Heap) -> Completion<()> {
        let name = key(heap, "return");
        // Step 2 — inside `Completion(...)`, so a throwing getter is `innerResult` and step 5
        // reports it. With a normal completion to keep there is nothing between the two.
        let method = vm.get_property_key(self.iterator, name, heap)?;
        // Step 3.b — §7.3.11 reads null as absent too, and an absent `return` is not a failure to
        // close: there was nothing to tell.
        if matches!(method, Value::Undefined | Value::Null) {
            return Ok(());
        }
        if !heap.is_callable(method) {
            return Err(Abrupt::type_error(
                "this iterator's return is not a function",
            ));
        }
        let answered = vm.call_value(method, self.iterator, &[], heap)?;
        // Step 6 — the answer has to be an object, and it is otherwise unexamined. This is the one
        // TypeError §7.4.9 raises itself rather than passing on.
        match answered {
            Value::Object(_) => Ok(()),
            _ => Err(Abrupt::type_error(
                "this iterator's return did not answer an object",
            )),
        }
    }
}

/// §7.4.2 step 4 — an iterator's `next`, read **once**, which is what makes a record a record.
///
/// For a caller that has already got the iterator some other way and needs the other half. Its
/// failure belongs to `GetIterator` rather than to the walk, and that is what decides whether an
/// abrupt completion afterwards closes anything: a `next` getter that throws leaves no record to
/// close, where a `next` *call* that throws leaves one that is already done.
pub(super) fn next_method(vm: &mut Vm, heap: &mut Heap, iterator: Value) -> Completion<Value> {
    let name = key(heap, "next");
    vm.get_property_key(iterator, name, heap)
}

/// How §7.3.35 `GroupBy` turns what the callback answered into the key it groups under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Keying {
    /// `property` — §7.1.19 `ToPropertyKey`, so the key becomes a String or a Symbol. Runs the
    /// program's own `toString`, and a throw from it closes the iterator.
    Property,
    /// `zero` — §24.5.1 `CanonicalizeKeyedCollectionKey`, which is only `-0` becoming `+0`.
    /// Every other value groups by `SameValue`, so `NaN` groups with `NaN` and two objects never do.
    Zero,
}

/// §7.3.35 `GroupBy ( items, callback, keyCoercion )` — the walk both `groupBy` methods are.
///
/// An **ordered** list of groups rather than a map, and that is the clause: §20.1.2.13 and
/// §24.1.2.1 both build their answer by walking `groups` in order, so the properties of the object
/// and the entries of the `Map` come out in the order their keys were *first* seen. A hash map here
/// would answer the same values in a different order, which `Object.keys` reports.
///
/// Every abrupt completion after the iterator exists closes it — the clause's
/// `IfAbruptCloseIterator` — because the walk is being abandoned part-way and the iterator is owed
/// the news. The callback's own throw and the key conversion's are both that.
pub(super) fn group_by(
    vm: &mut Vm,
    heap: &mut Heap,
    items: Value,
    callback: Value,
    keying: Keying,
) -> Completion<Vec<(Value, Vec<Value>)>> {
    // Steps 1 and 2, in that order and **before** the iterator is asked for: a nullish `items` is
    // refused without the callback being examined, and a callback that is not callable is refused
    // without `[@@iterator]` being read. Reordering either is observable from a getter.
    if matches!(items, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error("groupBy cannot walk undefined or null"));
    }
    if !heap.is_callable(callback) {
        return Err(Abrupt::type_error("groupBy needs a function to group by"));
    }
    let walk = Walk::over(vm, heap, items)?;
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut at: u64 = 0;
    loop {
        let Some(value) = walk.step(vm, heap)? else {
            return Ok(groups);
        };
        let index = Value::Number(at as f64);
        let key = match vm.call_value(callback, Value::Undefined, &[value, index], heap) {
            Ok(key) => key,
            Err(error) => {
                walk.close(vm, heap);
                return Err(error);
            }
        };
        let key = match keying {
            // §7.1.19, which can run a `toString` and so can throw — step 6.g.ii closes for it.
            Keying::Property => match vm.to_property_key(key, heap) {
                Ok(key) => heap.key_value(key),
                Err(error) => {
                    walk.close(vm, heap);
                    return Err(error);
                }
            },
            // §24.5.1 — `-0` and `+0` are one key, and nothing else is touched. No completion,
            // which is why this arm has no close: the operation cannot fail. `NaN` is deliberately
            // left alone and still groups with `NaN`, because `SameValue` says it does.
            Keying::Zero => match key {
                Value::Number(number) => Value::Number(match number == 0.0 {
                    true => 0.0,
                    false => number,
                }),
                other => other,
            },
        };
        // `AddValueToKeyedGroup` — an existing group is found by `SameValue`, so `NaN` joins `NaN`
        // and `0` joins `-0` only because the canonicalisation above already made them one value.
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.same_value(&key, heap))
        {
            Some((_, elements)) => elements.push(value),
            None => groups.push((key, vec![value])),
        }
        at += 1;
    }
}

/// Build the three iterator prototypes into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm) {
    // §27.1.2.1 — `[@@iterator]` answers the receiver. An iterator is iterable, which is what lets
    // one be passed anywhere an iterable is wanted.
    let shared = realm.iterator_prototype();
    let itself = heap.new_native_function(realm.function_prototype(), same);
    super::define_function_metadata(heap, itself, "[Symbol.iterator]", 0);
    if let Some(symbol) = realm.well_known(super::well_known_at("iterator")) {
        let name = crate::heap::PropertyKey::from_symbol(symbol);
        let _ = heap.define_own_property(
            shared,
            name,
            &PropertyDescriptor {
                value: Some(Value::Object(itself)),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }

    for (prototype, tag) in [
        (realm.array_iterator_prototype(), "Array Iterator"),
        (realm.string_iterator_prototype(), "String Iterator"),
    ] {
        define_method(heap, realm, prototype, "next", 0, next);
        // §23.1.5.2.2 and §22.1.5.2.2 — the tag is what tells the two apart in a message, and it
        // is the only thing that does: they are otherwise the same shape.
        let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) else {
            continue;
        };
        let name = crate::heap::PropertyKey::from_symbol(symbol);
        let units: Vec<u16> = tag.encode_utf16().collect();
        let value = Value::String(heap.intern(&units));
        let _ = heap.define_own_property(
            prototype,
            name,
            &PropertyDescriptor {
                value: Some(value),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}

/// §27.1.2.1 `%IteratorPrototype%[@@iterator]` — the receiver, whatever it is.
///
/// Does not check what it was given. §27.1.2.1 is one step long and has nothing to check: it
/// answers `this`, and an object that is not an iterator gets itself back and fails later, where
/// the failure names the right thing.
fn same(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(call.this_value)
}

/// §23.1.5.2.1 and §22.1.5.2.1 — `next` for both kinds of iterator.
fn next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(iterator) = call.this_value else {
        return Err(Abrupt::type_error("this method requires an iterator"));
    };
    // Step 2's `RequireInternalSlot`. An object that merely has the right prototype is not one of
    // these, and answering for it would mean inventing a position it never had.
    let Some(state) = heap
        .object(iterator)
        .and_then(crate::heap::Object::iteration)
    else {
        return Err(Abrupt::type_error("this method requires an iterator"));
    };
    let state = state.clone();
    let Some((value, at)) = step(vm, heap, &state)? else {
        // Once done, done: a later `next` must not start finding things again because the target
        // grew back. §23.1.5.2.1 step 4.b sets the kind to empty, and this is that.
        if let Some(found) = heap
            .object_mut(iterator)
            .and_then(crate::heap::Object::iteration_mut)
        {
            found.done = true;
        }
        return result(vm, heap, Value::Undefined, true);
    };
    if let Some(found) = heap
        .object_mut(iterator)
        .and_then(crate::heap::Object::iteration_mut)
    {
        found.at = at;
    }
    result(vm, heap, value, false)
}

/// One step: what this iterator answers next, and where it has got to, or `None` if it is spent.
fn step(vm: &mut Vm, heap: &mut Heap, state: &Iteration) -> Completion<Option<(Value, u64)>> {
    if state.done {
        return Ok(None);
    }
    match state.kind {
        Iterated::Characters => Ok(character(heap, state)),
        kind => indexed(vm, heap, state, kind),
    }
}

/// §22.1.5.1 — the code point at this position, and the position after it.
///
/// Code *points*, so a surrogate pair is one step. That is what makes `for`-`of` over a String
/// different from a `for` over its indices, and it is why this cannot be an Array Iterator with a
/// String inside it.
fn character(heap: &mut Heap, state: &Iteration) -> Option<(Value, u64)> {
    let Value::String(data) = state.over else {
        return None;
    };
    let units = heap.string(data)?.to_vec();
    let at = usize::try_from(state.at).ok()?; // a position past `usize` is past the string, and that is the answer
    let first = *units.get(at)?;
    // A leading surrogate followed by a trailing one is one code point and two units; anything
    // else, a lone surrogate included, is one of each.
    let paired = (0xD800..0xDC00).contains(&first)
        && units
            .get(at + 1)
            .is_some_and(|next| (0xDC00..0xE000).contains(next));
    let width = 1 + usize::from(paired);
    let taken = units.get(at..at + width)?.to_vec();
    let id = heap.intern(&taken);
    Some((Value::String(id), state.at + width as u64))
}

/// §23.1.5.2.1 — the key, the value, or both, at this position.
fn indexed(
    vm: &mut Vm,
    heap: &mut Heap,
    state: &Iteration,
    kind: Iterated,
) -> Completion<Option<(Value, u64)>> {
    // §23.1.5.1 splits here on what is being walked. A **TypedArray** goes through
    // `ValidateTypedArray`, which *throws* for a detached buffer or a window that no longer fits
    // one — where reading `length` as a property answers `0` and the walk ends quietly. Those are
    // the same answer for an array that merely ran out and a different one for a buffer that went
    // away underneath, which is exactly the distinction the clause draws.
    //
    // And the length is the view's own rather than a `Get`, so a `length` accessor a script put on
    // the prototype cannot lengthen the walk.
    let length = match matches!(state.over, Value::Object(id) if heap.typed_view(id).is_some()) {
        true => super::typed_methods::validate(heap, state.over)?.1.count() as u64,
        false => {
            let name = key(heap, "length");
            let length = vm.get_property_key(state.over, name, heap)?;
            // §23.1.5.2.1 step 6 reads `length` *every* step, not once — so an array that shrinks
            // while being walked stops early, and one that grows keeps going. That is observable
            // and is the reason this is not a count taken at the start.
            super::array_methods::to_length(vm.to_number(length, heap)?)
        }
    };
    if state.at >= length {
        return Ok(None);
    }
    let index = Value::Number(state.at as f64);
    let value = match kind {
        Iterated::Keys => index,
        _ => {
            let at = super::array_methods::index_key(heap, state.at);
            let element = vm.get_property_key(state.over, at, heap)?;
            match kind {
                Iterated::Entries => super::array::from_values(vm, heap, &[index, element])?,
                _ => element,
            }
        }
    };
    Ok(Some((value, state.at + 1)))
}

/// §7.4.13 `CreateIterResultObject` — `{value, done}`, ordinary in every way.
pub(super) fn result(vm: &mut Vm, heap: &mut Heap, value: Value, done: bool) -> Completion<Value> {
    let object = heap.new_object(Some(vm.realm().object_prototype()));
    for (name, held) in [("value", value), ("done", Value::Boolean(done))] {
        let name = key(heap, name);
        let _ = heap.define_own_property(
            object,
            name,
            &PropertyDescriptor {
                value: Some(held),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    Ok(Value::Object(object))
}

/// §23.1.5.1 `CreateArrayIterator`, for the three `Array.prototype` methods that make one.
pub(super) fn over_array(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: Iterated,
) -> Completion<Value> {
    // Step 1's `ToObject`, so `Array.prototype.values.call("ab")` walks a String object's
    // characters by index — which is not the same as a String's own iterator, and is right.
    let over = vm.object_for(call.this_value, heap)?;
    let prototype = vm.realm().array_iterator_prototype();
    Ok(Value::Object(heap.new_iterator(
        prototype,
        Iteration {
            over,
            at: 0,
            kind,
            done: false,
        },
    )))
}

/// §22.1.3.34 `String.prototype[@@iterator]` — `CreateStringIterator`.
pub(super) fn over_string(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    if matches!(call.this_value, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "this method cannot be called on undefined or null",
        ));
    }
    let data = vm.to_string(call.this_value, heap)?;
    let prototype = vm.realm().string_iterator_prototype();
    Ok(Value::Object(heap.new_iterator(
        prototype,
        Iteration {
            over: Value::String(data),
            at: 0,
            kind: Iterated::Characters,
            done: false,
        },
    )))
}
