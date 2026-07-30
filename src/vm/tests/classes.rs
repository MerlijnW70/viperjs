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
    // Refused rather than mis-compiled: the harness reports a refusal as *not run* rather than as a
    // wrong answer, and a class that silently dropped an element would be worse than one that will
    // not compile.
    //
    // This list has been shortened seven times, once per slice, and each time because a row was
    // asserting the opposite of what it said — a refusal test outlives the refusal it describes. Two
    // near-identical copies of it had also accumulated in this file, each shortened separately; they
    // are one test now, because the second was where a stale row could hide from the first.
    //
    // What is genuinely left: a private name, and a *compound* assignment through `super`, which
    // §13.3.7.1 leaves a three-value reference for where every compound form copies two.
    for (source, named) in [
        ("class C { #x = 1; }", "private name"),
        (
            "class B {} class C extends B { m() { super.x += 1; } }",
            "compound assignment to a `super` property",
        ),
    ] {
        let error = compile_error(source);
        assert!(error.contains(named), "{source}: got {error:?}");
    }
    // Everything else a class body can hold now compiles, so there is no row for it here.
    assert_eq!(
        run(
            "class C { a = 1; static b = 2; ['c'] = 3; static { this.d = 4; } m() {}              static n() {} get g() { return 5; } }              C.b + ',' + C.d + ',' + new C().a + ',' + new C().c + ',' + new C().g"
        ),
        "2,4,1,3,5"
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

#[test]
fn extends_points_both_halves_of_a_class_at_both_halves_of_its_parent() {
    // §15.7.14 steps 12 to 14 — two edges, not one, and each carries something different. The
    // prototype chain is what makes an inherited *method* reachable; the constructor chain is what
    // makes an inherited *static* reachable. An implementation that wired only the first would pass
    // every method test and answer `undefined` for `D.s()`.
    assert_eq!(
        run("(function () { class B { m() { return 'm'; } static s() { return 's'; } } \
             class D extends B {} return new D().m() + D.s(); })()"),
        "ms"
    );
    assert_eq!(
        run("(function () { class B {} class D extends B {} \
             return (Object.getPrototypeOf(D.prototype) === B.prototype) + ',' \
                  + (Object.getPrototypeOf(D) === B); })()"),
        "true,true"
    );
    // …and an instance is an instance of every class in the chain, which is the same two edges read
    // by §7.3.20 rather than by a call.
    assert_eq!(
        run("(function () { class B {} class D extends B {} class E extends D {} var e = new E(); \
             return (e instanceof E) + ',' + (e instanceof D) + ',' + (e instanceof B); })()"),
        "true,true,true"
    );
}

#[test]
fn a_heritage_that_is_not_a_constructor_is_a_type_error_and_null_is_not() {
    // §15.7.14 steps 9 to 11 read the value three ways, and the middle case is about `[[Construct]]`
    // rather than about being an object: `Math.max` is a function and is not a constructor, so it
    // fails here where reading its `prototype` would simply have found `undefined`.
    for heritage in ["1", "'a'", "{}", "Math.max", "(() => {})", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ class D extends {heritage} {{}} return 'no'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "extends {heritage}"
        );
    }
    // §15.7.14 step 9 — `extends null` is *not* an error. The class is made, its instances would
    // inherit from nothing, and it is still derived: so the error arrives per construction, when
    // `super()` looks for a constructor and finds `Function.prototype`.
    assert_eq!(
        run("(function () { class D extends null {} return typeof D; })()"),
        "function"
    );
    assert_eq!(
        run("(function () { class D extends null {} \
             return Object.getPrototypeOf(D.prototype) === null; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class D extends null {} \
             try { new D(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // A parent whose `prototype` was replaced with a primitive is step 11's other TypeError, and it
    // is a different check from the one above: `B` here *is* a constructor.
    assert_eq!(
        run("(function () { function B() {} B.prototype = 1; \
             try { class D extends B {} return 'no'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn a_derived_constructor_with_none_written_forwards_every_argument() {
    // §15.7.14 step 15 — the implicit one is `constructor(...args) { super(...args); }`, so the
    // arguments reach the parent unchanged and however many there are. An implementation that
    // synthesised an *empty* constructor would construct successfully and lose every argument.
    assert_eq!(
        run("(function () { class B { constructor(a, b) { this.sum = a + b; } } \
             class D extends B {} return new D(1, 2).sum; })()"),
        "3"
    );
    // Through two levels, each of which forwards, and with a count neither one names.
    assert_eq!(
        run("(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B {} class E extends D {} return new E(1, 2, 3, 4).n; })()"),
        "4"
    );
}

#[test]
fn a_derived_instance_inherits_from_the_class_that_was_written_after_new() {
    // §10.2.2 — `super()` inherits `new.target` rather than replacing it with the parent, and this is
    // the single most consequential thing about a derived construction: the *parent* makes the
    // object, so if it made one from its own `prototype` then `new D()` would answer a `B` and
    // `d instanceof D` would be false. Read from inside the parent, where the object is made.
    assert_eq!(
        run("(function () { class B { constructor() { this.p = Object.getPrototypeOf(this); } } \
             class D extends B {} return new D().p === D.prototype; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class B { constructor() { this.t = new.target; } } \
             class D extends B {} class E extends D {} return new E().t === E; })()"),
        "true"
    );
    // The running function is read at `super()` time, not captured when the class was defined — so
    // moving `D`'s prototype moves what `super()` reaches. A definition that had recorded the answer
    // would go on calling `B`.
    assert_eq!(
        run("(function () { class B { constructor() { this.who = 'B'; } } \
             class C { constructor() { this.who = 'C'; } } \
             class D extends B {} Object.setPrototypeOf(D, C); \
             return new D().who; })()"),
        "C"
    );
}

#[test]
fn a_derived_constructors_this_does_not_exist_until_super_has_returned() {
    // §10.2.2 and DR-0015 — the whole reason `this` is a binding there. Every one of these is a
    // ReferenceError, and each reaches the binding by a different route.
    let unbound = [
        // Read directly, above the call.
        "class D extends B { constructor() { this.x = 1; super(); } }",
        // Read by a parameter default, which runs before the body — so the binding has to exist
        // before the defaults do.
        "class D extends B { constructor(a = this) { super(); } }",
        // Never called at all, so the *return* is what finds the binding empty.
        "class D extends B { constructor() {} }",
        // Returned `undefined` explicitly, which is the same step by the other path.
        "class D extends B { constructor() { return undefined; } }",
        // Called twice: the second is §10.2.2's `BindThisValue` refusing an already-bound binding.
        "class D extends B { constructor() { super(); super(); } }",
    ];
    for source in unbound {
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{}} {source} \
                 try {{ new D(); return 'no'; }} catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "ReferenceError",
            "{source}"
        );
    }
    // …and after `super()` it is there, which is what makes the rows above about *timing* rather
    // than about `this` being broken.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); this.x = 1; } } \
             return new D().x; })()"),
        "1"
    );
    // A `try` around the read proves the throw is an ordinary abrupt completion and not a fault: the
    // constructor recovers and goes on to call `super()`.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { try { this; } \
                 catch (e) { super(); this.caught = 1; } } } \
             return new D().caught; })()"),
        "1"
    );
}

