//! The third ratchet's instrument — inputs the engine has never seen, checked for panics.
//!
//! # Why this exists and why it did not
//!
//! AGENTS.md names three ratchets and the third is "no input panics, ever. **Fuzz what you build**;
//! a crash is a P0". Three comments in the lexer say what "a fuzzer finds first" — a backslash
//! against the end of input, an escape cut in half — and nothing fuzzed anything. The invariant had
//! a rule, a P0 severity and no tool.
//!
//! DR-0002 is the promise: an embedder runs untrusted script inside their own process, so a script
//! that ends that process is the engine's central claim failing. Mutation coverage cannot see it —
//! a panic is not a wrong answer — and the expectations file cannot either, because a file that
//! crashes the worker is a file whose verdict never arrives.
//!
//! # Why the corpus is test262 rather than random bytes
//!
//! Random bytes are rejected by the lexer in the first few characters, so almost every one measures
//! the same rejection. What reaches the interesting code is text that is *nearly* valid, and there
//! are 48,000 nearly-valid files already on disk. So the corpus is the suite, and the mutations are
//! the four that turn valid text into the shapes those lexer comments describe:
//!
//! - **Truncation.** Every "against the end of input" case, for free and at every offset.
//! - **A flipped byte.** One code unit becomes another, which is how an escape is cut in half.
//! - **A deleted run.** Brackets and braces stop balancing, which is what the parser's recovery and
//!   the compiler's early errors have to survive.
//! - **A splice.** The tail of one file after the head of another, which produces nesting no
//!   hand-written test would.
//!
//! # What counts as a failure
//!
//! A **panic**, and nothing else. A SyntaxError is the right answer to nonsense; a RangeError from
//! the heap or the step budget is DR-0013 and DR-0022 doing their job; an infinite loop is a
//! program that loops for ever and is bounded by the time budget. Only a panic is a bug here, and
//! it is caught rather than reported by the process dying so that one crash does not end the run.
//!
//! Output is deliberately quiet: a seed, a count, and the first line of any input that panicked.
//! A fuzzer that prints its corpus is one nobody runs twice.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

/// How much heap one attempt may take before the engine refuses — DR-0013, in bytes.
///
/// Small on purpose. A mutated file is as likely to build a huge string as to do anything else, and
/// waiting for sixty-four mebibytes of it says nothing a smaller refusal does not.
const HEAP: usize = 8 << 20;

/// How long one attempt may run before the machine is stopped — DR-0022.
///
/// A mutation turns a bounded loop into an unbounded one all the time, and an interrupted run is a
/// pass here: what is being looked for is a panic, and a script that will not stop is not one.
const BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

/// What one attempt came to.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// It ran, or it was refused, or it was stopped — all of which the engine is allowed to do.
    Survived,
    /// It panicked, which DR-0002 says nothing may.
    Panicked(String),
}

/// A seeded xorshift, so a run is reproducible from its seed and nothing else.
///
/// The same generator `Math.random` uses, for the same reason: no dependency may enter this
/// repository (DR-0001), and a fuzzer whose corpus cannot be reproduced is a fuzzer whose findings
/// cannot be handed to anybody.
struct Rng(u64);

impl Rng {
    /// A generator started from `seed`, with zero — the one state a xorshift cannot leave — mapped
    /// away rather than rejected.
    ///
    /// **Only zero.** The obvious `seed | 1` maps zero away too and takes every even seed with it,
    /// so seeds 42 and 43 would name the same run — half of them useless, and silently. Caught by
    /// the test below asking whether two seeds disagree, which is the property the whole tool rests
    /// on. The replacement is the golden-ratio constant, chosen for being a long way from zero and
    /// for nothing else.
    fn new(seed: u64) -> Self {
        match seed {
            0 => Self(0x9E37_79B9_7F4A_7C15),
            _ => Self(seed),
        }
    }

    /// The next value, and the state advanced.
    fn next(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        state
    }

    /// A number below `bound`, or zero when there is nothing to choose from.
    ///
    /// The modulo bias is real and does not matter: what is being chosen is an offset into a file,
    /// and no bug hides in the last few offsets of a range more than in the others.
    fn below(&mut self, bound: usize) -> usize {
        match bound {
            0 => 0,
            _ => (self.next() % bound as u64) as usize,
        }
    }
}

