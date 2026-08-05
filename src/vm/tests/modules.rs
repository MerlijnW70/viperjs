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
fn a_module_may_await_at_its_top_level_and_everything_importing_it_waits() {
    // §16.2.1.5.3 — a module whose body contains a top-level `await` is asynchronous: its
    // evaluation answers a promise, and the modules that import it do not start until it settles.
    // The value the `await` produced is there to be exported like any other.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var answer = await Promise.resolve(41) + 1;"),
                ("main", "import { answer } from 'dep'; answer"),
            ],
            "main"
        ),
        "42"
    );
    // …and the *order* is what waiting means: `dep`'s body finishes before `main`'s begins, even
    // though finishing takes a turn of the job queue.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "globalThis.log = 'dep-start;'; \
                     await Promise.resolve(); \
                     globalThis.log += 'dep-end;'; \
                     export var ready = true;"
                ),
                (
                    "main",
                    "import { ready } from 'dep'; globalThis.log += 'main;'; globalThis.log"
                ),
            ],
            "main"
        ),
        "dep-start;dep-end;main;"
    );
    // Several awaits in a row, and one on a value that is not a promise — §27.7.5.3 wraps it, so it
    // still costs a turn.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "var seen = ''; \
                 seen += await 'a'; \
                 seen += await Promise.resolve('b'); \
                 seen += await 'c'; \
                 seen"
            )],
            "self"
        ),
        "abc"
    );
}

#[test]
fn what_an_asynchronous_module_left_queued_runs_before_the_host_is_answered() {
    // §9.5 — the jobs run when no execution is running, which for a script is after its last
    // statement and for a synchronous module is after its body. An asynchronous one reaches neither
    // of those, so without a drain of its own a `then` the body registered was still waiting when
    // the host was told the module had finished.
    assert_eq!(
        run_graph(
            &[(
                "m",
                "globalThis.out = 'queued'; \
                 Promise.resolve().then(function () { globalThis.out = 'ran'; }); \
                 await Promise.resolve(); \
                 globalThis.out"
            )],
            "m"
        ),
        // The body's own value is what the module evaluates to — read before the drain, since a job
        // is not the module — and the queue is empty by the time the answer is handed back.
        "ran"
    );
    // A computed key is an ordinary expression of the enclosing code, so an `await` in one is a
    // *top-level* await and makes the module asynchronous like any other.
    assert_eq!(
        run_graph(&[("m", "var o = { [await 'k']: 5 }; o.k")], "m"),
        "5"
    );
    assert_eq!(
        run_graph(
            &[("m", "class C { [await 'm']() { return 6; } } new C().m()")],
            "m"
        ),
        "6"
    );
}

#[test]
fn a_module_that_throws_after_awaiting_rejects_rather_than_escaping() {
    // §27.7.5.2's wrapper, which an asynchronous module gets for the reason an `async` function
    // does: a throw nothing inside caught **rejects the module's promise**, and §16.2.1.5.3 makes
    // that the module's failure. Before the `await` and after it are the same answer, which is what
    // makes the handler rather than the instruction the thing that decides.
    for source in [
        "throw new Error('before'); export var a = 1;",
        "await Promise.resolve(); throw new Error('after'); export var a = 1;",
    ] {
        let mut heap = Heap::new();
        let mut graph = crate::vm::Graph::new();
        for (specifier, text) in [("dep", source), ("main", "import { a } from 'dep'; a")] {
            let parsed = crate::parser::parse_module(text).expect("parses"); // the test is about the throw
            let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("compiles"); // same
            graph.insert(specifier, std::rc::Rc::new(chunk));
        }
        let mut vm = Vm::new(&mut heap);
        let outcome = vm
            .run_module_graph("main", &graph, &mut heap)
            .expect("well formed") // same
            .expect("the graph links"); // same
        assert!(
            matches!(outcome, crate::vm::Outcome::Thrown(_)),
            "for {source:?}: {outcome:?}"
        );
    }
    // …and a module that awaits without ever throwing is not a failure, which is what makes the
    // rows above about the throw.
    assert_eq!(
        run_graph(
            &[
                ("dep", "await Promise.resolve(); export var a = 7;"),
                ("main", "import { a } from 'dep'; a"),
            ],
            "main"
        ),
        "7"
    );
}

#[test]
fn a_top_level_await_is_only_a_module_thing() {
    // §16.2.1.5.3 belongs to the Module goal. A Script has no `AwaitExpression` at its top level at
    // all — §13.1 makes `await` an ordinary identifier there — so the same text means something
    // else rather than being refused.
    assert_eq!(run("var await = 3; await + 1"), "4");
    // …and a module compiled without one is **not** asynchronous, so its body still runs straight
    // through: the second pass is paid only by a module that uses the production.
    let mut heap = Heap::new();
    let plain = crate::parser::parse_module("export var a = 1;").expect("parses"); // the test is about the flag
    let plain = crate::compile::compile_module(&plain, &mut heap).expect("compiles"); // same
    assert!(!plain.is_async());
    let waiting = crate::parser::parse_module("export var a = await 1;").expect("parses"); // same
    let waiting = crate::compile::compile_module(&waiting, &mut heap).expect("compiles"); // same
    assert!(waiting.is_async());
    // An `await` inside an `async` function in the module does not make the *module* asynchronous:
    // that one has its own execution to park into.
    let inner =
        crate::parser::parse_module("async function f() { await 1; } f();").expect("parses"); // same
    let inner = crate::compile::compile_module(&inner, &mut heap).expect("compiles"); // same
    assert!(!inner.is_async());
}