#[test]
fn an_arrow_written_above_a_super_call_still_sees_the_instance() {
    // The case DR-0015 exists for, and the reason `this` is a binding rather than a flag beside the
    // register. An arrow captures its `this` as a *value* where it is written, so an arrow written
    // above the `super()` would have captured the placeholder and answered `undefined` forever.
    // Reading the binding instead means it sees the `super()` that ran after it was made.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => this; super(); \
                 this.ok = f() === this; } } \
             return new D().ok; })()"),
        "true"
    );
    // Called before the `super()`, the same arrow throws — so it is reading the binding each time
    // rather than having been repaired once.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => this; \
                 try { f(); } catch (e) { super(); this.e = e.constructor.name; } } } \
             return new D().e; })()"),
        "ReferenceError"
    );
    // Two levels of arrow, because the binding is reached by counting environments outward and one
    // level is where an off-by-one would still pass.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => () => this; super(); \
                 this.ok = f()() === this; } } \
             return new D().ok; })()"),
        "true"
    );
    // The dangerous direction: a body that binds `this` itself must *not* reach the binding. An
    // object-literal method and a function expression both get their own receiver, and a permissive
    // propagation rule would hand them the enclosing instance instead.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); \
                 var o = { m() { return this; } }; this.ok = o.m() === o; } } \
             return new D().ok; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); \
                 this.m = function () { return this; }; } } \
             var d = new D(); return d.m() === d; })()"),
        "true"
    );
}

