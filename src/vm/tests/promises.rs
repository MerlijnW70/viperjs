//! §27.2 — `Promise`, and the ordering that is the whole point of it.
//!
//! Every row that matters here is about *when*, not *what*. A promise's value is easy and an
//! implementation that gets only that right passes a surprising number of tests; the reason the
//! specification is as long as it is has to do with the order things happen in, and that is what
//! these check.
//!
//! `run_settled` runs a script, lets §9.5's jobs run, and then asks a second script what happened.
//! It has to: a script's completion value is decided by its last statement, which is before any
//! job has run at all.

use super::*;

#[test]
fn a_then_handler_runs_after_the_script_and_not_during_it() {
    // The one guarantee a promise makes about time. `Promise.resolve(1)` is *already* fulfilled and
    // its handler still does not run until the script is over — §27.2.5.4.1 step 10 enqueues a job
    // for an already-settled promise rather than calling the handler, precisely so that a program
    // cannot tell a promise that was settled early from one that was settled late.
    assert_eq!(
        run_settled(
            "var log = ''; Promise.resolve(1).then(function (v) { log += 'then' + v; }); \
                     log += 'sync';",
            "log"
        ),
        "syncthen1"
    );
    // The executor, by contrast, runs **now** — §27.2.3.1 step 8 calls it before the constructor
    // has returned, which is why a promise can be resolved before anything can hold it.
    assert_eq!(
        run_settled(
            "var log = ''; new Promise(function (r) { log += 'exec'; r(1); }); log += '|sync';",
            "log"
        ),
        "exec|sync"
    );
}

#[test]
fn handlers_run_in_the_order_they_were_written() {
    // §27.2.1.8 enqueues one job per reaction in list order, and §9.5's queue is a queue — so two
    // `then`s on one promise run in the order they were added, and two promises' handlers
    // interleave by when each was *registered* rather than by which promise settled first.
    assert_eq!(
        run_settled(
            "var log = ''; var p = Promise.resolve(1); \
             p.then(function () { log += 'a'; }); p.then(function () { log += 'b'; }); \
             Promise.resolve(2).then(function () { log += 'c'; });",
            "log"
        ),
        "abc"
    );
    // A job enqueued *by* a job goes on the end, which is what makes a chain interleave with
    // everything else rather than running to completion first. `first.then` and `second.then` are
    // both registered now; `first`'s second link is registered only when its first link has run.
    assert_eq!(
        run_settled(
            "var log = ''; \
             Promise.resolve().then(function () { log += '1'; }).then(function () { log += '3'; }); \
             Promise.resolve().then(function () { log += '2'; }).then(function () { log += '4'; });",
            "log"
        ),
        "1234"
    );
}

#[test]
fn a_handler_answers_the_promise_that_then_gave_back() {
    // §27.2.5.4 answers with a *new* promise, and what the handler returns settles it — which is
    // what makes a chain a chain rather than a list of independent callbacks.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve(1).then(function (v) { return v + 1; }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "2"
    );
    // …and it is a different promise from the one it was called on, every time.
    assert_eq!(
        run_settled(
            "var p = Promise.resolve(1); var q = p.then(function () {}); var same = p === q;",
            "same"
        ),
        "false"
    );
    // A handler that **throws** rejects that promise rather than escaping to the script, which has
    // finished. This is the row that says a job's completion is not the script's business.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve(1).then(function () { throw 'thrown'; }) \
             .catch(function (e) { out = 'caught ' + e; });",
            "out"
        ),
        "caught thrown"
    );
}

