# AGENTS.md — building ViperJS

> Vendor-neutral contributor and agent instructions, following the
> [AGENTS.md](https://agents.md) standard. The binding charter is **[GOAL.md](GOAL.md)** —
> read it first; it outranks this file.

**ViperJS** is an embeddable JavaScript engine in safe Rust, zero runtime dependencies, measured
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

`ls` answers the layout; what it cannot answer is the order. `src/` is the engine (zero deps, no
`unsafe`, no panics, fully ratcheted), `lab/` is experiments that cannot be imported by it,
`conformance/` is the test262 harness and its ratchet, `decisions/` is one record per architectural
choice, and `GOAL.md` is the binding charter.

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
api.rs       the embedding surface           [DONE — DR-0021; `examples/embed.rs` is the tour and
                                              `examples/agent_loop.rs` the sandbox it is for]
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
5. **The gate** — green (no-unsafe, the boundary, the decision records, the architectural
   constraints, fail-closed). **Then `cargo fmt --all --check`, `cargo clippy --workspace
   --all-targets -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`,
   each by name** — the gate runs none of the three, whatever an older version of this file said.
6. **Run the conformance suite.** The number goes up, or the change explains why — and see
   `conformance/README.md` on why a gain needs three runs to be believed and a regression does not.
7. **Commit.** The message says what changed *behaviourally*, not which files moved.

## House style

- Comments explain **why**, never what. The code says what.
- Doc comments on everything public (`missing_docs` is denied) — including what a function does
  when the input is nonsense.
- Errors are values with spans. No `unwrap()` in production paths; `expect("<invariant>")` only
  where a panic would mean an engine bug the types cannot encode.
- Tests are named as sentences about behaviour: `a_crlf_pair_ends_one_line_not_two`, not
  `test_line_col_2`. **`src/span.rs` is the worked example** — match its density of intent.

The per-area findings — what each slice cost beyond its clause, which comment turned out to be
wrong, and which hypotheses died on the way — live in **[notes/FINDINGS.md](notes/FINDINGS.md)**.
They are not loaded here because a session needs the section for the area it is touching and no
others; that file is thirty thousand words and this one is read every time. **Read its section
before starting in an area** — most of it exists because a dead end was re-derived once already.

## Start here

M1, M2, M3, M4 and M5 are done: the lexer, the parser, the value and object model, a bytecode
compiler and an interpreter that runs code, the whole ES5 library including our own `RegExp`, and
the conformance harness that measures it — which from here is what says what to build next.
**M6 is what is in progress.** Classes, `Promise` and §9.5's job queue, `Map` and `Set`, `Reflect`,
`Proxy`, `ArrayBuffer`, `DataView` and the TypedArrays were already in. **Generators, `yield`,
`yield*`, `async` functions and `await` are now in too** — see DR-0017 and `src/vm/suspend.rs` —
and so are **async generators**, §27.6, in `src/vm/async_generator.rs`. **`BigInt` is in**: the
literal, the arithmetic, the object, and now `BigInt64Array` and `BigUint64Array`.

**And `eval` runs, both ways.** §19.2.1.1's indirect mode was already there; the **direct** mode
resolves into the scopes its caller is *running* in — see DR-0018, `src/vm/eval.rs` and
`compile_direct_eval`. That is what the environments' name lists are for, and it was worth 973 runs.

**`with` runs too, and so do modules — including `import()`.** §16.2 is whole apart from
`import.meta`: `import` and `export` in every form, §16.2.1.5.2's live bindings, §10.4.6's namespace
objects, §16.2.1.6.3's `ResolveExport` across a graph, `export *` with its ambiguity rule, §13.3.10's
dynamic `import()`, and a top-level `await` that what imports the module waits for.
`src/vm/module.rs` is the linker, `src/heap/namespace.rs` the exotic object and `src/vm/loader.rs`
the host hook.

**And Annex B's block-level function declarations run**, along with §B.2.3's thirteen HTML methods.
DR-0008 was reversed and §B.3.2, §B.3.3 and §B.3.4 are in, in sloppy code — `src/compile/annex_b.rs`
decides which declarations earn the extra `var` binding. That was the last thing between the engine
and 80%, and `notes/FINDINGS.md` is what it cost.

**And §B.1.2's regular expression grammar runs**, which is the *other* half of Annex B and was still
refused wholesale until DR-0008's second amendment. A pattern carrying neither `u` nor `v` now reads
`/}/` as a brace, `/\1/` with no group as a legacy octal escape, `/\8/` as an `8`, `/\c1/` as three
characters and `/[\d-x]/` as a union. See `notes/FINDINGS.md` for the shape of it.

**`tests/cli.rs` is invisible to mutation coverage, and that is worth knowing before writing a
test there.** An integration test runs the binary as a **subprocess**, so a mutation in
`src/bin/viper.rs` that only the subprocess would notice survives — the mutant reaches the binary
the sandbox builds and not necessarily the one the test invokes. Found 2026-08-08 building `atob`:
eight survivors over eleven lines of bit arithmetic, every one of them killed when the mutation was
applied *by hand*. The count also moved between runs (8, then 4), which is the tell. **Put the
decision in a `#[cfg(test)]` unit test inside the binary and leave the integration test to prove
the *binding*** — doing that took the survivors to one, and the one that was left was a
`with_capacity` hint no test could ever pin, so it went.

**A run's number is stable and a *contended* one is not, and the difference cost a bad bless.**
Three consecutive runs alone give the same figure to the test; a fourth taken while the machine was
busy came back **204 lower**, all of it `RegExp/CharacterClassEscapes` and `property-escapes` — the
budget-bound tail this file already names, which halving the worker count removed the *self*-
contention from and not contention in general. Blessing that run added new paths to the expectations
file, which is the ratchet being laundered; it was caught by reading the count rather than the
verdict, and reverted. **Count the identical runs before blessing, and give the suite the machine.**

**And a second §9.3 realm exists**, which is DR-0025 and was worth **+383 runs** across five slices
— the largest single area since the generator work. `$262.createRealm` builds one; a function
carries the `[[Realm]]` it was made in; §10.1.14 `GetFunctionRealm` answers for a bound function and
a Proxy by recursing; and §10.3.1 step 3 makes a call run in the **callee's** realm, saved on the
frame beside the `this` and the `new.target` it already saved. §10.1.13 is a real `Get` now and
takes its default from the constructor's realm, which fixed every built-in constructor at once. See
`notes/FINDINGS.md` for the three ordering faults that fell out of it.

**And `$262.agent` runs, so ViperJS has more than one agent** — worth **+168**, and it is threads
rather than a scheduler because `atomicsHelper.js` waits for an agent with a `while` loop that has no
yield in it. A `SharedArrayBuffer`'s bytes are a `heap::Block` now: one allocation behind an `Arc`
that any number of heaps may hold, with §25.4.1's waiter list under the same lock so that a blocking
`Atomics.wait` compares and enqueues in one step. §9.7's `[[CanBlock]]` is a property of the *agent*,
answered by the host — `false` for an engine embedded on its own, `true` throughout `conformance`,
which is what swaps the `CanBlockIsFalse`/`CanBlockIsTrue` flags over. **Read
`notes/FINDINGS.md` before touching it**: the read-modify-write operations were not atomic and no
single-agent test could ever have said so, and the shape of the failure was a suite that hung rather
than a number that was wrong.

**And §9.5's drain knows what time it is, and collects.** DR-0024's `waitAsync` timeout is built —
`+46` — and it needed no timer: `TriggerTimeout` has only to run before anything can observe that it
has not, and a job boundary is such a point. What that slice *found* is the part to read
`notes/FINDINGS.md` for. A job runs its handler through a nested execution, so `reentries` is one for
the whole of it, and `execute`'s collection check is guarded on `reentries == 0` — so **the loop had
never collected during a job drain at all**, for any threshold any host could set. A promise chain
that re-arms itself reached DR-0013's budget at 38,174 turns and threw a RangeError *inside a job*,
where §9.5 step 3 discards it: the queue emptied, `run` returned, and the exit status was zero. A
silent stop is a failure mode this engine can produce and nothing was watching for it.

**And a sloppy direct `eval`'s `var` reaches the caller's scope**, which was the largest refusal
this engine still stated by name — worth **+49**. `Vm::var_environment` is §8.3.2's other
environment, tracked because a flat chain cannot say which level a call opened; `heap::Declared` is
§10.2.11 step 30's separate lexical record written down rather than built, which is what step 5.f's
SyntaxError needs. What is still refused is a call written in a **formal parameter list** — step 20
puts that one outside the parameters — and refusing it is load-bearing: the 192 `declare-arguments`
runs assert exactly that error. Read `notes/FINDINGS.md` before touching it; the recorded
prerequisite for this slice was wrong twice, and the obvious design (resolve the eval's names
dynamically) is wrong for a reason worth knowing.

**And case-insensitive matching reaches past ASCII**, which it never had: §22.2.2.9's
`Canonicalize` was `a`-`z` and nothing else, so `/café/i` did not match `CAFÉ` in any script, in
either direction, under `i` or `iu`. `src/unicode_case_table.rs` is generated from the same UCD
17.0.0 the identifier and property tables pin, and carries both of the clause's branches — folding
for `u` and `v`, uppercasing without them — plus the equivalence classes a *range* test needs. Worth
only +4 runs and a great deal more than that: **test262 barely reaches this**, which is why it
survived a conformance number of 86% and was found by differential sweep instead.

**A differential sweep against another engine is a fourth instrument**, beside reading, running
real code and fuzzing, and it finds a different class from all three. Roughly 2,200 probes — values,
evaluation order, RegExp, async job ordering, Proxy invariants, iterator closing, completion values,
detached buffers — found six real bugs, four of them invisible to the ratchet. The recipe and its
traps are in `notes/FINDINGS.md`; the shortest version is that each probe answers with a
self-describing string so the two engines are compared on *answers* rather than on message wording,
and that the oracle has to be validated before the diff is believed.

**A sweep can compare *structure* rather than answers, and then it is closed rather than a sample.**
Three of them are written up in `notes/FINDINGS.md`: the intrinsics' aliasing graph (which paths are
the same object), their structural facts (attributes, `length`, `name`, prototype, whether it
constructs), and a grid of 46 parser contexts × 36 constructs. Five more bugs, and two findings that
are the *other* engine's — a class's parts being strict mode code, which V8 applies only to method
bodies, and `await` in a static field initialiser. Read that section before repeating any of it; the
two traps that cost the most are that the roots must be *named* rather than taken from the global
object, and that the walk must be breadth-first over sorted keys or the same object gets a different
name in each engine.

