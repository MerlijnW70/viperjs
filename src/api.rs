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
    /// The source did not parse or did not compile. The text is what a `SyntaxError` would say.
    Syntax(String),
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
        let script = parse_script(source).map_err(|error| Error::Syntax(error.kind.to_string()))?;
        let chunk = compile_script(&script, &mut self.heap)
            .map_err(|error| Error::Syntax(error.message()))?;
        self.last = Rc::new(chunk);
        let chunk = Rc::clone(&self.last);
        self.vm.run(&chunk, &mut self.heap).map_err(Error::Engine)
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
mod tests {
    use super::*;
    use crate::heap::NativeCall;
    use crate::value::Completion;

    #[test]
    fn the_heap_budget_is_the_hosts_to_choose_and_defaults_to_something_useful() {
        // DR-0013's number is a policy rather than a fact about the machine, and which policy is
        // right belongs to the embedder. A default of zero would refuse every allocation, which is
        // the mistake `heap::Budget` exists to make unwritable — this row is what would notice.
        let mut engine = Engine::new();
        let answer = engine
            .eval("var a = []; for (var i = 0; i < 5000; i++) a.push({x: i}); a.length")
            .expect("the default budget allows an ordinary program");
        assert_eq!(engine.text(answer).as_deref(), Ok("5000"));

        // Lowered below what a program needs, the program is refused rather than the process dying
        // — which is the whole of what DR-0013 is for.
        let mut small = Engine::new();
        small.set_heap_budget(1 << 16);
        let refused = small.eval("var a = []; for (var i = 0; i < 200000; i++) a.push({x: i}); 1");
        assert!(
            matches!(&refused, Err(Error::Thrown(said)) if said.contains("heap has grown past")),
            "{refused:?}"
        );

        // …and raising it is what lets a program that legitimately wants the memory have it. The
        // pair matters more than either: a budget that only ever refuses is indistinguishable from
        // a broken engine, and one that never refuses is not a budget.
        let mut large = Engine::new();
        large.set_heap_budget(1 << 28);
        let answer = large
            .eval("var a = []; for (var i = 0; i < 200000; i++) a.push({x: i}); a.length")
            .expect("a raised budget allows it");
        assert_eq!(large.text(answer).as_deref(), Ok("200000"));
    }

    #[test]
    fn a_namespace_of_host_functions_is_an_ordinary_object_with_named_methods() {
        fn answer(_: &mut Vm, _: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
            Ok(call.argument(0))
        }
        let mut engine = Engine::new();
        engine.bind_namespace("host", &[("echo", 1, answer), ("also", 2, answer)]);
        // An ordinary object with ordinary properties: the program owns it and may take it apart.
        let answer = engine
            .eval("typeof host + ',' + Object.keys(host).join('|')")
            .expect("runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("object,echo|also"));
        let answer = engine.eval("host.echo(7)").expect("runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("7"));
        // §10.3.3's metadata, which is the whole reason this is not a loop over `bind`: a host
        // outside the crate can make the function but cannot name it, and an unnamed method is
        // what every hand-built namespace used to have.
        let answer = engine
            .eval("host.echo.name + ',' + host.echo.length + ',' + host.also.name")
            .expect("runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("host.echo,1,host.also"));
        // It inherits from `Object.prototype`, so the ordinary object protocol works on it.
        let answer = engine
            .eval("host.hasOwnProperty('echo') + ',' + ('toString' in host)")
            .expect("runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("true,true"));
    }

    #[test]
    fn a_script_answers_its_completion_value_and_the_host_reads_it_as_text() {
        let mut engine = Engine::new();
        let answer = engine.eval("1 + 1").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
        // §14.2.2's completion value, which is the last statement's: a declaration produces nothing
        // and leaves `undefined` behind rather than the value it bound.
        let answer = engine.eval("1; 2").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
        let answer = engine.eval("var x = 1").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("undefined"));
    }

    #[test]
    fn the_three_failures_are_told_apart_because_a_host_answers_them_differently() {
        let mut engine = Engine::new();
        // Source that cannot be read is the host's own bug — it gave the engine nonsense.
        assert!(matches!(engine.eval("1 +"), Err(Error::Syntax(_))));
        // A throw is the script's answer and is often the expected one, so it is a different case
        // and carries what a `catch` would have seen.
        assert_eq!(
            engine.eval("throw new TypeError('no')").unwrap_err(),
            Error::Thrown("TypeError: no".to_string())
        );
        // …and a thrown value that is not an Error at all still says something: §7.1.17 spells it.
        assert_eq!(
            engine.eval("throw 7").unwrap_err(),
            Error::Thrown("7".to_string())
        );
        // An engine-raised error reads the same as a script-thrown one, because it goes through the
        // realm's constructor rather than a spelling of this surface's own.
        assert_eq!(
            engine.eval("null.x").unwrap_err(),
            Error::Thrown(
                "TypeError: cannot read a property of something that is not an object".to_string()
            )
        );
    }

    /// Answers its first argument's length, so a test can tell it ran *and* that it was handed what
    /// the call passed.
    fn measure(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        let text = vm.to_string(call.argument(0), heap)?;
        let length = heap.string(text).map_or(0, <[u16]>::len);
        Ok(Value::Number(length as f64))
    }

    /// A native's `Err` is a throw in the language and not a Rust failure.
    fn refuse(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
        Err(crate::value::Abrupt::type_error("the host said no"))
    }

    #[test]
    fn a_host_function_is_reachable_from_script_and_sees_its_arguments() {
        let mut engine = Engine::new();
        engine.bind("measure", 1, measure);
        let answer = engine.eval("measure('abcd')").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("4"));
        // §10.3.3's two properties, which every built-in has and which diagnostics read. A host
        // function without them is not the same kind of thing as one of ours.
        let answer = engine
            .eval("measure.name + '/' + measure.length")
            .expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("measure/1"));
        // It is an ordinary function, so the language reaches it the way it reaches any other.
        let answer = engine
            .eval("[1, 22, 333].map(function (n) { return measure(String(n)) }).join(',')")
            .expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("1,2,3"));
    }

