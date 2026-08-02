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
    // environment. praxis resolves a name to a depth and an index when it compiles, and this source
    // did not exist then; DR-0018's name list on each environment is what makes it reachable.
    assert_eq!(run("(function () { var a = 1; return eval('a'); })()"), "1");
    assert_eq!(
        run("(function () { var a = 1; eval('a = 2'); return a; })()"),
        "2"
    );
    // A parameter, and the `arguments` object — which §10.2.11 makes for every non-arrow function
    // and praxis skips when nothing read the name. A direct eval may read it and the compiler
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

    // And the one shape DR-0018 leaves open: a sloppy `var` inside a function would have to grow
    // an environment whose slot count was fixed when that function was compiled. Refused by name
    // rather than put somewhere else, because somewhere else is a wrong answer that runs.
    assert_eq!(
        run("(function () { try { eval('var w = 1'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"),
        "SyntaxError"
    );
    assert_eq!(
        run(
            "(function () { try { eval('function w() {}'); return 'ran'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "SyntaxError"
    );
    // The refusal is about the *declaration* and not about being inside a function: an eval that
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

    // Step 5.f — `super(…)` needs a derived constructor. praxis grants it to the parser and the
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
