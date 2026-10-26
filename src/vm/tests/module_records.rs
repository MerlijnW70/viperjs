//! §16.2.1.4's module record — one per module, and kept only as long as the module's code can run.
//!
//! DR-0033. The machine filed each record under the address of the chunk it was made for and never
//! let one go, which made two mistakes of one: a chunk compiled where a freed one had been was told
//! it was that module, and every module a machine ever ran was kept until the machine went. Every
//! expected value here that a program computes was computed once with V8 (Node 22.22.1), importing
//! the same sources as `data:` URLs into one realm — a module per distinct source, which is what a
//! chunk per compile is here.

use super::*;
use std::rc::Rc;

/// Compile `source` as a Module.
fn module(source: &str, heap: &mut Heap) -> Chunk {
    let parsed = crate::parser::parse_module(source).expect("the source parses"); // a VM test needs a chunk
    crate::compile::compile_module(&parsed, heap).expect("the source compiles") // same
}

/// Run `chunk` as a module on a machine that already exists, and describe what it came to.
fn run_module_on(chunk: &Chunk, vm: &mut Vm, heap: &mut Heap) -> String {
    let outcome = vm
        .run_module(chunk, heap)
        .expect("the chunk is well formed"); // the test is about what it evaluates to
    describe(outcome, heap)
}

/// Run a script on the same machine and describe what it came to — how a test reads the globals the
/// modules left behind.
fn ask(source: &str, vm: &mut Vm, heap: &mut Heap) -> String {
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, heap).expect("the source compiles"); // same
    describe_run(&chunk, vm, heap)
}

/// Link and evaluate `graph` from `entry`, and describe what the entry came to.
fn link(entry: &str, graph: &crate::vm::Graph, vm: &mut Vm, heap: &mut Heap) -> String {
    let outcome = vm
        .run_module_graph(entry, graph, heap)
        .expect("the chunks are well formed") // same
        .expect("the graph links"); // same
    describe(outcome, heap)
}

/// A module that records its own `import.meta` and counts its own evaluation.
fn own_meta(who: &str) -> String {
    format!(
        "var m = import.meta; m.who = '{who}'; (globalThis.metas = globalThis.metas || []).push(m); \
         globalThis.runs = (globalThis.runs || 0) + 1; m.who"
    )
}

#[test]
fn two_module_chunks_run_one_after_another_from_one_local_are_two_modules() {
    // The case DR-0033 is about, where reuse is certain rather than likely: one local, so the
    // second chunk is moved into exactly the bytes the first one had. Filed by address, the second
    // module
    // found the first one's record — evaluated, with an environment its body was not running in —
    // and its `import.meta` was `Fault::ImportMetaOutsideModule`.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let mut chunk = module(&own_meta("first"), &mut heap);
    let mut answers = vec![run_module_on(&chunk, &mut vm, &mut heap)];
    chunk = module(&own_meta("second"), &mut heap);
    answers.push(run_module_on(&chunk, &mut vm, &mut heap));
    // …and running the second again is §16.2.1.6's `Evaluate` of a module already evaluated: the
    // same module, so its body does not run a second time.
    answers.push(run_module_on(&chunk, &mut vm, &mut heap));
    assert_eq!(answers, ["first", "second", "undefined"]);
    assert_eq!(
        ask(
            "[metas.length, metas[0] !== metas[1], metas[0].who, metas[1].who, runs].join()",
            &mut vm,
            &mut heap,
        ),
        "2,true,first,second,2"
    );
}

#[test]
fn three_thousand_module_chunks_in_a_loop_are_three_thousand_modules() {
    // Every other chunk is a local of the loop body, which reuses its bytes on every pass, and the
    // rest are in an `Rc` dropped at the end of the pass, which an allocator hands straight to the
    // next one of the same size. Each body records which pass it was; a record found again would
    // skip a body, share an object, or answer the wrong pass.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let mut wrong = Vec::new();
    for pass in 0..3000 {
        let source = format!(
            "var m = import.meta; m.n = {pass}; (globalThis.metas = globalThis.metas || []).push(m); \
             globalThis.runs = (globalThis.runs || 0) + 1; m.n"
        );
        let answer = match pass % 2 {
            0 => {
                let chunk = module(&source, &mut heap);
                run_module_on(&chunk, &mut vm, &mut heap)
            }
            _ => {
                let chunk = Rc::new(module(&source, &mut heap));
                run_module_on(&chunk, &mut vm, &mut heap)
            }
        };
        if answer != pass.to_string() {
            wrong.push((pass, answer));
        }
    }
    assert!(wrong.is_empty(), "{wrong:?}");
    assert_eq!(
        ask(
            "[runs, new Set(metas).size, metas.every(function (m, k) { return m.n === k; })].join()",
            &mut vm,
            &mut heap,
        ),
        "3000,3000,true"
    );
}

