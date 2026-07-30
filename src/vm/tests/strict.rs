//! §11.2.1 — strict mode, in the three things it decides once the program is running.
//!
//! The parser has always known which code is strict; nothing at run time could ask, so all three of
//! these answered the sloppy way. What makes them worth grouping is that each is a place where sloppy
//! mode **silently does nothing** and strict mode throws — so an engine that cannot tell them apart is
//! not merely incomplete, it gives a program the wrong answer and no way to find out.
//!
//! Every row here has its sloppy twin beside it. That is deliberate: a test that only checks the
//! strict half passes just as well against an engine that throws in both.

use super::*;

#[test]
fn a_refused_write_throws_in_strict_code_and_is_silent_in_sloppy() {
    // §6.2.5.6 `PutValue` step 6.d — the whole of what "strict mode catches your mistakes" means for
    // assignment. Three ways a write can be refused, and each behaves both ways.
    for refused in [
        "var o = Object.freeze({ a: 1 }); o.a = 2;",
        "var o = {}; Object.defineProperty(o, 'a', { value: 1, writable: false }); o.a = 2;",
        "var o = Object.preventExtensions({}); o.a = 2;",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ 'use strict'; {refused} return 'no'; }})()"
            ))
            .split(':')
            .next()
            .unwrap_or_default(),
            "thrown [object]",
            "strict: {refused}"
        );
        // …and the same source without the directive finishes, having done nothing.
        assert_eq!(
            run(&format!("(function () {{ {refused} return 'done'; }})()")),
            "done",
            "sloppy: {refused}"
        );
    }
    // The value is unchanged either way — a sloppy refusal is a refusal and not a write.
    assert_eq!(
        run("(function () { var o = Object.freeze({ a: 1 }); o.a = 2; return o.a; })()"),
        "1"
    );
    // §13.15.2 — the assignment still *evaluates* to what was assigned, which is what makes the
    // sloppy case invisible to anything but a later read.
    assert_eq!(
        run("(function () { var o = Object.freeze({ a: 1 }); return (o.a = 2); })()"),
        "2"
    );
}

#[test]
fn a_refused_delete_throws_in_strict_code_and_answers_false_in_sloppy() {
    // §13.5.1.2 step 5.b — the same rule from the other side, and the one that makes `delete` worth
    // checking the answer of at all.
    assert_eq!(
        run("(function () { 'use strict'; \
             try { delete Object.prototype; return 'no'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run("(function () { return delete Object.prototype; })()"),
        "false"
    );
    // A *configurable* property deletes in both, which is the row that says the throw is about the
    // refusal rather than about `delete` in strict code.
    assert_eq!(
        run("(function () { 'use strict'; var o = { a: 1 }; \
             return delete o.a; })()"),
        "true"
    );
}

#[test]
fn a_strict_function_keeps_the_receiver_it_was_given() {
    // §10.2.1.2 step 3 — the substitution belongs to the **function**, not to the shape of the call,
    // so a strict function handed `undefined` sees `undefined` however it was reached. This is the
    // rule that makes `this` in a strict function a reliable thing to test.
    assert_eq!(
        run("(function () { 'use strict'; function f() { return this; } return String(f()); })()"),
        "undefined"
    );
    assert_eq!(
        run("(function () { function f() { return this; } return typeof f(); })()"),
        "object"
    );
    // `null` stays `null`, and a **primitive** is handed over unwrapped — §7.1.18's wrapping is for a
    // sloppy function only.
    assert_eq!(
        run(
            "(function () { 'use strict'; function f() { return this; } \
             return String(f.call(null)) + ',' + String(f.call(7)); })()"
        ),
        "null,7"
    );
    // §15.7.1 — a class body is strict code without a directive of its own, which is why a method
    // borrowed and called with nothing sees `undefined` rather than the global object.
    assert_eq!(
        run("(function () { class C { m() { return typeof this; } } \
             return C.prototype.m.call(undefined); })()"),
        "undefined"
    );
    // …and strictness is inherited by everything nested, so a function written inside a strict one is
    // strict without saying so.
    assert_eq!(
        run("(function () { 'use strict'; \
             function outer() { function inner() { return this; } return String(inner()); } \
             return outer(); })()"),
        "undefined"
    );
    // An arrow has no receiver of its own, so it answers whatever the strict function around it does.
    assert_eq!(
        run(
            "(function () { 'use strict'; function f() { return (() => this)(); } \
             return String(f()); })()"
        ),
        "undefined"
    );
}
