//! §15.7 — a class body: its methods, its fields, its statics, and the scope it is.
//!
//! The rows that matter most are the ones that separate a class body from an object literal, because
//! that is where an implementation that reused the literal path would look right and be wrong: a
//! method here is not enumerable, `prototype` cannot be replaced, and the constructor refuses a call
//! without `new`.
//!
//! `extends` and `super` are [`super::inheritance`]. This heading claimed to exclude them long after
//! it had stopped doing so, which is the same failure as a refusal test that no longer refuses.

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
fn every_kind_of_class_element_compiles_and_the_refusal_list_is_empty() {
    // This was `what_a_class_body_cannot_hold_yet_is_refused_by_name`, and it had been shortened eight
    // times — once per slice, each time because a row had come to assert the opposite of what it said.
    // There is nothing left to shorten: §15.7 is complete, and so is the pair of forms that read a
    // reference back before writing it. A refusal test with no refusals in it is not a test, so what
    // remains is the positive half, which is what the rows were guarding all along.
    //
    // Kept as one test rather than deleted, because the *list* is the useful artefact: the next
    // element §15.7 grows gets a row here, and a reader looking for what a class body cannot hold
    // finds the answer in one place.
    assert_eq!(
        run(
            "class C { a = 1; static b = 2; ['c'] = 3; static { this.d = 4; } m() {} \
             static n() {} get g() { return 5; } #p = 6; #q() { return 7; } \
             get #r() { return 8; } static #s = 9; static #t() { return 10; } \
             all() { return this.#p + ',' + this.#q() + ',' + this.#r; } \
             static statics() { return C.#s + ',' + C.#t(); } } \
             C.b + ',' + C.d + ',' + new C().a + ',' + new C().c + ',' + new C().g \
             + ',' + new C().all() + ',' + C.statics()"
        ),
        "2,4,1,3,5,6,7,8,9,10"
    );
}

#[test]
fn a_field_is_defined_on_the_instance_and_not_assigned_to_it() {
    assert_eq!(
        run("(function () { class C { x = 1; } return new C().x; })()"),
        "1"
    );
    // §15.7.14 — a field written without an initialiser is `undefined`, which is not the same as the
    // field being absent: the own property is there and `for...in` finds it.
    assert_eq!(
        run("(function () { class C { x; } var o = new C(); \
             return o.hasOwnProperty('x') + ',' + (o.x === undefined); })()"),
        "true,true"
    );
    // Unlike a method, a field *is* enumerable — it is an ordinary data property of the instance.
    assert_eq!(
        run("(function () { class C { x = 1; } var seen = []; \
             for (var p in new C()) seen.push(p); return seen.join(','); })()"),
        "x"
    );
    assert_eq!(
        run("(function () { class C { x = 1; } \
             var d = Object.getOwnPropertyDescriptor(new C(), 'x'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "true,true,true"
    );
    // The row this whole design exists for. §15.7.14 initialises with `CreateDataPropertyOrThrow`,
    // which ignores an inherited setter; `this.s = 5` is `[[Set]]`, which would call it. So a field
    // *shadows* a prototype setter rather than running it — and an implementation that prepended
    // assignment statements to the constructor would pass every other row in this file and fail this
    // one.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 's', {set: function (v) { this.ran = v; }}); \
             class C { s = 5; } var o = new C(); \
             return o.hasOwnProperty('s') + ',' + (o.ran === undefined) + ',' + o.s; })()"),
        "true,true,5"
    );
    // …and the assignment form really would have run it, which is what makes the row above mean
    // something rather than merely pass.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 's', {set: function (v) { this.ran = v; }}); \
             var o = Object.create(B.prototype); o.s = 5; \
             return o.hasOwnProperty('s') + ',' + o.ran; })()"),
        "false,5"
    );
}

