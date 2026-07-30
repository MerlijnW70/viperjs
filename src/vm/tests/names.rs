//! §10.2.9 `SetFunctionName` and §8.6.3 `NamedEvaluation` — what a function is called.
//!
//! Two rules, and the second is the one that surprises people. A function is named by the position it
//! was *written* in, not by what it is later assigned to: `var f = function () {}` is called `"f"`
//! because the declaration is one of a closed list of positions the specification names, and
//! `o.p = function () {}` is called `""` because a property reference is not on that list. So this file
//! is mostly a list of positions, and the rows that matter most are the ones that are *not* named.

use super::*;

#[test]
fn a_function_is_named_by_the_position_it_was_written_in() {
    // §10.2.9 through a declaration, and the same through the four anonymous forms in a named
    // position — §8.6.3's `NamedEvaluation`, which is what makes an anonymous function not anonymous.
    assert_eq!(
        run("(function () { function f() {} return f.name; })()"),
        "f"
    );
    assert_eq!(
        run(
            "(function () { var f = function () {}; var g = () => {}; var D = class {}; \
             return f.name + ',' + g.name + ',' + D.name; })()"
        ),
        "f,g,D"
    );
    // An assignment to a plain name, which §13.15.2 reaches through `IsIdentifierRef`.
    assert_eq!(
        run("(function () { var h; h = function () {}; return h.name; })()"),
        "h"
    );
    // A function that names *itself* keeps that name: the written one wins over the position, which
    // is why §8.6.3 asks whether the definition is anonymous before it does anything.
    assert_eq!(
        run("(function () { var a = function named() {}; return a.name; })()"),
        "named"
    );
    // A class declaration and a named class expression, whose name is the constructor's.
    assert_eq!(
        run("(function () { class C {} var K = class Inner {}; return C.name + ',' + K.name; })()"),
        "C,Inner"
    );
}

#[test]
fn the_positions_that_do_not_name_a_function_leave_it_with_the_empty_string() {
    // §10.2.9 gives an unnamed function `""` and **not** the absence of the property: `'name' in f` is
    // true for every function, which is what makes the rows below assertions rather than omissions.
    assert_eq!(run("(function () { return (function () {}).name; })()"), "");
    assert_eq!(
        run("(function () { return 'name' in function () {}; })()"),
        "true"
    );
    // §13.15.2 asks for an `IsIdentifierRef`, and a property reference is not one — so this is the
    // row that says `NamedEvaluation` is about the *position* and not about what holds the result.
    assert_eq!(
        run("(function () { var o = {}; o.p = function () {}; return o.p.name; })()"),
        ""
    );
    // Nor is a compound assignment on the list, however plain the name is.
    assert_eq!(
        run("(function () { var f = 0; f ||= function () {}; return f.name; })()"),
        ""
    );
    // Nor anything that merely *evaluates* to a function: a parenthesised comma expression, a call, a
    // conditional. Each of these is where an implementation that named by assignment would be wrong.
    for source in [
        "var f = (0, function () {});",
        "var f = (function () { return function () {}; })();",
        "var f = true ? function () {} : null;",
    ] {
        assert_eq!(
            run(&format!("(function () {{ {source} return f.name; }})()")),
            "",
            "{source}"
        );
    }
}

#[test]
fn a_method_is_named_by_its_key_and_an_accessor_carries_the_word() {
    // §10.2.9 — `get ` and `set ` are part of the name and not decoration, which test262 checks by
    // reading `name` off a descriptor's `get`.
    assert_eq!(
        run(
            "(function () { class C { m() {} get a() {} set a(v) {} static s() {} } \
             var d = Object.getOwnPropertyDescriptor(C.prototype, 'a'); \
             return C.prototype.m.name + ',' + d.get.name + ',' + d.set.name + ',' + C.s.name; })()"
        ),
        "m,get a,set a,s"
    );
    // An object literal's methods and accessors are named the same way, §15.4.5 calling the same
    // operation — and a property whose *value* is a function is a named position too.
    assert_eq!(
        run(
            "(function () { var o = { m() {}, get a() {}, p: function () {}, q: () => {} }; \
             return o.m.name + ',' + Object.getOwnPropertyDescriptor(o, 'a').get.name \
                  + ',' + o.p.name + ',' + o.q.name; })()"
        ),
        "m,get a,p,q"
    );
    // A private method's `#` is part of its name, and so is the accessor word in front of it.
    assert_eq!(
        run(
            "(function () { class C { #m() {} read() { return this.#m.name; } } \
             return new C().read(); })()"
        ),
        "#m"
    );
    // A *computed* key names nothing: the name would be whatever the expression came to at run time,
    // and §10.2.9's fallback is the empty string rather than a guess.
    assert_eq!(
        run("(function () { var k = 'm'; var o = { [k]: function () {} }; return o.m.name; })()"),
        ""
    );
}

#[test]
fn a_default_and_a_field_are_named_positions_too() {
    // §8.6.3 reaches a destructuring default, a parameter default and a class field, which are the
    // three easiest to miss because none of them looks like an assignment.
    assert_eq!(
        run(
            "(function () { let [x = function () {}] = []; let { y = () => {} } = {}; \
             return x.name + ',' + y.name; })()"
        ),
        "x,y"
    );
    assert_eq!(
        run(
            "(function () { function f(a = function () {}, b = () => {}) { \
             return a.name + ',' + b.name; } return f(); })()"
        ),
        "a,b"
    );
    assert_eq!(
        run(
            "(function () { class C { x = function () {}; y = () => {}; #p = () => {}; \
             read() { return this.#p.name; } } \
             var c = new C(); return c.x.name + ',' + c.y.name + ',' + c.read(); })()"
        ),
        "x,y,#p"
    );
    // …and a *pattern* target names nothing, because it binds several names and none of them is the
    // name. This is the row that says the rule is about a single binding rather than about defaults.
    assert_eq!(
        run("(function () { let [[a] = function () {}] = [[1]]; return typeof a; })()"),
        "number"
    );
}

#[test]
fn the_name_property_has_the_attributes_every_built_in_property_has() {
    // §10.2.9 — not writable, not enumerable, and *configurable*, which is the set that lets a
    // decorator replace it and stops an assignment from doing so silently.
    assert_eq!(
        run("(function () { function f() {} \
             var d = Object.getOwnPropertyDescriptor(f, 'name'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "false,false,true"
    );
    // …and a class's `name`, which is defined where the constructor is made rather than where a
    // function is: two places give the property, so a row for one of them says nothing about the other.
    assert_eq!(
        run("(function () { class C {} \
             var d = Object.getOwnPropertyDescriptor(C, 'name'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "false,false,true"
    );
    // Not enumerable, so it does not show up in the ways a program lists what an object has.
    assert_eq!(
        run("(function () { function f() {} return Object.keys(f).length; })()"),
        "0"
    );
    // §20.2.3.2 reads it: a bound function's name is the target's with `bound ` in front, and
    // twice-bound puts it in front twice.
    assert_eq!(
        run("(function () { var f = function named() {}; \
             return f.bind(null).name + '|' + f.bind(null).bind(null).name; })()"),
        "bound named|bound bound named"
    );
}