#[test]
fn a_handler_that_is_not_a_function_passes_the_answer_along_unchanged() {
    // §27.2.5.4.1 steps 3 and 4 — a non-callable handler makes the reaction's `[[Handler]]`
    // **empty**, and an empty handler passes the argument through *with its type intact*. So a
    // fulfilment travels down a chain of `catch`es and a rejection travels down a chain of `then`s
    // until something takes it. Treating a missing handler as `function (v) { return v; }` would
    // turn a rejection into a fulfilment at the first `then` it passed.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve('v').catch(function () { return 'wrong'; }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "v"
    );
    assert_eq!(
        run_settled(
            "var out; Promise.reject('r').then(function () { return 'wrong'; }) \
             .catch(function (e) { out = e; });",
            "out"
        ),
        "r"
    );
    // The same for anything else that is not callable, which §7.2.3 makes the only test.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve('v').then(null).then(undefined).then(7) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "v"
    );
}

#[test]
fn resolving_with_a_thenable_adopts_its_answer_instead_of_holding_it() {
    // §27.2.1.3.2 steps 7 to 11 — the difference between *resolve* and *fulfil*, and the one that
    // an implementation is most likely to get wrong by doing the obvious thing. A promise resolved
    // with something that has a callable `then` is not fulfilled with it: it waits for it.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (r) { r(Promise.resolve('inner')); }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "inner"
    );
    // Any object with a callable `then` will do — §27.2.1.3.2 asks about the property and not
    // about the kind, which is what makes promises from different libraries interoperate at all.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (r) { r({ then: function (res) { res('duck'); } }); }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "duck"
    );
    // …and a `then` that is not callable makes it an ordinary value, object or not.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (r) { r({ then: 7 }); }) \
             .then(function (v) { out = typeof v + ',' + v.then; });",
            "out"
        ),
        "object,7"
    );
    // A `then` that throws rejects the promise, because reading and calling it are both part of
    // resolving and a resolution that cannot be carried out is a rejection.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (r) { r({ then: function () { throw 'bad'; } }); }) \
             .catch(function (e) { out = e; });",
            "out"
        ),
        "bad"
    );
    // Adoption costs **exactly two** turns of the queue, and the number is observable — it is the
    // thing people mean when they say resolving with a promise is slower than fulfilling with a
    // value. One job to call the inner promise's `then` (§27.2.2.2), and one for the reaction that
    // `then` registered; only then is the outer promise fulfilled and its own handler enqueued.
    //
    // So against a plain chain started at the same moment: `1` runs while the first of those two
    // is in flight, `2` while the second is, and `adopted` lands between `2` and `3`. Counting it
    // as one turn or three would put it in a different place, which is why the row spells out four
    // handlers rather than asserting that it is "late".
    assert_eq!(
        run_settled(
            "var log = ''; \
             new Promise(function (r) { r(Promise.resolve()); }).then(function () { log += 'adopted'; }); \
             Promise.resolve().then(function () { log += '1'; }) \
             .then(function () { log += '2'; }).then(function () { log += '3'; });",
            "log"
        ),
        "12adopted3"
    );
}

#[test]
fn a_promise_cannot_be_resolved_with_itself_and_cannot_be_settled_twice() {
    // §27.2.1.3.2 step 5 — resolving a promise with itself is a TypeError, because the alternative
    // is a promise waiting for its own answer for ever. The check is identity.
    assert_eq!(
        run_settled(
            "var out; var p = new Promise(function (r) { setTimeoutIsNotAThing = r; }); \
             setTimeoutIsNotAThing(p); p.catch(function (e) { out = e.constructor.name; });",
            "out"
        ),
        "TypeError"
    );
    // §27.2.1.3 steps 3 and 4 — `[[AlreadyResolved]]`, which makes an answer final. The second
    // call does nothing at all, and neither does a `reject` after a `resolve`.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (res) { res('first'); res('second'); }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "first"
    );
    assert_eq!(
        run_settled(
            "var out; new Promise(function (res, rej) { res('kept'); rej('ignored'); }) \
             .then(function (v) { out = 'value ' + v; }, function (e) { out = 'reason ' + e; });",
            "out"
        ),
        "value kept"
    );
    // …including an executor that throws *after* resolving: §27.2.3.1 step 9 calls reject, and
    // reject finds the promise already resolved and does nothing.
    assert_eq!(
        run_settled(
            "var out; new Promise(function (res) { res('kept'); throw 'late'; }) \
             .then(function (v) { out = 'value ' + v; }, function (e) { out = 'reason ' + e; });",
            "out"
        ),
        "value kept"
    );
    // An executor that throws *before* resolving rejects it, which is the same step from the other
    // side and is how a constructor reports a failure without the script seeing a throw.
    assert_eq!(
        run_settled(
            "var out; new Promise(function () { throw 'early'; }).catch(function (e) { out = e; });",
            "out"
        ),
        "early"
    );
}

