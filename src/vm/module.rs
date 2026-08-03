//! §16.2.1's Module Records — linking a graph of modules, and evaluating it in order.
//!
//! # What the host does and what this does
//!
//! §16.2.1.7's `HostLoadImportedModule` is the embedder's: a specifier is text, and only the host
//! knows whether it names a file, a URL or an entry in a map it built itself. praxis therefore
//! takes a graph that is already *resolved and compiled* — every module in it, under the specifier
//! the modules that import it wrote — and does the two things that are the language's:
//!
//! - **Link** (§16.2.1.5): give every module an environment, and make each imported name *be* the
//!   exporting module's slot rather than a copy of what it held. `export let n = 0; n++` is
//!   visible through every importer because of that, and it is the whole reason an import is not
//!   an assignment.
//! - **Evaluate** (§16.2.1.6): run each module body once, dependencies first, and never twice
//!   however many modules import it.
//!
//! Keeping the resolution outside is the same division DR-0014 makes for the time zone: the engine
//! implements the clause and the host supplies the one fact the clause cannot know.
//!
//! # What is not here yet
//!
//! A cycle evaluates in the order the depth-first walk reaches it, which is §16.2.1.6's order, but
//! there is no `[[Status]]` machinery to make a *self*-import legal — a module that imports itself
//! reads its own bindings in the dead zone, which is what the specification says and is reached
//! here by having nothing else. `export *` and re-exports are refused when the module is compiled,
//! so nothing here has to answer for them.
//!
//! A namespace object (`import * as n`) is made here, by §16.2.1.10 — one per module however many
//! importers ask for one, which is what makes two importers' namespaces the same object.

use super::{Fault, Outcome, Vm};
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, StringId};
use crate::value::{Completion, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

/// A graph of compiled modules, each under the specifier its importers write.
///
/// A map rather than a list because linking asks it by name once per import, and because two
/// modules importing the same specifier must reach the *same* record — that is what makes a module
/// evaluate once.
#[derive(Debug, Default)]
pub struct Graph {
    modules: BTreeMap<String, Rc<Chunk>>,
}

impl Graph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Put a compiled module in it under `specifier`, replacing any already there.
    pub fn insert(&mut self, specifier: &str, chunk: Rc<Chunk>) {
        self.modules.insert(specifier.to_string(), chunk);
    }

    /// The module `specifier` names, if the host supplied one.
    #[must_use]
    pub fn get(&self, specifier: &str) -> Option<&Rc<Chunk>> {
        self.modules.get(specifier)
    }

    /// Every specifier in it and the module it names, so one graph can be merged into another.
    pub(super) fn entries(&self) -> impl Iterator<Item = (&str, &Rc<Chunk>)> {
        self.modules
            .iter()
            .map(|(specifier, chunk)| (specifier.as_str(), chunk))
    }
}

/// Why a graph could not be linked — §16.2.1.5's errors, which are the program's and not a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// The host supplied no module for a specifier something imported.
    ///
    /// §16.2.1.7 leaves this to the host, so praxis cannot say more than which text failed. A
    /// SyntaxError is what a host reports for a specifier it cannot resolve.
    Unresolved(String),
    /// §16.2.1.6.3 `ResolveExport` found nothing — the module is there and the name is not.
    NoSuchExport {
        /// The module asked.
        specifier: String,
        /// The name asked for.
        name: String,
    },
}

impl LinkError {
    /// A sentence for the error a host would throw.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Unresolved(specifier) => {
                format!("no module was supplied for {specifier:?}")
            }
            Self::NoSuchExport { specifier, name } => {
                format!("{specifier:?} does not export {name:?}")
            }
        }
    }
}

impl Vm {
    /// Tell the machine how to answer a `ModuleSpecifier` at run time — §16.2.1.7.
    ///
    /// Only a dynamic `import()` needs one: a static graph is resolved by the caller and handed to
    /// [`Vm::run_module_graph`]. Without a loader an `import()` rejects, which is what a host that
    /// cannot load a module is supposed to do.
    pub fn set_module_loader(&mut self, loader: Box<dyn super::ModuleLoader>) {
        self.loader = Some(loader);
    }

