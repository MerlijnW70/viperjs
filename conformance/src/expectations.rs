//! The file that may only shrink.
//!
//! # What an entry is
//!
//! A line saying "this test fails, and here is what happened". It is a claim, and it is checked
//! against reality on every run in *both* directions:
//!
//! - A listed test that now passes fails the run until the line is deleted. A stale entry is a
//!   test nobody is watching, and it is how a suite quietly stops meaning anything.
//! - An unlisted test that now fails fails the run. That is the regression check.
//!
//! Nothing here can add a line. `--bless` rewrites the file wholesale and is a deliberate act by a
//! person; a harness that could write its own excuses would not be a ratchet.
//!
//! # Why the reason is stored and compared
//!
//! `AGENTS.md`: "Every line added to the conformance expectations is a claim that a failure is
//! acceptable-for-now, and that file is the one place the conformance ratchet can be quietly
//! laundered." The reason makes the file a work list rather than a list of names — and a failure
//! whose *reason changed* is reported even though the pass/fail did not, because a test that
//! started failing differently is a different fact.

use crate::runner::{Outcome, Verdict};
use std::collections::BTreeMap;
use std::path::Path;

/// The recorded failures, by the name their run is keyed under.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Expectations {
    entries: BTreeMap<String, String>,
    /// The test262 revision these entries were recorded against, if the file says.
    ///
    /// The suite moves — tests are added, corrected and withdrawn — so a set of expectations is
    /// only exact about the revision that produced it. `conformance/README.md` puts it plainly: a
    /// conformance number without a suite revision is not a number.
    pub suite: Option<String>,
}

/// What a run said about the expectations file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Judgement {
    /// Tests that failed and were not listed. Any one of these is a red build.
    pub regressions: Vec<(String, String)>,
    /// Tests that are listed and now pass. Delete their lines.
    pub fixed: Vec<String>,
    /// Tests that still fail, but differently from what was written down.
    pub changed: Vec<(String, String, String)>,
}

impl Judgement {
    /// Whether the run may go green.
    ///
    /// A fixed test is as red as a regression, which is the part people find surprising. It is the
    /// whole mechanism: if passing tests could stay listed, the file would only ever grow and the
    /// number it guards would stop being a number about the engine.
    ///
    /// **A changed reason is reported and does not stop it**, which it used to. That was not part
    /// of the ratchet: an entry whose reason moved is still an entry, still a failure, and the file
    /// has not grown or loosened. It is *information* — today it repeatedly named the next slice,
    /// because a test that starts failing differently is one whose first gap just closed — and
    /// [`Judgement`] carries it either way for a reader to act on.
    ///
    /// What it cannot be is a gate, because some reasons are not the engine's to fix. Three files
    /// report **which sub-case** failed first and that varies between runs (`Int16Array and
    /// makeArray` one time, `Int32Array and makeIterable` the next), and two sit close enough to
    /// the per-test budget to cross it either way. With those in the gate a clean run can never go
    /// green, and an exit code that is always red is one nobody reads.
    pub fn is_green(&self) -> bool {
        self.regressions.is_empty() && self.fixed.is_empty()
    }
}

