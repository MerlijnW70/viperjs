---
id: DR-0002
title: No input may panic — script text is data, never a trusted caller
status: enforced
invariant: forbid-unsafe src/lib.rs
---

Every failure a script author can cause is a `Result`. Not a panic, not an abort, not a
`process::exit`. This is the invariant that decides whether praxis can be embedded at all.

The reasoning is about *whose* bug it is. An embedder runs untrusted script inside their own
process — that is the entire use case. If a 10⁶-deep nested array literal unwinds through their
request handler, the incident is theirs and the cause is ours. "That input is absurd" is not a
defence; absurd input is precisely what an attacker sends.

Concretely this forbids, in production paths:

- `unwrap()` / `expect()` on anything derived from source text, and `panic!` as error handling.
  (`expect("<invariant>")` remains legal where a panic would mean an *engine* bug the types
  cannot encode — a bytecode index the compiler provably emitted in range. Say which invariant.)
- Arithmetic that can overflow on attacker-chosen sizes. Offsets and depths are checked.
- **Unbounded recursion.** Parser and VM depth are capped explicitly, and exceeding the cap is a
  `RangeError` the script can catch — never a stack overflow, which no `Result` can rescue and
  which aborts the embedder's process outright. A recursive-descent parser gets this wrong by
  default, so the cap is written the same day the recursion is.

`#![forbid(unsafe_code)]` is the companion half: memory safety is not a thing we argue about
case by case. Together they are what makes "embed this in your binary" a reasonable request.

The gate enforces the mechanical parts (it flags bare `unwrap()`
and `.ok()?`); the depth caps and the overflow discipline are enforced by tests and by fuzzing
whatever was just built.
