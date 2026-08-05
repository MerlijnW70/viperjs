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

#[test]
fn strict_code_may_not_create_a_global_by_assigning_to_an_undeclared_name() {
    // §6.2.5.6 `PutValue` step 6 — a reference that resolves to nothing is created on the global
    // object in **sloppy** code and is a ReferenceError in strict. This is the rule people reach
    // for strict mode to get, and ViperJS silently created the global instead.
    assert_eq!(
        run(
            "'use strict'; var e = 'none'; try { neverDeclared = 1 } catch (x) { e = x.constructor.name } e"
        ),
        "ReferenceError"
    );
    // The strictness is the *code's* that made the reference, so a strict function inside a sloppy
    // script throws and the script around it does not.
    assert_eq!(
        run("(function () { 'use strict'; var e = 'none'; \
               try { alsoNeverDeclared = 1 } catch (x) { e = x.constructor.name } return e })()"),
        "ReferenceError"
    );
    assert_eq!(run("sloppyIsFine = 5; sloppyIsFine"), "5");
    assert_eq!(
        run("(function () { sloppyToo = 6; return globalThis.sloppyToo })()"),
        "6"
    );
    // Nothing that *is* declared is affected, by any of the three ways a global comes to exist.
    assert_eq!(
        run("'use strict'; var declared = 1; declared = 2; declared"),
        "2"
    );
    assert_eq!(run("'use strict'; function f() {} f = 2; f"), "2");
    assert_eq!(
        run(
            "globalThis.assigned = 1; (function () { 'use strict'; assigned = 2; return assigned })()"
        ),
        "2"
    );
    // §9.1.1.4.1's `HasBinding` on the global object record is **`HasProperty`**, so it walks the
    // prototype chain: `toString` resolves at the top level through `Object.prototype`, and
    // assigning to it is not assigning to nothing. An own-property test would throw here.
    assert_eq!(
        run("(function () { 'use strict'; toString = 1; return typeof toString })()"),
        "number"
    );
    // A `let` or a `const` at the top level is the global *declarative* record rather than the
    // object, so it never reaches this instruction at all — and a `const` still refuses for its
    // own reason, which is a different error.
    assert_eq!(run("'use strict'; let a = 1; a = 2; a"), "2");
    assert_eq!(
        run(
            "'use strict'; const b = 1; var e = 'none'; try { b = 2 } catch (x) { e = x.constructor.name } e"
        ),
        "TypeError"
    );
}

#[test]
fn a_getter_that_deletes_itself_leaves_a_reference_with_no_binding() {
    // §9.1.1.4.5 `SetMutableBinding` step 2 — the property was there when the reference was made
    // and is gone by the time it is written, so strict code gets a ReferenceError. That is the
    // same answer step 6 gives above, which is why one check serves both: what matters is whether
    // the binding is there *now*.
    assert_eq!(
        run(
            "Object.defineProperty(globalThis, 'gx', {configurable: true, \
               get: function () { delete globalThis.gx; return 2 }}); \
             var e = 'none'; \
             (function () { 'use strict'; try { gx ^= 3 } catch (x) { e = x.constructor.name } })(); \
             e + ',' + ('gx' in globalThis)"
        ),
        "ReferenceError,false"
    );
    // …and the same program in sloppy code puts the property back, which is what makes the row
    // above about strictness rather than about the getter.
    assert_eq!(
        run(
            "Object.defineProperty(globalThis, 'gy', {configurable: true, \
               get: function () { delete globalThis.gy; return 2 }}); \
             gy ^= 3; globalThis.gy + ',' + ('gy' in globalThis)"
        ),
        "1,true"
    );
}
