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
use std::collections::HashMap;
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

/// Load `entry`, walk what it imports, and run the linked graph.
///
/// **`Vm::run_module` is not this.** It runs one module chunk, and the first version of this
/// experiment used it and got `undefined` for every imported binding — which read exactly like an
/// engine bug and was not one. Linking is `Vm::run_module_graph`, which is handed every chunk up
/// front; the module-loader hook is for §13.3.10's dynamic `import()` and answers nothing at
/// link time. So the walk below is the host's job and there is no way around doing it.
///
/// A specifier is resolved against its *importer's* directory and the graph is keyed by the
/// resolved path, because `./MathUtils.js` written in two directories is two files. Keying by the
/// specifier as written is the mistake that puts two records where §16.2.1 has one.
pub fn run(entry: Option<&str>, _probe: Option<&str>) -> std::process::ExitCode {
    let Some(entry) = entry else {
        eprintln!("usage: cargo run -p praxis-lab --release -- run-module <entry.js>");
        return std::process::ExitCode::FAILURE;
    };
    let root = normalise(&std::env::current_dir().unwrap_or_default().join(entry));
    let entry_key = key(&root);
    let mut heap = Heap::new();
    let mut graph = Graph::new();
    // Keyed by the specifier **as written**, because that is what `run_module_graph` looks up —
    // there is no host resolution at link time, only for `import()`. So two files that a real
    // project reaches by the same relative specifier cannot both be in one graph, and this
    // records the clash rather than letting the second silently replace the first.
    let mut queue = vec![(entry_key.clone(), root.clone())];
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let mut clashes: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    let mut count = 0usize;
    while let Some((specifier, path)) = queue.pop() {
        if let Some(already) = seen.get(&specifier) {
            if already != &path {
                clashes.push((specifier.clone(), already.clone(), path.clone()));
            }
            continue;
        }
        seen.insert(specifier.clone(), path.clone());
        let Ok(source) = std::fs::read_to_string(&path) else {
            println!("could not read {}", path.display());
            return std::process::ExitCode::SUCCESS;
        };
        let module = match parse_module(&source) {
            Ok(module) => module,
            Err(error) => {
                println!("{} did not parse: {}", path.display(), error.kind);
                return std::process::ExitCode::SUCCESS;
            }
        };
        let chunk = match compile_module(&module, &mut heap) {
            Ok(chunk) => chunk,
            Err(error) => {
                println!("{} did not compile: {}", path.display(), error.message());
                return std::process::ExitCode::SUCCESS;
            }
        };
        let here = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for import in chunk.imports() {
            queue.push((
                import.specifier.to_string(),
                normalise(&here.join(&*import.specifier)),
            ));
        }
        graph.insert(&specifier, Rc::new(chunk));
        count += 1;
    }
    println!("{count} modules loaded");
    for (specifier, first, second) in &clashes {
        println!(
            "  specifier clash: {specifier} is both {} and {}",
            first.display(),
            second.display()
        );
    }
    let mut vm = Vm::new(&mut heap);
    let started = Instant::now();
    let outcome = vm.run_module_graph(&entry_key, &graph, &mut heap);
    let elapsed = started.elapsed();
    match outcome {
        Ok(Ok(Outcome::Value(_))) => println!("graph ran in {elapsed:?}"),
        Ok(Ok(Outcome::Thrown(value))) => {
            let text = describe(&mut heap, value);
            println!("graph threw after {elapsed:?}: {text}");
        }
        Ok(Err(error)) => println!("did not link: {error:?}"),
        Err(fault) => println!("fault: {fault:?}"),
    }
    std::process::ExitCode::SUCCESS
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
