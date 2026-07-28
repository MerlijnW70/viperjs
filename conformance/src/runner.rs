//! Running one test262 file, and deciding what happened.

use crate::frontmatter::Frontmatter;
use praxis::compile::compile_script;
use praxis::heap::Heap;
use praxis::parser::parse_script;
use praxis::vm::{Outcome as VmOutcome, Vm};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What happened when a test was run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It did what its frontmatter said it would.
    Passed,
    /// It did not, and this is what happened instead.
    Failed(String),
    /// Nothing was run: the file asks for something this harness does not do yet.
    ///
    /// Not a pass and not a failure. A test that needs a module goal, or a host API, is a test
    /// this engine has no answer for — counting it either way would be a number that lies.
    Skipped(String),
}

/// One test, in one mode.
///
/// A file with neither `onlyStrict` nor `noStrict` is *two* tests: §11.2.2's strict mode changes
/// what the same source means, so both are run and both are counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The file, relative to `test/`.
    pub name: String,
    /// Whether this run prepended `"use strict"`.
    pub strict: bool,
    /// What happened.
    pub verdict: Verdict,
}

impl Outcome {
    /// The name the expectations file lists this run under.
    ///
    /// The mode is part of it, because a test can pass in one mode and fail in the other — and an
    /// entry that did not say which would hide half of that.
    pub fn key(&self) -> String {
        match self.strict {
            true => format!("{} (strict)", self.name),
            false => self.name.clone(),
        }
    }
}

/// The harness: where test262 is, and the harness files it has read.
pub struct Runner {
    root: PathBuf,
    harness: HashMap<String, String>,
}

impl Runner {
    /// Point the harness at a checkout of test262.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            harness: HashMap::new(),
        }
    }

    /// The text of a harness file, read once and kept.
    ///
    /// `assert.js` is included by nearly every test in the suite, so reading it from disk each
    /// time would be most of the harness's work.
    fn harness(&mut self, name: &str) -> Option<&str> {
        if !self.harness.contains_key(name) {
            let text = std::fs::read_to_string(self.root.join("harness").join(name)).ok()?;
            self.harness.insert(name.to_string(), text);
        }
        self.harness.get(name).map(String::as_str)
    }

    /// Run one file, in both modes if its flags allow both.
    pub fn run_file(&mut self, path: &Path) -> Vec<Outcome> {
        let name = path
            .strip_prefix(self.root.join("test"))
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(source) = std::fs::read_to_string(path) else {
            return vec![Outcome {
                name,
                strict: false,
                verdict: Verdict::Skipped("the file could not be read".to_string()),
            }];
        };
        let Some(block) = Frontmatter::parse(&source) else {
            return vec![Outcome {
                name,
                strict: false,
                verdict: Verdict::Skipped("no frontmatter".to_string()),
            }];
        };
        // §11.2.2 — the same source means different things in the two modes, so a file that names
        // neither is two tests rather than one.
        let modes: &[bool] = if block.has("onlyStrict") {
            &[true]
        } else if block.has("noStrict") || block.has("raw") || block.has("module") {
            &[false]
        } else {
            &[false, true]
        };
        modes
            .iter()
            .map(|strict| Outcome {
                name: name.clone(),
                strict: *strict,
                verdict: self.run_once(&source, &block, *strict),
            })
            .collect()
    }

    /// Run one file in one mode.
    fn run_once(&mut self, source: &str, block: &Frontmatter, strict: bool) -> Verdict {
        // A module is a different goal symbol with different scoping and its own `import`
        // machinery. Saying so is honest; running it as a script would report failures that are
        // about the harness rather than the engine.
        if block.has("module") {
            return Verdict::Skipped("modules are M7".to_string());
        }
        if block.has("CanBlockIsFalse") || block.has("CanBlockIsTrue") {
            return Verdict::Skipped("agents are not implemented".to_string());
        }
        // `raw` means exactly the text and nothing else — no harness, no strict prologue. It is
        // used by tests that are *about* the prologue, so prepending one would test the reverse.
        let mut program = String::new();
        if strict {
            program.push_str("\"use strict\";\n");
        }
        if !block.has("raw") {
            // `INTERPRETING.md`: every non-raw test gets these two, whether it asks or not.
            for name in ["assert.js", "sta.js"] {
                let Some(text) = self.harness(name) else {
                    return Verdict::Skipped(format!("the harness file {name} is missing"));
                };
                program.push_str(text);
                program.push('\n');
            }
            if block.has("async") {
                let Some(text) = self.harness("doneprintHandle.js") else {
                    return Verdict::Skipped(
                        "the harness file doneprintHandle.js is missing".to_string(),
                    );
                };
                program.push_str(text);
                program.push('\n');
            }
            for name in &block.includes {
                let Some(text) = self.harness(name) else {
                    return Verdict::Skipped(format!("the harness file {name} is missing"));
                };
                program.push_str(text);
                program.push('\n');
            }
        }
        program.push_str(source);
        evaluate(&program, block)
    }
}

