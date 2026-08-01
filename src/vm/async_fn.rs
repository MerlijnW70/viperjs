//! §27.7 — an `async` function, which is a generator with the resumptions written for it.
//!
//! The machinery underneath is the one `yield` uses and not a second copy of it: an `await` parks
//! the execution exactly as a `yield` does. What differs is everything around that.
//!
//! - A generator answers with itself and runs nothing; an `async` function answers with a
//!   **promise** and runs its body immediately, as far as the first `await`.
//! - A generator is resumed by a script calling `next`; an `async` function is resumed by a job,
//!   and the two functions that do it are made per `await` and handed to `then`.
//! - A generator's `return` becomes `{ value, done: true }`; an `async` function's *resolves* its
//!   promise, and a throw that escapes its body *rejects* it rather than travelling on.
//!
//! So the parked execution is the same record, held by an object of the same shape, and the three
//! differences are three places that ask which kind it is.

use super::call::Frame;
use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{Capability, Heap, ObjectId, ReactionKind, Role, Suspendable};
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    /// §27.7.5.1 `AsyncFunctionStart` — begin an `async` function's body and answer with a promise.
    ///
    /// Unlike a generator's entry this pushes a frame and lets the loop run: the body runs *now*,
    /// synchronously, as far as its first `await`. What the call answers with is decided by
    /// whatever stops it — `Await` and `Return` both leave the promise where a return value goes,
    /// so the caller is handed the same object however far the body got.
    ///
    /// The capability is built directly rather than through `NewPromiseCapability`, because
    /// §27.7.5.1 names `%Promise%` — the intrinsic and not `globalThis.Promise` — and for that
    /// constructor the two are the same object with the same pair of functions. Going through the
    /// executor would call a constructor once per invocation of every `async` function in the
    /// program for a result nothing can tell apart.
    pub(super) fn begin_async(&mut self, heap: &mut Heap) -> Option<ObjectId> {
        let capability = self.intrinsic_capability(heap);
        let context = heap.new_object(None);
        heap.brand_suspendable(context, Suspendable::Async);
        heap.object_mut(context)?.set_role(Role::Await(capability));
        Some(context)
    }

    /// `NewPromiseCapability(%Promise%)`, built without going through the constructor.
    ///
    /// §27.2.1.5 goes through `Construct` because the constructor may be a subclass whose executor
    /// is observable. Every caller of *this* one names the **intrinsic** `%Promise%` — §27.7.5.1
    /// and §27.1.4.2 both do — and for that constructor the executor merely hands back the pair
    /// this makes directly. Nothing can tell the two apart, and a program with an `await` in a loop
    /// would otherwise construct once per turn.
    pub(crate) fn intrinsic_capability(&mut self, heap: &mut Heap) -> Capability {
        let promise = heap.new_promise(Some(self.realm.promise_prototype()));
        let (resolve, reject) = crate::builtins::promise::resolving_functions(heap, self, promise);
        Capability {
            promise: Value::Object(promise),
            resolve,
            reject,
        }
    }

    /// §27.7.5.3 `Await` — hand the value to a promise and stop until it settles.
    ///
    /// Three steps in this order and it matters: the value is turned into a promise, the two
    /// resumption functions are attached to it, and only then is the execution parked. Everything
    /// before the park may run a script — `PromiseResolve` reads `constructor` off a promise-like
    /// value — and a script that threw after the park would have nothing to unwind.
    ///
    /// What is left behind is the *function's own* promise, because that is what its caller is
    /// waiting for. On the second and later awaits nobody is waiting: the resumption came from a
    /// job, whose completion §9.5 discards.
    pub(super) fn await_value(
        &mut self,
        context: ObjectId,
        value: Value,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let inner = match self.promise_for(value, heap) {
            Ok(promise) => promise,
            Err(error) => {
                self.raise(error, heap, chunk, current, at)?;
                return Ok(());
            }
        };
        // The two closures §27.7.5.3 steps 3 and 5 describe. A function object each, because each
        // has to carry which execution it revives and in which direction — a built-in's body is a
        // bare pointer and holds nothing.
        let fulfilled = heap.new_revive_function(context, ReactionKind::Fulfil);
        let rejected = heap.new_revive_function(context, ReactionKind::Reject);
        let then = crate::builtins::promise::perform_then(
            self,
            heap,
            inner,
            Value::Object(fulfilled),
            Value::Object(rejected),
            None,
        );
        if let Err(error) = then {
            self.raise(error, heap, chunk, current, at)?;
            return Ok(());
        }
        // What the resumption that reached this `await` answers with. An `async` function answers
        // with its own promise, which its caller has been holding since the first `await`; an
        // **async generator** answers with nothing at all, because the promise for the request in
        // service was pushed when that request was enqueued and the slot is already filled.
        let answer = match heap
            .object(context)
            .and_then(crate::heap::Object::suspendable)
        {
            Some(Suspendable::AsyncGenerator) => None,
            _ => Some(
                self.capability_of(context, heap)
                    .map_or(Value::Undefined, |held| held.promise),
            ),
        };
        let parked = self.park(current, at)?;
        // The context was made by `begin_async` and named by the frame, so it is an object.
        heap.park_into(context, parked);
        if let Some(answer) = answer {
            self.stack.push(answer);
        }
        Ok(())
    }

    /// Settle an `async` function's promise and answer with it — §27.7.5.2 steps 3 and 4.
    ///
    /// What both ways out of the body come to. A `return` resolves and an escaping throw rejects,
    /// and either way the value the *call* answers with is the promise, which the caller has been
    /// holding since the first `await`.
    pub(super) fn settle_async(
        &mut self,
        context: ObjectId,
        kind: ReactionKind,
        value: Value,
        heap: &mut Heap,
    ) -> Value {
        let Some(capability) = self.capability_of(context, heap) else {
            return Value::Undefined;
        };
        // §27.2.1.3's resolving functions answer `undefined` and settle by side effect, and a
        // throw from one is not this function's to report: a resolve that fails has already
        // rejected, which is the whole of `[[AlreadyResolved]]`.
        let _ = self.settle_capability(capability, kind, value, heap);
        capability.promise
    }

    /// The promise and its two halves, off an `async` execution's context object.
    fn capability_of(&self, context: ObjectId, heap: &Heap) -> Option<Capability> {
        match heap.object(context)?.role()? {
            Role::Await(capability) => Some(*capability),
            _ => None,
        }
    }

    /// §7.4's `PromiseResolve(%Promise%, value)` — the value as a promise, itself if it is one.
    fn promise_for(&mut self, value: Value, heap: &mut Heap) -> crate::value::Completion<ObjectId> {
        let constructor = self.realm.promise_constructor();
        let resolved = crate::builtins::promise::promise_resolve(self, heap, constructor, value)?;
        match resolved {
            Value::Object(promise) => Ok(promise),
            // `%Promise%` always answers with a promise object, so this is a shape nothing
            // produces; answering with a TypeError rather than pretending is what says so.
            _ => Err(Abrupt::type_error("await did not produce a promise")),
        }
    }

    /// §27.7.5.3 steps 3 and 5 — a settled promise putting an `async` body back.
    ///
    /// The mirror of `enter_resume`, and much smaller because there is no state machine to consult:
    /// a promise settles once, so exactly one of the two functions is ever called and it is called
    /// exactly once. What it answers is discarded — §9.5 step 3.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    pub(super) fn enter_revive(
        &mut self,
        kind: ReactionKind,
        context: ObjectId,
        callee_at: usize,
        receiver_at: usize,
        count: usize,
        heap: &mut Heap,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // From the callee's slot and not from the receiver's, which is where `enter_native` and
        // `enter_resume` both read theirs. The two are one apart for a method call and the *same
        // slot* for a plain one, so counting from the receiver happens to be right only because a
        // job always calls this as a method — and would be off by one the day anything else did.
        let settled = match count {
            0 => Value::Undefined,
            _ => self.stack[callee_at + 1],
        };
        self.stack.truncate(receiver_at);
        let Some(parked) = heap.take_parked(Value::Object(context)) else {
            // A promise settles once and takes its reactions off the list when it does, so the
            // other closure is never called and neither is this one twice. Answering `undefined`
            // rather than faulting, because a job's completion goes nowhere in either case.
            self.stack.push(Value::Undefined);
            return Ok(());
        };
        match kind {
            // Step 3 — the `await` expression evaluates to what the promise fulfilled with.
            ReactionKind::Fulfil => self.revive(parked, settled, receiver_at, current, at),
            // Step 5 — a rejection is a `throw` at the `await`, so a `try` around it catches it.
            // The same two steps `Generator.prototype.throw` takes, and for the same reason.
            ReactionKind::Reject => {
                self.revive(parked, Value::Undefined, receiver_at, current, at);
                let root = Chunk::from_parts(Vec::new(), Vec::new());
                self.unwind(settled, &root, current, at)?;
            }
        }
        Ok(())
    }

    /// Whether this frame's suspendable is an `async` function's rather than a generator's.
    ///
    /// Asked of the *object* rather than of the frame, because the object is what a resumption
    /// arrives at and the two have to agree. A frame with none is an ordinary call.
    pub(super) fn suspendable_of(frame: &Frame, heap: &Heap) -> Option<(ObjectId, Suspendable)> {
        let context = frame.generator?;
        let kind = heap.object(context)?.suspendable()?;
        Some((context, kind))
    }
}