/// Compile several modules and link them, answering what the entry evaluated to.
///
/// The host's half of §16.2.1.7 done by hand: the caller says which specifier names which source,
/// which is exactly what a real host would work out from a filesystem.
/// The same, with the collection schedule set — DR-0023's diagnosis harness.
fn run_graph_collecting(modules: &[(&str, &str)], entry: &str, growth: Option<usize>) -> String {
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    for (specifier, source) in modules {
        let parsed = crate::parser::parse_module(source).expect("the source parses"); // a VM test needs a chunk
        let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("it compiles"); // same
        graph.insert(specifier, std::rc::Rc::new(chunk));
    }
    let mut vm = Vm::new(&mut heap);
    vm.set_collection_growth(growth);
    let outcome = vm
        .run_module_graph(entry, &graph, &mut heap)
        .expect("the chunks are well formed") // same
        .expect("the graph links"); // same
    describe(outcome, &mut heap)
}

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
    // §10.4.6.12 step 8 — `@@toStringTag` is the one own property that is not an export, and its
    // three attributes are all false. `configurable: false` is what makes it unlike every other
    // `@@toStringTag` in the language: a namespace's cannot be deleted or redefined.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                (
                    "main",
                    "import * as ns from 'dep';                      var d = Object.getOwnPropertyDescriptor(ns, Symbol.toStringTag);                      [d.value, d.writable, d.enumerable, d.configurable].join(',') + '|' +                      Object.keys(ns).join(',')"
                ),
            ],
            "main"
        ),
        // …and not enumerable, so it is not one of the keys.
        "Module,false,false,false|a"
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
    // §10.4.6.7 — `in` answers **true** for an export whose module has not yet reached the line
    // that gives it a value. Presence and readiness are different questions, and only the second
    // throws: a descriptor for the same name is a ReferenceError two rows above.
    assert_eq!(
        run_graph(
            &[(
                "self",
                "import * as me from 'self';                  var out = ('late' in me) + ':' + ('nope' in me);                  export let late = 1;                  out"
            )],
            "self"
        ),
        "true:false"
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

#[test]
fn a_re_export_names_another_modules_binding_and_makes_none_of_its_own() {
    // §16.2.1.3 — an indirect export is **not** an import: `a` leaves this module without ever
    // being a name in it, which is why the middle module below cannot read what it passes on.
    assert_eq!(
        run_graph(
            &[
                (
                    "deep",
                    "export let a = 1; export function bump() { a = a + 1; }"
                ),
                ("middle", "export { a, bump } from 'deep';"),
                ("main", "import { a, bump } from 'middle'; bump(); a"),
            ],
            "main"
        ),
        "2"
    );
    assert_eq!(
        run_graph(
            &[
                ("deep", "export var a = 1;"),
                (
                    "middle",
                    "export { a } from 'deep'; \
                     export var saw = typeof a === 'undefined' ? 'unbound' : 'bound';"
                ),
                ("main", "import { saw } from 'middle'; saw"),
            ],
            "main"
        ),
        // A module is strict, so reading a name nothing bound would throw — `typeof` is the one
        // operator that asks without reading.
        "unbound"
    );
    // A chain of them: §16.2.1.6.3 walks as far as it has to, and the binding it lands on is the
    // one the original module has.
    assert_eq!(
        run_graph(
            &[
                ("one", "export let n = 5;"),
                ("two", "export { n } from 'one';"),
                ("three", "export { n as m } from 'two';"),
                ("main", "import { m } from 'three'; m"),
            ],
            "main"
        ),
        "5"
    );
}

#[test]
fn a_star_export_carries_every_name_but_default() {
    // §16.2.1.6.2 step 5.b — the one name a star does not bring, which is what makes
    // `export * from "m"` safe to write over a module that has a default.
    assert_eq!(
        run_graph(
            &[
                (
                    "deep",
                    "export var a = 1; export var b = 2; export default 3;"
                ),
                ("middle", "export * from 'deep'; export default 4;"),
                (
                    "main",
                    "import d, { a, b } from 'middle'; a + ':' + b + ':' + d"
                ),
            ],
            "main"
        ),
        "1:2:4"
    );
    // …and it is transitive: a star of a star reaches the original binding.
    assert_eq!(
        run_graph(
            &[
                ("one", "export var deep = 7;"),
                ("two", "export * from 'one';"),
                ("three", "export * from 'two';"),
                ("main", "import { deep } from 'three'; deep"),
            ],
            "main"
        ),
        "7"
    );
    // A namespace over a cycle of star exports terminates too, and that is a *second* walk:
    // §16.2.1.6.2 gathers the names and §16.2.1.6.3 resolves each, and each has its own reason to
    // stop. Without the first one's, building this object never returns.
    assert_eq!(
        run_graph(
            &[
                ("left", "export * from 'right'; export var here = 1;"),
                ("right", "export * from 'left'; export var there = 2;"),
                (
                    "main",
                    "import * as ns from 'left'; Object.keys(ns).join(',')"
                ),
            ],
            "main"
        ),
        "here,there"
    );
    // …and `default` is still the one name a star does not carry, through a cycle as anywhere else.
    assert_eq!(
        run_graph(
            &[
                ("deep", "export var a = 1; export default 9;"),
                ("middle", "export * from 'deep';"),
                (
                    "main",
                    "import * as ns from 'middle'; Object.keys(ns).join(',') + ':' +                      (typeof ns.default)"
                ),
            ],
            "main"
        ),
        "a:undefined"
    );
    // A cycle of star exports terminates — §16.2.1.6.2 step 1 and §16.2.1.6.3 step 1 — rather than
    // walking for ever, and the name is still found by the path that has it.
    assert_eq!(
        run_graph(
            &[
                ("left", "export * from 'right'; export var here = 1;"),
                ("right", "export * from 'left'; export var there = 2;"),
                ("main", "import { here, there } from 'left'; here + there"),
            ],
            "main"
        ),
        "3"
    );
}

#[test]
fn a_name_two_star_exports_disagree_about_is_refused_only_when_it_is_asked_for() {
    // §16.2.1.6.3 step 6.c — ambiguous, which is a SyntaxError for the *import* and not for the
    // module that has it: `middle` below is perfectly usable so long as nobody asks for `same`.
    let modules: &[(&str, &str)] = &[
        ("left", "export var same = 1; export var only_left = 10;"),
        ("right", "export var same = 2;"),
        ("middle", "export * from 'left'; export * from 'right';"),
    ];
    let mut asking: Vec<(&str, &str)> = modules.to_vec();
    asking.push(("main", "import { only_left } from 'middle'; only_left"));
    assert_eq!(run_graph(&asking, "main"), "10");
    // …and the same graph, asked for the ambiguous name, refuses before anything runs.
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    let mut refusing: Vec<(&str, &str)> = modules.to_vec();
    refusing.push(("main", "import { same } from 'middle'; same"));
    for (specifier, source) in &refusing {
        let parsed = crate::parser::parse_module(source).expect("parses"); // the test is about linking
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
    // §16.2.1.6.4 step 3 — an indirect export is resolved at link time whether or not anything
    // imports it. `middle` below re-exports a name `deep` does not have, and nothing asks for it:
    // the module that wrote the line is still refused.
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    for (specifier, source) in [
        ("deep", "export var real = 1;"),
        (
            "middle",
            "export { nope } from 'deep'; export var fine = 2;",
        ),
        ("main", "import { fine } from 'middle'; fine"),
    ] {
        let parsed = crate::parser::parse_module(source).expect("parses"); // the test is about linking
        let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("compiles"); // same
        graph.insert(specifier, std::rc::Rc::new(chunk));
    }
    let mut vm = Vm::new(&mut heap);
    let refused = vm
        .run_module_graph("main", &graph, &mut heap)
        .expect("well formed"); // same
    assert!(
        matches!(refused, Err(ref error) if error.message().contains("nope")),
        "{refused:?}"
    );
    // A diamond is **not** ambiguous: both paths reach the same binding, so the answer agrees with
    // itself and the name resolves.
    assert_eq!(
        run_graph(
            &[
                ("base", "export var shared = 4;"),
                ("left", "export * from 'base';"),
                ("right", "export * from 'base';"),
                ("middle", "export * from 'left'; export * from 'right';"),
                ("main", "import { shared } from 'middle'; shared"),
            ],
            "main"
        ),
        "4"
    );
}

#[test]
fn a_star_export_under_a_name_is_the_other_modules_whole_namespace() {
    // §16.2.1.6.3 step 3.a.ii — `export * as n from "m"` exports one name whose value is `m`'s
    // namespace object, and not any binding of `m`.
    assert_eq!(
        run_graph(
            &[
                ("deep", "export var a = 1; export var b = 2;"),
                ("middle", "export * as inner from 'deep';"),
                (
                    "main",
                    "import { inner } from 'middle'; import * as direct from 'deep'; \
                     Object.keys(inner).join(',') + ':' + (inner === direct)"
                ),
            ],
            "main"
        ),
        // §16.2.1.10 memoises one namespace per module, so the one reached this way is the same
        // object a direct `import * as` gives.
        "a,b:true"
    );
    // …and a namespace object built over re-exports lists them all, which is §16.2.1.6.2 feeding
    // §10.4.6.10.
    assert_eq!(
        run_graph(
            &[
                (
                    "deep",
                    "export var z = 1; export var a = 2; export default 3;"
                ),
                ("middle", "export * from 'deep'; export var own = 4;"),
                (
                    "main",
                    "import * as ns from 'middle'; Object.keys(ns).join(',')"
                ),
            ],
            "main"
        ),
        "a,own,z"
    );
}

#[test]
fn a_namespace_accepts_a_define_only_when_it_changes_nothing() {
    // §10.4.6.6 — the descriptor has to match the export exactly, attributes and all. A bare
    // `{ value }` does not: an omitted `configurable` is read as `false`… and the export's is
    // false too, so what refuses it is the *value* alone being restated as a full descriptor.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export var a = 1;"),
                (
                    "main",
                    "import * as ns from 'dep';                      var out = [];                      out.push('same:' + Reflect.defineProperty(ns, 'a',                        { value: 1, writable: true, enumerable: true, configurable: false }));                      out.push('other:' + Reflect.defineProperty(ns, 'a', { value: 2 }));                      out.push('configurable:' + Reflect.defineProperty(ns, 'a',                        { value: 1, writable: true, enumerable: true, configurable: true }));                      out.push('accessor:' + Reflect.defineProperty(ns, 'a',                        { get: function () { return 1; } }));                      out.push('fresh:' + Reflect.defineProperty(ns, 'b', { value: 1 }));                      out.push('a:' + ns.a);                      out.join(' ')"
                ),
            ],
            "main"
        ),
        "same:true other:false configurable:false accessor:false fresh:false a:1"
    );
}

