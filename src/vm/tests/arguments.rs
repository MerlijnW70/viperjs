//! §10.4.4 — the arguments object, and the parameter map that makes it exotic.
//!
//! Every row was checked against V8 first. The interesting ones are not about `arguments.length`;
//! they are about `arguments[0]` and the first parameter being *one variable*, and about the three
//! ways a program can separate them again.

use super::*;

#[test]
fn an_arguments_object_holds_what_the_call_was_given() {
    assert_eq!(
        run("function f() { return arguments.length; } f(1, 2, 3)"),
        "3"
    );
    assert_eq!(run("function f() { return arguments.length; } f()"), "0");
    // The count is what was *passed*, not what was declared — which is the whole reason the
    // object exists.
    assert_eq!(
        run("function f(a, b, c) { return arguments.length; } f(1)"),
        "1"
    );
    assert_eq!(
        run("function f() { return arguments[0]; } f('a', 'b')"),
        "a"
    );
    assert_eq!(
        run("function f() { return typeof arguments[5]; } f(1)"),
        "undefined"
    );
    assert_eq!(
        run("function f() { return typeof arguments; } f()"),
        "object"
    );
    // §10.4.4.4 step 15 — a mapped arguments object has a `callee`, and it is the function.
    assert_eq!(
        run("function f() { return arguments.callee === f; } f()"),
        "true"
    );
    // §20.1.3.6 step 8 — tagged by its parameter map, which is the only thing telling it from an
    // ordinary object with numeric keys.
    assert_eq!(
        run("function f() { return Object.prototype.toString.call(arguments); } f()"),
        "[object Arguments]"
    );
    // Ordinary in every other way: an ordinary prototype, and `length` and `callee` are §17
    // properties that `for`-`in` does not walk.
    assert_eq!(
        run("function f(a) { return arguments instanceof Object; } f(1)"),
        "true"
    );
    assert_eq!(
        run(
            "function f() { var r = ''; for (var k in arguments) { r = r + k; } return r; } f(1, 2)"
        ),
        "01"
    );
    assert_eq!(
        run(
            "function f() { var d = Object.getOwnPropertyDescriptor(arguments, 'length'); \
             return d.enumerable; } f(1)"
        ),
        "false"
    );
    // At the top level of a script there is no call, so the name is an ordinary global that is
    // not there.
    assert_eq!(run("typeof arguments"), "undefined");
}

#[test]
fn an_index_and_its_parameter_are_one_variable() {
    // The whole of §10.4.4, in four rows. Nothing is copied in either direction: writing one name
    // is writing the other, because they are the same binding.
    assert_eq!(
        run("function f(a) { arguments[0] = 2; return a; } f(1)"),
        "2"
    );
    assert_eq!(
        run("function f(a) { a = 5; return arguments[0]; } f(1)"),
        "5"
    );
    assert_eq!(
        run("function f(a, b) { arguments[1] = 'x'; return b; } f(1, 2)"),
        "x"
    );
    assert_eq!(
        run("function f(a) { arguments[0] = 'w'; return arguments[0] + ',' + a; } f('v')"),
        "w,w"
    );
    // §10.4.4.1 — and a *descriptor* reports the parameter's value too, which is what says the
    // object is not holding a copy that happens to agree.
    assert_eq!(
        run("function f(a) { a = 8; \
             return Object.getOwnPropertyDescriptor(arguments, '0').value; } f(1)"),
        "8"
    );
    // The map reaches only as far as there are parameters: an argument past the last one is an
    // ordinary property with nothing behind it.
    assert_eq!(
        run("function f(a) { arguments[1] = 'x'; return arguments[1]; } f(1, 2)"),
        "x"
    );
    // …and only the canonical spelling of an index is joined. `"01"` is not the key `"1"`, so it
    // is not a property of the object at all.
    assert_eq!(run("function f(a) { return arguments['0']; } f('z')"), "z");
    assert_eq!(
        run("function f(a) { return typeof arguments['01']; } f('z')"),
        "undefined"
    );
}

