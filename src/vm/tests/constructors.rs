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