#[test]
fn a_re_export_requires_its_module_even_when_no_name_crosses() {
    // §16.2.1.4's `[[RequestedModules]]` — `export {} from "m"` names nothing and still depends
    // on `m`, so a specifier nothing answers is a resolution error rather than a line with no
    // effect. Without the edge the module was never loaded and the line did nothing at all.
    let mut heap = Heap::new();
    let mut graph = crate::vm::Graph::new();
    let parsed = crate::parser::parse_module("export {} from 'nowhere'; 1").expect("parses"); // the test is about linking
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
    // …and one that *is* supplied is evaluated, in order, like any other dependency.
    assert_eq!(
        run_graph(
            &[
                ("side", "globalThis.ran = 'yes';"),
                ("main", "export {} from 'side'; globalThis.ran"),
            ],
            "main"
        ),
        "yes"
    );
}

/// A loader that answers from a table the test wrote — §16.2.1.7 done by hand.
struct Supplied {
    sources: Vec<(String, String)>,
}

impl crate::vm::ModuleLoader for Supplied {
    fn load(
        &mut self,
        _referrer: Option<&str>,
        specifier: &str,
        heap: &mut Heap,
    ) -> Result<(String, std::rc::Rc<crate::compile::Chunk>), String> {
        let source = self
            .sources
            .iter()
            .find(|(name, _)| name == specifier)
            .map(|(_, source)| source.clone())
            .ok_or_else(|| format!("nothing is at {specifier:?}"))?;
        let parsed = crate::parser::parse_module(&source)
            .map_err(|error| format!("{specifier:?} did not parse: {}", error.kind))?;
        let compiled = crate::compile::compile_module(&parsed, heap).map_err(|e| e.message())?;
        // A flat table of unique names, so the specifier *is* the key — DR-0020's degenerate case
        // and the one every host that already knows its whole program is in.
        Ok((specifier.to_string(), std::rc::Rc::new(compiled)))
    }
}

