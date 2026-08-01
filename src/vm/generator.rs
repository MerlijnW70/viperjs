//! §15.5.4 and §27.5.1 — making a generator, and the three ways of resuming one.
//!
//! Two halves of one idea, and both live here because both are *entries into the loop* rather than
//! ordinary calls. Calling a generator function does not run its body: it binds the parameters,
//! makes the generator object, parks an execution that has not started, and answers with the
//! object. Resuming one does the reverse — and it has to be done by the interpreter's own `enter`
//! rather than by a built-in, because a built-in is Rust and DR-0017 says a suspension may not be
//! handed back to a Rust call that is waiting.

use super::call::Entry;
use super::suspend::Suspended;
use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{Heap, Object, Resumption};
use crate::value::{Abrupt, Value};
use std::collections::VecDeque;
use std::rc::Rc;

impl Vm {
    /// §15.5.4 `GeneratorStart` — the instruction, run inside the callee with its parameters done.
    ///
    /// Makes the object §27.5.1 describes, hands the half-run execution to it, and answers the
    /// *call* with it. Everything before this instruction has already happened in an ordinary
    /// frame, which is the whole point: `FunctionDeclarationInstantiation` is not part of the
    /// generator's body and must not be delayed until the first `next`.
    ///
    /// `asynchronous` picks §27.6.2's object instead — a different prototype, a different brand,
    /// and a request queue. Nothing else about starting one differs.
    pub(super) fn start_generator(
        &mut self,
        asynchronous: bool,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let Some(frame) = self.frames.last() else {
            return Err(Fault::YieldOutsideGenerator);
        };
        // §10.2.2's *active function object*, which is what §10.1.13 reads `prototype` off. A frame
        // running a generator body always has one: the only way to reach this instruction is a
        // call, and a call names what it called.
        let Some(function) = frame.function else {
            return Err(Fault::YieldOutsideGenerator);
        };
        // The three things §27.6.2 changes about §15.5.4, decided together. Separately they were
        // three conditions asking one question, and the two that only added state — the brand's
        // sibling and the queue — could each be mutated away without any program noticing.
        let (fallback, brand, queue) = match asynchronous {
            true => (
                self.realm.async_generator_prototype(),
                crate::heap::Suspendable::AsyncGenerator,
                // §27.6.1's `[[AsyncGeneratorQueue]]`, empty and present from the start: a queue
                // that appears on first use is a queue that can be missing.
                Some(crate::heap::Role::Requests(VecDeque::new())),
            ),
            false => (
                self.realm.generator_prototype(),
                crate::heap::Suspendable::Generator,
                None,
            ),
        };
        // §10.1.13 `GetPrototypeFromConstructor`, and it is a full `[[Get]]`: `g.prototype` is an
        // ordinary writable property, so a script may replace it and a getter or a proxy there may
        // throw. It throws *inside the callee*, which is where §15.5.4 puts it.
        let prototype = match self.prototype_for(function, fallback, heap) {
            Ok(prototype) => prototype,
            Err(error) => {
                self.raise(error, heap, chunk, current, at)?;
                return Ok(());
            }
        };
        let generator = heap.new_object(Some(prototype));
        heap.brand_suspendable(generator, brand);
        if let (Some(queue), Some(object)) = (queue, heap.object_mut(generator)) {
            object.set_role(queue);
        }
        // Said before the park, because the park reads it: this is the execution's own answer to
        // "which generator am I", and a `return` inside the body has only the frame to ask.
        if let Some(frame) = self.frames.last_mut() {
            frame.generator = Some(generator);
        }
        // Not begun — the parameters ran, the body has not. §27.5.1.3 step 5 turns on exactly that
        // difference, and it is why a park at a `yield` and a park here are not the same record.
        let parked = self.park(current, at)?.before_the_body();
        heap.park_into(generator, parked);
        // Where a call leaves its answer, the park having truncated to it.
        self.stack.push(Value::Object(generator));
        Ok(())
    }

