//! What the interpreter does, said as sentences about behaviour.
//!
//! Split by what each group is about. The helpers live here because every group needs them, and
//! so do the chunk-level tests: a malformed chunk is the one thing no source can produce, and
//! those are built by hand.
//!
//! - `values` — the operators.
//! - `statements` — control flow, and what a script evaluates to.
//! - `objects` — literals, properties, attributes.
//! - `builtins` — the objects a script can reach without making them.
//! - `coercion` — what an operator does when an operand is an object.
//! - `accessors` — a property whose value is a pair of functions.
//! - `arrays` — §10.4.2's exotic `length`, and the literal that makes one.
//! - `array_methods` — §23.1.3, and the two of §20.2.3 that reach it.
//! - `array_more` — the rest of §23.1.3: folding, quantifying, and moving elements.
//! - `objects_builtin` — §20.1's `Object`, and a property descriptor as a value.
//! - `functions` — calls, closures, `this`.
//! - `arrows` — §15.3, and the `this` an arrow does not bind.

mod accessors;
mod array_methods;
mod array_more;
mod arrays;
mod arrows;
mod builtins;
mod coercion;
mod functions;
mod objects;
mod objects_builtin;
mod statements;
mod values;

use super::call::MAX_CALL_DEPTH;
use super::*;
use crate::ast::BinaryOperator;
use crate::compile::{compile_expression, compile_script};
use crate::heap::{ObjectId, PropertyKey, PropertyKind};
use crate::parser::{parse_expression, parse_script};

