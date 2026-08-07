//! §27.1.3 and §27.1.4 — the async iteration protocol, and the adapter that fakes it.
//!
//! # Why a wrapper exists at all
//!
//! `for await` walks an **async** iterator: every step answers a promise, and the loop awaits it
//! before the body runs. Most things are not async iterators — an array is not, a string is not, a
//! sync generator is not — and §7.4.3's `GetIterator(obj, async)` does not refuse them. It takes
//! their ordinary `[@@iterator]` and puts one of these in front, so the loop only ever talks to one
//! protocol and the adapting happens in one place.
//!
//! # What the adapter actually does with a turn
//!
//! It is not a `Promise.resolve` around the whole result. §27.1.4.4 reads `done` from the sync
//! result **first**, then awaits the *value*, then pairs the two — so an inner iterator that yields
//! a promise has that promise unwrapped, and the `done` beside it is the one that was read before
//! the await. Doing it the other way round would make `for await (const x of [Promise.resolve(1)])`
//! bind a promise rather than `1`, which is exactly what the protocol is for.
//!
//! That pairing has to survive a turn of the job queue, which is why the `done` is carried by
//! *which* function is attached rather than in a Rust local.

use super::{define_method, key};
use crate::heap::{Heap, NativeCall, PropertyKey, Role};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build §27.1.3's prototype and §27.1.4.2's wrapper prototype.
pub fn install(heap: &mut Heap, realm: &Realm) {
    // §27.1.3.1 `[@@asyncIterator]() { return this }` — one step long, and it is the whole of what
    // makes an async iterator *async iterable*. It was left out until now on the grounds that
    // nothing a script could name inherited from this object; §27.6's async generators do, and
    // without it `for await (const x of asyncGen())` finds no `[@@asyncIterator]`, falls back to
    // §7.4.3's synchronous path, and reads a `Symbol.iterator` the specification says it must not
    // even look for.
    let shared = realm.async_iterator_prototype();
    let itself = heap.new_native_function(realm.function_prototype(), same, realm.id());
    super::define_function_metadata(heap, itself, "[Symbol.asyncIterator]", 0);
    if let Some(symbol) = heap.well_known(super::well_known_at("asyncIterator")) {
        let name = PropertyKey::from_symbol(symbol);
        let _ = heap.define_own_property(
            shared,
            name,
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(itself)),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
    }
    let wrapper = realm.async_from_sync_iterator_prototype();
    define_method(heap, realm, wrapper, "next", 1, next);
    define_method(heap, realm, wrapper, "return", 1, close);
    define_method(heap, realm, wrapper, "throw", 1, hurl);
}

/// §27.1.3.1 `%AsyncIteratorPrototype%[@@asyncIterator]` — the receiver, whatever it is.
///
/// Checks nothing, because the clause has nothing to check: it is `return this` and a primitive
/// receiver is as valid an answer as an object.
fn same(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(call.this_value)
}

/// §27.1.4.1 `CreateAsyncFromSyncIterator` — put an adapter in front of a sync iterator.
///
/// Takes the **Iterator Record**, `next` and all, because that is what the clause takes: §7.4.3
/// step 1.b.iii has already run `GetIteratorFromMethod`, whose step 4 read `next` once and made a
/// record of it. Reading it again here is a second call of the program's own getter for a step the
/// specification does not have — and the doc used to say the read belonged here, "which is what
/// makes this an Iterator Record rather than a pair of lookups repeated per step". It was the pair
/// of lookups it described.
pub(crate) fn from_sync(
    vm: &mut Vm,
    heap: &mut Heap,
    iterator: Value,
    next: Value,
) -> Completion<(Value, Value)> {
    let wrapper = heap.new_object(Some(vm.realm().async_from_sync_iterator_prototype()));
    if let Some(object) = heap.object_mut(wrapper) {
        object.set_role(Role::SyncIterator { iterator, next });
    }
    // §27.1.4.1 step 4 reads the wrapper's own `next` back out, which is the one this module just
    // installed — a script that replaced it before the loop began gets the one it put there.
    let wrapped = Value::Object(wrapper);
    let method = vm.get_property_key(wrapped, key(heap, "next"), heap)?;
    Ok((wrapped, method))
}

