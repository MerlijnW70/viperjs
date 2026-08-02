//! What the interpreter does, said as sentences about behaviour.
//!
//! Split by what each group is about. The helpers live here because every group needs them, and
//! so do the chunk-level tests: a malformed chunk is the one thing no source can produce, and
//! those are built by hand.
//!
//! - `values` — the operators.
//! - `wrapper` — §20.3's `Boolean`, §21.1's `Number`, and `ToObject`.
//! - `statements` — control flow, and what a script evaluates to.
//! - `objects` — literals, properties, attributes.
//! - `builtins` — the objects a script can reach without making them.
//! - `coercion` — what an operator does when an operand is an object.
//! - `accessors` — a property whose value is a pair of functions.
//! - `arrays` — §10.4.2's exotic `length`, and the literal that makes one.
//! - `array_methods` — §23.1.3, and the two of §20.2.3 that reach it.
//! - `array_more` — the rest of §23.1.3: folding, quantifying, and moving elements.
//! - `array_sort` — §23.1.3.30 and §23.1.3.34, and the order that compares spellings.
//! - `array_copy` — §23.1.3's change-copies, and the index that throws rather than clamps.
//! - `array_flat` — §23.1.3's flattening, and the nesting that would exhaust a stack.
//! - `annex_b` — §B.2.2's four accessor methods, and how they differ from a descriptor.
//! - `species` — §7.3.23, and what a copying method answers *with*.
//! - `set_ops` — §24.2.4's seven, and the size that decides which side is walked.
//! - `shared` — §25.2's buffer and §25.4's `Atomics`, and the two brands that refuse each other.
//! - `number_format` — §21.1.3's three spellings, and the two exactnesses they need.
//! - `iterator_helpers` — §27.1's `Iterator`, and the methods that consume one.
//! - `iterator_lazy` — §27.1.4's five that *make* one, and why nothing runs until asked.
//! - `objects_builtin` — §20.1's `Object`, and a property descriptor as a value.
//! - `functions` — calls, closures, `this`.
//! - `arrows` — §15.3, and the `this` an arrow does not bind.
//! - `constructors` — §7.3.13, and which functions `new` may be written in front of.
//! - `inheritance` — §15.7's `extends` and `super`, and a `this` that starts out unbound.
//! - `private` — §15.7's `#x`, which is not a property by any test a program can make.
//! - `names` — §10.2.9 and §8.6.3, and the positions that do *not* name a function.
//! - `strict` — §11.2.1, and the three places sloppy mode is silent where strict throws.
//! - `destructuring` — §14.3.3, and the default that is for `undefined` rather than for absence.
//! - `for_of` — §14.7.5.7, and the four ways out of a loop that have to close its iterator.
//! - `globals` — §19.2, and why `parseInt` is not `Number`.
//! - `iterators` — §27.1, and where an iterator has got to that no script can reach.
//! - `json` — §25.5, and the promise that what `stringify` wrote will parse back.
//! - `object_state` — §20.1.2's whole-object statics, and the coercion they start with.
//! - `parameters` — §15.1, and the three things a list that is not simple decides.
//! - `strings` — §22.1 and §10.4.3, and the object with a property per character.
//! - `symbols` — §6.1.5 and §20.4, and the one primitive whose identity is itself.
//! - `templates` — §13.2.8, and the conversion a substitution gets that `+` does not.
//! - `string_methods` — the rest of §22.1.3, and the four rules for taking a piece of a string.
//! - `arguments` — §10.4.4, and the map that makes an index and a parameter one variable.
//! - `lexical` — §14.3.1's `let` and `const`, and the temporal dead zone.
//! - `for_in` — §14.7.5's enumeration, and the shadowing that decides what it visits.
//! - `bound` — §20.2's `Function`, and §10.4.1's bound functions.
//! - `math` — §21.3, and the four places it is not what a CPU does.
//! - `weak` — §24.3 and §24.4, and the methods they deliberately do not have.
//! - `weak_ref` — §26.1 and §26.2, and the registration that would defeat itself.
//! - `suspension` — DR-0017's parked frame, out of chunks no compiler emits yet.
//! - `generators` — §15.5 and §27.5's object and its state machine.
//! - `yielding` — §15.5.5 and §27.5.3, where the body stops and what a resumption sends back.
//! - `delegating` — §27.5.3.7 step 7's `yield*`, and the messages it passes both ways.
//! - `asynchrony` — §15.8 and §27.7, where the promise stands in for the generator object.
//! - `for_await` — §14.7.5.7 and §27.1.4, and the adapter that fakes an async iterator.
//!
//! There was a `compile_error` helper here, for rows asserting that some construct is refused rather
//! than mis-compiled. **There are no such rows left in this module** — every one was removed by the
//! slice that implemented what it described, which is what a refusal test is for. Bring it back with
//! the next refusal, and put the row beside the feature rather than in a list of its own: a list of
//! refusals outlives the refusals it describes, and this one had to be shortened eight times.

