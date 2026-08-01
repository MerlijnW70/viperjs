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
