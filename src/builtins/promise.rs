//! §27.2 — `Promise`.
//!
//! # What a promise actually is
//!
//! A record of one answer that has not arrived, and a list of what to do when it does. There is no
//! concurrency here and nothing runs in the background: the only thing that makes `then`
//! asynchronous is §9.5's rule about *when* a job may run, which is "when nothing else is
//! running". Everything else is bookkeeping — a state, a value, and two lists.
//!
//! # The three things that are easy to get subtly wrong
//!
//! - **A resolution is not a fulfilment.** §27.2.1.3.2 resolves; §27.2.1.4 fulfils. Resolving with
//!   a *thenable* does not settle the promise at all — it adopts that thenable's eventual answer,
//!   through a job, which is why `Promise.resolve(p)` where `p` is a promise takes two turns of
//!   the queue to see through and not none. An implementation that fulfils with the thenable
//!   passes every test about ordinary values and fails every test about chaining.
//! - **An absent handler is not `undefined`.** §27.2.1.2's `[[Handler]]` may be *empty*, and an
//!   empty one passes the argument through with its type intact: a rejection stays a rejection.
//!   That is the whole of how a `catch` halfway down a chain lets a fulfilment past.
//! - **`then` reads `constructor` first.** §27.2.5.4 goes through `SpeciesConstructor`, so what
//!   `then` answers with is decided by the promise's `constructor` property and its `@@species` —
//!   a subclass gets its own kind back, and a program that replaced `constructor` gets whatever it
//!   put there.

use super::{define_method, define_value, key};
use crate::heap::{
    Capability, Heap, Native, NativeCall, ObjectId, PromiseState, PropertyDescriptor, PropertyKey,
    Reaction, ReactionKind, Role, Settler,
};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::{Job, Vm};

/// Build `Promise` into `heap` as a property of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.promise_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "Promise", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, global, "Promise", Value::Object(constructor));

    // §27.2.5.2 — `Promise.prototype.constructor`, which is what `SpeciesConstructor` reads and so
    // is what decides the kind of promise `then` answers with. Not decoration.
    define_value(heap, prototype, "constructor", Value::Object(constructor));

    for (name, length, native) in [
        ("resolve", 1, resolve_static as Native),
        ("reject", 1, reject_static),
        // §27.2.4.1, §27.2.4.2 and §27.2.4.4 — each takes one iterable, and each `length` is 1.
        ("all", 1, super::promise_group::all),
        ("allSettled", 1, super::promise_group::all_settled),
        ("any", 1, super::promise_group::any),
        ("race", 1, super::promise_group::race),
    ] {
        define_method(heap, realm, constructor, name, length, native);
    }
    // §27.2.4.7 — `get Promise[@@species]` answers the receiver, so a subclass that does nothing
    // gets itself back and `then` on one of its instances makes another of them.
    define_species_getter(heap, realm, constructor);

    for (name, length, native) in [
        ("then", 2, then as Native),
        ("catch", 1, catch),
        ("finally", 1, finally),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §27.2.5.5 — `Promise.prototype[@@toStringTag]`, which is what makes
    // `Object.prototype.toString.call(Promise.resolve())` say `[object Promise]`.
    define_tag(heap, realm, prototype);
}

/// The `Promise` this realm installed, read back off the global object.
///
/// `install` is handed a finished [`Realm`] and cannot write to it, so the constructor it made is
/// found again here rather than passed back. Reading the global is safe at exactly this moment and
/// at no other: nothing has run yet, so the property is still the one just defined.
pub(crate) fn constructor_of(heap: &mut Heap, realm: &Realm) -> Option<ObjectId> {
    match super::own_value(heap, realm.global(), "Promise") {
        Some(Value::Object(id)) => Some(id),
        _ => None,
    }
}

/// `get [Symbol.species]` — §27.2.4.7, and the same one-liner §23.1.2.5 has.
fn species_getter(_: &mut Vm, _: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(call.this_value)
}

