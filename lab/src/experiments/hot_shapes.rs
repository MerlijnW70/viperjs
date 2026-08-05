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
//! Footprint is counted the same way `Heap::footprint` counts it — each arena's `len()`, which is a
//! **high-water mark**: DR-0019 hands a swept slot out again, and that stops the arena growing
//! without shortening the `Vec`. So a row's growth is what that shape made the heap *reach*, and
//! the `after gc` column beside it cannot fall even when every slot in it has been freed for reuse.
//! That is a property of the measure and not of the collector — see `reuse_check`, which is the
//! part of this experiment that can tell the two apart, and which exists because reading this table
//! as though the column meant "unreclaimable" is exactly the mistake it once produced.
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
        "{:<24} {:>10} {:>16} {:>10} {:>9}  what it is for",
        "shape", "time", "per pass", "leak/pass", "after gc"
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
    ceiling_check();
    threshold_sweep();
    live_set_cost();
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
    // …and then collect, with the chunk as the root. This column does **not** say what a collection
    // can reclaim, though it was read that way once and the reading became this notebook's headline.
    // `footprint` is a high-water mark, so a freed slot goes on being counted whether or not the
    // next allocation takes it — and the number therefore looks identical for an arena that reuses
    // perfectly and one that never does. `reuse_check` is what distinguishes them; this column's
    // honest use is only "what did this shape make the heap reach".
    vm.collect(&chunk, &mut heap);
    let after = heap.footprint().saturating_sub(before);
    Some((elapsed, grew, after, finished))
}

/// DR-0019's claim, checked directly: does a collection make the slots available again?
///
/// The table above cannot show this, and it took a run to see why. `Heap::footprint` counts each
/// arena's `len()`, and freeing a slot does not shorten the `Vec` — it makes the slot *reusable*.
/// So a collection at the end of a loop moves nothing, and the only thing that tells reuse from
/// tombstones is whether a **second** loop has to grow the arena at all.
///
/// Which is what this measures: the same loop twice with a collection between. With tombstones the
/// second run costs what the first did. With DR-0019's free list it costs nothing.
///
/// # Why there is a row per arena
///
/// DR-0019 landed for environments first, and this notebook's entry recorded that the objects,
/// Strings, Symbols and BigInts were "still tombstoned … and are next". They are not: all five are
/// the one `Arena<T>` now. That is readable in `src/heap/mod.rs` in a second, and a reading is not
/// what this file is for — a shape per arena turns it into a number, and the two rows that
/// allocate no *new* arena keep the check honest about what it is attributing.
fn reuse_check() {
    // One shape per arena that a loop can fill: a call makes an environment, a literal an object,
    // a concatenation a String, an arithmetic a BigInt. Symbols are left out — `Symbol()` in a loop
    // is not a shape any real program has, and the registry makes `Symbol.for` a strong root.
    let shapes: &[(&str, &str)] = &[
        (
            "environments (a call)",
            "function f(a) { return a } var s = 0; for (var i = 0; i < N; i++) { s = f(i); } s",
        ),
        (
            "objects (a literal)",
            "var s = 0; for (var i = 0; i < N; i++) { s = ({ a: i }).a; } s",
        ),
        (
            "strings (a concatenation)",
            "var s = ''; for (var i = 0; i < N; i++) { s = 'x' + i; } s.length",
        ),
        (
            "bigints (an addition)",
            "var s = 0n; for (var i = 0; i < N; i++) { s = 1n + BigInt(i); } 0",
        ),
    ];
    println!("\nDR-0019 — {ITERATIONS} passes, collect, {ITERATIONS} passes again, collect:");
    println!(
        "  {:<26} {:>13} {:>13}  verdict",
        "arena", "run 1 kept", "run 2 kept"
    );
    for (name, body) in shapes {
        let Some((first, second)) = reuse_of(body) else {
            println!("  {name:<26} did not run");
            continue;
        };
        // Four to one rather than "the second is zero": a collection leaves the chunk's own
        // constants and whatever the loop's last pass is still reachable from, so a reusing arena
        // keeps a little. The measured gap is three orders of magnitude and the threshold sits
        // nowhere near either side of it.
        let verdict = match second * 4 < first {
            true => "REUSED",
            false => "tombstoned — paid again",
        };
        println!("  {name:<26} {first:>11} B {second:>11} B  {verdict}");
    }
}

/// What each of two identical runs *permanently* added to the heap, each measured after a
/// collection — `None` if the shape could not be run at all.
///
/// # Why both ends are measured after a collect, which a first version of this got wrong
///
/// The obvious measurement — grow, collect, grow again, compare the two growths — answers wrongly
/// for exactly one arena, and reported Strings as tombstoned when they are not. `Heap::footprint`
/// is not a slot count: it is slots **plus** `string_units`, and a String's units are real memory
/// that the sweep genuinely gives back and the next allocation genuinely pays for again. So the
/// second run's growth for that shape is its units being bought a second time, which is correct
/// behaviour and says nothing at all about the slot.
///
/// Measuring the high-water mark *after* a collection removes the component that legitimately comes
/// and goes, and leaves the one the question is about. If slots are reused the arena never had to
/// grow, so the second number is the first. If they are tombstoned the second is twice it.
fn reuse_of(body: &str) -> Option<(usize, usize)> {
    let source = format!("var N = {ITERATIONS}; {body}");
    let mut heap = Heap::new();
    let script = parse_script(&source).ok()?;
    let chunk = compile_script(&script, &mut heap).ok()?;
    let start = heap.footprint();
    let mut vm = Vm::new(&mut heap);
    vm.run(&chunk, &mut heap).ok()?;
    vm.collect(&chunk, &mut heap);
    let after_first = heap.footprint();
    vm.run(&chunk, &mut heap).ok()?;
    vm.collect(&chunk, &mut heap);
    let after_second = heap.footprint();
    Some((
        after_first.saturating_sub(start),
        after_second.saturating_sub(after_first),
    ))
}

