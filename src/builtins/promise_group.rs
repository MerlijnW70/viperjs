//! §27.2.4.1 to §27.2.4.4 — the combinators, which are one promise waiting on many.
//!
//! # What they share, and why it is not obvious
//!
//! Each of them walks an iterable, turns every element into a promise, and settles one promise of
//! its own when the elements are done. The differences are small and each is a whole clause:
//! `all` fails on the first rejection, `allSettled` never fails, `race` takes the first *settled*
//! whichever way it went, and `any` is `all` with the two halves swapped.
//!
//! # The counter that starts at one
//!
//! `[[RemainingElements]]` begins at **1**, not at 0, and is decremented once when the iterator
//! runs out. That single line is what makes an empty iterable resolve rather than hang, and what
//! stops a batch of already-settled promises resolving the group before the iterator has finished
//! reading it: while the walk is in progress the count can never reach zero, because the walk
//! itself is holding one.
//!
//! # Why the walk is not "collect, then process"
//!
//! §27.2.4.1.1 reads *one* element, resolves it, subscribes to it, and only then reads the next.
//! An iterator with side effects can see that, and so can a `then` that a program replaced — so
//! draining the iterable into a list first and looping over the list afterwards gives the right
//! answer to every test about values and the wrong order to every test about effects.

use super::key;
use crate::heap::{
    Capability, Gather, Group, Heap, Native, NativeCall, ObjectId, PropertyDescriptor,
    ReactionKind, Role,
};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// §27.2.4.1 — `Promise.all(iterable)`.
pub(super) fn all(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    combine(vm, heap, call, Group::All)
}

/// §27.2.4.2 — `Promise.allSettled(iterable)`.
pub(super) fn all_settled(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    combine(vm, heap, call, Group::AllSettled)
}

/// §27.2.4.3 — `Promise.any(iterable)`.
pub(super) fn any(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    combine(vm, heap, call, Group::Any)
}

/// §27.2.4.4 — `Promise.race(iterable)`.
pub(super) fn race(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    combine(vm, heap, call, Group::Race)
}

/// The shape all four share — §27.2.4.1 steps 1 to 6, which are word for word the same in each.
///
/// The capability is made **first**, because every failure from here on is reported by rejecting it
/// rather than by throwing: a `Symbol.iterator` that throws, a `then` that is missing, an iterator
/// that misbehaves. That is why `Promise.all(null)` answers a rejected promise instead of raising,
/// and it is the single most surprising thing about these four.
fn combine(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, group: Group) -> Completion<Value> {
    let Value::Object(constructor) = call.this_value else {
        return Err(Abrupt::type_error(
            "a promise combinator must be called on a constructor",
        ));
    };
    let capability = super::promise::new_promise_capability(vm, heap, constructor)?;
    // Steps 3 and 4 — `resolve` is read from the constructor **once**, before the walk, so a
    // program that replaces `Promise.resolve` halfway through an iterable does not get two
    // different ones. It must be callable, and if it is not, that is the first thing rejected.
    let resolve = read_resolve(vm, heap, constructor);
    let outcome = resolve.and_then(|resolve| {
        // Step 5's `GetIterator` — lifted out of the walk so that step 8.a has something to close.
        // A failure here is *before* there is an iterator and closes nothing, which is why the two
        // are separate statements rather than one.
        let iterator = super::array::iterator_of(vm, heap, call.argument(0))?;
        let Some(iterator) = iterator else {
            return Err(Abrupt::type_error(
                "a promise combinator needs something iterable",
            ));
        };
        // §7.4.2 step 4 — reading `next` is part of building the **record**, not part of walking
        // it. So a `next` *getter* that throws is step 5's failure and step 8 is never reached:
        // there is nothing to close, because there was never an iterator record. Left inside the
        // walk it would have been a throw with `done` still false, and closed one.
        let next = super::iterator::next_method(vm, heap, iterator)?;
        walk(
            vm,
            heap,
            iterator,
            next,
            constructor,
            resolve,
            capability,
            group,
        )
    });
    match outcome {
        Ok(()) => Ok(capability.promise),
        // Steps 5 and 6 — anything that went wrong *rejects* rather than throwing, which is what
        // makes a combinator always answer a promise. `IfAbruptRejectPromise` is the specification's
        // name for these three lines and it appears in every one of the four clauses.
        Err(abrupt) => {
            let reason = vm.thrown_value(abrupt, heap);
            vm.call_value(capability.reject, Value::Undefined, &[reason], heap)?;
            Ok(capability.promise)
        }
    }
}

