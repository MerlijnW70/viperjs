//! §7.3.13 and §10.3 — which functions may be written after `new`, and which may only be called.
//!
//! Checked against V8 first. Being callable and being constructible are two properties, not one,
//! and nearly every built-in has only the first: `Object` is a constructor and `Object.keys` is
//! not, though `typeof` says `"function"` for both.

use super::*;

/// `new` in front of `source`, and the name of what came of it.
fn constructed(source: &str) -> String {
    run(&format!(
        "(function () {{ try {{ new {source}; return 'ok'; }} \
         catch (e) {{ return e.constructor.name; }} }})()"
    ))
}

#[test]
fn a_built_in_is_a_constructor_only_where_its_clause_says_so() {
    // The ones §10.3.2 gives a `[[Construct]]`.
    for constructor in [
        "Object()",
        "Array()",
        "String('a')",
        "Number(1)",
        "Boolean(true)",
        "Error('x')",
        "TypeError('x')",
    ] {
        assert_eq!(constructed(constructor), "ok");
        assert_eq!(run(&format!("typeof new {constructor}")), "object");
    }
    // …and everything else installed beside them, which is nearly all of it. Each of these is a
    // function by `typeof` and refuses `new` all the same.
    for method in ["Object.keys({})", "Math.max()", "''.charAt()", "[].push"] {
        assert_eq!(constructed(method), "TypeError");
    }
    assert_eq!(run("typeof Object.keys"), "function");
    assert_eq!(run("typeof Math.max"), "function");
}

#[test]
fn a_function_written_in_the_language_may_be_constructed_unless_it_is_an_arrow() {
    assert_eq!(constructed("(function f() {})()"), "ok");
    assert_eq!(
        run("(function () { function f() { this.x = 1; } return new f().x; })()"),
        "1"
    );
    // §15.3 — an arrow has no `[[Construct]]`, for the same reason it has no `this`: there is
    // nothing for `new` to give it.
    assert_eq!(constructed("(() => {})"), "TypeError");
    assert_eq!(run("typeof (() => {})"), "function");
}

#[test]
fn a_bound_function_is_a_constructor_exactly_when_its_target_is() {
    // §10.4.1.3 step 2 — the answer is the target's, decided when the bound function was made and
    // not by looking at what it wraps at the moment `new` is written.
    assert_eq!(
        run(
            "(function () { function f() { this.x = 1; } var b = f.bind(null); return new b().x; })()"
        ),
        "1"
    );
    assert_eq!(run("typeof new (Array.bind(null))()"), "object");
    assert_eq!(constructed("(Math.max.bind(null))"), "TypeError");
    assert_eq!(constructed("((() => {}).bind(null))"), "TypeError");
    // Binding twice does not change the answer either way, which is what makes it the *target's*
    // property rather than something a wrapper decides for itself.
    assert_eq!(constructed("(Math.max.bind(null).bind(null))"), "TypeError");
    assert_eq!(
        run("(function () { function f() { this.x = 2; } \
             return new (f.bind(null).bind(null))().x; })()"),
        "2"
    );
}

#[test]
fn new_target_is_the_constructor_a_new_named_and_undefined_in_every_other_call() {
    // §13.3.12 — the one thing that tells a function how it was reached. Nothing else does: the
    // receiver differs between `f()` and `new f()`, but a sloppy function handed the global object
    // cannot tell that from having been called as a method of it.
    assert_eq!(
        run("(function () { function f() { return new.target; } return f(); })()"),
        "undefined"
    );
    assert_eq!(
        run("(function () { function f() { return new.target === f; } return f.call(null); })()"),
        "false"
    );
    // The value is the constructor *object* and not a flag, which is what makes it usable: it is
    // read for its `prototype` and its `name`, and §15.7's derived classes take it further.
    assert_eq!(
        run("(function () { function f() { this.t = new.target; } return new f().t === f; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { function f() { this.p = new.target.prototype === f.prototype; } \
             return new f().p; })()"),
        "true"
    );
}

#[test]
fn a_call_inside_a_construction_does_not_take_the_constructions_new_target_with_it() {
    // §9.1.1.3 — `[[NewTarget]]` belongs to the *call*, so a plain call made from inside a
    // construction has none of its own and the construction still has its when the call comes back.
    // A machine that set the register and never put it back would answer the first of these
    // correctly and the second with `f`.
    assert_eq!(
        run("(function () { \
             function g() { return new.target; } \
             function f() { this.inner = g(); this.outer = new.target === f; } \
             var made = new f(); return made.inner + ' ' + made.outer; })()"),
        "undefined true"
    );
    // …and the same on the way out of a throw, which is a second path that pops frames. The
    // handler is in the constructor, so `g`'s frame is abandoned rather than returned from.
    assert_eq!(
        run("(function () { \
             function g() { throw 1; } \
             function f() { try { g(); } catch (e) {} this.outer = new.target === f; } \
             return new f().outer; })()"),
        "true"
    );
}

#[test]
fn an_arrow_answers_the_new_target_of_the_call_it_was_written_in() {
    // §13.3.12 reaches outward exactly as `this` does (§15.3), and the reach is settled where the
    // arrow was *written*: an arrow that read the register at call time would answer `undefined`
    // here, because by then the construction that made it has returned.
    // §10.2.2 step 13 hands back the arrow rather than the constructed object, an arrow being an
    // object — so this reads the capture after the construction that made it has returned, which is
    // the moment a register could not answer for.
    assert_eq!(
        run("(function () { function f() { return () => new.target; } \
             var arrow = new f(); return arrow() === f; })()"),
        "true"
    );
    // The same again through a property, so that the answer does not depend on that step.
    assert_eq!(
        run("(function () { function f() { this.arrow = () => new.target; } \
             var made = new f(); return made.arrow() === f; })()"),
        "true"
    );
    // An arrow written in a plain call captured `undefined`, which is the same rule and the other
    // answer — so the capture is of what was in force and not of the arrow's own way in.
    assert_eq!(
        run("(function () { function f() { return () => new.target; } \
             return f()(); })()"),
        "undefined"
    );
    // Two arrows from two calls disagree, which is what makes it a capture rather than a constant.
    assert_eq!(
        run("(function () { function f() { this.arrow = () => new.target; } \
             function g() { this.arrow = () => new.target; } \
             return (new f().arrow() === f) + ' ' + (new g().arrow() === f); })()"),
        "true false"
    );
}