#[test]
fn fields_run_in_source_order_and_before_the_constructor_body() {
    // Each field can see the ones above it through `this`, because they are already defined.
    assert_eq!(
        run("(function () { class C { x = 1; y = this.x + 1; } return new C().y; })()"),
        "2"
    );
    // §15.7.14 — `InitializeInstanceElements` runs before the constructor's first statement, so the
    // body finds every field already there.
    assert_eq!(
        run(
            "(function () { class C { x = 1; constructor() { this.y = this.x + 1; } } \
             return new C().y; })()"
        ),
        "2"
    );
    // The initialisers run in the order they are written, which a side effect can see.
    assert_eq!(
        run(
            "(function () { var order = []; var mark = function (n) { order.push(n); return n; }; \
             class C { a = mark(1); b = mark(2); c = mark(3); } new C(); \
             return order.join(''); })()"
        ),
        "123"
    );
    // Once per construction, not once per class.
    assert_eq!(
        run(
            "(function () { var calls = 0; var mark = function () { calls++; return 1; }; \
             class C { a = mark(); } new C(); new C(); return calls; })()"
        ),
        "2"
    );
    // A field and a constructor parameter of the same name do not collide: the field wins, because
    // it is initialised before the body could assign anything.
    assert_eq!(
        run(
            "(function () { class C { x = 'field'; constructor(x) { this.taken = x; } } \
             var o = new C('argument'); return o.x + ',' + o.taken; })()"
        ),
        "field,argument"
    );
    // An empty field list emits no prologue at all, which is the branch a class without fields takes.
    assert_eq!(
        run("(function () { class C { m() { return 1; } } return new C().m(); })()"),
        "1"
    );
}

#[test]
fn a_class_body_holds_every_kind_of_element_at_once() {
    // What was a second copy of the refusal list above, kept for the one thing it did that the other
    // did not: check that the element kinds still work when they are written together, rather than
    // one per test.
    assert_eq!(
        run("class C { a = 1; static b = 2; ['c'] = 3; } C.b + ',' + new C().a"),
        "2,1"
    );
}

#[test]
fn a_class_body_is_a_scope_holding_the_class_name() {
    // §15.7.14 steps 4 to 7 — the body gets a scope of its own with a binding for the class's own
    // name. A declaration also has an outer binding, and an expression has none, so this is the only
    // way either kind can name itself from inside.
    assert_eq!(
        run("(function () { var K = class C { m() { return C; } }; return new K().m() === K; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class C { m() { return C; } } return new C().m() === C; })()"),
        "true"
    );
    // A static method reaches it too, and so does a computed key — the binding is initialised before
    // any element is defined, which is what makes both work.
    assert_eq!(
        run(
            "(function () { var K = class C { static who() { return C; } }; return K.who() === K; })()"
        ),
        "true"
    );
    assert_eq!(
        run("(function () { var seen = []; \
             var K = class C { [(seen.push(typeof C), 'm')]() {} }; return seen.join(''); })()"),
        "function"
    );
    // The name does not escape a class expression: outside it, nothing was declared.
    assert_eq!(
        run("(function () { var K = class C {}; return typeof C; })()"),
        "undefined"
    );
    // Each class has its own, so a class made inside a method names itself and not the outer one.
    assert_eq!(
        run(
            "(function () { class Outer { m() { return class Inner { who() { return Inner; } }; } } \
             var I = new Outer().m(); return new I().who() === I; })()"
        ),
        "true"
    );
}

#[test]
fn the_inner_class_binding_is_a_different_one_from_the_outer() {
    // The observable difference, and the reason this is two bindings rather than one: reassigning the
    // outer name leaves what the body closed over alone. A single shared binding would answer `1`.
    assert_eq!(
        run(
            "(function () { class C { m() { return C; } } var was = C; C = 1; \
             return new was().m() === was; })()"
        ),
        "true"
    );
    // …and the outer one really is writable, which is what makes the row above mean something rather
    // than merely pass.
    assert_eq!(run("(function () { class C {} C = 1; return C; })()"), "1");
    // The inner binding is *immutable* — §15.7.14 creates it with `CreateImmutableBinding`, so a body
    // that assigns to its own class name is a TypeError rather than a rebinding.
    assert_eq!(
        run("(function () { var K = class C { m() { \
               try { C = 1; return 'assigned'; } catch (e) { return e.constructor.name; } } }; \
             return new K().m(); })()"),
        "TypeError"
    );
    // Including from a declaration, where an outer *mutable* binding of the same name exists: the
    // body sees the inner one, so this is a TypeError and not a write to the outer name.
    assert_eq!(
        run("(function () { class C { m() { \
               try { C = 1; return 'assigned'; } catch (e) { return e.constructor.name; } } } \
             return new C().m() + ',' + (typeof C); })()"),
        "TypeError,function"
    );
}

