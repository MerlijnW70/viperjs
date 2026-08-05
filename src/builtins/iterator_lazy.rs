//! §27.1.4's five methods that *make* an iterator, and §27.1.3.2's `Iterator.from`.
//!
//! # What "lazy" costs and buys
//!
//! `[1, 2, 3].values().map(f)` calls `f` **not at all** and returns at once. Nothing happens until
//! something asks the result for a value, and then exactly one value is drawn from underneath and
//! one call made. That is what lets `iterator.map(f).take(2)` over an endless source terminate, and
//! it is why these cannot be written as "collect, transform, hand back".
//!
//! # Why the state is a struct rather than a suspended frame
//!
//! §27.1.5 models each of these as a generator, and ViperJS has no generators yet. What a generator
//! would keep in its frame is kept in [`crate::heap::Helper`] instead: the iterator it draws from,
//! what it does to what it draws, and how far it has got. Every observable step is the same, and
//! when frames can be suspended this could become one without a program noticing.
//!
//! # The two prototypes
//!
//! §27.1.5.1's `%IteratorHelperPrototype%` is what these five answer with, and §27.1.3.2.1's
//! `%WrapForValidIteratorPrototype%` is what `Iterator.from` answers with when it is handed
//! something that is not already an Iterator. Both inherit from `%IteratorPrototype%`, so a helper
//! is itself iterable and can be handed to another helper.

use super::iterator::Walk;
use super::{define_method, key};
use crate::heap::{Heap, Helper, Native, NativeCall, ObjectId, Step};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The receiver as an iterator, and the helper built on it — §27.1.4's opening, twice over.
///
/// The callback is checked *after* the iterator is taken and closed if it is not callable, exactly
/// as the consuming methods do: by then the method is holding an iterator and has to give it back.
fn making(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    what: impl FnOnce(Value) -> Step,
    wants: &'static str,
    needs_callback: bool,
) -> Completion<Value> {
    let Value::Object(_) = call.this_value else {
        return Err(Abrupt::type_error("this is not an iterator"));
    };
    // Step 3 before step 4: the callback is judged *before* `GetIteratorDirect`, so a bad one
    // closes the iterator without `next` ever being read.
    let argument = call.argument(0);
    if needs_callback && !heap.is_callable(argument) {
        Walk::close_unread(vm, heap, call.this_value);
        return Err(Abrupt::type_error(wants));
    }
    let walk = Walk::direct(vm, heap, call.this_value)?;
    let helper = Helper::new(call.this_value, walk.next_method(), what(argument));
    let prototype = vm.realm().iterator_helper_prototype();
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_helper(helper);
    }
    Ok(Value::Object(object))
}

/// A count for `take` and `drop` — §27.1.4.12 steps 3 to 7.
///
/// `NaN` is a **RangeError** here rather than the zero `ToIntegerOrInfinity` gives it elsewhere,
/// which is the one place these two differ from every other count in the library. And the iterator
/// is closed on the way out, for the same reason a bad callback closes it.
fn limit(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<u64> {
    let Value::Object(_) = call.this_value else {
        return Err(Abrupt::type_error("this is not an iterator"));
    };
    // Steps 3 to 6 all come before step 7's `GetIteratorDirect`, so the count is converted and
    // judged with `next` still unread — and the close on failure happens that way too.
    let number = vm.to_number(call.argument(0), heap)?;
    if number.is_nan() || number.trunc() < 0.0 {
        Walk::close_unread(vm, heap, call.this_value);
        return Err(Abrupt::range_error("the count must not be negative or NaN"));
    }
    // `+∞` saturates to the largest count there is, which no walk could reach — so "take
    // everything" needs no case of its own.
    Ok(number.trunc() as u64)
}

/// Build a `take` or a `drop`, which take a count rather than a callback.
fn counted(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, take: bool) -> Completion<Value> {
    let count = limit(vm, heap, call)?;
    let walk = Walk::direct(vm, heap, call.this_value)?;
    let what = match take {
        true => Step::Take(count),
        false => Step::Drop(count),
    };
    let prototype = vm.realm().iterator_helper_prototype();
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_helper(Helper::new(call.this_value, walk.next_method(), what));
    }
    Ok(Value::Object(object))
}

