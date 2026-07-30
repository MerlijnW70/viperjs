//! §15.7 — classes, in the part that does not need `extends`, a field or a private name.
//!
//! The rows that matter most are the ones that separate a class body from an object literal, because
//! that is where an implementation that reused the literal path would look right and be wrong: a
//! method here is not enumerable, `prototype` cannot be replaced, and the constructor refuses a call
//! without `new`.

use super::*;

#[test]
fn a_class_puts_its_methods_on_the_prototype_and_its_statics_on_itself() {
    assert_eq!(
        run("(function () { class C { m() { return 1; } } return new C().m(); })()"),
        "1"
    );
    assert_eq!(
        run("(function () { class C { m() {} } return C.prototype.hasOwnProperty('m'); })()"),
        "true"
    );
    // A static is on the constructor and *not* on the prototype, which is the whole of what the
    // keyword does.
    assert_eq!(
        run("(function () { class C { static s() { return 2; } } return C.s(); })()"),
        "2"
    );
    assert_eq!(
        run("(function () { class C { static s() {} } \
             return C.hasOwnProperty('s') + ',' + C.prototype.hasOwnProperty('s'); })()"),
        "true,false"
    );
    // …and an instance method is not on the instance either: it is inherited.
    assert_eq!(
        run("(function () { class C { m() {} } var o = new C(); \
             return o.hasOwnProperty('m') + ',' + ('m' in o); })()"),
        "false,true"
    );
    // The same name may be both, and the two do not collide.
    assert_eq!(
        run(
            "(function () { class C { m() { return 'proto'; } static m() { return 'static'; } } \
             return C.m() + ',' + new C().m(); })()"
        ),
        "static,proto"
    );
}

