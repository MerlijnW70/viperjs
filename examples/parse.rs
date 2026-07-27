//! Parse real files with the engine, one or a whole repository at a time.
//!
//! ```text
//! cargo run --example parse -- <path>...              # parse, and say what happened
//! cargo run --example parse -- --tree <file>          # …and print the syntax tree
//! cargo run --example parse -- --module <file>        # force the Module goal
//! cargo run --example parse -- --script <file>        # force the Script goal
//! cargo run --example parse -- --quiet <dir>          # only the failures and the summary
//! ```
//!
//! A path may be a file or a directory; a directory is walked for `.js`, `.mjs` and `.cjs`. The
//! exit status is 0 only if every file parsed.
//!
//! # Which goal symbol
//!
//! A `Script` and a `Module` are different languages — see `src/parser/module.rs` — and nothing in
//! a `.js` file says which it is, so by default this asks the extension: `.mjs` is a module, `.cjs`
//! is a script, and `.js` is *tried as both*. That last one is the honest answer rather than a
//! guess: a file that parses under either goal has not told you anything, and one that parses
//! under exactly one has told you which it is. `--module` and `--script` say so instead.
//!
//! # What "the expected tree" means here
//!
//! `--tree` prints the parse with `{:#?}`, which is the whole tree including every span. That is
//! what the engine actually built, so it is what an expectation should be written against; the
//! compact s-expression rendering the parser's own tests use is `#[cfg(test)]` and deliberately
//! does not ship.
//!
//! This example takes no dependencies, because the engine has none and neither may anything the
//! repository builds by default (GOAL.md non-negotiable #2). The argument handling is hand-rolled
//! for the same reason.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use praxis::parser::{ParseError, parse_module, parse_script};
use praxis::span::line_col;

/// Which goal symbol to parse under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Goal {
    /// `--script`.
    Script,
    /// `--module`.
    Module,
    /// The default: ask the extension, and try both when it does not say.
    FromExtension,
}

/// What happened to one file.
enum Outcome {
    /// It parsed, under the goals named.
    Parsed(&'static str),
    /// It did not, and this is the first error and the goal that produced it.
    Failed(&'static str, ParseError),
}

fn main() -> ExitCode {
    let mut goal = Goal::FromExtension;
    let mut tree = false;
    let mut quiet = false;
    let mut paths: Vec<PathBuf> = Vec::new();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--module" => goal = Goal::Module,
            "--script" => goal = Goal::Script,
            "--tree" => tree = true,
            "--quiet" => quiet = true,
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("parse: unknown option `{other}`");
                print_usage();
                return ExitCode::FAILURE;
            }
            other => paths.push(PathBuf::from(other)),
        }
    }
    if paths.is_empty() {
        print_usage();
        return ExitCode::FAILURE;
    }

    let mut files = Vec::new();
    for path in &paths {
        if let Err(error) = collect(path, &mut files) {
            eprintln!("parse: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }
    if files.is_empty() {
        eprintln!("parse: no .js, .mjs or .cjs files under the paths given");
        return ExitCode::FAILURE;
    }

    let mut parsed = 0usize;
    let mut failed = 0usize;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                // Not a parse failure: a file that is not UTF-8 is not source text this engine
                // is defined over, and saying so is more useful than a syntax error would be.
                failed += 1;
                println!("{}: unreadable: {error}", file.display());
                continue;
            }
        };
        match parse(&source, file, goal) {
            Outcome::Parsed(under) => {
                parsed += 1;
                if !quiet {
                    println!("{}: ok ({under})", file.display());
                }
                if tree {
                    print_tree(&source, file, goal);
                }
            }
            Outcome::Failed(under, error) => {
                failed += 1;
                report(&source, file, under, error);
            }
        }
    }
    println!("{parsed} parsed, {failed} failed, {} total", files.len());
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Parse `source` under `goal`, reporting which goal or goals took it.
///
/// For a `.js` file both are tried, and the answer names whichever accepted it. A file that only
/// one goal takes is the interesting case, and it is the common one: anything with an `import` or
/// `export` is a module and nothing else, and anything with `with` or a legacy octal is a script.
fn parse(source: &str, path: &Path, goal: Goal) -> Outcome {
    match goal {
        Goal::Script => match parse_script(source) {
            Ok(_) => Outcome::Parsed("script"),
            Err(error) => Outcome::Failed("script", error),
        },
        Goal::Module => match parse_module(source) {
            Ok(_) => Outcome::Parsed("module"),
            Err(error) => Outcome::Failed("module", error),
        },
        Goal::FromExtension => match extension(path) {
            Some("mjs") => parse(source, path, Goal::Module),
            Some("cjs") => parse(source, path, Goal::Script),
            _ => match (parse_script(source), parse_module(source)) {
                (Ok(_), Ok(_)) => Outcome::Parsed("script and module"),
                (Ok(_), Err(_)) => Outcome::Parsed("script only"),
                (Err(_), Ok(_)) => Outcome::Parsed("module only"),
                // Neither took it. The script error is reported because a `.js` file is a script
                // far more often than not, so it is the one more likely to be about the code
                // rather than about the goal.
                (Err(error), Err(_)) => Outcome::Failed("script", error),
            },
        },
    }
}

/// Print the tree, under whichever goal took the file.
fn print_tree(source: &str, path: &Path, goal: Goal) {
    let as_module = match goal {
        Goal::Module => true,
        Goal::Script => false,
        Goal::FromExtension => extension(path) == Some("mjs") || parse_script(source).is_err(),
    };
    if as_module {
        match parse_module(source) {
            Ok(module) => println!("{module:#?}"),
            Err(error) => report(source, path, "module", error),
        }
    } else {
        match parse_script(source) {
            Ok(script) => println!("{script:#?}"),
            Err(error) => report(source, path, "script", error),
        }
    }
}

/// A parse failure, with the line, the column and the offending text underlined.
///
/// Every `ParseError` carries a span, so a caret is always available — which is the point of
/// errors being values with spans rather than strings.
fn report(source: &str, path: &Path, goal: &str, error: ParseError) {
    let at = line_col(source, error.span.start);
    println!(
        "{}:{}:{}: {} (as a {goal})",
        path.display(),
        at.line,
        at.column,
        error.kind
    );
    let Some(line) = source.lines().nth(at.line as usize - 1) else {
        return;
    };
    println!("  {line}");
    // The column is in characters, and so is the padding — a caret under a tab is going to be
    // wrong either way, and being wrong by a consistent amount is the lesser evil.
    let width = usize::max(error.span.len() as usize, 1);
    println!(
        "  {}{}",
        " ".repeat(at.column as usize - 1),
        "^".repeat(width)
    );
}

/// Every JavaScript file under `path`, or `path` itself when it is a file.
fn collect(path: &Path, into: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        into.push(path.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?.path();
        if entry.is_dir() {
            // `node_modules` is other people's code and there is a great deal of it; a repository
            // is being asked about here, not its dependencies.
            if entry.file_name().is_some_and(|name| name == "node_modules") {
                continue;
            }
            collect(&entry, into)?;
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

/// What the arguments are.
fn print_usage() {
    eprintln!(
        "\
usage: cargo run --example parse -- [options] <path>...

  <path>      a file, or a directory to walk for .js, .mjs and .cjs

  --script    parse under the Script goal
  --module    parse under the Module goal
              (the default asks the extension, and tries both for .js)
  --tree      print the syntax tree of each file that parses
  --quiet     print only the failures and the summary

exits 0 only if every file parsed."
    );
}
