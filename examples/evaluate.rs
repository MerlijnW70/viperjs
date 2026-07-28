//! Evaluate expressions and print what they came to — the tool the differential sweep drives.
//!
//! Reads one script per line from standard input and writes one answer per line: the result
//! written the way `String(x)` writes it, or a line beginning `!` for source praxis cannot yet
//! read. Printing the refusals rather than skipping them is the point — a sweep that quietly
//! dropped what the engine could not do would report agreement it had not earned.
//!
//! ```text
//! echo "1 + '1'" | cargo run --example evaluate
//! ```

use praxis::compile::compile_script;
use praxis::heap::Heap;
use praxis::parser::parse_script;
use praxis::vm::{Outcome, Vm};
use std::io::{self, BufRead, Write};

fn main() {
    let input = io::stdin();
    let output = io::stdout();
    let mut writer = io::BufWriter::new(output.lock());
    for line in input.lock().lines() {
        let Ok(source) = line else {
            break;
        };
        let answer = evaluate(&source);
        // A write that fails means the reader went away, which is not this program's problem.
        if writeln!(writer, "{answer}").is_err() {
            return;
        }
    }
    let _ = writer.flush();
}

/// What `source` evaluates to, or why it could not be evaluated.
fn evaluate(source: &str) -> String {
    let mut heap = Heap::new();
    let script = match parse_script(source) {
        Ok(script) => script,
        Err(error) => return format!("!parse: {}", error.kind),
    };
    let chunk = match compile_script(&script, &mut heap) {
        Ok(chunk) => chunk,
        Err(error) => return format!("!compile: {}", error.message()),
    };
    // A thrown outcome is marked rather than printed bare, so that a sweep comparing this
    // against another engine cannot read `throw 1` and `1` as the same answer.
    let mut vm = Vm::new(&mut heap);
    let (prefix, value) = match vm.run(&chunk, &mut heap) {
        Ok(Outcome::Value(value)) => ("", value),
        Ok(Outcome::Thrown(value)) => ("!thrown: ", value),
        Err(fault) => return format!("!fault: {fault:?}"),
    };
    // An object has no `toString` to call yet, so writing one down throws again. Naming it by its
    // type says which it was without the description itself failing.
    let Ok(id) = value.to_string(&mut heap) else {
        return format!("{prefix}[{}]", value.type_of(&heap));
    };
    format!(
        "{prefix}{}",
        String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
    )
}
