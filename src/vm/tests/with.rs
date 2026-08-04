//! §14.11 — `with`, the one scope whose bindings are an object's properties.
//!
//! Every row here is about a name meaning something the compiler could not have known. That is the
//! whole construct: praxis resolves a name to a depth and an index when it compiles, and inside one
//! of these it cannot, because the answer depends on what the object holds at the moment of the
//! read. §9.4.2's walk happens on every access instead — see `crate::vm::dynamic`.

use super::*;

#[test]
fn a_with_scope_is_consulted_before_everything_outside_it_and_falls_through_when_it_has_nothing() {
    assert_eq!(run("var o = { a: 1 }; with (o) { a }"), "1");
    // Shadowing, and the fall-through that is the other half of it. The same source with the same
    // compiler-visible scopes answers differently depending on what the object has.
    assert_eq!(
        run("var a = 'outer'; var o = { a: 'inner' }; with (o) { a }"),
        "inner"
    );
    assert_eq!(run("var a = 'outer'; var o = {}; with (o) { a }"), "outer");
    // §9.1.1.2.1 asks `HasProperty`, which walks the **prototype chain** — so an inherited property
    // is a binding too, and `Object.prototype`'s methods really do resolve as bare names.
    assert_eq!(run("with ({}) { typeof toString }"), "function");
    assert_eq!(
        run("var p = { a: 'proto' }; var o = Object.create(p); with (o) { a }"),
        "proto"
    );
    // …and a name nowhere at all is the ReferenceError an ordinary unresolvable name gets.
    assert_eq!(
        run("with ({}) { try { no_such_name_at_all; 'read' } catch (e) { e.constructor.name } }"),
        "ReferenceError"
    );
    // §14.11.2 step 2 — `ToObject` first, so the body never runs.
    assert_eq!(
        run("var ran = false; try { with (null) { ran = true; } } catch (e) {} ran"),
        "false"
    );
    assert_eq!(
        run("try { with (undefined) {} 'ran' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A primitive is wrapped, so its properties are bindings and writing one goes nowhere.
    assert_eq!(run("with ('abc') { length }"), "3");
}

#[test]
fn a_write_inside_a_with_goes_to_the_object_when_the_object_has_the_name() {
    assert_eq!(run("var o = { a: 1 }; with (o) { a = 2; } o.a"), "2");
    // …and outward when it does not, **without** creating the property — which is the difference
    // between resolving a name and assigning to a property of the object.
    assert_eq!(
        run("var o = {}; var a = 1; with (o) { a = 5; } a + ',' + (o.a === undefined)"),
        "5,true"
    );
    // A **`var`** inside one is an assignment and not a declaration — hoisting made the binding
    // before the body ran — so it goes through the walk and reaches the object.
    assert_eq!(run("var o = { a: 1 }; with (o) { var a = 5; } o.a"), "5");
    assert_eq!(
        run(
            "function f() { var o = { a: 1 }; var a = 0; with (o) { var a = 5; }              return o.a + ',' + a; } f()"
        ),
        "5,0"
    );
    // …where a `let` is a *declaration* of the block's own binding and never the object's, however
    // deep inside a `with` it is written. The two go through the same function and part here.
    assert_eq!(run("var o = { a: 1 }; with (o) { let a = 5; } o.a"), "1");
    assert_eq!(
        run("var o = { a: 1 }; with (o) { try { throw 2; } catch (a) { } } o.a"),
        "1"
    );
    // A name that is nowhere becomes a global, exactly as a sloppy assignment does anywhere else.
    assert_eq!(
        run("var o = {}; with (o) { made_by_with = 7; } globalThis.made_by_with"),
        "7"
    );
    // The binding found outside may be a `const`, and it refuses the same way it would have if the
    // compiler had resolved it — the mutability travels on the binding (DR-0018) and not on the
    // instruction.
    assert_eq!(
        run("const k = 1; var o = {}; \
             with (o) { try { k = 2; 'assigned' } catch (e) { e.constructor.name } }"),
        "TypeError"
    );
}

#[test]
fn a_call_through_a_with_name_is_made_on_the_object() {
    // §9.1.1.2.10 `WithBaseObject`, and the one place a call written as a bare name has a receiver.
    // This is why `with` is not sugar for a block of property reads.
    assert_eq!(
        run("var o = { m: function () { return this === o; } }; with (o) { m() }"),
        "true"
    );
    // …while a name that fell through to an outer scope is called with no receiver, as any other
    // bare call is.
    assert_eq!(
        run("var seen; function f() { seen = this === globalThis; } \
             var o = {}; with (o) { f(); } seen"),
        "true"
    );
    // The receiver and the callee come from **one** walk. Asking twice would ask an object that a
    // getter may have changed in between, and this counts the asks.
    assert_eq!(
        run("var reads = 0; \
             var o = new Proxy({ m: function () { return 1; } }, { \
                 get: function (t, k) { if (k === 'm') { reads++; } return t[k]; } }); \
             with (o) { m(); } reads"),
        "1"
    );
}

#[test]
fn unscopables_takes_a_name_back_out_of_the_scope() {
    // §14.11.2 — the list is why `Array.prototype` can grow methods without `with (array) { … }`
    // in old code silently meaning something new.
    assert_eq!(
        run(
            "var a = 'outer'; var o = { a: 'inner' }; o[Symbol.unscopables] = { a: true }; \
             with (o) { a }"
        ),
        "outer"
    );
    // A falsy entry blocks nothing, which is the difference between "there is a list" and "the
    // list says yes".
    assert_eq!(
        run(
            "var a = 'outer'; var o = { a: 'inner' }; o[Symbol.unscopables] = { a: false }; \
             with (o) { a }"
        ),
        "inner"
    );
    // …and a `@@unscopables` that is not an object is not consulted at all — step 6 asks.
    assert_eq!(
        run(
            "var a = 'outer'; var o = { a: 'inner' }; o[Symbol.unscopables] = true; \
             with (o) { a }"
        ),
        "inner"
    );
    // A blocked name is *not bound here*, so the walk carries on rather than stopping — which is
    // what makes the first row read the outer binding instead of throwing.
    assert_eq!(
        run("var o = { a: 1 }; o[Symbol.unscopables] = { a: true }; \
             with (o) { try { a; 'read' } catch (e) { e.constructor.name } }"),
        "ReferenceError"
    );
}

#[test]
fn a_function_written_inside_a_with_keeps_the_object_in_its_scope_chain() {
    // The closure is the reason `with_depth` is inherited by a nested body rather than recomputed:
    // `f`'s own scopes contain no `with`, and the chain it captured does.
    assert_eq!(
        run("var o = { a: 1 }; var f; with (o) { f = function () { return a; }; } f()"),
        "1"
    );
    // …and it is a *live* view of the object, not a copy taken when the function was made.
    assert_eq!(
        run("var o = { a: 1 }; var f; with (o) { f = function () { return a; }; } o.a = 9; f()"),
        "9"
    );
    // Deleting the property makes the same name resolve outwards afterwards, which is the thing no
    // index could have expressed.
    assert_eq!(
        run("var a = 'outer'; var o = { a: 'inner' }; var f; \
             with (o) { f = function () { return a; }; } \
             var before = f(); delete o.a; before + ',' + f()"),
        "inner,outer"
    );
}

#[test]
fn the_body_is_an_ordinary_statement_and_every_way_out_of_it_leaves_the_scope() {
    // The scope is a level like a block's, so the machinery that pops one on the way out of a
    // `break`, a `continue` or a `return` needs nothing new — and if it did, the next name read
    // would be one hop wrong rather than failing.
    assert_eq!(
        run("var o = { a: 1 }; var n = 0; \
             for (var i = 0; i < 3; i++) { with (o) { if (a) { continue; } } n = n + 1; } n"),
        "0"
    );
    assert_eq!(
        run("var o = { a: 1 }; var r = 0; \
             while (1) { with (o) { r = a; break; } } r"),
        "1"
    );
    assert_eq!(
        run("var o = { a: 1 }; function g() { with (o) { return a; } } g()"),
        "1"
    );
    // A throw out of one needs no instruction at all — the handler records the environment it was
    // installed in, which is the same reason a block needs none.
    assert_eq!(
        run("var a = 'outer'; var o = { a: 'inner' }; \
             try { with (o) { throw 1; } } catch (e) { a }"),
        "outer"
    );
    // …and the scope is gone afterwards, which is the one thing a missing `PopScope` would not
    // fail loudly about: the next name read would find the object still in the chain.
    assert_eq!(
        run("var a = 'outer'; var o = { a: 'inner' }; with (o) { } a"),
        "outer"
    );
    assert_eq!(
        run("var a = 'outer'; var o = { a: 'inner' }; var r; with (o) { r = a; } r + ',' + a"),
        "inner,outer"
    );
    // A body that is not a block, since §14.11's is a `Statement` and not a `Block`.
    assert_eq!(run("var o = { a: 4 }; var r; with (o) r = a; r"), "4");
    // Nested, where the innermost object wins and the outer one is still there behind it.
    assert_eq!(
        run("var o = { a: 1 }; var p = { b: 2 }; with (o) { with (p) { a + b } }"),
        "3"
    );
}

#[test]
fn a_with_is_a_syntax_error_in_strict_code_and_a_delete_inside_one_asks_where_the_name_lives() {
    // §11.2.1 — the parser refuses it, so this is not a run-time question at all.
    assert_eq!(
        run("try { eval('\"use strict\"; with ({}) {}'); 'ran' } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    // §13.5.1.2 — `delete a` inside a `with` has three possible answers and which applies is only
    // known when it runs, so the walk is emitted rather than a constant. A property of the object
    // goes, and the answer is true.
    assert_eq!(
        run("var o = { a: 1 }; with (o) { var gone = delete a; } gone + ':' + ('a' in o)"),
        "true:false"
    );
    // …a **declarative** binding does not, however the walk reached it — §9.1.1.1.5 makes every one
    // of them non-deletable, and this is the answer the compiler gives outside a `with` too.
    //
    // A `var` inside a *function* and a `let` at the top level, because a top-level `var` is not a
    // declarative binding at all: §16.1.7 makes it a property of the global object, which is the
    // third answer below and not this one.
    assert_eq!(
        run("function f() { var inner = 1; with ({}) { return delete inner; } } f()"),
        "false"
    );
    assert_eq!(
        run("let top = 1; with ({}) { var kept = delete top; } kept + ':' + top"),
        "false:1"
    );
    // …and a name that is nowhere is the global object's business, where §10.1.10.1 step 2 makes a
    // property that is not there true. A configurable global goes; one a `var` made does not.
    assert_eq!(
        run("with ({}) { var absent = delete nothingAtAll; } absent"),
        "true"
    );
    assert_eq!(
        run(
            "globalThis.loose = 1; with ({}) { var went = delete loose; } went + ':' + ('loose' in globalThis)"
        ),
        "true:false"
    );
    // A property the object does **not** have is not found there, so the walk carries on past it —
    // which is what makes `delete toString` inside a `with` answer about the global and leave
    // `Object.prototype.toString` exactly where it was.
    assert_eq!(
        run("var o = {}; with (o) { var answer = delete toString; } \
             answer + ':' + (typeof Object.prototype.toString)"),
        "true:function"
    );
    // A `delete` of anything else inside one is untouched.
    assert_eq!(
        run("var o = { a: 1 }; with (o) { delete o.a; } o.a === undefined"),
        "true"
    );
}

#[test]
fn a_write_to_a_name_inside_a_with_reaches_exactly_one_place() {
    // §9.1.1.2.4 and §9.1.1.1.5 — the walk finds one of three things and writes to that one. What
    // makes these rows worth having is the *second* half of each: nothing else is written, and in
    // particular nothing falls through to the global object when the name was found.
    //
    // A property of the `with` object.
    assert_eq!(
        run("var o = { a: 1 }; with (o) { a = 2; } o.a + ':' + (typeof globalThis.a)"),
        "2:undefined"
    );
    // A declarative binding, which a `with` that does not have the name walks straight past.
    assert_eq!(
        run("function f() { var v = 1; with ({}) { v = 2; } \
             return v + ':' + (typeof globalThis.v); } f()"),
        "2:undefined"
    );
    // …and a name nothing has is the global object's, which is what makes an undeclared assignment
    // inside a `with` behave as it does outside one.
    assert_eq!(run("with ({}) { fresh = 3; } globalThis.fresh"), "3");
}

#[test]
fn a_write_a_binding_refuses_inside_a_with_is_refused_the_same_way_it_would_be_outside() {
    // §9.1.1.1.5 step 5 — an immutable binding refuses the write, and step 5.b decides whether the
    // refusal is audible. A `with` body is sloppy code by construction (§11.2.1 refuses one in
    // strict code), so §15.2.5's function name is the case where the refusal is **silent**.
    assert_eq!(
        run("var out = 'no throw'; \
             var kept = (function f() { with ({}) { f = 1; } return typeof f; })(); \
             out + ':' + kept + ':' + (typeof globalThis.f)"),
        // Silent, still a function, and — the part a `return Ok(false)` would break — **not** written
        // to the global object instead.
        "no throw:function:undefined"
    );
    // A `const` refuses audibly wherever it is written, which is the other side of step 5.b.
    assert_eq!(
        run("const c = 1; var caught = 'none'; \
             with ({}) { try { c = 2; } catch (e) { caught = e.constructor.name; } } \
             caught + ':' + c"),
        "TypeError:1"
    );
    // …and a binding still in its dead zone is a ReferenceError rather than a write, which is a
    // different refusal from either: the binding is mutable and simply not ready.
    assert_eq!(
        run("var caught = 'none'; \
             { with ({}) { try { z = 1; } catch (e) { caught = e.constructor.name; } } \
               let z = 0; } \
             caught"),
        "ReferenceError"
    );
}

#[test]
fn a_compound_assignment_writes_through_the_reference_it_read() {
    // §13.15.2 evaluates the target **reference**, reads through it, evaluates the value, and
    // writes back through the *same* reference. praxis resolved the name twice, which is the same
    // answer for a slot and a different one inside a `with`: a getter may delete the property
    // between the two, and the second resolution then finds whatever the name means without it.
    //
    // So this wrote to the *outer* `x` and left `scope` without one.
    assert_eq!(
        run("var x = 0; \
             var scope = { get x() { delete this.x; return 2 } }; \
             with (scope) { x *= 3 } \
             'outer=' + x + ' scope=' + scope.x"),
        "outer=0 scope=6"
    );
    // The ordinary case is unchanged, which is what makes the above about the reference rather
    // than about `with` in general.
    assert_eq!(
        run("var x = 1; var o = { x: 5 }; with (o) { x += 2 } 'outer=' + x + ' o=' + o.x"),
        "outer=1 o=7"
    );
    // A name the object does not have falls through to the scope outside, and the write goes there.
    assert_eq!(run("var x = 1; with ({}) { x += 2 } x"), "3");
    // A deleted property is *recreated* by the write, because the reference still names the object:
    // `PutValue` on a property reference sets it whether or not it is still there.
    assert_eq!(
        run("var a = 'outer'; \
             var o = { get a() { delete o.a; return 1 } }; \
             with (o) { a += 1 } \
             'outer=' + a + ' o=' + o.a"),
        "outer=outer o=2"
    );
}

#[test]
fn a_resolved_reference_is_abandoned_by_a_throw_and_survives_a_yield() {
    // The reference is half-built state of exactly the kind an operand is, so it lives on a stack
    // with the same discipline: a handler records how many were waiting, and a throw truncates to
    // that mark. Without it the next assignment would write through a reference the abandoned
    // expression resolved.
    assert_eq!(
        run("var o = { x: 1 }; var said = 'none'; \
             try { with (o) { x += (function () { throw new TypeError('boom') })() } } \
             catch (e) { said = e.message } \
             said + '|' + o.x"),
        "boom|1"
    );
    // …and a second assignment afterwards still works, which is what proves nothing was left over.
    assert_eq!(
        run("var o = { x: 1 }; \
             try { with (o) { x += (function () { throw 1 })() } } catch (e) {} \
             with (o) { x += 5 } o.x"),
        "6"
    );
    // A **suspension** is the other direction: `with (o) { x += yield 1 }` resolves the target,
    // parks on the `yield`, and has to write through the reference the first half took rather than
    // one resolved again on the way back. So the reference stack parks with the body.
    assert_eq!(
        run(
            "function* g() { var o = { x: 1 }; with (o) { x += yield 1 } return o.x; } \
             var it = g(); it.next(); String(it.next(10).value)"
        ),
        "11"
    );
    // Nested, which is why it is a stack and not a register: the inner assignment resolves and
    // writes while the outer one is still waiting on its right-hand side.
    assert_eq!(
        run("var outer = { a: 1 }; var inner = { b: 10 }; \
             with (outer) { a += (function () { with (inner) { b += 5 } return inner.b })() } \
             outer.a + '|' + inner.b"),
        "16|15"
    );
}