    /// §16.2.1.5 `Link` and §16.2.1.6 `Evaluate`, over a graph the host has resolved.
    ///
    /// The entry module is evaluated last, because everything it imports has to have run first —
    /// that is what makes an imported binding hold a value rather than sit in its dead zone.
    ///
    /// The graph is **merged** into what this machine already knows rather than replacing it, so a
    /// second call — or a dynamic `import()` of a specifier this one supplied — finds the modules
    /// already linked instead of building a second copy of each.
    pub fn run_module_graph(
        &mut self,
        entry: &str,
        graph: &Graph,
        heap: &mut Heap,
    ) -> Result<Result<Outcome, LinkError>, Fault> {
        for (specifier, chunk) in graph.entries() {
            self.resolved.insert(specifier, Rc::clone(chunk));
        }
        self.link_and_evaluate(entry, heap)
    }

    /// Link everything `entry` reaches that is not linked yet, then evaluate what has not run.
    ///
    /// The whole of §16.2.1.5 and §16.2.1.6 for one entry point, and the only thing between a
    /// static graph and a dynamic `import()`: both arrive here once the specifier is a module this
    /// machine can find.
    pub(super) fn link_and_evaluate(
        &mut self,
        entry: &str,
        heap: &mut Heap,
    ) -> Result<Result<Outcome, LinkError>, Fault> {
        // §16.2.1.5.1 step 9 — every module the walk reaches gets its environment before any of
        // them is bound, because a cycle means a module's imports may name one the walk has not
        // reached yet.
        //
        // Keyed by the **chunk**, not by the specifier that reached it. Two specifiers may name one
        // module — a diamond, or a module that imports *itself*, which `instn-named-bndng-cls.js`
        // does — and a module is one record however many names lead to it. Keyed by name, a
        // self-import became a second copy that evaluated the whole body a second time.
        let mut order = Vec::new();
        let mut seen = Vec::new();
        if let Err(error) = self.visit(entry, &mut seen, &mut order, heap) {
            return Ok(Err(error));
        }
        if let Err(error) = self.initialize_environments(&order, heap) {
            return Ok(Err(error));
        }
        // §16.2.1.6.4 step 3 — **every** indirect export is resolved, and not only the ones
        // something imports. `export { x } from "m"` where `m`'s `x` is ambiguous is a SyntaxError
        // for the module that wrote the line, whether or not anything ever asks it for `x`.
        for chunk in &order {
            for export in chunk.exports() {
                // Every export and not only the indirect ones. §16.2.1.6.4 step 3 names
                // `[[IndirectExportEntries]]`, and a *local* export always resolves — it is a slot
                // of the module being asked, which is linked by the time this runs — so skipping
                // one is a branch whose two sides give the same answer.
                if matches!(
                    self.resolve_export(chunk, &export.export_name, heap),
                    ExportResolution::Missing | ExportResolution::Ambiguous
                ) {
                    return Ok(Err(LinkError::NoSuchExport {
                        specifier: entry.to_string(),
                        name: export.export_name.to_string(),
                    }));
                }
            }
        }
        // §16.2.1.6 — dependencies first, and each body once. `order` is the depth-first
        // post-order the walk produced, which is exactly that.
        let mut outcome = Outcome::Value(crate::value::Value::Undefined);
        for chunk in &order {
            let Some(record) = self.modules.get_mut(&identity(chunk)) else {
                continue;
            };
            // §16.2.1.6 step 4 — a module that has already run is not run again, whichever entry
            // point reached it. A module that already *threw* answers with the same value it threw
            // the first time rather than running its body a second time.
            if let Some(failure) = record.failure {
                return Ok(Ok(Outcome::Thrown(failure)));
            }
            if record.evaluated {
                continue;
            }
            let environment = record.environment;
            // Marked before the body runs, so that a module reached again *while* it is running —
            // which a cycle does, and which a dynamic `import()` of an ancestor does — is not
            // started a second time.
            record.evaluated = true;
            // §16.2.1.5.3 — a module whose body contains a top-level `await` is **asynchronous**: it
            // answers a promise rather than a value, and everything that imports it waits for that
            // promise before its own body runs. The waiting is what the job queue is for.
            outcome = match chunk.is_async() {
                true => self.evaluate_async_module(chunk, environment, heap)?,
                false => self.run_module_in(chunk, environment, heap)?,
            };
            // §16.2.1.6 step 12 — a module that throws stops the evaluation, and the throw is the
            // answer. Nothing after it runs, including the entry.
            if let Outcome::Thrown(thrown) = outcome {
                if let Some(record) = self.modules.get_mut(&identity(chunk)) {
                    record.failure = Some(thrown);
                }
                return Ok(Ok(outcome));
            }
        }
        Ok(Ok(outcome))
    }

