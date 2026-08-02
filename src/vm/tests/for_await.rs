//! §14.7.5.7 and §27.1.4 — `for await`, and the adapter that lets it walk a sync iterator.
//!
//! Every row needs [`super::run_settled`]: the loop lives inside an `async` function, so nothing it
//! does has happened when the script's last statement runs. That is the feature rather than an
//! inconvenience — a `for await` that finished synchronously would not be one.

use super::*;

#[test]
fn a_for_await_walks_an_ordinary_iterable_one_turn_at_a_time() {
    // §7.4.3 step 1.b — an array has no `[@@asyncIterator]`, so its sync iterator is wrapped and
    // the loop talks to the wrapper. Nothing about the array had to change.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { for await (var x of [1, 2, 3]) out.push(x); } f();",
            "out.join(',')"
        ),
        "1,2,3"
    );
    // A string, whose iterator walks code points.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { for await (var c of 'abc') out += c; } f();",
            "out"
        ),
        "abc"
    );
    // …and a sync generator, which is the shape this is most often written over.
    assert_eq!(
        run_settled(
            "var out = []; function* g() { yield 1; yield 2; } async function f() { for await (var x of g()) out.push(x); } f();",
            "out.join(',')"
        ),
        "1,2"
    );
    // An empty iterable runs the body no times and still finishes.
    assert_eq!(
        run_settled(
            "var ran = false, done = false; async function f() { for await (var x of []) ran = true; done = true; } f();",
            "ran + ':' + done"
        ),
        "false:true"
    );
}

#[test]
fn each_value_is_awaited_before_the_body_sees_it() {
    // §27.1.4.4 — the point of the whole protocol. The sync iterator yields *promises*, and the
    // body is handed what they settle with rather than the promises themselves.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { for await (var x of [Promise.resolve(1), Promise.resolve(2)]) out.push(x); } f();",
            "out.join(',')"
        ),
        "1,2"
    );
    // A mixture, because `PromiseResolve` passes a plain value through as readily as it unwraps.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { for await (var x of [1, Promise.resolve(2), 3]) out.push(typeof x + ':' + x); } f();",
            "out.join(' ')"
        ),
        "number:1 number:2 number:3"
    );
    // A rejected one throws *into the loop*, at the point the body would have run — so a `try`
    // around the loop catches it and the walk stops there.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { try { for await (var x of [1, Promise.reject('no'), 3]) out.push(x); } catch (e) { out.push('caught ' + e); } } f();",
            "out.join(',')"
        ),
        "1,caught no"
    );
}

#[test]
fn an_async_iterator_of_its_own_is_used_before_the_sync_one() {
    // §7.4.3 asks for `[@@asyncIterator]` first, and only falls back when there is none. An object
    // with both must use the async one, which is the only way to tell the order was right.
    let both = "var it = { [Symbol.asyncIterator]: function () { var n = 0; return { next: function () { n += 1; return Promise.resolve({ value: 'a' + n, done: n > 2 }); } }; }, [Symbol.iterator]: function () { return [9, 9][Symbol.iterator](); } };";
    assert_eq!(
        run_settled(
            &format!(
                "{both} var out = []; async function f() {{ for await (var x of it) out.push(x); }} f();"
            ),
            "out.join(',')"
        ),
        "a1,a2"
    );
    // The async iterator's results are awaited as a whole, so a `next` that answers a promise of a
    // result object works — which is what an async generator will hand back.
    assert_eq!(
        run_settled(
            "var it = { [Symbol.asyncIterator]: function () { var n = 0; return { next: function () { n += 1; return Promise.resolve({ value: n, done: n > 3 }); } }; } }; var total = 0; async function f() { for await (var x of it) total += x; } f();",
            "total"
        ),
        "6"
    );
    // §7.3.10 — a `[@@asyncIterator]` that is present and *not callable* is a TypeError rather
    // than a fall-through to the sync one. Absent and unusable are different answers.
    assert_eq!(
        run_settled(
            "var out = ''; var it = { [Symbol.asyncIterator]: 1, [Symbol.iterator]: function () { return [7][Symbol.iterator](); } }; async function f() { for await (var x of it) {} } f().then(null, function (e) { out = e.name; });",
            "out"
        ),
        "TypeError"
    );
    // …and a `[@@iterator]` that is present and not callable is refused on the same terms.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { for await (var x of { [Symbol.iterator]: 1 }) {} } f().then(null, function (e) { out = e.name; });",
            "out"
        ),
        "TypeError"
    );
    // An iterator method that answers a primitive is a TypeError too — §7.4.2 step 3.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { for await (var x of { [Symbol.asyncIterator]: function () { return 1; } }) {} } f().then(null, function (e) { out = e.name; });",
            "out"
        ),
        "TypeError"
    );
    // Something that is neither is a TypeError, and it rejects the function's promise rather than
    // throwing at the call — §7.4.3 runs inside the body.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { for await (var x of 1) {} } f().then(null, function (e) { out = e.name; });",
            "out"
        ),
        "TypeError"
    );
}

