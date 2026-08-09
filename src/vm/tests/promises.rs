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

/// Run `source`, let §9.5's jobs run, and describe every rejection nothing asked for.
///
/// The reasons rather than the promises, because a promise id is not something a row can read —
/// and the reason is the part a host reports. Joined with `|`, so a test can say both *how many*
/// and *which*, which are different claims and both are wrong in different ways.
fn unhandled(source: &str) -> String {
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script(source).expect("the source parses"); // the test is what is left over
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    let reasons: Vec<String> = vm
        .unhandled_rejections()
        .iter()
        .map(|promise| {
            let reason = heap
                .promise(*promise)
                .map_or(Value::Undefined, |found| found.result);
            describe(Outcome::Value(reason), &mut heap)
        })
        .collect();
    reasons.join("|")
}

#[test]
fn a_rejection_nothing_ever_asked_for_is_reported_to_the_host() {
    // §9.13 `HostPromiseRejectionTracker`, which ViperJS did not have and which is the only signal
    // the engine gives that a job drain stopped early — see DR-0029. A host reports these; the row
    // that matters to the engine is the last one here.
    assert_eq!(
        unhandled("Promise.reject('nobody wanted this');"),
        "nobody wanted this"
    );
    // A handler attached in a *later statement* takes the report back — §9.13's `"handle"`
    // operation. Without that every `var p = Promise.reject(1); p.catch(f)` would be reported,
    // which is two lines of ordinary JavaScript and would make the list worthless.
    assert_eq!(
        unhandled("var p = Promise.reject(1); p.catch(function () {});"),
        ""
    );
    // …and one attached *before* the rejection, which is the same claim from the other side: what
    // is asked is `[[PromiseIsHandled]]`, so the order of the two statements changes nothing.
    assert_eq!(
        unhandled("var d = Promise.withResolvers(); d.promise.catch(function () {}); d.reject(1);"),
        ""
    );
    // **`then` with no rejection handler still handles the promise**, and the rejection moves to
    // the promise `then` answered with — so the count stays one and does not become two. Reading
    // the slot as "has a rejection handler" would report every link of every chain.
    assert_eq!(
        unhandled("Promise.reject('moved').then(function () {});"),
        "moved"
    );
    // A fulfilment is never reported, whatever nothing is waiting for it.
    assert_eq!(unhandled("Promise.resolve('fine');"), "");
    // Two of them, in the order they were rejected, because a host printing them prints a program's
    // own order back to it.
    assert_eq!(
        unhandled("Promise.reject('first'); Promise.reject('second');"),
        "first|second"
    );
}

