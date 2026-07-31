//! Taking an execution out of the interpreter and putting it back — DR-0017's parked frame.
//!
//! Every test here builds its chunks by hand, and that is not a shortcut. Nothing compiles a
//! suspension yet: `yield` and `await` arrive in later slices, and the machinery under both is
//! worth having tested before either has a grammar pointed at it. So these are the same shape as
//! the fault tests in [`super`] — a chunk no compiler produces, handed to the VM to see what it
//! does with it.
//!
//! The body of a "generator" here is therefore a `Chunk::from_parts` installed as a function's
//! code, and the object it parks into is an ordinary object. What is being tested is the
//! interpreter, and the interpreter cannot tell the difference.

use super::*;
use crate::compile::Chunk;
use std::rc::Rc;

/// A function whose body is `code` with `constants`, closing over nothing.
///
/// The prototype is `Object.prototype` and no test reads it; what matters is that the object is
/// callable and that `enter` will push a frame for it, which is the thing a suspension needs.
fn function(vm: &Vm, heap: &mut Heap, code: Vec<Instruction>, constants: Vec<Value>) -> ObjectId {
    let body = Rc::new(Chunk::from_parts(code, constants));
    let environment = heap.new_environment(None, 0);
    heap.new_function(vm.realm().object_prototype(), body, environment, None)
}

/// What an Error object was built with, if `thrown` is one holding a string message.
///
/// Read from the property rather than through `String(e)`, because describing a thrown object
/// needs a `toString` call and there is no machine to make it from here.
fn message(heap: &mut Heap, thrown: ObjectId) -> Option<String> {
    let PropertyKind::Data {
        value: Value::String(text),
        ..
    } = own(heap, thrown, "message")?.kind
    else {
        return None;
    };
    Some(String::from_utf16_lossy(heap.string(text)?))
}

#[test]
fn a_parked_execution_carries_on_where_it_left_off() {
    // The whole of the slice in one chunk. The body pushes the object it will park in and the
    // value it is answering with, suspends, and — when it is put back — returns whatever was sent
    // into it. So `1` is what the call sees and `41` is what the revival sends, and adding them is
    // one assertion that both halves went the right way.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::Return,
        ],
        vec![Value::Object(holder), Value::Number(1.0)],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Revive,
            Instruction::Binary(BinaryOperator::Add),
            Instruction::SetCompletion,
        ],
        vec![
            Value::Object(body),
            Value::Object(holder),
            Value::Number(41.0),
        ],
    );
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "42");
}

#[test]
fn a_revival_resumes_the_expression_the_suspension_was_in_the_middle_of() {
    // A suspension is not a return: the operands the body had half-built are its own, and they
    // have to come back with it. Here `10 +` has been pushed and not yet applied when the
    // execution is parked, so the body answers `10 + 5` — a machine that dropped the stack slice
    // would answer `5`, and one that kept it on the *caller's* stack would unbalance the script.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Suspend,
            Instruction::Binary(BinaryOperator::Add),
            Instruction::Return,
        ],
        vec![
            Value::Number(10.0),
            Value::Object(holder),
            Value::Number(0.0),
        ],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::Pop,
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Revive,
            Instruction::SetCompletion,
        ],
        vec![
            Value::Object(body),
            Value::Object(holder),
            Value::Number(5.0),
        ],
    );
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "15");
}

#[test]
fn a_handler_the_parked_execution_installed_still_catches_at_a_different_depth() {
    // A `Handler` names an absolute depth in both stacks, and a revival almost never happens at
    // the depth the suspension did: here the script revives with one operand of its own already
    // pushed, so every mark is off by one. The `try` the body was inside has to survive that —
    // which is why the marks are stored relative to the frame's own floors and rebased on the way
    // back in. Without the rebasing the throw below lands somewhere else entirely.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![
            Instruction::PushHandler(6),
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::Throw,
            Instruction::Return,
            // The handler, which receives what was thrown and returns it.
            Instruction::Return,
        ],
        vec![Value::Object(holder), Value::Number(1.0)],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            // Kept on the stack across the revival, so that the revived frame's floor is one
            // higher than the suspended one's was.
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Revive,
            Instruction::Binary(BinaryOperator::Add),
            Instruction::SetCompletion,
        ],
        vec![
            Value::Object(body),
            Value::Object(holder),
            Value::Number(7.0),
        ],
    );
    // `1` from the suspension, and `7` thrown inside the revived body and caught by the handler it
    // installed before it ever suspended.
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "8");
}