/// §27.1.4.8 `Iterator.prototype.map`.
fn map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    making(
        vm,
        heap,
        call,
        Step::Map,
        "the mapper is not a function",
        true,
    )
}

/// §27.1.4.5 `Iterator.prototype.filter`.
fn filter(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    making(
        vm,
        heap,
        call,
        Step::Filter,
        "the predicate is not a function",
        true,
    )
}

/// §27.1.4.3 `Iterator.prototype.flatMap`.
fn flat_map(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    making(
        vm,
        heap,
        call,
        Step::FlatMap,
        "the mapper is not a function",
        true,
    )
}

/// §27.1.4.12 `Iterator.prototype.take`.
fn take(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    counted(vm, heap, call, true)
}

/// §27.1.4.4 `Iterator.prototype.drop`.
fn drop(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    counted(vm, heap, call, false)
}

/// The `{value, done}` an iterator answers with.
fn result(vm: &mut Vm, heap: &mut Heap, value: Value, done: bool) -> Value {
    let object = heap.new_object(Some(vm.realm().object_prototype()));
    for (name, held) in [("value", value), ("done", Value::Boolean(done))] {
        let key = key(heap, name);
        super::array_methods::define_named(heap, object, key, held);
    }
    Value::Object(object)
}

/// A helper's state, copied out of the heap.
///
/// Copied rather than borrowed because every step of a walk may run user code — a mapper, a
/// predicate, an inner iterator's `next` — and that code needs the heap. Holding a borrow across
/// it is not possible, and holding one *around* it would mean the callback could not touch the
/// heap at all.
struct State {
    /// The iterator being drawn from, and its `next`.
    source: Value,
    next: Value,
    /// What this helper does to what it draws.
    what: Step,
    /// How many values have been drawn.
    counter: u64,
    /// The inner iterator a `flatMap` is part-way through, and its `next`.
    inner: Option<(Value, Value)>,
    /// Whether it has already finished for good.
    done: bool,
}

/// The helper's state, copied out so the heap is free while user code runs.
fn state_of(heap: &Heap, object: ObjectId) -> Option<State> {
    heap.object(object)
        .and_then(crate::heap::Object::helper)
        .map(|found| State {
            source: found.source,
            next: found.next,
            what: found.what.clone(),
            counter: found.counter,
            inner: found.inner,
            done: found.done,
        })
}

/// Mark a helper finished, which nothing undoes — §27.1.5.1's completed generator.
fn finish(heap: &mut Heap, object: ObjectId) {
    if let Some(found) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::helper_mut)
    {
        found.done = true;
    }
}

