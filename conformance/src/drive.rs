//! Finding the tests, and running them all at once.
//!
//! # Why a test that hangs does not hang the run
//!
//! `while (true);` is a legal program and test262 contains loops that a slow engine takes a long
//! time over. Rust cannot stop a thread that will not stop, so a worker that runs past its budget
//! is *abandoned*: its test is recorded as having timed out, a replacement worker takes over the
//! queue, and the old thread is left to run until the process ends. That is why the run finishes
//! with `exit` rather than by joining — there may be threads that never join, and waiting for them
//! would be waiting forever for an answer already recorded.

use crate::runner::{Outcome, Runner, Verdict};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Everything a run came to.
#[derive(Debug, Default)]
pub struct Report {
    /// Every run, in the order they finished.
    pub outcomes: Vec<Outcome>,
}

impl Report {
    /// Why runs were not run, commonest first.
    ///
    /// This is the list M5 exists to produce. A skip reason with forty thousand runs behind it is
    /// the next thing to build and one with four is not — and without this, the only visible
    /// number is a percentage of whatever the engine happened to be able to attempt.
    pub fn skips(&self) -> Vec<(&str, usize)> {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for outcome in &self.outcomes {
            if let Verdict::Skipped(why) = &outcome.verdict {
                *counts.entry(why.as_str()).or_default() += 1;
            }
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        // By count, and then by name, so two runs of the same suite print the same order rather
        // than whichever of two equal buckets the map happened to yield first.
        counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
        counts
    }

    /// How many runs came to each verdict.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut passed = 0;
        let mut failed = 0;
        let mut skipped = 0;
        for outcome in &self.outcomes {
            match outcome.verdict {
                Verdict::Passed => passed += 1,
                Verdict::Failed(_) => failed += 1,
                Verdict::Skipped(_) => skipped += 1,
            }
        }
        (passed, failed, skipped)
    }
}

/// Whether a path under `test/` is a test this harness should run.
///
/// Three things there are not tests praxis can be measured against:
///
/// - `staging/` — proposals that have not landed. Not normative, and failing them says nothing.
/// - `intl402/` — ECMA-402, a different specification. praxis implements ECMA-262.
/// - `_FIXTURE.js` — imported *by* module tests rather than run. `INTERPRETING.md` names them.
pub fn is_test(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if !name.ends_with(".js") || name.ends_with("_FIXTURE.js") {
        return false;
    }
    let full = path.to_string_lossy().replace('\\', "/");
    !full.contains("/staging/") && !full.contains("/intl402/")
}

/// Every test under `test/`, sorted so that two runs list their failures in the same order.
pub fn find_tests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.join("test")];
    // Iterative rather than recursive: the tree is deep enough that this is a habit worth
    // keeping, and DR-0002 does not allow input to decide how much Rust stack we use.
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match path.is_dir() {
                true => pending.push(path),
                false if is_test(&path) => found.push(path),
                false => {}
            }
        }
    }
    found.sort();
    found
}

/// Which commit of test262 this checkout is at, if it can be told.
///
/// Read out of `.git` rather than asked of `git`, because a harness that needed a program on the
/// path to answer would answer differently on the machine that has one. `None` for a checkout with
/// no `.git` at all — a tarball is a perfectly good way to have the suite, and the run is still a
/// run; it is only a run whose revision nobody can name.
pub fn suite_revision(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    // A detached HEAD holds the hash outright; the usual case holds `ref: refs/heads/<branch>`.
    let Some(reference) = head.strip_prefix("ref: ") else {
        return Some(head.to_string());
    };
    if let Ok(loose) = std::fs::read_to_string(git.join(reference)) {
        return Some(loose.trim().to_string());
    }
    // A repository that has been packed keeps no loose file for the branch, so the one place left
    // to look is the table of packed references.
    let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (hash, name) = line.split_once(' ')?;
        (name.trim() == reference).then(|| hash.to_string())
    })
}

/// What a worker is doing, so that the main thread can notice it has stopped doing it.
type Watch = Arc<Mutex<Option<(usize, Instant)>>>;

/// Run every file, `workers` at a time, giving each `budget` before it is abandoned.
pub fn run_all(root: &Path, files: &[PathBuf], workers: usize, budget: Duration) -> Report {
    let files: Arc<Vec<PathBuf>> = Arc::new(files.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<(usize, Vec<Outcome>)>();

    let mut watches: Vec<Watch> = Vec::new();
    for _ in 0..workers.max(1) {
        watches.push(spawn(root, &files, &next, &sender));
    }
    // The main thread's own sender would keep the channel open forever if a worker's copy were
    // the only other one; dropping it is what lets a `recv_timeout` mean what it says.
    drop(sender);

    let mut report = Report::default();
    let mut accounted = vec![false; files.len()];
    let mut done = 0;
    while done < files.len() {
        // Short enough that an abandoned worker is replaced promptly, long enough that the main
        // thread is not spinning while thousands of tests run.
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok((index, outcomes)) => {
                // An abandoned worker may still finish and report. Its test already has an
                // answer, and a second one would be counted twice.
                if !std::mem::replace(&mut accounted[index], true) {
                    report.outcomes.extend(outcomes);
                    done += 1;
                }
            }
            // Nothing arrived, which is the normal case while long tests run and also exactly
            // what a hung worker looks like. Which it is, is what the watches say.
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Every sender is gone with work outstanding: every worker died. Nothing more will
            // arrive, so reporting what there is beats waiting for what there is not.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for watch in &mut watches {
            let overdue = watch
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .filter(|(_, since)| since.elapsed() > budget);
            let Some((index, _)) = overdue else {
                continue;
            };
            if !std::mem::replace(&mut accounted[index], true) {
                report.outcomes.push(Outcome {
                    name: name_of(root, &files[index]),
                    strict: false,
                    verdict: Verdict::Failed(format!(
                        "it did not finish within {} seconds",
                        budget.as_secs()
                    )),
                });
                done += 1;
            }
            // The thread is still running and cannot be stopped. Replacing it keeps the queue
            // moving; the old one is left to end when the process does.
            let (again, sender) = respawn(root, &files, &next);
            *watch = again;
            drop(sender);
        }
    }
    report
}

