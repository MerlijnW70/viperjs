//! Running one test262 file, and deciding what happened.

use crate::Negative;
use crate::frontmatter::Frontmatter;
use praxis::compile::ErrorKind;
use praxis::compile::{compile_module, compile_script};
use praxis::heap::Heap;
use praxis::parser::{parse_module, parse_script};
use praxis::vm::{Graph, Outcome as VmOutcome, Vm};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

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

/// Every outcome a killed worker leaves behind — what it answered, and what it did not.
///
/// Here rather than beside the supervisor that calls it because it is a *decision* about what a
/// run came to, which is this module's subject, and because a decision is something the mutation
/// ratchet can hold: the supervisor around it needs a filesystem and a stuck child to observe at
/// all, and this needs neither.
///
/// A scenario that was announced and never answered has not passed, so it is a failure; but the
/// *reason* distinguishes the one that hung from the ones that never got a turn. Announcements are
/// in the order they will run, so the first outstanding scenario is the one the child was inside
/// when the clock ran out and the rest are waiting behind it. Reporting all of them as "did not
/// finish" would say that a file with two modes hung twice, which is not what happened.
///
/// `answered` comes back at the front, because a child that answered one mode and hung on the
/// other has *told* us the first verdict and discarding it would turn a pass into a failure.
pub fn unfinished(
    mut answered: Vec<Outcome>,
    outstanding: &[(String, bool)],
    budget: Duration,
) -> Vec<Outcome> {
    for (position, (name, strict)) in outstanding.iter().enumerate() {
        let why = match position {
            0 => format!("it did not finish within {} seconds", budget.as_secs()),
            _ => format!(
                "an earlier run of the same file did not finish within {} seconds",
                budget.as_secs()
            ),
        };
        answered.push(Outcome {
            name: name.clone(),
            strict: *strict,
            verdict: Verdict::Failed(why),
        });
    }
    answered
}

/// What running one file is going to consist of — §11.2.2's modes, settled before the first runs.
///
/// Holds the source and the frontmatter so that deciding the modes and running them is one read of
/// the file rather than two. The file is read once and may be run twice, and a file that changed
/// between the two runs would be two different tests wearing one name.
pub struct Plan {
    /// The file, relative to `test/`.
    pub name: String,
    /// Which modes this file is run in, in the order they run.
    pub modes: Vec<bool>,
    source: String,
    block: Frontmatter,
    /// The directory the file is in — §16.2.1.7's resolution base for a module's specifiers.
    ///
    /// A specifier in test262 is a relative path beside the test, and `INTERPRETING.md` says so.
    /// Kept on the plan rather than recomputed because the run has the path and the evaluation
    /// does not.
    beside: PathBuf,
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

    /// What running one file will consist of, decided before any of it runs.
    ///
    /// Separate from running it because a worker has to be able to *say* what it is about to do:
    /// a child that is killed mid-file cannot report, and the parent can only answer for the
    /// scenarios it was told to expect. See [`crate::wire`] for what goes wrong without that.
    ///
    /// A file that settles without running — unreadable, or no frontmatter — answers `Err` with
    /// the outcome that settles it. There is nothing to announce in that case and nothing that can
    /// hang, so the distinction costs the caller one `match` and buys an honest count.
    pub fn plan(&self, path: &Path) -> Result<Plan, Outcome> {
        let name = path
            .strip_prefix(self.root.join("test"))
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(source) = std::fs::read_to_string(path) else {
            return Err(Outcome {
                name,
                strict: false,
                verdict: Verdict::Skipped("the file could not be read".to_string()),
            });
        };
        let Some(block) = Frontmatter::parse(&source) else {
            return Err(Outcome {
                name,
                strict: false,
                verdict: Verdict::Skipped("no frontmatter".to_string()),
            });
        };
        // §11.2.2 — the same source means different things in the two modes, so a file that names
        // neither is two tests rather than one.
        let modes: Vec<bool> = if block.has("onlyStrict") {
            vec![true]
        } else if block.has("noStrict") || block.has("raw") || block.has("module") {
            vec![false]
        } else {
            vec![false, true]
        };
        Ok(Plan {
            name,
            source,
            block,
            modes,
            beside: path.parent().unwrap_or(&self.root).to_path_buf(),
        })
    }

