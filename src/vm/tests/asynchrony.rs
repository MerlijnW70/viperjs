//! §15.8 and §27.7 — an `async` function, and the `await` that stops one.
//!
//! Every row here needs [`super::run_settled`], and that is the point rather than an inconvenience:
//! an `async` function's answer is a promise, and what it settles with is only knowable after §9.5's
//! jobs have run. A test that read the script's completion value would see the promise and never
//! what is in it — which is exactly what a caller sees, and exactly why the feature exists.

use super::*;

#[test]
fn an_async_function_answers_with_a_promise_before_its_body_is_finished() {
    // §27.7.5.1 — the call answers with a promise however far the body got, and the body really
    // does start: everything up to the first `await` has already run when the call returns.
    assert_eq!(
        run_settled(
            "var log = []; async function f() { log.push('body'); await 1; log.push('after'); } var p = f(); log.push('call');",
            "log.join(',')"
        ),
        "body,call,after"
    );
    assert_eq!(
        run_settled(
            "async function f() { return 1; } var p = f();",
            "p instanceof Promise"
        ),
        "true"
    );
    // A body with no `await` at all still answers with a promise and still settles a turn later,
    // which is what makes `async` a promise-returning contract rather than a shortcut.
    assert_eq!(
        run_settled(
            "var seen = 'pending'; async function f() { return 7; } f().then(function (v) { seen = v; });",
            "seen"
        ),
        "7"
    );
}

#[test]
fn a_return_resolves_the_promise_and_a_throw_rejects_it() {
    // §27.7.5.2 — the two ways out of the body, and neither reaches the caller as itself. A throw
    // that nothing inside caught becomes a rejection, so calling an `async` function never throws.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { return 'v'; } f().then(function (v) { out = 'resolved:' + v; }, function (e) { out = 'rejected:' + e; });",
            "out"
        ),
        "resolved:v"
    );
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { throw 'boom'; } f().then(function (v) { out = 'resolved:' + v; }, function (e) { out = 'rejected:' + e; });",
            "out"
        ),
        "rejected:boom"
    );
    // …and the call itself does not throw, which is the half a `try` around it would hide.
    assert_eq!(
        run_settled(
            "var out = 'no throw'; async function f() { throw 'boom'; } try { f(); } catch (e) { out = 'threw'; }",
            "out"
        ),
        "no throw"
    );
    // A throw *after* an `await` rejects too, which is the path through the job queue rather than
    // the synchronous one.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { await 1; throw 'late'; } f().then(null, function (e) { out = 'rejected:' + e; });",
            "out"
        ),
        "rejected:late"
    );
    // A `try` inside the body catches its own throw and the promise resolves, which says the
    // implicit handler is *outside* the body's own and not instead of it.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { try { throw 'x'; } catch (e) { return 'caught ' + e; } } f().then(function (v) { out = v; });",
            "out"
        ),
        "caught x"
    );
}

#[test]
fn an_await_evaluates_to_what_the_promise_settled_with() {
    // §27.7.5.3 step 3 — the value goes back into the expression that stopped, exactly as `next`'s
    // argument goes into a `yield`.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { var v = await Promise.resolve(5); return v * 2; } f().then(function (v) { out = v; });",
            "out"
        ),
        "10"
    );
    // A plain value is wrapped by `PromiseResolve` and still costs a turn, which is what makes
    // `await 1` observably asynchronous.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { return await 41; } f().then(function (v) { out = v + 1; });",
            "out"
        ),
        "42"
    );
    // Several in a row, each resuming the same parked execution — the locals between them are the
    // execution's own.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { var a = await 1; var b = await 2; var c = await 3; return a + b + c; } f().then(function (v) { out = v; });",
            "out"
        ),
        "6"
    );
    // …and the operands of a half-built expression survive it, like any suspension.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { return 100 + (await 5) * 2; } f().then(function (v) { out = v; });",
            "out"
        ),
        "110"
    );
}

