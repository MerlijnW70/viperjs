<div align="center">

# ViperJS

**An embeddable JavaScript engine in safe Rust, with zero runtime dependencies.**

[![CI](https://github.com/MerlijnW70/viperjs/actions/workflows/ci.yml/badge.svg)](https://github.com/MerlijnW70/viperjs/actions/workflows/ci.yml)
[![test262](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2FMerlijnW70%2Fviperjs%2Fmaster%2Fconformance%2Fsummary.json)](https://github.com/MerlijnW70/viperjs/actions/workflows/conformance.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
[![Dependencies: 0](https://img.shields.io/badge/dependencies-0-brightgreen.svg)](Cargo.toml)
[![unsafe: forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](src/lib.rs)

</div>

It runs about **85% of test262** — classes, generators, `async`/`await`, ES modules, `Proxy`,
`BigInt`, TypedArrays, and its own regular-expression engine. No `unsafe`, no crates, and no input
makes it panic.

**Why it exists.** Embedding a scripting language should not mean embedding a C++ codebase, a
build system and someone else's `unsafe`. The constraint — no dependencies, no `unsafe`, no panics
— is the product; everything else follows from it, including writing the regular-expression engine
by hand. [GOAL.md](GOAL.md) is the binding version of that argument.

## Run some JavaScript right now

A Rust toolchain and nothing else. No build script, no C compiler, no submodules.

```sh
git clone https://github.com/MerlijnW70/viperjs && cd viperjs
cargo build --release
./target/release/viper -e "[1,2,3].map(n => n * n).join(',')"
```

```
1,4,9
```

> Two names, so that neither surprises you: the **crate** is `viperjs` and the **command** it
> installs is `viper`.

Run a file, pipe one in, or type at a prompt:

```sh
viper script.js          # run a file
cat script.js | viper    # or pipe it
viper                    # a prompt, if stdin is a terminal
viper --help             # every option, which is not many
```

The command line binds `print`, a `console` of six logging methods, and the HTML standard's `atob`
and `btoa`. There is still no `require`, no `fs` and no `process` — GOAL.md §3 says the host
provides I/O, and this host provides very little of it on purpose. What it does provide is the
Minimum Common API's, which is the line: `console` and base64 are standards every JavaScript host
has and are pure computation, where a module loader is a runtime.

```js
// script.js
function fib(n) {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}
print([...Array(10).keys()].map(fib).join(','));
```

```
0,1,1,2,3,5,8,13,21,34
```

Untrusted input? `--time-budget` is DR-0022's bound, and a script **cannot catch it**:

```sh
viper --time-budget 100 -e "try { while (true) {} } catch (e) { 'caught' }"
```

```
viper: the run was stopped: it spent its time budget
```

Exit status is `0` ran, `1` the script threw or would not parse, `2` the arguments made no sense.

## Embed it

The whole surface fits on a screen. Run the real thing with `cargo run --example embed`:

```rust
use viperjs::api::{Engine, Error, Host};
use viperjs::heap::{Heap, NativeCall};
use viperjs::value::{Completion, Value};
use viperjs::vm::Vm;

fn print(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    println!("{}", host.text(call.argument(0))?);
    Ok(Value::Undefined)
}

let mut engine = Engine::new();

// We provide the language; the host provides everything else. An `Engine` starts with no
// `console`, no `print` and no I/O at all — the command line binds those, and so do you.
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

`examples/agent_loop.rs` is the same surface put to work as a **sandbox for code nobody has read** —
a process writes a script, runs it, and repairs it from what came back. It is worth reading for one
reason beyond the loop: the four `Error` cases are four different repairs, and a host that collapses
them into "it failed" hands back the wrong instruction. It also shows the two bounds that make the
sandbox a sandbox — a time budget the script cannot catch, and a heap budget — and the case no
engine can report, which is a program that runs perfectly and computes the wrong thing.

## Measure the conformance number yourself

Don't take the figure above on trust — not having to is the point.

```sh
git clone --depth 1 https://github.com/tc39/test262 ../test262
cargo run --release -p conformance -- --test262 ../test262
```

The badge above is that same command, run by
[the conformance workflow](.github/workflows/conformance.yml) on every push to `master`. It checks
out the **exact** test262 revision the expectations file names — a conformance number without a
suite revision is not a number — and it judges the ratchet: a listed failure that starts passing and
an unlisted test that starts failing both make the build red. `conformance/summary.json` is what the
badge reads, and the workflow refuses a build whose committed figure has drifted from the one it
just measured.

```
80078 passed, 12777 failed, 306 not run
86.24% of what ran — 85.96% of the whole suite
```

**Two caveats, both honest.** The second percentage is the one to quote; the first flatters an
engine that declines most of the suite, and it *falls* whenever the engine learns to compile
something new.

The number **used to move by a couple of hundred runs between invocations**, and that is worth
knowing about because the cause was the harness rather than the engine. Three consecutive runs of
one unchanged commit gave 78,222, then 78,504, then 78,566 passing: roughly 900
`RegExp/property-escapes` files sat exactly on the ten-second per-test budget and crossed it in
either direction with machine load, reporting a heap failure on one run and a timeout on the next.

The cause was one worker per hardware thread. Every worker is a process running JavaScript, so at
full subscription each test is slower than the same test run alone. **The default is half the
threads now**, and three consecutive runs of one commit are identical — which is what makes the
expectations file usable as a ratchet rather than a weather report. `--workers` and `--budget` make
it measurable, and a run prints both, because a number is comparable only with one taken under the
same pair.

**Give the run the machine, though.** That stability is a property of a suite running alone: a run
made while a second one was still going came back 34 lower, and nothing had changed. The tests near
the per-test budget are the ones that move, exactly as they did before — so a number taken under
load is not a number.
`conformance/expectations.txt` is the real record — it may only shrink, so a genuine regression is
still a hard failure.

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
cargo build --release      # the library and the `viper` binary
cargo test                 # the engine's tests, the CLI's, and the CLI as a process
cargo test --workspace     # and the conformance harness and the lab
cargo run --example embed  # the embedding tour
```

There is also `cargo run --release --example evaluate`, which reads **one script per line** and
answers one per line. That is not a worse CLI — it is a differential-sweep tool, built to be fed a
list of expressions and diffed against another engine's answers. Use `viper` to run programs.

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