/// Run `source` as a script, with `modules` reachable by a dynamic `import()`.
fn run_importing(modules: &[(&str, &str)], source: &str) -> String {
    let mut heap = Heap::new();
    let script = crate::parser::parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = crate::compile::compile_script(&script, &mut heap).expect("it compiles"); // same
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(Supplied {
        sources: modules
            .iter()
            .map(|(name, source)| ((*name).to_string(), (*source).to_string()))
            .collect(),
    }));
    let outcome = vm.run(&chunk, &mut heap).expect("the chunk makes sense"); // same
    describe(outcome, &mut heap)
}

/// The same, answering `globalThis.out` once every job has run.
///
/// What a settled promise did is only visible after the queue drains, and DR-0016 drains it inside
/// `run` — so a second `run` of a one-line script is how a test reads the result without the
/// engine growing an API for it.
fn after_jobs(modules: &[(&str, &str)], source: &str) -> String {
    let mut heap = Heap::new();
    let script = crate::parser::parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = crate::compile::compile_script(&script, &mut heap).expect("it compiles"); // same
    let read = crate::parser::parse_script("String(globalThis.out)").expect("parses"); // same
    let reader = crate::compile::compile_script(&read, &mut heap).expect("compiles"); // same
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(Supplied {
        sources: modules
            .iter()
            .map(|(name, source)| ((*name).to_string(), (*source).to_string()))
            .collect(),
    }));
    vm.run(&chunk, &mut heap).expect("the chunk makes sense"); // same
    let outcome = vm.run(&reader, &mut heap).expect("the chunk makes sense"); // same
    describe(outcome, &mut heap)
}