    /// §27.5.1's `next`, `return` and `throw` — resume a generator, or say why not.
    ///
    /// The three differ only in the completion the body is resumed with, which is why they share a
    /// way in. What they have in common is everything else: the receiver must be a generator
    /// (§27.5.1.2 step 2's `RequireInternalSlot`), a generator that is running refuses to be
    /// resumed again, and a generator that has finished answers without running anything.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    pub(super) fn enter_resume(
        &mut self,
        kind: Resumption,
        how: Entry,
        callee_at: usize,
        receiver_at: usize,
        count: usize,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // §10.3.1 passes the receiver straight through, as it does for a built-in: `gen.next()`
        // has one and `var n = gen.next; n()` does not, which is why the second is a TypeError.
        let receiver = match how {
            Entry::Method => self.stack[receiver_at],
            Entry::Plain | Entry::Construct | Entry::Super | Entry::Named => Value::Undefined,
        };
        let sent = match count {
            0 => Value::Undefined,
            _ => self.stack[callee_at + 1],
        };
        self.stack.truncate(receiver_at);
        // §27.5.1.2 step 2's `RequireInternalSlot` — a brand and not a shape, so an ordinary
        // object with a `next` of its own is not one however similar it looks.
        let generator = match receiver {
            Value::Object(id) if heap.object(id).is_some_and(Object::is_generator) => id,
            _ => {
                self.raise(
                    Abrupt::type_error("what was resumed is not a generator"),
                    heap,
                    chunk,
                    current,
                    at,
                )?;
                return Ok(());
            }
        };
        // §27.5.1.2 step 4 — a generator already running cannot be resumed, and *running* is a
        // question about the frame stack rather than about a flag: the execution is not parked
        // anywhere to be resumed from, because a frame is in the middle of it. Asked by walking the
        // frames, which costs nothing worth measuring — a resumption is rare and the stack is a few
        // deep — and cannot be stale, which a flag was.
        if self
            .frames
            .iter()
            .any(|frame| frame.generator == Some(generator))
        {
            self.raise(
                Abrupt::type_error("a generator cannot be resumed while it is running"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        }
        // Everything else is decided by whether there is an execution to resume: a *completed*
        // generator is precisely one with none.
        let parked = heap.take_parked(receiver);
        let begun = parked.as_ref().is_some_and(Suspended::begun);
        let outcome = match (parked, kind) {
            // §27.5.3.2 `GeneratorResume` — the only path that runs any code.
            (Some(parked), Resumption::Next) => {
                // Cleared rather than assumed: a `return` that reached a suspension with no
                // `ResumeMode` after it would leave this set, and the next ordinary resumption
                // would read it at a `yield` that never asked.
                self.resume_returns = false;
                self.revive(parked, sent, receiver_at, current, at);
                return Ok(());
            }
            // §27.5.3.4 `GeneratorResumeAbrupt` with a throw completion: the `throw` happens *at
            // the `yield`*, so a `try` the body is inside catches it. Put the execution back and
            // then unwind, which is the same two steps a `throw` written at that line would be.
            //
            // §27.5.1.4 step 5 asks for something else when the body has **not** begun — complete
            // it without resuming — and this arm answers for that case too, because the two cannot
            // be told apart. Reviving executes nothing before the unwind below, and an execution
            // that has not begun carries no handlers, so the unwind passes straight through the
            // frame it just pushed and lands exactly where the other path would have thrown.
            //
            // A `Suspended::begun` flag used to select between them. It was a survivor, and the
            // reason offered for keeping it — that the recorded parameter-default fix would put
            // real code at instruction zero and make the distinction bite — was wrong: nothing at
            // instruction zero ever runs on this path, whatever it is.
            (Some(parked), Resumption::Throw) => {
                // `undefined` rather than the thrown value: the `yield` never evaluates to
                // anything, and the unwinding below discards whatever is above the handler's mark.
                self.revive(parked, Value::Undefined, receiver_at, current, at);
                self.unwind(sent, chunk, current, at)?;
                return Ok(());
            }
            // §27.5.3.4 with a *return* completion, where the body has begun: it is resumed **at
            // the `yield`**, so the `finally` blocks between there and the end run and the open
            // iterators are closed. The value rides in as an ordinary resumption and
            // `Instruction::ResumeMode` — which the compiler put right after that `yield` — says
            // what it meant. Only the compiler standing at the `yield` knows what lies between it
            // and the end of the body, which is why the shape is this way round.
            (Some(parked), Resumption::Return) if begun => {
                self.resume_returns = true;
                self.revive(parked, sent, receiver_at, current, at);
                return Ok(());
            }
            // §27.5.1.3 step 5 — and a `return` completes it without running anything.
            (Some(_), Resumption::Return) => Finish::Value(sent),
            // §27.5.1.2 step 5 — a finished generator answers `{ value: undefined, done: true }`
            // for ever, however many times it is asked.
            (None, Resumption::Next) => Finish::Value(Value::Undefined),
            // §27.5.1.3 step 4 — `return` on a finished one hands back what it was given, which is
            // the one way to get a `value` other than `undefined` out of a completed generator.
            (None, Resumption::Return) => Finish::Value(sent),
            // §27.5.1.4 step 4 — and `throw` throws, having nothing to intercept it.
            (None, Resumption::Throw) => Finish::Thrown(sent),
        };
        match outcome {
            Finish::Value(value) => {
                let result = self.iterator_result(heap, value, true);
                self.stack.push(result);
            }
            Finish::Thrown(value) => self.raise(Abrupt::Thrown(value), heap, chunk, current, at)?,
        }
        Ok(())
    }

    /// §7.4.13 `CreateIterResultObject` — `{ value, done }`, ordinary in every way.
    ///
    /// Here rather than borrowed from `builtins::iterator`, because the two callers of this one are
    /// instructions: a generator's `return` wraps its answer, and so does every path above that
    /// does not run any code. Neither is a built-in and neither can throw, which is the difference
    /// from the version beside the array iterators.
    pub(crate) fn iterator_result(&mut self, heap: &mut Heap, value: Value, done: bool) -> Value {
        let object = heap.new_object(Some(self.realm.object_prototype()));
        for (name, held) in [("value", value), ("done", Value::Boolean(done))] {
            let name = crate::heap::PropertyKey::from_units(
                heap,
                &name.encode_utf16().collect::<Vec<_>>(),
            );
            let _ = heap.define_own_property(
                object,
                name,
                &crate::heap::PropertyDescriptor {
                    value: Some(held),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..crate::heap::PropertyDescriptor::EMPTY
                },
            );
        }
        Value::Object(object)
    }
}

/// What a resumption that runs no code answers with.
///
/// Two shapes because §27.5.1's three methods have two: `next` and `return` answer with an iterator
/// result, and `throw` throws. Named rather than handled in each arm so that the wrapping is
/// written once — three copies of `CreateIterResultObject` is three chances to pass the wrong
/// `done`.
enum Finish {
    /// Wrap this in `{ value, done: true }` and answer with it.
    Value(Value),
    /// Throw this.
    Thrown(Value),
}