#[test]
fn a_job_that_runs_out_of_heap_leaves_the_rejection_that_says_so() {
    // The row DR-0029 exists for, and the shape that was silent: a promise chain re-arming itself
    // reaches DR-0013's budget, the RangeError is thrown *inside a job*, and §9.5 step 3 discards
    // the completion. The queue empties, `run` answers normally, and the exit status is zero.
    //
    // What is left behind is exactly one rejected promise nobody claimed — the last link, whose
    // handler threw before it could arm the next. So a host that reads this list finds out; before
    // it existed there was nothing in the engine that could have told anyone.
    let mut heap = Heap::new();
    heap.set_budget(3 << 20);
    let mut vm = Vm::new(&mut heap);
    // No schedule, so the budget is reached rather than collected away — this is about what the
    // engine *says* when it runs out, not about whether it runs out.
    vm.set_collection_growth(None);
    let script = parse_script(
        "var p = Promise.resolve(); var n = 0; \
         function step() { n++; if (n < 2000000) { p.then(step) } } \
         p.then(step);",
    )
    .expect("the setup parses"); // the test is what is left over
    let chunk = compile_script(&script, &mut heap).expect("the setup compiles"); // same
    let outcome = vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    // The run itself is a success, which is the whole problem: nothing about the answer says the
    // program stopped two million turns short of what it asked for.
    assert!(matches!(outcome, Outcome::Value(_)));
    let left = vm.unhandled_rejections().to_vec();
    assert_eq!(left.len(), 1, "one link should have been left rejected");
    // …and it carries the reason, which is what makes the list worth reading rather than a count
    // of something odd having happened. Read as a property because a thrown Error object has no
    // `toString` a test can call — see `describe`, which answers `[object]` for exactly that reason.
    let Value::Object(error) = heap
        .promise(left[0])
        .map_or(Value::Undefined, |found| found.result)
    else {
        panic!("the reason should be the Error the engine refused with");
    };
    let message = match own(&mut heap, error, "message").map(|found| found.kind) {
        Some(PropertyKind::Data { value, .. }) => value,
        _ => Value::Undefined,
    };
    let Value::String(text) = message else {
        panic!("an Error's message is a String");
    };
    assert_eq!(
        heap.string(text).map(String::from_utf16_lossy),
        Some("the heap has grown past what this engine will allocate".to_string())
    );
}

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
fn a_script_that_throws_still_runs_its_jobs_and_still_reports_its_own_throw() {
    // An uncaught exception ends the *script*, not the queue: a `then` registered before the throw
    // is still waiting, and §9.5 says nothing about the script having gone well. So both happen —
    // the handler runs, and the answer is still the throw.
    //
    // Found by the conformance suite, which reported a `Fault` rather than a failure. A job's own
    // execution uses `escaped` to carry its throws back through Rust, so a script's uncaught throw
    // left sitting there was taken by the first job that ran; the script then looked as though it
    // had completed normally, with a stack full of the operands the throw had abandoned, and the
    // engine reported that its own compiler and interpreter disagreed. Two wrong answers from one
    // slot being read by two things.
    assert_eq!(
        run(
            "var log = ''; Promise.resolve(1).then(function () { log += 'ran'; }); \
             throw new Error('from the script');"
        ),
        "thrown [object]"
    );
    // …and the handler really did run, which the throw above cannot show.
    assert_eq!(
        run_settled(
            "var log = ''; Promise.resolve(1).then(function () { log += 'ran'; }); \
             try { throw new Error('caught'); } catch (e) {}",
            "log"
        ),
        "ran"
    );
    // A job that throws is discarded (§9.5 step 3) and does not become the script's answer, nor
    // does it stop the jobs behind it.
    assert_eq!(
        run_settled(
            "var log = ''; Promise.resolve().then(function () { throw new Error('in a job'); }); \
             Promise.resolve().then(function () { log += 'still ran'; });",
            "log"
        ),
        "still ran"
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

#[test]
fn all_answers_every_value_in_iteration_order_and_fails_on_the_first_rejection() {
    // §27.2.4.1 — the order of the answer is the order of the *iterable*, not the order the
    // promises settled in. A list appended to on settlement would give the right values in the
    // wrong places, and would do so only sometimes, which is the worst kind of wrong.
    assert_eq!(
        run_settled(
            "var out; Promise.all([Promise.resolve(1), 2, Promise.resolve(3)])              .then(function (v) { out = v.join(','); });",
            "out"
        ),
        "1,2,3"
    );
    // …including when the later element settles first, which is what the slot-made-early is for.
    assert_eq!(
        run_settled(
            "var out; var slow; var p = new Promise(function (r) { slow = r; });              Promise.all([p, Promise.resolve('fast')]).then(function (v) { out = v.join(','); });              slow('slow');",
            "out"
        ),
        "slow,fast"
    );
    // An **empty** iterable resolves, and resolves with an array. This is the row the counter that
    // starts at one exists for: with a counter starting at zero it would never settle.
    assert_eq!(
        run_settled(
            "var out; Promise.all([]).then(function (v) { out = Array.isArray(v) + ',' + v.length; });",
            "out"
        ),
        "true,0"
    );
    // The first rejection rejects the group, and the rest are neither waited for nor recorded.
    assert_eq!(
        run_settled(
            "var out; Promise.all([Promise.resolve(1), Promise.reject('no'), Promise.resolve(3)])              .then(function () { out = 'wrongly resolved'; }, function (e) { out = 'rejected ' + e; });",
            "out"
        ),
        "rejected no"
    );
    // An answer that is not iterable is a **rejection**, not a throw — which is the most
    // surprising thing about all four of these and is what `IfAbruptRejectPromise` is for.
    assert_eq!(
        run_settled(
            "var out; var p = Promise.all(null); p.catch(function (e) { out = e.constructor.name; });",
            "out"
        ),
        "TypeError"
    );
    assert_eq!(run("Promise.all(null) instanceof Promise"), "true");
}

#[test]
fn all_settled_records_how_each_one_settled_and_never_rejects() {
    // §27.2.4.2 — an outcome object per element, in two shapes. A rejection is not a failure of
    // the group; that is the whole difference from `all`.
    assert_eq!(
        run_settled(
            "var out; Promise.allSettled([Promise.resolve(1), Promise.reject('no')])              .then(function (v) {                 out = v[0].status + ':' + v[0].value + '|' + v[1].status + ':' + v[1].reason; });",
            "out"
        ),
        "fulfilled:1|rejected:no"
    );
    // Two shapes rather than one with a hole: a program tells them apart by `status`, and an
    // object carrying both keys would answer `'value' in result` wrongly for a rejection.
    assert_eq!(
        run_settled(
            "var out; Promise.allSettled([Promise.reject('no')]).then(function (v) {                 out = ('value' in v[0]) + ',' + ('reason' in v[0]); });",
            "out"
        ),
        "false,true"
    );
    assert_eq!(
        run_settled(
            "var out; Promise.allSettled([]).then(function (v) { out = v.length; });",
            "out"
        ),
        "0"
    );
}

#[test]
fn race_takes_the_first_to_settle_whichever_way_it_settled() {
    // §27.2.4.4 — and it keeps no state at all: each element is subscribed with the group's own
    // resolve and reject, so the first to arrive settles it and the rest find it already settled.
    // One that settles later wins over one that never settles, which is the plain case.
    assert_eq!(
        run_settled(
            "var out; var late; \
             Promise.race([new Promise(function (r) { late = r; }), new Promise(function () {})]) \
             .then(function (v) { out = v; }); \
             Promise.resolve().then(function () { late('late'); });",
            "out"
        ),
        "late"
    );
    // When two settled at the same moment, "first" means first *subscribed*, which is iteration
    // order. The whole walk is synchronous, so a promise a later statement resolves has already
    // been subscribed to by then and takes its turn in the queue behind the ones before it.
    assert_eq!(
        run_settled(
            "var out; var slow; \
             Promise.race([new Promise(function (r) { slow = r; }), Promise.resolve('quick')]) \
             .then(function (v) { out = v; }); slow('slow');",
            "out"
        ),
        "quick"
    );
    // A rejection wins just as a fulfilment does, which is what separates `race` from `any`.
    assert_eq!(
        run_settled(
            "var out; Promise.race([Promise.reject('first'), Promise.resolve('second')])              .then(function () { out = 'wrongly resolved'; }, function (e) { out = 'rejected ' + e; });",
            "out"
        ),
        "rejected first"
    );
    // An empty iterable never settles — there is nothing to be first. Nothing runs, and that is
    // the right answer rather than a hang: the queue empties and the program ends.
    assert_eq!(
        run_settled(
            "var out = 'never settled'; Promise.race([]).then(function () { out = 'wrong'; });",
            "out"
        ),
        "never settled"
    );
}

#[test]
fn a_combinator_reads_resolve_once_and_subscribes_before_reading_the_next_element() {
    // §27.2.4.1 step 3 — `resolve` is read from the constructor **once**, before the walk. A
    // program that replaces it halfway through would otherwise get two different functions for one
    // call, which no clause allows.
    assert_eq!(
        run_settled(
            "var seen = 0; class P extends Promise {}              Object.defineProperty(P, 'resolve', { get: function () { seen++; return Promise.resolve; } });              P.all([1, 2, 3]); var out = seen;",
            "out"
        ),
        "1"
    );
    // §27.2.4.1.1 step 8 — one element is read, resolved and subscribed before the next is read.
    // An iterator with side effects sees the interleaving, and a version that drained the iterable
    // first would show every read before any subscription.
    assert_eq!(
        run_settled(
            "var log = ''; var made = 0;              var iterable = {}; iterable[Symbol.iterator] = function () {                var at = 0; return { next: function () {                  log += 'read' + at + ','; at++;                  return at > 2 ? { done: true } : { done: false, value: at }; } }; };              var seen = Promise.resolve;              Promise.all(iterable); var out = log;",
            "out"
        ),
        "read0,read1,read2,"
    );
}

#[test]
fn an_element_settles_its_slot_once_however_many_times_it_is_told() {
    // §27.2.4.1.2 step 1 — `[[AlreadyCalled]]`. Every element gives up exactly one of the group's
    // count, and an element that gave up two would take the count to zero while slots were still
    // empty: the group would resolve early, with holes in its array.
    //
    // Reaching it needs a `resolve` that answers something other than a promise, because a real
    // promise settles once by itself. A subclass may replace it with anything, and then the walk
    // subscribes by calling that object's `then` — which is free to call back twice.
    let twice = "class P extends Promise {                    static resolve(v) { return { then: function (f) { f(v); f(v); } }; } }";
    assert_eq!(
        run_settled(
            &format!(
                "{twice} var out; P.all([1, 2, 3]).then(function (v) {{ out = v.join(','); }});"
            ),
            "out"
        ),
        "1,2,3"
    );
    // The same for `allSettled`, where the pair *shares* the record: an element told both that it
    // was fulfilled and that it was rejected fills its slot once, with whichever came first.
    let both = "class P extends Promise {                   static resolve(v) { return { then: function (f, r) { f(v); r('later'); } }; } }";
    assert_eq!(
        run_settled(
            &format!(
                "{both} var out; P.allSettled([1]).then(function (v) {{                    out = v.length + ':' + v[0].status + ':' + v[0].value; }});"
            ),
            "out"
        ),
        "1:fulfilled:1"
    );
}

#[test]
fn an_all_settled_outcome_is_an_ordinary_object_a_program_may_change() {
    // §27.2.4.2.2 steps 10 and 11 use `CreateDataPropertyOrThrow`, which is §7.3.5's *ordinary*
    // property: writable, enumerable and configurable. Not §6.1.7.1's defaults, which would make
    // the result unwritable and invisible to `Object.keys` — a result a program cannot inspect
    // with the tools it inspects everything else with.
    assert_eq!(
        run_settled(
            "var out; Promise.allSettled([Promise.resolve(1)]).then(function (v) {                var d = Object.getOwnPropertyDescriptor(v[0], 'value');                out = d.writable + ',' + d.enumerable + ',' + d.configurable                  + '|' + Object.keys(v[0]).join('+'); });",
            "out"
        ),
        "true,true,true|status+value"
    );
}

#[test]
fn any_takes_the_first_fulfilment_and_gathers_the_reasons_if_there_is_none() {
    // §27.2.4.3 — `all` with the two halves exchanged. The first fulfilment wins outright, and a
    // rejection is not a failure of the group but an entry in a list.
    assert_eq!(
        run_settled(
            "var out; Promise.any([Promise.reject('a'), Promise.resolve('yes'), Promise.reject('b')])              .then(function (v) { out = v; });",
            "out"
        ),
        "yes"
    );
    // Running out of elements is the failure, and what it failed with is every reason in
    // iteration order — which is what `AggregateError` exists for and its only use in the language.
    assert_eq!(
        run_settled(
            "var out; Promise.any([Promise.reject('a'), Promise.reject('b')])              .catch(function (e) {                 out = e.constructor.name + ':' + e.errors.join(',') + ':' + (e instanceof Error); });",
            "out"
        ),
        "AggregateError:a,b:true"
    );
    // An **empty** iterable rejects immediately, with no reasons at all — where `Promise.all([])`
    // resolves and `Promise.race([])` never settles. Three answers to the same question, and the
    // counter that starts at one is what makes each of them arrive.
    assert_eq!(
        run_settled(
            "var out; Promise.any([]).catch(function (e) {                 out = e.constructor.name + ':' + e.errors.length; });",
            "out"
        ),
        "AggregateError:0"
    );
    // The error `any` builds carries `errors` on the same terms the constructor's does — writable,
    // **not enumerable**, configurable — which is a separate piece of code and so a separate row:
    // §27.2.4.3.1 defines the property itself rather than going through §20.5.7.1.
    assert_eq!(
        run_settled(
            "var out; Promise.any([Promise.reject(1)]).catch(function (e) {                 var d = Object.getOwnPropertyDescriptor(e, 'errors');                 out = d.writable + ',' + d.enumerable + ',' + d.configurable                   + '|' + Object.keys(e).length; });",
            "out"
        ),
        "true,false,true|0"
    );
    // §27.2.4.3.1 makes the error *without* calling `AggregateError`, so a program that replaced
    // it does not change what `any` rejects with — and one that made it throw cannot make `any`
    // throw. The rejection is still an instance of the original, by its prototype.
    assert_eq!(
        run_settled(
            "var kept = AggregateError;              AggregateError = function () { throw new Error('replaced'); };              var out; Promise.any([Promise.reject(1)])              .catch(function (e) { out = (e instanceof kept) + ',' + e.errors.join(''); });",
            "out"
        ),
        "true,1"
    );
}

#[test]
fn an_aggregate_error_takes_its_errors_first_and_its_message_second() {
    // §20.5.7.1 — the argument order is the thing to get right, and it is the opposite of what
    // every other error constructor does. `new AggregateError("oops")` is a *message-less* error
    // whose `errors` is the characters of the string, because a string is iterable.
    assert_eq!(
        run("var e = new AggregateError([1, 2], 'why'); e.message + '|' + e.errors.join(',')"),
        "why|1,2"
    );
    assert_eq!(
        run("var e = new AggregateError('oops'); e.message + '|' + e.errors.join('')"),
        "|oops"
    );
    // §20.5.7.1 step 6 — `errors` is **not enumerable**, because an error is a thing programs log
    // wholesale and a list of causes in every `for...in` over one would be a surprise. It is
    // writable and configurable, which the other two attributes of a data property are.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(new AggregateError([1]), 'errors');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,true"
    );
    assert_eq!(run("Object.keys(new AggregateError([1], 'm')).length"), "0");
    // It is an `Error` and its prototype chain says so, which is what `catch (e) { if (e instanceof
    // Error) }` relies on.
    assert_eq!(
        run(
            "var e = new AggregateError([]); (e instanceof AggregateError) + ',' + (e instanceof Error)              + ',' + e.name + ',' + String(e)"
        ),
        "true,true,AggregateError,AggregateError"
    );
    // §20.5.7.2 — `AggregateError.prototype` is not writable, not enumerable and not configurable,
    // exactly as every other error constructor's is. A script may replace `f.prototype` on a
    // function it wrote and may not replace this one.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(AggregateError, 'prototype');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,false"
    );
    // §20.5.7.2 — `length` is 2 and the constructor inherits from `Error`, as a native error's does.
    assert_eq!(
        run("AggregateError.length + ',' + (Object.getPrototypeOf(AggregateError) === Error)"),
        "2,true"
    );
    // Something that is not iterable is a TypeError, because step 5 is `IterableToList`.
    assert_eq!(
        run("try { new AggregateError(1); } catch (e) { e.constructor.name; }"),
        "TypeError"
    );
}