#[test]
fn a_dynamic_import_answers_a_promise_and_never_settles_before_the_statement_ends() {
    // §13.3.10 — the value is a promise, and nothing it does may be observable before the
    // statement containing it has finished. A module loaded where the `import()` is written would
    // run another module's body in the middle of this expression.
    assert_eq!(
        run_importing(
            &[("dep", "globalThis.ran = 'yes'; export var a = 1;")],
            "import('dep'); typeof globalThis.ran"
        ),
        "undefined"
    );
    // …and once the queue has drained it has settled, with the module's **namespace** — §16.2.1.11
    // step 5 — rather than with whatever the body evaluated to.
    assert_eq!(
        after_jobs(
            &[(
                "dep",
                "export var a = 1; export default 2; 'the body value';"
            )],
            "import('dep').then(function (ns) { \
               globalThis.out = ns.a + ':' + ns.default + ':' + Object.keys(ns).join(); \
             });"
        ),
        "1:2:a,default"
    );
}

#[test]
fn a_dynamically_imported_module_is_the_same_one_a_static_import_reached() {
    // §16.2.1.6's "each body once" is a fact about the whole execution and not about one call, so
    // a module an earlier import evaluated is not evaluated again — and §16.2.1.10 answers with the
    // same namespace object for it.
    assert_eq!(
        after_jobs(
            &[
                (
                    "counter",
                    "globalThis.runs = (globalThis.runs || 0) + 1; export var n = 1;"
                ),
                ("mid", "import { n } from 'counter'; export var m = n;"),
            ],
            "import('mid') \
               .then(function () { return import('counter'); }) \
               .then(function (first) { \
                 return import('counter').then(function (again) { \
                   globalThis.out = globalThis.runs + ':' + (first === again); \
                 }); \
               });"
        ),
        "1:true"
    );
    // A module that **threw** throws the same value at every later importer rather than being run
    // again — §16.2.1.6 step 9's `[[EvaluationError]]`.
    assert_eq!(
        after_jobs(
            &[(
                "bad",
                "globalThis.tries = (globalThis.tries || 0) + 1; throw new Error('once');"
            )],
            "import('bad').catch(function (first) { \
               return import('bad').catch(function (again) { \
                 globalThis.out = globalThis.tries + ':' + (first === again) + ':' + first.message; \
               }); \
             });"
        ),
        "1:true:once"
    );
}

#[test]
fn a_dynamic_import_rejects_rather_than_throwing() {
    // §13.3.10 step 6 and §16.2.1.7 — a specifier nothing answers, a module that will not compile
    // and a body that throws are all **rejections**. None may throw out of the `import()`, because
    // the expression has already answered with a promise.
    let modules: &[(&str, &str)] = &[
        ("broken", "var 1 = 2;"),
        ("throws", "throw new Error('from the body');"),
    ];
    for (specifier, expected) in [
        ("nowhere", "nothing is at"),
        ("broken", "did not parse"),
        ("throws", "from the body"),
    ] {
        let answer = after_jobs(
            modules,
            &format!(
                "globalThis.out = 'never settled'; \
                 import('{specifier}').then( \
                   function () {{ globalThis.out = 'fulfilled'; }}, \
                   function (e) {{ globalThis.out = String(e.message); }});"
            ),
        );
        assert!(answer.contains(expected), "for {specifier:?}: {answer}");
    }
    // A `toString` on the specifier that throws rejects too — §13.3.10 step 6 — rather than
    // throwing where the `import()` was written.
    assert_eq!(
        after_jobs(
            &[],
            "globalThis.out = 'never settled'; \
             var threw = 'no'; \
             try { \
               import({ toString: function () { throw new Error('from toString'); } }) \
                 .catch(function (e) { globalThis.out = threw + ':' + e.message; }); \
             } catch (e) { threw = 'yes'; }"
        ),
        "no:from toString"
    );
    // With no loader at all, an `import()` rejects rather than throwing — which is what a host that
    // cannot load a module is supposed to do, and is the behaviour an embedder gets for free.
    let mut heap = Heap::new();
    let script = crate::parser::parse_script(
        "globalThis.out = 'never settled'; \
         import('anything').catch(function (e) { globalThis.out = String(e.message); });",
    )
    .expect("parses"); // the test is about the missing loader
    let chunk = crate::compile::compile_script(&script, &mut heap).expect("compiles"); // same
    let read = crate::parser::parse_script("String(globalThis.out)").expect("parses"); // same
    let reader = crate::compile::compile_script(&read, &mut heap).expect("compiles"); // same
    let mut vm = Vm::new(&mut heap);
    vm.run(&chunk, &mut heap).expect("makes sense"); // same
    let outcome = vm.run(&reader, &mut heap).expect("makes sense"); // same
    assert!(
        describe(outcome, &mut heap).contains("no module loader"),
        "a machine with no loader rejects and says so"
    );
}

