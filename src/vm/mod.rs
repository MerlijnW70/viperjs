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
//! - `async_generator` — §27.6, which is both of those at once and neither of them.
//! - `coerce` — §7.1.1's `ToPrimitive`, and the one place Rust re-enters the loop to get it.
//! - `dynamic` — §9.4.2's `ResolveBinding` done at run time, which is what `with` costs.
//! - `eval` — §19.2.1.1's **direct** mode, which resolves into the scopes the caller is *running*
//!   in. The indirect half is a built-in; this one cannot be, because a native call has no handle
//!   on its caller's environment.
//! - `global` — §9.1.1.4's Global Environment Record, where a name falls when it falls out of
//!   every scope.
//! - `jobs` — §9.5's queue, and what it means for a job to run "later" (DR-0016).
//! - `loader` — §16.2.1.7's `HostLoadImportedModule`, which is the host's and not the language's.
//! - `module` — §16.2.1's records: linking a graph of modules, and evaluating it in order.
//! - `proxy` — §10.5's four internal methods a property access goes through.
//! - `proxy_call` — §10.5.12 and §10.5.13, the two a proxy has only *sometimes*.
//! - `proxy_shape` — §10.5's other seven, the ones about an object's shape rather than its values.
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
mod dynamic;
mod eval;
mod execute;
mod generator;
mod global;
mod jobs;
mod loader;
mod module;
mod property;
mod proxy;
mod proxy_call;
mod proxy_shape;
mod suspend;

pub use self::loader::ModuleLoader;
pub use self::module::{Graph, LinkError};

use self::call::Frame;
pub(crate) use self::jobs::Job;
pub(crate) use self::suspend::Suspended;

use crate::ast::UnaryOperator;
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, PropertyKey, PropertyKind};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use std::rc::Rc;

/// How far [`Vm::realm_of`] follows a chain of proxies and bound functions before it stops.
///
/// §10.1.14 recurses and the depth is the program's: `bind` over `bind` over a `Proxy` nests as far
/// as a script cares to build one. DR-0006's rule is that a nesting the input decides is *counted*,
/// and the number is generous for the same reason `MAX_CALL_DEPTH` is — a chain this long is a
/// program looking for the wall, and what it gets is step 5's answer rather than a wrong one.
const MAX_REALM_CHAIN: usize = 1_000;

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
    /// A `PushScope` or `CopyScope` naming a scope this chunk does not have.
    ///
    /// The compiler records the entry before it patches the instruction that names it, so no chunk
    /// it produces can reach this. A hand-written one can, which is the only way it is tested.
    MissingScope,
    /// A `Return` with no call to return from.
    ReturnWithNoCall,
    /// An `Await` where no `async` function is running, or a `Yield` outside a generator.
    ///
    /// Two shapes and one answer: at the top level of a script there is no frame to park at all,
    /// and inside an ordinary function there is a frame that belongs to no generator. The compiler
    /// emits one only inside a generator body, which is exactly where the grammar puts `yield`.
    YieldOutsideGenerator,
    /// An `ImportMeta` where the running code belongs to no module.
    ///
    /// §13.3.12's early error makes `import.meta` a Syntax Error under any goal but Module, and the
    /// parser applies it — so no source reaches this. A hand-written chunk can, which is how it is
    /// tested.
    ImportMetaOutsideModule,
    /// A `LoadThrough` or `StoreThrough` with no Reference waiting.
    ///
    /// The compiler emits the three in one sequence with nothing between them that could unwind
    /// past a `ResolveName`, so no source reaches this. A hand-written chunk can.
    MissingReference,
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
/// Two of §6.2.4's completion types — the two a *script* can end with, since `break` and `continue`
/// never escape the code that names them and `return` needs a function to return from — and one
/// case that is not a completion at all, because the machine stopped before the code produced one.
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    /// The script finished; this is its completion value.
    Value(Value),
    /// The script threw and nothing caught it.
    ///
    /// The value is whatever was thrown, which need not be an Error: `throw 1` is legal and the
    /// specification never asks what it was given.
    Thrown(Value),
    /// The run spent its time budget and was stopped — DR-0022.
    ///
    /// **Not one of §6.2.4's completion types**, and that is the point: no completion of any kind
    /// left the code, because the machine stopped reading instructions. There is no value, because
    /// the script did not produce one and whatever it was part-way through is not an answer.
    ///
    /// A third case rather than a `Thrown` carrying a distinguished error, because a throw is
    /// something a script can catch and this must not be. See DR-0022.
    Interrupted,
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
    /// How many §9.4.2 References were waiting when the handler was installed.
    ///
    /// The same discipline as `depth` and for the same reason: a compound assignment resolves its
    /// target, evaluates a right-hand side that may throw, and only then writes back. The
    /// resolution is half-built state exactly as an operand is, so a throw has to abandon it.
    pub(super) references: usize,
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

