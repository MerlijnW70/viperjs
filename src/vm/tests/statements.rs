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
fn a_loop_that_never_runs_leaves_the_stack_alone_and_its_value_undefined() {
    // The stack-neutrality every statement promises, checked where it is easiest to break: a
    // loop whose body pushes and pops, taken zero times and many times.
    //
    // **The value is `undefined` and not the 7 before it.** §14.7.3.2's `WhileLoopEvaluation`
    // begins "Let V be undefined" and returns V when the test first fails, so a loop taken zero
    // times still *produces* a value — it is not empty, and the statement before it does not show
    // through. This row asserted 7 until §14.2.2's family landed, which is the shape AGENTS.md
    // warns about: a test can pin the engine's behaviour rather than the clause's.
    assert_eq!(run("7; while (0) { 1; 2; 3; }"), "undefined");
    assert_eq!(run("7; while (false) { }"), "undefined");
    // …and a body that *does* run replaces it, once per iteration.
    assert_eq!(
        run("7; var i = 0; while (i < 3) { i = i + 1; i * 10; }"),
        "30"
    );
    assert_eq!(run("7; for (var i = 0; i < 2; i = i + 1) i;"), "1");
    // The three that really are EMPTY, which is a different thing from `undefined` and is why the
    // list of statements that begin a completion had to be exact rather than "most of them".
    assert_eq!(run("7; { }"), "7");
    assert_eq!(run("7; ;"), "7");
    assert_eq!(run("7; var later;"), "7");
}