#[test]
fn resolving_an_export_remembers_the_name_it_asked_as_well_as_the_module() {
    // §16.2.1.6.3 step 1's `resolveSet` holds **pairs**. A module already asked for *this* name is
    // a cycle and answers nothing; the same module asked for a *different* name is an ordinary step
    // and must carry on. Remembering only the module stops a re-export a module makes of its own
    // export, which is what this is.
    assert_eq!(
        run_graph(
            &[
                (
                    "self",
                    "var v = 5; export { v as inner }; export { inner as outer } from 'self';"
                ),
                ("main", "import { outer } from 'self'; outer"),
            ],
            "main"
        ),
        "5"
    );
}

#[test]
fn two_star_exports_that_reach_the_same_binding_by_different_names_are_not_ambiguous() {
    // §16.2.1.6.3 step 6.c.ii — ambiguity is about the *resolution*, not about the path: two stars
    // that land on one binding agree, however differently they got there. Both paths have to reach
    // the last module asking for a different name, or the `resolveSet` above stops the second and
    // there is nothing to compare.
    assert_eq!(
        run_graph(
            &[
                ("base", "var v = 5; export { v as thing, v as shared };"),
                ("left", "export { thing as shared } from 'base';"),
                ("right", "export * from 'base';"),
                ("middle", "export * from 'left'; export * from 'right';"),
                ("main", "import { shared } from 'middle'; shared"),
            ],
            "main"
        ),
        "5"
    );
    // The same for a resolution that is a whole **module** rather than a binding: §16.2.1.10
    // memoises one namespace per module, so two paths to it are the same object and agree.
    assert_eq!(
        run_graph(
            &[
                ("base", "export var a = 1;"),
                ("holder_a", "export * as inner from 'base';"),
                ("holder_b", "export * as other from 'base';"),
                ("left", "export { inner as shared } from 'holder_a';"),
                ("right", "export { other as shared } from 'holder_b';"),
                ("middle", "export * from 'left'; export * from 'right';"),
                (
                    "main",
                    "import { shared } from 'middle'; import * as direct from 'base'; \
                     (shared === direct) + ':' + shared.a"
                ),
            ],
            "main"
        ),
        "true:1"
    );
}

/// A loader that resolves a specifier against its referrer, the way a filesystem host must.
///
/// The table is keyed by a *path-shaped* key, and a relative specifier is joined onto the
/// referrer's directory — which is the whole of what DR-0020's referrer parameter buys. Written
/// out here rather than assumed, because the interesting case is two directories writing the same
/// specifier and meaning different files.
struct Relative {
    /// Key to source, as a host's filesystem would answer.
    files: Vec<(String, String)>,
}

impl crate::vm::ModuleLoader for Relative {
    fn load(
        &mut self,
        referrer: Option<&str>,
        specifier: &str,
        heap: &mut Heap,
    ) -> Result<(String, std::rc::Rc<crate::compile::Chunk>), String> {
        // `a/index.js` importing `./thing.js` is `a/thing.js`. No `..` here: the rows below do not
        // need it and a resolution algorithm is the host's business, not this test's.
        let key = match (referrer, specifier.strip_prefix("./")) {
            (Some(from), Some(rest)) => match from.rsplit_once('/') {
                Some((directory, _)) => format!("{directory}/{rest}"),
                None => rest.to_string(),
            },
            _ => specifier.to_string(),
        };
        let source = self
            .files
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, source)| source.clone())
            .ok_or_else(|| format!("nothing is at {key:?}"))?;
        let parsed = crate::parser::parse_module(&source)
            .map_err(|error| format!("{key:?} did not parse: {}", error.kind))?;
        let compiled = crate::compile::compile_module(&parsed, heap).map_err(|e| e.message())?;
        Ok((key, std::rc::Rc::new(compiled)))
    }
}

#[test]
fn two_directories_may_write_the_same_specifier_and_mean_different_modules() {
    // DR-0020. `./thing.js` in `a/index.js` and in `b/index.js` are two files, and before the
    // referrer reached the loader there was nowhere to say so: the second overwrote the first and
    // both imports read the same module. Not an error — a wrong value, which is the shape this
    // engine treats as worst.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(Relative {
        files: [
            ("a/thing.js", "export const who = 'a';"),
            ("b/thing.js", "export const who = 'b';"),
            (
                "a/index.js",
                "import { who } from './thing.js'; export const from = who;",
            ),
            (
                "b/index.js",
                "import { who } from './thing.js'; export const from = who;",
            ),
            (
                "main.js",
                "import { from as a } from './a/index.js'; \
                 import { from as b } from './b/index.js'; a + b",
            ),
        ]
        .into_iter()
        .map(|(name, source)| (name.to_string(), source.to_string()))
        .collect(),
    }));
    // Nothing is supplied up front: the entry itself comes through the loader, which is the shape
    // a host that discovers its program is in.
    let outcome = vm
        .run_module_graph("main.js", &crate::vm::Graph::new(), &mut heap)
        .expect("the chunks are well formed") // a VM test needs an outcome
        .expect("the graph links"); // same
    assert_eq!(describe(outcome, &mut heap), "ab");
}