#[test]
fn leaving_early_closes_the_iterator_and_waits_for_it() {
    // §7.4.9 with the async hint. A `break` tells the iterator, and the wrapper forwards that to
    // the sync iterator's own `return`.
    assert_eq!(
        run_settled(
            "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) break; } f();",
            "closed"
        ),
        "true"
    );
    // A throw from the body closes it too, and the throw still reaches the function's promise.
    assert_eq!(
        run_settled(
            "var closed = false, out = ''; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) throw 'stop'; } f().then(null, function (e) { out = closed + ':' + e; });",
            "out"
        ),
        "true:stop"
    );
    // An iterable with no `return` is simply left, which §7.4.9 allows and is why an array works.
    assert_eq!(
        run_settled(
            "var out = 'not reached'; async function f() { for await (var x of [1, 2, 3]) break; out = 'left cleanly'; } f();",
            "out"
        ),
        "left cleanly"
    );
    // §7.4.11 step 3.d — the close is **awaited**, so a `return` that answers a rejected promise
    // takes the loop with it rather than being dropped on the floor.
    assert_eq!(
        run_settled(
            "var out = 'not rejected'; var it = { [Symbol.asyncIterator]: function () { return { next: function () { return Promise.resolve({ value: 1, done: false }); }, return: function () { return Promise.reject('close failed'); } }; } }; async function f() { for await (var x of it) break; } f().then(null, function (e) { out = 'rejected:' + e; });",
            "out"
        ),
        "rejected:close failed"
    );
    // …and step 6 — what it settled with has to be an object.
    assert_eq!(
        run_settled(
            "var out = ''; var it = { [Symbol.asyncIterator]: function () { return { next: function () { return Promise.resolve({ value: 1, done: false }); }, return: function () { return Promise.resolve(1); } }; } }; async function f() { for await (var x of it) break; } f().then(null, function (e) { out = e.name; });",
            "out"
        ),
        "TypeError"
    );
    // A sync iterator whose `return` is present and not callable is a TypeError too, and the
    // *message* is the assertion rather than the name. §7.3.10 puts the check at the **lookup**,
    // and without it the value would simply be called and fail there — the same kind of error one
    // step later, which is why only the wording tells the two apart.
    assert_eq!(
        run_settled(
            "var out = ''; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: 1 }; } }; async function f() { for await (var x of it) break; } f().then(null, function (e) { out = e.message; });",
            "out"
        ),
        "this iterator's method is not a function"
    );
    // §7.4.11 step 4 — but on the way out of a **throw** the close's own failure is discarded and
    // the body's exception is what arrives. Both halves matter: the `return` really is called, and
    // what it rejects with never reaches the caller.
    assert_eq!(
        run_settled(
            "var called = false, out = ''; var it = { [Symbol.asyncIterator]: function () { return { next: function () { return Promise.resolve({ value: 1, done: false }); }, return: function () { called = true; return Promise.reject('close failed'); } }; } }; async function f() { for await (var x of it) throw 'from the body'; } f().then(null, function (e) { out = called + ':' + e; });",
            "out"
        ),
        "true:from the body"
    );
    // A `return` out of the enclosing function closes it as well, which is the third way out.
    assert_eq!(
        run_settled(
            "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) return 'early'; } f();",
            "closed"
        ),
        "true"
    );
}

#[test]
fn the_loop_is_a_loop_and_the_awaits_do_not_disturb_it() {
    // The body may itself await, and the loop's own state survives it — the whole thing is one
    // parked execution, so `total` and the iterator both belong to it.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { var total = 0; for await (var x of [1, 2, 3]) { total += await x; } return total; } f().then(function (v) { out = v; });",
            "out"
        ),
        "6"
    );
    // `continue` skips the rest of the body without closing anything.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { for await (var x of [1, 2, 3, 4]) { if (x % 2) continue; out.push(x); } } f();",
            "out.join(',')"
        ),
        "2,4"
    );
    // Nested loops each keep their own iterator.
    assert_eq!(
        run_settled(
            "var out = []; async function f() { for await (var a of [1, 2]) for await (var b of ['x', 'y']) out.push(a + b); } f();",
            "out.join(',')"
        ),
        "1x,1y,2x,2y"
    );
    // …and everything after the loop still runs, which says the handler came down.
    assert_eq!(
        run_settled(
            "var out = ''; async function f() { for await (var x of [1]) {} return 'after'; } f().then(function (v) { out = v; });",
            "out"
        ),
        "after"
    );
}

