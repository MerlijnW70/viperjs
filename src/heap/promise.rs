//! §27.2.6 — what a Promise is, underneath the methods.
//!
//! A promise is an ordinary object with six internal slots and no exotic behaviour at all: its
//! properties, its prototype and its extensibility work exactly as any object's do. Everything a
//! program can observe about it comes from `then`, and everything `then` reads is here.
//!
//! # Why the reactions are kept and not the callbacks
//!
//! §27.2.1.2's PromiseReaction Record is three things — the capability whose promise is settled
//! afterwards, whether this reaction is for a fulfilment or a rejection, and the handler, which may
//! be **absent**. Absent is not `undefined`: an absent handler makes the reaction *pass through*,
//! which is how `p.then(undefined, f)` propagates a fulfilment to the promise `then` answered with
//! rather than dropping it. A list of plain callbacks cannot say that.
//!
//! # `[[PromiseIsHandled]]` is here now, and what brought it back
//!
//! This section used to say the slot was left out: §27.2.6 lists it, nothing in the language reads
//! it, its one use is §27.2.1.7 step 7's `HostPromiseRejectionTracker`, and ViperJS had no such
//! hook — so it would have been written and never read. That was right, and it ended with "it comes
//! back with the tracker".
//!
//! The tracker is built (§9.13, `Vm::unhandled_rejections`), and what asked for it was a **silent
//! stop**: a promise chain that re-armed itself ran out of heap, the RangeError was thrown inside a
//! job where §9.5 step 3 discards it, and the queue emptied with an exit status of zero. Nothing in
//! the engine was in a position to notice, because the only record that a rejection had gone
//! unclaimed was this slot and it did not exist. See DR-0029.
//!
//! # `[[AlreadyResolved]]` belongs to the pair, and not to the promise
//!
//! §27.2.1.3 puts it in a record shared by the two resolving functions. Putting it on the promise
//! instead looks equivalent and is not: §27.2.2.2 step 1.b makes a **second** pair for a promise
//! that is already resolved, so that a thenable can settle it, and that pair has to start
//! unresolved. With one flag on the promise the second pair is dead on arrival and every promise
//! resolved with another one waits for ever.
//!
//! So the flag is a cell the pair shares, which is what the specification says it is.

use crate::heap::{Heap, ObjectId};
use crate::value::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// `[[PromiseState]]` — §27.2.6.
///
/// Settled once and never again: every transition out of `Pending` is guarded by
/// `[[AlreadyResolved]]`, which is what makes a promise's answer stable no matter how many times
/// its resolving functions are called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    /// No answer yet, and the reaction lists are where the waiting is kept.
    Pending,
    /// `[[PromiseResult]]` is the value it was fulfilled with.
    Fulfilled,
    /// `[[PromiseResult]]` is the reason it was rejected with.
    Rejected,
}

/// Which of a reaction's two halves this is — §27.2.1.2's `[[Type]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionKind {
    /// Run when the promise is fulfilled.
    Fulfil,
    /// Run when it is rejected.
    Reject,
}

/// §27.2.1.1 — a PromiseCapability Record: a promise and the two functions that settle it.
///
/// Carried around rather than looked up, because §27.2.1.5 may have built it from a *subclass*
/// constructor, and then the only way back to its resolve and reject is the pair that constructor
/// handed the executor.
// No `PartialEq`: [`Value`] deliberately has none, JavaScript having three equalities.
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// `[[Promise]]` — what `then` answered with.
    pub promise: Value,
    /// `[[Resolve]]`.
    pub resolve: Value,
    /// `[[Reject]]`.
    pub reject: Value,
}

/// §27.6.3.1 — an AsyncGeneratorRequest Record: one `next`, `return` or `throw` awaiting service.
///
/// The completion the body will be resumed with, and the promise to settle with whatever that
/// resumption produces. Both halves are needed at two different moments — the completion when the
/// request reaches the front of the queue, the capability when the body next stops — and a request
/// may wait arbitrarily long in between.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    /// Which of the three was called, which decides how the body is resumed.
    pub kind: crate::heap::Resumption,
    /// The argument it was called with — the value of the `yield`, or what is returned or thrown.
    pub value: Value,
    /// `[[Capability]]` — the promise this request's caller is holding.
    pub capability: Capability,
}

/// §27.2.1.2 — a PromiseReaction Record: one `then` waiting for one answer.
#[derive(Debug, Clone, Copy)]
pub struct Reaction {
    /// `[[Capability]]` — absent for a reaction made by `await`, which has no promise to settle.
    pub capability: Option<Capability>,
    /// `[[Type]]`.
    pub kind: ReactionKind,
    /// `[[Handler]]`, which is **empty** rather than `undefined` when the argument was not callable.
    ///
    /// Empty means *pass the argument through unchanged* — a fulfilment resolves the capability
    /// with the value and a rejection rejects it with the reason. That is what makes `catch` work:
    /// it is `then(undefined, f)`, and the fulfilment half has no handler and must not swallow the
    /// value on the way past.
    pub handler: Option<Value>,
}

