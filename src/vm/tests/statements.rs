//! Statements: what a script comes to, and how control moves through it.
//!
//! Every row runs *source* rather than asserting on a chunk: an instruction sequence is an
//! implementation detail and a value is not.

use super::*;

#[test]
fn a_script_evaluates_to_its_last_value_producing_statement() {
    // §14.2.2's `UpdateEmpty`. A declaration produces nothing, so it does not replace what
    // came before — which is why the third row is 1 and not `undefined`.
    assert_eq!(run("1;"), "1");
    assert_eq!(run("1; 2;"), "2");
    assert_eq!(run("1; var x = 2;"), "1");
    assert_eq!(run("var x = 2;"), "undefined");
    assert_eq!(run(""), "undefined");
    assert_eq!(run(";;;"), "undefined");
    assert_eq!(run("1; ;"), "1");
    assert_eq!(run("{ 1; }"), "1");
    assert_eq!(run("{ } 1; { }"), "1");
}

#[test]
fn a_var_is_hoisted_so_it_exists_before_its_declaration_and_holds_nothing() {
    // The whole of what hoisting is: the binding is made before the first statement runs and
    // the initializer is not. `x` is readable and `undefined` on the first line.
    assert_eq!(run("var seen = typeof x; var x = 1; seen;"), "undefined");
    assert_eq!(run("var before = x; var x = 1; before;"), "undefined");
    assert_eq!(run("var x = 1; x;"), "1");
    // …including from inside a block or a loop, because `var` belongs to the script rather
    // than to where it was written. That is the difference `let` was introduced to fix.
    assert_eq!(run("{ var inner = 5; } inner;"), "5");
    assert_eq!(
        run("var i = 0; while (i < 1) { var loop_var = 9; i = i + 1; } loop_var;"),
        "9"
    );
    // A second `var` with no initializer does not wipe the first one's value.
    assert_eq!(run("var x = 1; var x; x;"), "1");
    assert_eq!(run("var x = 1; var x = 2; x;"), "2");
}

#[test]
fn assignment_is_an_expression_whose_value_is_what_was_assigned() {
    assert_eq!(run("var a; a = 5;"), "5");
    assert_eq!(run("var a; var b; a = b = 3; a;"), "3");
    assert_eq!(run("var a = 1; a += 2; a;"), "3");
    assert_eq!(run("var a = 1; (a += 2);"), "3");
    assert_eq!(run("var a = 'x'; a += 1; a;"), "x1");
    assert_eq!(run("var a = 8; a /= 2; a;"), "4");
    assert_eq!(run("var a = 5; a **= 2; a;"), "25");
    assert_eq!(run("var a = 12; a &= 10; a;"), "8");
    assert_eq!(run("var a = 1; a <<= 3; a;"), "8");
}

#[test]
fn an_if_runs_one_branch_and_a_missing_else_runs_none() {
    assert_eq!(run("var r = 'none'; if (1) r = 'then'; r;"), "then");
    assert_eq!(run("var r = 'none'; if (0) r = 'then'; r;"), "none");
    assert_eq!(run("var r; if (0) r = 'then'; else r = 'else'; r;"), "else");
    assert_eq!(run("var r; if (1) r = 'then'; else r = 'else'; r;"), "then");
    // Truthiness rather than equality with `true`, and nesting.
    assert_eq!(run("var r = 0; if ('0') r = 1; r;"), "1");
    assert_eq!(run("var r = 0; if ('') r = 1; r;"), "0");
    assert_eq!(run("var r; if (1) if (0) r = 'a'; else r = 'b'; r;"), "b");
}

#[test]
fn the_three_loops_agree_about_when_they_test() {
    // `while` tests first, `do` tests last — so a false condition runs the body once in one
    // of them and never in the other.
    assert_eq!(run("var n = 0; while (0) n = n + 1; n;"), "0");
    assert_eq!(run("var n = 0; do n = n + 1; while (0) n;"), "1");
    assert_eq!(run("var n = 0; while (n < 5) n = n + 1; n;"), "5");
    assert_eq!(run("var n = 0; do n = n + 1; while (n < 5) n;"), "5");
    assert_eq!(
        run("var n = 0; for (var i = 0; i < 5; i = i + 1) n = n + i; n;"),
        "10"
    );
    // A `for` with parts missing: no init, no update, and no test at all.
    assert_eq!(run("var i = 0; for (; i < 3; ) i = i + 1; i;"), "3");
    assert_eq!(
        run("var i = 0; for (;;) { i = i + 1; if (i > 3) break; } i;"),
        "4"
    );
}

