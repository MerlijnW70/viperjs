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
use crate::heap::{EnvironmentId, Heap};
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
    /// §16.2.1.5 `Link` and §16.2.1.6 `Evaluate`, over a graph the host has resolved.
    ///
    /// The entry module is evaluated last, because everything it imports has to have run first —
    /// that is what makes an imported binding hold a value rather than sit in its dead zone.
    pub fn run_module_graph(
        &mut self,
        entry: &str,
        graph: &Graph,
        heap: &mut Heap,
    ) -> Result<Result<Outcome, LinkError>, Fault> {
        // §16.2.1.5.1 step 9 — every module in the graph gets its environment before any of them
        // is bound, because a cycle means a module's imports may name one that has not been
        // reached yet.
        //
        // Keyed by the **chunk**, not by the specifier that reached it. Two specifiers may name one
        // module — a diamond, or a module that imports *itself*, which `instn-named-bndng-cls.js`
        // does — and a module is one record however many names lead to it. Keyed by name, a
        // self-import became a second copy that evaluated the whole body a second time.
        let mut order = Vec::new();
        let mut environments = BTreeMap::new();
        if let Err(error) = visit(entry, graph, &mut order, &mut environments, heap) {
            return Ok(Err(error));
        }
        // §16.2.1.5.2 `InitializeEnvironment` — and only now, when every environment exists.
        //
        // §16.2.1.10's `GetModuleNamespace` memoises one namespace object per *module*, so that
        // `import * as a from "m"` and `import * as b from "m"` in two importers give the same
        // object and `a === b`. Keyed by the same identity the environments are, for the same
        // reason: two specifiers may name one module.
        let mut namespaces: BTreeMap<usize, crate::heap::ObjectId> = BTreeMap::new();
        let mut link = Link {
            graph,
            environments: &environments,
            namespaces: &mut namespaces,
        };
        for chunk in &order {
            for entry in chunk.imports() {
                let Some(slot) = entry.slot else {
                    // `import "a";` binds nothing and is written for the other module being
                    // evaluated. The edge is still in the graph, which is what makes that happen.
                    continue;
                };
                let Some(from) = graph.get(&entry.specifier) else {
                    return Ok(Err(LinkError::Unresolved(entry.specifier.to_string())));
                };
                let Some(&here) = environments.get(&identity(chunk)) else {
                    continue;
                };
                // §16.2.1.5.2 step 1.a — `import * as n` binds an ordinary initialised slot holding
                // the namespace object, and **not** one of the module's own slots. That is the one
                // import that is a value rather than an alias, which is why it is settled here
                // rather than by `bind_import` below.
                let Some(name) = entry.import_name.as_ref() else {
                    let namespace = self.namespace_of(from, &mut link, heap);
                    heap.set_variable(here, slot, crate::value::Value::Object(namespace));
                    continue;
                };
                // §16.2.1.5.2 step 1.b — `ResolveExport`, which may walk through several modules
                // before it finds a binding: `export { a } from "m"` and `export * from "m"` both
                // send it further.
                match self.resolve_export(from, name, &mut link, heap) {
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
                        return Ok(Err(LinkError::NoSuchExport {
                            specifier: entry.specifier.to_string(),
                            name: name.to_string(),
                        }));
                    }
                }
            }
        }
        // §16.2.1.6.4 step 3 — **every** indirect export is resolved here, and not only the ones
        // something imports. `export { x } from "m"` where `m`'s `x` is ambiguous is a SyntaxError
        // for the module that wrote the line, whether or not anything ever asks it for `x`.
        for chunk in &order {
            for export in chunk.exports() {
                if matches!(export.from, crate::compile::ExportSource::Local(_)) {
                    continue;
                }
                if matches!(
                    self.resolve_export(chunk, &export.export_name, &mut link, heap),
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
            let Some(&environment) = environments.get(&identity(chunk)) else {
                continue;
            };
            outcome = self.run_module_in(chunk, environment, heap)?;
            // §16.2.1.6 step 12 — a module that throws stops the evaluation, and the throw is the
            // answer. Nothing after it runs, including the entry.
            if matches!(outcome, Outcome::Thrown(_)) {
                return Ok(Ok(outcome));
            }
        }
        Ok(Ok(outcome))
    }
}