#[test]
fn a_rejected_promise_throws_at_the_await() {
    // §27.7.5.3 step 5 — a rejection is not a value that comes back, it is a throw at the point
    // that stopped. So a `try` around the `await` catches it and the body carries on.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { try { await Promise.reject('why'); } catch (e) { return 'caught ' + e; } } f().then(function (v) { out = v; });",
            "out"
        ),
        "caught why"
    );
    // With no `try` it escapes the body and rejects the function's own promise, which is the two
    // mechanisms meeting.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { await Promise.reject('why'); return 'unreached'; } f().then(function (v) { out = 'resolved:' + v; }, function (e) { out = 'rejected:' + e; });",
            "out"
        ),
        "rejected:why"
    );
    // A `finally` runs on the way past, which is the ordinary handler machinery on a throw that
    // arrived from a job.
    assert_eq!(
        run_settled(
            "var seen = ''; async function f() { try { await Promise.reject('x'); } finally { seen = 'ran'; } } f().then(null, function () {});",
            "seen"
        ),
        "ran"
    );
}

#[test]
fn one_async_function_may_await_another() {
    // The shape the feature is actually used in, and the one that needs both halves at once: the
    // inner call answers with a promise, the outer `await` attaches to it, and the outer body is
    // revived by the job that settles it.
    assert_eq!(
        run_settled(
            "var out = ''; async function inner() { return 'i'; } async function outer() { return 'o' + (await inner()); } outer().then(function (v) { out = v; });",
            "out"
        ),
        "oi"
    );
    // A rejection travels the same way, through both.
    assert_eq!(
        run_settled(
            "var out = ''; async function inner() { throw 'deep'; } async function outer() { try { await inner(); } catch (e) { return 'outer caught ' + e; } } outer().then(function (v) { out = v; });",
            "out"
        ),
        "outer caught deep"
    );
    // …and a loop of awaits keeps its counter, which is the parked execution carrying its locals.
    assert_eq!(
        run_settled(
            "var out = ''; async function f(n) { var total = 0; for (var i = 1; i <= n; i++) { total += await i; } return total; } f(4).then(function (v) { out = v; });",
            "out"
        ),
        "10"
    );
}

#[test]
fn the_call_binds_this_and_the_arguments_as_any_other_call_does() {
    // The body is parked and revived, so both have to be part of the execution rather than of the
    // machine — a `this` read after an `await` is the one the call decided.
    assert_eq!(
        run_settled(
            "var out = ''; var o = { n: 5, m: async function () { await 1; return this.n; } }; o.m().then(function (v) { out = v; });",
            "out"
        ),
        "5"
    );
    assert_eq!(
        run_settled(
            "var out = ''; async function f(a, b) { await 1; return a + b; } f(1, 2).then(function (v) { out = v; });",
            "out"
        ),
        "3"
    );
    // §15.8's other productions: an async method, an async arrow, and an async function
    // expression. All four reach the same start, and a compiler that wired only the declaration
    // would pass every row above.
    assert_eq!(
        run_settled(
            "var out = ''; var g = async function () { return 'expr'; }; g().then(function (v) { out = v; });",
            "out"
        ),
        "expr"
    );
    assert_eq!(
        run_settled(
            "var out = ''; var o = { async m() { return 'meth'; } }; o.m().then(function (v) { out = v; });",
            "out"
        ),
        "meth"
    );
    assert_eq!(
        run_settled(
            "var out = ''; var a = async () => 'arrow'; a().then(function (v) { out = v; });",
            "out"
        ),
        "arrow"
    );
    // An async arrow has no `this` of its own, on the same terms as any other arrow — and it has to
    // still be true across the suspension.
    assert_eq!(
        run_settled(
            "var out = ''; var o = { n: 9, m: function () { var a = async () => { await 1; return this.n; }; return a(); } }; o.m().then(function (v) { out = v; });",
            "out"
        ),
        "9"
    );
    // …and an `async` function is not a constructor, which §15.8.3 gives it no `[[Construct]]` for.
    assert_eq!(
        run("async function f() {} try { new f(); } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn the_order_jobs_run_in_is_what_a_program_notices_first() {
    // §9.5 — nothing about `async` is concurrent. Everything after the first `await` is a job, so
    // it runs after every synchronous statement in the script, and the interleaving is decided by
    // the queue rather than by anything else.
    assert_eq!(
        run_settled(
            "var log = []; async function f() { log.push(2); await null; log.push(4); } log.push(1); f(); log.push(3);",
            "log.join(',')"
        ),
        "1,2,3,4"
    );
    // Two `async` functions started in order resume in order, one turn each, which is what makes
    // the queue a queue.
    assert_eq!(
        run_settled(
            "var log = []; async function f(n) { await null; log.push(n + 'a'); await null; log.push(n + 'b'); } f('x'); f('y');",
            "log.join(',')"
        ),
        "xa,ya,xb,yb"
    );
    // An `await` is at least one turn even for a value that is already a promise of this realm's
    // kind, so a `then` registered first still runs first.
    assert_eq!(
        run_settled(
            "var log = []; var p = Promise.resolve(1); p.then(function () { log.push('then'); }); (async function () { await p; log.push('await'); })();",
            "log.join(',')"
        ),
        "then,await"
    );
}

#[test]
fn an_async_generator_answers_every_resumption_with_a_promise() {
    // §27.6.1.2 — `next` answers a promise, not an iterator result, and the result arrives inside
    // it. That is the whole difference from §27.5.1, and it is what a caller sees first.
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; } var r = 'no'; var p = g().next();              p.then(function (v) { r = v.value + ':' + v.done; });",
            "(p instanceof Promise) + ' ' + r"
        ),
        "true 1:false"
    );
    // …and the last one is `{ undefined, true }`, for ever after.
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; } var it = g(); var r = [];              it.next().then(function (v) { r.push(v.value + ':' + v.done);                return it.next(); }).then(function (v) { r.push(v.value + ':' + v.done);                return it.next(); }).then(function (v) { r.push(v.value + ':' + v.done); });",
            "r.join(',')"
        ),
        "1:false,undefined:true,undefined:true"
    );
}

