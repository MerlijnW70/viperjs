//! `viper` — run JavaScript from a command line.
//!
//! ```text
//! viper script.js              run a file
//! viper -e "1 + 1"             run one expression
//! cat script.js | viper        run standard input
//! viper                        a prompt, if standard input is a terminal
//! ```
//!
//! # Why the engine ships one at all
//!
//! It is not for the conformance number, which it cannot move. It is DR-0021's argument one level
//! up: `api.rs` exists because a twenty-line program written against the public surface found four
//! things fourteen unit tests inside the crate could not, and a *binary* is the same forcing
//! function again — the first thing anyone evaluating the engine reaches for, written where only
//! the published API is in scope.
//!
//! It also closes a gap the README made obvious. `examples/evaluate` reads **one script per line**,
//! because it was written to drive a differential sweep against another engine, and a file with a
//! function in it therefore came apart into fragments. There was no way to run an ordinary `.js`
//! file at all.
//!
//! # What it deliberately does not do
//!
//! GOAL.md §3: no Node compatibility. There is no `require`, no `fs`, no module resolution against
//! `node_modules`, and there will not be. The one thing bound here is `print`, because a language
//! with no way to say anything is hard to evaluate — everything else is the host's to provide, and
//! this host provides nothing.
//!
//! Modules are not run here either. [`viperjs::api::Engine`] evaluates Script code, and a Module is
//! a different goal symbol needing a resolver; `--module` would be a flag that lies. When the
//! embedding surface grows one, this gains the flag.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;
use std::time::Duration;
use viperjs::api::{Engine, Error, Host};
use viperjs::heap::{Heap, NativeCall};
use viperjs::value::{Completion, Value};
use viperjs::vm::Vm;

/// What the command line asked for.
///
/// Separated from doing it so that the *decision* can be tested without a filesystem, a terminal or
/// a process exit — which is the whole of what is worth testing here.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the file at this path.
    File(String),
    /// Run this source text, given on the command line.
    Source(String),
    /// Read the whole of standard input and run it as one script.
    Stdin,
    /// Prompt, read a line, print what it came to, repeat.
    Prompt,
    /// Print the usage text and stop.
    Help,
    /// Print the version and stop.
    Version,
    /// The arguments made no sense, and this says why.
    Bad(String),
}

/// What the command line asked for, and how long a run may take.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    /// The thing to do.
    command: Command,
    /// DR-0022's budget, if `--time-budget` named one.
    budget: Option<Duration>,
}

/// Read the arguments — everything after the program's own name.
///
/// `interactive` is whether standard input is a terminal, which is the one thing that decides what
/// a bare `viper` means: a prompt when a person is typing, and "read the pipe" when something else
/// is. Passed in rather than asked here so that the answer is a *test's* to choose.
///
/// Nothing here allocates a parser or reaches for a crate. Five options is not a grammar, and
/// GOAL.md §2's empty dependency table is worth more than a tidier `--flag=value` story.
fn read_arguments(arguments: &[String], interactive: bool) -> Asked {
    let mut command = None;
    let mut budget = None;
    let mut at = 0;
    while at < arguments.len() {
        let argument = arguments[at].as_str();
        at += 1;
        match argument {
            "-h" | "--help" => return Asked::of(Command::Help),
            "-V" | "--version" => return Asked::of(Command::Version),
            "-e" | "--eval" => match arguments.get(at) {
                Some(source) => {
                    at += 1;
                    command = Some(Command::Source(source.clone()));
                }
                None => return Asked::of(Command::Bad("-e wants source after it".into())),
            },
            "--time-budget" => match arguments.get(at).map(|text| text.parse::<u64>()) {
                Some(Ok(millis)) => {
                    at += 1;
                    budget = Some(Duration::from_millis(millis));
                }
                _ => {
                    return Asked::of(Command::Bad(
                        "--time-budget wants a whole number of milliseconds".into(),
                    ));
                }
            },
            // A lone `-` is the conventional spelling of "standard input", and is what lets a
            // caller be explicit about it when a terminal would otherwise mean the prompt.
            "-" => command = Some(Command::Stdin),
            other if other.starts_with('-') => {
                return Asked::of(Command::Bad(format!("unknown option {other}")));
            }
            path => command = Some(Command::File(path.to_string())),
        }
    }
    Asked {
        // Nothing named: a person at a terminal gets a prompt, and a pipe gets read. The second is
        // the case that makes `cat script.js | viper` work and `examples/evaluate` not.
        command: command.unwrap_or(match interactive {
            true => Command::Prompt,
            false => Command::Stdin,
        }),
        budget,
    }
}

impl Asked {
    /// This command, with no budget — the shape every early return wants.
    fn of(command: Command) -> Self {
        Self {
            command,
            budget: None,
        }
    }
}

/// The usage text, which is also the documentation.
const USAGE: &str = "\
viper — ViperJS, an embeddable JavaScript engine, on a command line

usage:
  viper <file>              run a file as a Script
  viper -e <source>         run source given here
  viper -                   run standard input, even from a terminal
  viper                     run standard input, or prompt if it is a terminal

