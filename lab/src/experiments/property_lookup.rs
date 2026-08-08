//! Where a property read's time goes: the scan, the shapes, or the chain.
//!
//! ```text
//! cargo run -p viperjs-lab --release -- property-lookup
//! cargo run -p viperjs-lab --release -- property-lookup <row>
//! ```
//!
//! # The question
//!
//! `interpreter-speed` ranks *shapes and inline caches* top of the performance list, on one number:
//! a property read is 174 ns with one shape and 275 ns with eight. That is being quoted as "an
//! inline cache would buy 101 ns" — and **ViperJS has no inline cache, so eight shapes have nothing
//! to miss.** Whatever the row is measuring, it is not the thing it is being quoted for.
//!
//! Reading the benchmark says what else varies in it. It builds each object as `o["f" + j] = j` for
//! `j` in `0..=k` and then `o.x = k`, so `x` sits at position 1 in the first object and position 8
//! in the last; own properties are a `Vec<(PropertyKey, Property)>` walked in order, so the average
//! scan is 4.5 entries rather than 1. **So how much of the 101 ns is the scan, how much is
//! polymorphism proper, and how much is the prototype chain?**
//!
//! The answer decides the next slice. A lookup structure is small, local and risks no semantics;
//! hidden classes with inline caches are large and put §10.1.11's key order at risk — integer
//! indices ascending, then strings in insertion order, then symbols. Ranking those the wrong way
//! round costs a milestone.
//!
//! # Why four axes and not a benchmark
//!
//! Every row here differs from the row it is read against in **exactly one thing**, so the
//! interesting quantity is always a subtraction and never an absolute. A single "property read"
//! figure cannot tell a scan from a cache miss, which is how the 101 ns came to be quoted for the
//! wrong cause in the first place.
//!
//! - **A — scan length.** One shape, `x` at position 1, 2, 4, 8, 16, 32, 64. Linear in the position
//!   if the table is walked, flat if it is not.
//! - **B — polymorphism.** `k` distinct shapes at one site with `x` at the *same* position in every
//!   one. Flat is the prediction, and a rise is what an inline cache would buy.
//! - **C — prototype depth.** `x` found 0, 1, 2 and 4 levels up. A chain is the other linear thing
//!   in a lookup and no benchmark here has ever isolated it.
//! - **D — a miss.** `o.absent`, which is the whole table *and* the whole chain, and is what a
//!   defaulting read and `in` pay.
//!
//! # What this cannot see
//!
//! Anything below the instruction dispatch — there is no profiler here. Every row carries the same
//! interpreter overhead, which is why the differences are trustworthy and the absolutes are not:
//! read a column against its own axis's first row, never against another axis's.

use std::time::{Duration, Instant};
use viperjs::compile::compile_script;
use viperjs::heap::Heap;
use viperjs::parser::parse_script;
use viperjs::vm::Vm;

/// How many reads each row performs. Large enough to drown the setup, small enough that the whole
/// table finishes in a few seconds.
const ITERATIONS: usize = 1_000_000;

/// How many times each row is run. The fastest is reported — see [`measure`].
const ATTEMPTS: usize = 5;

/// One measured row: a name, the JavaScript that sets up and reads, and which subtraction it is in.
struct Row {
    /// What the row is called, and what selects it on the command line.
    name: &'static str,
    /// Which axis it belongs to — rows are only comparable within one.
    axis: &'static str,
    /// The setup, run once, leaving whatever the loop reads from in scope.
    setup: &'static str,
    /// The expression read `ITERATIONS` times. Summed, so nothing can be discarded as dead.
    read: &'static str,
    /// What this row is *for*.
    about: &'static str,
}