#[test]
fn the_link_can_be_broken_and_never_comes_back() {
    // §10.4.4.5 step 4 — deleting the index separates the two names for good: the parameter goes
    // on changing and the object no longer hears about it.
    assert_eq!(
        run("function f(a) { delete arguments[0]; a = 9; return typeof arguments[0]; } f(1)"),
        "undefined"
    );
    // …and the parameter is untouched by the delete, because what was removed is a property and
    // not a variable.
    assert_eq!(
        run("function f(a) { delete arguments[0]; return a; } f(1)"),
        "1"
    );
    // §10.4.4.2 step 5 — redefining the index as an accessor breaks it too, a parameter being no
    // sort of accessor.
    assert_eq!(
        run("function f(a) { Object.defineProperty(arguments, '0', \
             {get: function () { return 3; }}); return a; } f(1)"),
        "1"
    );
    // …and so does making it unwritable. The order in §10.4.4.2 is the interesting part: the
    // value is written through *first* and the link is broken after, so a define that does both
    // leaves the parameter changed.
    assert_eq!(
        run("function f(a) { Object.defineProperty(arguments, '0', \
             {value: 7, writable: false}); return a; } f(1)"),
        "7"
    );
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0', {writable: false}); \
             a = 9; return arguments[0]; } f(1)"
        ),
        "1"
    );
    // An ordinary define still writes through, which is the case the three above are exceptions
    // to rather than the rule — and the link survives it, so the parameter goes on being read
    // through the index afterwards.
    assert_eq!(
        run("function f(a) { Object.defineProperty(arguments, '0', {value: 7}); return a; } f(1)"),
        "7"
    );
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0', {value: 7});              a = 9; return arguments[0]; } f(1)"
        ),
        "9"
    );
    // Only §10.4.4.2's two changes break it. Making the property non-configurable is neither of
    // them, so the two names are still one variable afterwards.
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0', {configurable: false});              a = 9; return arguments[0]; } f(1)"
        ),
        "9"
    );
    // §10.4.4.2 step 3 — a define the ordinary rules *refuse* changes nothing, including the
    // mapping. Non-configurable and then asked to become an accessor is the one refusal a still
    // mapped index can be handed.
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0', {configurable: false});              try { Object.defineProperty(arguments, '0', {get: function () { return 3; }}); }              catch (e) {} a = 9; return arguments[0]; } f(1)"
        ),
        "9"
    );
    // …and once an accessor define is *allowed*, a later define that puts a plain value back does
    // not rejoin them. Nothing does.
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0',              {get: function () { return 3; }}); Object.defineProperty(arguments, '0',              {value: 5, writable: true, configurable: true}); return a; } f(1)"
        ),
        "1"
    );
    // Nor does assigning the index again after a delete: the property comes back, the link does
    // not.
    assert_eq!(
        run("function f(a) { delete arguments[0]; arguments[0] = 4; return a; } f(1)"),
        "1"
    );
    assert_eq!(
        run(
            "function f(a) { Object.defineProperty(arguments, '0', {value: 7, writable: false});              a = 9; return arguments[0]; } f(1)"
        ),
        "7"
    );
}

#[test]
fn each_call_has_its_own_and_an_arrow_has_none() {
    // A nested *function* has an arguments object of its own, so the outer one is not what it
    // reads.
    assert_eq!(
        run("function f(a) { return (function () { return arguments[0]; })('inner'); } f('outer')"),
        "inner"
    );
    assert_eq!(
        run(
            "function f(a) { var g = function () { return arguments.length; }; return g(1, 2); } f(9)"
        ),
        "2"
    );
    // §15.3 — an arrow has none, so the name reaches the function it is written inside, exactly
    // as `this` does. This is why the enclosing function has to be told that an arrow within it
    // used the name: it is the one that builds the object.
    assert_eq!(
        run("function outer(a) { var g = () => arguments[0]; return g(); } outer('lex')"),
        "lex"
    );
    assert_eq!(
        run("function outer(a) { var g = () => arguments.length; return g(); } outer(1, 2, 3)"),
        "3"
    );
    // §10.2.11 step 19 — a parameter or a `var` of that name takes it, and then there is no
    // arguments object at all.
    assert_eq!(
        run("function f(arguments) { return arguments; } f('shadowed')"),
        "shadowed"
    );
    assert_eq!(
        run("function f() { var arguments = 5; return arguments; } f(1)"),
        "5"
    );
    // …but a bare `var arguments` does not, and this is the half of step 19 that surprises. It
    // names the parameters, the hoisted functions and the lexical declarations; `var` is none of
    // them, so the object is made and the declaration finds it already in the slot.
    assert_eq!(
        run("function f() { var arguments; return typeof arguments; } f(1)"),
        "object"
    );
    assert_eq!(
        run("function f(a) { var arguments; return arguments[0]; } f(7)"),
        "7"
    );
    // A declaration that does take the name puts its own thing there, whichever kind it is.
    assert_eq!(
        run("function f() { function arguments() {} return typeof arguments; } f(1)"),
        "function"
    );
    assert_eq!(
        run("function f() { let arguments; return typeof arguments; } f(1)"),
        "undefined"
    );
    // It outlives the call, which is why the collector has to keep the call's variables: the
    // object handed back is still reading them.
    assert_eq!(
        run("function f(a) { return arguments; } typeof f(1)"),
        "object"
    );
    assert_eq!(
        run("function f(a) { return arguments; } f('kept')[0]"),
        "kept"
    );
}