options:
  -e, --eval <source>        source to run
      --time-budget <ms>     stop a run that takes longer, uncatchably (DR-0022)
  -h, --help                 this text
  -V, --version              the version

The host binds one function, `print`. There is no `require`, no `fs` and no module loading:
GOAL.md §3 says the host provides I/O and this host provides almost none of it.

exit status: 0 ran, 1 the script threw or would not parse, 2 the arguments made no sense.
";

/// §19's `print` for a command line: write one argument and a newline.
///
/// Not `console.log`. A `console` object is a host API with a shape and a specification of its own,
/// and pretending to have one would be the first step toward the Node compatibility GOAL.md §3
/// refuses. This is a function that prints, named for what it does.
fn print(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    let text = host.text(call.argument(0))?;
    // A closed pipe is not this program's problem — `viper big.js | head` is an ordinary thing to
    // write, and the script goes on running as it would with any other host.
    let _ = writeln!(std::io::stdout(), "{text}");
    Ok(Value::Undefined)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let asked = read_arguments(&arguments, std::io::stdin().is_terminal());
    let mut engine = Engine::new();
    engine.bind("print", 1, print);
    engine.set_time_budget(asked.budget);

    match asked.command {
        Command::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("viper {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Bad(why) => {
            eprintln!("viper: {why}");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Command::File(path) => match std::fs::read_to_string(&path) {
            Ok(source) => run(&mut engine, &source, Show::Nothing),
            Err(why) => {
                eprintln!("viper: {path}: {why}");
                ExitCode::from(2)
            }
        },
        Command::Source(source) => run(&mut engine, &source, Show::Answer),
        Command::Stdin => {
            let mut source = String::new();
            match std::io::stdin().read_to_string(&mut source) {
                Ok(_) => run(&mut engine, &source, Show::Nothing),
                Err(why) => {
                    eprintln!("viper: could not read standard input: {why}");
                    ExitCode::from(2)
                }
            }
        }
        Command::Prompt => prompt(&mut engine),
    }
}

/// Whether a run's completion value is printed.
///
/// A file prints nothing unless it says so: a script whose last statement happens to be an
/// expression should not spray it at the terminal, and §14.2.2's completion value is not a return
/// value. `-e` and the prompt are the other way round — the answer is the entire reason they were
/// typed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    /// Print what the source came to.
    Answer,
    /// Print only what the script itself printed.
    Nothing,
}

