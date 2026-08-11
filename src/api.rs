//! The embedding surface — DR-0021.
//!
//! Everything else public in this crate is an *internal* of the engine that `examples/` and
//! `conformance/` happen to need. This module is the surface someone else builds on: one type that
//! owns both halves of the engine, runs source, moves values across the boundary, and lets the host
//! bind functions of its own.
//!
//! ```
//! use viperjs::api::Engine;
//!
//! let mut engine = Engine::new();
//! let answer = engine.eval("1 + 1").expect("it runs");
//! assert_eq!(engine.text(answer).as_deref(), Ok("2"));
//! ```
//!
//! # Why one type and not two
//!
//! A [`Vm`] and a [`Heap`] are separate objects inside, and every operation takes both. Nothing in
//! that shape says they belong together: two heaps and one machine compile, and the result is
//! *silently wrong* rather than refused, because a `Value` naming an object is an index into an
//! arena and the wrong arena has something else at it. `Engine` owning both makes that
//! unrepresentable — which is why [`Vm::to_string`](crate::vm::Vm) and its neighbours stay crate
//! -private and are wrapped here rather than exported.
//!
//! # What a value is, and how long it lasts
//!
//! [`Value`] is `Copy` and small, and an object one is a handle into this engine's heap. It is
//! **not** a garbage-collection root: [`Engine::collect`] keeps what the *program* can still reach,
//! and a value sitting in a Rust local is not that. To keep one across a collection, put it
//! somewhere the program reaches — [`Engine::set_global`] is that, and it is the same rule a script
//! lives under.
//!
//! Reading a value the collector has taken is **refused** — [`Error::Collected`] — and that refusal
//! is this module's rather than the heap's. DR-0019's generations stop a stale handle finding some
//! *other* value, but `[[Get]]` on an object that is not there degrades to `undefined`, which is
//! what an absent property gives too. Telling those apart is the reason there is a boundary here at
//! all, so every value the host passes in is checked first.
//!
//! # Stopping a script that will not stop
//!
//! [`Engine::set_time_budget`] does it, and exceeding the budget is **not a throw** — a budget a
//! script can `catch` is not a budget, so `try { while (true) {} } catch (e) {}` still ends.
//! DR-0022 is the record; this section said "there is no deadline and no interrupt, so
//! `while (true) {}` runs until the process does not", which was true of DR-0021 and was answered
//! by the very next record.
//!
//! What the budget does **not** reach is a §22.2 match already running and a host function that
//! blocks: `/(a+)+b/` against 22 `a`s takes about 700 ms against a 10 ms budget, which is a test
//! rather than a sentence.
//!
//! # What a host still cannot bind, measured against two real packages
//!
//! Running npm packages through the command line found exactly two things missing, and neither is
//! the engine — both are the *host's* to provide, and this surface cannot yet express either.
//! Written down here rather than left as a bug report, because each is a decision and not an
//! omission.
//!
//! **A constructor.** [`Engine::bind`] and [`Engine::bind_namespace`] make functions, and
//! `Heap::new_native_function` gives them no `[[Construct]]` — so a host cannot offer
//! `new TextEncoder()`. Nor can it build the `Uint8Array` such a thing would answer with: the view
//! constructors are crate-private, and an embedder holding [`Engine::heap_mut`] can make a buffer
//! and not a view over it. `pako` wants both. Neither is hard; both are public API, which is the
//! kind of thing that wants saying out loud before it is added rather than after.
//!
//! **Cryptographic randomness cannot be offered by *this* crate at all**, and that is the more
//! interesting of the two. `crypto.getRandomValues` needs the operating system's entropy, and the
//! two ways to reach it are a dependency (DR-0001 forbids it) and an `unsafe` FFI call (DR-0002
//! forbids that). `/dev/urandom` is a third and exists on one of the two platforms this is built
//! for. What is left is seeding a generator from the clock, and a predictable stream under the name
//! `crypto` is worse than an absent one: a library that finds the function missing says so, and one
//! that finds a fake generates keys with it. So it stays absent here **and belongs to the
//! embedder**, who links whatever they like and can bind it in three lines. `crypto-js` is the
//! package that wants it.

use crate::compile::{Chunk, compile_script};
use crate::heap::{Heap, Native, ObjectId, PropertyDescriptor, PropertyKey};
use crate::parser::parse_script;
use crate::value::{Completion, Value};
use crate::vm::{Fault, Outcome, Vm};
use std::rc::Rc;