/// Evaluate `source` and describe the result the way `String(x)` would, so that a row of a
/// test reads as the JavaScript it is about.
fn eval(source: &str) -> String {
    let mut heap = Heap::new();
    let expression = parse_expression(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_expression(&expression, &mut heap).expect("the source compiles"); // same
    let outcome = Vm::new(&mut heap)
        .run(&chunk, &mut heap)
        .expect("the chunk is well formed"); // same
    describe(outcome, &mut heap)
}

/// Run a whole script and describe its completion value the way `String(x)` would.
fn run(source: &str) -> String {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let outcome = Vm::new(&mut heap)
        .run(&chunk, &mut heap)
        .expect("the chunk is well formed"); // same
    describe(outcome, &mut heap)
}

/// The property `object` files under `name`, if it has one of its own.
///
/// Own rather than inherited, and a whole `Property` rather than a value, because several of
/// §17's rules are about *attributes* and nothing in the language can read one yet.
fn own(heap: &mut Heap, object: ObjectId, name: &str) -> Option<crate::heap::Property> {
    let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
    heap.object(object)?.get_own_property(key).copied()
}

/// The outcome, written the way `String(x)` would write it — with a thrown one marked, so
/// that a test row saying `"thrown 1"` cannot be confused with one saying `"1"`.
fn describe(outcome: Outcome, heap: &mut Heap) -> String {
    let (prefix, value) = match outcome {
        Outcome::Value(value) => ("", value),
        Outcome::Thrown(value) => ("thrown ", value),
    };
    // A thrown *object* has no `toString` to call yet, so writing it down would throw again.
    // Naming it by its type is enough for a test row to say which error it was, and it stops
    // one describing failure from failing.
    let Ok(id) = value.to_string(heap) else {
        return format!("{prefix}[{}]", value.type_of(heap));
    };
    format!(
        "{prefix}{}",
        String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
    )
}

#[test]
fn a_chunk_that_does_not_make_sense_is_a_fault_and_not_a_panic() {
    // The three ways a chunk can be wrong, each reached by handing the VM one no compiler
    // would produce. A script cannot get here; a compiler bug can, and DR-0002 is a promise
    // about *any* input rather than about correct ones.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);

    let underflow = Chunk::from_parts(vec![Instruction::Binary(BinaryOperator::Add)], Vec::new());
    assert!(matches!(
        vm.run(&underflow, &mut heap),
        Err(Fault::StackUnderflow)
    ));
    let one_short = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::Binary(BinaryOperator::Add),
        ],
        vec![Value::Null],
    );
    assert!(matches!(
        vm.run(&one_short, &mut heap),
        Err(Fault::StackUnderflow)
    ));

    let missing = Chunk::from_parts(vec![Instruction::Constant(7)], Vec::new());
    assert!(matches!(
        vm.run(&missing, &mut heap),
        Err(Fault::MissingConstant)
    ));

    // A jump past the end, including the placeholder an unpatched one carries — which is the
    // shape a compiler bug would actually take.
    let far = Chunk::from_parts(vec![Instruction::Jump(99)], Vec::new());
    assert!(matches!(
        vm.run(&far, &mut heap),
        Err(Fault::JumpOutOfRange)
    ));
    let unpatched = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::JumpKeeping(ShortCircuit::WhenTruthy, u32::MAX),
        ],
        vec![Value::Boolean(true)],
    );
    let _ = &unpatched;
    assert!(matches!(
        vm.run(&unpatched, &mut heap),
        Err(Fault::JumpOutOfRange)
    ));
    // …while a jump to exactly the end is how every short circuit finishes, and is fine.
    let to_the_end = Chunk::from_parts(
        vec![
            Instruction::Constant(0),
            Instruction::SetCompletion,
            Instruction::Jump(3),
        ],
        vec![Value::Boolean(true)],
    );
    assert!(matches!(
        vm.run(&to_the_end, &mut heap),
        Ok(Outcome::Value(Value::Boolean(true)))
    ));
    // A short circuit that has to peek at an empty stack is an underflow like any other.
    let nothing_to_peek = Chunk::from_parts(
        vec![Instruction::JumpKeeping(ShortCircuit::WhenFalsy, 1)],
        Vec::new(),
    );
    assert!(matches!(
        vm.run(&nothing_to_peek, &mut heap),
        Err(Fault::StackUnderflow)
    ));
    let nothing_to_pop = Chunk::from_parts(vec![Instruction::Pop], Vec::new());
    assert!(matches!(
        vm.run(&nothing_to_pop, &mut heap),
        Err(Fault::StackUnderflow)
    ));
    let nothing_to_test = Chunk::from_parts(vec![Instruction::JumpIfFalse(1)], Vec::new());
    assert!(matches!(
        vm.run(&nothing_to_test, &mut heap),
        Err(Fault::StackUnderflow)
    ));

    // Two values pushed and nothing to join them, and a chunk that pushed none at all.
    let leftover = Chunk::from_parts(
        vec![Instruction::Constant(0), Instruction::Constant(0)],
        vec![Value::Null],
    );
    assert!(matches!(
        vm.run(&leftover, &mut heap),
        Err(Fault::UnbalancedStack)
    ));
    // An *empty* chunk is not a fault — it is an empty script, whose completion value is
    // `undefined`.
    let empty = Chunk::from_parts(Vec::new(), Vec::new());
    assert!(matches!(
        vm.run(&empty, &mut heap),
        Ok(Outcome::Value(Value::Undefined))
    ));

    // A slot the frame does not have, in both directions.
    let no_such_slot = Chunk::from_parts(vec![Instruction::LoadVariable(0, 3)], Vec::new());
    assert!(matches!(
        vm.run(&no_such_slot, &mut heap),
        Err(Fault::MissingLocal)
    ));
    let nowhere_to_store = Chunk::from_parts(
        vec![Instruction::Constant(0), Instruction::StoreVariable(0, 3)],
        vec![Value::Null],
    );
    assert!(matches!(
        vm.run(&nowhere_to_store, &mut heap),
        Err(Fault::MissingLocal)
    ));
    let nothing_to_store = Chunk::from_parts(vec![Instruction::StoreVariable(0, 0)], Vec::new());
    assert!(matches!(
        vm.run(&nothing_to_store, &mut heap),
        Err(Fault::StackUnderflow)
    ));
    let nothing_to_complete = Chunk::from_parts(vec![Instruction::SetCompletion], Vec::new());
    assert!(matches!(
        vm.run(&nothing_to_complete, &mut heap),
        Err(Fault::StackUnderflow)
    ));

    // …and the machine still works afterwards, which is the other half of the claim: a fault
    // is about the chunk, not about the interpreter.
    let sound = Chunk::from_parts(
        vec![Instruction::Constant(0), Instruction::SetCompletion],
        vec![Value::Null],
    );
    assert!(matches!(
        vm.run(&sound, &mut heap),
        Ok(Outcome::Value(Value::Null))
    ));
}