#[test]
fn two_asks_made_before_the_first_is_answered_are_served_in_order() {
    // §27.6.3.2's queue, which is the one thing a synchronous generator needs nothing like: `next`
    // returns a promise straight away, so both of these are outstanding against a body that has
    // not reached its first `yield`. They must be answered `1` then `2` and not both `1`.
    //
    // Without the queue the second ask finds the body parked and resumes it *again* from the same
    // `yield`, which is a plausible-looking wrong answer rather than a crash — the reason this is
    // pinned by a test rather than left to the conformance number.
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; yield 2; } var it = g(); var r = [];              it.next().then(function (v) { r.push(v.value); });              it.next().then(function (v) { r.push(v.value); });",
            "r.join(',')"
        ),
        "1,2"
    );
}

#[test]
fn an_ask_made_while_the_body_is_awaiting_waits_rather_than_reviving_it() {
    // The case the queue exists for, and the one the two-asks test above does *not* reach: by the
    // time the second `next` runs there, the body is parked at a `yield` with nothing in service,
    // so serving it straight away is right. Here the body is parked **inside an `await`** with the
    // first request still in service, and reviving it would resume it from the middle of that
    // `await` with the wrong value.
    //
    // Told apart by the queue and by nothing else: both states are "parked", which is why there is
    // no `[[AsyncGeneratorState]]` field to consult and why this test is the one that pins it.
    //
    // What the body yields has to be **what it awaited**, and that is not decoration. Written with
    // constants the wrong engine gives the right answer: reviving the parked body directly hands
    // the `await` the second `next`'s argument instead of the settled `7`, and then yields the
    // same numbers in the same order anyway. Only a value that came *through* the await separates
    // the two — the first version of this test did not have one, and mutating the guard proved it.
    assert_eq!(
        run_settled(
            "async function* g() { var a = await 7; yield a; yield a + 1; } var it = g(); var r = [];              it.next().then(function (v) { r.push(v.value); });              it.next().then(function (v) { r.push(v.value); });",
            "r.join(',')"
        ),
        "7,8"
    );
}

#[test]
fn a_return_into_a_suspended_body_runs_the_finally_blocks_it_is_inside() {
    // §27.5.1.3 step 5's distinction, which §27.6 inherits: a body that has begun is resumed *at
    // the `yield`* so that what it is inside gets to run, where one that has not begun is simply
    // completed. `it.return(9)` here has to leave `9` and to have run the `finally`.
    assert_eq!(
        run_settled(
            "var ran = false;              async function* g() { try { yield 1; } finally { ran = true; } }              var it = g(); var r = 'no';              it.next().then(function () { return it.return(9); })                .then(function (v) { r = v.value + ':' + v.done; });",
            "ran + ' ' + r"
        ),
        "true 9:true"
    );
}

