//! `$262.agent` â€” INTERPRETING.md's concurrency API, and the threads underneath it.
//!
//! # Why this is threads and not a scheduler
//!
//! Because the tests spin. `harness/atomicsHelper.js` waits for an agent to start like this:
//!
//! ```text
//! while ((agents = Atomics.load(typedArray, index)) !== expected) { /* nothing */ }
//! ```
//!
//! There is no yield in that loop and nothing a cooperative scheduler could hook. The only thing
//! that ends it is another agent writing to the block **while this one is running**, which means a
//! second operating-system thread and no way around it. `$262.agent.start` therefore starts a
//! thread with an [`Engine`] of its own: two heaps, two realms, two of every intrinsic, and exactly
//! one thing in common â€” the [`Block`] a `SharedArrayBuffer`'s bytes are.
//!
//! # Every agent here may block, which is a host decision with two flags' worth of consequence
//!
//! Â§9.7's `[[CanBlock]]` is the host's to answer and this host answers **true**, for the agent
//! running the test file as much as for the ones it starts. That is what a shell host does and it
//! is not a free choice: `atomicsHelper.js`'s `safeBroadcast` â€” which **106 of the 109** files that
//! start an agent go through â€” checks that a TypedArray is shareable by calling `Atomics.wait` on a
//! throwaway one and treating *any* throw as "this kind cannot be waited on". On an agent that
//! cannot block, that throw is `AgentCanSuspend()` refusing, and every one of those files fails
//! before it broadcasts anything.
//!
//! The other side of it is `runner.rs`'s two flags, which swap: a `CanBlockIsFalse` test describes a
//! host this is not and is skipped, and a `CanBlockIsTrue` test describes this one and now runs.
//! Two files against seven, and the 109 behind them.
//!
//! **What that risks is a hang**, and it was measured rather than assumed: outside the tests that
//! start agents there are nine `Atomics.wait` calls in the suite with no timeout argument, and every
//! one of them throws first â€” a Symbol index, a `Float64Array`, an index out of range. Nothing waits
//! for ever with nobody left to notify it. A test that did would be stopped by the worker's
//! per-test budget, because `wire.rs`'s workers are processes for exactly this kind of reason.
//!
//! # What crosses between agents, and what cannot
//!
//! Three things and nothing else: a [`Block`], a report (a String), and the second argument to
//! `broadcast` â€” which INTERPRETING.md restricts to "an Int32 or BigInt" and which crosses **as its
//! source text**, to be evaluated on the far side. A [`Value`] is a handle into one heap and names
//! something else entirely in another, so nothing that is one is ever sent.
//!
//! # The two things this host does not do
//!
//! **An agent parked in `Atomics.wait` for ever is a leaked thread.** Nothing here can interrupt
//! one â€” that is what blocking means â€” so a test that starts an agent, has it wait, and then fails
//! before notifying leaves the thread parked until the worker process is killed at its budget. It
//! costs a thread and not a hung suite, for the same reason as above.
//!
//! **An agent's `Atomics.waitAsync` cannot be woken by another agent.** DR-0024's amendment says
//! why: an asynchronous waiter holds a promise, and settling a promise means running a job on the
//! machine that made it. The agent-side `waitAsync` tests are the ones that still fail.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use viperjs::api::{Engine, Host};
use viperjs::heap::{Block, Heap, Native, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use viperjs::realm::Realm;
use viperjs::value::{Abrupt, Completion, Value};
use viperjs::vm::Vm;

/// How long `$262.agent.start` waits for the agent it started to say it is running.
///
/// INTERPRETING.md says the call blocks until then and gives it no way to fail. A bound turns an
/// agent whose script does not finish â€” an infinite loop at its top level, which a test may
/// perfectly well write â€” into a test that fails rather than a worker that never answers again.
const STARTING: Duration = Duration::from_secs(10);

/// How long `$262.agent.broadcast` waits for each agent to take the message.
///
/// The same bound for the same reason. INTERPRETING.md notes that broadcasting "assumes that all
/// agents that were started are still running", and an agent that is not is precisely what this
/// stops from hanging the worker.
const HANDOVER: Duration = Duration::from_secs(10);

thread_local! {
    /// What the agent that *started* others remembers about them.
    ///
    /// A thread local and not a field on anything, because a [`Native`] is a plain function pointer
    /// with nowhere to hang state â€” and because the state genuinely is per-thread: an agent is a
    /// thread, so "this agent's agents" and "this agent's outbox" are exactly what one holds. A
    /// worker runs one test at a time, so there is never more than one parent here.
    static STARTED: RefCell<Parent> = RefCell::new(Parent::default());
    /// What an agent knows about itself. Absent on the thread that started it, which is also how
    /// [`attach`] tells the two halves of the API apart.
    static INSIDE: RefCell<Option<Inside>> = const { RefCell::new(None) };
}

/// The agent that started others, as it sees them.
#[derive(Default)]
struct Parent {
    /// One per `$262.agent.start`, in the order they were started â€” which is the order `broadcast`
    /// hands the message over in.
    agents: Vec<Started>,
    /// Where `getReport` reads from. Every agent writes to the other end.
    reports: Option<Receiver<String>>,
    /// The end handed to each agent as it starts.
    posts: Option<Sender<String>>,
    /// What `monotonicNow` counts from, here and in every agent this one starts.
    ///
    /// Shared rather than taken per agent, because two agents' readings are compared: separate
    /// origins would make the difference between them a number about when each thread started.
    since: Option<Instant>,
}

/// One agent, from the outside.
struct Started {
    /// How a broadcast reaches it.
    hand: Sender<Message>,
    /// How it says it has taken one â€” INTERPRETING.md: `broadcast` blocks until every agent has.
    taken: Receiver<()>,
}

/// What `$262.agent.broadcast` sends.
struct Message {
    /// Â§25.2's memory, which is the whole point of the exercise.
    block: Block,
    /// The second argument, as source text â€” see [`source_of`].
    id: String,
}

/// What an agent knows about itself.
struct Inside {
    /// Where `report` puts what it is given.
    posts: Sender<String>,
    /// Set by `leaving`, which is the script saying this agent is finished.
    ///
    /// Read in one place and it is not the obvious one: an agent that called `leaving` at its *top
    /// level* has no broadcast coming and would otherwise sit on `recv` until the test ended. The
    /// scripts that call it from inside a `receiveBroadcast` callback are already about to return.
    leaving: bool,
    /// The origin `monotonicNow` counts from, taken from the agent that started this one.
    since: Instant,
}

/// The five things the agent running the test file can do â€” INTERPRETING.md's parent half.
const OUTSIDE: [(&str, Native); 5] = [
    ("start", start as Native),
    ("broadcast", broadcast),
    ("getReport", get_report),
    ("sleep", sleep),
    ("monotonicNow", monotonic_now),
];

/// The five an agent has of itself.
const WITHIN: [(&str, Native); 5] = [
    ("receiveBroadcast", receive_broadcast as Native),
    ("report", report),
    ("sleep", sleep),
    ("leaving", leaving),
    ("monotonicNow", monotonic_now),
];

/// Hang an `agent` object off a `$262`, with whichever half of the API this thread is.
///
/// Decided from the thread rather than passed in, because `$262.createRealm` builds a second `$262`
/// and has no idea which side of the boundary it is being called on. The thread knows: an agent has
/// registered itself as an agent before any of its code runs.
pub fn attach(heap: &mut Heap, realm: &Realm, host: ObjectId) {
    let within = INSIDE.with(|inside| inside.borrow().is_some());
    let methods: &[(&str, Native)] = match within {
        true => &WITHIN,
        false => &OUTSIDE,
    };
    let agent = heap.new_object(Some(realm.object_prototype()));
    for (name, native) in methods {
        let function = heap.new_native_function(realm.function_prototype(), *native, realm.id());
        define(heap, agent, name, Value::Object(function));
    }
    define(heap, host, "agent", Value::Object(agent));
}

/// One writable, enumerable, configurable property â€” which `atomicsHelper.js` requires.
///
/// It replaces `$262.agent.getReport` with a wrapper of its own on every host, saying so in a
/// comment: "All runtimes currently have their own `$262.agent.getReport` which is wrong". A
/// property that could not be written over would fail every test that includes that file.
fn define(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
    let units: Vec<u16> = name.encode_utf16().collect();
    let key = PropertyKey::from_units(heap, &units);
    let _ = heap.define_own_property(object, key, &PropertyDescriptor::data(value));
}

/// Forget every agent this thread started â€” the boundary between one test and the next.
///
/// A worker process runs test after test, so without this the second test to use agents would find
/// the first one's still listed and broadcast to them. Forgetting an agent closes the channel this
/// end of it held, which is how one waiting for a broadcast learns that its test is over and returns.
/// An agent that is already *running* is not stopped by this and cannot be; see the module
/// documentation.
pub fn forget() {
    STARTED.with(|parent| {
        let mut parent = parent.borrow_mut();
        parent.agents.clear();
        parent.reports = None;
        parent.posts = None;
    });
}

/// `$262.agent.start(source)` â€” a thread, an engine of its own, and that script running in it.
fn start(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let source = Host::new(vm, heap).text(call.argument(0))?;
    let (hand, takes) = channel();
    let (took, taken) = channel();
    let (running, runs) = channel();
    let (posts, since) = STARTED.with(|parent| {
        let mut parent = parent.borrow_mut();
        if parent.posts.is_none() {
            let (posts, reports) = channel();
            parent.posts = Some(posts);
            parent.reports = Some(reports);
        }
        let since = *parent.since.get_or_insert_with(Instant::now);
        (parent.posts.clone(), since)
    });
    let Some(posts) = posts else {
        return Err(Abrupt::type_error("this agent cannot start another"));
    };
    let started = std::thread::Builder::new()
        .name("$262.agent".to_string())
        // Said rather than inherited, because the engine's depth caps are calibrated against a
        // stack: `compile::tests`, `parser::tests` and `vm::tests::coercion` each pin theirs inside
        // a **one** mebibyte thread, so twice that is above the measured need on every platform,
        // and a spawn that took whatever the operating system felt like giving it would make the
        // caps mean something different in an agent from what they mean in the test that set them.
        .stack_size(2 * 1024 * 1024)
        .spawn(move || live(&source, &takes, &took, posts, since, &running));
    if started.is_err() {
        return Err(Abrupt::type_error("this agent could not start another"));
    }
    // "Will block until that agent is running", read as *has run its top level* â€” the stronger of
    // the two readings, on purpose. An agent script's whole job is to call `receiveBroadcast`, so
    // returning any earlier would let a broadcast reach an agent that has not yet said what to do
    // with one.
    let _ = runs.recv_timeout(STARTING);
    STARTED.with(|parent| parent.borrow_mut().agents.push(Started { hand, taken }));
    Ok(Value::Undefined)
}

/// One agent, from its own side: build an engine, run the script, take the broadcast, run the
/// callback, and stop.
///
/// Nothing here reports a failure anywhere, and that is deliberate rather than lazy. An agent has no
/// way to fail a test â€” no `assert`, and no channel for one â€” so what a broken agent script produces
/// is a **missing report**, which the test's own `getReport` is already waiting for and will fail
/// on. Inventing a second failure path would mean deciding what a test *meant* to assert.
fn live(
    source: &str,
    takes: &Receiver<Message>,
    took: &Sender<()>,
    posts: Sender<String>,
    since: Instant,
    running: &Sender<()>,
) {
    // Registered before the engine is built, because [`attach`] reads it to decide which half of
    // the API this thread gets and `install_host` runs below.
    INSIDE.with(|inside| {
        *inside.borrow_mut() = Some(Inside {
            posts,
            leaving: false,
            since,
        });
    });
    let mut engine = Engine::new();
    engine.set_can_block(true);
    let realm = engine.realm();
    crate::runner::install_host(engine.heap_mut(), &realm);
    let _ = engine.eval(source);
    let _ = running.send(());
    // An agent that said it was leaving at its top level has no broadcast coming and no callback to
    // be given one, so waiting for a message it will never be sent would keep the thread until the
    // test ended.
    if INSIDE.with(|inside| {
        inside
            .borrow()
            .as_ref()
            .is_some_and(|inside| inside.leaving)
    }) {
        return;
    }
    // A closed channel is the test having ended without ever broadcasting, which is what every
    // negative case does â€” there is nothing left to take and nothing to do but stop.
    let Ok(message) = takes.recv() else {
        return;
    };
    let taken = engine.new_shared_buffer(&message.block);
    let _ = took.send(());
    if engine.set_global(BROADCAST, taken).is_err() {
        return;
    }
    // Called through `eval` rather than through a host call, because that is what drains Â§9.5's job
    // queue afterwards: a `receiveBroadcast` callback may be `async`, and what it queued has to be
    // delivered before this thread stops.
    let _ = engine.eval(&format!("{RECEIVE}({BROADCAST}, {})", message.id));
}

/// Where the received `SharedArrayBuffer` is put for the callback to be handed.
const BROADCAST: &str = "$__broadcast";

/// Where `receiveBroadcast` keeps what it was given.
///
/// On the global and not in a Rust local, because a `Value` a host is holding is **not** a
/// collection root â€” `api`'s own documentation says so â€” and a collection between the registration
/// and the broadcast would otherwise free the callback. The global is the one place a host can put
/// a value that the program itself then keeps alive.
const RECEIVE: &str = "$__receiveBroadcast";

/// `$262.agent.broadcast(sab, id)` â€” hand the block to every agent and wait until each has it.
fn broadcast(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let block = match call.argument(0) {
        Value::Object(object) => heap
            .object(object)
            .and_then(viperjs::heap::Object::buffer)
            .and_then(viperjs::heap::Buffer::block)
            .cloned(),
        _ => None,
    };
    let Some(block) = block else {
        return Err(Abrupt::type_error(
            "$262.agent.broadcast wants a SharedArrayBuffer",
        ));
    };
    let id = source_of(vm, heap, call.argument(1))?;
    STARTED.with(|parent| {
        let parent = parent.borrow();
        // Handed to all of them first and waited for afterwards, which is what makes a broadcast
        // one event rather than a sequence: the agents that have taken it start running while the
        // rest are still being told, and a test that counts agents in a shared slot depends on it.
        for agent in &parent.agents {
            let _ = agent.hand.send(Message {
                block: block.clone(),
                id: id.clone(),
            });
        }
        for agent in &parent.agents {
            let _ = agent.taken.recv_timeout(HANDOVER);
        }
    });
    Ok(Value::Undefined)
}

/// The second argument to `broadcast`, as text the receiving agent can evaluate.
///
/// INTERPRETING.md restricts it to "an Int32 or BigInt", and both are written by the characters they
/// are spelled with â€” so the source text *is* the value, exactly, where handing over a `f64` would
/// lose a BigInt's magnitude. The `n` is what tells the two apart, and dropping it would turn a
/// BigInt into a Number in silence: `broadcast(sab, 1n)` would hand the agent `1`, and the
/// `BigInt64Array` tests are written to notice.
///
/// Absent is what all but a handful of tests pass, since `safeBroadcast` sends only the buffer, and
/// it needs no case of its own: `ToString(undefined)` is the five letters that evaluate back to it.
///
/// A String would not survive the trip â€” it would arrive as an identifier â€” and neither would an
/// object. Nothing is done about either, because INTERPRETING.md says what may be sent and a host
/// inventing a refusal the document does not describe is a host answering a question nobody asked.
fn source_of(vm: &mut Vm, heap: &mut Heap, given: Value) -> Completion<String> {
    let big = matches!(given, Value::BigInt(_));
    let text = Host::new(vm, heap).text(given)?;
    Ok(match big {
        true => format!("{text}n"),
        false => text,
    })
}

/// `$262.agent.getReport()` â€” the oldest report any agent has sent, or `null`.
fn get_report(vm: &mut Vm, heap: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    let taken = STARTED.with(|parent| {
        parent
            .borrow()
            .reports
            .as_ref()
            .and_then(|reports| reports.try_recv().ok())
    });
    let Some(text) = taken else {
        return Ok(Value::Null);
    };
    Ok(Host::new(vm, heap).string(&text))
}

/// `$262.agent.report(message)` â€” INTERPRETING.md's, whose conversion to a String is explicit in it.
fn report(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let said = Host::new(vm, heap).text(call.argument(0))?;
    INSIDE.with(|inside| {
        if let Some(inside) = inside.borrow().as_ref() {
            // A closed channel is the test having ended. There is nothing to report it to and
            // nothing to be done about it, which is why this is discarded rather than thrown.
            let _ = inside.posts.send(said);
        }
    });
    Ok(Value::Undefined)
}

/// `$262.agent.receiveBroadcast(f)` â€” remember `f` and return, which is what the document allows.
///
/// "This function may return before a broadcast is received (eg to return to an event loop to await
/// a message) and no code should follow the call to this function." So it does exactly that: the
/// callback goes on the global, the agent's script finishes, and [`live`] calls it when the message
/// arrives.
fn receive_broadcast(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let global = vm.realm().global();
    define(heap, global, RECEIVE, call.argument(0));
    Ok(Value::Undefined)
}

/// `$262.agent.leaving()` â€” the script saying this agent may be terminated.
///
/// Recorded rather than acted on. In almost every script it is the last statement of a
/// `receiveBroadcast` callback, which is about to return and end the agent anyway; the one place
/// the flag is read is an agent that says it at its **top level**, which is a script with no
/// broadcast coming. See [`Inside::leaving`].
fn leaving(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    INSIDE.with(|inside| {
        if let Some(inside) = inside.borrow_mut().as_mut() {
            inside.leaving = true;
        }
    });
    Ok(Value::Undefined)
}

/// `$262.agent.sleep(ms)` â€” "sleeps the agent for approximately that duration".
///
/// The same function on both sides, because it means the same thing on both: this thread stops. On
/// the agent holding the test that is `atomicsHelper.js`'s `tryYield`, which is how it gives the
/// agents it started a chance to reach their waits.
fn sleep(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let asked = Host::new(vm, heap).number(call.argument(0))?;
    // A negative or absent duration is no sleep rather than an error: the document describes a
    // duration and not an argument to be validated, and `ToNumber(undefined)` is NaN.
    if let Ok(long) = Duration::try_from_secs_f64(asked.max(0.0) / 1000.0) {
        std::thread::sleep(long);
    }
    Ok(Value::Undefined)
}

/// `$262.agent.monotonicNow()` â€” milliseconds since an origin every agent here shares.
///
/// Monotonic in the sense the document asks for, because it is `Instant` and not the wall clock:
/// one of those cannot go backwards and the other can.
fn monotonic_now(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    let within = INSIDE.with(|inside| inside.borrow().as_ref().map(|inside| inside.since));
    let since = match within {
        Some(since) => since,
        None => STARTED.with(|parent| *parent.borrow_mut().since.get_or_insert_with(Instant::now)),
    };
    Ok(Value::Number(since.elapsed().as_secs_f64() * 1000.0))
}
