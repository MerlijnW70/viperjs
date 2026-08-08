//! What one bytecode instruction costs, and what the interpreter's loop spends around it.
//!
//! ```text
//! cargo run -p viperjs-lab --release -- dispatch-cost
//! ```
//!
//! # The question
//!
//! `property-lookup` measured both of the property path's remaining levers away — eight shapes and
//! the prototype walk are memory locality, not algorithms — and left one term neither experiment
//! touched: **a property read is 176 ns where an empty loop is ~122**, so the lookup is a third of
//! it and the loop is the rest. `interpreter-speed` put the engine at "~20 ns per bytecode
//! instruction" against a good non-JIT interpreter's 2–5, and that figure came from dividing one
//! loop's time by an *estimated* instruction count.
//!
//! **So: what does an instruction actually cost, and is the cost the dispatch or the work?**
//!
//! # Self-calibrating, because the estimate is the thing under suspicion
//!
//! Nothing here guesses at an instruction count. The lab can compile, so each row is compiled and
//! `Chunk::code().len()` is read off it, and every figure is a **difference between two rows**: two
//! bodies that differ by `k` copies of one statement differ by exactly the instructions that
//! statement emits, and by exactly the time they take. `ns/instruction` is that quotient and never
//! a division by a number somebody counted by hand.
//!
//! That also makes the loop's own overhead fall out. Row `k = 0` is the loop and nothing else, so
//! the intercept is the per-iteration bookkeeping — the counter, the `stopped` flag, the chunk
//! deref, the bounds check — and the slope is what an instruction costs on top of it.
//!
//! # What this cannot see
//!
//! Which *part* of the dispatch is dear. A slope is one number and the loop does five things per
//! instruction; separating those needs the same treatment `property-lookup` used on its suspects,
//! which is to short-circuit one and re-run. This measures the thing to be explained, not the
//! explanation.

use std::time::{Duration, Instant};
use viperjs::compile::compile_script;
use viperjs::heap::Heap;
use viperjs::parser::parse_script;
use viperjs::value::Value;
use viperjs::vm::Vm;

/// How many times each body runs.
const ITERATIONS: usize = 1_000_000;

/// How many times each row is timed. The fastest is reported: a minimum is the run with the least
/// interference in it, where an average is the average amount of interference.
const ATTEMPTS: usize = 5;

/// One family of rows: a statement, repeated `k` times inside the loop for each `k` in [`COUNTS`].
struct Family {
    /// What the family is called, and what selects it on the command line.
    name: &'static str,
    /// The statement whose cost is being measured, repeated.
    statement: &'static str,
    /// What has to exist before the loop for the statement to compile and mean something.
    setup: &'static str,
    /// What the family is *for* — which instruction mix it prices.
    about: &'static str,
}

/// The repetition counts. The first is the loop with an empty body, which is the intercept.
const COUNTS: &[usize] = &[0, 1, 2, 4, 8];

const FAMILIES: &[Family] = &[
    Family {
        name: "add-local",
        statement: "s = s + 1;",
        setup: "var s = 0;",
        about: "a local read, a constant, an add and a local write",
    },
    Family {
        name: "copy-local",
        statement: "t = s;",
        setup: "var s = 1, t = 0;",
        about: "a local read and a local write — no arithmetic at all",
    },
    Family {
        name: "constant",
        statement: "t = 1;",
        setup: "var t = 0;",
        about: "a constant and a local write — the cheapest statement there is",
    },
    Family {
        name: "compare",
        statement: "t = s < 2;",
        setup: "var s = 1, t = false;",
        about: "a comparison rather than an add, on the same shape as `add-local`",
    },
    Family {
        name: "branch",
        statement: "if (s) { t = 1; }",
        setup: "var s = 1, t = 0;",
        about: "a jump that is taken, which is what a loop body is made of",
    },
    Family {
        name: "member",
        statement: "t = o.x;",
        setup: "var o = { x: 1 }, t = 0;",
        about: "a property read, so the two experiments meet on one axis",
    },
];

