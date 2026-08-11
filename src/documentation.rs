//! Two things about the comments that no compiler checks, checked here.
//!
//! # Why this is a test and not a convention
//!
//! `cargo doc` with `-D warnings` resolves every intra-doc link and refuses a broken one, which
//! makes `[`Compiler::binding`]` in a `///` comment as safe as the code beside it. It says nothing
//! at all about two other things, and this repository has drifted on both:
//!
//! - A name written in a plain `//` comment, or in backticks without brackets. Rustdoc never sees
//!   those, so a doc comment naming a function two commits after it was deleted still builds and
//!   still reads as authoritative. Two were found by hand — one of them a bracketed link that
//!   happened to sit in a `//` comment, where it looks checked and is not.
//! - A module doc that *lists* the module's parts. Seven of them had fallen behind the `mod`
//!   declarations beside them, one by fifteen entries, and the compiler is happy to add a file
//!   without mentioning it in the map a reader uses to find anything.
//!
//! Both are the same failure: prose that describes the code, sitting where nothing compares the
//! two. A convention would have to be remembered; this fails the build.
//!
//! # What it deliberately does not check
//!
//! Whether a comment is *true*. Nothing here can tell that a sentence about §14.7.5.7 still
//! describes what the function does — see the `for_in_parts` comment that claimed a refusal for
//! two commits after the refusal was implemented. That remains a reading problem, and the only
//! defence is the habit of reading the comment when changing the code under it.
//!
//! The most expensive instance so far is worth naming, because it shows what the class costs at
//! full size. DR-0019 gave the arena a free list and generation-tagged handles; six comments across
//! four files went on saying a swept slot is *never* reused, and one of them —
//! [`crate::heap::collect`]'s module doc — said so under the heading "why there is still none".
//! Two were load-bearing arguments rather than description: `Heap::define_own_property` justified
//! an unreachable branch with "an arena only grows", and [`crate::heap::weak_ref`] explained
//! `deref`'s soundness by the same sentence — so a reader who believed the comment would have
//! concluded that a correct function was a use-after-free. Every one of them compiled, linked and
//! passed every test. **A comment that states a rule is the kind that stops the next reader
//! checking**, which is exactly why it is worth suspecting one that reads well.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under the crate's `src/`, with its contents.
fn sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|kind| kind == "rs")
                && let Ok(text) = fs::read_to_string(&path)
            {
                found.push((path, text));
            }
        }
    }
    found
}

/// The path as the repository writes it, for a failure message somebody can act on.
fn shown(path: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// §6.1.6.1 and §6.1.6.2's abstract operations on the two numeric types.
///
/// The specification spells these `Number::add` and `BigInt::equal` — with `::`, exactly as Rust
/// spells an associated function — so a comment citing one looks to any scanner like a mention of
/// code. They are exempt, and the list is closed: §6.1.6 defines these operations for both types
/// and no others, so this cannot fall behind the way a list of *our* names would.
const NUMERIC_OPERATIONS: [&str; 19] = [
    "unaryMinus",
    "bitwiseNOT",
    "exponentiate",
    "multiply",
    "divide",
    "remainder",
    "add",
    "subtract",
    "leftShift",
    "signedRightShift",
    "unsignedRightShift",
    "lessThan",
    "equal",
    "sameValue",
    "sameValueZero",
    "bitwiseAND",
    "bitwiseXOR",
    "bitwiseOR",
    "toString",
];

/// Whether `owner::member` is one of those rather than a name in this crate.
fn is_numeric_operation(owner: &str, member: &str) -> bool {
    matches!(owner, "Number" | "BigInt") && NUMERIC_OPERATIONS.contains(&member)
}

/// Every identifier this crate defines: functions, fields, variants, types, constants.
///
/// Deliberately one flat set rather than a map from owner to members. What is being caught is a
/// name that exists **nowhere**, which is what a deleted function looks like; asking whether a
/// member belongs to the type the comment named would need to resolve `use` and `impl` blocks, and
/// a checker that is nearly a compiler is one that will disagree with the compiler.
fn defined(sources: &[(PathBuf, String)]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (_, text) in sources {
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            let mut rest = trimmed;
            // `fn name`, `struct Name`, `const NAME` — the keyword and the word after it.
            for keyword in [
                "fn ", "struct ", "enum ", "trait ", "type ", "const ", "static ",
            ] {
                if let Some(at) = rest.find(keyword) {
                    let after = &rest[at + keyword.len()..];
                    let word: String = after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !word.is_empty() {
                        names.insert(word);
                    }
                }
            }
            // A field or a variant: the first word on the line, when a `:`, `(`, `{` or `,`
            // follows it. Loose on purpose — this set is used to say a name *exists*, so a false
            // entry costs a missed report and never a false alarm.
            //
            // The visibility comes off first. Without that, `pub(super) outer_arguments: bool`
            // contributes the word `pub` and not the field, so every mention of a non-private
            // field reads as dangling — which is what this test reported about itself on the first
            // run, and is why the extractor is written to be re-read rather than trusted.
            for visibility in ["pub(crate) ", "pub(super) ", "pub(self) ", "pub ", "mut "] {
                if let Some(shorter) = rest.strip_prefix(visibility) {
                    rest = shorter.trim_start();
                }
            }
            let word: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !word.is_empty() {
                rest = &rest[word.len()..];
                if rest.starts_with([':', '(', '{', ',']) {
                    names.insert(word);
                }
            }
        }
    }
    names
}