#[test]
fn a_module_reached_by_two_names_is_still_evaluated_once() {
    // §16.2.1.6's "each body once" is a fact about the **key** and never about the text — which is
    // the invariant DR-0020 states and the one the old flat map could not promise. `a/index.js`
    // and `./index.js` written from `a/` resolve to one key, so the body runs once however many
    // spellings reach it.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(Relative {
        files: [
            (
                "a/shared.js",
                "globalThis.runs = (globalThis.runs || 0) + 1; export const n = 1;",
            ),
            (
                "a/one.js",
                "import { n } from './shared.js'; export const one = n;",
            ),
            (
                "a/two.js",
                "import { n } from './shared.js'; export const two = n;",
            ),
            (
                "main.js",
                "import { one } from './a/one.js'; import { two } from './a/two.js'; \
                 one + two + globalThis.runs",
            ),
        ]
        .into_iter()
        .map(|(name, source)| (name.to_string(), source.to_string()))
        .collect(),
    }));
    let outcome = vm
        .run_module_graph("main.js", &crate::vm::Graph::new(), &mut heap)
        .expect("the chunks are well formed") // a VM test needs an outcome
        .expect("the graph links"); // same
    // 1 + 1 + one evaluation of the shared module.
    assert_eq!(describe(outcome, &mut heap), "3");
}

#[test]
fn a_supplied_module_still_has_its_own_imports_fetched() {
    // The two shapes DR-0020 says coexist, in one program: the host hands over the entry and the
    // loader answers for everything under it. A module already *resolved* is not one already
    // *walked* — and getting that wrong is not a slow path but a broken one, because the queue
    // empties after the first step and the entry's very first import reads as unresolved.
    let mut heap = Heap::new();
    let parsed = crate::parser::parse_module("import { n } from './dep.js'; n + 1")
        .expect("the source parses"); // a VM test needs a chunk
    let chunk = crate::compile::compile_module(&parsed, &mut heap).expect("it compiles"); // same
    let mut graph = crate::vm::Graph::new();
    graph.insert("a/main.js", std::rc::Rc::new(chunk));
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(Relative {
        files: vec![("a/dep.js".to_string(), "export const n = 41;".to_string())],
    }));
    let outcome = vm
        .run_module_graph("a/main.js", &graph, &mut heap)
        .expect("the chunks are well formed") // a VM test needs an outcome
        .expect("the graph links"); // same
    // …and the loader resolved `./dep.js` against `a/main.js`, so it found `a/dep.js` and not a
    // `dep.js` at the root, which is the referrer doing its work.
    assert_eq!(describe(outcome, &mut heap), "42");
}

#[test]
fn a_host_that_supplies_everything_is_never_asked_for_a_loader() {
    // The other half of the same sentence: `load_reachable` skips what is already resolved, so a
    // complete graph walks to an empty queue and never reaches the `no module loader is set`
    // refusal. Without this row that refusal would fire for every host that predates DR-0020.
    assert_eq!(
        run_graph(
            &[
                ("dep", "export const n = 20;"),
                ("main", "import { n } from 'dep'; n + 22"),
            ],
            "main"
        ),
        "42"
    );
}

#[test]
fn import_meta_is_one_ordinary_object_per_module() {
    // §13.3.12 — an object with a **null** prototype, made once and answered with every time. The
    // null prototype is step 4.a's `OrdinaryObjectCreate(null)` and is not decoration: a host
    // property called `toString` must not read as `Object.prototype`'s.
    assert_eq!(run_module_source("typeof import.meta"), "object");
    assert_eq!(
        run_module_source("Object.getPrototypeOf(import.meta) === null"),
        "true"
    );
    // Not callable and not a constructor, which is what makes "ordinary object" the whole of what
    // it is.
    assert_eq!(
        run_module_source(
            "var said = 'none'; try { import.meta() } catch (e) { said = e.constructor.name } said"
        ),
        "TypeError"
    );
    // Step 5 — the same object on every read, including from inside a function, because step 4
    // caches it on the module record rather than building one per evaluation.
    assert_eq!(
        run_module_source(
            "var a = import.meta; var b = function () { return import.meta }(); \
             (import.meta === a) + '|' + (import.meta === b)"
        ),
        "true|true"
    );
    // It is extensible, which is what lets a host or a script put something on it.
    assert_eq!(
        run_module_source("import.meta.mine = 7; import.meta.mine"),
        "7"
    );
}

#[test]
fn import_meta_belongs_to_the_module_the_code_was_written_in() {
    // The half that cannot be answered by asking what is running. §10.2.1.1 gives a call its
    // **callee's** `[[ScriptOrModule]]`, so a function declared in one module and called from
    // another answers with the module it was *written* in. praxis walks out of the environment the
    // code is closed over, and a closure's chain ends where it was written — the same fact.
    assert_eq!(
        run_graph(
            &[
                (
                    "dep",
                    "export var mine = import.meta; export function ours() { return import.meta }"
                ),
                (
                    "main",
                    "import { mine, ours } from 'dep'; \
                     (import.meta === mine) + '|' + (mine === ours())"
                ),
            ],
            "main",
        ),
        "false|true"
    );
}

