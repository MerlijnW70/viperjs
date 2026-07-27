# Contributing to praxis

Thank you for looking. Read [GOAL.md](GOAL.md) first — it is the binding charter and it outranks
everything here. [AGENTS.md](AGENTS.md) has the milestone plan and the house style.

## The shape of the work

This project is **long, not hard**. There is no breakthrough waiting; there are roughly 50,000
test262 tests, each of which either passes or does not. Three consequences follow, and they are
most of what a review will be about:

- **Never guess at the spec.** ECMA-262 is online and unambiguous. Every non-obvious behaviour
  gets a comment citing its section (`// ECMA-262 §13.15.2 — assignment evaluates the target
  reference BEFORE the value`). "I think JS does X" is how an engine acquires a bug that takes
  three months to find.
- **Prefer the boring implementation.** The clever one costs you at every conformance edge case.
  Optimise when a benchmark says to, not when an intuition does — and prototype it in
  [`lab/`](lab/README.md) first.
- **Land small.** One coherent slice per pull request. A 3,000-line "parser done" change cannot
  be reviewed and cannot be bisected.

## Before you open a pull request

```
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --no-deps --workspace          # missing_docs is denied
```

CI runs all of these on Linux, Windows and macOS, plus a check against the minimum supported
Rust version (1.88). Two of the charter's guarantees are also checked mechanically there: the
dependency table is empty, and `#![forbid(unsafe_code)]` is still in `src/lib.rs`.

## What a review looks for

**Tests that fail when the logic is wrong.** A test that merely passes is worth very little. The
question asked of every new test is: if the branch it covers were subtly wrong, would this test
notice? If a branch genuinely cannot be distinguished by any input, that is a design signal —
such a branch usually should not exist, and the arithmetic or the types can often say the same
thing with nothing left to guard.

**Tests that assert the spec, not the code.** The cheapest way to make a failing test pass is to
assert what the code currently does. Such a test pins the bug in place permanently. In
spec-sensitive areas, cite the section and assert what it says.

**Errors that read like a good compiler's.** Errors are values with spans. No `unwrap()` in
production paths; `expect("<invariant>")` only where a panic would mean an engine bug the types
cannot encode.

**Tests named as sentences about behaviour** — `a_crlf_pair_ends_one_line_not_two`, not
`test_line_col_2`. `src/span.rs` is the worked example of the bar for the whole repository;
match its density of intent.

## Things that will be declined

- **A runtime dependency.** The table in `Cargo.toml` stays empty, forever (DR-0001). If you
  believe you need a crate, open an issue and say so rather than opening a pull request.
- **`unsafe`.** Crate-wide, no exceptions (DR-0002).
- **A performance change without a measurement.** Prototype it in `lab/`, write the verdict in
  `lab/NOTES.md`, then implement it in `src/` from scratch — never by copying the spike.
- **A conformance regression.** The expectations file may shrink, never grow.

## Architectural changes

Anything architectural gets a decision record in [`decisions/`](decisions): one file, the
reasoning in prose, and the invariant it implies. The existing eight are worth reading before
proposing the ninth — they are short.

## Licence

By contributing you agree that your work is dual-licensed under
[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), matching the project.
