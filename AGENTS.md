# AGENTS.md — building praxis

> Vendor-neutral contributor and agent instructions, following the
> [AGENTS.md](https://agents.md) standard. The binding charter is **[GOAL.md](GOAL.md)** —
> read it first; it outranks this file.

**praxis** is an embeddable JavaScript engine in safe Rust, zero runtime dependencies, measured
against test262. You are building it one milestone at a time, and the measure of progress is a
conformance number, not a feeling of completeness.

## The shape of the work

This project is **long, not hard**. There is no breakthrough waiting; there are ~50,000 test262
tests, each of which either passes or does not. That has three consequences for how you work:

- **Never guess at the spec.** ECMA-262 is online and unambiguous. Every non-obvious behaviour
  gets a comment citing its section (`// ECMA-262 §13.15.2 — assignment evaluates the target
  reference BEFORE the value`). "I think JS does X" is how an engine acquires a bug that takes
  three months to find.
- **Prefer the boring implementation.** The clever one costs you at every conformance edge case.
  Optimize when a benchmark, not an intuition, says to — and prototype it in `lab/` first.
- **Land small.** One coherent slice per commit, green each time. A 3,000-line "parser
  done" commit cannot be reviewed, cannot be bisected, and cannot be probed meaningfully.

## Repository layout

| Path | What |
| --- | --- |
| `src/` | The engine. Zero deps, no `unsafe`, no panics, fully tested. |
| `lab/` | Experiments — see [`lab/README.md`](lab/README.md). Not shipped, and cannot be imported by the engine. |
| `conformance/` | The test262 harness and its expectations ratchet (arrives at M5). |
| `decisions/` | Decision records for anything architectural: one file, prose + the invariant it implies. |
| `GOAL.md` | The charter. Binding. |

Planned module order inside `src/` — build in this order, because each genuinely needs the one
before it:

```
span.rs      source positions                          [DONE — the worked example of the bar]
lexer.rs     source -> tokens
ast.rs       the syntax tree
parser.rs    tokens -> AST  (ASI, early errors)
value.rs     the value representation
heap.rs      objects, properties, prototypes, GC
compile.rs   AST -> bytecode
vm.rs        the interpreter loop
builtins/    Object, Function, Array, String, Number, Math, JSON, Error, ...
api.rs       the embedding surface
```

## Milestones

Each milestone names its **oracle** — the objective thing that says "done". Work to the oracle,
not to your own sense of completeness.

**M1 — Lexer.** All ES2015 token forms: identifiers with Unicode escapes, all numeric literals
(legacy octal, `0b`/`0o`/`0x`, separators), strings with every escape, template literals with
nesting, regex-vs-division disambiguation, comments, all four line terminators, and the newline
flags ASI will need. *Oracle:* hand-written token tests per form, plus a round-trip property —
every token knows its span, and the spans of a token stream reconstruct the source exactly.

**M2 — Parser.** ES5 statements and expressions, functions, full operator precedence, ASI, and
the early errors the spec demands (assignment to a call, `delete x` in strict mode, duplicate
parameters, `break` outside a loop). Errors carry spans and read like a good compiler's.
*Oracle:* test262's `syntax` and `early` tests for the implemented grammar — these run before
you have a VM, which is why the parser is worth finishing properly first.

**M3 — Values, heap, and a VM that runs code.** Value representation (start with a plain enum —
NaN-boxing is an M8 experiment, and the lab decides it with a number), the object model with
prototypes and property attributes, a mark-sweep GC, bytecode compiler, interpreter loop.
Arithmetic and coercion (`ToNumber`, `ToString`, `ToPrimitive` — the abstract operations, spelled
out as such), control flow, functions, closures, `this`, `try`/`catch`/`finally`. *Oracle:* the
first real test262 run. Expect a low number; it is a starting line, not a grade.

**M4 — The ES5 library.** `Object`, `Function`, `Array`, `String`, `Number`, `Boolean`, `Math`,
`JSON`, `Date`, `RegExp` (our own backtracking engine — no dependency), the `Error` hierarchy.
Property descriptors, getters/setters, strict-mode semantics throughout. *Oracle:* ES5-tagged
test262 passing at 95%+.

**M5 — The conformance harness proper.** `conformance/` runs test262 in parallel, understands
its YAML frontmatter (`includes`, `flags: onlyStrict/noStrict/module/raw`, `negative`), and
maintains an **expectations file that may only shrink**. Wire it before M6 — from here on it is
the thing that tells you what to build next.

**M6 — ES2015 core.** `let`/`const` and the temporal dead zone, classes, arrow functions,
destructuring, spread/rest, template literals, `for...of`, iterators, generators, `Symbol`,
`Map`/`Set`/`WeakMap`/`WeakSet`, `Promise` and the job queue, `Proxy`/`Reflect`. This is the
largest milestone by far — split it, and let the conformance failure buckets choose the order.

**M7 — Modern syntax.** `async`/`await`, async iteration, optional chaining, nullish coalescing,
exponentiation, `BigInt`, modules with a host-provided resolver.

**M8 — Performance.** Only now: shapes/hidden classes, inline caches, interned strings,
NaN-boxing, a register VM. Every one of these is a lab experiment with a benchmark before it is
a commit, and none may cost a single conformance test.

## Workflow for one change

1. **Read the spec section.** Cite it in a comment. If you cannot find it, you do not yet know
   what you are implementing.
2. **Unsure of the design? Go to `lab/` first.** Prototype, measure, write the NOTES.md verdict,
   then implement in `src/` from scratch — never by copying the spike (`lab/README.md` rule 3).
3. **Write the code and its tests together.** A test that merely passes is worth little; it must
   *fail* when the logic is wrong. That is what mutation testing is about to check.
4. **Mutation testing** — zero survivors on the lines you touched. A survivor is a precise
   statement that a branch you wrote is untested, and it comes with the input that
   distinguishes it. Fix the test, not the branch.
5. **The gate** — green (fmt, clippy `-D warnings`, `missing_docs`, no-unsafe, the boundary).
6. **Run the conformance suite** once M5 exists. The number goes up, or the change explains why.
7. **Commit.** The message says what changed *behaviourally*, not which files moved.

## House style

- Comments explain **why**, never what. The code says what.
- Doc comments on everything public (`missing_docs` is denied) — including what a function does
  when the input is nonsense.
- Errors are values with spans. No `unwrap()` in production paths; `expect("<invariant>")` only
  where a panic would mean an engine bug the types cannot encode.
- Tests are named as sentences about behaviour: `a_crlf_pair_ends_one_line_not_two`, not
  `test_line_col_2`. **`src/span.rs` is the worked example** — match its density of intent.

## Start here

M1, M2, M3, M4 and M5 are done: the lexer, the parser, the value and object model, a bytecode
compiler and an interpreter that runs code, the whole ES5 library including our own `RegExp`, and
the conformance harness that measures it — which from here is what says what to build next.
**M6 is what is in progress.** Classes, `Promise` and §9.5's job queue, `Map` and `Set`, `Reflect`,
`Proxy`, `ArrayBuffer`, `DataView` and the TypedArrays were already in. **Generators, `yield`,
`yield*`, `async` functions and `await` are now in too** — see DR-0017 and `src/vm/suspend.rs` —
and so are **async generators**, §27.6, in `src/vm/async_generator.rs`.

Conformance as of this commit is **73.42% of test262** — 68,401 of 93,160 runs. Treat that number
as perishable and re-measure rather than quoting it; the point of the figure is the work list under
it. Let the failure buckets choose the next slice, not intuition. The largest right now:

| Runs | What stops them |
| --- | --- |
| 6,263 | `BigInt` literals |
| 849 | dynamic `import` |
| 830 | modules |
| 306 | `(?i:…)` — the RegExp **modifiers proposal**, and not ES2023; see below |
| 280 | `with` |
| 280 | `with` |

**63.06% to 71.65% in three slices**, all of them §27.6 and its neighbourhood: async generators
themselves (+4,814), `yield*` inside one — §15.5.5 step 4's `GetIterator(value, async)` (+1,590) —
and then `GeneratorStart` becoming an instruction (+1,598). That last one is worth reading before
touching this area: a generator's parameters are **not** part of its body, so
`FunctionDeclarationInstantiation` runs at the call and only `GeneratorStart` parks what is left.
Deciding it in `enter` instead put the whole parameter list inside the parked body, where it ran at
the first `next` — invisible until a parameter can throw or be observed, and then 1,598 tests at
once, most of them in `dstr` directories that have nothing to do with generators.

**75% is `BigInt`**: 6,263 runs, a new numeric type through the value representation, every operator
and every coercion. Nothing else on the list is within a factor of five, and the next largest things
after it are Unicode property escapes (1,318) and per-iteration environments (1,234).

**Block scoping is done, and there is no refusal left in it.** A block that declares something
gets its own environment (`Instruction::PushScope`); a `for (let i = …; …; …)` head gets §14.7.4.7's
copy per pass; a `for`-`of` and a `for`-`in` head get §14.7.5.7's fresh environment per pass. A
closure made in any of them keeps the binding that pass had.

Three things about it are worth keeping, none of them about closures:

- **An exit's depth indexes the break lists, not the loop nesting.** A label on a plain block pushes
  one of those without being a loop, so `L: { let x; break L; }` counted the other way never emits
  its `PopScope` and the code after reads a *different variable*.
- **A throw needs no `PopScope`** — a handler records the environment it was installed in, which is
  what `Handler::environment` is for. That is the line between an exit that runs instructions on the
  way out and one that jumps.
- **`unwind_across` stops at the first entry a jump does not cross**, so the order of the entries is
  load-bearing. A `for`-`of`'s per-iteration environment sits *inside* its iterator's entry: a
  `continue` leaves the environment and deliberately does not close the iterator, and with the two
  the wrong way round it stops at the iterator and leaks an environment per pass.

**The garbage collector's root set is settled; its schedule is not.** `Vm::collect` is the host's
to call, and the interpreter does not run one on a timer. That is measured, not deferred:
`Heap::footprint` counts arena *slots* and DR-0010 does not reuse a swept one, so a collection
reclaims Strings, environments and buffers and cannot reclaim what an object took. Scheduled every
eight mebibytes it cost 318 conformance files their time budget to buy six passes; run once at the
budget, 79 files to buy none. **The next step there is slot reuse with generation-tagged handles —
a decision record, not a patch**, and after it the timer is one line.

What *is* settled is the part that cannot be left half-right. Four whole classes of reference were
untraced before this: a bound function's target and arguments, a revive closure's context, a
compiled chunk's constant table, and a queued job's payload. A collection with any of those missing
frees something a later instruction reads — silently, as a wrong value rather than a crash. The
root set lives in `Vm::roots` and is checked against the collector in `vm::tests::collecting`,
including the one case that distinguishes it: an intrinsic *nothing has reached yet*.

**A compile error is not automatically a skip**A compile error is not automatically a skip, and treating it as one hid failures.** §22.2.1's
early errors are decided by the *compiler* — §12.9.5 reads a regular expression literal's shape and
its pattern only afterwards — so `conformance` used to drop every one of them into "not run". 560
runs came out of that column when it was fixed: 366 pass, and **194 fail and could not be seen
before**. The split it rests on is `ErrorKind::BadPattern` against `Unsupported`, and getting it
backwards is not symmetric: a *gap* recorded as an early error passes every test asserting "this
must be rejected", and a proposal's negative tests are exactly that shape. `(?i:…)` is the live
example — see `regexp::Error::unimplemented`.

**Two buckets are not ES2023 and must not be counted as cheap.** `Temporal` (3,476 runs) is a Stage
3 proposal with a surface larger than `Date`, `Intl` and `RegExp` combined — building it would raise
the number while making the engine no more of a JavaScript engine. The 306 runs that stop on `this
is not a kind of group` are the same thing in miniature: they are `built-ins/RegExp/regexp-modifiers`
and the `(?i:…)` syntax is Stage 3 as well. That bucket reads like a cheap 306 in a finished area,
which is exactly why it is worth naming here. **Check a bucket's directory before costing it.**

**Read the *failure* buckets, not only that table.** The list above is what stopped the tests that
never ran. Sorting the ~15,000 that **run and fail** by reason is what finds the slices worth a day:

    grep -av '^#' conformance/expectations.txt | sed 's/.* :: //' | sort | uniq -c | sort -rn | head -25

Bucket by *path* too (`awk -F/ '{print $1"/"$2}'`) to see which area is worth a slice rather than a
method.

**And ECMA-262 cannot be read with a fetch tool.** Both the multipage and single-page builds answer
with their table of contents whatever anchor is asked for, so "read the clause first" is not
available that way. The vendored suite is the oracle instead: implement, run `--only <area>`, and
read the failing tests' `info:` frontmatter — which quotes the numbered steps verbatim, and is how
a wrong reading gets caught.

**Three ways a green run can be lying**, all met in the generator work and all worth knowing:

- A **decision record can be wrong**, and confidently. DR-0017 said twice that a suspension may not
  cross a re-entry; both readings refused ordinary programs like `[1].map(it.next.bind(it))`, and
  the second was in the code as a check before the first program that needed it was written. Write
  the program that would break, run it, and only then believe the record.
- **A test can pass for the wrong reason**, and a whole bucket of them can. Check what the
  conformance run says *moved*: every new expectations line must be a test that was **skipped**
  before, and the run's own arithmetic proves it — tests leaving "not run" must equal new passes
  plus new failures exactly, or something that used to pass now does not.
- **A stored state can go stale where a derived one cannot.** `[[GeneratorState]]` was a field until
  a throw escaping a body left it saying `executing` for ever. Suspended is "it holds a parked
  execution", executing is "a live frame names it", completed is neither — all three are questions
  about somewhere else, and a field repeating the answers is a field that can disagree with them.

The local loop is `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` **and**
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`, each by name. The gate does not cover
the third: a public item's doc linking to a private one is an error in CI and nowhere else.

Read [`GOAL.md`](GOAL.md) first — it is binding and it outranks this file — then `src/span.rs` to
calibrate on the bar. `cargo run --release --example parse -- --commonjs <dir>` over a real
repository is the fastest way to find something worth fixing; `examples/evaluate.rs` runs code.