#[test]
fn a_class_method_is_not_enumerable_and_an_object_literal_method_is() {
    // §15.7.14 against §15.4.5 — the single runtime difference between the two bodies, and the one
    // an implementation that shared their code path would get wrong in a way nothing else notices.
    assert_eq!(
        run("(function () { class C { m() {} } return Object.keys(C.prototype).length; })()"),
        "0"
    );
    assert_eq!(run("Object.keys({m: function () {}}).length"), "1");
    assert_eq!(
        run("(function () { class C { m() {} } var seen = []; \
             for (var k in new C()) seen.push(k); return seen.length; })()"),
        "0"
    );
    // Not absent, though — the property is there and is writable and configurable.
    assert_eq!(
        run("(function () { class C { m() {} } \
             var d = Object.getOwnPropertyDescriptor(C.prototype, 'm'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "true,false,true"
    );
    // A static method carries the same three.
    assert_eq!(
        run("(function () { class C { static s() {} } \
             var d = Object.getOwnPropertyDescriptor(C, 's'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "true,false,true"
    );
    // `constructor` is not enumerable either, and is the reason `Object.keys` above is empty rather
    // than one short.
    assert_eq!(
        run("(function () { class C {} \
             var d = Object.getOwnPropertyDescriptor(C.prototype, 'constructor'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "true,false,true"
    );
}

#[test]
fn an_accessor_in_a_class_body_is_one_property_with_two_halves() {
    assert_eq!(
        run("(function () { class C { get a() { return 3; } } return new C().a; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { class C { set a(v) { this.taken = v; } } \
             var o = new C(); o.a = 4; return o.taken; })()"),
        "4"
    );
    // Defining one half must not wipe the other, which is `DefineOwnProperty` with only the half
    // that was written rather than `CreateDataProperty`.
    assert_eq!(
        run(
            "(function () { class C { get a() { return this.held; } set a(v) { this.held = v * 2; } } \
             var o = new C(); o.a = 5; return o.a; })()"
        ),
        "10"
    );
    assert_eq!(
        run("(function () { class C { get a() {} set a(v) {} } \
             var d = Object.getOwnPropertyDescriptor(C.prototype, 'a'); \
             return (typeof d.get) + ',' + (typeof d.set) + ',' + d.enumerable; })()"),
        "function,function,false"
    );
    // A static accessor lands on the constructor.
    assert_eq!(
        run("(function () { class C { static get a() { return 6; } } return C.a; })()"),
        "6"
    );
}

#[test]
fn a_computed_key_runs_where_it_is_written_and_in_order() {
    assert_eq!(
        run("(function () { class C { ['a' + 'b']() { return 7; } } return new C().ab(); })()"),
        "7"
    );
    // The keys are evaluated as the body is walked, so their side effects come in source order —
    // and a static between two instance methods does not jump the queue.
    assert_eq!(
        run(
            "(function () { var order = []; var k = function (n) { order.push(n); return 'm' + n; }; \
             class C { [k(1)]() {} static [k(2)]() {} [k(3)]() {} } \
             return order.join(''); })()"
        ),
        "123"
    );
    // Two methods with the same key leave the later one in place.
    assert_eq!(
        run(
            "(function () { class C { m() { return 'first'; } m() { return 'second'; } } \
             return new C().m(); })()"
        ),
        "second"
    );
}

#[test]
fn the_constructor_is_the_class_and_cannot_be_called_without_new() {
    assert_eq!(
        run("(function () { class C { constructor(x) { this.x = x; } } return new C(5).x; })()"),
        "5"
    );
    // A class with no constructor written still has one, and it takes no arguments.
    assert_eq!(run("(function () { class C {} return C.length; })()"), "0");
    assert_eq!(
        run("(function () { class C { constructor(a, b) {} } return C.length; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { class C {} return typeof C; })()"),
        "function"
    );
    assert_eq!(
        run("(function () { class C {} return new C() instanceof C; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class C {} return C.prototype.constructor === C; })()"),
        "true"
    );
    // A written constructor is *not* also defined as a prototype method. `class C {}` cannot show
    // this — with no constructor element there is nothing for the loop to skip — so the row needs an
    // explicit one, and what it catches is `C.prototype.constructor` becoming a second function
    // object compiled from the same body.
    assert_eq!(
        run(
            "(function () { class C { constructor() {} } return C.prototype.constructor === C; })()"
        ),
        "true"
    );
    assert_eq!(
        run(
            // Insertion order, which §10.1.11 fixes: `constructor` comes from the class definition
            // itself and `m` from the walk over the body. Not sorted, because `Array.prototype.sort`
            // does not exist yet and a row should not depend on a slice that has not landed.
            "(function () { class C { constructor() { this.n = 1; } m() {} } \
             return Object.getOwnPropertyNames(C.prototype).join(','); })()"
        ),
        "constructor,m"
    );
    // §15.7.14 — the `[[Call]]` exists only to refuse, however the function is reached.
    assert_eq!(
        run("(function () { class C {} try { C(); return 'called'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { class C {} try { C.prototype.constructor(); return 'called'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { class C {} try { C.call(null); return 'called'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // …and a method of a class is an ordinary function, which may be called.
    assert_eq!(
        run("(function () { class C { m() { return 8; } } return C.prototype.m.call(null); })()"),
        "8"
    );
}

#[test]
fn a_class_prototype_cannot_be_pointed_somewhere_else() {
    // §15.7.14 — non-writable *and* non-configurable, unlike §10.2.5's for an ordinary function,
    // which is writable. An instance already inherits from it by the time a script could look.
    assert_eq!(
        run(
            "(function () { class C {} var d = Object.getOwnPropertyDescriptor(C, 'prototype'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "false,false,false"
    );
    // An ordinary function's is writable, which is the row that makes the one above mean something.
    assert_eq!(
        run("(function () { function f() {} \
             var d = Object.getOwnPropertyDescriptor(f, 'prototype'); \
             return d.writable + ',' + d.configurable; })()"),
        "true,false"
    );
    assert_eq!(
        run(
            "(function () { class C {} var was = C.prototype; C.prototype = {}; \
             return C.prototype === was; })()"
        ),
        "true"
    );
}

#[test]
fn a_class_declaration_is_lexical_and_an_expression_is_a_value() {
    // §15.7.11 — hoisted and left uninitialised, so a reference above it is the dead zone rather
    // than `undefined`. That is the difference from a function declaration, which is callable above
    // its own line.
    assert_eq!(
        run(
            "(function () { try { C; return 'read'; } catch (e) { return e.constructor.name; } \
             class C {} })()"
        ),
        "ReferenceError"
    );
    assert_eq!(
        run("(function () { return typeof f; function f() {} })()"),
        "function"
    );
    // An expression is just a value, and needs no name.
    assert_eq!(
        run("(function () { var K = class { m() { return 9; } }; return new K().m(); })()"),
        "9"
    );
    assert_eq!(run("typeof (class {})"), "function");
    assert_eq!(
        run("(function () { var a = [class {}, class {}]; return a[0] === a[1]; })()"),
        "false"
    );
    // §15.7.11 binds the name *mutably*, unlike `const`: a class declaration is a `let`-shaped
    // binding, so the name may be pointed at something else afterwards. Nothing else in this file
    // would notice if it were immutable, which is why this row is here rather than assumed.
    assert_eq!(run("(function () { class C {} C = 1; return C; })()"), "1");
    assert_eq!(
        run("(function () { class C {} var was = C; C = 1; return typeof was; })()"),
        "function"
    );
    // Each evaluation of a class expression makes a *new* constructor and a new prototype.
    assert_eq!(
        run(
            "(function () { var make = function () { return class { m() {} }; }; \
             return make().prototype === make().prototype; })()"
        ),
        "false"
    );
}

#[test]
fn what_a_class_body_cannot_hold_yet_is_refused_by_name() {
    // Each of these is a slice of its own. Refused rather than mis-compiled, because a class that
    // silently dropped its fields would be worse than one that will not compile — and the
    // conformance harness reports a refusal as *not run* rather than as a wrong answer.
    for (source, what) in [
        ("class C extends Object {}", "extends"),
        ("class C { x = 1; }", "field"),
        ("class C { x; }", "field"),
        ("class C { static { 1; } }", "static block"),
    ] {
        let error = compile_error(source);
        assert!(
            error.contains(what),
            "{source:?} should be refused for {what:?}, got {error:?}"
        );
    }
}
