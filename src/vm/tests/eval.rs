//! §19.2.1's `eval` — the indirect half, and the call site that tells the two apart.

use super::*;

#[test]
fn an_indirect_eval_runs_its_source_and_answers_the_completion_value() {
    // §19.2.1.1 — reached by anything that is not the bare name `eval`, and the four spellings
    // that matter are each a row: the comma trick, a variable it was assigned to, a property of
    // the global object, and a call through `Function.prototype.call`.
    assert_eq!(run("(0, eval)('1 + 1')"), "2");
    assert_eq!(run("var e = eval; e('2 * 3')"), "6");
    assert_eq!(run("globalThis.eval('4 - 1')"), "3");
    assert_eq!(run("eval.call(null, '5 + 5')"), "10");
    // §14.2.2's completion value, which is not the last *statement* but the last one that produced
    // a value — so a declaration answers `undefined` and an expression before it does not.
    assert_eq!(run("typeof (0, eval)('var q = 1;')"), "undefined");
    assert_eq!(run("(0, eval)('7; var q = 1;')"), "7");
    assert_eq!(run("typeof (0, eval)('')"), "undefined");
}

#[test]
fn eval_answers_anything_that_is_not_a_string_unchanged() {
    // §19.2.1.1 step 2, and it is *not* a coercion. A `String` object is the case that shows the
    // difference: it has a `toString` that would give perfectly good source, and eval does not ask.
    assert_eq!(run("(0, eval)(42)"), "42");
    assert_eq!(run("typeof (0, eval)(undefined)"), "undefined");
    assert_eq!(run("typeof (0, eval)(new String('1 + 1'))"), "object");
    assert_eq!(run("(0, eval)(null)"), "null");
    // …and an object with a `toString` is not run either, which is the whole safety of the rule.
    assert_eq!(
        run("typeof (0, eval)({ toString: function () { return '1 + 1'; } })"),
        "object"
    );
}

