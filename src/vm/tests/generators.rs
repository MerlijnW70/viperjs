//! §15.5 and §27.5 — a generator function, the object it answers with, and the three resumptions.
//!
//! Nothing here yields: `yield` is a slice of its own, and everything below is about the half that
//! has to be right before it can arrive. A generator function that never suspends is still a very
//! different thing from an ordinary one — it runs none of its body when called, it is not a
//! constructor, and what it hands back has a state machine of its own.

use super::*;

#[test]
fn calling_a_generator_function_runs_none_of_its_body() {
    // §15.5.4 — the difference a script can see first, and the one everything else rests on.
    // `EvaluateGeneratorBody` binds the parameters, makes the object and stops; the body waits for
    // a resumption that may never come.
    assert_eq!(
        run(
            "var ran = false; function* g() { ran = true; } var it = g(); var before = ran; it.next(); before + ':' + ran"
        ),
        "false:true"
    );
    // The arguments **are** bound at the call, which is what makes two generators from one
    // function independent — see `a_generator_keeps_the_this_and_the_arguments_of_the_call`.
    //
    // A parameter *default* is not, and that is a known divergence rather than a claim: praxis
    // compiles defaults into the top of the body, so `function* g(a = side()) {}` runs `side` at
    // the first resumption where §15.5.4 runs it at the call. Fixing it means splitting the
    // prologue from the body so the call can run the first and park at the second, and it is
    // written down here rather than asserted either way — a row pinning what the engine does today
    // would make the bug permanent.
}

#[test]
fn a_generator_answers_with_an_iterator_result() {
    // §27.5.3.2 step 5 — a resumption does not answer with what the body returned. It wraps it,
    // which is what makes a generator usable as an iterator without anything in between.
    assert_eq!(
        run("function* g() { return 7; } var r = g().next(); r.value + ':' + r.done"),
        "7:true"
    );
    // A body that falls off the end returned `undefined`, and the wrapping is the same.
    assert_eq!(
        run("function* g() {} var r = g().next(); r.value + ':' + r.done"),
        "undefined:true"
    );
    // The result is an ordinary object with two own properties, and §7.4.13 gives each of them
    // **all three** attributes. `Object.keys` only asks about the enumerable one, so the other two
    // are read off the descriptor — a result object whose `value` could not be written or removed
    // would pass every row above and fail the first program that reused one.
    assert_eq!(
        run("function* g() {} Object.keys(g().next()).join(',')"),
        "value,done"
    );
    for name in ["value", "done"] {
        assert_eq!(
            run(&format!(
                "function* g() {{}} var d = Object.getOwnPropertyDescriptor(g().next(), '{name}'); d.writable + ':' + d.enumerable + ':' + d.configurable"
            )),
            "true:true:true",
            "the descriptor of {name}"
        );
    }
}

