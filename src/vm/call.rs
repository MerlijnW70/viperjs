//! Entering a function — §10.2.1's `[[Call]]`, as the interpreter performs it.
//!
//! Three things are decided here and none of them belongs to the function object: the environment
//! the call runs in, the `this` it sees, and the frame that says how to get back. A function
//! object holds only the two halves that *are* its own — the code, and the environment it was
//! written in.

use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, Object};
use crate::realm::NativeError;
use crate::value::{TypeError, Value};
use std::rc::Rc;

impl Vm {
    /// Call whatever is on the stack, leaving the interpreter running inside it.
    ///
    /// The callee sits under its arguments, because it was pushed first — and a method call has
    /// its receiver under that again. Nothing recurses: the frame is pushed, the code is swapped,
    /// and [`Vm::run`]'s loop goes round again in the callee.
    pub(super) fn enter(
        &mut self,
        method: bool,
        count: u32,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let count = count as usize;
        // The callee sits under its arguments, because it was pushed first — and a
        // method call has its receiver under *that*.
        let Some(callee_at) = self.stack.len().checked_sub(count + 1) else {
            return Err(Fault::StackUnderflow);
        };
        let receiver_at = if method {
            match callee_at.checked_sub(1) {
                Some(at) => at,
                None => return Err(Fault::StackUnderflow),
            }
        } else {
            callee_at
        };
        let callee = self.stack[callee_at];
        // §10.2.1.2 — a call with no receiver, and a sloppy-mode function, get the
        // global object rather than `undefined`. Strict mode keeps the `undefined`,
        // and telling the two apart needs the flag the parser already computes.
        let receiver = if method {
            self.stack[receiver_at]
        } else {
            Value::Object(self.realm.global())
        };
        let Value::Object(object) = callee else {
            self.throw_type_error(
                TypeError("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        let Some(body) = heap.object(object).and_then(Object::call) else {
            self.throw_type_error(
                TypeError("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        let body = body.clone();
        if self.frames.len() >= MAX_CALL_DEPTH {
            let thrown = self
                .realm
                .error(heap, NativeError::Range, "too much recursion");
            self.unwind(thrown, chunk, current, at)?;
            return Ok(());
        }
        // §10.2.11 — a new environment per call, written inside the one the function
        // was *defined* in. That parent is the whole of what a closure is: the
        // caller's environment has nothing to do with it, which is the difference
        // between lexical scope and dynamic scope.
        let Some(defined_in) = heap.object(object).and_then(Object::environment) else {
            return Err(Fault::MissingFunction);
        };
        let environment = heap.new_environment(Some(defined_in), body.locals());
        for offset in 0..body.parameters().min(count) {
            let argument = self.stack[callee_at + 1 + offset];
            let index = u32::try_from(offset).unwrap_or(u32::MAX);
            heap.set_variable(environment, index, argument);
        }
        self.stack.truncate(receiver_at);
        self.frames.push(Frame {
            code: (*current).take(),
            at: *at,
            this_value: self.this_value,
            environment: self.environment,
            stack_base: receiver_at,
            handlers_base: self.handlers.len(),
        });
        self.environment = environment;
        self.this_value = receiver;
        *current = Some(body);
        *at = 0;
        Ok(())
    }
}

/// One suspended call — where to come back to, and what to put back when we do.
///
/// A call does **not** recurse into the interpreter. The loop stays one loop and a frame is a
/// record, which is why a thousand-deep JavaScript recursion costs a thousand small structs
/// rather than a thousand Rust stack frames — and why the limit on it can be a number rather than
/// a guess about the host's stack.
#[derive(Debug)]
pub(super) struct Frame {
    /// The code that was running, and the instruction to come back to.
    pub(super) code: Option<Rc<Chunk>>,
    pub(super) at: usize,
    /// The `this` to go back to.
    pub(super) this_value: Value,
    /// The environment to go back to.
    ///
    /// Not the callee's — that one may outlive the call, if the callee made a closure over it.
    pub(super) environment: EnvironmentId,
    /// Where this frame's operands begin.
    ///
    /// A floor rather than a count: returning truncates back to it, which is what makes a
    /// `return` from the middle of an expression leave nothing of that expression behind.
    pub(super) stack_base: usize,
    /// How many handlers were installed when the call began.
    ///
    /// A `try` inside the callee must not catch on the caller's behalf, and a throw that escapes
    /// the callee must find the caller's handlers intact — so unwinding pops frames and handlers
    /// together, down to this mark.
    pub(super) handlers_base: usize,
}

/// How many calls may be waiting at once before a further one is a **RangeError**.
///
/// Every engine has one and none of them is in the specification: §9.4's note says an
/// implementation may limit recursion and should report it as a RangeError, which is the
/// "Maximum call stack size exceeded" every browser prints.
///
/// The number is about memory rather than about the host's stack, because a call here is a frame
/// *record* and not a Rust frame — the interpreter's loop stays one loop however deep the
/// JavaScript goes. Ten thousand is deeper than any recursion a program means to make and
/// shallow enough that overrunning it costs a few hundred kilobytes rather than the machine.
pub(super) const MAX_CALL_DEPTH: usize = 10_000;