#[test]
fn an_execution_can_only_be_revived_once() {
    // §27.5.1.2's state machine is not here yet, and this is what stands in for it: the suspension
    // is *moved* out of its holder, so a second revival finds nothing rather than running the same
    // frame twice. Two live copies of one execution would share an environment and disagree about
    // where they were.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::Return,
        ],
        vec![Value::Object(holder), Value::Number(1.0)],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::Pop,
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Revive,
            Instruction::Pop,
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Revive,
            Instruction::SetCompletion,
        ],
        vec![
            Value::Object(body),
            Value::Object(holder),
            Value::Number(0.0),
        ],
    );
    assert!(matches!(
        vm.run(&script, &mut heap),
        Err(Fault::NothingToRevive)
    ));
}

#[test]
fn reviving_something_that_never_suspended_is_a_fault() {
    // The two shapes that hold no execution, and they are one arm rather than two: a value that is
    // not an object at all, and an object that simply has nothing parked. Neither is reachable
    // from a compiled chunk, which is why they are faults and not TypeErrors.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let ordinary = heap.new_object(None);
    for holder in [Value::Object(ordinary), Value::Number(1.0)] {
        let script = Chunk::from_parts(
            vec![
                Instruction::Constant(0),
                Instruction::Constant(1),
                Instruction::Revive,
                Instruction::SetCompletion,
            ],
            vec![holder, Value::Number(0.0)],
        );
        assert!(matches!(
            vm.run(&script, &mut heap),
            Err(Fault::NothingToRevive)
        ));
    }
}

#[test]
fn a_suspension_needs_an_object_to_park_in() {
    // The mirror of the row above. Nothing else can hold an execution: a suspension has to go
    // somewhere the heap can trace, and a number is not somewhere.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let body = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(0),
            Instruction::Suspend,
            Instruction::Return,
        ],
        vec![Value::Number(1.0)],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::SetCompletion,
        ],
        vec![Value::Object(body)],
    );
    assert!(matches!(
        vm.run(&script, &mut heap),
        Err(Fault::NotAnObject)
    ));
}

#[test]
fn a_body_that_reaches_under_its_own_floor_is_parked_rather_than_panicked_on() {
    // Both of a frame's marks are where the callee's *own* things begin, and neither is enforced
    // while the callee runs: `Pop` answers for the stack as a whole and `PopHandler` for the
    // handlers, so a body may take what its caller left. This one takes both — it pops the
    // script's handler and then suspends with the script's two operands as its own — and the marks
    // end up above what is actually there.
    //
    // Splitting either stack at a mark past its end would panic, and DR-0002 does not allow a
    // chunk to decide that. The compiler emits no such body; a hand-written one is the only way
    // here, which is the whole reason [`Fault`] exists.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![Instruction::PopHandler, Instruction::Suspend],
        Vec::new(),
    );
    let script = Chunk::from_parts(
        vec![
            // Never taken — the callee takes it down before anything can throw.
            Instruction::PushHandler(6),
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Call(0),
            Instruction::SetCompletion,
        ],
        vec![
            Value::Object(holder),
            Value::Number(3.0),
            Value::Object(body),
        ],
    );
    // The suspension still answers with the value it was given, which is what says the machine
    // carried on rather than merely survived.
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "3");
}

#[test]
fn a_suspension_at_the_top_level_has_no_call_to_park() {
    // A suspension parks the running *function*, and a script is not one. The grammar says the
    // same thing — `yield` is only ever inside a `function*` — so this is a chunk no compiler
    // produces rather than a program anything could write.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::SetCompletion,
        ],
        vec![Value::Object(holder), Value::Number(1.0)],
    );
    assert!(matches!(
        vm.run(&script, &mut heap),
        Err(Fault::SuspendWithNoCall)
    ));
}

