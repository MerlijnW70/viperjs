//! §10.2.11 and §15.1 — the parameters that are not simply names.
//!
//! Checked against V8 first. A default and a rest parameter both make a parameter list *not
//! simple* (§15.1.4), and that one fact decides three other things: how many arguments the
//! function says it needs, which arguments object a call builds, and whether `arguments.callee`
//! may be read at all.

use super::*;

#[test]
fn a_default_fills_in_for_undefined_and_for_nothing_at_all() {
    assert_eq!(run("(function (a = 1) { return a; })()"), "1");
    assert_eq!(run("(function (a = 1) { return a; })(2)"), "2");
    // §10.2.11 applies the default when the parameter *is* `undefined`, so passing one explicitly
    // takes it too — `f()` and `f(undefined)` agree, and that is the rule rather than a shortcut.
    assert_eq!(run("(function (a = 1) { return a; })(undefined)"), "1");
    // …and nothing else is `undefined`. Every other falsy value is passed through, which is the
    // difference between this and `a = a || 1`.
    for given in ["null", "0", "''", "false", "NaN"] {
        assert_eq!(
            run(&format!(
                "(function (a = 1) {{ return a; }})({given}) === 1"
            )),
            "false"
        );
    }
    // A default is an expression, evaluated inside the callee, so it may name the parameters to
    // its left and may call anything.
    assert_eq!(run("(function (a, b = a + 1) { return b; })(5)"), "6");
    assert_eq!(run("(function (a = 1, b = a * 2) { return b; })()"), "2");
    assert_eq!(
        run("(function (a = (function () { return 8; })()) { return a; })()"),
        "8"
    );
    // A later parameter without a default is still positional, so a `undefined` in front of it
    // takes the default and it takes its own argument.
    assert_eq!(
        run("(function (a = 1, b) { return b; })(undefined, 5)"),
        "5"
    );
    assert_eq!(
        run("(function (a = 1, b = 2, c = 3) { return a + b + c; })(undefined, 5)"),
        "9"
    );
    assert_eq!(
        run("(function () { var g = (a = 4) => a; return g(); })()"),
        "4"
    );
}

#[test]
fn a_rest_parameter_is_an_ordinary_array_of_what_was_left_over() {
    assert_eq!(run("(function (...r) { return r.length; })(1, 2, 3)"), "3");
    assert_eq!(run("(function (...r) { return r.length; })()"), "0");
    assert_eq!(
        run("(function (a, ...r) { return r.join(','); })(1, 2, 3)"),
        "2,3"
    );
    assert_eq!(run("(function (a, ...r) { return a; })(1, 2, 3)"), "1");
    // Fewer arguments than named parameters leaves it empty rather than short — there is nothing
    // past the end to collect.
    assert_eq!(run("(function (a, b, ...r) { return r.length; })(1)"), "0");
    // An *ordinary* Array, not something array-like: `arguments` is the array-like and this is
    // the thing it was added to replace.
    assert_eq!(
        run("(function (...r) { return Array.isArray(r); })()"),
        "true"
    );
    assert_eq!(
        run("(function (...r) { return r instanceof Array; })()"),
        "true"
    );
    assert_eq!(
        run("(function (...r) { return Object.getPrototypeOf(r) === Array.prototype; })()"),
        "true"
    );
    assert_eq!(
        run("(function (...r) { r.push(9); return r.length; })(1)"),
        "2"
    );
    assert_eq!(
        run("(function (...r) { return r === arguments; })(1)"),
        "false"
    );
    assert_eq!(
        run("(function () { var g = (...r) => r.length; return g(1, 2); })()"),
        "2"
    );
}

#[test]
fn length_is_what_the_function_says_it_needs_and_not_how_many_slots_it_has() {
    // §20.2.4.1 — the count stops at the first default and never counts a rest parameter. A
    // reader who expects 3 from the second row is counting slots, which is a different number
    // and the one the *call* uses.
    assert_eq!(run("(function (a, b) {}).length"), "2");
    assert_eq!(run("(function (a, b = 1, c) {}).length"), "1");
    assert_eq!(run("(function (...r) {}).length"), "0");
    assert_eq!(run("(function (a, ...r) {}).length"), "1");
    assert_eq!(run("(function () {}).length"), "0");
    assert_eq!(run("(function (a = 1) { return a; }).length"), "0");
    // Not writable and not enumerable, and *configurable* — which is the one that lets a wrapper
    // give itself the length of what it wraps.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(function (a, b) {}, 'length').writable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(function (a, b) {}, 'length').enumerable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(function (a, b) {}, 'length').configurable"),
        "true"
    );
}