#[test]
fn a_next_that_fails_does_not_close_the_iterator() {
    // §14.7.5.7 step 3 — the `?` on `next()`, on `IteratorComplete` and on `IteratorValue`
    // propagates **without** closing. Only the binding and the body reach `IteratorClose`, at steps
    // 3.n and 3.q. An iterator whose own `next` threw is not one to tell the walk is over, and
    // calling `return` on it is one call too many that a counting iterator can see.
    let throwing = "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { throw 'from next'; }, return: function () { closed = true; return {}; } }; } };";
    assert_eq!(
        run(&format!(
            "{throwing} try {{ for (var x of it) {{}} }} catch (e) {{}} closed"
        )),
        "false"
    );
    assert_eq!(
        run_settled(
            &format!(
                "{throwing} async function f() {{ for await (var x of it) {{}} }} f().then(null, function () {{}});"
            ),
            "closed"
        ),
        "false"
    );
    // …and the other half, which is what stops this being a licence to never close: an abrupt
    // completion from the **body** still closes, and so does one from the binding.
    let yielding = "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: 1, done: false }; }, return: function () { closed = true; return {}; } }; } };";
    assert_eq!(
        run(&format!(
            "{yielding} try {{ for (var x of it) {{ throw 'from body'; }} }} catch (e) {{}} closed"
        )),
        "true"
    );
    assert_eq!(
        run_settled(
            &format!(
                "{yielding} async function f() {{ for await (var x of it) {{ throw 'from body'; }} }} f().then(null, function () {{}});"
            ),
            "closed"
        ),
        "true"
    );
}

#[test]
fn a_rejected_value_closes_the_sync_iterator_underneath() {
    // §27.1.4.4 step 13 — the sync side has no idea its value was a promise, so when that promise
    // rejects nothing else can tell it the walk ended badly. The *wrapper* closes it, which is the
    // one place that knows both halves. Without this the iterator is left open for good.
    assert_eq!(
        run_settled(
            "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: Promise.reject('no'), done: false }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) {} } f().then(null, function () {});",
            "closed"
        ),
        "true"
    );
    // Step 6 — the same close when `PromiseResolve` itself throws on the way in.
    assert_eq!(
        run_settled(
            "var closed = false; var p = Promise.resolve(0); Object.defineProperty(p, 'constructor', { get: function () { throw new Error('x'); } }); var it = { [Symbol.iterator]: function () { return { next: function () { return { value: p, done: false }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) {} } f().then(null, function () {});",
            "closed"
        ),
        "true"
    );
    // …and **not** when the sync result said it was done: step 13 is guarded on `done` being false,
    // because an iterator that has finished has nothing left to be told.
    assert_eq!(
        run_settled(
            "var closed = false; var it = { [Symbol.iterator]: function () { return { next: function () { return { value: Promise.reject('no'), done: true }; }, return: function () { closed = true; return {}; } }; } }; async function f() { for await (var x of it) {} } f().then(null, function () {});",
            "closed"
        ),
        "false"
    );
}

#[test]
fn every_pass_of_a_for_await_binds_afresh_exactly_as_a_for_of_does() {
    // §14.7.5.7 `ForIn/OfBodyEvaluation` step 3.e — a `let` or `const` head gets a **new**
    // environment per iteration, so a closure made in the body keeps the value that pass had. The
    // synchronous loop was given that when block scoping landed; `for await` is the same clause and
    // was not, so its head was one binding for the whole walk and every closure answered with the
    // last value.
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             async function f() { \
                 var fs = []; \
                 for await (const x of [1, 2, 3]) fs.push(function () { return x; }); \
                 return fs.map(function (g) { return g(); }).join(','); \
             } \
             f().then(function (v) { out = v; });",
            "out"
        ),
        "1,2,3"
    );
    // `var` in the head is function-scoped and must *not* be copied — one binding, last value, in
    // both loops. The two rows together are what say the copy follows the declaration kind.
    assert_eq!(
        run_settled(
            "var out = 'pending'; \
             async function f() { \
                 var fs = []; \
                 for await (var y of [1, 2, 3]) fs.push(function () { return y; }); \
                 return fs.map(function (g) { return g(); }).join(','); \
             } \
             f().then(function (v) { out = v; });",
            "out"
        ),
        "3,3,3"
    );
}