#[test]
fn source_that_is_not_a_script_is_a_syntax_error_wherever_praxis_notices() {
    // §19.2.1.1 step 8. The interesting half is that praxis decides some early errors in the
    // *compiler* rather than the parser — §22.2.1's regular-expression ones — so both refusals have
    // to arrive as the same error. A program can otherwise tell where the check happens to live.
    assert_eq!(
        run("try { (0, eval)('var ='); } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run("try { (0, eval)('('); } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run("try { (0, eval)('/(?<a>x)(?<a>y)/'); } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    // A *runtime* throw is not a SyntaxError and travels out as the value it was.
    assert_eq!(
        run("try { (0, eval)('throw new RangeError(\"x\")'); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { (0, eval)('undeclared_name_xyz'); } catch (e) { e.constructor.name }"),
        "ReferenceError"
    );
}

#[test]
fn an_indirect_eval_declares_vars_globally_and_keeps_its_lets_to_itself() {
    // §19.2.1.1's indirect mode: the *variable* scope is the global one and the *lexical* scope is
    // a fresh declarative one. §16.1.7 already splits a script's top-level declarations that way,
    // which is why this needs no special case — and it is exactly what a program can observe.
    assert_eq!(run("(0, eval)('var ev1 = 7;'); globalThis.ev1"), "7");
    assert_eq!(
        run("(0, eval)('let ev2 = 8;'); typeof globalThis.ev2"),
        "undefined"
    );
    // A function declaration goes the same way a `var` does.
    assert_eq!(run("(0, eval)('function ev3() { return 3; }'); ev3()"), "3");
    // It reads globals, and it does **not** read the caller's lexical scope — that is the whole
    // difference from a direct eval, and the reason the two cannot share an implementation.
    assert_eq!(run("globalThis.gv = 5; (0, eval)('gv + 1')"), "6");
    assert_eq!(
        run("(function () { let hidden = 9; return (0, eval)('typeof hidden'); })()"),
        "undefined"
    );
    // An eval inside an eval is still indirect, and still sees the global scope.
    assert_eq!(run("(0, eval)(\"(0, eval)('3 + 4')\")"), "7");
}

#[test]
fn a_call_is_a_direct_eval_only_when_the_name_eval_holds_eval_itself() {
    // §13.3.6.1 — two halves, and each on its own is not enough. The compiler sees the *spelling*
    // and the interpreter sees the *identity*, and a call is direct only when both agree.
    //
    // Direct eval needs the caller's scope, which praxis does not have yet, so it is refused by
    // name. Running it in the global scope instead would be indirect eval wearing its clothes —
    // right until the source mentions something the caller declared, and then quietly wrong.
    assert_eq!(
        run("try { eval('1 + 1'); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Spelled `eval` but holding something else: an ordinary call, and it must not be refused.
    assert_eq!(
        run(
            "(function () { var eval = function (s) { return 'shadow:' + s; }; return eval('hi'); })()"
        ),
        "shadow:hi"
    );
    assert_eq!(
        run("(function (eval) { return eval('hi'); })(function (s) { return 'param:' + s; })"),
        "param:hi"
    );
    // …and holding `eval` itself under another name is *indirect*, so it runs rather than refusing.
    assert_eq!(run("var notEval = eval; notEval('1 + 1')"), "2");
    // A property access is never the bare name, however it is spelled.
    assert_eq!(run("globalThis['eval']('1 + 1')"), "2");
}

#[test]
fn eval_is_an_ordinary_function_of_the_global_object() {
    // §19.2.1's shape, which a program checks before it uses it — and which is what switches on
    // whole swathes of test262's own harness.
    assert_eq!(run("typeof eval"), "function");
    assert_eq!(run("eval.length"), "1");
    assert_eq!(run("eval.name"), "eval");
    // Not a constructor: §19.2.1 gives it no `[[Construct]]`, so `new eval()` is a TypeError.
    assert_eq!(
        run("try { new eval('1'); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn an_eval_leaves_the_stack_and_the_scope_it_interrupted_exactly_as_they_were() {
    // A script running in the middle of an expression, which is what makes this different from
    // `Vm::run`. Three things have to come back: the operand stack (the addition below still has
    // its left operand waiting), the caller's environment, and its completion value.
    assert_eq!(run("1 + (0, eval)('2') + 3"), "6");
    assert_eq!(
        run("(function () { let a = 1; let b = (0, eval)('2'); return a + b; })()"),
        "3"
    );
    // A throw out of the eval must unwind to the caller's handler and no further, leaving the
    // caller able to carry on — the case where a half-built expression is abandoned mid-stack.
    assert_eq!(
        run("var n = 0; try { n = 1 + (0, eval)('throw 5'); } catch (e) { n = e; } n"),
        "5"
    );
    // …and the script's own completion value is not disturbed by the eval's.
    assert_eq!(run("9; (0, eval)('1'); 8"), "8");
}

#[test]
fn an_eval_inside_an_eval_is_a_rust_frame_and_is_counted_like_one() {
    // A script running here is a real Rust call into the interpreter, exactly as a coercion's
    // re-entry is — so `eval("eval(…)")` nests the host's stack and is bounded by the same counter
    // rather than by `MAX_CALL_DEPTH`. Without the bound this is a stack overflow, which DR-0002
    // says no `Result` can rescue.
    //
    // The *depth* is asserted and not merely the error, because the two halves of the guard fail
    // differently: a cap that never fires is a crash, and one that fires a level late is a guard
    // whose boundary has quietly moved. 33 is `MAX_REENTRY_DEPTH` levels of eval plus the call
    // that started them.
    assert_eq!(
        run("globalThis.n = 0; \
             globalThis.go = function () { n++; return (0, eval)('go()'); }; \
             var caught = 'none'; \
             try { go(); } catch (e) { caught = e.constructor.name; } \
             caught + ':' + n"),
        "RangeError:33"
    );
}

#[test]
fn evals_that_have_finished_do_not_count_against_the_next_one() {
    // The other half of the counter, and the half that is invisible until it is wrong: a re-entry
    // that was never given back would make the *forty-first* sequential `eval` in a program fail
    // for depth it is not using. Sequential and nested are the two shapes, and only one of them is
    // supposed to be bounded.
    assert_eq!(
        run("var total = 0; for (var i = 0; i < 40; i++) { total += (0, eval)('1'); } total"),
        "40"
    );
    // …and the counter comes back after a *throw* too, which is the path that does not run the
    // ordinary way out. Forty evals that each threw must leave room for a forty-first that does not.
    assert_eq!(
        run(
            "for (var i = 0; i < 40; i++) { try { (0, eval)('throw 1'); } catch (e) {} } \
             (0, eval)('2 + 2')"
        ),
        "4"
    );
}
