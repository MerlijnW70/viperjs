//! `cargo run -p conformance` — run test262 and check the ratchet.

use conformance::drive::{Report, WORKER_FLAG, find_tests, run_all, suite_revision, work};
use conformance::expectations::{Expectations, Judgement};
use conformance::options::{self, value_for};
use conformance::runner::Verdict;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// How long one test may take before it is abandoned, unless `--budget` says otherwise.
///
/// Generous for what it was written for — surviving `while (true);` rather than measuring speed —
/// and **not** generous enough for `RegExp/property-escapes`, which is why it is a default rather
/// than a constant now. Measured 2026-08-05: that directory alone passes 814 of its 1,226 runs,
/// and inside a full parallel run only about 300 of the same tests finish in time. The difference
/// is contention, not the engine: every worker gets a smaller share of the machine, each test
/// takes longer in wall-clock, and a budget written per test cannot see that.
///
/// The cost of getting it wrong is not a wrong verdict but a *noisy* one. Those runs cross the
/// line in both directions between runs of one unchanged commit, so the report's "newly passing"
/// swung between 264 and 844 in a single afternoon and every real gain had to be found by hand
/// underneath it.
///
/// **Thirty seconds and not ten, since 2026-08-08, and the reason is that this is now a gate.** At
/// ten, four scenarios in the whole suite sat exactly on the line — `decodeURI` and
/// `decodeURIComponent`'s `A2.5_T1`, in both modes — and crossed it in both directions between
/// consecutive runs of one unchanged commit, at every worker count tried. They were the *only*
/// timing-bound entries left in the expectations file, the several hundred that used to keep them
/// company having gone when the worker count was halved. So the choice was a ratchet that goes red
/// at random or a budget those four clear: measured, the whole `decodeURI` directory finishes in
/// 15 s of wall clock at two workers, and every one of its 222 scenarios passes at thirty.
///
/// What it costs is that a genuinely hung test takes thirty seconds to say so instead of ten, and
/// nothing else — no test that passes at ten fails at thirty, and the file now contains **no entry
/// that names a time at all**. A ratchet that a machine is going to enforce has to be able to
/// answer the same way twice.
const BUDGET: Duration = Duration::from_secs(30);

/// How many tests run at once, unless `--workers` says otherwise.
///
/// The question the host answers — how many hardware threads there are — and nothing else.
/// [`options::workers_for`] is the decision made about that number, and it lives there because it
/// is testable and this is not: `available_parallelism` answers whatever the machine running the
/// test happens to have.
fn default_workers() -> usize {
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get());
    options::workers_for(threads)
}