/// The comment lines of `text` that are prose, paired with their line numbers.
///
/// **A doc comment's fenced block is code, not prose**, and it is code `cargo test --doc` compiles
/// and runs — so a name inside one is already checked by the only thing that can check it properly.
/// Reading it as prose reports every standard-library call an example makes as drift, which is a
/// false alarm about the one kind of documentation that cannot rot silently.
///
/// Fences are counted rather than matched, so an unterminated one hides the rest of a file's
/// comments instead of reporting them as code. That is the safe direction for a checker whose false
/// entries cost a missed report and never a false alarm.
fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut fenced = false;
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            continue;
        }
        // The marker sits after `///`, `//!` or `//`, and may be indented inside the comment.
        let body = trimmed
            .trim_start_matches('/')
            .trim_start_matches('!')
            .trim_start();
        if body.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            lines.push((number, line));
        }
    }
    lines
}

/// Each type-and-member pair a comment line names, with or without brackets and backticks.
fn mentions(line: &str) -> Vec<(String, String)> {
    let bytes: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != ':' || at + 1 >= bytes.len() || bytes[at + 1] != ':' {
            at += 1;
            continue;
        }
        // Backwards for the owner, which must start with a capital to be a type.
        let mut start = at;
        while start > 0 && (bytes[start - 1].is_alphanumeric() || bytes[start - 1] == '_') {
            start -= 1;
        }
        let owner: String = bytes[start..at].iter().collect();
        // Forwards for the member.
        let mut end = at + 2;
        while end < bytes.len() && (bytes[end].is_alphanumeric() || bytes[end] == '_') {
            end += 1;
        }
        let member: String = bytes[at + 2..end].iter().collect();
        at = end.max(at + 2);
        if owner.is_empty() || member.is_empty() {
            continue;
        }
        if !owner.starts_with(|c: char| c.is_uppercase()) {
            continue;
        }
        // A path rather than a mention: `crate::heap::Heap` ends at a type, and the segment before
        // it is a module. Only the last `::` in a run is a member reference.
        if member.starts_with(|c: char| c.is_uppercase()) {
            continue;
        }
        found.push((owner, member));
    }
    found
}

#[test]
fn a_fenced_block_in_a_doc_comment_is_code_and_is_not_read_as_prose() {
    // The false alarm this exists to stop: an example is compiled and run by `cargo test --doc`,
    // so the names in it are checked by the compiler. Reading them here reports every call an
    // example makes into the standard library — or into a caller's own crate — as drift.
    // Joined rather than written as one literal: a source line that *begins* with `//` is a comment
    // to the scan above, whatever quotes are around it, so a fixture written the readable way is
    // read as this file's own documentation and reported as drift. Found by this test failing on
    // itself, which is the second time the checker has caught its own fixture.
    let text = [
        "/// Does a thing.",
        "///",
        "/// ```",
        "/// let d = Nowhere::missing(50);",
        "/// ```",
        "///",
        "/// See Also::gone for the rest.",
        "fn thing() {}",
    ]
    .join("\n");
    let prose: Vec<&str> = prose_lines(&text)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    assert!(
        prose.iter().any(|line| line.contains("Also::gone")),
        "prose outside the fence is still read: {prose:?}"
    );
    assert!(
        !prose.iter().any(|line| line.contains("Nowhere::missing")),
        "the fenced line was read as prose: {prose:?}"
    );
    // The fence markers themselves are not prose either, so a bare ``` cannot be mistaken for a
    // line to inspect.
    assert!(!prose.iter().any(|line| line.contains("```")));
    // And the line numbers are the file's, not the filtered list's — a report that pointed at the
    // wrong line would send someone to a comment that is fine.
    let numbered = prose_lines(&text);
    let (number, _) = numbered
        .iter()
        .find(|(_, line)| line.contains("Also::gone"))
        .expect("it is there");
    assert_eq!(*number, 6);
}

