//! §20.2 — the `Function` global, and §10.4.1's bound functions.
//!
//! Checked against V8 first, like the other rows written recently. The interesting part is that a
//! bound function is *not* a function: it is an object standing in front of one, and the two
//! places that shows are `new` and a second `bind`.

use super::*;

#[test]
fn the_function_global_names_the_prototype_every_function_already_had() {
    assert_eq!(run("typeof Function"), "function");
    // §20.2.2.2 — the same object functions already inherited from, now reachable by name. This
    // is what a great many of test262's harness files reach for before anything else.
    assert_eq!(
        run("Function.prototype === Object.getPrototypeOf(function () {})"),
        "true"
    );
    assert_eq!(run("Function.prototype.constructor === Function"), "true");
    assert_eq!(run("typeof Function.prototype.call"), "function");
    assert_eq!(run("typeof Function.prototype.bind"), "function");
    // §20.2.2.2 makes `prototype` unwritable, for the reason `Object.prototype` is: everything
    // callable in the realm points at it.
    assert_eq!(
        run("var was = Function.prototype; Function.prototype = 1; Function.prototype === was"),
        "true"
    );
    // §20.2.1.1 — building a function out of source text is refused rather than faked, and says
    // so where a program can see it.
    assert_eq!(
        run("try { Function('return 1'); } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn a_bound_function_calls_its_target_with_the_receiver_and_arguments_it_was_given() {
    assert_eq!(
        run("var f = function (a, b) { return this.v + a + b; }; f.bind({v: 1}, 2)(3)"),
        "6"
    );
    assert_eq!(
        run("var f = function () { return this.v; }; f.bind({v: 'x'})()"),
        "x"
    );
    assert_eq!(
        run("var b = (function (a, b, c) { return a + b + c; }).bind(null, 1, 2); b(3)"),
        "6"
    );
    // §10.4.1.1 replaces the receiver, so *how* the bound function is called stops mattering: a
    // method call on another object still sees the bound `this`.
    assert_eq!(
        run("var f = function () { return this.n; }; var o = {n: 5}; \
             var p = {n: 9, m: f.bind(o)}; p.m()"),
        "5"
    );
    // A binding of `null` or `undefined` still meets §10.2.1.2's substitution, because the target
    // is a sloppy-mode function and that rule belongs to the target rather than to the call.
    assert_eq!(
        run("var f = function () { return this === null; }; f.bind(null)()"),
        "false"
    );
    assert_eq!(
        run("var f = function () { return this; }; typeof f.bind(undefined)()"),
        "object"
    );
    // A throw from the target is the caller's, unchanged.
    assert_eq!(
        run("var f = function () { throw new TypeError('x'); }; \
             try { f.bind(null)(); } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn binding_a_bound_function_keeps_the_first_receiver_and_puts_the_arguments_in_order() {
    // §10.4.1.1 never looks past its own target, so the *innermost* binding decides the receiver
    // — the second `bind` binds the bound function, and that one's `this` is already settled.
    assert_eq!(
        run("var f = function () { return this.n; }; f.bind({n: 'first'}).bind({n: 'second'})()"),
        "first"
    );
    // The arguments accumulate outermost-last, because each binding puts its own in front of
    // whatever the call supplies.
    assert_eq!(
        run(
            "var f = function (a, b, c) { return a + b + c; }; f.bind(null, 'a').bind(null, 'b')('c')"
        ),
        "abc"
    );
    assert_eq!(
        run("var f = function (a) { return a; }; f.bind(null, 7).bind(null)()"),
        "7"
    );
    // A chain is flattened rather than followed by recursing — a hundred of them is a hundred
    // bindings and no Rust stack at all, which is what DR-0002 asks of anything a script decides
    // the size of.
    assert_eq!(
        run("var f = function (a) { return a; }; var b = f; \
             for (var i = 0; i < 200; i = i + 1) { b = b.bind(null); } b('deep')"),
        "deep"
    );
}

#[test]
fn new_on_a_bound_function_constructs_the_target_and_ignores_the_bound_receiver() {
    // §10.4.1.2 — `new` makes its own receiver, so a bound `this` has nothing to say about it.
    // The bound *arguments* still go in front, which is the half that does apply.
    assert_eq!(
        run("function F(a) { this.a = a; } var B = F.bind(null, 5); new B().a"),
        "5"
    );
    assert_eq!(
        run("function F(a, b) { this.s = a + b; } var B = F.bind(null, 1); new B(2).s"),
        "3"
    );
    // The receiver `new` made is the one the body sees, not the object that was bound.
    assert_eq!(
        run(
            "var other = {tag: 'bound'}; function F() { this.tag = 'constructed'; } \
             var B = F.bind(other); new B().tag + ',' + other.tag"
        ),
        "constructed,bound"
    );
}

#[test]
fn bind_refuses_what_is_not_callable_and_answers_a_new_object_each_time() {
    // §20.2.3.2 step 2 — the receiver has to be callable, and "callable" is not "an object".
    assert_eq!(
        run("try { Function.prototype.bind.call(1); } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Function.prototype.bind.call({}); } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Function.prototype.bind.call(undefined); } catch (e) { e.name }"),
        "TypeError"
    );
    // Two bindings of one function are two objects — §10.4.1.3 makes a new one each time.
    assert_eq!(
        run("var f = function () {}; f.bind(null) === f.bind(null)"),
        "false"
    );
    assert_eq!(run("var f = function () {}; f.bind(null) === f"), "false");
    // §10.4.1.3 step 1 — the bound function inherits from the *target's* prototype.
    assert_eq!(
        run("var f = function () {}; Object.getPrototypeOf(f.bind(null)) === Function.prototype"),
        "true"
    );
    // §20.2.3.2 step 8 — the name is the target's with `bound ` in front. This row said `"bound "`
    // while praxis gave an ordinary function no `name` of its own, and its comment said so; §10.2.9
    // has since arrived, which is what a test that asserts the *absence* of a feature is for.
    assert_eq!(
        run("var f = function foo() {}; f.bind(null).name"),
        "bound foo"
    );
    // …and twice over, because §20.2.3.2 reads the target's `name` however that target got one.
    assert_eq!(
        run("var f = function foo() {}; f.bind(null).bind(null).name"),
        "bound bound foo"
    );
    assert_eq!(
        run("Function.prototype.call.bind(Array.prototype.join).name"),
        "bound call"
    );
    // §20.2.3.2 steps 5 and 6 — the length is what a caller still has to supply: the target's,
    // less the arguments already bound, and never below zero. Every row here is a built-in,
    // because praxis gives an ordinary function no `length` of its own to subtract from yet.
    assert_eq!(
        run("Function.prototype.call.bind(Array.prototype.join).length"),
        "1"
    );
    assert_eq!(run("Function.prototype.apply.bind(null).length"), "2");
    assert_eq!(run("Function.prototype.apply.bind(null, 1, 2).length"), "0");
    assert_eq!(
        run("Function.prototype.apply.bind(null, 1, 2, 3, 4).length"),
        "0"
    );
}

#[test]
fn the_properties_these_objects_grow_have_the_attributes_the_specification_asks_for() {
    // Nothing in a running program reads an attribute by accident, so these are the rows that
    // hold them in place — and every one was taken from V8 rather than from memory.
    //
    // §10.3.3 — a function's `length` and `name` are not writable, not enumerable, and *are*
    // configurable, which is what lets a program delete them and put its own back.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function.prototype.call.bind(null), 'length');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,true"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function.prototype.call.bind(null), 'name');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,true"
    );
    // §20.2.2.2 — `Function.prototype` is none of the three. It is the one object every callable
    // thing in the realm inherits from, so a program may not move it or hide it.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function, 'prototype');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,false"
    );
    // …while a method on it is an ordinary §17 built-in: writable and configurable so a program
    // can replace it, and never enumerable so `for`-`in` does not walk into the standard library.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function.prototype, 'bind');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,true"
    );
}

#[test]
fn a_bound_builtin_is_how_the_test_suite_reaches_a_method_generically() {
    // The shape test262's `propertyHelper.js` is built out of, and the reason this slice was
    // worth doing before anything larger: one missing global was keeping thousands of files from
    // running at all.
    assert_eq!(
        run("var join = Function.prototype.call.bind(Array.prototype.join); join([1, 2], '-')"),
        "1-2"
    );
    assert_eq!(
        run(
            "var has = Function.prototype.call.bind(Object.prototype.hasOwnProperty); \
             has({a: 1}, 'a') + ',' + has({a: 1}, 'b')"
        ),
        "true,false"
    );
    assert_eq!(
        run(
            "var push = Function.prototype.call.bind(Array.prototype.push); \
             var a = []; push(a, 7); a[0]"
        ),
        "7"
    );
}
