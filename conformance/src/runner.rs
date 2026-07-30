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
    ///
    /// The error is a sentence rather than a flag because the two ways this fails need different
    /// answers from whoever reads it: a file that is not there means the checkout is wrong, and a
    /// file that is there but unreadable means the machine is. Reporting both as "missing" would
    /// send someone to re-clone a suite they already have.
    fn harness(&mut self, name: &str) -> Result<&str, String> {
        use std::collections::hash_map::Entry;

        // `entry` rather than `contains_key` then `get`: the second lookup would be a branch on a
        // key just inserted, which no test could distinguish from its absence.
        Ok(match self.harness.entry(name.to_string()) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(slot) => {
                let path = self.root.join("harness").join(name);
                let text = std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                slot.insert(text)
            }
        })
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
        // An `async` test signals that it finished by calling `$DONE`. INTERPRETING.md leaves the
        // function to the host, and this host defines it below — the prologue, because a *later*
        // definition would be the test's own and the point is that the host supplies it.
        let asynchronous = block.has("async");
        // `raw` means exactly the text and nothing else — no harness, no strict prologue. It is
        // used by tests that are *about* the prologue, so prepending one would test the reverse.
        let mut program = String::new();
        if strict {
            program.push_str("\"use strict\";\n");
        }
        if !block.has("raw") {
            // `INTERPRETING.md`: every non-raw test gets `assert.js` and `sta.js` whether it asks
            // or not, and then whatever the file named. In that order, because an include may use
            // `assert` at its own top level, and several do.
            let always = ["assert.js", "sta.js"].into_iter();
            for name in always.chain(block.includes.iter().map(String::as_str)) {
                match self.harness(name) {
                    Err(why) => return Verdict::Skipped(why),
                    Ok(text) => {
                        program.push_str(text);
                        program.push('\n');
                    }
                }
            }
        }
        // Before the test and after the includes, because `asyncHelpers.js` asks whether `$DONE`
        // is an **own property of the global object** and refuses to run if it is not — which a
        // function declaration at the top level of a script is, and nothing else here would be.
        if asynchronous {
            program.push_str(DONE);
        }
        program.push_str(source);
        evaluate(&program, block, asynchronous)
    }
}

/// The host's `$DONE`, in the terms §INTERPRETING.md gives it.
///
/// test262 ships `harness/doneprintHandle.js`, which spells the same thing in terms of a host
/// `print`. This is that with the printing taken out: praxis has no `print` and inventing one would
/// be a second host function to explain, where what is actually needed is somewhere to put one
/// word. No test in the suite includes `doneprintHandle.js` itself, so nothing is being overridden.
///
/// Three states rather than two. "Never" is the one that matters and is the reason the whole thing
/// exists: a test that *did not finish* must not be a pass, and without a third state it would be
/// indistinguishable from one that finished cleanly.
const DONE: &str = "var $__status = 'the test never called $DONE';\n     function $DONE(error) {\n       if ($__status !== 'the test never called $DONE') { return; }\n       $__status = arguments.length === 0 || error === undefined ? 'done'\n         : 'the test called $DONE with ' + String(error);\n     }\n";

/// The second script, which reads the status after §9.5's jobs have run.
const PROBE: &str = "$__status;";

/// Run a whole program and decide what its frontmatter says about the result.
fn evaluate(program: &str, block: &Frontmatter, asynchronous: bool) -> Verdict {
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
        // Nothing was thrown, which for an ordinary test is the whole answer and for an async one
        // is not an answer at all: what it says about itself is in `$DONE`, which was called — or
        // was not — while the jobs ran.
        Ok(VmOutcome::Value(_)) if asynchronous && negative.is_none() => {
            reported(&mut vm, &mut heap)
        }
        Ok(VmOutcome::Value(_)) => match negative {
            None => Verdict::Passed,
            Some(expected) => Verdict::Failed(format!(
                "expected a {} at {} time, and nothing was thrown",
                expected.kind, expected.phase
            )),
        },
        Ok(VmOutcome::Thrown(thrown)) => {
            let what = describe(thrown, &mut heap);
            let said = explain(thrown, &mut heap);
            match negative {
                // A test that must fail at *parse* time and instead threw while running has not
                // passed. The program should never have begun, and that it went wrong later is a
                // different fact about a different thing.
                Some(expected) if expected.phase == "runtime" => match what == expected.kind {
                    true => Verdict::Passed,
                    false => {
                        Verdict::Failed(format!("expected a {} and it threw {said}", expected.kind))
                    }
                },
                Some(expected) => Verdict::Failed(format!(
                    "expected a {} at {} time, and it threw {said} while running",
                    expected.kind, expected.phase
                )),
                None => Verdict::Failed(format!("it threw {said}")),
            }
        }
    }
}

