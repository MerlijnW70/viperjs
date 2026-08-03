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
//! here by having nothing else. Namespace objects (`import * as n`), `export *` and re-exports are
//! refused when the module is compiled, so nothing here has to answer for them.

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
        for chunk in &order {
            for entry in chunk.imports() {
                let (Some(slot), Some(name)) = (entry.slot, entry.import_name.as_ref()) else {
                    // `import "a";` binds nothing, and `import * as n` is a namespace object the
                    // compiler refuses — so there is nothing to bind either way.
                    continue;
                };
                let Some(from) = graph.get(&entry.specifier) else {
                    return Ok(Err(LinkError::Unresolved(entry.specifier.to_string())));
                };
                let Some(exported) = from
                    .exports()
                    .iter()
                    .find(|export| *export.export_name == **name)
                else {
                    return Ok(Err(LinkError::NoSuchExport {
                        specifier: entry.specifier.to_string(),
                        name: name.to_string(),
                    }));
                };
                let (Some(&here), Some(&there)) = (
                    environments.get(&identity(chunk)),
                    environments.get(&identity(from)),
                ) else {
                    continue;
                };
                heap.bind_import(here, slot, there, exported.slot);
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

/// Which module a chunk *is*, for a map that must not tell two names for one module apart.
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
    let dependencies: Vec<String> = chunk
        .imports()
        .iter()
        .map(|entry| entry.specifier.to_string())
        .collect();
    for dependency in dependencies {
        visit(&dependency, graph, order, environments, heap)?;
    }
    order.push(Rc::clone(chunk));
    Ok(())
}
