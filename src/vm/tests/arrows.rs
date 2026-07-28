//! §15.3 — an arrow function, and the three things it does not have.
//!
//! An arrow differs from a function expression in one fact with three consequences: it is written
//! *over* the scope around it rather than opening one of its own, so it has no `this`, no
//! `prototype` and no `[[Construct]]`. The rows below are about that fact, and most of them are
//! arranged so that a caller-`this` implementation — which agrees with a lexical one whenever the
//! arrow is called from inside the call that made it — gives a different answer.

use super::*;

#[test]
fn a_concise_body_returns_its_expression_and_a_block_body_does_not() {
    // §15.3.3 — `ConciseBody : ExpressionBody` evaluates and *returns*; `{ … }` is an ordinary
    // `FunctionBody`, so a value written there is discarded like any expression statement.
    assert_eq!(run("var f = x => x + 1; f(2)"), "3");
    assert_eq!(run("var f = (a, b) => a * b; f(3, 4)"), "12");
    assert_eq!(run("var f = () => 'x'; f()"), "x");
    assert_eq!(run("var f = () => { return 7; }; f()"), "7");
    assert_eq!(run("var f = () => { 7; }; typeof f()"), "undefined");
    // §10.2.1 step 4 — a block body that falls off the end returns `undefined`, exactly as a
    // function's does. An expression body has no such path: it always returns.
    assert_eq!(run("var f = () => {}; typeof f()"), "undefined");
    assert_eq!(
        run("var f = x => { if (x) { return 'y'; } return 'n'; }; f(1) + f(0)"),
        "yn"
    );
    // The body is an expression and not a statement list, so `{` after the arrow can only be a
    // block — which is why an object literal needs its parentheses (§15.3.3's lookahead).
    assert_eq!(run("var f = () => ({ v: 1 }); f().v"), "1");
}

#[test]
fn an_arrow_takes_the_this_of_where_it_was_written_not_of_who_calls_it() {
    // §10.2.1.2 step 1 — an arrow's `[[ThisMode]]` is `lexical`, so `OrdinaryCallBindThis` binds
    // nothing and the receiver the call computed is discarded.
    let made = "var o = { n: 1, m: function () { return () => this.n; } }; ";
    // Called from inside the call that made it, every implementation agrees…
    assert_eq!(run(&format!("{made} o.m()()")), "1");
    // …so these are the rows that matter: the arrow leaves the call that made it and is then
    // called in four ways that would each hand an ordinary function a different `this`.
    assert_eq!(run(&format!("{made} var g = o.m(); g()")), "1");
    assert_eq!(
        run(&format!("{made} var p = {{ n: 2, f: o.m() }}; p.f()")),
        "1"
    );
    assert_eq!(
        run(&format!(
            "{made} var g = o.m(); var q = {{ n: 3, h: function () {{ return g(); }} }}; q.h()"
        )),
        "1"
    );
    assert_eq!(run(&format!("{made} [0].map(o.m())[0]")), "1");
    // …including the one that exists to set `this`, which an arrow is immune to.
    assert_eq!(run(&format!("{made} o.m().call({{ n: 9 }})")), "1");
    // The same when the arrow is made by a constructor: it keeps the object under construction
    // even after being pulled off it, which is what makes an arrow a safe callback.
    assert_eq!(
        run("function F() { this.n = 5; this.get = () => this.n; } var d = new F().get; d()"),
        "5"
    );
    // An arrow inside an arrow reaches through both — there is no `this` at either level to stop
    // at, so it arrives at the same one.
    assert_eq!(
        run("var o = { n: 6, m: function () { return () => () => this.n; } }; o.m()()()"),
        "6"
    );
    // A `this` captured where there is no function at all is the script's, which sloppy mode makes
    // the global object rather than `undefined`.
    assert_eq!(run("var f = () => this; f() === this"), "true");
}

#[test]
fn an_ordinary_function_written_inside_an_arrow_still_binds_its_own_this() {
    // The other half of the rule, and the one an implementation that simply never wrote `this`
    // would get wrong: lexical `this` is the *arrow's* property, not a mode the machine enters.
    // A `function` inside an arrow binds `this` from its own call, as it always did.
    assert_eq!(
        run(
            "var o = { n: 1, m: function () { return () => { var g = function () { return this.n; }; return g.call({ n: 2 }); }; } }; o.m()()"
        ),
        "2"
    );
    // …and after that inner call returns, the arrow's own `this` is back — a frame restores it.
    assert_eq!(
        run(
            "var o = { n: 1, m: function () { return () => { (function () { return this; }).call({ n: 2 }); return this.n; }; } }; o.m()()"
        ),
        "1"
    );
}