    /// §16.2.1.5.3 — evaluate an asynchronous module, and do not return until it has settled.
    ///
    /// The specification threads `[[PendingAsyncDependencies]]` through the graph so that each
    /// module resumes when the ones it imports have finished. praxis evaluates a graph inside one
    /// call, so the same order is reached by draining the queue here: the next module's body cannot
    /// start until this one's promise has an answer, which is exactly what waiting means.
    ///
    /// What that does **not** reproduce is the interleaving between two asynchronous modules that
    /// do not import one another — §16.2.1.5.3's counter orders those, and here the first is run to
    /// completion before the second begins.
    fn evaluate_async_module(
        &mut self,
        chunk: &Rc<Chunk>,
        environment: EnvironmentId,
        heap: &mut Heap,
    ) -> Result<Outcome, Fault> {
        let promise = match self.run_async_module(chunk, environment, heap)? {
            Ok(promise) => promise,
            // A throw that escaped the wrapper. There is no promise to reject, so it is the
            // module's failure directly.
            Err(thrown) => return Ok(Outcome::Thrown(thrown)),
        };
        let Value::Object(promise) = promise else {
            return Err(Fault::NotAnObject);
        };
        // The body has stopped at its first `await`, and what resumes it is a job. Draining until
        // the promise answers is the whole of the wait — and it terminates for the reason any
        // `await` does: a promise nothing will ever settle leaves the queue empty, and the loop
        // ends with the module unsettled rather than spinning.
        loop {
            let state = heap
                .promise(promise)
                .map_or(crate::heap::PromiseState::Rejected, |found| found.state);
            match state {
                crate::heap::PromiseState::Pending if !self.jobs.is_empty() => {
                    self.drain_jobs(heap);
                }
                // §16.2.1.5.3's `AsyncModuleExecutionRejected` — the module failed, and every
                // importer sees the same value. `link_and_evaluate` records it on the record.
                crate::heap::PromiseState::Rejected => {
                    let thrown = heap
                        .promise(promise)
                        .map_or(Value::Undefined, |found| found.result);
                    return Ok(Outcome::Thrown(thrown));
                }
                // Fulfilled, or pending with nothing left that could settle it. A module that never
                // settles has still *run*, and what it exported is what it managed to export —
                // which is what a host sees when a top-level `await` never resolves.
                //
                // §14.2.2's completion value is read here rather than carried through the promise,
                // which is fulfilled with `undefined` because §16.2.1.5.3 gives a host a promise and
                // not a value. The register holds the body's because the drain ends the moment that
                // promise answers, and what answers it is the body's own last instruction.
                _ => {
                    let answer = self.completion;
                    // §9.5 — what is left of the queue runs before the answer is handed back, which
                    // is what `run` does after a script's last statement and what `run_prepared`
                    // does after a synchronous module's. An asynchronous one reaches neither, so a
                    // `then` registered by the body was still waiting when the host was told the
                    // module had finished.
                    self.drain_jobs(heap);
                    self.escaped = None;
                    return Ok(Outcome::Value(answer));
                }
            }
        }
    }