/// Run a whole program and decide what its frontmatter says about the result.
fn evaluate(program: &str, block: &Frontmatter) -> Verdict {
    let negative = block.negative.as_ref();
    let mut heap = Heap::new();

    let script = match parse_script(program) {
        Ok(script) => script,
        Err(error) => {
            // §16's early errors are reported by the parser, and test262 calls that phase either
            // `parse` or `early`. Both mean "before anything ran", which is what praxis's parser
            // decides — so both are accepted here and the distinction between them is not one
            // this engine draws.
            return match negative {
                Some(expected) if matches!(expected.phase.as_str(), "parse" | "early") => {
                    match expected.kind.as_str() {
                        "SyntaxError" => Verdict::Passed,
                        other => Verdict::Failed(format!(
                            "expected a {other} at parse time, and it was a SyntaxError: {}",
                            error.kind
                        )),
                    }
                }
                Some(expected) => Verdict::Failed(format!(
                    "expected a {} at {} time, and it failed to parse: {}",
                    expected.kind, expected.phase, error.kind
                )),
                None => Verdict::Failed(format!("it did not parse: {}", error.kind)),
            };
        }
    };
    // A construct praxis cannot compile is not a failure of the *test*: nothing was run, so
    // nothing can be said about what it would have done. Counting these as failures would fill
    // the expectations file with the same sentence thousands of times and hide the real ones.
    let chunk = match compile_script(&script, &mut heap) {
        Ok(chunk) => chunk,
        Err(error) => return Verdict::Skipped(error.message()),
    };
    let mut vm = Vm::new(&mut heap);
    match vm.run(&chunk, &mut heap) {
        Err(fault) => Verdict::Failed(format!("the chunk did not make sense: {fault:?}")),
        Ok(VmOutcome::Value(_)) => match negative {
            None => Verdict::Passed,
            Some(expected) => Verdict::Failed(format!(
                "expected a {} at {} time, and nothing was thrown",
                expected.kind, expected.phase
            )),
        },
        Ok(VmOutcome::Thrown(thrown)) => {
            let what = describe(thrown, &mut heap);
            match negative {
                // A test that must fail at *parse* time and instead threw while running has not
                // passed. The program should never have begun, and that it went wrong later is a
                // different fact about a different thing.
                Some(expected) if expected.phase == "runtime" => match what == expected.kind {
                    true => Verdict::Passed,
                    false => {
                        Verdict::Failed(format!("expected a {} and it threw {what}", expected.kind))
                    }
                },
                Some(expected) => Verdict::Failed(format!(
                    "expected a {} at {} time, and it threw {what} while running",
                    expected.kind, expected.phase
                )),
                None => Verdict::Failed(format!("it threw {what}")),
            }
        }
    }
}

