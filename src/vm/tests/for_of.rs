//! §14.7.5.7 — `for`-`of`, and the four ways out of it that have to tell the iterator.
//!
//! Checked against V8 first. The loop itself is the easy half. The half worth testing is
//! §7.4.9 `IteratorClose`: a `break`, a labelled break crossing this loop, a `return`, and a throw
//! all leave early, and each has to call the iterator's `return` exactly once — while the ordinary
//! end of the loop, where the iterator said it was done, must not call it at all.

use super::*;

/// An iterable whose iterator never ends and records that it was closed, as source text.
///
/// Written once because every `IteratorClose` row needs one and they differ only in what the loop
/// around it does. `record` is the statement that runs when `return` is called.
fn endless(record: &str) -> String {
    format!(
        "var o = {{}}; o[Symbol.iterator] = function () {{ return {{\
         next: function () {{ return {{value: 1, done: false}}; }}, \
         return: function () {{ {record} return {{}}; }}}}; }};"
    )
}

#[test]
fn a_for_of_walks_whatever_hands_it_an_iterator() {
    assert_eq!(
        run("(function () { var r = ''; for (var x of [1, 2, 3]) { r += x; } return r; })()"),
        "123"
    );
    assert_eq!(
        run("(function () { var r = 0; for (var x of []) { r++; } return r; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var r = ''; for (let x of [1, 2]) { r += x; } return r; })()"),
        "12"
    );
    assert_eq!(
        run("(function () { var r = ''; for (const x of 'abc') { r += x; } return r; })()"),
        "abc"
    );
    // A String iterates by code point, so an astral character is one turn of two units.
    assert_eq!(
        run(
            "(function () { var r = ''; for (var x of '\\ud83d\\ude00a') { r += x.length; } \
             return r; })()"
        ),
        "21"
    );
    // An iterator is itself iterable, which is what makes these work at all.
    assert_eq!(
        run("(function () { var r = ''; for (var x of [1, 2].keys()) { r += x; } return r; })()"),
        "01"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (var e of ['a'].entries()) { r += e[0] + e[1]; } \
             return r; })()"
        ),
        "0a"
    );
    // …and a hand-written one is no different, which is the whole point of the protocol.
    assert_eq!(
        run("(function () { var n = 0; var o = {}; \
             o[Symbol.iterator] = function () { return {next: function () { n++; \
             return {value: n, done: n > 2}; }}; }; \
             var r = ''; for (var x of o) { r += x; } return r; })()"),
        "12"
    );
    // §14.7.5.5 — a `var` in the head is not the loop's, so it outlives it.
    assert_eq!(
        run(
            "(function () { var r = ''; for (var x of [1, 2, 3]) { r += x; } \
             return r + typeof x; })()"
        ),
        "123number"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (var x of [1, 2]) { for (var y of [3, 4]) { \
             r += x + '' + y; } } return r; })()"
        ),
        "13142324"
    );
}

#[test]
fn what_is_not_iterable_says_so_rather_than_looping_for_ever() {
    // §7.4.2 — no `@@iterator`, or one that is not callable, and the call fails. A number and a
    // plain object have none; `null` has nothing to be asked at all.
    for source in [
        "5",
        "null",
        "undefined",
        "{}",
        "(function () { var o = {}; o[Symbol.iterator] = 5; return o; })()",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ for (var x of {source}) {{}} return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // §7.4.5 step 3 — a `next` that answers a primitive is a TypeError and not a loop that never
    // ends. Without the check, `done` would be read off the primitive's prototype as `undefined`,
    // which is falsy, and the loop would run for ever.
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return 5; }}; }; \
             try { for (var x of o) {} return 'ok'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // A throw from `next` is the iterator's own and travels out unchanged.
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { throw new Error('boom'); }}; }; \
             try { for (var x of o) {} return 'ok'; } catch (e) { return e.message; } })()"
        ),
        "boom"
    );
    // §7.4.2 asks for the iterator **once** and reads `next` once, so neither is re-read per turn.
    assert_eq!(
        run("(function () { var n = 0; var o = {}; \
             o[Symbol.iterator] = function () { n++; return [1].values(); }; \
             for (var x of o) {} return n; })()"),
        "1"
    );
    assert_eq!(
        run(
            "(function () { var calls = 0; var it = {next: function () { calls++; \
             return {done: true}; }}; var o = {}; o[Symbol.iterator] = function () { return it; }; \
             for (var x of o) {} \
             it.next = function () { return {value: 9, done: false}; }; return calls; })()"
        ),
        "1"
    );
}

