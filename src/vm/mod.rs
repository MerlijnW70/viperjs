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
//! - `call` — what happens when a function is entered: a frame, an environment, a `this`.
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

mod call;
mod coerce;
mod global;
mod property;

use self::call::{Entry, Frame};

use crate::ast::UnaryOperator;
use crate::compile::{Chunk, Instruction, ShortCircuit};
use crate::heap::{EnvironmentId, Heap, PropertyDescriptor};
use crate::realm::{NativeError, Realm};
use crate::value::{Abrupt, Completion, Value};
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
    /// Where a nested execution stops — see [`Floor`].
    floor: Floor,
    /// Instructions still to run before the heap budget is looked at again — DR-0013.
    ///
    /// A countdown rather than a modulus on an instruction count: the check is rare and the
    /// decrement is what every instruction pays, so the cheap operation is the one in the hot
    /// path. It starts at zero so that a script which allocates before its first jump is still
    /// asked about.
    until_heap_check: usize,
    /// How many nested executions are running, which is how much Rust stack they are using.
    ///
    /// The main loop does not recurse: ten thousand nested JavaScript calls cost ten thousand
    /// small structs and no Rust frames at all. A *coercion* is the exception, because the answer
    /// is needed in the middle of an instruction — so each one is a real Rust call, counted here
    /// and bounded far lower than [`crate::vm::call::MAX_CALL_DEPTH`].
    reentries: usize,
}

/// How many instructions run between two looks at the heap budget — DR-0013.
///
/// A thousand is chosen from both ends. Small enough that a loop allocating on every pass cannot
/// get far past the budget before it is stopped — a thousand objects is under a hundred kilobytes,
/// which is nothing beside a 256 MiB limit — and large enough that the check is lost in the noise
/// of the instructions around it.
const HEAP_CHECK_INTERVAL: usize = 1_000;