#[test]
fn finally_runs_its_handler_without_changing_the_answer() {
    // §27.2.5.3 — what `finally` is *for*: the value and the reason go past it untouched, which is
    // exactly what `then(f, f)` cannot do.
    assert_eq!(
        run_settled(
            "var log = ''; Promise.resolve('v').finally(function () { log += 'fin,'; }) \
             .then(function (v) { log += 'value ' + v; });",
            "log"
        ),
        "fin,value v"
    );
    assert_eq!(
        run_settled(
            "var log = ''; Promise.reject('r').finally(function () { log += 'fin,'; }) \
             .catch(function (e) { log += 'reason ' + e; });",
            "log"
        ),
        "fin,reason r"
    );
    // …and what it answers is ignored, which is the other half of the same promise.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve('kept').finally(function () { return 'discarded'; }) \
             .then(function (v) { out = v; });",
            "out"
        ),
        "kept"
    );
    // A handler that **throws** does replace the answer, because a throw is not a return: it is
    // the one way a `finally` gets to have an opinion.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve('kept').finally(function () { throw 'raised'; }) \
             .catch(function (e) { out = e; });",
            "out"
        ),
        "raised"
    );
    // A handler that is not callable is passed to `then` as both arguments, where it makes two
    // empty reactions — so the chain is untouched rather than broken.
    assert_eq!(
        run_settled(
            "var out; Promise.resolve('v').finally(null).then(function (v) { out = v; });",
            "out"
        ),
        "v"
    );
    // §27.2.5.3.1 steps 5 to 7 — the handler's own answer goes through `PromiseResolve` and is
    // *waited for*. So a `finally` whose handler returns a promise holds the chain until it
    // settles, which is the whole reason those steps are there.
    assert_eq!(
        run_settled(
            "var log = ''; var release; \
             var gate = new Promise(function (r) { release = r; }); \
             Promise.resolve('v').finally(function () { return gate; }) \
             .then(function (v) { log += 'arrived ' + v; }); \
             Promise.resolve().then(function () { log += '1,'; }) \
             .then(function () { log += '2,'; release(); });",
            "log"
        ),
        "1,2,arrived v"
    );
}

#[test]
fn a_promise_is_an_ordinary_object_with_the_properties_the_specification_names() {
    assert_eq!(run("typeof Promise"), "function");
    assert_eq!(run("Promise.length"), "1");
    assert_eq!(run("Promise.name"), "Promise");
    assert_eq!(run("Promise.prototype.then.length"), "2");
    assert_eq!(run("Promise.prototype.catch.length"), "1");
    assert_eq!(run("Promise.prototype.finally.length"), "1");
    // §27.2.5.2 — the link `SpeciesConstructor` reads.
    assert_eq!(run("Promise.prototype.constructor === Promise"), "true");
    assert_eq!(run("Promise.resolve(1) instanceof Promise"), "true");
    // §27.2.5.5 — the tag, which is the only thing that tells a promise from any other object
    // through `Object.prototype.toString`.
    assert_eq!(
        run("Object.prototype.toString.call(Promise.resolve())"),
        "[object Promise]"
    );
    // §27.2.4.7 — an accessor answering the receiver, so a subclass inherits it and gets itself.
    assert_eq!(run("Promise[Symbol.species] === Promise"), "true");
    // §27.2.3.1 step 1 — a plain call has no `new.target` to take a prototype from, and says so.
    assert_eq!(
        run("try { Promise(function () {}); } catch (e) { e.constructor.name; }"),
        "TypeError"
    );
    // …and step 2, which is checked before anything is allocated.
    assert_eq!(
        run("try { new Promise(1); } catch (e) { e.constructor.name; }"),
        "TypeError"
    );
    // §27.2.5.4 step 2 — `then` requires an actual promise, so borrowing it does not work.
    assert_eq!(
        run(
            "try { Promise.prototype.then.call({}, function () {}); } catch (e) { e.constructor.name; }"
        ),
        "TypeError"
    );
}