/// §27.2.4.1 step 3 `GetPromiseResolve` — the constructor's `resolve`, which must be callable.
fn read_resolve(vm: &mut Vm, heap: &mut Heap, constructor: ObjectId) -> Completion<Value> {
    let name = key(heap, "resolve");
    let resolve = vm.get_property_key(Value::Object(constructor), name, heap)?;
    match heap.is_callable(resolve) {
        true => Ok(resolve),
        false => Err(Abrupt::type_error(
            "the constructor's resolve is not a function",
        )),
    }
}

/// §27.2.4.1.1 and its three siblings — the walk over the iterable.
#[allow(clippy::too_many_arguments)] // one Iterator Record and one group, spelled out
fn walk(
    vm: &mut Vm,
    heap: &mut Heap,
    iterator: Value,
    next: Value,
    constructor: ObjectId,
    resolve: Value,
    capability: Capability,
    group: Group,
) -> Completion<()> {
    // The counter starts at one and the walk itself holds that one, so the group cannot settle
    // while there are still elements to read — however many of them are already settled.
    let gather = Rc::new(RefCell::new(Gather {
        values: Vec::new(),
        remaining: 1,
        capability,
        group,
    }));
    let done_key = key(heap, "done");
    let value_key = key(heap, "value");
    let then_key = key(heap, "then");
    let mut index = 0_usize;
    loop {
        // §7.4.8 steps 2.a, 5.a and 9 — a step that throws leaves the record **done**, so nothing
        // below closes for it. Said by *where the `?` is* rather than by a flag: everything from
        // here to the value arriving propagates untouched, and everything after it goes through
        // the close below. A flag would have needed an initial value no input could reach.
        let step = vm.call_value(next, iterator, &[], heap)?;
        let Value::Object(_) = step else {
            return Err(Abrupt::type_error("an iterator must answer an object"));
        };
        if vm.get_property_key(step, done_key, heap)?.to_boolean(heap) {
            break;
        }
        let element = vm.get_property_key(step, value_key, heap)?;
        // A value arrived, so the walk is live and §27.2.4.1 step 8.a now has an iterator to
        // close. `resolve`, the `then` lookup and the `then` call are all inside that.
        // The slot is made now and filled later, so that the answer is in *iteration* order
        // however the promises settle. A list appended to on settlement would hold the right
        // values in the wrong places, and only sometimes.
        //
        // Kept for `race` too, which reads neither. It could be skipped and the skipping could
        // not be tested: `race` subscribes the capability's own functions, so nothing ever reads
        // a slot or the count, and a guard here would be a branch with no behaviour behind it.
        // The one place the difference is real is the settlement below, which `race` must not do.
        gather.borrow_mut().values.push(Value::Undefined);
        let subscribed = subscribe(
            vm,
            heap,
            element,
            index,
            then_key,
            &gather,
            constructor,
            resolve,
            group,
        );
        if let Err(error) = subscribed {
            // Step 8.a's `IteratorClose`, and the **swallowing** one: the clause hands §7.4.9 an
            // abrupt completion and its step 4 keeps that one, so what the `return` method does
            // next is not the program's answer.
            super::iterator::Walk::close_unread(vm, heap, iterator);
            return Err(error);
        }
        index += 1;
    }
    // The walk is over and gives up the one it was holding. Only now can the count reach zero, and
    // for an empty iterable it reaches zero here — which is why `Promise.all([])` resolves with an
    // empty array rather than waiting for ever.
    //
    // And why `Promise.race([])` must *not* come here: §27.2.4.4.1 has no such step, because
    // there is nothing for an empty race to be first at. Settling it would resolve with an empty
    // array, which is `all`'s answer to a question `race` was not asked.
    if group != Group::Race {
        gather.borrow_mut().remaining -= 1;
        settle_if_done(vm, heap, &gather)?;
    }
    Ok(())
}