/// The ceiling this notebook has predicted twice, and what a schedule does to it.
///
/// `hot-shapes` measured 74 B of arena per call and DR-0019's note turned that into "about 900,000
/// calls before any program dies". This runs the loop it describes, at four sizes, with the loop
/// allowed to collect and with it not — which is the difference between an engine that can run
/// ordinary code of ordinary size and one that cannot.
fn ceiling_check() {
    println!(
        "
the call ceiling, with a schedule and without:"
    );
    println!(
        "  {:<12} {:>26} {:>26}",
        "calls", "no schedule", "collect every 1 MiB grown"
    );
    for calls in [100_000usize, 800_000, 1_000_000, 5_000_000] {
        let mut row = Vec::new();
        for growth in [None, Some(1 << 20)] {
            let source = format!(
                "function f(a) {{ return a + 1 }} var s = 0;                  for (var i = 0; i < {calls}; i++) {{ s = f(s) }} s"
            );
            let mut heap = Heap::new();
            let Ok(script) = parse_script(&source) else {
                row.push("did not parse".to_string());
                continue;
            };
            let Ok(chunk) = compile_script(&script, &mut heap) else {
                row.push("did not compile".to_string());
                continue;
            };
            let mut vm = Vm::new(&mut heap);
            vm.set_collection_growth(growth);
            let started = Instant::now();
            let outcome = vm.run(&chunk, &mut heap);
            let elapsed = started.elapsed();
            row.push(match outcome {
                Ok(praxis::vm::Outcome::Value(_)) => {
                    format!("ok {elapsed:?}, {} KiB", heap.footprint() / 1024)
                }
                Ok(praxis::vm::Outcome::Thrown(_)) => format!("THREW after {elapsed:?}"),
                _ => "stopped".to_string(),
            });
        }
        println!("  {:<12} {:>26} {:>26}", calls, row[0], row[1]);
    }
}

/// What the threshold is worth choosing, on the loop the ceiling was measured with.
fn threshold_sweep() {
    println!(
        "
threshold sweep — 2,000,000 calls:"
    );
    println!(
        "  {:<20} {:>14} {:>14}",
        "collect after", "time", "footprint"
    );
    for growth in [
        None,
        Some(256 << 10),
        Some(1 << 20),
        Some(4 << 20),
        Some(16 << 20),
    ] {
        let source = "function f(a) { return a + 1 } var s = 0;                       for (var i = 0; i < 2000000; i++) { s = f(s) } s";
        let mut heap = Heap::new();
        let Ok(script) = parse_script(source) else {
            continue;
        };
        let Ok(chunk) = compile_script(&script, &mut heap) else {
            continue;
        };
        let mut vm = Vm::new(&mut heap);
        vm.set_collection_growth(growth);
        let started = Instant::now();
        let outcome = vm.run(&chunk, &mut heap);
        let elapsed = started.elapsed();
        let label = match growth {
            None => "never".to_string(),
            Some(bytes) => format!("{} KiB grown", bytes / 1024),
        };
        let result = match outcome {
            Ok(praxis::vm::Outcome::Value(_)) => format!("{elapsed:?}"),
            _ => format!("THREW after {elapsed:?}"),
        };
        println!(
            "  {:<20} {:>14} {:>12} KiB",
            label,
            result,
            heap.footprint() / 1024
        );
    }
}

/// The case a fixed threshold could be pathological on: a **large live set**.
///
/// Every collection walks what is reachable, so a program holding a great deal and allocating
/// steadily pays that walk once per threshold of growth. This builds a live set first and then runs
/// the same call loop over it, which is the shape where a fixed threshold amplifies.
fn live_set_cost() {
    println!(
        "
walk cost against a large live set — 500,000 calls over a held array:"
    );
    println!(
        "  {:<20} {:>14} {:>14}",
        "collect after", "time", "footprint"
    );
    for growth in [None, Some(1 << 20), Some(4 << 20), Some(16 << 20)] {
        let source = "var held = []; for (var i = 0; i < 150000; i++) { held.push({ n: i }) }                       function f(a) { return a + 1 } var s = 0;                       for (var j = 0; j < 500000; j++) { s = f(s) } s + held.length";
        let mut heap = Heap::new();
        let Ok(script) = parse_script(source) else {
            continue;
        };
        let Ok(chunk) = compile_script(&script, &mut heap) else {
            continue;
        };
        let mut vm = Vm::new(&mut heap);
        vm.set_collection_growth(growth);
        let started = Instant::now();
        let outcome = vm.run(&chunk, &mut heap);
        let elapsed = started.elapsed();
        let label = match growth {
            None => "never".to_string(),
            Some(bytes) => format!("{} KiB grown", bytes / 1024),
        };
        let result = match outcome {
            Ok(praxis::vm::Outcome::Value(_)) => format!("{elapsed:?}"),
            _ => format!("THREW after {elapsed:?}"),
        };
        println!(
            "  {:<20} {:>14} {:>12} KiB",
            label,
            result,
            heap.footprint() / 1024
        );
    }
}
