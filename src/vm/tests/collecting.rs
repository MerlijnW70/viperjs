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
