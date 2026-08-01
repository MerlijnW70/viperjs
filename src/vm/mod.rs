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
//! - `execute` — the loop itself, which is one `match` and long enough to want its own file.
//! - `property` — the object's internal methods a running program reaches: `[[Get]]`, `[[Set]]`,
//!   `[[Delete]]` and `[[HasProperty]]`, each of which can throw.
//! - `suspend` — taking an execution out of the loop and putting it back, which is what a
//!   generator and an `async` function are both made of.
//! - `generator` — §15.5.4 and §27.5.1, which are the two ends of that: making a generator, and
//!   resuming one.
//! - `async_fn` — §27.7, which is the same suspension with a promise where the generator was.
//! - here — the loop, the frames, and the two kinds of failure.
//!
//! # A throw is an answer, not a failure
//!
//! §6.2.4's Completion Records have five types, and a bytecode compiler turns four of them into
//! jumps: `break`, `continue` and `return` are known at compile time and become instructions.
//! Only **throw** has to travel at run time, because where it lands depends on what the stack
//! looks like when it happens. So an [`Outcome`] is a value or a thrown value, and the rest of
//! §6.2.4 lives in [`crate::compile`].

mod async_fn;
mod async_generator;
mod call;
mod coerce;
mod execute;
mod generator;
mod global;
mod jobs;
mod property;
mod proxy;
mod proxy_call;
mod proxy_shape;
mod suspend;

use self::call::Frame;
pub(crate) use self::jobs::Job;
pub(crate) use self::suspend::Suspended;

