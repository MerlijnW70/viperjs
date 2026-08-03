//! Where the interpreter's time and its allocations actually go, per source shape.
//!
//! ```text
//! cargo run -p praxis-lab --release -- hot-shapes           # every shape
//! cargo run -p praxis-lab --release -- hot-shapes <name>    # one shape
//! ```
//!
//! # The question
//!
//! `gc-pressure` established that `RegExp/property-escapes` is a *time* problem and not a memory
//! one: even a zero-cost collector leaves `ASCII.js` at 21.8 s against a 10 s budget. It left two
//! numbers pointing at the interpreter and did not chase them —
//!
//! - `for (let i …)` costs 4.2 us/iteration over `for (var i …)`
//! - `a[0] = i` costs 1.9 us/store and **17 MiB per million**, at a single index
//!
//! — which are the two candidates for "make the interpreter several times faster". This measures
//! them precisely enough to say what eliding each would buy, before anything in `src/` is touched.
//!
//! # Why a matrix and not a benchmark
//!
//! A single number for "the interpreter" is useless: the question is which *shape* is expensive,
//! because a compiler can only fix a shape it can recognise. So each row is one source shape run
//! over a fixed iteration count, and the interesting quantity is always a **difference between two
//! rows that differ in one thing**. `let` against `var` is the per-iteration environment; `a[0]`
//! against `o.x` is the element path against the named one; `a[0]` read against `a[0]` written is
//! which direction allocates.
//!
//! Footprint is counted the same way `Heap::footprint` counts it — arena slots, which DR-0010 does
//! not reuse — so a row's growth is what that shape *retains*, not what it touched.
//!
//! # What this cannot see
//!
//! Anything below the instruction dispatch. There is no profiler here and `lab` cannot instrument
//! `praxis`, so a row that is slow says "this shape is slow" and never "this function is slow".
//! Attribution beyond that is by subtraction, and the shapes are chosen to make the subtractions
//! meaningful — see the pairs above.

use praxis::compile::compile_script;
use praxis::heap::Heap;
use praxis::parser::parse_script;
use praxis::vm::Vm;
use std::time::{Duration, Instant};

/// How many times each shape's body runs. Large enough that the loop dominates the fixed cost of
/// parsing and compiling, small enough that the slowest row finishes in a few seconds.
const ITERATIONS: usize = 100_000;

/// One measured shape: a name, and the body that runs `ITERATIONS` times.
///
/// Every body is written to leave its result somewhere the compiler cannot discard, because an
/// engine that dropped the work would make this measure nothing at all.
struct Shape {
    /// What the row is called, and what selects it on the command line.
    name: &'static str,
    /// The loop, already written out.
    source: &'static str,
    /// What this row is *for* — which subtraction it takes part in.
    about: &'static str,
}

/// The shapes, ordered so that each is next to the one it is subtracted from.
const SHAPES: &[Shape] = &[
    Shape {
        name: "empty-var",
        source: "for (var i = 0; i < N; i++) {}",
        about: "the yardstick: dispatch and nothing else",
    },
    Shape {
        name: "fn-empty-var",
        source: "(function () { for (var i = 0; i < N; i++) {} })()",
        about: "the same loop in a function — where `var` is a slot rather than a global property",
    },
    Shape {
        name: "fn-empty-let",
        source: "(function () { for (let i = 0; i < N; i++) {} })()",
        about: "minus fn-empty-var = the per-iteration environment, measured honestly",
    },
    Shape {
        name: "empty-let",
        source: "for (let i = 0; i < N; i++) {}",
        about: "minus empty-var = §14.7.4.7's per-iteration environment",
    },
    Shape {
        name: "empty-let-captured",
        source: "var sink; for (let i = 0; i < N; i++) { sink = function () { return i } }",
        about: "the same loop where a closure DOES capture it — the case that needs the copy",
    },
    Shape {
        name: "empty-block-let",
        source: "for (var i = 0; i < N; i++) { let x = i; }",
        about: "a `let` in the body rather than the head — a scope per pass, no copy",
    },
    Shape {
        name: "named-store",
        source: "var o = {x: 0}; for (var i = 0; i < N; i++) { o.x = i; }",
        about: "the named-property path, as the floor for a store",
    },
    Shape {
        name: "element-store",
        source: "var a = [0]; for (var i = 0; i < N; i++) { a[0] = i; }",
        about: "minus named-store = what an element index costs, at one index",
    },
    Shape {
        name: "element-read",
        source: "var a = [7]; var s = 0; for (var i = 0; i < N; i++) { s = a[0]; }",
        about: "against element-store = which direction allocates",
    },
    Shape {
        name: "element-store-growing",
        source: "var a = []; for (var i = 0; i < N; i++) { a[i] = i; }",
        about: "minus element-store = what a *varying* index costs on top",
    },
    Shape {
        name: "string-concat",
        source: "var s = ''; for (var i = 0; i < N; i++) { s = 'x'; }",
        about: "a String per pass, as a known-allocating control",
    },
    Shape {
        name: "call",
        source: "function f(a) { return a } var s = 0; for (var i = 0; i < N; i++) { s = f(i); }",
        about: "one call per pass — a frame and an environment",
    },
];

