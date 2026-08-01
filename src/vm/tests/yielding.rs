//! §15.5.5 and §27.5.3 — `yield`, and what a resumption sends back into one.
//!
//! The half of a generator that makes it worth having. `generators` next door is about the object
//! and its state machine over a body that runs straight through; here the body stops, and stopping
//! is observable in four ways at once — what the caller is handed, what the `yield` evaluates to,
//! what is still on the stack when it carries on, and what a `throw` from outside lands on.

use super::*;

#[test]
fn a_yield_answers_the_resumption_and_the_body_waits() {
    // §27.5.3.7 — the value is wrapped as `{ value, done: false }`, and `false` is the whole point
    // of it: this is what tells a `for`-`of` that there is more to come.
    assert_eq!(
        run(
            "function* g() { yield 1; yield 2; } var it = g(); var a = it.next(), b = it.next(), c = it.next(); [a.value + ':' + a.done, b.value + ':' + b.done, c.value + ':' + c.done].join(' ')"
        ),
        "1:false 2:false undefined:true"
    );
    // §15.5.5's first production — a bare `yield` yields `undefined`, and is not the same as no
    // yield at all: it still stops.
    assert_eq!(
        run(
            "function* g() { yield; return 1; } var it = g(); var a = it.next(); a.value + ':' + a.done + ':' + it.next().value"
        ),
        "undefined:false:1"
    );
    // Nothing after the last `yield` has run when it is answered, which is what "the body waits"
    // means and the thing a generator is actually for.
    assert_eq!(
        run(
            "var log = ''; function* g() { log += 'a'; yield 1; log += 'b'; } var it = g(); it.next(); var half = log; it.next(); half + '|' + log"
        ),
        "a|ab"
    );
}

#[test]
fn a_yield_evaluates_to_what_the_next_resumption_sends() {
    // §27.5.3.2 — `next`'s argument is not a signal, it is the value of the expression that
    // stopped. This is what makes a generator a two-way channel rather than a lazy list.
    assert_eq!(
        run(
            "function* g() { var a = yield 1; return a; } var it = g(); it.next(); it.next(9).value"
        ),
        "9"
    );
    // The *first* `next` has no `yield` to send to, so its argument is discarded — the body has not
    // reached one yet, and there is nowhere for the value to go.
    assert_eq!(
        run(
            "function* g() { var a = yield 1; return a; } var it = g(); it.next(7); it.next(9).value"
        ),
        "9"
    );
    // Nothing sent is `undefined`, which is an ordinary value and not an absence.
    assert_eq!(
        run("function* g() { return typeof (yield 1); } var it = g(); it.next(); it.next().value"),
        "undefined"
    );
    // A `yield` in the middle of an expression evaluates in place, so the operands around it are
    // waiting when the body carries on.
    assert_eq!(
        run(
            "function* g() { return [1, yield 'a', 3].join(''); } var it = g(); it.next(); it.next(2).value"
        ),
        "123"
    );
}

#[test]
fn a_generator_walks_a_loop_one_turn_at_a_time() {
    // What the feature is for, and a check that the parked execution really carries a whole
    // execution: the loop counter is a local, and it survives every suspension.
    assert_eq!(
        run(
            "function* upto(n) { for (var i = 0; i < n; i++) yield i; } var out = []; for (var x of upto(4)) out.push(x); out.join(',')"
        ),
        "0,1,2,3"
    );
    // …and a generator is iterable because §27.5.1 inherits `[@@iterator]`, so spread reaches it
    // without anything in between.
    assert_eq!(
        run("function* g() { yield 1; yield 2; yield 3; } [...g()].join('-')"),
        "1-2-3"
    );
    // Closing a `for`-`of` early calls `return` on the generator, which finishes it.
    assert_eq!(
        run(
            "function* g() { yield 1; yield 2; } var it = g(); for (var x of it) break; var after = it.next(); after.value + ':' + after.done"
        ),
        "undefined:true"
    );
}