/// One engine: a heap, a machine, and one realm's worth of intrinsics.
///
/// Two `Engine`s share nothing at all, which is how GOAL.md §3's advice — isolation comes from
/// running more engines — is actually true rather than merely intended.
pub struct Engine {
    /// The machine. Holds the realm, the module registry and the job queue.
    vm: Vm,
    /// Every value this engine has made.
    heap: Heap,
    /// The last chunk run, kept as [`Engine::collect`]'s root.
    ///
    /// A collection has to know which code is about to run, because a chunk's constant table holds
    /// Strings the machine will reach for and nothing else names them. An `Rc` rather than the
    /// chunk itself so that running it does not borrow `self` while `self.vm` is borrowed
    /// mutably — the clone is a refcount and the alternative is moving the chunk out and back.
    last: Rc<Chunk>,
}

/// The engine, as a **host function** sees it — the other half of [`Engine::bind`].
///
/// A [`Native`] is handed a `&mut Vm` and a `&mut Heap` because that is what the interpreter has,
/// and every operation on those is crate-private for the reason DR-0021 gives. So without this a
/// bound function could be installed and could do nothing with what it was passed: the surface
/// would let a host *register* I/O and not *implement* it.
///
/// Borrows both halves rather than owning them, which is the only difference from [`Engine`] — the
/// engine is mid-call, and the frames underneath belong to the script that is waiting.
///
/// Every operation answers a [`Completion`], not this module's [`Error`], so `?` in a native does
/// what a `throw` does: the failure becomes the script's to catch.
///
/// ```
/// use viperjs::api::{Engine, Host};
/// use viperjs::heap::{Heap, NativeCall};
/// use viperjs::value::{Completion, Value};
/// use viperjs::vm::Vm;
///
/// fn shout(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
///     let mut host = Host::new(vm, heap);
///     let said = host.text(call.argument(0))?;
///     Ok(host.string(&said.to_uppercase()))
/// }
///
/// let mut engine = Engine::new();
/// engine.bind("shout", 1, shout);
/// let answer = engine.eval("shout('hi')").expect("it runs");
/// assert_eq!(engine.text(answer).as_deref(), Ok("HI"));
/// ```
pub struct Host<'a> {
    /// The machine, mid-call.
    vm: &'a mut Vm,
    /// Its heap — the same one, which is the invariant DR-0021 exists to keep.
    heap: &'a mut Heap,
}

impl<'a> Host<'a> {
    /// The two halves a [`Native`] is handed, as one thing.
    pub fn new(vm: &'a mut Vm, heap: &'a mut Heap) -> Self {
        Self { vm, heap }
    }

    /// `String(value)` — §7.1.17, so an object's `toString` runs and may throw.
    ///
    /// # Errors
    ///
    /// Whatever the conversion threw, as the script's own throw: a Symbol always refuses, and an
    /// object may.
    pub fn text(&mut self, value: Value) -> Completion<String> {
        let id = self.vm.to_string(value, self.heap)?;
        Ok(self
            .heap
            .string(id)
            .map(String::from_utf16_lossy)
            .unwrap_or_default())
    }

    /// `Number(value)` — §7.1.4, so an object's `valueOf` runs and may throw.
    ///
    /// The other half of [`Host::text`], and it was missing until a host function wanted to take a
    /// duration: a native could be handed `$262.agent.sleep(100)` and had no way to find the 100
    /// except by converting to a String and parsing it back, which is a second implementation of
    /// §7.1.4 that agrees with the first only by luck.
    ///
    /// # Errors
    ///
    /// Whatever the conversion threw. A Symbol and a BigInt always refuse — §7.1.4 has no reading
    /// for either — and an object may, through its own `valueOf` or `toString`.
    pub fn number(&mut self, value: Value) -> Completion<f64> {
        self.vm.to_number(value, self.heap)
    }

    /// A JavaScript String from Rust text.
    ///
    /// Interned, so two natives answering the same word share one String rather than filling the
    /// arena with copies — which is the allocation a host loop makes most of.
    pub fn string(&mut self, text: &str) -> Value {
        let units: Vec<u16> = text.encode_utf16().collect();
        Value::String(self.heap.intern(&units))
    }

