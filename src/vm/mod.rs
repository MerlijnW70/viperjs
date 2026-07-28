//! The interpreter loop — a stack, a chunk, and one `match`.
//!
//! # Two kinds of failure, and only one of them is the language's
//!
//! A script can fail: it throws, and that is a value travelling upwards. A *chunk* can also be
//! wrong — an instruction that pops two values from a stack holding one, a constant index past
//! the end of the table — and that is not something a script can cause. The compiler does not
//! produce such a chunk; a hand-written one can, and this module answers for it with a [`Fault`]
//! rather than a panic.
//!
//! Keeping the two apart matters. If a malformed chunk were reported as a thrown value, a bug in
//! the compiler would arrive as a `catch` block running, which is the kind of thing that takes a
//! week to find. And if it were a panic, DR-0002 would hold only as long as the compiler is
//! correct, which is not what DR-0002 says.
//!
//! # How this module is laid out
//!
//! - `property` — the object's internal methods a running program reaches: `[[Get]]`, `[[Set]]`,
//!   `[[Delete]]` and `[[HasProperty]]`, each of which can throw.
//! - here — the loop, the frames, and the two kinds of failure.
//!
//! # A throw is an answer, not a failure
//!
//! §6.2.4's Completion Records have five types, and a bytecode compiler turns four of them into
//! jumps: `break`, `continue` and `return` are known at compile time and become instructions.
//! Only **throw** has to travel at run time, because where it lands depends on what the stack
//! looks like when it happens. So an [`Outcome`] is a value or a thrown value, and the rest of
//! §6.2.4 lives in [`crate::compile`].

mod property;

use crate::ast::UnaryOperator;
use crate::compile::{Chunk, Instruction, ShortCircuit};
use crate::heap::{EnvironmentId, Heap, Object, PropertyDescriptor};
use crate::realm::{NativeError, Realm};
use crate::value::{Completion, TypeError, Value, apply_binary};
use std::rc::Rc;

/// A chunk that does not make sense.
///
/// Never reachable from a script. Reachable from a hand-written chunk, which is how it is tested,
/// and from a compiler bug, which is what it exists to make loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// An instruction wanted more values than the stack held.
    StackUnderflow,
    /// A `Constant` instruction pointed past the end of the constant table.
    MissingConstant,
    /// A jump named an instruction past the end of the chunk.
    ///
    /// Includes the placeholder an unpatched forward jump carries, which is `u32::MAX` precisely
    /// so that forgetting to patch one is loud rather than a jump to somewhere plausible.
    JumpOutOfRange,
    /// A `LoadLocal` or `StoreLocal` named a slot the frame does not have.
    MissingLocal,
    /// A `PopHandler` with no matching `PushHandler`.
    UnmatchedPopHandler,
    /// A `MakeFunction` naming a body this chunk does not have.
    MissingFunction,
    /// A `Return` with no call to return from.
    ReturnWithNoCall,
    /// A `DefineField` on something that is not an object.
    ///
    /// Only an object literal emits one, and it emits `NewObject` first, so no chunk the compiler
    /// produces can reach this.
    NotAnObject,
    /// The chunk finished with something still on the stack.
    ///
    /// Every statement is stack-neutral and every expression consumes its operands, so a chunk
    /// that has run to the end has nothing left over. Anything else is a compiler bug that would
    /// otherwise show up much later as the wrong value.
    UnbalancedStack,
}

/// What running a chunk came to.
///
/// Two of §6.2.4's completion types, and the two that a *script* can end with. `break` and
/// `continue` never escape the code that names them, and `return` needs a function to return
/// from.
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    /// The script finished; this is its completion value.
    Value(Value),
    /// The script threw and nothing caught it.
    ///
    /// The value is whatever was thrown, which need not be an Error: `throw 1` is legal and the
    /// specification never asks what it was given.
    Thrown(Value),
}

/// One suspended call — where to come back to, and what to put back when we do.
///
/// A call does **not** recurse into the interpreter. The loop stays one loop and a frame is a
/// record, which is why a thousand-deep JavaScript recursion costs a thousand small structs
/// rather than a thousand Rust stack frames — and why the limit on it can be a number rather than
/// a guess about the host's stack.
#[derive(Debug)]
struct Frame {
    /// The code that was running, and the instruction to come back to.
    code: Option<Rc<Chunk>>,
    at: usize,
    /// The `this` to go back to.
    this_value: Value,
    /// The environment to go back to.
    ///
    /// Not the callee's — that one may outlive the call, if the callee made a closure over it.
    environment: EnvironmentId,
    /// Where this frame's operands begin.
    ///
    /// A floor rather than a count: returning truncates back to it, which is what makes a
    /// `return` from the middle of an expression leave nothing of that expression behind.
    stack_base: usize,
    /// How many handlers were installed when the call began.
    ///
    /// A `try` inside the callee must not catch on the caller's behalf, and a throw that escapes
    /// the callee must find the caller's handlers intact — so unwinding pops frames and handlers
    /// together, down to this mark.
    handlers_base: usize,
}

/// Where a throw goes, and what the stack should look like when it gets there.
#[derive(Debug, Clone, Copy)]
struct Handler {
    /// The instruction to continue at.
    target: u32,
    /// How many calls were waiting when the handler was installed.
    ///
    /// A `try` in a caller must still be found by a throw from a callee, and the jump it makes is
    /// into the *caller's* code — so unwinding pops frames back to here before it jumps, and
    /// `depth` below is a depth in that frame's stack rather than in the one that threw.
    frames: usize,
    /// How deep the operand stack was when the handler was installed.
    ///
    /// A throw in the middle of an expression leaves its half-built operands behind. Without
    /// this, a caught exception would leave rubbish under everything the handler pushed
    /// afterwards, and the imbalance would surface somewhere else entirely.
    depth: usize,
}

/// The interpreter.
///
/// Holds the operand stack and nothing else so far. Call frames, the environment and the job
/// queue join it as the things that need them arrive.
#[derive(Debug)]
pub struct Vm {
    stack: Vec<Value>,
    /// The intrinsics a thrown error is built from.
    realm: Realm,
    /// A throw that nothing caught, kept until the loop can stop.
    ///
    /// The loop cannot return from inside the `match` — an operation that throws has to leave the
    /// program counter somewhere legal so that `while let` ends — so the value waits here.
    escaped: Option<Value>,
    /// The handlers a throw would look at, innermost last.
    handlers: Vec<Handler>,
    /// The calls that are waiting, outermost first.
    frames: Vec<Frame>,
    /// The running function's `this` — §10.2.1.2's binding, decided by the call.
    this_value: Value,
    /// The environment the running code is in.
    ///
    /// Set when [`Vm::run`] begins and changed by every call and return. A variable is found by
    /// walking out from here, which the compiler has already counted the steps for.
    environment: EnvironmentId,
    /// The script's completion value so far — §14.2.2's `UpdateEmpty`, as a register.
    completion: Value,
}

impl Vm {
    /// A machine with an empty stack, belonging to a realm built into `heap`.
    ///
    /// Takes the heap because a machine cannot run without intrinsics: the first TypeError it
    /// throws needs a prototype to be an instance of.
    pub fn new(heap: &mut Heap) -> Self {
        Self {
            realm: Realm::new(heap),
            escaped: None,
            stack: Vec::new(),
            handlers: Vec::new(),
            frames: Vec::new(),
            // Replaced by the script's own before anything runs; a machine with no environment
            // at all is not a state that has to be representable.
            environment: heap.new_environment(None, 0),
            this_value: Value::Undefined,
            completion: Value::Undefined,
        }
    }