    #[test]
    fn a_host_function_that_throws_is_caught_by_the_script() {
        let mut engine = Engine::new();
        engine.bind("refuse", 0, refuse);
        let answer = engine
            .eval("try { refuse() } catch (e) { e.constructor.name + ': ' + e.message }")
            .expect("it runs");
        assert_eq!(
            engine.text(answer).as_deref(),
            Ok("TypeError: the host said no")
        );
        // …and uncaught it reaches the host as a throw rather than as a fault.
        assert_eq!(
            engine.eval("refuse()").unwrap_err(),
            Error::Thrown("TypeError: the host said no".to_string())
        );
    }

    #[test]
    fn a_value_crosses_the_boundary_in_both_directions() {
        let mut engine = Engine::new();
        let object = engine
            .eval("({ a: 1, greet: function (who) { return 'hi ' + who } })")
            .expect("it runs");
        // Reading a property walks the whole prototype chain and runs a getter, so it is the
        // script's own read rather than a peek at a table.
        let a = engine.get(object, "a").expect("a is there");
        assert_eq!(engine.text(a).as_deref(), Ok("1"));
        // Calling back in: the receiver is the host's to choose, which is what makes this a method
        // call rather than a bare one.
        let greet = engine.get(object, "greet").expect("greet is there");
        let name = engine.eval("'world'").expect("it runs");
        let said = engine.call(greet, object, &[name]).expect("it calls");
        assert_eq!(engine.text(said).as_deref(), Ok("hi world"));
        // A value the host holds can be handed back to the script by name.
        engine.set_global("held", object).expect("it is live");
        let answer = engine.eval("held.a").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("1"));
    }

    #[test]
    fn calling_something_that_is_not_a_function_throws_rather_than_faulting() {
        let mut engine = Engine::new();
        let not_callable = engine.eval("({})").expect("it runs");
        assert!(matches!(
            engine.call(not_callable, Value::Undefined, &[]),
            Err(Error::Thrown(_))
        ));
        // Reading a property of `undefined` is the same: the host is given the script's error and
        // not a Rust one, because DR-0002 says nothing a caller does may panic.
        assert!(matches!(
            engine.get(Value::Undefined, "anything"),
            Err(Error::Thrown(_))
        ));
    }

    /// Uppercases its argument through [`Host`] alone — the operations an *out-of-crate* native has.
    ///
    /// The whole of this test's point: fourteen tests passed while a bound function could not
    /// convert its own arguments, because a test inside the crate can reach `Vm::to_string` and a
    /// real host cannot. `examples/embed.rs` found it; this pins it.
    fn shout(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        let mut host = Host::new(vm, heap);
        let said = host.text(call.argument(0))?;
        Ok(host.string(&said.to_uppercase()))
    }

    /// Calls its first argument with its second, so a callback crosses the boundary both ways.
    fn apply_to(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        let mut host = Host::new(vm, heap);
        host.call(call.argument(0), Value::Undefined, &[call.argument(1)])
    }

    /// Reads `.width` off its argument and refuses what has none.
    fn width_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        let mut host = Host::new(vm, heap);
        let width = host.get(call.argument(0), "width")?;
        match width {
            Value::Number(_) => Ok(width),
            _ => Err(Host::type_error("that has no numeric width")),
        }
    }

    #[test]
    fn a_native_can_do_its_work_with_only_what_a_host_can_reach() {
        let mut engine = Engine::new();
        engine.bind("shout", 1, shout);
        engine.bind("applyTo", 2, apply_to);
        engine.bind("widthOf", 1, width_of);

        let answer = engine.eval("shout('hi') + shout(42)").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("HI42"));

        // A callback handed to a native and called back into the script — most of what a host API
        // is, and it needs `Host::call` rather than anything the interpreter exposes.
        let answer = engine
            .eval("applyTo(function (n) { return n * 3 }, 7)")
            .expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("21"));

        // Reading a property, and refusing with a message the script catches as its own TypeError.
        let answer = engine.eval("widthOf({ width: 5 })").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("5"));
        let answer = engine
            .eval("try { widthOf({}) } catch (e) { e.constructor.name + ': ' + e.message }")
            .expect("it runs");
        assert_eq!(
            engine.text(answer).as_deref(),
            Ok("TypeError: that has no numeric width")
        );

        // A conversion that throws inside a native travels out as the script's throw, not as a
        // Rust failure — which is what makes `?` the right thing to write in one.
        let answer = engine
            .eval("try { shout(Symbol()) } catch (e) { e.constructor.name }")
            .expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("TypeError"));
    }

    #[test]
    fn a_string_a_native_makes_is_interned_rather_than_copied() {
        // `Host::string` goes through the intern table, so a host loop answering one word does not
        // fill the arena with copies of it. Measured as a footprint that does not grow rather than
        // as an identity, because two equal Strings are equal either way and only the cost differs.
        let mut engine = Engine::new();
        engine.bind("shout", 1, shout);
        engine.eval("shout('warm')").expect("it runs");
        let before = engine.footprint();
        engine
            .eval("for (var i = 0; i < 200; i++) { shout('warm') }")
            .expect("it runs");
        let grew = engine.footprint() - before;
        assert!(
            grew < 200 * std::mem::size_of::<u16>() * 4,
            "200 answers of one word grew the heap by {grew} bytes"
        );
    }

    /// A budget small enough that a runaway is stopped quickly, and large enough that an ordinary
    /// script finishes well inside it. Every row below runs one loop that never ends, so the test
    /// costs about this much wall-clock each time it is reached.
    const BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

    #[test]
    fn a_script_that_will_not_stop_is_stopped() {
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        assert_eq!(
            engine.eval("while (true) {}").unwrap_err(),
            Error::Interrupted
        );
        // The machine is usable again, because DR-0022 clears the flag when a run *begins* rather
        // than when one ends — the flag has to survive unwinding every nested execution above it.
        let answer = engine.eval("1 + 1").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
        // …and the budget is still in force, so the next runaway is stopped too. Set once, applied
        // per run: a deadline fixed when the host set it would have passed by now.
        assert_eq!(engine.eval("for (;;) {}").unwrap_err(), Error::Interrupted);
    }

    #[test]
    fn a_stopped_run_cannot_be_caught_and_runs_no_finally() {
        // The decision the whole record turns on. A budget a script can catch is not a budget:
        // `catch` would swallow it and the loop would resume, and the check meant to stop a runaway
        // would fire again for ever.
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        engine.eval("var reached = 'no'").expect("it runs");
        assert_eq!(
            engine
                .eval("try { while (true) {} } catch (e) { reached = 'catch' }")
                .unwrap_err(),
            Error::Interrupted
        );
        let answer = engine.eval("reached").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("no"));
        // A `finally` is the same answer for the same reason: it is code, and the machine reads no
        // more instructions. A host that needs cleanup has to do it in Rust, and DR-0022 says so.
        assert_eq!(
            engine
                .eval("try { while (true) {} } finally { reached = 'finally' }")
                .unwrap_err(),
            Error::Interrupted
        );
        let answer = engine.eval("reached").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    }

    #[test]
    fn a_stop_underneath_a_call_stops_the_caller_too() {
        // Why the flag is read before *every* instruction and not only where the deadline is
        // checked. When a nested execution stops it simply returns, and the call it was serving is
        // left with a frame it never popped and a value it never produced — so the caller carries
        // on as though the call had answered. Without this check the outer loop runs on for another
        // whole check interval, which is a thousand instructions of a script that was supposed to
        // have been stopped.
        //
        // **It is not enough to wrap the call in `try`/`catch`.** A stopped nested execution does
        // not throw — that was the first guess and it left this row passing with the check removed,
        // because the `catch` was never reached in either case. What distinguishes them is an
        // ordinary statement *after* the call.
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        engine.eval("var reached = 'no'").expect("it runs");
        assert_eq!(
            engine
                .eval("[1].map(function () { while (true) {} }); reached = 'after the callback'")
                .unwrap_err(),
            Error::Interrupted
        );
        let answer = engine.eval("reached").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("no"));
        // The same one level down through a coercion, which enters the loop from the middle of an
        // instruction rather than from a native — a different way in and the same answer.
        assert_eq!(
            engine
                .eval("({ valueOf: function () { while (true) {} } }) + 1; reached = 'after the coercion'")
                .unwrap_err(),
            Error::Interrupted
        );
        let answer = engine.eval("reached").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    }

    #[test]
    fn a_stopped_run_drains_no_jobs() {
        // §9.5's queue is drained at the end of a run, and a job is code like any other — a `then`
        // handler that loops for ever is the same problem wearing a promise. So an interrupted run
        // answers without draining.
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        engine.eval("var ran = 'no'").expect("it runs");
        assert_eq!(
            engine
                .eval("Promise.resolve().then(function () { ran = 'yes' }); while (true) {}")
                .unwrap_err(),
            Error::Interrupted
        );
        let answer = engine.eval("ran").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    }

    #[test]
    fn a_loop_inside_a_coercion_or_a_callback_is_stopped_too() {
        // The reason the flag is read before an instruction rather than checked once per run: a
        // coercion re-enters the interpreter from the *middle* of an instruction, and a callback
        // enters it from inside a native. Both are executions of their own and both must stop.
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        assert_eq!(
            engine
                .eval("({ valueOf: function () { while (true) {} } }) + 1")
                .unwrap_err(),
            Error::Interrupted
        );
        assert_eq!(
            engine
                .eval("[1, 2, 3].map(function () { while (true) {} })")
                .unwrap_err(),
            Error::Interrupted
        );
        // A generator resumed from a `for`-`of` is a third way in, and parks and revives an
        // execution rather than nesting one.
        assert_eq!(
            engine
                .eval("function* g() { while (true) { yield 1 } } for (var x of g()) {}")
                .unwrap_err(),
            Error::Interrupted
        );
    }

    #[test]
    fn no_budget_is_the_default_and_removing_one_restores_it() {
        // Off unless a host asks, which is what leaves the conformance suite, the examples and
        // every existing caller exactly as they were.
        let mut engine = Engine::new();
        let answer = engine
            .eval("var n = 0; for (var i = 0; i < 200000; i++) { n += i } n")
            .expect("no budget, so it finishes however long it takes");
        assert_eq!(engine.text(answer).as_deref(), Ok("19999900000"));
        // A budget that is generous does not stop ordinary work either — the check is a deadline
        // and not a step count, so a loop that finishes in time finishes.
        engine.set_time_budget(Some(std::time::Duration::from_secs(30)));
        let answer = engine
            .eval("var n = 0; for (var i = 0; i < 200000; i++) { n += i } n")
            .expect("well inside thirty seconds");
        assert_eq!(engine.text(answer).as_deref(), Ok("19999900000"));
        // …and it can be taken off again.
        engine.set_time_budget(None);
        let answer = engine.eval("1 + 1").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    }

    #[test]
    fn the_budget_does_not_reach_the_regular_expression_matcher() {
        // DR-0022 says this in its "what this does not stop", and a limitation stated only in prose
        // is one nobody finds out has changed. §22.2's backtracking is its own loop and does not
        // read the stop flag, so a hostile pattern runs to completion however small the budget is.
        //
        // `/(a+)+b/` against a subject of `a`s that can never match is the classic: every extra `a`
        // doubles the work. Measured here at 52 ms, 210 ms and 689 ms for widths 18, 20 and 22
        // against a 10 ms budget — so 22 leaves a margin of about seventy times over, which is what
        // keeps this from being a test about how fast the machine is.
        //
        // **If this fails, the matcher has gained a check and that is good news** — update
        // DR-0022's list and this row rather than the budget.
        let mut engine = Engine::new();
        engine.set_time_budget(Some(std::time::Duration::from_millis(10)));
        let answer = engine
            .eval("/(a+)+b/.test('aaaaaaaaaaaaaaaaaaaaaa')")
            .expect("the matcher runs to the end, budget or no budget");
        assert_eq!(engine.text(answer).as_deref(), Ok("false"));
        // …and the machine was never stopped, so the very next statement runs in the same breath.
        let answer = engine.eval("1 + 1").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    }

    #[test]
    fn a_thrown_object_is_described_by_the_fields_it_actually_has() {
        // What a host prints when a script throws, and the three shapes are genuinely different
        // rather than an ordering of one rule. An Error has both fields; an object may have one,
        // the other, or neither, and `throw` accepts all of them.
        let mut engine = Engine::new();
        let said = |engine: &mut Engine, source: &str| match engine.eval(source) {
            Err(Error::Thrown(text)) => text,
            other => panic!("expected a throw, got {other:?}"),
        };
        assert_eq!(
            said(&mut engine, "throw new TypeError('no')"),
            "TypeError: no"
        );
        // §20.5.3.3's `message` defaults to the **empty string**, so an Error made without one is
        // its name alone. Written as `name` and not `"name: "` — the separator belongs to the
        // message, and a trailing colon is what joining them unconditionally produces.
        assert_eq!(said(&mut engine, "throw new TypeError()"), "TypeError");
        // A name and no message at all. `undefined` is *absent* here rather than a value to spell,
        // which is the distinction `[[Get]]` cannot draw and this layer must.
        assert_eq!(said(&mut engine, "throw ({ name: 'Weird' })"), "Weird");
        // …and with no name there is nothing to lead with, so the object speaks for itself through
        // §7.1.17 rather than being announced as `undefined`.
        assert_eq!(
            said(&mut engine, "throw ({ message: 'lonely' })"),
            "[object Object]"
        );
        assert_eq!(said(&mut engine, "throw ({})"), "[object Object]");
        // An empty name is present and useless, which is the same case as absent.
        assert_eq!(
            said(&mut engine, "throw ({ name: '', message: 'm' })"),
            "[object Object]"
        );
        // A thrown primitive never had fields to read.
        assert_eq!(said(&mut engine, "throw 7"), "7");
        assert_eq!(said(&mut engine, "throw 'plain'"), "plain");
        // A subclass keeps its own name, which is the reason to read the field rather than the
        // constructor: `name` is what the object says it is.
        assert_eq!(
            said(
                &mut engine,
                "class Mine extends Error { constructor() { super('detail'); this.name = 'Mine' } } throw new Mine()"
            ),
            "Mine: detail"
        );
    }

    #[test]
    fn a_collected_value_is_refused_and_never_read_as_undefined() {
        // DR-0021's rule, and the reason it is a record rather than a doc line: a `Value` the host
        // holds is not a root, and `collect` keeps only what the *program* can reach.
        //
        // **The refusal is this surface's and not the heap's**, which is what the first draft got
        // wrong. `[[Get]]` on an object that is no longer there degrades to `undefined` — the same
        // answer an absent property gives — so without a check the host cannot tell "no such
        // property" from "the object you are asking about is gone". Measured before it was written:
        // the read answered `Ok(undefined)`.
        let mut engine = Engine::new();
        let held = engine.eval("({ a: 1 })").expect("it runs");
        // Enough other work that nothing the machine happens to be holding — §14.2.2's completion
        // register, the operand stack — still names it. Which of those root a value is not part of
        // any promise, and a test that relied on one would be pinning an accident.
        for _ in 0..50 {
            engine.eval("({ junk: 1 })").expect("it runs");
        }
        assert!(engine.collect().objects > 0, "the collector did something");
        assert_eq!(engine.get(held, "a").unwrap_err(), Error::Collected);
        assert_eq!(engine.text(held), Err(Error::Collected));
        assert_eq!(
            engine.call(held, Value::Undefined, &[]).unwrap_err(),
            Error::Collected
        );
        assert_eq!(engine.set_global("x", held), Err(Error::Collected));
        // An argument is checked too, and not only the callee — a collected one would arrive in the
        // script as `undefined` and read as a value the host chose to pass.
        let live = engine.eval("(function (x) { return x })").expect("it runs");
        assert_eq!(
            engine.call(live, Value::Undefined, &[held]).unwrap_err(),
            Error::Collected
        );
    }

    #[test]
    fn the_global_object_is_how_a_host_roots_a_value() {
        // The escape hatch, and it is the one the language already has rather than a second
        // lifetime discipline: anything the program can reach survives, so anything on the global
        // does.
        let mut engine = Engine::new();
        let held = engine.eval("({ a: 1 })").expect("it runs");
        engine.set_global("held", held).expect("it is live");
        for _ in 0..50 {
            engine.eval("({ junk: 1 })").expect("it runs");
        }
        engine.collect();
        let a = engine.get(held, "a").expect("it survived");
        assert_eq!(engine.text(a).as_deref(), Ok("1"));
        // …and the script sees the same object, which is what makes it a root rather than a copy.
        let answer = engine.eval("held.a = 2; held.a").expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("2"));
        let a = engine.get(held, "a").expect("still there");
        assert_eq!(engine.text(a).as_deref(), Ok("2"));
    }

    #[test]
    fn a_value_with_nothing_to_point_at_is_never_collected() {
        // The four that live wholly inside the `Value` have no handle to go stale, so they must not
        // be refused after a collection — a liveness check that asked about them would answer for a
        // slot that does not exist.
        let mut engine = Engine::new();
        let number = engine.eval("42").expect("it runs");
        let boolean = engine.eval("true").expect("it runs");
        let nothing = engine.eval("null").expect("it runs");
        let missing = engine.eval("undefined").expect("it runs");
        for _ in 0..50 {
            engine.eval("({ junk: 1 })").expect("it runs");
        }
        engine.collect();
        assert_eq!(engine.text(number).as_deref(), Ok("42"));
        assert_eq!(engine.text(boolean).as_deref(), Ok("true"));
        assert_eq!(engine.text(nothing).as_deref(), Ok("null"));
        assert_eq!(engine.text(missing).as_deref(), Ok("undefined"));
    }

    #[test]
    fn two_engines_share_nothing() {
        // GOAL.md §3 says isolation comes from running more engines, which is only true if this is.
        let mut first = Engine::new();
        let mut second = Engine::new();
        first.eval("var shared = 1").expect("it runs");
        assert!(matches!(second.eval("shared"), Err(Error::Thrown(_))));
        let answer = first.eval("shared").expect("it runs");
        assert_eq!(first.text(answer).as_deref(), Ok("1"));
    }

    #[test]
    fn the_job_queue_has_run_by_the_time_eval_returns() {
        // DR-0016 — jobs run inside `run`, so a promise settled by the script has had its reactions
        // delivered when the host is handed the answer. Without that a host would have to be told
        // to drain a queue it cannot see.
        let mut engine = Engine::new();
        let answer = engine
            .eval("var seen = 'pending'; Promise.resolve(7).then(function (v) { seen = v }); 0")
            .expect("it runs");
        assert_eq!(engine.text(answer).as_deref(), Ok("0"));
        let seen = engine.eval("seen").expect("it runs");
        assert_eq!(engine.text(seen).as_deref(), Ok("7"));
    }
    #[test]
    fn probe_completion_register_rooting() {
        let mut e = Engine::new();
        let a = e.eval("({ n: 1 })").expect("runs");
        for _ in 0..50 {
            e.eval("({ junk: 1 })").expect("runs");
        }
        let freed = e.collect();
        println!("freed {:?}", freed.objects);
        match e.get(a, "n") {
            Ok(v) => println!("read gave Ok: {}", e.text(v).unwrap_or_default()),
            Err(err) => println!("read gave Err: {err:?}"),
        }
    }
}
