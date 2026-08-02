//! The line protocol a worker process answers its parent in.
//!
//! Workers are separate processes rather than threads, because a thread that will not stop cannot
//! be stopped and a process can — see [`crate::drive`] for why that decision was forced. The cost
//! is that an [`Outcome`] has to cross a pipe, and this is the crossing.
//!
//! One line per outcome, tab-separated, with a `.` on its own line ending a file's block. A verdict
//! carries a *reason* written by whatever failed, so it may hold tabs and newlines and anything
//! else — which is why the fields are escaped rather than merely joined. A protocol that assumed
//! well-behaved text would corrupt exactly the outcomes that are most interesting to read.
//!
//! # Why a scenario is announced before it is run
//!
//! A worker that is killed cannot say what it was doing, and the parent has to answer for the file
//! anyway. Without an announcement the only honest thing it can say is "one run of this file did
//! not finish" — but a file is *two* runs whenever §11.2.2 gives it both modes, so a timed-out file
//! contributed one outcome where a finished one contributes two, and the size of the suite moved
//! with the number of timeouts. A change that made tests slower then read as a change that removed
//! them.
//!
//! So the child says what it is about to run before it runs any of it, and the parent keeps the
//! list. What comes back is struck off; whatever is left when the child dies is what the parent
//! reports as unfinished, by name and by mode.

use crate::runner::{Outcome, Verdict};

/// The line that ends one file's block of outcomes.
///
/// A file answers with none, one or two of them — §11.2.2's two modes — so the reader cannot know
/// a block has finished by counting. It has to be told.
pub const END_OF_BLOCK: &str = ".";

/// One thing a worker says about the file it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// "I am about to run this file in this mode" — sent for every scenario before the first runs.
    ///
    /// All of them up front rather than one before each: a child killed during the *first* run has
    /// then already said that a second was coming, and the parent can answer for both. Announced
    /// one at a time, the second would be indistinguishable from a file that never had one.
    Planned {
        /// The file, named as the expectations file names it.
        name: String,
        /// Whether this scenario prepends `"use strict"`.
        strict: bool,
    },
    /// "This scenario came to this" — the verdict itself.
    Finished(Outcome),
}

/// The plan for one scenario as a line, with no trailing newline.
pub fn encode_plan(name: &str, strict: bool) -> String {
    let strict = if strict { "1" } else { "0" };
    format!("A\t{strict}\t{}", escape(name))
}

/// One outcome as a line, with no trailing newline.
pub fn encode(outcome: &Outcome) -> String {
    let (kind, detail) = match &outcome.verdict {
        Verdict::Passed => ("P", String::new()),
        Verdict::Failed(why) => ("F", escape(why)),
        Verdict::Skipped(why) => ("S", escape(why)),
    };
    let strict = if outcome.strict { "1" } else { "0" };
    format!("{kind}\t{strict}\t{}\t{detail}", escape(&outcome.name))
}

