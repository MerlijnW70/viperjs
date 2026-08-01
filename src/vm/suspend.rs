//! Parking an execution and reviving it — DR-0017's suspended frame, as data.
//!
//! A generator suspends at `yield` and is revived by `next`; an `async` function suspends at
//! `await` and is revived by a job. Both need the same thing, and this is it: an execution that can
//! be taken out of the interpreter, held as a value, and put back.
//!
//! That it can be taken out at all is a property of [`super::call::Frame`] — a plain record,
//! borrowing nothing — and of the loop above it, which does not recurse for a JavaScript call.
//! DR-0017 is the other half: a parked execution keeps no return address, so where it is revived
//! has nothing to do with where it was parked.

use super::call::Frame;
use super::{Fault, Handler, Vm};
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, ObjectId};
use crate::value::Value;
use std::rc::Rc;

/// One execution, out of the interpreter and kept whole — §27.5.1's `[[GeneratorContext]]`.
///
/// Everything the loop was using and nothing it was borrowing: where the code had got to, the
/// three registers a call decides, the operands it had built, and the handlers it had installed.
/// Put back by [`Vm::revive`], it carries on at the instruction after the one that parked it.
///
/// The two stacks are its *own* slices, taken from the marks the frame already carried — which is
/// what makes this a record of one execution rather than of the machine. The handlers in it are
/// rebased to those marks for the same reason: a [`Handler`] names an absolute depth, and a
/// revival almost never happens at the depth the suspension did.
#[derive(Debug)]
pub(crate) struct Suspended {
    /// The code that was running, and the instruction to carry on at.
    ///
    /// `None` is the shape a [`Frame`] uses for the root chunk, which the caller owns. Nothing the
    /// compiler emits parks one — a suspension is only ever inside a function body — and a
    /// hand-built chunk that managed it would revive into the root rather than crashing.
    code: Option<Rc<Chunk>>,
    at: usize,
    this_value: Value,
    new_target: Value,
    environment: EnvironmentId,
    /// The function object this execution is running — §10.2.2's *active function object*.
    function: Option<ObjectId>,
    /// The generator this execution belongs to, if it is a generator's body.
    ///
    /// §27.5.1's other direction: the generator holds the execution and the execution knows which
    /// generator it is. Both are needed, and at different moments — a resumption starts from the
    /// object, and a `return` inside the body has only the frame to ask.
    generator: Option<ObjectId>,
    /// Whether this execution has run at all — §27.5.1's `suspendedStart` against
    /// `suspendedYield`.
    ///
    /// Here rather than on the generator, because it is a fact about the *execution*: one made by
    /// [`Suspended::started`] has not begun and one made by [`Vm::park`] has, and there is no third
    /// way to make one. What reads it is `throw`, which has a `try` to consider only if the body
    /// has reached one.
    ///
    /// **Nothing can currently tell the two apart**, and that is worth writing down rather than
    /// acting on. §27.5.1.4 step 5 completes a not-yet-started generator without running it, where
    /// step 8 resumes a suspended one abruptly — but resuming an execution parked at instruction
    /// zero, with no handlers installed, unwinds before a single instruction is read, so both paths
    /// reach the same place. Measured rather than assumed: flipping this constant leaves all 93,153
    /// test262 runs identical, which is why mutation coverage reports it as a survivor.
    ///
    /// It stays because the coincidence has a known expiry. The parameter-default divergence
    /// recorded beside `yield` is fixed by running the prologue at the *call* and parking at the
    /// body — and then instruction zero is real code, reviving a not-yet-started generator would
    /// run it, and step 5 forbids exactly that. Deleting this to satisfy a mutation score would
    /// plant that bug in advance.
    begun: bool,
    /// The operands it had built, from its own floor upwards.
    stack: Vec<Value>,
    /// The handlers it had installed, each rebased to that floor.
    handlers: Vec<Handler>,
}

impl Suspended {
    /// Every value this parked execution can still reach.
    ///
    /// The collector's view of it, and the reason a parked execution is safe to hold: nothing in
    /// here is reachable any other way. The operands are on no stack the machine owns any more,
    /// and a `this` captured mid-call may be the only reference to the object it names.
    pub(crate) fn reachable(&self) -> impl Iterator<Item = Value> + '_ {
        self.stack
            .iter()
            .copied()
            .chain([self.this_value, self.new_target])
            .chain(self.function.map(Value::Object))
            .chain(self.generator.map(Value::Object))
    }

    /// Whether this execution has run any of its body — §27.5.1's two suspended states.
    pub(super) fn begun(&self) -> bool {
        self.begun
    }

    /// The environment its next instruction will read a variable from.
    ///
    /// Kept alive by this and by nothing else once the call that made it has been parked: the
    /// frames that would have named it are gone.
    pub(crate) fn environment(&self) -> EnvironmentId {
        self.environment
    }

    /// An execution that has not begun — §15.5.4's `GeneratorStart`, as a parked frame.
    ///
    /// Instruction zero, an empty operand stack and no handlers, which is exactly what a frame the
    /// loop has never run would look like. That is the whole trick behind a generator function
    /// answering without running anything: there is no separate "not started yet" state, only a
    /// suspension that happens to be at the beginning.
    pub(super) fn started(
        code: Rc<Chunk>,
        environment: EnvironmentId,
        this_value: Value,
        new_target: Value,
        function: ObjectId,
        generator: ObjectId,
    ) -> Self {
        Self {
            code: Some(code),
            at: 0,
            this_value,
            new_target,
            environment,
            function: Some(function),
            generator: Some(generator),
            begun: false,
            stack: Vec::new(),
            handlers: Vec::new(),
        }
    }
}

