# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) — with the pre-1.0 caveat that the
public API is not stable and may change in any release.

## [Unreleased]

## [0.2.2] — 2026-08-06

### Security

- **A `BigInt` division could panic in the embedder's process** — GHSA-6976-qm5m-7mcj,
  reported privately by [@Zniece](https://github.com/Zniece). `1n / ((1n << 33554399n) * 2n)`
  ended the process with an index out of bounds, which for an engine whose whole promise is that
  no input may panic is the invariant itself failing. Affects every release up to and including
  0.2.1.

  The cause was one comparison against the wrong number: a left shift reserved a limb for what it
  might push off the top, measured *that* against the size ceiling, and trimmed it away afterwards
  — so a magnitude landing exactly on the ceiling was refused for room it never keeps. The refusal
  was then read as impossible by the division and discarded, leaving an empty divisor to be
  indexed at `usize::MAX`.

  Two silent wrong answers came from the same line and are fixed with it: `d % 7n` answered `0n`
  where the true remainder is `1n`, and `String(d)` answered `"0"`. Both now answer correctly.

### Changed

- `BigInt::to_digits` returns a `Result`. A magnitude this engine cannot divide has no decimal
  form, and §6.1.4 requires an implementation that imposes a limit to throw rather than to answer
  something else — so `String(x)` and `x.toString()` raise a RangeError where they used to spell
  `"0"`. Pre-1.0 and breaking, as the note at the top of this file allows.
- A left shift that lands **exactly** on the size ceiling is now allowed, where it was refused for
  the transient limb. `1n << n` therefore succeeds for one more value of `n` than before.


## [0.2.1] — 2026-08-05

### Fixed

- **The crate-level documentation described a crate that no longer exists.** It called the public
  surface "deliberately tiny" directly above the fourteen public modules docs.rs lists, and pointed
  readers at `AGENTS.md` — a file the packaged crate deliberately does not ship, so the reference
  resolved to nothing for anyone arriving from crates.io or docs.rs. It now names [`api`] as the
  entry point, says the wide surface is deliberate, and links only to things inside the crate.

## [0.2.0] — 2026-08-05

The first released version. `0.1.0` was never published; this is what the crate has grown into
since, and the headline is that the engine **runs JavaScript** rather than merely reading it.

### Added

- **Lexer.** Every token form ECMA-262 §12 defines: identifiers over the real Unicode
  `ID_Start`/`ID_Continue` sets, all numeric literals including Annex B's legacy forms and
  `BigInt`, strings and templates as UTF-16 code units, regular-expression literals under a goal
  symbol supplied by the parser, and all four line terminators. Every token knows its exact span,
  and the token spans plus the trivia between them reconstruct the source byte for byte.
- **Parser.** Statements and declarations, the full expression grammar with correct precedence
  and associativity, automatic semicolon insertion, functions, classes with fields, static blocks
  and private names, generators, `async`/`await`, arrow functions, destructuring and the three
  cover grammars, template literals, optional chaining, and modules — each with the early errors
  the spec demands. Errors are values carrying spans, and read like a good compiler's.
- **Values, objects and a garbage collector.** The object model with prototypes, property
  descriptors, getters and setters and the ordinary internal methods; a mark-sweep collector with
  slot reuse behind generation-tagged handles, scheduled by the interpreter itself.
- **A bytecode compiler and interpreter.** Block scoping with the temporal dead zone, closures,
  `this`, `try`/`catch`/`finally`, and the abstract operations spelled out as such — `ToNumber`,
  `ToString`, `ToPrimitive`, `ToPropertyKey`.
- **The ES5 library, complete.** `Object`, `Function`, `Array`, `String`, `Number`, `Boolean`,
  `Math`, `JSON`, `Date`, the `Error` hierarchy, and **our own backtracking `RegExp` engine** —
  no dependency — including named groups, lookbehind, the `u` and `v` flags and Annex B's §B.1.2
  grammar.
- **ES2015 and beyond.** Classes, iterators and generators, `Symbol`, `Map`/`Set`/`WeakMap`/
  `WeakSet`, `Promise` with §9.5's job queue, `Proxy` and `Reflect`, `ArrayBuffer`, `DataView`
  and the TypedArrays including the resizable-buffer semantics, `async` functions and `await`,
  async generators, `BigInt`, and modules — live bindings, namespace objects, `export *` with its
  ambiguity rule, dynamic `import()` and top-level `await`.
- **`eval` in both modes, `with`, and Annex B.** Direct `eval` resolves into the scopes its
  caller is running in; §B.3's block-level function declarations and §B.2's library additions are
  implemented for sloppy code.
- **An embedding surface** (`api.rs`): `Engine` owns the heap and the machine together, host
  functions can be bound from Rust, and a stale handle is refused rather than read as `undefined`.
  `examples/embed.rs` is the tour.
- **An interruptible run.** `Engine::set_time_budget` stops a script that will not stop, and
  because a budget a script can `catch` is not a budget, exceeding it is not a throw.
- **A `viper` command line** — run a file, a string with `-e`, or standard input.
- **A test262 harness** (`conformance/`) with an expectations file that may only shrink.

### Conformance

About **84% of test262** at this release — 78,222 of 93,161 runs. Treat that number as
perishable and re-measure rather than quoting it. The largest remaining gaps are proposals rather
than the standard: `Temporal`, explicit resource management, decorators, and `Atomics` tests
needing multiple agents.

### Guarantees held throughout

- Zero runtime dependencies; the dependency table is empty and checked in CI.
- `#![forbid(unsafe_code)]`, crate-wide.
- No input panics. Nesting is bounded by an explicit count rather than by hitting the OS stack
  guard, and a full-depth parse is asserted to survive one mebibyte of stack.

[Unreleased]: https://github.com/MerlijnW70/viperjs/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.2
[0.2.1]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.1
[0.2.0]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.0
