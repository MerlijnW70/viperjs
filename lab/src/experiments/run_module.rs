//! Run a real module graph off the disk, and see how far a library gets.
//!
//! ```text
//! cargo run -p praxis-lab --release -- run-module <entry.js> [expression]
//! ```
//!
//! # The question
//!
//! `examples/parse` sweeps a repository and answers "does it parse". That is the front end only.
//! §16.2's linker, the namespace objects and the live bindings are all *runtime*, and nothing here
//! has ever pointed them at a graph somebody else wrote. This does: a a module loader that
//! resolves a relative specifier against the importing file and reads it from disk, which is the
//! host hook `src/vm/loader.rs` was built for.
//!
//! # Why the loader is nine lines and the interesting part is the resolution
//!
//! The engine memoises — a specifier it has loaded is never asked for again — so a host needs no
//! cache. What it does need is to answer *consistently*, and the trap is that `./MathUtils.js`
//! means a different file depending on which module wrote it. So the key is the resolved absolute
//! path and never the specifier as written; keyed by the specifier, two importers of the same
//! neighbour would get two records where §16.2.1 has one.
//!
//! # What it cannot do
//!
//! Anything a browser supplies. A module that touches `document`, `WebGL2RenderingContext` or
//! `performance` throws a ReferenceError, and that is the correct answer rather than a gap — those
//! are host objects and praxis is not a host. Pure computation is what this is pointed at.

use praxis::compile::compile_module;
use praxis::heap::Heap;
use praxis::parser::parse_module;
use praxis::vm::{Graph, Outcome, Vm};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

/// `a/b/../c` as `a/c`, without asking the filesystem — the file may not exist and the error for
/// that belongs to the read rather than to the resolution.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Load `entry`, and let the engine pull the rest through a loader that resolves relatively.
///
/// The host supplies **only the entry**. Everything else arrives through [`FromDisk`], which is
/// handed the referrer — DR-0020 — and so can resolve `./MathUtils.js` against the directory of
/// the module that wrote it rather than against a guess. Before that record this walked the graph
/// by hand and keyed it by the specifier as written, which meant two directories saying
/// `./index.js` collided.
pub fn run(entry: Option<&str>, _probe: Option<&str>) -> std::process::ExitCode {
    let Some(entry) = entry else {
        eprintln!("usage: cargo run -p praxis-lab --release -- run-module <entry.js>");
        return std::process::ExitCode::FAILURE;
    };
    let root = normalise(&std::env::current_dir().unwrap_or_default().join(entry));
    let entry_key = key(&root);
    let mut heap = Heap::new();
    let Ok(source) = std::fs::read_to_string(&root) else {
        println!("could not read {}", root.display());
        return std::process::ExitCode::SUCCESS;
    };
    let module = match parse_module(&source) {
        Ok(module) => module,
        Err(error) => {
            println!("{} did not parse: {}", root.display(), error.kind);
            return std::process::ExitCode::SUCCESS;
        }
    };
    let chunk = match compile_module(&module, &mut heap) {
        Ok(chunk) => chunk,
        Err(error) => {
            println!("{} did not compile: {}", root.display(), error.message());
            return std::process::ExitCode::SUCCESS;
        }
    };
    let mut graph = Graph::new();
    graph.insert(&entry_key, Rc::new(chunk));
    let loaded = Rc::new(std::cell::Cell::new(1usize));
    let mut vm = Vm::new(&mut heap);
    vm.set_module_loader(Box::new(FromDisk {
        loaded: Rc::clone(&loaded),
    }));
    let started = Instant::now();
    let outcome = vm.run_module_graph(&entry_key, &graph, &mut heap);
    let elapsed = started.elapsed();
    println!("{} modules loaded", loaded.get());
    match outcome {
        Ok(Ok(Outcome::Value(_))) => println!("graph ran in {elapsed:?}"),
        Ok(Ok(Outcome::Thrown(value))) => {
            let text = describe(&mut heap, value);
            println!("graph threw after {elapsed:?}: {text}");
        }
        Ok(Err(error)) => println!("did not link: {}", error.message()),
        Err(fault) => println!("fault: {fault:?}"),
    }
    std::process::ExitCode::SUCCESS
}

/// Resolves each specifier against the **directory of the module that wrote it**, and answers with
/// the resolved path as the key.
///
/// Nine lines, and every one of them impossible before DR-0020: without the referrer there is
/// nothing to resolve against, and without answering a key the engine would file the module under
/// the text rather than under what it turned out to be.
struct FromDisk {
    /// How many modules were read, for the report.
    loaded: Rc<std::cell::Cell<usize>>,
}

impl praxis::vm::ModuleLoader for FromDisk {
    fn load(
        &mut self,
        referrer: Option<&str>,
        specifier: &str,
        heap: &mut Heap,
    ) -> Result<(String, Rc<praxis::compile::Chunk>), String> {
        // The referrer is a *file*, so what a relative specifier is relative to is its directory.
        let base = referrer
            .map(|from| normalise(Path::new(from)))
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let path = normalise(&base.join(specifier));
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let module = parse_module(&source)
            .map_err(|error| format!("{} did not parse: {}", path.display(), error.kind))?;
        let chunk = compile_module(&module, heap)
            .map_err(|error| format!("{} did not compile: {}", path.display(), error.message()))?;
        self.loaded.set(self.loaded.get() + 1);
        Ok((key(&path), Rc::new(chunk)))
    }
}

/// The graph's key for a file — its resolved path, which is a module's identity.
fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// What a thrown value says about itself, using only what the embedding surface exposes.
fn describe(heap: &mut Heap, value: praxis::value::Value) -> String {
    let praxis::value::Value::Object(id) = value else {
        return value
            .to_string(heap)
            .ok()
            .and_then(|s| heap.string(s).map(String::from_utf16_lossy))
            .unwrap_or_else(|| "a value that will not print".to_string());
    };
    let mut parts = Vec::new();
    for name in ["name", "message"] {
        let key =
            praxis::heap::PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
        let held = heap
            .object(id)
            .and_then(|found| found.get_own_property(key))
            .map(|property| property.kind);
        if let Some(praxis::heap::PropertyKind::Data { value, .. }) = held
            && let Ok(text) = value.to_string(heap)
            && let Some(units) = heap.string(text)
        {
            parts.push(String::from_utf16_lossy(units));
        }
    }
    match parts.is_empty() {
        true => "an object with nothing to say".to_string(),
        false => parts.join(": "),
    }
}
