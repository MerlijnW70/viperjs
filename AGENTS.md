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

M1, M2, M3 and M5 are done: the lexer, the parser, the value and object model, a bytecode compiler
and an interpreter that runs code — and the conformance harness that measures it, which from here is
what says what to build next. **M4 is what is in progress.** `Object`, `Function`, `Array`, `String`,
`Number`, `Boolean`, `Math`, `JSON`, `Date` and the `Error` hierarchy are in, and so is a good deal
of M6: classes, `Promise` and §9.5's job queue, `Map` and `Set`, `Reflect`, `ArrayBuffer`,
`DataView` and the TypedArrays. **`RegExp` is what remains of M4**, and the regular expression
engine is ours to write — no dependency.

Conformance as of this commit is **50.30% of test262** — 46,859 of 93,161 runs. Treat that number as
perishable and re-measure rather than quoting it; the point of the figure is the work list under it.
Let the failure buckets choose the next slice, not intuition. The largest right now:

| Runs | What stops them |
| --- | --- |
| 17,069 | `async` functions and generators |
| 6,896 | regular expression literals |
| 3,474 | `Temporal` — a Stage 3 proposal, and **not** ES2023 core |
| 3,137 | `BigInt` literals |
| 1,339 | `eval`, and 412 more for `new Function` — both need compiling at run time |
| 872 | a closure over a `let` or `const` declared in a loop — per-iteration environments |
| 838 | `BigInt64Array` and `BigUint64Array`, which need `BigInt` first |
| 830 | modules |
| 779 | `RegExp` as a *global*, on top of the 6,896 literals |
| 770 | `Proxy` and `Reflect`'s other half |
| 228 | the *lazy* `Iterator` helpers — `map`, `filter`, `take`, `drop`, `flatMap` and `Iterator.from`, each of which needs a helper object carrying state |
| 408 | `SharedArrayBuffer`, and 182 more for `Atomics` |

**Read the *failure* buckets, not only that table.** The list above is what stopped the tests that
never ran, and it is all architecture. Sorting the ~17,000 that **run and fail** by reason is what
finds the slices worth a day, and none of this session's were visible above:

    grep -av '^#' conformance/expectations.txt | sed 's/.* :: //' | sort | uniq -c | sort -rn | head -25

That is how `Array.prototype.sort` (85 runs), the four change-copy methods (~130), `ToObject` on a
primitive receiver (116) and the weak collections (~570) were found. Bucket by *path* too
(`awk -F/ '{print $1"/"$2}'`) to see which area is worth a slice rather than a method.

**And ECMA-262 cannot be read with a fetch tool.** Both the multipage and single-page builds answer
with their table of contents whatever anchor is asked for, so "read the clause first" is not
available that way. The vendored suite is the oracle instead: implement, run `--only <area>`, and
read the failing tests' `info:` frontmatter — which quotes the numbered steps verbatim, and is how
a wrong reading gets caught.

Note what that list says about order. The parser already accepts generators and `async`, so that
bucket is compiler and runtime work rather than grammar — and it is *one* piece of work, because
both need the same thing: an interpreter whose frames can be suspended and resumed. That is the
largest single change left in the engine and it is worth planning before starting.

Two things about reading that table. **`Temporal` is not ES2023** — it is a Stage 3 proposal with a
surface larger than `Date`, `Intl` and `RegExp` combined, and building it would raise the number
while making the engine no more of a JavaScript engine. Leave it.

And a bucket whose reason names the *harness* rather than the engine is worth being suspicious of.
10,737 runs were once skipped for "an async test reports through `$DONE`" — one missing host
function standing in front of a fifth of the suite. Providing it moved +786 to passing and revealed
that the real top blocker was async and generators all along.

**The two mistakes this session made twice**, both worth knowing about before the next slice:

- A test can *pass for the wrong reason*. `Promise.all.call(eval)` threw a TypeError because
  `Promise.all` was undefined, which is what the test asked for and not why; and six TypedArray
  tests passed because `testTypedArray.js` runs its body once per constructor and there were none.
  Both only became visible when the thing they were about arrived, and both look like regressions.
- The local loop is `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` **and**
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`, each by name. The gate does not
  cover the third: a public item's doc linking to a private one is an error in CI and nowhere else,
  and it reddened a build.

Read [`GOAL.md`](GOAL.md) first — it is binding and it outranks this file — then `src/span.rs` to
calibrate on the bar. `cargo run --release --example parse -- --commonjs <dir>` over a real
repository is the fastest way to find something worth fixing; `examples/evaluate.rs` runs code.