#[test]
fn a_graph_sees_a_module_run_earlier_as_evaluated_only_when_it_is_the_same_module() {
    // Three imports of a module a host ran first. The first is a *different* module compiled into
    // the very allocation the one the host ran had — `Rc::get_mut` replaces the chunk in place —
    // and filed by address it was found evaluated, so its body was skipped and the importer was
    // bound to the first module's environment. The other two are the same module reached again,
    // by a second graph and after a lone run, and must not run it twice.
    let dependency = |who: &str| {
        format!(
            "export var runs = globalThis.runs = (globalThis.runs || 0) + 1; \
             export var who = '{who}';"
        )
    };
    let reads = |from: &str| format!("import {{ runs, who }} from '{from}'; who + ':' + runs");
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let mut held = Rc::new(module(&dependency("first"), &mut heap));
    run_module_on(&held, &mut vm, &mut heap);
    *Rc::get_mut(&mut held).expect("nothing else holds the chunk") =
        module(&dependency("second"), &mut heap);
    let mut answers = Vec::new();
    let mut graph = crate::vm::Graph::new();
    graph.insert("dep", Rc::clone(&held));
    graph.insert("main", Rc::new(module(&reads("dep"), &mut heap)));
    answers.push(link("main", &graph, &mut vm, &mut heap));
    let mut again = crate::vm::Graph::new();
    again.insert("again", Rc::new(module(&reads("dep"), &mut heap)));
    answers.push(link("again", &again, &mut vm, &mut heap));
    let third = Rc::new(module(&dependency("third"), &mut heap));
    run_module_on(&third, &mut vm, &mut heap);
    let mut after = crate::vm::Graph::new();
    after.insert("other", Rc::clone(&third));
    after.insert("last", Rc::new(module(&reads("other"), &mut heap)));
    answers.push(link("last", &after, &mut vm, &mut heap));
    assert_eq!(answers, ["second:2", "second:2", "third:3"]);
    assert_eq!(ask("runs", &mut vm, &mut heap), "3");
}

#[test]
fn a_long_series_of_module_runs_keeps_inside_the_heap_budget() {
    // The memory half of DR-0033, in DR-0013's units: two thousand modules, each holding a
    // 256-element Array on its `import.meta`, under four MiB, with the host collecting every twenty
    // runs. A module costs the arena about forty kilobytes, so a machine that kept every record
    // kept every Array and threw DR-0013's RangeError before the hundredth. Collected with their
    // code instead, what survives a collection does not depend on how many modules have come and
    // gone.
    let mut heap = Heap::new();
    heap.set_budget(1 << 22);
    let mut vm = Vm::new(&mut heap);
    let mut wrong = 0;
    let mut live = Vec::new();
    for pass in 0..2000 {
        let chunk = module(
            &format!(
                "import.meta.payload = new Array(256).fill(0); \
                 globalThis.runs = (globalThis.runs || 0) + 1; import.meta.payload.length // {pass}"
            ),
            &mut heap,
        );
        if run_module_on(&chunk, &mut vm, &mut heap) != "256" {
            wrong += 1;
        }
        if pass % 20 == 19 {
            vm.collect(&chunk, &mut heap);
            live.push((heap.object_count(), heap.environment_count()));
        }
    }
    assert_eq!(wrong, 0);
    assert_eq!(ask("runs", &mut vm, &mut heap), "2000");
    assert!(live.windows(2).all(|pair| pair[0] == pair[1]), "{live:?}");
}

