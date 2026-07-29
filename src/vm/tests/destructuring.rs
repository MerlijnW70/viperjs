//! §14.3.3 — taking an object apart in a declaration.
//!
//! Checked against V8 first. Two things are worth reading twice: a default is compared against
//! `undefined` and not against absence, and it is *evaluated* only when it is needed — so a
//! property that is present and `undefined` takes the default, and one that is present and
//! anything else never runs it.
//!
//! Array patterns are refused for now. They need `GetIterator` and a step per element, and the
//! sequencing that takes is a slice of its own; `statements` has the row that says so.

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
