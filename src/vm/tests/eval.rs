//! §19.2.1's `eval` — both halves, and the call site that tells them apart.
//!
//! The rows worth reading twice are the ones where the *same text* answers differently through a
//! direct call and an indirect one. That difference is the whole of §19.2.1.1, and an engine could
//! pass every other row here with the two sharing one implementation.

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
fn source_that_is_not_a_script_is_a_syntax_error_wherever_viperjs_notices() {
    // §19.2.1.1 step 8. The interesting half is that ViperJS decides some early errors in the
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
    assert_eq!(run("eval('1 + 1')"), "2");
    // The half a program can actually observe: a direct eval reads the caller's scope and an
    // indirect one does not, from the same text at the same place.
    assert_eq!(
        run("(function () { let hidden = 9; return eval('hidden'); })()"),
        "9"
    );
    assert_eq!(
        run("(function () { let hidden = 9; return (0, eval)('typeof hidden'); })()"),
        "undefined"
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

#[test]
fn a_direct_eval_resolves_into_the_scopes_its_caller_is_running_in() {
    // §19.2.1.1 step 12 — the evaluated source's outer scope is the caller's *running* lexical
    // environment. ViperJS resolves a name to a depth and an index when it compiles, and this source
    // did not exist then; DR-0018's name list on each environment is what makes it reachable.
    assert_eq!(run("(function () { var a = 1; return eval('a'); })()"), "1");
    assert_eq!(
        run("(function () { var a = 1; eval('a = 2'); return a; })()"),
        "2"
    );
    // A parameter, and the `arguments` object — which §10.2.11 makes for every non-arrow function
    // and ViperJS skips when nothing read the name. A direct eval may read it and the compiler
    // cannot have seen that, so the call site asks for one.
    assert_eq!(run("(function (p) { return eval('p'); })('in')"), "in");
    assert_eq!(
        run("(function () { return eval('arguments[0]'); })('a0')"),
        "a0"
    );
    // Every kind of scope between the call and the script, since each is one hop of the chain and
    // a hop counted wrong reads a different variable rather than failing.
    assert_eq!(
        run("(function () { { let b = 7; return eval('b'); } })()"),
        "7"
    );
    assert_eq!(
        run(
            "(function () { for (let i = 0; i < 3; i++) { if (i === 2) { return eval('i'); } } })()"
        ),
        "2"
    );
    assert_eq!(
        run("(function () { try { throw 7; } catch (e) { return eval('e'); } })()"),
        "7"
    );
    assert_eq!(
        run("(function () { switch (1) { case 1: let q = 'Q'; return eval('q'); } })()"),
        "Q"
    );
    // The script's own environment is the end of every one of those chains.
    assert_eq!(
        run("let top = 3; (function () { return eval('top'); })()"),
        "3"
    );
    // A name that is nowhere is still a global read, and still throws when there is no global.
    assert_eq!(
        run("globalThis.gd = 4; (function () { return eval('gd'); })()"),
        "4"
    );
    assert_eq!(
        run("try { eval('no_such_name_at_all'); } catch (e) { e.constructor.name }"),
        "ReferenceError"
    );
}

#[test]
fn a_direct_eval_carries_the_mutability_of_what_it_resolved() {
    // §9.1.1.1.5 — whether a write is a TypeError is decided by the compiler that resolved the
    // name, so a compiler seeded from a running chain has to be told. Without it on the
    // environment this assigns, silently, and `k` is 2 afterwards.
    assert_eq!(
        run(
            "(function () { const k = 1; try { eval('k = 2'); return 'assigned'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run("(function () { const k = 1; try { eval('k = 2'); } catch (e) {} return k; })()"),
        "1"
    );
    // …and a `let` beside it is writable, so this is the mutability travelling and not a blanket
    // refusal to write anything an eval did not declare.
    assert_eq!(
        run("(function () { let m = 1; eval('m = 2'); return m; })()"),
        "2"
    );
}

#[test]
fn what_a_direct_eval_declares_goes_where_its_callers_variable_scope_is() {
    // §19.2.1.1 steps 12 to 14, which is three answers and not one.

    // At the top level of a script the variable scope is the global object, exactly as §16.1.7
    // puts a script's own `var` there — so this is the indirect mode's answer reached by the
    // direct path, and a function declaration goes the same way.
    assert_eq!(run("eval('var dv1 = 3'); globalThis.dv1"), "3");
    assert_eq!(run("eval('function dh() { return 4; }'); dh()"), "4");
    // …while the *lexical* scope is the eval's own and is discarded with it, which is what makes
    // the two halves of one statement disagree.
    assert_eq!(run("eval('let dl = 2; dl')"), "2");
    assert_eq!(run("eval('let dl = 2;'); typeof dl"), "undefined");
    // A `let` in the eval shadows a caller's binding for the eval's length and leaves it alone.
    assert_eq!(
        run("(function () { var a = 1; eval('let a = 2;'); return a; })()"),
        "1"
    );

    // Step 14 — **strict** eval's declarations are its own wherever it is written, so they are
    // slots in its own environment and go away with it.
    assert_eq!(
        run("(function () { 'use strict'; eval('var v = 1'); return typeof v; })()"),
        "undefined"
    );
    // …and it is a binding while the eval runs, so the declaration works and only its *lifetime*
    // is different. A refusal would pass the row above and fail this one.
    assert_eq!(
        run("(function () { 'use strict'; return eval('var v = 1; v'); })()"),
        "1"
    );
    // Strictness comes from either side: the caller's, or the evaluated text's own directive.
    assert_eq!(
        run("(function () { eval('\"use strict\"; var v = 1;'); return typeof v; })()"),
        "undefined"
    );

    // Step 16.b — the shape DR-0018 left open until the machine started tracking which level of
    // the chain is the variable one. The binding is appended to a scope that is already running,
    // and the name is the caller's from then on.
    assert_eq!(run("(function () { eval('var w = 1'); return w; })()"), "1");
    // It is the *function's* scope and not the eval's, so it outlives the eval and no more.
    assert_eq!(
        run("(function () { eval('var w = 1'); })(); typeof w"),
        "undefined"
    );
    // …and not the *block's*, which is the whole reason a variable environment is tracked
    // separately from the lexical one: a `{}` moves the second and leaves the first alone.
    assert_eq!(
        run("(function () { { eval('var w = 3'); } return w; })()"),
        "3"
    );
    // Step 16.b.ii — a name already bound there keeps its value. A `var` re-declaring something is
    // not an initialisation, so this is 1 rather than `undefined`.
    assert_eq!(
        run("(function () { var w = 1; eval('var w'); return w; })()"),
        "1"
    );
    // …and it is hoisted, so the assignment above the declaration in the evaluated text finds a
    // binding rather than making a global.
    assert_eq!(
        run("(function () { eval('w = 4; var w;'); return w; })()"),
        "4"
    );

    // Step 16.b.ii.1 — `CreateMutableBinding(vn, **true**)`. Every other creator of a declarative
    // binding passes `false`, so this is the only one in the language `delete` may take away, and
    // §9.1.1.1.7 answering `true` for it is the whole of the difference.
    assert_eq!(
        run("(function () { eval('var d = 1'); return delete d; })()"),
        "true"
    );
    // …and it is *gone*, not merely reported gone: a name that resolves nowhere is `undefined` to
    // `typeof`, and a second `var` of the same name makes a fresh binding rather than finding this
    // one still standing under a spelling nothing can reach.
    assert_eq!(
        run("(function () { eval('var d = 1'); delete d; return typeof d; })()"),
        "undefined"
    );
    assert_eq!(
        run("(function () { eval('var d = 1'); delete d; eval('var d = 2'); return d; })()"),
        "2"
    );
    // Every *other* declarative binding stays permanent, which is the rule this is the exception
    // to. A `var` the eval only re-declared was not created by step 16.b and keeps its own answer.
    assert_eq!(
        run("(function () { var x = 1; eval('var x'); return delete x; })()"),
        "false"
    );
    assert_eq!(
        run("(function () { let y = 1; return delete y; })()"),
        "false"
    );
    assert_eq!(run("(function (a) { return delete a; })(1)"), "false");
    // The slot cannot be given back — `declare_in` may only append precisely because every
    // `(depth, index)` already handed out goes on naming what it named — so a deletion unspells the
    // name *and* empties the slot. The second is what a reference compiled after the delete needs:
    // it reads nothing and raises a ReferenceError rather than the `undefined` the slot was still
    // holding. This is `var-env-var-init-local-new-delete.js`, whose two assertions are the two
    // halves of that.
    assert_eq!(
        run("(function () { var initial, after; \
             eval('initial = x; delete x; after = function () { x; }; var x;'); \
             try { after(); return initial + '/no throw'; } \
             catch (e) { return initial + '/' + e.constructor.name; } })()"),
        "undefined/ReferenceError"
    );
    // …and reading it *inside* the evaluated text after the delete is the same ReferenceError,
    // where `typeof` beside it is not. Both come from resolving by name once the delete has made a
    // slot the compiler placed unreachable by any other route.
    assert_eq!(
        run("(function () { var r; \
             eval('var x; delete x; try { r = x } catch (e) { r = e.constructor.name }'); \
             return r; })()"),
        "ReferenceError"
    );
    assert_eq!(
        run("(function () { var r; eval('var x; delete x; r = typeof x;'); return r; })()"),
        "undefined"
    );

    // Step 5.f — a binding the walk out to the variable environment passes *through* is a
    // SyntaxError, because one name would otherwise mean two bindings with no rule saying which a
    // reference takes. The block's `let` is such a level and the function's own `var` is not.
    assert_eq!(
        run(
            "(function () { { let y; try { eval('var y'); return 'ran'; } \
             catch (e) { return e.constructor.name; } } })()"
        ),
        "SyntaxError"
    );
    // …and so is a `let` at the **top level of the body**, which is the case the depth alone cannot
    // see: §10.2.11 step 30 gives a sloppy function's top-level lexical declarations an environment
    // of their own precisely so this walk crosses one, and ViperJS answers the same question from
    // `Declared` instead. A comparison that asked only "strictly inside" would let this through.
    assert_eq!(
        run(
            "(function () { let y; try { eval('var y'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );
    assert_eq!(
        run(
            "(function () { const y = 1; try { eval('var y'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );

    // The **block that opened an environment** is what says the depth is counted and not assumed.
    // A `var` declared from inside one belongs to the function all the same, so a walk that stopped
    // at the first level would put it in a scope that ends at the closing brace.
    assert_eq!(
        run("(function () { { let q; eval('var c = 3'); } return c; })()"),
        "3"
    );
    // Two new names in one eval, which is what says the second prediction follows the first: both
    // are appended, and a count that did not move would give them the same slot.
    assert_eq!(
        run("(function () { eval('var p = 1, r = 2'); return p + r; })()"),
        "3"
    );
    // A body whose **slots run past its names** — a closed block's are still taken, and
    // `Chunk::locals` is the high-water mark where `Chunk::bindings` is the top level alone. So the
    // appended binding goes after the slack rather than after the names, which is the one place the
    // compiler's prediction has to know both numbers. Predicted from the names alone, `z` is read
    // from a slot the block left behind and answers `undefined`.
    assert_eq!(
        run("(function () { { let a, b, c; } eval('var z = 7'); return z; })()"),
        "7"
    );

    // §10.2.11 step 20 — and a call written in a **formal parameter list** is refused by name,
    // because the clause hands it a variable environment outside the parameters and ViperJS puts
    // parameters and `var`s in one. Refusing is what keeps `f(arguments, p = eval('var arguments'))`
    // a SyntaxError; declaring would answer that program with silence.
    assert_eq!(
        run(
            "(function () { try { (function (a, p = eval('var a')) {})(); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );

    // Step 15 — a function declaration is var-scoped too and is *not* one of `VarDeclaredNames`:
    // the two static-semantics walks differ on exactly that production, which is why
    // `hoist_functions` is told the depth rather than being handed a list. It goes to the same
    // place a `var` does, and unlike a `var` it is **stored** rather than left alone.
    assert_eq!(
        run("(function () { eval('function w() { return 1; }'); return w(); })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var w = 1; eval('function w() { return 2; }'); return w(); })()"),
        "2"
    );
    // …and only the *top level* of the evaluated text. A declaration inside a block is §B.3.3.3's
    // and a lexical binding of that block, so it must not reach the caller — and must not overwrite
    // the eval's own `let` on the way, which is what leaving the depth set for every nested block
    // did.
    assert_eq!(
        run(
            "(function () { var a; eval('let f = 123; { function f() {} } a = f;'); return a; })()"
        ),
        "123"
    );

    // §15.2.5's binding of a function expression's own name sits in a `funcEnv` *outside* the
    // variable environment. So step 5.f's walk never reaches it — this is not the error — and step
    // 16.b.i finds no such binding in `varEnv` and makes a **new** one holding `undefined`, which
    // shadows the function's own name for the rest of the call. Both halves are the same fact about
    // where that binding lives, and getting either the other way round would be visible here.
    assert_eq!(
        run("(function g () { eval('var g'); return typeof g; })()"),
        "undefined"
    );
    // §B.3.5 — nor does a simple catch parameter, which the clause exempts by name.
    assert_eq!(
        run("(function () { try { throw 1 } catch (e) { eval('var e'); return e; } })()"),
        "1"
    );
    // …and neither does a parameter, which §10.2.11 keeps in an environment of its own.
    assert_eq!(run("(function (a) { eval('var a'); return a; })(3)"), "3");
    // The refusals are about the *declaration* and not about being inside a function: an eval that
    // declares nothing var-scoped is compiled there like anywhere else, which is most of them.
    assert_eq!(
        run("(function () { var a = 1; return eval('let b = 2; a + b'); })()"),
        "3"
    );
}

#[test]
fn a_direct_eval_shares_the_scope_it_read_rather_than_a_copy_of_it() {
    // The claim a snapshot would also satisfy every row above and fail here. A closure the eval
    // made reads the caller's binding *later*, so the two must be one variable.
    assert_eq!(
        run(
            "(function () { var a = 5; var f = eval('(function () { return a; })'); \
             a = 6; return f(); })()"
        ),
        "6"
    );
    // …and a closure the caller made sees what the eval wrote.
    assert_eq!(
        run(
            "(function () { var a = 5; var f = function () { return a; }; \
             eval('a = 7'); return f(); })()"
        ),
        "7"
    );
    // An eval inside an eval is direct too, and reaches all the way out.
    assert_eq!(
        run("(function () { var a = 1; return eval('eval(\"a\")'); })()"),
        "1"
    );
    // A function written *inside* the eval'd source has its own scope, and a direct eval in that
    // resolves through both.
    assert_eq!(
        run(
            "(function () { var a = 1; return eval('(function () { return eval(\"a\"); })()'); })()"
        ),
        "1"
    );
}

#[test]
fn a_direct_eval_is_script_code_however_deep_in_a_function_it_is_written() {
    // §19.2.1.1 evaluates a **Script**, so §14.2.2's completion value applies and `return` is a
    // Syntax Error. Both were wrong when one flag answered this question and where a `var` goes:
    // every direct eval inside a function evaluated to `undefined`.
    assert_eq!(run("(function () { return eval('1; ;'); })()"), "1");
    assert_eq!(run("(function () { return eval('2'); })()"), "2");
    // A declaration produces no value, so the completion value is `undefined` — asked of a strict
    // body, since a sloppy `var` in a function is the shape that is refused.
    assert_eq!(
        run("(function () { 'use strict'; return typeof eval('var q;'); })()"),
        "undefined"
    );
    assert_eq!(
        run(
            "(function () { try { eval('return 1'); } catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );
    // §19.2.1.1 leaves `this` alone — a direct eval sees the caller's, where an indirect one gets
    // the global object.
    assert_eq!(
        run("var o = { m: function () { return eval('this'); } }; o.m() === o"),
        "true"
    );
}

#[test]
fn a_direct_eval_is_strict_when_either_side_says_so_and_the_parser_is_the_one_told() {
    // §19.2.1.1 step 5. Strictness cannot be set on a finished tree: it decides §11.2.1's early
    // errors and it settles the `is_strict` of every function written inside the text, both of
    // which happen while the source is being read. So the caller's is handed to the parser.

    // §10.2.1.2 step 3 — a strict function's `undefined` receiver is not replaced by the global
    // object. The function is written *inside* the eval'd text and its own text says nothing about
    // strictness, so it can only be strict by inheriting the caller's.
    assert_eq!(
        run(
            "(function () { 'use strict'; return typeof eval('(function () { return this; })()'); })()"
        ),
        "undefined"
    );
    // …and from a sloppy caller the same text substitutes the global object, which is what makes
    // this a test of the inheritance rather than of strict mode.
    assert_eq!(
        run("(function () { return typeof eval('(function () { return this; })()'); })()"),
        "object"
    );
    // A `"use strict"` in the text alone does it too, from a sloppy caller.
    assert_eq!(
        run(
            "(function () { return typeof eval('\"use strict\"; (function () { return this; })()'); })()"
        ),
        "undefined"
    );

    // §11.2.1's early errors, which are the half a tree cannot be told about afterwards: both of
    // these parse in sloppy code and are Syntax Errors in strict code.
    assert_eq!(
        run(
            "(function () { 'use strict'; try { eval('with (Math) { PI }'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );
    assert_eq!(
        run(
            "(function () { 'use strict'; try { eval('var o = {}; delete o;'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );
    // …and an indirect eval is never made strict by its caller, however strict the caller is:
    // §19.2.1.1 asks about the call site and an indirect call has none to speak of.
    assert_eq!(
        run(
            "(function () { 'use strict'; return typeof (0, eval)('(function () { return this; })()'); })()"
        ),
        "object"
    );
}

#[test]
fn a_direct_eval_may_say_what_the_execution_around_it_may_say() {
    // §19.2.1.1 steps 3.b and 5.d to 5.f — three questions about the execution the call was made
    // from, each granting one construct to the evaluated text. The same characters are legal in one
    // place and a Syntax Error in another, which is not something the text can decide and is why
    // the parser has to be told rather than the tree corrected afterwards.

    // Step 5.e — `super.a` needs the running function to have a `[[HomeObject]]`.
    assert_eq!(
        run("class A { m() { return 'base'; } } \
             class B extends A { m() { return eval('super.m()'); } } new B().m()"),
        "base"
    );
    assert_eq!(
        run("var o = { m() { return eval('super.constructor === Object'); } }; o.m()"),
        "true"
    );
    // …including from an **arrow** written inside the method, which has no home object of its own
    // and is given the enclosing one when it is made.
    assert_eq!(
        run("class A { m() { return 9; } } \
             class B extends A { m() { var f = () => eval('super.m()'); return f(); } } new B().m()"),
        "9"
    );
    // A plain function is not a method however it is called, so the same text is refused there —
    // §15.2.1 makes `super` in a `FunctionBody` a Syntax Error outright.
    assert_eq!(
        run(
            "var f = function () { try { eval('super.x'); return 'ran'; } \
             catch (e) { return e.constructor.name; } }; \
             var o = { m: f }; o.m()"
        ),
        "SyntaxError"
    );
    // …and at the top of a script there is no execution to inherit from at all.
    assert_eq!(
        run("try { eval('super.x'); 'ran' } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );

    // Step 5.d — `new.target` needs there to be a function around the call.
    assert_eq!(
        run("(function () { return eval('typeof new.target'); })()"),
        "undefined"
    );
    // Read through a global rather than returned: §10.2.2 discards a primitive a constructor
    // returns, so `new C()` would answer the object however true the comparison was.
    assert_eq!(
        run("function C() { globalThis.saw = eval('new.target === C'); } new C(); globalThis.saw"),
        "true"
    );
    assert_eq!(
        run("try { eval('new.target'); 'ran' } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );

    // Step 5.f — `super(…)` needs a derived constructor. ViperJS grants it to the parser and the
    // *compiler* then says it has not built it, which is the ordinary division here: the grammar
    // admits the construct and the refusal names what is missing. Before this it was refused as
    // "`super` is only allowed inside a method", which diagnosed the wrong thing.
    assert_eq!(
        run("class A { constructor() { this.v = 1; } } \
             class B extends A { constructor() { \
                 try { eval('super()'); globalThis.said = 'ran'; } \
                 catch (e) { globalThis.said = e.constructor.name; } \
                 super(); } } \
             new B(); globalThis.said"),
        "SyntaxError"
    );
}

#[test]
fn a_direct_eval_at_the_top_of_a_nested_script_is_not_inside_the_function_that_started_it() {
    // §19.2.1.1 — an *indirect* eval evaluates Script code in the global environment, so a direct
    // eval written at the top of it is not in a function however deep the call that reached it. The
    // enclosing function's frame is below the nested execution's floor, and that is exactly what
    // the floor is for: reading it as "the running frame" would let `new.target` through where
    // §13.3.12 makes it a Syntax Error.
    assert_eq!(
        run("function outer() { \
               return (0, eval)(\"try { eval('new.target'); 'allowed' } \
                                 catch (e) { e.constructor.name }\"); \
             } outer()"),
        "SyntaxError"
    );
    // …and the same eval written directly in `outer` *is* in a function, which is what makes the
    // row above about the floor rather than about `new.target` being refused everywhere.
    assert_eq!(
        run("function outer() { return typeof eval('new.target'); } outer()"),
        "undefined"
    );
}

#[test]
fn an_evals_global_bindings_may_be_deleted_and_a_scripts_may_not() {
    // §9.1.1.4.17 `CreateGlobalVarBinding(N, D)` has one parameter and its two callers disagree
    // about it: §16.1.7 step 18 passes `false` for a Script and §19.2.1.1 step 8 passes `true` for
    // an `eval`. So the same three characters make a permanent property in one place and a
    // removable one in the other, and `delete` is where a program sees it.
    assert_eq!(
        run("eval('var x = 1'); Object.getOwnPropertyDescriptor(globalThis, 'x').configurable"),
        "true"
    );
    assert_eq!(
        run("var y = 1; Object.getOwnPropertyDescriptor(globalThis, 'y').configurable"),
        "false"
    );
    // …and the other two attributes are the same on both sides, which is what says this is one
    // parameter of one operation rather than two ways of making a property.
    assert_eq!(
        run(
            "eval('var x = 1'); var d = Object.getOwnPropertyDescriptor(globalThis, 'x'); \
             [d.writable, d.enumerable].join(',')"
        ),
        "true,true"
    );
    // The deletion itself, which is the point of the attribute.
    assert_eq!(run("eval('var x = 1'); delete x; typeof x"), "undefined");
    assert_eq!(run("eval('var x = 1'); delete x"), "true");
    assert_eq!(run("var y = 1; delete y; typeof y"), "number");
    assert_eq!(run("var y = 1; delete y"), "false");
    // §19.2.1.1 does not ask which kind of call it was, so an indirect `eval` answers alike. Both
    // halves are asserted because they reach the compiler by different entry points.
    assert_eq!(
        run(
            "(0, eval)('var x = 1'); Object.getOwnPropertyDescriptor(globalThis, 'x').configurable"
        ),
        "true"
    );
    assert_eq!(
        run("(0, eval)('var x = 1'); delete x; typeof x"),
        "undefined"
    );
    // A function declaration goes through the same operation here, and so does §B.3.3's binding
    // for a block-level one — an `eval` may not fix either on the global object for good.
    assert_eq!(
        run(
            "eval('function f() {}'); Object.getOwnPropertyDescriptor(globalThis, 'f').configurable"
        ),
        "true"
    );
    assert_eq!(
        run("eval('{ function f() {} }'); \
             Object.getOwnPropertyDescriptor(globalThis, 'f').configurable"),
        "true"
    );
    assert_eq!(
        run("{ function g() {} } Object.getOwnPropertyDescriptor(globalThis, 'g').configurable"),
        "false"
    );
    // A name the global object already has keeps the attributes it had, whichever side declares
    // it — `CreateGlobalVarBinding` leaves an existing property alone, so `D` never reaches it.
    assert_eq!(
        run("var y = 1; eval('var y = 2'); \
             Object.getOwnPropertyDescriptor(globalThis, 'y').configurable + ',' + y"),
        "false,2"
    );
}

#[test]
fn an_arrow_is_transparent_to_new_target_inside_a_direct_eval() {
    // §15.3 gives an arrow no `[[NewTarget]]` of its own, so `new.target` inside one reads the
    // function around it — exactly as `this` does. A **direct `eval`** has to decide whether to
    // *parse* it at all, and could only ask what it was running inside: the function object says
    // that it is an arrow and not where it was written.
    //
    // So the fact travels on the chunk. An arrow written inside a function inherits it; one written
    // at a script's top level does not.
    assert_eq!(
        run("function F() { return (() => eval('new.target'))(); } (new F()).name"),
        "F"
    );
    // Nested arrows are the same answer as many times as it takes.
    assert_eq!(
        run("function F() { return (() => (() => eval('new.target'))())(); } (new F()).name"),
        "F"
    );
    // A plain call has a `new.target` of `undefined`, which is a *value* and not a refusal — the
    // syntax is allowed either way and only the answer differs.
    assert_eq!(
        run("function F() { return (() => eval('typeof new.target'))(); } F()"),
        "undefined"
    );
    // The two halves that already worked, which is what makes this about the arrow.
    assert_eq!(
        run("function F() { return eval('new.target'); } (new F()).name"),
        "F"
    );
    assert_eq!(
        run("function F() { return (() => new.target)(); } (new F()).name"),
        "F"
    );
    // And a script's top level still refuses, arrow or not: there is no function being constructed
    // and nothing for `new.target` to mean.
    assert_eq!(
        run("try { (0, eval)('new.target'); 'accepted' } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run(
            "try { (0, eval)('(() => new.target)()'); 'accepted' } catch (e) { e.constructor.name }"
        ),
        "SyntaxError"
    );
}

#[test]
fn a_direct_eval_in_a_static_field_initialiser_may_not_read_arguments() {
    // §15.7.1 — `arguments` is forbidden in a class field's initialiser, and the parser refuses
    // every spelling it can see where the class is written. A **direct `eval`** is the one it
    // cannot: the text is parsed when the field runs, so the compiled body has to carry the
    // position to it.
    //
    // A **SyntaxError**, and therefore before the text runs at all — which is what distinguishes
    // this from a ReferenceError raised while evaluating: the assignment ahead of the read must not
    // happen. `evaluated` is what says so.
    assert_eq!(
        run("var evaluated = false;\
             try { class C { static x = eval('evaluated = true; arguments;'); } }\
             catch (e) { e.name + ':' + evaluated }"),
        "SyntaxError:false"
    );
    // `Contains` stops at a function boundary and not at an arrow, so the two nest differently.
    assert_eq!(
        run("try { class C { static x = eval('() => arguments'); } } catch (e) { e.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run("class C { static x = eval('(function () { return arguments })'); } typeof C.x"),
        "function"
    );
    // …and the same text outside a field is an ordinary read of the enclosing `arguments`.
    assert_eq!(
        run("function f() { return eval('arguments').length } f(1, 2)"),
        "2"
    );
}