/// One element: resolve it through the constructor and subscribe this group's handlers to it.
///
/// Everything §27.2.4.1.1 does *after* a value has been taken from the iterator, which is exactly
/// the part step 8.a closes for. Split out so that the closing is one branch at one place rather
/// than a flag the loop keeps in step with three `?`s.
#[allow(clippy::too_many_arguments)] // the element, where it goes, and the group it belongs to
fn subscribe(
    vm: &mut Vm,
    heap: &mut Heap,
    element: Value,
    index: usize,
    then_key: crate::heap::PropertyKey,
    gather: &Rc<RefCell<Gather>>,
    constructor: ObjectId,
    resolve: Value,
    group: Group,
) -> Completion<()> {
    let promise = vm.call_value(resolve, Value::Object(constructor), &[element], heap)?;
    let (on_fulfilled, on_rejected) = handlers(vm, heap, gather, index, group);
    // Incremented *before* subscribing, because an already-settled promise settles this element
    // during `then` — and a count raised afterwards would already be wrong.
    gather.borrow_mut().remaining += 1;
    let then = vm.get_property_key(promise, then_key, heap)?;
    vm.call_value(then, promise, &[on_fulfilled, on_rejected], heap)?;
    // DR-0013 — an iterator that never says it is done would otherwise grow the list until the
    // process died, and each element allocates. The heap's budget is what notices, and it counts
    // as abandoning the walk: the iterator is owed the news like any other early exit.
    super::array_methods::within_budget(heap)
}

/// The two functions this element is subscribed with.
///
/// `race` needs none of its own: §27.2.4.4.1 subscribes each element with the *capability's* two
/// functions directly, so the first to settle wins and the rest find `[[AlreadyResolved]]` set.
/// That is the whole implementation of racing, and it is why `race` keeps no state at all.
fn handlers(
    vm: &Vm,
    heap: &mut Heap,
    gather: &Rc<RefCell<Gather>>,
    index: usize,
    group: Group,
) -> (Value, Value) {
    let capability = gather.borrow().capability;
    if group == Group::Race {
        return (capability.resolve, capability.reject);
    }
    // `[[AlreadyCalled]]` is shared by the pair, which matters only for `allSettled`: it has two
    // element functions per element, and a promise that somehow settled both ways must fill its
    // slot once. `all` has one function and shares the record with nothing.
    let called = Rc::new(Cell::new(false));
    let mut make = |native: Native, kind: ReactionKind| {
        let function = heap.new_native_function(vm.realm().function_prototype(), native);
        super::define_function_metadata(heap, function, "", 1);
        if let Some(object) = heap.object_mut(function) {
            object.set_role(Role::Element {
                index,
                called: Rc::clone(&called),
                gather: Rc::clone(gather),
                kind,
            });
        }
        Value::Object(function)
    };
    // §27.2.4.3 is §27.2.4.1 with the two halves exchanged, and saying so here is the whole of
    // the difference: `any` collects *rejections* and lets the first fulfilment through, where
    // `all` collects fulfilments and lets the first rejection through.
    match group {
        // §27.2.4.1.1 step 8.j — the group's own reject, so the first rejection rejects it
        // directly and no slot is ever filled.
        Group::All => (
            make(element_settled, ReactionKind::Fulfil),
            capability.reject,
        ),
        // §27.2.4.3.1 step 8.j — the group's own resolve, for the same reason from the other side.
        Group::Any => (
            capability.resolve,
            make(element_settled, ReactionKind::Reject),
        ),
        _ => (
            make(element_settled, ReactionKind::Fulfil),
            make(element_settled, ReactionKind::Reject),
        ),
    }
}

/// §27.2.4.1.2 and §27.2.4.2.2 — one element has settled.
fn element_settled(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Some(Role::Element {
        index,
        called,
        gather,
        kind,
    }) = heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
        .cloned()
    else {
        return Ok(Value::Undefined);
    };
    // §27.2.4.1.2 step 1 — once. A promise that called both of its handlers, or the same one
    // twice, must not decrement the count twice: the group would settle early, with holes.
    if called.replace(true) {
        return Ok(Value::Undefined);
    }
    let group = gather.borrow().group;
    let settled = call.argument(0);
    // §27.2.4.2.2 — `allSettled` records *how* it settled, in an object with two shapes. `all`
    // records the value itself, having no other outcome to distinguish.
    let recorded = match group {
        Group::AllSettled => outcome_object(vm, heap, kind, settled)?,
        _ => settled,
    };
    {
        let mut state = gather.borrow_mut();
        if let Some(slot) = state.values.get_mut(index) {
            *slot = recorded;
        }
        state.remaining -= 1;
    }
    settle_if_done(vm, heap, &gather)?;
    Ok(Value::Undefined)
}