#[test]
fn no_comment_names_a_function_or_field_that_does_not_exist() {
    let sources = sources();
    let defined = defined(&sources);
    let mut dangling = Vec::new();
    for (path, text) in &sources {
        for (number, line) in prose_lines(text) {
            for (owner, member) in mentions(line) {
                if defined.contains(&member) || is_numeric_operation(&owner, &member) {
                    continue;
                }
                dangling.push(format!(
                    "{}:{}: `{owner}::{member}` names nothing this crate defines",
                    shown(path),
                    number + 1
                ));
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "a comment names something that is not there — the code moved and the prose did not:\n{}\n\
         Fix the comment. If the name is a specification operation rather than one of ours, say so \
         in words instead of in `::`, which is how a reader tells the two apart.",
        dangling.join("\n")
    );
}

#[test]
fn every_module_doc_that_lists_its_parts_lists_all_of_them() {
    let mut wrong = Vec::new();
    for (path, text) in sources() {
        let mut listed = Vec::new();
        let mut declared = Vec::new();
        let mut gated = false;
        for line in text.lines() {
            let trimmed = line.trim_start();
            // A bullet of the module doc: `//! - `name` — what it is`.
            if let Some(rest) = trimmed.strip_prefix("//! - ") {
                let name: String = rest
                    .trim_start_matches('`')
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    listed.push(name);
                }
            }
            if trimmed.starts_with("#[cfg(test)]") {
                gated = true;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("mod ").or_else(|| {
                trimmed
                    .strip_prefix("pub mod ")
                    .or_else(|| trimmed.strip_prefix("pub(crate) mod "))
            }) && let Some(name) = rest.strip_suffix(';')
            {
                // A module only compiled for tests is not part of the map a reader needs, and
                // `tests` itself never is.
                if !gated && name != "tests" {
                    declared.push(name.to_string());
                }
                gated = false;
                continue;
            }
            if !trimmed.is_empty() {
                gated = false;
            }
        }
        // Only a module that has *chosen* to list its parts is held to listing all of them. A
        // module doc with no bullets is prose, and prose is not a promise of completeness.
        if listed.is_empty() || declared.is_empty() {
            continue;
        }
        for name in declared {
            if !listed.contains(&name) {
                wrong.push(format!(
                    "{}: `{name}` is declared and not listed",
                    shown(&path)
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a module doc lists its parts and has fallen behind the files beside it:\n{}\n\
         Add a line for each, or delete the list — a map missing a road is worse than no map.",
        wrong.join("\n")
    );
}

/// The behavioural-coverage configuration, if this checkout has one.
///
/// Identified by what it contains rather than by what it is called: the root-level YAML declaring a
/// `sources:` list. The published tree carries no such file — the configuration is stripped on the
/// way out — and answering `None` there is correct rather than a gap, because there is nothing in
/// that tree for the list to have fallen behind.
fn coverage_configuration() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = None;
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|kind| kind == "yaml" || kind == "yml")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if text.lines().any(|line| line.trim_end() == "sources:") {
            // Concatenated rather than taken, so that two of them cannot silently mean one is
            // ignored — the union is what is actually being probed either way.
            found = Some(match found {
                None => text,
                Some(seen) => format!("{seen}\n{text}"),
            });
        }
    }
    found
}

/// Every `.rs` file under `src/` and `conformance/src/` that is not a test, as repository paths.
///
/// Spelled with forward slashes whatever the host uses, because what they are compared against is a
/// hand-written list in a configuration file and that list is written one way.
fn probeable_sources() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = BTreeSet::new();
    let mut pending = vec![root.join("src"), root.join("conformance").join("src")];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !path.extension().is_some_and(|kind| kind == "rs") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let spelled = relative.to_string_lossy().replace('\\', "/");
            // A test file is not a target and must not be one: a mutated assertion fails the test
            // it is in, so it scores a kill that means nothing and costs a deep run the time to
            // find it. Two were swept in by a refactor once and had to be taken back out.
            if spelled.contains("/tests/") || spelled.ends_with("tests.rs") {
                continue;
            }
            found.insert(spelled);
        }
    }
    found
}

