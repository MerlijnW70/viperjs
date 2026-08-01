//! §9.5 — the job queue, and what it means for a job to run "later".
//!
//! A job is work the specification hands back to the host with an instruction about *when*:
//! §9.5's `HostEnqueuePromiseJob` says a job runs only when there is no running execution context,
//! which for a script means after the last statement. That is the whole of what makes `then`
//! asynchronous — nothing about a promise is concurrent, and there is no clock anywhere near it.
//!
//! # Why a queue and not a list
//!
//! Order is observable and is the first thing a program notices. §27.2.1.8 enqueues one job per
//! reaction in the order the reactions were added, so `p.then(a); p.then(b)` runs `a` before `b`;
//! and a job enqueued *by* a job goes on the end, so `p.then(a).then(b)` runs `a`, then whatever
//! else was already waiting, then `b`. A stack would reverse both and look almost right.
//!
//! # What happens to a job that throws
//!
//! Nothing. §9.5 step 3 says the host discards the completion, and there is nowhere for it to go:
//! the script that would have caught it has already finished. A rejection handler that throws
//! rejects the promise `then` answered with, which is the reaction's own doing and happens before
//! the job returns — so the only completions dropped here are ones no promise was waiting for.

use crate::heap::{Capability, Heap, ObjectId, Reaction, ReactionKind};
use crate::value::{Completion, Value};
use crate::vm::Vm;

/// One piece of work waiting for the stack to empty.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Job {
    /// §27.2.2.1 `NewPromiseReactionJob` — run one reaction against the value that settled it.
    Reaction {
        /// The reaction, taken off the promise's list when it settled.
        reaction: Reaction,
        /// `[[PromiseResult]]` at the moment it settled.
        argument: Value,
    },
    /// §27.2.2.2 `NewPromiseResolveThenableJob` — hand our resolving functions to a thenable.
    ///
    /// A job rather than a call, and that is not an optimisation: it is what stops a `then` written
    /// by a program from running in the middle of the statement that resolved with it. The
    /// difference is visible in one line of output ordering and in nothing else.
    ResolveThenable {
        /// The promise being resolved.
        promise: ObjectId,
        /// The object that turned out to have a callable `then`.
        thenable: Value,
        /// That `then`, read once — §27.2.1.3.2 step 9 reads it before enqueueing, so a getter that
        /// changes it afterwards changes nothing.
        then: Value,
    },
}

impl Job {
    /// Every heap value this job will need when it runs.
    ///
    /// A queued job is often the only thing naming what it will run with: a reaction holds the
    /// handler a program passed to `then` and let go of, and the capability it will settle. Written
    /// beside the variants so that a third kind of job cannot be added without its own line here.
    pub(crate) fn names(&self, into: &mut Vec<Value>) {
        match self {
            Job::Reaction { reaction, argument } => {
                into.push(*argument);
                into.extend(reaction.handler);
                if let Some(capability) = reaction.capability {
                    into.extend([capability.promise, capability.resolve, capability.reject]);
                }
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => into.extend([Value::Object(*promise), *thenable, *then]),
        }
    }
}

impl Vm {
    /// Put a job on the end of the queue — §9.5 `HostEnqueuePromiseJob`.
    pub(crate) fn enqueue(&mut self, job: Job) {
        self.jobs.push_back(job);
    }

    /// Run every job, and every job they make, until there are none — §9.5.
    ///
    /// Not bounded, and deliberately: `function f() { Promise.resolve().then(f); }` never stops,
    /// exactly as `while (true) {}` never stops. DR-0002 is about panics, not about halting, and a
    /// cap here would be a made-up limit that no specification mentions and that a correct program
    /// could reach.
    pub(crate) fn drain_jobs(&mut self, heap: &mut Heap) {
        while let Some(job) = self.jobs.pop_front() {
            // The completion is discarded, which is §9.5 step 3. Everything a promise was waiting
            // for has already been settled by the job itself before it could throw.
            let _ = self.run_job(job, heap);
        }
    }

    /// One job.
    fn run_job(&mut self, job: Job, heap: &mut Heap) -> Completion<Value> {
        match job {
            Job::Reaction { reaction, argument } => self.reaction_job(reaction, argument, heap),
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => self.thenable_job(promise, thenable, then, heap),
        }
    }

    /// §27.2.2.1 — run a reaction's handler and settle the capability with what it answered.
    fn reaction_job(
        &mut self,
        reaction: Reaction,
        argument: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        // Steps 4 and 5 — an *absent* handler passes the argument through unchanged, which is not
        // the same as a handler that returns its argument: a rejection with no handler rejects the
        // capability rather than fulfilling it with the reason.
        let outcome = match reaction.handler {
            None => match reaction.kind {
                ReactionKind::Fulfil => Ok(argument),
                ReactionKind::Reject => Err(crate::value::Abrupt::Thrown(argument)),
            },
            Some(handler) => self.call_value(handler, Value::Undefined, &[argument], heap),
        };
        // Step 7 — with no capability there is nothing to settle and a throw goes nowhere. That is
        // the shape `await` uses, and it is why the completion is returned rather than swallowed.
        let Some(capability) = reaction.capability else {
            return outcome.map(|_| Value::Undefined);
        };
        match outcome {
            Ok(value) => self.call_value(capability.resolve, Value::Undefined, &[value], heap),
            Err(abrupt) => {
                let reason = self.thrown_value(abrupt, heap);
                self.call_value(capability.reject, Value::Undefined, &[reason], heap)
            }
        }
    }

    /// §27.2.2.2 — call a thenable's `then` with our resolving functions.
    fn thenable_job(
        &mut self,
        promise: ObjectId,
        thenable: Value,
        then: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let (resolve, reject) = crate::builtins::promise::resolving_functions(heap, self, promise);
        let outcome = self.call_value(then, thenable, &[resolve, reject], heap);
        // Step 1.d — a `then` that throws rejects the promise, *unless* it had already called one
        // of the two functions it was given. `reject` answers that question itself by way of
        // `[[AlreadyResolved]]`, so this needs no separate check.
        match outcome {
            Ok(value) => Ok(value),
            Err(abrupt) => {
                let reason = self.thrown_value(abrupt, heap);
                self.call_value(reject, Value::Undefined, &[reason], heap)
            }
        }
    }

    /// A capability's two halves, called in the order the specification settles them.
    ///
    /// Not on the [`Capability`] itself, because settling one is a *call* and a record cannot make
    /// one — the two are together here so that no caller has to remember which argument goes where.
    pub(crate) fn settle_capability(
        &mut self,
        capability: Capability,
        kind: ReactionKind,
        argument: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let half = match kind {
            ReactionKind::Fulfil => capability.resolve,
            ReactionKind::Reject => capability.reject,
        };
        self.call_value(half, Value::Undefined, &[argument], heap)
    }
}