/// Run every family, or one of them.
pub fn run(argument: Option<&str>) -> std::process::ExitCode {
    let chosen: Vec<&Family> = match argument {
        Some(name) => FAMILIES.iter().filter(|f| f.name == name).collect(),
        None => FAMILIES.iter().collect(),
    };
    if chosen.is_empty() {
        eprintln!("viperjs-lab: no family named `{}`", argument.unwrap_or(""));
        eprintln!(
            "families: {}",
            FAMILIES
                .iter()
                .map(|f| f.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return std::process::ExitCode::FAILURE;
    }

    // Facts rather than measurements, and worth printing beside the numbers: every dispatch copies
    // an `Instruction` out of the code and most of them move a `Value` on or off the stack, so the
    // width of both is paid per instruction whatever the loop does.
    println!(
        "size_of::<Value>() = {}, size_of::<Instruction>() = {}",
        std::mem::size_of::<Value>(),
        std::mem::size_of::<viperjs::compile::Instruction>()
    );
    println!("{ITERATIONS} iterations per row, best of {ATTEMPTS}, --release\n");
    println!(
        "{:<12} {:>3} {:>7} {:>10} {:>12} {:>10}  what it is for",
        "family", "k", "instrs", "time", "per pass", "per instr"
    );
    println!("{}", "-".repeat(104));

    for family in chosen {
        let mut base: Option<(usize, Duration)> = None;
        for &count in COUNTS {
            let body = family.statement.repeat(count);
            let Some((instructions, elapsed)) = measure(family.setup, &body) else {
                println!("{:<12} {count:>3}  did not run", family.name);
                continue;
            };
            let passes = u32::try_from(ITERATIONS).unwrap_or(u32::MAX);
            // The slope, and never a division by a hand-counted instruction total: both the extra
            // instructions and the extra time are differences against the empty-bodied row.
            let per_instruction = match base {
                Some((first_instrs, first_time)) if instructions > first_instrs => {
                    let extra = u32::try_from(instructions - first_instrs).unwrap_or(1);
                    format!(
                        "{:?}",
                        elapsed.saturating_sub(first_time) / (extra.saturating_mul(passes))
                    )
                }
                // The `k = 0` row is the intercept: the loop with nothing in it, which is the
                // per-iteration bookkeeping every other row also pays.
                _ => "- (loop)".to_string(),
            };
            println!(
                "{:<12} {count:>3} {instructions:>7} {:>10.2?} {:>12.1?} {:>10}  {}",
                family.name,
                elapsed,
                elapsed / passes,
                per_instruction,
                if count == 0 { family.about } else { "" }
            );
            if base.is_none() {
                base = Some((instructions, elapsed));
            }
        }
        println!();
    }
    std::process::ExitCode::SUCCESS
}

/// Compile `setup` and a loop over `body`, count the instructions, and time the run.
///
/// Inside a function, so the variables are slots rather than properties of the global object —
/// `property-lookup` learned that the hard way, where a top-level yardstick read 645 ns against a
/// slotted one's 176 and buried everything under it.
fn measure(setup: &str, body: &str) -> Option<(usize, Duration)> {
    let source = format!(
        "(function () {{ {setup} \
         for (var i = 0; i < {ITERATIONS}; i++) {{ {body} }} return i }}())"
    );
    let mut instructions = 0;
    let mut best: Option<Duration> = None;
    for _ in 0..ATTEMPTS {
        let mut heap = Heap::new();
        let script = parse_script(&source).ok()?;
        let chunk = compile_script(&script, &mut heap).ok()?;
        // The loop is inside the function, so the instructions that matter are the *function's*
        // and not the script's — the script is four instructions that make a closure and call it.
        instructions = chunk.function(0).map_or(0, |body| body.code().len());
        let mut vm = Vm::new(&mut heap);
        let started = Instant::now();
        let outcome = vm.run(&std::rc::Rc::new(chunk), &mut heap).ok()?;
        let elapsed = started.elapsed();
        match outcome {
            viperjs::vm::Outcome::Value(_) => {}
            _ => return None,
        }
        best = Some(best.map_or(elapsed, |seen: Duration| seen.min(elapsed)));
    }
    best.map(|elapsed| (instructions, elapsed))
}
