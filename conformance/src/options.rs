//! The decisions `main` makes about what it was asked to do, separated from the doing.
//!
//! `main.rs` is orchestration and reporting: it walks a directory, supervises processes and prints.
//! None of that is usefully mutation-tested — an integration test drives the binary as a
//! *subprocess*, so a mutant reaches the binary the sandbox builds rather than the one the test
//! invokes, and the survivors it produces move between runs. What *is* testable is the handful of
//! pure decisions buried in it, and they are here so that they can be on the coverage list while
//! the printing around them is not.
//!
//! The reason this became worth doing: `main.rs` was on that list for one commit, on the strength
//! of [`value_for`] being added to it, and reported **64 survivors** — every one of them a branch
//! in `main` itself or in the report. The file was not the unit of judgement; these two functions
//! were.

/// The value belonging to a flag, or what to say about its absence.
///
/// Every value-taking flag reads through this rather than calling `next` for itself, because a bare
/// `next` fails **open** in two ways and both have been seen here:
///
/// - A missing value is `None`, and an arm that writes `option = next().map(…)` treats that as "no
///   option was asked for". `--summary` with no path wrote no badge and reported nothing wrong, so
///   three consecutive runs left a stale number on disk and looked like they had refreshed it.
/// - A value that is *itself* a flag is taken literally. `--summary --bless` writes a file called
///   `--bless` **and does not bless** — a spelling close enough to the working one to be typed by
///   accident, and one whose whole effect is silence.
///
/// Refusing a value beginning with `--` costs nothing: no path, count or filter this harness takes
/// may start that way, and a directory that did could still be reached as `./--odd`.
pub fn value_for(
    flag: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    match arguments.next() {
        Some(value) if !value.starts_with("--") => Ok(value),
        Some(flagged) => Err(format!(
            "{flag} needs a value, and {flagged} is another flag"
        )),
        None => Err(format!("{flag} needs a value")),
    }
}

/// How many workers a machine with this many hardware threads gets, unless `--workers` says.
///
/// **Half, and the halving is a measurement rather than a hunch.** One worker per thread maximises
/// throughput and is what made the per-test budget bite: each worker is a separate process running
/// JavaScript, so at full subscription every test is slower in wall-clock than the same test run
/// alone, and the ~880 `RegExp/property-escapes` runs that sit near the line cross it in whichever
/// direction the scheduler happens to go.
///
/// Measured on 2026-08-05, same commit, same machine:
///
/// | workers | newly passing | failing differently |
/// | --- | --- | --- |
/// | one per thread (32) | 264, 386, 514, 606, 788, 844 | 78 to 610 |
/// | half (16) | 890, 890, 890 | 6, 6, 6 |
///
/// Three runs at half subscription were **identical**, down to which tests. A slower run that
/// answers the same thing twice is worth more than a fast one that does not: the whole point of the
/// number is to compare it with the last one.
///
/// Never zero. A machine reporting one thread halves to none, and a run with no workers finishes
/// instantly with nothing to say — which is a green build that measured nothing.
pub const fn workers_for(threads: usize) -> usize {
    match threads / 2 {
        0 => 1,
        half => half,
    }
}

#[cfg(test)]
mod tests {
    use super::{value_for, workers_for};

    /// The arguments as the loop sees them, from the words a shell would hand over.
    fn words(text: &str) -> impl Iterator<Item = String> + use<> {
        text.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn a_flag_reads_the_word_after_it() {
        let mut arguments = words("conformance/summary.json --bless");
        assert_eq!(
            value_for("--summary", &mut arguments),
            Ok("conformance/summary.json".to_owned())
        );
        // …and leaves the rest of the line alone, which is what makes the order of flags free.
        assert_eq!(arguments.next(), Some("--bless".to_owned()));
    }

    #[test]
    fn a_flag_with_nothing_after_it_is_refused_rather_than_ignored() {
        // The failure this exists for: `--summary` alone used to set no path and say nothing, so a
        // run that was asked for a badge quietly wrote none and left whatever was on disk. Three
        // consecutive runs then agreed with each other and with a number none of them measured.
        assert_eq!(
            value_for("--summary", &mut words("")),
            Err("--summary needs a value".to_owned())
        );
    }

    #[test]
    fn a_flag_will_not_swallow_the_flag_that_follows_it() {
        // `--summary --bless` is one space away from the working spelling and its whole effect is
        // silence: a file named `--bless` appears and the expectations file is not rewritten. The
        // message names the word that was found, because the mistake is invisible in the line.
        assert_eq!(
            value_for("--summary", &mut words("--bless")),
            Err("--summary needs a value, and --bless is another flag".to_owned())
        );
        // A path is still a path when it merely *contains* dashes, and one that has to begin with
        // them can be spelled relative to here.
        for allowed in ["-x", "./--odd", "a--b", "built-ins/Array"] {
            assert_eq!(
                value_for("--only", &mut words(allowed)),
                Ok(allowed.to_owned()),
                "{allowed} is a value and not a flag"
            );
        }
    }

    #[test]
    fn a_machine_gets_half_its_threads_and_never_none() {
        // The halving, which is the measurement above.
        for (threads, expected) in [(32, 16), (16, 8), (8, 4), (5, 2), (4, 2), (3, 1), (2, 1)] {
            assert_eq!(workers_for(threads), expected, "{threads} threads");
        }
        // …and the floor, which is the part that is not arithmetic: a single-threaded machine
        // halves to zero, and a run with no workers reports 0 passed and 0 failed in no time at
        // all. That is not a slow build, it is a build that says the suite is empty.
        assert_eq!(workers_for(1), 1);
        assert_eq!(workers_for(0), 1);
    }
}
