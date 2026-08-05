//! The root set — what a running program can still name, checked against the collector.
//!
//! §9's execution contexts are what a collector has to be told about, and there is no way to work
//! them out from the heap: the machine holds Values in a stack, in registers, in frames, in a job
//! queue and in a table of template objects, and the chunks it is running hold Strings in their
//! constant tables.
//!
//! # Why this is tested apart from the collector's own tests
//!
//! [`crate::heap::collect`]'s tests build a root set by hand and prove that what it names survives.
//! That is the *sweeper's* contract. This is the other half and the one that fails silently: a
//! collection that runs with an incomplete root set frees something a later instruction is going to
//! read, and nothing about it looks wrong until a value comes back as the wrong thing entirely.
//!
//! Every test here runs a program, collects with [`Vm::roots`] as the root set, and then asks the
//! program to keep going. A missed root shows up as a wrong answer or a fault, not as a leak.

use super::*;

/// Run `setup`, collect with the machine's own root set, then evaluate `after` on the same machine.
///
/// The collection happens *between* two chunks that share a heap, which is the only place a test
/// can put one: the interpreter does not collect while it runs, so a value that the root set
/// forgot has to be reached for afterwards.
fn survives(setup: &str, after: &str) -> String {
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);

    let script = parse_script(setup).expect("the setup parses"); // the test is what survives
    let chunk = compile_script(&script, &mut heap).expect("the setup compiles"); // same
    vm.run(&chunk, &mut heap).expect("the setup runs"); // same

    // What an embedder asks for between two pieces of work, with the chunk that was running —
    // whose constant table holds every String the setup mentioned.
    vm.collect(&chunk, &mut heap);

    let script = parse_script(after).expect("the question parses"); // same
    let asked = compile_script(&script, &mut heap).expect("the question compiles"); // same
    describe_run(&asked, &mut vm, &mut heap)
}

#[test]
fn a_global_and_everything_it_reaches_survives_a_collection() {
    // The plainest claim there is, and the one that fails first if the global object or the
    // realm's intrinsics are not roots.
    assert_eq!(
        survives("var kept = { a: 1, b: 'two' };", "kept.a + kept.b"),
        "1two"
    );
    assert_eq!(survives("var kept = [1, [2, [3]]];", "kept[1][1][0]"), "3");
}

#[test]
fn an_intrinsic_nothing_has_reached_yet_survives_a_collection() {
    // The case the realm's ceiling exists for, and the only one that shows it: most intrinsics are
    // properties of the global object and would survive through it, so a root set that forgot the
    // realm entirely still passes every test above. `%GeneratorPrototype%` is different — nothing
    // in a program that has never written `function*` reaches it — and the realm keeps its
    // identity in a field it will hand to the *next* generator made. Swept, that field addresses
    // an empty slot and the generator is built on no prototype at all: `it.next` is `undefined`
    // rather than anything failing.
    assert_eq!(
        survives(
            "var nothing = 1;",
            "function* g() { yield 'later'; } typeof g().next"
        ),
        "function"
    );
    // …and it is the same generator prototype, not a fresh one — which is what `g().next` finding
    // its way to `%GeneratorPrototype%` two hops up actually means.
    assert_eq!(
        survives(
            "var nothing = 1;",
            "function* g() { yield 'later'; } g().next().value"
        ),
        "later"
    );
}

#[test]
fn a_closure_keeps_the_environment_it_was_written_in() {
    // §10.2.11's environment is reachable from the function object and from nothing else once the
    // call that made it has returned. A collector told about the stack but not about what the
    // objects on it point at would free the variable this closure is about to read.
    assert_eq!(
        survives(
            "function make(n) { var secret = n * 2; return function () { return secret; }; } \
             var f = make(21);",
            "f()"
        ),
        "42"
    );
    // The body itself is a chunk, and its constants are Strings. Reading one *after* the
    // collection is what proves the constant table was traced: the literal below exists nowhere
    // else, since the function that mentions it has not been called yet.
    assert_eq!(
        survives(
            "var f = function () { return 'a string only the body mentions'; };",
            "f()"
        ),
        "a string only the body mentions"
    );
}

#[test]
fn a_bound_function_keeps_its_target_and_its_arguments() {
    // §10.4.1's exotic object names three things nothing else does — the function it stands in
    // front of, the receiver it was given and the arguments it holds — and the collector reaches
    // them through the *callable* rather than through any property.
    assert_eq!(
        survives(
            "function add(a, b) { return this.base + a + b; } \
             var bound = add.bind({ base: 100 }, 20);",
            "bound(3)"
        ),
        "123"
    );
}

