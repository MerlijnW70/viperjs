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
fn an_async_generator_is_refused_rather_than_guessed() {
    // §27.6, and it is refused for the reason `for await` was until it landed: what it would
    // compile to here is a *wrong* answer rather than a missing one. An async generator's `next`
    // answers with a promise of an iterator result, so its `yield` has to settle one and the object
    // needs a queue of pending requests. Compiled as the ordinary generator it nearly works, which
    // is the worst thing it could do — 3,500 test262 files failed with plausible-looking errors
    // instead of being skipped until this row went in.
    for source in [
        "async function* g() {}",
        "var g = async function* () {};",
        "var o = { async *m() {} };",
        "class C { async *m() {} }",
    ] {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the row parses"); // a row that does not is the bug
        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported("an async generator"),
            "compiling {source:?}"
        );
    }
}
