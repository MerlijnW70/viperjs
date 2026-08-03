//! §10.1.8.1, §10.1.9.2 and §15.4.5 — a property whose value is a pair of functions.

use super::*;

#[test]
fn a_getter_runs_when_the_property_is_read() {
    assert_eq!(run("var o = {get x() { return 5 }}; o.x"), "5");
    assert_eq!(
        run("var n = 0; var o = {get x() { n = n + 1; return n }}; o.x; o.x; n"),
        "2"
    );
    // It is a *call*, so it can throw and the throw is the read's.
    assert_eq!(
        run("var o = {get x() { throw new RangeError('g') }}; try { o.x } catch (e) { e.name }"),
        "RangeError"
    );
    // An accessor with no getter reads as `undefined` rather than throwing — §10.1.8.1 step 5,
    // and the reason a write-only property is a thing that can exist.
    assert_eq!(run("var o = {set x(v) {}}; typeof o.x"), "undefined");
}

#[test]
fn the_receiver_is_the_object_the_property_was_read_through() {
    // §10.1.8.1 step 6 hands the getter the object the read *started* from, not the one the
    // property was found on. That is what lets an accessor on a prototype see the instance, and
    // it is the whole reason a getter is useful rather than a constant.
    let inherited = "var p = {get a() { return this.n }}; var o = Object.create(p); o.n = 4; ";
    assert_eq!(run(&format!("{inherited} o.a")), "4");
    assert_eq!(run(&format!("{inherited} p.n = 9; p.a")), "9");
    // The same for a setter — §10.1.9.2 step 5.
    let setter = "var p = {set a(v) { this.got = v }}; var o = Object.create(p); ";
    assert_eq!(
        run(&format!("{setter} o.a = 1; o.got + '|' + typeof p.got")),
        "1|undefined"
    );
}

#[test]
fn a_setter_cannot_refuse_a_write_it_can_only_decline_to_record_it() {
    // §10.1.9.2 step 5 throws the setter's answer away, so a setter that returns `false` has not
    // refused anything — and one that stores nothing leaves the property reading as `undefined`.
    assert_eq!(
        run("var o = {set x(v) { this.y = v }}; o.x = 3; o.y + '|' + typeof o.x"),
        "3|undefined"
    );
    assert_eq!(
        run("var o = {set x(v) { return false }}; o.x = 1; typeof o.x"),
        "undefined"
    );
    // A write to an accessor with no setter is ignored rather than throwing, which is the
    // sloppy-mode half of §10.1.9.2 — and it does *not* make an own data property that shadows.
    assert_eq!(run("var o = {get x() { return 1 }}; o.x = 9; o.x"), "1");
}

#[test]
fn a_getter_and_a_setter_are_two_halves_of_one_property() {
    // §15.4.5 — defining one must leave the other where it is. A `CreateDataProperty` would
    // replace the whole property, and `{get a() {}, set a(v) {}}` would end with only the setter.
    let both = "var o = {get x() { return this.v }, set x(n) { this.v = n * 2 }}; ";
    assert_eq!(run(&format!("{both} o.x = 3; o.x")), "6");
    assert_eq!(run(&format!("{both} Object.keys(o).length")), "1");
    // …in either order.
    let reversed = "var o = {set x(n) { this.v = n }, get x() { return this.v }}; ";
    assert_eq!(run(&format!("{reversed} o.x = 7; o.x")), "7");
    // §15.4.5 gives it the two attributes an ordinary literal property gets, and neither a
    // `value` nor a `writable`.
    let shape = "var o = {get x() { return 1 }}; var d = Object.getOwnPropertyDescriptor(o, 'x'); \
                 typeof d.get + '|' + typeof d.set + '|' + d.enumerable + '|' + d.configurable \
                 + '|' + ('value' in d)";
    assert_eq!(run(shape), "function|undefined|true|true|false");
}