fn main() -> ExitCode {
    let mut root = std::env::var("TEST262").map(PathBuf::from).ok();
    let mut expectations_path = PathBuf::from("conformance/expectations.txt");
    let mut bless = false;
    // Where to leave the machine-readable summary, if a caller asked for one.
    let mut summary_path: Option<PathBuf> = None;
    let mut filter = None;
    let mut worker = false;
    let mut workers = default_workers();
    let mut budget = BUDGET;
    let mut fuzz_attempts: Option<usize> = None;
    // The default seed is a constant rather than a clock: a run nobody can reproduce is a run whose
    // findings cannot be handed to anybody, and `--seed` is how a second one differs.
    let mut seed: u64 = 1;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bless" => bless = true,
            "--summary" => match value_for("--summary", &mut arguments) {
                Ok(path) => summary_path = Some(PathBuf::from(path)),
                Err(problem) => return complain(&problem),
            },
            // How the run talks to its own child processes — see `drive::work`. Deliberately
            // absent from the usage text, because nobody should be typing it.
            WORKER_FLAG => worker = true,
            "--test262" => match value_for("--test262", &mut arguments) {
                Ok(path) => root = Some(PathBuf::from(path)),
                Err(problem) => return complain(&problem),
            },
            "--expectations" => match value_for("--expectations", &mut arguments) {
                Ok(path) => expectations_path = PathBuf::from(path),
                Err(problem) => return complain(&problem),
            },
            // Running one directory is how a failure bucket gets worked through, and it must not
            // touch the ratchet — a partial run has nothing to say about the tests it skipped.
            "--only" => match value_for("--only", &mut arguments) {
                Ok(chosen) => filter = Some(chosen),
                Err(problem) => return complain(&problem),
            },
            // Both of these change what a *timing* failure means, so they are named in the report
            // below: a number produced under one pair is not comparable with one produced under
            // another, and a reader who cannot see which was used cannot know that.
            "--workers" => match arguments.next().and_then(|text| text.parse().ok()) {
                Some(0) | None => return complain("--workers needs a positive count"),
                Some(count) => workers = count,
            },
            "--budget" => match arguments.next().and_then(|text| text.parse().ok()) {
                Some(0) | None => return complain("--budget needs a positive number of seconds"),
                Some(seconds) => budget = Duration::from_secs(seconds),
            },
            // DR-0002's ratchet, which is a different question from the expectations file's and
            // shares only the corpus — see [`conformance::fuzz`] for why the suite is the corpus.
            "--fuzz" => match arguments.next().and_then(|text| text.parse().ok()) {
                Some(0) | None => return complain("--fuzz needs a positive number of attempts"),
                Some(count) => fuzz_attempts = Some(count),
            },
            "--seed" => match arguments.next().and_then(|text| text.parse().ok()) {
                None => return complain("--seed needs a number"),
                Some(chosen) => seed = chosen,
            },
            "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return complain(&format!("unknown argument {other}")),
        }
    }
    let Some(root) = root else {
        return complain(
            "no test262 checkout: pass --test262 <path> or set the TEST262 environment variable",
        );
    };
    // A worker answers on standard output and judges nothing: the ratchet, the tally and the
    // expectations file all belong to the run that started it.
    if worker {
        work(&root);
        return ExitCode::SUCCESS;
    }

    // The third ratchet, before the suite is gathered: it wants the same checkout and nothing else
    // this function does. It answers on its own and does not run the suite, because a fuzzing run
    // and a conformance run are two different questions and combining them would mean neither
    // number could be read on its own.
    if let Some(attempts) = fuzz_attempts {
        return fuzz(&root, seed, attempts);
    }
    let mut files = find_tests(&root);
    if files.is_empty() {
        return complain(&format!("no tests under {}/test", root.display()));
    }
    if let Some(filter) = &filter {
        files.retain(|file| file.to_string_lossy().replace('\\', "/").contains(filter));
        println!("running {} of the suite matching {filter}", files.len());
    }
    println!(
        "running {} files on {workers} threads, {}s per test",
        files.len(),
        budget.as_secs()
    );
    let report = run_all(&root, &files, workers, budget);
    announce(&report);
    // Written before the ratchet is judged, so a run that goes **red** still leaves the number it
    // measured. A build that fails and says nothing about where it got to is one somebody has to
    // reproduce by hand to find out.
    if let Some(path) = &summary_path
        && let Err(error) = write_summary(path, &report)
    {
        return complain(&format!("{}: {error}", path.display()));
    }

    if filter.is_some() {
        // A filtered run cannot judge the ratchet: every test it did not run would read as one
        // that stopped failing, and blessing from it would delete most of the file.
        //
        // So it prints the failures instead, which is the whole reason to run one. Without this a
        // narrowed run answered "56 failed" and nothing else, and the only way to see *why* one
        // failed was a full run blessed into a scratch file — two and a half minutes to read a
        // sentence the worker already knew. Bounded, because a filter matching a thousand tests is
        // a filter, not a question.
        report_failures(&report);
        println!("\nthis was a partial run, so the expectations file was not checked");
        return ExitCode::SUCCESS;
    }
    let revision = suite_revision(&root);
    if bless {
        let expectations = Expectations::from_outcomes(&report.outcomes, revision);
        if let Err(error) = std::fs::write(&expectations_path, expectations.render()) {
            return complain(&format!("{}: {error}", expectations_path.display()));
        }
        println!(
            "\nwrote {} failures to {}",
            expectations.len(),
            expectations_path.display()
        );
        return ExitCode::SUCCESS;
    }
    let expectations = match Expectations::read(&expectations_path) {
        Ok(expectations) => expectations,
        Err(error) => return complain(&format!("{}: {error}", expectations_path.display())),
    };
    // Said, not enforced. The suite moves, and measuring against a newer one is the normal way to
    // find out what moved — but a disagreement here explains a wall of regressions that are not
    // the engine's doing, and finding that out from a printed line beats finding it out by
    // bisecting.
    if expectations.suite.is_some() && expectations.suite != revision {
        println!(
            "\nthe expectations were recorded against test262 {}, and this checkout is {}",
            expectations
                .suite
                .as_deref()
                .unwrap_or("an unknown revision"),
            revision.as_deref().unwrap_or("an unknown revision"),
        );
    }
    let judgement = expectations.judge(&report.outcomes);
    report_judgement(&judgement);
    // Abandoned workers never end, so joining them would be waiting forever for answers already
    // recorded — the process leaves rather than waits.
    match judgement.is_green() {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}