    /// Run `chunk` to the end and answer the single value it leaves behind.
    ///
    /// The stack is cleared first, so a machine that faulted once is usable again: a fault says
    /// the chunk was wrong, not that the interpreter is now untrustworthy.
    pub fn run(&mut self, chunk: &Chunk, heap: &mut Heap) -> Result<Outcome, Fault> {
        self.stack.clear();
        self.handlers.clear();
        self.frames.clear();
        self.escaped = None;
        // §14.2.2 — a statement list whose statements all produce nothing has the value
        // `undefined`, which is what `eval("var x")` and `eval(";")` come to.
        self.completion = Value::Undefined;
        // §16.1.7 — the script's own environment, and the root of every chain a function it
        // declares will walk.
        self.environment = heap.new_environment(None, chunk.locals());
        // §16.1.7 — a Script's `this` is the global object. A Module's is `undefined`, which is
        // the one place the two goal symbols disagree about it.
        self.this_value = Value::Object(self.realm.global());
        // `None` is the script itself, which the caller owns; `Some` is a function body, which
        // the function object owns. Two sources rather than one because the root is borrowed and
        // a callee is reference-counted, and moving the root into an `Rc` would make every
        // embedder pay for a call it may never make.
        let mut current: Option<Rc<Chunk>> = None;
        let mut at = 0_usize;
        // A counter rather than an iterator, because a jump moves it. Nothing bounds how long
        // this runs: a backward jump is how a loop will be built, and a script that loops forever
        // is a script that loops forever. DR-0002 is about panics, not about halting.
        loop {
            let running: &Chunk = current.as_deref().unwrap_or(chunk);
            let code = running.code();
            let Some(instruction) = code.get(at).copied() else {
                break;
            };
            at += 1;
            match instruction {
                Instruction::Constant(index) => {
                    // The *running* chunk's table, not the root's. A callee has its own
                    // constants numbered from zero, and reading the caller's by mistake is the
                    // kind of bug that gives a plausible wrong value rather than a crash.
                    let value = running.constant(index).ok_or(Fault::MissingConstant)?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let operand = self.pop()?;
                    let value = apply_unary(operator, operand, heap);
                    match self.settle(value, heap, chunk, &mut current, &mut at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Binary(operator) => {
                    // Right first: it was pushed second, so it is on top. Getting this backwards
                    // would make every subtraction and comparison silently mirror itself.
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let value = apply_binary(operator, left, right, heap);
                    match self.settle(value, heap, chunk, &mut current, &mut at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Jump(target) => at = jump_to(target, code.len())?,
                Instruction::JumpIfFalse(target) => {
                    // The test is consumed either way — this is the conditional operator's jump,
                    // and `a ? b : c` evaluates to `b` or `c` and never to `a`.
                    if !self.pop()?.to_boolean(heap) {
                        at = jump_to(target, code.len())?;
                    }
                }
                Instruction::JumpKeeping(condition, target) => {
                    // Peeked, not popped: if the short circuit fires, this value *is* the answer.
                    let deciding = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let stop = match condition {
                        ShortCircuit::WhenFalsy => !deciding.to_boolean(heap),
                        ShortCircuit::WhenTruthy => deciding.to_boolean(heap),
                        ShortCircuit::WhenNotNullish => {
                            !matches!(deciding, Value::Undefined | Value::Null)
                        }
                    };
                    if stop {
                        at = jump_to(target, code.len())?;
                    } else {
                        // It did not decide, so it is not the answer and the right operand's
                        // value will take its place.
                        self.pop()?;
                    }
                }
                Instruction::JumpIfTrue(target) => {
                    if self.pop()?.to_boolean(heap) {
                        at = jump_to(target, code.len())?;
                    }
                }
                Instruction::Pop => {
                    self.pop()?;
                }
                Instruction::LoadVariable(depth, index) => {
                    let value = heap
                        .environment_at(self.environment, depth)
                        .and_then(|at| heap.variable(at, index))
                        .ok_or(Fault::MissingLocal)?;
                    self.stack.push(value);
                }
                Instruction::StoreVariable(depth, index) => {
                    // Peeked, not popped: assignment is an expression, and `a = (b = 1)` needs
                    // the inner one to leave its value behind.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let target = heap
                        .environment_at(self.environment, depth)
                        .ok_or(Fault::MissingLocal)?;
                    if !heap.set_variable(target, index, value) {
                        return Err(Fault::MissingLocal);
                    }
                }
                Instruction::SetCompletion => {
                    self.completion = self.pop()?;
                }
                Instruction::MakeFunction(index) => {
                    let Some(body) = running.function(index) else {
                        return Err(Fault::MissingFunction);
                    };
                    // The environment the function is *written* in, not the one it will run
                    // in. That is the whole of a closure: `counter()` returns a function that
                    // still holds the environment `counter`'s call made, after the call is gone.
                    let object = heap.new_function(
                        self.realm.function_prototype(),
                        body.clone(),
                        self.environment,
                    );
                    self.stack.push(Value::Object(object));
                }
                Instruction::Call(count) | Instruction::CallMethod(count) => {
                    let method = matches!(instruction, Instruction::CallMethod(_));
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
                            &mut current,
                            &mut at,
                        )?;
                        continue;
                    };
                    let Some(body) = heap.object(object).and_then(Object::call) else {
                        self.throw_type_error(
                            TypeError("what was called is not a function"),
                            heap,
                            chunk,
                            &mut current,
                            &mut at,
                        )?;
                        continue;
                    };
                    let body = body.clone();
                    if self.frames.len() >= MAX_CALL_DEPTH {
                        let thrown =
                            self.realm
                                .error(heap, NativeError::Range, "too much recursion");
                        self.unwind(thrown, chunk, &mut current, &mut at)?;
                        continue;
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
                        code: current.take(),
                        at,
                        this_value: self.this_value,
                        environment: self.environment,
                        stack_base: receiver_at,
                        handlers_base: self.handlers.len(),
                    });
                    self.environment = environment;
                    self.this_value = receiver;
                    current = Some(body);
                    at = 0;
                }
                Instruction::Return => {
                    let value = self.pop()?;
                    let Some(frame) = self.frames.pop() else {
                        return Err(Fault::ReturnWithNoCall);
                    };
                    // Everything the callee left behind goes with it: its operands, its locals,
                    // and any handler it installed and did not take down.
                    self.stack.truncate(frame.stack_base);
                    self.handlers.truncate(frame.handlers_base);
                    self.stack.push(value);
                    self.environment = frame.environment;
                    self.this_value = frame.this_value;
                    current = frame.code;
                    at = frame.at;
                }
                Instruction::LoadThis => self.stack.push(self.this_value),
                Instruction::Duplicate => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    self.stack.push(value);
                }
                Instruction::NewObject => {
                    let object = heap.new_object(Some(self.realm.object_prototype()));
                    self.stack.push(Value::Object(object));
                }
                Instruction::DuplicateTwo => {
                    let depth = self.stack.len();
                    let (Some(first), Some(second)) = (
                        depth
                            .checked_sub(2)
                            .and_then(|at| self.stack.get(at))
                            .copied(),
                        self.stack.last().copied(),
                    ) else {
                        return Err(Fault::StackUnderflow);
                    };
                    self.stack.push(first);
                    self.stack.push(second);
                }
                Instruction::DefineField => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    // Peeked: an object literal defines one property after another and the object
                    // is the expression's value, so it stays where it is until the last one.
                    let base = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let Value::Object(base) = base else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = match self.property_key(key, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.throw_type_error(error, heap, chunk, &mut current, &mut at)?;
                            continue;
                        }
                    };
                    let descriptor = PropertyDescriptor {
                        value: Some(value),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..PropertyDescriptor::EMPTY
                    };
                    // `CreateDataProperty` on a fresh, extensible object cannot be refused.
                    let _ = heap.define_own_property(base, key, &descriptor);
                }
                Instruction::GetProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let got = self.get_property(base, key, heap);
                    match self.settle(got, heap, chunk, &mut current, &mut at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::SetProperty => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.set_property(base, key, value, heap);
                    match self.settle(done, heap, chunk, &mut current, &mut at)? {
                        // §13.15.2 — the value of an assignment is the value assigned, whether or
                        // not the object took it. A silent refusal is what sloppy mode is.
                        Some(_) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::DeleteProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.delete_property(base, key, heap);
                    match self.settle(done, heap, chunk, &mut current, &mut at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::HasProperty => {
                    let base = self.pop()?;
                    let key = self.pop()?;
                    let has = self.has_property(base, key, heap);
                    match self.settle(has, heap, chunk, &mut current, &mut at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Throw => {
                    // §6.2.4 — a throw completion travels up until something wants it. Here that
                    // is the innermost handler; with nothing to want it, it leaves the script.
                    let thrown = self.pop()?;
                    self.unwind(thrown, chunk, &mut current, &mut at)?;
                }
                Instruction::PushHandler(target) => self.handlers.push(Handler {
                    target,
                    frames: self.frames.len(),
                    depth: self.stack.len(),
                }),
                Instruction::PopHandler => {
                    // A pop with nothing to pop is a chunk that does not make sense: the compiler
                    // emits these in pairs.
                    self.handlers.pop().ok_or(Fault::UnmatchedPopHandler)?;
                }
            }
        }
        // Nothing should be left. Anything else means the chunk and the compiler disagree about
        // what the instructions do, and saying so here is cheaper than finding out later.
        if let Some(thrown) = self.escaped {
            return Ok(Outcome::Thrown(thrown));
        }
        if !self.stack.is_empty() {
            return Err(Fault::UnbalancedStack);
        }
        Ok(Outcome::Value(self.completion))
    }

    /// Throw a TypeError with this message, from a place that has no completion to settle.
    fn throw_type_error(
        &mut self,
        error: TypeError,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let TypeError(message) = error;
        let thrown = self.realm.error(heap, NativeError::Type, message);
        self.unwind(thrown, root, current, at)?;
        Ok(())
    }

    /// What to do with an operation that may have thrown.
    ///
    /// `Ok(Some(value))` is the ordinary answer. `Ok(None)` means a handler took the throw and
    /// `at` has been moved to it, so the caller should go round the loop again rather than push
    /// anything. `Err` is a chunk that does not make sense, which is a different thing entirely.
    ///
    /// One place rather than one per instruction, because every operation that converts a value
    /// can now throw and they should all unwind the same way.
    fn settle(
        &mut self,
        outcome: Completion<Value>,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Option<Value>, Fault> {
        let TypeError(message) = match outcome {
            Ok(value) => return Ok(Some(value)),
            Err(error) => error,
        };
        // The value layer said which error; the realm decides what object stands for it. This is
        // the seam described in [`crate::realm`].
        let thrown = self.realm.error(heap, NativeError::Type, message);
        self.unwind(thrown, root, current, at)
    }

    /// Hand `thrown` to the innermost handler, or answer that nothing wanted it.
    fn unwind(
        &mut self,
        thrown: Value,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Option<Value>, Fault> {
        let Some(handler) = self.handlers.pop() else {
            // Nothing wanted it anywhere, in this call or in any that is waiting. The script
            // ends with a throw completion, and the frames go with it.
            self.escaped = Some(thrown);
            self.frames.clear();
            *current = None;
            *at = root.code().len();
            return Ok(None);
        };
        // The handler may belong to a caller. Abandoning the calls between here and there is what
        // an exception *is*: each one's operands, locals and code are dropped on the way past.
        while self.frames.len() > handler.frames {
            let Some(frame) = self.frames.pop() else {
                return Err(Fault::ReturnWithNoCall);
            };
            self.environment = frame.environment;
            self.this_value = frame.this_value;
            *current = frame.code;
            *at = frame.at;
        }
        let length = current.as_deref().unwrap_or(root).code().len();
        self.stack.truncate(handler.depth);
        self.stack.push(thrown);
        *at = jump_to(handler.target, length)?;
        Ok(None)
    }

    /// Take the top of the stack.
    fn pop(&mut self) -> Result<Value, Fault> {
        self.stack.pop().ok_or(Fault::StackUnderflow)
    }
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
const MAX_CALL_DEPTH: usize = 10_000;

/// Where a jump goes, or a fault if that is not inside the chunk.
///
/// `length` itself is a legal target and means "the end": a jump over the last instruction lands
/// there, and the compiler emits exactly that for `a || b` when `b` is the final expression.
/// Anything past it is a chunk that does not make sense — including the `u32::MAX` placeholder a
/// jump carries before it is patched, which is why that placeholder is `u32::MAX`.
fn jump_to(target: u32, length: usize) -> Result<usize, Fault> {
    let target = target as usize;
    if target > length {
        return Err(Fault::JumpOutOfRange);
    }
    Ok(target)
}

/// The unary operators — §13.5.
///
/// `delete` is absent because it takes a reference rather than a value, and the compiler refuses
/// it; the rest are one conversion each, and each of those conversions is already written down.
fn apply_unary(operator: UnaryOperator, operand: Value, heap: &mut Heap) -> Completion<Value> {
    Ok(match operator {
        // §13.5.2 — `void` evaluates its operand and throws the value away. That it evaluates it
        // at all is the point: `void f()` calls `f`.
        UnaryOperator::Void => Value::Undefined,
        // §13.5.3 — the operator that never throws, which is why `typeof undeclared` is the one
        // safe way to ask about a name that may not exist.
        UnaryOperator::Typeof => {
            let text = operand.type_of(heap);
            Value::String(heap.new_string(text.encode_utf16().collect()))
        }
        // §13.5.4 — unary `+` is `ToNumber` and nothing else, which is why `+x` is the shortest
        // spelling of it and why `+"1"` is `1` while `+"a"` is NaN.
        UnaryOperator::Plus => Value::Number(operand.to_number(heap)?),
        // §13.5.5 — `ToNumber` and then negate. Negation is not subtraction from zero: `-0` is
        // `-0` where `0 - 0` is `+0`.
        UnaryOperator::Minus => Value::Number(-operand.to_number(heap)?),
        // §13.5.6 — `ToInt32` and then complement, so `~x` is `-(x + 1)` for a 32-bit `x`, and
        // `~"abc"` is `-1` because NaN becomes `+0` on the way through.
        UnaryOperator::BitwiseNot => Value::Number(f64::from(!operand.to_int32(heap)?)),
        // §13.5.7 — `ToBoolean` and then negate, which is why `!!x` is the shortest cast.
        UnaryOperator::LogicalNot => Value::Boolean(!operand.to_boolean(heap)),
        // Refused by the compiler, which is where the message with a span comes from. Answering
        // `undefined` here means a mistake shows up as a wrong value rather than a plausible one.
        UnaryOperator::Delete => Value::Undefined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOperator;
    use crate::compile::{compile_expression, compile_script};
    use crate::heap::{ObjectId, PropertyKey, PropertyKind};
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
    fn a_script_evaluates_to_its_last_value_producing_statement() {
        // §14.2.2's `UpdateEmpty`. A declaration produces nothing, so it does not replace what
        // came before — which is why the third row is 1 and not `undefined`.
        assert_eq!(run("1;"), "1");
        assert_eq!(run("1; 2;"), "2");
        assert_eq!(run("1; var x = 2;"), "1");
        assert_eq!(run("var x = 2;"), "undefined");
        assert_eq!(run(""), "undefined");
        assert_eq!(run(";;;"), "undefined");
        assert_eq!(run("1; ;"), "1");
        assert_eq!(run("{ 1; }"), "1");
        assert_eq!(run("{ } 1; { }"), "1");
    }

    #[test]
    fn a_var_is_hoisted_so_it_exists_before_its_declaration_and_holds_nothing() {
        // The whole of what hoisting is: the binding is made before the first statement runs and
        // the initializer is not. `x` is readable and `undefined` on the first line.
        assert_eq!(run("var seen = typeof x; var x = 1; seen;"), "undefined");
        assert_eq!(run("var before = x; var x = 1; before;"), "undefined");
        assert_eq!(run("var x = 1; x;"), "1");
        // …including from inside a block or a loop, because `var` belongs to the script rather
        // than to where it was written. That is the difference `let` was introduced to fix.
        assert_eq!(run("{ var inner = 5; } inner;"), "5");
        assert_eq!(
            run("var i = 0; while (i < 1) { var loop_var = 9; i = i + 1; } loop_var;"),
            "9"
        );
        // A second `var` with no initializer does not wipe the first one's value.
        assert_eq!(run("var x = 1; var x; x;"), "1");
        assert_eq!(run("var x = 1; var x = 2; x;"), "2");
    }

    #[test]
    fn assignment_is_an_expression_whose_value_is_what_was_assigned() {
        assert_eq!(run("var a; a = 5;"), "5");
        assert_eq!(run("var a; var b; a = b = 3; a;"), "3");
        assert_eq!(run("var a = 1; a += 2; a;"), "3");
        assert_eq!(run("var a = 1; (a += 2);"), "3");
        assert_eq!(run("var a = 'x'; a += 1; a;"), "x1");
        assert_eq!(run("var a = 8; a /= 2; a;"), "4");
        assert_eq!(run("var a = 5; a **= 2; a;"), "25");
        assert_eq!(run("var a = 12; a &= 10; a;"), "8");
        assert_eq!(run("var a = 1; a <<= 3; a;"), "8");
    }

    #[test]
    fn an_if_runs_one_branch_and_a_missing_else_runs_none() {
        assert_eq!(run("var r = 'none'; if (1) r = 'then'; r;"), "then");
        assert_eq!(run("var r = 'none'; if (0) r = 'then'; r;"), "none");
        assert_eq!(run("var r; if (0) r = 'then'; else r = 'else'; r;"), "else");
        assert_eq!(run("var r; if (1) r = 'then'; else r = 'else'; r;"), "then");
        // Truthiness rather than equality with `true`, and nesting.
        assert_eq!(run("var r = 0; if ('0') r = 1; r;"), "1");
        assert_eq!(run("var r = 0; if ('') r = 1; r;"), "0");
        assert_eq!(run("var r; if (1) if (0) r = 'a'; else r = 'b'; r;"), "b");
    }

    #[test]
    fn the_three_loops_agree_about_when_they_test() {
        // `while` tests first, `do` tests last — so a false condition runs the body once in one
        // of them and never in the other.
        assert_eq!(run("var n = 0; while (0) n = n + 1; n;"), "0");
        assert_eq!(run("var n = 0; do n = n + 1; while (0) n;"), "1");
        assert_eq!(run("var n = 0; while (n < 5) n = n + 1; n;"), "5");
        assert_eq!(run("var n = 0; do n = n + 1; while (n < 5) n;"), "5");
        assert_eq!(
            run("var n = 0; for (var i = 0; i < 5; i = i + 1) n = n + i; n;"),
            "10"
        );
        // A `for` with parts missing: no init, no update, and no test at all.
        assert_eq!(run("var i = 0; for (; i < 3; ) i = i + 1; i;"), "3");
        assert_eq!(
            run("var i = 0; for (;;) { i = i + 1; if (i > 3) break; } i;"),
            "4"
        );
    }

    #[test]
    fn break_leaves_the_loop_and_continue_goes_round_again() {
        assert_eq!(
            run("var n = 0; while (1) { n = n + 1; if (n > 2) break; } n;"),
            "3"
        );
        assert_eq!(
            run(
                "var n = 0; var i = 0; while (i < 5) { i = i + 1; if (i < 3) continue; n = n + 1; } n;"
            ),
            "3"
        );
        // In a `for` loop, `continue` still runs the update — which is the whole reason the third
        // part exists, and the thing a `while` translation gets wrong.
        assert_eq!(
            run(
                "var n = 0; for (var i = 0; i < 5; i = i + 1) { if (i < 3) continue; n = n + 1; } n;"
            ),
            "2"
        );
        assert_eq!(
            run("var i = 0; for (i = 0; i < 5; i = i + 1) { continue; } i;"),
            "5"
        );
        // In a `do` loop, `continue` goes to the *test*, so a loop whose test then fails stops.
        assert_eq!(
            run("var n = 0; do { n = n + 1; continue; } while (n < 3) n;"),
            "3"
        );
        // The innermost loop is the one that is left, and the outer one carries on.
        assert_eq!(
            run(
                "var n = 0; var i = 0; while (i < 3) { i = i + 1; var j = 0; while (1) { j = j + 1; if (j > 1) break; n = n + 1; } } n;"
            ),
            "3"
        );
    }

    #[test]
    fn a_loop_that_never_runs_leaves_the_stack_and_the_completion_value_alone() {
        // The stack-neutrality every statement promises, checked where it is easiest to break: a
        // loop whose body pushes and pops, taken zero times and many times.
        assert_eq!(run("7; while (0) { 1; 2; 3; }"), "7");
        // …and a body that *does* run replaces the completion value, once per iteration.
        assert_eq!(
            run("7; var i = 0; while (i < 3) { i = i + 1; i * 10; }"),
            "30"
        );
        assert_eq!(run("7; for (var i = 0; i < 2; i = i + 1) i;"), "1");
    }

    #[test]
    fn a_script_that_cannot_be_compiled_yet_says_which_construct_and_where() {
        let cases = [
            ("let x = 1;", "let and const"),
            ("const x = 1;", "let and const"),
            ("function* g() {}", "an async function or a generator"),
            ("try { } catch ([a]) { }", "a destructuring catch parameter"),
            ("switch (1) { }", "switch"),
            ("for (var k in 1) ;", "for-in and for-of"),
            ("var [a] = 1;", "a destructuring binding"),
            ("outer: while (1) break outer;", "a labelled statement"),
            ("x;", "a reference to an undeclared name"),
            ("undeclared = 1;", "an assignment to an undeclared name"),
            ("delete x;", "deleting a name"),
            ("var a; a ||= 1;", "a logical assignment"),
        ];
        for (source, what) in cases {
            let mut heap = Heap::new();
            let script = parse_script(source).expect("the source parses"); // the test is about compiling
            let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
            assert_eq!(
                error.kind,
                crate::compile::ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
        }
    }

    #[test]
    fn a_literal_evaluates_to_itself() {
        // The floor everything else stands on. `false` is here rather than assumed because a
        // compiler that pushed `true` for both would pass every other test in this file.
        assert_eq!(eval("1"), "1");
        assert_eq!(eval("1.5"), "1.5");
        assert_eq!(eval("true"), "true");
        assert_eq!(eval("false"), "false");
        assert_eq!(eval("null"), "null");
        assert_eq!(eval("'text'"), "text");
        assert_eq!(eval("''"), "");
        // …and a Number literal is written back the way §6.1.6.1.20 writes it, not the way the
        // source spelled it: `0x10` is `16` and `1e3` is `1000`.
        assert_eq!(eval("0x10"), "16");
        assert_eq!(eval("1e3"), "1000");
        assert_eq!(eval("1_000"), "1000");
        assert_eq!(eval("1e21"), "1e+21");
    }

    #[test]
    fn arithmetic_comes_out_the_way_the_language_says() {
        // Precedence and associativity are the parser's; that they survive into the bytecode is
        // this test's. `**` is the one right-associative operator, so the sixth row is 512 and
        // not 64.
        assert_eq!(eval("1 + 2"), "3");
        assert_eq!(eval("1 + 2 * 3"), "7");
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("7 % 3"), "1");
        assert_eq!(eval("-7 % 3"), "-1");
        assert_eq!(eval("2 ** 3 ** 2"), "512");
        assert_eq!(eval("1 / 0"), "Infinity");
        assert_eq!(eval("-1 / 0"), "-Infinity");
        assert_eq!(eval("0 / 0"), "NaN");
        // Subtraction and division are not commutative, so an operand order bug in the VM shows
        // up here and almost nowhere else.
        assert_eq!(eval("10 - 3"), "7");
        assert_eq!(eval("10 / 4"), "2.5");
        assert_eq!(eval("2 ** -1"), "0.5");
    }

    #[test]
    fn plus_concatenates_as_soon_as_either_side_is_a_string() {
        assert_eq!(eval("'a' + 'b'"), "ab");
        assert_eq!(eval("1 + '1'"), "11");
        assert_eq!(eval("'1' + 1"), "11");
        // …and grouping decides which: the first is `(1 + 2) + "3"`, the second `"3" + 1` then
        // `+ 2`. Left associativity is the whole difference.
        assert_eq!(eval("1 + 2 + '3'"), "33");
        assert_eq!(eval("'3' + 1 + 2"), "312");
        // Every other operator reads the String as a Number instead.
        assert_eq!(eval("'3' - 1"), "2");
        assert_eq!(eval("'3' * '4'"), "12");
        assert_eq!(eval("'a' - 1"), "NaN");
    }

    #[test]
    fn the_unary_operators_are_each_one_conversion() {
        assert_eq!(eval("-'5'"), "-5");
        assert_eq!(eval("+'5'"), "5");
        assert_eq!(eval("+'a'"), "NaN");
        assert_eq!(eval("!0"), "true");
        assert_eq!(eval("!''"), "true");
        assert_eq!(eval("!'0'"), "false");
        assert_eq!(eval("!!1"), "true");
        assert_eq!(eval("~5"), "-6");
        assert_eq!(eval("~'abc'"), "-1");
        assert_eq!(eval("~~1.7"), "1");
        assert_eq!(eval("void 0"), "undefined");
        assert_eq!(eval("typeof 1"), "number");
        assert_eq!(eval("typeof 'a'"), "string");
        assert_eq!(eval("typeof true"), "boolean");
        assert_eq!(eval("typeof null"), "object");
        assert_eq!(eval("typeof void 0"), "undefined");
        // Negation keeps the sign where subtraction does not, and `String` hides it again — so
        // the difference is only visible by dividing into it.
        assert_eq!(eval("1 / -0"), "-Infinity");
        assert_eq!(eval("1 / (0 - 0)"), "Infinity");
    }

    #[test]
    fn comparison_and_equality_agree_with_the_algorithms_they_come_from() {
        assert_eq!(eval("1 < 2"), "true");
        assert_eq!(eval("'10' < '9'"), "true");
        assert_eq!(eval("'10' < 9"), "false");
        // `undefined` is spelled `void 0` here because it is an *identifier*, not a literal —
        // which is exactly why minifiers write it that way, and why the compiler cannot read the
        // other spelling until names resolve.
        assert_eq!(eval("null == void 0"), "true");
        assert_eq!(eval("null === void 0"), "false");
        assert_eq!(eval("'' == 0"), "true");
        assert_eq!(eval("'' === 0"), "false");
        assert_eq!(eval("'1' == true"), "true");
        assert_eq!(eval("'true' == true"), "false");
        assert_eq!(eval("0 / 0 == 0 / 0"), "false");
        assert_eq!(eval("1 <= 1"), "true");
        assert_eq!(eval("(0 / 0) <= 1"), "false");
        assert_eq!(eval("1 << 32"), "1");
        assert_eq!(eval("-1 >>> 0"), "4294967295");
    }

    #[test]
    fn the_three_bitwise_operators_are_three_different_operators() {
        // Chosen so that no two of `&`, `|` and `^` agree: 12 is 1100 and 10 is 1010, so the
        // three answers are 1000, 1110 and 0110. A table of equal-answer rows would let any two
        // of them be swapped without a test noticing.
        assert_eq!(eval("12 & 10"), "8");
        assert_eq!(eval("12 | 10"), "14");
        assert_eq!(eval("12 ^ 10"), "6");
        // Through ToInt32, which is where the 32-bit truncation and the sign come from.
        assert_eq!(eval("2147483648 | 0"), "-2147483648");
        assert_eq!(eval("4294967296 | 0"), "0");
        assert_eq!(eval("-1 & 255"), "255");
        assert_eq!(eval("1.9 | 0"), "1");
        assert_eq!(eval("'abc' | 0"), "0");
        assert_eq!(eval("(0 / 0) | 0"), "0");
    }

    #[test]
    fn each_comparison_is_a_different_comparison() {
        // Every one of the eight, on operands where the answers differ — so that no two of them
        // can be confused for each other and no negation can be dropped.
        assert_eq!(eval("1 < 2"), "true");
        assert_eq!(eval("2 < 1"), "false");
        assert_eq!(eval("1 > 2"), "false");
        assert_eq!(eval("2 > 1"), "true");
        assert_eq!(eval("1 <= 2"), "true");
        assert_eq!(eval("2 <= 1"), "false");
        assert_eq!(eval("1 >= 2"), "false");
        assert_eq!(eval("2 >= 1"), "true");
        // …and the two negations, which a missing `!` would turn into their opposites.
        assert_eq!(eval("1 == 1"), "true");
        assert_eq!(eval("1 != 1"), "false");
        assert_eq!(eval("1 != 2"), "true");
        assert_eq!(eval("1 === 1"), "true");
        assert_eq!(eval("1 !== 1"), "false");
        assert_eq!(eval("1 !== '1'"), "true");
        assert_eq!(eval("1 != '1'"), "false");
    }

    #[test]
    fn an_infinite_exponent_is_nan_only_over_a_base_of_magnitude_one() {
        // §6.1.6.1.3 steps 11 and 12. The guard is a conjunction, and loosening it either way is
        // wrong in a different direction — so both halves need a row that says so.
        assert_eq!(eval("1 ** (1 / 0)"), "NaN");
        assert_eq!(eval("(0 - 1) ** (1 / 0)"), "NaN");
        assert_eq!(eval("2 ** (1 / 0)"), "Infinity");
        assert_eq!(eval("0.5 ** (1 / 0)"), "0");
        assert_eq!(eval("1 ** 2"), "1");
        assert_eq!(eval("(0 - 1) ** 3"), "-1");
    }

    #[test]
    fn a_short_circuit_answers_with_the_operand_that_decided() {
        // The thing that makes `&&` and `||` operators rather than `if` in disguise: the value
        // that stopped the evaluation *is* the answer. `0 || 'a'` is `'a'`, and `1 || 'a'` is
        // `1` and not `true`.
        assert_eq!(eval("1 && 2"), "2");
        assert_eq!(eval("0 && 2"), "0");
        assert_eq!(eval("'' && 2"), "");
        assert_eq!(eval("1 || 2"), "1");
        assert_eq!(eval("0 || 2"), "2");
        assert_eq!(eval("'' || 'a'"), "a");
        assert_eq!(eval("null || 'a'"), "a");
        // Chained, and left-associative: `a && b && c`.
        assert_eq!(eval("1 && 2 && 3"), "3");
        assert_eq!(eval("1 && 0 && 3"), "0");
        assert_eq!(eval("0 || '' || 'last'"), "last");
        // Mixed with an operator that is not short-circuiting, to check the stack comes out level.
        assert_eq!(eval("(1 && 2) + 1"), "3");
        assert_eq!(eval("1 + (0 || 5)"), "6");
    }

    #[test]
    fn nullish_coalescing_asks_a_different_question_from_or() {
        // The whole reason `??` was added: `||` tests truthiness and `??` tests only `null` and
        // `undefined`, so every falsy value that is not nullish is where they part company.
        assert_eq!(eval("0 || 'fallback'"), "fallback");
        assert_eq!(eval("0 ?? 'fallback'"), "0");
        assert_eq!(eval("'' ?? 'fallback'"), "");
        assert_eq!(eval("false ?? 'fallback'"), "false");
        assert_eq!(eval("(0 / 0) ?? 'fallback'"), "NaN");
        // …and where they agree.
        assert_eq!(eval("null ?? 'fallback'"), "fallback");
        assert_eq!(eval("void 0 ?? 'fallback'"), "fallback");
        assert_eq!(eval("1 ?? 'fallback'"), "1");
    }

    #[test]
    fn the_conditional_operator_evaluates_one_branch_and_never_the_test() {
        // Unlike a short circuit, the test is thrown away: `a ? b : c` is `b` or `c` and is never
        // `a`, however truthy `a` was.
        assert_eq!(eval("1 ? 'yes' : 'no'"), "yes");
        assert_eq!(eval("0 ? 'yes' : 'no'"), "no");
        assert_eq!(eval("'' ? 'yes' : 'no'"), "no");
        assert_eq!(eval("'0' ? 'yes' : 'no'"), "yes");
        assert_eq!(eval("null ? 'yes' : 'no'"), "no");
        // Right-associative, so this is `1 ? 'a' : (0 ? 'b' : 'c')` and nesting works in both
        // branches — the two jumps have to be patched to different places.
        assert_eq!(eval("1 ? 'a' : 0 ? 'b' : 'c'"), "a");
        assert_eq!(eval("0 ? 'a' : 0 ? 'b' : 'c'"), "c");
        assert_eq!(eval("0 ? 'a' : 1 ? 'b' : 'c'"), "b");
        assert_eq!(eval("(1 ? 2 : 3) + 10"), "12");
    }

    #[test]
    fn the_comma_operator_keeps_the_last_value_and_discards_the_rest() {
        assert_eq!(eval("(1, 2)"), "2");
        assert_eq!(eval("(1, 2, 3)"), "3");
        assert_eq!(eval("(1, 2) + 1"), "3");
        // Each earlier operand is still *evaluated* — the discarding is of the value, not of the
        // work — which is the only reason anyone writes one.
        assert_eq!(eval("('a' + 'b', 'c')"), "c");
    }

    #[test]
    fn a_throw_that_nothing_catches_leaves_the_script() {
        // §14.14 — anything at all may be thrown, and nothing asks what it is. An Error object
        // would be the usual thing; there are no objects yet and the language never required one.
        assert_eq!(run("throw 1;"), "thrown 1");
        assert_eq!(run("throw 'a' + 'b';"), "thrown ab");
        assert_eq!(run("throw void 0;"), "thrown undefined");
        // Everything after the throw is skipped, including the statement that would have set the
        // completion value.
        assert_eq!(run("1; throw 2; 3;"), "thrown 2");
        assert_eq!(
            run("var n = 0; while (1) { n = n + 1; if (n > 2) throw n; } n;"),
            "thrown 3"
        );
    }

    #[test]
    fn a_catch_block_receives_the_value_and_the_script_carries_on() {
        assert_eq!(run("try { throw 1; } catch (e) { e; }"), "1");
        assert_eq!(
            run("try { throw 'x'; } catch (e) { 'caught ' + e; }"),
            "caught x"
        );
        // The try block's own value survives when nothing is thrown, and the catch block is not
        // entered at all.
        assert_eq!(run("try { 7; } catch (e) { 8; }"), "7");
        // ES2019's optional binding: the value is simply discarded.
        assert_eq!(run("try { throw 1; } catch { 'caught'; }"), "caught");
        // A throw inside a loop inside a try still finds the handler.
        assert_eq!(
            run(
                "try { var i = 0; while (1) { i = i + 1; if (i > 2) throw i; } } catch (e) { 'caught ' + e; }"
            ),
            "caught 3"
        );
    }

    #[test]
    fn a_throw_in_the_middle_of_an_expression_leaves_no_rubbish_behind() {
        // The handler puts the operand stack back to the depth the protected region began at, so
        // the half-built operands of the interrupted expression are discarded rather than left
        // under everything that follows. No source can reach this yet — nothing throws from
        // inside an expression until an operation can — so the chunk is written by hand, the way
        // a malformed one is.
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);
        let chunk = Chunk::from_parts(
            vec![
                // try {
                Instruction::PushHandler(6),
                // …two operands of an expression that never finishes…
                Instruction::Constant(0),
                Instruction::Constant(0),
                // …and a throw from the middle of it.
                Instruction::Constant(1),
                Instruction::Throw,
                Instruction::PopHandler,
                // catch: the thrown value is here and the two operands are not.
                Instruction::SetCompletion,
            ],
            vec![Value::Number(9.0), Value::Number(1.0)],
        );
        // A leftover operand would be an unbalanced stack rather than a wrong answer, which is
        // exactly what makes the balance check worth having.
        let outcome = vm.run(&chunk, &mut heap).expect("well formed"); // the test is about the outcome
        assert_eq!(describe(outcome, &mut heap), "1");
    }

    #[test]
    fn a_nested_try_is_caught_by_the_innermost_handler_that_is_still_open() {
        assert_eq!(
            run("try { try { throw 1; } catch (e) { 'inner ' + e; } } catch (e) { 'outer'; }"),
            "inner 1"
        );
        // A throw from a *catch* block is not caught by its own try.
        assert_eq!(
            run("try { try { throw 1; } catch (e) { throw 2; } } catch (e) { 'outer ' + e; }"),
            "outer 2"
        );
        // …and one that nothing catches still leaves the script.
        assert_eq!(
            run("try { throw 1; } catch (e) { throw e + 1; }"),
            "thrown 2"
        );
    }

    #[test]
    fn a_finally_block_runs_on_both_ways_out() {
        // The normal way…
        assert_eq!(
            run("var log = ''; try { log = log + 'a'; } finally { log = log + 'b'; } log;"),
            "ab"
        );
        // …and the way that carries a thrown value, which then carries on outwards.
        assert_eq!(
            run("var log = ''; try { throw 1; } finally { log = log + 'f'; }"),
            "thrown 1"
        );
        assert_eq!(
            run(
                "var log = ''; try { try { throw 1; } finally { log = log + 'f'; } } catch (e) { log + e; }"
            ),
            "f1"
        );
        // All three tails together, and a throw from the *catch* block still runs the finally.
        assert_eq!(
            run(
                "var log = ''; try { try { throw 1; } catch (e) { log = log + 'c'; throw 2; } finally { log = log + 'f'; } } catch (e) { log + e; }"
            ),
            "cf2"
        );
        // …and when nothing throws at all, the catch is skipped and the finally is not.
        assert_eq!(
            run(
                "var log = ''; try { log = log + 't'; } catch (e) { log = log + 'c'; } finally { log = log + 'f'; } log;"
            ),
            "tf"
        );
    }

    #[test]
    fn a_catch_parameter_shadows_an_outer_name_only_inside_its_block() {
        // §14.15.3 — the parameter is a binding of its own. Inside the block it is the thrown
        // value; outside it, the outer binding is untouched.
        assert_eq!(
            run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; }"),
            "inner"
        );
        assert_eq!(
            run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; } e;"),
            "outer"
        );
        // Assigning to it inside the block does not reach the outer one either.
        assert_eq!(
            run("var e = 'outer'; try { throw 1; } catch (e) { e = 'changed'; } e;"),
            "outer"
        );
    }

    #[test]
    fn leaving_a_try_that_has_a_finally_is_refused_rather_than_skipping_it() {
        // A `break` past a `finally` is a third way out, and the finally would have to run on the
        // way. Refusing is narrow: a loop written *inside* the try is unaffected, which is the
        // second row.
        let mut heap = Heap::new();
        let script = parse_script("while (1) { try { break; } finally { } }").expect("parses"); // the test is about compiling
        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
        );
        // A loop inside the `try` may still be left, because that jump crosses no finally.
        assert_eq!(
            run("var n = 0; try { while (1) { n = n + 1; break; } } finally { n = n + 10; } n;"),
            "11"
        );
        // …and a `break` inside a `try` that has only a `catch` is fine too.
        assert_eq!(
            run("var n = 0; while (1) { try { break; } catch (e) { } } n;"),
            "0"
        );

        // The guard belongs to the `try` that raised it and is put down when that `try` ends, so
        // a `break` *after* one is crossing nothing.
        assert_eq!(
            run("var n = 0; while (1) { try { } finally { } n = 1; break; } n;"),
            "1"
        );
        // …and an inner `try` with no finally does not put down the outer one's guard, so a
        // `break` inside it is still refused.
        let source = "while (1) { try { try { } catch (e) { } break; } finally { } }";
        let script = parse_script(source).expect("parses"); // the test is about compiling
        let error = compile_script(&script, &mut heap).expect_err("still crosses a finally"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
        );
    }

    #[test]
    fn an_object_literal_makes_properties_that_can_be_read_back() {
        assert_eq!(run("var o = {a: 1}; o.a;"), "1");
        assert_eq!(run("var o = {a: 1, b: 2}; o.a + o.b;"), "3");
        assert_eq!(run("var o = {}; o.missing;"), "undefined");
        // Every spelling of a key names the same property, because every one of them is the
        // String `ToString` writes: a quoted name, a bare name, a number, a computed expression.
        assert_eq!(run("var o = {'a': 1}; o.a;"), "1");
        assert_eq!(run("var o = {1: 'x'}; o[1];"), "x");
        assert_eq!(run("var o = {1: 'x'}; o['1'];"), "x");
        assert_eq!(run("var o = {1.0: 'x'}; o[1];"), "x");
        assert_eq!(run("var k = 'a'; var o = {[k]: 1}; o.a;"), "1");
        assert_eq!(run("var o = {1e21: 'x'}; o['1e+21'];"), "x");
        // A later property wins, and it is one property rather than two.
        assert_eq!(run("var o = {a: 1, a: 2}; o.a;"), "2");
    }

    #[test]
    fn a_property_is_written_read_and_deleted_through_the_prototype_chain() {
        assert_eq!(run("var o = {}; o.a = 1; o.a;"), "1");
        assert_eq!(run("var o = {}; o['a'] = 1; o.a;"), "1");
        assert_eq!(run("var o = {a: 1}; o.a = 2; o.a;"), "2");
        // Assignment is an expression whose value is the value assigned.
        assert_eq!(run("var o = {}; o.a = 5;"), "5");
        assert_eq!(run("var o = {}; var x = o.a = 5; x;"), "5");
        // Compound assignment reads and writes the same property.
        assert_eq!(run("var o = {a: 1}; o.a += 2; o.a;"), "3");
        assert_eq!(run("var o = {a: 'x'}; o.a += 'y'; o.a;"), "xy");
        assert_eq!(run("var o = {a: 8}; o['a'] /= 2; o.a;"), "4");
        // `delete` answers whether the property is gone, which is true even when there was none.
        assert_eq!(run("var o = {a: 1}; delete o.a;"), "true");
        assert_eq!(run("var o = {a: 1}; delete o.a; o.a;"), "undefined");
        assert_eq!(run("var o = {}; delete o.nothing;"), "true");
        // …and `in` asks about the chain rather than about own properties.
        assert_eq!(run("var o = {a: 1}; 'a' in o;"), "true");
        assert_eq!(run("var o = {a: 1}; 'b' in o;"), "false");
        assert_eq!(run("var o = {a: 1}; delete o.a; 'a' in o;"), "false");
    }

    #[test]
    fn a_key_is_evaluated_once_even_when_the_property_is_read_and_written() {
        // §13.15.2 — `o[k] += 1` evaluates the key once. With no function calls yet the only way
        // to see that is a key expression with a side effect, and assignment is one: if the key
        // were evaluated twice, `i` would end at 2 and the property written would be `o[1]`.
        assert_eq!(run("var o = {}; var i = 0; o[i = i + 1] = 5; i;"), "1");
        assert_eq!(
            run("var o = {0: 10}; var i = 0; o[i = i + 1] = 5; o[1];"),
            "5"
        );
        assert_eq!(
            run("var o = {1: 10}; var i = 0; o[i = i + 1] += 5; o[1];"),
            "15"
        );
        assert_eq!(
            run("var o = {1: 10}; var i = 0; o[i = i + 1] += 5; i;"),
            "1"
        );
    }

    #[test]
    fn reading_a_property_of_something_that_is_not_an_object_is_a_type_error() {
        // Right for `null` and `undefined`, and temporary for the rest: §7.3.2 wraps a primitive
        // in its own object first, and there is no `String.prototype` to wrap one in yet.
        assert_eq!(run("try { null.a; } catch (e) { e.name; }"), "TypeError");
        assert_eq!(
            run("try { (void 0).a; } catch (e) { e.name; }"),
            "TypeError"
        );
        assert_eq!(
            run("try { null.a = 1; } catch (e) { e.name; }"),
            "TypeError"
        );
        assert_eq!(run("try { 1 in 2; } catch (e) { e.name; }"), "TypeError");
        // The error is an object with a message of its own and a name from its prototype, which
        // is the seam between the value layer and the realm made visible.
        assert_eq!(run("try { null.a; } catch (e) { typeof e; }"), "object");
        assert_eq!(
            run("try { ({}) + 1; } catch (e) { e.name + ': ' + e.message; }"),
            "TypeError: cannot convert an object to a primitive value"
        );
    }

    #[test]
    fn an_object_inherits_from_its_prototype_and_shadows_what_it_writes() {
        // Everything a literal makes inherits from `Object.prototype`, so a property put there is
        // visible from every object — and writing the same name makes an own property that hides
        // it rather than changing it.
        assert_eq!(run("var o = {}; 'nothing' in o;"), "false");
        assert_eq!(run("var o = {a: 1}; var p = {a: 2}; o.a + p.a;"), "3");
        // A property that does not exist reads as `undefined` rather than throwing, which is the
        // difference between a property and a name.
        assert_eq!(run("var o = {}; typeof o.missing;"), "undefined");
        assert_eq!(run("var o = {a: void 0}; 'a' in o;"), "true");
    }

    #[test]
    fn a_long_chain_costs_no_stack_and_a_deep_nest_is_refused() {
        // The two shapes a deep expression comes in, and they need opposite answers.
        //
        // A *chain* — `a + b + c` — is a tree as deep as it is long, and minified code chains
        // thousands of terms. It costs no recursion at all, because the left spine is walked with
        // a loop; two hundred thousand terms compile on a 1 MiB stack where four hundred used to
        // overflow.
        let long_chain = std::iter::repeat_n("1", 5000)
            .collect::<Vec<_>>()
            .join(" + ");
        assert_eq!(run(&long_chain), "5000");
        assert_eq!(
            run(&std::iter::repeat_n("1", 300)
                .collect::<Vec<_>>()
                .join(" + ")),
            "300"
        );

        // A *nest* does recurse, and is refused with a span rather than crashing. The parser
        // stops most of these first — this is the backstop for the ones it does not.
        let nested = format!("{}1{}", "[".repeat(4000), "]".repeat(4000));
        let mut heap = Heap::new();
        if let Ok(script) = parse_script(&nested) {
            let error = compile_script(&script, &mut heap).expect_err("too deep to compile"); // the test is about the error
            assert!(matches!(
                error.kind,
                crate::compile::ErrorKind::TooDeep | crate::compile::ErrorKind::Unsupported(_)
            ));
        }
    }

    /// The property `object` files under `name`, if it has one of its own.
    fn own(heap: &mut Heap, object: ObjectId, name: &str) -> Option<crate::heap::Property> {
        let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
        heap.object(object)?.get_own_property(key).copied()
    }

    fn key_of(heap: &mut Heap, name: &str) -> Value {
        Value::String(heap.new_string(name.encode_utf16().collect()))
    }

    #[test]
    fn an_object_literal_makes_the_same_ordinary_properties_assignment_does() {
        // §13.2.5's `CreateDataPropertyOrThrow` gives all three attributes, and they are *not*
        // §6.1.7.1's defaults: a property a program writes is writable, enumerable and
        // configurable, where one `Object.defineProperty` makes is none of those.
        //
        // No source can see this yet — that needs `for...in` or `getOwnPropertyDescriptor` — but
        // the object itself is the script's completion value, so the heap can be asked directly.
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);
        let script = parse_script("({a: 1})").expect("parses"); // the test is about the object
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
        let outcome = vm.run(&chunk, &mut heap).expect("well formed"); // same
        let Outcome::Value(Value::Object(object)) = outcome else {
            panic!("an object literal evaluates to an object")
        };
        let property = own(&mut heap, object, "a").expect("just defined"); // same
        assert!(property.enumerable);
        assert!(property.configurable);
        assert!(matches!(
            property.kind,
            PropertyKind::Data {
                value: Value::Number(value),
                writable: true
            } if value == 1.0
        ));
    }

    #[test]
    fn assignment_and_a_literal_both_make_an_ordinary_property() {
        // §10.1.9's `CreateDataProperty` and §13.2.5's define give the same three attributes, and
        // they are *not* §6.1.7.1's defaults: a property a program makes is writable, enumerable
        // and configurable, where one `Object.defineProperty` makes is none of those.
        //
        // Nothing in the language can see this yet — that needs `for...in` and
        // `getOwnPropertyDescriptor` — so it is checked where it is decided.
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let object = heap.new_object(None);
        let key = key_of(&mut heap, "a");
        let base = Value::Object(object);
        assert!(matches!(
            vm.set_property(base, key, Value::Number(1.0), &mut heap),
            Ok(Value::Boolean(true))
        ));
        let property = own(&mut heap, object, "a").expect("just assigned"); // the test is about it
        assert!(property.enumerable);
        assert!(property.configurable);
        assert!(matches!(
            property.kind,
            PropertyKind::Data { writable: true, .. }
        ));
    }

    #[test]
    fn assignment_keeps_the_attributes_an_existing_own_property_had() {
        // §10.1.9.2 — writing to an own property changes its value and nothing else. A property
        // that was hidden stays hidden, which is why assigning to a built-in does not suddenly
        // make it turn up in `for...in`.
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let object = heap.new_object(None);
        let hidden = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, hidden, &descriptor));

        let key = key_of(&mut heap, "a");
        let base = Value::Object(object);
        assert!(matches!(
            vm.set_property(base, key, Value::Number(2.0), &mut heap),
            Ok(Value::Boolean(true))
        ));
        let property = own(&mut heap, object, "a").expect("still there"); // same
        assert!(!property.enumerable);
        assert!(!property.configurable);
        assert!(matches!(
            property.kind,
            PropertyKind::Data { value: Value::Number(value), .. } if value == 2.0
        ));
    }

    #[test]
    fn a_write_is_refused_by_what_it_would_have_to_go_through() {
        // The three ways §10.1.9 answers `false`, and none of them throws: a plain assignment in
        // sloppy code swallows the answer, which is why `o.frozen = 1` is silent.
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());

        // A non-writable *inherited* data property refuses the write on the receiver too.
        let frozen = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(prototype, name, &frozen));
        let key = key_of(&mut heap, "a");
        let base = Value::Object(object);
        assert!(matches!(
            vm.set_property(base, key, Value::Number(2.0), &mut heap),
            Ok(Value::Boolean(false))
        ));
        assert!(own(&mut heap, object, "a").is_none());