/// Where a nested execution stops, and how far a throw inside it may travel.
///
/// §7.1.1's `ToPrimitive` calls a method, and that method may throw. The throw has to come back
/// to whatever asked for the conversion — an operator, half-way through evaluating — rather than
/// jumping into a `try` in the *caller*, because the Rust call that started the nested execution
/// is still on the stack waiting for an answer. Jumping past it would leave that call stranded.
///
/// So a nested execution has a floor. A handler below it belongs to the code that started the
/// execution and is not this one's to use; a throw that reaches the floor stops and travels the
/// rest of the way as a return value instead.
#[derive(Debug, Clone, Copy, Default)]
struct Floor {
    /// Handlers below this index belong to the code that started the nested execution.
    handlers: usize,
    /// Frames below this index likewise.
    frames: usize,
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
            floor: Floor::default(),
            until_heap_check: 0,
            reentries: 0,
        }
    }

    /// The intrinsics this machine belongs to — §9.3's realm.
    ///
    /// `Copy`, so this hands one out rather than lending it: a built-in that needs
    /// `%Object.prototype%` should not be holding a borrow of the machine it is running inside.
    pub fn realm(&self) -> Realm {
        self.realm
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
        self.execute(chunk, heap, &mut current, &mut at)?;
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

    /// Run instructions until the code runs out, or until a nested call has returned.
    ///
    /// One loop rather than two, so the two paths into it can never disagree about what an
    /// instruction does. A nested execution stops without being told to: its root chunk is empty,
    /// so the moment `Return` points the program counter back at it there is nothing to read.
    fn execute(
        &mut self,
        root: &Chunk,
        heap: &mut Heap,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        loop {
            // DR-0013 — the heap has a budget, and this is where a script that has spent it finds
            // out. Between instructions rather than at each allocation: the allocating functions
            // answer handles rather than completions, and making forty of them fallible for a
            // condition the loop can see from here would put a refusal on every one of their
            // callers. It is the shape `MAX_CALL_DEPTH` already uses.
            //
            // Counted down rather than asked every time. `Heap::footprint` is cheap but it is not
            // free, and a loop body of three instructions should not pay for it three times.
            //
            // The counter is reset *before* the throw rather than after it. Written the other way
            // round — as one assignment whose `None` arm throws — the `continue` leaves the
            // expression before the assignment happens, the counter stays at zero, and every pass
            // through the loop raises the error again. Each of those raises allocates the Error
            // object it is about to throw, so the check meant to stop a runaway becomes one.
            if self.until_heap_check > 0 {
                self.until_heap_check -= 1;
            } else {
                self.until_heap_check = HEAP_CHECK_INTERVAL;
                if heap.is_exhausted() {
                    let thrown = self.realm.error(
                        heap,
                        NativeError::Range,
                        "the heap has grown past what this engine will allocate",
                    );
                    // Nothing catches it in the usual case, and then `unwind` points the program
                    // counter past the end of the code — so the loop reads no instruction and
                    // stops, which is what makes this a refusal rather than a spin.
                    self.unwind(thrown, root, current, at)?;
                    continue;
                }
            }
            let running: &Chunk = current.as_deref().unwrap_or(root);
            let code = running.code();
            let Some(instruction) = code.get(*at).copied() else {
                return Ok(());
            };
            *at += 1;
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
                    let value = self.unary(operator, operand, heap);
                    match self.settle(value, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Binary(operator) => {
                    // Right first: it was pushed second, so it is on top. Getting this backwards
                    // would make every subtraction and comparison silently mirror itself.
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let value = self.binary(operator, left, right, heap);
                    match self.settle(value, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Jump(target) => *at = jump_to(target, code.len())?,
                Instruction::JumpIfFalse(target) => {
                    // The test is consumed either way — this is the conditional operator's jump,
                    // and `a ? b : c` evaluates to `b` or `c` and never to `a`.
                    if !self.pop()?.to_boolean(heap) {
                        *at = jump_to(target, code.len())?;
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
                        *at = jump_to(target, code.len())?;
                    } else {
                        // It did not decide, so it is not the answer and the right operand's
                        // value will take its place.
                        self.pop()?;
                    }
                }
                Instruction::JumpIfTrue(target) => {
                    if self.pop()?.to_boolean(heap) {
                        *at = jump_to(target, code.len())?;
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
                Instruction::LoadGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    // §6.2.5.5 `GetValue` — an unresolvable reference is a ReferenceError, and
                    // this is the line that makes `missing` differ from `o.missing`.
                    let Some(read) = self.global_binding(key, heap) else {
                        let message = self.missing_global(key, heap);
                        let thrown = self.realm.error(heap, NativeError::Reference, &message);
                        self.unwind(thrown, root, current, at)?;
                        continue;
                    };
                    match self.settle(read, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::StoreGlobal(index) => {
                    // Peeked, not popped, for the same reason `StoreVariable` peeks.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let key = self.global_name(running, index, heap)?;
                    let global = Value::Object(self.realm.global());
                    // §6.2.5.6 `PutValue`: a name that resolves to nothing is created on the
                    // global object. That is the *sloppy* answer — strict code throws a
                    // ReferenceError instead — and this engine does not yet carry a strictness
                    // through to here, so it gives the one that is right for a Script's default.
                    let stored = self.set_property_key(global, key, value, heap);
                    // No `continue`: this is the end of the arm either way, so a handled throw
                    // and an ordinary store leave the loop in the same place.
                    self.settle(stored, heap, root, current, at)?;
                }
                Instruction::TypeofGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    // §13.5.1.1 step 2 — no such global is `"undefined"`, not a throw.
                    let read = match self.global_binding(key, heap) {
                        Some(read) => read,
                        None => Ok(Value::Undefined),
                    };
                    let answer = match self.settle(read, heap, root, current, at)? {
                        Some(value) => value.type_of(heap),
                        None => continue,
                    };
                    let id = heap.new_string(answer.encode_utf16().collect());
                    self.stack.push(Value::String(id));
                }
                Instruction::DeclareGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    self.declare_global(key, heap);
                }
                Instruction::Bury(depth) => {
                    // Removed from the top and inserted lower down rather than swapped along:
                    // everything it passes keeps its order, and only this value moves.
                    let value = self.pop()?;
                    let at = self
                        .stack
                        .len()
                        .checked_sub(depth as usize)
                        .ok_or(Fault::StackUnderflow)?;
                    self.stack.insert(at, value);
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
                    //
                    // An arrow captures the `this` in force here for exactly the same reason and
                    // at exactly the same moment (§10.2.3 step 6). Reading it at *call* time
                    // instead would be dynamic `this` wearing a lexical name: the two agree only
                    // while the arrow is called from inside the call that made it.
                    let lexical_this = body.is_arrow().then_some(self.this_value);
                    let object = heap.new_function(
                        self.realm.function_prototype(),
                        body.clone(),
                        self.environment,
                        lexical_this,
                    );
                    // §10.2.5's `MakeConstructor`: every ordinary function gets a `prototype`
                    // object, and that object gets a `constructor` back. The pair is what makes
                    // `new f() instanceof f` true, and it is made eagerly because a function may
                    // be constructed with at any time — including before anything reads it.
                    //
                    // An arrow gets neither. §15.3 gives it no `[[Construct]]`, so a `prototype`
                    // would be an object nothing could ever inherit from.
                    if !body.is_arrow() {
                        self.realm.make_constructor(heap, object);
                    }
                    self.stack.push(Value::Object(object));
                }
                Instruction::Call(count) | Instruction::CallMethod(count) => {
                    let method = matches!(instruction, Instruction::CallMethod(_));
                    let how = if method { Entry::Method } else { Entry::Plain };
                    self.enter(how, count, heap, root, current, at)?;
                }
                Instruction::Construct(count) => {
                    self.enter(Entry::Construct, count, heap, root, current, at)?;
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
                    // §10.2.2 step 13 — a construction answers with the object it made, unless
                    // the body returned an object of its own. A primitive `return` is *ignored*,
                    // which is why `function F() { return 1; }` still constructs an `F`.
                    let answer = match (frame.constructed, value) {
                        (Some(_), Value::Object(_)) | (None, _) => value,
                        (Some(made), _) => made,
                    };
                    self.stack.push(answer);
                    self.environment = frame.environment;
                    self.this_value = frame.this_value;
                    *current = frame.code;
                    *at = frame.at;
                }
                Instruction::LoadThis => self.stack.push(self.this_value),
                Instruction::Duplicate => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    self.stack.push(value);
                }
                Instruction::NewArray(length) => {
                    let prototype = self.realm.array_prototype();
                    let array = heap.new_array(prototype, length);
                    self.stack.push(Value::Object(array));
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
                            self.throw_type_error(error, heap, root, current, at)?;
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
                Instruction::DefineGetter | Instruction::DefineSetter => {
                    let function = self.pop()?;
                    let key = self.pop()?;
                    let base = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let Value::Object(base) = base else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = match self.property_key(key, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.throw_type_error(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    // Only the half that was written. §10.1.6.3 leaves an absent field alone, so
                    // a getter defined after a setter joins it rather than replacing it — which
                    // is what makes `{get a() {}, set a(v) {}}` one property with both.
                    let half = match instruction {
                        Instruction::DefineGetter => PropertyDescriptor {
                            getter: Some(function),
                            ..PropertyDescriptor::EMPTY
                        },
                        _ => PropertyDescriptor {
                            setter: Some(function),
                            ..PropertyDescriptor::EMPTY
                        },
                    };
                    // §15.4.5 gives an accessor made this way `[[Enumerable]]` and
                    // `[[Configurable]]`, the same two an ordinary literal property gets.
                    let descriptor = PropertyDescriptor {
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..half
                    };
                    let _ = heap.define_own_property(base, key, &descriptor);
                }
                Instruction::GetProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let got = self.get_property(base, key, heap);
                    match self.settle(got, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::SetProperty => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.set_property(base, key, value, heap);
                    match self.settle(done, heap, root, current, at)? {
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
                    match self.settle(done, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::HasProperty => {
                    let base = self.pop()?;
                    let key = self.pop()?;
                    let has = self.has_property(base, key, heap);
                    match self.settle(has, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Instanceof => {
                    // Right first: it was pushed second, so it is on top — the same order every
                    // binary operator here pops in.
                    let target = self.pop()?;
                    let value = self.pop()?;
                    let answer = self.instance_of(value, target, heap);
                    match self.settle(answer, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Throw => {
                    // §6.2.4 — a throw completion travels up until something wants it. Here that
                    // is the innermost handler; with nothing to want it, it leaves the script.
                    let thrown = self.pop()?;
                    self.unwind(thrown, root, current, at)?;
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
    }

    /// Throw, from a place that has no completion to settle.
    fn throw_type_error(
        &mut self,
        error: Abrupt,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let thrown = self.thrown_value(error, heap);
        self.unwind(thrown, root, current, at)?;
        Ok(())
    }

    /// The value a `catch` block receives, out of an abrupt completion.
    ///
    /// The seam described in [`crate::realm`], in one place: the value layer says *which* error
    /// and why, and the realm decides what object stands for it. A completion that already
    /// carries a value has nothing to decide — it is the one that was thrown, and building a
    /// second object from its parts would hand the program a different error than the one it
    /// raised.
    fn thrown_value(&self, error: Abrupt, heap: &mut Heap) -> Value {
        match error {
            Abrupt::Raised(kind, message) => self.realm.error(heap, kind, message),
            Abrupt::Thrown(value) => value,
        }
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
        let error = match outcome {
            Ok(value) => return Ok(Some(value)),
            Err(error) => error,
        };
        let thrown = self.thrown_value(error, heap);
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
        // A handler below the floor belongs to the code that started this nested execution, and
        // reaching into it would jump out of a Rust call that is still waiting for an answer. The
        // throw stops here instead and travels the rest of the way as a return value.
        let found = match self.handlers.len() > self.floor.handlers {
            true => self.handlers.pop(),
            false => None,
        };
        let Some(handler) = found else {
            // Nothing wanted it anywhere this execution can reach. For a script that is the end
            // of it; for a nested one it is an abrupt completion for whoever asked.
            self.escaped = Some(thrown);
            self.frames.truncate(self.floor.frames);
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
mod tests;