impl Expectations {
    /// Read the file. A file that does not exist is an empty set — the state before the first run.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        Ok(Self::parse(&text))
    }

    /// Read the file's text.
    pub fn parse(text: &str) -> Self {
        let suite = text
            .lines()
            .find_map(|line| line.strip_prefix(SUITE))
            .map(|revision| revision.trim().to_string())
            .filter(|revision| !revision.is_empty());
        let entries = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            // A line with no separator is malformed, and dropping it would silently un-list a
            // test — so it is kept with an empty reason, which then reads as a changed entry and
            // makes itself visible on the next run.
            .map(|line| match line.split_once(" :: ") {
                Some((name, reason)) => (name.to_string(), canonical(reason)),
                None => (line.to_string(), String::new()),
            })
            .collect();
        Self { entries, suite }
    }

    /// How many failures are recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is recorded, which is where this ends.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The file's text, for `--bless`.
    pub fn render(&self) -> String {
        let mut text = String::from(HEADER);
        if let Some(suite) = &self.suite {
            text.push_str(SUITE);
            text.push_str(suite);
            text.push('\n');
        }
        for (name, reason) in &self.entries {
            text.push_str(name);
            text.push_str(" :: ");
            text.push_str(reason);
            text.push('\n');
        }
        text
    }

    /// What a set of outcomes says about this file.
    ///
    /// Skipped runs are not judged at all. A test the engine declined to run is not a failure to
    /// record and not a pass to celebrate — writing it down either way would put thousands of
    /// "not implemented yet" lines in a file that is supposed to be read.
    pub fn judge(&self, outcomes: &[Outcome]) -> Judgement {
        let mut judgement = Judgement::default();
        let mut seen = BTreeMap::new();
        for outcome in outcomes {
            let key = outcome.key();
            match &outcome.verdict {
                Verdict::Skipped(_) => continue,
                Verdict::Passed => {
                    if self.entries.contains_key(&key) {
                        judgement.fixed.push(key.clone());
                    }
                }
                Verdict::Failed(why) => match self.entries.get(&key) {
                    None => judgement.regressions.push((key.clone(), why.clone())),
                    Some(recorded) if *recorded != canonical(why) => {
                        judgement
                            .changed
                            .push((key.clone(), recorded.clone(), why.clone()));
                    }
                    Some(_) => {}
                },
            }
            seen.insert(key, ());
        }
        // A listed test that did not run at all — deleted upstream, or skipped because the engine
        // now refuses to compile it — is a line about nothing. It is reported as fixed because the
        // remedy is the same: delete it.
        for name in self.entries.keys() {
            if !seen.contains_key(name) {
                judgement.fixed.push(name.clone());
            }
        }
        judgement.fixed.sort();
        judgement.fixed.dedup();
        judgement
    }

    /// The expectations a set of outcomes would produce, for `--bless`.
    pub fn from_outcomes(outcomes: &[Outcome], suite: Option<String>) -> Self {
        let entries = outcomes
            .iter()
            .filter_map(|outcome| match &outcome.verdict {
                Verdict::Failed(why) => Some((outcome.key(), canonical(why))),
                _ => None,
            })
            .collect();
        Self { entries, suite }
    }
}

/// A reason in the only form this file can hold it.
///
/// One line of a text file cannot carry trailing whitespace: reading it back gives the line
/// without it, whatever was written. So a reason is stored *already* in that form, and comparing
/// two of them compares like with like.
///
/// This is not hypothetical tidiness. `language/comments/S7.4_A5.js` throws `'#' + uu + ' '`, and
/// with the raw reason on one side and the read-back reason on the other it reported as changed on
/// every run forever — a line that could never be made green, which is the kind of entry a reader
/// learns to scroll past.
///
/// A line cannot carry a *newline* either, and that one is worse than untidy. The format is one
/// entry per line, so a reason containing one was written across several and read back as an entry
/// plus a handful of nonsense ones made from its own continuation lines — which then reported as
/// tests that had started passing, because no run ever produces a test called `  "Get:length",`.
/// `Array/prototype/reverse/length-exceeding-integer-limit-with-proxy.js` quotes a whole JavaScript
/// array in its message and put 36 such phantoms in the file, and the run they made un-green could
/// not be made green by fixing anything.
///
/// So a newline becomes a space: the reason stays readable and greppable, comparison compares like
/// with like, and the result is idempotent — canonicalising an already-stored reason yields itself,
/// which is what lets a stored entry match a freshly produced one at all.
fn canonical(reason: &str) -> String {
    reason
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .trim_end()
        .to_string()
}

/// The header line that records which test262 the entries were measured against.
const SUITE: &str = "# test262 ";

/// The lines at the top of the file, which are there for whoever opens it next.
const HEADER: &str = "\
# Every line here is a test262 failure ViperJS has not fixed yet, and a claim that the failure is
# understood. The file may only shrink: a listed test that starts passing fails the run until its
# line is deleted, and an unlisted test that starts failing fails the run.
#
# Format: <test path>[ (strict)] :: what went wrong
#
# Regenerate deliberately with `cargo run -p conformance -- --bless`. Do not regenerate to make a
# red build green — that is the one move this file exists to prevent.
";

#[cfg(test)]
mod tests {
    use super::*;

    /// One failing outcome, so a row can say what it is about rather than how to build one.
    fn failed(name: &str, why: &str) -> Outcome {
        Outcome {
            name: name.to_string(),
            strict: false,
            verdict: Verdict::Failed(why.to_string()),
        }
    }