#[test]
fn promise_try_runs_now_and_answers_for_the_throw() {
    // §27.2.4.9 — the point of it is the *synchronous* throw. `Promise.resolve().then(f)` also
    // gives a promise and runs `f` a turn later; this runs it immediately, which is the half a
    // bare call already does, and still turns a throw into a rejection, which is the half it
    // cannot.
    assert_eq!(
        run(
            "var order = ['before']; Promise.try(function () { order.push('inside') }); \
             order.push('after'); order.join(',')"
        ),
        "before,inside,after"
    );
    assert_eq!(
        run_settled(
            "var out = ''; Promise.try(function () { throw 'boom' }) \
                 .then(function (v) { out = 'resolved:' + v }, function (e) { out = 'rejected:' + e });",
            "out"
        ),
        "rejected:boom"
    );
    // The extra arguments are forwarded, and the receiver is `undefined` rather than the
    // constructor: the callback belongs to the caller, and giving it `Promise` as `this` would
    // invent a binding the clause does not make. The callback is strict so that §10.2.1.2's
    // substitution does not turn that `undefined` into the global object before it can be seen —
    // a sloppy one answers `false` here and says nothing about what was passed.
    assert_eq!(
        run_settled(
            "var out = ''; \
             Promise.try(function (a, b) { 'use strict'; return a + b + '/' + (this === undefined) }, 'x', 'y') \
                 .then(function (v) { out = v });",
            "out"
        ),
        "xy/true"
    );
    // Step 2 refuses a receiver that is not an Object **before** the capability is asked for, so
    // the callback has not run when it throws. A check placed after the call would leave the
    // effect behind and only then complain.
    assert_eq!(
        run("var ran = 0; var caught = 'none'; \
             try { Promise.try.call(1, function () { ran = 1 }) } catch (e) { caught = e.constructor.name } \
             caught + ',' + ran"),
        "TypeError,0"
    );
    assert_eq!(
        run("[Promise.try.length, Promise.try.name].join(',')"),
        "1,try"
    );
}

