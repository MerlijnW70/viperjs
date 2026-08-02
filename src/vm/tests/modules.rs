//! §16.2's `Module` — the goal symbol, and the four things it does not share with a `Script`.
//!
//! Every row here is a program whose *text* would mean something else read as a script. That is
//! the whole subject: the two goal symbols differ in what the same characters do, not in what they
//! admit — and `import` and `export`, which only one of them admits, are the rest of M7.

use super::*;

#[test]
fn a_module_is_strict_without_saying_so() {
    // §11.2.2 — a module is strict code with no directive needed, and none can turn it off. Three
    // things turn on that, and each is a row: §10.2.1.2 does not substitute the global object for
    // an `undefined` receiver, §6.2.5.6 throws where a sloppy assignment is silent, and §13.5.1.2
    // refuses a `delete` rather than answering `false`.
    assert_eq!(
        run_module_source("var f = function () { return this; }; typeof f()"),
        "undefined"
    );
    assert_eq!(
        run_module_source(
            "var o = {}; Object.defineProperty(o, 'a', { value: 1, writable: false });              var caught = 'none'; try { o.a = 2; } catch (e) { caught = e.constructor.name; } caught"
        ),
        "TypeError"
    );
    // …and the same two texts read as a *script* answer the other way, which is what makes these
    // about the goal symbol rather than about strict mode.
    assert_eq!(
        run("var f = function () { return this; }; typeof f()"),
        "object"
    );
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', { value: 1, writable: false });              var caught = 'none'; try { o.a = 2; } catch (e) { caught = e.constructor.name; } caught"
        ),
        "none"
    );
}

#[test]
fn a_modules_top_level_declarations_are_its_own_and_not_the_global_objects() {
    // §16.1.7 puts a Script's top-level `var` on the global object. §16.2.1.6 does not: a module's
    // are slots of its own scope, which is why a module cannot be observed from outside by reading
    // `globalThis` and a script can.
    assert_eq!(
        run_module_source("var x = 1; typeof globalThis.x"),
        "undefined"
    );
    assert_eq!(
        run_module_source("function f() {} typeof globalThis.f"),
        "undefined"
    );
    assert_eq!(run("var x = 1; typeof globalThis.x"), "number");
    assert_eq!(run("function f() {} typeof globalThis.f"), "function");
    // The binding is still there to be read — it is *scoped*, not absent.
    assert_eq!(run_module_source("var x = 1; x"), "1");
    assert_eq!(run_module_source("function f() { return 2; } f()"), "2");
    assert_eq!(run_module_source("let y = 3; const z = 4; y + z"), "7");
}

#[test]
fn a_modules_this_is_undefined() {
    // §16.2.1.6 — the one place the two goal symbols disagree about `this`, and the only difference
    // between them that is decided when the body *runs* rather than when it is compiled.
    assert_eq!(run_module_source("typeof this"), "undefined");
    assert_eq!(run("typeof this"), "object");
    assert_eq!(run("this === globalThis"), "true");
}

#[test]
fn a_top_level_await_is_refused_rather_than_compiled_into_a_chunk_that_cannot_run() {
    // §16.2.1.5.3 — a module may `await` at its top level, and a module that does is *asynchronous*:
    // its evaluation answers a promise and everything importing it waits. praxis has none of that,
    // and `Instruction::Await` parks the running execution — so at a module's top level it parked
    // with nothing to park into and the interpreter answered `Fault::YieldOutsideGenerator`. That
    // is a chunk that does not make sense, which is a bug rather than a missing feature, and 169
    // conformance files reached it.
    let mut heap = Heap::new();
    let module = crate::parser::parse_module("await 1;").expect("a module may say this"); // the test is about the refusal
    let error = crate::compile::compile_module(&module, &mut heap).expect_err("not built yet"); // same
    assert_eq!(
        error.kind,
        crate::compile::ErrorKind::Unsupported("a top-level `await`")
    );
    // …and it is the *top level* that is refused, not `await`. One inside an `async` function in
    // the same module has an execution to park and compiles.
    let module = crate::parser::parse_module("async function f() { await 1; } f();")
        .expect("a module may say this"); // same
    assert!(crate::compile::compile_module(&module, &mut heap).is_ok());
}