#[test]
fn the_two_symbol_properties_have_the_attributes_the_specification_gives_them() {
    // §17's convention is one thing and these are not it, which is why they are worth pinning: a
    // built-in *method* is writable, and both of these are not — one because it is an accessor with
    // no setter, the other because §27.2.5.5 says `{ [[Writable]]: false }` outright.
    //
    // Every one of them is a thing a program can detect and several polyfills read.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Promise, Symbol.species); \
             (typeof d.get) + ',' + (d.set === undefined) + ',' + d.enumerable + ',' + d.configurable"
        ),
        "function,true,false,true"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Promise, Symbol.species).get.name"),
        "get [Symbol.species]"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Promise.prototype, Symbol.toStringTag); \
             d.value + ',' + d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "Promise,false,false,true"
    );
    // Configurable is what makes both of them replaceable, which is the whole reason a
    // specification ever says so: a program may take `@@toStringTag` off and change what
    // `Object.prototype.toString` answers.
    assert_eq!(
        run("delete Promise.prototype[Symbol.toStringTag]; \
             Object.prototype.toString.call(Promise.resolve())"),
        "[object Object]"
    );
}

#[test]
fn a_capability_needs_a_constructor_that_supplies_both_of_its_functions() {
    // §27.2.1.5 — the three ways building a capability fails, each of which is a subclass
    // misbehaving rather than a program doing something odd. They are separate checks because they
    // are separate mistakes, and an engine that made one error message for all three would be
    // unhelpful in exactly the case where the answer is not obvious.
    //
    // Step 1 — not a constructor at all. The *message* is what this row is about: without the
    // check the construction below fails anyway, because nothing can be constructed from a plain
    // object — so the guard does not change whether this throws, only whether the program is told
    // what was actually wrong. "not a function" would be true and useless; the thing handed over
    // was a function often enough that the useful sentence is the one about constructors.
    assert_eq!(
        run(
            "try { Promise.resolve.call({}, 1); } catch (e) { e.constructor.name + ': ' + e.message; }"
        ),
        "TypeError: a promise capability needs a constructor"
    );
    // Step 6, both halves missing — a constructor that never called the executor it was given.
    assert_eq!(
        run(
            "class P extends Promise { constructor() { super(function () {}); } } \
             try { P.resolve(1); } catch (e) { e.constructor.name; }"
        ),
        "TypeError"
    );
    // …and one half missing, which is the same refusal for a different reason: a constructor that
    // called the executor with a resolve and no reject leaves a capability that could never be
    // rejected, and §27.2.1.5 would rather say so now than fail silently later.
    assert_eq!(
        run(
            "class P extends Promise { constructor(ex) { super(function (res) { ex(res); }); } } \
             try { P.resolve(1); } catch (e) { e.constructor.name; }"
        ),
        "TypeError"
    );
    // §27.2.1.5.1 step 2 — the executor refuses a *second* call, so a constructor cannot hand out
    // two pairs for one promise and leave the capability holding whichever arrived last.
    assert_eq!(
        run("class P extends Promise { \
               constructor(ex) { super(function (res, rej) { ex(res); ex(res, rej); }); } } \
             try { P.resolve(1); } catch (e) { e.constructor.name; }"),
        "TypeError"
    );
}

