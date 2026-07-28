//! Functions, calls, closures and `this`.
//!
//! Every row runs *source* rather than asserting on a chunk: an instruction sequence is an
//! implementation detail and a value is not.

use super::*;

#[test]
fn a_function_declaration_exists_before_the_line_that_declares_it() {
    // The difference between a declaration and an assignment, and the reason both spellings
    // exist. §10.2.11 *initialises* a function declaration at instantiation time; a `var`
    // holding a function expression is only declared then, and assigned where it is written.
    assert_eq!(run("f(); function f() {} 'ran';"), "ran");
    assert_eq!(run("typeof f; function f() {}"), "function");
    assert_eq!(
        run("try { g(); } catch (e) { e.name; } var g = function () {};"),
        "TypeError"
    );
}

#[test]
fn a_call_passes_its_arguments_and_answers_what_was_returned() {
    assert_eq!(run("function f(a, b) { return a + b; } f(1, 2);"), "3");
    assert_eq!(
        run("function f(a, b) { return a + b; } f(1, 2) + f(10, 20);"),
        "33"
    );
    assert_eq!(run("function f() { return 'x'; } f();"), "x");
    // §10.2.1 step 4 — falling off the end is `undefined`, and so is a bare `return`.
    assert_eq!(run("function f() {} typeof f();"), "undefined");
    assert_eq!(run("function f() { return; } typeof f();"), "undefined");
    assert_eq!(run("function f() { 1; } typeof f();"), "undefined");
    // A parameter the caller did not supply is `undefined`; an argument too many is
    // discarded, since reaching it needs `arguments`.
    assert_eq!(
        run("function f(a, b) { return typeof b; } f(1);"),
        "undefined"
    );
    assert_eq!(run("function f(a) { return a; } f(1, 2, 3);"), "1");
    assert_eq!(run("function f() { return 1; } f(1, 2, 3);"), "1");
}

#[test]
fn a_function_is_a_value_and_says_so() {
    assert_eq!(run("function f() {} typeof f;"), "function");
    assert_eq!(run("typeof function () {};"), "function");
    assert_eq!(run("var f = function () {}; typeof f;"), "function");
    assert_eq!(run("typeof {};"), "object");
    // It can be passed, returned and called through another name — and it is an object, so
    // two of them are never the same value.
    assert_eq!(run("function id(x) { return x; } id(id)(42);"), "42");
    assert_eq!(run("function f() {} var g = f; g === f;"), "true");
    assert_eq!(
        run("function make() { return function () {}; } make() === make();"),
        "false"
    );
    // …and a function is truthy and is an ordinary object otherwise.
    assert_eq!(run("function f() {} f ? 'yes' : 'no';"), "yes");
    assert_eq!(run("function f() {} f.own = 1; f.own;"), "1");
}

#[test]
fn a_functions_own_names_do_not_leak_and_the_scripts_do_not_hide() {
    // A parameter and a `var` belong to the call, so each call gets its own and the script
    // never sees them…
    assert_eq!(
        run("var n = 'outer'; function f(n) { return n; } f('inner') + n;"),
        "innerouter"
    );
    assert_eq!(
        run("var only = 'outer'; function f() { var only = 'inner'; return only; } f() + only;"),
        "innerouter"
    );
    // …while a name declared at the top level is reachable from inside, and writing it
    // reaches the same binding rather than a copy.
    assert_eq!(
        run("var total = 0; function add(n) { total = total + n; } add(2); add(3); total;"),
        "5"
    );
    assert_eq!(
        run("var shared = 'seen'; function f() { return shared; } f();"),
        "seen"
    );
}

#[test]
fn recursion_works_and_runs_out_with_a_range_error_rather_than_a_crash() {
    assert_eq!(
        run("function fact(n) { if (n <= 1) return 1; return n * fact(n - 1); } fact(10);"),
        "3628800"
    );
    assert_eq!(
        run("function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } fib(15);"),
        "610"
    );
    // §9.4's note: an implementation may limit recursion and should report it as a
    // RangeError. A frame here is a record rather than a Rust stack frame, so this is a
    // number the engine chose and not the host's stack running out.
    assert_eq!(
        run("function loop(n) { return loop(n + 1); } try { loop(0); } catch (e) { e.name; }"),
        "RangeError"
    );
    // …and the machine is usable afterwards, which is the half that matters: the frames are
    // unwound rather than abandoned.
    assert_eq!(
        run(
            "function loop(n) { return loop(n + 1); } function ok() { return 'fine'; } try { loop(0); } catch (e) { ok(); }"
        ),
        "fine"
    );
}