#[test]
fn a_list_that_is_not_simple_gets_an_arguments_object_that_joins_nothing() {
    // §10.2.11 step 22 — a *mapped* arguments object only for a simple parameter list. This is
    // the row it is all about: with a default anywhere in the list, an index and its parameter
    // are two variables again.
    assert_eq!(
        run("(function (a) { arguments[0] = 7; return a; })(3)"),
        "7"
    );
    assert_eq!(
        run("(function (a = 1) { arguments[0] = 7; return a; })(3)"),
        "3"
    );
    assert_eq!(
        run("(function (a = 1) { a = 7; return arguments[0]; })(3)"),
        "3"
    );
    assert_eq!(
        run("(function (a = 1) { delete arguments[0]; return a; })(3)"),
        "3"
    );
    // It is still an arguments object in every other way — the values it holds are what the call
    // actually passed, which is why a default that filled in for a missing argument is not there.
    assert_eq!(
        run("(function (a = 1) { return arguments.length; })()"),
        "0"
    );
    assert_eq!(
        run("(function (a = 1) { return arguments.length; })(9)"),
        "1"
    );
    assert_eq!(
        run("(function (a = 1) { return typeof arguments[0]; })()"),
        "undefined"
    );
    assert_eq!(run("(function (a = 1) { return arguments[0]; })(3)"), "3");
    assert_eq!(
        run("(function (...r) { return arguments.length; })(1, 2)"),
        "2"
    );
    // §20.1.3.6 step 8 tags it `Arguments` because it *has* a parameter map, not because the map
    // joins anything — so an unmapped one is tagged the same.
    assert_eq!(
        run("(function (a = 1) { return Object.prototype.toString.call(arguments); })()"),
        "[object Arguments]"
    );
    assert_eq!(
        run("(function (a = 1) { return typeof arguments; })()"),
        "object"
    );
    // §10.4.4.6 step 6 — and its `callee` is poisoned with %ThrowTypeError%, both halves. A
    // function with a default is ES2015 code, and `arguments.callee` is what ES2015 closed off.
    assert_eq!(
        run("(function (a = 1) { try { return arguments.callee; } \
             catch (e) { return e.constructor.name; } })(1)"),
        "TypeError"
    );
    // …and the property itself is not enumerable and not configurable, so `for`-`in` walks the
    // indices as it does on a mapped object and nothing can define the poison away.
    let describe =
        "(function (a = 1) { var d = Object.getOwnPropertyDescriptor(arguments, 'callee'); return ";
    assert_eq!(run(&format!("{describe}d.enumerable; }})()")), "false");
    assert_eq!(run(&format!("{describe}d.configurable; }})()")), "false");
    assert_eq!(run(&format!("{describe}typeof d.get; }})()")), "function");
    // Both halves are the *same* function object — §10.2.4.1 has one %ThrowTypeError% per realm,
    // which is what makes two of them comparable with `===`.
    assert_eq!(run(&format!("{describe}d.get === d.set; }})()")), "true");
    assert_eq!(
        run(
            "(function (a = 1) { var r = ''; for (var k in arguments) { r += k; } return r; })(1, 2)"
        ),
        "01"
    );
    assert_eq!(
        run("(function (a = 1) { return Object.getOwnPropertyNames(arguments).join(','); })(1)"),
        "0,length,callee"
    );
    // …while a simple list still has it, and it is the function.
    assert_eq!(
        run("(function (a) { return arguments.callee !== undefined; })(1)"),
        "true"
    );
}

#[test]
fn a_parameter_may_be_a_pattern_and_is_taken_apart_after_its_default() {
    assert_eq!(run("(function ({a}) { return a; })({a: 1})"), "1");
    assert_eq!(
        run("(function ({a, b}) { return a + b; })({a: 1, b: 2})"),
        "3"
    );
    assert_eq!(run("(function ([a, b]) { return a + b; })([1, 2])"), "3");
    assert_eq!(run("(function ({a: x}) { return x; })({a: 5})"), "5");
    assert_eq!(run("(function ({a = 7}) { return a; })({})"), "7");
    assert_eq!(run("(function ({a: {b}}) { return b; })({a: {b: 9}})"), "9");
    assert_eq!(run("(function ([{a}]) { return a; })([{a: 6}])"), "6");
    assert_eq!(
        run("(function ([a, ...r]) { return a + ':' + r.join(','); })([1, 2, 3])"),
        "1:2,3"
    );
    assert_eq!(run("(function (x, {a}) { return x + a; })(1, {a: 2})"), "3");
    assert_eq!(
        run("(function ({a}, [b]) { return a + b; })({a: 1}, [2])"),
        "3"
    );
    assert_eq!(
        run("(function () { var f = ({a}) => a; return f({a: 4}); })()"),
        "4"
    );
    assert_eq!(
        run("(function () { var f = ([a]) => a; return f([5]); })()"),
        "5"
    );
    // §10.2.11 step 24 — the *default* stands in for a missing argument first, and the pattern
    // reads whichever of the two arrived. Written the other way round, a call with nothing would
    // try to take `undefined` apart.
    assert_eq!(run("(function ({a} = {a: 3}) { return a; })()"), "3");
    assert_eq!(run("(function ([a] = [4]) { return a; })()"), "4");
    // …and with no default there is nothing to stand in, so `undefined` is what it refuses.
    for call in ["(null)", "()"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return (function ({{a}}) {{ return a; }}){call}; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    assert_eq!(
        run(
            "(function () { try { return (function ([a]) { return a; })(5); } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // The names are the function's own bindings, so the body's `var`s and its closures share them.
    assert_eq!(
        run("(function ({a}) { var b = 2; return a + b; })({a: 1})"),
        "3"
    );
    assert_eq!(
        run("(function ({a}) { function g() { return a; } return g(); })({a: 8})"),
        "8"
    );
    assert_eq!(
        run("(function ({a}) { return typeof a; })({})"),
        "undefined"
    );
    // §20.2.4.1 — a pattern with no default still counts towards `length`, because `length` stops
    // at the first *default* and not at the first thing that is not a name.
    assert_eq!(run("(function ({a}) { return a; }).length"), "1");
    assert_eq!(run("(function ({a} = {}) { return a; }).length"), "0");
    assert_eq!(run("(function ([a], b) { return b; }).length"), "2");
    // §15.1.4 — a pattern makes the list not simple, so the arguments object joins nothing.
    assert_eq!(
        run("(function ({a}) { arguments[0] = {a: 9}; return a; })({a: 1})"),
        "1"
    );
    assert_eq!(
        run("(function ({a}) { return arguments.length; })({a: 1})"),
        "1"
    );
}