#[test]
fn a_deeply_nested_expression_does_not_grow_the_rust_stack() {
    // The reason for bytecode, seen from the other side: the tree is nested a thousand deep
    // and the interpreter's loop is flat, so this costs a thousand stack *slots* rather than
    // a thousand Rust frames. The parser's own limit (DR-0006) is what bounds the tree.
    let source = format!("{}1{}", "(".repeat(60), ")".repeat(60));
    assert_eq!(eval(&source), "1");
    let sum = std::iter::repeat_n("1", 500)
        .collect::<Vec<_>>()
        .join(" + ");
    assert_eq!(eval(&sum), "500");
}

/// Run `source` on a heap that has already been given `filled` bytes of DR-0013's budget.
///
/// The filling is one String of the right length rather than a million objects: nothing ever reads
/// its units, so the operating system may never have to back them, and a test about a 64 MiB
/// budget costs neither 64 MiB of writing nor a second of allocating.
fn run_with_heap_filled_to(filled: usize, source: &str) -> String {
    let mut heap = Heap::new();
    heap.new_string(vec![0; filled / size_of::<u16>()]);
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let outcome = Vm::new(&mut heap)
        .run(&chunk, &mut heap)
        .expect("the chunk is well formed"); // same
    describe(outcome, &mut heap)
}

#[test]
fn a_script_that_allocates_without_end_is_stopped_rather_than_allowed_to_exhaust_the_machine() {
    // DR-0013. Before this, `while (true) { ({}); }` was an input that took the *process* down —
    // an allocation failure in Rust aborts, so nothing catches it and nothing gets to report it.
    // DR-0002 has no answer for an abort, which is why the engine stops first.
    //
    // Started just under the budget so the loop only needs a few thousand objects to cross it.
    // A test that began from an empty heap would allocate the whole 64 MiB to prove the same
    // thing, and would take a second to do it.
    let nearly = crate::heap::MAX_HEAP_BYTES - (1 << 20);
    assert_eq!(
        run_with_heap_filled_to(
            nearly,
            "try { while (true) { ({}); } } catch (e) { e.name; }"
        ),
        "RangeError"
    );
    // The other three shapes a runaway takes, because each grows a different arena: an object per
    // pass, a String per pass, and an environment per pass.
    for body in [
        "'' + i;",
        "var o = {}; o.a = 1;",
        "(function () { var v = 1; return v; })();",
    ] {
        let source = format!(
            "var i = 0; try {{ while (true) {{ i = i + 1; {body} }} }} catch (e) {{ e.name; }}"
        );
        assert_eq!(
            run_with_heap_filled_to(nearly, &source),
            "RangeError",
            "running {body:?}"
        );
    }
    // A loop that allocates *nothing* is not stopped — it is not the thing the budget is about,
    // and stopping it would be an engine that gave up on `while (true) { i = i + 1; }`.
    assert_eq!(
        run_with_heap_filled_to(nearly, "var i = 0; while (i < 100000) { i = i + 1; } i;"),
        "100000"
    );
}

#[test]
fn the_heap_budget_is_checked_again_after_it_is_caught() {
    // The bug this pins, which the first implementation had: resetting the countdown *after* the
    // throw rather than before it left it at zero, so every following pass raised the error again
    // — and raising it allocates the Error object, so the guard against a runaway became one.
    //
    // A script that catches and carries on must therefore still make progress, and must be told
    // again when it keeps allocating rather than either spinning or falling silent.
    let nearly = crate::heap::MAX_HEAP_BYTES - (1 << 20);
    assert_eq!(
        run_with_heap_filled_to(
            nearly,
            "var caught = 0;
             for (var round = 0; round < 3; round = round + 1) {
                 try { while (true) { ({}); } } catch (e) { caught = caught + 1; }
             }
             caught;"
        ),
        "3"
    );
}
