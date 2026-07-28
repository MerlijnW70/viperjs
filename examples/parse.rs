//! Sweep a repository with the engine and triage whatever it will not parse.
//!
//! ```text
//! git clone --depth 1 https://github.com/some/repo /tmp/repo
//! cargo run --release --example parse -- /tmp/repo
//! ```
//!
//! ```text
//! cargo run --example parse -- [options] <path>...
//!
//!   <path>            a file, or a directory walked for .js, .mjs and .cjs
//!
//!   --script          parse everything under the Script goal
//!   --module          parse everything under the Module goal
//!   --commonjs        also try the wrapper node puts around a .js file
//!   --exclude <name>  a directory name to skip (repeatable)
//!   --show <n>        example failures to print per error kind (default 3)
//!   --list            a line per file instead of the grouped report
//!   --tree            print the syntax tree of each file that parses
//! ```
//!
//! # Why the report is grouped
//!
//! One line per failure is useless past about fifty files: a single missing production shows up
//! as hundreds of unrelated-looking lines. Grouping by error kind and sorting by count turns the
//! same output into a ranked list of *suspects*, because a parser bug is almost always one bucket
//! with a large number in front of it. Three real bugs were found this way within minutes of the
//! first sweep, and every one of them was the top bucket.
//!
//! A large bucket is not proof of a bug — `return` outside a function is a large bucket on any
//! CommonJS repository and the parser is right every time. That is what `--commonjs` is for: it
//! removes the noise so the buckets that are left mean something.
//!
//! # Which goal symbol
//!
//! A `Script` and a `Module` are different languages — see `src/parser/module.rs` — and nothing
//! *in* a `.js` file says which it is. `.mjs` and `.cjs` say it themselves; for `.js` the answer
//! lives in the nearest `package.json`, and this asks it the way node does: walk up until one is
//! found, and read its top-level `"type"`.
//!
//! Every reading is still tried, so a file counts as a failure only when *none* of them takes it
//! — the honest bar, since the question is whether the engine can parse real JavaScript and not
//! whether the sweep guessed the goal. What the manifest changes is which reading goes **first**,
//! and that is what matters, because the error reported is the first reading's. Before this, a
//! `.js` module was always tried as a script first, so every ESM file that failed for a real
//! reason was reported as `expected \`(\` or \`.\`, found \`{\`` on its first `import` line —
//! a message about the goal, pointing at the wrong line, hiding the error that mattered.
//!
//! # Panics are failures too
//!
//! DR-0002: no input may panic, ever. A sweep is the cheapest fuzzer this project has, so each
//! file is parsed inside `catch_unwind` and a panic is counted and reported rather than taking the
//! run with it. A single one is a P0 regardless of how odd the file looked.
//!
//! A *stack overflow* is the one thing this cannot contain: it aborts the process, so the run
//! stops and the summary never prints. That is not a shortcoming to work around — it is the
//! loudest possible way of reporting the same P0. `--list` names each file before parsing it,
//! which is how to find out which one did it.
//!
//! No dependencies, because the engine has none and neither may anything the repository builds by
//! default (GOAL.md non-negotiable #2). The argument handling and the walk are hand-rolled.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use praxis::span::line_col;

/// Which goal symbol to parse under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Goal {
    /// `--script`.
    Script,
    /// `--module`.
    Module,
    /// The default: ask the extension, and try every reading a `.js` might be.
    FromExtension,
}

/// What the package scope around a file says a `.js` in it is (node's `"type"` field).
///
/// `.mjs` and `.cjs` say it themselves and never ask. This is only about `.js`, which is the
/// one extension whose meaning lives somewhere else entirely.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PackageType {
    /// The nearest `package.json` says `"type": "module"`.
    Module,
    /// It says `"type": "commonjs"`, or says nothing, which is the same thing — node's default.
    CommonJs,
    /// There is no `package.json` above the file at all, so nobody has said.
    Unstated,
}

