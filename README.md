<div align="center">

# praxis

**An embeddable JavaScript engine in safe Rust, with zero runtime dependencies.**

[![CI](https://github.com/MerlijnW70/praxis/actions/workflows/ci.yml/badge.svg)](https://github.com/MerlijnW70/praxis/actions/workflows/ci.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Dependencies: 0](https://img.shields.io/badge/dependencies-0-brightgreen.svg)](Cargo.toml)
[![unsafe: forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](src/lib.rs)

</div>

> **Status: not usable yet.** praxis does not execute JavaScript. The lexer is complete and the
> parser covers most of the modern grammar; there is no interpreter, so there is no conformance
> number to report. See [the milestones](AGENTS.md#milestones) for what lands when, and
> [GOAL.md](GOAL.md) for the charter that binds all of it.

## Why

If you want to embed a scripting language in a Rust binary today, the practical options are a C
engine with its own memory-safety history, or a large runtime whose dependency tree is bigger
than your application. praxis aims at the gap: QuickJS's niche — small, embeddable,
spec-faithful — without `unsafe` and without pulling in a single crate.

Three properties are non-negotiable, and they are what the project is actually about:

- **No `unsafe`.** `#![forbid(unsafe_code)]`, crate-wide. Not something to be traded away later
  for a benchmark.
- **No input panics.** Untrusted script is *input*, not an exception. Syntax errors, absurd
  literals, pathological nesting — all are `Result`, never a crash in the embedder's process.
  Nesting is bounded by an explicit count rather than by hitting the OS stack guard, and a
  full-depth parse is tested against one mebibyte of stack.
- **Conformance is measured, not claimed.** test262 is the arbiter, and the number may only go
  up. Until there is a number, this README will not imply one.

## What works today

| Area | State |
| --- | --- |
| **Lexer** | Complete. Every token form of ECMA-262 §12: identifiers over the real Unicode `ID_Start`/`ID_Continue` sets, all numeric literals (Annex B legacy forms, separators, BigInt), strings and templates as UTF-16 code units, regular-expression literals under a parser-supplied goal symbol, all four line terminators. |
| **Parser** | Statements, the full expression grammar, ASI, functions, classes with fields, static blocks and private names, generators, `async`/`await`, arrow functions, destructuring, optional chaining, `import()`/`import.meta`, and modules — each with the early errors the spec demands. |
| **Static semantics** | Declared names and their collisions, label resolution, strict-mode rules. |
| **Interpreter** | Not started. This is M3. |
| **Built-ins, conformance harness** | Not started. M4 and M5. |

Errors are values with spans, and are meant to read like a good compiler's:

```
$ cargo run --example parse -- demo.js
    1 x  strict mode code may not `delete` a name, only a property
         demo.js:3:8
           delete x;
                  ^
```

## Try it

There is no embedding API yet — the only thing to run is the parser, over a single file or a
whole tree:

```sh
cargo run --release --example parse -- path/to/file.js     # one file
cargo run --release --example parse -- --commonjs path/    # sweep a repository
cargo run --release --example parse -- --tree file.mjs     # print the syntax tree
```

The sweep skips `node_modules`, groups failures by kind, and reports how much it read. It exists
to find grammar bugs against real-world code, and it is how several were found.

## Build

```sh
cargo test --workspace      # the engine and its tests
cargo doc --no-deps --open  # the API, such as it is
```

Requires Rust 1.85 or later (edition 2024).

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) has the process; [AGENTS.md](AGENTS.md) has the milestone plan
and the house style. Two things are worth knowing before you start: no change may add a runtime
dependency or any `unsafe`, and a test that merely passes is worth very little — it has to *fail*
when the logic is wrong.

Architectural changes get a decision record in [`decisions/`](decisions). The existing eight are
short, and are the fastest way to understand why the engine is shaped as it is.

## Licence

Dual-licensed, at your option, under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work, as defined in the Apache-2.0 licence, shall be dual-licensed as above, without any
additional terms or conditions.