#[test]
fn a_suspension_may_not_cross_a_nested_execution() {
    // DR-0017, and the reason it is written down. `valueOf` is called from the *middle* of an
    // addition — a real Rust call, waiting for a value — so the frame the suspension would park
    // has that call underneath it. Parking it would leave that call with nothing to return to and
    // the revival would resume into a stack that is no longer there.
    //
    // The language never asks for this: a `yield` is only ever in the body of the `function*` that
    // owns it, and a coercion reaches JavaScript from a native whose own body is Rust. So the
    // check is the thing that keeps the next native with a callback from breaking it silently.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let suspends = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::Return,
        ],
        vec![Value::Object(holder), Value::Number(1.0)],
    );
    // An object whose `valueOf` is that function, so that `object + 0` reaches it through
    // §7.1.1's `ToPrimitive` — which is DR-0011's nested execution.
    let coerced = heap.new_object(Some(vm.realm().object_prototype()));
    let key = PropertyKey::from_units(&mut heap, &"valueOf".encode_utf16().collect::<Vec<_>>());
    heap.define_own_property(
        coerced,
        key,
        &PropertyDescriptor::data(Value::Object(suspends)),
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Binary(BinaryOperator::Add),
            Instruction::SetCompletion,
        ],
        vec![Value::Object(coerced), Value::Number(0.0)],
    );
    // The fault does not come back as one: a nested execution answers with a completion, so a
    // fault met inside it becomes a TypeError rather than escaping past the Rust call that is
    // waiting. Read by its message, because that is what says *which* fault — had the suspension
    // gone through, the addition would have read `1` as `valueOf`'s answer and the script would
    // have finished with it.
    let Ok(Outcome::Thrown(Value::Object(thrown))) = vm.run(&script, &mut heap) else {
        panic!("the conversion throws") // the test is about what it throws
    };
    assert_eq!(
        message(&mut heap, thrown).as_deref(),
        Some("the code of a conversion did not make sense")
    );
}

#[test]
fn a_suspension_inside_a_nested_execution_is_fine_if_something_is_under_it() {
    // The other half of DR-0017, and the reason the check is about one frame rather than about
    // nested executions as a whole. Here `valueOf` calls a function that suspends, so what the
    // park hands control back to is `valueOf` — an ordinary JavaScript frame the loop can carry
    // on in — and the coercion still gets a value `valueOf` really returned.
    //
    // `[1].map(() => gen.next())` is this shape, and it is an ordinary program. A check that
    // refused every suspension under a re-entry would refuse it.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let inner = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Suspend,
            Instruction::Return,
        ],
        vec![Value::Object(holder), Value::Number(4.0)],
    );
    let outer = function(
        &vm,
        &mut heap,
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::Return,
        ],
        vec![Value::Object(inner)],
    );
    let coerced = heap.new_object(Some(vm.realm().object_prototype()));
    let key = PropertyKey::from_units(&mut heap, &"valueOf".encode_utf16().collect::<Vec<_>>());
    heap.define_own_property(
        coerced,
        key,
        &PropertyDescriptor::data(Value::Object(outer)),
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Binary(BinaryOperator::Add),
            Instruction::SetCompletion,
        ],
        vec![Value::Object(coerced), Value::Number(3.0)],
    );
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "7");
}

#[test]
fn a_parked_execution_keeps_what_it_was_holding() {
    // The operands a suspension took with it are on no stack the collector can see, and its
    // environment is named by no frame — the frame that named it is the one that was parked. So
    // the holder is the only path to either, and a collector that did not walk it would free what
    // the execution is about to carry on with.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let holder = heap.new_object(None);
    let held = heap.new_object(None);
    let body = function(
        &vm,
        &mut heap,
        vec![
            // The object under test is pushed *first*, so it is an operand of the parked
            // execution rather than the value the suspension answers with.
            Instruction::Constant(0),
            Instruction::Constant(1),
            Instruction::Constant(2),
            Instruction::Suspend,
            Instruction::Pop,
            Instruction::Return,
        ],
        vec![
            Value::Object(held),
            Value::Object(holder),
            Value::Number(1.0),
        ],
    );
    let script = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Call(0),
            Instruction::SetCompletion,
        ],
        vec![Value::Object(body)],
    );
    assert_eq!(describe_run(&script, &mut vm, &mut heap), "1");
    let roots = crate::heap::Roots {
        values: vec![Value::Object(holder)],
        ..crate::heap::Roots::default()
    };
    heap.collect(&roots);
    assert!(heap.object(held).is_some());
}