#[test]
fn a_script_that_cannot_be_compiled_yet_says_which_construct_and_where() {
    // The one construct the compiler still refuses, in two positions. This row used to be `with`,
    // which is the shape AGENTS.md warns about: a test that asserts a refusal outlives the refusal
    // it describes, and then asserts the opposite of what the engine does. Every other row it has
    // held has landed, and nothing a **script** can say is refused any more — so these are modules.
    let cases = [
        ("var r = /(?i:a)/;", "the RegExp modifiers proposal"),
        (
            "async function* g() { yield /(?i:a)/; }",
            "the RegExp modifiers proposal",
        ),
    ];
    for (source, what) in cases {
        let mut heap = Heap::new();
        let module = crate::parser::parse_module(source).expect("the source parses"); // the test is about compiling
        let error =
            crate::compile::compile_module(&module, &mut heap).expect_err("not implemented yet"); // same
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
fn every_way_out_of_a_try_runs_its_finally_on_the_way_past() {
    // §14.15.3 — a `finally` runs on *every* completion of its `try`, and there are five: falling
    // off the end, throwing, `break`, `continue` and `return`. The first two were the easy ones,
    // because they are the two paths the code already had somewhere to put.
    assert_eq!(
        run("var n = 0; while (1) { try { break; } finally { n = 1; } } n;"),
        "1"
    );
    assert_eq!(
        run(
            "var r = ''; for (var i = 0; i < 3; i++) { try { continue; } finally { r += 'f'; } } r;"
        ),
        "fff"
    );
    // A `break` that is *not* the first statement still runs it, and only once.
    assert_eq!(
        run(
            "var r = ''; for (var i = 0; i < 3; i++) { try { if (i === 1) break; r += i; } \
             finally { r += 'f'; } } r;"
        ),
        "0ff"
    );
    // A loop written *inside* the `try` crosses no finally, and a `break` after one has already
    // ended crosses nothing either — both were the cases the old refusal took care to allow, and
    // both still have to give the same answers now that nothing is refused.
    assert_eq!(
        run("var n = 0; try { while (1) { n = n + 1; break; } } finally { n = n + 10; } n;"),
        "11"
    );
    assert_eq!(
        run("var n = 0; while (1) { try { } finally { } n = 1; break; } n;"),
        "1"
    );
    // Nested, innermost first — an inner `try` with no `finally` of its own is not a reason to
    // skip the outer one's.
    assert_eq!(
        run(
            "var r = ''; while (1) { try { try { } catch (e) { } break; } finally { r += 'o'; } } r;"
        ),
        "o"
    );
    assert_eq!(
        run(
            "var r = ''; while (1) { try { try { break; } finally { r += 'i'; } } \
             finally { r += 'o'; } } r;"
        ),
        "io"
    );
}

#[test]
fn a_finally_that_cannot_be_compiled_is_reported_from_the_innermost_one() {
    // A `break` compiles every `finally` it crosses, innermost first, and the first one that
    // cannot be compiled is the answer. Carrying on past it would report whichever of them failed
    // *last* — and if a later one compiled cleanly, would report no failure at all and emit a
    // `break` past a block that was never built.
    //
    // One refusal in the *inner* `finally`, and an ordinary statement in the outer. This used to
    // use two different ones, so that "the first" and "the last" were distinguishable answers
    // rather than one sentence arriving by two routes — but every refusal this test has held has
    // since landed, and §16.2.1.9's `import.meta` is the only one left in the engine. What survives
    // is the half that matters: a compiler that carried on past the inner failure would report
    // **no** failure at all, and emit a `break` past a block it never built.
    let source = "while (1) { try { try { break; } finally { /(?i:a)/; } } \
                  finally { globalThis.x = 1; } }";
    let mut heap = Heap::new();
    let module = crate::parser::parse_module(source).expect("the source parses"); // the test is about compiling
    let error = crate::compile::compile_module(&module, &mut heap)
        .expect_err("the inner finally is refused"); // same
    assert_eq!(
        error.kind,
        crate::compile::ErrorKind::Unsupported("the RegExp modifiers proposal")
    );
}

#[test]
fn a_return_runs_the_finallys_it_leaves_and_one_of_them_may_replace_it() {
    // §14.15.3 — the `try` completes with a return, the `finally` runs, and `UpdateEmpty` keeps
    // the return unless the `finally` completes abruptly itself. Skipping the block entirely
    // passed every test about the returned *value* and was wrong about everything else: a
    // `finally` is where a program puts the thing that must happen.
    assert_eq!(
        run(
            "var log = ''; function g() { try { return 1; } finally { log += 'f'; } } \
             var v = g(); v + ',' + log;"
        ),
        "1,f"
    );
    // …and when the `finally` has a completion of its own, that one wins. Both directions: a
    // `return` in the `finally` replaces the value, and one replaces a *throw* as well.
    assert_eq!(
        run("function g() { try { return 1; } finally { return 2; } } g();"),
        "2"
    );
    assert_eq!(
        run("function g() { try { throw new Error('x'); } finally { return 2; } } g();"),
        "2"
    );
    // Every enclosing one, innermost first, and the value survives all of them — it is parked in
    // a slot rather than left on the stack, because the blocks in between use the stack.
    assert_eq!(
        run(
            "var r = ''; function g() { try { try { return 'v'; } finally { r += 'i'; } } \
             finally { r += 'o'; } } g() + r;"
        ),
        "vio"
    );
    // The value is evaluated *before* the finally runs, so a `finally` that changes what the
    // expression read does not change what was returned.
    assert_eq!(
        run("function g() { var n = 1; try { return n; } finally { n = 99; } } g();"),
        "1"
    );
}

#[test]
fn a_jump_out_of_a_try_takes_down_the_handlers_it_jumps_past() {
    // Not the specification's: §14.15 unwinds by completion, where this VM unwinds by a stack of
    // handlers. A `break` that jumped out of a `try` used to leave its handler armed, and the
    // stale one then caught a throw that happened *after* the `try` had been left — landing in a
    // `catch` block belonging to a statement the program had finished with. An exception appearing
    // to be handled by code that is no longer running is about as wrong as an answer gets, and
    // nothing about it looks like an error from the outside.
    assert_eq!(
        run(
            "(function () { for (;;) { try { break; } catch (e) { return 'a stale handler fired'; } } \
             try { null.x; } catch (e) { return 'caught here'; } })()"
        ),
        "caught here"
    );
    // …and with no `try` after it at all, the throw escapes rather than finding the old one.
    assert_eq!(
        run(
            "(function () { try { for (;;) { try { break; } catch (e) { return 'stale'; } } null.x; } \
             catch (e) { return 'the outer one'; } })()"
        ),
        "the outer one"
    );
    // A `catch` block is inside one fewer handler than the `try` block is, because the throw that
    // got there took its own handler off the stack — so a `break` out of a `catch` owes one less.
    assert_eq!(
        run(
            "(function () { for (;;) { try { null.x; } catch (e) { break; } } \
             try { null.y; } catch (e) { return 'caught here'; } return 'escaped'; })()"
        ),
        "caught here"
    );
    // …and the same again with a `finally`, which leaves one armed where a bare `catch` leaves none.
    assert_eq!(
        run(
            "(function () { var r = ''; for (;;) { try { null.x; } catch (e) { break; } \
             finally { r += 'f'; } } try { null.y; } catch (e) { return r + ',caught'; } })()"
        ),
        "f,caught"
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
fn a_labelled_jump_runs_every_finally_between_here_and_its_label() {
    // A labelled break leaves more statements than an unlabelled one, and that is the whole of
    // what is different about it: each one it leaves gets what it is owed, in order.
    assert_eq!(
        run("var r = ''; outer: while (1) { try { break outer; } finally { r += 'f'; } } r;"),
        "f"
    );
    // Two of them, innermost first — the order is the nesting and nothing else.
    assert_eq!(
        run(
            "var r = ''; outer: while (1) { try { try { break outer; } finally { r += 'i'; } } \
             finally { r += 'o'; } } r;"
        ),
        "io"
    );
    // A labelled `continue` crossing a finally runs it too, and goes round again — which is the
    // row that separates the two: `break` and `continue` cross the same `finally` and stop at
    // different places.
    assert_eq!(
        run(
            "var r = ''; outer: for (var i = 0; i < 2; i++) { for (var j = 0; j < 2; j++) { \
             try { if (j === 0) continue outer; r += 'x'; } finally { r += 'f'; } } } r;"
        ),
        "ff"
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

#[test]
fn a_label_on_something_that_is_not_a_loop_is_a_break_target() {
    // §14.13.4 — a label on a non-iteration statement is a break target and nothing more:
    // `outer: { break outer; }` leaves the block, and there is no loop under it for the jump to land
    // in. `continue outer` naming one is a Syntax Error the parser refuses, which is why the label
    // needs a break list and no continue list.
    assert_eq!(
        run("(function () { var seen = []; \
             outer: { seen.push(1); break outer; seen.push(2); } \
             seen.push(3); return seen.join(','); })()"),
        "1,3"
    );
    // A break naming an outer label leaves everything between, loops included.
    assert_eq!(
        run("(function () { var n = 0; \
             outer: { for (var i = 0; i < 3; i++) { if (i === 1) break outer; n++; } n = 99; } \
             return n; })()"),
        "1"
    );
    // Two labels on one statement, which §14.13 allows and which share the one target.
    assert_eq!(
        run(
            "(function () { var out = []; a: b: { out.push(1); break a; } return out.join(','); })()"
        ),
        "1"
    );
    // A labelled block *inside* a loop does not swallow a `continue` aimed past it.
    assert_eq!(
        run("(function () { var log = []; \
             outer: for (const x of [1, 2, 3]) { inner: { if (x === 2) continue outer; log.push(x); } } \
             return log.join(','); })()"),
        "1,3"
    );
}

#[test]
fn a_labelled_break_closes_exactly_the_iterators_it_leaves() {
    // §7.4.9 — and the pair of rows that says the bookkeeping is right, because the two shapes differ
    // only in where the label sits. The list of open iterators is indexed by *breakable statement*,
    // one entry each with `None` for the ones that drive nothing; before this it held one entry per
    // open `for`-`of`, and the two indices agree only while every enclosing breakable is a `for`-`of`.
    // A `switch` or a labelled block between a label and its loop was enough to close the wrong ones.
    let counting = "var closed = 0; var it = { [Symbol.iterator]() { var i = 0; return { \
                    next() { return { value: i++, done: i > 5 }; }, \
                    return() { closed++; return {}; } }; } };";
    // Leaving a label *around* the loop leaves the loop, so the iterator is told.
    assert_eq!(
        run(&format!(
            "(function () {{ {counting} lab: {{ for (const x of it) {{ break lab; }} }} \
             return closed; }})()"
        )),
        "1"
    );
    // Leaving a label *inside* the loop stays in the loop, so it is not.
    assert_eq!(
        run(&format!(
            "(function () {{ {counting} for (const x of it) {{ lab: {{ break lab; }} }} \
             return closed; }})()"
        )),
        "0"
    );
    // A plain `break` out of a loop *inside* a `for`-`of` leaves that loop and not the `for`-`of`, so
    // the iterator is untouched — the row that says every breakable has an entry of its own, rather
    // than the innermost `for`-`of`'s being found by accident.
    assert_eq!(
        run(&format!(
            "(function () {{ {counting}              for (const x of it) {{ while (true) {{ break; }} break; }}              return closed; }})()"
        )),
        "1"
    );
    assert_eq!(
        run(&format!(
            "(function () {{ {counting} var n = 0;              for (const x of it) {{ for (var i = 0; i < 2; i++) {{ n++; break; }} }}              return closed + ',' + n; }})()"
        )),
        "0,5"
    );
    // …and a `switch` between the label and the loop is the shape that was wrong before: the break
    // leaves the labelled block and both loops inside it.
    assert_eq!(
        run(&format!(
            "(function () {{ {counting} \
             lab: {{ switch (1) {{ default: {{ for (const x of it) {{ break lab; }} }} }} }} \
             return closed; }})()"
        )),
        "1"
    );
}

#[test]
fn a_switch_has_one_environment_over_all_of_its_cases() {
    // §14.12.4 step 3 — `NewDeclarativeEnvironment` around the whole `CaseBlock`, and *one* of
    // them: a `let` in one case is in scope in the next, and none of them is in scope outside.
    assert_eq!(
        run("let a = 'outer'; switch (1) { case 1: let a2 = 'inner'; } a"),
        "outer"
    );
    assert_eq!(run("switch (1) { case 1: let h = 3; case 2: h }"), "3");
    assert_eq!(
        run("switch (99) { case 1: let e = 1; } 'fell through'"),
        "fell through"
    );
    assert_eq!(run("switch (99) { default: let g = 7; g }"), "7");
    // A closure made in a case keeps the case block's binding, which is what having an environment
    // at all is for.
    assert_eq!(
        run("var f; switch (1) { case 1: let c = 5; f = function () { return c; }; } f()"),
        "5"
    );
    // …and a switch entered twice makes its bindings twice, so two passes give two closures two
    // values rather than one shared slot.
    assert_eq!(
        run("var fs = []; for (var i = 0; i < 2; i++) { \
             switch (i) { case 0: case 1: let k = i; fs.push(function () { return k; }); } } \
             fs.map(function (g) { return g(); }).join(',')"),
        "0,1"
    );
}

#[test]
fn every_way_out_of_a_switch_leaves_its_discriminant_behind_exactly_once() {
    // The discriminant stays on the operand stack for the whole `CaseBlock`, because each case is
    // compared against it in turn. Falling off the end, a `break`, and no case matching at all all
    // converge on the one `Pop` — but a `continue` and a `break` to an outer label jump clean past
    // it, and those two **faulted the interpreter** before this: `UnbalancedStack`, from ordinary
    // source, on the next pass of the enclosing loop.
    assert_eq!(
        run("var n = 0; for (var i = 0; i < 3; i++) { switch (i) { case 1: continue; } n++; } n"),
        "2"
    );
    assert_eq!(
        run(
            "var n = 0; for (var i = 0; i < 3; i++) { switch (i) { case 1: let d = 1; continue; } n++; } n"
        ),
        "2"
    );
    assert_eq!(
        run(
            "var n = 0; outer: for (var i = 0; i < 3; i++) { switch (i) { case 1: break outer; } n++; } n"
        ),
        "1"
    );
    // A `return` is the exception and must **not** pop: the value it is returning is already on the
    // stack above the discriminant, so tidying here would discard that instead — and a call throws
    // its whole operand stack away regardless, so what is left under it was never a leak.
    assert_eq!(
        run("(function () { switch (1) { case 1: return 'returned'; } })()"),
        "returned"
    );
    assert_eq!(
        run("(function () { switch (1) { case 1: let a = 1; return 'with a let'; } })()"),
        "with a let"
    );
}

#[test]
fn a_loop_written_inside_a_switch_can_still_be_labelled_and_jumped_to() {
    // An exit's depth is one number indexing the break lists, and a switch pushes one of those
    // without being continuable — so the two stacks came apart, a label recorded against the break
    // list indexed a continue list that was not there, and the jump was never patched. It reached
    // the interpreter as the `u32::MAX` placeholder and faulted with `JumpOutOfRange`.
    assert_eq!(
        run(
            "var n = 0; switch (1) { case 1: outer: for (var i = 0; i < 3; i++) { n++; continue outer; } } n"
        ),
        "3"
    );
    assert_eq!(
        run(
            "var n = 0; switch (1) { case 1: outer: for (var i = 0; i < 3; i++) { n++; break outer; } } n"
        ),
        "1"
    );
    // The plain forms of both, in the same position, which never depended on the label lookup.
    assert_eq!(
        run("var n = 0; switch (1) { case 1: for (var i = 0; i < 3; i++) { n++; continue; } } n"),
        "3"
    );
    assert_eq!(
        run("var n = 0; switch (1) { case 1: for (var i = 0; i < 3; i++) { n++; break; } } n"),
        "1"
    );
    // A switch nested in a switch, so the depth is off by more than one in both directions.
    assert_eq!(
        run(
            "var n = 0; for (var i = 0; i < 3; i++) { switch (i) { case 0: switch (i) { case 0: continue; } } n++; } n"
        ),
        "2"
    );
}

#[test]
fn a_finally_re_emitted_by_a_jump_is_compiled_in_the_scope_it_was_written_in() {
    // `unwind_across` emits a `finally` again at every exit that leaves it, and it emits the
    // `PopScope`s for the scopes that exit is leaving *first*. The compiler's own idea of which
    // scopes are open did not follow, so the finally's names were resolved one hop too deep — a
    // `break` out of a block inside a `try`/`finally` reached the interpreter as
    // `Fault::MissingLocal`, from ordinary source and with no way to see it coming.
    assert_eq!(
        run(
            "(function () { var r = ''; for (;;) { try { { let a = 1; break; } } finally { r += 'f'; } } return r; })()"
        ),
        "f"
    );
    // The finally reads a name from the scope *around* the block, which is the read that was
    // resolving into the block's environment instead.
    assert_eq!(
        run("(function () { var out = 'no'; var tag = 'outer'; \
             for (;;) { try { { let a = 1; break; } } finally { out = tag; } } return out; })()"),
        "outer"
    );
    // Two scopes deep, so the correction is a count rather than a flag.
    assert_eq!(
        run(
            "(function () { var r = ''; for (;;) { try { { let a = 1; { let b = 2; break; } } } finally { r += 'f'; } } return r; })()"
        ),
        "f"
    );
    // …and a `continue` out of the same shape, which crosses the same entries by the other rule.
    assert_eq!(
        run(
            "(function () { var r = ''; for (var i = 0; i < 2; i++) { try { { let a = 1; continue; } } finally { r += 'f'; } } return r; })()"
        ),
        "ff"
    );
}

#[test]
fn a_var_belongs_to_its_function_however_many_scopes_it_is_written_inside() {
    // §14.3.2 — hoisting has already given a `var` a slot in the function, so the declaration is a
    // *store* to that binding and not a new one. `Compiler::declare` searches only the innermost
    // level, so inside a block with an environment of its own it made a second binding and stored
    // into that instead — and the value never reached the name anyone could read.
    assert_eq!(
        run("(function () { { let a = 1; var x = 2; } return x; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { try { null.y } catch (e) { var x = 2; } return x; })()"),
        "2"
    );
    // Assigning to an existing `var` from inside a scope, which is the same resolution by the
    // other route and was already right — here so the pair cannot drift.
    assert_eq!(
        run("(function () { var x = 1; { let a = 2; x = 3; } return x; })()"),
        "3"
    );
    // Two scopes deep, so the depth is a count rather than a flag.
    assert_eq!(
        run("(function () { { let a = 1; { let b = 2; var x = 3; } } return x; })()"),
        "3"
    );
    // …and read *before* the block runs, which is what hoisting means: the binding is there and
    // holds nothing, rather than being made when the declaration is reached.
    assert_eq!(
        run("(function () { var seen = typeof x; { let a = 1; var x = 2; } return seen; })()"),
        "undefined"
    );
}

#[test]
fn a_var_in_a_script_is_a_global_property_wherever_it_is_written() {
    // §16.1.7 — a script's `var` goes in the global variable scope, and no block it is written
    // inside changes that. The question used to be asked as "are any scopes open", which stopped
    // being true the moment a block or a catch opened one: the `var` then took the slot path and
    // became a binding of that block, so `{ let a = 1; var foo = 2; } foo` answered `undefined`.
    assert_eq!(run("{ let a = 1; var foo = 2; } foo"), "2");
    assert_eq!(run("{ let a = 1; var foo = 2; } globalThis.foo"), "2");
    assert_eq!(
        run("try { throw 1 } catch (e) { var foo = 'in catch'; } foo"),
        "in catch"
    );
    assert_eq!(run("for (let i = 0; i < 1; i++) { var foo = 3; } foo"), "3");
    // A `let` beside it still does not become a property, which is the half that was always right
    // and is what makes the two answers different at all.
    assert_eq!(
        run("{ let a = 1; var foo = 2; } typeof globalThis.a"),
        "undefined"
    );
}

#[test]
fn a_catch_parameter_is_a_binding_of_the_catch_and_of_nothing_around_it() {
    // §14.15.3 — an environment of its own, and only when there *is* a parameter: `catch { }`
    // evaluates its block in the scope around it.
    assert_eq!(
        run("var e = 'outer'; try { null.x } catch (e) {} e"),
        "outer"
    );
    assert_eq!(run("try { null.x } catch (e) {} typeof e"), "undefined");
    // A closure made in the catch keeps that catch's parameter, and two catches keep their own.
    assert_eq!(
        run("var f; try { throw 'caught' } catch (e) { f = function () { return e; }; } f()"),
        "caught"
    );
    assert_eq!(
        run(
            "var a, b; try { throw 1 } catch (e) { a = function () { return e; }; } \
             try { throw 2 } catch (e) { b = function () { return e; }; } a() + ',' + b()"
        ),
        "1,2"
    );
    // The exits out of it, which is where an environment usually goes wrong.
    assert_eq!(
        run("(function () { for (;;) { try { null.x } catch (e) { break; } } return 'ok'; })()"),
        "ok"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (;;) { try { null.x } catch (e) { break; } finally { r += 'f'; } } return r; })()"
        ),
        "f"
    );
    assert_eq!(
        run("(function () { try { null.x } catch (e) { return 'returned'; } })()"),
        "returned"
    );
    // §14.15.3's optional binding gets no environment at all, and a pattern declares every name in
    // it — both are the parameter question asked at its two edges.
    assert_eq!(run("try { null.x } catch { 'no param' }"), "no param");
    assert_eq!(run("try { throw { a: 5 } } catch ({ a }) { a }"), "5");
}

#[test]
fn eight_statement_forms_produce_undefined_where_three_produce_nothing_at_all() {
    // §14.2.2's `UpdateEmpty`, asked of a script's own completion value — which is what `run`
    // answers with, so no `eval` is needed to see it.
    //
    // The whole of what it turns on: a statement's value is EMPTY, or
    // it is a value. `undefined` is a *value* — so a statement producing one replaces whatever
    // came before it, and a statement producing EMPTY lets it show through. Eight forms begin
    // their evaluation with "Let V be undefined" and are therefore never empty.
    for (source, answer) in [
        // §14.6.2 — `UpdateEmpty(stmtCompletion, undefined)`, both with a branch and without one.
        ("1; if (true) ;", "undefined"),
        ("1; if (false) ;", "undefined"),
        ("1; if (false) 9; else ;", "undefined"),
        ("1; if (true) 9;", "9"),
        // §14.12.4's `CaseBlockEvaluation`.
        ("1; switch ('a') { case 'a': break; default: }", "undefined"),
        ("2; switch ('a') { case 'a': { 3; break; } default: }", "3"),
        ("1; switch ('z') { case 'a': 9; }", "undefined"),
        // The four iteration statements — §14.7.2.2, §14.7.3.2, §14.7.4.7 and §14.7.5.6.
        ("1; while (false) { }", "undefined"),
        ("2; while (false) { 3; }", "undefined"),
        ("1; do ; while (false)", "undefined"),
        ("1; for (;false;) ;", "undefined"),
        ("1; for (var k in {}) ;", "undefined"),
        ("1; for (var v of []) ;", "undefined"),
        ("1; for (var v of [7]) ;", "undefined"),
        ("1; for (var v of [7]) v;", "7"),
        // §14.15.3 — `try`, with each of its three shapes.
        ("1; try { } finally { }", "undefined"),
        ("1; try { } catch (e) { }", "undefined"),
        ("1; try { throw 0 } catch (e) { }", "undefined"),
        ("1; try { 9 } finally { }", "9"),
        // §14.11.2 — a `with`, which is its body's value.
        ("1; with ({}) ;", "undefined"),
        ("1; with ({}) 9;", "9"),
        // …and the three that are genuinely **EMPTY**, which is the half that would look identical
        // if the list above were "every statement". §14.2.2 gives an empty `Block` EMPTY, §14.4.1
        // an `EmptyStatement`, and §14.3.2.1 a `VariableStatement`.
        ("1; { }", "1"),
        ("1; ;", "1"),
        ("1; var x;", "1"),
        ("1; var x = 5;", "1"),
        ("1; debugger;", "1"),
        ("1; { var y; }", "1"),
        ("1; function f() { }", "1"),
        // A block is empty only when nothing in it produces a value.
        ("1; { 9; }", "9"),
    ] {
        assert_eq!(run(source), answer, "{source}");
    }
}

#[test]
fn a_label_passes_its_bodys_completion_through_rather_than_having_one() {
    // §14.13.4 evaluates the `LabelledItem` and hands its value on, so whether a label starts a
    // completion is a question about what is *under* it. A labelled `if` is never empty and a
    // labelled `var` always is — the row that says the test is on the body and not on the label.
    assert_eq!(run("1; L: if (true) ;"), "undefined");
    assert_eq!(run("1; L: var y;"), "1");
    assert_eq!(run("1; L: ;"), "1");
    assert_eq!(run("1; L: { }"), "1");
    assert_eq!(run("1; L: M: while (false) ;"), "undefined");
    assert_eq!(run("1; L: { 9; }"), "9");
    // §14.12.4 again, through a `break` that leaves the loop it names: the value is what the loop
    // had reached, and `undefined` when it had reached nothing.
    assert_eq!(
        run("4; do { switch ('a') { case 'a': continue; default: } } while (false)"),
        "undefined"
    );
    assert_eq!(
        run("5; do { switch ('a') { case 'a': { 6; continue; } default: } } while (false)"),
        "6"
    );
    // None of this exists inside a function, where §14.2.2's value is nobody's business but
    // `return`'s — so the same statements cost nothing there and answer nothing.
    assert_eq!(
        run("function f() { 1; if (true) ; } String(f())"),
        "undefined"
    );
}

#[test]
fn a_finally_that_falls_off_its_own_end_contributes_no_completion_value() {
    // §14.15.3 step 3 — "If F is a normal completion, set F to B". The `finally` block's own value
    // is thrown away when the block finishes ordinarily, so the statement answers with whatever the
    // `try` or the `catch` produced. This answered 3 here, because the finalizer was compiled as an
    // ordinary block and its statements overwrote the completion register.
    assert_eq!(run("1; try { 2 } finally { 3 }"), "2");
    assert_eq!(run("1; try { throw 0 } catch (e) { 2 } finally { 3 }"), "2");
    // …and step 4's `UpdateEmpty(F, undefined)`: an empty `try` leaves nothing for step 3 to hand
    // over, so the answer is `undefined` and *not* the finalizer's 3.
    assert_eq!(run("typeof eval('1; try { } finally { 3 }')"), "undefined");
    // A `finally` with nothing in it never had a value to contribute, which is the row that fails
    // if the fix is "always answer the try block" rather than "discard the finalizer's".
    assert_eq!(run("1; try { 2 } finally { }"), "2");
    // The value is still the *script's* only at the top level: inside a function nothing is kept
    // either way, and this is the row that catches a fix that turned tracking on where it was off.
    assert_eq!(
        run("(function () { try { return 2 } finally { 3 } })()"),
        "2"
    );
    // …and step 3's **other** half: when the finalizer leaves abruptly the value it produced is the
    // statement's, because `F` is not a normal completion and never becomes `B`. So the `try`'s 39
    // is discarded here rather than kept.
    assert_eq!(
        run("99; do { try { 39 } finally { 42; break } } while (false)"),
        "42"
    );
    assert_eq!(run("99; L: { try { 39 } finally { 42; break L } }"), "42");
    // …and step 4's `UpdateEmpty(F, undefined)` under it: a finalizer that leaves abruptly having
    // produced *nothing* answers `undefined`, which is the row that fails if the fix is "keep the
    // finalizer's value when it breaks" without starting that value empty.
    assert_eq!(
        run("typeof eval('99; do { try { 39 } finally { break } } while (false)')"),
        "undefined"
    );
    assert_eq!(
        run("typeof eval('99; do { 5; try { 39 } finally { break } } while (false)')"),
        "undefined"
    );
    // A `break` *after* the try statement is not a break out of the finalizer, so the ordinary
    // rule applies and the `try`'s value stands. This is the control for the four rows above.
    assert_eq!(
        run("99; do { try { 39 } finally { 42 }; break } while (false)"),
        "39"
    );

    // §14.15.3 leaves `return` inside a script's `finally` a Syntax Error, which is a *different*
    // question from the completion value and is answered by a different flag. Turning the wrong one
    // off makes this legal.
    assert_eq!(
        run(
            "(function () { try { eval('try { } finally { return 1 }'); return 'ran' } \
             catch (e) { return e.constructor.name } })()"
        ),
        "SyntaxError"
    );
}