#[test]
fn a_derived_constructor_may_only_answer_with_an_object_or_undefined() {
    // §10.2.2 step 13, which is *stricter* than a base constructor's: there a primitive `return` is
    // ignored and the constructed object is answered with anyway. Here it is a TypeError, and that
    // difference is the whole reason the two returns cannot share one instruction.
    for value in ["1", "'a'", "true", "null"] {
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{}} \
                 class D extends B {{ constructor() {{ super(); return {value}; }} }} \
                 try {{ new D(); return 'no'; }} catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "return {value}"
        );
        // The same value from a *base* constructor is ignored, not an error.
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{ constructor() {{ return {value}; }} }} \
                 return new B() instanceof B; }})()"
            )),
            "true",
            "base return {value}"
        );
    }
    // An object return wins, exactly as in a base constructor — and it does not have to be the
    // instance.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); return { z: 9 }; } } \
             return new D().z; })()"),
        "9"
    );
    // `return;` with nothing is `return undefined`, which is answered with the bound `this`.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); this.x = 1; return; } } \
             return new D().x; })()"),
        "1"
    );
}

#[test]
fn a_derived_classs_fields_are_initialised_by_super_and_not_on_entry() {
    // §15.7.14 — `InitializeInstanceElements` runs at step 7 of `SuperCall`, after the parent has made
    // the object, because until then there is nothing to define a property on. So a field initialiser
    // can read what the parent wrote, which is what makes the ordering observable rather than
    // internal.
    assert_eq!(
        run("(function () { class B { constructor() { this.x = 10; } } \
             class D extends B { y = this.x + 1; } return new D().y; })()"),
        "11"
    );
    // …and the parent cannot see the field, which is the same ordering from the other side.
    assert_eq!(
        run("(function () { class B { constructor() { this.seen = this.y; } } \
             class D extends B { y = 1; } return String(new D().seen); })()"),
        "undefined"
    );
    // Fields go in source order, and after the parent's work in both cases.
    assert_eq!(
        run("(function () { var order = []; \
             class B { constructor() { order.push('B'); } } \
             class D extends B { a = order.push('a'); b = order.push('b'); \
               constructor() { super(); order.push('body'); } } \
             new D(); return order.join(','); })()"),
        "B,a,b,body"
    );
    // A computed field name in a derived class is still evaluated once, at definition time — the slot
    // it was left in is reached from the field initialiser body, which is one environment further out
    // than in a base class because that body is nested inside the constructor.
    assert_eq!(
        run("(function () { var n = 0; class B {} \
             class D extends B { [(n++, 'k')] = 1; } \
             new D(); new D(); return new D().k + ',' + n; })()"),
        "1,1"
    );
}

#[test]
fn super_forwards_a_spread_and_the_arguments_a_written_constructor_chooses() {
    // §13.3.8 through §13.3.7 — a spread in a `super()` has no count until it is iterated, exactly as
    // in any other call, and it goes through the same array-building path. The implicit constructor
    // uses it too, which is why that path had to exist before `extends` could.
    assert_eq!(
        run("(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B { constructor(list) { super(...list, 9); } } \
             return new D([1, 2, 3]).n; })()"),
        "4"
    );
    // A written constructor may pass whatever it likes, which is the difference from the implicit one.
    assert_eq!(
        run("(function () { class B { constructor(a) { this.a = a; } } \
             class D extends B { constructor(a) { super(a * 2); } } \
             return new D(21).a; })()"),
        "42"
    );
    // A spread whose iterator is exhausted contributes nothing, and `super()` with no arguments is
    // the same call with a count of zero.
    assert_eq!(
        run("(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B { constructor() { super(...[]); } } \
             return new D().n; })()"),
        "0"
    );
}

