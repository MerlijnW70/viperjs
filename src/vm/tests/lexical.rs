//! §14.3.1 — `let` and `const`, and the dead zone that is the whole difference from `var`.
//!
//! Every row here was checked against V8 before it was written down. That matters more than usual
//! in this file: the temporal dead zone is a rule about *when* a binding exists, and an engine
//! that got it slightly wrong would still run almost every program correctly.

use super::*;

#[test]
fn a_lexical_binding_holds_its_value_and_a_const_refuses_to_be_moved() {
    assert_eq!(run("let a = 1; a"), "1");
    assert_eq!(run("const b = 2; b"), "2");
    assert_eq!(run("let a = 1; a = 3; a"), "3");
    // §14.3.1's `let` without an initializer is `undefined` — which is a *value*, and so is the
    // end of the dead zone rather than a continuation of it.
    assert_eq!(run("let a; typeof a"), "undefined");
    // §9.1.1.1.5 step 3 — a `const` is immutable, and the assignment is a TypeError however it is
    // spelled. Not a SyntaxError: the right-hand side runs first.
    assert_eq!(
        run("const c = 1; try { c = 2; } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var ran = false; const c = 1; try { c = (ran = true); } catch (e) { ran }"),
        "true"
    );
    // …while what a `const` *refers to* is not frozen. The binding cannot move; the object can.
    assert_eq!(run("const o = {v: 1}; o.v = 2; o.v"), "2");
    assert_eq!(run("const arr = [1, 2, 3]; arr[0]"), "1");
}

#[test]
fn reading_a_lexical_binding_above_its_declaration_is_a_reference_error() {
    // §9.1.1.1.6 step 3, and the reason `let` is not `var` with a nicer scope. The binding exists
    // from the top of the block — it is not "not there" — and reading it is an error until its
    // declaration has run.
    assert_eq!(
        run("try { x; let x = 1; } catch (e) { e.name }"),
        "ReferenceError"
    );
    assert_eq!(
        run("try { let x = x; } catch (e) { e.name }"),
        "ReferenceError"
    );
    // Writing to one is the same error, by §9.1.1.1.5 step 2. An assignment is not a way to
    // initialise a binding early — only its declaration is.
    assert_eq!(
        run("try { x = 1; let x; } catch (e) { e.name }"),
        "ReferenceError"
    );
    // `typeof` does not save it. §13.5.1.1 spares an *unresolvable* reference, and a binding in
    // its dead zone is perfectly resolvable — which is the one place `typeof x` can throw.
    assert_eq!(
        run("try { typeof x; let x; } catch (e) { e.name }"),
        "ReferenceError"
    );
    // …while a name that is nowhere at all still answers, so the two cases stay distinct.
    assert_eq!(run("typeof nothing_at_all"), "undefined");
    // A `var` in the same position reads as `undefined`, which is what the dead zone replaced.
    assert_eq!(run("typeof v; var v = 1;"), "undefined");
}

#[test]
fn a_block_is_a_scope_and_its_bindings_do_not_outlive_it() {
    assert_eq!(run("{ let x = 1; x }"), "1");
    assert_eq!(run("{ let x = 1; } typeof x"), "undefined");
    // Shadowing, in both directions: the inner binding hides the outer for the block's length and
    // the outer is untouched afterwards.
    assert_eq!(run("var x = 'outer'; { let x = 'inner'; x }"), "inner");
    assert_eq!(run("var x = 'outer'; { let x = 'inner'; } x"), "outer");
    assert_eq!(run("let a = 1; { let a = 2; a }"), "2");
    assert_eq!(run("let a = 1; { let a = 2; } a"), "1");
    assert_eq!(
        run("var out = ''; { let p = 'a'; { let p = 'b'; out = out + p; } out = out + p; } out"),
        "ba"
    );
    // A block with no binding of its own still reaches the one outside it.
    assert_eq!(run("let n = 0; { n = 5; } n"), "5");
    // §14.15 — a `try`, a `catch` and a `finally` body are Blocks too, and each is a scope. This
    // is the row that caught them not being: `x = 1` above a `let x` in the same block has to find
    // that binding in its dead zone rather than quietly making a global.
    assert_eq!(run("try { let t = 1; } catch (e) {} typeof t"), "undefined");
    assert_eq!(
        run("try { throw 1; } catch (e) { let t = 2; } typeof t"),
        "undefined"
    );
    assert_eq!(run("try { } finally { let t = 3; } typeof t"), "undefined");
}

#[test]
fn two_blocks_side_by_side_do_not_share_a_binding() {
    // The bug this exists to stop, and it is invisible without closures. If a block's slots were
    // given back when it ended, the next block would be handed the same ones — and a function
    // made in the first block would then read the second block's variable. Both closures outlive
    // both blocks, so nothing about the value they answer with is a coincidence.
    assert_eq!(
        run(
            "var fs = []; { let x = 1; fs.push(function () { return x; }); } \
             { let y = 2; fs.push(function () { return y; }); } fs[0]() + ',' + fs[1]()"
        ),
        "1,2"
    );
    // The same through arrows, which capture by the same route.
    assert_eq!(
        run(
            "var fs = []; { let x = 'a'; fs.push(() => x); } { let y = 'b'; fs.push(() => y); } \
             fs[0]() + fs[1]()"
        ),
        "ab"
    );
    // …and a closure over a block binding still reads the *current* value, not a copy taken when
    // it was made.
    assert_eq!(run("var f; { let x = 1; f = () => x; x = 2; } f()"), "2");
}