#[test]
fn a_module_nothing_names_but_a_function_it_made_keeps_its_record() {
    // The other side of collecting a record with its code: a function the module made is code of
    // the module, and §10.2.1.1 gives it the module as its `[[ScriptOrModule]]`. The chunk is gone
    // and the collection is told only of the script about to run, so the record is reachable
    // through the function's environment and nothing else — and its `import.meta` object, which
    // nothing names at all, through the record.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    {
        let chunk = module(
            "import.meta.tag = 'kept'; globalThis.f = function () { return import.meta; };",
            &mut heap,
        );
        run_module_on(&chunk, &mut vm, &mut heap);
    }
    // A script in between, so that the machine's own registers — the environment it last ran in,
    // the completion value — no longer name the module either.
    ask("0", &mut vm, &mut heap);
    let script =
        parse_script("[typeof f(), f() === f(), f().tag].join()").expect("the source parses"); // a VM test needs a chunk
    let probe = compile_script(&script, &mut heap).expect("the source compiles"); // same
    vm.collect(&probe, &mut heap);
    assert_eq!(describe_run(&probe, &mut vm, &mut heap), "object,true,kept");
}

#[test]
fn a_module_that_threw_throws_the_same_value_when_run_again_even_after_a_collection() {
    // §16.2.1.6 step 9's `[[EvaluationError]]`, which a lone run records as a graph does. The
    // thrown object is named by the record alone once something else has run, so a collection in
    // between is what says the record keeps it.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let thrower = module(
        "globalThis.tries = (globalThis.tries || 0) + 1; throw { n: globalThis.tries };",
        &mut heap,
    );
    let first = vm
        .run_module(&thrower, &mut heap)
        .expect("the chunk is well formed"); // a VM test needs the outcome
    let Outcome::Thrown(Value::Object(thrown)) = first else {
        panic!("the module threw an object, not {first:?}"); // same
    };
    run_module_on(&module("0", &mut heap), &mut vm, &mut heap);
    vm.collect(&thrower, &mut heap);
    let again = vm
        .run_module(&thrower, &mut heap)
        .expect("the chunk is well formed"); // same
    assert!(matches!(again, Outcome::Thrown(Value::Object(same)) if same == thrown));
    assert!(heap.object(thrown).is_some());
    assert_eq!(ask("tries", &mut vm, &mut heap), "1");
}

#[test]
fn a_namespace_nothing_else_names_is_the_same_object_after_a_collection() {
    // §16.2.1.10's memo is on the record, and the namespace object is reachable through nothing
    // else here: the host asked for it and holds only the handle, which is not a root.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let dependency = Rc::new(module("export var n = 1;", &mut heap));
    let mut graph = crate::vm::Graph::new();
    graph.insert("dep", Rc::clone(&dependency));
    link("dep", &graph, &mut vm, &mut heap);
    let namespace = vm.module_namespace(&dependency, &mut heap);
    let other = module("0", &mut heap);
    run_module_on(&other, &mut vm, &mut heap);
    vm.collect(&other, &mut heap);
    assert!(heap.object(namespace).is_some());
    assert_eq!(vm.module_namespace(&dependency, &mut heap), namespace);
}

#[test]
fn a_module_a_specifier_answered_is_not_run_again_after_a_collection() {
    // The machine's memo of specifiers is what keeps a graph's modules: a host that let go of its
    // graph has still told the machine that `dep` is this module, and a later import of `dep` must
    // find it evaluated however many collections have happened since. Nothing else holds `dep`'s
    // environment once both graphs' entry modules have finished.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    {
        let mut graph = crate::vm::Graph::new();
        graph.insert(
            "dep",
            Rc::new(module(
                "globalThis.runs = (globalThis.runs || 0) + 1; export var n = 1;",
                &mut heap,
            )),
        );
        graph.insert(
            "main",
            Rc::new(module("import { n } from 'dep'; n", &mut heap)),
        );
        link("main", &graph, &mut vm, &mut heap);
    }
    let other = module("0", &mut heap);
    run_module_on(&other, &mut vm, &mut heap);
    vm.collect(&other, &mut heap);
    let mut later = crate::vm::Graph::new();
    later.insert(
        "later",
        Rc::new(module(
            "import { n } from 'dep'; n + globalThis.runs",
            &mut heap,
        )),
    );
    assert_eq!(link("later", &later, &mut vm, &mut heap), "2");
}

