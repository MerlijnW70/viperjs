//! An agent loop: a sandbox that runs code it does not trust, and hands back what broke.
//!
//! ```text
//! cargo run --example agent_loop
//! ```
//!
//! The shape this demonstrates is a process — an AI agent, a rules engine, a user's own macro —
//! writing a script, running it here, reading the failure, and writing a better one. What makes
//! ViperJS suitable for it is not speed; it is that **every way a script can fail arrives as a
//! value the host chooses what to do with**, and that a script which will not stop is stopped by
//! the host rather than by luck.
//!
//! # What the sandbox is
//!
//! GOAL.md §3: this crate provides the language and the host provides everything else. A fresh
//! [`Engine`] has the ECMAScript intrinsics and **nothing else** — no `console`, no file system, no
//! network, no way to reach the host process at all. Whatever the script can touch is what the host
//! put there with `bind`, `bind_namespace` or `set_global`, so the attack surface is a list you can
//! read. Two bounds close the rest:
//!
//! - [`Engine::set_time_budget`] — DR-0022. A script that will not stop is stopped, and it **cannot
//!   catch that**: no `catch` runs, no `finally` runs, and the job queue is not drained. A budget a
//!   script could catch would not be a budget. Attempt 3 below is exactly this.
//! - [`Engine::set_heap_budget`] — DR-0013. A script that allocates without end is refused with a
//!   `RangeError` instead of taking the host's memory with it.
//!
//! # Why each attempt gets a fresh engine
//!
//! A script that was interrupted did not finish, and what it had already written to the global
//! object is still written. Reusing one engine across attempts would let a half-run script poison
//! the next one — and the failure would be a wrong *answer* rather than an error, which is the
//! worst kind. Building a realm is cheap; correctness here is not worth trading for it.
//!
//! # The agent is a stand-in, and that is deliberate
//!
//! There is no model in this file: `ATTEMPTS` is a fixed list of what one plausibly writes. Its job
//! is to show that each failure carries **enough to repair from** — a syntax error names what it
//! expected, a throw names what it read, an interrupt names the bound. Swap the list for a call to
//! whatever writes your scripts and the loop is unchanged.

use std::time::Duration;
use viperjs::api::{Engine, Error};

/// The task the agent was set, in the words it would have been given.
const TASK: &str = "answer with the total of every order's `price`";

/// What the task is worth, checked against the answer rather than against the absence of an error.
///
/// The oracle is the *answer*. A loop that stops at "it ran without throwing" accepts attempt 4
/// below, which is a program that runs perfectly and counts the orders instead of adding them —
/// and no engine can tell you that, because nothing went wrong.
const EXPECTED: &str = "4210";

/// The data the host puts in the sandbox, as source the host controls.
///
/// Source rather than a built value because it is the host's own text, so there is nothing here for
/// the script to have influenced. A real embedding reads this out of a database and builds it with
/// `set_global`; the difference does not matter to the loop.
const FIXTURE: &str = "var orders = [
    { sku: 'A-1', price: 1299 },
    { sku: 'B-7', price: 2450 },
    { sku: 'C-3', price: 461 }
];";

/// What the agent wrote, attempt by attempt — each one repaired from the feedback above it.
///
/// The four failures are one of each kind the loop has to tell apart, in the order they cost a host
/// the most trouble to distinguish: source that is not a program, a program that threw, a program
/// that does not end, and a program that is simply wrong.
const ATTEMPTS: &[&str] = &[
    // 1 — a missing `)`. Never reached the machine at all.
    "orders.reduce(function (total, order) { return total + order.price }, 0",
    // 2 — reads a `details` object that is not in the data.
    "orders.reduce(function (total, order) { return total + order.details.price }, 0)",
    // 3 — a hand-written walk whose counter is never advanced.
    "var total = 0, at = 0;
     while (at < orders.length) { total = total + orders[at].price }
     total",
    // 4 — runs, answers, and answers the wrong thing: this counts the orders.
    "orders.reduce(function (total) { return total + 1 }, 0)",
    // 5 — attempt 1 with the paren it was missing.
    "orders.reduce(function (total, order) { return total + order.price }, 0)",
];