#[test]
fn a_completed_generator_answers_each_queued_ask_in_that_ask_s_own_way() {
    // §27.6.3.6's drain, and the three methods do not agree: `next` answers `undefined`, `return`
    // hands back what it was given, and `throw` **rejects**. All three are queued behind the
    // request that finished the body, so all three are answered without anything running.
    assert_eq!(
        run_settled(
            "async function* g() { return 1; } var it = g(); var r = [];              it.next().then(function (v) { r.push('n' + v.value); });              it.return(5).then(function (v) { r.push('r' + v.value + ':' + v.done); });              it.throw(new TypeError('x')).then(function () { r.push('resolved'); },                function (e) { r.push('t' + e.name); });",
            "r.join(',')"
        ),
        "n1,r5:true,tTypeError"
    );
}

#[test]
fn the_async_iterator_symbol_has_the_attributes_that_clause_gives_it() {
    // §27.1.3.1 — writable, not enumerable, configurable. A method a script may replace and may
    // not find by enumeration, which is what every method in §27 is and what makes the difference
    // between a built-in and a property somebody assigned.
    assert_eq!(
        run_settled(
            "async function* g() {}              var p = Object.getPrototypeOf(Object.getPrototypeOf(Object.getPrototypeOf(g())));              var d = Object.getOwnPropertyDescriptor(p, Symbol.asyncIterator);",
            "d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,true"
    );
}

#[test]
fn an_await_inside_the_body_does_not_answer_the_request() {
    // §27.6.3.8 against §27.7.5.3 — an `await` parks the body without taking anything off the
    // queue, so the request that is in service stays in service and is answered by the `yield`
    // that comes after. A body that answered at the `await` would resolve with the awaited value.
    assert_eq!(
        run_settled(
            "async function* g() { var a = await 10; yield a + 1; } var r = 'no';              g().next().then(function (v) { r = v.value + ':' + v.done; });",
            "r"
        ),
        "11:false"
    );
}

#[test]
fn a_throw_escaping_the_body_rejects_the_request_being_served() {
    // §27.6.3.2 with a throw completion. The promise the caller is holding *rejects* — it does not
    // resolve with an error object, which is what a body that treated the throw as a return would
    // produce.
    assert_eq!(
        run_settled(
            "async function* g() { throw new TypeError('no'); } var r = 'no';              g().next().then(function () { r = 'resolved'; }, function (e) { r = e.name; });",
            "r"
        ),
        "TypeError"
    );
}

#[test]
fn a_return_completes_it_and_answers_everything_still_queued() {
    // §27.6.3.6's drain: once the body is gone, every request behind the one that finished it is
    // answered too, and each in the way its own method demands — `next` with `undefined`, `return`
    // with what it was given.
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; } var it = g(); var r = [];              it.return(9).then(function (v) { r.push(v.value + ':' + v.done); });              it.next().then(function (v) { r.push(v.value + ':' + v.done); });",
            "r.join(',')"
        ),
        "9:true,undefined:true"
    );
}

#[test]
fn a_delegation_inside_an_async_generator_asks_for_the_async_iterator() {
    // §15.5.5 step 3 and step 4 — `GetIterator(value, async)`, which asks `[@@asyncIterator]` and
    // falls back to §27.1.4's wrapper. Delegating to another async generator is the case that only
    // works if the async one is asked for: its `next` answers a promise, and the synchronous walk
    // would read `done` off that promise, find it absent, and loop for ever.
    assert_eq!(
        run_settled(
            "async function* inner() { yield 1; yield 2; }              async function* g() { yield* inner(); yield 3; }              var r = []; (async function () { for await (var x of g()) { r.push(x); } })();",
            "r.join(',')"
        ),
        "1,2,3"
    );
    // …and a plain array still works, through the async-from-sync wrapper §7.4.3 falls back to.
    assert_eq!(
        run_settled(
            "async function* g() { yield* [1, 2]; } var r = [];              (async function () { for await (var x of g()) { r.push(x); } })();",
            "r.join(',')"
        ),
        "1,2"
    );
}

#[test]
fn a_delegation_inside_an_async_generator_never_reads_symbol_iterator() {
    // The half of step 4 that a passing walk does not prove: when `[@@asyncIterator]` is there,
    // `[@@iterator]` must not be *looked at* — not read and discarded, not read as a fallback.
    // A whole bucket of test262 checks exactly this by leaving a throwing getter on the sync one,
    // and the synchronous delegation this replaced tripped every one of them.
    assert_eq!(
        run_settled(
            "var asked = false;              var inner = { };              inner[Symbol.asyncIterator] = function () {                var n = 0; return { next: function () { n += 1;                  return Promise.resolve({ value: n, done: n > 2 }); } }; };              Object.defineProperty(inner, Symbol.iterator,                { get: function () { asked = true; } });              async function* g() { yield* inner; } var r = [];              (async function () { for await (var x of g()) { r.push(x); } })();",
            "asked + ' ' + r.join(',')"
        ),
        "false 1,2"
    );
}