#[test]
fn calling_something_that_is_not_a_function_is_a_type_error() {
    for source in [
        "var x = 1; x();",
        "var x = 'a'; x();",
        "var x = {}; x();",
        "var x = null; x();",
    ] {
        let script = format!("try {{ {source} }} catch (e) {{ e.name + ': ' + e.message; }}");
        assert_eq!(
            run(&script),
            "TypeError: what was called is not a function",
            "running {source:?}"
        );
    }
}

#[test]
fn a_throw_crosses_a_call_and_finds_the_handler_that_was_waiting() {
    assert_eq!(
        run("function t() { throw 'inside'; } try { t(); } catch (e) { 'caught ' + e; }"),
        "caught inside"
    );
    // Through two calls, and past a `finally` that runs on the way.
    assert_eq!(
        run(
            "var log = ''; function inner() { throw 1; } function outer() { try { inner(); } finally { log = log + 'f'; } } try { outer(); } catch (e) { log + e; }"
        ),
        "f1"
    );
    // A handler *inside* the callee catches first, and the caller's is untouched.
    assert_eq!(
        run(
            "function t() { try { throw 1; } catch (e) { return 'inner'; } } try { t(); } catch (e) { 'outer'; }"
        ),
        "inner"
    );
    // …and the operand stack comes back level, so what follows is computed on a clean one.
    assert_eq!(
        run("function t() { throw 1; } var r; try { r = 1 + t(); } catch (e) { r = 9; } r;"),
        "9"
    );
}

#[test]
fn what_a_function_evaluates_to_is_not_the_scripts_completion_value() {
    // §14.2.2 — the completion value belongs to the script. A statement inside a function
    // discards its value, so calling one cannot change what the script came to.
    assert_eq!(run("7; function f() { 99; } f();"), "undefined");
    assert_eq!(run("function f() { 99; } f(); 7;"), "7");
    assert_eq!(run("7; function f() { 99; }"), "7");
}

#[test]
fn what_functions_cannot_do_yet_says_which_and_where() {
    let cases = [
        ("function* g() {}", "an async function or a generator"),
        ("async function f() {}", "an async function or a generator"),
        ("function f(a = 1) {}", "a default parameter"),
        ("function f(...rest) {}", "a rest parameter"),
        ("function f([a]) {}", "a destructuring parameter"),
        ("function f() {} f(...[1]);", "a spread argument"),
        ("var f = function () {}; f?.();", "optional chaining"),
    ];
    let mut heap = Heap::new();
    for (source, what) in cases {
        let script = parse_script(source).expect("the row parses"); // a row that does not is the bug
        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported(what),
            "compiling {source:?}"
        );
    }
}

#[test]
fn a_functions_statements_do_not_touch_the_scripts_completion_value() {
    // §14.2.2 — the completion value is the *script's*. The call itself is an expression
    // statement and sets it, which hides the difference; a call in a *declaration* does not,
    // so this is where a function writing to it would show.
    assert_eq!(run("7; function f() { 99; } var x = f();"), "7");
    assert_eq!(run("7; function f() { 99; } f();"), "undefined");
    assert_eq!(run("function f() { 99; } var x = f(); 'end';"), "end");
}

#[test]
fn the_call_limit_is_a_count_of_frames_and_the_count_is_exact() {
    // The limit is a number this engine chose, so an off-by-one in it is invisible unless
    // something counts. This counts: every entry increments, and the call that is refused is
    // the one that would have made the frames one deeper than allowed.
    let reached = run(
        "var deep = 0; function f() { deep = deep + 1; return f(); } \
         try { f(); } catch (e) { deep; }",
    );
    assert_eq!(reached, MAX_CALL_DEPTH.to_string());
    // …and it is a RangeError rather than anything else, which is what §9.4's note asks for.
    assert_eq!(
        run("function f() { return f(); } try { f(); } catch (e) { e.name; }"),
        "RangeError"
    );
}

#[test]
fn a_closure_keeps_the_variables_of_a_call_that_has_already_returned() {
    // The definition of a closure, and the reason a variable cannot live in a frame: by the
    // time `next` runs, `counter`'s call is over and `n` is still there.
    assert_eq!(
        run(
            "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
             var next = counter(); next(); next(); next();"
        ),
        "3"
    );
    // Capturing by *value* at creation would answer 1 three times, which is why this is the
    // first row: it is the mistake the whole design exists to avoid.
    assert_eq!(
        run("function adder(x) { return function (y) { return x + y; }; } adder(3)(4);"),
        "7"
    );
}

#[test]
fn each_call_makes_its_own_environment_and_closures_over_it_share_only_that_one() {
    // Two calls to the same function make two environments, so two closures made from them
    // count separately — while two closures from the *same* call share one.
    assert_eq!(
        run(
            "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
             var a = counter(); var b = counter(); a(); a(); b();"
        ),
        "1"
    );
    assert_eq!(
        run(
            "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
             var a = counter(); a(); a(); a();"
        ),
        "3"
    );
    // A recursive call does not overwrite its caller's variables, which is the same rule seen
    // from the other side.
    assert_eq!(
        run("function f(n) { var mine = n; if (n > 0) f(n - 1); return mine; } f(3);"),
        "3"
    );
}