/// The package scope of each directory, worked out once.
///
/// A sweep asks this per file and a repository has far more files than directories, so without
/// the cache a 20,000-file run would stat its way up the tree 20,000 times. Every directory on
/// the way up gets the answer, not just the one that had the `package.json`.
#[derive(Default)]
struct Scopes {
    known: HashMap<PathBuf, PackageType>,
}

impl Scopes {
    /// What a `.js` at `file` is, by node's rule.
    ///
    /// Node walks up from the file until it finds a `package.json` and reads its top-level
    /// `"type"`; the *nearest* one wins whether or not it has the field, and a missing field
    /// means CommonJS. The walk goes to the filesystem root rather than stopping at whatever
    /// directory the sweep was pointed at, because that is what node does — sweeping
    /// `some-repo/lib` has to find `some-repo/package.json` or it would answer differently from
    /// the runtime the code was written for.
    fn of(&mut self, file: &Path) -> PackageType {
        let mut visited = Vec::new();
        let mut directory = file.parent();
        let answer = loop {
            let Some(current) = directory else {
                break PackageType::Unstated;
            };
            if let Some(known) = self.known.get(current) {
                break *known;
            }
            visited.push(current.to_path_buf());
            let manifest = current.join("package.json");
            // A manifest that cannot be read is treated as absent rather than as an error: the
            // sweep is about the engine, and a permission problem three directories up is not
            // something to abort a 20,000-file run over.
            if let Ok(text) = std::fs::read_to_string(&manifest) {
                break match top_level_string(&text, "type").as_deref() {
                    Some("module") => PackageType::Module,
                    _ => PackageType::CommonJs,
                };
            }
            directory = current.parent();
        };
        for directory in visited {
            self.known.insert(directory, answer);
        }
        answer
    }
}