#[test]
fn an_async_generator_is_what_for_await_walks() {
    // §27.1.3.1's `[@@asyncIterator]`, which is only reachable now that something inherits it.
    // Without it the loop falls back to §7.4.3's synchronous path and reads a `Symbol.iterator`
    // the specification says it must not even ask for — so this asserts both halves.
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; yield 2; } var n = 0;              (async function () { for await (var x of g()) { n += x; } })();",
            "n"
        ),
        "3"
    );
    assert_eq!(
        run_settled(
            "async function* g() { yield 1; } var asked = false;              var it = g(); Object.defineProperty(it, Symbol.iterator,                { get: function () { asked = true; } });              (async function () { for await (var x of it) {} })();",
            "asked"
        ),
        "false"
    );
}

#[test]
fn an_async_generator_awaits_the_value_it_yields_and_an_ordinary_one_does_not() {
    // §27.6.3.8 step 5 — `AsyncGeneratorYield` awaits the value *before* handing it out, which
    // §27.5.3.7's ordinary `GeneratorYield` does not. That one step is the whole difference, and
    // it is what makes a rejected promise reject the promise `next()` answered rather than being
    // handed over as a value nobody looked inside.
    assert_eq!(
        run_settled(
            "var out = 'pending'; var e = new Error('x'); \
             async function* g() { yield Promise.reject(e); yield 'unreachable' } \
             var it = g(); \
             it.next().then(function (v) { out = 'resolved:' + v.value }, \
                            function (r) { out = 'rejected:' + (r === e) });",
            "out"
        ),
        "rejected:true"
    );
    // A resolved promise is unwrapped for the same reason, so the *value* is what it settled to.
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             async function* g() { yield Promise.resolve(7) } \
             g().next().then(function (v) { out = v.value + ',' + v.done });",
            "out"
        ),
        "7,false"
    );
    // …and a thenable is awaited like any other, which is what says this is `Await` and not a
    // test for `Promise`.
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             async function* g() { yield {then: function (ok) { ok(9) }} } \
             g().next().then(function (v) { out = String(v.value) });",
            "out"
        ),
        "9"
    );
    // The generator is **closed** by the rejection, so the next `next()` answers done — which is
    // what makes the first row a rejection rather than a value that happens to be a promise.
    assert_eq!(
        run_settled(
            "var out = 'pending'; var e = new Error('x'); \
             async function* g() { yield Promise.reject(e); yield 'unreachable' } \
             var it = g(); \
             it.next().then(null, function () { \
                it.next().then(function (v) { out = v.done + ',' + String(v.value) }) });",
            "out"
        ),
        "true,undefined"
    );
    // An ordinary generator does **not** await, so the promise object itself is the value. This is
    // the row that says the step belongs to §27.6 and not to `yield`.
    assert_eq!(
        run("function* g() { yield Promise.resolve(1) } \
             var v = g().next().value; typeof v.then"),
        "function"
    );
    assert_eq!(
        run("function* g() { yield {a: 1} } g().next().value.a"),
        "1"
    );
}