#[test]
fn break_leaves_the_loop_and_continue_goes_round_again() {
    assert_eq!(
        run("var n = 0; while (1) { n = n + 1; if (n > 2) break; } n;"),
        "3"
    );
    assert_eq!(
        run(
            "var n = 0; var i = 0; while (i < 5) { i = i + 1; if (i < 3) continue; n = n + 1; } n;"
        ),
        "3"
    );
    // In a `for` loop, `continue` still runs the update — which is the whole reason the third
    // part exists, and the thing a `while` translation gets wrong.
    assert_eq!(
        run("var n = 0; for (var i = 0; i < 5; i = i + 1) { if (i < 3) continue; n = n + 1; } n;"),
        "2"
    );
    assert_eq!(
        run("var i = 0; for (i = 0; i < 5; i = i + 1) { continue; } i;"),
        "5"
    );
    // In a `do` loop, `continue` goes to the *test*, so a loop whose test then fails stops.
    assert_eq!(
        run("var n = 0; do { n = n + 1; continue; } while (n < 3) n;"),
        "3"
    );
    // The innermost loop is the one that is left, and the outer one carries on.
    assert_eq!(
        run(
            "var n = 0; var i = 0; while (i < 3) { i = i + 1; var j = 0; while (1) { j = j + 1; if (j > 1) break; n = n + 1; } } n;"
        ),
        "3"
    );
}

#[test]
fn a_loop_that_never_runs_leaves_the_stack_and_the_completion_value_alone() {
    // The stack-neutrality every statement promises, checked where it is easiest to break: a
    // loop whose body pushes and pops, taken zero times and many times.
    assert_eq!(run("7; while (0) { 1; 2; 3; }"), "7");
    // …and a body that *does* run replaces the completion value, once per iteration.
    assert_eq!(
        run("7; var i = 0; while (i < 3) { i = i + 1; i * 10; }"),
        "30"
    );
    assert_eq!(run("7; for (var i = 0; i < 2; i = i + 1) i;"), "1");
}

#[test]
fn a_script_that_cannot_be_compiled_yet_says_which_construct_and_where() {
    let cases = [
        (
            "for (let i = 0; i < 1; i++) { (function () { return i; }); }",
            "a function that closes over a `let` or `const` declared in a loop",
        ),
        (
            "for (let i = 0; i < 1; i++) { (() => i); }",
            "a function that closes over a `let` or `const` declared in a loop",
        ),
        ("function* g() {}", "an async function or a generator"),
        ("try { } catch ([a]) { }", "a destructuring catch parameter"),
        ("for (var k of []) ;", "for-of"),
        ("var [a] = 1;", "a destructuring binding"),
        (
            "outer: { break outer; }",
            "a label on something that is not a loop",
        ),
        ("delete x;", "deleting a name"),
        ("var a; a ||= 1;", "a logical assignment"),
    ];
    for (source, what) in cases {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the source parses"); // the test is about compiling
        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported(what),
            "compiling {source:?}"
        );
    }
}

#[test]
fn a_throw_that_nothing_catches_leaves_the_script() {
    // §14.14 — anything at all may be thrown, and nothing asks what it is. An Error object
    // would be the usual thing; there are no objects yet and the language never required one.
    assert_eq!(run("throw 1;"), "thrown 1");
    assert_eq!(run("throw 'a' + 'b';"), "thrown ab");
    assert_eq!(run("throw void 0;"), "thrown undefined");
    // Everything after the throw is skipped, including the statement that would have set the
    // completion value.
    assert_eq!(run("1; throw 2; 3;"), "thrown 2");
    assert_eq!(
        run("var n = 0; while (1) { n = n + 1; if (n > 2) throw n; } n;"),
        "thrown 3"
    );
}