#[test]
fn an_accessor_defined_by_hand_runs_the_same_way_a_written_one_does() {
    // The two routes have to agree, because `Object.defineProperty` is how a program builds one
    // when the name is not known until run time.
    assert_eq!(
        run("var o = {}; Object.defineProperty(o, 'a', {get: function () { return 9 }}); o.a"),
        "9"
    );
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {set: function (v) { this.b = v }}); o.a = 2; o.b"
        ),
        "2"
    );
    // …including on the global object, where a name resolves through the same `[[Get]]`.
    assert_eq!(
        run(
            "Object.defineProperty(globalThis, 'made', {get: function () { return 'here' }}); made"
        ),
        "here"
    );
}

#[test]
fn a_shorthand_is_the_name_twice_and_a_method_is_a_function_expression() {
    // §13.2.5 — `{a}` is `{a: a}` with the name read as an ordinary reference, so it sees
    // whatever `a` names where the literal is written.
    assert_eq!(run("var a = 1; var o = {a}; o.a"), "1");
    assert_eq!(run("var a = 1; var b = 2; var o = {a, b}; o.a + o.b"), "3");
    assert_eq!(
        run("function f() { var a = 'inner'; return {a} } f().a"),
        "inner"
    );
    // A name that is nowhere is the same ReferenceError it would be anywhere else.
    assert_eq!(
        run("try { ({nowhere}) } catch (e) { e.name }"),
        "ReferenceError"
    );
    // §15.4.4 — a method is a function made where the literal is *evaluated*, so a literal in a
    // loop makes one function object per turn.
    assert_eq!(run("var o = {m() { return 1 }}; o.m()"), "1");
    assert_eq!(run("var o = {m() { return this.v }, v: 7}; o.m()"), "7");
    assert_eq!(
        run(
            "var seen = []; for (var i = 0; i < 2; i = i + 1) { seen.push({m() {}}.m) } seen[0] === seen[1]"
        ),
        "false"
    );
    // A computed key works for every one of them.
    assert_eq!(run("var k = 'a'; var o = {[k]() { return 2 }}; o.a()"), "2");
    assert_eq!(
        run("var k = 'a'; var o = {get [k]() { return 3 }}; o.a"),
        "3"
    );
}

#[test]
fn a_function_declaration_in_a_block_belongs_to_that_block() {
    // §14.1 `BlockDeclarationInstantiation` step 3.a.ii — created *and initialised* before the
    // block's first statement, which is the one declaration that is hoisted and lexical at once.
    // So it is callable above its own line, and only inside.
    assert_eq!(run("{ function g() { return 1 } } 'no error'"), "no error");
    assert_eq!(run("{ function g() { return 1 } g() }"), "1");
    assert_eq!(run("{ g(); function g() { return 'hoisted' } }"), "hoisted");
    assert_eq!(
        run("function f() { { function g() { return 2 } return g() } } f()"),
        "2"
    );
    // It belongs to the block, so a second entry makes a second function — the same claim block
    // scoping makes about `let`, and the reason this could not land before that did.
    assert_eq!(
        run(
            "var r = []; for (var i = 0; i < 2; i++) { function g() { return i } r.push(g) }              r[0] === r[1]"
        ),
        "false"
    );
    // …and in strict code the name does not escape the block at all, §B.3.3 being conditioned on
    // sloppiness. This is the whole of what DR-0008's reversal turns on, so it is asserted from
    // both sides: the same program without the directive is `annex_b::tests`.
    assert_eq!(
        run(
            "'use strict'; var e = 'none'; { function g() {} } try { g; } catch (x) { e = x.constructor.name } e"
        ),
        "ReferenceError"
    );
    // …and one at a body's top level is hoisted as it always was, in a script and in a function.
    assert_eq!(run("function f() { return 1 } f()"), "1");
    assert_eq!(
        run("function f() { function g() { return 2 } return g() } f()"),
        "2"
    );
    assert_eq!(run("f(); function f() { return 3 }"), "3");
}
