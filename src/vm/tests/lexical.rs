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
fn a_for_of_head_gives_every_pass_its_own_binding() {
    // §14.7.5.7 step 3.g — `NewDeclarativeEnvironment` per pass, so three closures made walking
    // three values answer with three values rather than three copies of the last.
    assert_eq!(
        run(
            "var f = []; for (const x of [1, 2, 3]) { f.push(function () { return x; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "1,2,3"
    );
    // A `continue` is the exit that decides where the environment sits in the unwind order. It
    // leaves the pass — so its `PopScope` must be emitted — and it must **not** close the
    // iterator. The two are recorded next to each other and `unwind_across` stops at the first
    // thing a jump does not cross, so with them the wrong way round a `continue` stops at the
    // iterator and never leaves the environment at all, deepening the chain once per pass.
    assert_eq!(
        run(
            "var f = []; for (const x of [1, 2, 3]) { if (x === 2) { continue; } f.push(function () { return x; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "1,3"
    );
    assert_eq!(
        run(
            "var f = []; for (const x of [1, 2, 3]) { if (x === 3) { break; } f.push(function () { return x; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "1,2"
    );
    assert_eq!(
        run(
            "function g() { for (const x of [1, 2, 3]) { if (x === 2) { return 'r' + x; } } return 'no'; } g()"
        ),
        "r2"
    );
    // `for`-`in` is the same clause and closes nothing, which makes it the simpler half.
    assert_eq!(
        run(
            "var f = []; for (const k in { a: 1, b: 2 }) { f.push(function () { return k; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "a,b"
    );
    // …and a `var` head is **not** the loop's — §14.7.5.5 gives it no per-iteration binding, so
    // its closures still share one. The pair is the claim.
    assert_eq!(
        run(
            "var f = []; for (var v of [1, 2, 3]) { f.push(function () { return v; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "3,3,3"
    );
}

#[test]
fn a_block_entered_twice_makes_its_lexical_bindings_twice() {
    // §14.2.2 — a block's Declarative Environment Record is made when the block is *entered*, so a
    // loop body entered three times has made three of them and the closures made in each hold
    // three different bindings. This is the one thing everybody knows ES2015 changed, and until
    // this slice praxis refused it rather than answering `2,2,2`.
    assert_eq!(
        run(
            "var f = []; for (var i = 0; i < 3; i++) { let x = i; f.push(function () { return x; }); }              f[0]() + ',' + f[1]() + ',' + f[2]()"
        ),
        "0,1,2"
    );
    // A `while` is the same statement for this purpose — the block is what makes the binding, not
    // the loop — and it is worth testing separately because only `for` has a head to confuse it
    // with.
    assert_eq!(
        run(
            "var f = []; var n = 0; while (n < 3) { let x = n; f.push(function () { return x; }); n++; }              f[0]() + ',' + f[1]() + ',' + f[2]()"
        ),
        "0,1,2"
    );
    // …and a `var` still shares one binding, which is what it is for. Written beside the above
    // because the interesting claim is that the two now differ.
    assert_eq!(
        run(
            "var f = []; for (var i = 0; i < 3; i++) { var x = i; f.push(function () { return x; }); }              f[0]() + ',' + f[1]() + ',' + f[2]()"
        ),
        "2,2,2"
    );
}

#[test]
fn a_jump_or_a_throw_out_of_a_block_leaves_its_environment_behind() {
    // The half of block scoping that is not about closures at all. Leaving the block by running
    // off the end emits a `PopScope`; leaving it by `break`, `continue` or `return` has to emit one
    // too, and leaving it by a *throw* must not — the handler recorded the environment it was
    // installed in. Get any of the three wrong and the code after the block reads its variables one
    // hop too shallow, which finds a *different variable* rather than failing.
    assert_eq!(
        run(
            "var f = []; for (var i = 0; i < 3; i++) { let x = i; if (i === 1) { break; } f.push(x); } f.length + ':' + i"
        ),
        "1:1"
    );
    assert_eq!(
        run(
            "var seen = ''; try { for (var i = 0; i < 2; i++) { let x = i; if (i === 1) { throw new Error('e'); } } } catch (e) { seen = 'caught'; } seen + ':' + i"
        ),
        "caught:1"
    );
    assert_eq!(
        run(
            "function f() { for (var i = 0; i < 3; i++) { let x = i; if (x === 1) { return 'left ' + i; } } return 'ran out'; } f()"
        ),
        "left 1"
    );
    // A `continue` crosses the block on every pass, so an unbalanced one would deepen the chain
    // once per iteration rather than once — which shows up as the wrong answer only after the loop.
    assert_eq!(
        run(
            "var t = 0; for (var i = 0; i < 4; i++) { let x = i; if (x % 2 === 0) { continue; } t += x; } t + ':' + i"
        ),
        "4:4"
    );
}

#[test]
fn a_for_head_gives_every_pass_its_own_binding() {
    // §14.7.4.7 `CreatePerIterationEnvironment`, which is the one everybody has met: three closures
    // made in three passes answer with three different numbers, where a single shared slot would
    // make all of them say `3`.
    assert_eq!(
        run(
            "var f = []; for (let i = 0; i < 3; i++) { f.push(function () { return i; }); }              f[0]() + ',' + f[1]() + ',' + f[2]()"
        ),
        "0,1,2"
    );
    // …and the loop still counts, which is the half a naive fresh-binding-per-pass would break:
    // the copy happens *before* the update, so `i++` increments the next pass's binding while the
    // closure the last pass made keeps its own.
    assert_eq!(
        run("var s = ''; for (let i = 0; i < 3; i++) { s += i; } s"),
        "012"
    );
    // A `continue` reaches step 3.d as well, so a pass skipped this way still turns the binding
    // over — written out because it is the one exit that must *not* skip the copy.
    assert_eq!(
        run(
            "var f = []; for (let i = 0; i < 4; i++) { if (i === 1) { continue; } f.push(function () { return i; }); }              f.map(function (g) { return g(); }).join(',')"
        ),
        "0,2,3"
    );
    // The initialiser's own environment is never the one a body runs in — step 2's copy — so the
    // first pass is no different from the rest. Nothing but a closure made on that first pass can
    // tell, which is why it is asserted rather than assumed.
    assert_eq!(
        run(
            "var f = []; for (let i = 0; i < 2; i++) { f.push(function () { return i; }); }              f[0]() === f[1]()"
        ),
        "false"
    );
}

/// What a running scope calls its slots, reached through a closure that was made in it.
///
/// A function object holds the environment it was *defined* in, which is the only handle on a
/// scope that a script has once the scope has been left. So the script under test parks a closure
/// in the global `f` and this asks the closure where it came from — which is how the chain a
/// direct `eval` would walk can be read from outside without an `eval`.
fn scope_of(source: &str, out: u32) -> Vec<String> {
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    let global = vm.realm().global();
    let Some(crate::heap::Property {
        kind:
            PropertyKind::Data {
                value: Value::Object(closure),
                ..
            },
        ..
    }) = own(&mut heap, global, "f")
    else {
        panic!("the script leaves a closure in `f`") // the test is about where it was made
    };
    let defined_in = heap
        .object(closure)
        .and_then(crate::heap::Object::environment)
        .expect("a closure knows the scope it was written in"); // same
    let at = heap
        .environment_at(defined_in, out)
        .expect("the chain reaches that far"); // same
    heap.environment_names(at)
        .expect("a scope a source wrote knows its names") // same
        .iter()
        .map(|binding| binding.name.to_string())
        .collect()
}

#[test]
fn a_running_scope_knows_what_the_source_called_its_slots() {
    // DR-0018 — a name resolved when the code was compiled needs nothing at run time, and a
    // direct `eval` needs everything: §19.2.1.1 hands the evaluated source the *running* lexical
    // environment as its outer scope, so the scopes have to carry the names the compiler used up.
    //
    // Read through a closure rather than through an `eval`, because the resolver that uses these
    // is the next slice and the lists are what it will be handed.

    // A call's own environment — the parameters, then the `var`s.
    assert_eq!(
        scope_of(
            "var f; (function (a) { var b; f = function () { return a + b; }; })(1);",
            0
        ),
        ["a", "arguments", "b"]
    );
    // A block's, with the function's one hop further out. Two lists and not one is what makes a
    // `let` in a block a binding of the block.
    let source = "var f; (function (a) { { let b = 1; f = function () { return a + b; }; } })(1);";
    assert_eq!(scope_of(source, 0), ["b"]);
    assert_eq!(scope_of(source, 1), ["a", "arguments"]);
    // §14.7.4.7's per-iteration copy is the same scope, so the closure the third pass made names
    // what the first pass's did.
    assert_eq!(
        scope_of(
            "var f; for (let i = 0; i < 3; i++) { f = function () { return i; }; }",
            0
        ),
        ["i"]
    );
    // …and the script's own environment is at the end of every one of those chains, holding what
    // §16.1.7 puts in it: the top-level lexical declarations, where a `var` is a global property.
    assert_eq!(
        scope_of("let seen = 1; var f = function () { return seen; };", 0),
        ["seen"]
    );
}
