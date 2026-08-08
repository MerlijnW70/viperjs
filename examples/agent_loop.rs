//! An agent loop: a sandbox that runs code it does not trust, and patches it from what broke.
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
//! Nothing here is scripted. One draft goes in, and each patch is computed from the failure the
//! engine reported for the round before it — so the transcript is what the loop did, not a list
//! somebody wrote out.
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
//!   script could catch would not be a budget. Round 3 below is exactly this.
//! - [`Engine::set_heap_budget`] — DR-0013. A script that allocates without end is refused with a
//!   `RangeError` instead of taking the host's memory with it.
//!
//! # Why every run gets a fresh engine
//!
//! A script that was interrupted did not finish, and what it had already written to the global
//! object is still written. Reusing one engine across rounds would let a half-run script poison the
//! next one — and that failure would be a wrong *answer* rather than an error, which is the worst
//! kind. Building a realm is cheap; correctness is not worth trading for it. The same reasoning
//! makes every probe below its own engine too.
//!
//! # The three rules are a stand-in, and the loop is not
//!
//! [`repair`] is three textual rules where a real embedding calls a model. They are crude on
//! purpose and their point is what they are crude *about*: each one is derived from the failure,
//! and each checks its own work against the engine rather than trusting itself.
//!
//! - A syntax error is repaired by **asking the engine whether the repair parses**. The parser
//!   names what it wanted, but [`Error::Syntax`] carries no span — the sentence says `expected )`
//!   and not *where* — so appending it is a guess until something confirms it. A scratch engine is
//!   what confirms it.
//! - A `TypeError` is repaired by **asking the data**, not by guessing at the code: the rule finds
//!   the links the script reads *through*, and asks the sandbox which of them no order actually
//!   has. That is the engine acting as the oracle for its own failure.
//! - An interrupt is repaired by the one thing that is true of every runaway `while`: the condition
//!   names something the body never writes.
//!
//! And a fourth failure has no rule at all, deliberately — see [`Rejection::Wrong`]. The second
//! draft below exists to reach it.

use std::time::Duration;
use viperjs::api::{Engine, Error};

/// What the task is worth, checked against the answer rather than against the absence of an error.
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

/// How many rounds before the loop admits it is not converging.
///
/// A bound and not a guess at how many are needed: a rule that patches without helping would
/// otherwise run for ever. [`solve`] also stops the moment a patch fails to change anything, which
/// catches the same thing one round sooner.
const MAX_ROUNDS: usize = 8;

/// Two first drafts of the same task: "answer with the total of every order's `price`".
///
/// The first carries one typo and two real bugs and is repaired to `4210`. **The second is
/// deliberately unrepairable** — it runs, finishes, and answers the wrong thing, which is the one
/// failure no engine can report and no rule here can act on.
const DRAFTS: &[(&str, &str)] = &[
    (
        "a hand-written walk",
        "(function () {
  var total = 0, at = 0;
  while (at < orders.length) { total = total + orders[at].details.price }
  return total;
}()",
    ),
    (
        "a fold that folds the wrong thing",
        "orders.reduce(function (total) { return total + 1 }, 0)",
    ),
];

fn main() {
    println!("task: answer with the total of every order's `price`\nexpecting: {EXPECTED}\n");

    let solved: Vec<bool> = DRAFTS
        .iter()
        .map(|(label, draft)| solve(label, draft))
        .collect();

    // The second draft is *meant* to defeat the loop, so "did it converge" is not the check — "did
    // it converge on the one it can and admit the one it cannot" is.
    assert_eq!(
        solved,
        [true, false],
        "the loop's outcome is not the one this example is written to show"
    );
}

/// Run one draft to acceptance, patching it from each failure — `true` if it converged.
fn solve(label: &str, draft: &str) -> bool {
    println!("=== draft: {label} ===");
    let mut source = draft.to_string();

    for round in 1..=MAX_ROUNDS {
        println!("--- round {round} ---\n{}", indent(&source));

        let rejection = match attempt(&source) {
            Ok(answer) => {
                println!("accepted, and it answers {answer}\n");
                return true;
            }
            Err(rejection) => rejection,
        };
        // The feedback is printed rather than sent, because the point is that it is an ordinary
        // value at a boundary the host owns: nothing crashed, nothing escaped, and the process is
        // free to try again.
        println!("rejected: {}", rejection.feedback());

        let Some(patch) = repair(&source, &rejection) else {
            println!("no rule applies — stopping.\n");
            return false;
        };
        // A patch that changes nothing would make the next round identical to this one, which is
        // the shape a bounded loop hides rather than reports.
        if patch.source == source {
            println!("the patch changed nothing — stopping.\n");
            return false;
        }
        println!("patch: {}\n", patch.what);
        source = patch.source;
    }

    println!("give up: still not {EXPECTED} after {MAX_ROUNDS} rounds\n");
    false
}