/// What the test said about itself through `$DONE`.
///
/// A second script in the same realm, run after the first has finished and after §9.5's jobs have
/// run with it — which is the only moment the answer exists. DR-0016 explains why it cannot be the
/// first script's completion value: that is decided by its last statement, and a handler that calls
/// `$DONE` has not run yet at that point.
fn reported(vm: &mut Vm, heap: &mut Heap) -> Verdict {
    let Ok(script) = parse_script(PROBE) else {
        return Verdict::Failed("the harness could not read the test's status".to_string());
    };
    let Ok(chunk) = compile_script(&script, heap) else {
        return Verdict::Failed("the harness could not read the test's status".to_string());
    };
    match vm.run(&chunk, heap) {
        Ok(VmOutcome::Value(status)) => match status.to_string(heap) {
            Ok(id) if text(heap, id) == "done" => Verdict::Passed,
            Ok(id) => Verdict::Failed(text(heap, id)),
            Err(_) => Verdict::Failed("the test's status could not be read".to_string()),
        },
        _ => Verdict::Failed("the test's status could not be read".to_string()),
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

/// What was thrown, said the way a work list needs it: the kind, and what it said.
///
/// `describe` answers the *kind* alone because that is what `negative.type` names and what a
/// negative test is checked against. A failure line is read by a person deciding what to build
/// next, and seventeen thousand lines reading `it threw ReferenceError` say nothing at all —
/// where `ReferenceError: Object is not defined` names the builtin that is missing and sorts
/// itself into a bucket.
fn explain(thrown: praxis::value::Value, heap: &mut Heap) -> String {
    let kind = describe(thrown, heap);
    match message(thrown, heap) {
        Some(message) if !message.is_empty() => format!("{kind}: {message}"),
        _ => kind,
    }
}

/// An error's own `message`, if it has one that is a string.
///
/// Own rather than inherited, because §20.5.3.3 puts an empty `message` on `Error.prototype` and
/// an inherited empty string is the absence of a message rather than a message.
fn message(thrown: praxis::value::Value, heap: &mut Heap) -> Option<String> {
    use praxis::heap::{PropertyKey, PropertyKind};
    use praxis::value::Value;

    let Value::Object(object) = thrown else {
        return None;
    };
    let key = PropertyKey::from_units(heap, &"message".encode_utf16().collect::<Vec<_>>());
    let property = heap.object(object)?.get_own_property(key)?;
    match property.kind {
        PropertyKind::Data {
            value: Value::String(id),
            ..
        } => Some(text(heap, id)),
        _ => None,
    }
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
        let asynchronous = block.has("async");
        // The prologue is what `Runner::run_once` would have prepended; without it an async test
        // would be judged on a `$__status` that no one declared.
        let program = match asynchronous {
            true => format!("{DONE}{source}"),
            false => source.to_string(),
        };
        evaluate(&program, &block, asynchronous)
    }

    #[test]
    fn an_async_test_passes_only_when_it_says_so_and_never_by_finishing() {
        // The whole reason this was refused for so long. A test that *did not finish* ends without
        // throwing, exactly as one that finished cleanly does — so "nothing was thrown" cannot be
        // the verdict, and reading `$DONE` is the only thing that separates them.
        assert!(matches!(
            verdict(
                "/*---
flags: [async]
---*/
$DONE();"
            ),
            Verdict::Passed
        ));
        let never = verdict(
            "/*---
flags: [async]
---*/
var quietly = 1;",
        );
        assert!(
            matches!(&never, Verdict::Failed(why) if why.contains("never called $DONE")),
            "{never:?}"
        );
        // `$DONE(error)` is the failure report, and the reason travels into the verdict — which is
        // what makes the expectations file say something useful about an async test rather than
        // the same sentence thousands of times over.
        let failed = verdict(
            "/*---
flags: [async]
---*/
$DONE('it went wrong');",
        );
        assert!(
            matches!(&failed, Verdict::Failed(why) if why.contains("it went wrong")),
            "{failed:?}"
        );
        // `$DONE(undefined)` is a *pass*: `doneprintHandle.js` tests the argument for truthiness,
        // and `asyncHelpers.js` calls it from a `then` handler that takes one argument and is
        // handed `undefined`. Asking "was it called with anything" would fail every test that uses
        // the standard helper, which is 391 of them.
        assert!(matches!(
            verdict(
                "/*---
flags: [async]
---*/
$DONE(undefined);"
            ),
            Verdict::Passed
        ));
        // The **first** call is the answer, so a second one cannot overwrite a failure with a pass.
        let twice = verdict(
            "/*---
flags: [async]
---*/
$DONE('first'); $DONE();",
        );
        assert!(
            matches!(&twice, Verdict::Failed(why) if why.contains("first")),
            "{twice:?}"
        );
        // A test that is **not** async gets no `$DONE` at all. The host provides it for the tests
        // that report through it and for no others: a global that appeared in every test would be
        // one more thing the suite could accidentally depend on, and `asyncHelpers.js` decides
        // whether it may run by asking whether the global object has it.
        assert!(matches!(
            verdict("if (typeof $DONE !== 'undefined') { throw new Error('it was defined'); }"),
            Verdict::Passed
        ));
        // …and the answer arrives from a *job*, which is what every real async test does. This is
        // the row that fails if the status is read as the script's completion value: by then the
        // handler has not run. DR-0016 is the long version.
        assert!(matches!(
            verdict(
                "/*---
flags: [async]
---*/
Promise.resolve().then(function () { $DONE(); });"
            ),
            Verdict::Passed
        ));
    }

    /// A checkout with a hand-written harness, so that what gets prepended is observable.
    ///
    /// The real `assert.js` needs globals praxis has not built, so a test about *assembly* would
    /// be drowned by one skip reason. These stand in for it and are small enough that what each
    /// file contributes can be seen in the verdict.
    fn checkout(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("praxis-conformance-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("harness")).expect("a writable temp dir"); // the test needs one
        std::fs::create_dir_all(root.join("test")).expect("writable"); // same
        for (file, text) in [
            ("assert.js", "function assert(ok) { if (!ok) { throw 1; } }"),
            ("sta.js", "var fromSta = 1;"),
            // An include that uses `assert` at its top level, which only works if the two that
            // are always there were written out first.
            (
                "uses-assert.js",
                "assert(fromSta === 1); var fromInclude = 2;",
            ),
        ] {
            std::fs::write(root.join("harness").join(file), text).expect("writable"); // same
        }
        root
    }

    /// Write one test into a checkout and run it.
    fn run(root: &Path, source: &str) -> Vec<Outcome> {
        let file = root.join("test").join("one.js");
        std::fs::write(&file, source).expect("writable"); // the test needs the file
        Runner::new(root).run_file(&file)
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
        // A generator, because this row needs something the compiler still refuses and the example
        // has to be replaced each time one of them lands — `class C {}` was here until classes
        // compiled, at which point the row was asserting the opposite of what it says.
        assert!(matches!(
            verdict("/*---\ndescription: uses a generator\n---*/\nfunction* g() {}"),
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
    fn a_strict_run_prepends_the_prologue_and_a_sloppy_one_does_not() {
        // Observable because §12.9.3.1 keeps Annex B's legacy octal literals out of strict code
        // and B.1.1 allows them everywhere else — so the same source parses in one mode and not
        // the other, and only a prologue that is really there can make that happen.
        let root = checkout("prologue");
        let outcomes = run(&root, "/*---\ndescription: x\n---*/\nvar x = 010;");
        let strict = outcomes
            .iter()
            .find(|run| run.strict)
            .expect("a strict run"); // the test is about it
        let sloppy = outcomes
            .iter()
            .find(|run| !run.strict)
            .expect("a sloppy run"); // same
        assert!(matches!(strict.verdict, Verdict::Failed(_)), "{strict:?}");
        assert_eq!(sloppy.verdict, Verdict::Passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_this_harness_cannot_honestly_run_is_skipped_and_says_which() {
        let root = checkout("declined");
        // A module is a different goal symbol with its own scoping and `import` machinery.
        let module = run(&root, "/*---\nflags: [module]\n---*/\nassert(true);");
        assert!(matches!(&module[0].verdict, Verdict::Skipped(why) if why.contains("module")));
        // …and it is one run rather than two: a module is always strict, so there is no second
        // mode to measure.
        assert_eq!(module.len(), 1);
        assert!(!module[0].strict);

        // An async test is *run*, and what it says about itself comes from `$DONE` — which this
        // host defines in the prologue, so the test finds it on the global object as its own
        // property, which is what `asyncHelpers.js` insists on.
        let done = run(&root, "/*---\nflags: [async]\n---*/\n$DONE();");
        assert!(matches!(&done[0].verdict, Verdict::Passed), "{done:?}");

        // A test that is **not** async gets no `$DONE` at all. The host provides it for the tests
        // that report through it and for no others: a global that appeared in every test would be
        // one more thing the suite could accidentally come to depend on, and `asyncHelpers.js`
        // decides whether it may run at all by asking whether the global object has it.
        let plain = run(
            &root,
            "/*---
description: plain
---*/
             if (typeof $DONE !== 'undefined') { throw new Error('it was defined'); }",
        );
        assert!(matches!(&plain[0].verdict, Verdict::Passed), "{plain:?}");

        // Both spellings of the agent flag, because each is a separate claim about the host.
        for flag in ["CanBlockIsFalse", "CanBlockIsTrue"] {
            let outcomes = run(
                &root,
                &format!("/*---\nflags: [{flag}]\n---*/\nassert(true);"),
            );
            assert!(
                matches!(&outcomes[0].verdict, Verdict::Skipped(why) if why.contains("agents")),
                "{flag}: {outcomes:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_raw_test_is_one_run_and_it_is_the_sloppy_one() {
        // `raw` means exactly the text: no harness, and no prologue either. The tests that use it
        // are largely tests *about* the prologue, so a strict run of one would check the reverse
        // of what it was written to check.
        let root = checkout("raw");
        let outcomes = run(&root, "/*---\nflags: [raw]\n---*/\nvar x = 1;");
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].strict);
        assert_eq!(outcomes[0].verdict, Verdict::Passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_test_gets_assert_and_sta_whether_it_asked_for_them_or_not() {
        // `INTERPRETING.md` says so, and a test that named neither still uses what they define —
        // most of test262 calls `assert` without an `includes` line anywhere in the file.
        let root = checkout("always");
        let outcomes = run(
            &root,
            "/*---\ndescription: x\n---*/\nassert(fromSta === 1);",
        );
        assert!(outcomes.iter().all(|run| run.verdict == Verdict::Passed));

        // `raw` means exactly the text and nothing else. Nothing was prepended, so the name that
        // `sta.js` would have declared is a name nothing declares — and reading one of those is a
        // ReferenceError, which is how the absence is observed.
        let outcomes = run(&root, "/*---\nflags: [raw]\n---*/\nvar x = fromSta;");
        assert!(
            matches!(&outcomes[0].verdict, Verdict::Failed(why) if why.contains("ReferenceError")),
            "{:?}",
            outcomes[0].verdict
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_include_comes_after_the_two_that_are_always_there() {
        // Not a formality: an include may use `assert` at its own top level, and this one does.
        // Written out first it would be a reference to a name nothing has declared yet.
        let root = checkout("order");
        let source = "/*---\nincludes: [uses-assert.js]\n---*/\nassert(fromInclude === 2);";
        let outcomes = run(&root, source);
        assert!(outcomes.iter().all(|run| run.verdict == Verdict::Passed));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_with_neither_mode_flag_is_two_runs_and_one_with_a_flag_is_one() {
        // §11.2.2 — the same source means different things in the two modes, so a file that names
        // neither has to be measured in both or half of any disagreement goes unseen.
        let root = checkout("modes");
        let both = run(&root, "/*---\ndescription: x\n---*/\nassert(true);");
        assert_eq!(both.len(), 2);
        assert_eq!(both.iter().filter(|run| run.strict).count(), 1);

        let strict = run(&root, "/*---\nflags: [onlyStrict]\n---*/\nassert(true);");
        assert_eq!(strict.len(), 1);
        assert!(strict[0].strict);

        let sloppy = run(&root, "/*---\nflags: [noStrict]\n---*/\nassert(true);");
        assert_eq!(sloppy.len(), 1);
        assert!(!sloppy[0].strict);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_harness_file_that_is_not_there_says_which_one_and_where() {
        // "missing" alone would send someone to re-clone a suite they already have. The path is
        // what tells a wrong checkout from an unreadable one.
        let root = checkout("missing");
        let outcomes = run(&root, "/*---\nincludes: [nosuch.js]\n---*/\nassert(true);");
        let Verdict::Skipped(why) = &outcomes[0].verdict else {
            panic!("a missing include cannot be run"); // the test is about the skip
        };
        assert!(why.contains("nosuch.js"), "{why}");

        // A file that is not a test at all is not run either, and says which of the two it is.
        let file = root.join("test").join("bare.js");
        std::fs::write(&file, "var x = 1;").expect("writable"); // the test needs the file
        let outcomes = Runner::new(&root).run_file(&file);
        assert!(
            matches!(&outcomes[0].verdict, Verdict::Skipped(why) if why.contains("frontmatter"))
        );
        // Recorded as the sloppy run, because there is only one of it — a name that said
        // "(strict)" would claim a mode was measured when nothing was.
        assert!(!outcomes[0].strict);

        // …and neither is a file that cannot be read, which on every platform includes a
        // directory. Also one run, and also the sloppy one.
        let outcomes = Runner::new(&root).run_file(&root.join("test"));
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].strict);
        assert!(matches!(outcomes[0].verdict, Verdict::Skipped(_)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_test_is_named_by_its_path_below_test_slash() {
        // The name is what the expectations file keys on, so it has to be the same on a machine
        // whose checkout is somewhere else — and the same on Windows, whose paths are otherwise
        // spelled with backslashes.
        let root = checkout("naming");
        let outcomes = run(&root, "/*---\ndescription: x\n---*/\nassert(true);");
        assert_eq!(outcomes[0].name, "one.js");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failure_says_what_was_thrown_and_what_it_said() {
        // The line a person reads to decide what to build next. `it threw ReferenceError` seventeen
        // thousand times says nothing; `ReferenceError: Object is not defined` names the builtin.
        let thrown = "/*---
description: x
---*/
var e = {}; e.name = 'Weird'; e.message = 'because'; throw e;";
        assert_eq!(
            verdict(thrown),
            Verdict::Failed("it threw Weird: because".to_string())
        );
        // An *empty* message is the absence of one, not a message that is blank — so the kind
        // stands alone rather than trailing a colon with nothing after it.
        let blank = "/*---
description: x
---*/
var e = {}; e.name = 'Weird'; e.message = ''; throw e;";
        assert_eq!(
            verdict(blank),
            Verdict::Failed("it threw Weird".to_string())
        );
        // …and so is no `message` property at all, which is what §20.5.1.1 gives `new Error()`.
        let silent = "/*---
description: x
---*/
var e = {}; e.name = 'Weird'; throw e;";
        assert_eq!(
            verdict(silent),
            Verdict::Failed("it threw Weird".to_string())
        );
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