#[test]
fn every_early_way_out_closes_the_iterator_and_the_ordinary_way_does_not() {
    let closed = endless("c = true;");
    // §7.4.9 — a `break`, a `return`, and a throw are each a way of leaving the loop before the
    // iterator said it was finished, and each has to say so.
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {closed} for (var x of o) {{ break; }} return c; }})()"
        )),
        "true"
    );
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {closed} \
             (function () {{ for (var x of o) {{ return; }} }})(); return c; }})()"
        )),
        "true"
    );
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {closed} \
             try {{ for (var x of o) {{ throw new Error('e'); }} }} catch (e) {{}} return c; }})()"
        )),
        "true"
    );
    // …and the throw path takes *no* handler down on its way, because the throw already consumed
    // the loop's own. Popping one there would steal the next handler out, and a `try` around the
    // loop would stop catching what came after it.
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {closed}              try {{ try {{ for (var x of o) {{ throw new Error('a'); }} }} catch (e) {{}}              throw new Error('b'); }} catch (e) {{ return e.message; }} }})()"
        )),
        "b"
    );
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {closed}              try {{ for (var x of o) {{ throw new Error('a'); }} }}              catch (e) {{ return 'inner ' + e.message; }} }})()"
        )),
        "inner a"
    );
    // A `continue` stays in the loop and has nothing to tell it.
    assert_eq!(
        run(&format!(
            "(function () {{ var n = 0; var c = 0; {} \
             for (var x of o) {{ n++; if (n < 3) continue; break; }} return c; }})()",
            endless("c++;")
        )),
        "1"
    );
    // …and an iterator that ran to its own end is already finished with — §7.4.5 says so, and
    // calling `return` on it would be one call the specification does not make.
    assert_eq!(
        run(
            "(function () { var c = 0; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: true}; }, \
             return: function () { c++; return {}; }}; }; \
             for (var x of o) {} return c; })()"
        ),
        "0"
    );
    // An iterator with no `return` at all is simply left, which is step 4 and not an error.
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: false}; }}; }; \
             for (var x of o) { break; } return 'left quietly'; })()"
        ),
        "left quietly"
    );
}

#[test]
fn a_labelled_break_closes_every_loop_it_crosses_and_a_close_is_called_once() {
    // Innermost first, and both of them — §7.4.9 is applied per loop left, not once for the jump.
    assert_eq!(
        run("(function () { var c = ''; \
             function it(n) { var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: false}; }, \
             return: function () { c += n; return {}; }}; }; return o; } \
             outer: for (var x of it('A')) { for (var y of it('B')) { break outer; } } \
             return c; })()"),
        "BA"
    );
    // A labelled *continue* leaves the inner loop and stays in the outer, so it closes one.
    assert_eq!(
        run("(function () { var r = ''; \
             outer: for (var x of [1, 2]) { for (var y of [3, 4]) { \
             if (y === 4) continue outer; r += x + '' + y; } } return r; })()"),
        "1323"
    );
    // §7.4.9 step 5 answers a *throw* completion before it ever looks at what `return` gave
    // back, so on the way out of a throw a primitive answer is not an error — the original throw
    // is what travels on. The same iterator left by a `break` does raise one, below: the two
    // paths differ in exactly this, and nothing else would show it.
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.iterator] = function () {              return {next: function () { return {value: 1, done: false}; },              return: function () { return 5; }}; };              try { for (var x of o) { throw new Error('mine'); } }              catch (e) { return e.constructor.name + ':' + e.message; } })()"
        ),
        "Error:mine"
    );
    // §7.4.9 step 6 — on a deliberate exit, a `return` that answers a primitive is a TypeError.
    // And it is called *once*: the loop's handler comes down before the closing, or the throw
    // from the closing would be caught by the very handler that would close again.
    assert_eq!(
        run(
            "(function () { var c = 0; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: false}; }, \
             return: function () { c++; return 5; }}; }; \
             try { for (var x of o) { break; } return 'ok'; } \
             catch (e) { return e.constructor.name + ',' + c; } })()"
        ),
        "TypeError,1"
    );
}