#[test]
fn a_class_may_extend_an_ordinary_function_and_be_called_only_with_new() {
    // §15.7.14 does not require the parent to be a class. An ordinary function is a constructor, so
    // it is a legal heritage, and `super()` constructs it — which is how a subclass of a
    // pre-class-syntax constructor works.
    assert_eq!(
        run("(function () { function B(x) { this.x = x; } \
             B.prototype.m = function () { return this.x; }; \
             class D extends B { constructor() { super(7); } } return new D().m(); })()"),
        "7"
    );
    // …and a derived constructor is still a class constructor, so calling it without `new` is a
    // TypeError before anything in its body runs.
    assert_eq!(
        run("(function () { class B {} class D extends B {} \
             try { D(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn super_reads_from_one_level_above_where_the_method_was_defined() {
    // §9.1.1.3 `GetSuperBase` — the home object's *prototype*, not the home object. A method that
    // read its own home would find itself, and `super.m()` would be infinite recursion rather than a
    // call to the parent.
    assert_eq!(
        run("(function () { class B { m() { return 1; } } \
             class D extends B { m() { return super.m() + 1; } } return new D().m(); })()"),
        "2"
    );
    // Three deep, so the base is where the method was *defined* and not where it was found: `D`'s
    // `m` reads `C`'s however it was reached, which is what makes the chain terminate.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class C extends B { m() { return super.m() + 'C'; } } \
             class D extends C { m() { return super.m() + 'D'; } } return new D().m(); })()"),
        "BCD"
    );
    // A computed key is the same reference with the key evaluated at run time.
    assert_eq!(
        run("(function () { class B {} B.prototype.v = 5; \
             class D extends B { m() { return super['v'] + super['v' + '']; } } \
             return new D().m(); })()"),
        "10"
    );
    // Absent is `undefined` rather than an error, as any read is.
    assert_eq!(
        run("(function () { class B {} class D extends B { m() { return String(super.nothing); } } \
             return new D().m(); })()"),
        "undefined"
    );
    // A base class's method has a home too — its prototype's prototype is `Object.prototype`, so
    // this is not a special case for derived classes.
    assert_eq!(
        run("(function () { class C { m() { return typeof super.hasOwnProperty; } } \
             return new C().m(); })()"),
        "function"
    );
}

#[test]
fn super_keeps_this_as_the_receiver_and_not_the_object_it_looked_on() {
    // §13.3.7.1 — the reference has two objects, and this is the row that tells them apart. The
    // parent's getter is *found* on `B.prototype` and called with the instance, so it can read a
    // field the instance has and the prototype does not. An implementation that passed the base for
    // both would answer `undefined` here and pass every other row in this file.
    assert_eq!(
        run("(function () { class B { get g() { return this.x; } } \
             class D extends B { constructor() { super(); this.x = 7; } read() { return super.g; } } \
             return new D().read(); })()"),
        "7"
    );
    // The same for a method call, which is the common case: `super.m()` is `this.m()` with the
    // lookup started higher.
    assert_eq!(
        run("(function () { class B { m() { return this.tag; } } \
             class D extends B { m() { return super.m(); } } \
             var d = new D(); d.tag = 'inst'; return d.m(); })()"),
        "inst"
    );
    // A getter with no getter half answers `undefined` rather than throwing, which is a different
    // route to the same answer as an absent property.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 'w', { set: function (v) {} }); \
             class D extends B { m() { return String(super.w); } } return new D().m(); })()"),
        "undefined"
    );
}

