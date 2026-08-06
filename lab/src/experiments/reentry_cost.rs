//! How much stack one *re-entry* costs, and what `MAX_REENTRY_DEPTH` could afford.
//!
//! ```text
//! cargo run -p viperjs-lab -- reentry-cost
//! ```
//!
//! # The question
//!
//! `MAX_REENTRY_DEPTH` is 32. A re-entry is a native calling back into the interpreter, so every
//! `map`, `forEach`, `sort`, `reduce` and `then` handler is one, and a recursive walk written the
//! ordinary way —
//!
//! ```js
//! function walk(node) { return node.children.map(walk); }
//! ```
//!
//! — stops at depth 33. `ajv` hits it while compiling a schema. The cap's own comment used to say
//! this was "nothing anyone will meet"; it is, and the question is how much higher it can go.
//!
//! # Why it forks, and why one mebibyte
//!
//! A stack overflow aborts. It cannot be caught, so a bisection cannot run inside one process:
//! each candidate depth is a fresh child of this binary and the parent reads the exit status.
//! One mebibyte is the smallest thread stack in common use and is what
//! `compiling_at_the_cap_fits_in_the_stack_it_claims_to_need` holds the parser to.
//!
//! # It needs the cap out of the way
//!
//! With the constant at its real value this measures the constant. Raise it first:
//!
//! ```text
//! sed -i 's/MAX_REENTRY_DEPTH: usize = 32/MAX_REENTRY_DEPTH: usize = 1_000_000/' ../src/vm/coerce.rs
//! cargo run -p viperjs-lab -- reentry-cost
//! git checkout ../src/vm/coerce.rs
//! ```
//!
//! # Why three shapes
//!
//! The engine cannot be instrumented from out here, so a per-arm cost is reached by subtraction.
//! Each shape re-enters through a different native, and the differences say what the *native's*
//! own frames cost on top of the interpreter's:
//!
//! - `map` is the ordinary case and the one real code meets.
//! - `valueOf` is what the cap's comment thought it was about, and goes through `ToPrimitive`.
//! - `sort` carries a comparator across a `Vec` the native owns, which is the fattest of the
//!   three and is why it is here.

use std::process::{Command, ExitCode};

/// The stack a trial gets — the smallest in common use, which is what the engine is held to.
const STACK: usize = 1024 * 1024;

/// One way of re-entering the interpreter from a native.
struct Shape {
    /// What to pass on the command line.
    name: &'static str,
    /// A program whose recursion goes through that native `depth` times.
    build: fn(usize) -> String,
}

/// The three, in the order the report prints them.
const SHAPES: &[Shape] = &[
    Shape {
        name: "map",
        build: |depth| {
            format!(
                "function f(n) {{ return n === 0 ? 0 : [0].map(function () {{ return f(n - 1); }})[0]; }} f({depth});"
            )
        },
    },
    Shape {
        name: "valueOf",
        build: |depth| {
            format!(
                "function f(n) {{ if (n === 0) return 0; \
                 return 1 * {{ valueOf: function () {{ return f(n - 1); }} }}; }} f({depth});"
            )
        },
    },
    Shape {
        name: "sort",
        build: |depth| {
            format!(
                "function f(n) {{ if (n === 0) return 0; var out = 0; \
                 [1, 2].sort(function () {{ out = f(n - 1); return 0; }}); return out; }} f({depth});"
            )
        },
    },
];

/// Run every shape, or one named on the command line, or a single trial as a child.
pub fn run(argument: Option<&str>, rest: &[String]) -> ExitCode {
    if argument == Some("--trial") {
        let name = rest.first().map(String::as_str).unwrap_or("map");
        let depth = rest.get(1).and_then(|d| d.parse().ok()).unwrap_or(1);
        return trial(name, depth);
    }
    println!("stack per trial: {} KiB\n", STACK / 1024);
    println!("{:<10} {:>8} {:>14}", "shape", "deepest", "bytes/level");
    for shape in SHAPES {
        if let Some(name) = argument
            && name != shape.name
        {
            continue;
        }
        match cliff(shape.name) {
            Some(deepest) if deepest > 0 => {
                let per = STACK / deepest;
                println!("{:<10} {deepest:>8} {per:>14}", shape.name);
            }
            Some(_) => println!("{:<10} {:>8}", shape.name, "0"),
            None => println!(
                "{:<10} {:>8}  the cap refused it — raise MAX_REENTRY_DEPTH first",
                shape.name, "-"
            ),
        }
    }
    ExitCode::SUCCESS
}

/// The deepest re-entry chain of `name` that survives, or `None` when the cap refused it first.
fn cliff(name: &str) -> Option<usize> {
    let mut low = 1;
    let mut high = 2;
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
    /// `MAX_REENTRY_DEPTH` refused it, so nothing about the stack was learnt.
    Refused,
}

/// One trial, in a child of this same binary.
fn run_trial(name: &str, depth: usize) -> Trial {
    let status = Command::new(std::env::current_exe().expect("this binary has a path"))
        .args(["reentry-cost", "--trial", name, &depth.to_string()])
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

/// The child: run one shape at one depth in exactly `STACK` bytes.
fn trial(name: &str, depth: usize) -> ExitCode {
    let shape = SHAPES
        .iter()
        .find(|shape| shape.name == name)
        .expect("a known shape");
    let source = (shape.build)(depth);
    let worker = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let mut engine = viperjs::api::Engine::new();
            // A refusal walks only as many frames as the cap allows, so it measures the cap and
            // not the stack. Saying which happened is the whole reason this returns a code.
            match engine.eval(&source) {
                Err(viperjs::api::Error::Thrown(said)) => said.contains("too much recursion"),
                _ => false,
            }
        })
        .expect("a thread");
    match worker.join() {
        Ok(true) => ExitCode::from(2),
        Ok(false) => ExitCode::SUCCESS,
        // A panic inside the thread is not an overflow — an overflow kills the process outright,
        // so this only sees the ordinary kind and reports it as a failed trial.
        Err(_) => ExitCode::from(3),
    }
}