    /// Read `name` off `value`, exactly as `value.name` would — the whole chain, and a getter runs.
    ///
    /// # Errors
    ///
    /// Whatever the read threw, which reading a property of `undefined` or `null` does.
    pub fn get(&mut self, value: Value, name: &str) -> Completion<Value> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let key = PropertyKey::from_units(self.heap, &units);
        self.vm.get_property_key(value, key, self.heap)
    }

    /// Call `callee` with `this_value` and `arguments`.
    ///
    /// This is how a native takes a callback — which is most of what a host API is: `setTimeout`,
    /// a comparator, a visitor.
    ///
    /// # Errors
    ///
    /// Whatever the call threw, including a callee that is not callable.
    pub fn call(
        &mut self,
        callee: Value,
        this_value: Value,
        arguments: &[Value],
    ) -> Completion<Value> {
        self.vm.call_value(callee, this_value, arguments, self.heap)
    }

    /// Run `source` as a **Script** in this realm, from inside a host function.
    ///
    /// §16.1.7's goal and not §19.2.1.1's, which is one argument's worth of difference and is
    /// observable: a Script's `var` becomes a permanent property of the global object where an
    /// `eval`'s is deletable. That is what §INTERPRETING.md defines `$262.evalScript` as, and an
    /// `evalScript` written as `(0, eval)(source)` fails the tests that ask
    /// `verifyProperty(globalThis, 'f', {configurable: false})`.
    ///
    /// The run is *nested*: the frames underneath belong to the script that is waiting, and they
    /// come back untouched. [`Engine::eval`] cannot be used for this — it starts a fresh top-level
    /// run and clears the machine, which from inside a call would take the caller's stack with it.
    ///
    /// # Errors
    ///
    /// A **SyntaxError** for source that will not parse or that an early error refuses, and
    /// otherwise whatever the script threw. Both arrive as a `Completion`, so `?` in a native does
    /// what a `throw` does.
    pub fn eval_script(&mut self, source: &str) -> Completion<Value> {
        crate::builtins::eval::source_as(
            self.vm,
            self.heap,
            source,
            crate::builtins::eval::Goal::Script,
        )
    }

    /// A refusal, in the terms the language uses — the commonest thing a host says.
    ///
    /// A plain function rather than a method because it borrows nothing: `Abrupt::Raised` carries a
    /// `&'static str` and needs no realm until the machine turns it into a value.
    #[must_use]
    pub const fn type_error(message: &'static str) -> crate::value::Abrupt {
        crate::value::Abrupt::type_error(message)
    }
}