/// §7.4.3 `GetIterator(obj, async)` — an async iterator and its `next`, however `obj` is shaped.
///
/// Three ways in, and the order matters because each is observable. `[@@asyncIterator]` is asked
/// for first; only when there is none does the sync `[@@iterator]` get read, and then the result is
/// wrapped. Asking for both up front would call a getter the specification never reads.
pub(crate) fn get_async_iterator(
    vm: &mut Vm,
    heap: &mut Heap,
    iterable: Value,
) -> Completion<(Value, Value)> {
    if let Some(key) = well_known(heap, "asyncIterator")
        && let Some(method) = method_at(vm, heap, iterable, key)?
    {
        return from_method(vm, heap, iterable, method);
    }
    // Step 1.b — no async iterator, so the sync one is adapted. A thing with neither is a
    // TypeError, and this is where `for await (const x of 1)` is refused.
    let sync = match well_known(heap, "iterator") {
        Some(key) => method_at(vm, heap, iterable, key)?,
        None => None,
    };
    let Some(sync) = sync else {
        return Err(Abrupt::type_error("this is not async iterable"));
    };
    let (iterator, next) = from_method(vm, heap, iterable, sync)?;
    from_sync(vm, heap, iterator, next)
}

/// §7.4.2 `GetIteratorFromMethod` — call the method, check the answer, read its `next` once.
fn from_method(
    vm: &mut Vm,
    heap: &mut Heap,
    iterable: Value,
    method: Value,
) -> Completion<(Value, Value)> {
    let iterator = vm.call_value(method, iterable, &[], heap)?;
    // Step 3 — an iterator that is not an object would have `next` read off a primitive's
    // prototype, and the loop would call something that was never there.
    let Value::Object(_) = iterator else {
        return Err(Abrupt::type_error(
            "an iterator method answered with something that is not an object",
        ));
    };
    let next = vm.get_property_key(iterator, key(heap, "next"), heap)?;
    Ok((iterator, next))
}

/// §7.3.10 `GetMethod` — a property that must be callable if it is there at all.
///
/// `undefined` **and** `null` both mean "there is none", and that distinction is load-bearing
/// twice: §7.4.3 falls back from `[@@asyncIterator]` to `[@@iterator]` on it, and §27.1.4.2.2
/// treats a sync iterator with no `return` as one with nothing to be told rather than an error.
/// Anything else that is not callable is a TypeError, so a misspelled method is reported.
fn method_at(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
    name: PropertyKey,
) -> Completion<Option<Value>> {
    let found = vm.get_property_key(value, name, heap)?;
    if matches!(found, Value::Undefined | Value::Null) {
        return Ok(None);
    }
    if !heap.is_callable(found) {
        return Err(Abrupt::type_error(
            "this iterator's method is not a function",
        ));
    }
    Ok(Some(found))
}

/// The key one of §6.1.5.1's Symbols names, if this heap has it.
fn well_known(heap: &Heap, symbol: &str) -> Option<PropertyKey> {
    let id = heap.well_known(super::well_known_at(symbol))?;
    Some(PropertyKey::from_symbol(id))
}

/// The record a wrapper stands in front of, or a TypeError for anything else.
fn wrapped(heap: &Heap, receiver: Value) -> Completion<(Value, Value)> {
    let held = match receiver {
        Value::Object(id) => heap.object(id).and_then(crate::heap::Object::role),
        _ => None,
    };
    match held {
        Some(Role::SyncIterator { iterator, next }) => Ok((*iterator, *next)),
        _ => Err(Abrupt::type_error(
            "this is not an async-from-sync iterator",
        )),
    }
}