#[test]
fn a_finished_generator_answers_the_same_thing_for_ever() {
    // §27.5.1.2 step 5 — there is no execution left to resume, and asking again is not an error.
    // The `7` is gone with the first answer: a generator does not remember what it returned.
    assert_eq!(
        run(
            "function* g() { return 7; } var it = g(); it.next(); var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
    assert_eq!(
        run(
            "function* g() { return 7; } var it = g(); it.next(); it.next(); var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
}

#[test]
fn a_generator_object_inherits_from_the_function_that_made_it() {
    // §15.5.4 step 3's `OrdinaryCreateFromConstructor` — out of the *function's own* `prototype`,
    // which is an ordinary writable property. This is the chain `next` is found along.
    assert_eq!(
        run("function* g() {} Object.getPrototypeOf(g()) === g.prototype"),
        "true"
    );
    // …and that object inherits from %GeneratorPrototype%, which is where `next` actually lives.
    assert_eq!(
        run(
            "function* g() {} var proto = Object.getPrototypeOf(Object.getPrototypeOf(g())); proto.hasOwnProperty('next')"
        ),
        "true"
    );
    // §10.1.13 — a `prototype` a script replaced with a non-object falls back to
    // %GeneratorPrototype% rather than failing, and the generator still works.
    assert_eq!(
        run("function* g() {} g.prototype = 1; var r = g().next(); r.done"),
        "true"
    );
    // A `prototype` a script replaced with an object is used, which is the other half of the same
    // sentence and the reason it is a `[[Get]]` rather than a look at the intrinsic.
    assert_eq!(
        run(
            "function* g() {} var mine = {}; g.prototype = mine; Object.getPrototypeOf(g()) === mine"
        ),
        "true"
    );
}

#[test]
fn the_two_prototypes_are_reachable_only_through_a_generator_function() {
    // §27.3 puts no `GeneratorFunction` on the global object, so this walk is the only way in.
    // The tags are what tell the two objects apart, and `Object.prototype.toString` reads them.
    assert_eq!(
        run("function* g() {} Object.prototype.toString.call(Object.getPrototypeOf(g()))"),
        "[object Generator]"
    );
    assert_eq!(
        run("function* g() {} Object.prototype.toString.call(Object.getPrototypeOf(g))"),
        "[object GeneratorFunction]"
    );
    // §27.3.3.2 and §27.5.1.1 — the two point at each other, which is what makes the pair a chain
    // rather than two unrelated objects.
    assert_eq!(
        run(
            "function* g() {} var f = Object.getPrototypeOf(g); f.prototype === Object.getPrototypeOf(Object.getPrototypeOf(g()))"
        ),
        "true"
    );
    assert_eq!(
        run(
            "function* g() {} var p = Object.getPrototypeOf(Object.getPrototypeOf(g())); p.constructor === Object.getPrototypeOf(g)"
        ),
        "true"
    );
    // §27.5.1's `[[Prototype]]` is %IteratorPrototype%, which is the whole of what makes a
    // generator iterable: the `[@@iterator]` it inherits answers the generator itself.
    assert_eq!(
        run("function* g() {} var it = g(); it[Symbol.iterator]() === it"),
        "true"
    );
    // A generator function is still a function, so §20.2.3's methods are up its chain.
    assert_eq!(run("function* g() {} typeof g.call"), "function");
}

#[test]
fn a_generator_function_is_not_a_constructor() {
    // §15.5.3 gives it no `[[Construct]]`, and for a reason that is not a method's or an arrow's:
    // it has a `this` and is written like an ordinary declaration. What `new` would make is an
    // object nothing ever inherits from — a generator's instances come from calling it.
    assert_eq!(
        run("function* g() {} try { new g(); } catch (e) { e.name }"),
        "TypeError"
    );
    // …and so §15.5.4's `prototype` carries no `constructor` back-pointer, unlike §10.2.5's.
    assert_eq!(
        run("function* g() {} g.prototype.hasOwnProperty('constructor')"),
        "false"
    );
    // It is writable and not configurable, which is what §15.5.4 gives it and is *not* the pair
    // §10.3.3 gives `length` beside it.
    assert_eq!(
        run(
            "function* g() {} var d = Object.getOwnPropertyDescriptor(g, 'prototype'); d.writable + ':' + d.enumerable + ':' + d.configurable"
        ),
        "true:false:false"
    );
}

#[test]
fn a_generator_cannot_be_resumed_while_it_is_running() {
    // §27.5.1.2 step 4 — the one state that is an error rather than an answer, because the
    // execution is not parked anywhere to be resumed *from*: something is in the middle of it.
    assert_eq!(
        run(
            "var it; function* g() { it.next(); } it = g(); try { it.next(); } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn the_three_resumptions_refuse_anything_that_is_not_a_generator() {
    // §27.5.1.2 step 2's `RequireInternalSlot`. An object that merely looks similar is not one,
    // and neither is a receiver that was lost on the way — which is what taking the method off the
    // object does.
    for source in [
        "var n = g().next; try { n(); } catch (e) { e.name }",
        "try { Object.getPrototypeOf(Object.getPrototypeOf(g())).next.call({}); } catch (e) { e.name }",
        "try { Object.getPrototypeOf(Object.getPrototypeOf(g())).next.call(1); } catch (e) { e.name }",
    ] {
        assert_eq!(run(&format!("function* g() {{}} {source}")), "TypeError");
    }
    // …and none of the three is a constructor, which §27.5.1 gives no method. Asked through
    // `extends` rather than through `new`, because those two ask *different questions*: §15.7.14
    // wants `IsConstructor` of the superclass and refuses at the class definition, where `new`
    // would go on to fail for the second reason as well — a construction passes no receiver, so a
    // resumption reached that way has no generator either way and answers TypeError regardless of
    // what `[[Construct]]` says. Only the first tells the two apart.
    // The `prototype` is given one on the way past, because §15.7.14 asks two questions in a row
    // and would otherwise refuse for the second: a superclass whose `prototype` is neither an
    // object nor null is a TypeError too, and a resumption has no `prototype` at all.
    assert_eq!(
        run(
            "function* g() {} var n = g().next; n.prototype = {}; try { class C extends n {} 'defined' } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn return_before_the_body_begins_completes_the_generator_without_running_it() {
    // §27.5.1.3 step 5 — there is no `try` the body could have entered, so nothing can intercept
    // the return. The argument becomes the answer and the generator is finished.
    assert_eq!(
        run(
            "var ran = false; function* g() { ran = true; } var it = g(); var r = it.return(4); r.value + ':' + r.done + ':' + ran"
        ),
        "4:true:false"
    );
    // …and it stays finished, which is what says the execution was dropped rather than skipped.
    assert_eq!(
        run(
            "function* g() { return 1; } var it = g(); it.return(4); var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
    // §27.5.1.3 step 4 — on an already-finished generator it hands back what it was given, which
    // is the one way to read a `value` other than `undefined` out of one.
    assert_eq!(
        run(
            "function* g() {} var it = g(); it.next(); var r = it.return(4); r.value + ':' + r.done"
        ),
        "4:true"
    );
}

#[test]
fn throw_before_the_body_begins_completes_it_and_throws() {
    // §27.5.1.4 step 5 — the mirror of `return`, and the value travels unchanged: a generator does
    // not wrap what it is asked to throw, because a throw is not a completion the caller reads.
    assert_eq!(
        run(
            "var ran = false; function* g() { ran = true; } var it = g(); try { it.throw(9); } catch (e) { e + ':' + ran }"
        ),
        "9:false"
    );
    // …and the generator is finished afterwards.
    assert_eq!(
        run(
            "function* g() { return 1; } var it = g(); try { it.throw(9); } catch (e) {} var r = it.next(); r.value + ':' + r.done"
        ),
        "undefined:true"
    );
    // §27.5.1.4 step 4 — and on a finished one it throws too, having nothing to intercept it.
    assert_eq!(
        run(
            "function* g() {} var it = g(); it.next(); try { it.throw(9); } catch (e) { 'caught ' + e }"
        ),
        "caught 9"
    );
}

#[test]
fn a_generator_keeps_the_this_and_the_arguments_of_the_call_that_made_it() {
    // The call decides both and the body reads them a resumption later, which is what makes the
    // parked execution a *whole* execution rather than a program counter. `this` is bound by
    // §10.2.1.2 when the generator function is called and cannot be moved afterwards.
    assert_eq!(
        run("var o = { n: 5, m: function* () { return this.n; } }; o.m().next().value"),
        "5"
    );
    assert_eq!(
        run("function* g(a, b) { return a + b; } g(1, 2).next().value"),
        "3"
    );
    assert_eq!(
        run("function* g() { return arguments.length; } g(1, 2, 3).next().value"),
        "3"
    );
    // A closure the body reads is the one the *call* made, so two generators from one function do
    // not share it.
    assert_eq!(
        run(
            "function* g(a) { return a; } var x = g(1), y = g(2); x.next().value + ':' + y.next().value"
        ),
        "1:2"
    );
}

#[test]
fn a_generator_method_and_a_generator_expression_are_generators_too() {
    // §15.5.1's three productions — a declaration, an expression, and a `*m()` in an object or a
    // class. All four reach the same `EvaluateGeneratorBody`, and a compiler that wired only the
    // declaration would pass every test above.
    assert_eq!(
        run("var g = function* () { return 1; }; g().next().value"),
        "1"
    );
    assert_eq!(
        run("var o = { *m() { return 2; } }; o.m().next().value"),
        "2"
    );
    assert_eq!(
        run("class C { *m() { return 3; } } new C().m().next().value"),
        "3"
    );
    // A method is not a constructor either, and a generator method is refused for both reasons at
    // once — which is worth a row because the two flags are separate.
    assert_eq!(
        run("var o = { *m() {} }; try { new o.m(); } catch (e) { e.name }"),
        "TypeError"
    );
}