#[test]
fn a_chunk_the_collector_was_not_told_of_loses_its_module_and_then_has_one_again() {
    // The root set's contract, which DR-0032 states for a chunk's constants and template objects:
    // code held where the collector is not told of it is code it cannot keep anything for. So the
    // module goes, and running the chunk afterwards is a new module — which is then one module like
    // any other, run once however often it is run. No V8 value applies: V8 has no way to free a
    // module's code while a host still holds it, so the numbers are what that contract says.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let counted = module("globalThis.runs = (globalThis.runs || 0) + 1;", &mut heap);
    let mut answers = vec![run_module_on(&counted, &mut vm, &mut heap)];
    answers.push(run_module_on(&counted, &mut vm, &mut heap));
    let other = module("0", &mut heap);
    run_module_on(&other, &mut vm, &mut heap);
    vm.collect(&other, &mut heap);
    answers.push(run_module_on(&counted, &mut vm, &mut heap));
    answers.push(run_module_on(&counted, &mut vm, &mut heap));
    assert_eq!(answers, ["1", "undefined", "2", "undefined"]);
}

#[test]
fn one_module_chunk_is_one_module_whichever_realm_the_machine_is_running() {
    // The records are the machine's — one graph, linked once, however many realms it builds —
    // which is what the address-keyed map was too. DR-0033 names the machine by its *first*
    // realm's global object so that a host switching the running realm underneath it, as
    // `templates` does to give one chunk two template objects, still has one module per chunk. No
    // program can switch it, so V8 has nothing to say; the numbers are the machine's own contract.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm;
    let second = vm.create_realm(&mut heap);
    let counted = module("globalThis.runs = (globalThis.runs || 0) + 1;", &mut heap);
    let mut answers = vec![run_module_on(&counted, &mut vm, &mut heap)];
    vm.realm = second;
    answers.push(run_module_on(&counted, &mut vm, &mut heap));
    vm.realm = first;
    assert_eq!(answers, ["1", "undefined"]);
}

/// A host whose only `import.meta` property says how long the module's key is.
///
/// §16.2.1.9 is the host's, so there is no V8 value to compare with: the expected numbers are the
/// lengths of the keys the rows below register, which is what makes a key that went missing — or
/// one that was handed to the wrong module — visible as a number.
struct KeyLength;

impl crate::vm::ModuleLoader for KeyLength {
    fn load(
        &mut self,
        _referrer: Option<&str>,
        specifier: &str,
        _heap: &mut Heap,
    ) -> Result<(String, Rc<Chunk>), String> {
        Err(format!("nothing is loaded here, so not {specifier:?}"))
    }

    fn import_meta_properties(&mut self, key: &str) -> Vec<(String, Value)> {
        vec![("keyLength".to_string(), Value::Number(key.len() as f64))]
    }
}

#[test]
fn a_module_keeps_the_key_a_host_resolved_it_by_whichever_entry_point_reached_it_first() {
    // DR-0020's key is on the record now, so it has to arrive whichever way the module did. A
    // module the host ran on its own has none, and takes the one a graph later reaches it under; a
    // module a graph linked keeps its key when the host then runs it on its own, which is not a
    // resolution.
    let source = "export function keyed() { return import.meta.keyLength; }";
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(KeyLength));
    let alone = Rc::new(module(source, &mut heap));
    run_module_on(&alone, &mut vm, &mut heap);
    let mut graph = crate::vm::Graph::new();
    graph.insert("dep", Rc::clone(&alone));
    graph.insert(
        "main",
        Rc::new(module("import { keyed } from 'dep'; keyed()", &mut heap)),
    );
    let mut answers = vec![link("main", &graph, &mut vm, &mut heap)];
    let linked = Rc::new(module(source, &mut heap));
    let mut first = crate::vm::Graph::new();
    first.insert("other", Rc::clone(&linked));
    first.insert(
        "quiet",
        Rc::new(module(
            "import { keyed } from 'other'; globalThis.keyed = keyed; 0",
            &mut heap,
        )),
    );
    link("quiet", &first, &mut vm, &mut heap);
    answers.push(run_module_on(&linked, &mut vm, &mut heap));
    // Read from a script, so that no link walks the module again between the lone run and the read
    // and puts back a key the run might have taken away.
    answers.push(ask("keyed()", &mut vm, &mut heap));
    let mut second = crate::vm::Graph::new();
    second.insert(
        "loud",
        Rc::new(module("import { keyed } from 'other'; keyed()", &mut heap)),
    );
    answers.push(link("loud", &second, &mut vm, &mut heap));
    assert_eq!(answers, ["3", "undefined", "5", "5"]);
}