/// Why an attempt was not accepted, in the terms [`repair`] acts on.
///
/// **This is why [`Error`] is an enum rather than a message.** Each case calls for a different
/// repair, and a host that collapses them into "it failed" hands back the wrong instruction: source
/// that will not compile is a typo, a throw is a wrong assumption about the data, and an interrupt
/// is a loop that does not advance — re-running which unchanged will be stopped again.
enum Rejection {
    /// It never reached the machine. Carries the parser's own sentence.
    Unparsed(String),
    /// It ran and threw, with the value spelled as `String(e)` spells it.
    Threw(String),
    /// It did not finish inside the time budget and was stopped — DR-0022.
    Ranaway,
    /// It ran, finished, and answered something else.
    ///
    /// **The failure no engine can report**, and the reason [`attempt`] judges the answer rather
    /// than the absence of an error: this is a correct program computing something nobody asked
    /// for, and there is no message to repair from because nothing went wrong.
    Wrong(String),
    /// Not the script's fault, so not the script's to fix.
    ///
    /// [`Error::Engine`] is a fault the engine's own types rule out — DR-0002 says no input can
    /// cause one, so telling an author "your code did this" spends the next round rewriting
    /// something that was already right. [`Error::Collected`] is the host reading a value too late.
    /// Both are bug reports about this side of the boundary.
    NotTheScript(String),
}

impl Rejection {
    /// The sentence whoever wrote the script would be handed.
    fn feedback(&self) -> String {
        match self {
            Self::Unparsed(said) => format!(
                "it is not a program yet — the parser said: {said}. Nothing ran, so there is \
                 nothing here about the data."
            ),
            Self::Threw(said) => {
                format!("it ran and threw: {said}. Something it assumed about the data is not so.")
            }
            Self::Ranaway => "it did not finish inside the time it was given, and was stopped. \
                              That is a loop whose condition never becomes false."
                .to_string(),
            Self::Wrong(got) => format!(
                "it ran, finished, and answered {got} where the total is {EXPECTED}. Nothing went \
                 wrong — this is a working program that computes something else, so there is no \
                 error message to go on."
            ),
            Self::NotTheScript(said) => format!("the sandbox itself is at fault: {said}"),
        }
    }
}

/// Run one draft in a sandbox of its own, and judge it — the answer, or why it was not accepted.
///
/// **Judging is part of the same job, and separating the two is the mistake this exists to avoid.**
/// A loop whose `Ok` means "the engine did not object" accepts the second draft, which runs
/// perfectly and counts the orders. The engine has nothing to say about that and never will.
fn attempt(source: &str) -> Result<String, Rejection> {
    let mut engine = sandbox();

    // The fixture is the host's own source and is not the script's to get wrong, so a failure here
    // is a bug in this file rather than something to report as feedback.
    engine.eval(FIXTURE).expect("the host's own fixture runs");

    let value = engine.eval(source).map_err(classify)?;

    // A value is not yet an answer: `String(value)` is §7.1.17, so an object's own `toString` runs
    // here and is free to throw. That throw is the script's too, and classifies like any other.
    let answer = engine.text(value).map_err(classify)?;

    if answer == EXPECTED {
        Ok(answer)
    } else {
        Err(Rejection::Wrong(answer))
    }
}

/// Which kind of failure this is — the one place the API's four cases become the loop's five.
fn classify(error: Error) -> Rejection {
    match error {
        Error::Syntax(said) => Rejection::Unparsed(said),
        Error::Thrown(said) => Rejection::Threw(said),
        Error::Interrupted => Rejection::Ranaway,
        Error::Collected => {
            Rejection::NotTheScript("it answered with a value already freed".to_string())
        }
        Error::Engine(fault) => Rejection::NotTheScript(format!(
            "a fault DR-0002 says no input can cause: {fault:?}"
        )),
    }
}