        // An accessor with no setter refuses as well…
        let setterless =
            PropertyKey::from_units(&mut heap, &"b".encode_utf16().collect::<Vec<_>>());
        let accessor = PropertyDescriptor {
            getter: Some(Value::Undefined),
            setter: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(prototype, setterless, &accessor));
        let key = key_of(&mut heap, "b");
        assert!(matches!(
            vm.set_property(base, key, Value::Number(2.0), &mut heap),
            Ok(Value::Boolean(false))
        ));
        // …and one whose setter is not callable is a TypeError rather than a refusal, because
        // §10.1.9.2 calls it and calling a non-function throws.
        let uncallable =
            PropertyKey::from_units(&mut heap, &"c".encode_utf16().collect::<Vec<_>>());
        let accessor = PropertyDescriptor {
            setter: Some(Value::Number(0.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(prototype, uncallable, &accessor));
        let key = key_of(&mut heap, "c");
        assert!(
            vm.set_property(base, key, Value::Number(2.0), &mut heap)
                .is_err()
        );
    }

    #[test]
    fn a_writable_inherited_property_is_shadowed_rather_than_changed() {
        // §10.1.9.2 again, and the case that makes prototypes useful: writing a name the
        // prototype has puts an *own* property on the receiver and leaves the prototype's alone.
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
        let inherited = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(prototype, name, &inherited));

        let key = key_of(&mut heap, "a");
        let base = Value::Object(object);
        assert!(matches!(
            vm.set_property(base, key, Value::Number(2.0), &mut heap),
            Ok(Value::Boolean(true))
        ));
        assert!(matches!(
            own(&mut heap, object, "a").expect("shadowed").kind, // the test is about it
            PropertyKind::Data { value: Value::Number(value), .. } if value == 2.0
        ));
        assert!(matches!(
            own(&mut heap, prototype, "a").expect("untouched").kind, // same
            PropertyKind::Data { value: Value::Number(value), .. } if value == 1.0
        ));
    }