#[test]
fn an_inner_function_writes_the_outer_variable_rather_than_a_copy() {
    assert_eq!(
        run("function o() { var x = 'a'; function set() { x = 'b'; } set(); return x; } o();"),
        "b"
    );
    // Through two levels, which is where a depth counted wrongly would show.
    assert_eq!(
        run(
            "function outer() { var x = 1; function middle() { function inner() { return x; } \
             return inner(); } return middle(); } outer();"
        ),
        "1"
    );
    assert_eq!(
        run(
            "function outer() { var x = 1; function middle() { function inner() { x = 9; } \
             inner(); } middle(); return x; } outer();"
        ),
        "9"
    );
    // …and the script's own variables are the far end of the same chain.
    assert_eq!(
        run("var top = 1; function f() { function g() { top = top + 1; } g(); } f(); top;"),
        "2"
    );
}

#[test]
fn a_parameter_is_a_variable_of_the_call_like_any_other() {
    // §10.2.11 — the parameters are the first slots of the call's environment, so a closure
    // over one is a closure over that call's copy.
    assert_eq!(
        run(
            "function hold(x) { return function () { return x; }; } var a = hold(1); \
             var b = hold(2); a() + b();"
        ),
        "3"
    );
    assert_eq!(
        run(
            "function hold(x) { return function () { x = x + 1; return x; }; } \
             var f = hold(10); f(); f();"
        ),
        "12"
    );
}

#[test]
fn a_method_call_receives_the_object_it_was_found_on() {
    // §13.3.6.1 — the receiver travels with the *call*, not with the function. The same
    // function called two ways has two different `this`, which is the whole reason a method
    // is not simply a property whose value happens to be callable.
    assert_eq!(
        run("var o = { v: 7 }; o.get = function () { return this.v; }; o.get();"),
        "7"
    );
    assert_eq!(
        run("var o = { v: 7 }; o.get = function () { return this.v; }; var f = o.get; typeof f();"),
        "undefined"
    );
    // The *nearest* base is the receiver, not the outermost one.
    assert_eq!(
        run("var o = { a: { v: 1 } }; o.a.get = function () { return this.v; }; o.a.get();"),
        "1"
    );
    // Arguments still work, and a computed key finds the same method.
    assert_eq!(
        run("var o = { v: 2 }; o.m = function (x) { return this.v + x; }; o.m(3);"),
        "5"
    );
    assert_eq!(
        run("var o = { v: 1 }; o['m'] = function () { return this.v; }; o['m']();"),
        "1"
    );
}

#[test]
fn the_base_of_a_method_call_is_evaluated_exactly_once() {
    // `f().m()` calls `f` once. Compiling the base twice — once to find the method and once
    // to be the receiver — would call it twice, and that is a side effect nobody asked for.
    assert_eq!(
        run("var calls = 0; function base() { calls = calls + 1; \
             return { m: function () { return 'ok'; } }; } base().m(); calls;"),
        "1"
    );
    // …and a computed key is evaluated once too, which is the same rule one level down.
    assert_eq!(
        run(
            "var keys = 0; var o = { m: function () { return 'ok'; } }; \
             function key() { keys = keys + 1; return 'm'; } o[key()](); keys;"
        ),
        "1"
    );
}

#[test]
fn a_call_with_no_receiver_gets_the_global_object() {
    // §10.2.1.2's substitution. Strict mode keeps the `undefined` instead, and telling the
    // two apart needs the flag the parser already computes — so this is the sloppy answer,
    // which is what an ordinary script gets.
    assert_eq!(run("function f() { return typeof this; } f();"), "object");
    assert_eq!(run("typeof this;"), "object");
    // The script's `this` and a plain call's are the same object (§16.1.7).
    assert_eq!(
        run("var top = this; function f() { return this === top; } f();"),
        "true"
    );
    // …and a method's is not.
    assert_eq!(
        run("var top = this; var o = { m: function () { return this === top; } }; o.m();"),
        "false"
    );
}

#[test]
fn this_is_restored_when_a_call_returns_however_it_returns() {
    assert_eq!(
        run(
            "var o = { v: 'inner', m: function () { return this.v; } }; \
             var outer = this; o.m(); typeof this;"
        ),
        "object"
    );
    // Including when the call left by throwing, which unwinds frames rather than returning.
    assert_eq!(
        run("var top = this; var o = { m: function () { throw 1; } }; \
             try { o.m(); } catch (e) { this === top; }"),
        "true"
    );
}
