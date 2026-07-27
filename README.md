# praxis

An embeddable JavaScript engine in safe Rust, with **zero runtime dependencies**.

> Status: **M0 — foundations.** The engine does not run JavaScript yet. Conformance: not yet
> measured. See [GOAL.md](GOAL.md) for the charter and [AGENTS.md](AGENTS.md) for the milestones.

## Why

If you want to embed a scripting language in a Rust binary today, the practical options are a
C engine with its own memory-safety history, or a large runtime with a dependency tree bigger
than your application. praxis aims at the gap: QuickJS's niche — small, embeddable, spec-faithful
— without `unsafe` and without pulling in a single crate.

Three properties are non-negotiable, and they are what the project is actually about:

- **No `unsafe`.** `#![forbid(unsafe_code)]`, crate-wide.
- **No input panics.** Untrusted script is *input*, not an exception. Syntax errors, absurd
  literals, pathological nesting — all are `Result`, never a crash in the embedder's process.
- **Conformance is measured, not claimed.** test262 is the arbiter, and the number may only go up.

## Build

```
cargo test           # the engine
cargo run -p praxis-lab   # experiments (see lab/README.md)
```

## Licence

MIT OR Apache-2.0.
