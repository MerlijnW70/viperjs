//! §27.5.3.7 step 7 — `yield*`, and the messages it passes in both directions.
//!
//! A `yield*` is not shorthand for a loop of `yield`s. It stands between the outer generator's
//! caller and an inner iterator and forwards everything: the values outward, the argument to `next`
//! inward, and a `throw` inward as a call to the inner iterator's own `throw`. Each of those is a
//! row here, because an implementation that only forwarded the first would pass every test about
//! what `[...g()]` gives.

use super::*;

#[test]
fn a_delegation_yields_everything_the_inner_iterator_has() {
    // Step 7.a — the outer generator produces the inner one's values as its own, and the caller
    // cannot tell there were two.
    assert_eq!(
        run(
            "function* inner() { yield 1; yield 2; } function* outer() { yield 0; yield* inner(); yield 3; } [...outer()].join(',')"
        ),
        "0,1,2,3"
    );
    // Any iterable, not only a generator: §7.4.2's `GetIterator` is what it asks for.
    assert_eq!(
        run("function* g() { yield* [7, 8]; } [...g()].join(',')"),
        "7,8"
    );
    assert_eq!(
        run("function* g() { yield* 'ab'; } [...g()].join('-')"),
        "a-b"
    );
    // Nested delegation is just delegation, and the depth is not observable.
    assert_eq!(
        run(
            "function* a() { yield 1; } function* b() { yield* a(); yield 2; } function* c() { yield* b(); yield 3; } [...c()].join(',')"
        ),
        "1,2,3"
    );
}

#[test]
fn a_delegation_evaluates_to_what_the_inner_iterator_returned() {
    // Step 7.a.v — `yield*` is an expression, and its value is the inner iterator's *return*
    // value, which is the one thing a `for`-`of` over the same iterator throws away.
    assert_eq!(
        run(
            "function* inner() { yield 1; return 'r'; } function* outer() { var v = yield* inner(); yield v; } [...outer()].join(',')"
        ),
        "1,r"
    );
    // An inner iterator that yields nothing still returns something.
    assert_eq!(
        run(
            "function* inner() { return 9; } function* outer() { yield (yield* inner()); } outer().next().value"
        ),
        "9"
    );
    // An array's iterator returns `undefined`, which is an ordinary value here.
    assert_eq!(
        run("function* g() { yield typeof (yield* []); } g().next().value"),
        "undefined"
    );
}

#[test]
fn what_the_caller_sends_goes_to_the_inner_iterator() {
    // Step 7.a.i — `next`'s argument is forwarded inward, so a two-way generator keeps working
    // through a delegation. The outer generator is a wire and not a filter.
    assert_eq!(
        run(
            "function* inner() { var a = yield 1; yield a * 2; } function* outer() { yield* inner(); } var it = outer(); it.next(); it.next(5).value"
        ),
        "10"
    );
    // …and the *first* inner `next` is sent `undefined` whatever the resumption that reached the
    // delegation was given, because step 3 starts the received completion at `undefined`. Only a
    // hand-written iterator can see that: a generator has not reached a `yield` yet, so its first
    // resumption has nowhere to put the argument either way.
    assert_eq!(
        run(
            "var seen = []; var iterable = { [Symbol.iterator]: function () { return { next: function (v) { seen.push(typeof v); return { value: 1, done: seen.length > 1 }; } }; } }; function* g() { yield* iterable; } var it = g(); it.next(7); it.next(8); seen.join(',')"
        ),
        "undefined,number"
    );
}