#[test]
fn every_source_file_is_either_covered_or_excluded_on_purpose() {
    // **The failure this exists for has happened at least six times and the recorded fix has always
    // been "remember".** The behavioural-coverage configuration names the files it probes one per
    // line — deliberately, because some are plumbing whose mutants no test could kill — and a file
    // it does not name is not probed *and the run reports green*. Nothing warns. The unit suite
    // passes, the conformance figure does not move, and the diff-scoped run prints a plausible
    // number of changed lines.
    //
    // A refactor is the worst case: splitting a file moves working, covered code into a name the
    // list has never heard of, and every other signal agrees that nothing is wrong — because
    // nothing *is* wrong except that the check stopped happening. That is how UAX #15 shipped with
    // all four normalization forms interchangeable and every Hangul index calculation unexercised.
    //
    // So this is the audit, wired to the build rather than to anybody's memory. It is not a
    // judgement about *whether* a file should be probed — that judgement is the exclusion list
    // below, and adding to it is a deliberate act with a reason written beside it. What it refuses
    // is a file that is on neither list, which is the only state that is always a mistake.
    // The configuration is found by its *shape* rather than by its name — the root-level YAML that
    // declares a `sources:` list. Two reasons, and the second is the load-bearing one: a name
    // written here would be a second place to update when the tooling changes, and this file is
    // published while the configuration is not, so a hard-coded name would be a reference to
    // something no reader of the published tree can look at.
    let Some(configuration) = coverage_configuration() else {
        // Nothing to compare against, which is the published tree's normal state. A check that
        // invented a failure there would be worse than no check.
        return;
    };

    // Files that are deliberately unprobed, each for a reason recorded beside the list itself.
    // Named here as well so that the two cannot drift apart silently: an entry removed from the
    // configuration without being removed here fails this test rather than passing it.
    const EXCLUDED: [&str; 5] = [
        // Orchestration and the report. Listed for exactly one commit, on the strength of a
        // fail-closed argument reader being added to it, and it answered with 64 survivors — every
        // one a branch in `main` itself or in the printing, none of them killable, because an
        // integration test drives this binary as a *subprocess* and a mutant reaches the binary the
        // sandbox built rather than the one the test ran. The file was not the unit of judgement:
        // the two pure decisions inside it moved to `conformance/src/options.rs`, which is.
        "conformance/src/main.rs",
        // Walks a directory and supervises threads it cannot stop. What it does is observable only
        // by having a filesystem and a stuck worker.
        "conformance/src/drive.rs",
        // Threads, channels and a thread-local, where every branch is decided by whether another
        // operating-system thread has got somewhere yet. A mutant is killed by a timeout or by
        // nothing, depending on scheduling.
        "conformance/src/agent.rs",
        // `mod` declarations over the files that are listed.
        "conformance/src/lib.rs",
        // This file: `#[cfg(test)]` throughout, so a mutant fails the assertion it is inside.
        "src/documentation.rs",
    ];

    let listed: BTreeSet<String> = configuration
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- "))
        // A listed source is one word. The architectural rules in the same file are also `- ` items
        // and name `.rs` paths inside a sentence — `confine-ref std::fs src/bin/** …` — so an
        // `ends_with` alone reads a rule as a file and then reports it missing. Caught by the
        // second half of this test on its first run, which is the half that exists for exactly the
        // case of a list naming something that is not there.
        .filter(|path| path.ends_with(".rs") && !path.contains(char::is_whitespace))
        .map(str::to_owned)
        .collect();

    let mut unaccounted = Vec::new();
    for path in probeable_sources() {
        if !listed.contains(&path) && !EXCLUDED.contains(&path.as_str()) {
            unaccounted.push(path);
        }
    }
    assert!(
        unaccounted.is_empty(),
        "a source file is on neither the coverage list nor the exclusion list beside it, so \
         nothing is mutating it and every run reports green:\n  {}\n\nAdd it to the `sources:` \
         list in the coverage configuration, or to EXCLUDED here with the reason — but not \
         neither.",
        unaccounted.join("\n  ")
    );

    // …and the other direction: a listed path that no longer exists means a rename left the
    // configuration pointing at nothing, which is a file that has quietly stopped being probed
    // while the list still claims it is.
    let present = probeable_sources();
    let mut missing: Vec<&String> = listed
        .iter()
        .filter(|path| !present.contains(*path))
        .collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "the coverage list names files that are not there, so those entries guard nothing:\n  {}",
        missing
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
