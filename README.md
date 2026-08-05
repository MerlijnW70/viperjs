<div align="center">

# praxis

**An embeddable JavaScript engine in safe Rust, with zero runtime dependencies.**

[![CI](https://github.com/MerlijnW70/praxis/actions/workflows/ci.yml/badge.svg)](https://github.com/MerlijnW70/praxis/actions/workflows/ci.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
[![Dependencies: 0](https://img.shields.io/badge/dependencies-0-brightgreen.svg)](Cargo.toml)
[![unsafe: forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](src/lib.rs)

</div>

It runs about **84% of test262** — classes, generators, `async`/`await`, ES modules, `Proxy`,
`BigInt`, TypedArrays, and its own regular-expression engine. No `unsafe`, no crates, and no input
makes it panic.

## Run some JavaScript right now

A Rust toolchain and nothing else. No build script, no C compiler, no submodules.

```sh
git clone https://github.com/MerlijnW70/praxis && cd praxis
echo "[1,2,3].map(n => n * n).join(',')" | cargo run --release --example evaluate
```

```
1,4,9
```

`evaluate` reads **one script per line** from standard input and writes one answer per line, the way
`String(x)` would write it. Anything it cannot run comes back beginning with `!`, so a sweep never
silently drops a line:

```sh
printf '%s\n' \
  'class A { #x = 7; get x() { return this.#x } } new A().x' \
  '/(?<y>\d{4})-(?<m>\d{2})/.exec("2026-08").groups.y' \
  '2n ** 64n' \
  'function* g() { yield* [1,2,3] } [...g()].join()' \
  '[..."héllo"].length' \
  'typeof Temporal' \
  | cargo run --release --example evaluate
```

```
7
2026
18446744073709551616
1,2,3
5
undefined
```

## Embed it

The whole surface fits on a screen. Run the real thing with `cargo run --example embed`:

```rust
use praxis::api::{Engine, Error, Host};
use praxis::heap::{Heap, NativeCall};
use praxis::value::{Completion, Value};
use praxis::vm::Vm;

fn print(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    println!("{}", host.text(call.argument(0))?);
    Ok(Value::Undefined)
}

let mut engine = Engine::new();

// We provide the language; the host provides everything else. There is no `console`
// until you bind one.
engine.bind("print", 1, print);

let answer = engine
    .eval("print('hi'); ({ sum: [1,2,3,4].reduce((a, b) => a + b) })")
    .expect("it runs");
let sum = engine.get(answer, "sum").expect("sum is there");
println!("sum = {}", engine.text(sum).unwrap_or_default());

// A script's error is a value, not a crash.
match engine.eval("undefined.x") {
    Err(Error::Thrown(said)) => println!("the script threw: {said}"),
    other => println!("unexpected: {other:?}"),
}
```

`examples/embed.rs` adds the rest: calling back into the script with a receiver you choose, and what
happens to a `Value` you hold across a garbage collection.

## Measure the conformance number yourself

Don't take the figure above on trust — not having to is the point.

```sh
git clone --depth 1 https://github.com/tc39/test262 ../test262
cargo run --release -p conformance -- --test262 ../test262
```

```
78222 passed, 14619 failed, 320 not run
84.25% of what ran — 83.96% of the whole suite
```

**Two caveats, both honest.** The second percentage is the one to quote; the first flatters an
engine that declines most of the suite, and it *falls* whenever the engine learns to compile
something new. And the number **moves by a couple of hundred runs between invocations**: roughly 900
test262 files sit exactly on the harness's ten-second per-test budget and cross it in either
direction with machine load. `conformance/expectations.txt` is the real record — it may only shrink,
so a genuine regression is still a hard failure.

## Point it at your own code

```sh
cargo run --release --example parse -- --commonjs /path/to/a/repo
```

It walks the directory, parses every `.js`, `.mjs` and `.cjs` under both the Script and Module
goals, and groups whatever it cannot read by error kind — so one missing production appears as one
bucket with a large number, not four hundred unrelated-looking lines.

Over `nodejs/node`, `webpack` and `ramda` — 20,379 files — it parses 20,102 and panics on none.
Every one of the 277 refusals is a Stage 3 proposal, JSX, a deliberately-broken test fixture, or, in
one case, a real duplicate-binding bug in webpack's own test suite that node rejects identically.

## What works, and what does not

**In.** The ES5 library entire; `let`/`const` and the temporal dead zone; classes with private
fields, methods and static blocks; destructuring, spread and rest; template literals; iterators and
generators; `async`/`await` and async generators; `Symbol`; `Map`, `Set`, `WeakMap`, `WeakSet`,
`WeakRef`; `Promise` and the job queue; `Proxy` and `Reflect`; `ArrayBuffer`, `DataView` and all
eleven TypedArrays including the BigInt pair; `BigInt`; `eval` in both its modes; `with`; ES modules
with live bindings, namespace objects, cycles, top-level `await` and dynamic `import()`; and Annex B
— block-level function declarations, the HTML string methods, and §B.1.2's regular-expression
grammar, so `/}/`, `/\1/` and `/[\d-x]/` mean what a browser means by them.

The regular-expression engine is ours — backtracking, with named groups, lookbehind, the `d` flag's
`indices`, Unicode property escapes and the `v` flag's set notation.

**Out.**

- **`Temporal`** — a Stage 3 proposal with a surface larger than `Date`, `Intl` and `RegExp`
  together. It is 8,316 of the failing runs on the `Temporal is not defined` message alone (9,222
  mentioning it at all), and building it would raise the number without making this more of a
  JavaScript engine.
- **`Intl`** — ECMA-402 is a separate specification, not attempted.
- **`ShadowRealm`, `SharedArrayBuffer`, `Atomics` needing agents** — this is single-threaded.
- **`String.prototype.normalize`** — needs the Unicode decomposition tables. Absent on purpose
  rather than stubbed: a `normalize` that returned its receiver would pass eleven of its fourteen
  test files and be a silently wrong answer for the other three.
- **`Function.prototype.toString`** does not reproduce source text.
- **Proposals**: decorators, `using`/`await using`, `import defer`, `import source`, `Iterator.zip`
  and its neighbours.

**Speed.** A straightforward bytecode interpreter: no JIT, no inline caches, no hidden classes. That
work is a later milestone and each piece of it has to arrive with a benchmark rather than a hunch.
A million-iteration arithmetic loop takes about 200 ms here against 3 ms in node — call it 70×, and
note that a JIT can prove most of that loop dead while an interpreter cannot. Linking and evaluating
ramda's 1,027 modules takes about 83 ms. If you want JavaScript at V8 speed this is not it; if you
want a small spec-faithful engine inside a Rust binary with no `unsafe` anywhere in it, that is the
trade on offer.

## The properties that are not negotiable

- **No `unsafe`.** `#![forbid(unsafe_code)]`, crate-wide, no exceptions.
- **No input panics.** Untrusted script is *input*, not an exception. Syntax errors, absurd
  literals, pathological nesting, a regular expression built to explode — every one is a `Result`,
  never a crash in your process. Two bounds are yours to set: a per-run time budget a script cannot
  catch, and a heap budget it can.
- **Conformance is measured, not claimed.** test262 is the arbiter and the expectations file may
  only shrink.
- **Zero runtime dependencies.** The `[dependencies]` table is empty and stays empty.

## Building and testing

```sh
cargo test                 # the engine's own tests — about 1,560 of them
cargo test --workspace     # and the conformance harness and the lab
cargo run --example embed  # the embedding tour
```

Needs a toolchain with edition 2024 support; built with 1.97.

## Where to read next

| | |
| --- | --- |
| [GOAL.md](GOAL.md) | the charter — what this is for, and what it refuses to become |
| [AGENTS.md](AGENTS.md) | how the work is done, the milestones, and the current work list |
| [decisions/](decisions/) | one record per architectural decision, with the argument that settled it |
| [src/span.rs](src/span.rs) | the worked example of the standard everything else is held to |
| [lab/NOTES.md](lab/NOTES.md) | the notebook — measurements, and the dead ends worth not repeating |

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) has the process; [AGENTS.md](AGENTS.md) has the milestone plan
and the house style. Two things are worth knowing before you start: no change may add a runtime
dependency or any `unsafe`, and a test that merely passes is worth very little — it has to *fail*
when the logic is wrong.

Architectural changes get a decision record in [`decisions/`](decisions). The existing 23 are short,
and are the fastest way to understand why the engine is shaped as it is.

## Licence

Copyright © 2026 MerlijnW70. Dual-licensed, at your option, under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

See [COPYRIGHT](COPYRIGHT) for the whole of it. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in this work, as defined in the Apache-2.0
licence, shall be dual-licensed as above, without any additional terms or conditions.