#[test]
fn a_for_head_binding_belongs_to_the_loop_and_not_to_what_is_around_it() {
    assert_eq!(
        run("var s = 0; for (let i = 0; i < 4; i = i + 1) { s = s + i; } s"),
        "6"
    );
    assert_eq!(
        run("for (let i = 0; i < 3; i = i + 1) {} typeof i"),
        "undefined"
    );
    // A binding in the *body* is fresh enough to be re-declared on each pass — the dead zone is
    // re-entered, so this is not one binding being read twice.
    assert_eq!(
        run("var s = ''; for (let i = 0; i < 2; i = i + 1) { let j = i * 2; s = s + j; } s"),
        "02"
    );
    assert_eq!(
        run(
            "var s = 0; for (let i = 0; i < 3; i = i + 1) { for (let j = 0; j < 3; j = j + 1) { s = s + 1; } } s"
        ),
        "9"
    );
    // The outer name is untouched by a loop that shadows it.
    assert_eq!(
        run("let i = 'kept'; for (let i = 0; i < 2; i = i + 1) {} i"),
        "kept"
    );
}

#[test]
fn a_switch_is_one_scope_over_all_of_its_cases() {
    // §14.12.4 — the `CaseBlock` is a single scope, not one per case, so a `let` in one case is
    // in scope in the others and its dead zone begins at the top of the whole block.
    assert_eq!(run("switch (1) { case 1: let y = 5; y }"), "5");
    assert_eq!(
        run("var r = ''; switch (2) { case 1: r = 'a'; break; case 2: let z = 'b'; r = z; } r"),
        "b"
    );
    // Reached from an *earlier* case, the binding is there and is not initialised — which is a
    // ReferenceError and not `undefined`.
    assert_eq!(
        run("try { switch (1) { case 1: y; break; case 2: let y = 1; } } catch (e) { e.name }"),
        "ReferenceError"
    );
    assert_eq!(
        run("switch (1) { case 1: let w = 1; } typeof w"),
        "undefined"
    );
}

#[test]
fn a_function_sees_the_lexical_bindings_it_was_written_inside() {
    assert_eq!(run("let q = 1; function g() { return q; } g()"), "1");
    assert_eq!(
        run("function h() { let w = 1; return function () { return w; }; } h()()"),
        "1"
    );
    // A function body is a scope of its own, on the same terms as a block.
    assert_eq!(
        run("function f() { let v = 1; { let v = 2; } return v; } f()"),
        "1"
    );
    assert_eq!(
        run("function f() { let v = 1; { let v = 2; return v; } } f()"),
        "2"
    );
    // …and the dead zone reaches into a function called too early, because it is about *when*
    // rather than about where.
    assert_eq!(
        run(
            "function early() { return later; } try { early(); let later = 1; } catch (e) { e.name }"
        ),
        "ReferenceError"
    );
}

#[test]
fn a_closure_over_a_binding_a_loop_re_creates_is_refused_rather_than_got_wrong() {
    // §14.7.4.7 `CreatePerIterationEnvironment` — each pass of a loop gets a *fresh* binding, so
    // three closures made in three passes answer with three different values. praxis gives a
    // lexical declaration one slot for the whole call, which is right for a block entered once
    // and wrong for one entered again; the two differ only when a closure escapes the pass that
    // made it.
    //
    // So it is refused. The alternative is every closure in the loop sharing one variable and all
    // of them answering the last value, which is a wrong answer that runs.
    for source in [
        "for (let i = 0; i < 1; i++) { (function () { return i; }); }",
        "for (let i = 0; i < 1; i++) { (() => i); }",
        "for (let i = 0; i < 1; i++) { let j = i; (() => j); }",
        "while (true) { let x = 1; (() => x); break; }",
        "do { let x = 1; (() => x); } while (false);",
    ] {
        let error = crate::compile::compile_script(
            &parse_script(source).expect("the source parses"), // the test is about the refusal
            &mut Heap::new(),
        )
        .expect_err("refused"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported(
                "a function that closes over a `let` or `const` declared in a loop"
            ),
            "compiling {source:?}"
        );
    }
    // What is *not* refused: a `var` in a loop, which is one binding by design and whose closures
    // are supposed to share it…
    assert_eq!(
        run("var fs = []; for (var i = 0; i < 3; i = i + 1) { fs.push(() => i); } fs[0]()"),
        "3"
    );
    // …a function in a loop that closes over nothing the loop declares…
    assert_eq!(
        run("var f; for (var i = 0; i < 1; i = i + 1) { f = () => 'fixed'; } f()"),
        "fixed"
    );
    // …and a function written after the loop has ended, which no longer has the binding in scope.
    assert_eq!(
        run("for (let i = 0; i < 1; i = i + 1) { } var f = () => 1; f()"),
        "1"
    );
    // Both halves of "live *and* lexical", because either alone would refuse a program that is
    // perfectly well defined.
    //
    // A binding declared in the loop that is no longer in scope: the inner block has ended, so
    // nothing the function could write down still refers to it, and re-creating it on the next
    // pass changes nothing anybody can see.
    //
    // A `for (let …)` head is live for the whole body, so the loop below is a `while`: the only
    // lexical binding it declares has already gone out of scope where the function is written.
    assert_eq!(
        run("var f; var n = 0; while (n < 1) { { let gone = 1; } f = () => 'made'; n = 1; } f()"),
        "made"
    );
    // A binding declared in the loop that is *not* lexical: a catch parameter is block-scoped but
    // it is not re-created by the loop in a way a closure can tell apart, and §14.7.4.7 is about
    // `let` and `const`.
    assert_eq!(
        run(
            "var f; for (var i = 0; i < 1; i = i + 1) { try { throw 7; } catch (e) { f = () => e; } } f()"
        ),
        "7"
    );
}
