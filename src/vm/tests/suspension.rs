//! DR-0017's parked execution — what a suspension keeps, and where one may not happen.
//!
//! The mechanism these are about is [`crate::vm`]'s `park` and `revive`, but they are written as
//! JavaScript because there is now JavaScript that reaches it: a `yield` is a park and a `next` is
//! a revival. What is being checked is not the generator API — that is next door in `generators` —
//! but the things a parked *execution* has to carry: its operands, its handlers, its position, and
//! the objects only it still names.
//!
//! Two rows are still hand-written chunks. A `Yield` outside a generator is a chunk no compiler
//! produces, and it is the one shape of this that no source text can reach.

use super::*;
use crate::compile::Chunk;
use std::rc::Rc;

#[test]
fn a_revival_resumes_the_expression_the_suspension_was_in_the_middle_of() {
    // A suspension is not a return: the operands the body had half-built are its own, and they
    // have to come back with it. `10 +` has been pushed and not yet applied when the execution is
    // parked, so this answers `10 + 5` — a machine that dropped the stack slice would answer `5`,
    // and one that left it on the *caller's* stack would unbalance the script.
    assert_eq!(
        run("function* g() { return 10 + (yield 1); } var it = g(); it.next(); it.next(5).value"),
        "15"
    );
    // Nested a little deeper, so that more than one operand is in flight: `1 + (2 * (…))` has two
    // half-built expressions waiting, and both have to survive.
    assert_eq!(
        run(
            "function* g() { return 1 + 2 * (yield 0); } var it = g(); it.next(); it.next(4).value"
        ),
        "9"
    );
    // …and the locals go with it, which is the environment rather than the stack — a different
    // half of the same record.
    assert_eq!(
        run(
            "function* g() { var a = 1; yield 0; a += 2; return a; } var it = g(); it.next(); it.next().value"
        ),
        "3"
    );
}

#[test]
fn a_handler_the_parked_execution_installed_still_catches_after_the_revival() {
    // A `Handler` names an absolute depth in both stacks, and a revival almost never happens at
    // the depth the suspension did. The `try` the body was inside has to survive that, which is
    // why the marks are stored relative to the frame's own floors and rebased on the way back in.
    // Without the rebasing the throw below lands somewhere else entirely.
    assert_eq!(
        run(
            "function* g() { try { yield 1; throw 7; } catch (e) { return e; } } var it = g(); it.next(); it.next().value"
        ),
        "7"
    );
    // Revived from inside an expression that has operands of its own, so every mark really is off
    // by something rather than by nothing.
    assert_eq!(
        run(
            "function* g() { try { yield 1; throw 7; } catch (e) { return e; } } var it = g(); it.next(); 100 + it.next().value"
        ),
        "107"
    );
    // A `finally` runs on the way out too, and it is the same handler machinery.
    assert_eq!(
        run(
            "var seen = ''; function* g() { try { yield 1; } finally { seen = 'ran'; } } var it = g(); it.next(); it.next(); seen"
        ),
        "ran"
    );
}

#[test]
fn a_suspension_is_not_a_return_and_a_return_is_not_a_suspension() {
    // The two leave the same shape behind and differ in exactly one thing: whether the execution
    // is kept. A body that yields and then returns is asked twice and answers differently, which
    // is what says the first answer did not end it.
    assert_eq!(
        run(
            "function* g() { yield 1; return 2; } var it = g(); var a = it.next(); var b = it.next(); var c = it.next(); [a.value + ':' + a.done, b.value + ':' + b.done, c.value + ':' + c.done].join(' ')"
        ),
        "1:false 2:true undefined:true"
    );
}

#[test]
fn a_parked_execution_keeps_what_it_was_holding() {
    // The operands a suspension took with it are on no stack the collector can see, and its
    // environment is named by no frame — the frame that named it is the one that was parked. So
    // the generator is the only path to either, and a collector that did not walk it would free
    // what the execution is about to carry on with.
    //
    // Written as a collection with the generator as the only root, because that is the question:
    // not whether the values are correct, but whether they are still there.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script(
        "var held = { tag: 'kept' }; function* g(o) { var local = o; return [local, (yield 1)]; } var it = g(held); it.next(); held = null; it;",
    )
    .expect("the source parses"); // the test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let Ok(Outcome::Value(Value::Object(generator))) = vm.run(&chunk, &mut heap) else {
        panic!("the script answers with the generator") // the test is about that object
    };
    let roots = crate::heap::Roots {
        values: vec![Value::Object(generator)],
        ..crate::heap::Roots::default()
    };
    heap.collect(&roots);
    // The only remaining reference to the object is the parked execution's parameter slot, and
    // resuming has to find it there.
    let after = parse_script("it.next(2).value[0].tag").expect("the probe parses"); // same
    let chunk = compile_script(&after, &mut heap).expect("the probe compiles"); // same
    assert_eq!(describe_run(&chunk, &mut vm, &mut heap), "kept");
}

#[test]
fn a_yield_outside_a_generator_is_a_chunk_that_does_not_make_sense() {
    // The one shape of this no source text can reach: the parser refuses `yield` outside a
    // generator body, so the only way here is a chunk written by hand. Two of them, because there
    // are two ways to have no generator — no frame at all, and a frame belonging to an ordinary
    // function.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let top_level = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Yield,
            Instruction::SetCompletion,
        ],
        vec![Value::Number(1.0)],
    );
    assert!(matches!(
        vm.run(&top_level, &mut heap),
        Err(Fault::YieldOutsideGenerator)
    ));

    let body = Rc::new(Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Yield,
            Instruction::Return,
        ],
        vec![Value::Number(1.0)],
    ));
    let environment = heap.new_environment(None, 0);
    let ordinary = heap.new_function(vm.realm().object_prototype(), body, environment, None);
    let calls = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::SetCompletion,
        ],
        vec![Value::Object(ordinary)],
    );
    assert!(matches!(
        vm.run(&calls, &mut heap),
        Err(Fault::YieldOutsideGenerator)
    ));
}