#[test]
fn a_throw_into_a_delegating_generator_goes_to_the_inner_iterator() {
    // Step 7.b — and this is the one an implementation forgets. The outer generator does not catch
    // the throw and does not rethrow it: it calls the *inner* iterator's `throw`, which may handle
    // it and carry on.
    assert_eq!(
        run(
            "function* inner() { try { yield 1; } catch (e) { yield 'in:' + e; } } function* outer() { yield* inner(); } var it = outer(); it.next(); it.throw('x').value"
        ),
        "in:x"
    );
    // The inner one may swallow it and finish, and then the delegation is over and the outer
    // generator carries on with the statement after the `yield*`.
    assert_eq!(
        run(
            "function* inner() { try { yield 1; } catch (e) {} } function* outer() { yield* inner(); yield 'after'; } var it = outer(); it.next(); it.throw('x').value"
        ),
        "after"
    );
    // An inner iterator with no `throw` cannot be told, so §27.5.3.7 step 7.b.iii closes it and
    // throws a TypeError — the throw is neither swallowed nor forwarded outward unchanged.
    assert_eq!(
        run(
            "function* g() { yield* [1, 2]; } var it = g(); it.next(); try { it.throw('x'); } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // …and it really is *closed* on the way, which is what tells that apart from a bare refusal.
    assert_eq!(
        run(
            "var closed = false; var iterable = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: function () { closed = true; return {}; } }; } }; function* g() { yield* iterable; } var it = g(); it.next(); try { it.throw('x'); } catch (e) {} closed"
        ),
        "true"
    );
}

#[test]
fn the_inner_result_object_is_handed_out_whole() {
    // Step 7.a.vii yields `innerResult` itself rather than rewrapping its `value`, and this is the
    // only way to see the difference: an extra property on the inner iterator's result object
    // reaches the outer caller.
    assert_eq!(
        run(
            "var iterable = { [Symbol.iterator]: function () { var first = true; return { next: function () { if (first) { first = false; return { value: 1, done: false, extra: 'here' }; } return { value: 0, done: true }; } }; } }; function* g() { yield* iterable; } g().next().extra"
        ),
        "here"
    );
    // A `done` that is truthy without being `true` ends the delegation, because §7.4.4 asks for
    // `ToBoolean` and not for an identity.
    assert_eq!(
        run(
            "var iterable = { [Symbol.iterator]: function () { return { next: function () { return { value: 'v', done: 1 }; } }; } }; function* g() { yield (yield* iterable); } g().next().value"
        ),
        "v"
    );
}

#[test]
fn the_inner_next_is_read_once_and_the_answer_must_be_an_object() {
    // §7.4.2 reads `next` when the delegation begins, so replacing it part-way through does not
    // change what the rest of the delegation calls — the same promise `for`-`of` makes.
    assert_eq!(
        run(
            "var it = { n: 0, next: function () { this.n += 1; return { value: this.n, done: this.n > 2 }; } }; var iterable = { [Symbol.iterator]: function () { return it; } }; function* g() { yield* iterable; } var out = []; var walk = g(); var step; while (!(step = walk.next()).done) { out.push(step.value); it.next = function () { return { value: 'replaced', done: true }; }; } out.join(',')"
        ),
        "1,2"
    );
    // §7.4.4 step 1 — a `next` that answers with a primitive is a TypeError, not a value.
    assert_eq!(
        run(
            "var iterable = { [Symbol.iterator]: function () { return { next: function () { return 1; } }; } }; function* g() { yield* iterable; } try { g().next(); } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // §7.4.2 step 5 — and so is an `[@@iterator]` that answers with one.
    assert_eq!(
        run(
            "function* g() { yield* { [Symbol.iterator]: function () { return 1; } }; } try { g().next(); } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // Something with no `[@@iterator]` at all is refused before any of that.
    assert_eq!(
        run("function* g() { yield* 1; } try { g().next(); } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn a_delegation_is_still_a_suspension_and_keeps_its_place() {
    // The parked execution carries the delegation's own state — which iterator, which `next`, and
    // what is being sent — so two delegating generators from one function do not share it.
    assert_eq!(
        run(
            "function* inner(n) { yield n; yield n + 1; } function* outer(n) { yield* inner(n); } var a = outer(10), b = outer(20); a.next(); b.next(); a.next().value + ':' + b.next().value"
        ),
        "11:21"
    );
    // …and a `yield*` in the middle of an expression leaves the operands around it waiting, like
    // any other suspension.
    assert_eq!(
        run(
            "function* inner() { yield 1; return 5; } function* outer() { yield 100 + (yield* inner()); } var it = outer(); it.next(); it.next().value"
        ),
        "105"
    );
}

#[test]
fn a_return_into_a_delegating_generator_leaves_the_outer_body_properly() {
    // §27.5.3.7 step 7.c. The resumption reaches the delegation, not the inner iterator's `next`,
    // so it must not be read as a value sent inward — without the check it was, and the delegation
    // carried on yielding and answered `{ value: 'after', done: false }` to a `return`.
    assert_eq!(
        run(
            "function* inner() { yield 1; yield 2; } function* outer() { yield* inner(); yield 'after'; } var it = outer(); it.next(); var r = it.return(9); r.value + ':' + r.done"
        ),
        "9:true"
    );
    // The outer generator's `finally` runs on the way out, as it does for a plain `yield`.
    assert_eq!(
        run(
            "var seen = ''; function* inner() { yield 1; } function* outer() { try { yield* inner(); } finally { seen = 'ran'; } } var it = outer(); it.next(); it.return(3); seen"
        ),
        "ran"
    );
    // …and it stays finished.
    assert_eq!(
        run(
            "function* inner() { yield 1; } function* outer() { yield* inner(); } var it = outer(); it.next(); it.return(0); var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
    // An ordinary resumption after all that is still ordinary — the mode a `return` set does not
    // survive to be read at a `yield` that never asked.
    assert_eq!(
        run(
            "function* inner() { yield 1; yield 2; } function* outer() { yield* inner(); yield 'after'; } var it = outer(); it.next(); var b = it.next(); var c = it.next(); b.value + '|' + c.value"
        ),
        "2|after"
    );
}
