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
    // §20.2.1.1 — and it builds a function out of source text, called or constructed alike.
    assert_eq!(run("Function('return 1')()"), "1");
    assert_eq!(run("new Function('a', 'return a * 2')(21)"), "42");
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
    // while ViperJS gave an ordinary function no `name` of its own, and its comment said so; §10.2.9
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
    // because ViperJS gives an ordinary function no `length` of its own to subtract from yet.
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
    // **Both are `Get`, not a read of the property table**, which is where this was wrong. §20.2.3.2
    // step 6.a and step 8 are internal method calls, so an accessor decides the answer and a
    // throwing one propagates — where reading the own *data* property answered 0 and `"bound "`
    // and swallowed the throw.
    assert_eq!(
        run(
            "var f = function () {};              Object.defineProperty(f, 'name', { get: function () { return 'got'; } });              f.bind(null).name"
        ),
        "bound got"
    );
    assert_eq!(
        run(
            "var f = function () {};              Object.defineProperty(f, 'length', { get: function () { return 7; } });              f.bind(null, 1).length"
        ),
        "6"
    );
    for (property, expected) in [("name", "TypeError"), ("length", "TypeError")] {
        assert_eq!(
            run(&format!(
                "(function () {{ var f = function () {{}};                   Object.defineProperty(f, '{property}',                     {{ get: function () {{ throw new TypeError('x'); }} }});                   try {{ f.bind(null); return 'no throw'; }}                   catch (e) {{ return e.constructor.name; }} }})()"
            )),
            expected,
            "{property}"
        );
    }
    // …and a Proxy target answers through its trap, which is the same fact from the other side.
    assert_eq!(
        run(
            "var f = new Proxy(function () {},              { get: function (t, k) { return k === 'name' ? 'proxied' : t[k]; } });              f.bind(null).name"
        ),
        "bound proxied"
    );
    // Step 6.b.i is `ToIntegerOrInfinity`, so a fractional `length` **truncates** rather than
    // carrying its fraction into the bound function's.
    assert_eq!(
        run(
            "var f = function () {}; Object.defineProperty(f, 'length', { value: 3.7 });              f.bind(null).length"
        ),
        "3"
    );
    // …and only a Number is read at all, so a `length` that spells one is still 0.
    assert_eq!(
        run(
            "var f = function () {}; Object.defineProperty(f, 'length', { value: '5' });              f.bind(null).length"
        ),
        "0"
    );
    // Both infinities, which §20.2.3.2 gives their own steps and this does not: the subtraction
    // answers them, and an arm of its own for either was a branch no input could reach.
    assert_eq!(
        run(
            "var f = function () {}; Object.defineProperty(f, 'length', { value: Infinity });              f.bind(null, 1, 2).length"
        ),
        "Infinity"
    );
    assert_eq!(
        run(
            "var f = function () {}; Object.defineProperty(f, 'length', { value: -Infinity });              f.bind(null).length"
        ),
        "0"
    );
    // **Step 5 is `HasOwnProperty`, so an inherited `length` is not the target's length.** Without
    // that test the `Get` below it answers 3 here, and a `length` the target does not have becomes
    // the bound function's.
    assert_eq!(
        run(
            "var base = function (a, b, c) {}; var f = function () {};              Object.setPrototypeOf(f, base); delete f.length; f.bind(null).length"
        ),
        "0"
    );
    // …and the `Get` does not run at all, which an inherited getter that throws is the way to say.
    assert_eq!(
        run(
            "(function () {              var base = function () {};              Object.defineProperty(base, 'length', { get: function () { throw new TypeError('x'); } });              var f = function () {}; Object.setPrototypeOf(f, base); delete f.length;              try { return f.bind(null).length; } catch (e) { return e.constructor.name; } })()"
        ),
        "0"
    );
    // Step 8 has no own-property test in front of it, unlike step 5 — so an **inherited** `name` is
    // the target's name, where an inherited `length` is not the target's length.
    assert_eq!(
        run(
            "var base = function () {}; Object.defineProperty(base, 'name', { value: 'inherited' });              var f = function () {}; Object.setPrototypeOf(f, base);              Object.defineProperty(f, 'name', { value: 0, configurable: true }); delete f.name;              f.bind(null).name"
        ),
        "bound inherited"
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

#[test]
fn a_dynamic_function_compiles_against_the_global_scope_and_nothing_else() {
    // §20.2.1.1.1 step 30 — the *realm's* global environment, never the caller's. A reader who
    // expects a closure is reading `eval`; this is the difference between the two, and it is the
    // whole reason a dynamic function can be compiled without the caller's scope to hand.
    assert_eq!(
        run(
            "var x = 'global'; function f() { var x = 'local'; return Function('return x')(); } f()"
        ),
        "global"
    );
    assert_eq!(
        run("function f() { var only = 1; return Function('return typeof only')(); } f()"),
        "undefined"
    );
    // Steps 5 to 11 — the last argument is the body and the rest are parameters, so no arguments
    // at all is a function of none with an empty body rather than an error.
    assert_eq!(run("new Function('a', 'b', 'return a + b')(2, 3)"), "5");
    assert_eq!(run("typeof Function()()"), "undefined");
    assert_eq!(run("Function('a,b', 'return a * b')(6, 7)"), "42");
    // Steps 31 to 33 — `length` counts what was written, the name is always `anonymous`, and it
    // constructs like any ordinary function.
    assert_eq!(
        run("var f = new Function('a', 'b', ''); f.length + ':' + f.name"),
        "2:anonymous"
    );
    assert_eq!(
        run("var f = new Function('a', 'this.a = a'); new f(1).a"),
        "1"
    );
    assert_eq!(run("var f = Function(''); new f() instanceof f"), "true");
}

#[test]
fn a_dynamic_function_is_assembled_before_it_is_parsed() {
    // Steps 12 to 20 build one string and require the **whole** of it to parse. That is what makes
    // this a SyntaxError rather than two functions: the parameter text and the body text have to
    // agree about where the function ends, and parsing them apart would accept it.
    assert_eq!(
        run("try { new Function('a', '){ } , function f2(', ''); } catch (e) { e.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run("try { new Function('return }{'); } catch (e) { e.name }"),
        "SyntaxError"
    );
    // …and the newlines the clause puts around the body are load-bearing: a body ending in a line
    // comment would otherwise swallow the closing brace.
    assert_eq!(run("Function('return 1 // done')()"), "1");
    // A parameter list that is not one is refused on the same terms.
    assert_eq!(
        run("try { new Function('a b', 'return 1'); } catch (e) { e.name }"),
        "SyntaxError"
    );
}

#[test]
fn instanceof_asks_the_right_operand_what_it_means() {
    // §13.10.2 step 2 — the operator looks up `%Symbol.hasInstance%` on its right operand and,
    // finding one, **calls it and believes it**. ViperJS went straight to the prototype walk, so an
    // object saying what `instanceof` means for it was ignored or refused.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.hasInstance] = function (v) { return v === 1 }; \
             (1 instanceof o) + ',' + (2 instanceof o)"
        ),
        "true,false"
    );
    assert_eq!(
        run(
            "class C { static [Symbol.hasInstance](v) { return v === 7 } } \
             (7 instanceof C) + ',' + (8 instanceof C)"
        ),
        "true,false"
    );
    // Step 3 is `ToBoolean` of what it answered, so anything truthy counts and the operator still
    // evaluates to a Boolean.
    assert_eq!(
        run(
            "var o = {[Symbol.hasInstance]: function () { return 'yes' }}; \
             (1 instanceof o) + ',' + typeof (1 instanceof o)"
        ),
        "true,boolean"
    );
    assert_eq!(
        run("var o = {[Symbol.hasInstance]: function () { return 0 }}; 1 instanceof o"),
        "false"
    );
    // It is called with the *right* operand as receiver and the left as the argument, which is the
    // opposite way round from how the operator reads.
    assert_eq!(
        run(
            "var seen; var o = {[Symbol.hasInstance]: function (v) { seen = (this === o) + ',' + v; \
             return true }}; 5 instanceof o; seen"
        ),
        "true,5"
    );
    // §7.3.11 — a `@@hasInstance` that is there and is not callable is a TypeError rather than a
    // silent fall through to the ordinary walk, which would make a misspelled override look as
    // though it had worked.
    // **The message is the assertion**: calling a `1` would be a TypeError too, so only the
    // sentence tells the guard from its absence.
    assert_eq!(
        run("var o = {}; o[Symbol.hasInstance] = 1; \
             try { 1 instanceof o; 'no error' } \
             catch (e) { e.constructor.name + ': ' + e.message }"),
        "TypeError: Symbol.hasInstance is not a function"
    );
    // …and a getter for it runs, so it can throw.
    assert_eq!(
        run(
            "var o = {get [Symbol.hasInstance]() { throw new EvalError('g') }}; \
             try { 1 instanceof o; 'no error' } catch (e) { e.constructor.name }"
        ),
        "EvalError"
    );
    // The two TypeErrors the operator raises itself: a right operand that is not an object at all
    // (step 1), and one that is an object with no `@@hasInstance` anywhere and is not callable
    // (step 4).
    assert_eq!(
        run("try { 1 instanceof 2; 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { 1 instanceof {}; 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and the ordinary walk is unchanged, reached now by way of §20.2.3.6's method rather than
    // directly.
    assert_eq!(run("({}) instanceof Object"), "true");
    assert_eq!(run("(function () {}) instanceof Function"), "true");
    assert_eq!(run("1 instanceof Object"), "false");
    assert_eq!(
        run(
            "function F() {} var x = new F(); var was = x instanceof F; F.prototype = {}; \
             was + ',' + (x instanceof F)"
        ),
        "true,false"
    );
    assert_eq!(
        run("function F() {} F.prototype = 1; \
             try { ({}) instanceof F; 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_default_has_instance_is_a_method_that_cannot_be_replaced() {
    // §20.2.3.6 — and it is the **only** method on `Function.prototype` that is neither writable
    // nor configurable. That is what makes it the intrinsic by construction rather than by
    // remembering: no program can put anything else there.
    assert_eq!(
        run("typeof Function.prototype[Symbol.hasInstance]"),
        "function"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Function.prototype, Symbol.hasInstance); \
             [d.writable, d.enumerable, d.configurable].join(',')"
        ),
        "false,false,false"
    );
    assert_eq!(
        run("Function.prototype[Symbol.hasInstance].name + ',' \
             + Function.prototype[Symbol.hasInstance].length"),
        "[Symbol.hasInstance],1"
    );
    // §7.3.22 step 1 answers **false** for a receiver that is not callable, rather than refusing
    // it — the refusal belongs to §13.10.2 step 4 and this method is not reached that way.
    assert_eq!(
        run("Function.prototype[Symbol.hasInstance].call(1, {})"),
        "false"
    );
    assert_eq!(
        run("Function.prototype[Symbol.hasInstance].call({}, {})"),
        "false"
    );
    // It answers for its receiver, so calling it directly is the operator without the lookup.
    assert_eq!(
        run("function F() {} Function.prototype[Symbol.hasInstance].call(F, new F())"),
        "true"
    );
}

#[test]
fn a_bound_function_answers_instanceof_for_what_it_was_bound_to() {
    // §7.3.22 step 2 — a bound function hands the question to its `[[BoundTargetFunction]]`, so
    // `x instanceof f.bind()` is `x instanceof f`. ViperJS reached step 4 instead and threw,
    // because a bound function has no `prototype` of its own.
    assert_eq!(
        run("function F() {} var b = F.bind(null); \
             (new F() instanceof b) + ',' + (new F() instanceof F)"),
        "true,true"
    );
    assert_eq!(
        run("function F() {} var bb = F.bind(null).bind(null).bind(null); new F() instanceof bb"),
        "true"
    );
    assert_eq!(
        run("function F() {} function G() {} (new G() instanceof F.bind(null))"),
        "false"
    );
    // A chain long enough to have overflowed the stack had this been written the way the clause
    // writes it — mutual recursion between §7.3.22 and §13.10.2, one Rust frame per `bind`.
    assert_eq!(
        run("function F() {} var deep = F; \
             for (var i = 0; i < 3000; i++) { deep = deep.bind(null) } \
             new F() instanceof deep"),
        "true"
    );
    // …and the reason the loop cannot just unwind blindly: a target with a `@@hasInstance` of its
    // own decides, because step 2 hands it back to §13.10.2 rather than to the walk. The bound
    // function does not inherit it — §20.2.3.2 gives a bound function its target's *prototype*,
    // which is `Function.prototype` — so it is reached only by unwinding to the target and asking
    // again, which is the whole of what the loop has to keep from the recursion it replaced.
    assert_eq!(
        run("function F() {} \
             Object.defineProperty(F, Symbol.hasInstance, \
               {value: function (v) { return v === 'yes' }}); \
             ('yes' instanceof F.bind(null)) + ',' + ('no' instanceof F.bind(null))"),
        "true,false"
    );
}

#[test]
fn function_prototype_is_itself_a_function_that_answers_undefined() {
    // §20.2.3 — `Function.prototype` **is** a built-in function object, not an ordinary one. It
    // reads as a curiosity until §7.3.22 step 1 asks whether a receiver is callable and answers
    // `false` for one that is not: `[] instanceof Function.prototype` then answers instead of
    // reaching step 4, where reading a `prototype` that is not an object is the TypeError.
    assert_eq!(run("typeof Function.prototype"), "function");
    assert_eq!(run("String(Function.prototype())"), "undefined");
    assert_eq!(run("String(Function.prototype(1, 2, 3))"), "undefined");
    // It has no `[[Construct]]` and no `prototype` property of its own.
    assert_eq!(
        run("try { new Function.prototype(); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Function.prototype.hasOwnProperty('prototype')"),
        "false"
    );
    // …and being one, it carries §10.3.3's two own properties like any other function: a `length`
    // of +0 and a `name` of the **empty** string, both stated by §20.2.3 rather than derived.
    assert_eq!(
        run("Function.prototype.hasOwnProperty('length') + ':' + Function.prototype.length"),
        "true:0"
    );
    assert_eq!(
        run("Function.prototype.hasOwnProperty('name') + ':[' + Function.prototype.name + ']'"),
        "true:[]"
    );
    // What having them is *for*, and the only thing that can tell them from their absence: both
    // are configurable on every built-in, so a `delete` succeeds and the read that follows walks
    // one step up the chain and lands here. Without them it answered `undefined` — which no
    // property of `Function.prototype` itself can distinguish.
    assert_eq!(run("delete parseInt.length; String(parseInt.length)"), "0");
    assert_eq!(run("delete parseInt.name; '[' + parseInt.name + ']'"), "[]");
    assert_eq!(
        run("delete Function.prototype.length; String(Function.prototype.length)"),
        "undefined"
    );
    // The row that made this necessary: callable, so the walk is reached, so the `prototype` it
    // was given is read and refused.
    assert_eq!(
        run(
            "Function.prototype.prototype = '';              try { [] instanceof Function.prototype; 'no error' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_function_cannot_be_given_a_has_instance_by_assignment() {
    // A consequence of §20.2.3.6's attributes that catches people, and caught this test: the
    // inherited `@@hasInstance` is **not writable**, so §10.1.9.2 refuses an assignment through
    // it. Every function inherits it, so `F[Symbol.hasInstance] = fn` silently does nothing in
    // sloppy code and is a TypeError in strict — and `instanceof` goes on using the default.
    assert_eq!(
        run(
            "function F() {} F[Symbol.hasInstance] = function () { return true }; \
             ('x' instanceof F) + ',' + F.hasOwnProperty(Symbol.hasInstance)"
        ),
        "false,false"
    );
    assert_eq!(
        run("'use strict'; function F() {} \
             try { F[Symbol.hasInstance] = 1; 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // `Object.defineProperty` is how it is done, and it works — the refusal is §10.1.9.2's rule
    // about assignment, not a rule about the key.
    assert_eq!(
        run("function F() {} \
             Object.defineProperty(F, Symbol.hasInstance, \
               {value: function (v) { return v === 1 }}); \
             (1 instanceof F) + ',' + (2 instanceof F)"),
        "true,false"
    );
    // …and a class's `static [Symbol.hasInstance]` *defines* rather than assigns, which is why
    // that spelling works where the assignment does not.
    assert_eq!(
        run(
            "class C { static [Symbol.hasInstance](v) { return v === 1 } } \
             (1 instanceof C) + ',' + (2 instanceof C)"
        ),
        "true,false"
    );
    // An object that is not a function inherits from `Object.prototype`, which has no
    // `@@hasInstance` — so assignment works there, and this is the row that says the refusal above
    // is about what was inherited rather than about the operation.
    assert_eq!(
        run("var o = {}; o[Symbol.hasInstance] = function (v) { return v === 1 }; 1 instanceof o"),
        "true"
    );
}

#[test]
fn a_bound_function_carries_new_target_inward_one_binding_at_a_time() {
    // §10.4.1.2 step 5 — "if SameValue(F, newTarget) is true, set newTarget to target" — which is a
    // rule per binding and was applied at none of them. A plain `new` was right by accident: a
    // construction takes its `new.target` from the callee, and flattening had already replaced the
    // callee with the target.
    let setup = "var seen; function A() { seen = new.target } function G() {} \
                 var B = A.bind(); var C = B.bind(); ";
    // `Reflect.construct` is the only way to name a `new.target` that is not the callee, and a
    // doubly-bound function has to walk *both* hops: C to B, then B to A.
    assert_eq!(
        run(&format!("{setup} Reflect.construct(C, [], C); seen === A")),
        "true"
    );
    // …and one naming the *inner* binding walks the remaining hop only.
    assert_eq!(
        run(&format!("{setup} Reflect.construct(C, [], B); seen === A")),
        "true"
    );
    // A `new.target` that names something else is not touched at all, which is the row that stops
    // this passing by always answering the target.
    assert_eq!(
        run(&format!("{setup} Reflect.construct(C, [], G); seen === G")),
        "true"
    );
    // The prototype follows the same value, which is what a program actually sees.
    assert_eq!(
        run(&format!(
            "{setup} Object.getPrototypeOf(Reflect.construct(C, [], C)) === A.prototype"
        )),
        "true"
    );
    assert_eq!(
        run(&format!(
            "{setup} Object.getPrototypeOf(Reflect.construct(B, [], G)) === G.prototype"
        )),
        "true"
    );
    // …and the plain `new` cases, unchanged.
    assert_eq!(run(&format!("{setup} new B(); seen === A")), "true");
    assert_eq!(run(&format!("{setup} new C(); seen === A")), "true");
}
