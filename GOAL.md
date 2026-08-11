# GOAL.md — the binding charter

**ViperJS** is an embeddable JavaScript engine written in safe Rust with zero runtime
dependencies, whose conformance is measured continuously against test262 and may only improve.

This file is binding. Every change must comply with it; where `AGENTS.md` and this file
disagree, this file wins.

## §1 What we are building

A small, correct, embeddable ECMAScript engine — the niche QuickJS occupies, in a language that
does not hand you a memory-safety CVE for your trouble. The target embedder is someone putting
a scripting layer inside their own binary: an edge runtime, a plugin host, a game, a device.

**Correctness before speed, always.** A fast engine that is subtly wrong is worthless; a correct
engine that is slow is a performance project. We take the second problem.

## §2 The five non-negotiables

1. **Zero runtime dependencies.** Pure `std`. The `[dependencies]` table stays empty forever.
   The lab may depend on anything; the engine may not.
2. **No `unsafe`.** `#![forbid(unsafe_code)]` at the crate root. If a performance milestone
   genuinely requires it (NaN-boxing does not; a custom allocator might), it needs a decision
   record and a measurement first — not a hunch.
3. **No input may panic.** Every failure a script author can cause is a `Result`. Syntax errors,
   1 GB literals, 10⁶-deep nesting, lone surrogates, `\0` in source — all are inputs, not
   exceptions. An embedder runs untrusted code inside their process; our panic is their outage.
   Stack depth is bounded explicitly, never by hitting the OS stack guard.
4. **Conformance may only improve.** The test262 expectations file may shrink, never grow. A
   change that breaks a passing test is a regression, full stop — no "temporarily".
5. **Every change is proven tested.** Mutation testing shows zero new survivors, the gate
   is green. Untested logic does not enter, however obviously correct it looks.

## §3 What we are explicitly NOT building

Naming these keeps them from arriving by accident, one reasonable-sounding PR at a time:

- **No JIT.** Not in 1.0. A bytecode interpreter that is right beats a JIT that is nearly right,
  and a JIT doubles the surface every conformance bug can hide in.
- **No Node.js compatibility.** No `require`, no `fs`, no event loop beyond the ECMAScript job
  queue. The host provides I/O; we provide the language.
- **No Intl / full Unicode collation.** Stubs that throw honestly, not approximations that lie.
- **No threads inside the engine.** One thread, and no parallelism inside it. Embedders get
  isolation by running more engines, which is cheap when the engine is small. (This read "one
  realm, one thread" until 2026-08-08. The realm half was never what this bullet refuses — §9.3's
  realms are ECMA-262's own, §4 makes the suite the arbiter, and a second one adds no thread and
  grants no isolation. DR-0025 records what moved, and that it was built before this line was
  read.)

  **What §9.7's agents added, and what they did not — 2026-08-10.** The engine still starts no
  thread: `SharedArrayBuffer`'s bytes are one allocation behind an `Arc` that any number of *heaps*
  may hold, and a host that wants a second agent runs a second engine on a thread of its own, which
  is exactly what this bullet already said embedders should do. So the refusal stands as written and
  the sentence "no parallelism inside it" now needs the qualifier: two engines sharing a buffer are
  parallel with each other, and §25.4's blocking `Atomics` are how they agree about it. What is
  still refused is the engine spawning anything.
- **No `eval` of native code, no FFI.** The host binds functions in; nothing escapes outward.

## §4 The oracle

test262 — roughly 50,000 tests maintained by TC39 — is the arbiter. Not our opinion of the
spec, not our own test suite: the same suite every other engine is measured by.

This matters more than it sounds. It means progress is a number that goes up, visible every
night, and it means the engine cannot drift into "correct according to us". Where test262 and
our reading of ECMA-262 disagree, **test262 wins** and our reading was wrong.

## §5 Definition of done, per milestone

A milestone is done when all four hold — not three:

1. Its features work on real scripts, not just unit tests.
2. Mutation testing reports zero new survivors over the milestone's code.
3. The test262 expectations file shrank by the milestone's target, and nothing regressed.
4. The public API additions are documented, with an example that compiles.

## §6 Release posture

Private until it can pass a conformance bar worth publishing (target: ES5 complete, ES2015 core
substantially complete). At release, the internal development toolchain and
everything under `lab/` is **cut away entirely**. The shipped artifact is a Rust crate with an
empty dependency table, its own tests, and a conformance report. How it was built is our
business; what it does is the user's.
