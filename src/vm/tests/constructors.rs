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

#[test]
fn a_built_in_constructor_makes_its_object_from_new_target_and_not_from_itself() {
    // §10.3.2 through §10.1.13 — a built-in's `[[Construct]]` is given a newTarget and builds its
    // object with `OrdinaryCreateFromConstructor(newTarget, …)`. Invisible until `super()` existed,
    // because for a plain `new` the target *is* the function; through a subclass the two differ, and
    // reading its own intrinsic gave an instance that was not an instance of the class that made it.
    for (parent, extra) in [
        ("Error", "'x'"),
        ("TypeError", "'x'"),
        ("RangeError", "'x'"),
        ("Array", ""),
        ("Object", ""),
        ("Number", "1"),
        ("Boolean", "true"),
        ("String", "'ab'"),
        ("Date", "0"),
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ class D extends {parent} {{}} var d = new D({extra}); \
                 return (d instanceof D) + ',' + (d instanceof {parent}) + ',' \
                      + (Object.getPrototypeOf(d) === D.prototype); }})()"
            )),
            "true,true,true",
            "extends {parent}"
        );
    }
    // Two levels, because the target is inherited by each `super()` in turn rather than becoming
    // whichever class is nearest the built-in.
    assert_eq!(
        run("(function () { class D extends Error {} class E extends D {} \
             return new E() instanceof E; })()"),
        "true"
    );
}

#[test]
fn a_subclass_of_a_built_in_keeps_what_the_built_in_does() {
    // The prototype is the only thing new.target decides. Everything the parent's `[[Construct]]`
    // does to the object it made still happens, which is what makes a subclass useful rather than
    // merely correctly-shaped.
    assert_eq!(
        run("(function () { class D extends Error {} return new D('boom').message; })()"),
        "boom"
    );
    // §10.4.2's exotic `length` belongs to the object the parent made, so a subclass has it.
    assert_eq!(
        run("(function () { class D extends Array {} var d = new D(); d.push(1); d.push(2); \
             return d.length + ',' + Array.isArray(d); })()"),
        "2,true"
    );
    assert_eq!(
        run("(function () { class D extends Number {} return new D(3).valueOf() + 1; })()"),
        "4"
    );
    assert_eq!(
        run("(function () { class D extends String {} var d = new D('ab'); \
             return d.length + ',' + d[0]; })()"),
        "2,a"
    );
    assert_eq!(
        run("(function () { class D extends Date {} return new D(0).getTime(); })()"),
        "0"
    );
    // A subclass inherits the parent's prototype methods through the chain rather than by copying,
    // and may override one — which is the ordinary prototype mechanism and not a special case.
    assert_eq!(
        run("(function () { class D extends Error { toString() { return 'mine'; } } \
             return String(new D('x')); })()"),
        "mine"
    );
}

#[test]
fn a_built_in_called_without_new_is_unchanged_by_any_of_this() {
    // The other half of §20.5.1.1 step 1 — no newTarget means the active function object, so a plain
    // call still builds from the function's own `prototype`. These are the rows that would have broken
    // had `prototype_from` read the target without a fallback.
    assert_eq!(
        run("(function () { var e = Error('x'); \
             return (e instanceof Error) + ',' + e.message; })()"),
        "true,x"
    );
    assert_eq!(run("typeof Number(3)"), "number");
    assert_eq!(run("typeof String(3)"), "string");
    assert_eq!(run("typeof Boolean(0)"), "boolean");
    assert_eq!(run("typeof new Number(3)"), "object");
    // §10.1.13 falls back rather than throwing when the constructor's `prototype` is not an object,
    // which is why this is an ordinary error and not a TypeError.
    assert_eq!(
        run("(function () { var kept = Error.prototype; \
             try { Object.defineProperty(Error, 'prototype', { value: 1 }); } catch (e) {} \
             return typeof new Error('x'); })()"),
        "object"
    );
}
