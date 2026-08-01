//! §27.6 — the async generator, which is both of the other two at once and neither of them.
//!
//! A synchronous generator answers its resumption with an iterator result. An `async` function
//! answers its *one* caller with a promise. An async generator answers **every** resumption with a
//! promise of an iterator result, and that one difference is where all of §27.6 comes from: a
//! caller need not wait before asking again, so the asks have to be remembered.
//!
//! # The queue, and why it is not optional
//!
//! `gen.next()` returns immediately with a promise, so
//!
//! ```javascript
//! const a = gen.next(), b = gen.next();
//! ```
//!
//! has two requests outstanding against a body that has not reached its first `yield`. §27.6.3.2
//! `AsyncGeneratorEnqueue` puts both on a queue and they are served in order: `a` gets the first
//! yielded value and `b` the second. A synchronous generator cannot be asked this way — its `next`
//! either runs the body or throws — which is why it needs nothing like this.
//!
//! # No `[[AsyncGeneratorState]]`, because the queue already says it
//!
//! §27.6.1's state field has six values, and the two that matter here are `suspendedYield` — the
//! body is parked and may be resumed — and the awaiting states, where the body is parked and may
//! **not** be, because a job is going to resume it when a promise settles. Both are "parked", so a
//! naive look at the suspension cannot tell them apart, and resuming the second would run the body
//! from the middle of an `await`.
//!
//! The queue distinguishes them without a field. A request is taken off the queue exactly when the
//! body produces something for it — a `yield`, a `return`, a throw — and an `await` produces
//! nothing. So:
//!
//! - parked with an **empty** queue: nothing is in service, so this is the start or a `yield`, and
//!   it may be resumed;
//! - parked with a **non-empty** queue: the request at the front is mid-flight and the body is
//!   inside an `await` waiting for a job; a new ask is enqueued behind it and nothing is resumed;
//! - not parked, with a live frame naming it: the body is running, and the same applies;
//! - not parked, with no live frame: completed, for ever.
//!
//! This is the same argument [`super::generator`] makes for asking the frame stack rather than
//! keeping a flag, and it is here for the same reason: a stored state has to be *maintained*, and
//! the one place it will not be is the path nobody wrote a test for. See the note in `AGENTS.md`
//! about `[[GeneratorState]]` going stale in exactly that way.

use super::call::Entry;
use super::{Fault, Vm};
use crate::heap::{Heap, Object, ObjectId, ReactionKind, Request, Resumption, Role, Suspendable};
use crate::value::{Abrupt, Value};
use std::collections::VecDeque;
use std::rc::Rc;