/// The headline number again, in the one shape a machine can read it in.
///
/// Shields.io's endpoint schema exactly, and nothing else in the file. A badge is the only intended
/// consumer, and extra keys would invite a second one that then depends on a shape this was never
/// promising — the record of *which* tests fail and why is the expectations file, which is in the
/// repository and is a great deal more use than any JSON of counts.
///
/// **The share of the whole suite, not of what ran.** [`announce`] prints both and says why: the
/// share of what ran flatters an engine that declines most of the suite, and it *falls* every time
/// the engine learns to compile something new. A badge that could go down on good news is worse
/// than no badge.
fn write_summary(path: &std::path::Path, report: &Report) -> std::io::Result<()> {
    let (passed, failed, skipped) = report.tally();
    let total = passed + failed + skipped;
    let whole = match total {
        0 => 0.0,
        _ => passed as f64 * 100.0 / total as f64,
    };
    // Hand-written rather than serialised, because the alternative is a dependency and DR-0001 says
    // no. Every field is a number this harness produced or a literal, so there is nothing here that
    // could need escaping.
    let json = format!(
        "{{\n  \"schemaVersion\": 1,\n  \"label\": \"test262\",\n  \"message\": \"{whole:.2}% ({passed} of {total})\",\n  \"color\": \"{}\"\n}}\n",
        // Round numbers that mean nothing in themselves. They exist so a reader can see a
        // catastrophic drop without reading the digits, and nothing turns on which band it is in.
        match whole {
            share if share >= 90.0 => "brightgreen",
            share if share >= 75.0 => "green",
            share if share >= 50.0 => "yellow",
            _ => "orange",
        }
    );
    std::fs::write(path, json)
}

/// The numbers, which are the reason the harness exists.
fn announce(report: &Report) {
    let (passed, failed, skipped) = report.tally();
    let judged = passed + failed;
    // Against what was *run*, not against the whole suite: a percentage that counted skipped
    // tests as failures would fall every time the engine learned to compile something new.
    let share = match judged {
        0 => 0.0,
        _ => passed as f64 * 100.0 / judged as f64,
    };
    let total = judged + skipped;
    let whole = match total {
        0 => 0.0,
        _ => passed as f64 * 100.0 / total as f64,
    };
    println!("\n{passed} passed, {failed} failed, {skipped} not run");
    // Both, always, because either alone misleads. The share of what ran flatters an engine that
    // declines most of the suite, and it *falls* every time the engine learns something new; the
    // share of everything is the honest conformance figure and the one to quote.
    println!("{share:.2}% of what ran — {whole:.2}% of the whole suite");
    let skips = report.skips();
    if !skips.is_empty() {
        println!("\nwhat stopped the rest, commonest first:");
        for (why, count) in skips.iter().take(15) {
            println!("  {count:>7}  {why}");
        }
        if skips.len() > 15 {
            // Said out loud, because a list that silently stopped at fifteen would read as the
            // whole story and quietly hide a bucket somebody should be looking at.
            println!("  {:>7}  and {} more kinds", "", skips.len() - 15);
        }
    }
}