/// One mutation of `source`, chosen by `rng` — the four shapes the module doc lists.
///
/// Takes a second file for the splice, because a splice needs one and generating a plausible tail
/// is exactly the thing the corpus is here to avoid.
fn mutate(rng: &mut Rng, source: &str, other: &str) -> String {
    // Byte offsets have to land on a character boundary or the slice panics, which would be this
    // tool crashing rather than the engine — the one failure that would read as a finding and is
    // not one.
    let at = |text: &str, offset: usize| {
        let mut offset = offset.min(text.len());
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    };
    match rng.next() % 4 {
        // Truncation: every "against the end of input" case, at every offset.
        0 => source[..at(source, rng.below(source.len().max(1)))].to_string(),
        // A flipped code unit. Written through `char_indices` so the replacement is a character and
        // not half of one.
        1 => {
            let cut = at(source, rng.below(source.len().max(1)));
            let rest = source[cut..].chars().next().map_or(0, char::len_utf8);
            let mut out = String::with_capacity(source.len());
            out.push_str(&source[..cut]);
            // One of the characters the lexer branches on, so the flip lands somewhere it matters.
            out.push(
                *b"\\/'\"`${}()[]\n\r*?:;="
                    .get(rng.below(19))
                    .unwrap_or(&b'/') as char,
            );
            out.push_str(&source[cut + rest..]);
            out
        }
        // A deleted run, which is what stops brackets balancing.
        2 => {
            let start = at(source, rng.below(source.len().max(1)));
            let end = at(source, start + 1 + rng.below(64));
            let mut out = String::with_capacity(source.len());
            out.push_str(&source[..start]);
            out.push_str(&source[end..]);
            out
        }
        // A splice: the tail of another file after the head of this one.
        _ => {
            let head = at(source, rng.below(source.len().max(1)));
            let tail = at(other, rng.below(other.len().max(1)));
            format!("{}{}", &source[..head], &other[tail..])
        }
    }
}

/// Run `source` through the whole engine and answer whether it panicked.
///
/// Parse, compile and **run**, because each stage has its own way of meeting an input nobody
/// expected and the last one is where a chunk and its compiler can disagree. The budgets are set so
/// that a mutation which turns a loop unbounded costs a quarter of a second rather than the run.
fn attempt(source: &str) -> Verdict {
    // The engine is built inside the guarded region: `Engine::new` allocates a realm, and a panic
    // there is as much a panic as one from the parser.
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut engine = viperjs::api::Engine::new();
        engine.set_heap_budget(HEAP);
        engine.set_time_budget(Some(BUDGET));
        // The answer is discarded on purpose. Every `Err` here is the engine refusing, which is the
        // behaviour being protected rather than a failure of it.
        let _ = engine.eval(source);
    }));
    match outcome {
        Ok(()) => Verdict::Survived,
        // The payload is whatever `panic!` was given, which for an index out of bounds is the
        // message the standard library wrote. Kept because it is the whole of what a finding says
        // before somebody reduces it.
        Err(payload) => Verdict::Panicked(describe(&payload)),
    }
}

/// What a panic payload says, as far as one can be read.
fn describe(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "a panic with no message".to_string()
}

/// Every `.js` file under `root`, which is the corpus.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|kind| kind == "js") {
                found.push(path);
            }
        }
    }
    // Sorted, so that a seed picks the same files on two machines. `read_dir` does not promise an
    // order and a fuzzer whose corpus depends on the filesystem is not reproducible.
    found.sort();
    found
}

/// What a run came to, for the caller to print and to exit on.
pub struct Report {
    /// How many mutations were attempted.
    pub attempts: usize,
    /// Each panic that was found.
    pub panics: Vec<Finding>,
}

/// One panic, with enough to reproduce it without running the fuzzer again.
pub struct Finding {
    /// The corpus file the mutation was made from — context, not a reproduction.
    pub from: PathBuf,
    /// What the panic said.
    pub said: String,
    /// **The mutated source itself**, which is the only thing that reproduces it.
    ///
    /// Carried rather than left to the seed, and that distinction cost a wrong claim. A seed
    /// reproduces the *input sequence* and not the engine's behaviour: `Math.random` is seeded from
    /// the clock at every `Engine::new`, and `Date.now` answers a different number every run — so a
    /// mutated file that branches on either takes a different path each time. A panic found once at
    /// seed 1 was not there at seed 1 the next day, and the record said the fix had closed it. It
    /// had not been shown to.
    pub source: String,
}