impl Vm {
    /// §27.6.2's `AsyncGeneratorFunction` call — make the object and run none of the body.
    ///
    /// The synchronous twin of this is [`Vm::enter_generator`], and the two differ in exactly three
    /// things: the prototype, the brand, and that this one starts with an empty request queue.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    pub(super) fn enter_async_generator(
        &mut self,
        body: Rc<crate::compile::Chunk>,
        function: ObjectId,
        receiver: Value,
        new_target: Value,
        environment: crate::heap::EnvironmentId,
        receiver_at: usize,
        heap: &mut Heap,
        chunk: &crate::compile::Chunk,
        current: &mut Option<Rc<crate::compile::Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // §10.1.13 with %AsyncGeneratorPrototype% as the fallback, and a full `[[Get]]` for the
        // same reason the synchronous one is: `g.prototype` is an ordinary writable property.
        let prototype =
            match self.prototype_for(function, self.realm.async_generator_prototype(), heap) {
                Ok(prototype) => prototype,
                Err(error) => {
                    self.raise(error, heap, chunk, current, at)?;
                    return Ok(());
                }
            };
        let generator = heap.new_object(Some(prototype));
        heap.brand_suspendable(generator, Suspendable::AsyncGenerator);
        // §27.6.1's `[[AsyncGeneratorQueue]]`, empty. Set here rather than lazily so that every
        // async generator has one from the moment it exists — a queue that appears on first use is
        // a queue that can be missing, and the code that reads it would need a case for that.
        if let Some(object) = heap.object_mut(generator) {
            object.set_role(Role::Requests(VecDeque::new()));
        }
        let parked =
            super::Suspended::started(body, environment, receiver, new_target, function, generator);
        heap.park_into(generator, parked);
        self.stack.truncate(receiver_at);
        self.stack.push(Value::Object(generator));
        Ok(())
    }

    /// §27.6.1's `next`, `return` and `throw` — enqueue an ask, and serve it if nothing is ahead.
    ///
    /// Unlike §27.5.1's three, none of these can fail for being called at a bad moment: a generator
    /// that is running refuses a synchronous resumption with a TypeError, and an async one simply
    /// remembers the ask. The only rejection here is §27.6.1.2 step 3's — the receiver is not an
    /// async generator — and even that is a *rejected promise* rather than a thrown error, because
    /// the method's contract is to answer with a promise whatever happens.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    pub(super) fn enter_async_resume(
        &mut self,
        kind: Resumption,
        how: Entry,
        callee_at: usize,
        receiver_at: usize,
        count: usize,
        heap: &mut Heap,
        chunk: &crate::compile::Chunk,
        current: &mut Option<Rc<crate::compile::Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let receiver = match how {
            Entry::Method => self.stack[receiver_at],
            Entry::Plain | Entry::Construct | Entry::Super | Entry::Named => Value::Undefined,
        };
        let sent = match count {
            0 => Value::Undefined,
            _ => self.stack[callee_at + 1],
        };
        self.stack.truncate(receiver_at);
        let capability = self.intrinsic_capability(heap);
        // §27.6.1.2 step 3 — and it *rejects* rather than throwing, which is the difference from
        // the synchronous method that otherwise reads the same. A caller writing `gen.next()` on
        // the wrong object gets a promise it can `catch`, not an exception at the call.
        let generator = match receiver {
            Value::Object(id) if heap.object(id).is_some_and(Object::is_async_generator) => id,
            _ => {
                let error = self.thrown_value(
                    Abrupt::type_error("what was resumed is not an async generator"),
                    heap,
                );
                let _ = self.settle_capability(capability, ReactionKind::Reject, error, heap);
                self.stack.push(capability.promise);
                return Ok(());
            }
        };
        // Read *before* the enqueue: "was anything already in service" is the question, and after
        // pushing this request the answer would always be yes.
        let idle = self
            .requests_of(generator, heap)
            .is_none_or(VecDeque::is_empty);
        self.enqueue_request(
            generator,
            Request {
                kind,
                value: sent,
                capability,
            },
            heap,
        );
        // The call's answer, pushed **now** and not by whatever eventually stops the body. That is
        // the invariant the rest of §27.6 rests on: a resumption's value is known the moment it is
        // asked for, so the body may stop any number of times afterwards without owing anyone a
        // slot, and a request served out of the queue needs no caller at all.
        self.stack.push(capability.promise);
        // A body with a live frame is running, and a running body serves its own queue when it
        // next stops. Asked of the frame stack for the reason §27.5.1's running check is.
        let running = self
            .frames
            .iter()
            .any(|frame| frame.generator == Some(generator));
        if !idle || running {
            return Ok(());
        }
        match heap.take_parked(Value::Object(generator)) {
            // There is a body and nothing ahead of this request, so it is served now — above the
            // promise, which has already been pushed.
            Some(parked) => {
                let base = self.stack.len();
                self.serve(
                    parked, generator, kind, sent, base, heap, chunk, current, at,
                )
            }
            // §27.6.1's completed generator: no body, so the request is answered from the queue
            // without running anything.
            None => {
                self.drain(generator, heap);
                Ok(())
            }
        }
    }

    /// §27.6.3.8 step 10 — serve whatever was asked while the body was busy.
    ///
    /// Called where the body has just stopped at a `yield` and come off the front of the queue. If
    /// something is waiting behind it, that request is resumed straight away, at a slot above
    /// everything: its caller was handed a promise when it asked and is owed nothing here, so the
    /// body may stop again without leaving a value anybody would read.
    pub(super) fn serve_queued(
        &mut self,
        generator: ObjectId,
        heap: &mut Heap,
        chunk: &crate::compile::Chunk,
        current: &mut Option<Rc<crate::compile::Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let Some(request) = self.next_request(generator, heap) else {
            return Ok(());
        };
        let Some(parked) = heap.take_parked(Value::Object(generator)) else {
            // No body left to serve it with, which a `return` at the `yield` produces: the queue is
            // answered as a completed generator's is.
            self.drain(generator, heap);
            return Ok(());
        };
        let base = self.stack.len();
        self.serve(
            parked,
            generator,
            request.kind,
            request.value,
            base,
            heap,
            chunk,
            current,
            at,
        )
    }

    /// Put a body back and resume it with the completion the request at the front asked for.
    ///
    /// The three-way split [`Vm::enter_resume`] makes, minus the arm for a body that has not begun:
    /// a `return` before the first `next` is served by [`Vm::drain`] above, since the request is on
    /// the queue and the body will never produce anything for it.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    fn serve(
        &mut self,
        parked: super::Suspended,
        generator: ObjectId,
        kind: Resumption,
        sent: Value,
        base: usize,
        heap: &mut Heap,
        chunk: &crate::compile::Chunk,
        current: &mut Option<Rc<crate::compile::Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let begun = parked.begun();
        match kind {
            Resumption::Next => {
                self.resume_returns = false;
                self.revive(parked, sent, base, current, at);
            }
            // Resumed *at the `await` or `yield`*, so the body's `try` blocks see it — the same
            // two steps §27.5.3.4 is, and for the same reason.
            Resumption::Throw => {
                self.revive(parked, Value::Undefined, base, current, at);
                self.unwind(sent, chunk, current, at)?;
            }
            Resumption::Return if begun => {
                self.resume_returns = true;
                self.revive(parked, sent, base, current, at);
            }
            // A `return` before the body has begun completes it without running a step, which for
            // an async generator means the queue is served and the execution is simply dropped.
            Resumption::Return => {
                self.answer_step(generator, sent, true, heap);
                self.drain(generator, heap);
            }
        }
        Ok(())
    }

    /// §27.6.3.2 `AsyncGeneratorCompleteStep` — answer the request at the front and take it off.
    ///
    /// The answer is the promise that was settled, which the caller has been holding since it
    /// asked. `undefined` when the queue is empty, which is not a defect: a body revived by a job
    /// after an `await` may reach its `return` with nothing waiting, if the only request it had was
    /// already answered by a `yield`.
    pub(super) fn answer_step(
        &mut self,
        generator: ObjectId,
        value: Value,
        done: bool,
        heap: &mut Heap,
    ) -> Value {
        let result = self.iterator_result(heap, value, done);
        self.settle_front(generator, ReactionKind::Fulfil, result, heap)
    }

    /// The same with a throw completion — §27.6.3.2 with `completion` abrupt.
    ///
    /// Separate from [`Vm::answer_step`] rather than a `kind` beside it, because a rejection has
    /// **no `done`**: the reason is the whole answer and there is no iterator result to put a flag
    /// in. Written as one function taking both, the flag is a value no program can observe — which
    /// is a thing mutation coverage says out loud and a reader never would.
    pub(super) fn reject_step(
        &mut self,
        generator: ObjectId,
        reason: Value,
        heap: &mut Heap,
    ) -> Value {
        self.settle_front(generator, ReactionKind::Reject, reason, heap)
    }

    /// Take the request at the front, if there is one, and settle its promise with `answer`.
    fn settle_front(
        &mut self,
        generator: ObjectId,
        kind: ReactionKind,
        answer: Value,
        heap: &mut Heap,
    ) -> Value {
        // `pop_front` rather than a length check and an index: the emptiness is the `None`, so
        // there is no condition of this function's own for a reader — or a mutation — to get wrong.
        let Some(served) = self
            .requests_mut(generator, heap)
            .and_then(VecDeque::pop_front)
        else {
            return Value::Undefined;
        };
        let _ = self.settle_capability(served.capability, kind, answer, heap);
        served.capability.promise
    }

    /// §27.6.3.6 `AsyncGeneratorDrainQueue` — answer everything left, the body being gone.
    ///
    /// Called once the execution has finished or has been discarded. A `next` on a completed
    /// generator is `{ value: undefined, done: true }`, a `return` hands back its argument, and a
    /// `throw` rejects — the same three answers §27.5.1 gives synchronously, as promises.
    pub(super) fn drain(&mut self, generator: ObjectId, heap: &mut Heap) {
        while let Some(request) = self.next_request(generator, heap) {
            match request.kind {
                Resumption::Next => self.answer_step(generator, Value::Undefined, true, heap),
                Resumption::Return => self.answer_step(generator, request.value, true, heap),
                Resumption::Throw => self.reject_step(generator, request.value, heap),
            };
        }
    }

    /// The request at the front, without taking it off — `None` when nothing is waiting.
    fn next_request(&self, generator: ObjectId, heap: &Heap) -> Option<Request> {
        self.requests_of(generator, heap)?.front().copied()
    }

    /// Put one more ask on the end of the queue.
    fn enqueue_request(&mut self, generator: ObjectId, request: Request, heap: &mut Heap) {
        if let Some(requests) = self.requests_mut(generator, heap) {
            requests.push_back(request);
        }
    }

    /// The queue on an async generator, to read.
    fn requests_of<'a>(
        &self,
        generator: ObjectId,
        heap: &'a Heap,
    ) -> Option<&'a VecDeque<Request>> {
        match heap.object(generator)?.role()? {
            Role::Requests(requests) => Some(requests),
            _ => None,
        }
    }

    /// The same, to change.
    fn requests_mut<'a>(
        &self,
        generator: ObjectId,
        heap: &'a mut Heap,
    ) -> Option<&'a mut VecDeque<Request>> {
        match heap.object_mut(generator)?.role_mut()? {
            Role::Requests(requests) => Some(requests),
            _ => None,
        }
    }
}