mod accessors;
mod annex_b;
mod arguments;
mod array_copy;
mod array_flat;
mod array_methods;
mod array_more;
mod array_sort;
mod arrays;
mod arrows;
mod asynchrony;
mod bigints;
mod bound;
mod buffers;
mod builtins;
mod classes;
mod coercion;
mod collecting;
mod collections;
mod constructors;
mod date;
mod delegating;
mod destructuring;
mod for_await;
mod for_in;
mod for_of;
mod functions;
mod generators;
mod globals;
mod inheritance;
mod iterator_helpers;
mod iterator_lazy;
mod iterators;
mod json;
mod lexical;
mod math;
mod names;
mod number_format;
mod object_state;
mod objects;
mod objects_builtin;
mod parameters;
mod private;
mod promises;
mod proxy;
mod reflection;
mod regexp;
mod regexp_symbols;
mod set_ops;
mod shared;
mod species;
mod statements;
mod strict;
mod string_methods;
mod string_replace;
mod strings;
mod suspension;
mod symbols;
mod templates;
mod typed;
mod values;
mod weak;
mod weak_ref;
mod wrapper;
mod yielding;

use super::call::MAX_CALL_DEPTH;
use super::*;
use crate::ast::BinaryOperator;
use crate::compile::{Instruction, ShortCircuit, compile_expression, compile_script};
use crate::heap::{ObjectId, PropertyDescriptor, PropertyKey, PropertyKind};
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

/// Run a chunk on a machine that already exists, and describe what it came to.
///
/// For the chunks that are built by hand rather than compiled: those need a heap prepared in
/// advance — a function object holding the body under test, an object to park in — so the machine
/// and the heap come from the caller instead of being made here.
fn describe_run(chunk: &Chunk, vm: &mut Vm, heap: &mut Heap) -> String {
    let outcome = vm.run(chunk, heap).expect("the chunk is well formed"); // the test is about what it evaluates to
    describe(outcome, heap)
}

/// Whether a script gets as far as being a chunk at all.
///
/// For the early errors — the ones §22.2.1.1 and its neighbours make a property of the *text*
/// rather than of running it. A construct refused here is one no `try` can reach, and that is the
/// whole difference between an early error and an ordinary one.
fn compiles(source: &str) -> bool {
    let mut heap = Heap::new();
    parse_script(source).is_ok_and(|script| compile_script(&script, &mut heap).is_ok())
}

/// Run a script, let §9.5's jobs run, and describe what `probe` then evaluates to.
///
/// A script's completion value is decided by its last statement, which is *before* any job runs, so
/// nothing a `then` handler does can be seen through it. An embedder that wants to know asks
/// afterwards — which is what this is: a second script in the same realm, whose own completion
/// value is read. Both scripts share a global object, so a `var` the first one declared is a
/// property the second one reads.
fn run_settled(source: &str, probe: &str) -> String {
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let first = vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    if let Outcome::Thrown(_) = first {
        return describe(first, &mut heap);
    }
    let after = parse_script(probe).expect("the probe parses"); // same
    let chunk = compile_script(&after, &mut heap).expect("the probe compiles"); // same
    let outcome = vm.run(&chunk, &mut heap).expect("the probe is well formed"); // same
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
