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
use crate::heap::{Heap, Object, ObjectId, Resumption};
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    /// §15.5.4 `EvaluateGeneratorBody` — make the generator object and run nothing.
    ///
    /// Reached once the environment is built and the arguments are in their slots, because that is
    /// the order the clause has: `FunctionDeclarationInstantiation` first, then the object, then
    /// `GeneratorStart`. A parameter's default expression therefore runs when the generator
    /// function is *called* and not when it is first resumed, which is observable and is why the
    /// order is worth stating.
    ///
    /// Nothing is pushed onto the frame stack. The execution that would have been a frame is
    /// parked from the start, at instruction zero with an empty operand stack — which is exactly
    /// what a frame the loop has not run yet would have looked like.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    pub(super) fn enter_generator(
        &mut self,
        body: Rc<Chunk>,
        function: ObjectId,
        receiver: Value,
        new_target: Value,
        environment: crate::heap::EnvironmentId,
        receiver_at: usize,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // §10.1.13 `GetPrototypeFromConstructor` with %GeneratorPrototype% as the fallback, and it
        // is a full `[[Get]]`: `g.prototype` is an ordinary writable property, so a script may
        // replace it, and a getter or a proxy there may throw.
        let prototype = match self.prototype_for(function, self.realm.generator_prototype(), heap) {
            Ok(prototype) => prototype,
            Err(error) => {
                self.raise(error, heap, chunk, current, at)?;
                return Ok(());
            }
        };
        let generator = heap.new_object(Some(prototype));
        heap.brand_suspendable(generator, crate::heap::Suspendable::Generator);
        let parked =
            Suspended::started(body, environment, receiver, new_target, function, generator);
        // The object was just made, so it is an object and this cannot answer `false`.
        let _ = heap.park_into(Value::Object(generator), parked);
        // Where a call leaves its answer: the callee and its arguments go, and the generator takes
        // their place. From here it is an ordinary value that happens to hold an execution.
        self.stack.truncate(receiver_at);
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
        // generator is precisely one with none, and one that has begun is told from one that has
        // not by the execution itself.
        let parked = heap.take_parked(receiver);
        let begun = parked.as_ref().is_some_and(Suspended::begun);
        let outcome = match (parked, kind) {
            // §27.5.3.2 `GeneratorResume` — the only path that runs any code.
            (Some(parked), Resumption::Next) => {
                self.revive(parked, sent, receiver_at, current, at);
                return Ok(());
            }
            // §27.5.3.4 `GeneratorResumeAbrupt` with a throw completion, where the body has begun:
            // the `throw` happens *at the `yield`*, so a `try` the body is inside catches it. The
            // execution is put back and then unwound, which is the same two steps a `throw`
            // written at that line would have been.
            (Some(parked), Resumption::Throw) if begun => {
                // `undefined` rather than the thrown value: the `yield` never evaluates to
                // anything, and the unwinding below discards whatever is above the handler's mark.
                self.revive(parked, Value::Undefined, receiver_at, current, at);
                self.unwind(sent, chunk, current, at)?;
                return Ok(());
            }
            // §27.5.1.4 step 5 — before the body has begun there is no `try` it could have entered,
            // so the generator is finished and the value travels out unchanged.
            (Some(_), Resumption::Throw) => Finish::Thrown(sent),
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
    pub(super) fn iterator_result(&mut self, heap: &mut Heap, value: Value, done: bool) -> Value {
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