    /// §16.2.1.5.2 `InitializeEnvironment` — bind every import, now that every environment exists.
    fn initialize_environments(
        &mut self,
        order: &[Rc<Chunk>],
        heap: &mut Heap,
    ) -> Result<(), LinkError> {
        for chunk in order {
            for entry in chunk.imports() {
                let Some(slot) = entry.slot else {
                    // `import "a";` binds nothing and is written for the other module being
                    // evaluated. The edge is still in the graph, which is what makes that happen.
                    continue;
                };
                let Some(from) = self.resolved.get(&entry.specifier).cloned() else {
                    return Err(LinkError::Unresolved(entry.specifier.to_string()));
                };
                let Some(here) = self.environment_of(chunk) else {
                    continue;
                };
                // §16.2.1.5.2 step 1.a — `import * as n` binds an ordinary initialised slot holding
                // the namespace object, and **not** one of the module's own slots. That is the one
                // import that is a value rather than an alias, which is why it is settled here
                // rather than by `bind_import` below.
                let Some(name) = entry.import_name.as_ref() else {
                    let namespace = self.namespace_of(&from, heap);
                    heap.set_variable(here, slot, crate::value::Value::Object(namespace));
                    continue;
                };
                // §16.2.1.5.2 step 1.b — `ResolveExport`, which may walk through several modules
                // before it finds a binding: `export { a } from "m"` and `export * from "m"` both
                // send it further.
                match self.resolve_export(&from, name, heap) {
                    ExportResolution::Slot(there, at) => {
                        heap.bind_import(here, slot, there, at);
                    }
                    // `export * as n from "m"` — the name resolves to a whole namespace and not to
                    // any binding, so the importer gets a value like a namespace import does.
                    ExportResolution::Namespace(namespace) => {
                        heap.set_variable(here, slot, crate::value::Value::Object(namespace));
                    }
                    // §16.2.1.5.2 steps 1.b.i and 1.b.ii — both a name nothing exports and one two
                    // star exports disagree about are SyntaxErrors the host reports, and neither is
                    // something the program could catch.
                    ExportResolution::Missing | ExportResolution::Ambiguous => {
                        return Err(LinkError::NoSuchExport {
                            specifier: entry.specifier.to_string(),
                            name: name.to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// §16.2.1.6.2 `GetExportedNames` — every name a module offers, its stars followed.
    ///
    /// In the order a namespace object does not care about: §10.4.6.10 sorts them by code unit, so
    /// this only has to be complete. Duplicates are left in and removed there.
    ///
    /// §16.2.1.6.2 step 5.b's exclusion of `default` is **not** here, even though the clause puts it
    /// here. Every name this answers is handed straight to §16.2.1.6.3, whose step 4 refuses to reach
    /// a `default` through a star export — so a second copy of the rule changes no answer, and a rule
    /// stated twice is one that can be half-changed. It is stated where a test can reach it.
    fn exported_names(&self, chunk: &Rc<Chunk>) -> Vec<Box<str>> {
        let mut names: Vec<Box<str>> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        let mut pending: Vec<Rc<Chunk>> = vec![Rc::clone(chunk)];
        while let Some(module) = pending.pop() {
            // §16.2.1.6.2 step 1 — a module reached twice is a cycle, and stopping is what makes the
            // walk terminate. Two modules that `export *` from each other is a legal graph.
            if seen.contains(&identity(&module)) {
                continue;
            }
            seen.push(identity(&module));
            for export in module.exports() {
                names.push(export.export_name.clone());
            }
            for specifier in module.star_exports() {
                if let Some(from) = self.resolved.get(specifier) {
                    pending.push(Rc::clone(from));
                }
            }
        }
        names
    }

    /// §13.3.10 steps 2 to 7 — make the promise, and queue the work that settles it.
    ///
    /// Everything that can be observed happens later: the specifier is converted here because
    /// §13.3.10 step 5 converts it before the job exists — a `toString` that throws **rejects** the
    /// promise rather than throwing out of the `import()`, which is why the conversion's failure is
    /// a rejection and not an error.
    pub(super) fn begin_dynamic_import(
        &mut self,
        specifier: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let capability = crate::builtins::promise::new_promise_capability(
            self,
            heap,
            self.realm.promise_constructor(),
        )?;
        // Step 5 — `ToString`, and step 6 rejects with whatever it threw.
        let text = match self.to_string(specifier, heap) {
            Ok(text) => text,
            Err(error) => {
                let thrown = self.thrown_value(error, heap);
                self.settle_capability(
                    capability,
                    crate::heap::ReactionKind::Reject,
                    thrown,
                    heap,
                )?;
                return Ok(capability.promise);
            }
        };
        // Step 7 — §16.2.1.7 `HostLoadImportedModule`, which DR-0016 puts in a job: `import()` must
        // not settle synchronously, or a module's body would run in the middle of the expression
        // that asked for it.
        self.jobs.push_back(super::Job::Import {
            specifier: text,
            capability,
        });
        Ok(capability.promise)
    }

    /// §13.3.10 step 7 and §16.2.1.11 `ContinueDynamicImport` — load it, run it, settle.
    pub(super) fn import_job(
        &mut self,
        specifier: StringId,
        capability: crate::heap::Capability,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let text = String::from_utf16_lossy(heap.string(specifier).unwrap_or(&[]));
        // §14.2.2's completion value belongs to the *script*, and running a module body writes it —
        // that is how `run_module_graph` produces its answer. A job runs between statements of a
        // program that is still going, so it has to put back what it found: without this,
        // `typeof import("m")` evaluated to whatever `m`'s last statement did.
        let completion = self.completion;
        let outcome = self.load_and_run(&text, heap);
        self.completion = completion;
        let (kind, argument) = match outcome {
            // §16.2.1.11 step 5 — the module evaluated, and the promise is resolved with its
            // *namespace* rather than with what its body evaluated to.
            Ok(Ok(namespace)) => (crate::heap::ReactionKind::Fulfil, namespace),
            // A module that threw, or a host that could not load one. Both reject, and both reject
            // with a value rather than throwing out of the job — a job has nobody to throw to.
            Ok(Err(thrown)) => (crate::heap::ReactionKind::Reject, thrown),
            // A chunk that does not make sense is an engine bug, not something to hand a script.
            // A job has nobody to throw to, so it is reported as what it is: the promise rejects
            // with an error naming the fault, and the run continues rather than stopping.
            Err(fault) => {
                let message = format!("a module's chunk did not make sense: {fault:?}");
                let thrown = self
                    .realm
                    .error(heap, crate::realm::NativeError::Syntax, &message);
                (crate::heap::ReactionKind::Reject, thrown)
            }
        };
        self.settle_capability(capability, kind, argument, heap)
    }

    /// Load everything `specifier` reaches that is not loaded, link it, run it, and answer with its
    /// namespace — or with what a rejection should carry.
    ///
    /// `Ok(Err(value))` is a program-visible failure: a specifier the host cannot answer, a module
    /// that would not compile, a body that threw. `Err` is a chunk that does not make sense, which
    /// is a bug in the engine and not something to hand a script.
    fn load_and_run(
        &mut self,
        specifier: &str,
        heap: &mut Heap,
    ) -> Result<Result<Value, Value>, Fault> {
        if let Err(why) = self.load_reachable(specifier, heap) {
            return Ok(Err(self.host_error(&why, heap)));
        }
        match self.link_and_evaluate(specifier, heap)? {
            Ok(Outcome::Thrown(thrown)) => return Ok(Err(thrown)),
            Ok(Outcome::Value(_)) => {}
            Err(error) => return Ok(Err(self.host_error(&error.message(), heap))),
        }
        let Some(chunk) = self.resolved.get(specifier).cloned() else {
            return Ok(Err(self.host_error("no module was supplied", heap)));
        };
        Ok(Ok(Value::Object(self.namespace_of(&chunk, heap))))
    }

    /// §16.2.1.7 `HostLoadImportedModule`, over everything the module reaches.
    ///
    /// Depth-first and iterative, because a module's own dependencies are only known once it has
    /// been compiled — and because DR-0002 does not allow a graph's depth to be this engine's stack
    /// depth. A specifier already answered is not asked for again, which is what makes a cycle
    /// terminate and what makes a host's loader safe to write without a cache of its own.
    fn load_reachable(&mut self, specifier: &str, heap: &mut Heap) -> Result<(), String> {
        let mut pending = vec![specifier.to_string()];
        while let Some(asked) = pending.pop() {
            if self.resolved.get(&asked).is_some() {
                continue;
            }
            // Taken out for the call and put back after it: a loader is handed the heap, and a host
            // that compiles a module needs it mutably while this machine is borrowed too.
            let Some(mut loader) = self.loader.take() else {
                return Err(format!(
                    "no module loader is set, so {asked:?} cannot be resolved"
                ));
            };
            let loaded = loader.load(&asked, heap);
            self.loader = Some(loader);
            let chunk = loaded?;
            pending.extend(requested_modules(&chunk));
            self.resolved.insert(&asked, chunk);
        }
        Ok(())
    }

    /// The error a host reports for a module it could not supply — §16.2.1.7.
    ///
    /// A SyntaxError, which is what a host answers for a specifier that resolves nowhere and for a
    /// module that would not parse. It is made rather than thrown because everywhere this is used
    /// the value is about to *reject* a promise.
    fn host_error(&mut self, why: &str, heap: &mut Heap) -> Value {
        self.realm
            .error(heap, crate::realm::NativeError::Syntax, why)
    }

    /// The environment a linked module was given, if this machine has linked it.
    fn environment_of(&self, chunk: &Rc<Chunk>) -> Option<EnvironmentId> {
        self.modules
            .get(&identity(chunk))
            .map(|record| record.environment)
    }

    /// Walk the graph depth-first, giving each module an environment and recording the order.
    ///
    /// Post-order, so a module appears after everything it imports. A module linked by an *earlier*
    /// call is walked again all the same: its record already has its environment, so nothing is made
    /// twice, and the walk is what puts it in `order` where the evaluation can see whether it ran.
    ///
    /// `seen` is what **this walk** has reached, and it is not the same question as whether a record
    /// exists. A module is given its record before its dependencies are walked — a cycle has to find
    /// something and stop — so "has a record" is true halfway through its own visit, and reading
    /// that as "already placed" pushes a module into `order` ahead of the dependencies it is still
    /// descending into. §16.2.1.6 evaluates in the order this list ends up in, so that is a module
    /// running before what it imports.
    fn visit(
        &mut self,
        specifier: &str,
        seen: &mut Vec<usize>,
        order: &mut Vec<Rc<Chunk>>,
        heap: &mut Heap,
    ) -> Result<(), LinkError> {
        let Some(chunk) = self.resolved.get(specifier).cloned() else {
            return Err(LinkError::Unresolved(specifier.to_string()));
        };
        // A diamond, a cycle, or a module that imports itself — all three the same answer: this
        // walk has been here, and the module is either placed already or still descending.
        if seen.contains(&identity(&chunk)) {
            return Ok(());
        }
        seen.push(identity(&chunk));
        // §16.2.1.5.1 step 9 — the environment exists before anything is bound and before the
        // dependencies are walked, because a cycle means a module's imports may name one the walk
        // has not reached yet. Only for a module nothing has linked: a second environment for one
        // already linked would leave every binding of the first unreachable.
        if let std::collections::btree_map::Entry::Vacant(slot) =
            self.modules.entry(identity(&chunk))
        {
            let environment =
                heap.new_named_environment(None, chunk.locals(), Rc::clone(chunk.bindings()));
            slot.insert(super::loader::ModuleRecord {
                environment,
                namespace: None,
                evaluated: false,
                failure: None,
            });
        }
        for dependency in requested_modules(&chunk) {
            self.visit(&dependency, seen, order, heap)?;
        }
        order.push(chunk);
        Ok(())
    }
}

/// What §16.2.1.6.3 `ResolveExport` answers.
///
/// Named for the question rather than for the answer, because `dynamic::Resolved` is a different
/// resolution entirely — §9.4.2's, of a *name* against the running scopes — and the two appear a
/// few lines apart in the interpreter.
enum ExportResolution {
    /// A binding: which environment, and which slot in it.
    Slot(EnvironmentId, u32),
    /// A whole module, which is what `export * as n from "m"` resolves to.
    Namespace(crate::heap::ObjectId),
    /// No module in the graph exports this name — §16.2.1.6.3's `null`.
    Missing,
    /// Two `export *`s bring the same name from different modules — §16.2.1.6.3's `ambiguous`.
    ///
    /// Not an error until something asks for it: a module may have an ambiguous name and be
    /// perfectly usable, so long as nobody imports *that* name. It is left out of the namespace
    /// object for the same reason.
    Ambiguous,
}

impl Vm {
    /// §16.2.1.10 `GetModuleNamespace` — the object `import * as n` binds, memoised per module.
    ///
    /// The object is made and recorded **before** its names are resolved, because §16.2.1.6.3 can
    /// come back around to the module it started from: `export * as a from "b"` with `b` pointing
    /// back is a legal graph, and the walk terminates only because the object is already there to be
    /// found.
    pub(super) fn namespace_of(
        &mut self,
        chunk: &Rc<Chunk>,
        heap: &mut Heap,
    ) -> crate::heap::ObjectId {
        if let Some(made) = self
            .modules
            .get(&identity(chunk))
            .and_then(|record| record.namespace)
        {
            return made;
        }
        // A module the walk has not linked cannot happen here — every caller has just linked the
        // graph — but "cannot happen" is not a thing to encode as a panic. An empty environment
        // gives a namespace whose every export reads a slot that is not there, which is the dead
        // zone answer and not a wrong value.
        let environment = self
            .environment_of(chunk)
            .unwrap_or_else(|| heap.new_environment(None, 0));
        let object = heap.new_namespace(
            environment,
            self.realm
                .well_known(crate::builtins::well_known_at("toStringTag")),
        );
        if let Some(record) = self.modules.get_mut(&identity(chunk)) {
            record.namespace = Some(object);
        }
        // §16.2.1.10 step 7 — a name that resolves to nothing, or ambiguously, is simply not a
        // property of the namespace. Only a *direct* import of it is an error.
        let mut exports = Vec::new();
        for name in self.exported_names(chunk) {
            match self.resolve_export(chunk, &name, heap) {
                ExportResolution::Slot(environment, slot) => {
                    exports.push((name, crate::heap::NamespaceBinding::Slot(environment, slot)));
                }
                ExportResolution::Namespace(other) => exports.push((
                    name,
                    crate::heap::NamespaceBinding::Value(crate::value::Value::Object(other)),
                )),
                ExportResolution::Missing | ExportResolution::Ambiguous => {}
            }
        }
        heap.fill_namespace(object, exports);
        object
    }

    /// §16.2.1.6.3 `ResolveExport` — which binding, anywhere in the graph, an exported name is.
    ///
    /// Iterative over the indirect chain and looping over the star exports, with a `seen` list doing
    /// the specification's `resolveSet`: a cycle answers `Missing` rather than spinning, which is
    /// exactly what step 1 says to do.
    fn resolve_export(
        &mut self,
        chunk: &Rc<Chunk>,
        name: &str,
        heap: &mut Heap,
    ) -> ExportResolution {
        let mut seen: Vec<(usize, String)> = Vec::new();
        self.resolve_seen(chunk, name, &mut seen, heap)
    }

    /// The body of `ResolveExport`, with §16.2.1.6.3 step 1's `resolveSet` carried through.
    fn resolve_seen(
        &mut self,
        chunk: &Rc<Chunk>,
        name: &str,
        seen: &mut Vec<(usize, String)>,
        heap: &mut Heap,
    ) -> ExportResolution {
        // §16.2.1.6.3 step 1 — asked this of this module already, so the graph is circular here and
        // the answer is that this path finds nothing. Another path may still find something.
        if seen
            .iter()
            .any(|(module, asked)| *module == identity(chunk) && asked == name)
        {
            return ExportResolution::Missing;
        }
        seen.push((identity(chunk), name.to_string()));
        for export in chunk.exports() {
            if *export.export_name != *name {
                continue;
            }
            match &export.from {
                // Step 2 — a local export, which is the end of the walk.
                crate::compile::ExportSource::Local(slot) => {
                    return match self.environment_of(chunk) {
                        Some(environment) => ExportResolution::Slot(environment, *slot),
                        None => ExportResolution::Missing,
                    };
                }
                crate::compile::ExportSource::Indirect {
                    specifier,
                    import_name,
                } => {
                    let Some(from) = self.resolved.get(specifier).cloned() else {
                        return ExportResolution::Missing;
                    };
                    return match import_name {
                        // Step 3.a.ii — `export * as n from "m"` resolves to the *module*.
                        None => ExportResolution::Namespace(self.namespace_of(&from, heap)),
                        // Step 3.a.iii — ask the other module, which may send it further still.
                        Some(asked) => self.resolve_seen(&from, asked, seen, heap),
                    };
                }
            }
        }
        // §16.2.1.6.3 step 4 — `export *` never brings `default`. That is what makes
        // `export * from "m"` safe to write over a module that has one: the name a default would
        // collide with is the one name a star does not carry.
        if name == "default" {
            return ExportResolution::Missing;
        }
        // Step 6 — every star export, and two that disagree make the name ambiguous rather than
        // picking one. Two that agree — a diamond, where both paths reach the same binding — are
        // not ambiguous, which is why the comparison is of the resolution and not of the module
        // that answered.
        let mut found: Option<ExportResolution> = None;
        // Copied out because resolving one of them borrows this machine again — and a star export
        // list is a handful of names settled when the module was compiled.
        let stars: Vec<Box<str>> = chunk.star_exports().to_vec();
        for specifier in &stars {
            let Some(from) = self.resolved.get(specifier).cloned() else {
                continue;
            };
            let answer = self.resolve_seen(&from, name, seen, heap);
            match (&found, &answer) {
                (_, ExportResolution::Missing) => {}
                (_, ExportResolution::Ambiguous) | (Some(ExportResolution::Ambiguous), _) => {
                    return ExportResolution::Ambiguous;
                }
                (None, _) => found = Some(answer),
                (Some(ExportResolution::Slot(one, at)), ExportResolution::Slot(two, other))
                    if one == two && at == other => {}
                (Some(ExportResolution::Namespace(one)), ExportResolution::Namespace(two))
                    if one == two => {}
                (Some(_), _) => return ExportResolution::Ambiguous,
            }
        }
        found.unwrap_or(ExportResolution::Missing)
    }
}

/// §16.2.1.4's `[[RequestedModules]]` — every specifier a module names.
///
/// An `export` makes one as much as an `import` does: `export * from "m"` and `export { a } from
/// "m"` both require `m`, and `export {} from "m"` requires it while naming nothing at all.
fn requested_modules(chunk: &Rc<Chunk>) -> Vec<String> {
    chunk
        .imports()
        .iter()
        .map(|entry| entry.specifier.to_string())
        .chain(
            chunk
                .exports()
                .iter()
                .filter_map(|export| match &export.from {
                    crate::compile::ExportSource::Indirect { specifier, .. } => {
                        Some(specifier.to_string())
                    }
                    crate::compile::ExportSource::Local(_) => None,
                }),
        )
        .chain(chunk.star_exports().iter().map(ToString::to_string))
        .collect()
}

/// Which module a chunk *is*, for a map that must not tell two names for one module apart./// Which module a chunk *is*, for a map that must not tell two names for one module apart.
///
/// The address, which is stable because a `Graph` holds every chunk by `Rc` for the whole link and
/// evaluation — nothing here creates or drops one. It is never dereferenced and never outlives the
/// call; it is only an identity.
fn identity(chunk: &Rc<Chunk>) -> usize {
    Rc::as_ptr(chunk) as usize
}