/// §27.1.4.2.1 — `next`, which answers a promise of the sync iterator's next result.
fn next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let capability = vm.intrinsic_capability(heap);
    let stepped = wrapped(heap, call.this_value).and_then(|(iterator, next)| {
        // §7.4.4 `IteratorNext` — the argument is forwarded only when one was given, which is
        // observable: a sync iterator that counts its arguments sees none for a bare `next()`.
        let arguments: &[Value] = match call.arguments.is_empty() {
            true => &[],
            false => &call.arguments[..1],
        };
        vm.call_value(next, iterator, arguments, heap)
    });
    let sync = wrapped(heap, call.this_value)
        .map(|(iterator, _)| iterator)
        .ok();
    continuation(vm, heap, stepped, capability, sync)
}

/// §27.1.4.2.2 — `return`, which tells the sync iterator the walk is over.
fn close(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let capability = vm.intrinsic_capability(heap);
    let sent = call.argument(0);
    let stepped = wrapped(heap, call.this_value).and_then(|(iterator, _)| {
        let name = key(heap, "return");
        let method = method_at(vm, heap, iterator, name)?;
        match method {
            // Step 5 — a sync iterator with no `return` has nothing to be told, and the walk is
            // over all the same: the answer is a *fulfilled* `{ value, done: true }` rather than a
            // rejection, because failing to close something that cannot be closed is not an error.
            None => Err(Abrupt::Thrown(Value::Undefined)),
            Some(method) => vm.call_value(method, iterator, &[sent], heap),
        }
    });
    match stepped {
        // The shape above, unwound: nothing to call, so the result is built here.
        Err(Abrupt::Thrown(Value::Undefined)) => {
            let result = vm.iterator_result(heap, sent, true);
            let _ =
                vm.settle_capability(capability, crate::heap::ReactionKind::Fulfil, result, heap);
            Ok(capability.promise)
        }
        stepped => continuation(vm, heap, stepped, capability, None),
    }
}

/// §27.1.4.2.3 — `throw`, which hands the reason to the sync iterator's own `throw`.
fn hurl(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let capability = vm.intrinsic_capability(heap);
    let sent = call.argument(0);
    let stepped = wrapped(heap, call.this_value).and_then(|(iterator, _)| {
        let name = key(heap, "throw");
        let method = method_at(vm, heap, iterator, name)?;
        match method {
            // Step 5 — a sync iterator that cannot be told to throw is closed instead and the
            // reason is *rejected*, which is the difference from `return` above: the caller asked
            // for something to be thrown and nothing threw it.
            None => {
                crate::builtins::iterator::Walk::close_unread(vm, heap, iterator);
                Err(Abrupt::type_error(
                    "the iterator being walked has no `throw` method",
                ))
            }
            Some(method) => vm.call_value(method, iterator, &[sent], heap),
        }
    });
    continuation(vm, heap, stepped, capability, None)
}