#[test]
fn a_catch_block_receives_the_value_and_the_script_carries_on() {
    assert_eq!(run("try { throw 1; } catch (e) { e; }"), "1");
    assert_eq!(
        run("try { throw 'x'; } catch (e) { 'caught ' + e; }"),
        "caught x"
    );
    // The try block's own value survives when nothing is thrown, and the catch block is not
    // entered at all.
    assert_eq!(run("try { 7; } catch (e) { 8; }"), "7");
    // ES2019's optional binding: the value is simply discarded.
    assert_eq!(run("try { throw 1; } catch { 'caught'; }"), "caught");
    // A throw inside a loop inside a try still finds the handler.
    assert_eq!(
        run(
            "try { var i = 0; while (1) { i = i + 1; if (i > 2) throw i; } } catch (e) { 'caught ' + e; }"
        ),
        "caught 3"
    );
}

#[test]
fn a_throw_in_the_middle_of_an_expression_leaves_no_rubbish_behind() {
    // The handler puts the operand stack back to the depth the protected region began at, so
    // the half-built operands of the interrupted expression are discarded rather than left
    // under everything that follows. No source can reach this yet — nothing throws from
    // inside an expression until an operation can — so the chunk is written by hand, the way
    // a malformed one is.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let chunk = Chunk::from_parts(
        vec![
            // try {
            Instruction::PushHandler(6),
            // …two operands of an expression that never finishes…
            Instruction::Constant(0),
            Instruction::Constant(0),
            // …and a throw from the middle of it.
            Instruction::Constant(1),
            Instruction::Throw,
            Instruction::PopHandler,
            // catch: the thrown value is here and the two operands are not.
            Instruction::SetCompletion,
        ],
        vec![Value::Number(9.0), Value::Number(1.0)],
    );
    // A leftover operand would be an unbalanced stack rather than a wrong answer, which is
    // exactly what makes the balance check worth having.
    let outcome = vm.run(&chunk, &mut heap).expect("well formed"); // the test is about the outcome
    assert_eq!(describe(outcome, &mut heap), "1");
}

#[test]
fn a_nested_try_is_caught_by_the_innermost_handler_that_is_still_open() {
    assert_eq!(
        run("try { try { throw 1; } catch (e) { 'inner ' + e; } } catch (e) { 'outer'; }"),
        "inner 1"
    );
    // A throw from a *catch* block is not caught by its own try.
    assert_eq!(
        run("try { try { throw 1; } catch (e) { throw 2; } } catch (e) { 'outer ' + e; }"),
        "outer 2"
    );
    // …and one that nothing catches still leaves the script.
    assert_eq!(
        run("try { throw 1; } catch (e) { throw e + 1; }"),
        "thrown 2"
    );
}

#[test]
fn a_finally_block_runs_on_both_ways_out() {
    // The normal way…
    assert_eq!(
        run("var log = ''; try { log = log + 'a'; } finally { log = log + 'b'; } log;"),
        "ab"
    );
    // …and the way that carries a thrown value, which then carries on outwards.
    assert_eq!(
        run("var log = ''; try { throw 1; } finally { log = log + 'f'; }"),
        "thrown 1"
    );
    assert_eq!(
        run(
            "var log = ''; try { try { throw 1; } finally { log = log + 'f'; } } catch (e) { log + e; }"
        ),
        "f1"
    );
    // All three tails together, and a throw from the *catch* block still runs the finally.
    assert_eq!(
        run(
            "var log = ''; try { try { throw 1; } catch (e) { log = log + 'c'; throw 2; } finally { log = log + 'f'; } } catch (e) { log + e; }"
        ),
        "cf2"
    );
    // …and when nothing throws at all, the catch is skipped and the finally is not.
    assert_eq!(
        run(
            "var log = ''; try { log = log + 't'; } catch (e) { log = log + 'c'; } finally { log = log + 'f'; } log;"
        ),
        "tf"
    );
}

#[test]
fn a_catch_parameter_shadows_an_outer_name_only_inside_its_block() {
    // §14.15.3 — the parameter is a binding of its own. Inside the block it is the thrown
    // value; outside it, the outer binding is untouched.
    assert_eq!(
        run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; }"),
        "inner"
    );
    assert_eq!(
        run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; } e;"),
        "outer"
    );
    // Assigning to it inside the block does not reach the outer one either.
    assert_eq!(
        run("var e = 'outer'; try { throw 1; } catch (e) { e = 'changed'; } e;"),
        "outer"
    );
}

