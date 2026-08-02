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
fn no_comment_names_a_function_or_field_that_does_not_exist() {
    let sources = sources();
    let defined = defined(&sources);
    let mut dangling = Vec::new();
    for (path, text) in &sources {
        for (number, line) in text.lines().enumerate() {
            if !line.trim_start().starts_with("//") {
                continue;
            }
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