/// Build an object with `x` at 1-based position `at` and `filler` other properties before it.
///
/// Written out rather than generated in JavaScript, because `o["f" + j] = j` in a loop is what the
/// benchmark under suspicion does and it is the very thing that made `x`'s position vary.
const ROWS: &[Row] = &[
    // ------------------------------------------------------------------ A — scan length
    Row {
        name: "scan-1",
        axis: "A scan",
        setup: "var o = { x: 1 };",
        read: "o.x",
        about: "the yardstick: one property, found first",
    },
    Row {
        name: "scan-2",
        axis: "A scan",
        setup: "var o = { a: 0, x: 1 };",
        read: "o.x",
        about: "one entry walked past",
    },
    Row {
        name: "scan-4",
        axis: "A scan",
        setup: "var o = { a: 0, b: 0, c: 0, x: 1 };",
        read: "o.x",
        about: "three walked past",
    },
    Row {
        name: "scan-8",
        axis: "A scan",
        setup: "var o = { a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, x: 1 };",
        read: "o.x",
        about: "seven walked past — the position the 8-shape row averages",
    },
    Row {
        name: "scan-16",
        axis: "A scan",
        setup: "var o = {}; for (var j = 0; j < 15; j++) { o['p' + j] = j } o.x = 1;",
        read: "o.x",
        about: "fifteen walked past",
    },
    Row {
        name: "scan-32",
        axis: "A scan",
        setup: "var o = {}; for (var j = 0; j < 31; j++) { o['p' + j] = j } o.x = 1;",
        read: "o.x",
        about: "thirty-one walked past",
    },
    Row {
        name: "scan-64",
        axis: "A scan",
        setup: "var o = {}; for (var j = 0; j < 63; j++) { o['p' + j] = j } o.x = 1;",
        read: "o.x",
        about: "sixty-three walked past — is it linear?",
    },
    // ------------------------------------------------------------ B — polymorphism proper
    Row {
        name: "shapes-1",
        axis: "B shapes",
        setup: "var os = [{ x: 1, a: 0 }];",
        read: "os[i & 0].x",
        about: "one shape, `x` first — the yardstick, and it carries the array read",
    },
    Row {
        name: "shapes-2",
        axis: "B shapes",
        setup: "var os = [{ x: 1, a: 0 }, { x: 1, b: 0 }];",
        read: "os[i & 1].x",
        about: "two shapes, `x` still first in both",
    },
    Row {
        name: "shapes-4",
        axis: "B shapes",
        setup: "var os = [{ x: 1, a: 0 }, { x: 1, b: 0 }, { x: 1, c: 0 }, { x: 1, d: 0 }];",
        read: "os[i & 3].x",
        about: "four shapes",
    },
    Row {
        name: "shapes-8",
        axis: "B shapes",
        setup: "var os = [{ x: 1, a: 0 }, { x: 1, b: 0 }, { x: 1, c: 0 }, { x: 1, d: 0 }, \
                 { x: 1, e: 0 }, { x: 1, f: 0 }, { x: 1, g: 0 }, { x: 1, h: 0 }];",
        read: "os[i & 7].x",
        about: "eight shapes, `x` first in every one — this is the row that matters",
    },
    // The row that decides whether axis B is measuring shapes at all: eight objects that are all
    // the *same* shape. Whatever this costs over `shapes-1` is the price of touching eight objects
    // rather than one — cache lines, not hidden classes — and an inline cache cannot remove it.
    Row {
        name: "shapes-8-same",
        axis: "B shapes",
        setup: "var os = [{ x: 1, a: 0 }, { x: 1, a: 0 }, { x: 1, a: 0 }, { x: 1, a: 0 },                  { x: 1, a: 0 }, { x: 1, a: 0 }, { x: 1, a: 0 }, { x: 1, a: 0 }];",
        read: "os[i & 7].x",
        about: "eight objects, one shape — the control for the row above",
    },
    Row {
        name: "shapes-8-varying",
        axis: "B shapes",
        setup: "var os = []; for (var k = 0; k < 8; k++) { var o = {}; \
                 for (var j = 0; j <= k; j++) { o['f' + j] = j } o.x = k; os.push(o) }",
        read: "os[i & 7].x",
        about: "the original benchmark's own eight, where `x` moves — the row under suspicion",
    },
    // ------------------------------------------------------------- C — prototype depth
    Row {
        name: "proto-0",
        axis: "C proto",
        setup: "var o = { x: 1 };",
        read: "o.x",
        about: "found on the object itself",
    },
    Row {
        name: "proto-1",
        axis: "C proto",
        setup: "var o = Object.create({ x: 1 });",
        read: "o.x",
        about: "one level up",
    },
    Row {
        name: "proto-2",
        axis: "C proto",
        setup: "var o = Object.create(Object.create({ x: 1 }));",
        read: "o.x",
        about: "two levels up",
    },
    Row {
        name: "proto-4",
        axis: "C proto",
        setup: "var o = Object.create(Object.create(Object.create(Object.create({ x: 1 }))));",
        read: "o.x",
        about: "four levels up — is a level cheaper or dearer than a table entry?",
    },
    // -------------------------------------------------------------------- D — a miss
    // The yardstick carries the `===` and the `?:` too. The first version of this axis did not,
    // and its +114 ns was the comparison and the branch as much as the miss — a subtraction is
    // only clean when the two rows differ in the one thing being measured.
    Row {
        name: "miss-hit-8",
        axis: "D miss",
        setup: "var o = { a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, x: 1 };",
        read: "(o.x === undefined ? 1 : 0)",
        about: "the yardstick: a hit at the end of eight",
    },
    Row {
        name: "miss-absent-8",
        axis: "D miss",
        setup: "var o = { a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, x: 1 };",
        read: "(o.absent === undefined ? 1 : 0)",
        about: "the whole table and then the whole chain, for nothing",
    },
    Row {
        name: "miss-null-proto",
        axis: "D miss",
        setup: "var o = Object.create(null); o.x = 1;",
        read: "(o.absent === undefined ? 1 : 0)",
        about: "the same miss with no chain at all — how much of it is the walk",
    },
];