/// §27.2.6's slots, all six of them.
#[derive(Debug)]
pub struct Promise {
    /// `[[PromiseState]]`.
    pub state: PromiseState,
    /// `[[PromiseResult]]` — meaningful only once the state is not `Pending`.
    pub result: Value,
    /// `[[PromiseFulfillReactions]]`, in the order they were added.
    ///
    /// Order matters and is observable: §27.2.1.8 enqueues a job per reaction in list order, and
    /// the job queue is a queue, so two `then`s on the same promise run in the order they were
    /// written.
    pub fulfil: Vec<Reaction>,
    /// `[[PromiseRejectReactions]]`.
    pub reject: Vec<Reaction>,
    /// `[[PromiseIsHandled]]` — whether anything has ever asked this promise for its answer.
    ///
    /// Set by §27.2.5.4.1 step 12, and by nothing else: it is `then` that handles a promise, so a
    /// promise reaches `true` the first time one is registered and never goes back. **Not the same
    /// as "somebody wanted the fulfilment"** — `p.then(f)` with no rejection handler still sets it,
    /// because the rejection then travels to the promise `then` answered with and *that* one is
    /// what is unhandled. Reading it as "has a rejection handler" would report every `.then(f)`
    /// chain as unhandled at every link.
    pub handled: bool,
}

/// The two things a resolving function carries — §27.2.1.3's `[[Promise]]` and
/// `[[AlreadyResolved]]`.
///
/// The flag is shared with the other half of the pair, which is what makes `resolve` and `reject`
/// contradict each other exactly never: whichever is called first settles the promise, and the
/// other then finds the flag set and does nothing. Shared by `Rc` because that is what "the same
/// record" means when neither function owns the other.
#[derive(Debug, Clone)]
pub struct Settler {
    /// `[[Promise]]` — what this pair settles.
    pub promise: ObjectId,
    /// `[[AlreadyResolved]]`, the pair's own.
    pub resolved: Rc<Cell<bool>>,
}

impl Settler {
    /// A fresh pair's shared state.
    #[must_use]
    pub fn new(promise: ObjectId) -> Self {
        Self {
            promise,
            resolved: Rc::new(Cell::new(false)),
        }
    }

    /// Claim the one settlement this pair is allowed — §27.2.1.3.2 steps 3 and 4.
    ///
    /// `true` the first time and `false` for ever after, which is the whole of what makes a
    /// promise's answer final however many times a program calls the functions it was handed.
    pub fn claim(&self) -> bool {
        !self.resolved.replace(true)
    }
}

/// Which of §27.2.4's combinators a group of elements belongs to.
///
/// They differ in three small ways and each is a whole clause: what a rejection does, whether the
/// value is recorded or the outcome is, and whether anything is recorded at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// §27.2.4.1 — every value, in iteration order; the first rejection rejects the group.
    All,
    /// §27.2.4.2 — an outcome object per element, and no rejection at all.
    AllSettled,
    /// §27.2.4.3 — the first fulfilment; running out is a rejection carrying every reason.
    Any,
    /// §27.2.4.4 — the first to settle, whichever way it settled, and nothing is collected.
    Race,
}

/// What a group of elements shares — §27.2.4.1.1's `values` and `remainingElementsCount`.
///
/// One record behind an `Rc`, because the specification's is shared by every element function and
/// by the walk that made them: each holds it, each changes it, and the last one to put it down is
/// the one that settles the promise.
#[derive(Debug)]
pub struct Gather {
    /// One slot per element, made when the element is *read* and filled when it settles.
    ///
    /// Made early on purpose: the answer is in iteration order however the promises settle, and a
    /// list appended to on settlement would be in completion order instead.
    pub values: Vec<Value>,
    /// `[[RemainingElements]]`, which starts at **one** — see the module documentation.
    pub remaining: usize,
    /// What is settled when the count reaches zero.
    pub capability: Capability,
    /// Which combinator this is.
    pub group: Group,
}