#[test]
fn a_modules_import_meta_survives_a_collection() {
    // A root-set omission is the shape that no ordinary test can reach: leaving `import_meta` out
    // of `Vm::roots` changes nothing at all until a collection happens between two reads, and then
    // §13.3.12's one promise — the same object every time — is broken silently.
    //
    // The second module is not decoration. §14.2.2's completion register is itself a root, so an
    // object a module has just answered with is reachable through it; running something else first
    // is what makes this measure the record rather than the register.
    let mut heap = Heap::new();
    let first = crate::parser::parse_module("import.meta").expect("it parses"); // a VM test needs a chunk
    let first = crate::compile::compile_module(&first, &mut heap).expect("it compiles"); // same
    let second = crate::parser::parse_module("0").expect("it parses"); // same
    let second = crate::compile::compile_module(&second, &mut heap).expect("it compiles"); // same

    let mut vm = Vm::new(&mut heap);
    let outcome = vm.run_module(&first, &mut heap).expect("it runs"); // same
    let Outcome::Value(Value::Object(meta)) = outcome else {
        panic!("import.meta answered with {outcome:?}"); // a VM test needs the object
    };
    vm.run_module(&second, &mut heap).expect("it runs"); // a VM test needs a chunk
    vm.collect(&second, &mut heap);
    assert!(
        heap.object(meta).is_some(),
        "the module record no longer keeps its import.meta alive"
    );
}

#[test]
fn a_module_the_host_ran_directly_is_not_run_again_when_a_graph_reaches_it() {
    // §16.2.1.6's "each body once" is a fact about the *machine*, not about the link — so a module
    // the host ran with `Vm::run_module` has run, and a graph that later imports the same chunk
    // finds it evaluated. That is what the record `run_module` registers is for, and it is the only
    // thing that makes the two entry points agree about a module they have both seen.
    //
    // The counter is the whole test: a second evaluation would leave 2 behind, and the import would
    // still answer — with the wrong number, silently.
    let mut heap = Heap::new();
    // The counter has to live somewhere the body does not reset. `var runs = 0` looks like one and
    // is not: a second evaluation runs that initialiser too, so it answers 1 whether the body ran
    // once or twice — which is how the first version of this row passed against the bug it was
    // written to catch.
    let source = "export var runs = globalThis.seen = (globalThis.seen || 0) + 1;";
    let parsed = crate::parser::parse_module(source).expect("it parses"); // a VM test needs a chunk
    let dependency = std::rc::Rc::new(
        crate::compile::compile_module(&parsed, &mut heap).expect("it compiles"), // same
    );
    let importer =
        crate::parser::parse_module("import { runs } from 'dep'; runs").expect("it parses"); // same
    let importer = crate::compile::compile_module(&importer, &mut heap).expect("it compiles"); // same

    let mut vm = Vm::new(&mut heap);
    vm.run_module(&dependency, &mut heap).expect("it runs"); // same

    let mut graph = crate::vm::Graph::new();
    graph.insert("dep", std::rc::Rc::clone(&dependency));
    graph.insert("main", std::rc::Rc::new(importer));
    let outcome = vm
        .run_module_graph("main", &graph, &mut heap)
        .expect("the chunks are well formed") // same
        .expect("the graph links"); // same
    assert_eq!(describe(outcome, &mut heap), "1");
}

#[test]
fn a_graph_survives_a_collection_taken_between_two_of_its_modules() {
    // DR-0023's root set over a module graph, which is a claim a script never has to make. A graph
    // is several compiled bodies run one after another, so while the first executes the ones that
    // have not started are reachable from nothing the collector walks — and their constant tables
    // are Strings. This forces a collection at every check, which is the only setting that shows it:
    // with a sensible threshold the graph finishes before one is ever due.
    //
    // It came back `undefined` before `Vm::roots` walked `self.resolved`, because `main`'s `'c'`
    // had been freed while `dep` was still running.
    let awaiting = &[
        (
            "dep",
            "globalThis.log = 'a;'; await Promise.resolve(); globalThis.log += 'b;';              export var r = true;",
        ),
        (
            "main",
            "import { r } from 'dep'; globalThis.log += 'c'; globalThis.log",
        ),
    ];
    let plain = &[
        ("dep", "globalThis.log = 'a;'; export var r = true;"),
        (
            "main",
            "import { r } from 'dep'; globalThis.log += 'c'; globalThis.log",
        ),
    ];
    // Every setting, because the two failures this found were different: an uninitialised window
    // made a graph collect at the *first* check whatever the threshold, and the missing root only
    // ever showed at a threshold small enough to collect mid-evaluation.
    for growth in [None, Some(1usize << 20), Some(0usize)] {
        assert_eq!(
            run_graph_collecting(awaiting, "main", growth),
            "a;b;c",
            "top-level await at {growth:?}"
        );
        assert_eq!(
            run_graph_collecting(plain, "main", growth),
            "a;c",
            "no await at {growth:?}"
        );
    }
}