#[test]
fn a_suspended_generator_keeps_the_body_it_is_going_to_carry_on_in() {
    // A parked execution holds its own operand stack, its registers and its *code*, and after the
    // frames that ran it are gone it is the only thing pointing at any of them. The String below
    // is in the body's constant table and is reached for after the collection.
    assert_eq!(
        survives(
            "function* g() { yield 1; yield 'the second one'; } var it = g(); it.next();",
            "it.next().value"
        ),
        "the second one"
    );
    // An `async` function parks into a context object of its own, which is not a property of
    // anything a script can name — the promise the caller holds is the only way back to it.
    assert_eq!(
        survives(
            "var seen = 'pending'; async function f() { return 'settled'; } \
             f().then(function (v) { seen = v; });",
            "seen"
        ),
        "settled"
    );
}

#[test]
fn a_queued_job_keeps_what_it_is_going_to_run_with() {
    // §9.5's queue is emptied by `run`, so a job survives a collection only when the collection
    // happens with jobs still on it — which is what a `then` registered on an already-settled
    // promise arranges. The handler is a function nothing else names by the time it runs.
    assert_eq!(
        survives(
            "var seen = 'never'; Promise.resolve('the value').then(function (v) { seen = v; });",
            "seen"
        ),
        "the value"
    );
}

#[test]
fn a_bigint_is_kept_while_something_names_it_and_freed_when_nothing_does() {
    // §6.1.6.2's magnitude is the program's to size, so a BigInt nothing names is worth reclaiming
    // for the same reason a String is — and unlike a String it is never interned, so the collector
    // is the only thing that can.
    assert_eq!(
        survives("var kept = 2n ** 200n;", "kept === 2n ** 200n"),
        "true"
    );
    // …and the other direction, which is what makes the first one mean something: a value the
    // program has let go of does come back.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script("var kept = 1n; var dropped = 2n ** 200n; dropped = null;")
        .expect("the setup parses"); // the test is about what is freed
    let chunk = compile_script(&script, &mut heap).expect("the setup compiles"); // same
    vm.run(&chunk, &mut heap).expect("the setup runs"); // same
    let freed = vm.collect(&chunk, &mut heap);
    assert!(
        freed.bigints > 0,
        "the dropped BigInt should have gone: {freed:?}"
    );
    // The one still named is still readable, digits and all.
    let script = parse_script("String(kept)").expect("the question parses"); // same
    let asked = compile_script(&script, &mut heap).expect("the question compiles"); // same
    assert_eq!(describe_run(&asked, &mut vm, &mut heap), "1");
}

#[test]
fn what_nothing_names_any_more_is_freed() {
    // The other direction, and it is what makes the tests above mean anything: a root set that
    // named *everything* would pass all of them and collect nothing. So this one asks the heap
    // whether the garbage went.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script("var kept = { a: 1 }; var dropped = { b: 2 }; dropped = null;")
        .expect("the setup parses"); // the test is about what is freed
    let chunk = compile_script(&script, &mut heap).expect("the setup compiles"); // same
    vm.run(&chunk, &mut heap).expect("the setup runs"); // same

    let before = heap.object_count();
    let freed = vm.collect(&chunk, &mut heap);
    assert!(
        freed.objects > 0,
        "the dropped object should have gone: {freed:?}"
    );
    assert!(heap.object_count() < before);
    // …and the one still named is still there.
    let script = parse_script("kept.a").expect("the question parses"); // same
    let asked = compile_script(&script, &mut heap).expect("the question compiles"); // same
    assert_eq!(describe_run(&asked, &mut vm, &mut heap), "1");
}

/// Run `source` twice on fresh machines, once with the loop collecting and once without, and
/// answer both results and both footprints.
fn with_and_without_a_schedule(source: &str) -> (String, String, usize, usize) {
    let mut answers = Vec::new();
    let mut footprints = Vec::new();
    for growth in [Some(0), None] {
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);
        vm.set_collection_growth(growth);
        let script = parse_script(source).expect("the source parses"); // the test is the answer
        let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
        answers.push(describe_run(&chunk, &mut vm, &mut heap));
        footprints.push(heap.footprint());
    }
    (
        answers[0].clone(),
        answers[1].clone(),
        footprints[0],
        footprints[1],
    )
}

#[test]
fn a_collection_at_every_check_changes_no_answer() {
    // The root set's contract, forced. `Some(0)` collects at **every** thousand-instruction check,
    // so anything the root set forgot is freed while the program that names it is still running —
    // and the answer changes, or a later instruction reads a slot that has been handed to somebody
    // else. Each of these touches a different kind of thing the machine holds outside the heap.
    for source in [
        // Strings from a chunk's constant table, concatenated across an allocation storm.
        "var s = ''; for (var i = 0; i < 3000; i++) { s = 'ab' + 'cd'; } s",
        // Closures over a per-iteration binding — the shape that retains most per pass.
        "var f; for (let i = 0; i < 3000; i++) { f = function () { return i } } f()",
        // Objects reachable only through another object's property.
        "var o = { deep: { n: 0 } }; \
         for (var i = 0; i < 3000; i++) { o.deep = { n: o.deep.n + 1 } } o.deep.n",
        // A generator, whose parked execution is not on the frame stack at all.
        "function* g() { var kept = 'held'; for (var i = 0; i < 500; i++) { yield i } return kept } \
         var it = g(); var last; for (var j = 0; j < 400; j++) { last = it.next().value } \
         last + ',' + it.next().value",
        // A job queue that outlives the statement that filled it.
        "var seen = []; for (var i = 0; i < 200; i++) { Promise.resolve(i).then(function (v) { \
             seen.push(v) }) } seen.length",
        // A `Map`'s keys, which the collector reaches through a collection rather than a property.
        "var m = new Map(); for (var i = 0; i < 2000; i++) { m.set('k' + (i % 7), { at: i }) } \
         m.size + ',' + m.get('k3').at",
        // Template objects, which the machine keeps per call site rather than per chunk.
        "function tag(parts) { return parts[0] } var t; \
         for (var i = 0; i < 2000; i++) { t = tag`held` } t",
    ] {
        let (scheduled, unscheduled, _, _) = with_and_without_a_schedule(source);
        assert_eq!(scheduled, unscheduled, "{source}");
    }
}