    /// Run one of a plan's scenarios.
    pub fn run_planned(&mut self, plan: &Plan, strict: bool) -> Outcome {
        Outcome {
            name: plan.name.clone(),
            strict,
            verdict: self.run_once(
                &plan.source,
                &plan.block,
                strict,
                &plan.beside,
                plan.name.rsplit('/').next().map(str::to_string),
            ),
        }
    }

    /// Run one file, in both modes if its flags allow both.
    pub fn run_file(&mut self, path: &Path) -> Vec<Outcome> {
        let plan = match self.plan(path) {
            Ok(plan) => plan,
            Err(outcome) => return vec![outcome],
        };
        plan.modes
            .clone()
            .into_iter()
            .map(|strict| self.run_planned(&plan, strict))
            .collect()
    }

    /// Run one file in one mode.
    fn run_once(
        &mut self,
        source: &str,
        block: &Frontmatter,
        strict: bool,
        beside: &Path,
        beside_name: Option<String>,
    ) -> Verdict {
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
        // §INTERPRETING.md's host object, as much of it as this host can honestly answer. Before
        // the test and after the includes, so that a harness file asking for it finds it.
        if !block.has("raw") {
            program.push_str(HOST);
        }
        // Before the test and after the includes, because `asyncHelpers.js` asks whether `$DONE`
        // is an **own property of the global object** and refuses to run if it is not — which a
        // function declaration at the top level of a script is, and nothing else here would be.
        if asynchronous {
            program.push_str(DONE);
        }
        program.push_str(source);
        evaluate(&program, block, asynchronous, beside, beside_name)
    }
}

/// `$262`, with the two members this host can answer honestly.
///
/// INTERPRETING.md names seven, and providing one this engine cannot really do would be worse than
/// providing none: a test that asked for `createRealm` and got something that pretended would
/// report a failure about the wrong thing entirely. So there are two.
///
/// `detachArrayBuffer` is §25.1.5.5's `transfer`, which is the operation the host API *is* — it
/// throws the bytes away and leaves the object. Written in JavaScript rather than as a native
/// because it already exists in the language, and a second implementation of it in Rust would be a
/// second thing that could disagree with the first.
///
/// The five that are missing — `createRealm`, `evalScript`, `agent`, `gc`, `IsHTMLDDA` — are absent
/// rather than stubbed, so a test that needs one fails saying so.
const HOST: &str =
    "var $262 = { global: this,      detachArrayBuffer: function (buffer) { buffer.transfer(); } };
";

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

/// What a `SyntaxError` decided before anything ran means, given what the test expected.
///
/// §16's early errors are reported by the parser for most of the grammar and by the compiler for
/// §22.2.1's patterns, and test262 calls the phase either `parse` or `early`. Both mean "before
/// anything ran", which is the only distinction praxis draws — so both are accepted here and
/// neither is checked against the other.
fn judge_early(negative: Option<&Negative>, why: &str) -> Verdict {
    match negative {
        Some(expected) if matches!(expected.phase.as_str(), "parse" | "early") => {
            match expected.kind.as_str() {
                "SyntaxError" => Verdict::Passed,
                other => Verdict::Failed(format!(
                    "expected a {other} before anything ran, and it was a SyntaxError: {why}"
                )),
            }
        }
        Some(expected) => Verdict::Failed(format!(
            "expected a {} at {} time, and it was a SyntaxError before anything ran: {why}",
            expected.kind, expected.phase
        )),
        None => Verdict::Failed(format!("it did not parse: {why}")),
    }
}

/// What the entry module is called inside the graph.
///
/// A name no specifier can be, so it cannot collide with one a test writes. The entry is not
/// imported by anything, so nothing ever asks for it by name — it is only how the graph and the
/// evaluation agree about where to start.
const ENTRY: &str = "\u{0}entry";

