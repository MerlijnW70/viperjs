//! How much stack a nesting level costs, per shape, and where the array literal's goes.
//!
//! ```text
//! cargo run -p praxis-lab -- nesting-cost              # every shape
//! cargo run -p praxis-lab -- nesting-cost <shape>      # one shape
//! ```
//!
//! # Why it forks
//!
//! A stack overflow aborts the process. It cannot be caught, so a bisection cannot run in one:
//! every candidate depth is a fresh child of this same binary, and the parent reads the exit
//! status. That is slow and it is the only honest way to find a cliff.
//!
//! # Why the shapes are what they are
//!
//! The engine cannot be instrumented from out here — `lab` depends on `praxis` and not the other
//! way round — so the cost of an individual function is reached by *subtraction*. Each shape is
//! chosen to walk a known segment of the parser's call graph, and the differences between them
//! are what attribute the bytes:
//!
//! - `!!!!1` recurses inside `parse_unary` alone.
//! - `((((1))))` goes through the assignment level and the cover-group reader, and never touches
//!   the operand ladder: a `(` is intercepted before it.
//! - `[[[[]]]]` is the same assignment level *plus* the whole ladder down to `parse_primary`,
//!   plus the literal's own two frames.
//!
//! So `[` minus `(` is what the ladder and the literal cost together, and `!` is the yardstick
//! for one ordinary frame.
//!
//! # It needs the cap out of the way
//!
//! `MAX_NESTING_DEPTH` refuses a deep shape long before the stack would, and a refusal walks only
//! as many frames as the cap allows — so with the constant at its real value this measures the
//! constant and nothing else. Set it to something enormous first:
//!
//! ```text
//! sed -i 's/MAX_NESTING_DEPTH: u32 = 64/MAX_NESTING_DEPTH: u32 = 1_000_000/' ../src/parser/mod.rs
//! cargo run -p praxis-lab -- nesting-cost
//! git checkout ../src/parser/mod.rs
//! ```
//!
//! A trial that is refused rather than parsed says so, and the report calls the shape
//! `cap-limited` instead of printing a number that means nothing.

use std::process::{Command, ExitCode};

/// One measurable shape: a name and how to build it at `n` levels.
struct Shape {
    name: &'static str,
    /// What part of the call graph it walks, for the report.
    path: &'static str,
    build: fn(usize) -> String,
}

const SHAPES: &[Shape] = &[
    Shape {
        name: "unary",
        path: "parse_unary, recursing into itself",
        build: |n| format!("{}1", "!".repeat(n)),
    },
    Shape {
        name: "block",
        path: "the statement path only",
        build: |n| format!("{}{}", "{".repeat(n), "}".repeat(n)),
    },
    Shape {
        name: "group",
        path: "assignment -> arrow_or_group -> cover_group",
        build: |n| format!("{}1{}", "(".repeat(n), ")".repeat(n)),
    },
    Shape {
        name: "array",
        path: "assignment -> ladder -> primary -> array_literal -> array_elements",
        build: |n| format!("{}{};", "[".repeat(n), "]".repeat(n)),
    },
    Shape {
        name: "array-pattern",
        path: "the same, and the refinement walk on top",
        build: |n| format!("{}a{} = b;", "[".repeat(n), "]".repeat(n)),
    },
    Shape {
        name: "object",
        path: "the same ladder, into object_literal instead",
        build: |n| format!("{}1{};", "({a: ".repeat(n), "})".repeat(n)),
    },
    Shape {
        name: "computed-member",
        path: "member -> computed_member_after -> expression -> ladder",
        build: |n| format!("a{}{};", "[".repeat(n), "0]".repeat(n)),
    },
    Shape {
        name: "conditional",
        path: "assignment -> conditional_tail -> assignment",
        build: |n| format!("a ? {}1{} : c;", "a ? ".repeat(n), " : c".repeat(n)),
    },
];

/// The stack every trial runs in. One mebibyte is the smallest in common use, and is what the
/// engine's own stack test asserts against.
const STACK: usize = 1024 * 1024;

/// Run the experiment, or one trial of it when the parent asks for one.
pub fn run(argument: Option<&str>) -> ExitCode {
    // The child form: `nesting-cost --trial <shape> <depth>`. Exits 0 if it survived.
    let arguments: Vec<String> = std::env::args().collect();
    if let Some(position) = arguments.iter().position(|value| value == "--trial") {
        let name = &arguments[position + 1];
        let depth: usize = arguments[position + 2].parse().expect("a depth");
        return trial(name, depth);
    }

    let chosen: Vec<&Shape> = match argument {
        Some(name) => SHAPES.iter().filter(|shape| shape.name == name).collect(),
        None => SHAPES.iter().collect(),
    };
    if chosen.is_empty() {
        eprintln!("no such shape; try one of:");
        for shape in SHAPES {
            eprintln!("  {}", shape.name);
        }
        return ExitCode::FAILURE;
    }

    println!("{:<17} {:>7}  {:>10}  path", "shape", "levels", "per level");
    for shape in chosen {
        match cliff(shape.name) {
            Some(levels) => println!(
                "{:<17} {levels:>7}  {:>7.1} KiB  {}",
                shape.name,
                STACK as f64 / levels as f64 / 1024.0,
                shape.path
            ),
            None => println!(
                "{:<17} {:>7}  {:>11}  {}",
                shape.name, "—", "cap-limited", shape.path
            ),
        }
    }
    ExitCode::SUCCESS
}

/// The deepest nesting of `name` that survives, or `None` when the cap refused it first.
fn cliff(name: &str) -> Option<usize> {
    let mut low = 1;
    let mut high = 2;
    // Upward first: a cheap shape can be hundreds of levels deep and a fixed bracket would either
    // waste trials or miss the cliff entirely.
    loop {
        match run_trial(name, high) {
            Trial::Refused => return None,
            Trial::Survived => {
                low = high;
                high *= 2;
            }
            Trial::Overflowed => break,
        }
    }
    while high - low > 1 {
        let middle = (low + high) / 2;
        match run_trial(name, middle) {
            Trial::Refused => return None,
            Trial::Survived => low = middle,
            Trial::Overflowed => high = middle,
        }
    }
    Some(low)
}

/// What one trial did.
enum Trial {
    /// The stack held.
    Survived,
    /// The stack did not.
    Overflowed,
    /// `MAX_NESTING_DEPTH` refused the shape, so nothing about the stack was learnt.
    Refused,
}

/// One trial, in a child of this same binary.
fn run_trial(name: &str, depth: usize) -> Trial {
    let status = Command::new(std::env::current_exe().expect("this binary has a path"))
        .args(["nesting-cost", "--trial", name, &depth.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("a child process");
    match status.code() {
        Some(0) => Trial::Survived,
        Some(2) => Trial::Refused,
        _ => Trial::Overflowed,
    }
}

/// The child: parse one shape at one depth in exactly `STACK` bytes.
fn trial(name: &str, depth: usize) -> ExitCode {
    let shape = SHAPES
        .iter()
        .find(|shape| shape.name == name)
        .expect("a known shape");
    let source = (shape.build)(depth);
    let worker = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            // A refusal walks only as many frames as the cap allows, so it measures the cap and
            // not the stack. Saying which happened is the whole reason this returns a code rather
            // than a boolean.
            matches!(
                praxis::parser::parse_script(&source),
                Err(error) if error.kind == praxis::parser::ParseErrorKind::TooDeeplyNested
            )
        })
        .expect("a thread");
    match worker.join() {
        Ok(true) => ExitCode::from(2),
        Ok(false) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