#[test]
fn a_schedule_stops_the_arena_growing_which_is_the_whole_point_of_it() {
    // DR-0019 makes a swept slot reusable, so a collection does not lower `footprint` — it stops
    // it *rising*. That is what this measures, and it is the difference between a program that
    // reaches DR-0013's budget and one that does not.
    // A closure over a per-iteration binding, which is the shape that retains most per pass — a
    // plain call retains too little for 20,000 of them to separate the two runs decisively, and
    // raising the count instead would make this the slowest test in the suite.
    let (scheduled, unscheduled, with, without) = with_and_without_a_schedule(
        "var f; for (let i = 0; i < 20000; i++) { f = function () { return i } } f()",
    );
    assert_eq!(scheduled, "19999");
    assert_eq!(unscheduled, "19999");
    // Not a ratio anybody should read as a promise — what is being asserted is the *direction*,
    // and that the gap is far outside anything noise could produce.
    //
    // A base of `Some(0)` is not "collect at every check for ever": after the first collection the
    // allowance becomes the live set, which is the proportional rule. So this measures a schedule
    // behaving as a schedule rather than one thrashing, and the gap is what it is worth.
    assert!(
        with * 2 < without,
        "collecting should hold the arena well below the run that never does: {with} against {without}"
    );
}
#[test]
fn a_body_compiled_at_run_time_survives_the_same_schedule() {
    // The chunks with no source file behind them, which are the ones a reader suspects first when
    // asking what the root set can reach. Each holds a String constant that appears nowhere else,
    // so a collection that lost the chunk's table would answer with something other than it.
    for source in [
        "eval(\"var t = ''; for (var i = 0; i < 4000; i++) { t = 'only-in-eval' } t\")",
        "var g = eval; g(\"var u = ''; for (var i = 0; i < 4000; i++) { u = 'indirect-only' } u\")",
        "function h() { return eval(\"var v = ''; for (var i = 0; i < 4000; i++) { v = 'in-a-function' } v\") } h()",
        "new Function(\"var w = ''; for (var i = 0; i < 4000; i++) { w = 'dynamic-fn' } return w\")()",
    ] {
        let (scheduled, unscheduled, _, _) = with_and_without_a_schedule(source);
        assert_eq!(scheduled, unscheduled, "{source}");
    }
}

#[test]
fn a_sort_with_a_comparator_survives_a_collection_in_the_middle_of_it() {
    // A built-in that re-enters the interpreter *and* holds a Rust-side working set while it does.
    // `Array.prototype.sort` reads the elements out, calls a comparator that runs a program, and
    // writes them back — so anything it is holding between those two moments is reachable only from
    // a Rust local, which is the one place a root set cannot look.
    let (scheduled, unscheduled, _, _) = with_and_without_a_schedule(
        "var a = []; for (var i = 0; i < 2048; i++) { a.push({ n: 'A' + i, r: i % 3 }) } \
         a.sort(function (x, y) { return x.r - y.r }); \
         a.length + ',' + a[0].r + ',' + a[2047].r + ',' + (typeof a[5].n)",
    );
    assert_eq!(scheduled, unscheduled);
    assert_eq!(scheduled, "2048,0,2,string");
}

#[test]
fn a_threshold_the_program_never_reaches_collects_nothing() {
    // The other side of the trigger's comparison. A base larger than everything the program
    // allocates must leave the arena exactly as an unscheduled run does — which is what says the
    // condition is a threshold rather than "collect whenever asked".
    let source = "var f; for (let i = 0; i < 2000; i++) { f = function () { return i } } f()";
    let mut footprints = Vec::new();
    for growth in [Some(usize::MAX), None] {
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);
        vm.set_collection_growth(growth);
        let script = parse_script(source).expect("the source parses"); // the test is the arena
        let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
        assert_eq!(describe_run(&chunk, &mut vm, &mut heap), "1999");
        footprints.push(heap.footprint());
    }
    assert_eq!(
        footprints[0], footprints[1],
        "a threshold nothing reaches must behave exactly as no threshold at all"
    );
}