Conformance as of this commit is **86.32% of test262** — 80,413 of 93,161 runs, the same figure on
three consecutive runs alone. Treat that number as perishable and re-measure rather than quoting it;
the point of the figure is the work list under it. Only 306 runs are now *stopped* before anything
executes. **One of them was misfiled here for a long time and it matters:** `(?i:…)` 170 is the
RegExp **modifiers** proposal and is excluded, but a property of strings is **not** a proposal —
`regexp-v-flag` sits unmarked in test262's `features.txt` and shipped in ES2024. `\q{…}` was 54 of
those and is now built; what is left of the flag is the 92 runs that need Unicode emoji sequence
data, six of them the `v`-flag's other stragglers. The rest is `super` in an arrow's direct `eval`
16, two dozen module-beside-the-test parse failures that are proposals, and **`this agent can
block` 4** — which used to read `this agent cannot block` 14 and is now the *other* flag: see the
agents entry below.

**The failure buckets are the whole work list now.** Sorted by reason the largest look actionable
and mostly are not, which is worth doing once and writing down rather than re-deriving:

| Runs | Reason | What it really is |
| --- | --- | --- |
| 8,316 | `Temporal is not defined` | a proposal — costed and refused, see below |
| 405 | `what was called is not a function` | proposals now, and only now: `Array.fromAsync`, `Iterator.zip`/`zipKeyed`/`concat`, `Promise.allKeyed`/`allSettledKeyed`, `Map`/`WeakMap`'s `getOrInsert`, `Uint8Array` base64, `DataView`'s `getFloat16`. **It was 939, then 821, and this file called it "mostly proposals" at both figures — twice wrongly.** Seven *shipped* functions were hiding in it the first time and **369 runs of `$262.createRealm`** the second, which is nearly half. It is 407 because both were built. **Ask the engine what it has** before believing this row; it has misled two readers already |
| 126 | `cannot read a property…` | **was 352 and the 224 that went were `$262.agent`, which is built** — see below. What is left is `Error.prototype.stack` 64 and `legacy-accessors` 24, both proposals, `Promise.allKeyed`/`allSettledKeyed` 26, another, and a dozen real ones |
| 208 | `$DONE with what was called is not a function` | the asynchronous half of the row above, and the same proposals |
| 293 | `expected 'meta', found an identifier` | `import.defer` and `import.source` — two proposals, not `import.meta` |
| 238 | `Calling as constructor…` | all `Temporal` |
| 224 | `expected ';', found an identifier` | `using` / `await using` — explicit resource management, a proposal |
| 178 + 144 | `DisposableStack`, `AsyncDisposableStack` | the same proposal's library half |
| 118 | `ShadowRealm is not defined` | a proposal, and **not** DR-0025's realm: that one shares a heap and passes objects freely, where this puts a membrane between the two sides |
| 34 | `it did not parse: unexpected character` | **decorators**, a proposal — and the one row here whose reason says nothing at all about what it is. Its paths do |
| 2 | `the test never called $DONE` | **was 48, and 46 of them were one bug that had nothing to do with the reason string.** The row read as an asynchronous test giving up; what it was is a job queue that emptied because a promise chain had silently run out of heap. See `notes/FINDINGS.md`. The two left are `top-level-await`'s ordering pair |