/// Read, parse and compile everything `specifier` reaches, into `graph`.
///
/// §16.2.1.7's `HostLoadImportedModule`, and it is depth-first because a module's own imports are
/// only known once it has been parsed. A specifier already in the graph is one a diamond or a
/// cycle has reached before, and stopping there is what makes both terminate.
fn gather(graph: &mut Graph, from: &str, beside: &Path, heap: &mut Heap) -> Result<(), String> {
    let Some(chunk) = graph.get(from).cloned() else {
        return Ok(());
    };
    for entry in chunk.imports() {
        let specifier = &*entry.specifier;
        if graph.get(specifier).is_some() {
            continue;
        }
        let path = beside.join(specifier.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Ok(source) = std::fs::read_to_string(&path) else {
            return Err(format!("no module beside the test at {specifier:?}"));
        };
        let parsed = parse_module(&source)
            .map_err(|error| format!("an imported module did not parse: {}", error.kind))?;
        let compiled = compile_module(&parsed, heap).map_err(|error| error.message())?;
        graph.insert(specifier, Rc::new(compiled));
        gather(graph, specifier, beside, heap)?;
    }
    Ok(())
}

/// Which goal symbol a test's frontmatter asked for, with the tree it produced.
///
/// §16.1 and §16.2 are two productions and two compilers, and the difference is decided by one
/// flag on the test rather than by anything in the text — so it is carried as a value here rather
/// than by two copies of everything below.
enum Goal {
    /// §16.1's `Script`.
    Script(praxis::ast::Script),
    /// §16.2's `Module`.
    Module(praxis::ast::Module),
}

/// Run a whole program and decide what its frontmatter says about the result.
fn evaluate(
    program: &str,
    block: &Frontmatter,
    asynchronous: bool,
    beside: &Path,
    beside_name: Option<String>,
) -> Verdict {
    let negative = block.negative.as_ref();
    let mut heap = Heap::new();
    // §16.2 — a module is a different goal symbol, read and compiled by its own pair. Everything
    // after that is the same: a module throws, or does not, exactly as a script does.
    let module = block.has("module");

    let parsed = match module {
        true => parse_module(program).map(Goal::Module),
        false => parse_script(program).map(Goal::Script),
    };
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            // §16's early errors are reported by the parser, and test262 calls that phase either
            // `parse` or `early`. Both mean "before anything ran", which is what praxis's parser
            // decides — so both are accepted here and the distinction between them is not one
            // this engine draws.
            return judge_early(negative, &error.kind.to_string());
        }
    };
    // A construct praxis cannot compile is not a failure of the *test*: nothing was run, so
    // nothing can be said about what it would have done. Counting these as failures would fill
    // the expectations file with the same sentence thousands of times and hide the real ones.
    //
    // **Except one kind.** §22.2.1's early errors are decided by the compiler rather than the
    // parser — a regular expression literal's shape is read by §12.9.5 and its *pattern* only
    // afterwards — and they are as much a decision about the program as anything the parser says.
    // Skipping them let a test that expected no error at all disappear into the "not run" column
    // instead of failing, which is a hole in the ratchet rather than a kindness.
    let compiled = match &parsed {
        Goal::Script(script) => compile_script(script, &mut heap),
        Goal::Module(module) => compile_module(module, &mut heap),
    };
    let chunk = match compiled {
        Ok(chunk) => chunk,
        Err(error) if matches!(error.kind, ErrorKind::BadPattern(_)) => {
            return judge_early(negative, &error.message());
        }
        Err(error) => return Verdict::Skipped(error.message()),
    };
    let mut vm = Vm::new(&mut heap);
    // §16.2.1.7 `HostLoadImportedModule` is the host's, and this host is a directory: a specifier
    // is a path beside the test. Everything it reaches is read, parsed and compiled here, and the
    // engine is handed a graph it can link — see `praxis::vm::Graph`.
    //
    // *Every* module goes through this, including one that imports nothing: linking a graph of one
    // is what §16.2.1.6 says to do, and a second path for the empty case would be a branch whose
    // two sides must agree for ever without anything checking that they do.
    let outcome = if module {
        let mut graph = Graph::new();
        let root = Rc::new(chunk);
        graph.insert(ENTRY, Rc::clone(&root));
        // A module may import *itself* — `instn-named-bndng-cls.js` does, to watch its own binding
        // in its dead zone — so the entry answers to the specifiers that name it as well. The
        // engine keys a module by its chunk rather than by the name that reached it, so these are
        // all one record and the body runs once.
        if let Some(file) = beside_name {
            graph.insert(&file, Rc::clone(&root));
            graph.insert(&format!("./{file}"), Rc::clone(&root));
        }
        if let Err(why) = gather(&mut graph, ENTRY, beside, &mut heap) {
            return Verdict::Skipped(why);
        }
        match vm.run_module_graph(ENTRY, &graph, &mut heap) {
            Ok(Ok(outcome)) => Ok(outcome),
            // §16.2.1.5's own errors — a specifier nothing answers, a name nothing exports. A host
            // reports both as a SyntaxError, which is the phase test262 calls `resolution`.
            Ok(Err(error)) => return judge_early(negative, &error.message()),
            Err(fault) => Err(fault),
        }
    } else {
        vm.run(&chunk, &mut heap)
    };
    match outcome {
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

    /// The two modes of one file, in the order a worker announces them.
    fn both_modes(name: &str) -> Vec<(String, bool)> {
        vec![(name.to_string(), false), (name.to_string(), true)]
    }

    #[test]
    fn a_file_that_hangs_answers_for_every_mode_it_said_it_would_run() {
        // The defect this exists for. A file with both of §11.2.2's modes is two runs, and a
        // worker killed inside the first used to leave *one* outcome behind — so the size of the
        // suite fell by one for every timeout, and a change that made tests slower read as a
        // change that removed them. Both are named here, or the count is still wrong.
        let left = unfinished(Vec::new(), &both_modes("a.js"), Duration::from_secs(10));
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].key(), "a.js");
        assert_eq!(left[1].key(), "a.js (strict)");
        assert!(
            left.iter()
                .all(|run| matches!(run.verdict, Verdict::Failed(_)))
        );
    }

    #[test]
    fn the_run_that_hung_is_named_as_such_and_the_one_behind_it_is_not() {
        // Announcements are in the order they run, so the first outstanding scenario is the one
        // the child was inside and the rest never got a turn. Told the same way round, a file with
        // two modes would report that it hung twice — which is not what happened, and would double
        // every timeout in the failure buckets the work list is read from.
        let left = unfinished(Vec::new(), &both_modes("a.js"), Duration::from_secs(10));
        let Verdict::Failed(hung) = &left[0].verdict else {
            panic!("the run that did not finish is a failure"); // the test is about the reason
        };
        let Verdict::Failed(behind) = &left[1].verdict else {
            panic!("the run behind it is a failure too"); // the test is about the reason
        };
        assert_eq!(hung, "it did not finish within 10 seconds");
        assert_eq!(
            behind,
            "an earlier run of the same file did not finish within 10 seconds"
        );
    }

    #[test]
    fn what_a_dying_worker_already_answered_is_kept() {
        // A child that answered the sloppy run and then hung on the strict one has *told* us the
        // first verdict, and throwing it away would turn a pass into a failure. Only what is still
        // outstanding is answered for.
        let answered = vec![Outcome {
            name: "a.js".to_string(),
            strict: false,
            verdict: Verdict::Passed,
        }];
        let left = unfinished(
            answered,
            &[("a.js".to_string(), true)],
            Duration::from_secs(10),
        );
        assert_eq!(left.len(), 2);
        assert!(matches!(left[0].verdict, Verdict::Passed));
        assert_eq!(left[1].key(), "a.js (strict)");
    }

    #[test]
    fn a_worker_that_died_saying_nothing_leaves_no_outcome_for_the_caller_to_name_it() {
        // The one case where the parent genuinely cannot know how many runs the file was: it was
        // killed before it said. `supervise` puts one failure in rather than guessing at two, and
        // this is the function answering that it has nothing of its own to add.
        assert!(unfinished(Vec::new(), &[], Duration::from_secs(10)).is_empty());
    }

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
        // No directory: these rows are about deciding a verdict, and none of them imports.
        evaluate(&program, &block, asynchronous, Path::new("."), None)
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
        // A destructuring rest parameter, because this row needs something the compiler still
        // refuses and the example has to be replaced each time one of them lands — `class C {}`
        // was here until classes compiled, `function* g() {}` until generators did, and
        // `async function f() {}` until `await` did, at which point the row was asserting the
        // opposite of what it says.
        assert!(matches!(
            verdict(
                "/*---\ndescription: a destructuring rest parameter\n---*/\nfunction f(...[a]) {}"
            ),
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
    fn a_module_test_is_run_as_a_module_and_a_script_test_is_not() {
        // §16.1 and §16.2 are two goal symbols, and the frontmatter's one word is the whole of what
        // decides between them here. `this` is the cheapest thing that tells them apart: §16.2.1.6
        // gives a module's top level `undefined`, and §16.1.6 gives a script the global object.
        let root = checkout("goal-symbol");
        let outcomes = run(
            &root,
            "/*---
description: x
flags: [module]
---*/
if (this !== undefined) { throw 1; }",
        );
        assert!(
            outcomes.iter().all(|run| run.verdict == Verdict::Passed),
            "{outcomes:?}"
        );
        // …and the same source as a script sees the global object instead, so neither row passes by
        // the harness having only one path.
        let outcomes = run(
            &root,
            "/*---
description: x
---*/
if (this !== globalThis) { throw 1; }",
        );
        assert!(
            outcomes.iter().all(|run| run.verdict == Verdict::Passed),
            "{outcomes:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_modules_imports_are_read_from_beside_the_test() {
        // §16.2.1.7 `HostLoadImportedModule` is the host's to answer, and test262's host is a
        // directory: `import { x } from './dep.js'` means the file next to the test. Nothing in the
        // engine can find that file, so this is the harness's own half of the module goal.
        let root = checkout("beside");
        std::fs::write(
            root.join("test").join("dep.js"),
            "export var from_dep = 6; export default 7;",
        )
        .expect("writable"); // the test needs the file
        let outcomes = run(
            &root,
            "/*---
description: x
flags: [module]
---*/
             import seven, { from_dep } from './dep.js';
             if (from_dep + seven !== 13) { throw 1; }",
        );
        assert!(
            outcomes.iter().all(|run| run.verdict == Verdict::Passed),
            "{outcomes:?}"
        );
        // A specifier nothing answers is the *host* failing rather than the test, so it is skipped
        // and not counted as a failure of the engine.
        let outcomes = run(
            &root,
            "/*---
description: x
flags: [module]
---*/
import { a } from './nowhere.js';",
        );
        assert!(
            outcomes
                .iter()
                .all(|run| matches!(run.verdict, Verdict::Skipped(_))),
            "{outcomes:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_module_two_others_import_is_read_once_and_a_cycle_terminates() {
        // Depth-first over a graph, so the only thing that makes it stop is the check that a
        // specifier already gathered is not gathered again. Without it a diamond compiles a module
        // twice — two records for one file, and §16.2.1.6's "evaluated once" quietly broken — and a
        // cycle does not return at all.
        let root = checkout("diamond");
        for (file, text) in [
            (
                "shared.js",
                "globalThis.runs = (globalThis.runs || 0) + 1; export var n = 1;",
            ),
            (
                "left.js",
                "import { n } from './shared.js'; export var l = n;",
            ),
            (
                "right.js",
                "import { n } from './shared.js'; export var r = n;",
            ),
            // A cycle: each names the other, and the pair is reachable from the test.
            (
                "ping.js",
                "import { pong } from './pong.js'; export var ping = 1;",
            ),
            (
                "pong.js",
                "import { ping } from './ping.js'; export var pong = 2;",
            ),
        ] {
            std::fs::write(root.join("test").join(file), text).expect("writable"); // same
        }
        let outcomes = run(
            &root,
            "/*---
description: x
flags: [module]
---*/
             import { l } from './left.js';
             import { r } from './right.js';
             import { ping } from './ping.js';
             if (l + r !== 2 || globalThis.runs !== 1 || ping !== 1) { throw 1; }",
        );
        assert!(
            outcomes.iter().all(|run| run.verdict == Verdict::Passed),
            "{outcomes:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
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
    fn a_pattern_the_specification_forbids_is_judged_and_one_praxis_lacks_is_not() {
        let root = checkout("patterns");
        // §22.2.1's early errors are the compiler's, not the parser's, and they are still early:
        // a test asserting that a pattern must be rejected is *passed* by rejecting it.
        let forbidden = run(
            &root,
            "/*---
negative:
  phase: parse
  type: SyntaxError
---*/
var r = /(/;",
        );
        assert_eq!(forbidden[0].verdict, Verdict::Passed);
        // …and the other direction is the reason this is worth a test rather than a line: a
        // program that expected no error at all now *fails* here, where it used to disappear into
        // the "not run" column and take a real regression with it.
        let unexpected = run(
            &root,
            "/*---
description: fine
---*/
var r = /(/;",
        );
        assert!(
            matches!(&unexpected[0].verdict, Verdict::Failed(why) if why.contains("did not parse")),
            "{unexpected:?}"
        );
        // A pattern praxis has not *built* is still skipped, and that difference is the whole
        // point of the split: judging it would pass every negative test the proposal ships —
        // which asserts that particular malformed modifier groups are rejected — while every
        // positive one failed. An engine implementing none of it would be credited with half.
        let unbuilt = run(
            &root,
            "/*---
negative:
  phase: parse
  type: SyntaxError
---*/
var r = /(?i:a)/;",
        );
        assert!(
            matches!(&unbuilt[0].verdict, Verdict::Skipped(why) if why.contains("modifiers")),
            "{unbuilt:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_this_harness_cannot_honestly_run_is_skipped_and_says_which() {
        let root = checkout("declined");
        // A module is a different goal symbol, and it **runs** — read by `parse_module` and
        // compiled by `compile_module`, which is §16.2's half of the pair.
        let module = run(&root, "/*---\nflags: [module]\n---*/\nassert(true);");
        assert!(matches!(&module[0].verdict, Verdict::Passed), "{module:?}");
        // …including one that exports, which the graph the host resolves is linked through.
        let exports = run(&root, "/*---\nflags: [module]\n---*/\nexport var a = 1;");
        assert!(
            matches!(&exports[0].verdict, Verdict::Passed),
            "{exports:?}"
        );
        // What it still declines is the three forms that reach into another module's *list* rather
        // than at a name of its own — a namespace object, an `export *`, a re-export.
        let star = run(
            &root,
            "/*---\nflags: [module]\n---*/\nexport * from './a.js';",
        );
        assert!(
            matches!(&star[0].verdict, Verdict::Skipped(why) if why.contains("export")),
            "{star:?}"
        );
        // …and it is one run rather than two: a module is always strict, so there is no second
        // mode to measure.
        assert_eq!(module.len(), 1);
        assert!(!module[0].strict);

        // An async test is *run*, and what it says about itself comes from `$DONE` — which this
        // host defines in the prologue, so the test finds it on the global object as its own
        // property, which is what `asyncHelpers.js` insists on.
        let done = run(&root, "/*---\nflags: [async]\n---*/\n$DONE();");
        assert!(matches!(&done[0].verdict, Verdict::Passed), "{done:?}");

        // §INTERPRETING.md's `$262`, with the two members this host can answer. `detachArrayBuffer`
        // is what `harness/detachArrayBuffer.js` looks for, and without it every test about a
        // buffer going away reports that the *harness* is missing something rather than testing
        // what it came to test.
        let host = run(
            &root,
            "/*---\ndescription: host\n---*/\n\
             if (typeof $262.detachArrayBuffer !== 'function') { throw new Error('missing'); }\n\
             var b = new ArrayBuffer(8); $262.detachArrayBuffer(b);\n\
             if (!b.detached) { throw new Error('not detached'); }\n\
             if ($262.global !== this) { throw new Error('not the global'); }",
        );
        assert!(matches!(&host[0].verdict, Verdict::Passed), "{host:?}");

        // …and the five it cannot are *absent* rather than stubbed, so a test that needs one fails
        // saying so rather than reporting a failure about the wrong thing entirely.
        let absent = run(
            &root,
            "/*---\ndescription: absent\n---*/\n\
             if (typeof $262.createRealm !== 'undefined') { throw new Error('pretending'); }\n\
             if (typeof $262.evalScript !== 'undefined') { throw new Error('pretending'); }",
        );
        assert!(matches!(&absent[0].verdict, Verdict::Passed), "{absent:?}");

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
        // Nothing at all, which includes `$262`. "Exactly the text" is the whole meaning of the
        // flag, and a host object appearing in a test written to check what the global object has
        // would change the answer it came to check.
        let bare = run(
            &root,
            "/*---\nflags: [raw]\n---*/\n\
             if (typeof $262 !== 'undefined') { throw new Error('the host got in'); }",
        );
        assert_eq!(bare[0].verdict, Verdict::Passed, "{bare:?}");
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
