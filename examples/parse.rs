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
//! A `Script` and a `Module` are different languages — see `src/parser/module.rs` — and nothing in
//! a `.js` file says which it is. So the default asks the extension, and for `.js` tries every
//! reading it might be. A file only counts as a failure when *none* of them takes it, which is the
//! honest bar: the question here is whether the engine can parse real JavaScript, not whether it
//! guessed the goal.
//!
//! # Panics are failures too
//!
//! DR-0002: no input may panic, ever. A sweep is the cheapest fuzzer this project has, so each
//! file is parsed inside `catch_unwind` and a panic is counted and reported rather than taking the
//! run with it. A single one is a P0 regardless of how odd the file looked.
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
    for file in &files {
        report.record(file, &options);
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
    fn record(&mut self, file: &Path, options: &Options) {
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
        match parse(&source, file, options) {
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
fn parse(source: &str, path: &Path, options: &Options) -> Outcome {
    let readings: &[(&'static str, bool)] = match options.goal {
        Goal::Script => &[("script", false)],
        Goal::Module => &[("module", true)],
        Goal::FromExtension => match extension(path) {
            Some("mjs") => &[("module", true)],
            Some("cjs") => &[("script", false)],
            // Nothing in a `.js` file says which it is, so it is a failure only if neither takes
            // it. The script error is the one reported: a `.js` file is a script far more often
            // than not, so it is the more likely to be about the code rather than the goal.
            _ => &[("script", false), ("module", true)],
        },
    };
    let mut first: Option<praxis::parser::ParseError> = None;
    for (name, as_module) in readings {
        match guarded(source, *as_module) {
            Err(()) => return Outcome::Panicked,
            Ok(Ok(())) => return Outcome::Parsed(name),
            Ok(Err(error)) => first.get_or_insert(error),
        };
    }
    // What node does to a `.js` file: wrap it, so a top-level `return` is a return from the
    // module wrapper. An approximation of the real wrapper, and enough to tell a CommonJS file
    // from one this parser cannot read. The verdict comes from the wrapped source and the
    // *message* from the unwrapped one, so no offset ever refers to text nobody wrote.
    if options.commonjs && !matches!(options.goal, Goal::Module) {
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