#[test]
fn a_class_at_the_top_level_of_a_script_leaves_the_stack_as_it_found_it() {
    // Every other row in this file wraps its class in `(function () { … })()`, and that hid a real
    // bug: an extra value left on the stack only trips the end-of-chunk balance check of a *script*,
    // so a function-wrapped class compiled and ran while 1,084 test262 files failed with
    // `UnbalancedStack`. A class written where a script's own statements go is the shape that
    // notices.
    assert_eq!(run("class C {} typeof C"), "function");
    assert_eq!(run("class C { m() { return 1; } } new C().m()"), "1");
    assert_eq!(
        run("class C { m() { return C; } } new C().m() === C"),
        "true"
    );
    assert_eq!(run("var K = class C {}; typeof K"), "function");
    assert_eq!(run("class C { x = 1; } new C().x"), "1");
    assert_eq!(
        run("class C {} class D {} typeof C + ',' + typeof D"),
        "function,function"
    );
    // …and one after other statements, so the imbalance cannot be absorbed by whatever came before.
    assert_eq!(run("var a = 1; class C {} a + 1"), "2");
}

#[test]
fn a_static_field_is_defined_on_the_constructor_when_the_class_is() {
    assert_eq!(run("class C { static x = 1; } C.x"), "1");
    assert_eq!(
        run("class C { static x; } C.hasOwnProperty('x') + ',' + (C.x === undefined)"),
        "true,true"
    );
    // On the constructor and nowhere else.
    assert_eq!(
        run("class C { static x = 1; } var o = new C(); \
             C.hasOwnProperty('x') + ',' + C.prototype.hasOwnProperty('x') + ',' \
             + o.hasOwnProperty('x')"),
        "true,false,false"
    );
    assert_eq!(
        run(
            "class C { static x = 1; } var d = Object.getOwnPropertyDescriptor(C, 'x'); \
             d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,true,true"
    );
    // Once, when the class is defined — not per instance.
    assert_eq!(
        run("var n = 0; class C { static x = (n++, 1); } new C(); new C(); n"),
        "1"
    );
    // An instance field and a static one of the same class do not reach each other.
    assert_eq!(
        run("class C { x = 1; static y = 2; } var o = new C(); \
             o.x + ',' + C.y + ',' + (o.y === undefined) + ',' + (C.x === undefined)"),
        "1,2,true,true"
    );
    assert_eq!(
        run("class C { static a = 1; static b = 2; } C.a + ',' + C.b"),
        "1,2"
    );
}

#[test]
fn a_static_initialiser_runs_with_this_bound_to_the_constructor() {
    // §15.7.14 — and the reason the initialiser is compiled as a body and *called* rather than
    // emitted inline: a call is the only thing that binds a receiver, and inline code would take
    // whatever `this` the surrounding scope has. A wrong answer, not a missing one.
    assert_eq!(run("class C { static x = this; } C.x === C"), "true");
    assert_eq!(
        run("class C { static m() { return 7; } static x = this.m(); } C.x"),
        "7"
    );
    // The surrounding `this` is something else, which is what makes the rows above mean something.
    assert_eq!(
        run("var outer = {}; \
             var made = function () { class C { static x = this; } return C.x === C; }; \
             made.call(outer)"),
        "true"
    );
    // The class body's scope holds the name, so a static initialiser can read a static field defined
    // above it through the class itself.
    assert_eq!(
        run("class C { static x = 1; static y = C.x + 1; } C.y"),
        "2"
    );
}

#[test]
fn a_static_field_evaluates_its_name_in_the_walk_and_its_initialiser_after_it() {
    // The two halves of one static field happen at different times, and this is the row that says so.
    // §15.7.14 evaluates a `ClassElementName` during the walk over the elements, and runs a static
    // initialiser only once every element has been defined. A first attempt emitted both together and
    // produced `21` here.
    assert_eq!(
        run(
            "var order = []; var k = function (n) { order.push(n); return 'k' + n; }; \
             class C { static [k(1)] = order.push(2); } order.join('')"
        ),
        "12"
    );
    // And the name is evaluated *interleaved with the methods*, in source order — not gathered up and
    // done at the end. Nothing but a method on each side of a static field can show that.
    assert_eq!(
        run(
            "var order = []; var k = function (n) { order.push(n); return 'k' + n; }; \
             class C { [k(1)]() {} static [k(2)] = 0; [k(3)]() {} } order.join('')"
        ),
        "123"
    );
    // Two static fields keep their own names apart, which they would not if one temporary were shared.
    assert_eq!(
        run("var names = ['a', 'b']; var at = 0; \
             class C { static [names[at++]] = 1; static [names[at++]] = 2; } \
             C.a + ',' + C.b"),
        "1,2"
    );
    // Every initialiser runs after every name, so a later name is already evaluated when an earlier
    // initialiser runs.
    assert_eq!(
        run(
            "var order = []; var k = function (n) { order.push('k' + n); return 'f' + n; }; \
             var v = function (n) { order.push('v' + n); return n; }; \
             class C { static [k(1)] = v(1); static [k(2)] = v(2); } order.join(',')"
        ),
        "k1,k2,v1,v2"
    );
}

#[test]
fn a_computed_field_name_is_evaluated_once_and_kept() {
    assert_eq!(run("class C { ['a' + 'b'] = 1; } new C().ab"), "1");
    // The point of keeping it: §15.7.14 evaluates the name once, at definition time, however many
    // instances are made. An implementation that re-evaluated it per construction would answer 2.
    assert_eq!(
        run(
            "var n = 0; var k = function () { n++; return 'x'; }; class C { [k()] = 1; } new C(); new C(); n"
        ),
        "1"
    );
    // Every name is evaluated before any initialiser runs, because the names belong to the class
    // definition and the initialisers to the construction.
    assert_eq!(
        run(
            "var order = []; var k = function (n) { order.push('k' + n); return 'f' + n; }; \
             class C { [k(1)] = order.push('v1'); [k(2)] = order.push('v2'); } \
             new C(); order.join(',')"
        ),
        "k1,k2,v1,v2"
    );
    // Two computed names keep their own values apart, which they would not if one slot were shared.
    assert_eq!(
        run("var names = ['a', 'b']; var at = 0; \
             class C { [names[at++]] = 1; [names[at++]] = 2; } \
             var o = new C(); o.a + ',' + o.b"),
        "1,2"
    );
    // Computed and plain names side by side, in order.
    assert_eq!(
        run(
            "class C { ['a'] = 1; b = 2; ['c'] = 3; } var o = new C(); o.a + ',' + o.b + ',' + o.c"
        ),
        "1,2,3"
    );
    // And interleaved with the method keys, in source order — the same rule the static fields follow.
    assert_eq!(
        run(
            "var order = []; var k = function (n) { order.push(n); return 'k' + n; }; \
             class C { [k(1)]() {} [k(2)] = 0; [k(3)]() {} } order.join('')"
        ),
        "123"
    );
    // A static and an instance computed name in one class do not disturb each other.
    assert_eq!(
        run("class C { static ['s'] = 1; ['i'] = 2; } C.s + ',' + new C().i"),
        "1,2"
    );
}

#[test]
fn a_static_block_runs_once_with_this_bound_to_the_constructor() {
    assert_eq!(run("class C { static { this.x = 1; } } C.x"), "1");
    // §15.7.14 binds `this` to the constructor, which is what a block is for — it defines nothing on
    // its own, so without a receiver it could do nothing at all.
    assert_eq!(
        run("class C { static { this.self = this; } } C.self === C"),
        "true"
    );
    assert_eq!(
        run("class C { static { this.m = function () { return 5; }; } } C.m()"),
        "5"
    );
    // Once, when the class is defined — not per instance.
    assert_eq!(
        run("var n = 0; class C { static { n++; } } new C(); new C(); n"),
        "1"
    );
    // A body, not an expression: declarations inside it are its own.
    assert_eq!(
        run("class C { static { var a = 1; this.v = a + 1; } } C.v"),
        "2"
    );
    assert_eq!(
        run("class C { static { let a = 2; this.v = a; } } C.v"),
        "2"
    );
    // It sees a static field defined above it, because every static element runs after every element
    // has been defined and they run in order.
    assert_eq!(
        run("class C { static x = 1; static { this.y = this.x + 1; } } C.y"),
        "2"
    );
    // Blocks and fields are one list in source order — nothing distinguishes them at that point, and
    // gathering them separately would lose the order between them.
    assert_eq!(
        run("var order = []; \
             class C { static { order.push(1); } static a = order.push(2); \
                       static { order.push(3); } } order.join('')"),
        "123"
    );
    // Two blocks in one class, each run.
    assert_eq!(
        run("class C { static { this.a = 1; } static { this.b = this.a + 1; } } C.a + ',' + C.b"),
        "1,2"
    );
}