/// §27.2.4.2.2 steps 9 to 12 — `{ status, value }` or `{ status, reason }`.
///
/// Two shapes and not one with a `null`: a program tells them apart by `status`, and an object
/// carrying both keys would answer `'value' in result` wrongly for a rejection.
fn outcome_object(
    vm: &Vm,
    heap: &mut Heap,
    kind: ReactionKind,
    settled: Value,
) -> Completion<Value> {
    let object = heap.new_object(Some(vm.realm().object_prototype()));
    let (status, name) = match kind {
        ReactionKind::Fulfil => ("fulfilled", "value"),
        ReactionKind::Reject => ("rejected", "reason"),
    };
    let text = super::text(heap, status);
    for (name, value) in [(key(heap, "status"), text), (key(heap, name), settled)] {
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(object, name, &descriptor);
    }
    Ok(Value::Object(object))
}

/// Settle the group if the count has reached zero — the caller has already given up its own.
fn settle_if_done(vm: &mut Vm, heap: &mut Heap, gather: &Rc<RefCell<Gather>>) -> Completion<Value> {
    let (finished, capability, values, group) = {
        let state = gather.borrow();
        (
            state.remaining,
            state.capability,
            state.values.clone(),
            state.group,
        )
    };
    if finished != 0 {
        return Ok(Value::Undefined);
    }
    let array = super::array::from_values(vm, heap, &values)?;
    // §27.2.4.3.1 step 8.d — running out of elements is `any`'s *failure*, and the reasons
    // gathered along the way are what it failed with. Every other combinator's end is its success.
    if group == Group::Any {
        let error = aggregate_error(vm, heap, array)?;
        return vm.call_value(capability.reject, Value::Undefined, &[error], heap);
    }
    vm.call_value(capability.resolve, Value::Undefined, &[array], heap)
}

/// §27.2.4.3.1 step 8.d.iii — an `AggregateError` carrying the reasons, made without the
/// constructor.
///
/// Built directly rather than through `AggregateError` itself, because §27.2.4.3.1 says
/// `OrdinaryCreateFromConstructor(%AggregateError%, …)` and then defines `errors` — it never calls
/// the constructor, so a program that replaced `AggregateError` does not change what `any` rejects
/// with, and one that made it throw cannot make `any` throw.
fn aggregate_error(vm: &mut Vm, heap: &mut Heap, errors: Value) -> Completion<Value> {
    let error = heap.new_object(Some(vm.realm().aggregate_error_prototype()));
    let descriptor = PropertyDescriptor {
        value: Some(errors),
        writable: Some(true),
        enumerable: Some(false),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    let name = key(heap, "errors");
    let _ = heap.define_own_property(error, name, &descriptor);
    Ok(Value::Object(error))
}

/// §7.4.14 `IterableToList` — everything an iterable has, in order.
///
/// Here rather than beside the arrays because §20.5.7.1 is its only other caller and this is where
/// the walking of an iterable already lives.
pub(super) fn iterable_to_list(
    vm: &mut Vm,
    heap: &mut Heap,
    iterable: Value,
) -> Completion<Vec<Value>> {
    let Some(iterator) = super::array::iterator_of(vm, heap, iterable)? else {
        return Err(Abrupt::type_error("this is not iterable"));
    };
    let next = key(heap, "next");
    let next = vm.get_property_key(iterator, next, heap)?;
    let done_key = key(heap, "done");
    let value_key = key(heap, "value");
    let mut taken = Vec::new();
    loop {
        let step = vm.call_value(next, iterator, &[], heap)?;
        let Value::Object(_) = step else {
            return Err(Abrupt::type_error("an iterator must answer an object"));
        };
        if vm.get_property_key(step, done_key, heap)?.to_boolean(heap) {
            return Ok(taken);
        }
        taken.push(vm.get_property_key(step, value_key, heap)?);
        // DR-0013 — an iterator that never says it is done would grow this until the process died.
        super::array_methods::within_budget(heap)?;
    }
}