#[test]
fn an_arrow_is_not_a_constructor_and_has_no_prototype_to_be_one_with() {
    // §15.3 gives an arrow no `[[Construct]]`, which §10.2.5's `MakeConstructor` is the other side
    // of: no `prototype` property is made, because nothing could ever inherit from it.
    assert_eq!(run("var f = () => 1; typeof f.prototype"), "undefined");
    // Absent rather than present-and-undefined: there is no own `prototype` at all. Asked as
    // `indexOf` rather than as a count, because a count would also be asserting which *other* own
    // properties exist — and §10.2.3's `length` and `name` are not built yet. A test that said
    // "no own properties" would pass today for the wrong reason and have to be deleted the day
    // they land.
    assert_eq!(
        run("var f = () => 1; Object.getOwnPropertyNames(f).indexOf('prototype')"),
        "-1"
    );
    // …while an ordinary function has one, which is what says the absence is the arrow's doing.
    assert_eq!(run("var f = function () {}; typeof f.prototype"), "object");
    assert_eq!(
        run("var f = function () {}; Object.getOwnPropertyNames(f).indexOf('prototype') >= 0"),
        "true"
    );
    assert_eq!(
        run("var f = () => 1; try { new f(); } catch (e) { e.name }"),
        "TypeError"
    );
    // Assigning a `prototype` does not make it constructable: the refusal is about the code, not
    // about whether the property happens to be there.
    assert_eq!(
        run("var f = () => 1; f.prototype = {}; try { new f(); } catch (e) { e.name }"),
        "TypeError"
    );
    // It is a function in every other way: callable, an object, and its own value.
    assert_eq!(run("typeof (() => 1)"), "function");
    assert_eq!(run("var f = () => 1; f.own = 2; f.own"), "2");
    assert_eq!(run("var f = () => 1; f === f"), "true");
    assert_eq!(run("(() => 1) === (() => 1)"), "false");
}

#[test]
fn an_arrows_parameters_and_variables_belong_to_the_call() {
    // Nothing lexical about `this` leaks into the ordinary scoping: an arrow is still a function,
    // so its parameters are its own and each call gets a fresh set.
    assert_eq!(
        run("var n = 'outer'; var f = n => n; f('inner') + n"),
        "innerouter"
    );
    assert_eq!(
        run("var f = () => { var v = 'inner'; return v; }; typeof v + f()"),
        "undefinedinner"
    );
    // A parameter the caller did not supply is `undefined`, and a surplus argument is discarded.
    assert_eq!(run("var f = (a, b) => typeof b; f(1)"), "undefined");
    assert_eq!(run("var f = a => a; f(1, 2, 3)"), "1");
    // It closes over the scope it was written in, by reference rather than by copy…
    assert_eq!(run("var a = 1; var f = () => a; a = 2; f()"), "2");
    assert_eq!(run("var f = a => b => a + b; f(1)(2)"), "3");
    // …and a `var` inside a block body hoists to the call, as a function's does.
    assert_eq!(
        run("var f = () => { if (true) { var v = 1; } return v; }; f()"),
        "1"
    );
    // A throw from inside one is an ordinary throw.
    assert_eq!(
        run("var f = () => { throw new RangeError('r'); }; try { f(); } catch (e) { e.name }"),
        "RangeError"
    );
}

#[test]
fn the_arrow_forms_that_are_not_built_yet_are_refused_rather_than_guessed() {
    // Each of these has semantics the engine does not have, and a refusal is the only answer that
    // is not a wrong one. §15.9's async arrow needs a job queue; the three parameter forms need
    // code to run inside the callee before its body.
    for (source, what) in [
        ("var f = async x => x;", "an async arrow function"),
        ("var f = (...a) => a;", "a rest parameter"),
        ("var f = (a = 1) => a;", "a default parameter"),
        ("var f = ([a]) => a;", "a destructuring parameter"),
    ] {
        let script = crate::parser::parse_script(source).expect("the source parses"); // the test is about the refusal
        let mut heap = Heap::new();
        let error = compile_script(&script, &mut heap).expect_err("refused"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported(what),
            "compiling {source:?}"
        );
    }
}