#[test]
fn leaving_a_try_that_has_a_finally_is_refused_rather_than_skipping_it() {
    // A `break` past a `finally` is a third way out, and the finally would have to run on the
    // way. Refusing is narrow: a loop written *inside* the try is unaffected, which is the
    // second row.
    let mut heap = Heap::new();
    let script = parse_script("while (1) { try { break; } finally { } }").expect("parses"); // the test is about compiling
    let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
    assert_eq!(
        error.kind,
        crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
    );
    // A loop inside the `try` may still be left, because that jump crosses no finally.
    assert_eq!(
        run("var n = 0; try { while (1) { n = n + 1; break; } } finally { n = n + 10; } n;"),
        "11"
    );
    // …and a `break` inside a `try` that has only a `catch` is fine too.
    assert_eq!(
        run("var n = 0; while (1) { try { break; } catch (e) { } } n;"),
        "0"
    );

    // The guard belongs to the `try` that raised it and is put down when that `try` ends, so
    // a `break` *after* one is crossing nothing.
    assert_eq!(
        run("var n = 0; while (1) { try { } finally { } n = 1; break; } n;"),
        "1"
    );
    // …and an inner `try` with no finally does not put down the outer one's guard, so a
    // `break` inside it is still refused.
    let source = "while (1) { try { try { } catch (e) { } break; } finally { } }";
    let script = parse_script(source).expect("parses"); // the test is about compiling
    let error = compile_script(&script, &mut heap).expect_err("still crosses a finally"); // same
    assert_eq!(
        error.kind,
        crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
    );
}

#[test]
fn a_long_chain_costs_no_stack_and_a_deep_nest_is_refused() {
    // The two shapes a deep expression comes in, and they need opposite answers.
    //
    // A *chain* — `a + b + c` — is a tree as deep as it is long, and minified code chains
    // thousands of terms. It costs no recursion at all, because the left spine is walked with
    // a loop; two hundred thousand terms compile on a 1 MiB stack where four hundred used to
    // overflow.
    let long_chain = std::iter::repeat_n("1", 5000)
        .collect::<Vec<_>>()
        .join(" + ");
    assert_eq!(run(&long_chain), "5000");
    assert_eq!(
        run(&std::iter::repeat_n("1", 300)
            .collect::<Vec<_>>()
            .join(" + ")),
        "300"
    );

    // A *nest* does recurse, and is refused with a span rather than crashing. The parser
    // stops most of these first — this is the backstop for the ones it does not.
    let nested = format!("{}1{}", "[".repeat(4000), "]".repeat(4000));
    let mut heap = Heap::new();
    if let Ok(script) = parse_script(&nested) {
        let error = compile_script(&script, &mut heap).expect_err("too deep to compile"); // the test is about the error
        assert!(matches!(
            error.kind,
            crate::compile::ErrorKind::TooDeep | crate::compile::ErrorKind::Unsupported(_)
        ));
    }
}

#[test]
fn a_switch_falls_through_because_that_is_what_the_algorithm_says() {
    // §14.12.4 runs the tests in order until one is strictly equal, and then runs every statement
    // from there to the end — through the other cases, not into them. Fall-through is not a quirk
    // of the syntax; it is the algorithm.
    assert_eq!(
        run(
            "var r = ''; switch (2) { case 1: r = r + 'a'; case 2: r = r + 'b'; case 3: r = r + 'c'; } r;"
        ),
        "bc"
    );
    assert_eq!(
        run(
            "var r = ''; switch (2) { case 1: r = r + 'a'; break; case 2: r = r + 'b'; break; } r;"
        ),
        "b"
    );
    assert_eq!(
        run("var r = ''; switch (9) { case 1: r = r + 'a'; break; default: r = r + 'd'; } r;"),
        "d"
    );
    // Strictly equal, so no conversion: `'1'` does not match `1`.
    assert_eq!(
        run("switch ('1') { case 1: 'number'; break; default: 'no match'; }"),
        "no match"
    );
    assert_eq!(
        run("switch (1) { case 1: 'matched'; break; default: 'no'; }"),
        "matched"
    );
    // A switch with nothing in it evaluates its discriminant and does nothing else.
    assert_eq!(run("var seen = 0; switch (seen = 1) { } seen;"), "1");
}

