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

/// Compile several modules and link them, answering what the entry evaluated to.
///
/// The host's half of §16.2.1.7 done by hand: the caller says which specifier names which source,
/// which is exactly what a real host would work out from a filesystem.
fn run_graph(modules: &[(&str, &str)], entry: &str) -> String {
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    for (specifier, source) in modules {
        let parsed = crate::parser::parse_module(source).expect("the source parses"); // a VM test needs a chunk
        let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("it compiles"); // same
        graph.insert(specifier, std::rc::Rc::new(chunk));
    }
    let mut vm = Vm::new(&mut heap);
    let outcome = vm
        .run_module_graph(entry, &graph, &mut heap)
        .expect("the chunks are well formed") // same
        .expect("the graph links"); // same
    describe(outcome, &mut heap)
}

#[test]
fn an_import_is_the_exporting_modules_binding_and_not_a_copy_of_it() {
    // §16.2.1.5.2 `CreateImportBinding` — the whole reason an import is not an assignment. Two
    // modules are two chains with no depth between them, so the slot has to say where it really
    // lives; a copy taken at link time would answer 0 below for ever.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export let n = 0; export function bump() { n = n + 1; }"
                ),
                ("main", "import { n, bump } from 'dep'; bump(); bump(); n"),
            ],
            "main"
        ),
        "2"
    );
    // Every declaration form an `export` may stand in front of.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export var a = 1; export let b = 2; export const c = 3; \
                     export function f() { return 4; } export class K { m() { return 5; } }"
                ),
                (
                    "main",
                    "import { a, b, c, f, K } from 'dep'; a + b + c + f() + new K().m()"
                ),
            ],
            "main"
        ),
        "15"
    );
    // `export {a as b}` renames on the way out, and `import {b as c}` on the way in — neither is a
    // binding in the module that writes it, which is why both sides may say a reserved word.
    assert_eq!(
        run_graph(
            &[
                ("dep", "var x = 7; export { x as out };"),
                ("main", "import { out as here } from 'dep'; here"),
            ],
            "main"
        ),
        "7"
    );
}

#[test]
fn an_import_may_not_be_assigned_to() {
    // §16.2.1.5.2 makes the binding immutable, and a module is strict — so the assignment is a
    // TypeError rather than the silent failure a sloppy one would be.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                (
                    "main",
                    "import { a } from 'dep'; \
                     var caught = 'none'; try { a = 2; } catch (e) { caught = e.constructor.name; } caught"
                ),
            ],
            "main"
        ),
        "TypeError"
    );
}

#[test]
fn a_module_is_evaluated_once_and_after_everything_it_imports() {
    // §16.2.1.6 — dependencies first, and a module that two others import runs *once*. The order
    // is what makes an imported binding hold a value rather than sit in its dead zone.
    assert_eq!(
        run_graph(
            &[
                (
                    "counter",
                    "globalThis.runs = (globalThis.runs || 0) + 1; export var n = 1;"
                ),
                ("left", "import { n } from 'counter'; export var l = n;"),
                ("right", "import { n } from 'counter'; export var r = n;"),
                (
                    "main",
                    "import { l } from 'left'; import { r } from 'right'; l + r + globalThis.runs"
                ),
            ],
            "main"
        ),
        "3"
    );
    // A module that imports **itself** is one module and not two. Its own bindings are in their
    // dead zone while its body runs, which is what the specification says and is why the record is
    // keyed by the module rather than by the name that reached it.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import { later as seen } from 'self'; \
                 var caught = 'none'; try { seen; } catch (e) { caught = e.constructor.name; } \
                 export let later = 1; caught"
            )],
            "self"
        ),
        "ReferenceError"
    );
}

#[test]
fn a_default_export_is_a_name_no_module_can_spell() {
    // §16.2.3.7 — the exported name is `default` whatever the thing is called, and an expression
    // has no name of its own at all.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export default 41 + 1;"),
                ("main", "import d from 'dep'; d"),
            ],
            "main"
        ),
        "42"
    );
    // A *declaration* keeps its own binding too, so the module can still use it under its name.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export default function f() { return 6; } export var also = f;"
                ),
                (
                    "main",
                    "import d, { also } from 'dep'; (d === also) + ':' + d()"
                ),
            ],
            "main"
        ),
        "true:6"
    );
    // …and an anonymous one has only the unspellable slot, which the import still reaches.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export default function () { return 8; }"),
                ("main", "import d from 'dep'; d()"),
            ],
            "main"
        ),
        "8"
    );
    // §8.6.3 — an anonymous default is a named position, and the name is `"default"`.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export default function () {};"),
                ("main", "import d from 'dep'; d.name"),
            ],
            "main"
        ),
        "default"
    );
}

#[test]
fn a_specifier_nothing_answers_and_a_name_nothing_exports_are_both_the_hosts_to_report() {
    // §16.2.1.5's own errors, which are about the *graph* rather than about any module's code — so
    // they are answered before anything runs and are not a throw the program could catch.
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    let parsed = crate::parser::parse_module("import { a } from 'nowhere'; a").expect("parses"); // the test is about linking
    let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("compiles"); // same
    graph.insert("main", std::rc::Rc::new(chunk));
    let mut vm = Vm::new(&mut heap);
    let refused = vm
        .run_module_graph("main", &graph, &mut heap)
        .expect("well formed"); // same
    assert!(
        matches!(refused, Err(ref error) if error.message().contains("no module was supplied")),
        "{refused:?}"
    );

    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    for (specifier, source) in [
        ("dep", "export var b = 1;"),
        ("main", "import { a } from 'dep';"),
    ] {
        let parsed = crate::parser::parse_module(source).expect("parses"); // same
        let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("compiles"); // same
        graph.insert(specifier, std::rc::Rc::new(chunk));
    }
    let mut vm = Vm::new(&mut heap);
    let refused = vm
        .run_module_graph("main", &graph, &mut heap)
        .expect("well formed"); // same
    assert!(
        matches!(refused, Err(ref error) if error.message().contains("does not export")),
        "{refused:?}"
    );
}

#[test]
fn a_bare_import_is_an_edge_that_binds_nothing() {
    // `import "a";` names no binding and is written for the other module being evaluated. The edge
    // is still in the graph, which is the only thing that makes the side effect happen.
    assert_eq!(
        run_graph(
            &[
                ("side", "globalThis.ran = 'yes';"),
                ("main", "import 'side'; globalThis.ran"),
            ],
            "main"
        ),
        "yes"
    );
}