/// §27.1.4.4 `AsyncFromSyncIteratorContinuation` — await the value, then pair it with the `done`.
///
/// The order is the whole of it. `done` is read from the sync result *before* the await, and the
/// value is awaited on its own — so a sync iterator that yields a promise has it unwrapped, and the
/// `done` it is paired with is the one that came with it rather than whatever a later step says.
fn continuation(
    vm: &mut Vm,
    heap: &mut Heap,
    stepped: Completion<Value>,
    capability: crate::heap::Capability,
    close_on_rejection: Option<Value>,
) -> Completion<Value> {
    let settled = stepped.and_then(|result| {
        // §7.4.4 step 3 — a `next` that answered with a primitive is a TypeError, and it is this
        // promise's rejection rather than the caller's exception.
        let Value::Object(_) = result else {
            return Err(Abrupt::type_error(
                "an iterator answered with something that is not an object",
            ));
        };
        let done = vm
            .get_property_key(result, key(heap, "done"), heap)?
            .to_boolean(heap);
        let value = vm.get_property_key(result, key(heap, "value"), heap)?;
        Ok((done, value))
    });
    let (done, value) = match settled {
        Ok(pair) => pair,
        // `IfAbruptRejectPromise` — every step above rejects this promise rather than throwing,
        // which is what makes `next()` answer a promise in every case and never raise.
        Err(abrupt) => return Ok(reject_with(vm, heap, capability, abrupt)),
    };
    // §27.1.4.4 step 13 — while there is more to come, a *rejected* value closes the sync iterator
    // underneath. The sync side has no idea its value was a promise, so nothing else can tell it
    // the walk ended badly, and without this it is left open for good.
    let closing = match (close_on_rejection, done) {
        (Some(iterator), false) => Some(iterator),
        _ => None,
    };
    let constructor = vm.realm().promise_constructor();
    let wrapper = match crate::builtins::promise::promise_resolve(vm, heap, constructor, value) {
        Ok(wrapper) => wrapper,
        // Step 6 — the same close for a `PromiseResolve` that threw on the way in, and the abrupt
        // completion is still the one that reaches the caller.
        Err(abrupt) => {
            if let Some(iterator) = closing {
                crate::builtins::iterator::Walk::close_unread(vm, heap, iterator);
            }
            return Ok(reject_with(vm, heap, capability, abrupt));
        }
    };
    // §27.1.4.4 step 5's closure carries one thing — the `done` read before the await — and it
    // carries it by *being* one of two functions rather than by holding a flag. A flag would have
    // to be read back out, and the arm for "this function is not one of ours" would be a branch
    // nothing could ever take.
    let native = match done {
        true => finished,
        false => carrying_on,
    };
    let unwrap = heap.new_native_function(vm.realm().function_prototype(), native, vm.realm().id());
    super::define_function_metadata(heap, unwrap, "", 1);
    let Value::Object(wrapper) = wrapper else {
        return Ok(reject_with(
            vm,
            heap,
            capability,
            Abrupt::type_error("a resolved value is not a promise"),
        ));
    };
    // Step 13.b — the closure that does it, as a function object, because it has to survive the
    // turn the value spends settling. It carries the sync iterator in the slot the wrapper itself
    // uses; the `next` half is not read from here.
    let on_rejected = match closing {
        None => Value::Undefined,
        Some(iterator) => {
            let closer = heap.new_native_function(
                vm.realm().function_prototype(),
                close_and_rethrow,
                vm.realm().id(),
            );
            super::define_function_metadata(heap, closer, "", 1);
            if let Some(object) = heap.object_mut(closer) {
                object.set_role(Role::SyncIterator {
                    iterator,
                    next: Value::Undefined,
                });
            }
            Value::Object(closer)
        }
    };
    let attached = crate::builtins::promise::perform_then(
        vm,
        heap,
        wrapper,
        Value::Object(unwrap),
        on_rejected,
        Some(capability),
    );
    match attached {
        Ok(_) => Ok(capability.promise),
        Err(abrupt) => Ok(reject_with(vm, heap, capability, abrupt)),
    }
}

/// `IfAbruptRejectPromise` — settle the capability's rejection half and answer with its promise.
fn reject_with(
    vm: &mut Vm,
    heap: &mut Heap,
    capability: crate::heap::Capability,
    abrupt: Abrupt,
) -> Value {
    let reason = vm.thrown_value(abrupt, heap);
    let _ = vm.settle_capability(capability, crate::heap::ReactionKind::Reject, reason, heap);
    capability.promise
}

/// §27.1.4.4 step 13.a — tell the sync iterator the walk ended, then let the reason travel on.
///
/// §7.4.9 with a throw completion, which is what `close_unread` performs: `return` is called if it
/// is there, and anything it does wrong is discarded because the reason already travelling wins.
fn close_and_rethrow(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let held = heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
        .and_then(|role| match role {
            Role::SyncIterator { iterator, .. } => Some(*iterator),
            _ => None,
        });
    if let Some(iterator) = held {
        crate::builtins::iterator::Walk::close_unread(vm, heap, iterator);
    }
    Err(Abrupt::Thrown(call.argument(0)))
}

/// §27.1.4.4 step 5's closure, for a sync result that said it was **done**.
fn finished(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(vm.iterator_result(heap, call.argument(0), true))
}

/// …and for one that said there was more to come.
fn carrying_on(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(vm.iterator_result(heap, call.argument(0), false))
}