fn main() {
    println!("task: {TASK}\nexpecting: {EXPECTED}\n");

    for (round, source) in ATTEMPTS.iter().enumerate() {
        println!("--- attempt {} ---", round + 1);
        println!("{}", indent(source));
        match attempt(source) {
            Ok(answer) => {
                println!("accepted, and it answers {answer}\n");
                return;
            }
            // Everything an agent would be handed. Printed rather than sent, because the point is
            // that it is a `String` at a boundary the host owns — nothing has crashed, nothing has
            // escaped, and the process is free to try again.
            Err(feedback) => println!("rejected: {feedback}\n"),
        }
    }

    // Falling out of the loop is a result too, and a host has to have a plan for it: an agent that
    // cannot repair its own code in five tries is not going to on the sixth.
    println!(
        "give up: {} attempts and none of them answered {EXPECTED}",
        ATTEMPTS.len()
    );
    std::process::exit(1);
}

/// Run one attempt in a sandbox of its own, and judge it — the answer, or what to tell its author.
///
/// `Err` is not a failure of this function: it is the feedback, which is the product. The only way
/// this can end the process is [`Error::Engine`], and that is deliberate — see below.
///
/// **Judging it is part of the same job, and separating the two is the mistake this exists to
/// avoid.** A loop whose `Ok` means "the engine did not object" accepts attempt 4, which runs
/// perfectly and counts the orders. The engine has nothing to say about that and never will: it is
/// a correct program answering a question nobody asked.
fn attempt(source: &str) -> Result<String, String> {
    let mut engine = sandbox();

    // The fixture is the host's own source and is not the agent's to get wrong, so a failure here
    // is a bug in this file rather than something to report as feedback.
    engine.eval(FIXTURE).expect("the host's own fixture runs");

    let value = engine.eval(source).map_err(|error| feedback(&error))?;

    // A value is not yet an answer: `String(value)` is §7.1.17, so an object's own `toString` runs
    // here and is free to throw. That throw is the script's too, and reads back as feedback like
    // any other.
    let answer = engine.text(value).map_err(|error| feedback(&error))?;

    if answer != EXPECTED {
        return Err(format!(
            "it ran, finished, and answered {answer} where the total is {EXPECTED}. Nothing went \
             wrong — this is a working program that computes something else, so there is no error \
             message to go on. Read the task again rather than the code."
        ));
    }
    Ok(answer)
}

/// A fresh realm with the host's bounds on it and nothing of the host's inside it.
///
/// The budgets are the whole of what makes this safe to point at code nobody has read. They are
/// small on purpose: this task is arithmetic over three rows, and a bound chosen from what the work
/// needs is a bound that catches a runaway on its first pass rather than its thousandth.
fn sandbox() -> Engine {
    let mut engine = Engine::new();
    engine.set_time_budget(Some(Duration::from_millis(50)));
    engine.set_heap_budget(8 * 1024 * 1024);
    engine
}

/// What to hand back to whoever wrote the script — one sentence per kind of failure.
///
/// **The four cases are why [`Error`] is an enum rather than a message**, and the difference is not
/// cosmetic: each one calls for a different repair, and an agent told only "it failed" will make
/// the wrong one. Source that will not compile is a typo; a throw is a wrong assumption about the
/// data; an interrupt is a loop that does not advance, and re-running it unchanged will interrupt
/// again.
fn feedback(error: &Error) -> String {
    match error {
        Error::Syntax(said) => {
            format!(
                "it is not a program yet — the parser said: {said}. Fix the source itself; \
                     nothing ran, so there is nothing here about the data."
            )
        }
        Error::Thrown(said) => {
            format!(
                "it ran and threw: {said}. Something it assumed about the data is not so; \
                     check what is actually there before reading through it."
            )
        }
        Error::Interrupted => {
            "it did not finish inside the time it was given, and was stopped. That is a loop \
             whose condition never becomes false — find what should advance each pass and does \
             not. Re-running it unchanged will be stopped again."
                .to_string()
        }
        Error::Collected => {
            "it answered with a value the sandbox has since freed. Nothing the script did is \
             wrong; the host read it too late."
                .to_string()
        }
        // DR-0002: no input can cause one of these, so this is not the script's fault and must not
        // be fed back as though it were. An agent told "your code did this" would spend the next
        // attempt rewriting something that was already right. It is a bug report about *this
        // engine*, and the honest thing is to say so and stop.
        Error::Engine(fault) => {
            panic!(
                "the engine reached a state its own types rule out — please report this: {fault:?}"
            )
        }
    }
}

/// The script as it reads in a transcript, so the output shows what was run rather than describing
/// it.
fn indent(source: &str) -> String {
    source
        .lines()
        .map(|line| format!("    {}", line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}