#[test]
fn a_static_methods_super_reads_the_parent_class_and_not_its_prototype() {
    // §15.7.14 gives a static method the *constructor* as its home, so its super base is the parent
    // constructor — which is how a static method is inherited and overridden. Getting the home wrong
    // here would look for a static on the parent's prototype and find nothing.
    assert_eq!(
        run("(function () { class B { static s() { return 's'; } } \
             class D extends B { static s() { return super.s() + 't'; } } return D.s(); })()"),
        "st"
    );
    assert_eq!(
        run("(function () { class B { static get g() { return 'bg'; } } \
             class D extends B { static read() { return super.g; } } return D.read(); })()"),
        "bg"
    );
}

#[test]
fn super_survives_being_taken_off_the_class_it_was_written_in() {
    // `[[HomeObject]]` is fixed where the method was *written* and has nothing to do with how it is
    // called — which is the whole reason it is a field on the function rather than something derived
    // from `this`. A method borrowed by an unrelated object still reads the original parent.
    assert_eq!(
        run("(function () { class B { m() { return 1; } } \
             class D extends B { m() { return super.m() + 1; } } \
             var taken = new D().m; return taken.call({}); })()"),
        "2"
    );
    // …and an arrow written inside a method reaches the enclosing method's home, because §15.3 gives
    // it none of its own — the same outward reach as `this`, captured at the same moment and in the
    // same field, so the two cannot disagree about which method the arrow was written in.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { m() { var f = () => super.m(); return f() + 'D'; } } \
             return new D().m(); })()"),
        "BD"
    );
    // Two levels deep, where a capture that reached only one would still have passed.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { m() { var f = () => () => super.m(); return f()(); } } \
             return new D().m(); })()"),
        "B"
    );
}

#[test]
fn a_write_through_super_lands_on_the_receiver_and_not_on_the_base() {
    // §13.3.7.1 with `[[Set]]` — the receiver decides where the value goes, so `super.x = 1` makes an
    // own property of the *instance* and leaves the parent prototype alone. That reads oddly and is
    // the same rule an ordinary assignment through a prototype follows.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { m() { super.q = 3; \
                 return this.q + ',' + B.prototype.hasOwnProperty('q') \
                      + ',' + this.hasOwnProperty('q'); } } \
             return new D().m(); })()"),
        "3,false,true"
    );
    // …and the same when the base *does* have the property already, which is the case that says the
    // write is not simply falling through to an ordinary assignment on the base: the parent's
    // property is untouched and the instance shadows it.
    assert_eq!(
        run("(function () { class B {} B.prototype.p = 1; \
             class D extends B { m() { super.p = 2; \
                 return this.p + ',' + B.prototype.p + ',' + this.hasOwnProperty('p'); } } \
             return new D().m(); })()"),
        "2,1,true"
    );
    // An inherited setter is called instead, with `this` as its receiver — so it writes wherever it
    // means to, and nothing is defined on the instance by this instruction.
    assert_eq!(
        run("(function () { class B { set s(v) { this.taken = v * 2; } } \
             class D extends B { m() { super.s = 4; return this.taken; } } \
             return new D().m(); })()"),
        "8"
    );
    // A setter-less accessor and a non-writable data property both refuse the write, silently in
    // sloppy code, which is what an ordinary assignment does too.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 'r', { get: function () { return 1; } }); \
             class D extends B { m() { super.r = 9; return String(this.r); } } \
             return new D().m(); })()"),
        "1"
    );
    // The assignment is an expression, so its value is what was written and not what was read back.
    assert_eq!(
        run("(function () { class B {} class D extends B { m() { return (super.z = 5); } } \
             return new D().m(); })()"),
        "5"
    );
}