/// Run the table, or one row of it.
pub fn run(argument: Option<&str>) -> std::process::ExitCode {
    let chosen: Vec<&Row> = match argument {
        Some(name) => ROWS.iter().filter(|row| row.name == name).collect(),
        None => ROWS.iter().collect(),
    };
    if chosen.is_empty() {
        eprintln!("viperjs-lab: no row named `{}`", argument.unwrap_or(""));
        eprintln!(
            "rows: {}",
            ROWS.iter()
                .map(|row| row.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return std::process::ExitCode::FAILURE;
    }

    println!("{ITERATIONS} reads per row, --release");
    println!("a row is comparable only with the others on its own axis\n");
    println!(
        "{:<10} {:<18} {:>10} {:>10} {:>10}  what it is for",
        "axis", "row", "time", "per read", "over base"
    );
    println!("{}", "-".repeat(110));

    let mut axis = "";
    let mut base: Option<Duration> = None;
    for row in chosen {
        if row.axis != axis {
            axis = row.axis;
            base = None;
            println!();
        }
        let Some(elapsed) = measure(row.setup, row.read) else {
            println!("{:<10} {:<18} {:>10}", row.axis, row.name, "did not run");
            continue;
        };
        let count = u32::try_from(ITERATIONS).unwrap_or(u32::MAX);
        let per_read = elapsed / count;
        let over = match base {
            Some(first) => format!("+{:?}", per_read.saturating_sub(first / count)),
            None => "-".to_string(),
        };
        println!(
            "{:<10} {:<18} {:>10.2?} {:>10.1?} {:>10}  {}",
            row.axis, row.name, elapsed, per_read, over, row.about
        );
        if base.is_none() {
            base = Some(elapsed);
        }
    }
    println!();
    std::process::ExitCode::SUCCESS
}

/// Time `ITERATIONS` reads of `read`, after running `setup` once. The best of [`ATTEMPTS`] runs.
///
/// **Inside a function, and the first version of this was not.** At script top level `o`, `s` and
/// `i` are properties of the global object, so every one of them is itself a property lookup —
/// which put the yardstick at 645 ns against the ladder's 174 and buried every difference this
/// experiment exists to measure under the thing it is measuring. A benchmark that reads a variable
/// has to say which kind of variable, and a slot is the one a program's hot loop actually uses.
///
/// The best of several runs rather than one, because the quantities here are tens of nanoseconds
/// against a scheduler that moves by more than that: a minimum is the run with the least
/// interference in it, and an average is the average amount of interference.
fn measure(setup: &str, read: &str) -> Option<Duration> {
    let source = format!(
        "(function () {{ {setup} var s = 0;          for (var i = 0; i < {ITERATIONS}; i++) {{ s += {read} }} return s }}())"
    );
    let mut best: Option<Duration> = None;
    for _ in 0..ATTEMPTS {
        let mut heap = Heap::new();
        let script = parse_script(&source).ok()?;
        let chunk = compile_script(&script, &mut heap).ok()?;
        // Timed after compiling, so the number is the interpreter's and not the front end's.
        let mut vm = Vm::new(&mut heap);
        let started = Instant::now();
        let outcome = vm.run(&std::rc::Rc::new(chunk), &mut heap).ok()?;
        let elapsed = started.elapsed();
        // A row that threw measured a throw. Better to lose the row than to report the time it took
        // to fail as though it were the time it took to read a property.
        match outcome {
            viperjs::vm::Outcome::Value(_) => {}
            _ => return None,
        }
        best = Some(best.map_or(elapsed, |seen: Duration| seen.min(elapsed)));
    }
    best
}