#[test]
fn a_throw_into_a_suspended_body_lands_at_the_yield() {
    // §27.5.3.4 `GeneratorResumeAbrupt` — the throw happens *where the body stopped*, so a `try`
    // the body is inside catches it. That is the difference from throwing before it began, and it
    // is why the execution is put back before being unwound rather than dropped.
    assert_eq!(
        run(
            "function* g() { try { yield 1; } catch (e) { return 'caught ' + e; } } var it = g(); it.next(); it.throw('x').value"
        ),
        "caught x"
    );
    // A body with no `try` around the `yield` does not catch it, and the throw comes back out of
    // `throw` itself.
    assert_eq!(
        run(
            "function* g() { yield 1; } var it = g(); it.next(); try { it.throw('x'); } catch (e) { 'escaped ' + e }"
        ),
        "escaped x"
    );
    // …and then the generator is finished, which is what says the execution was not put back.
    assert_eq!(
        run(
            "function* g() { yield 1; } var it = g(); it.next(); try { it.throw('x'); } catch (e) {} var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
    // A `finally` runs on the way past, which is the ordinary handler machinery doing its job on a
    // throw that came from outside the body.
    assert_eq!(
        run(
            "var seen = ''; function* g() { try { yield 1; } finally { seen = 'ran'; } } var it = g(); it.next(); try { it.throw('x'); } catch (e) {} seen"
        ),
        "ran"
    );
    // Caught and then carried on: the body is still suspended afterwards, at the *next* `yield`.
    assert_eq!(
        run(
            "function* g() { try { yield 1; } catch (e) { yield 'saw ' + e; } yield 'end'; } var it = g(); it.next(); var a = it.throw('x'); var b = it.next(); a.value + '|' + b.value"
        ),
        "saw x|end"
    );
}

#[test]
fn a_body_that_throws_of_its_own_finishes_the_generator() {
    // The other direction, and the reason nothing writes a "completed" flag: the execution was
    // taken out of the generator to be run and the throw means it is not going back.
    assert_eq!(
        run(
            "function* g() { yield 1; throw 'boom'; } var it = g(); it.next(); try { it.next(); } catch (e) { 'threw ' + e }"
        ),
        "threw boom"
    );
    assert_eq!(
        run(
            "function* g() { throw 'boom'; } var it = g(); try { it.next(); } catch (e) {} var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
}

#[test]
fn a_suspension_may_be_revived_from_somewhere_else_entirely() {
    // DR-0017's invariant, as a program. A parked execution keeps no return address, so where it
    // is resumed has nothing to do with where it stopped — including a resumption reached from
    // *Rust*, which is what a built-in's callback and a coercion both are.
    //
    // Each of these was refused by an earlier reading of that record, and each is an ordinary
    // program: what the Rust call underneath is waiting for is a value, and a `yield` leaves one.
    assert_eq!(
        run(
            "function* g() { yield 41; } var it = g(); var r = [1].map(it.next.bind(it)); r[0].value + ':' + r[0].done"
        ),
        "41:false"
    );
    assert_eq!(
        run(
            "function* g() { yield 41; } var it = g(); var o = { valueOf: it.next.bind(it) }; o + ''"
        ),
        "[object Object]"
    );
    // Parked in one place and revived in another, with the halves as far apart as a script can put
    // them: suspended inside a callback, resumed at the top level afterwards.
    assert_eq!(
        run(
            "function* g() { yield 1; return 'resumed'; } var it = g(); [1].forEach(function () { it.next(); }); it.next().value"
        ),
        "resumed"
    );
    // A generator function used *as* a callback makes a generator per call and runs none of them.
    assert_eq!(
        run(
            "var made = [1, 2].map(function* (x) { yield x; }); made[0].next().value + ':' + made[1].next().value"
        ),
        "1:2"
    );
}

#[test]
fn two_generators_from_one_function_stop_in_different_places() {
    // Each call parks an execution of its own, over an environment of its own. Nothing about the
    // function object is per-instance, which is what makes this the interesting case: a machine
    // that kept the suspension anywhere but on the generator would have them share one.
    assert_eq!(
        run(
            "function* g() { yield 1; yield 2; yield 3; } var a = g(), b = g(); a.next(); a.next(); b.next(); a.next().value + ':' + b.next().value"
        ),
        "3:2"
    );
    // …and a generator resumed from inside another generator's body is two live executions at
    // once, which is what says the frame stack really did give one of them up.
    assert_eq!(
        run(
            "function* inner() { yield 'i'; } function* outer(x) { yield x.next().value; yield 'o'; } var it = outer(inner()); it.next().value + it.next().value"
        ),
        "io"
    );
}

#[test]
fn a_return_into_a_suspended_body_finishes_it_without_running_its_finally() {
    // §27.5.3.4 with a *return* completion, and the half of it that is here. The generator is
    // finished and the argument becomes the answer, which is right for every body that is not
    // inside a `try`:
    assert_eq!(
        run(
            "function* g() { yield 1; yield 2; } var it = g(); it.next(); var r = it.return(5); r.value + ':' + r.done"
        ),
        "5:true"
    );
    assert_eq!(
        run(
            "function* g() { yield 1; } var it = g(); it.next(); it.return(5); var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );

    // …and the half that is **not**: a `finally` around the `yield` should run on the way out and
    // does not. `throw` manages it because a throw is a value the interpreter already knows how to
    // unwind with; a *return completion* is not — praxis compiles `finally` inline at each exit the
    // compiler can see, so there is no way to inject one at a `yield` from the outside. Doing it
    // needs a completion the unwinder carries past `catch` handlers and stops at `finally` ones,
    // which is a slice of its own and is also what `yield*` will need to forward a return.
    //
    // The row below asserts the wrong answer on purpose, and the test's *name* says so. It is a
    // marker rather than a claim: it will fail the day the completion arrives, which is the signal
    // to delete it and write the real one. A silent gap here would be found by test262 instead, and
    // much later.
    assert_eq!(
        run(
            "var seen = ''; function* g() { try { yield 1; } finally { seen = 'ran'; } } var it = g(); it.next(); it.return(5); seen"
        ),
        ""
    );
}