/// §25.4.1.2's Waiter Record, for the one agent this engine has.
///
/// **A waiter here is not a parked thread**, and that is why the list is worth keeping at all.
/// `Atomics.waitAsync` does not block: the agent that parks a waiter carries straight on and may
/// wake its *own* waiter with `Atomics.notify` a statement later. test262's
/// `undefined-for-timeout.js` does exactly that — four waits on an infinite timeout, then one
/// notify — where a blocking `Atomics.wait` in a single agent could never be woken by anybody and
/// is refused outright.
pub(crate) struct Waiter {
    /// The buffer holding the position. Two views over one buffer share a list.
    pub(crate) buffer: crate::heap::ObjectId,
    /// The **byte** offset in that buffer, so views of different element widths agree about
    /// whether they name the same position — §25.4.1 keys a list on a block and a byte index, and
    /// an element index would make an `Int32Array`'s slot 1 miss a `BigInt64Array`'s slot 0.
    pub(crate) byte: usize,
    /// The promise `waitAsync` answered with, settled with `"ok"` when a notify reaches it.
    pub(crate) capability: crate::heap::Capability,
}

/// The interpreter.
///
/// Holds the operand stack and nothing else so far. Call frames, the environment and the job
/// queue join it as the things that need them arrive.
pub struct Vm {
    stack: Vec<Value>,
    /// The intrinsics a thrown error is built from — the realm this machine is running in.
    realm: Realm,
    /// Every realm this machine has built, indexed by its [`RealmId`] — the first at zero.
    ///
    /// This is what a function's `[[Realm]]` names, and it is what the collector walks. §9.3 does
    /// not destroy a realm and neither does this, so nothing is ever removed and an id stays valid
    /// for the life of the machine.
    ///
    /// The running realm above is `realms[0]` today, and the repetition is deliberate rather than
    /// missed: `realm` answers **which one is running** and this answers **which one is number n**.
    /// They come apart the moment a frame can carry a realm, which is DR-0025's next step; until
    /// then a `Realm` is `Copy` and is never mutated after `Realm::new` returns, so the two have no
    /// way to disagree.
    realms: Vec<Realm>,
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
    /// §25.4.1.2's waiter records — the agent's own `Atomics.waitAsync` parks, still unwoken.
    ///
    /// On the machine for the same reason the modules below are: a waiter outlives the call that
    /// made it, and the promise it is holding is reachable from nothing else the collector walks
    /// until a notify or a `then` finds it.
    waiters: Vec<Waiter>,
    /// Every module this execution has linked, by chunk identity — §16.2.1's records.
    ///
    /// On the machine rather than in a local of the link, because §16.2.1.6's "each body once" is a
    /// fact about the whole execution and not about one call: a dynamic `import()` arriving later
    /// has to find what an earlier one evaluated, and find the *same* namespace object for it.
    modules: std::collections::BTreeMap<usize, loader::ModuleRecord>,
    /// Every specifier that has been answered, and the module it answered with.
    ///
    /// The memo in front of [`ModuleLoader`], so a host is never asked twice for one specifier. A
    /// `Graph` the caller handed to [`Vm::run_module_graph`] is merged into this, which is what
    /// lets a dynamic `import()` of a statically-loaded specifier find the module already there
    /// rather than loading a second copy of it.
    resolved: Graph,
    /// What each specifier *meant* where it was written — DR-0020's `(referrer, specifier) -> key`.
    ///
    /// Beside `resolved` rather than replacing it, because the two answer different questions: this
    /// says which module a piece of text named, and `resolved` says what that module is. A pair
    /// that is absent resolves to the specifier itself, which is what a host that supplied a whole
    /// `Graph` of unique names meant and is exactly the behaviour that predates this record.
    edges: std::collections::BTreeMap<(String, String), String>,
    /// Which key each chunk was registered under, so a module can be asked what it is called.
    ///
    /// Keyed by the chunk's address, which is what `identity` already uses for cycle detection.
    /// Needed because linking walks *chunks* and a specifier has to be resolved against the one
    /// that wrote it.
    keys: std::collections::BTreeMap<usize, String>,
    // No `Debug` on the machine any more: a host's loader is the host's type and cannot be made to
    // have one, so a derive here would demand it of every embedder. What a debug print of the field
    // would say is "there is one", which is not worth that.
    /// How the host answers a specifier at run time — §16.2.1.7, and `None` until one is given.
    ///
    /// A `Box<dyn>` and not a type parameter, because the machine is one type across every
    /// embedding and a parameter here would spread to `Heap`, to every builtin and to the public
    /// API. The call happens once per module ever loaded, so the indirection costs nothing that a
    /// parameter would buy back.
    loader: Option<Box<dyn ModuleLoader>>,
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
    /// Instructions still to run before the periodic checks are made — DR-0013 and DR-0022.
    ///
    /// A countdown rather than a modulus on an instruction count: the checks are rare and the
    /// decrement is what every instruction pays, so the cheap operation is the one in the hot
    /// path. It starts at zero so that a script which allocates before its first jump is still
    /// asked about.
    ///
    /// **One counter scheduling two checks**, which is not the "one field answering two questions"
    /// mistake: both ask the same thing of it — *is it time for housekeeping* — and neither reads
    /// it for a second meaning. Two counters would be two intervals that could drift apart, and
    /// the one nobody read would be the one that mattered.
    until_check: usize,
    /// How long a single [`Vm::run`] may take, if the host has said — DR-0022.
    ///
    /// A duration and not an instant, because the limit belongs to a *run*: an instant fixed once
    /// would be an engine that stops working at a wall-clock moment rather than one that bounds
    /// what a script may take. `None` is no budget, which is what every caller gets until a host
    /// says otherwise.
    time_budget: Option<std::time::Duration>,
    /// When the run in progress must stop, derived from `time_budget` when it began.
    ///
    /// Read only inside the periodic check, so a run with no budget never asks the clock at all.
    expires_at: Option<std::time::Instant>,
    /// How much the arena may **grow** between collections before the loop runs one itself.
    ///
    /// `None` is the historical behaviour: no collection happens unless a host asks for one.
    ///
    /// Growth and not size, because [`crate::heap::Heap::footprint`] is a high-water mark that
    /// never falls. A threshold on the total would fire once and then at every check for ever,
    /// since a collection cannot lower the number it is being compared against. Growth *since the
    /// last collection* is the question that has an answer: DR-0019 hands a swept slot out again,
    /// so a program whose live set is steady stops growing the arena entirely and never triggers a
    /// second pass, while one that really is accumulating triggers as often as it accumulates.
    collect_after_growth: Option<usize>,
    /// The footprint when the last collection finished — the base `collect_after_growth` measures
    /// from. Set at the start of a run so a fresh machine does not count the realm as growth.
    collected_at: usize,
    /// How much growth the *next* collection waits for, which is the base or the live set,
    /// whichever is larger — see the site in `execute` for the measurement that made it
    /// proportional rather than fixed.
    collect_next: usize,
    /// Whether the machine has been stopped and must read no further instruction — DR-0022.
    ///
    /// Checked before an instruction is read, which is what makes it reach *every* execution: a
    /// coercion re-entered from the middle of an instruction, a native's callback, a job. Cleared
    /// when a run begins, so an interrupted machine is usable again and a host does not have to
    /// build a new one.
    stopped: bool,
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
    /// §9.4.2's References that a compound assignment has resolved and not yet written through.
    ///
    /// **A stack and not a register.** The right-hand side is evaluated between the read and the
    /// write, and it may contain another compound assignment — `with (o) { a += f() }` where `f`
    /// does the same thing inside a `with` of its own. A register would be clobbered by the inner
    /// one and the outer would write through it.
    ///
    /// Truncated wherever the operand stack is: by a handler when a throw is caught, and by a frame
    /// when a call returns. That is the discipline this needs and the reason it is not a field.
    references: Vec<dynamic::Resolved>,
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
        // Realm zero, and the machine's realm table starts with it — DR-0025.
        let first = Realm::new(heap, crate::heap::RealmId(0));
        Self {
            waiters: Vec::new(),
            modules: std::collections::BTreeMap::new(),
            resolved: Graph::new(),
            edges: std::collections::BTreeMap::new(),
            keys: std::collections::BTreeMap::new(),
            loader: None,
            realm: first,
            realms: vec![first],
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
            references: Vec::new(),
            until_check: 0,
            time_budget: None,
            // DR-0023 — **on**. An engine that cannot run `for (i = 0; i < 1e6; i++) s = f(i)` is
            // not one anybody can embed, and that loop reached DR-0013's budget at about 900,000
            // calls until this line. One mebibyte of *growth*, and the allowance after each
            // collection is the live set, so a program holding a great deal is not walked over and
            // over — see the site in `execute` and the record for both measurements.
            collect_after_growth: Some(1 << 20),
            collected_at: 0,
            collect_next: 0,
            expires_at: None,
            stopped: false,
            templates: std::collections::HashMap::new(),
            reentries: 0,
            resume_returns: false,
        }
    }

    /// Run a Module's body — §16.2.1.6 `ExecuteModule`.
    ///
    /// [`Vm::run`] with one thing changed, and it is the one thing §16.1.7 and §16.2.1.6 disagree
    /// about: a Module's `this` is **`undefined`** where a Script's is the global object. Every
    /// other difference between the two goal symbols is decided when the body is compiled — see
    /// [`crate::compile::compile_module`] — and none of it reaches here.
    pub fn run_module(&mut self, chunk: &Chunk, heap: &mut Heap) -> Result<Outcome, Fault> {
        // §16.2.1 — a module that runs **is** a module record, even when nothing imported it and
        // there is no graph to link. Registering one here is what gives `import.meta` a module to
        // belong to: without it a lone module reading `import.meta` would find no record and report
        // a Fault, which is an engine-bug channel and not an answer to an ordinary program.
        //
        // Keyed by the chunk's address, which is what the linker uses too, and with no entry in
        // `keys` — this module has no resolved identity because no host resolved it, so §16.2.1.9
        // is not asked and the object is the empty one it describes.
        let environment =
            heap.new_named_environment(None, chunk.locals(), Rc::clone(chunk.bindings()));
        self.modules
            .entry(std::ptr::from_ref(chunk) as usize)
            .or_insert_with(|| loader::ModuleRecord {
                environment,
                namespace: None,
                import_meta: None,
                // `true`, and it is read: §16.2.1.6's "each body once" is a fact about the
                // machine and not about the link, so a graph that later imports this same chunk
                // finds it evaluated and does not run the body again. The linker's own insert is
                // guarded on the entry being *vacant*, which is what makes this record the one it
                // then reads.
                evaluated: true,
                failure: None,
            });
        self.run_prepared(chunk, environment, Value::Undefined, heap)
    }

    /// Run one module's body in an environment the link step already made — §16.2.1.6.
    ///
    /// Apart from [`Vm::run_module`] because a linked module's environment is *not* fresh: its
    /// import slots were bound before anything ran, and making a new one here would throw that
    /// away and leave every imported name unbound.
    fn run_module_in(
        &mut self,
        chunk: &Chunk,
        environment: crate::heap::EnvironmentId,
        heap: &mut Heap,
    ) -> Result<Outcome, Fault> {
        self.run_prepared(chunk, environment, Value::Undefined, heap)
    }

    /// §16.2.1.5.3 — run an **asynchronous** module's body, and answer the promise it settles.
    ///
    /// A body that may `await` has to be able to park, and parking pops a *frame* and records the
    /// chunk it was running — so unlike every other module body this one is entered as a callee
    /// rather than as the root: an empty chunk stands in for "the code that started this", which is
    /// Rust, and the module's own chunk becomes the running one. That is the same shape
    /// [`Vm::nested_body`] uses for a call made from Rust, and for the same reason.
    ///
    /// The environment is the one the link step made and bound imports into, which is why this
    /// cannot be an ordinary call: `enter` would give the body a fresh scope, and every import
    /// would then be a slot of a scope nothing had bound.
    fn run_async_module(
        &mut self,
        chunk: &Rc<Chunk>,
        environment: crate::heap::EnvironmentId,
        heap: &mut Heap,
    ) -> Result<Result<Value, Value>, Fault> {
        // Nothing of the machine is cleared, and that is the difference from every other way of
        // starting a body: an asynchronous module is evaluated from a **job** as often as from a
        // host, and a job runs inside an execution that owns the stack. Clearing it took the
        // operands of whatever was running and left the promise behind where they had been, which
        // the outer chunk then reported as an unbalanced stack — a `Fault`, from ordinary source.
        let floor = std::mem::replace(
            &mut self.floor,
            Floor {
                handlers: self.handlers.len(),
                frames: self.frames.len(),
            },
        );
        let base = self.stack.len();
        let handlers_base = self.handlers.len();
        let outer_environment = self.environment;
        let outer_this = self.this_value;
        let outer_target = self.new_target;
        self.environment = environment;
        self.this_value = Value::Undefined;
        self.new_target = Value::Undefined;
        // §27.7.5.1's capability, made without going through the constructor: this promise is the
        // module's own and no script can have replaced what makes it.
        let Some(context) = self.begin_async(heap) else {
            return Err(Fault::NotAnObject);
        };
        let Some(capability) = self.capability_of(context, heap) else {
            return Err(Fault::NotAnObject);
        };
        let promise = capability.promise;
        let root = Chunk::from_parts(Vec::new(), Vec::new());
        let mut current: Option<Rc<Chunk>> = Some(Rc::clone(chunk));
        let mut at = 0_usize;
        self.frames.push(Frame {
            // Nothing to come back to: the loop stops when this frame is popped, because the
            // chunk it returns into has no instructions.
            code: None,
            at: 0,
            this_value: outer_this,
            new_target: outer_target,
            // §16.2.1.6.2 gives a module a realm of its own; ViperJS has one graph in one realm, so
            // a module body runs in the realm that asked for it and goes back to the same.
            realm: self.realm.id(),
            environment: outer_environment,
            stack_base: base,
            handlers_base,
            constructed: None,
            // No function object — a module body is not one, and nothing here asks for the callee.
            function: None,
            // What `Await` parks into and what `Return` settles, which is the whole point.
            generator: Some(context),
        });
        let ran = self.execute(&root, heap, &mut current, &mut at);
        // Whatever happened. `Return` and `Await` both restore what the frame recorded, but a
        // `Fault` returns without unwinding anything — and this call has an outer execution to hand
        // back to either way.
        self.stack.truncate(base);
        self.handlers.truncate(handlers_base);
        self.floor = floor;
        self.environment = outer_environment;
        self.this_value = outer_this;
        self.new_target = outer_target;
        ran?;
        // A throw the wrapper did not catch cannot happen — §27.7.5.2's handler is the outermost
        // thing in the body — but a chunk is only as good as the compiler that made it, and a
        // rejection is the honest answer for one that escaped rather than a lost error.
        if let Some(thrown) = self.escaped.take() {
            return Ok(Err(thrown));
        }
        Ok(Ok(promise))
    }

    /// The intrinsics this machine belongs to — §9.3's realm.
    ///
    /// `Copy`, so this hands one out rather than lending it: a built-in that needs
    /// `%Object.prototype%` should not be holding a borrow of the machine it is running inside.
    pub fn realm(&self) -> Realm {
        self.realm
    }

    /// The realm an id names, or the running one if it names none.
    ///
    /// An id is only ever handed out by [`Vm::create_realm`] and a realm is never removed, so the
    /// fallback is not a case a program can reach through an id. It is reachable through a
    /// *function* that has no `[[Realm]]` at all, which is what [`Vm::realm_of`] leans on — and
    /// making that the same answer in both places is what keeps this total without a branch nothing
    /// can take.
    #[must_use]
    pub fn realm_by_id(&self, id: crate::heap::RealmId) -> Realm {
        self.realms
            .get(id.0 as usize)
            .copied()
            .unwrap_or(self.realm)
    }

    /// Only the `[[Realm]]` slot §10.1.14 step 2 reads — `None` where the clause has none.
    ///
    /// The half of [`Vm::realm_of`] that does **not** recurse and does not fall back, which is what
    /// a caller wants when the absence is the answer rather than a reason to guess. §10.3.1's realm
    /// switch is that caller: a built-in runs in its own realm, and something with no realm of its
    /// own must not move the running one at all.
    #[must_use]
    pub fn own_realm(&self, function: crate::heap::ObjectId, heap: &Heap) -> Option<Realm> {
        let object = heap.object(function)?;
        // A proxy is asked about first for the reason `realm_of` gives: §10.5 gives it a
        // `[[Call]]` through `Heap::make_callable`, so it carries a `Callable::Native` and a realm
        // the clause never gave it. `None` here is the whole point — §10.5.12 is an *internal
        // method* and not a built-in function, so calling through a proxy pushes no execution
        // context and the running realm stays the caller's. `Proxy/apply/arguments-realm.js`
        // measures exactly that, by asking which realm made the trap's arguments array.
        if object.proxy().is_some() {
            return None;
        }
        match object.call()? {
            crate::heap::Callable::Bytecode { realm, .. }
            | crate::heap::Callable::Native { realm, .. } => Some(self.realm_by_id(*realm)),
            _ => None,
        }
    }

    /// §10.1.14 `GetFunctionRealm` — which realm `function` belongs to.
    ///
    /// Step 2 reads the `[[Realm]]` slot, which here is [`Vm::own_realm`]. Steps 3 and 4 are the
    /// two exotic objects that have none of their own and answer by recursing: a **bound function**
    /// into its `[[BoundTargetFunction]]`, a **Proxy** into its `[[ProxyTarget]]`. Step 5 — anything
    /// else — is the *current* realm, which is where §27.5.1's resumption methods and §27.7.5.3's
    /// revive closures land.
    ///
    /// **Written as a loop rather than as the recursion the clause is spelled with**, because the
    /// depth is a program's to choose: `Proxy` over `Proxy` over a bound function nests as far as a
    /// script cares to build, and DR-0006 already says a nesting the input decides is counted
    /// rather than trusted to the Rust stack. A chain longer than `MAX_REALM_CHAIN` is a program
    /// looking for the wall, and what it gets is step 5's answer rather than a wrong one.
    ///
    /// A **revoked** Proxy has neither target nor handler, and §10.1.14 throws a TypeError for it.
    /// This answers the running realm instead, deliberately: every caller here is looking for a
    /// *default prototype*, and a revoked proxy cannot be a `new.target` that got this far —
    /// §7.3.13 `Construct` refuses it several steps earlier with a TypeError of its own. Turning
    /// this into a `Completion` for a case no program can reach would put a `?` on every caller.
    #[must_use]
    pub fn realm_of(&self, function: crate::heap::ObjectId, heap: &Heap) -> Realm {
        let mut at = function;
        for _ in 0..MAX_REALM_CHAIN {
            let Some(object) = heap.object(at) else {
                return self.realm;
            };
            // Asked before the `[[Call]]` below, because §10.5 gives a proxy its `[[Call]]` through
            // `Heap::make_callable` — so a proxy *does* hold a `Callable::Native` here, carrying a
            // realm the clause never gives it. Reading that would answer whoever built the proxy
            // rather than whoever wrote the target.
            if let Some(proxy) = object.proxy() {
                match proxy.parts() {
                    Some((target, _)) => at = target,
                    None => return self.realm,
                }
                continue;
            }
            match object.call() {
                Some(crate::heap::Callable::Bytecode { realm, .. })
                | Some(crate::heap::Callable::Native { realm, .. }) => {
                    return self.realm_by_id(*realm);
                }
                Some(crate::heap::Callable::Bound(bound)) => at = bound.target,
                _ => return self.realm,
            }
        }
        self.realm
    }

    /// Build a second §9.3 realm on the same heap, and answer it — DR-0025.
    ///
    /// A whole new set of intrinsics: its own `Object.prototype`, its own `Array`, its own global.
    /// **Not** its own well-known Symbols, which §6.1.5.1 shares between realms and the heap
    /// therefore owns — so `other.Symbol.iterator` is `Symbol.iterator` and an object made here is
    /// iterable there.
    ///
    /// One heap, so a value crosses freely and `Reflect.construct(OtherArray, [])` needs no
    /// marshalling. That is what §9.3's realm is; a membrane between the two sides is `ShadowRealm`,
    /// which is a different proposal and is not this.
    ///
    /// The realm is remembered so the collector keeps it — a caller that dropped the returned
    /// `Realm` would otherwise leave every one of its intrinsics unreachable while a script still
    /// held its global.
    ///
    /// What is **not** here yet is running code in it: nothing switches the machine's realm, so a
    /// function taken from the new global still resolves its intrinsics against the old one. See
    /// DR-0025 for the two steps that finish it.
    pub fn create_realm(&mut self, heap: &mut Heap) -> Realm {
        // The id is taken *before* the realm is built, because everything `Realm::new` makes stamps
        // it onto the functions it creates — a realm has to know its own number to build itself.
        let id = crate::heap::RealmId(u32::try_from(self.realms.len()).unwrap_or(u32::MAX));
        let realm = Realm::new(heap, id);
        self.realms.push(realm);
        realm
    }

    /// Run `chunk` to the end and answer the single value it leaves behind.
    ///
    /// The stack is cleared first, so a machine that faulted once is usable again: a fault says
    /// the chunk was wrong, not that the interpreter is now untrustworthy.
    pub fn run(&mut self, chunk: &Chunk, heap: &mut Heap) -> Result<Outcome, Fault> {
        let global = Value::Object(self.realm.global());
        self.run_with_this(chunk, global, heap)
    }

    /// What both goal symbols do, given the `this` each is owed.
    fn run_with_this(
        &mut self,
        chunk: &Chunk,
        this_value: Value,
        heap: &mut Heap,
    ) -> Result<Outcome, Fault> {
        // §16.1.7 — the code's own environment, and the root of every chain a function it declares
        // will walk. Named, because a direct `eval` at the top level resolves into it exactly as
        // one inside a function resolves into that call's.
        let environment =
            heap.new_named_environment(None, chunk.locals(), Rc::clone(chunk.bindings()));
        self.run_prepared(chunk, environment, this_value, heap)
    }

    /// The same, in an environment the caller already has — §16.2.1.6's linked module.
    ///
    /// A linked module's environment is *not* fresh: its import slots were bound to other modules'
    /// before anything ran, and making a new one here would throw that away and leave every
    /// imported name unbound.
    fn run_prepared(
        &mut self,
        chunk: &Chunk,
        environment: crate::heap::EnvironmentId,
        this_value: Value,
        heap: &mut Heap,
    ) -> Result<Outcome, Fault> {
        self.stack.clear();
        self.handlers.clear();
        self.frames.clear();
        self.escaped = None;
        // DR-0022 — the budget is per run, so the deadline is taken here rather than when the host
        // set it. Clearing `stopped` here and not at the end of the previous run is what makes an
        // interrupted machine usable again: the flag has to survive the unwinding of every nested
        // execution above, and the only moment it is certainly finished doing that is the next
        // time a run begins.
        self.stopped = false;
        self.expires_at = self
            .time_budget
            .map(|budget| std::time::Instant::now() + budget);
        // The growth the schedule measures is this run's, so the base is taken here for the same
        // reason the deadline is: a machine that ran once already has a footprint, and counting it
        // as growth would collect on the first check of every later run.
        self.begin_collection_window(heap);
        // §14.2.2 — a statement list whose statements all produce nothing has the value
        // `undefined`, which is what `eval("var x")` and `eval(";")` come to.
        self.completion = Value::Undefined;
        self.environment = environment;
        // §16.1.7 — a Script's `this` is the global object. A Module's is `undefined` (§16.2.1.6),
        // which is the one place the two goal symbols disagree about it, and so is the one thing
        // the caller has to say.
        self.this_value = this_value;
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
        // DR-0022 — before anything is read out of the machine. A stopped run has no answer: the
        // completion register holds whatever the last finished statement left, which is not what
        // the script came to, and §9.5's queue must not be drained because a job is code like any
        // other and a `then` handler that loops for ever is the same problem wearing a promise.
        if self.stopped {
            return Ok(Outcome::Interrupted);
        }
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
            self.realm = self.realm_by_id(frame.realm);
            *current = frame.code;
            *at = frame.at;
        }
        let length = current.as_deref().unwrap_or(root).code().len();
        // …and the block the throw was inside is left too. Written after the loop above rather
        // than instead of it: popping frames restores the *caller's* environment, and this is the
        // one the handler itself was installed in, which may be several blocks further in.
        self.environment = handler.environment;
        self.stack.truncate(handler.depth);
        // Beside the operand stack, because a resolved-and-unwritten reference is half-built state
        // of exactly the same kind — see `Vm::references`.
        self.references.truncate(handler.references);
        self.stack.push(thrown);
        *at = jump_to(handler.target, length)?;
        Ok(None)
    }

    /// Bound how long a single [`Vm::run`] may take — DR-0022, and `None` to remove the bound.
    ///
    /// Exceeding it is **not a throw**: the machine stops reading instructions and `run` answers
    /// [`Outcome::Interrupted`]. A script cannot catch it, a `finally` does not run, and neither
    /// does §9.5's job queue. That is what makes it a bound on untrusted code rather than a
    /// suggestion — see the record for why the heap's budget is catchable and this one is not.
    ///
    /// The bound is honoured to within one check interval, which is a thousand instructions.
    /// **It does not cover** the regular expression matcher, a single long-running built-in, or a
    /// host function that blocks; DR-0022 says why each is a separate piece of work.
    pub fn set_time_budget(&mut self, budget: Option<std::time::Duration>) {
        self.time_budget = budget;
    }

    /// Start a run's collection window — DR-0023's base, taken where a run begins.
    ///
    /// Two callers, and the second is the reason this is a method rather than two lines: `Vm::run`
    /// and `Vm::run_module_graph` are both "a run", and only the first has a preamble. A graph that
    /// skipped this measured its growth from zero, so the realm's own footprint cleared every
    /// threshold before a single module statement had executed.
    pub(super) fn begin_collection_window(&mut self, heap: &Heap) {
        self.collected_at = heap.footprint();
        // The first collection of a run waits for the base; every one after it is told by the live
        // set the previous one left.
        self.collect_next = self.collect_after_growth.unwrap_or(0);
    }

    /// Let the loop collect for itself once the arena has grown this much since the last one.
    ///
    /// `None`, the default, is the behaviour every version until now: the collector runs only when
    /// a host calls [`Vm::collect`]. What that costs is not abstract — a function call retains
    /// about 74 bytes of arena, so `for (i = 0; i < 1e6; i++) s = f(i)` reaches DR-0013's budget
    /// and throws a RangeError somewhere past 800,000 calls. With a threshold set it does not.
    ///
    /// # Why the argument is growth rather than a ceiling
    ///
    /// [`crate::heap::Heap::footprint`] is a high-water mark and a collection does not lower it —
    /// DR-0019 makes a swept slot *reusable*, which stops the arena growing rather than shrinking
    /// it. So "collect above 8 MiB" would fire at every check once crossed, for ever. Growth since
    /// the last collection is self-limiting instead: a steady program stops growing and stops
    /// collecting, and one that is genuinely accumulating pays in proportion to what it accumulates.
    ///
    /// # What it is honoured to
    ///
    /// The same interval as DR-0022's budget — a thousand instructions, and between them rather
    /// than inside one. A single built-in that allocates without returning is not interrupted, so
    /// this bounds a *program*'s growth and not any one operation's.
    pub fn set_collection_growth(&mut self, growth: Option<usize>) {
        self.collect_after_growth = growth;
    }

    /// Free everything this machine can no longer reach, and answer how much that was.
    ///
    /// The host's to call, and the interpreter runs none on a schedule of its own. That was
    /// measured twice in `execute` — every eight mebibytes cost 318 conformance files their time
    /// budget to buy six passes, and once at the budget cost 79 to buy none.
    ///
    /// **The reason those numbers were what they were has since changed, and they have not been
    /// taken again.** They were explained by DR-0010 never reusing a swept slot, so a collection
    /// could not lower what `Heap::footprint` measures; DR-0019 does reuse one, so a collection now
    /// stops the arena growing even though `footprint` is a high-water mark and still never falls.
    /// Whether a timer pays for itself is therefore an open question with a stale answer — re-run
    /// it before quoting it, and do not read the absence of a schedule as a finding.
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
        let roots = self.roots(root, None);
        heap.collect(&roots)
    }

    /// The same, from inside the loop, where one more chunk is live than `roots` can find.
    ///
    /// `running` is the body the machine is **currently** executing, and it is in none of the
    /// places `roots` looks. `Frame::code` holds the chunk to go *back* to, so the innermost body
    /// is only ever in `execute`'s own `current` local — every frame on the stack names its
    /// caller's code and not its own.
    ///
    /// **No program has been found that needs it, and that was measured rather than assumed.** The
    /// line was removed by hand and the whole suite run again with a collection at *every* check:
    /// nothing failed, including bodies entered through a resumed generator, a direct `eval`, an
    /// indirect one and `new Function` — each of which this doc previously claimed would need it.
    /// They do not. Every chunk that can be `current` is owned by something already rooted: the
    /// root chunk is walked above, and a function body is reached through the frame that names its
    /// function object, whose `Callable::Bytecode` the collector walks the constants of.
    ///
    /// It is kept anyway, and the reason is asymmetry rather than doubt about the argument. Being
    /// wrong here does not throw — it hands a later instruction a slot somebody else now owns, and
    /// this file's own header says that is the failure a root set exists to prevent. The cost is one
    /// walk of one constant table per collection, paid only when collecting. A guard that cannot be
    /// distinguished is usually one to delete; a *root* that cannot be distinguished is one whose
    /// absence no test would announce.
    fn collect_running(
        &self,
        root: &Chunk,
        running: Option<&Chunk>,
        heap: &mut Heap,
    ) -> crate::heap::Collected {
        let roots = self.roots(root, running);
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
    /// What is deliberately absent: `floor`, `until_check` and the `at`/`stack_base`
    /// marks are numbers; `code` is an `Rc<Chunk>` whose *constants* are named through
    /// [`Chunk::names`]; the intern table is not a root at all, which is what lets a name a program
    /// computed and dropped be collected.
    ///
    /// `root` is the chunk being run, which the machine does not hold: it is lent to `run` and its
    /// constants are the Strings the outermost code is about to use.
    /// §25.4.1.5 `AddWaiter` — park a `waitAsync`'s promise on a position for a later notify.
    pub(crate) fn park_waiter(
        &mut self,
        buffer: crate::heap::ObjectId,
        byte: usize,
        capability: crate::heap::Capability,
    ) {
        self.waiters.push(Waiter {
            buffer,
            byte,
            capability,
        });
    }

    /// §25.4.1.7 `RemoveWaiters` — take up to `count` of the waiters on a position, oldest first.
    ///
    /// The order is the specification's and is observable: §25.4.1.5 appends, and a notify with a
    /// count smaller than the list wakes the *earliest* waiters, so two promises settle in the
    /// order their `waitAsync` calls were made rather than in whatever order a container happens to
    /// hold them.
    ///
    /// `count` is an `f64` because §25.4.3.7 step 3 makes a missing count **+∞** — a value no
    /// integer type can hold, and one that must mean "all of them" rather than saturating to a
    /// large number that happens to exceed the list.
    pub(crate) fn take_waiters(
        &mut self,
        buffer: crate::heap::ObjectId,
        byte: usize,
        count: f64,
    ) -> Vec<crate::heap::Capability> {
        let mut taken = Vec::new();
        let mut left = count;
        self.waiters.retain(|waiter| {
            if left > 0.0 && waiter.buffer == buffer && waiter.byte == byte {
                taken.push(waiter.capability);
                left -= 1.0;
                return false;
            }
            true
        });
        taken
    }

    fn roots(&self, root: &Chunk, running: Option<&Chunk>) -> crate::heap::Roots {
        let mut values = Vec::new();
        let mut environments = vec![self.environment];
        // The body the machine is in the middle of, when there is one — see `collect_running` for
        // why no frame names it.
        if let Some(chunk) = running {
            chunk.names(&mut values); // no program distinguishes it — see `collect_running`
        }

        values.extend(self.stack.iter().copied());
        values.extend([self.this_value, self.new_target, self.completion]);
        values.extend(self.escaped);
        values.extend(self.templates.values().copied().map(Value::Object));
        // Everything the realm built before a script ran. Not the individual intrinsics, because a
        // list of those is one an intrinsic added later is left out of — see `Realm::intrinsics`.
        // Every realm, not only the running one. Each has its own range rather than a ceiling, so a
        // realm built second roots what *it* made and not everything older than it.
        for realm in &self.realms {
            values.extend(realm.intrinsics().map(Value::Object));
        }
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
        // §16.2.1's *chunks*, which is a different claim from its records below and the one a
        // script never has to make. A graph is a set of compiled bodies the machine is holding, and
        // evaluating it runs them one after another — so while the first is executing, the ones
        // that have not started are reachable from nothing else the collector walks. Their constant
        // tables are Strings, and freeing those hands the next module a `'c'` that is no longer a
        // `'c'`.
        //
        // Found by collecting at *every* check over a two-module graph, where the answer came back
        // `undefined`. A threshold large enough to be sensible hides it completely, which is why
        // `vm::tests::collecting` forces the schedule rather than trusting a default.
        for (_, chunk) in self.resolved.entries() {
            chunk.names(&mut values);
        }
        // §16.2.1's records. A module that has been linked is reachable for the rest of the
        // execution whether or not anything is currently running it — a later `import()` of the same
        // specifier must find the same namespace and must not run the body again. Freeing an
        // environment here would leave a namespace object reading slots that are gone.
        // §25.4.1's waiter records. A parked `waitAsync` holds the only reference to the promise
        // it answered with until a notify or a `then` reaches it, so a collection in between would
        // free a promise a later `Atomics.notify` is about to settle.
        for waiter in &self.waiters {
            values.extend([
                waiter.capability.promise,
                waiter.capability.resolve,
                waiter.capability.reject,
            ]);
        }
        for record in self.modules.values() {
            environments.push(record.environment);
            values.extend(record.namespace.map(Value::Object));
            values.extend(record.failure);
            // §13.3.12 caches this on the record and answers with it for ever, so it is reachable
            // for as long as the module is — a collection that freed it would hand the next read a
            // different object, which is the one thing the clause promises cannot happen.
            values.extend(record.import_meta.map(Value::Object));
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
        // §13.5.4 step 3 has no BigInt case at all, and that is deliberate: `+1n` is a TypeError
        // where every other unary operator has a BigInt meaning. Unary `+` *is* `ToNumber`, and
        // asking a BigInt for a Number is the conversion §7.1.4 refuses — so the operator that
        // looks harmless is the one BigInt does not have.
        UnaryOperator::Plus => Value::Number(operand.to_number(heap)?),
        // §13.5.5 — `ToNumber` and then negate. Negation is not subtraction from zero: `-0` is
        // `-0` where `0 - 0` is `+0`.
        UnaryOperator::Minus => match operand {
            // §13.5.5 step 3 — §6.1.6.2.1 for a BigInt, which is the same magnitude with the other
            // sign. `-0n` is `0n`: there is no negative zero to keep.
            Value::BigInt(id) => {
                let negated = heap
                    .bigint(id)
                    .map_or_else(crate::bigint::BigInt::zero, crate::bigint::BigInt::negate);
                Value::BigInt(heap.new_bigint(negated))
            }
            _ => Value::Number(-operand.to_number(heap)?),
        },
        // §13.5.6 — `ToInt32` and then complement, so `~x` is `-(x + 1)` for a 32-bit `x`, and
        // `~"abc"` is `-1` because NaN becomes `+0` on the way through.
        UnaryOperator::BitwiseNot => match operand {
            // §13.5.6 step 3 — §6.1.6.2.2, which is `-(x + 1)` at *every* width rather than at
            // thirty-two of them. `~0n` is `-1n` and `~0` is also `-1`; the two part company as
            // soon as the operand does not fit in an `i32`.
            Value::BigInt(id) => {
                let value = heap
                    .bigint(id)
                    .cloned()
                    .unwrap_or_else(crate::bigint::BigInt::zero);
                match value.not() {
                    Ok(complement) => Value::BigInt(heap.new_bigint(complement)),
                    Err(_) => {
                        return Err(Abrupt::range_error(
                            "this BigInt is larger than this engine will hold",
                        ));
                    }
                }
            }
            _ => Value::Number(f64::from(!operand.to_int32(heap)?)),
        },
        // §13.5.7 — `ToBoolean` and then negate, which is why `!!x` is the shortest cast.
        UnaryOperator::LogicalNot => Value::Boolean(!operand.to_boolean(heap)),
        // Refused by the compiler, which is where the message with a span comes from. Answering
        // `undefined` here means a mistake shows up as a wrong value rather than a plausible one.
        UnaryOperator::Delete => Value::Undefined,
    })
}

#[cfg(test)]
mod tests;