/// A new draft, and what changed — for the transcript, so a patch can be read rather than diffed.
struct Patch {
    /// What the rule did, in the words the rule would use to justify it.
    what: String,
    /// The source to run next round.
    source: String,
}

/// Compute the next draft from the failure, or `None` when no rule applies.
///
/// The rules are tried by the *kind* of failure and not in sequence, which is the whole point: the
/// loop converges because each round is told something different, not because the rules happen to
/// be in the right order.
fn repair(source: &str, rejection: &Rejection) -> Option<Patch> {
    match rejection {
        Rejection::Unparsed(said) => close_the_delimiter(source, said),
        Rejection::Threw(said) if said.contains("not an object") => drop_the_dead_link(source),
        Rejection::Ranaway => advance_the_counter(source),
        // No rule, deliberately. See `Rejection::Wrong`: there is nothing in a right answer to the
        // wrong question that says which part is wrong.
        Rejection::Threw(_) | Rejection::Wrong(_) | Rejection::NotTheScript(_) => None,
    }
}

/// Close what the parser said was missing — and check the answer by parsing it again.
///
/// The parser names what it wanted between backticks, but [`Error::Syntax`] carries no span: the
/// sentence says *what* and never *where*. So appending it to the end is a guess, and the guess is
/// only a repair because a scratch engine confirms the result parses. Without that check this rule
/// would happily produce a second draft that is broken in a new way, and the loop would report the
/// new breakage as though the rule had helped.
fn close_the_delimiter(source: &str, said: &str) -> Option<Patch> {
    let wanted = said.split('`').nth(1)?;
    // Only a closing delimiter, because only those can be missing from the *end*. A parser wanting
    // an identifier or an expression is telling you about a hole somewhere in the middle, and this
    // rule has nothing to say about that.
    if !matches!(wanted, ")" | "}" | "]") {
        return None;
    }
    let patched = format!("{source}{wanted}");
    parses(&patched).then(|| Patch {
        what: format!("appended the `{wanted}` the parser asked for, and it parses now"),
        source: patched,
    })
}

/// Drop a property the data does not have — asked of the data, never guessed from the code.
///
/// §7.1.2-ish reading of the failure: `cannot read a property of something that is not an object`
/// means the script read *through* a link that is `undefined`. So the rule looks for exactly that
/// shape — a `.name.` that something is read through — and asks the sandbox whether any order has
/// it. A name nothing has is the dead link, and removing it is the repair.
///
/// **Asking the data is what makes this a repair rather than a guess**, and it is also what keeps
/// it from touching `orders.length`: `length` is read *from* an array and not *through*, so it is
/// never a candidate however the probe would answer.
fn drop_the_dead_link(source: &str) -> Option<Patch> {
    for name in links_read_through(source) {
        if ask(&format!(
            "orders.some(function (o) {{ return '{name}' in Object(o) }})"
        )) {
            continue;
        }
        return Some(Patch {
            what: format!("no order has a `{name}`, so the read goes straight through it"),
            source: source.replace(&format!(".{name}."), "."),
        });
    }
    None
}

/// Advance the counter a runaway `while` never advances.
///
/// The one thing true of every loop the time budget stops: the condition tests something the body
/// does not change. So the rule reads the first name in the condition, checks the body never
/// assigns it, and writes the advance the author left out. It answers `None` when the body *does*
/// assign it — that is a runaway of some other kind, and a rule that patched it anyway would be
/// guessing.
fn advance_the_counter(source: &str) -> Option<Patch> {
    let head = source.find("while (")? + "while (".len();
    let condition_end = head + source[head..].find(')')?;
    let name = identifier_at(&source[head..condition_end])?;

    let open = condition_end + source[condition_end..].find('{')?;
    let close = matching_brace(source, open)?;
    if assigns(&source[open + 1..close], &name) {
        return None;
    }

    // On its own line, so §12.10's automatic semicolon insertion ends whatever statement the body
    // finished with. Appended without a newline this would read as one expression and the next
    // round would report a syntax error the rule had itself introduced.
    let mut patched = String::from(source[..close].trim_end());
    patched.push_str(&format!("\n  {name} = {name} + 1;\n"));
    patched.push_str(&source[close..]);
    Some(Patch {
        what: format!("the condition tests `{name}` and the body never writes it — advancing it"),
        source: patched,
    })
}