#[test]
fn a_functions_prototype_is_chosen_by_both_of_the_words_in_front_of_it() {
    // §27.3.3, §27.4.3 and §27.7.3 — each of the four kinds has an object of its own, and
    // `Object.getPrototypeOf` is the only route to any of the three that are not
    // `%Function.prototype%`. ViperJS asked only whether a function was a generator, which sent an
    // `async function*` to %GeneratorFunction.prototype% and an `async function` to
    // %Function.prototype% — two of the four wrong, and invisible until something asked.
    assert_eq!(
        run("var plain = Object.getPrototypeOf(function () {}); \
             var gen = Object.getPrototypeOf(function* () {}); \
             var async_ = Object.getPrototypeOf(async function () {}); \
             var both = Object.getPrototypeOf(async function* () {}); \
             [plain === Function.prototype, gen === plain, async_ === plain, both === plain, \
              gen === async_, gen === both, async_ === both].join(',')"),
        "true,false,false,false,false,false,false"
    );
    // Each carries its own `@@toStringTag`, which is what a program sees them through.
    assert_eq!(
        run(
            "[function () {}, function* () {}, async function () {}, async function* () {}] \
             .map(function (f) { return Object.prototype.toString.call(f) }).join(',')"
        ),
        "[object Function],[object GeneratorFunction],[object AsyncFunction],[object AsyncGeneratorFunction]"
    );
    // An **arrow** takes the same one its non-arrow spelling does — §15.9.4 passes
    // `%AsyncFunction.prototype%` explicitly, so an async arrow is not a special case.
    assert_eq!(
        run(
            "Object.getPrototypeOf(async () => {}) === Object.getPrototypeOf(async function () {})"
        ),
        "true"
    );
    assert_eq!(
        run("Object.getPrototypeOf(() => {}) === Function.prototype"),
        "true"
    );
    // §27.7.3 — the prototype itself is an ordinary object: not callable, and with none of the
    // properties a function has of its own.
    assert_eq!(
        run("typeof Object.getPrototypeOf(async function () {})"),
        "object"
    );
    assert_eq!(
        run("var p = Object.getPrototypeOf(async function () {}); \
             try { p(); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var p = Object.getPrototypeOf(async function () {}); \
             [p.hasOwnProperty('length'), p.hasOwnProperty('name')].join(',')"),
        "false,false"
    );
}