/// The queue plus a fresh worker on it.
fn spawn(
    root: &Path,
    files: &Arc<Vec<PathBuf>>,
    next: &Arc<AtomicUsize>,
    sender: &mpsc::Sender<(usize, Vec<Outcome>)>,
) -> Watch {
    let watch: Watch = Arc::new(Mutex::new(None));
    let (root, files, next) = (root.to_path_buf(), Arc::clone(files), Arc::clone(next));
    let (mine, sender) = (Arc::clone(&watch), sender.clone());
    // Detached deliberately — see the module comment. A worker that hangs is never joined.
    std::thread::spawn(move || {
        let mut runner = Runner::new(&root);
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(file) = files.get(index) else {
                return;
            };
            if let Ok(mut slot) = mine.lock() {
                *slot = Some((index, Instant::now()));
            }
            let outcomes = runner.run_file(file);
            if let Ok(mut slot) = mine.lock() {
                *slot = None;
            }
            if sender.send((index, outcomes)).is_err() {
                return;
            }
        }
    });
    watch
}

/// A replacement worker, which needs its own sender because the original was dropped.
///
/// The sender is handed back rather than kept so that the count of live senders still falls to
/// zero when the last worker ends — which is what makes a disconnected channel mean "everyone is
/// gone" rather than "everyone but the spare".
fn respawn(
    root: &Path,
    files: &Arc<Vec<PathBuf>>,
    next: &Arc<AtomicUsize>,
) -> (Watch, mpsc::Sender<(usize, Vec<Outcome>)>) {
    let (sender, _) = mpsc::channel();
    let watch = spawn(root, files, next, &sender);
    (watch, sender)
}

/// A file's name as the expectations file writes it.
fn name_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root.join("test"))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_not_a_test_is_not_run() {
        assert!(is_test(Path::new("test/language/block-scope.js")));
        // `staging/` is proposals that have not landed, so failing them says nothing about
        // conformance to the specification praxis implements.
        assert!(!is_test(Path::new("test/staging/sm/anything.js")));
        // ECMA-402 is a different specification.
        assert!(!is_test(Path::new("test/intl402/DateTimeFormat/x.js")));
        // A fixture is imported by a module test rather than run on its own.
        assert!(!is_test(Path::new(
            "test/language/module-code/x_FIXTURE.js"
        )));
        assert!(!is_test(Path::new("test/language/README.md")));
        // Windows writes the same paths with backslashes, and a harness that only excluded the
        // forward-slash spelling would run `staging/` on one platform and not the other.
        assert!(!is_test(Path::new(r"test\staging\sm\anything.js")));
    }

    #[test]
    fn a_run_counts_each_verdict_once() {
        let report = Report {
            outcomes: vec![
                Outcome {
                    name: "a.js".to_string(),
                    strict: false,
                    verdict: Verdict::Passed,
                },
                Outcome {
                    name: "a.js".to_string(),
                    strict: true,
                    verdict: Verdict::Failed("it threw".to_string()),
                },
                Outcome {
                    name: "b.js".to_string(),
                    strict: false,
                    verdict: Verdict::Skipped("let is M6".to_string()),
                },
            ],
        };
        assert_eq!(report.tally(), (1, 1, 1));
        assert_eq!(Report::default().tally(), (0, 0, 0));
    }

    #[test]
    fn the_suite_revision_is_read_out_of_git_in_each_of_the_shapes_it_takes() {
        let root = std::env::temp_dir().join("praxis-conformance-revision");
        let git = root.join(".git");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(git.join("refs/heads")).expect("a writable temp dir"); // the test needs one
        // A checkout with no `.git` is still a checkout — a tarball is a fine way to have the
        // suite. The revision is unknown, not zero, and saying so is the honest answer.
        assert_eq!(suite_revision(Path::new("no/such/checkout")), None);

        // The usual case: HEAD names a branch and the branch is a loose file.
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").expect("writable"); // same
        std::fs::write(git.join("refs/heads/main"), "abc123\n").expect("writable"); // same
        assert_eq!(suite_revision(&root), Some("abc123".to_string()));

        // A packed repository keeps no loose file, so the table is the only place left.
        std::fs::remove_file(git.join("refs/heads/main")).expect("it is there"); // same
        std::fs::write(
            git.join("packed-refs"),
            "# pack-refs with: peeled\ndef456 refs/heads/main\n",
        )
        .expect("writable"); // same
        assert_eq!(suite_revision(&root), Some("def456".to_string()));

        // A detached HEAD holds the hash outright.
        std::fs::write(git.join("HEAD"), "999fff\n").expect("writable"); // same
        assert_eq!(suite_revision(&root), Some("999fff".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_checkout_yields_no_tests_rather_than_failing() {
        // The harness is run by people who may not have cloned test262 yet, and `main` says so in
        // a sentence. Walking a directory that is not there is not an error worth propagating.
        assert!(find_tests(Path::new("no/such/checkout")).is_empty());
    }
}