/// Run every shape, or the one named.
pub fn run(argument: Option<&str>) -> std::process::ExitCode {
    let chosen: Vec<&Shape> = match argument {
        Some(name) => SHAPES.iter().filter(|shape| shape.name == name).collect(),
        None => SHAPES.iter().collect(),
    };
    if chosen.is_empty() {
        eprintln!("praxis-lab: no shape named `{}`", argument.unwrap_or(""));
        eprintln!(
            "shapes: {}",
            SHAPES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
        );
        return std::process::ExitCode::FAILURE;
    }
    println!("{ITERATIONS} iterations per shape, --release\n");
    println!(
        "{:<24} {:>10} {:>16} {:>10} {:>9}  {}",
        "shape", "time", "per pass", "leak/pass", "after gc", "what it is for"
    );
    println!("{}", "-".repeat(96));
    let mut floor: Option<Duration> = None;
    for shape in chosen {
        let Some((elapsed, retained, after, finished)) = measure(shape.source) else {
            println!("{:<24} {:>10}  {}", shape.name, "FAILED", shape.about);
            continue;
        };
        let count = u32::try_from(ITERATIONS).unwrap_or(u32::MAX);
        // The first row measured is the yardstick every other row is read against, which is why
        // `empty-var` is first in the table and why selecting one shape prints no delta.
        let per_pass = elapsed / count;
        let over = match (floor, finished) {
            (Some(base), true) => format!("+{:?}", per_pass.saturating_sub(base / count)),
            // A row that stopped early ran an unknown number of passes, so a per-pass figure for
            // it would be a division by a count it never reached. Say so instead of computing one.
            _ => "-".to_string(),
        };
        println!(
            "{:<24} {:>10.2?} {:>16} {:>8} B {:>7} B  {}",
            shape.name,
            elapsed,
            match finished {
                true => format!("{per_pass:?} ({over})"),
                false => "OUT OF HEAP".to_string(),
            },
            retained / ITERATIONS,
            after / ITERATIONS,
            shape.about
        );
        if floor.is_none() && finished {
            floor = Some(elapsed);
        }
    }
    reuse_check();
    println!("\nread the differences, not the rows — see this module's doc for the pairs.");
    println!("OUT OF HEAP is DR-0013's budget reached: the row stopped early and its time is not");
    println!("comparable. It is a result — that shape cannot run a million passes at all.");
    std::process::ExitCode::SUCCESS
}

/// Run one shape and answer how long it took and how much arena it retained.
///
/// `None` for a shape that did not parse, compile or run — which is a bug in the shape rather than
/// a result, and is printed as such.
fn measure(body: &str) -> Option<(Duration, usize, usize, bool)> {
    let source = format!("var N = {ITERATIONS}; {body}");
    let mut heap = Heap::new();
    let script = parse_script(&source).ok()?;
    let chunk = compile_script(&script, &mut heap).ok()?;
    // Measured after compiling, so the number is the interpreter's and not the front end's.
    let before = heap.footprint();
    let started = Instant::now();
    let mut vm = Vm::new(&mut heap);
    let outcome = vm.run(&chunk, &mut heap);
    let elapsed = started.elapsed();
    // **A thrown completion is `Ok`**, and the first run of this experiment reported the shapes
    // that exhausted DR-0013's budget as the *fastest* of the ten — they stopped early. That is
    // the finding rather than a nuisance, so it is carried out rather than filtered: a row that
    // did not finish is a row whose time means nothing and whose footprint means everything.
    let finished = matches!(outcome, Ok(praxis::vm::Outcome::Value(_)));
    let grew = heap.footprint().saturating_sub(before);
    // …and then collect, with the chunk as the root. If the footprint does not fall, what the row
    // retained is not *garbage the collector missed* — it is arena slots DR-0010 declines to reuse,
    // and no collection schedule can give them back. That distinction is the whole verdict.
    vm.collect(&chunk, &mut heap);
    let after = heap.footprint().saturating_sub(before);
    Some((elapsed, grew, after, finished))
}

/// DR-0019's claim, checked directly: does a collection make the slots available again?
///
/// The table above cannot show this, and it took a run to see why. `Heap::footprint` counts
/// `environments.len()`, and freeing a slot does not shorten the `Vec` — it makes the slot
/// *reusable*. So a collection at the end of a loop moves nothing, and the only thing that tells
/// reuse from tombstones is whether a **second** loop has to grow the arena at all.
///
/// Which is what this measures: the same loop twice with a collection between. With tombstones
/// the second run costs what the first did. With DR-0019's free list it costs nothing.
fn reuse_check() {
    let source = format!(
        "var N = {ITERATIONS}; function f(a) {{ return a }}          var s = 0; for (var i = 0; i < N; i++) {{ s = f(i); }} s"
    );
    let mut heap = Heap::new();
    let Ok(script) = parse_script(&source) else {
        return;
    };
    let Ok(chunk) = compile_script(&script, &mut heap) else {
        return;
    };
    let start = heap.footprint();
    let mut vm = Vm::new(&mut heap);
    if vm.run(&chunk, &mut heap).is_err() {
        return;
    }
    let after_first = heap.footprint();
    vm.collect(&chunk, &mut heap);
    let swept = heap.footprint();
    if vm.run(&chunk, &mut heap).is_err() {
        return;
    }
    let after_second = heap.footprint();
    let first = after_first.saturating_sub(start);
    let second = after_second.saturating_sub(swept);
    println!(
        "
DR-0019 — {ITERATIONS} calls, collect, {ITERATIONS} calls again:"
    );
    println!("  first run grew the arena by  {first:>9} B");
    println!(
        "  the collection gave back     {:>9} B",
        after_first.saturating_sub(swept)
    );
    println!("  second run grew it by        {second:>9} B");
    println!(
        "  => slots are {}",
        match second * 4 < first {
            true => "REUSED",
            false => "tombstones — the second run paid again",
        }
    );
}