#[test]
fn species_falls_back_to_promise_when_nothing_has_an_opinion() {
    // §7.3.22 steps 2 and 5 — **two** ways of saying "no opinion", and they are two because they
    // arise differently: no `constructor` at all is an object whose prototype chain was cut, and a
    // `@@species` of `undefined` or `null` is a subclass deliberately declining to be inherited.
    // Falling through to the type check instead of defaulting would make each of them a TypeError.
    assert_eq!(
        run_settled(
            "var out; var p = Promise.resolve(1); p.constructor = undefined; \
             var q = p.then(function () {}); out = q instanceof Promise;",
            "out"
        ),
        "true"
    );
    assert_eq!(
        run(
            "class P extends Promise { static get [Symbol.species]() { return undefined; } } \
             P.resolve(1).then(function () {}) instanceof Promise"
        ),
        "true"
    );
    // §7.3.22 step 5 names `null` beside `undefined`, and only one of the two is obvious.
    assert_eq!(
        run(
            "class P extends Promise { static get [Symbol.species]() { return null; } } \
             P.resolve(1).then(function () {}) instanceof Promise"
        ),
        "true"
    );
    // Anything else must actually be a constructor — a species that is a plain object is a
    // TypeError rather than something to fall back from.
    assert_eq!(
        run(
            "class P extends Promise { static get [Symbol.species]() { return {}; } } \
             try { P.resolve(1).then(function () {}); } catch (e) { e.constructor.name; }"
        ),
        "TypeError"
    );
    // …and a `constructor` that is not an object is a TypeError too, which is the step between the
    // two: `undefined` defaults, an object is asked, and a number is neither.
    assert_eq!(
        run("var p = Promise.resolve(1); p.constructor = 7; \
             try { p.then(function () {}); } catch (e) { e.constructor.name; }"),
        "TypeError"
    );
}

#[test]
fn promise_resolve_hands_back_a_promise_of_its_own_kind_unchanged() {
    // §27.2.4.6 step 3 — the identity is observable and programs rely on it: wrapping a promise
    // that is already the right kind would cost a turn of the queue for nothing.
    assert_eq!(
        run("var p = Promise.resolve(1); Promise.resolve(p) === p"),
        "true"
    );
    // …and only when the `constructor` matches, which is what the check actually reads.
    assert_eq!(
        run("var p = Promise.resolve(1); p.constructor = function () {}; Promise.resolve(p) === p"),
        "false"
    );
    // `Promise.reject` has no such shortcut, because a reason is a reason whatever its type — a
    // rejected promise handed to it becomes the *reason* of a new one rather than being adopted.
    assert_eq!(
        run_settled(
            "var out; var p = Promise.resolve('inner'); \
             Promise.reject(p).catch(function (e) { out = e === p; });",
            "out"
        ),
        "true"
    );
}

#[test]
fn then_answers_a_promise_of_the_kind_species_names() {
    // §27.2.5.4 step 3 — `SpeciesConstructor`, which is why a subclass gets its own kind back
    // from `then` rather than a plain promise.
    assert_eq!(
        run("class P extends Promise {} var p = P.resolve(1); p.then(function () {}) instanceof P"),
        "true"
    );
    // …and §27.2.1.5, which builds that promise by *calling the constructor* — so a subclass sees
    // its executor called, with two functions, exactly as `Promise` does.
    assert_eq!(
        run(
            "var seen = 0; class P extends Promise { constructor(ex) { seen++; super(ex); } } \
             P.resolve(1).then(function () {}); seen"
        ),
        "2"
    );
    // A `@@species` that names something else is what is used, which is the point of the hook.
    assert_eq!(
        run(
            "class P extends Promise { static get [Symbol.species]() { return Promise; } } \
             var p = P.resolve(1).then(function () {}); \
             (p instanceof P) + ',' + (p instanceof Promise)"
        ),
        "false,true"
    );
}