/// What went wrong, in terms an embedder can act on.
///
/// Three cases and not one, because the host's response to each differs: source it cannot compile
/// is a bug in what it was given, a throw is the script's own answer and often expected, and a
/// [`Fault`] is a bug in *this engine* — DR-0002 says a script cannot cause one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The source did not parse or did not compile.
    ///
    /// **Carries where, and it did not until 2026-08-09.** The variant was a bare `String`, so
    /// every host built on this surface — `viper` included — could say `expected an expression,
    /// found ';'` and not which line, while the parser had known the answer all along and
    /// `examples/parse` was already printing it with a caret. That is the house rule about errors
    /// carrying spans stopping at the one boundary where it is most useful, and it made a syntax
    /// error in a 286 KB bundle nearly unactionable.
    Syntax {
        /// What a `SyntaxError` would say.
        message: String,
        /// Where it went wrong. For an unexpected token this is the span of *that token* rather
        /// than of the construct it interrupted, because a caret under the surprise beats one
        /// under its context — the same choice [`crate::parser::ParseError`] makes.
        ///
        /// Turn it into a line and a column with [`crate::span::line_col`], which needs the source
        /// text: a span is a byte range and this type deliberately does not hold a copy of the
        /// program to resolve it against.
        span: crate::span::Span,
    },
    /// The script threw and nothing caught it, with the value spelled as `String(e)` would spell
    /// it.
    ///
    /// The value itself is deliberately not here: it belongs to the engine's heap and would outlive
    /// the borrow that produced it. [`Engine::eval_value`] hands back the value for a caller that
    /// wants to inspect it.
    Thrown(String),
    /// The engine reached a state its own types say is impossible. Not something a script can
    /// cause — see DR-0002 — so this is a bug report, not a condition to handle.
    Engine(Fault),
    /// The run spent the time budget [`Engine::set_time_budget`] set, and was stopped — DR-0022.
    ///
    /// Carries nothing, because there is nothing to carry: no value was produced and nothing was
    /// thrown. A script cannot catch this and no `finally` ran, which is what makes a budget a
    /// bound on untrusted code rather than a suggestion.
    ///
    /// The engine is usable afterwards — the next [`Engine::eval`] starts a fresh run — but what
    /// the interrupted script had already done to the global object is still done.
    Interrupted,
    /// The value names something [`Engine::collect`] has already freed.
    ///
    /// DR-0021: a `Value` the host is holding is not a garbage-collection root. Without this case
    /// the read would answer **`undefined`** — `[[Get]]` on an object that is not there degrades to
    /// exactly what an absent property gives, and the host could not tell "no such property" from
    /// "the object you are asking about is gone". That is the one shape a boundary exists to stop.
    Collected,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A new engine with a fresh realm: the global object, the intrinsics, nothing of the host's.
    #[must_use]
    pub fn new() -> Self {
        let mut heap = Heap::new();
        let vm = Vm::new(&mut heap);
        // An empty chunk so that `collect` has a root before anything has been run. The
        // alternative is an `Option` whose `None` arm is reachable exactly once per engine, which
        // is a branch that exists to describe a moment rather than a state.
        let empty = parse_script("").map(|script| compile_script(&script, &mut heap));
        let chunk = match empty {
            Ok(Ok(chunk)) => chunk,
            // Neither can fail for the empty string — the grammar accepts an empty `StatementList`
            // and there is nothing to compile — so this is the engine's own bug, not an input's.
            _ => Chunk::default(),
        };
        Self {
            vm,
            heap,
            last: Rc::new(chunk),
        }
    }

    /// Run `source` as a Script and answer its completion value.
    ///
    /// §14.2.2's completion value, which is what the last statement produced — so `eval("1; 2")` is
    /// `2` and `eval("var x = 1")` is `undefined`. Jobs queued by the script run before this
    /// returns, because DR-0016 puts the job queue inside the run.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] if it does not parse or compile, [`Error::Thrown`] if it threw and nothing
    /// caught it, [`Error::Engine`] for a fault the engine's own types say cannot happen.
    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        match self.eval_value(source)? {
            Outcome::Value(value) => Ok(value),
            Outcome::Thrown(value) => Err(Error::Thrown(self.describe(value))),
            Outcome::Interrupted => Err(Error::Interrupted),
        }
    }

    /// The same, without turning a throw into text.
    ///
    /// For a host that wants the thrown *value* — to read its `name`, or to rethrow it into another
    /// call — rather than the sentence [`Engine::eval`] makes of it.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] and [`Error::Engine`] as above. A throw is an `Ok(Outcome::Thrown)` here
    /// rather than an `Err`, which is the whole difference.
    pub fn eval_value(&mut self, source: &str) -> Result<Outcome, Error> {
        // Both failures already carry a span — see `parser::ParseError` and `compile::CompileError`
        // — and both are kept. §16's early errors reach here as the *compiler's*, because §22.2.1's
        // patterns are decided there, so a host that only carried the parser's would lose the
        // position of every bad regular expression.
        let script = parse_script(source).map_err(|error| Error::Syntax {
            message: error.kind.to_string(),
            span: error.span,
        })?;
        let chunk = compile_script(&script, &mut self.heap).map_err(|error| Error::Syntax {
            message: error.message(),
            span: error.span,
        })?;
        self.last = Rc::new(chunk);
        let chunk = Rc::clone(&self.last);
        self.vm.run(&chunk, &mut self.heap).map_err(Error::Engine)
    }

    /// Fix what `Math.random` will answer, instead of letting the clock decide.
    ///
    /// §21.3.2.27 requires an approximately uniform distribution over `[0, 1)` and **nothing** about
    /// unpredictability, so a host that fixes the sequence is inside the clause. What wants it is a
    /// tool that has to run the same program twice and compare — a fuzzer, a bisect, a bug report
    /// somebody else has to reproduce. Without it the engine's answer to identical input differs
    /// between runs, and a finding that appears once is a finding nobody can act on.
    ///
    /// **A seeded generator is a predicted one.** Nothing that needs unpredictability may use this,
    /// and GOAL.md §3 leaves anything cryptographic to the host in any case.
    ///
    /// Per **thread**, because the generator is: a second `Engine` on this thread shares the
    /// sequence, and one on another thread has its own. Setting it does not reach an agent a host
    /// started, which has to set its own.
    ///
    /// It does not make a run reproducible on its own. `Date.now` still moves, and a program that
    /// branches on the clock branches differently — see `conformance::fuzz`, which is where this
    /// limit was found and which reproduces a finding by keeping the source rather than the seed.
    pub fn set_random_seed(&mut self, seed: u64) {
        crate::builtins::math::set_seed(seed);
    }

    /// §9.13 — every rejection of the last run that nothing ever asked for, oldest first.
    ///
    /// The reasons rather than the promises, because the reason is what a host reports and the
    /// promise is an identity it has no use for. Empty in a program that handles what it rejects.
    ///
    /// Two quite different things put entries here, and a host should say so differently:
    ///
    /// - **A program's own bug.** `Promise.reject(x)` with no `catch`, an `async` function whose
    ///   caller ignored it. Node prints a warning; a browser fires `unhandledrejection`. This is the
    ///   common case and it is not the engine's business to do more than report it.
    /// - **The engine giving up inside a job.** DR-0013's heap RangeError is thrown wherever the
    ///   allocation happened, and if that is inside a `then` handler then §9.5 step 3 discards the
    ///   completion — so [`Engine::eval`] answers `Ok`, the exit status is zero, and a program that
    ///   should have kept running has stopped. **This list is the only sign.** See DR-0029.
    ///
    /// Read it after a run and before the next one, which clears it: a rejection is unhandled only
    /// once the queue has drained and nothing can still attach a handler.
    #[must_use]
    pub fn unhandled_rejections(&self) -> Vec<Value> {
        self.vm
            .unhandled_rejections()
            .iter()
            .map(|promise| {
                self.heap
                    .promise(*promise)
                    .map_or(Value::Undefined, |found| found.result)
            })
            .collect()
    }

    /// `String(value)` — §7.1.17, so an object's `toString` runs and may itself throw.
    ///
    /// # Errors
    ///
    /// [`Error::Thrown`] if the conversion threw, which a `Symbol` always does and an object with a
    /// hostile `toString` may.
    pub fn text(&mut self, value: Value) -> Result<String, Error> {
        if !self.live(value) {
            return Err(Error::Collected);
        }
        match self.vm.to_string(value, &mut self.heap) {
            Ok(id) => Ok(self
                .heap
                .string(id)
                .map(String::from_utf16_lossy)
                // A String the heap does not hold is a handle the collector has taken, which
                // DR-0019 makes answerable rather than dangerous. Empty is the honest reading:
                // there is no text there.
                .unwrap_or_default()),
            Err(abrupt) => Err(self.raised(abrupt)),
        }
    }

    /// Read `name` off `value`, exactly as `value.name` would.
    ///
    /// The whole prototype chain, and a getter runs — so this can throw whatever the script's own
    /// read would.
    ///
    /// # Errors
    ///
    /// [`Error::Thrown`] if the read threw, which reading a property of `undefined` or `null` does.
    pub fn get(&mut self, value: Value, name: &str) -> Result<Value, Error> {
        if !self.live(value) {
            return Err(Error::Collected);
        }
        let key = self.key(name);
        self.vm
            .get_property_key(value, key, &mut self.heap)
            .map_err(|abrupt| self.raised(abrupt))
    }

    /// Call `callee` with `this_value` and `arguments`, as a method call would.
    ///
    /// # Errors
    ///
    /// [`Error::Thrown`] if the callee is not callable or the call threw.
    pub fn call(
        &mut self,
        callee: Value,
        this_value: Value,
        arguments: &[Value],
    ) -> Result<Value, Error> {
        // Every value the host hands in, not only the callee: an argument that has been collected
        // would arrive in the script as `undefined` and be read as one the host chose to pass.
        if !self.live(callee) || !self.live(this_value) || !arguments.iter().all(|v| self.live(*v))
        {
            return Err(Error::Collected);
        }
        self.vm
            .call_value(callee, this_value, arguments, &mut self.heap)
            .map_err(|abrupt| self.raised(abrupt))
    }

    /// Bind a host function into the global object under `name`.
    ///
    /// `length` is what §10.3.3 puts on the function's own `length` property — how many arguments
    /// the host *documents*, not a limit on what it will be passed. The function receives every
    /// argument the call was given, through [`crate::heap::NativeCall::argument`].
    ///
    /// This is the operation GOAL.md §3 means by "the host binds functions in", and until it
    /// existed there was none: `conformance` binds its own `$262` by writing JavaScript source,
    /// because there was nothing else to write.
    pub fn bind(&mut self, name: &str, length: u32, native: Native) {
        let prototype = self.vm.realm().function_prototype();
        let realm = self.vm.realm().id();
        let function = self.heap.new_native_function(prototype, native, realm);
        crate::builtins::define_function_metadata(&mut self.heap, function, name, length);
        // Made a line ago, so it cannot have been collected — the `Result` is the host's concern
        // and not this one's.
        let _ = self.set_global(name, Value::Object(function));
    }

    /// Bind an object of host functions into the global object under `name`.
    ///
    /// `console`, and whatever else a host groups: each entry becomes a method of a plain object
    /// inheriting from `Object.prototype`, named `name.method` the way §10.3.3 names a built-in's,
    /// and the object goes on the global.
    ///
    /// # Why this is not a loop over [`Engine::bind`]
    ///
    /// It could not be. A host outside this crate can make an object and a native function — both
    /// are public on the heap — but not give the function the `name` and `length` §10.3.3 requires,
    /// because that operation is internal. So an embedder building `console` by hand got methods
    /// whose `name` was the empty string, and the only way to a correct one was to write JavaScript
    /// source and evaluate it. Our own command line hit exactly that, which is what
    /// `examples/embed.rs` is for.
    ///
    /// The methods are ordinary writable, enumerable, configurable properties — a namespace a host
    /// hands over belongs to the program, which may take it apart.
    pub fn bind_namespace(&mut self, name: &str, methods: &[(&str, u32, Native)]) {
        let object = self
            .heap
            .new_object(Some(self.vm.realm().object_prototype()));
        for (method, length, native) in methods {
            let prototype = self.vm.realm().function_prototype();
            let realm = self.vm.realm().id();
            let function = self.heap.new_native_function(prototype, *native, realm);
            // §10.3.3's name for a method of a namespace is the qualified one, which is what a
            // stack trace and `console.log.name` should both say.
            let spelled = format!("{name}.{method}");
            crate::builtins::define_function_metadata(&mut self.heap, function, &spelled, *length);
            let key = self.key(method);
            let _ = self.heap.define_own_property(
                object,
                key,
                &PropertyDescriptor::data(Value::Object(function)),
            );
        }
        // Made in this call, so it cannot have been collected.
        let _ = self.set_global(name, Value::Object(object));
    }

    /// Put `value` on the global object under `name`.
    ///
    /// Also the way to **root** a value: the global object is reachable from the program, so
    /// anything hanging off it survives [`Engine::collect`]. A `Value` held only in a Rust local
    /// does not — DR-0021.
    ///
    /// # Errors
    ///
    /// [`Error::Collected`] if `value` has already been freed — rooting a handle that names nothing
    /// would store `undefined` under the name and report success.
    pub fn set_global(&mut self, name: &str, value: Value) -> Result<(), Error> {
        if !self.live(value) {
            return Err(Error::Collected);
        }
        let key = self.key(name);
        let global = self.vm.realm().global();
        let _ = self
            .heap
            .define_own_property(global, key, &PropertyDescriptor::data(value));
        Ok(())
    }

    /// Say how much memory this engine may hand out before it refuses — DR-0013's number.
    ///
    /// [`crate::heap::MAX_HEAP_BYTES`] is the default and is 64 MiB, which is a policy rather than
    /// a fact about the machine: a runaway `while (true) { ({}); }` has to be stopped by something,
    /// and an abort is the one failure DR-0002 cannot answer. Which number is right is the
    /// **host's** question — a command line running one trusted script and a server running
    /// untrusted snippets want opposite answers.
    ///
    /// It is not a theoretical knob. A 1.9 MB bundle of `mathjs`, built the way an application
    /// would build one, needs more than 256 MiB here and runs at 512; the default refuses it, and
    /// before this there was no way for the host to say otherwise.
    ///
    /// Checked against what has already been taken, so lowering it below the current footprint
    /// refuses the next allocation rather than freeing anything.
    pub fn set_heap_budget(&mut self, bytes: usize) {
        self.heap.set_budget(bytes);
    }

    /// The global object, for a host that wants to define properties on it directly.
    #[must_use]
    pub fn global(&self) -> ObjectId {
        self.vm.realm().global()
    }

    /// Bound how long a single [`Engine::eval`] may take — DR-0022, and `None` to remove the bound.
    ///
    /// This is the answer to a script that will not stop. Exceeding the budget is **not a throw**:
    /// the machine stops reading instructions and the call answers [`Error::Interrupted`]. The
    /// script cannot catch it, no `finally` runs, and §9.5's job queue is not drained.
    ///
    /// ```
    /// use viperjs::api::{Engine, Error};
    /// use std::time::Duration;
    ///
    /// let mut engine = Engine::new();
    /// engine.set_time_budget(Some(Duration::from_millis(50)));
    /// assert_eq!(engine.eval("while (true) {}").unwrap_err(), Error::Interrupted);
    /// // …and the engine still works afterwards.
    /// let answer = engine.eval("1 + 1").expect("it runs");
    /// assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    /// ```
    ///
    /// **It does not cover** the regular expression matcher — a pattern that backtracks
    /// catastrophically still runs to completion — nor a single long-running built-in, nor a host
    /// function that blocks. DR-0022 says why each is a separate piece of work.
    pub fn set_time_budget(&mut self, budget: Option<std::time::Duration>) {
        self.vm.set_time_budget(budget);
    }

    /// Say whether this agent may be **suspended** — §9.7's `[[CanBlock]]`.
    ///
    /// `false` is the default and is a claim about the shape of the embedding rather than caution:
    /// an engine running on its own has no second agent, so an `Atomics.wait` that parked here could
    /// never be woken and §25.4.3.14 step 12 is right to throw. A browser's main thread answers the
    /// same way.
    ///
    /// A host that runs several engines — one per thread, sharing memory through
    /// [`Engine::new_shared_buffer`] — turns it on. Whether to turn it on for the engine that
    /// *starts* the others is the decision worth thinking about, and it is a trade rather than a
    /// rule: leaving it off means that agent can never park itself with nobody left running to
    /// notify it, and it also means every `Atomics.wait` it makes throws — which some programs use
    /// as a *probe* rather than as a wait, and are then told the wrong thing. `conformance` turns it
    /// on everywhere and its `agent` module says what that cost.
    ///
    /// **`Atomics.wait` is the only thing that reads it**, and it blocks the calling thread. A host
    /// that sets this must be able to afford that thread stopping for as long as the script asked
    /// for — [`Engine::set_time_budget`] does not cover it, because the machine is not executing
    /// instructions while it waits. That is the whole of what turning this on costs: an engine every
    /// deadline could stop acquires one operation that no deadline can.
    pub fn set_can_block(&mut self, can: bool) {
        self.vm.set_can_block(can);
    }

    /// This engine's realm — its intrinsics, its global, and the identity a function is made in.
    ///
    /// An escape hatch like [`Engine::heap_mut`], and named as one: a [`Realm`](crate::realm::Realm)
    /// is an engine internal whose shape this record does not promise to keep. What it is for is a
    /// host building something this surface cannot express yet. [`Engine::bind`] makes a function on
    /// the global and [`Engine::bind_namespace`] an object of them; anything a level deeper than
    /// that needs the prototypes only a realm has, which is how `conformance` builds a `$262` with
    /// an `agent` inside it.
    #[must_use]
    pub fn realm(&self) -> crate::realm::Realm {
        self.vm.realm()
    }

    /// The bytes a `SharedArrayBuffer` holds, as something another engine can be given.
    ///
    /// `None` for every other value, an ordinary `ArrayBuffer` included: §25.1's bytes belong to one
    /// heap and there is no block underneath them to hand over.
    ///
    /// This is the whole of what one agent passes to another. A [`Value`] is a handle into *this*
    /// engine's heap and means nothing in another one, but a [`Block`](crate::heap::Block) is the
    /// memory itself — clone it, move the clone to the other thread, and
    /// [`Engine::new_shared_buffer`] grows an object over it there.
    #[must_use]
    pub fn shared_block(&self, value: Value) -> Option<crate::heap::Block> {
        let Value::Object(id) = value else {
            return None;
        };
        self.heap
            .object(id)
            .and_then(crate::heap::Object::buffer)
            .and_then(crate::heap::Buffer::block)
            .cloned()
    }

    /// A `SharedArrayBuffer` in **this** engine over bytes another engine already has.
    ///
    /// The receiving half of [`Engine::shared_block`]. The object is this realm's — its prototype,
    /// its brand, its identity — and the bytes are the ones the block names, so a write through one
    /// agent's view is a read through the other's.
    ///
    /// The bytes are **not** charged to this engine's heap budget, because they were charged where
    /// they were allocated: a second name for one allocation is not a second allocation, and
    /// charging both would refuse the sharing at half the memory it appears to be using.
    pub fn new_shared_buffer(&mut self, block: &crate::heap::Block) -> Value {
        let object = self
            .heap
            .new_object(Some(self.vm.realm().shared_buffer_prototype()));
        if let Some(found) = self.heap.object_mut(object) {
            found.set_buffer(crate::heap::Buffer::over(block));
        }
        Value::Object(object)
    }

    /// Free everything the program can no longer reach, and answer how much that was.
    ///
    /// **A `Value` the host is holding is not a root.** See the module documentation: put it on the
    /// global with [`Engine::set_global`] if it has to survive, or call this only at a moment when
    /// nothing is held. Reading a value that has been collected is safe and answers nothing.
    pub fn collect(&mut self) -> crate::heap::Collected {
        let root = Rc::clone(&self.last);
        self.vm.collect(&root, &mut self.heap)
    }

    /// How many bytes this engine's heap has handed out — DR-0013's budget, as a number to watch.
    #[must_use]
    pub fn footprint(&self) -> usize {
        self.heap.footprint()
    }

    /// The heap, for a host reaching an operation this surface does not wrap yet.
    ///
    /// An escape hatch and named as one: everything reachable through it is an engine internal
    /// whose shape this record does not promise to keep.
    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// Whether `value`'s handle still names something in this heap.
    ///
    /// Every variant that carries one is asked, not only objects: a String, a Symbol and a BigInt
    /// live in arenas of their own and are swept on the same terms. The four that are wholly
    /// contained in the `Value` — `undefined`, `null`, a Boolean, a Number — are always live,
    /// because there is nothing for them to point at.
    fn live(&self, value: Value) -> bool {
        match value {
            Value::Object(id) => self.heap.object(id).is_some(),
            Value::String(id) => self.heap.string(id).is_some(),
            Value::Symbol(id) => self.heap.symbol(id).is_some(),
            Value::BigInt(id) => self.heap.bigint(id).is_some(),
            Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => true,
        }
    }

    /// An [`Abrupt`] as the [`Error`] a host sees.
    ///
    /// Through [`Vm::thrown_value`] rather than a spelling of its own, so the sentence is the one a
    /// script catching the same failure would read. §6.2.4's throw completion comes in two shapes —
    /// an error the engine names and a value already built — and only the machine knows how to turn
    /// the first into the second, because it owns the realm the constructor lives in.
    fn raised(&mut self, abrupt: crate::value::Abrupt) -> Error {
        let value = self.vm.thrown_value(abrupt, &mut self.heap);
        Error::Thrown(self.describe(value))
    }

    /// A property key for `name`, interned so that two reads of one name share a String.
    fn key(&mut self, name: &str) -> PropertyKey {
        let units: Vec<u16> = name.encode_utf16().collect();
        PropertyKey::from_units(&mut self.heap, &units)
    }

    /// What a thrown value says about itself — `name: message` for an Error, `String(v)` otherwise.
    ///
    /// Never itself throws: a `toString` that fails is reported as the value refusing to be spelled
    /// rather than replacing one error with another, because the caller is already being told
    /// something went wrong and a second failure buried inside the first helps nobody.
    fn describe(&mut self, value: Value) -> String {
        let Value::Object(_) = value else {
            return self.spell(value);
        };
        let name = self.field(value, "name");
        let message = self.field(value, "message");
        match (name, message) {
            (Some(name), Some(message)) if !name.is_empty() && !message.is_empty() => {
                format!("{name}: {message}")
            }
            (Some(name), _) if !name.is_empty() => name,
            _ => self.spell(value),
        }
    }

    /// The text of `value.name`, or `None` when there is no such property.
    ///
    /// **`undefined` is absent.** `[[Get]]` cannot tell a missing property from one holding
    /// `undefined`, and for a diagnostic they mean the same thing — so spelling the answer
    /// unconditionally put the word into every message about a thrown object that was not an Error:
    /// `throw ({})` read as `"undefined: undefined"`. Mutation coverage found it, by way of two
    /// guards below that no test could distinguish while every field was `Some`.
    fn field(&mut self, value: Value, name: &str) -> Option<String> {
        match self.get(value, name) {
            Ok(Value::Undefined) | Err(_) => None,
            Ok(found) => Some(self.spell(found)),
        }
    }

    /// `String(value)`, with a conversion that throws reported rather than propagated.
    fn spell(&mut self, value: Value) -> String {
        match self.vm.to_string(value, &mut self.heap) {
            Ok(id) => self
                .heap
                .string(id)
                .map(String::from_utf16_lossy)
                .unwrap_or_default(),
            Err(_) => "a value that will not print".to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
