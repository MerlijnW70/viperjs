//! §14.3.3 — taking an object apart in a declaration.
//!
//! Checked against V8 first. Two things are worth reading twice: a default is compared against
//! `undefined` and not against absence, and it is *evaluated* only when it is needed — so a
//! property that is present and `undefined` takes the default, and one that is present and
//! anything else never runs it.
//!
//! An array pattern is not a shorter object one: it drives an *iterator*, so the source need not
//! be an Array and need not have a `length`. What that buys, and what it costs in closing, is the
//! second half of this file.

use super::*;

#[test]
fn a_pattern_reads_the_properties_it_names() {
    assert_eq!(run("(function () { var {a} = {a: 1}; return a; })()"), "1");
    assert_eq!(
        run("(function () { var {a, b} = {a: 1, b: 2}; return a + b; })()"),
        "3"
    );
    // `{a: x}` reads `a` and binds `x` — the key and the name are two things, and shorthand is
    // the case where they happen to be spelled the same.
    assert_eq!(
        run("(function () { var {a: x} = {a: 5}; return x; })()"),
        "5"
    );
    assert_eq!(run("(function () { let {a} = {a: 3}; return a; })()"), "3");
    assert_eq!(
        run("(function () { const {a} = {a: 4}; return a; })()"),
        "4"
    );
    // A property that is not there binds `undefined` rather than failing.
    assert_eq!(
        run("(function () { var {a} = {}; return typeof a; })()"),
        "undefined"
    );
    // Every kind of key a property may have, since the pattern reads one the same way an ordinary
    // member expression would.
    assert_eq!(
        run("(function () { var {'a b': v} = {'a b': 6}; return v; })()"),
        "6"
    );
    assert_eq!(
        run("(function () { var {0: v} = ['q']; return v; })()"),
        "q"
    );
    assert_eq!(
        run("(function () { var k = 'z'; var {[k]: v} = {z: 8}; return v; })()"),
        "8"
    );
    // A primitive source is *coercible*, so it is read through the object it stands for.
    assert_eq!(
        run("(function () { var {length: n} = 'ab'; return n; })()"),
        "2"
    );
    // The same key twice binds twice, which is legal for a `var` and reads oddly and is right.
    assert_eq!(
        run("(function () { var {a, a: b} = {a: 1}; return a + ',' + b; })()"),
        "1,1"
    );
    // The names are ordinary bindings afterwards — a copy, not a window onto the source.
    assert_eq!(
        run("(function () { var o = {a: 1}; var {a} = o; a = 2; return o.a; })()"),
        "1"
    );
    assert_eq!(run("var {a} = {a: 1}; a"), "1");
    assert_eq!(
        run(
            "(function () { try { const {a} = {a: 1}; a = 2; return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}

#[test]
fn a_default_is_for_undefined_and_is_run_only_when_it_is_wanted() {
    assert_eq!(run("(function () { var {a = 7} = {}; return a; })()"), "7");
    assert_eq!(
        run("(function () { var {a = 7} = {a: 1}; return a; })()"),
        "1"
    );
    // §14.3.3 compares against `undefined`, not against absence — so a property that is *there*
    // and `undefined` takes the default, and `null` does not. The pair is the whole rule.
    assert_eq!(
        run("(function () { var {a = 7} = {a: undefined}; return a; })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var {a = 7} = {a: null}; return a; })()"),
        "null"
    );
    // …and it is *evaluated* only when it is needed, which is observable through a side effect.
    assert_eq!(
        run("(function () { var n = 0; var {a = (n++, 7)} = {a: 1}; return n; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var n = 0; var {a = (n++, 7)} = {}; return n + ',' + a; })()"),
        "1,7"
    );
}

#[test]
fn a_pattern_nests_because_what_it_reads_may_be_taken_apart_too() {
    assert_eq!(
        run("(function () { var {a: {b}} = {a: {b: 9}}; return b; })()"),
        "9"
    );
    // A default on the way down, so the inner pattern has something to read — the idiom for
    // "this whole group is optional".
    assert_eq!(
        run("(function () { var {a: {b = 2} = {}} = {}; return b; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { var {a: {b: {c}}} = {a: {b: {c: 'deep'}}}; return c; })()"),
        "deep"
    );
    assert_eq!(
        run("(function () { function f() { var {a} = {a: 1}; return a; } return f(); })()"),
        "1"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (var i = 0; i < 2; i++) { var {a} = {a: i}; r += a; } \
             return r; })()"
        ),
        "01"
    );
}