/// What a line says, or `None` if it says nothing this protocol defines.
///
/// Fallible because the other end of a pipe is not a trusted caller: a worker that dies mid-line,
/// or writes something of its own to stdout, must not be able to turn into a *wrong verdict*. A
/// line that does not decode is one the parent will report as a failure to answer, which is what
/// actually happened.
pub fn decode(line: &str) -> Option<Message> {
    let mut fields = line.split('\t');
    let kind = fields.next()?;
    let strict = match fields.next()? {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    let name = unescape(fields.next()?);
    // The detail is the rest, and it is the only field that may be absent — a pass has none, and
    // an announcement is nothing but a name and a mode.
    let detail = unescape(fields.next().unwrap_or(""));
    // Exactly the fields named: a line with more is a line from a protocol this is not.
    if fields.next().is_some() {
        return None;
    }
    // An announcement carries no verdict, so it is answered before one is built rather than by
    // inventing a fourth `Verdict` that nothing else in the harness could ever hold.
    if kind == "A" {
        // A plan with a detail field is not a plan this wrote. Refused rather than ignored: the
        // point of the field check above is that a line means one thing, and a silently dropped
        // field is how two versions of a protocol agree while meaning different things.
        return detail
            .is_empty()
            .then_some(Message::Planned { name, strict });
    }
    let verdict = match kind {
        "P" => Verdict::Passed,
        "F" => Verdict::Failed(detail),
        "S" => Verdict::Skipped(detail),
        _ => return None,
    };
    Some(Message::Finished(Outcome {
        name,
        strict,
        verdict,
    }))
}

/// The text with every character the protocol reserves written as an escape.
///
/// Backslash first, and it has to be: escaping it after the others would go on to escape the
/// backslashes those had just introduced, and `"\n"` would arrive as `"\\n"`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// What [`escape`] was given, for anything it produced.
///
/// A trailing lone backslash, and any escape this does not know, are kept as they were written
/// rather than dropped. Neither can come from `escape`; both can come from a worker writing
/// something else to stdout, and losing characters would make a corrupt line look like a clean one.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(outcome: Outcome) {
        let line = encode(&outcome);
        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "a line must be one line: {line:?}"
        );
        let back = decode(&line).expect("what encode wrote, decode reads"); // the test is the round trip
        let Message::Finished(back) = back else {
            panic!("an encoded outcome decodes as an outcome"); // the test is the round trip
        };
        assert_eq!(back.name, outcome.name);
        assert_eq!(back.strict, outcome.strict);
        assert_eq!(
            format!("{:?}", back.verdict),
            format!("{:?}", outcome.verdict)
        );
    }

    #[test]
    fn every_verdict_survives_the_crossing_with_its_reason_intact() {
        round_trip(Outcome {
            name: "language/block-scope.js".into(),
            strict: false,
            verdict: Verdict::Passed,
        });
        round_trip(Outcome {
            name: "built-ins/Array/length.js".into(),
            strict: true,
            verdict: Verdict::Failed("it threw TypeError: no".into()),
        });
        round_trip(Outcome {
            name: "x.js".into(),
            strict: false,
            verdict: Verdict::Skipped("modules are M7".into()),
        });
    }

    #[test]
    fn an_announced_scenario_crosses_as_a_plan_and_not_as_a_verdict() {
        // The line a worker sends before it runs anything. It must arrive as a *plan*: decoded as
        // an outcome of any kind it would be counted, and a file would be tallied twice — once
        // when it was announced and once when it answered.
        let line = encode_plan("built-ins/Array/length.js", true);
        assert_eq!(
            decode(&line),
            Some(Message::Planned {
                name: "built-ins/Array/length.js".to_string(),
                strict: true,
            })
        );
    }

    #[test]
    fn a_plan_keeps_the_mode_it_was_given() {
        // Both modes, because a plan that lost the flag would strike the wrong scenario off the
        // outstanding list and leave a run unaccounted for at exactly the moment it matters.
        let Some(Message::Planned { strict, .. }) = decode(&encode_plan("a.js", false)) else {
            panic!("a plan decodes as a plan"); // the test is about the mode
        };
        assert!(!strict);
        let Some(Message::Planned { strict, .. }) = decode(&encode_plan("a.js", true)) else {
            panic!("a plan decodes as a plan"); // the test is about the mode
        };
        assert!(strict);
    }

    #[test]
    fn a_name_holding_the_separators_survives_being_announced() {
        // The same reason `encode` escapes: a plan carries a name written by the filesystem, and a
        // name with a tab in it would arrive as an extra field and be refused outright.
        let line = encode_plan("with\ttab\nand a newline.js", false);
        assert!(!line.contains('\n'));
        assert_eq!(
            decode(&line),
            Some(Message::Planned {
                name: "with\ttab\nand a newline.js".to_string(),
                strict: false,
            })
        );
    }

    #[test]
    fn a_plan_carrying_a_verdict_is_refused_rather_than_read_as_one() {
        // A plan has no detail field. A line claiming to be one *and* carrying a reason comes from
        // a protocol this is not, and reading it as a bare plan would be two versions silently
        // agreeing while meaning different things.
        assert_eq!(decode("A\t0\ta.js\tit threw"), None);
    }

    #[test]
    fn a_reason_that_holds_the_separators_arrives_as_it_was_written() {
        // The case the protocol exists for. An engine's error message is written by whatever
        // failed, and a specification quoted back in one carries newlines — so a reason with a
        // tab, a newline and a backslash in it must not become two lines or three fields.
        round_trip(Outcome {
            name: "with\ttab.js".into(),
            strict: true,
            verdict: Verdict::Failed("line one\nline two\tafter a tab\\and a backslash".into()),
        });
        // A backslash before a letter that spells an escape, which a careless unescape would
        // read as the escape rather than as the two characters they are.
        round_trip(Outcome {
            name: "a.js".into(),
            strict: false,
            verdict: Verdict::Failed("literally \\n and \\t and \\\\".into()),
        });
        round_trip(Outcome {
            name: "\r\n\t\\".into(),
            strict: false,
            verdict: Verdict::Skipped("\r\n\t\\".into()),
        });
        round_trip(Outcome {
            name: String::new(),
            strict: false,
            verdict: Verdict::Failed(String::new()),
        });
    }

    #[test]
    fn a_line_that_is_not_one_of_ours_is_refused_rather_than_guessed_at() {
        // A worker is not a trusted caller: it can die mid-line, and anything it writes to stdout
        // of its own arrives here. None of that may become a verdict — a corrupt line that decoded
        // to `Passed` would raise the conformance number with a test that never ran.
        assert!(decode("").is_none());
        assert!(decode("P").is_none());
        assert!(decode("P\t1").is_none());
        assert!(decode("X\t1\tname\t").is_none());
        assert!(decode("P\t2\tname\t").is_none());
        assert!(decode("P\tyes\tname\t").is_none());
        assert!(decode(END_OF_BLOCK).is_none());
        assert!(decode("thread 'main' panicked at src/lib.rs:1:1").is_none());
        // …and a line with a field too many, which is what a protocol that grew a column would
        // look like arriving at a reader that had not.
        assert!(decode("P\t1\tname\tdetail\textra").is_none());
        // A pass writes an empty detail and its line still has the field; one without it is
        // accepted, because the field is the only optional one and a missing reason is no reason.
        assert!(decode("P\t1\tname").is_some());
    }
}