#[test]
fn the_default_case_is_tried_last_wherever_it_is_written() {
    // §14.12.4 runs *every* test first and only then comes back to the default — so where the
    // default sits decides what falls through into what, and not whether it is reached.
    assert_eq!(
        run("var r = ''; switch (1) { default: r = r + 'd'; case 1: r = r + 'b'; } r;"),
        "b"
    );
    assert_eq!(
        run("var r = ''; switch (2) { default: r = r + 'd'; case 1: r = r + 'b'; } r;"),
        "db"
    );
    assert_eq!(
        run("var r = ''; switch (2) { case 1: r = r + 'b'; default: r = r + 'd'; } r;"),
        "d"
    );
}

#[test]
fn a_label_names_the_statement_that_break_and_continue_aim_at() {
    // §14.13 — the label is on the statement, and `break name` leaves *that* one rather than the
    // innermost. Two loops deep is where the difference shows.
    assert_eq!(
        run("var n = 0; outer: while (1) { while (1) { n = n + 1; break outer; } } n;"),
        "1"
    );
    assert_eq!(
        run(
            "var n = 0; outer: for (var i = 0; i < 3; i = i + 1) { for (var j = 0; j < 3; j = j + 1) { n = n + 1; continue outer; } } n;"
        ),
        "3"
    );
    // …and on one loop it behaves as the unlabelled form does.
    assert_eq!(
        run(
            "var s = 0; a: for (var i = 0; i < 3; i = i + 1) { if (i === 1) continue a; s = s + i; } s;"
        ),
        "2"
    );
    assert_eq!(run("var n = 0; a: while (1) { n = 1; break a; } n;"), "1");
    // An inner label wins over an outer one of a different name only where it is aimed at.
    assert_eq!(
        run(
            "var log = ''; a: for (var i = 0; i < 2; i = i + 1) { b: for (var j = 0; j < 2; j = j + 1) { log = log + 'x'; continue b; } } log;"
        ),
        "xxxx"
    );
}

#[test]
fn a_labelled_break_may_not_cross_a_finally_either() {
    // The same rule the unlabelled one has, and the label makes it easier to break: `break outer`
    // from inside a `try` leaves a statement further out, so it crosses the finally on the way.
    let mut heap = Heap::new();
    let source = "outer: while (1) { try { break outer; } finally { } }";
    let script = parse_script(source).expect("parses"); // the test is about compiling
    let error = compile_script(&script, &mut heap).expect_err("crosses a finally"); // same
    assert_eq!(
        error.kind,
        crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
    );

    // A loop *inside* the try is unaffected, labelled or not — the jump crosses nothing.
    assert_eq!(
        run(
            "var n = 0; try { inner: while (1) { n = 1; break inner; } } finally { n = n + 10; } n;"
        ),
        "11"
    );
    // …and so is a labelled break that stays outside the try altogether.
    assert_eq!(
        run(
            "var n = 0; outer: while (1) { try { n = 1; } finally { n = n + 10; } break outer; } n;"
        ),
        "11"
    );
}

#[test]
fn a_name_that_is_nowhere_is_a_reference_error_when_it_is_read() {
    // §6.2.5.5 — the one line that separates a name from a property. `o.missing` is `undefined`
    // because an object was asked for a property it has not got; `missing` is an error because a
    // *scope* was asked for a binding that does not exist, and there is no such thing.
    assert_eq!(
        run("try { nowhere } catch (e) { e.name }"),
        "ReferenceError"
    );
    // The message names it, because at run time the name is the whole diagnosis — there is no
    // span to point at and nothing else to say.
    assert_eq!(
        run("try { nowhere } catch (e) { e.message }"),
        "nowhere is not defined"
    );
    // A compound assignment reads before it writes (§13.15.2), so it throws from the read.
    assert_eq!(
        run("try { nowhere += 1 } catch (e) { e.name }"),
        "ReferenceError"
    );
    // …and an object really does answer `undefined` for the same question, which is the contrast
    // the whole rule exists to make.
    assert_eq!(run("var o = {}; o.missing"), "undefined");
}

