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
