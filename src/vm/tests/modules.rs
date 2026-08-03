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

#[test]
fn a_namespace_object_is_the_module_seen_as_an_object() {
    // §10.4.6 — one property per export, each a **live** read of the exporting module's slot. A
    // snapshot taken at link time would answer 0 below for ever.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export let n = 0; export function bump() { n = n + 1; } export default 9;"
                ),
                (
                    "main",
                    "import * as ns from 'dep'; ns.bump(); ns.bump(); \
                     ns.n + ':' + ns.default + ':' + typeof ns"
                ),
            ],
            "main"
        ),
        "2:9:object"
    );
    // §16.2.1.10 memoises one object per module, so two importers of the same module — and two
    // clauses in one importer — see the *same* namespace.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                ("left", "import * as ns from 'dep'; export var l = ns;"),
                (
                    "main",
                    "import * as ns from 'dep'; import { l } from 'left'; ns === l"
                ),
            ],
            "main"
        ),
        "true"
    );
}

#[test]
fn a_namespace_refuses_every_way_of_changing_it() {
    // §10.4.6.9's `[[Set]]`, §10.4.6.11's `[[Delete]]` and §10.4.6.2's extensibility. A module is
    // strict, so each of the first two is a TypeError rather than the silent failure sloppy code
    // would get.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                (
                    "main",
                    "import * as ns from 'dep'; \
                     var out = []; \
                     try { ns.a = 2; } catch (e) { out.push('set:' + e.constructor.name); } \
                     try { delete ns.a; } catch (e) { out.push('delete:' + e.constructor.name); } \
                     try { ns.fresh = 1; } catch (e) { out.push('add:' + e.constructor.name); } \
                     out.push('extensible:' + Object.isExtensible(ns)); \
                     var elsewhere = {}; \
                     out.push('reflect:' + Reflect.set(ns, 'a', 3, elsewhere)); \
                     out.push('elsewhere:' + ('a' in elsewhere)); \
                     out.push('proto:' + Object.getPrototypeOf(ns)); \
                     out.push('a:' + ns.a); \
                     out.join(' ')"
                ),
            ],
            "main"
        ),
        // §10.4.6.9 refuses whatever the receiver is, and writes nothing to it. An ordinary
        // `[[Set]]` would find a writable data property here — an export reports `writable: true` —
        // and would go on to define the name on the *receiver*, answering true.
        "set:TypeError delete:TypeError add:TypeError extensible:false reflect:false \
         elsewhere:false proto:null a:1"
    );
}

#[test]
fn a_namespaces_names_are_sorted_and_a_symbol_is_not_one_of_them() {
    // §10.4.6.10 — by code unit, which is the one enumeration order in the language that is not
    // the order things were written in. `@@toStringTag` comes after them all and is not enumerable.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export var zebra = 1; export var apple = 2; export default 3; \
                     export var Banana = 4;"
                ),
                (
                    "main",
                    "import * as ns from 'dep'; \
                     Object.keys(ns).join(',') + '|' + Object.prototype.toString.call(ns) + \
                     '|' + ('apple' in ns) + ('nope' in ns)"
                ),
            ],
            "main"
        ),
        // `B` is 0x42 and `a` is 0x61, so an upper-case name sorts before every lower-case one —
        // which alphabetical order would not do and code-unit order does.
        "Banana,apple,default,zebra|[object Module]|truefalse"
    );
    // §10.4.6.5 — an export is a **data** property, not an accessor, and `writable: true` beside a
    // `[[Set]]` that always refuses.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                (
                    "main",
                    "import * as ns from 'dep'; \
                     var d = Object.getOwnPropertyDescriptor(ns, 'a'); \
                     [d.value, d.writable, d.enumerable, d.configurable].join(',')"
                ),
            ],
            "main"
        ),
        "1,true,true,false"
    );
}

#[test]
fn reading_a_namespaces_export_before_its_module_ran_is_a_reference_error() {
    // §10.4.6.8 step 9 — the dead zone reached through an object rather than through a name. An
    // importer always evaluates *after* what it imports, so the only way to see a module's binding
    // before its `let` has run is for the module to hold its own namespace — which a self-import
    // gives it, and which §16.2.1.10 makes the same object either way.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self'; \
                 var before = 'none'; \
                 try { me.late; } catch (e) { before = e.constructor.name; } \
                 export let late = 1; \
                 before + ':' + me.late"
            )],
            "self"
        ),
        "ReferenceError:1"
    );
    // §16.2.3.7 — `export default <expression>` is a **lexical** declaration of `*default*`, so it
    // has a dead zone like any other; `export default function () {}` is hoisted and does not.
    // Nothing in the module can spell the name, so its own namespace is the only way to see either.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self';                  var caught = 'none';                  try { me.default; } catch (e) { caught = e.constructor.name; }                  export default 5;                  caught + ':' + me.default"
            )],
            "self"
        ),
        "ReferenceError:5"
    );
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self'; \
                 var early = me.default; \
                 export default function () {} \
                 typeof early + ':' + (early === me.default)"
            )],
            "self"
        ),
        // The **same** function, not merely a function at both moments: hoisting it and then
        // building it again where it stands would answer `function:false`, and the binding the
        // export points at would be a second object nothing else had ever seen.
        "function:true"
    );
    // A class default is the other half of the same rule: §16.2.3.7 leaves it lexical, so unlike a
    // function it has a dead zone *and* is built where it stands.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self'; \
                 var caught = 'none'; \
                 try { me.default; } catch (e) { caught = e.constructor.name; } \
                 export default class { m() { return 7; } } \
                 caught + ':' + new me.default().m()"
            )],
            "self"
        ),
        "ReferenceError:7"
    );
    // §10.4.6.5 step 4 — and *asking about* the property throws too, because the descriptor is
    // built out of `[[Get]]`. That is why `Object.keys` on a namespace can throw at all: it asks
    // each name whether it is enumerable, which cannot be answered without the value.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self';                  var out = [];                  try { Object.keys(me); } catch (e) { out.push('keys:' + e.constructor.name); }                  try { Object.getOwnPropertyDescriptor(me, 'late'); }                    catch (e) { out.push('descriptor:' + e.constructor.name); }                  try { Object.prototype.hasOwnProperty.call(me, 'late'); }                    catch (e) { out.push('has:' + e.constructor.name); }                  export let late = 1;                  out.join(' ')"
            )],
            "self"
        ),
        "keys:ReferenceError descriptor:ReferenceError has:ReferenceError"
    );
    // …and a name the module does not export at all is `undefined` rather than an error, which is
    // the difference between a binding that is not ready and one that does not exist.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                ("main", "import * as ns from 'dep'; String(ns.nope)"),
            ],
            "main"
        ),
        "undefined"
    );
}