#[test]
fn number_holds_the_same_two_parsers_the_global_does() {
    // §21.1.2.12 and §21.1.2.13 — the **same function object**, not a second one with the same
    // body. Installing a copy answers every other question identically and this one wrongly.
    assert_eq!(
        run(
            "[Number.parseFloat === parseFloat, Number.parseInt === parseInt, \
             Number.parseFloat('1.5'), Number.parseInt('ff', 16)].join(',')"
        ),
        "true,true,1.5,255"
    );
    // §17's ordinary shape for a built-in's property, which `define_fixed` would have got wrong.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Number, 'parseFloat'); \
             [d.writable, d.enumerable, d.configurable].join(',')"
        ),
        "true,false,true"
    );
}

#[test]
fn a_combinator_that_gives_up_part_way_closes_the_iterable() {
    // §27.2.4.1 step 8.a and its three siblings — an abrupt walk closes the iterator unless the
    // iterator is where it went wrong. ViperJS had no such step at all, so `Promise.all` over an
    // iterable whose `C.resolve` throws left it open; and a `resolve` that throws is the first
    // thing that happens after a value has been taken.
    let combinator = |name: &str| {
        format!(
            "var closed = 0; \
             var able = {{}}; able[Symbol.iterator] = function () {{ return {{ \
                 next: function () {{ return {{ done: false, value: 1 }} }}, \
                 return: function () {{ closed += 1; return {{}} }} }} }}; \
             var C = function (x) {{ x(function () {{}}, function () {{}}) }}; \
             C.prototype = Promise.prototype; \
             C.resolve = function () {{ throw 'from resolve' }}; \
             Promise.{name}.call(C, able); closed"
        )
    };
    for name in ["all", "allSettled", "any", "race"] {
        assert_eq!(run(&combinator(name)), "1", "Promise.{name}");
    }
    // The `then` lookup and the `then` call are inside the walk too, so both are places the
    // iterator is still owed the news.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { closed += 1; return {} } } }; \
             var C = function (x) { x(function () {}, function () {}) }; \
             C.prototype = Promise.prototype; \
             C.resolve = function () { return { get then() { throw 'from then' } } }; \
             Promise.all.call(C, able); closed"),
        "1"
    );
    // §7.4.8 — a step that throws leaves the record **done**, so nothing is closed: an iterator
    // that failed to produce was not abandoned. This is the row that stops the fix from being
    // "close whenever anything goes wrong".
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { throw 'from next' }, \
                 return: function () { closed += 1; return {} } } }; \
             Promise.all(able); closed"),
        "0"
    );
    // §7.4.2 step 4 — reading `next` builds the **record**, so a `next` *getter* that throws is
    // `GetIterator` failing and step 8 is never reached: there is nothing to close. That is the
    // one place the initial `[[Done]]` matters, and reading `next` inside the walk instead would
    // have closed here.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 get next() { throw 'from the next getter' }, \
                 return: function () { closed += 1; return {} } } }; \
             Promise.all(able); closed"),
        "0"
    );
    // And a walk that finishes on its own closes nothing either — an empty iterable and a finite
    // one both run to `done` and were never abandoned.
    assert_eq!(
        run("var closed = 0; var n = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { n += 1; return n <= 2 ? { done: false, value: n } : { done: true } }, \
                 return: function () { closed += 1; return {} } } }; \
             Promise.all(able); closed"),
        "0"
    );
    // The combinator still answers a promise rather than throwing, which is what
    // `IfAbruptRejectPromise` is for and what the close must not disturb.
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { return {} } } }; \
             class Sub extends Promise { static resolve() { throw 'from resolve' } } \
             Promise.all.call(Sub, able).then(null, function (e) { out = 'rejected:' + e });",
            "out"
        ),
        "rejected:from resolve"
    );
}