impl Vm {
    /// Take the running execution out of the machine, leaving its caller running.
    ///
    /// A [`Instruction::Return`](crate::compile::Instruction::Return) in every respect but one:
    /// the callee's operands, handlers and position are kept instead of being dropped. What is
    /// left behind is exactly what a return leaves — the caller's registers, its code, and its
    /// stack truncated to where the call began — so the instruction that parks may push the value
    /// the call answers with and carry on.
    ///
    /// # DR-0017
    ///
    /// What is kept is the *callee's* half of the frame and none of the caller's. The code, the
    /// instruction and the registers to come back to stay in the frame and are put back here; the
    /// parked execution carries no return address at all, which is what makes it portable. A
    /// generator suspended inside a `map` callback may be resumed from anywhere later, because
    /// nothing in it remembers `map`.
    ///
    /// That is why there is no check here against DR-0011's nested execution, and it took two
    /// wrong answers to be sure of: the Rust call waiting mid-instruction is waiting for a *value*,
    /// and the instruction that parks leaves one. See the record.
    pub(super) fn park(
        &mut self,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Suspended, Fault> {
        let Some(frame) = self.frames.pop() else {
            return Err(Fault::YieldOutsideGenerator);
        };
        // `reentries` is non-zero exactly when a nested execution is running, and its floor is the
        // depth it started at — so the two together say "the frame just popped was the one that
        // execution entered". Equality rather than `<=`: the nested loop stops the moment its
        // entry frame is gone, so nothing can pop past the floor and come back here.
        // Where this frame sat, now that it is gone — the depth every handler in it was installed
        // at or above, and so what makes their `frames` marks relative rather than absolute.
        let floor = self.frames.len();
        // Split at the marks directly, with nothing guarding them, and that is a claim worth
        // stating: **a frame's two floors are never above its top.** Both places that give a frame
        // a suspendable truncate to the floor before the body starts — `revive` to `base` and
        // `enter` to the receiver's slot — so the body begins with nothing of its own, and a
        // compiled body pops only what it pushed and installs handlers in pairs.
        //
        // There was a `.min()` here until `Yield` began reading the generator off the *frame*.
        // Before that a hand-built chunk could park with an under-popped stack, and one did, in a
        // test; now `frame.generator` is set only by `revive` and by an `async` entry, so no chunk
        // an embedder can write reaches this at all. What was left was two branches nothing could
        // execute — which is the thing mutation coverage exists to find, and it found them.
        let operands = frame.stack_base;
        let installed = frame.handlers_base;
        let parked = Suspended {
            code: current.take(),
            at: *at,
            this_value: self.this_value,
            new_target: self.new_target,
            environment: self.environment,
            function: frame.function,
            generator: frame.generator,
            begun: true,
            stack: self.stack.split_off(operands),
            handlers: self
                .handlers
                .split_off(installed)
                .into_iter()
                .map(|handler| Handler {
                    target: handler.target,
                    frames: handler.frames.saturating_sub(floor),
                    depth: handler.depth.saturating_sub(operands),
                })
                .collect(),
        };
        // …and the caller comes back, which is the half this shares with a return.
        self.environment = frame.environment;
        self.this_value = frame.this_value;
        self.new_target = frame.new_target;
        *current = frame.code;
        *at = frame.at;
        Ok(parked)
    }

    /// Put a parked execution back and carry on inside it.
    ///
    /// The mirror of [`Vm::park`], and an entry into a call in every respect but one: there is no
    /// body to start at the beginning, because this one is half-run. `base` is where its operands
    /// belong — the stack index the value it eventually returns will be left at, exactly as a call
    /// leaves its answer where the callee sat.
    ///
    /// `sent` is pushed on top of the operands it had built, so it becomes the value of the
    /// expression that parked. That is what makes `gen.next(v)` an argument rather than a signal:
    /// the `yield` that suspended evaluates to `v` when the body starts moving again.
    ///
    /// There is no recursion limit here, and that is not an omission: this pushes the frame a
    /// suspension popped, so the pair conserves the frame stack, and the calls that could nest one
    /// revival inside another are bounded where [`Vm::enter`] bounds every call.
    pub(super) fn revive(
        &mut self,
        parked: Suspended,
        sent: Value,
        base: usize,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) {
        // Read before the push, so that it is the index the revived frame is about to occupy —
        // the same number `park` recorded the handlers against.
        let floor = self.frames.len();
        self.frames.push(Frame {
            code: (*current).take(),
            at: *at,
            this_value: self.this_value,
            new_target: self.new_target,
            environment: self.environment,
            stack_base: base,
            handlers_base: self.handlers.len(),
            // §10.2.2 step 13's preference belongs to a construction, and a revival is not one:
            // whatever made this execution decided that once, and it is not being decided again.
            constructed: None,
            function: parked.function,
            generator: parked.generator,
        });
        self.stack.truncate(base);
        self.stack.extend_from_slice(&parked.stack);
        self.stack.push(sent);
        for handler in parked.handlers {
            self.handlers.push(Handler {
                target: handler.target,
                frames: handler.frames + floor,
                depth: handler.depth + base,
            });
        }
        self.this_value = parked.this_value;
        self.new_target = parked.new_target;
        self.environment = parked.environment;
        *current = parked.code;
        *at = parked.at;
    }
}