/// What one of §27.2's function objects carries where the specification writes a closure.
///
/// Three of them capture state: the resolve and reject functions capture the promise they settle
/// (§27.2.1.3), and the executor `NewPromiseCapability` passes to a constructor captures the
/// capability it is filling in (§27.2.1.5.1). A built-in body is a bare function pointer holding
/// nothing, so the capture is on the object rather than in the body — which is what the
/// specification says it is anyway, these being ordinary function objects with internal slots.
#[derive(Debug, Clone)]
pub enum Role {
    /// §28.2.2.1.1's `[[RevocableProxy]]` — the proxy a revocation function turns off.
    ///
    /// Here rather than in a type of its own for the reason the two below it are: a built-in's body
    /// is a bare function pointer holding no state, so what the specification captures in a closure
    /// is carried on the function object.
    Revoke(crate::heap::ObjectId),
    /// §27.1.4's `[[SyncIteratorRecord]]` — what an async-from-sync wrapper is wrapping.
    ///
    /// The iterator and its `next`, read once when the wrapper was made, on the same terms as every
    /// other Iterator Record: replacing `next` on the sync iterator afterwards does not change the
    /// walk.
    SyncIterator {
        /// The synchronous iterator being adapted.
        iterator: Value,
        /// …and its `next`, read once.
        next: Value,
    },
    /// The promise an `async` function's execution will settle — §27.7.5.1's capability.
    ///
    /// Held on the execution's own context object rather than on a function, which is the one
    /// place it can be: what settles it is a `return` or a throw from a body that may be parked
    /// for any length of time, and by then nothing else still names the promise.
    Await(Capability),
    /// §27.6.3's `[[AsyncGeneratorQueue]]` — the requests waiting on one async generator.
    ///
    /// The reason an async generator needs a queue where a synchronous one needs nothing: `next`
    /// answers a **promise** and returns immediately, so a caller may ask again long before the
    /// first answer exists, and §27.6.3.2 says every ask is served, in order. A synchronous `next`
    /// cannot be asked twice at once — it either runs the body to the next `yield` or throws — so
    /// there is nothing to remember.
    ///
    /// Held on the generator object itself, which is where §27.6.1's slot is. That is also what
    /// makes the queue *reachable*: a request holds the promise its caller is waiting for, and
    /// while the generator is suspended nothing else names it.
    Requests(std::collections::VecDeque<Request>),
    /// A resolve function — §27.2.1.3.2.
    Resolve(Settler),
    /// A reject function — §27.2.1.3.1.
    Reject(Settler),
    /// `[[Capability]]` of a capabilities executor — §27.2.1.5.1.
    ///
    /// Filled in when the executor is called, and read back by `NewPromiseCapability` afterwards.
    /// Both halves start as `undefined`, and step 2 of §27.2.1.5.1 is the check that the
    /// constructor called it once rather than keeping the pair for itself.
    Executor {
        /// What the constructor passed as the resolve function.
        resolve: Value,
        /// …and as the reject function.
        reject: Value,
    },
    /// `[[OnFinally]]` and `[[Constructor]]` of one of §27.2.5.3's two wrappers.
    Finally {
        /// The function `finally` was given, called for its effect and not for its answer.
        handler: Value,
        /// The constructor `PromiseResolve` builds with — §27.2.5.3 step 4's `SpeciesConstructor`.
        constructor: Value,
    },
    /// `[[Value]]` of a thunk — a function that ignores its arguments and answers this.
    ///
    /// §27.2.5.3.1 step 6 makes one per settled value, and it is what carries the original answer
    /// *past* the handler's own: the handler's result is waited for, and then this is returned
    /// instead of it.
    Thunk(Value),
    /// The same, for the rejection half — §27.2.5.3.2 step 6, which **throws** rather than answers.
    Thrower(Value),
    /// One element function of a combinator — §27.2.4.1.2 and §27.2.4.2.2.
    Element {
        /// Which slot of the shared list this one fills.
        index: usize,
        /// `[[AlreadyCalled]]`, shared with the other half of this element's pair.
        ///
        /// Shared because `allSettled` subscribes two functions per element and a promise that
        /// managed to call both must fill its slot once: settling twice would take the count down
        /// twice and resolve the group early, with holes in it.
        called: Rc<Cell<bool>>,
        /// The group this element belongs to.
        gather: Rc<RefCell<Gather>>,
        /// Which half of the pair this is, which is what `allSettled` records as `status`.
        kind: ReactionKind,
    },
}

impl Promise {
    /// A pending promise with nothing waiting on it — §27.2.3.1 steps 4 and 5.
    fn new() -> Self {
        Self {
            state: PromiseState::Pending,
            result: Value::Undefined,
            fulfil: Vec::new(),
            reject: Vec::new(),
            handled: false,
        }
    }
}

impl Heap {
    /// A new pending promise inheriting from `prototype`.
    pub fn new_promise(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let id = self.new_object(prototype);
        if let Some(object) = self.object_mut(id) {
            object.set_promise(Promise::new());
        }
        id
    }

    /// The promise state of an object, if it is a promise at all.
    ///
    /// `None` for every other object, which is exactly the test §27.2.5.4 step 2 makes before it
    /// will do anything — `Promise.prototype.then.call({})` is a TypeError and not a promise that
    /// never settles.
    pub fn promise(&self, id: ObjectId) -> Option<&Promise> {
        self.object(id).and_then(super::Object::promise)
    }

    /// The same, to change.
    pub fn promise_mut(&mut self, id: ObjectId) -> Option<&mut Promise> {
        self.object_mut(id).and_then(super::Object::promise_mut)
    }
}