/// The value of a top-level string field of a JSON object, or `None` if there is not one.
///
/// Hand-written because the repository takes no dependencies (GOAL.md non-negotiable #2), and
/// deliberately not a substring search for `"type"`: a `package.json` puts a `"type"` inside
/// `"exports"` conditions all the time, and one nested three levels down would answer for the
/// whole package. So the nesting is tracked, and only a key at depth one is an answer.
///
/// Escapes are located but not decoded — a `\"` does not end the string, which is what keeps the
/// scan aligned, but `m` stays as written. The two values node's resolution asks about are
/// `module` and `commonjs`, neither of which can contain an escape, so a value that does simply
/// fails to match and the caller falls back to CommonJS. That is node's own default, so the one
/// imprecision here fails in the safe direction.
fn top_level_string(json: &str, key: &str) -> Option<String> {
    let bytes = json.as_bytes();
    let mut at = 0;
    let mut depth = 0usize;
    // The last key seen at depth one, which is what the next value at depth one belongs to.
    let mut pending: Option<String> = None;
    while at < bytes.len() {
        match bytes[at] {
            b'{' | b'[' => {
                depth += 1;
                at += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                at += 1;
            }
            b'"' => {
                let (text, after) = json_string(json, at)?;
                let next = after
                    + bytes[after..]
                        .iter()
                        .take_while(|b| b.is_ascii_whitespace())
                        .count();
                if bytes.get(next) == Some(&b':') {
                    // A key, at whatever depth. It is not filtered here because it cannot need
                    // to be: a value at depth one is always preceded by a key at depth one, so a
                    // key from deeper down is overwritten before it could ever be consulted. The
                    // depth that decides an answer is the one on the *value*, below.
                    pending = Some(text.to_string());
                    at = next + 1;
                } else {
                    if depth == 1 && pending.as_deref() == Some(key) {
                        return Some(text.to_string());
                    }
                    pending = None;
                    at = after;
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// The contents of the JSON string starting at `at`, and the offset just past its closing quote.
///
/// Returns `None` for a string that never closes, which is a malformed manifest — the caller
/// treats that as having said nothing, which is the same as having no manifest at all.
fn json_string(json: &str, at: usize) -> Option<(&str, usize)> {
    let bytes = json.as_bytes();
    let mut cursor = at + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            // A backslash escapes whatever follows it, including a quote and another backslash.
            // Skipping two bytes is what stops `"a\\"` from being read as unterminated and
            // `"a\""` from being read as ending early.
            b'\\' => cursor += 2,
            b'"' => return json.get(at + 1..cursor).map(|text| (text, cursor + 1)),
            _ => cursor += 1,
        }
    }
    None
}

/// Everything the command line said.
struct Options {
    goal: Goal,
    commonjs: bool,
    excluded: Vec<String>,
    show: usize,
    list: bool,
    tree: bool,
}

/// What happened to one file.
enum Outcome {
    /// It parsed, under the reading named.
    Parsed(&'static str),
    /// It did not, under any reading. The message and where it was.
    Failed { message: String, at: Option<Where> },
    /// The parser panicked, which DR-0002 says may never happen.
    Panicked,
}

/// A place in a file, for the report.
struct Where {
    line: u32,
    column: u32,
    /// The line's text and the width to underline, when both are available.
    context: Option<(String, usize)>,
}

fn main() -> ExitCode {
    let (options, paths) = match parse_arguments() {
        Ok(parsed) => parsed,
        Err(()) => return ExitCode::FAILURE,
    };
    if paths.is_empty() {
        print_usage();
        return ExitCode::FAILURE;
    }

    let mut files = Vec::new();
    for path in &paths {
        if let Err(error) = collect(path, &options.excluded, &mut files) {
            eprintln!("parse: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }
    if files.is_empty() {
        eprintln!("parse: no .js, .mjs or .cjs files under the paths given");
        return ExitCode::FAILURE;
    }
    files.sort();

    // A panic is being *reported* rather than printed, so the default hook would only interleave
    // a backtrace with the report. Restored before returning so nothing else is affected.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let started = Instant::now();
    let mut report = Report::default();
    let mut scopes = Scopes::default();
    for file in &files {
        let scope = scopes.of(file);
        report.record(file, &options, scope);
    }
    let elapsed = started.elapsed();
    std::panic::set_hook(hook);

    report.print(files.len(), elapsed, &options);
    if report.failed == 0 && report.panicked == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Everything the sweep found, ready to be printed.
#[derive(Default)]
struct Report {
    parsed: usize,
    failed: usize,
    panicked: usize,
    bytes: u64,
    /// Failures by their message, which is what makes one bug one bucket.
    buckets: HashMap<String, Vec<(PathBuf, Option<Where>)>>,
}

impl Report {
    /// Parse one file and fold the result in.
    fn record(&mut self, file: &Path, options: &Options, scope: PackageType) {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                // Not a parse failure: a file that is not UTF-8 is not source text this engine is
                // defined over, so it is its own bucket rather than a syntax error.
                self.failed += 1;
                self.buckets
                    .entry(format!("unreadable: {error}"))
                    .or_default()
                    .push((file.to_path_buf(), None));
                return;
            }
        };
        self.bytes += source.len() as u64;
        match parse(&source, file, options, scope) {
            Outcome::Parsed(under) => {
                self.parsed += 1;
                if options.list {
                    println!("{}: ok ({under})", file.display());
                }
                if options.tree {
                    print_tree(&source, file, options);
                }
            }
            Outcome::Failed { message, at } => {
                self.failed += 1;
                if options.list {
                    println!("{}: {message}", file.display());
                }
                self.buckets
                    .entry(message)
                    .or_default()
                    .push((file.to_path_buf(), at));
            }
            Outcome::Panicked => {
                self.panicked += 1;
                self.buckets
                    .entry("PANICKED — DR-0002 says no input may do this".to_string())
                    .or_default()
                    .push((file.to_path_buf(), None));
            }
        }
    }

    /// The grouped report, biggest bucket first.
    fn print(&self, total: usize, elapsed: std::time::Duration, options: &Options) {
        let mut buckets: Vec<_> = self.buckets.iter().collect();
        // By count, then by message, so two runs over the same corpus print the same thing.
        buckets.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
        for (message, files) in &buckets {
            println!();
            println!("{:>5} x  {message}", files.len());
            for (file, at) in files.iter().take(options.show) {
                match at {
                    Some(at) => println!("         {}:{}:{}", file.display(), at.line, at.column),
                    None => println!("         {}", file.display()),
                }
                if let Some(Where {
                    context: Some((line, width)),
                    column,
                    ..
                }) = at
                {
                    let (text, caret) = window(line, *column as usize, *width);
                    println!("           {text}");
                    println!("           {}{}", " ".repeat(caret), "^".repeat(*width));
                }
            }
            if files.len() > options.show {
                println!("         … and {} more", files.len() - options.show);
            }
        }
        println!();
        println!(
            "{total} files, {} parsed, {} failed, {} panicked — {:.1} MB in {:.1}s",
            self.parsed,
            self.failed,
            self.panicked,
            self.bytes as f64 / 1_048_576.0,
            elapsed.as_secs_f64()
        );
    }
}

/// Parse `source` every way it might legitimately be read, inside a panic guard.
fn parse(source: &str, path: &Path, options: &Options, scope: PackageType) -> Outcome {
    const AS_MODULE: [(&str, bool); 2] = [("module", true), ("script", false)];
    const AS_SCRIPT: [(&str, bool); 2] = [("script", false), ("module", true)];
    let readings: &[(&str, bool)] = match options.goal {
        Goal::Script => &[("script", false)],
        Goal::Module => &[("module", true)],
        Goal::FromExtension => match extension(path) {
            Some("mjs") => &[("module", true)],
            Some("cjs") => &[("script", false)],
            // Both readings, ordered by what the package scope says the file is. Both, because
            // a manifest can be wrong or absent and the bar is whether *any* reading takes it;
            // ordered, because the error reported is the first one's and an error about the
            // wrong goal is worse than no error at all — it points at the wrong line.
            _ if scope == PackageType::Module => &AS_MODULE,
            _ => &AS_SCRIPT,
        },
    };
    // When no reading takes the file, the one that got *furthest* is the one worth reporting.
    // An ESM file read as a script fails on its first `import`, which is a fact about the goal
    // and not about the code; read as a module it fails wherever the real trouble is. Reporting
    // the first reading's error instead sends a reader to line 1 of 1,338 of react's files, and
    // that is how long it took to notice.
    let mut furthest: Option<praxis::parser::ParseError> = None;
    for (name, as_module) in readings {
        match guarded(source, *as_module) {
            Err(()) => return Outcome::Panicked,
            Ok(Ok(())) => return Outcome::Parsed(name),
            Ok(Err(error)) => {
                let further = furthest
                    .as_ref()
                    .is_none_or(|best| error.span.start > best.span.start);
                if further {
                    furthest = Some(error);
                }
            }
        };
    }
    let first = furthest;
    // What node does to a `.js` file: wrap it, so a top-level `return` is a return from the
    // module wrapper. An approximation of the real wrapper, and enough to tell a CommonJS file
    // from one this parser cannot read. The verdict comes from the wrapped source and the
    // *message* from the unwrapped one, so no offset ever refers to text nobody wrote.
    // Not for a file the package scope calls a module: node does not wrap those, and wrapping
    // one here would only give a second wrong reading a chance to hide the first right one.
    if options.commonjs
        && !matches!(options.goal, Goal::Module)
        && scope != PackageType::Module
        && extension(path) != Some("mjs")
    {
        // A `HashbangComment` (§12.5) is only one at offset zero, so the wrapper goes *after* it
        // — which is the order node does it in too, the hashbang being stripped before the module
        // wrapper is applied.
        let (hashbang, body) = match source.starts_with("#!") {
            true => source.split_at(source.find('\n').map_or(source.len(), |at| at + 1)),
            false => ("", source),
        };
        let wrapped = format!(
            "{hashbang}(function (exports, require, module, __filename, __dirname) {{\n{body}\n}});"
        );
        match guarded(&wrapped, false) {
            Err(()) => return Outcome::Panicked,
            Ok(Ok(())) => return Outcome::Parsed("commonjs"),
            Ok(Err(_)) => {}
        }
    }
    let Some(error) = first else {
        // `readings` is never empty, so `first` is always set by the loop above.
        return Outcome::Failed {
            message: "no reading attempted".to_string(),
            at: None,
        };
    };
    Outcome::Failed {
        message: error.kind.to_string(),
        at: Some(locate(source, error)),
    }
}

/// One parse, with a panic turned into a value.
///
/// The engine forbids `unsafe` and every parse is a pure function of a `&str`, so nothing here can
/// be left half-built by an unwind — the guard is about *reporting* the panic rather than about
/// surviving it.
fn guarded(source: &str, as_module: bool) -> Result<Result<(), praxis::parser::ParseError>, ()> {
    catch_unwind(AssertUnwindSafe(|| {
        if as_module {
            praxis::parser::parse_module(source).map(|_| ())
        } else {
            praxis::parser::parse_script(source).map(|_| ())
        }
    }))
    .map_err(|_| ())
}

/// A slice of `line` around `column`, and where the caret goes in it.
///
/// Minified and generated files have lines in the hundreds of thousands of characters, and
/// printing one buries the report it was meant to explain. The window is what a caret needs to
/// be useful: enough either side to see what was written, and an ellipsis where the rest went.
fn window(line: &str, column: usize, width: usize) -> (String, usize) {
    const EITHER_SIDE: usize = 40;
    let characters: Vec<char> = line.trim_end().chars().collect();
    if characters.len() <= EITHER_SIDE * 2 + width {
        return (characters.iter().collect(), column - 1);
    }
    let start = column.saturating_sub(EITHER_SIDE + 1);
    let end = usize::min(characters.len(), column - 1 + width + EITHER_SIDE);
    let mut text = String::new();
    if start > 0 {
        text.push('…');
    }
    text.extend(&characters[start..end]);
    if end < characters.len() {
        text.push('…');
    }
    (text, column - 1 - start + usize::from(start > 0))
}

/// Where an error was, with the offending line if it can be found.
fn locate(source: &str, error: praxis::parser::ParseError) -> Where {
    let at = line_col(source, error.span.start);
    // `lines` and `line_col` agree about CRLF, but a span can still point at the end of input,
    // where there is no line to show. The caret is worth having only when there is.
    let context = source
        .lines()
        .nth(at.line as usize - 1)
        .filter(|line| (at.column as usize) <= line.chars().count() + 1)
        .map(|line| (line.to_string(), usize::max(error.span.len() as usize, 1)));
    Where {
        line: at.line,
        column: at.column,
        context,
    }
}

/// Print the tree, under whichever goal took the file.
fn print_tree(source: &str, path: &Path, options: &Options) {
    let as_module = match options.goal {
        Goal::Module => true,
        Goal::Script => false,
        Goal::FromExtension => {
            extension(path) == Some("mjs") || praxis::parser::parse_script(source).is_err()
        }
    };
    if as_module {
        if let Ok(module) = praxis::parser::parse_module(source) {
            println!("{module:#?}");
        }
    } else if let Ok(script) = praxis::parser::parse_script(source) {
        println!("{script:#?}");
    }
}

/// Every JavaScript file under `path`, or `path` itself when it is a file.
fn collect(path: &Path, excluded: &[String], into: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        into.push(path.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?.path();
        if entry.is_dir() {
            // A repository is being asked about, not its dependencies and not its history.
            let skip = entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| excluded.iter().any(|excluded| excluded == name));
            if !skip {
                collect(&entry, excluded, into)?;
            }
        } else if matches!(extension(&entry), Some("js" | "mjs" | "cjs")) {
            into.push(entry);
        }
    }
    Ok(())
}

/// A path's extension as a `&str`, if it has one that is UTF-8.
fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

/// The command line, or a complaint about it.
fn parse_arguments() -> Result<(Options, Vec<PathBuf>), ()> {
    let mut options = Options {
        goal: Goal::FromExtension,
        commonjs: false,
        excluded: ["node_modules", ".git"]
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        show: 3,
        list: false,
        tree: false,
    };
    let mut paths = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--module" => options.goal = Goal::Module,
            "--script" => options.goal = Goal::Script,
            "--commonjs" => options.commonjs = true,
            "--list" => options.list = true,
            "--tree" => options.tree = true,
            "--exclude" => match arguments.next() {
                Some(name) => options.excluded.push(name),
                None => {
                    eprintln!("parse: --exclude wants a directory name");
                    return Err(());
                }
            },
            "--show" => match arguments.next().map(|value| value.parse()) {
                Some(Ok(show)) => options.show = show,
                _ => {
                    eprintln!("parse: --show wants a number");
                    return Err(());
                }
            },
            "-h" | "--help" => {
                print_usage();
                return Err(());
            }
            other if other.starts_with('-') => {
                eprintln!("parse: unknown option `{other}`");
                print_usage();
                return Err(());
            }
            other => paths.push(PathBuf::from(other)),
        }
    }
    Ok((options, paths))
}

/// What the arguments are.
fn print_usage() {
    eprintln!(
        "\
usage: cargo run --release --example parse -- [options] <path>...

  <path>            a file, or a directory walked for .js, .mjs and .cjs

  --script          parse everything under the Script goal
  --module          parse everything under the Module goal
                    (the default asks the extension, and tries both for .js)
  --commonjs        also try the wrapper node puts around a .js file
  --exclude <name>  a directory name to skip; node_modules and .git always are
  --show <n>        example failures to print per error kind (default 3)
  --list            a line per file instead of the grouped report
  --tree            print the syntax tree of each file that parses

Failures are grouped by error kind and sorted by count, because a parser bug is
almost always one bucket with a large number in front of it. A panic is counted
separately and is a P0 whatever the input looked like (DR-0002).

exits 0 only if every file parsed."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_of(json: &str) -> Option<String> {
        top_level_string(json, "type")
    }

    #[test]
    fn a_top_level_type_is_read_and_a_nested_one_is_not() {
        assert_eq!(type_of(r#"{"type": "module"}"#).as_deref(), Some("module"));
        assert_eq!(
            type_of(r#"{"type":"commonjs"}"#).as_deref(),
            Some("commonjs")
        );
        // The whole reason this is not a substring search. A `package.json` puts a `"type"`
        // inside `"exports"` conditions constantly, and one of those answers for nothing.
        assert_eq!(type_of(r#"{"exports": {"./x": {"type": "module"}}}"#), None);
        assert_eq!(type_of(r#"{"a": {"b": {"type": "module"}}}"#), None);
        // …including inside an array, where the depth also has to be counted.
        assert_eq!(type_of(r#"{"files": [{"type": "module"}]}"#), None);
        // A nested key must clear the pending one, or the value that closes the nesting would
        // be read as the outer key's.
        assert_eq!(type_of(r#"{"type": {"a": "module"}}"#), None);
        // The field is found wherever it stands among its siblings.
        assert_eq!(
            type_of(r#"{"name": "x", "type": "module", "main": "i.js"}"#).as_deref(),
            Some("module")
        );
    }

    #[test]
    fn a_string_that_looks_like_the_field_but_is_not_it_is_ignored() {
        // A *value* that happens to spell the key. `pending` is what tells them apart.
        assert_eq!(type_of(r#"{"name": "type"}"#), None);
        assert_eq!(type_of(r#"{"keywords": ["type", "module"]}"#), None);
        // A different key entirely.
        assert_eq!(type_of(r#"{"types": "module"}"#), None);
        assert_eq!(type_of(r#"{"subtype": "module"}"#), None);
    }

    #[test]
    fn an_escape_inside_a_string_does_not_end_it() {
        // The alignment case: a `\"` that ended the string early would leave the scan reading
        // the rest of the file as keys and values it is not.
        assert_eq!(
            type_of(r#"{"name": "a\"b", "type": "module"}"#).as_deref(),
            Some("module")
        );
        // A trailing backslash is itself escaped, so this string ends at the *second* quote.
        assert_eq!(
            type_of(r#"{"name": "a\\", "type": "module"}"#).as_deref(),
            Some("module")
        );
        // A `:` or a brace inside a string is text, not structure.
        assert_eq!(
            type_of(r#"{"desc": "a{b}:c", "type": "module"}"#).as_deref(),
            Some("module")
        );
        // The ones that actually pin it, and finding them took a search rather than an argument:
        // most misalignments come back into step by accident a token or two later, so a test has
        // to be one where they do not. Ending a string at an escaped quote leaves every later
        // quote on the wrong side of the boundary, and the field is then never seen as a key.
        assert_eq!(
            type_of(r#"{"d": "\"", "type": "commonjs"}"#).as_deref(),
            Some("commonjs")
        );
        assert_eq!(
            type_of(r#"{"d": "q\"", "type": "commonjs"}"#).as_deref(),
            Some("commonjs")
        );
        // A description that quotes a manifest at anyone reading it — the realistic shape of the
        // same trap, and the one where a mis-scan would answer with the *quoted* package's type.
        assert_eq!(
            type_of(r#"{"x": "\"type\":\"module\",\"y\":\"", "type": "commonjs"}"#).as_deref(),
            Some("commonjs")
        );
    }

    #[test]
    fn a_manifest_that_says_nothing_usable_answers_nothing() {
        assert_eq!(type_of("{}"), None);
        assert_eq!(type_of(""), None);
        assert_eq!(type_of("null"), None);
        assert_eq!(type_of(r#"{"type": 1}"#), None);
        assert_eq!(type_of(r#"{"type": true}"#), None);
        assert_eq!(type_of(r#"{"type": null}"#), None);
        assert_eq!(type_of(r#"{"type": ["module"]}"#), None);
        // Malformed, which a caller treats as no manifest at all rather than as an error.
        assert_eq!(type_of(r#"{"type": "modu"#), None);
        assert_eq!(type_of(r#"{"type""#), None);
        assert_eq!(type_of("{{{{"), None);
        assert_eq!(type_of("}}}}"), None);
    }

    #[test]
    fn no_input_makes_the_scanner_loop_or_panic() {
        // The sweep's own rule, applied to itself: this reads files nobody vetted, so every
        // shape has to terminate with an answer rather than with a crash.
        for json in [
            "\"",
            "\\",
            "{\"",
            "{\"a\"",
            "{\"a\":",
            "{\"a\":\"",
            "[[[[",
            "\u{1f600}",
            "{\"type\":\"m\u{fffd}\"}",
            "{\"\\\\\"",
            "{\"a\\",
            "\0",
            "{\"type\":\"module\"",
        ] {
            let _ = top_level_string(json, "type");
        }
        // A long alternation of opens and closes: depth must not underflow on the closes.
        let deep: String = "[{]}".repeat(2_000);
        assert_eq!(top_level_string(&deep, "type"), None);
    }

    #[test]
    fn the_nearest_manifest_wins_and_every_directory_on_the_way_is_remembered() {
        let root = std::env::temp_dir().join("praxis-scope-test");
        let _ = std::fs::remove_dir_all(&root);
        let inner = root.join("packages").join("inner").join("src");
        std::fs::create_dir_all(&inner).expect("a temp tree");
        std::fs::write(root.join("package.json"), r#"{"type": "commonjs"}"#).expect("write");
        std::fs::write(
            root.join("packages").join("inner").join("package.json"),
            r#"{"type": "module"}"#,
        )
        .expect("write");

        let mut scopes = Scopes::default();
        // The inner manifest is nearer, so it is the one that answers.
        assert!(scopes.of(&inner.join("a.js")) == PackageType::Module);
        // …and the outer one still answers for a file outside the inner package.
        assert!(scopes.of(&root.join("b.js")) == PackageType::CommonJs);
        // Every directory walked past is cached, not only the one holding the manifest.
        assert!(scopes.known.contains_key(&inner));
        assert!(scopes.known.contains_key(&root));
        // And a second file in a directory already answered for gets the same answer. This is
        // the only thing that reads the cache back, so without it the read is a branch no test
        // touches — and a cache that answers differently the second time is exactly the bug
        // worth having a test for, since it would make two files in one directory disagree.
        assert!(scopes.of(&inner.join("b.js")) == PackageType::Module);
        assert!(scopes.of(&inner.join("deeper").join("c.js")) == PackageType::Module);

        let _ = std::fs::remove_dir_all(&root);
    }
}