#[test]
fn typeof_is_the_one_operator_that_survives_a_name_that_is_nowhere() {
    // §13.5.1.1 step 2. This is how a program asks whether a feature exists at all, and test262's
    // own harness does exactly this before it reaches for JSON — so getting it wrong turns the
    // question into the error it was written to avoid.
    assert_eq!(run("typeof nowhere"), "undefined");
    // Only the *bare name* form is spared. Everything else evaluates its operand first, so a
    // property of a name that is nowhere still throws.
    assert_eq!(
        run("try { typeof nowhere.x } catch (e) { e.name }"),
        "ReferenceError"
    );
    // A global that exists is described by what it holds, not by whether it was declared.
    assert_eq!(run("var here = 1; typeof here"), "number");
    assert_eq!(run("var undef; typeof undef"), "undefined");
    // A local shadows the global path entirely, and `typeof` follows it.
    assert_eq!(
        run("function f() { var here = 'a'; return typeof here } f()"),
        "string"
    );
}

#[test]
fn a_script_var_is_a_property_of_the_global_object_and_a_function_local_is_not() {
    // §9.1.1.4 — the difference that makes `globalThis` mean anything. A script's `var` is a
    // property; a function's is a slot nothing outside can reach.
    assert_eq!(run("var x = 5; this.x"), "5");
    assert_eq!(run("var x = 5; globalThis.x"), "5");
    assert_eq!(
        run("function f() { var inner = 1; return 0 } f(); typeof globalThis.inner"),
        "undefined"
    );
    // A function declaration at the top level is one too, and it is the same object.
    assert_eq!(run("function f() { return 7 } globalThis.f()"), "7");
    // Assigning to a name that is nowhere creates the global — §6.2.5.6's sloppy-mode half.
    assert_eq!(run("made = 3; globalThis.made"), "3");
}

#[test]
fn a_script_var_may_not_be_deleted_and_a_property_of_the_same_name_may() {
    // §9.1.1.4.17 gives a `var` binding `[[Configurable]]: false`, and that single attribute is
    // the whole observable difference between `var x = 1` and `globalThis.x = 1`.
    assert_eq!(run("var x = 1; delete globalThis.x"), "false");
    assert_eq!(run("function f() {} delete globalThis.f"), "false");
    assert_eq!(run("globalThis.y = 1; delete globalThis.y"), "true");
    // …and deleting one that was never there is `true`, which is what makes the first row a
    // statement about the attribute rather than about the property merely existing.
    assert_eq!(run("delete globalThis.neverThere"), "true");
    // Hoisting does not put `undefined` back over a value that is already there.
    assert_eq!(run("var x = 1; var x; x"), "1");
}

#[test]
fn the_globals_that_are_values_rather_than_functions_are_there_with_their_attributes() {
    // §19.1.2–4. That `undefined` is a read-only property rather than a keyword is why
    // `var undefined = 1` is legal and does nothing at all.
    assert_eq!(run("undefined"), "undefined");
    assert_eq!(run("NaN !== NaN"), "true");
    assert_eq!(run("1 / 0 === Infinity"), "true");
    assert_eq!(run("globalThis === this"), "true");
    // Not writable, and a sloppy-mode write is silently ignored rather than an error.
    assert_eq!(run("Infinity = 1; 1 / 0 === Infinity"), "true");
    // Not configurable either, which `globalThis` — an ordinary property — is.
    assert_eq!(run("delete globalThis.NaN"), "false");
    assert_eq!(run("delete globalThis.globalThis"), "true");
}

#[test]
fn a_global_is_looked_up_each_time_because_a_script_can_change_it_underneath() {
    // The reason a global carries its *name* into the bytecode while a local carries a number:
    // the global scope is not closed. A function compiled before the global existed still finds
    // it, which is what makes the whole of test262's harness work.
    assert_eq!(run("function f() { return later } later = 4; f()"), "4");
    // …and one that stops existing goes back to being an error.
    assert_eq!(
        run(
            "globalThis.gone = 1; function f() { return gone } delete globalThis.gone; try { f() } catch (e) { e.name }"
        ),
        "ReferenceError"
    );
}