/// §27.2.4.7 — an **accessor** and not a value, which is observable: `Promise[Symbol.species]`
/// answers whatever it is read on, so a subclass reading it inherits the getter and gets itself.
fn define_species_getter(heap: &mut Heap, realm: &Realm, constructor: ObjectId) {
    let Some(species) = realm.well_known(super::well_known_at("species")) else {
        return;
    };
    let getter = heap.new_native_function(realm.function_prototype(), species_getter);
    super::define_function_metadata(heap, getter, "get [Symbol.species]", 0);
    let _ = heap.define_own_property(
        constructor,
        PropertyKey::from_symbol(species),
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §27.2.5.5 — `Promise.prototype[@@toStringTag]`, which is what makes
/// `Object.prototype.toString.call(Promise.resolve())` say `[object Promise]`.
fn define_tag(heap: &mut Heap, realm: &Realm, prototype: ObjectId) {
    let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) else {
        return;
    };
    let units: Vec<u16> = "Promise".encode_utf16().collect();
    let value = Value::String(heap.intern(&units));
    let _ = heap.define_own_property(
        prototype,
        PropertyKey::from_symbol(symbol),
        &PropertyDescriptor {
            value: Some(value),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §27.2.3.1 — `new Promise(executor)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — a plain call is a TypeError. Not because the body would misbehave, but because
    // there would be no `new.target` to take a prototype from, and §27.2.3.1 says so outright.
    if !call.constructing() {
        return Err(Abrupt::type_error("Promise must be called with new"));
    }
    let executor = call.argument(0);
    // Step 2 — checked *before* the promise is made, so a bad executor leaves nothing behind.
    if !heap.is_callable(executor) {
        return Err(Abrupt::type_error("the Promise executor is not a function"));
    }
    let prototype = super::prototype_from(heap, call, vm.realm().promise_prototype());
    let promise = heap.new_promise(Some(prototype));
    let (resolve, reject) = resolving_functions(heap, vm, promise);

    // Steps 8 and 9 — the executor runs **now**, synchronously, and a throw from it rejects the
    // promise rather than escaping. `reject` checks `[[AlreadyResolved]]` itself, so an executor
    // that resolved and *then* threw is a resolved promise and not a rejected one.
    if let Err(abrupt) = vm.call_value(executor, Value::Undefined, &[resolve, reject], heap) {
        let reason = vm.thrown_value(abrupt, heap);
        vm.call_value(reject, Value::Undefined, &[reason], heap)?;
    }
    Ok(Value::Object(promise))
}

/// §27.2.1.3 `CreateResolvingFunctions` — the pair a promise is settled through.
///
/// Public to the crate because §27.2.2.2's job needs a second pair for the same promise, which is
/// the one place the specification makes two: a thenable is handed functions that settle *this*
/// promise, and `[[AlreadyResolved]]` is what stops the second pair contradicting the first.
pub(crate) fn resolving_functions(heap: &mut Heap, vm: &Vm, promise: ObjectId) -> (Value, Value) {
    // One [`Settler`] and two functions holding it, which is what makes the flag shared: whichever
    // of them is called first settles the promise and the other finds nothing left to do.
    let settler = Settler::new(promise);
    let prototype = vm.realm().function_prototype();
    let mut make = |native: Native, role: Role| {
        let function = heap.new_native_function(prototype, native);
        super::define_function_metadata(heap, function, "", 1);
        if let Some(object) = heap.object_mut(function) {
            object.set_role(role);
        }
        Value::Object(function)
    };
    (
        make(resolve_function, Role::Resolve(settler.clone())),
        make(reject_function, Role::Reject(settler)),
    )
}

/// The pair state a resolving function carries instead of a closure.
fn settler_of(heap: &Heap, call: &NativeCall<'_>) -> Option<Settler> {
    match heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
    {
        Some(Role::Resolve(settler) | Role::Reject(settler)) => Some(settler.clone()),
        _ => None,
    }
}

/// §27.2.1.3.2 — the resolve function.
fn resolve_function(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // A function object with no `[[Promise]]` cannot be reached from any program: these are made
    // in one place and handed straight to the executor. Answering `undefined` rather than
    // asserting keeps the built-in total for a chunk that was written by hand.
    let Some(settler) = settler_of(heap, call) else {
        return Ok(Value::Undefined);
    };
    // Steps 3 and 4 — once, and only once, and *before* anything below can change the promise.
    if !settler.claim() {
        return Ok(Value::Undefined);
    }
    resolve_promise(vm, heap, settler.promise, call.argument(0))
}

/// §27.2.1.3.2 from step 5 onwards, with the claim already made by the caller.
///
/// Separate from the claim because the two happen at different moments: the claim is what the
/// *function* does, and this is what resolving *is* — §27.2.2.2's second pair claims its own flag
/// and then arrives here for the same work.
pub(crate) fn resolve_promise(
    vm: &mut Vm,
    heap: &mut Heap,
    promise: ObjectId,
    resolution: Value,
) -> Completion<Value> {
    // Step 5 — resolving a promise with itself is a TypeError, because the alternative is a
    // promise waiting for itself for ever. The self-check is `SameValue`, so it is identity.
    if matches!(resolution, Value::Object(id) if id == promise) {
        let reason = vm.thrown_value(
            Abrupt::type_error("a promise cannot be resolved with itself"),
            heap,
        );
        return reject_promise(vm, heap, promise, reason);
    }
    // Step 6 — anything that is not an object cannot be a thenable, so it is the answer itself.
    let Value::Object(object) = resolution else {
        return fulfil_promise(vm, heap, promise, resolution);
    };
    // Steps 7 and 8 — `then` is *read*, once, and a getter that throws rejects the promise. This
    // is the line that makes a promise adopt another one's answer rather than holding it.
    let then = match vm.get_property_key(Value::Object(object), key(heap, "then"), heap) {
        Ok(then) => then,
        Err(abrupt) => {
            let reason = vm.thrown_value(abrupt, heap);
            return reject_promise(vm, heap, promise, reason);
        }
    };
    // Step 9 — an object whose `then` is not callable is an ordinary value, `Promise` or not.
    if !heap.is_callable(then) {
        return fulfil_promise(vm, heap, promise, resolution);
    }
    // Steps 10 and 11 — a *job*, not a call. What that buys is ordering: the `then` of a thenable
    // runs after the statement that resolved with it has finished, so a program cannot observe its
    // own resolution half-done.
    vm.enqueue(Job::ResolveThenable {
        promise,
        thenable: resolution,
        then,
    });
    Ok(Value::Undefined)
}

/// §27.2.1.3.1 — the reject function.
fn reject_function(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Some(settler) = settler_of(heap, call) else {
        return Ok(Value::Undefined);
    };
    if !settler.claim() {
        return Ok(Value::Undefined);
    }
    reject_promise(vm, heap, settler.promise, call.argument(0))
}

/// §27.2.1.4 `FulfillPromise`.
fn fulfil_promise(
    vm: &mut Vm,
    heap: &mut Heap,
    promise: ObjectId,
    value: Value,
) -> Completion<Value> {
    settle(vm, heap, promise, value, ReactionKind::Fulfil)
}

/// §27.2.1.7 `RejectPromise`.
pub(crate) fn reject_promise(
    vm: &mut Vm,
    heap: &mut Heap,
    promise: ObjectId,
    reason: Value,
) -> Completion<Value> {
    settle(vm, heap, promise, reason, ReactionKind::Reject)
}

/// The half of §27.2.1.4 and §27.2.1.7 that is the same in both, which is all of it but the state.
///
/// **Both** reaction lists are cleared, not just the one that ran: §27.2.1.4 step 5 says so, and
/// the reason is that a settled promise can never run the other half, so keeping it would be a
/// list of callbacks nothing will ever call and everything they hold kept alive with it.
fn settle(
    vm: &mut Vm,
    heap: &mut Heap,
    promise: ObjectId,
    value: Value,
    kind: ReactionKind,
) -> Completion<Value> {
    let Some(state) = heap.promise_mut(promise) else {
        return Ok(Value::Undefined);
    };
    let reactions = match kind {
        ReactionKind::Fulfil => std::mem::take(&mut state.fulfil),
        ReactionKind::Reject => std::mem::take(&mut state.reject),
    };
    match kind {
        ReactionKind::Fulfil => state.reject.clear(),
        ReactionKind::Reject => state.fulfil.clear(),
    }
    state.result = value;
    state.state = match kind {
        ReactionKind::Fulfil => PromiseState::Fulfilled,
        ReactionKind::Reject => PromiseState::Rejected,
    };
    // §27.2.1.8 `TriggerPromiseReactions` — a job each, in list order, which is what makes two
    // `then`s on one promise run in the order they were written.
    for reaction in reactions {
        vm.enqueue(Job::Reaction {
            reaction,
            argument: value,
        });
    }
    Ok(Value::Undefined)
}

/// §27.2.5.4 — `then`.
fn then(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(promise) = call.this_value else {
        return Err(Abrupt::type_error(
            "then was called on something that is not a promise",
        ));
    };
    if heap.promise(promise).is_none() {
        return Err(Abrupt::type_error(
            "then was called on something that is not a promise",
        ));
    }
    // Step 3 — `SpeciesConstructor`, so a subclass gets its own kind back and a program that
    // replaced `constructor` gets what it put there. Read before anything is added to a list.
    let default = vm.realm().promise_constructor();
    let species = species_constructor(vm, heap, promise, default)?;
    let capability = new_promise_capability(vm, heap, species)?;
    perform_then(
        vm,
        heap,
        promise,
        call.argument(0),
        call.argument(1),
        Some(capability),
    )?;
    Ok(capability.promise)
}

/// §27.2.5.4.1 `PerformPromiseThen` — the part `then` and `await` share.
pub(crate) fn perform_then(
    vm: &mut Vm,
    heap: &mut Heap,
    promise: ObjectId,
    on_fulfilled: Value,
    on_rejected: Value,
    capability: Option<Capability>,
) -> Completion<Value> {
    // Steps 3 and 4 — anything that is not callable makes the handler **empty**, which is not the
    // same as a handler that returns its argument: an empty one keeps a rejection a rejection.
    let handler = |value| heap.is_callable(value).then_some(value);
    let fulfil = Reaction {
        capability,
        kind: ReactionKind::Fulfil,
        handler: handler(on_fulfilled),
    };
    let reject = Reaction {
        capability,
        kind: ReactionKind::Reject,
        handler: handler(on_rejected),
    };
    let Some(state) = heap.promise_mut(promise) else {
        return Ok(Value::Undefined);
    };
    let settled = state.state;
    let result = state.result;
    match settled {
        // Step 9 — still waiting, so the reactions join the lists and nothing runs yet.
        PromiseState::Pending => {
            state.fulfil.push(fulfil);
            state.reject.push(reject);
        }
        // Steps 10 and 11 — already settled, so the job is enqueued now. Enqueued rather than
        // called: `Promise.resolve(1).then(f)` runs `f` after the current script and not during
        // it, which is the guarantee that makes a promise's timing worth anything.
        PromiseState::Fulfilled => vm.enqueue(Job::Reaction {
            reaction: fulfil,
            argument: result,
        }),
        PromiseState::Rejected => vm.enqueue(Job::Reaction {
            reaction: reject,
            argument: result,
        }),
    }
    // Step 12 sets `[[PromiseIsHandled]]`, which this engine does not keep: nothing reads it
    // without `HostPromiseRejectionTracker`, and praxis has no such hook. See `heap::promise`.
    Ok(Value::Undefined)
}

/// §27.2.5.1 — `catch(f)`, which is `then(undefined, f)` and is specified as exactly that.
fn catch(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Through `Invoke`, not through the `then` above: §27.2.5.1 step 2 calls the `then` *property*,
    // so a promise whose `then` a program replaced uses the replacement. That is observable and is
    // the reason `catch` is not written as a second copy of `then`.
    let then = vm.get_property_key(call.this_value, key(heap, "then"), heap)?;
    vm.call_value(
        then,
        call.this_value,
        &[Value::Undefined, call.argument(0)],
        heap,
    )
}

/// §27.2.5.3 — `finally(f)`.
fn finally(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(promise) = call.this_value else {
        return Err(Abrupt::type_error(
            "finally was called on something that is not an object",
        ));
    };
    // Step 4 — the constructor is read *before* the handler is looked at, so a `@@species` that
    // throws throws even for `finally(null)`.
    let default = vm.realm().promise_constructor();
    let species = species_constructor(vm, heap, promise, default)?;
    let handler = call.argument(0);
    let then = vm.get_property_key(call.this_value, key(heap, "then"), heap)?;
    // Step 6 — a handler that is not callable is passed to `then` **as both arguments**, where it
    // is not callable either and so makes both reactions empty. The value and the reason travel on
    // unchanged, which is what `finally(null)` should do and does.
    if !heap.is_callable(handler) {
        return vm.call_value(then, call.this_value, &[handler, handler], heap);
    }
    // §27.2.5.3.1 and §27.2.5.3.2 — one wrapper each, both holding the handler and the constructor.
    let constructor = Value::Object(species);
    let on_fulfilled = wrapper(
        vm,
        heap,
        finally_fulfilled,
        Role::Finally {
            handler,
            constructor,
        },
    );
    let on_rejected = wrapper(
        vm,
        heap,
        finally_rejected,
        Role::Finally {
            handler,
            constructor,
        },
    );
    vm.call_value(then, call.this_value, &[on_fulfilled, on_rejected], heap)
}

/// A one-argument built-in carrying `role` where the specification writes a captured variable.
fn wrapper(vm: &Vm, heap: &mut Heap, native: Native, role: Role) -> Value {
    let function = heap.new_native_function(vm.realm().function_prototype(), native);
    super::define_function_metadata(heap, function, "", 1);
    if let Some(object) = heap.object_mut(function) {
        object.set_role(role);
    }
    Value::Object(function)
}

/// §27.2.5.3.1 — the fulfilment wrapper `finally` puts on.
///
/// The shape that matters is steps 5 to 7: the handler's own answer is put through
/// `PromiseResolve` and *waited for*, and only then is the original value handed on. So a
/// `finally` whose handler returns a promise delays the chain until that promise settles, and a
/// `finally` whose handler returns a plain value still costs a turn of the queue. Answering the
/// value directly would be right about the value and wrong about every ordering test there is.
fn finally_fulfilled(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    finally_step(vm, heap, call, ReactionKind::Fulfil)
}

/// §27.2.5.3.2 — the rejection wrapper, whose thunk **throws** the reason on rather than answering.
fn finally_rejected(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    finally_step(vm, heap, call, ReactionKind::Reject)
}

/// The body both wrappers share, differing only in what the thunk does with the value.
fn finally_step(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: ReactionKind,
) -> Completion<Value> {
    let Some(Role::Finally {
        handler,
        constructor,
    }) = heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
        .cloned()
    else {
        return Ok(Value::Undefined);
    };
    let settled = call.argument(0);
    // Step 3 — the handler is called with no arguments and its answer is not the chain's answer.
    let result = vm.call_value(handler, Value::Undefined, &[], heap)?;
    let Value::Object(species) = constructor else {
        return Err(Abrupt::type_error("the promise species is not an object"));
    };
    let waiting = promise_resolve(vm, heap, species, result)?;
    let thunk = match kind {
        ReactionKind::Fulfil => wrapper(vm, heap, thunk_value, Role::Thunk(settled)),
        ReactionKind::Reject => wrapper(vm, heap, thunk_throw, Role::Thrower(settled)),
    };
    // Step 7 — through the `then` *property*, so a promise whose `then` was replaced uses the
    // replacement here as it does everywhere else.
    let then = vm.get_property_key(waiting, key(heap, "then"), heap)?;
    vm.call_value(then, waiting, &[thunk], heap)
}

/// §27.2.5.3.1 step 6's thunk — the original value, whatever it is called with.
fn thunk_value(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    match heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
    {
        Some(Role::Thunk(value)) => Ok(*value),
        _ => Ok(Value::Undefined),
    }
}

/// §27.2.5.3.2 step 6's thrower — the original reason, thrown on so the chain stays rejected.
fn thunk_throw(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    match heap
        .object(call.function)
        .and_then(crate::heap::Object::role)
    {
        Some(Role::Thrower(reason)) => Err(Abrupt::Thrown(*reason)),
        _ => Ok(Value::Undefined),
    }
}

/// §27.2.4.6.1 `PromiseResolve` — a promise of `constructor`'s kind for a value that may be one.
///
/// The identity in step 2 is observable and programs rely on it: a promise of exactly this kind is
/// handed straight back, so `Promise.resolve(p) === p` and a chain does not grow a turn for
/// nothing.
fn promise_resolve(
    vm: &mut Vm,
    heap: &mut Heap,
    constructor: ObjectId,
    value: Value,
) -> Completion<Value> {
    if let Value::Object(id) = value
        && heap.promise(id).is_some()
    {
        let its = vm.get_property_key(value, key(heap, "constructor"), heap)?;
        if matches!(its, Value::Object(other) if other == constructor) {
            return Ok(value);
        }
    }
    let capability = new_promise_capability(vm, heap, constructor)?;
    vm.settle_capability(capability, ReactionKind::Fulfil, value, heap)?;
    Ok(capability.promise)
}

/// §27.2.4.6 — `Promise.resolve(x)`.
fn resolve_static(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(constructor) = call.this_value else {
        return Err(Abrupt::type_error(
            "Promise.resolve was called on something that is not an object",
        ));
    };
    promise_resolve(vm, heap, constructor, call.argument(0))
}

/// §27.2.4.5 — `Promise.reject(r)`, which does **not** have the shortcut above: a rejected promise
/// handed to it is wrapped in another one, because a reason is a reason whatever its type.
fn reject_static(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(constructor) = call.this_value else {
        return Err(Abrupt::type_error(
            "Promise.reject was called on something that is not an object",
        ));
    };
    let capability = new_promise_capability(vm, heap, constructor)?;
    vm.settle_capability(capability, ReactionKind::Reject, call.argument(0), heap)?;
    Ok(capability.promise)
}

/// §27.2.1.5 `NewPromiseCapability` — a promise of `constructor`'s kind, and its two functions.
///
/// The general path, and it goes through the constructor even when that constructor is `%Promise%`.
/// Making the promise directly for the common case would be faster and would skip the `executor`
/// call — which a subclass can observe, and which §27.2.1.5 step 5 requires to have happened
/// *before* step 6 reads the functions back out.
pub(crate) fn new_promise_capability(
    vm: &mut Vm,
    heap: &mut Heap,
    constructor: ObjectId,
) -> Completion<Capability> {
    if !heap.is_constructor(Value::Object(constructor)) {
        return Err(Abrupt::type_error(
            "a promise capability needs a constructor",
        ));
    }
    // Steps 4 and 5 — the executor is an ordinary function object whose `[[Capability]]` is where
    // the constructor's two arguments end up. §27.2.1.5.1 step 2 refuses a second call, which is
    // what stops a constructor handing out two pairs for one promise.
    let executor = heap.new_native_function(vm.realm().function_prototype(), capabilities_executor);
    super::define_function_metadata(heap, executor, "", 2);
    if let Some(object) = heap.object_mut(executor) {
        object.set_role(Role::Executor {
            resolve: Value::Undefined,
            reject: Value::Undefined,
        });
    }
    let promise =
        vm.construct_value(Value::Object(constructor), &[Value::Object(executor)], heap)?;
    let Some(Role::Executor { resolve, reject }) =
        heap.object(executor).and_then(crate::heap::Object::role)
    else {
        return Err(Abrupt::type_error(
            "the promise executor lost its capability",
        ));
    };
    let (resolve, reject) = (*resolve, *reject);
    // Step 6 — a constructor that never called its executor, or called it with something that is
    // not a function, does not produce a capability. `Promise` itself always does; this is about
    // the subclass that does not.
    if !heap.is_callable(resolve) || !heap.is_callable(reject) {
        return Err(Abrupt::type_error(
            "the promise constructor did not supply resolve and reject functions",
        ));
    }
    Ok(Capability {
        promise,
        resolve,
        reject,
    })
}

/// §27.2.1.5.1 — the executor a capability is built with, which only records its arguments.
fn capabilities_executor(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (first, second) = (call.argument(0), call.argument(1));
    let Some(object) = heap.object_mut(call.function) else {
        return Ok(Value::Undefined);
    };
    // Step 2 — both halves must still be `undefined`, so a constructor that calls its executor
    // twice is a TypeError rather than a capability that quietly forgot its first pair.
    match object.role_mut() {
        Some(Role::Executor { resolve, reject })
            if matches!(resolve, Value::Undefined) && matches!(reject, Value::Undefined) =>
        {
            *resolve = first;
            *reject = second;
            Ok(Value::Undefined)
        }
        _ => Err(Abrupt::type_error(
            "the promise executor was called more than once",
        )),
    }
}

/// §7.3.22 `SpeciesConstructor` — what kind of promise `then` should answer with.
fn species_constructor(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    default: ObjectId,
) -> Completion<ObjectId> {
    let constructor = vm.get_property_key(Value::Object(object), key(heap, "constructor"), heap)?;
    // Step 2 — no `constructor` at all means the default, which is what an object with a null
    // prototype gets.
    if matches!(constructor, Value::Undefined) {
        return Ok(default);
    }
    let Value::Object(_) = constructor else {
        return Err(Abrupt::type_error("constructor is not an object"));
    };
    let Some(species) = vm.realm().well_known(super::well_known_at("species")) else {
        return Ok(default);
    };
    let chosen = vm.get_property_key(constructor, PropertyKey::from_symbol(species), heap)?;
    // Step 5 — `undefined` **or null** means the default. Two spellings of "I have no opinion",
    // and only one of them is the obvious one.
    if matches!(chosen, Value::Undefined | Value::Null) {
        return Ok(default);
    }
    match chosen {
        Value::Object(id) if heap.is_constructor(chosen) => Ok(id),
        _ => Err(Abrupt::type_error(
            "the species of this constructor is not a constructor",
        )),
    }
}