/// §27.1.5.1.1 `%IteratorHelperPrototype%.next` — draw one value, doing this helper's work.
///
/// Written as a loop rather than a recursion because `filter` and `flatMap` may draw any number of
/// values before yielding one, and a source of a million rejected values is a million steps rather
/// than a million frames.
fn helper_next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.this_value else {
        return Err(Abrupt::type_error("this is not an Iterator Helper"));
    };
    let Some(state) = state_of(heap, object) else {
        return Err(Abrupt::type_error("this is not an Iterator Helper"));
    };
    let State {
        source,
        next,
        what,
        mut counter,
        mut inner,
        done,
    } = state;
    // Once finished, finished — the source may start answering again and this does not.
    if done {
        return Ok(result(vm, heap, Value::Undefined, true));
    }
    let walk = Walk::of(source, next);
    // §27.1.4.4 step 5.b — a `drop` skips before it yields anything, and the skipping happens on
    // the *first* `next` rather than when the helper was made. Nothing is drawn until it is asked.
    if let Step::Drop(count) = what
        && counter == 0
    {
        for _ in 0..count {
            match walk.step(vm, heap)? {
                Some(_) => {}
                None => {
                    finish(heap, object);
                    return Ok(result(vm, heap, Value::Undefined, true));
                }
            }
            super::array_methods::within_budget(heap)?;
        }
        counter = count;
    }
    loop {
        // A `flatMap` part-way through an inner iterator draws from that one until it runs out.
        if let Some((iterator, inner_next)) = inner {
            let inner_walk = Walk::of(iterator, inner_next);
            match inner_walk.step(vm, heap) {
                Ok(Some(value)) => {
                    store(heap, object, counter, Some((iterator, inner_next)));
                    return Ok(result(vm, heap, value, false));
                }
                Ok(None) => inner = None,
                Err(raised) => {
                    finish(heap, object);
                    walk.close(vm, heap);
                    return Err(raised);
                }
            }
            continue;
        }
        // §27.1.4.12 step 5.a — a `take` that has had its fill closes the source and stops. The
        // close happens *before* the next value is drawn, so the source is never over-read.
        if let Step::Take(count) = what
            && counter >= count
        {
            finish(heap, object);
            walk.close(vm, heap);
            return Ok(result(vm, heap, Value::Undefined, true));
        }
        let Some(value) = walk.step(vm, heap)? else {
            finish(heap, object);
            return Ok(result(vm, heap, Value::Undefined, true));
        };
        let position = Value::Number(counter as f64);
        counter += 1;
        let answered = match &what {
            Step::Take(_) | Step::Drop(_) => {
                store(heap, object, counter, None);
                return Ok(result(vm, heap, value, false));
            }
            Step::Map(function) | Step::Filter(function) | Step::FlatMap(function) => {
                match vm.call_value(*function, Value::Undefined, &[value, position], heap) {
                    Ok(answer) => answer,
                    // An abrupt callback finishes the helper and closes the source, then carries
                    // its own completion out.
                    Err(raised) => {
                        finish(heap, object);
                        walk.close(vm, heap);
                        return Err(raised);
                    }
                }
            }
        };
        match &what {
            Step::Map(_) => {
                store(heap, object, counter, None);
                return Ok(result(vm, heap, answered, false));
            }
            Step::Filter(_) => {
                if answered.to_boolean(heap) {
                    store(heap, object, counter, None);
                    return Ok(result(vm, heap, value, false));
                }
            }
            // §27.1.4.3 step 5.b.iv — what the mapper answered must be *iterable*, and a string is
            // deliberately not treated as one here: `GetIteratorFlattenable` is called with
            // `reject-primitives`, so `flatMap(() => "ab")` is a TypeError rather than two letters.
            Step::FlatMap(_) => match flattenable(vm, heap, answered) {
                Ok((iterator, inner_next)) => inner = Some((Value::Object(iterator), inner_next)),
                Err(raised) => {
                    finish(heap, object);
                    walk.close(vm, heap);
                    return Err(raised);
                }
            },
            Step::Take(_) | Step::Drop(_) => unreachable!(),
        }
        store(heap, object, counter, inner);
        super::array_methods::within_budget(heap)?;
    }
}

/// Write the moving parts of a helper's state back.
fn store(heap: &mut Heap, object: ObjectId, counter: u64, inner: Option<(Value, Value)>) {
    if let Some(found) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::helper_mut)
    {
        found.counter = counter;
        found.inner = inner;
    }
}

