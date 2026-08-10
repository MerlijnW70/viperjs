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
    /// §9.4.2 References this body resolved and has not written through — see `Vm::references`.
    ///
    /// `with (o) { a += yield 1 }` is the shape that needs it: the target is resolved, the
    /// right-hand side parks, and the write has to go through the reference the *first* half
    /// took rather than one resolved again on the way back.
    references: Vec<super::dynamic::Resolved>,
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
    /// §8.3.2's VariableEnvironment, which a suspension has to carry for the reason it carries the
    /// one above.
    ///
    /// A `yield` inside a block parks with the two *different*, and only the pair says where a
    /// `var` belongs. Recomputing it on revival is not available — a flat chain of environments
    /// does not record which level a call opened — so a generator revived inside a block would take
    /// the block for its variable scope, and a sloppy direct `eval` after the `yield` would put its
    /// declarations in a scope that goes away at the closing brace.
    var_environment: EnvironmentId,
    /// The function object this execution is running — §10.2.2's *active function object*.
    function: Option<ObjectId>,
    /// The generator this execution belongs to, if it is a generator's body.
    ///
    /// §27.5.1's other direction: the generator holds the execution and the execution knows which
    /// generator it is. Both are needed, and at different moments — a resumption starts from the
    /// object, and a `return` inside the body has only the frame to ask.
    generator: Option<ObjectId>,
    /// Whether this execution has run any of its body — §27.5.1's two suspended states.
    ///
    /// It was here before, for `throw`, and came out because nothing could see it: reviving runs no
    /// instruction before the unwind, so a body that had not begun answered the same either way.
    /// **`return` is not like that.** §27.5.1.3 step 5 completes a not-yet-started generator without
    /// running it, where a suspended one is resumed *at the `yield`* so its `finally` blocks run —
    /// and a body that has not begun has no `yield` to resume at. That difference is a test.
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
        // The code it will carry on in, which names Strings its constant table alone holds. A
        // parked body is often the *only* thing pointing at its chunk: the function object that
        // made it may have been collected already, and the frames that ran it are gone.
        let mut named = Vec::new();
        if let Some(code) = &self.code {
            code.names(&mut named);
        }
        self.stack
            .iter()
            .copied()
            .chain([self.this_value, self.new_target])
            .chain(self.function.map(Value::Object))
            .chain(self.generator.map(Value::Object))
            .chain(named)
    }

    /// Whether this execution has begun — see the field.
    pub(super) fn begun(&self) -> bool {
        self.begun
    }

    /// Say that this parked execution has run no statement of its body — §15.5.4's suspendedStart.
    ///
    /// A method rather than an argument to [`Vm::park`], because every *other* park happens at a
    /// `yield` or an `await` and cannot be anything but begun. Passed as a flag it was a value one
    /// caller could get wrong and no program could observe; said here it is one statement, and
    /// deleting it changes what `return` does to a generator nobody has resumed.
    pub(super) fn before_the_body(mut self) -> Self {
        self.begun = false;
        self
    }

    /// The environment its next instruction will read a variable from.
    ///
    /// Kept alive by this and by nothing else once the call that made it has been parked: the
    /// frames that would have named it are gone.
    pub(crate) fn environment(&self) -> EnvironmentId {
        self.environment
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
            var_environment: self.var_environment,
            function: frame.function,
            generator: frame.generator,
            begun: true,
            stack: self.stack.split_off(operands),
            // Beside the operand stack, and for the same reason a handler's mark travels: a
            // compound assignment inside a `with` may have resolved its target and be waiting on a
            // right-hand side that contains the `yield` doing the parking. The frame does not
            // record a base for these — nothing below a call's floor can be pending across one, so
            // the whole of what is here belongs to the body being parked.
            references: std::mem::take(&mut self.references),
            handlers: self
                .handlers
                .split_off(installed)
                .into_iter()
                .map(|handler| Handler {
                    target: handler.target,
                    // Absolute, so it crosses a suspension untouched where the two counts below
                    // do not: an environment is a place on the heap, not a mark on a stack.
                    environment: handler.environment,
                    frames: handler.frames.saturating_sub(floor),
                    depth: handler.depth.saturating_sub(operands),
                    references: handler.references,
                })
                .collect(),
        };
        // …and the caller comes back, which is the half this shares with a return.
        self.environment = frame.environment;
        self.var_environment = frame.var_environment;
        self.this_value = frame.this_value;
        self.new_target = frame.new_target;
        self.realm = self.realm_by_id(frame.realm);
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
            // A revival goes back to whatever realm was running when it was asked for, and the
            // parked body's own realm is restored below out of the suspension.
            realm: self.realm.id(),
            environment: self.environment,
            var_environment: self.var_environment,
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
        self.references = parked.references;
        for handler in parked.handlers {
            self.handlers.push(Handler {
                target: handler.target,
                environment: handler.environment,
                frames: handler.frames + floor,
                depth: handler.depth + base,
                references: handler.references,
            });
        }
        self.this_value = parked.this_value;
        self.new_target = parked.new_target;
        self.environment = parked.environment;
        self.var_environment = parked.var_environment;
        *current = parked.code;
        *at = parked.at;
    }
}