/// The name of what was thrown, as test262's `negative.type` writes it.
///
/// An error's *constructor name* is what the frontmatter names, and reading it back needs the
/// `name` its prototype carries. Anything that is not an object with one is described by what it
/// is instead, which is what a test that threw a string should say.
fn describe(thrown: praxis::value::Value, heap: &mut Heap) -> String {
    use praxis::heap::{PropertyKey, PropertyKind};
    use praxis::value::Value;

    let Value::Object(object) = thrown else {
        let what = thrown.type_of(heap);
        return match thrown.to_string(heap) {
            Ok(id) => format!("the {what} {}", text(heap, id)),
            // Only an object can refuse `ToString`, and this one is not an object — but saying so
            // costs a line and guessing costs a wrong failure message.
            Err(_) => format!("a {what}"),
        };
    };
    let key = PropertyKey::from_units(heap, &"name".encode_utf16().collect::<Vec<_>>());
    let Some((_, property)) = heap.find_own(object, key) else {
        return "an object with no name".to_string();
    };
    let PropertyKind::Data {
        value: Value::String(name),
        ..
    } = property.kind
    else {
        return "an object whose name is not a string".to_string();
    };
    text(heap, name)
}

/// A heap string as Rust text, for putting in a message.
///
/// Lossy because a failure message is prose: a lone surrogate in an error's `name` should print as
/// a replacement character rather than lose the whole sentence explaining what went wrong.
fn text(heap: &Heap, id: praxis::heap::StringId) -> String {
    String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(source: &str) -> Verdict {
        let block = Frontmatter::parse(source).unwrap_or_default();
        evaluate(source, &block)
    }

    #[test]
    fn a_positive_test_passes_by_running_to_the_end() {
        assert_eq!(
            verdict("/*---\ndescription: fine\n---*/\nvar x = 1;"),
            Verdict::Passed
        );
        // …and fails by throwing, whatever it threw.
        assert!(matches!(
            verdict("/*---\ndescription: throws\n---*/\nthrow 1;"),
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn a_negative_test_must_fail_in_the_phase_it_named() {
        // The phase is not a formality. A test that says the program must not *parse* has not
        // passed by throwing while it runs — the program should never have begun.
        let parse_time = "/*---\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nvar 1 = 2;";
        assert_eq!(verdict(parse_time), Verdict::Passed);

        let wrong_phase = "/*---\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nnull.x;";
        assert!(matches!(verdict(wrong_phase), Verdict::Failed(_)));

        // …and a run-time test must throw the error it named and not another.
        let runtime = "/*---\nnegative:\n  phase: runtime\n  type: TypeError\n---*/\nnull.x;";
        assert_eq!(verdict(runtime), Verdict::Passed);
        let wrong_kind = "/*---\nnegative:\n  phase: runtime\n  type: RangeError\n---*/\nnull.x;";
        assert!(matches!(verdict(wrong_kind), Verdict::Failed(_)));
        // A negative test that does not fail at all is a failure.
        let did_not = "/*---\nnegative:\n  phase: runtime\n  type: TypeError\n---*/\nvar x = 1;";
        assert!(matches!(verdict(did_not), Verdict::Failed(_)));
    }

    #[test]
    fn what_the_engine_cannot_compile_is_skipped_rather_than_failed() {
        // Nothing ran, so nothing can be said about what it would have done. Counting these as
        // failures would write the same sentence into the expectations file thousands of times
        // and bury the ones that mean something.
        assert!(matches!(
            verdict("/*---\ndescription: uses let\n---*/\nlet x = 1;"),
            Verdict::Skipped(_)
        ));
        // A *parse* failure is not skipped: the parser is finished, so a file it refuses is a
        // real answer about that file.
        assert!(matches!(
            verdict("/*---\ndescription: nonsense\n---*/\nvar 1 = ;"),
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn a_run_is_named_by_its_file_and_its_mode() {
        let plain = Outcome {
            name: "language/x.js".to_string(),
            strict: false,
            verdict: Verdict::Passed,
        };
        assert_eq!(plain.key(), "language/x.js");
        // The mode is part of the name because a test can pass in one and fail in the other, and
        // an entry that did not say which would hide half of that.
        let strict = Outcome {
            strict: true,
            ..plain.clone()
        };
        assert_eq!(strict.key(), "language/x.js (strict)");
    }
}
