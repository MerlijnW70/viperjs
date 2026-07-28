//! `cargo run -p conformance` — run test262 and check the ratchet.

use conformance::drive::{Report, find_tests, run_all, suite_revision};
use conformance::expectations::{Expectations, Judgement};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// How long one test may take before it is abandoned.
///
/// Generous, because the point is to survive `while (true);` rather than to measure speed. A test
/// that legitimately needs longer than this on any machine would be a performance bug worth its
/// own line in the expectations file.
const BUDGET: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let mut root = std::env::var("TEST262").map(PathBuf::from).ok();
    let mut expectations_path = PathBuf::from("conformance/expectations.txt");
    let mut bless = false;
    let mut filter = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bless" => bless = true,
            "--test262" => root = arguments.next().map(PathBuf::from),
            "--expectations" => match arguments.next() {
                Some(path) => expectations_path = PathBuf::from(path),
                None => return complain("--expectations needs a path"),
            },
            // Running one directory is how a failure bucket gets worked through, and it must not
            // touch the ratchet — a partial run has nothing to say about the tests it skipped.
            "--only" => filter = arguments.next(),
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

    let mut files = find_tests(&root);
    if files.is_empty() {
        return complain(&format!("no tests under {}/test", root.display()));
    }
    if let Some(filter) = &filter {
        files.retain(|file| file.to_string_lossy().replace('\\', "/").contains(filter));
        println!("running {} of the suite matching {filter}", files.len());
    }
    let workers = std::thread::available_parallelism().map_or(4, |count| count.get());
    println!("running {} files on {workers} threads", files.len());
    let report = run_all(&root, &files, workers, BUDGET);
    announce(&report);

    if filter.is_some() {
        // A filtered run cannot judge the ratchet: every test it did not run would read as one
        // that stopped failing, and blessing from it would delete most of the file.
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
  --bless                rewrite the expectations file from this run
  --help                 this";
