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
    // A **logical** assignment *is* on the list — §13.15.2's evaluation for `&&=`, `||=` and `??=`
    // has a step 5 that the arithmetic forms have no equivalent of. This row asserted the opposite
    // for as long as the compiler did, which is what an overfitted test looks like from the
    // inside: it read as a rule about "compound assignment" and was a description of a bug.
    assert_eq!(
        run("(function () { var f = 0; f ||= function () {}; return f.name; })()"),
        "f"
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

#[test]
fn a_method_is_not_a_constructor_and_has_no_prototype() {
    // §15.4.5 and §15.7.14 — a `MethodDefinition` is made by `OrdinaryFunctionCreate` *without*
    // `MakeConstructor`, so it has neither `[[Construct]]` nor the `prototype` object one would
    // inherit from. praxis gave every non-arrow function both, which was a silent wrong answer: `new
    // o.m()` produced an object where the specification asks for a TypeError.
    assert_eq!(
        run("(function () { var o = { m() {} }; \
             return ('prototype' in o.m) + ',' + Object.getOwnPropertyNames(o.m).join('|'); })()"),
        "false,length|name"
    );
    assert_eq!(
        run("(function () { var o = { m() {} }; \
             try { new o.m(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // The row that says this is about the *production* and not about the spelling: a property whose
    // value happens to be a function is an ordinary function and constructs.
    assert_eq!(
        run("(function () { var o = { m: function () {} }; \
             return ('prototype' in o.m) + ',' + (typeof new o.m()); })()"),
        "true,object"
    );
    // Every kind of class element that is a method: instance, static, accessor, private.
    assert_eq!(
        run(
            "(function () { class C { m() {} static s() {} get a() {} } \
             var d = Object.getOwnPropertyDescriptor(C.prototype, 'a'); \
             return ('prototype' in C.prototype.m) + ',' + ('prototype' in C.s) \
                  + ',' + ('prototype' in d.get); })()"
        ),
        "false,false,false"
    );
    assert_eq!(
        run(
            "(function () { class C { #m() {} take() { return 'prototype' in this.#m; } } \
             return new C().take(); })()"
        ),
        "false"
    );
    assert_eq!(
        run("(function () { class C { m() {} } \
             try { new C.prototype.m(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // …and the two things in a class body that *are* constructors are untouched: the class itself,
    // which is the one element that constructs, and an ordinary function written anywhere.
    assert_eq!(
        run("(function () { class C {} return ('prototype' in C) + ',' + (typeof new C()); })()"),
        "true,object"
    );
    assert_eq!(
        run(
            "(function () { function f() {} return ('prototype' in f) + ',' + (typeof new f()); })()"
        ),
        "true,object"
    );
}

#[test]
fn a_named_function_expression_can_see_itself_and_nothing_outside_it_can_take_that_away() {
    // §15.2.5 steps 3 to 5 and 9 — a named function *expression* gets an environment of its own
    // holding an immutable binding of its name, and closes over it. It is the only way such a
    // function can refer to itself: an expression makes no binding outside, so without this the
    // name is whatever the surrounding scope happened to have, or nothing at all.
    assert_eq!(
        run("var f = function g() { return typeof g; }; f()"),
        "function"
    );
    assert_eq!(run("var f = function g() { return g === f; }; f()"), "true");
    // The binding is the *function's*, so it survives whatever happens to the name outside. Both
    // halves matter: an outer binding of the same name is shadowed, and reassigning the name the
    // expression was stored under does not reach it.
    assert_eq!(
        run("var g = 1; var f = function g() { return typeof g; }; f()"),
        "function"
    );
    assert_eq!(
        run("var f = function g() { return g; }; var h = f; f = 1; h() === h"),
        "true"
    );
    // …and it is not visible from outside, which is what makes it the *function's* scope rather
    // than a declaration in disguise.
    assert_eq!(run("var f = function g() {}; typeof g"), "undefined");
    // A recursive call through it works, which is the reason the clause exists at all.
    assert_eq!(
        run(
            "var f = function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); }; \
             var kept = f; f = null; kept(5)"
        ),
        "120"
    );
    // Every kind of function expression, since §15.5.5, §15.6.4 and §15.8.4 all defer to §15.2.5.
    assert_eq!(
        run("var f = function* g() { yield typeof g; }; f().next().value"),
        "function"
    );
    assert_eq!(
        run("var f = async function g() { return typeof g; }; typeof f()"),
        "object"
    );
    // An anonymous expression binds nothing, so §8.6.3's name is a property and not a scope.
    assert_eq!(
        run("var f = function () { return typeof f; }; f()"),
        "function"
    );
    assert_eq!(run("(function () {}).name"), "");
}

#[test]
fn a_declarations_name_is_the_scopes_and_an_expressions_name_is_its_own() {
    // The same two words, `function g`, and two different bindings. A declaration's name belongs to
    // the scope around it and is an ordinary mutable binding; an expression's belongs to the
    // function. Assigning to it is where they part, and it is the whole reason the two productions
    // cannot share one path.
    assert_eq!(
        run("function d() { d = 1; return typeof d; } d()"),
        "number"
    );
    assert_eq!(
        run("var f = function g() { g = 1; return typeof g; }; f()"),
        "function"
    );
    // A method is not a `BindingIdentifier` either, so `{ m() { … } }` has no self-binding.
    assert_eq!(
        run("var o = { m: function () { return typeof m; } }; o.m()"),
        "undefined"
    );
    assert_eq!(
        run("var o = { m() { return typeof m; } }; o.m()"),
        "undefined"
    );
}

#[test]
fn assigning_to_a_function_expressions_own_name_is_refused_and_says_so_only_in_strict_code() {
    // §9.1.1.1.5, and the reason praxis's mutability is three answers rather than a flag.
    // §15.2.5 step 5 creates the binding with `CreateImmutableBinding(name, **false**)` — the only
    // production in the language that passes `false` — so step 2 does not force the throw and step
    // 5.b asks the *assignment* instead.
    //
    // Sloppy: the write never happens and nothing is said about it.
    assert_eq!(
        run("var f = function g() { g = 1; return g === f; }; f()"),
        "true"
    );
    // …and the assignment still evaluates to its right-hand side, because that is what an
    // assignment is worth whether or not anything kept it.
    assert_eq!(run("var f = function g() { return (g = 7); }; f()"), "7");
    // Strict: the same refusal, said out loud.
    assert_eq!(
        run(
            "var f = function g() { 'use strict'; try { g = 1; return 'assigned'; } \
             catch (e) { return e.constructor.name; } }; f()"
        ),
        "TypeError"
    );
    // Strict from the code around it rather than from the body's own directive, since §11.2.1
    // makes strictness inherited.
    assert_eq!(
        run(
            "'use strict'; var f = function g() { try { g = 1; return 'assigned'; } \
             catch (e) { return e.constructor.name; } }; f()"
        ),
        "TypeError"
    );
    // A `const` is the other immutable binding and is *not* the same: §14.3.1 creates it with
    // `CreateImmutableBinding(N, true)`, so it throws wherever it is written. The two rows below
    // are the same program in the two strictnesses, and only one of them agrees with the rows
    // above — which is what a single flag could not express.
    assert_eq!(
        run(
            "const c = 1; (function () { try { c = 2; return 'assigned'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // The right-hand side runs first either way — §13.15.2 evaluates it before the reference is
    // written — so a refusal is not a way to skip it.
    assert_eq!(
        run("var ran = false; var f = function g() { g = (ran = true); return ran; }; f()"),
        "true"
    );
}

#[test]
fn a_direct_eval_in_a_named_function_expression_resolves_its_name_too() {
    // The binding is an environment's (DR-0018), not a slot the compiler kept to itself — so the
    // one thing that resolves against a *running* scope can find it, and finds it with the right
    // mutability.
    assert_eq!(
        run("var f = function g() { return eval('typeof g'); }; f()"),
        "function"
    );
    assert_eq!(
        run("var f = function g() { return eval('g') === f; }; f()"),
        "true"
    );
    assert_eq!(
        run("var f = function g() { eval('g = 1'); return g === f; }; f()"),
        "true"
    );
    assert_eq!(
        run(
            "var f = function g() { 'use strict'; try { eval('g = 1'); return 'assigned'; } \
             catch (e) { return e.constructor.name; } }; f()"
        ),
        "TypeError"
    );
}

#[test]
fn the_three_logical_assignments_name_what_they_assign_and_the_arithmetic_ones_do_not() {
    // §13.15.2's evaluation for `&&=`, `||=` and `??=` has a step 5 the arithmetic forms have no
    // equivalent of: `IsAnonymousFunctionDefinition(rhs)` and `IsIdentifierRef(lhs)` together make
    // this a `NamedEvaluation` position. Grouping the three with `+=` because all four are spelled
    // "compound" is the mistake — §8.6.3's list is drawn per-production, not per-category.
    assert_eq!(run("var v = 1; v &&= () => {}; v.name"), "v");
    assert_eq!(run("var v = 0; v ||= () => {}; v.name"), "v");
    assert_eq!(run("var v; v ??= () => {}; v.name"), "v");
    // Every anonymous definition, not only arrows.
    assert_eq!(run("var v = 0; v ||= function () {}; v.name"), "v");
    assert_eq!(run("var v = 0; v ||= class {}; v.name"), "v");
    assert_eq!(run("var v = 0; v ||= function* () {}; v.name"), "v");
    assert_eq!(run("var v = 0; v ||= async function () {}; v.name"), "v");
    // …and a definition that is *not* anonymous keeps its own name, which is what says the naming
    // is `NamedEvaluation` rather than an assignment overwriting a name.
    assert_eq!(run("var v = 0; v ||= function named() {}; v.name"), "named");
    // Step 5 wants `IsIdentifierRef` of the **target**, so a property target names nothing —
    // there is no identifier for it to take the name of.
    assert_eq!(run("var o = {}; o.p ||= () => {}; o.p.name"), "");
    assert_eq!(run("var o = {}; o['q'] ??= () => {}; o.q.name"), "");
    // The arithmetic forms have no such step, and `+` on a function would not reach one anyway.
    assert_eq!(run("var v = 0; v += function () {}; typeof v"), "string");
    // The short circuit still decides whether anything is assigned at all: a name is only given
    // to a function the operator actually evaluated.
    assert_eq!(run("var v = 1; v ||= () => {}; v"), "1");
    assert_eq!(run("var n = 0; var v = 1; v ||= (n = 1, () => {}); n"), "0");
}