#[test]
fn caller_and_arguments_are_refused_through_any_function_rather_than_being_absent() {
    // §10.2.4 `AddRestrictedFunctionProperties` — ES5's two ways of walking the call stack from
    // inside a function, closed off. What replaced them is not their *absence*: they are accessor
    // properties whose getter and setter both throw, so a program that asks gets a TypeError. The
    // difference is what a feature test can see — `undefined` would say this engine has not got
    // round to them.
    assert_eq!(
        run("function f() {} try { f.caller; 'read' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("function f() {} try { f.arguments; 'read' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("function f() {} try { f.caller = 1; 'wrote' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Through an arrow, a method and a bound function too, since the pair is on the prototype they
    // all share rather than on any of them.
    assert_eq!(
        run("try { (() => {}).caller; 'read' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { (function () {}).bind(null).arguments; 'read' } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );

    // §10.2.4 puts them on `Function.prototype` **and on no individual function**, which is where
    // ES2015 moved them: ES5 gave every strict function a pair of its own, and this is the check
    // that tells the two arrangements apart.
    assert_eq!(
        run("function f() { 'use strict'; } \
             Object.prototype.hasOwnProperty.call(f, 'caller') + ',' + \
             Object.prototype.hasOwnProperty.call(Function.prototype, 'caller')"),
        "false,true"
    );
    // The descriptor: an accessor, both halves the same function, not enumerable, and
    // **configurable** — the one attribute these two do not share with §17's usual shape, so that
    // a host or a script may replace them.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function.prototype, 'caller'); \
             [typeof d.get, d.get === d.set, d.enumerable, d.configurable].join(',')"
        ),
        "function,true,false,true"
    );
}

#[test]
fn there_is_one_function_that_refuses_and_every_restricted_property_shares_it() {
    // §10.2.4.1 %ThrowTypeError% is a single object per realm, and a program can see that it is:
    // both halves of both restricted accessors are the same function, and so is the poisoned
    // `callee` of an unmapped arguments object. Anything else would be four functions that behave
    // alike, which is not what the specification says and is observable with `===`.
    assert_eq!(
        run(
            "var caller = Object.getOwnPropertyDescriptor(Function.prototype, 'caller'); \
             var args = Object.getOwnPropertyDescriptor(Function.prototype, 'arguments'); \
             var callee = Object.getOwnPropertyDescriptor( \
                 function () { 'use strict'; return arguments; }(), 'callee'); \
             [caller.get === caller.set, caller.get === args.get, args.get === args.set, \
              caller.get === callee.get, callee.get === callee.set].join(',')"
        ),
        "true,true,true,true,true"
    );
    // Its own shape, which is stricter than any other built-in's. `name` is the **empty string**
    // and not `"ThrowTypeError"` — that is the specification's name for it, not one a program may
    // read — and `length` and `name` are non-writable *and* non-configurable where §17 makes every
    // other built-in's configurable.
    assert_eq!(
        run(
            "var T = Object.getOwnPropertyDescriptor(Function.prototype, 'caller').get; \
             var n = Object.getOwnPropertyDescriptor(T, 'name'); \
             var l = Object.getOwnPropertyDescriptor(T, 'length'); \
             [n.value === '', n.writable, n.configurable, l.value, l.writable, l.configurable] \
                 .join(',')"
        ),
        "true,false,false,0,false,false"
    );
    // …and it is shut: not extensible, and frozen, so nothing can be hung on the one function every
    // restricted property in the realm shares. It also has no `prototype` of its own and is not a
    // constructor.
    assert_eq!(
        run(
            "var T = Object.getOwnPropertyDescriptor(Function.prototype, 'caller').get; \
             [Object.isExtensible(T), Object.isFrozen(T), \
              Object.prototype.hasOwnProperty.call(T, 'prototype'), \
              Object.getPrototypeOf(T) === Function.prototype].join(',')"
        ),
        "false,true,false,true"
    );
    // Calling it is the refusal itself, which is what the accessors are for.
    assert_eq!(
        run(
            "var T = Object.getOwnPropertyDescriptor(Function.prototype, 'caller').get; \
             try { T(); 'called' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_strict_functions_arguments_is_not_joined_to_its_parameters() {
    // §10.2.11 step 22 asks **two** questions: the parameter list must be simple *and* the code
    // must be sloppy. Asking only about the list gave a strict function the mapped object, and the
    // join is observable in both directions.
    assert_eq!(
        run("function f(a) { 'use strict'; a = 2; return arguments[0]; } f(1)"),
        "1"
    );
    assert_eq!(
        run("function f(a) { 'use strict'; arguments[0] = 2; return a; } f(1)"),
        "1"
    );
    // …where a sloppy one with the same parameters is one variable seen two ways, which is what
    // makes this a test of the condition rather than of mapping being gone.
    assert_eq!(
        run("function g(a) { a = 2; return arguments[0]; } g(1)"),
        "2"
    );
    assert_eq!(
        run("function g(a) { arguments[0] = 2; return a; } g(1)"),
        "2"
    );
    // Strictness inherited from the code around it counts too — §11.2.1 makes it a property of the
    // body, not of the directive being written in that body.
    assert_eq!(
        run("'use strict'; function f(a) { a = 2; return arguments[0]; } f(1)"),
        "1"
    );
    // And `callee` follows the same split: the function on a mapped object, poisoned on an
    // unmapped one — §10.4.4.6 step 6, and the idiom ES2015 was closing off.
    assert_eq!(
        run("function g(a) { return arguments.callee === g; } g(1)"),
        "true"
    );
    assert_eq!(
        run(
            "function f(a) { 'use strict'; try { return arguments.callee; } \
             catch (e) { return e.constructor.name; } } f(1)"
        ),
        "TypeError"
    );
}

#[test]
fn arguments_is_the_running_functions_however_many_scopes_are_between() {
    // §10.2.11 step 19 gives the *function* the object, and the compiler decides whether to build
    // one by whether the name resolved to its slot. That comparison used to be against depth zero,
    // which is only right when nothing has opened a scope since — so an ordinary block with a `let`
    // in it made the read resolve one hop out, told the **enclosing** function to build an object,
    // and left this one reading a slot nothing had filled. It threw.
    assert_eq!(
        run("function f() { { let z = 1; return arguments[0]; } } f(7)"),
        "7"
    );
    assert_eq!(
        run("function f() { { let z = 1; return typeof arguments; } } f()"),
        "object"
    );
    // Every kind of scope that adds a hop, since each reaches the same comparison.
    assert_eq!(
        run("function f() { for (let i = 0; i < 1; i++) { return arguments[0]; } } f(4)"),
        "4"
    );
    assert_eq!(
        run("function f() { switch (1) { case 1: let q = 1; return arguments[0]; } } f(5)"),
        "5"
    );
    assert_eq!(
        run("function f() { try { throw 1; } catch (e) { return arguments[0]; } } f(6)"),
        "6"
    );
    assert_eq!(
        run("function f() { var o = {}; with (o) { return arguments[0]; } } f(7)"),
        "7"
    );
    // …and `typeof` is a read like any other, which is a second path to the same question and did
    // not ask it: `with (o) { typeof arguments }` answered `"undefined"` with the object in reach.
    assert_eq!(
        run("function f() { var o = {}; with (o) { return typeof arguments; } } f()"),
        "object"
    );
    // The other side of the comparison still holds: an **arrow** has none of its own, so its read
    // resolves further out and the function around it is the one that builds the object.
    assert_eq!(
        run("function f(a) { { let z = 1; return (() => arguments[0])(); } } f(3)"),
        "3"
    );
}