/// Fuzz the engine with `attempts` mutations of the suite under `root`, from `seed`.
///
/// **The seed reproduces the inputs and not the run.** The same seed, corpus and count generate the
/// same sequence of mutated sources — that part is exact. What it does not fix is what the *engine*
/// does with one: `Math.random` is seeded from the clock at every `Engine::new` and `Date.now`
/// answers a different number each time, so a mutated file that branches on either takes a different
/// path on every run. A finding is therefore reproduced by [`Finding::source`], which is why that
/// field exists; the seed is for re-running the same *search*, which is a weaker thing.
///
/// This was claimed the stronger way for one commit. See [`Finding::source`].
#[must_use]
pub fn run(root: &Path, seed: u64, attempts: usize) -> Report {
    let files = corpus(root);
    let mut rng = Rng::new(seed);
    let mut panics = Vec::new();
    // The default hook prints its own line to standard error for every panic, which for a tool
    // whose whole output is a report turns one finding into a wall — measured by injecting a panic
    // into the lexer on purpose and watching three findings print six times. The payload is what
    // this reports and the hook adds nothing to it, so it is silenced for the run and put back
    // afterwards, because a *host* embedding the engine should keep whatever hook it installed.
    //
    // Set **after** the empty-corpus check, so that the one path which returns without reaching the
    // restore below cannot leave the process without a hook. Reordered rather than given a second
    // restore: two restores is two things that can come apart.
    if files.is_empty() {
        return Report {
            attempts: 0,
            panics,
        };
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for _ in 0..attempts {
        let chosen = &files[rng.below(files.len())];
        let other = &files[rng.below(files.len())];
        let (Ok(source), Ok(second)) = (
            std::fs::read_to_string(chosen),
            std::fs::read_to_string(other),
        ) else {
            continue;
        };
        let mutated = mutate(&mut rng, &source, &second);
        if let Verdict::Panicked(said) = attempt(&mutated) {
            panics.push(Finding {
                from: chosen.clone(),
                said,
                source: mutated,
            });
        }
    }
    std::panic::set_hook(previous);
    Report { attempts, panics }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_reproducible_and_never_stops_moving() {
        // A fuzzer whose corpus cannot be reproduced from its seed is one whose findings cannot be
        // handed to anybody, so this is the property the whole tool rests on.
        let mut first = Rng::new(42);
        let mut second = Rng::new(42);
        let taken: Vec<u64> = (0..8).map(|_| first.next()).collect();
        assert_eq!(taken, (0..8).map(|_| second.next()).collect::<Vec<_>>());
        // …and two seeds do not agree, which is what makes a second run worth doing.
        let mut other = Rng::new(43);
        assert_ne!(taken[0], other.next());
        // Zero is the one state a xorshift cannot leave, and it is mapped away rather than refused.
        let mut zero = Rng::new(0);
        assert_ne!(zero.next(), 0);
    }

    #[test]
    fn below_stays_in_range_including_the_empty_one() {
        let mut rng = Rng::new(7);
        for _ in 0..64 {
            assert!(rng.below(3) < 3);
        }
        // An empty corpus must not divide by zero, which is the one way this tool could panic
        // while looking for panics.
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn a_mutation_never_splits_a_character() {
        // The one failure that would read as a finding and is not one: a byte offset landing inside
        // a multi-byte character panics in the *fuzzer*. Every branch is exercised against text
        // made entirely of them.
        let source = "const é = '日本語'; // ☃\n";
        let other = "función(ñ) { return '→' }\n";
        let mut rng = Rng::new(1);
        for _ in 0..400 {
            let mutated = mutate(&mut rng, source, other);
            // Nothing is asserted about the result beyond its existing: the property under test is
            // that building it did not panic.
            assert!(mutated.len() <= source.len() + other.len() + 1);
        }
    }

    #[test]
    fn an_input_that_refuses_is_a_survivor_and_only_a_panic_is_not() {
        // The tool's own judgement, which decides what it reports. Nonsense, a throw, a runaway and
        // an allocation past the budget are all the engine behaving.
        assert_eq!(attempt("const = = ="), Verdict::Survived);
        assert_eq!(attempt("throw new Error('x')"), Verdict::Survived);
        assert_eq!(attempt("while (true) {}"), Verdict::Survived);
        assert_eq!(
            attempt("var a = []; for (;;) { a.push(new Array(1000)) }"),
            Verdict::Survived
        );
        assert_eq!(attempt(""), Verdict::Survived);
        // …and the shapes the lexer's own comments name as what a fuzzer finds first.
        for source in ["'\\", "`${", "/[", "0x", "'\\u{", "//"] {
            assert_eq!(attempt(source), Verdict::Survived, "{source}");
        }
    }
}
