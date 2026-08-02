//! What the `property-escapes` bucket actually costs — time, and what a collection can give back.
//!
//! The question: 878 of those tests fail with DR-0013's RangeError. Is that a *memory* problem a
//! collector fixes, or a *time* problem that only a faster interpreter fixes? The engine's own
//! conformance run cannot tell the two apart, because a test that runs out of time is dropped by
//! the harness into no column at all.
//!
//! Takes a test262 file, prepends the harness includes it needs, and runs it with a wall clock.

use praxis::compile::compile_script;
use praxis::heap::Heap;
use praxis::parser::parse_script;
use praxis::vm::{Outcome, Vm};
use std::path::Path;
use std::time::Instant;

/// The harness files every one of these tests includes, in the order test262 loads them.
const INCLUDES: [&str; 3] = ["assert.js", "sta.js", "regExpUtils.js"];

pub fn run(argument: Option<&str>) -> std::process::ExitCode {
    let Some(file) = argument else {
        eprintln!("usage: cargo run -p praxis-lab -- gc-pressure <path to a test262 file>");
        eprintln!("       (TEST262 names the checkout, for the harness includes)");
        return std::process::ExitCode::FAILURE;
    };
    let root = std::env::var("TEST262").unwrap_or_else(|_| "../test262".to_string());
    let mut source = String::new();
    for include in INCLUDES {
        let path = Path::new(&root).join("harness").join(include);
        match std::fs::read_to_string(&path) {
            Ok(text) => source.push_str(&text),
            Err(error) => {
                eprintln!("could not read {}: {error}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        }
        source.push('\n');
    }
    match std::fs::read_to_string(file) {
        Ok(text) => source.push_str(&text),
        Err(error) => {
            eprintln!("could not read {file}: {error}");
            return std::process::ExitCode::FAILURE;
        }
    }

    let started = Instant::now();
    let mut heap = Heap::new();
    let script = match parse_script(&source) {
        Ok(script) => script,
        Err(error) => {
            println!("parse failed: {}", error.kind);
            return std::process::ExitCode::SUCCESS;
        }
    };
    let chunk = match compile_script(&script, &mut heap) {
        Ok(chunk) => chunk,
        Err(error) => {
            println!("compile failed: {}", error.message());
            return std::process::ExitCode::SUCCESS;
        }
    };
    let compiled = started.elapsed();
    let mut vm = Vm::new(&mut heap);
    let outcome = vm.run(&chunk, &mut heap);
    let finished = started.elapsed();

    let verdict = match outcome {
        Ok(Outcome::Value(value)) => match value.to_string(&mut heap) {
            Ok(id) => format!(
                "completed: {}",
                String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
            ),
            Err(_) => "completed with something that will not print".to_string(),
        },
        Ok(Outcome::Thrown(value)) => match value.to_string(&mut heap) {
            Ok(id) => format!(
                "threw: {}",
                String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
            ),
            Err(_) => "threw something that will not print".to_string(),
        },
        Err(fault) => format!("fault: {fault:?}"),
    };
    println!("{verdict}");
    println!(
        "compile {:?}, run {:?}, total {:?}",
        compiled,
        finished - compiled,
        finished
    );
    println!("footprint at the end: {} MiB", heap.footprint() / 1_048_576);
    std::process::ExitCode::SUCCESS
}