    #[test]
    fn a_reason_with_a_newline_is_one_entry_and_not_an_entry_plus_rubbish() {
        // `Array/prototype/reverse/length-exceeding-integer-limit-with-proxy.js` quotes a whole
        // JavaScript array back in its failure message. The format is one entry per line, so the
        // reason was written across several — and read back as the entry plus one nonsense entry
        // per continuation line. Those then reported as tests that had started passing, because no
        // run ever produces a test named `  "Get:length",`. 36 of them made the suite un-green in a
        // way that fixing the engine could not cure.
        let outcomes = [failed(
            "reverse.js",
            "expected the traps to be [\n  \"Get:length\",\n  \"Has:0\",\n]",
        )];
        let blessed = Expectations::from_outcomes(&outcomes, None);
        let text = blessed.render();
        let reread = Expectations::parse(&text);
        // One entry, and it is the test's. Without folding the newline this is four.
        assert_eq!(reread.len(), 1);
        let judgement = reread.judge(&outcomes);
        assert!(
            judgement.fixed.is_empty(),
            "a continuation line became a test that had started passing: {:?}",
            judgement.fixed
        );
        assert!(judgement.changed.is_empty());
        assert!(judgement.regressions.is_empty());
        // A reason that really is different is still caught, folded or not.
        let moved_on = [failed(
            "reverse.js",
            "expected the traps to be [\n  \"Has:1\",",
        )];
        assert_eq!(reread.judge(&moved_on).changed.len(), 1);
    }

    #[test]
    fn a_reason_that_ends_in_a_space_does_not_report_as_changed_for_ever() {
        // A real entry: `language/comments/S7.4_A5.js` throws `'#' + uu + ' '`, so its reason ends
        // in a space — and one line of a text file cannot carry that. Written out and read back,
        // the reason came home shorter than it left, and the run reported it as *changed* against
        // itself on every run for ever. A ratchet with a line nobody can ever make green is a
        // ratchet people learn to scroll past.
        let outcomes = [failed(
            "comments.js",
            "it threw an object with no name: #0000 ",
        )];
        let blessed = Expectations::from_outcomes(&outcomes, None);
        // Blessed and judged immediately: nothing changed in between, so nothing may be reported.
        assert!(blessed.judge(&outcomes).changed.is_empty());
        // …and again through the file, which is the trip that actually loses it.
        let reread = Expectations::parse(&blessed.render());
        let judgement = reread.judge(&outcomes);
        assert!(
            judgement.changed.is_empty(),
            "a written-and-read expectation reported as changed: {:?}",
            judgement.changed
        );
        assert!(judgement.regressions.is_empty());
        assert!(judgement.fixed.is_empty());
        // A reason that really is different is still caught — the point is to stop comparing a
        // string with a trimmed copy of itself, not to stop comparing.
        let moved_on = [failed(
            "comments.js",
            "it threw an object with no name: #0001 ",
        )];
        assert_eq!(reread.judge(&moved_on).changed.len(), 1);
    }

    fn outcome(name: &str, verdict: Verdict) -> Outcome {
        Outcome {
            name: name.to_string(),
            strict: false,
            verdict,
        }
    }