/// Whether `source` is a program at all, asked of the engine rather than answered here.
///
/// It is *run* to find out, in a scratch sandbox with no fixture and a budget small enough that a
/// runaway costs nothing: a throw, an interrupt or a `ReferenceError` for the data that is not
/// there all mean the same thing here, which is that it parsed. Only [`Error::Syntax`] means it did
/// not, and whatever the run did to that engine goes when the engine does.
fn parses(source: &str) -> bool {
    let mut engine = Engine::new();
    engine.set_time_budget(Some(Duration::from_millis(5)));
    !matches!(engine.eval(source), Err(Error::Syntax(_)))
}

/// Put a yes/no question to the data, in JavaScript, in a sandbox of its own.
///
/// The repairer's way of looking at what it is working with. Anything that is not the answer `true`
/// — a throw, a runaway, a value that is not a Boolean — is `false`, because a question the data
/// cannot answer has not been answered yes.
fn ask(question: &str) -> bool {
    let mut engine = sandbox();
    engine.eval(FIXTURE).expect("the host's own fixture runs");
    engine
        .eval(question)
        .and_then(|value| engine.text(value))
        .is_ok_and(|answer| answer == "true")
}

/// A fresh realm with the host's bounds on it and nothing of the host's inside it.
///
/// The budgets are the whole of what makes this safe to point at code nobody has read. They are
/// small on purpose: this task is arithmetic over three rows, and a bound chosen from what the work
/// needs catches a runaway on its first pass rather than its thousandth.
fn sandbox() -> Engine {
    let mut engine = Engine::new();
    engine.set_time_budget(Some(Duration::from_millis(50)));
    engine.set_heap_budget(8 * 1024 * 1024);
    engine
}

/// Every name the source reads *through* — a `.name` with another `.` straight after it.
///
/// Not every property access: the failure being repaired is a read through something that is not an
/// object, so a name at the end of a chain was never a candidate. That distinction is what keeps
/// `orders.length` out of it while `orders[at].details.price` puts `details` in.
fn links_read_through(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'.' {
            at += 1;
            continue;
        }
        let start = at + 1;
        let mut end = start;
        while end < bytes.len() && is_name_byte(bytes[end]) {
            end += 1;
        }
        if end > start && bytes.get(end) == Some(&b'.') {
            found.push(source[start..end].to_string());
        }
        // `max` because an empty name leaves `end` at `start`, and a scan that does not move is a
        // loop this file exists to be able to talk about.
        at = end.max(start);
    }
    found
}

/// The first identifier in `text`, which for a loop condition is what the loop tests.
fn identifier_at(text: &str) -> Option<String> {
    let start = text.find(|c: char| is_name_byte(c as u8))?;
    let end = text[start..]
        .find(|c: char| !is_name_byte(c as u8))
        .map_or(text.len(), |at| start + at);
    Some(text[start..end].to_string())
}

/// The `}` that closes the `{` at `open`, or `None` if the source runs out first.
fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (at, byte) in source.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `body` writes to `name` — an assignment, a compound one, or an increment.
///
/// Bounded at both ends so `at` is not found inside `that` and `o.at` is not mistaken for the
/// variable: a property of that name is a different thing entirely, and patching on it would add an
/// advance to a loop that already has one.
fn assigns(body: &str, name: &str) -> bool {
    body.match_indices(name).any(|(at, _)| {
        let before = body[..at].bytes().next_back();
        if before.is_some_and(|byte| is_name_byte(byte) || byte == b'.') {
            return false;
        }
        let after = body[at + name.len()..].trim_start();
        match after.as_bytes() {
            [b'=', b'=', ..] => false,
            [b'=', ..] | [b'+', b'+', ..] | [b'-', b'-', ..] => true,
            [b'+', b'=', ..] | [b'-', b'=', ..] => true,
            _ => false,
        }
    })
}

/// Whether this byte may appear in an identifier — ASCII only, which is all these rules read.
const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// The script as it reads in a transcript, so the output shows what ran rather than describing it.
fn indent(source: &str) -> String {
    source
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