/// DR-0002's ratchet — mutations of the suite, checked for panics.
///
/// Red on the first one, because a panic is a P0 and a count of them is not a thing to get used to.
/// The seed is printed whatever happens, since it and the attempt count are the whole of what
/// reproducing a finding needs.
fn fuzz(root: &std::path::Path, seed: u64, attempts: usize) -> ExitCode {
    println!(
        "fuzzing {attempts} mutations of {} from seed {seed}",
        root.display()
    );
    let report = conformance::fuzz::run(root, seed, attempts);
    if report.attempts == 0 {
        return complain(&format!("no .js files under {}", root.display()));
    }
    if report.panics.is_empty() {
        println!("\n{} attempts, no panics", report.attempts);
        return ExitCode::SUCCESS;
    }
    println!(
        "\n{} PANIC(S) in {} attempts:",
        report.panics.len(),
        report.attempts
    );
    // The offending source is **written to disk**, because it and not the seed is what reproduces a
    // finding — the seed fixes the inputs and not what the engine does with one, since `Math.random`
    // is clock-seeded and `Date.now` moves. See `fuzz::Finding::source`.
    for (at, finding) in report.panics.iter().enumerate() {
        let path = std::path::PathBuf::from(format!("fuzz-panic-{at}.js"));
        let written = std::fs::write(&path, &finding.source);
        println!(
            "  mutated from {}\n    {}\n    input: {}",
            finding.from.display(),
            finding.said,
            match written {
                Ok(()) => path.display().to_string(),
                Err(error) => format!("could not be written ({error})"),
            }
        );
    }
    println!("\nre-run the same search with --fuzz {attempts} --seed {seed}");
    ExitCode::FAILURE
}

/// Every failure of a narrowed run, with the reason the worker gave.
///
/// Only for a filtered run. A full one has twelve thousand of these and the expectations file is
/// where they belong; a run narrowed to one directory has a handful, and they are the answer to the
/// question that made somebody narrow it.
fn report_failures(report: &Report) {
    let failures: Vec<_> = report
        .outcomes
        .iter()
        .filter_map(|outcome| match &outcome.verdict {
            Verdict::Failed(why) => Some((outcome, why)),
            _ => None,
        })
        .collect();
    if failures.is_empty() {
        return;
    }
    println!("\nwhat failed:");
    for (outcome, why) in failures.iter().take(FAILURES_SHOWN) {
        let strict = match outcome.strict {
            true => " (strict)",
            false => "",
        };
        println!("  {}{strict}\n    {why}", outcome.name);
    }
    if failures.len() > FAILURES_SHOWN {
        // Named rather than trailed off, for [`announce`]'s reason: a list that stopped silently
        // would read as the whole story.
        println!(
            "  …and {} more — narrow the filter to see them",
            failures.len() - FAILURES_SHOWN
        );
    }
}

/// How many failures a narrowed run prints before it says how many it did not.
///
/// A policy figure. Large enough for a directory's worth of one bug, small enough that a filter
/// matching half the suite does not become the output.
const FAILURES_SHOWN: usize = 40;

/// What the ratchet has to say, and what to do about it.
fn report_judgement(judgement: &Judgement) {
    for (name, reason) in &judgement.regressions {
        println!("REGRESSED {name}\n          {reason}");
    }
    for name in &judgement.fixed {
        println!("FIXED     {name}\n          delete its line from the expectations file");
    }
    for (name, was, now) in &judgement.changed {
        println!("CHANGED   {name}\n          was: {was}\n          now: {now}");
    }
    match judgement.is_green() {
        true => println!("the expectations file is exact"),
        false => println!(
            "\n{} regressions, {} newly passing, {} failing differently",
            judgement.regressions.len(),
            judgement.fixed.len(),
            judgement.changed.len()
        ),
    }
}

/// Say what is wrong on stderr and leave.
fn complain(problem: &str) -> ExitCode {
    eprintln!("conformance: {problem}\n\n{USAGE}");
    ExitCode::FAILURE
}

/// What the arguments are.
const USAGE: &str = "\
usage: cargo run -p conformance -- [options]

  --test262 <path>       a checkout of tc39/test262 (or set TEST262)
  --expectations <path>  the ratchet file (default conformance/expectations.txt)
  --only <substring>     run just the matching files, and do not judge the ratchet
  --workers <count>      how many tests run at once (default: half the hardware threads)
  --budget <seconds>     how long one test may take (default 30). Both of these decide what a
                         timing failure means, so a number is only comparable with one measured
                         under the same pair
  --bless                rewrite the expectations file from this run
  --summary <path>       also write the headline number as shields.io endpoint JSON
  --fuzz <attempts>      DR-0002 instead of the suite: mutate the corpus and look for panics
  --seed <number>        which fuzzing run to reproduce (default 1)
  --help                 this";