/// What linking needs to hand about between the modules — the graph and what has been made for it.
///
/// One struct rather than four parameters because §16.2.1.6.2 and §16.2.1.6.3 call each other:
/// resolving a name may need another module's namespace, and building a namespace resolves every
/// name in it.
struct Link<'a> {
    /// Every module the host supplied, by the specifier its importers wrote.
    graph: &'a Graph,
    /// The environment each module was given, by module identity.
    environments: &'a BTreeMap<usize, EnvironmentId>,
    /// §16.2.1.10's memo — one namespace object per module, however many times it is asked for.
    namespaces: &'a mut BTreeMap<usize, crate::heap::ObjectId>,
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
    fn namespace_of(
        &self,
        chunk: &Rc<Chunk>,
        link: &mut Link<'_>,
        heap: &mut Heap,
    ) -> crate::heap::ObjectId {
        if let Some(&made) = link.namespaces.get(&identity(chunk)) {
            return made;
        }
        let environment = link
            .environments
            .get(&identity(chunk))
            .copied()
            .unwrap_or_else(|| heap.new_environment(None, 0));
        let object = heap.new_namespace(
            environment,
            self.realm
                .well_known(crate::builtins::well_known_at("toStringTag")),
        );
        link.namespaces.insert(identity(chunk), object);
        // §16.2.1.10 step 7 — a name that resolves to nothing, or ambiguously, is simply not a
        // property of the namespace. Only a *direct* import of it is an error.
        let mut exports = Vec::new();
        for name in exported_names(chunk, link.graph) {
            match self.resolve_export(chunk, &name, link, heap) {
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
        &self,
        chunk: &Rc<Chunk>,
        name: &str,
        link: &mut Link<'_>,
        heap: &mut Heap,
    ) -> ExportResolution {
        let mut seen: Vec<(usize, String)> = Vec::new();
        self.resolve_seen(chunk, name, &mut seen, link, heap)
    }

    /// The body of `ResolveExport`, with §16.2.1.6.3 step 1's `resolveSet` carried through.
    fn resolve_seen(
        &self,
        chunk: &Rc<Chunk>,
        name: &str,
        seen: &mut Vec<(usize, String)>,
        link: &mut Link<'_>,
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
                    return match link.environments.get(&identity(chunk)) {
                        Some(&environment) => ExportResolution::Slot(environment, *slot),
                        None => ExportResolution::Missing,
                    };
                }
                crate::compile::ExportSource::Indirect {
                    specifier,
                    import_name,
                } => {
                    let Some(from) = link.graph.get(specifier).cloned() else {
                        return ExportResolution::Missing;
                    };
                    return match import_name {
                        // Step 3.a.ii — `export * as n from "m"` resolves to the *module*.
                        None => ExportResolution::Namespace(self.namespace_of(&from, link, heap)),
                        // Step 3.a.iii — ask the other module, which may send it further still.
                        Some(asked) => self.resolve_seen(&from, asked, seen, link, heap),
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
        for specifier in chunk.star_exports() {
            let Some(from) = link.graph.get(specifier).cloned() else {
                continue;
            };
            let answer = self.resolve_seen(&from, name, seen, link, heap);
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

/// §16.2.1.6.2 `GetExportedNames` — every name a module offers, its stars followed.
///
/// In the order a namespace object does not care about: §10.4.6.10 sorts them by code unit, so this
/// only has to be complete. Duplicates are left in and removed there.
fn exported_names(chunk: &Rc<Chunk>, graph: &Graph) -> Vec<Box<str>> {
    let mut names: Vec<Box<str>> = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    let mut pending: Vec<Rc<Chunk>> = vec![Rc::clone(chunk)];
    let mut first = true;
    while let Some(module) = pending.pop() {
        // §16.2.1.6.2 step 1 — a module reached twice is a cycle, and stopping is what makes the
        // walk terminate.
        if seen.contains(&identity(&module)) {
            continue;
        }
        seen.push(identity(&module));
        for export in module.exports() {
            // §16.2.1.6.2 step 5.b — `default` is a name a star export does not carry, so it counts
            // only for the module actually asked.
            if first || *export.export_name != *"default" {
                names.push(export.export_name.clone());
            }
        }
        for specifier in module.star_exports() {
            if let Some(from) = graph.get(specifier) {
                pending.push(Rc::clone(from));
            }
        }
        first = false;
    }
    names
}

/// Which module a chunk *is*, for a map that must not tell two names for one module apart./// Which module a chunk *is*, for a map that must not tell two names for one module apart.
///
/// The address, which is stable because a `Graph` holds every chunk by `Rc` for the whole link and
/// evaluation — nothing here creates or drops one. It is never dereferenced and never outlives the
/// call; it is only an identity.
fn identity(chunk: &Rc<Chunk>) -> usize {
    Rc::as_ptr(chunk) as usize
}

/// Walk the graph depth-first, making an environment per module and recording the order.
///
/// Post-order, so a module appears after everything it imports. A module already in
/// `environments` is one the walk has reached before — which is either a diamond or a cycle, and
/// the answer to both is the same: it has its environment and its place in the order already.
fn visit(
    specifier: &str,
    graph: &Graph,
    order: &mut Vec<Rc<Chunk>>,
    environments: &mut BTreeMap<usize, EnvironmentId>,
    heap: &mut Heap,
) -> Result<(), LinkError> {
    let Some(chunk) = graph.get(specifier) else {
        return Err(LinkError::Unresolved(specifier.to_string()));
    };
    if environments.contains_key(&identity(chunk)) {
        return Ok(());
    }
    // Recorded *before* the dependencies are walked, so a cycle — and a module that imports itself
    // — finds it and stops rather than descending for ever. §16.2.1.5.1's `[[Status]]` is what the
    // specification uses for this and the map is what praxis has.
    let environment = heap.new_named_environment(None, chunk.locals(), Rc::clone(chunk.bindings()));
    environments.insert(identity(chunk), environment);
    // §16.2.1.4's `[[RequestedModules]]` — every specifier the module names, which an `export` does
    // as much as an `import`: `export * from "m"` and `export { a } from "m"` make `m` a dependency
    // that is evaluated in order like any other, even though this module binds nothing from it.
    let dependencies: Vec<String> = chunk
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
        .collect();
    for dependency in dependencies {
        visit(&dependency, graph, order, environments, heap)?;
    }
    order.push(Rc::clone(chunk));
    Ok(())
}
