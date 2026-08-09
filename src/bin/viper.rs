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
//! `node_modules`, and there will not be. What is bound is `print`, because a language with no way
//! to say anything is hard to evaluate, and a `console` of six logging methods — everything else is
//! the host's to provide, and this host provides nothing.
//!
//! **`console` is not a step across that line, and its absence used to be one.** It is a WHATWG
//! surface, in the Minimum Common API that browsers, workers, Deno and Bun all implement, and it is
//! no more Node's than `Math` is. Refusing it did not keep this host small; it made ordinary
//! libraries die on a ReferenceError in code that was never about output. Most of the console
//! specification is still absent — `group`, `table`, `time`, `count`, `assert`, `dir`, and the
//! Formatter's `%s` and `%d` — and finding those missing is better than finding a plausible fake.
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
use viperjs::parser::parse_module;
use viperjs::value::{Abrupt, Completion, Value};
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
    /// DR-0013's budget in bytes, if `--heap-budget` named one.
    heap: Option<usize>,
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
    let mut heap = None;
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
            // DR-0013's, in mebibytes rather than bytes: the number a person types is a size and
            // nobody thinks about it in bytes. A `0` is refused rather than taken as "no limit",
            // because a heap that may hand out nothing refuses the first allocation and would look
            // like the engine being broken.
            "--heap-budget" => match arguments.get(at).map(|text| text.parse::<usize>()) {
                Some(Ok(mib)) if mib > 0 => {
                    at += 1;
                    heap = Some(mib << 20);
                }
                _ => {
                    return Asked::of(Command::Bad(
                        "--heap-budget wants a whole number of mebibytes, above zero".into(),
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
        heap,
    }
}

impl Asked {
    /// This command, with no budget — the shape every early return wants.
    fn of(command: Command) -> Self {
        Self {
            command,
            budget: None,
            heap: None,
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
      --heap-budget <MiB>    how much memory a run may take before it refuses (default 64)
  -h, --help                 this text
  -V, --version              the version

The host binds `print` and a `console` of six logging methods — log, info and debug to standard
output, warn, error and trace to standard error. There is no `require`, no `fs` and no module
loading: GOAL.md §3 says the host provides I/O and this host provides almost none of it.

exit status: 0 ran, 1 the script threw or would not parse, 2 the arguments made no sense.
";

/// The base64 alphabet, in the order the HTML standard's forgiving-base64 numbers it.
/// The base64 alphabet, in the order the HTML standard's forgiving-base64 numbers it.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// What every "this is not base64" refusal answers with.
///
/// One function rather than the sentence repeated: they are the same refusal reached three ways —
/// a code point past U+00FF, a character outside the alphabet, and a `=` where no padding may be —
/// and the standard gives all three the same name.
///
/// A **TypeError** carrying that name rather than the `InvalidCharacterError` DOMException the
/// standard asks for, which this host has no way to make and no business inventing. The kind is
/// wrong, the reason is not, and a caller matching on the message still finds the name.
fn not_base64() -> Abrupt {
    Abrupt::type_error("InvalidCharacterError: this is not base64")
}

/// The HTML standard's base64 *encode*, over one byte per element.
///
/// Separated from the host function around it so that mutation coverage can see it. `tests/cli.rs`
/// exercises the binding by running the binary as a **subprocess**, and a mutation that only the
/// subprocess would notice survives — the mutant is compiled into a binary the test then does not
/// necessarily rebuild. So the arithmetic is tested here, where the mutant and the assertion are in
/// one process, and the integration test is left to prove the *binding* rather than the bit-work.
fn encode_base64(bytes: &[u8]) -> String {
    // No capacity hint. The arithmetic for one — four characters per three bytes — is a claim no
    // test can hold: a capacity too small or too large changes nothing but how often the String
    // grows, so mutation coverage reported it as untested and was right. This repository's rule for
    // an optimisation is that it earns a benchmark before it earns code, and none has asked.
    let mut out = String::new();
    for group in bytes.chunks(3) {
        // Padded to three so the shifts are one expression rather than three cases; what differs
        // between a full group and a short one is only how much of it is written out.
        let packed = u32::from(group[0]) << 16
            | u32::from(group.get(1).copied().unwrap_or(0)) << 8
            | u32::from(group.get(2).copied().unwrap_or(0));
        out.push(char::from(BASE64[(packed >> 18) as usize & 63]));
        out.push(char::from(BASE64[(packed >> 12) as usize & 63]));
        out.push(match group.len() {
            1 => '=',
            _ => char::from(BASE64[(packed >> 6) as usize & 63]),
        });
        out.push(match group.len() {
            3 => char::from(BASE64[packed as usize & 63]),
            _ => '=',
        });
    }
    out
}

/// The HTML standard's *forgiving*-base64 decode, answering one code unit per byte.
///
/// "Forgiving" is the standard's own word and it names three allowances rather than a general
/// leniency: ASCII whitespace is stripped anywhere, up to two trailing `=` are optional when the
/// length is already a multiple of four, and **nothing else** is tolerated. A length one past a
/// multiple of four is refused outright, because no number of bytes encodes to it.
fn decode_base64(text: &str) -> Completion<String> {
    // Step 1 — ASCII whitespace, which is the HTML infra set and **not** §12.2's: no U+000B, no
    // U+00A0 and no Unicode spaces. A wider set here would accept input a browser refuses.
    let mut trimmed: Vec<u8> = Vec::with_capacity(text.len());
    for point in text.chars() {
        if matches!(point, '\t' | '\n' | '\u{c}' | '\r' | ' ') {
            continue;
        }
        match u8::try_from(point as u32) {
            Ok(byte) => trimmed.push(byte),
            Err(_) => return Err(not_base64()),
        }
    }
    // Step 2 — the padding comes off only when the length is *already* a multiple of four, so
    // `"YQ="` stays four-and-a-remainder and is refused rather than quietly read as `"YQ"`.
    if trimmed.len().is_multiple_of(4) {
        for _ in 0..2 {
            if trimmed.last() == Some(&b'=') {
                trimmed.pop();
            }
        }
    }
    // Step 3 — one past a multiple of four encodes no number of bytes at all.
    if trimmed.len() % 4 == 1 {
        return Err(Abrupt::type_error(
            "InvalidCharacterError: this base64 has a length no number of bytes encodes to",
        ));
    }
    let mut out = String::with_capacity(trimmed.len() / 4 * 3);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in trimmed {
        let Some(index) = BASE64.iter().position(|known| *known == byte) else {
            return Err(not_base64());
        };
        buffer = buffer << 6 | index as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            // A byte, so the code unit is 0 to 255 and `from_u32` cannot fail. The `unwrap_or` is
            // the total spelling rather than a case any input reaches.
            out.push(char::from_u32(buffer >> bits & 0xff).unwrap_or('\u{0}'));
        }
    }
    Ok(out)
}

/// `btoa(data)` — base64 encode, over one byte per code unit.
///
/// Not ECMAScript, and here on the same terms as `console`: it is in the Minimum Common API, every
/// host has it, and it is pure arithmetic — no clock, no filesystem, no entropy. `entities`, which
/// `htmlparser2` and `cheerio` are built on, decodes its own tables with `atob` while it loads and
/// dies on a ReferenceError without it. That is how this was found.
///
/// **A code point above U+00FF is refused.** `btoa` encodes *bytes* and a String holds code units,
/// so there is no encoding to choose and the standard refuses rather than choosing one.
fn btoa(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = Host::new(vm, heap).text(call.argument(0))?;
    let mut bytes = Vec::with_capacity(text.len());
    for point in text.chars() {
        match u8::try_from(point as u32) {
            Ok(byte) => bytes.push(byte),
            Err(_) => {
                return Err(Abrupt::type_error(
                    "InvalidCharacterError: btoa encodes bytes, and this string has a code point above U+00FF",
                ));
            }
        }
    }
    let encoded = encode_base64(&bytes);
    Ok(Host::new(vm, heap).string(&encoded))
}

/// `atob(data)` — forgiving-base64 decode, as one code unit per byte.
///
/// The answer is a String of code units 0 to 255 — bytes, not text. Deliberately: what the bytes
/// *mean* is the caller's, which is why `entities` reads them back with `charCodeAt` and builds its
/// own `Uint16Array` rather than asking for a decoded string.
fn atob(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = Host::new(vm, heap).text(call.argument(0))?;
    let decoded = decode_base64(&text)?;
    Ok(Host::new(vm, heap).string(&decoded))
}

/// §19's `print` for a command line: write one argument and a newline.
///
/// Kept beside `console` rather than replaced by it. This is what a script written *for* this host
/// uses, it takes one argument and says so in its `length`, and the conformance harness and every
/// example depend on it.
fn print(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    let text = host.text(call.argument(0))?;
    // A closed pipe is not this program's problem — `viper big.js | head` is an ordinary thing to
    // write, and the script goes on running as it would with any other host.
    let _ = writeln!(std::io::stdout(), "{text}");
    Ok(Value::Undefined)
}

/// Every argument, converted and joined by a space — what a `console` method writes.
///
/// The conversion is `ToString` per argument, which is `String(x)` and not `JSON.stringify`: an
/// object comes out as `[object Object]`. The WHATWG console's *Formatter* — `%s`, `%d`, `%o` and
/// the rest — is deliberately not implemented, and a first argument containing one is written out
/// as it stands rather than being read as a directive.
fn joined(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<String> {
    let mut host = Host::new(vm, heap);
    let mut out = String::new();
    for (at, value) in call.arguments.iter().enumerate() {
        if at > 0 {
            out.push(' ');
        }
        out.push_str(&host.text(*value)?);
    }
    Ok(out)
}

/// `console.log`, `console.info` and `console.debug` — to standard output.
fn console_out(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = joined(vm, heap, call)?;
    let _ = writeln!(std::io::stdout(), "{text}");
    Ok(Value::Undefined)
}

/// `console.warn`, `console.error` and `console.trace` — to standard error.
///
/// The split matters more here than it looks: a library writing a deprecation notice must not end
/// up inside the output a script is piped into. Node and every browser divide them the same way,
/// and a host that sent everything to one stream would silently corrupt `viper x.js > out.json`.
fn console_err(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = joined(vm, heap, call)?;
    let _ = writeln!(std::io::stderr(), "{text}");
    Ok(Value::Undefined)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let asked = read_arguments(&arguments, std::io::stdin().is_terminal());
    let mut engine = Engine::new();
    engine.bind("print", 1, print);
    // The HTML standard's two, and the same argument that admitted `console`: not
    // ECMAScript, in the Minimum Common API, and pure arithmetic. `entities` — which
    // `htmlparser2` and `cheerio` are built on — decodes its own tables with `atob` while it
    // loads, so without them a very common dependency chain dies on a ReferenceError before
    // any of the program has run.
    engine.bind("atob", 1, atob);
    engine.bind("btoa", 1, btoa);
    // Six methods and no more. Real libraries reach for `console.warn` to report a deprecation and
    // `console.error` to report a failure they are handling; without one they die on a
    // ReferenceError in code that was never about output. `ajv` — a top-20 package — does exactly
    // that, which is how this got noticed.
    //
    // This is not a step toward Node compatibility, which GOAL.md §3 refuses: `console` is a
    // WHATWG surface that browsers, workers, Deno and Bun all have, and it is in the Minimum Common
    // API. What is *not* here is most of the specification — `group`, `table`, `time`, `count`,
    // `assert`, `dir` and the Formatter's `%s`/`%d` directives — so a program that needs those will
    // still find them missing, and finding them missing is better than finding a plausible fake.
    engine.bind_namespace(
        "console",
        &[
            ("log", 0, console_out),
            ("info", 0, console_out),
            ("debug", 0, console_out),
            ("warn", 0, console_err),
            ("error", 0, console_err),
            ("trace", 0, console_err),
        ],
    );
    engine.set_time_budget(asked.budget);
    if let Some(bytes) = asked.heap {
        engine.set_heap_budget(bytes);
    }

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
        // A Script parse that fails on text which *is* a Module deserves to say so. Without this
        // the message is about the token the parser tripped on — "expected an expression, found
        // `export`" — which is true and tells a person nothing about why their file will not run.
        Err(Error::Syntax { message: said, .. }) if is_module(source) => {
            eprintln!("viper: this is Module code — it parses under §11.2's Module goal but not");
            eprintln!("       as a Script, which is what viper runs. A Module's imports need a");
            eprintln!("       resolver, and GOAL.md §3 leaves that to the host: see");
            eprintln!("       viperjs::api::Engine to supply one.");
            eprintln!("       as a Script it said: {said}");
            ExitCode::FAILURE
        }
        // A syntax error says *where*, which the one-line `describe` cannot: it has no source text
        // to resolve a span against, and every other kind of failure has no span to resolve.
        Err(Error::Syntax { message, span }) => {
            eprintln!("viper: SyntaxError: {message}");
            for line in point_at(source, span) {
                eprintln!("{line}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("viper: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

/// The line and column a span starts at, and the source line under a caret.
///
/// The same shape `examples/parse` prints, because a person reading one has already read the other
/// and two spellings of one idea is one too many. Written here rather than shared: the example is an
/// example, and a binary that depended on it would make it load-bearing.
///
/// Answers **nothing** when the span points past the last line, which the end of input does — there
/// is no line to underline, and a caret under empty space is worse than none. The position itself is
/// always on the first line of the answer, so a span at the end of input still says where.
fn point_at(source: &str, span: viperjs::span::Span) -> Vec<String> {
    let at = viperjs::span::line_col(source, span.start);
    let mut said = vec![format!("  at line {}, column {}", at.line, at.column)];
    // `lines` and `line_col` agree about CRLF. The filter is the end-of-input case: a column one
    // past the last character is a legal position and not a place a caret can go.
    let Some(text) = source
        .lines()
        .nth(at.line as usize - 1)
        .filter(|line| (at.column as usize) <= line.chars().count() + 1)
    else {
        return said;
    };
    said.push(format!("    {text}"));
    // Tabs are one character to `line_col` and eight columns to a terminal, so the padding copies
    // the line's own leading whitespace rather than counting spaces — which is what keeps the caret
    // under the token in a file indented with tabs.
    let pad: String = text
        .chars()
        .take(at.column as usize - 1)
        .map(|found| if found == '\t' { '\t' } else { ' ' })
        .collect();
    let width = usize::max(span.len() as usize, 1);
    said.push(format!("    {pad}{}", "^".repeat(width)));
    said
}

/// Whether this is Module code, asked only once a Script parse has already failed.
///
/// **Asked by parsing rather than by guessing**, because the two goal symbols of §11.2 differ in
/// exactly what they accept and nothing else can answer it: a file extension is a convention the
/// language does not have, and scanning for `import` finds it in `import("x")`, in a property
/// called `export`, and in a string.
///
/// The direction matters and makes this precise. Module code is strict, so nearly everything a
/// Script accepts and a Module does not would fail *both* parses; what a Module accepts and a
/// Script cannot is `import` and `export` declarations and a top-level `await`. So a source that
/// fails as a Script and parses as a Module is one of those, which is the sentence below.
///
/// Only reached after a failure, so a program that runs pays nothing for it.
fn is_module(source: &str) -> bool {
    parse_module(source).is_ok()
}

/// What went wrong, in one line, for a person reading a terminal.
fn describe(error: &Error) -> String {
    match error {
        // Reached only where there is no source to point into — `describe` is the one-line form,
        // and the caller that *has* the program prints the position and the caret itself.
        Error::Syntax { message, .. } => format!("SyntaxError: {message}"),
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
    use super::*;

    /// `point_at`, asserted on its exact output rather than through the binary.
    ///
    /// **Here rather than in `tests/cli.rs` deliberately.** An integration test runs `viper` as a
    /// subprocess, so mutation coverage cannot see a decision that only the subprocess would
    /// notice — this function produced six survivors in one run, every one of them arithmetic that
    /// puts a caret in the wrong place. A caret under the wrong token is worse than no caret,
    /// because it is believed. The integration test's job is to prove the *binding*: that a syntax
    /// error reaches this at all.
    #[test]
    fn a_caret_lands_under_the_token_the_span_names() {
        let source = "var a = 1;\nvar b = ;\nvar c = 3;";
        // The `;` on line 2 is at offset 19. Picking the wrong line, or the right line off by one,
        // shows a row of the program that parsed perfectly well.
        let said = point_at(source, viperjs::span::Span::new(19, 20));
        assert_eq!(
            said,
            vec![
                "  at line 2, column 9".to_string(),
                "    var b = ;".to_string(),
                "            ^".to_string(),
            ]
        );

        // A span at the very end of input: the column is one *past* the last character, which is a
        // legal position and the place a missing `}` is missing from. The line is still worth
        // showing, and the boundary is exactly where an off-by-one stops showing it.
        let source = "function f() {";
        let said = point_at(source, viperjs::span::Span::new(14, 14));
        assert_eq!(said.len(), 3, "the line is shown at the end of input");
        assert_eq!(said[0], "  at line 1, column 15");
        assert_eq!(said[2], "                  ^");

        // Past the end of the last line there is nothing to underline, so the position is given
        // alone rather than with a caret under empty space. An offset beyond the source resolves to
        // the position *after* the final line terminator — line 2 of a one-line file — and
        // `str::lines` does not yield a line there, which is what the absent caret is.
        let said = point_at("ab\n", viperjs::span::Span::new(90, 91));
        assert_eq!(said, vec!["  at line 2, column 1".to_string()]);

        // Tabs are one character to a column count and eight to a terminal, so the padding copies
        // the line's own whitespace. Counting spaces instead puts the caret seven columns left of
        // the token on a file indented with tabs, which is most files that use them.
        let source = "function f() {\n\tconst x = [1, 2;\n}";
        let semicolon = source.find(';').expect("the source has one") as u32;
        let said = point_at(source, viperjs::span::Span::new(semicolon, semicolon + 1));
        assert_eq!(said[0], "  at line 2, column 17");
        assert_eq!(said[1], "    \tconst x = [1, 2;");
        // Written as a repeat rather than as a run of literal spaces, because counting them by eye
        // is how the first version of this row was wrong — and a caret test that is off by one is
        // exactly the defect it exists to catch.
        assert_eq!(
            said[2],
            format!("    \t{}^", " ".repeat(15)),
            "the padding is a tab followed by spaces, not eight spaces"
        );

        // A span wider than one character underlines all of it, and a zero-width one still gets a
        // caret rather than nothing.
        let source = "let value = notdefined;";
        assert_eq!(
            point_at(source, viperjs::span::Span::new(12, 22))[2],
            "                ^^^^^^^^^^"
        );
        assert_eq!(point_at("abc", viperjs::span::Span::new(1, 1))[2], "     ^");
    }

    #[test]
    fn base64_encodes_every_group_length_and_pads_each_the_way_the_standard_says() {
        // The three lengths a group can end on, each with its own amount of padding — and the
        // padding is what a shift or a mask off by one place gets wrong while still looking like
        // base64. A round trip alone does not catch that: two mistakes can cancel.
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"a"), "YQ==");
        assert_eq!(encode_base64(b"ab"), "YWI=");
        assert_eq!(encode_base64(b"abc"), "YWJj");
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        // The high bytes, which reach the top of the alphabet where the low ones never do.
        assert_eq!(encode_base64(&[0xff, 0xfe]), "//4=");
        assert_eq!(encode_base64(&[0x00, 0x00, 0x00]), "AAAA");
        assert_eq!(encode_base64(&[0xff, 0xff, 0xff]), "////");
        // One byte per position, so a shift that reads the wrong third of the group shows up.
        assert_eq!(encode_base64(&[0xff, 0x00, 0x00]), "/wAA");
        assert_eq!(encode_base64(&[0x00, 0xff, 0x00]), "AP8A");
        assert_eq!(encode_base64(&[0x00, 0x00, 0xff]), "AAD/");
    }

    #[test]
    fn base64_decodes_what_it_encoded_for_every_byte_there_is() {
        // Every value, which is the only test that covers the alphabet's edges: a table with one
        // character out of place round-trips almost everything and fails on a handful.
        let all: Vec<u8> = (0..=255).collect();
        let encoded = encode_base64(&all);
        let back = decode_base64(&encoded).expect("what we encoded decodes"); // the test is the round trip
        assert_eq!(
            back.chars().map(|c| c as u32).collect::<Vec<_>>(),
            (0..=255).collect::<Vec<u32>>()
        );
        // …and the encoding itself is the standard's, not merely self-consistent.
        assert_eq!(&encoded[..8], "AAECAwQF");
    }

    #[test]
    fn forgiving_base64_forgives_exactly_three_things() {
        // Whitespace anywhere, and it is *ASCII* whitespace — the HTML infra set. U+000B and
        // U+00A0 look blank and are not on it, so a string a reader would call empty is refused.
        assert_eq!(
            decode_base64(" a G V s b G 8 = ").ok().as_deref(),
            Some("hello")
        );
        assert_eq!(decode_base64("aGVs\nbG8=").ok().as_deref(), Some("hello"));
        assert!(decode_base64("aGVs\u{b}bG8=").is_err());
        assert!(decode_base64("aGVs\u{a0}bG8=").is_err());

        // The padding, optional only when the length is *already* a multiple of four — so `YQ=`
        // stays four-and-a-remainder and is refused rather than read as `YQ`.
        assert_eq!(decode_base64("aGVsbG8").ok().as_deref(), Some("hello"));
        assert_eq!(decode_base64("YQ==").ok().as_deref(), Some("a"));
        assert!(decode_base64("YQ=").is_err());

        // And a length one past a multiple of four, which no number of bytes encodes to.
        assert!(decode_base64("a").is_err());
        assert!(decode_base64("aGVsb").is_err());

        // Nothing else is forgiven.
        for bad in ["!!!!", "a=b=", "ab-d", "ab_d", "\u{100}"] {
            assert!(decode_base64(bad).is_err(), "for {bad:?}");
        }
        assert_eq!(decode_base64("").ok().as_deref(), Some(""));
    }

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