/// §7.4.11 `GetIteratorFlattenable` with `reject-primitives` — an iterator and its `next`.
fn flattenable(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<(ObjectId, Value)> {
    let Value::Object(_) = value else {
        return Err(Abrupt::type_error(
            "what the mapper answered is not iterable",
        ));
    };
    // Step 2.b — `[@@iterator]` if it has one, and the object *itself* if it does not. So an
    // object with only a `next` is accepted, which is what makes a helper flattenable into another.
    let method = match vm.realm().well_known(super::well_known_at("iterator")) {
        Some(symbol) => {
            vm.get_property_key(value, crate::heap::PropertyKey::from_symbol(symbol), heap)?
        }
        None => Value::Undefined,
    };
    let iterator = match matches!(method, Value::Undefined | Value::Null) {
        true => value,
        false => vm.call_value(method, value, &[], heap)?,
    };
    let Value::Object(found) = iterator else {
        return Err(Abrupt::type_error("an iterator must be an object"));
    };
    let name = key(heap, "next");
    let next = vm.get_property_key(iterator, name, heap)?;
    Ok((found, next))
}

/// §27.1.5.1.2 `%IteratorHelperPrototype%.return` — finish, and close what is underneath.
fn helper_return(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.this_value else {
        return Err(Abrupt::type_error("this is not an Iterator Helper"));
    };
    let Some(state) = state_of(heap, object) else {
        return Err(Abrupt::type_error("this is not an Iterator Helper"));
    };
    // Closing twice closes the source once — the second `return` finds a helper that has already
    // finished and has nothing left to pass on. Which is also why a helper whose source ran out on
    // its own does not forward: it is finished, and §7.4.9 is not owed a second telling.
    //
    // **Reporting rather than swallowing.** §27.1.4's `return` closes with a normal completion, so
    // §7.4.9 step 4 has nothing to keep and what the close finds is what the caller sees — a
    // throwing `return`, a throwing `return` *getter*, and a `return` answering a primitive are
    // three errors this method raises. The helper is marked finished first, so a source that throws
    // on the way out is still only asked once.
    if !state.done {
        finish(heap, object);
        Walk::of(state.source, state.next).close_reporting(vm, heap)?;
    }
    Ok(result(vm, heap, Value::Undefined, true))
}

/// §27.1.3.2 `Iterator.from`.
fn from(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `iterate-string-primitives`, so unlike `flatMap` a string *is* iterable here.
    let given = call.argument(0);
    let opened = match given {
        Value::String(_) => {
            let Some(iterator) = super::array::iterator_of(vm, heap, given)? else {
                return Err(Abrupt::type_error("this is not iterable"));
            };
            let Value::Object(found) = iterator else {
                return Err(Abrupt::type_error("an iterator must be an object"));
            };
            let name = key(heap, "next");
            let next = vm.get_property_key(iterator, name, heap)?;
            (found, next)
        }
        _ => flattenable(vm, heap, given)?,
    };
    // Steps 2 and 3 — something that already inherits from `Iterator.prototype` is handed straight
    // back rather than wrapped, so `Iterator.from(x) === x` for anything already an Iterator.
    if inherits_iterator(heap, vm.realm().iterator_prototype(), opened.0) {
        return Ok(Value::Object(opened.0));
    }
    let prototype = vm.realm().wrap_iterator_prototype();
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        // A wrapper is a helper that does nothing to what it draws — `Take` of everything there
        // could be. Modelling it as its own kind would be a second state machine to keep in step
        // with this one, for a difference no program can see.
        found.set_helper(Helper::new(
            Value::Object(opened.0),
            opened.1,
            Step::Take(u64::MAX),
        ));
    }
    Ok(Value::Object(object))
}

/// Whether `object` has `%IteratorPrototype%` anywhere in its chain — §27.1.3.2 step 2.
///
/// Takes the object rather than a value, because every caller has already established it is one:
/// an arm for "not an object" would be a branch no input could reach.
fn inherits_iterator(heap: &Heap, wanted: ObjectId, object: ObjectId) -> bool {
    let mut walk = object;
    loop {
        let Some(next) = heap.object(walk).and_then(|found| found.prototype()) else {
            return false;
        };
        if next == wanted {
            return true;
        }
        walk = next;
    }
}

/// Build §27.1.4's five makers, and the two prototypes their results wear.
pub(super) fn install(heap: &mut Heap, realm: &Realm, constructor: ObjectId) {
    let prototype = realm.iterator_prototype();
    for (name, native) in [
        ("map", map as Native),
        ("filter", filter),
        ("take", take),
        ("drop", drop),
        ("flatMap", flat_map),
    ] {
        define_method(heap, realm, prototype, name, 1, native);
    }
    define_method(heap, realm, constructor, "from", 1, from);
    // §27.1.5.1 and §27.1.3.2.1 — both answer `next` and `return`, and the wrapper's are the same
    // pair: a wrapper is a helper that transforms nothing.
    for target in [
        realm.iterator_helper_prototype(),
        realm.wrap_iterator_prototype(),
    ] {
        define_method(heap, realm, target, "next", 0, helper_next);
        define_method(heap, realm, target, "return", 0, helper_return);
    }
    super::collection::tag_with(
        heap,
        realm,
        realm.iterator_helper_prototype(),
        "Iterator Helper",
    );
}