**Two buckets have been costed and must not be re-costed.** A third — `Function.prototype.caller`,
23 runs — was costed, refused as *not in the language*, put to the user as DR-0008's shape, and then
**built on instruction**: see DR-0028. It is the one piece of ViperJS that is deliberately outside
ECMA-262, and the record says exactly where it stops and how a program gets the standard behaviour
back.

- **`RegExp/property-escapes` is not what this file said it was, and the correction is instructive.**
  It read: "dead as a GC target … these need an interpreter several times faster, which is M8",
  resting on `lab/NOTES.md`'s `gc-pressure`, where `ASCII.js` takes 21.8 s against a 10 s budget.
  That measurement is about **one** file and was generalised to all 878. Measured 2026-08-05: run
  alone the directory passes **814 of its 1,226 runs**, and 890 of them stabilised the moment the
  worker count was halved. So most of the bucket needed a flag, not a milestone — and the reason
  string in the expectations file, which said `the heap has grown past…`, was simply the wrong
  failure. **Read a reason as a claim somebody made, not as a measurement.** The experiment's two
  real findings — a throwaway heap String per computed property key, and a timed-out run landing in
  no column — are both **fixed**; do not go looking for them again.

  **…and the sentence that used to end this paragraph was wrong too, in the same direction.** It
  said "what is left of the bucket is genuinely slow and M8 is genuinely its answer, but it is
  hundreds of runs rather than 878". Measured 2026-08-09, the directory alone: **1,206 of 1,226 runs
  pass**, 14 are the `\p{RGI_Emoji}` skips, and the expectations file lists **six**. All six are
  `Script=Unknown` and `Script_Extensions=Unknown` — a UCD default the table generator never
  emitted, because `Scripts.txt` states `Unknown` only in an `@missing` line and the set is the
  *complement* of every other script. That is a data gap of six runs, not a performance bucket, and
  **`property-escapes` is no longer on M8's list at all**. The whole directory is 143 s of wall
  clock, which is a real cost and a separate question from whether anything fails.

  Three corrections to one paragraph, each in the direction of "the bucket is smaller than the note
  says". Re-measure this row before quoting any part of it.
- **`Temporal` is a Stage 3 proposal with a surface larger than `Date`, `Intl` and `RegExp`
  combined.** Building it would raise the number while making the engine no more of a JavaScript
  engine, and it will sit at the top of that list for as long as this file is worth reading.