    #[test]
    fn a_file_that_is_not_there_is_no_expectations_and_one_that_will_not_open_is_an_error() {
        // The first run of a new checkout has no file, and starting from nothing is right — every
        // failure is then a regression, which is exactly what `--bless` is for.
        let missing = std::env::temp_dir().join("ViperJS-conformance-no-such-file.txt");
        let _ = std::fs::remove_file(&missing);
        assert!(
            Expectations::read(&missing)
                .expect("a missing file is empty")
                .is_empty()
        ); // the test is about the value

        // Any other failure is not "no expectations". Read as an empty set, a ratchet file that
        // could not be opened would turn every recorded failure into a regression and every run
        // into a red one for a reason that is not about the engine at all. A directory is the
        // portable way to have a path that exists and cannot be read as a file.
        let directory = std::env::temp_dir().join("ViperJS-conformance-not-a-file");
        std::fs::create_dir_all(&directory).expect("a writable temp dir"); // the test needs one
        assert!(Expectations::read(&directory).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_failure_that_is_written_down_is_not_a_regression_and_one_that_is_not_is() {
        let expectations = Expectations::parse("a.js :: it threw TypeError\n");
        let judgement = expectations.judge(&[
            outcome("a.js", Verdict::Failed("it threw TypeError".to_string())),
            outcome("b.js", Verdict::Failed("it threw RangeError".to_string())),
        ]);
        assert!(judgement.fixed.is_empty());
        assert_eq!(
            judgement.regressions,
            [("b.js".to_string(), "it threw RangeError".to_string())]
        );
        assert!(!judgement.is_green());
    }

    #[test]
    fn a_listed_test_that_passes_is_as_red_as_one_that_regressed() {
        // The half people find surprising, and the half that makes this a ratchet: if a passing
        // test could stay listed, the file would only ever grow.
        let expectations = Expectations::parse("a.js :: it threw TypeError\n");
        let judgement = expectations.judge(&[outcome("a.js", Verdict::Passed)]);
        assert_eq!(judgement.fixed, ["a.js"]);
        assert!(judgement.regressions.is_empty());
        assert!(!judgement.is_green());

        // …and deleting the line is what makes it green, which is the point.
        let judgement = Expectations::default().judge(&[outcome("a.js", Verdict::Passed)]);
        assert!(judgement.is_green());
    }

    #[test]
    fn a_failure_that_changed_its_reason_is_reported_even_though_it_still_fails() {
        // The pass/fail did not move, so a name-only file would say nothing. But a test that
        // started failing differently is a different fact, and the recorded reason is now a
        // sentence about something that is no longer happening.
        let expectations = Expectations::parse("a.js :: it threw TypeError\n");
        let judgement =
            expectations.judge(&[outcome("a.js", Verdict::Failed("it threw a string".into()))]);
        assert_eq!(
            judgement.changed,
            [(
                "a.js".to_string(),
                "it threw TypeError".to_string(),
                "it threw a string".to_string()
            )]
        );
        // …and it is **reported without stopping the run**. The entry is still an entry and the
        // file has neither grown nor loosened, so this is not a ratchet violation — it is the
        // signal that a test's first gap closed and a second one is now in front of it. Gating on
        // it made green unreachable, because a handful of reasons are not deterministic: some
        // tests report which sub-case failed first, and that varies with scheduling.
        assert!(judgement.is_green());
    }

    #[test]
    fn a_line_about_a_test_that_no_longer_runs_is_reported_so_it_gets_deleted() {
        // Deleted upstream, renamed, or now skipped because the engine refuses to compile it.
        // Whatever the cause, the line is about nothing and the remedy is to remove it.
        let expectations = Expectations::parse("gone.js :: it threw TypeError\n");
        let judgement = expectations.judge(&[outcome("here.js", Verdict::Passed)]);
        assert_eq!(judgement.fixed, ["gone.js"]);
    }

    #[test]
    fn a_skipped_run_is_neither_recorded_nor_missed() {
        // A test the engine declined to run says nothing about the engine's conformance. Counted
        // as a failure it would bury the real entries; counted as a pass it would be a lie.
        let judgement = Expectations::default()
            .judge(&[outcome("a.js", Verdict::Skipped("let is M6".to_string()))]);
        assert!(judgement.is_green());
        assert!(
            Expectations::from_outcomes(
                &[outcome("a.js", Verdict::Skipped("let is M6".to_string()))],
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn the_file_round_trips_through_its_own_format() {
        let outcomes = [
            outcome("b.js", Verdict::Failed("it threw a string".to_string())),
            outcome("a.js", Verdict::Failed("it did not parse".to_string())),
            outcome("c.js", Verdict::Passed),
        ];
        let expectations = Expectations::from_outcomes(&outcomes, Some("abc123".to_string()));
        assert_eq!(expectations.len(), 2);
        let reread = Expectations::parse(&expectations.render());
        assert_eq!(reread, expectations);
        // Reading back what was written must judge the same run green, or `--bless` would leave a
        // red build behind.
        assert!(reread.judge(&outcomes).is_green());
        // The header is comments, and comments are not entries — including the one naming the
        // suite, which has to survive the round trip or a re-blessed file would forget which
        // test262 the numbers under it were measured against.
        assert!(expectations.render().starts_with('#'));
        assert_eq!(reread.suite.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_line_with_no_reason_is_kept_rather_than_dropped() {
        // Dropping it would silently un-list a test — a regression that reports as green. Kept
        // with an empty reason it reads as a changed entry on the next run and says so.
        let expectations = Expectations::parse("# a comment\n\na.js\n");
        assert_eq!(expectations.len(), 1);
        let judgement =
            expectations.judge(&[outcome("a.js", Verdict::Failed("it threw".to_string()))]);
        assert_eq!(judgement.changed.len(), 1);
    }
}