#[test]
fn undefined_and_null_are_refused_before_anything_is_read() {
    // §14.3.3.7 step 1 is `RequireObjectCoercible`, and the empty pattern is what makes it a step
    // of its own: with a property in it the first read would throw anyway, and with none there is
    // nothing to read and it throws all the same.
    for source in ["null", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var {{}} = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var {{a}} = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // Everything else is coercible, including a primitive with no properties worth reading.
    assert_eq!(run("(function () { var {} = 5; return 'ok'; })()"), "ok");
    assert_eq!(run("(function () { var {} = 'a'; return 'ok'; })()"), "ok");
    assert_eq!(run("(function () { var {} = true; return 'ok'; })()"), "ok");
}

#[test]
fn an_array_pattern_drives_an_iterator_rather_than_reading_indices() {
    assert_eq!(
        run("(function () { var [a, b] = [1, 2]; return a + ',' + b; })()"),
        "1,2"
    );
    assert_eq!(
        run("(function () { var [a] = [1, 2, 3]; return a; })()"),
        "1"
    );
    // An iterator that runs out leaves the rest `undefined` rather than failing.
    assert_eq!(
        run("(function () { var [a, b, c] = [1]; return a + ',' + typeof b + ',' + typeof c; })()"),
        "1,undefined,undefined"
    );
    // An elision takes a turn and binds nothing — not the same as a name that gets `undefined`.
    assert_eq!(
        run("(function () { var [, b] = [1, 2]; return b; })()"),
        "2"
    );
    assert_eq!(run("(function () { var [a = 9] = []; return a; })()"), "9");
    assert_eq!(
        run("(function () { var [a = 9] = [undefined]; return a; })()"),
        "9"
    );
    assert_eq!(
        run("(function () { var [a = 9] = [null]; return a; })()"),
        "null"
    );
    assert_eq!(run("(function () { let [a] = [3]; return a; })()"), "3");
    assert_eq!(run("(function () { const [a] = [4]; return a; })()"), "4");
    assert_eq!(run("var [a] = [1]; a"), "1");
    // Any iterable, which is the whole difference from reading `0`, `1`, `2`: a String iterates
    // by code point and has no elements at all.
    assert_eq!(
        run("(function () { var [a, b] = 'xy'; return a + b; })()"),
        "xy"
    );
    // A rest element collects what is left, as an ordinary Array.
    assert_eq!(
        run("(function () { var [a, ...r] = [1, 2, 3]; return a + ':' + r.join(','); })()"),
        "1:2,3"
    );
    assert_eq!(
        run("(function () { var [...r] = [1, 2]; return Array.isArray(r) + ',' + r.length; })()"),
        "true,2"
    );
    assert_eq!(
        run("(function () { var [a, ...r] = [1]; return r.length; })()"),
        "0"
    );
    // …and patterns nest through each other in both directions.
    assert_eq!(
        run("(function () { var [[a], [b]] = [[1], [2]]; return a + b; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { var [{a}] = [{a: 5}]; return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var {a: [b]} = {a: [7]}; return b; })()"),
        "7"
    );
}

#[test]
fn an_array_pattern_stops_asking_a_spent_iterator_and_closes_one_it_abandons() {
    // §8.6.2 — the `done` latches, so two names over a one-element iterable call `next` twice and
    // not three times. A counter in `next` is the only thing that can see it.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { n++; return {value: n, done: n > 1}; }}; }; \
             var [a, b] = o; return n + ':' + a + ',' + typeof b; })()"
        ),
        "2:1,undefined"
    );
    // §8.6.2 step 4 — a pattern that finishes while the iterator has not abandons it, and says
    // so. This is the case an object pattern has no equivalent of.
    let endless = "var o = {}; o[Symbol.iterator] = function () { return {\
                   next: function () { return {value: 1, done: false}; }, \
                   return: function () { c = true; return {}; }}; };";
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {endless} var [a] = o; return c; }})()"
        )),
        "true"
    );
    // …and one that ran out on its own is already finished with.
    assert_eq!(
        run(
            "(function () { var c = false; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: true}; }, \
             return: function () { c = true; return {}; }}; }; var [a] = o; return c; })()"
        ),
        "false"
    );
    // An error while binding abandons it too — a default that throws is the easiest way in.
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {endless} \
             try {{ var [a = (function () {{ throw new Error('x'); }})()] = o; }} catch (e) {{}} \
             return c; }})()"
        )),
        "true"
    );
    // What is not iterable says so, and that includes a plain object — which an object pattern
    // would have taken apart happily.
    for source in ["5", "null", "{}"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var [a] = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
}
