//! The `praxis` binary, run as a process.
//!
//! # Why these are out here rather than beside it
//!
//! Everything the command line decides *before* it acts is a pure function and is tested in
//! `src/bin/praxis.rs`. What is left is the acting: reading a file, printing a completion value or
//! declining to, and choosing an exit status. None of that is reachable from a unit test — it is
//! the process's behaviour, not a function's — and it is exactly what a person trying the engine
//! meets first.
//!
//! This is DR-0021's lesson applied once more. `examples/embed.rs` found four faults in the
//! embedding surface that fourteen unit tests inside the crate could not, because it was outside
//! the crate. These are outside the *binary*, for the same reason.

use std::io::Write;
use std::process::{Command, Stdio};

/// Run the binary with these arguments and answer its stdout, stderr and exit status.
fn praxis(arguments: &[&str], stdin: &str) -> (String, String, Option<i32>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_praxis"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary was built");
    // Dropped straight after writing, which closes the pipe — otherwise a run that reads standard
    // input waits for an end that never comes and the test hangs rather than fails.
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes()).expect("the pipe takes it");
    }
    let done = child.wait_with_output().expect("it finishes");
    (
        String::from_utf8_lossy(&done.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&done.stderr).replace("\r\n", "\n"),
        done.status.code(),
    )
}

#[test]
fn a_multi_line_file_runs_as_one_script() {
    // The gap this binary was written for. `examples/evaluate` reads one script per *line*, so this
    // source came apart into eight fragments and reported eight parse errors — there was no way to
    // run an ordinary `.js` file at all.
    let directory = std::env::temp_dir().join("praxis-cli-test");
    std::fs::create_dir_all(&directory).expect("a place to put a file");
    let path = directory.join("fib.js");
    std::fs::write(
        &path,
        "function fib(n) {\n  return n < 2 ? n : fib(n - 1) + fib(n - 2);\n}\n\
         var out = [];\nfor (var i = 0; i < 10; i++) {\n  out.push(fib(i));\n}\n\
         print(out.join(','));\n",
    )
    .expect("it writes");

    let (out, err, status) = praxis(&[path.to_str().expect("a path")], "");
    assert_eq!(out, "0,1,1,2,3,5,8,13,21,34\n");
    assert_eq!(err, "");
    assert_eq!(status, Some(0));
}

#[test]
fn a_file_prints_only_what_it_printed_and_minus_e_prints_the_answer() {
    // §14.2.2's completion value is not a return value. A script whose last statement happens to be
    // an expression must not spray it at the terminal — while `-e` exists *for* the answer, which
    // is the whole difference between the two and the one thing `Show` decides.
    let directory = std::env::temp_dir().join("praxis-cli-test");
    std::fs::create_dir_all(&directory).expect("a place to put a file");
    let path = directory.join("quiet.js");
    std::fs::write(&path, "var n = 1 + 1;\nn;\n").expect("it writes");

    let (out, _, status) = praxis(&[path.to_str().expect("a path")], "");
    assert_eq!(out, "", "a file's completion value is not output");
    assert_eq!(status, Some(0));

    let (out, _, status) = praxis(&["-e", "1 + 1"], "");
    assert_eq!(out, "2\n", "`-e` is asked for the answer");
    assert_eq!(status, Some(0));

    // …and standard input is a file, not an expression — same rule.
    let (out, _, status) = praxis(&[], "var n = 1 + 1;\nn;\n");
    assert_eq!(out, "");
    assert_eq!(status, Some(0));
}

#[test]
fn a_piped_script_runs_whole_rather_than_a_line_at_a_time() {
    let (out, err, status) = praxis(&[], "function f(a) {\n  return a * 3;\n}\nprint(f(14));\n");
    assert_eq!(out, "42\n");
    assert_eq!(err, "");
    assert_eq!(status, Some(0));
    // `-` says the same thing out loud, which is the spelling that works from a terminal.
    let (out, _, status) = praxis(&["-"], "print('read');\n");
    assert_eq!(out, "read\n");
    assert_eq!(status, Some(0));
}

#[test]
fn the_exit_status_tells_a_shell_which_kind_of_wrong_it_was() {
    // Three outcomes a caller scripts against, and they are deliberately different numbers: a
    // script that threw is not the same event as a command line nobody could read.
    let (_, err, status) = praxis(&["-e", "throw new TypeError('nope')"], "");
    assert_eq!(status, Some(1));
    assert!(err.contains("TypeError: nope"), "{err}");

    let (_, err, status) = praxis(&["-e", "var x = ;"], "");
    assert_eq!(status, Some(1));
    assert!(err.contains("SyntaxError"), "{err}");

    let (_, err, status) = praxis(&["--nope"], "");
    assert_eq!(status, Some(2));
    assert!(err.contains("unknown option"), "{err}");

    let (_, err, status) = praxis(&["no-such-file-anywhere.js"], "");
    assert_eq!(status, Some(2));
    assert!(err.contains("no-such-file-anywhere.js"), "{err}");
}

#[test]
fn a_time_budget_stops_a_loop_the_script_tries_to_catch() {
    // DR-0022, from the outside. The budget is not a throw — a script cannot catch it, which is
    // what makes it a bound on untrusted code rather than a suggestion — so this `catch` never
    // runs and the process still ends.
    let (out, err, status) = praxis(
        &[
            "--time-budget",
            "150",
            "-e",
            "try { while (true) {} } catch (e) { 'caught' }",
        ],
        "",
    );
    assert_eq!(out, "", "nothing was produced, so nothing is printed");
    assert!(err.contains("time budget"), "{err}");
    assert_eq!(status, Some(1));

    // …and a budget the run does not spend changes nothing.
    let (out, _, status) = praxis(&["--time-budget", "10000", "-e", "1 + 1"], "");
    assert_eq!(out, "2\n");
    assert_eq!(status, Some(0));
}

#[test]
fn help_and_version_answer_on_stdout_and_succeed() {
    // A help text on stderr with a failing status is a small cruelty to anyone piping it to a
    // pager, and asking for help is not an error.
    let (out, err, status) = praxis(&["--help"], "");
    assert!(out.contains("usage:"), "{out}");
    assert_eq!(err, "");
    assert_eq!(status, Some(0));

    let (out, _, status) = praxis(&["-V"], "");
    assert!(out.starts_with("praxis "), "{out}");
    assert_eq!(status, Some(0));
}

#[test]
fn the_host_binds_print_and_nothing_else() {
    // GOAL.md §3 — the host provides I/O and this host provides almost none. Naming what is absent
    // is the point: a `console` or a `require` arriving by accident is exactly the drift toward
    // Node compatibility the charter refuses, and this is what would notice.
    let (out, _, status) = praxis(
        &[
            "-e",
            "[typeof print, typeof console, typeof require, typeof process, typeof globalThis].join()",
        ],
        "",
    );
    assert_eq!(out, "function,undefined,undefined,undefined,object\n");
    assert_eq!(status, Some(0));
}