    #[test]
    fn an_accessor_answers_undefined_without_a_getter_and_throws_with_one() {
        // §10.1.8.1 steps 5 and 6. Nothing is callable yet, so the second is a TypeError for
        // whatever was put there — and both are reachable by defining the property directly,
        // which is why neither is a branch nothing can take.
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let object = heap.new_object(None);
        let getterless =
            PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
        let accessor = PropertyDescriptor {
            getter: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, getterless, &accessor));
        let base = Value::Object(object);
        let key = key_of(&mut heap, "a");
        assert!(matches!(
            vm.get_property(base, key, &mut heap),
            Ok(Value::Undefined)
        ));

        let uncallable =
            PropertyKey::from_units(&mut heap, &"b".encode_utf16().collect::<Vec<_>>());
        let accessor = PropertyDescriptor {
            getter: Some(Value::Number(0.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, uncallable, &accessor));
        let key = key_of(&mut heap, "b");
        assert!(vm.get_property(base, key, &mut heap).is_err());
    }

    #[test]
    fn delete_reaches_only_own_properties_and_a_non_reference_is_always_gone() {
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(prototype, name, &descriptor));
        // Deleting an inherited property answers `true` and leaves it exactly where it was, which
        // is the trap: `delete o.inherited` looks like it worked and `o.inherited` still reads.
        let base = Value::Object(object);
        let key = key_of(&mut heap, "a");
        assert!(matches!(
            vm.delete_property(base, key, &mut heap),
            Ok(Value::Boolean(true))
        ));
        assert!(own(&mut heap, prototype, "a").is_some());
        assert!(matches!(
            vm.get_property(base, key, &mut heap),
            Ok(Value::Number(value)) if value == 1.0
        ));
        // …and deleting something that is not a property reference at all is `true` too, which is
        // why `delete 1` is legal outside strict mode.
        assert_eq!(run("delete (1 + 1);"), "true");
        assert_eq!(run("var n = 0; delete (n = 1); n;"), "1");
    }

    #[test]
    fn optional_chaining_and_private_names_are_refused_with_a_span() {
        let mut heap = Heap::new();
        for (source, what) in [
            ("var o = {}; o?.a;", "optional chaining"),
            ("var o = {}; delete o?.a;", "optional chaining"),
            ("var o = {}; o?.['a'];", "optional chaining"),
            ("var o = {}; delete o?.['a'];", "optional chaining"),
        ] {
            // Every row here parses; a row that did not would silently test nothing, which is
            // how a table of refusals stops refusing anything.
            let script = parse_script(source).expect("the row parses"); // a row that does not is the bug

            let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // the test is about the error
            assert_eq!(
                error.kind,
                crate::compile::ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
        }
    }

    #[test]
    fn a_function_declaration_exists_before_the_line_that_declares_it() {
        // The difference between a declaration and an assignment, and the reason both spellings
        // exist. §10.2.11 *initialises* a function declaration at instantiation time; a `var`
        // holding a function expression is only declared then, and assigned where it is written.
        assert_eq!(run("f(); function f() {} 'ran';"), "ran");
        assert_eq!(run("typeof f; function f() {}"), "function");
        assert_eq!(
            run("try { g(); } catch (e) { e.name; } var g = function () {};"),
            "TypeError"
        );
    }

    #[test]
    fn a_call_passes_its_arguments_and_answers_what_was_returned() {
        assert_eq!(run("function f(a, b) { return a + b; } f(1, 2);"), "3");
        assert_eq!(
            run("function f(a, b) { return a + b; } f(1, 2) + f(10, 20);"),
            "33"
        );
        assert_eq!(run("function f() { return 'x'; } f();"), "x");
        // §10.2.1 step 4 — falling off the end is `undefined`, and so is a bare `return`.
        assert_eq!(run("function f() {} typeof f();"), "undefined");
        assert_eq!(run("function f() { return; } typeof f();"), "undefined");
        assert_eq!(run("function f() { 1; } typeof f();"), "undefined");
        // A parameter the caller did not supply is `undefined`; an argument too many is
        // discarded, since reaching it needs `arguments`.
        assert_eq!(
            run("function f(a, b) { return typeof b; } f(1);"),
            "undefined"
        );
        assert_eq!(run("function f(a) { return a; } f(1, 2, 3);"), "1");
        assert_eq!(run("function f() { return 1; } f(1, 2, 3);"), "1");
    }

    #[test]
    fn a_function_is_a_value_and_says_so() {
        assert_eq!(run("function f() {} typeof f;"), "function");
        assert_eq!(run("typeof function () {};"), "function");
        assert_eq!(run("var f = function () {}; typeof f;"), "function");
        assert_eq!(run("typeof {};"), "object");
        // It can be passed, returned and called through another name — and it is an object, so
        // two of them are never the same value.
        assert_eq!(run("function id(x) { return x; } id(id)(42);"), "42");
        assert_eq!(run("function f() {} var g = f; g === f;"), "true");
        assert_eq!(
            run("function make() { return function () {}; } make() === make();"),
            "false"
        );
        // …and a function is truthy and is an ordinary object otherwise.
        assert_eq!(run("function f() {} f ? 'yes' : 'no';"), "yes");
        assert_eq!(run("function f() {} f.own = 1; f.own;"), "1");
    }

    #[test]
    fn a_functions_own_names_do_not_leak_and_the_scripts_do_not_hide() {
        // A parameter and a `var` belong to the call, so each call gets its own and the script
        // never sees them…
        assert_eq!(
            run("var n = 'outer'; function f(n) { return n; } f('inner') + n;"),
            "innerouter"
        );
        assert_eq!(
            run(
                "var only = 'outer'; function f() { var only = 'inner'; return only; } f() + only;"
            ),
            "innerouter"
        );
        // …while a name declared at the top level is reachable from inside, and writing it
        // reaches the same binding rather than a copy.
        assert_eq!(
            run("var total = 0; function add(n) { total = total + n; } add(2); add(3); total;"),
            "5"
        );
        assert_eq!(
            run("var shared = 'seen'; function f() { return shared; } f();"),
            "seen"
        );
    }

    #[test]
    fn recursion_works_and_runs_out_with_a_range_error_rather_than_a_crash() {
        assert_eq!(
            run("function fact(n) { if (n <= 1) return 1; return n * fact(n - 1); } fact(10);"),
            "3628800"
        );
        assert_eq!(
            run(
                "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } fib(15);"
            ),
            "610"
        );
        // §9.4's note: an implementation may limit recursion and should report it as a
        // RangeError. A frame here is a record rather than a Rust stack frame, so this is a
        // number the engine chose and not the host's stack running out.
        assert_eq!(
            run("function loop(n) { return loop(n + 1); } try { loop(0); } catch (e) { e.name; }"),
            "RangeError"
        );
        // …and the machine is usable afterwards, which is the half that matters: the frames are
        // unwound rather than abandoned.
        assert_eq!(
            run(
                "function loop(n) { return loop(n + 1); } function ok() { return 'fine'; } try { loop(0); } catch (e) { ok(); }"
            ),
            "fine"
        );
    }

    #[test]
    fn calling_something_that_is_not_a_function_is_a_type_error() {
        for source in [
            "var x = 1; x();",
            "var x = 'a'; x();",
            "var x = {}; x();",
            "var x = null; x();",
        ] {
            let script = format!("try {{ {source} }} catch (e) {{ e.name + ': ' + e.message; }}");
            assert_eq!(
                run(&script),
                "TypeError: what was called is not a function",
                "running {source:?}"
            );
        }
    }

    #[test]
    fn a_throw_crosses_a_call_and_finds_the_handler_that_was_waiting() {
        assert_eq!(
            run("function t() { throw 'inside'; } try { t(); } catch (e) { 'caught ' + e; }"),
            "caught inside"
        );
        // Through two calls, and past a `finally` that runs on the way.
        assert_eq!(
            run(
                "var log = ''; function inner() { throw 1; } function outer() { try { inner(); } finally { log = log + 'f'; } } try { outer(); } catch (e) { log + e; }"
            ),
            "f1"
        );
        // A handler *inside* the callee catches first, and the caller's is untouched.
        assert_eq!(
            run(
                "function t() { try { throw 1; } catch (e) { return 'inner'; } } try { t(); } catch (e) { 'outer'; }"
            ),
            "inner"
        );
        // …and the operand stack comes back level, so what follows is computed on a clean one.
        assert_eq!(
            run("function t() { throw 1; } var r; try { r = 1 + t(); } catch (e) { r = 9; } r;"),
            "9"
        );
    }

    #[test]
    fn what_a_function_evaluates_to_is_not_the_scripts_completion_value() {
        // §14.2.2 — the completion value belongs to the script. A statement inside a function
        // discards its value, so calling one cannot change what the script came to.
        assert_eq!(run("7; function f() { 99; } f();"), "undefined");
        assert_eq!(run("function f() { 99; } f(); 7;"), "7");
        assert_eq!(run("7; function f() { 99; }"), "7");
    }

    #[test]
    fn what_functions_cannot_do_yet_says_which_and_where() {
        let cases = [
            ("function* g() {}", "an async function or a generator"),
            ("async function f() {}", "an async function or a generator"),
            ("function f(a = 1) {}", "a default parameter"),
            ("function f(...rest) {}", "a rest parameter"),
            ("function f([a]) {}", "a destructuring parameter"),
            ("function f() {} f(...[1]);", "a spread argument"),
            ("var f = function () {}; f?.();", "optional chaining"),
        ];
        let mut heap = Heap::new();
        for (source, what) in cases {
            let script = parse_script(source).expect("the row parses"); // a row that does not is the bug
            let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
            assert_eq!(
                error.kind,
                crate::compile::ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
        }
    }

    #[test]
    fn a_functions_statements_do_not_touch_the_scripts_completion_value() {
        // §14.2.2 — the completion value is the *script's*. The call itself is an expression
        // statement and sets it, which hides the difference; a call in a *declaration* does not,
        // so this is where a function writing to it would show.
        assert_eq!(run("7; function f() { 99; } var x = f();"), "7");
        assert_eq!(run("7; function f() { 99; } f();"), "undefined");
        assert_eq!(run("function f() { 99; } var x = f(); 'end';"), "end");
    }

    #[test]
    fn the_call_limit_is_a_count_of_frames_and_the_count_is_exact() {
        // The limit is a number this engine chose, so an off-by-one in it is invisible unless
        // something counts. This counts: every entry increments, and the call that is refused is
        // the one that would have made the frames one deeper than allowed.
        let reached = run(
            "var deep = 0; function f() { deep = deep + 1; return f(); } \
             try { f(); } catch (e) { deep; }",
        );
        assert_eq!(reached, MAX_CALL_DEPTH.to_string());
        // …and it is a RangeError rather than anything else, which is what §9.4's note asks for.
        assert_eq!(
            run("function f() { return f(); } try { f(); } catch (e) { e.name; }"),
            "RangeError"
        );
    }

    #[test]
    fn a_closure_keeps_the_variables_of_a_call_that_has_already_returned() {
        // The definition of a closure, and the reason a variable cannot live in a frame: by the
        // time `next` runs, `counter`'s call is over and `n` is still there.
        assert_eq!(
            run(
                "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
                 var next = counter(); next(); next(); next();"
            ),
            "3"
        );
        // Capturing by *value* at creation would answer 1 three times, which is why this is the
        // first row: it is the mistake the whole design exists to avoid.
        assert_eq!(
            run("function adder(x) { return function (y) { return x + y; }; } adder(3)(4);"),
            "7"
        );
    }

    #[test]
    fn each_call_makes_its_own_environment_and_closures_over_it_share_only_that_one() {
        // Two calls to the same function make two environments, so two closures made from them
        // count separately — while two closures from the *same* call share one.
        assert_eq!(
            run(
                "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
                 var a = counter(); var b = counter(); a(); a(); b();"
            ),
            "1"
        );
        assert_eq!(
            run(
                "function counter() { var n = 0; return function () { n = n + 1; return n; }; } \
                 var a = counter(); a(); a(); a();"
            ),
            "3"
        );
        // A recursive call does not overwrite its caller's variables, which is the same rule seen
        // from the other side.
        assert_eq!(
            run("function f(n) { var mine = n; if (n > 0) f(n - 1); return mine; } f(3);"),
            "3"
        );
    }

    #[test]
    fn an_inner_function_writes_the_outer_variable_rather_than_a_copy() {
        assert_eq!(
            run("function o() { var x = 'a'; function set() { x = 'b'; } set(); return x; } o();"),
            "b"
        );
        // Through two levels, which is where a depth counted wrongly would show.
        assert_eq!(
            run(
                "function outer() { var x = 1; function middle() { function inner() { return x; } \
                 return inner(); } return middle(); } outer();"
            ),
            "1"
        );
        assert_eq!(
            run(
                "function outer() { var x = 1; function middle() { function inner() { x = 9; } \
                 inner(); } middle(); return x; } outer();"
            ),
            "9"
        );
        // …and the script's own variables are the far end of the same chain.
        assert_eq!(
            run("var top = 1; function f() { function g() { top = top + 1; } g(); } f(); top;"),
            "2"
        );
    }

    #[test]
    fn a_parameter_is_a_variable_of_the_call_like_any_other() {
        // §10.2.11 — the parameters are the first slots of the call's environment, so a closure
        // over one is a closure over that call's copy.
        assert_eq!(
            run(
                "function hold(x) { return function () { return x; }; } var a = hold(1); \
                 var b = hold(2); a() + b();"
            ),
            "3"
        );
        assert_eq!(
            run(
                "function hold(x) { return function () { x = x + 1; return x; }; } \
                 var f = hold(10); f(); f();"
            ),
            "12"
        );
    }

    #[test]
    fn a_method_call_receives_the_object_it_was_found_on() {
        // §13.3.6.1 — the receiver travels with the *call*, not with the function. The same
        // function called two ways has two different `this`, which is the whole reason a method
        // is not simply a property whose value happens to be callable.
        assert_eq!(
            run("var o = { v: 7 }; o.get = function () { return this.v; }; o.get();"),
            "7"
        );
        assert_eq!(
            run(
                "var o = { v: 7 }; o.get = function () { return this.v; }; var f = o.get; typeof f();"
            ),
            "undefined"
        );
        // The *nearest* base is the receiver, not the outermost one.
        assert_eq!(
            run("var o = { a: { v: 1 } }; o.a.get = function () { return this.v; }; o.a.get();"),
            "1"
        );
        // Arguments still work, and a computed key finds the same method.
        assert_eq!(
            run("var o = { v: 2 }; o.m = function (x) { return this.v + x; }; o.m(3);"),
            "5"
        );
        assert_eq!(
            run("var o = { v: 1 }; o['m'] = function () { return this.v; }; o['m']();"),
            "1"
        );
    }

    #[test]
    fn the_base_of_a_method_call_is_evaluated_exactly_once() {
        // `f().m()` calls `f` once. Compiling the base twice — once to find the method and once
        // to be the receiver — would call it twice, and that is a side effect nobody asked for.
        assert_eq!(
            run("var calls = 0; function base() { calls = calls + 1; \
                 return { m: function () { return 'ok'; } }; } base().m(); calls;"),
            "1"
        );
        // …and a computed key is evaluated once too, which is the same rule one level down.
        assert_eq!(
            run(
                "var keys = 0; var o = { m: function () { return 'ok'; } }; \
                 function key() { keys = keys + 1; return 'm'; } o[key()](); keys;"
            ),
            "1"
        );
    }

    #[test]
    fn a_call_with_no_receiver_gets_the_global_object() {
        // §10.2.1.2's substitution. Strict mode keeps the `undefined` instead, and telling the
        // two apart needs the flag the parser already computes — so this is the sloppy answer,
        // which is what an ordinary script gets.
        assert_eq!(run("function f() { return typeof this; } f();"), "object");
        assert_eq!(run("typeof this;"), "object");
        // The script's `this` and a plain call's are the same object (§16.1.7).
        assert_eq!(
            run("var top = this; function f() { return this === top; } f();"),
            "true"
        );
        // …and a method's is not.
        assert_eq!(
            run("var top = this; var o = { m: function () { return this === top; } }; o.m();"),
            "false"
        );
    }

    #[test]
    fn this_is_restored_when_a_call_returns_however_it_returns() {
        assert_eq!(
            run(
                "var o = { v: 'inner', m: function () { return this.v; } }; \
                 var outer = this; o.m(); typeof this;"
            ),
            "object"
        );
        // Including when the call left by throwing, which unwinds frames rather than returning.
        assert_eq!(
            run("var top = this; var o = { m: function () { throw 1; } }; \
                 try { o.m(); } catch (e) { this === top; }"),
            "true"
        );
    }

    #[test]
    fn a_chunk_that_does_not_make_sense_is_a_fault_and_not_a_panic() {
        // The three ways a chunk can be wrong, each reached by handing the VM one no compiler
        // would produce. A script cannot get here; a compiler bug can, and DR-0002 is a promise
        // about *any* input rather than about correct ones.
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);

        let underflow =
            Chunk::from_parts(vec![Instruction::Binary(BinaryOperator::Add)], Vec::new());
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
        let nothing_to_store =
            Chunk::from_parts(vec![Instruction::StoreVariable(0, 0)], Vec::new());
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
}
