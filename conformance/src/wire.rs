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

use crate::runner::{Outcome, Verdict};

/// The line that ends one file's block of outcomes.
///
/// A file answers with none, one or two of them — §11.2.2's two modes — so the reader cannot know
/// a block has finished by counting. It has to be told.
pub const END_OF_BLOCK: &str = ".";

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

/// The outcome a line spells, or `None` if it spells nothing.
///
/// Fallible because the other end of a pipe is not a trusted caller: a worker that dies mid-line,
/// or writes something of its own to stdout, must not be able to turn into a *wrong verdict*. A
/// line that does not decode is one the parent will report as a failure to answer, which is what
/// actually happened.
pub fn decode(line: &str) -> Option<Outcome> {
    let mut fields = line.split('\t');
    let kind = fields.next()?;
    let strict = match fields.next()? {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    let name = unescape(fields.next()?);
    // The detail is the rest, and it is the only field that may be absent — a pass has none.
    let detail = unescape(fields.next().unwrap_or(""));
    // Exactly the fields named: a line with more is a line from a protocol this is not.
    if fields.next().is_some() {
        return None;
    }
    let verdict = match kind {
        "P" => Verdict::Passed,
        "F" => Verdict::Failed(detail),
        "S" => Verdict::Skipped(detail),
        _ => return None,
    };
    Some(Outcome {
        name,
        strict,
        verdict,
    })
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
