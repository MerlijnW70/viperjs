//! Finding the tests, and running them all at once.
//!
//! # Why a test that hangs does not hang the run, and why a worker is a process
//!
//! `while (true);` is a legal program and test262 contains loops that a slow engine takes a long
//! time over. Rust cannot stop a thread that will not stop, so the first version of this ran
//! workers as threads and *abandoned* one that outran its budget: its test was recorded as timed
//! out and a replacement took over the queue.
//!
//! Abandoning does not scale, and the cost was measured rather than reasoned about. An abandoned
//! worker goes on running for the rest of the run — holding its heap and its core — and its
//! replacement can be abandoned in turn. One run reached **60 GB across roughly 250 abandoned
//! workers** and died of an allocation failure without producing a report at all, so a handful of
//! unbounded tests took the whole conformance number with them.
//!
//! So a worker is a child process. A process can be killed, killing one gives back its memory and
//! its core, and nothing is left running afterwards to accumulate. The cost is that an outcome has
//! to cross a pipe, which is what [`crate::wire`] is for.

use crate::runner::{Outcome, Runner, Verdict};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

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

/// Run every file, `workers` at a time, killing any worker that outruns `budget`.
pub fn run_all(root: &Path, files: &[PathBuf], workers: usize, budget: Duration) -> Report {
    let files: Arc<Vec<PathBuf>> = Arc::new(files.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel::<Vec<Outcome>>();

    let mut supervisors = Vec::new();
    for _ in 0..workers.max(1) {
        let (root, files, next) = (root.to_path_buf(), Arc::clone(&files), Arc::clone(&next));
        let sender = sender.clone();
        // Joinable, unlike what came before. A supervisor never waits without a deadline — every
        // wait it makes is a `recv_timeout`, and the answer to running out of time is to kill the
        // child rather than to wait longer. So it always ends, and the run can join it.
        supervisors.push(std::thread::spawn(move || {
            supervise(&root, &files, &next, &sender, budget);
        }));
    }
    // The loop below ends when every sender is gone, so the one held here has to go first.
    drop(sender);

    let mut report = Report::default();
    for outcomes in receiver {
        report.outcomes.extend(outcomes);
    }
    for supervisor in supervisors {
        // A supervisor that panicked loses its share of the queue and nothing else — the report
        // says what was collected. Joining is what makes sure no child outlives the run.
        let _ = supervisor.join();
    }
    report
}

/// Take tests from the queue and put them through a child, replacing the child when it stops.
fn supervise(
    root: &Path,
    files: &Arc<Vec<PathBuf>>,
    next: &Arc<AtomicUsize>,
    sender: &mpsc::Sender<Vec<Outcome>>,
    budget: Duration,
) {
    let Some(mut worker) = Worker::start(root) else {
        return;
    };
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(file) = files.get(index) else {
            return;
        };
        let outcomes = match worker.run(file, budget) {
            Some(outcomes) => outcomes,
            // The child did not answer in time, or lost the protocol. Either way it has been
            // killed, and the test it was on is recorded as the failure it is: a test that does
            // not finish has not passed, and saying so is what keeps one hang from quietly
            // shrinking the suite.
            None => {
                let name = name_of(root, file);
                match Worker::start(root) {
                    Some(fresh) => worker = fresh,
                    None => return,
                }
                vec![Outcome {
                    name,
                    strict: false,
                    verdict: Verdict::Failed(format!(
                        "it did not finish within {} seconds",
                        budget.as_secs()
                    )),
                }]
            }
        };
        if sender.send(outcomes).is_err() {
            return;
        }
    }
}

/// One child process, and the thread reading what it says.
struct Worker {
    child: std::process::Child,
    /// Lines the child has written, as they arrive.
    ///
    /// A channel rather than reading the pipe directly, because a read from a pipe has no
    /// deadline: a child that says nothing would block its supervisor exactly as a hung thread
    /// used to block the run. The reader ends when the pipe closes, which killing the child does.
    lines: mpsc::Receiver<String>,
}

impl Worker {
    /// Start a child of this same program, in the mode that runs tests and says what happened.
    fn start(root: &Path) -> Option<Self> {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().ok()?)
            .arg(WORKER_FLAG)
            .arg("--test262")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited deliberately: a panic message from a child belongs on the run's stderr,
            // where whoever is watching can see it, rather than mixed into the protocol.
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Some(Self { child, lines })
    }

    /// Put one file through the child, or answer `None` if it did not come back in time.
    ///
    /// `None` leaves the child killed. There is no half-way state worth recovering: a worker that
    /// missed its deadline may be in a loop that never ends, and the only thing to do with one of
    /// those is stop it.
    fn run(&mut self, file: &Path, budget: Duration) -> Option<Vec<Outcome>> {
        use std::io::Write;
        // A path with a newline in it would be two paths by the time it arrived. No checkout has
        // one, which is exactly why it is worth refusing rather than trusting.
        let path = file.to_string_lossy().replace(['\n', '\r'], "");
        let asked = self.child.stdin.as_mut().map(|input| {
            writeln!(input, "{path}")?;
            input.flush()
        });
        if !matches!(asked, Some(Ok(()))) {
            self.kill();
            return None;
        }
        let mut outcomes = Vec::new();
        loop {
            // The budget is against each *line*, and so against the test: a child that is
            // answering is a child that is working, and one that has stopped answering is the
            // case this exists for.
            let Ok(line) = self.lines.recv_timeout(budget) else {
                self.kill();
                return None;
            };
            if line == crate::wire::END_OF_BLOCK {
                return Some(outcomes);
            }
            match crate::wire::decode(&line) {
                Some(outcome) => outcomes.push(outcome),
                // Anything else on stdout is not a verdict and must not become one — a corrupt
                // line decoded as a pass would raise the number with a test that never ran.
                None => {
                    self.kill();
                    return None;
                }
            }
        }
    }

    /// Stop the child and wait for it, so that nothing of it outlives this call.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    /// A supervisor that ends — because the queue is empty, or because it panicked — takes its
    /// child with it. Without this, finishing the run would leave one child per worker alive.
    fn drop(&mut self) {
        self.kill();
    }
}

/// The argument that puts this program in the mode a worker runs in.
///
/// Not in the usage text: it is how the program talks to itself, and an option nobody should pass
/// is an option nobody needs to be told about.
pub const WORKER_FLAG: &str = "--worker";

/// Run what arrives on standard input, a path to a line, and say what happened.
///
/// The other half of the worker pool above. Reading paths rather than taking one and exiting
/// keeps a process per *worker* rather than per test — forty-eight thousand spawns would cost
/// minutes of nothing but starting up — while still leaving each one killable at any moment.
pub fn work(root: &Path) {
    use std::io::{BufRead, Write};
    let mut runner = Runner::new(root);
    let input = std::io::stdin();
    let output = std::io::stdout();
    let mut writer = std::io::BufWriter::new(output.lock());
    for line in input.lock().lines().map_while(Result::ok) {
        for outcome in runner.run_file(Path::new(&line)) {
            if writeln!(writer, "{}", crate::wire::encode(&outcome)).is_err() {
                return;
            }
        }
        // Flushed here rather than left to the buffer: the parent is waiting on lines, and a
        // finished block still sitting in this writer is a worker that looks hung while it is in
        // fact done.
        if writeln!(writer, "{}", crate::wire::END_OF_BLOCK).is_err() || writer.flush().is_err() {
            return;
        }
    }
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