#[test]
fn async_function_is_a_constructor_that_nothing_can_name() {
    // §27.7 deliberately keeps `%AsyncFunction%` off the global object, so the only route to it is
    // through the prototype every async function already has — which is exactly how test262's
    // `getWellKnownIntrinsicObject` finds it, and why it has to exist for that route to lead
    // anywhere.
    assert_eq!(run("typeof globalThis.AsyncFunction"), "undefined");
    assert_eq!(
        run(
            "var AF = Object.getPrototypeOf(async function () {}).constructor; \
             [AF.name, AF.length, AF.prototype === Object.getPrototypeOf(async function () {})] \
             .join(',')"
        ),
        "AsyncFunction,1,true"
    );
    // §27.7.2 — its own `[[Prototype]]` is `%Function%`, the constructor and not the prototype.
    assert_eq!(
        run(
            "var AF = Object.getPrototypeOf(async function () {}).constructor; \
             (Object.getPrototypeOf(AF) === Function) + ',' + (AF instanceof Function)"
        ),
        "true,true"
    );
    // §27.7.3.1's attributes on the `constructor` — writable false, enumerable false, and
    // **configurable true**, which is the shape every `constructor` on a prototype has and is not
    // the shape `prototype` itself has. §27.7.2 gives that one all three false.
    assert_eq!(
        run("var p = Object.getPrototypeOf(async function () {}); \
             var d = Object.getOwnPropertyDescriptor(p, 'constructor'); \
             [d.writable, d.enumerable, d.configurable].join(',')"),
        "false,false,true"
    );
    assert_eq!(
        run(
            "var AF = Object.getPrototypeOf(async function () {}).constructor; \
             var d = Object.getOwnPropertyDescriptor(AF, 'prototype'); \
             [d.writable, d.enumerable, d.configurable].join(',')"
        ),
        "false,false,false"
    );
    // §20.2.1.1 with one word more: it builds an async function from source text.
    assert_eq!(
        run(
            "var AF = Object.getPrototypeOf(async function () {}).constructor; \
             var f = new AF('a', 'return a * 2'); typeof f + ',' + typeof f(21).then"
        ),
        "function,function"
    );
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             var AF = Object.getPrototypeOf(async function () {}).constructor; \
             new AF('a', 'return a * 2')(21).then(function (v) { out = String(v) });",
            "out"
        ),
        "42"
    );
    // §27.7.4 — an async function is **not** a constructor, and one built from source is no
    // different: it gets no `prototype` property and no `[[Construct]]`.
    assert_eq!(
        run(
            "var AF = Object.getPrototypeOf(async function () {}).constructor; \
             var f = new AF('return 1'); \
             f.hasOwnProperty('prototype') + ',' \
             + (function () { try { new f(); return 'constructed' } \
                              catch (e) { return e.constructor.name } })()"
        ),
        "false,TypeError"
    );
    assert_eq!(
        run("try { new (async function () {})(); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_parameter_that_throws_rejects_the_promise_rather_than_the_call() {
    // §15.8.4 step 2 runs `FunctionDeclarationInstantiation` as a **Completion** and step 3 rejects
    // the promise with what it produced — where §15.5.4 and §15.6.5, the generator clauses, both
    // write `Perform ?` and let it throw. One `?` is the whole difference, and it decides whether
    // `f()` returns or raises.
    //
    // §10.2.11 step 21's dead zone is the readiest way to make the instantiation fail: a parameter
    // is created uninitialised, so a default reading its own name finds nothing there.
    assert_eq!(
        run_settled(
            "var out = 'pending'; async function f(x = x) { out = 'body ran'; } \
             f().then(function () { out = 'resolved' }, function (e) { out = 'rejected:' + e.constructor.name });",
            "out"
        ),
        "rejected:ReferenceError"
    );
    // …and the call itself does not throw, which is the half a `.then` alone would not show.
    assert_eq!(
        run_settled(
            "var out = 'no throw'; async function f(x = x) {} try { f() } catch (e) { out = 'threw' }",
            "out"
        ),
        "no throw"
    );
    // Any throw from the parameters travels the same way, not only the dead zone's — the clause is
    // about where the completion goes and says nothing about what produced it.
    assert_eq!(
        run_settled(
            "var out = 'pending'; async function f(a = (function () { throw 'boom' })()) {} \
             f().then(null, function (e) { out = 'rejected:' + e });",
            "out"
        ),
        "rejected:boom"
    );
    // A destructuring parameter fails during the same instantiation and is no different.
    assert_eq!(
        run_settled(
            "var out = 'pending'; async function f({a}) {} \
             f(null).then(null, function (e) { out = 'rejected:' + e.constructor.name });",
            "out"
        ),
        "rejected:TypeError"
    );
    // An async *generator* keeps the `?`: §15.6.5 performs the instantiation and lets it throw, so
    // the call raises exactly as a sync generator's does. That is the row that stops this being
    // applied to every async body there is.
    assert_eq!(
        run(
            "var out = 'no throw'; async function* g(x = x) {} try { g() } catch (e) { out = e.constructor.name } out"
        ),
        "ReferenceError"
    );
    assert_eq!(
        run(
            "var out = 'no throw'; function* g(x = x) {} try { g() } catch (e) { out = e.constructor.name } out"
        ),
        "ReferenceError"
    );
    // And an ordinary function still throws at the call, which is what makes the async answer a
    // difference rather than a general softening.
    assert_eq!(
        run(
            "var out = 'no throw'; function f(x = x) {} try { f() } catch (e) { out = e.constructor.name } out"
        ),
        "ReferenceError"
    );
    // The body must not have run. A rejection that arrived *after* the body would be the same
    // string with a very different meaning.
    assert_eq!(
        run_settled(
            "var calls = 0; async function f(x = x) { calls = calls + 1; } f().then(null, function () {}); ",
            "calls"
        ),
        "0"
    );
}

#[test]
fn wrapping_a_sync_iterator_reads_its_next_once() {
    // §7.4.3 step 1.b.iii runs `GetIteratorFromMethod`, whose step 4 reads `next` **once** and
    // makes a record of it; step 1.b.iv hands that record to §27.1.4.1. ViperJS read it there and
    // again, so a sync iterator with a `next` *getter* saw two calls for one `yield*` — and the
    // doc said the read belonged there, "which is what makes this an Iterator Record rather than a
    // pair of lookups repeated per step". It was the pair of lookups it described.
    let source = "var reads = 0; var out = ''; \
        var obj = {}; \
        obj[Symbol.iterator] = function () { return { \
            get next() { reads += 1; return function () { return { value: 1, done: true } } } } }; \
        var g = async function* () { yield* obj; }; \
        g().next().then(function () { out = 'reads=' + reads; });";
    assert_eq!(run_settled(source, "out"), "reads=1");
    // The synchronous `yield*` reads it once too and always did — which is what says this was the
    // adapter's own step and not something about `yield*`.
    assert_eq!(
        run("var reads = 0; var obj = {}; \
             obj[Symbol.iterator] = function () { return { \
                 get next() { reads += 1; return function () { return { value: 1, done: true } } } } }; \
             function* g() { yield* obj; } g().next(); reads"),
        "1"
    );
}