/// Run `source`, report whatever went wrong, and answer the process's exit status.
fn run(engine: &mut Engine, source: &str, show: Show) -> ExitCode {
    match engine.eval(source) {
        Ok(value) if show == Show::Answer => match engine.text(value) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            // The answer exists and describing it threw — a `toString` of its own that failed.
            // The run still succeeded, so this is a message rather than a status.
            Err(error) => {
                eprintln!("viper: could not describe the answer: {}", describe(&error));
                ExitCode::SUCCESS
            }
        },
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("viper: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

/// What went wrong, in one line, for a person reading a terminal.
fn describe(error: &Error) -> String {
    match error {
        Error::Syntax(said) => format!("SyntaxError: {said}"),
        Error::Thrown(said) => said.clone(),
        Error::Interrupted => "the run was stopped: it spent its time budget".to_string(),
        Error::Collected => "a value was read after the collector had freed it".to_string(),
        // DR-0002: a script cannot cause this, so it is a bug report and says so.
        Error::Engine(fault) => format!("internal error, which is a bug in ViperJS: {fault:?}"),
    }
}

/// What a line typed at the prompt means: source to run, or nothing.
///
/// A pure function rather than a condition inside the loop, because the loop cannot be tested at
/// all — it needs a terminal, and a test that pipes to it is by definition not one. This is the only
/// decision in there, so lifting it out leaves the loop with nothing a mutation could silently
/// change.
///
/// Whitespace is nothing. `Enter` on an empty line is how a person pauses to think, and answering
/// `undefined` at them for it would be noise.
fn prompt_line(line: &str) -> Option<&str> {
    match line.trim().is_empty() {
        true => None,
        false => Some(line),
    }
}

/// Read a line, print what it came to, repeat.
///
/// One line is one Script. That is a real limit — a function written across three lines cannot be
/// typed here — and it is stated rather than worked around, because deciding that a line is
/// "incomplete" means asking the parser a question it does not currently answer: whether the error
/// was *at the end of input* or somewhere a second line could not fix.
fn prompt(engine: &mut Engine) -> ExitCode {
    println!(
        "ViperJS {} — one line is one script, ^D to leave",
        env!("CARGO_PKG_VERSION")
    );
    let input = std::io::stdin();
    loop {
        print!("> ");
        // Prompts are not newline-terminated, so nothing reaches the terminal without this.
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) => {
                println!();
                return ExitCode::SUCCESS;
            }
            Ok(_) => {}
            Err(why) => {
                eprintln!("viper: {why}");
                return ExitCode::FAILURE;
            }
        }
        // The status is ignored on purpose: a mistake at a prompt ends the line, not the session.
        if let Some(source) = prompt_line(&line) {
            let _ = run(engine, source, Show::Answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Asked, Command, read_arguments};

    /// The arguments, as a caller would pass them.
    fn read(arguments: &[&str], interactive: bool) -> Asked {
        let owned: Vec<String> = arguments.iter().map(|text| (*text).to_string()).collect();
        read_arguments(&owned, interactive)
    }

    #[test]
    fn a_bare_invocation_prompts_at_a_terminal_and_reads_the_pipe_otherwise() {
        // The one decision that cannot be made from the arguments alone, and the reason
        // `read_arguments` is handed the answer rather than asking: `cat x.js | ViperJS` has to
        // read, and a person typing `ViperJS` has to be given a prompt rather than a silent hang.
        assert_eq!(read(&[], true).command, Command::Prompt);
        assert_eq!(read(&[], false).command, Command::Stdin);
        // …and `-` says "the pipe" out loud, which is what makes it reachable from a terminal.
        assert_eq!(read(&["-"], true).command, Command::Stdin);
    }

    #[test]
    fn a_path_is_a_file_and_an_unknown_option_is_a_usage_error() {
        assert_eq!(
            read(&["script.js"], false).command,
            Command::File("script.js".to_string())
        );
        // A path that looks like an option is the case that separates the two arms. Anything
        // starting with `-` is refused rather than opened, because a typo'd flag silently read as a
        // filename is a confusing error much later.
        assert!(matches!(read(&["--nope"], false).command, Command::Bad(_)));
        // …and a file may still be named after options.
        assert_eq!(
            read(&["--time-budget", "50", "a.js"], false).command,
            Command::File("a.js".to_string())
        );
    }

    #[test]
    fn eval_takes_the_argument_after_it_and_complains_when_there_is_none() {
        assert_eq!(
            read(&["-e", "1 + 1"], false).command,
            Command::Source("1 + 1".to_string())
        );
        assert_eq!(
            read(&["--eval", "1 + 1"], false).command,
            Command::Source("1 + 1".to_string())
        );
        assert!(matches!(read(&["-e"], false).command, Command::Bad(_)));
        // The source is taken as *source*, not re-read as an option — `ViperJS -e "-h"` runs a
        // script, and consuming the argument is what makes that true.
        assert_eq!(
            read(&["-e", "-h"], false).command,
            Command::Source("-h".to_string())
        );
    }

    #[test]
    fn a_time_budget_is_milliseconds_and_anything_else_is_refused() {
        let asked = read(&["--time-budget", "250", "a.js"], false);
        assert_eq!(asked.budget, Some(std::time::Duration::from_millis(250)));
        assert_eq!(read(&["a.js"], false).budget, None);
        // Refused rather than ignored: a budget that silently did not apply would be worse than
        // none at all, this being the bound an embedder runs untrusted code behind.
        assert!(matches!(
            read(&["--time-budget", "soon"], false).command,
            Command::Bad(_)
        ));
        assert!(matches!(
            read(&["--time-budget"], false).command,
            Command::Bad(_)
        ));
        // A negative number is not a duration, and `parse::<u64>` is what says so.
        assert!(matches!(
            read(&["--time-budget", "-5"], false).command,
            Command::Bad(_)
        ));
    }

    #[test]
    fn help_and_version_win_over_everything_after_them() {
        // Asked for help, get help — not a file read, not a parse error about a script that was
        // never the point.
        assert_eq!(read(&["-h", "script.js"], false).command, Command::Help);
        assert_eq!(read(&["--help"], false).command, Command::Help);
        assert_eq!(read(&["-V", "script.js"], false).command, Command::Version);
        assert_eq!(read(&["--version"], false).command, Command::Version);
    }

    #[test]
    fn a_blank_line_at_the_prompt_runs_nothing() {
        // `Enter` on an empty line is how a person pauses to think; answering `undefined` at them
        // would be noise. Whitespace counts as blank, and a line with anything on it does not.
        assert_eq!(super::prompt_line(""), None);
        assert_eq!(
            super::prompt_line(
                "
"
            ),
            None
        );
        assert_eq!(
            super::prompt_line(
                "   	 
"
            ),
            None
        );
        // The source is handed on **unstripped** — trimming it would be a second, invisible edit to
        // what the person typed, and the parser has its own opinion about whitespace.
        assert_eq!(
            super::prompt_line(
                "  1 + 1  
"
            ),
            Some(
                "  1 + 1  
"
            )
        );
        assert_eq!(super::prompt_line("0"), Some("0"));
    }

    #[test]
    fn the_last_thing_named_is_what_runs() {
        // Two files is not an error worth inventing a message for — the last wins, as it does for
        // every other option here.
        assert_eq!(
            read(&["a.js", "b.js"], false).command,
            Command::File("b.js".to_string())
        );
        assert_eq!(
            read(&["a.js", "-e", "1"], false).command,
            Command::Source("1".to_string())
        );
    }
}