#[test]
fn an_object_literal_method_has_a_home_and_a_function_written_as_a_value_does_not() {
    // §15.4.5 calls `MakeMethod` for a `MethodDefinition` and not for a property whose value happens
    // to be a function. That is the only difference between the two shapes, and `super` is the only
    // thing that can see it — which is why the parser makes `super` in the second a Syntax Error.
    assert_eq!(
        run("(function () { var parent = { m() { return 'p'; } }; \
             var child = { m() { return super.m() + 'c'; } }; \
             Object.setPrototypeOf(child, parent); return child.m(); })()"),
        "pc"
    );
    assert_eq!(
        run("(function () { var parent = { get g() { return 'pg'; } }; \
             var child = { read() { return super.g; } }; \
             Object.setPrototypeOf(child, parent); return child.read(); })()"),
        "pg"
    );
    // An accessor in a literal is a method definition too, so it has a home.
    assert_eq!(
        run("(function () { var parent = { m() { return 'p'; } }; \
             var child = { get g() { return super.m(); } }; \
             Object.setPrototypeOf(child, parent); return child.g; })()"),
        "p"
    );
}

#[test]
fn super_in_a_class_that_extends_null_refuses_the_read_rather_than_faulting() {
    // §9.1.1.3 — the home object exists and its prototype is `null`, so the base is `null` and the
    // read is a TypeError. Not a fault and not `undefined`: the class was made, and it is the *read*
    // that has nowhere to go.
    assert_eq!(
        run("(function () { class D extends null { m() { return super.anything; } } \
             var d = Object.create(D.prototype); \
             try { d.m(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn deleting_a_property_of_super_is_a_reference_error_after_the_key_has_run() {
    // §13.5.1.1 step 3 — there is no super reference `delete` is legal for, so this is unconditional.
    // It was a *silent* wrong answer the moment `super` began to compile: the reference resolves, and
    // an implementation that let it through would delete a property of the parent prototype.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { m() { try { delete super.x; return 'no'; } \
                 catch (e) { return e.constructor.name; } } } \
             return new D().m(); })()"),
        "ReferenceError"
    );
    // A run-time throw and not an early error, which is observable: step 1 evaluates the reference,
    // so `ToPropertyKey` has already run its side effect by the time step 3 refuses.
    assert_eq!(
        run("(function () { var order = []; class B {} \
             class D extends B { m() { \
                 try { delete super[(order.push('key'), 'k')]; } \
                 catch (e) { order.push(e.constructor.name); } \
                 return order.join(','); } } \
             return new D().m(); })()"),
        "key,ReferenceError"
    );
    // An ordinary delete is untouched, which is the row that says the refusal is about `super` and
    // not about member deletion.
    assert_eq!(
        run("(function () { var o = { x: 1 }; return delete o.x; })()"),
        "true"
    );
}

#[test]
fn super_reaches_the_right_home_from_every_synthesised_body_in_a_class() {
    // praxis compiles four things as bodies of their own that the specification writes as inline
    // code: a static block, a static field's initialiser, and a derived class's instance field
    // initialisers. Each therefore needs a `[[HomeObject]]` it did not get from being defined on
    // anything, and each needs a *different* one — which is why they are four rows and not one.
    //
    // A static block and a static field belong to the **constructor**, so `super` in either reads the
    // parent class rather than its prototype.
    assert_eq!(
        run("(function () { class B { static s() { return 'S'; } } \
             class D extends B { static { D.got = super.s(); } } return D.got; })()"),
        "S"
    );
    assert_eq!(
        run("(function () { class B { static s() { return 'S'; } } \
             class D extends B { static f = super.s(); } return D.f; })()"),
        "S"
    );
    // An instance field initialiser belongs to the **prototype**, and in a derived class it runs from
    // `super()` inside a body of its own — so it takes the constructor's home rather than being told
    // a prototype it has no way to name from there.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { f = super.m(); } return new D().f; })()"),
        "B"
    );
    // …and an arrow inside such an initialiser reaches through it, which is two captures deep.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { f = () => super.m(); } return new D().f(); })()"),
        "B"
    );
    // A *base* class's field initialiser is inline in the constructor, so it uses the constructor's
    // home directly — the same answer by a different path, which is worth pinning separately.
    assert_eq!(
        run("(function () { class C { f = typeof super.hasOwnProperty; } return new C().f; })()"),
        "function"
    );
}