use crate::ast::UnaryOperator;
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, PropertyKey, PropertyKind};
use crate::realm::Realm;
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
    /// A `PopScope` or `CopyScope` where nothing had been pushed — the environment chain has no
    /// parent to go back to. The compiler emits these in pairs with a `PushScope`, so this is a
    /// hand-written chunk rather than anything a source can produce.
    UnmatchedPopScope,
    /// A `MakeFunction` naming a body this chunk does not have.
    MissingFunction,
    /// A `Return` with no call to return from.
    ReturnWithNoCall,
    /// An `Await` where no `async` function is running, or a `Yield` outside a generator.
    ///
    /// Two shapes and one answer: at the top level of a script there is no frame to park at all,
    /// and inside an ordinary function there is a frame that belongs to no generator. The compiler
    /// emits one only inside a generator body, which is exactly where the grammar puts `yield`.
    YieldOutsideGenerator,
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
pub(super) struct Handler {
    /// The instruction to continue at.
    pub(super) target: u32,
    /// How many calls were waiting when the handler was installed.
    ///
    /// A `try` in a caller must still be found by a throw from a callee, and the jump it makes is
    /// into the *caller's* code — so unwinding pops frames back to here before it jumps, and
    /// `depth` below is a depth in that frame's stack rather than in the one that threw.
    pub(super) frames: usize,
    /// How deep the operand stack was when the handler was installed.
    ///
    /// A throw in the middle of an expression leaves its half-built operands behind. Without
    /// this, a caught exception would leave rubbish under everything the handler pushed
    /// afterwards, and the imbalance would surface somewhere else entirely.
    pub(super) depth: usize,
    /// Which environment was in force when it was installed — §8.3.2's running LexicalEnvironment.
    ///
    /// A `throw` out of a block leaves that block's environment behind exactly as it leaves its
    /// operands, and for the same reason: the jump goes to code compiled against the *outer* one,
    /// so a variable read after the `catch` would be read at the wrong depth. Popping frames
    /// already restores this when the handler belongs to a caller; this is the case where it does
    /// not, and the handler is in the same frame as the throw.
    ///
    /// An absolute heap identity rather than a count, so unlike `frames` and `depth` it needs no
    /// rebasing when an execution is parked and revived somewhere else.
    pub(super) environment: EnvironmentId,
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
    /// The running function's `new.target` — §9.1.1.3's `[[NewTarget]]`, decided by the call.
    ///
    /// A register beside `this_value` and for the same reason: both belong to the call rather than
    /// to the function, both are restored by a return, and both are captured by an arrow written
    /// inside. `undefined` in a plain call, which is every call the script itself makes.
    new_target: Value,
    /// The environment the running code is in.
    ///
    /// Set when [`Vm::run`] begins and changed by every call and return. A variable is found by
    /// walking out from here, which the compiler has already counted the steps for.
    environment: EnvironmentId,
    /// The script's completion value so far — §14.2.2's `UpdateEmpty`, as a register.
    completion: Value,
    /// §9.5's jobs, waiting for the stack to empty — see [`jobs`].
    ///
    /// On the VM rather than on the realm because a job is *work in progress*, not an intrinsic:
    /// it belongs to the execution the way the frame stack does, and a second run of a fresh VM
    /// over the same heap starts with nothing waiting.
    ///
    /// A waiting job holds a handler and a capability that nothing else need be holding. Nothing
    /// collects while one is waiting — DR-0013 refuses a heap that has grown too far rather than
    /// collecting — so this is not a root yet, and it is the first thing that must become one when
    /// the collector is wired to the interpreter.
    jobs: std::collections::VecDeque<Job>,
    /// Where a nested execution stops — see [`Floor`].
    floor: Floor,
    /// Instructions still to run before the heap budget is looked at again — DR-0013.
    ///
    /// A countdown rather than a modulus on an instruction count: the check is rare and the
    /// decrement is what every instruction pays, so the cheap operation is the one in the hot
    /// path. It starts at zero so that a script which allocates before its first jump is still
    /// asked about.
    until_heap_check: usize,
    /// §13.2.8.3's template objects, one per tagged-template site that has been reached.
    ///
    /// Kept on the machine rather than on the chunk because the object belongs to a *realm*: a chunk
    /// is immutable and may run in two of them, and each needs its own. Not cleared between runs of
    /// the same machine, which is what makes the identity survive — that is the whole point of it.
    templates: std::collections::HashMap<execute::TemplateSite, crate::heap::ObjectId>,
    /// What the resumption that is about to revive a generator asked for — §27.5.1.3's completion.
    ///
    /// Read by exactly one instruction, `ResumeMode`, which the compiler emits immediately after
    /// every `Yield`. Nothing runs in between, which is what makes a register safe here where a
    /// flag consulted later would not be: it is set by `enter_resume`, read once, and cleared.
    ///
    /// `false` is §27.5.1.2's `next`, which is every other resumption and the common case.
    resume_returns: bool,
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
pub(super) const HEAP_CHECK_INTERVAL: usize = 1_000;

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
            jobs: std::collections::VecDeque::new(),
            stack: Vec::new(),
            handlers: Vec::new(),
            frames: Vec::new(),
            // Replaced by the script's own before anything runs; a machine with no environment
            // at all is not a state that has to be representable.
            environment: heap.new_environment(None, 0),
            this_value: Value::Undefined,
            new_target: Value::Undefined,
            completion: Value::Undefined,
            floor: Floor::default(),
            until_heap_check: 0,
            templates: std::collections::HashMap::new(),
            reentries: 0,
            resume_returns: false,
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
        // §16.1.7 — nothing constructed the script, so its `new.target` is `undefined`. The parser
        // makes `new.target` at the top level a Syntax Error, so no program can read this one; it
        // is set for the same reason `this_value` is, which is that a machine run twice must not
        // start the second time where the first left off.
        self.new_target = Value::Undefined;
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
        // What the *script* did, taken before anything else runs. A throw that nothing caught is
        // waiting in `escaped`, and it has to come out here: a job's own execution takes that slot
        // to carry its own throws, so leaving the script's answer in it lets the first job steal it
        // and report the script's exception as its own. What the script did is then reported as
        // nothing at all, and the stack it left behind becomes an `UnbalancedStack` fault — which
        // is what happened, and what the conformance suite found.
        let escaped = self.escaped.take();
        // §9.5 — the jobs run when no execution context is running, which for a script is here:
        // the last statement has finished and the answer has not been handed back yet. This one
        // line is the whole of what makes `then` asynchronous.
        //
        // They run whether or not the script threw. An uncaught exception ends the script and not
        // the queue: a `then` registered before the throw is still waiting, and a host reports the
        // error and carries on. Nothing a job does can change the answer below, which is already
        // decided.
        self.drain_jobs(heap);
        // §9.5 step 3 — a job's completion is discarded, and so is a throw that escaped one. There
        // is nowhere for it to go: the script that would have caught it has finished.
        self.escaped = None;
        // Nothing should be left. Anything else means the chunk and the compiler disagree about
        // what the instructions do, and saying so here is cheaper than finding out later.
        if let Some(thrown) = escaped {
            return Ok(Outcome::Thrown(thrown));
        }
        if !self.stack.is_empty() {
            return Err(Fault::UnbalancedStack);
        }
        Ok(Outcome::Value(self.completion))
    }

    /// The next name a `for`-`in` should visit, or `undefined` when there are none left.
    ///
    /// §14.7.5.10's iterator step. The list of names was taken once, before the loop began, so
    /// this walks it — and asks the *object* about each one on the way past, because a property
    /// deleted since the list was made must not be visited. A name added since is not in the list
    /// and so is not visited either, which the same clause allows.
    fn enumerate_next(
        &mut self,
        object: Value,
        keys: u32,
        index: u32,
        heap: &mut Heap,
    ) -> Result<Completion<Value>, Fault> {
        loop {
            let at = match heap.variable(self.environment, index) {
                Some(Some(Value::Number(at))) => at,
                // The compiler owns both slots and writes a number into one before the loop
                // begins, so anything else is a chunk that does not make sense.
                _ => return Err(Fault::MissingLocal),
            };
            let Some(Some(Value::Object(list))) = heap.variable(self.environment, keys) else {
                return Err(Fault::MissingLocal);
            };
            let position = u32::try_from(at as u64).unwrap_or(u32::MAX);
            let slot = heap.index_key(position);
            let Some(next) = heap
                .object(list)
                .and_then(|found| found.get_own_property(slot))
            else {
                return Ok(Ok(Value::Undefined));
            };
            let PropertyKind::Data { value, .. } = next.kind else {
                return Err(Fault::MissingLocal);
            };
            if !heap.set_variable(self.environment, index, Value::Number(at + 1.0)) {
                return Err(Fault::MissingLocal);
            }
            let Value::String(name) = value else {
                return Err(Fault::MissingLocal);
            };
            // Still there? A `delete` inside the body reaches this before the name does — and the
            // question is §7.3.11 `HasProperty`, so a proxy answers it with its `has` trap and may
            // throw, which is why this hands back a completion rather than a value.
            let key = PropertyKey::from_string(heap, name);
            if let Value::Object(_) = object {
                match self.has_property_key(object, key, heap) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => return Ok(Err(error)),
                }
            }
            return Ok(Ok(value));
        }
    }

    /// Throw, from a place that has no completion to settle.
    ///
    /// Any kind of error, not only a TypeError: this was named after its first caller and is
    /// now handed §9.1.1.1.6's ReferenceError too.
    pub(super) fn raise(
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
    pub(crate) fn thrown_value(&self, error: Abrupt, heap: &mut Heap) -> Value {
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
            self.new_target = frame.new_target;
            *current = frame.code;
            *at = frame.at;
        }
        let length = current.as_deref().unwrap_or(root).code().len();
        // …and the block the throw was inside is left too. Written after the loop above rather
        // than instead of it: popping frames restores the *caller's* environment, and this is the
        // one the handler itself was installed in, which may be several blocks further in.
        self.environment = handler.environment;
        self.stack.truncate(handler.depth);
        self.stack.push(thrown);
        *at = jump_to(handler.target, length)?;
        Ok(None)
    }

    /// Free everything this machine can no longer reach, and answer how much that was.
    ///
    /// The host's to call, and deliberately: the interpreter does not run this on a schedule of its
    /// own because `Heap::footprint` counts arena slots and DR-0010 does not reuse a swept one, so
    /// a collection cannot lower what the budget is measured against for objects. Until it can, a
    /// timer inside the loop costs more than it saves — measured, twice, in `execute`.
    ///
    /// An embedder knows things the loop does not: when a request finished, when a frame was
    /// drawn, when nothing is holding the values a script just made. `root` is the chunk being run
    /// or the one about to be, whose constants are Strings the machine will reach for.
    ///
    /// # Safety of a sort
    ///
    /// Calling this between two `run`s is what the tests do and what an embedder should. Calling it
    /// is safe at any time in the Rust sense — nothing here can dangle a pointer — but a root set
    /// is a claim about what a *program* can still name, so this is not for calling from inside a
    /// native while the interpreter is mid-instruction: the operands of the instruction in flight
    /// are on the stack and rooted, but a value a Rust local is holding and has not pushed is not.
    pub fn collect(&self, root: &Chunk, heap: &mut Heap) -> crate::heap::Collected {
        let roots = self.roots(root);
        heap.collect(&roots)
    }

    /// Everything a running program can still name — §9's execution contexts, as a root set.
    ///
    /// The list the collector cannot work out for itself, and the one place in the engine where
    /// being *incomplete* is worse than being wrong: a missing root does not fail, it frees
    /// something a later instruction reads. So it is written against the fields of [`Vm`] and
    /// [`super::call::Frame`] rather than against a memory of what they hold, and every one of
    /// them appears here or is deliberately absent with a reason.
    ///
    /// What is deliberately absent: `steps`, `floor`, `until_heap_check` and the `at`/`stack_base`
    /// marks are numbers; `code` is an `Rc<Chunk>` whose *constants* are named through
    /// [`Chunk::names`]; the intern table is not a root at all, which is what lets a name a program
    /// computed and dropped be collected.
    ///
    /// `root` is the chunk being run, which the machine does not hold: it is lent to `run` and its
    /// constants are the Strings the outermost code is about to use.
    fn roots(&self, root: &Chunk) -> crate::heap::Roots {
        let mut values = Vec::new();
        let mut environments = vec![self.environment];

        values.extend(self.stack.iter().copied());
        values.extend([self.this_value, self.new_target, self.completion]);
        values.extend(self.escaped);
        values.extend(self.templates.values().copied().map(Value::Object));
        // Everything the realm built before a script ran. Not the individual intrinsics, because a
        // list of those is one an intrinsic added later is left out of — see `Realm::intrinsics`.
        values.extend(self.realm.intrinsics().map(Value::Object));
        root.names(&mut values);

        for frame in &self.frames {
            values.extend([frame.this_value, frame.new_target]);
            values.extend(frame.constructed);
            values.extend(frame.function.map(Value::Object));
            values.extend(frame.generator.map(Value::Object));
            environments.push(frame.environment);
            if let Some(code) = &frame.code {
                code.names(&mut values);
            }
        }
        // A handler remembers the environment it was installed in, and a throw is going to restore
        // it — so it is live however deeply the block it belongs to has been left.
        environments.extend(self.handlers.iter().map(|handler| handler.environment));
        // §9.5's queue. A job that has not run yet holds the only reference to what it will run
        // with: a promise reaction names its handler, its capability and the value it settled.
        for job in &self.jobs {
            job.names(&mut values);
        }
        crate::heap::Roots {
            values,
            environments,
        }
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
pub(super) fn jump_to(target: u32, length: usize) -> Result<usize, Fault> {
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
