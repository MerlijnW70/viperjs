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

| Path | What |
| --- | --- |
| `src/` | The engine. Zero deps, no `unsafe`, no panics, fully tested. |
| `lab/` | Experiments — see [`lab/README.md`](lab/README.md). Not shipped, and cannot be imported by the engine. |
| `conformance/` | The test262 harness and its expectations ratchet. |
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
api.rs       the embedding surface           [DONE — DR-0021, and `examples/embed.rs` is the tour]
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
and 80%, and the section below is what it cost.

**And §B.1.2's regular expression grammar runs**, which is the *other* half of Annex B and was still
refused wholesale until DR-0008's second amendment. A pattern carrying neither `u` nor `v` now reads
`/}/` as a brace, `/\1/` with no group as a legacy octal escape, `/\8/` as an `8`, `/\c1/` as three
characters and `/[\d-x]/` as a union. See the section below for the shape of it.

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
the section below for the three ordering faults that fell out of it.

Conformance as of this commit is **85.66% of test262** — 79,802 of 93,161 runs on the run this
sentence was written from, and a few hundred either way on the next. Treat that number as
perishable and re-measure rather than quoting it; the point of the figure is the work list under it.
Only 316 runs are now *stopped* before anything executes. **One of them was misfiled here for a
long time and it matters:** `(?i:…)` 170 is the RegExp **modifiers** proposal and is excluded, but a
property of strings is **not** a proposal — `regexp-v-flag` sits unmarked in test262's
`features.txt` and shipped in ES2024. `\q{…}` was 54 of those and is now built; what is left of the
flag is the 86 runs that need Unicode emoji sequence data (92 on the current run, six of them the
`v`-flag's other stragglers). The rest is `this agent cannot block` 14, `super` in an arrow's direct
`eval` 16, and two dozen module-beside-the-test parse failures that are proposals.

**The failure buckets are the whole work list now.** Sorted by reason the largest look actionable
and mostly are not, which is worth doing once and writing down rather than re-deriving:

| Runs | Reason | What it really is |
| --- | --- | --- |
| 8,316 | `Temporal is not defined` | a proposal — see below |
| 405 | `what was called is not a function` | proposals now, and only now: `Array.fromAsync`, `Iterator.zip`/`zipKeyed`/`concat`, `Promise.allKeyed`/`allSettledKeyed`, `Map`/`WeakMap`'s `getOrInsert`, `Uint8Array` base64, `DataView`'s `getFloat16`. **It was 939, then 821, and this file called it "mostly proposals" at both figures — twice wrongly.** Seven *shipped* functions were hiding in it the first time and **369 runs of `$262.createRealm`** the second, which is nearly half. It is 407 because both were built. **Ask the engine what it has** before believing this row; it has misled two readers already |
| 352 | `cannot read a property…` | **Atomics 224, and every one of them is `$262.agent`** — the row is `$262.agent.start` reading a property of `undefined`. Done as a target, see below. Beside it `Error.prototype.stack` 64, a proposal, and `legacy-accessors` 24, which is another |
| 208 | `$DONE with what was called is not a function` | the asynchronous half of the row above, and the same proposals |
| 293 | `expected 'meta', found an identifier` | `import.defer` and `import.source` — two proposals, not `import.meta` |
| 238 | `Calling as constructor…` | all `Temporal` |
| 224 | `expected ';', found an identifier` | `using` / `await using` — explicit resource management, a proposal |
| 178 + 144 | `DisposableStack`, `AsyncDisposableStack` | the same proposal's library half |
| 118 | `ShadowRealm is not defined` | a proposal, and **not** DR-0025's realm: that one shares a heap and passes objects freely, where this puts a membrane between the two sides |
| 34 | `it did not parse: unexpected character` | **decorators**, a proposal — and the one row here whose reason says nothing at all about what it is. Its paths do |

**Two buckets have been costed and must not be re-costed.**

- **`RegExp/property-escapes` is not what this file said it was, and the correction is instructive.**
  It read: "dead as a GC target … these need an interpreter several times faster, which is M8",
  resting on `lab/NOTES.md`'s `gc-pressure`, where `ASCII.js` takes 21.8 s against a 10 s budget.
  That measurement is about **one** file and was generalised to all 878. Measured 2026-08-05: run
  alone the directory passes **814 of its 1,226 runs**, and 890 of them stabilised the moment the
  worker count was halved. So most of the bucket needed a flag, not a milestone — and the reason
  string in the expectations file, which said `the heap has grown past…`, was simply the wrong
  failure. **Read a reason as a claim somebody made, not as a measurement.** What is left of the
  bucket is genuinely slow and M8 is genuinely its answer, but it is hundreds of runs rather than
  878. The experiment's two real findings — a throwaway heap String per computed property key, and
  a timed-out run landing in no column — are both **fixed**; do not go looking for them again.
- **`Temporal` is a Stage 3 proposal with a surface larger than `Date`, `Intl` and `RegExp`
  combined.** Building it would raise the number while making the engine no more of a JavaScript
  engine, and it will sit at the top of that list for as long as this file is worth reading.

### A second realm, and the five things that had never had to say *whose* intrinsics — DR-0025

+383 across five slices, 79,307 to 79,690, and the largest actionable area since the generators.
`$262.createRealm` was 381 runs on its own, and it was hiding in a bucket this file had twice called
"mostly proposals". Read the record before touching any of it; what follows is what the record could
not know in advance.

**The order in the record was wrong and the buckets said so.** It put the host binding last, so that
no test could go green against half an implementation. Measured instead: of the 195 files that need
a second realm, 111 only take an object or a constructor across and 17 run code in it — so most were
never waiting on the machine's realm at all. Binding `$262.createRealm` **second** turned a guess
about what remained into a measurement, and 168 runs came back before any of the engine work.

**Two bugs were older than the feature and could not be seen from inside one realm.** The
well-known Symbols were built by `Realm::new`, so a second realm would have made a second
`Symbol.iterator` — and the failure would have been silent, since an object carrying one realm's
`@@iterator` is not iterable in the other rather than erroring. And `Realm::intrinsics` was a
*ceiling*: a realm built second would have rooted everything older than it, leaving DR-0023's
collector sound and blind. Both are the same shape — **a fact that is only wrong when there are two
of something**, in a codebase that had one for its whole life.

**Three ordering faults fell out of §10.1.13 becoming a real `Get`.** Reading an own data property
observes nothing, so *when* it ran had never mattered; a `Get` can call a getter, and then it does:

- `AllocateArrayBuffer` step 3.a's RangeError comes before step 4's `OrdinaryCreateFromConstructor`.
- §23.2.5.1's two branches allocate in **opposite** orders — step 6.b.i first for an Object
  argument, step 6.c.ii's `ToIndex` first for anything else.
- §10.2.2 removes the callee's context at step 13 and runs step 14's TypeError and step 15's
  `GetThisBinding` after it, so both belong to the **caller's** realm.

**And `enter_native` put the realm back one line too early.** `settle` is where an `Abrupt` becomes
an error *object*, and §10.3.1's context is still the callee's when it does — so a built-in from
another realm threw the caller's `TypeError`. Eighteen runs ask for the other one by identity.

**A `Frame` must carry a `RealmId` and not a `Realm`, and I got that wrong first.** The record made
the argument about *function objects* — a `Realm` is 616 bytes and a function is made per closure —
and the same argument is sharper for a frame, which is pushed per **call**. Holding the realm itself
took a frame from 128 bytes to **736**, against DR-0019's measured 74 bytes of retained cost per
call. Nothing a program does distinguishes the two, so no behavioural test failed and none could;
what holds it now is a structural test beside `Frame`. **When a record gives a reason, check every
place the reason applies rather than the one it names.**

**`own_realm` exists beside `realm_of` for one distinction.** §10.5.12 is an *internal method*, not
a built-in: calling through a Proxy pushes no execution context, so the running realm stays the
caller's and the trap's arguments array is made in it. ViperJS gives a proxy its `[[Call]]` through
`make_callable`, so it does carry a realm — whoever built the proxy — and switching on it regresses
`Proxy/apply/arguments-realm.js`, which is the test that exists to notice.

**What is left of the area is a long tail rather than a slice**: ~84 runs across `MakeConstructor`
step 7.a's `%Object.prototype%`, `bind`'s prototype, `Symbol.split`'s splitter and a dozen others,
each two to ten runs and each its own clause. `ShadowRealm`'s 118 are a different proposal.

### An array index is a key of its own, and the biggest win was in the file nobody costed

DR-0026, and the first slice here taken for **speed** rather than for a conformance number — which
moved not at all, deliberately. `PropertyKey` gained an `Index(u32)` variant, canonical: a key is
`Index` exactly when §6.1.7 says its spelling is an array index, so `a[0]` and `a["0"]` are one key
by construction while `a["01"]` stays an ordinary named property. `a[i]` used to spell the Number,
encode it to UTF-16 and intern it, then decode the units back at the access; both halves are gone.

| | before | after |
| --- | --- | --- |
| `a[i & 7]` — varying index | 412 ns | **212** |
| `a[i & 7] = i` — write | 895 ns | **439** |
| `Int32Array` element read | 958 ns | **217** |

**The TypedArray row is 4.4× and no estimate named it.** §7.1.21 `CanonicalNumericIndexString` was
implemented by decoding the key's UTF-16 into a Rust `String`, parsing an `f64` and formatting it
back to check the spelling round-trips — per element access. **A representation change pays wherever
the representation was being re-derived, and those places are found by grepping for the decode, not
by reasoning from the hot path.** The estimate was drawn from a benchmark, and a benchmark only
names the rows somebody thought to write.

Four things around it are the reusable part:

- **A transparent optimisation cannot be killed by mutation coverage, and this is the third one.**
  Breaking the fast path leaves the slow path answering identically, so no program tells the
  difference and the survivors sit there. What closes it is a **structural** test — here, that the
  numeric index test and the one asked of the spelling agree over a corpus.
- **And that test found a real bug the behavioural tests could not.** `ToString(-0)` is `"0"`, so
  the *Number* `-0` **is** index zero; `"-0"` the *text* is not an index, and the fast path had been
  written with a sign check copied from the text rule. Every program was right, because the slow
  path was right. **A rule about a spelling does not transfer to the value it spells.**
- **Four branches were equivalent mutants and each was a duplicate to delete, not a test to
  write.** A key-comparison spelled out by hand where one method already said it, a Symbol guard in
  front of a walk that already declines Symbols, a proxy's key-to-value with an arm for a shape the
  type does not have, and a dead `&Heap` parameter on `as_array_index`. Once one question has one
  answer, the second copy of it is unreachable by construction — which is what the survivors were
  saying.
- **The comment-drift test earned its keep again**: removing `as_string` left a doc naming it, and
  the suite failed on the prose rather than on the code.

**And the conformance suite found a bug the slice was never near, because it took away what had been
hiding it.** Eight runs regressed, all `Array.prototype.slice`, all timing out. `slice` asked
`ArraySpeciesCreate` for a **zero**-length array where §23.1.3.25 step 8 asks for `count` — so
`{length: 2 ** 32}.slice(0, 2 ** 32)` never got §10.4.2.2 step 1's RangeError and walked four
billion absent elements instead. It had always done that, and the walk had always ended: interning a
key per index spent DR-0013's heap budget, which throws a RangeError of its own, and
`assert.throws(RangeError, …)` cannot tell two RangeErrors apart. An `Index` key allocates nothing,
so the walk stopped ending.

Three things in that, and the third is the one to carry:

- **`assert.throws(RangeError, …)` is the weakest assertion test262 has**, and this is the fifth
  time in this file that a test passed against the bug it exists to catch. A count that is
  observable as a *constructor argument* is what distinguishes them, and there is now a test asking
  for it: `class A extends Array { constructor(n) { seen = n } }`.
- **`within_budget`'s doc stated the premise in as many words** — "each pass interns a key, so a
  walk that is going nowhere is also a walk that is spending the budget" — and DR-0026 falsified it.
  A doc that explains *why* something works is also the thing that tells you what broke it.
- **The hang is not a hole to plug.** `Array.prototype.indexOf.call({length: 2 ** 53 - 1}, x)` loops
  because §23.1.3.17 says to, and **node does not return from it either — measured rather than
  assumed**. The engine's answer to a program that will not stop is DR-0022's time budget, which is
  the host's to set. What the old accident bought was termination for the wrong reason.

### §25.4's three waits do not share a fate, and one of them works with a single agent

+170 runs across three commits, and the reason it was worth more than the estimate is that
"ViperJS has one agent" had been read as one fact when it is two.

**A blocking wait is impossible and a non-blocking one is not.** `Atomics.wait` throws a TypeError
because DoWait step 12 asks `AgentCanSuspend()` and an agent that suspended here could never be
woken — a browser's main thread answers identically, which is what test262's `CanBlockIsFalse` flag
means. But `Atomics.waitAsync` **never suspends**: the agent parks a promise, carries straight on,
and reaches `Atomics.notify` a statement later to wake its *own* waiter. `undefined-for-timeout.js`
is exactly that program. So §25.4.1's waiter list is real here, and `notify` counts what it woke.

Four things around it cost more than the clauses did:

- **Two clauses that look alike decide oppositely about a plain `ArrayBuffer`.** `wait` refuses it
  with a TypeError; `notify` answers `0`. Nothing puts those side by side — it came out of two
  separate `info:` blocks.
- **`ValidateSharedIntegerTypedArray`'s refusal is inside step 1, so it lands before the index is
  converted.** Both orders answer TypeError and only a poisoned getter says which ran first, which
  is what `non-shared-bufferdata-throws.js` measures. Those four tests **had been passing on the
  gap**: calling an undefined `Atomics.wait` is also a TypeError, so they were green against the
  bug they exist to catch. Expect that whenever a slice removes a refusal.
- **The waiter list is keyed on a buffer and a *byte* offset**, per §25.4.1. A `BigInt64Array`'s
  slot 0 and an `Int32Array`'s slot 0 are one position; an element index makes them two lists and a
  notify through the wrong view silently misses.
- **The harness was skipping `CanBlockIsFalse` and `CanBlockIsTrue` alike**, reason "agents are not
  implemented". The first describes *this* host and needs no agents. Fourth time a reason string in
  the harness's voice was read as a fact about the engine.

**DR-0024 records what is not built**, and the boundary is one shape: a waiter with a finite,
non-zero timeout that nothing notifies should settle `"timed-out"` when it elapses, and there is no
clock the job queue can wait on. Settling it early is a lie `Date.now()` can measure, so it stays
parked and a test pins that it does.

**And two mutation survivors pointed opposite ways within an hour**, which is the distinction worth
carrying: `notify`'s `undefined` branch was unobservable — `ToNumber(undefined)` is `NaN`, cannot
throw, and the count was discarded — so the **code** was deleted; `waitAsync`'s `write_numeric(…,
false)` was observable and the **test** was missing, because every existing row used small numbers
where wrapping and clamping agree. Same report, opposite conclusions, and the question that
separates them is whether a program could ever see the difference.

**A shared helper was leaking a sign.** `to_integer_or_infinity(-0)` answered `-0` where §7.1.5
step 3 gives the *mathematical* value 0, which carries no sign — invisible wherever the answer
becomes an index, visible wherever it is handed back. The test asks `1 / x` rather than `x`,
because the two zeroes print identically and an assertion on the value passes either way.

### Duplicate named capture groups, and the check that needs *which* disjunction

§22.2.1.1 lets two groups share a name when `MightBothParticipate` is false — when some
`Disjunction` has them in different `Alternative`s, so no single match can fill in both.
`/(?<x>a)|(?<x>b)/` is legal and `/(?<x>a)(?<x>b)/` is not. +26 runs.

**Depth is not enough, and that is the whole design.** Recording each name's nesting depth and
comparing alternative indices level by level gets `/(?:(?<x>a)|b)(?:c|(?<x>d))/` wrong: both groups
are the *n*-th alternative at the same depth, of two **different** disjunctions sitting side by
side, and `"ad"` fills in both. So `survey` carries a path of `(disjunction id, alternative index)`
and the walk has three outcomes — same disjunction and different alternatives is the clause's
`false`; a different disjunction means the two are in separate groups side by side, so `true` and
nothing deeper is shared; running out is `true` as well.

Two consequences beyond the parser, each one line and each with a test that fails without it:

- **`\k<name>` refers to *every* group of that name**, §22.2.2.9, so the lookup is `find_map` over
  all of them rather than `find` on the first. The first one wearing the name may be in the
  alternative the match did not take, and a backreference to a group that did not participate
  matches the **empty string** — so `/(?:(?<x>a)|(?<x>b))\k<x>/` would match `"b"` alone.
- **`groups` gets one property per *distinct* name**, at the position the name is first written,
  holding whichever group took part. Defining it once per group lets a later `undefined` overwrite
  the alternative that matched, and the answer would depend on source order.

**What it exposed is worth naming: six tests moved from `did not parse` to a real gap** — the `d`
flag's `.indices` array, which ViperJS did not build at all. **That slice has since landed**
(§22.2.7.8, +48 runs), so `/(a)(b)/d.exec("ab").indices` is `[[0,2],[0,1],[1,2]]` today. Left here
because the *shape* is the reusable part: a bucket that stops saying `did not parse` and starts
naming a real gap is the ratchet handing you the next slice.

### One `?`, and 109 runs: §15.8.4 rejects where the generator clauses throw

Three clauses evaluate a body, and they differ in one character. §15.5.4 and §15.6.5 — a generator
and an async generator — both begin `Perform ? FunctionDeclarationInstantiation(…)`, so a throw from
a parameter default reaches the **caller**. §15.8.4, a plain `async` function, runs the same
instantiation as a **Completion** and step 3 hands an abrupt one to the promise's `reject`. So
`async function f(x = x) {}` answers with a rejected promise where `function f(x = x) {}` throws at
the call, and `async function* g(x = x) {}` throws too.

ViperJS put the rejecting handler *below* the parameter prologue on the reading that "a throw from a
parameter default is the caller's to catch" — which is right for two of the three clauses and was
written in a comment as though it were a rule. Moving it above the prologue for a non-generator
`async` function is the whole change; the async generator keeps its handler where it was, and that
is the row that stops the fix being applied to every async body there is.

**The bucket undercounted it by three to one.** 36 runs wore the dead zone's reason string
(`a let or const was read before its declaration ran`); the other 73 were every other way a
parameter list can throw — a pattern against `null`, a default that calls something — each filed
under a reason of its own. **A clause about where a completion *goes* will never bucket cleanly,
because the bucket is keyed on what produced it.**

### §10.4.5's internal methods, and the three comments that stated the bug as a rule

+44 runs across `[[Set]]`, `[[DefineOwnProperty]]`, `[[Delete]]` and `[[OwnPropertyKeys]]`, and not
one of them was a missing feature. Each was a clause ViperJS had *nearly* right, and **three of the
four were a doc comment asserting the opposite of the specification** — the shape this file already
names, in its most expensive form, because a comment that reads like a rule is what stops the next
reader checking.

- **`[[Set]]` never consulted its Receiver.** §10.4.5.4 step 2.b.i writes the element only when
  `SameValue(O, Receiver)`; otherwise the write is §10.1.9.2's and lands on the receiver. ViperJS
  wrote the buffer regardless, above a comment saying "the element belongs to the buffer and no
  receiver can move it elsewhere" — which cites §10.4.5.**5**, the *define*, for a rule the
  `[[Set]]` clause does not have. So `Reflect.set(ta, 0, v, {})` wrote `ta`, gave the plain object
  nothing, and *converted* `v`, which step 2.b.ii also does not.
- **A define never converted its value.** §10.4.5.5 step 1.f hands it to §10.4.5.16, so
  `Object.defineProperty(ta, 0, {value: {valueOf(){ throw }}})` throws and one holding `"7"` stores
  a seven. ViperJS stored `NaN` for everything that was neither Number nor BigInt, under a comment
  reading "a define carries a value that is already a Value, so there is no conversion to run here"
  — true of the datum, false of the clause.
- **A detached buffer kept its length.** `view_out_of_bounds` answers `false` for one deliberately,
  so its callers can raise their own error; `any_view` treated that as the whole question, and a
  detached view resolved to its stored length. §10.4.5.1 `IsValidIntegerIndex` step 1 says
  otherwise, and the three methods that ask it were each **exactly wrong**: the define accepted,
  the delete refused, and `Object.getOwnPropertyNames` named four indices whose descriptors were
  every one `undefined`.

**Two things about the shape are worth carrying beyond this area.**

- **The two clauses run their conversion and their index test in opposite orders, and that is what
  distinguishes them.** `[[Set]]` converts first and judges the index after (§10.4.5.16 step 1, so
  `ta[99] = {valueOf(){ throw }}` throws); the define judges first and converts at step 1.f, so an
  out-of-range index and a `configurable: false` descriptor both refuse without running anything.
  Counting `valueOf` calls is the only test that tells them apart — an assertion about the
  *answer* passes either way.
- **DR-0011's seam moved a whole clause, and deleting the half-answer was the point.** The heap
  cannot run §10.4.5.16, so `Vm::define_through` owns steps 1.f and 1.g and the heap keeps 1.a to
  1.e. `DefineOutcome::WrongContent` went with it: it existed only because the heap could refuse
  the *types* while unable to convert the *values*, and once the conversion moved, the branch was
  one no program could reach. §10.4.5.5 step 1.g is also why the answer is not re-derived after the
  conversion — a `valueOf` that detaches still leaves a define answering `true`, and asking the
  heap a second time said `false`.

### A lookahead is quantifiable, and Annex B says exactly where

+10 runs, and the whole of it is that §22.2.1's `Term :: Assertion` carries no `Quantifier` at all.
Annex B §B.1.2.1 adds one exception, and it is narrower twice over: `QuantifiableAssertion` is
`(?=…)` and `(?!…)` **only**, and the production is `[~UnicodeMode]`. So `^*` and `*` are refused
whatever the flags, a lookbehind was never quantifiable, and `/(?=a)*/u` is a SyntaxError where
`/(?=a)*/` is a pattern. It was written down here as "the one place left where a flag decides the
*grammar* rather than the matching", which was true of the engine and never of the specification —
see the §B.1.2 section below, where the other dozen productions turned out to be, and where this
slice is what showed the line had already moved.

**The comment above the check had gone stale in the way this file keeps meeting**: it said DR-0008
refused Annex B's syntactic extensions "in both", which was true when it was written and stopped
being true when §B.3 landed. The behaviour it described was still right for `^*` — for a different
reason, which is exactly what makes that class of drift invisible.

**And a probe written through a shell can lie about backslashes.** `eval("/\b+/")` reached the
engine as `/+/` with `` a *backspace*, so a valid pattern read as ViperJS wrongly accepting
`+` — a bug that does not exist, and a unit test three lines away said so. The engine's own test
helpers take a Rust string and have no such layer; when a hand probe and a unit test disagree about
an escape, **suspect the probe**. It is the third time an escaping layer has manufactured a finding
here. It happened a **fourth** time in the §B.1.2 slice below — `printf` in a shell ate a backslash
level and made `.source` look as though it dropped one — and the fix was the same: write the probe
with a file tool and feed it to `examples/evaluate` on stdin.

### §B.1.2 is the other half of Annex B, and DR-0008 had already stopped covering it

+32 runs across three commits, no regressions, and none of it was hard. What makes it worth reading
is that **the decision record said this was refused and the code had already stopped refusing it**,
in one production, correctly, for exactly the reason the record's own amendment gives.

§B.1.2 replaces a dozen of §22.2.1's productions when a pattern carries neither `u` nor `v`. DR-0008
was reversed for §B.3 on the line "an Annex B rule is in when it is conditioned on strictness and
nothing else, that being a question the compiler can already answer". §B.1.2 is conditioned on
strictness not at all — it is conditioned on **a flag written on the literal**, which is a *more*
static fact, not a weaker one. The reason covered it and the wording did not, so the line is now:
**an Annex B rule is in when the source itself says which reading applies.** That is DR-0008's
second amendment; read it before touching that area.

**One shape runs through every production, and naming it once is worth more than the list.** Under
Annex B a production that *fails to match* is not an error — it hands the same text to the next
production. ViperJS refused at each of those points instead:

- `AtomEscape :: DecimalEscape` is conditioned on the number naming a group that exists. Out of
  range it is not a bad backreference, it is **not that production**, so `/\1/` with no groups is a
  `\x01` and `/(.)\1/` is still a reference. The group count decides.
- `LegacyOctalEscapeSequence`'s four productions differ in one thing: how many digits may follow the
  first. A leading `0`–`3` takes two more, a leading `4`–`7` one — which is what keeps the value in a
  byte, so **`\400` is a space and a `0`** rather than 0o400. `8` and `9` are in none of the four and
  fall to the identity escape, which is the whole of `/\8/`.
- A short `\x` or `\u` is the same idea: `/\xa/` matches `xa`, and `/\u{2}/` is `u` **quantified**,
  the braces having no other reading without the flag.
- `\c` not followed by a letter it accepts makes the **backslash alone** the atom and the `c` is read
  again. Inside a class the accepted set is wider — `ClassControlLetter` adds the digits and `_` — so
  `[\c0]` is a `\x10` where `\c0` outside one is three characters.
- `\k` is a named backreference only in a pattern that **has a group name** (`N` in the grammar,
  which the survey already answers). With none, `/\k<a>/` matches `k<a>`. The same fact takes `k` out
  of the identity escape, which is what stops the two readings ever meeting over one pattern.
- §B.1.4.1.1's `CharacterRangeOrUnion` — a range whose end stands for a set is a **union of three**,
  hyphen included, so `[\d-z]` matches a hyphen. Two plausible wrong readings agree with the right
  one on most patterns and the hyphen is what tells them apart.

**Two survivors were equivalent mutants, and deleting the argument was the fix.** An `in_class` flag
threaded to three call sites was unobservable at two of them: the digit fallback can never reach a
`\c`, and `\q{…}` needs `v`, under which the wider reading does not exist to choose. AGENTS.md
already records that an equivalent mutant is a signal to change the code rather than to write a test;
this is the second time, and the shape both times was **a parameter carrying a fact only one caller
has**. `class_atom` answers it now and the other two do not ask.

**And a bucket that "fails differently" named the next slice again.** Four runs moved off
`a backreference names no group` onto a real one: a **lone surrogate** in a pattern comes back from
`.source` as U+FFFD, because the parser reads Rust `char`s and a lone surrogate is not one. That is
DR-0004's seam, it was invisible while the pattern was refused, and it is unbuilt.

**What is left of Annex B in §22.2 is `legacy-accessors` — `RegExp.$1` and its ten siblings, 48
runs.** Those are the *Legacy RegExp Features* proposal and not §B.1.2, so the directory name is
misleading in the way this file warns about: **check a bucket's directory *and* its feature flag
before costing it.**

### Two of §20.2.1.1's four kinds were missing, and the lookup answered with the wrong callable

+48 runs. `%GeneratorFunction%` and `%AsyncGeneratorFunction%` — §27.3.1 and §27.4.1 — were not
built at all, so `Object.getPrototypeOf(function* () {}).constructor` walked **past**
%GeneratorFunction.prototype% to `Function.prototype.constructor` and answered plain `%Function%`.
That then assembled `function anonymous() { yield 1 }` and refused it, and the failure bucket said
`the source of a dynamic function does not parse` — a sentence about the program's own text, for an
intrinsic that was not there.

**A missing intrinsic that answers as its parent is worse than one that answers `undefined`**, and
that is the shape worth carrying: the wrong object is *callable*, so nothing refuses, and the error
names the wrong thing. `%AsyncFunction%` is reached the same way and had been built for the same
reason; nothing connected the two until the bucket was read.

Two details cost more than the two constructors did:

- **The two links between a constructor and its prototype have different shapes.** §27.3.2.1 gives
  `%GeneratorFunction%.prototype` all three attributes `false`; §27.3.3.1 gives the `constructor`
  pointing back `configurable: true`. One helper for both gets exactly one of them wrong, and
  `propertyHelper.js` checks both.
- **Step 27 is the one step the four kinds do not share.** Only the ordinary kind is a constructor.
  A plain `async function` gets no `prototype` at all; the two generator kinds get §15.5.4's, which
  inherits from %GeneratorPrototype% and has **no `constructor` back-pointer** — a generator
  function has no `[[Construct]]`, so the property would be a lie a script can read.

**Still missing beside it, and separate:** `Function.prototype.toString` of *any* dynamically built
function answers `function anonymous() { [native code] }` rather than §20.2.3.5's assembled source.
That is one bucket for all four kinds and was not part of this slice.

### `arguments` was not iterable, and the bucket that found it was three layers away

+160 runs from one missing property. §10.4.4.4 step 16 and §10.4.4.6 step 7 both give an arguments
object `%Symbol.iterator%` = `%Array.prototype.values%`, and ViperJS gave it none — so
`[...arguments]` and `for (x of arguments)` threw `what was called is not a function`, which is
§7.4.4 asking an object with no `@@iterator` for one. Every other part of the object said array-like.

**How it was found is the reusable part, and it took three steps none of which named it.** The
bucket said `log.length Expected SameValue(«3», «2»)` across 48 runs of `yield-star-sync-*`. That
was a *real and different* bug — §7.4.3 step 1.b.iii runs `GetIteratorFromMethod`, whose step 4
reads `next` once and makes a record, and step 1.b.iv hands the **record** to §27.1.4.1; ViperJS read
`next` there and again, so a `next` getter fired twice. Fixing it moved all 48 to
`what was called is not a function` — 0 newly passing — and only then did reproducing the test's own
helper by hand show that `[...arguments]` was the thing that threw.

- **"Failing differently" with zero newly passing is a *good* result, not a wash.** It is the
  ratchet saying the first gap is closed and naming the one behind it. The temptation is to revert.
- **A hand-written probe of a failing test is worth more than the test's reason string**, and the
  fastest way to write one is to copy what the test's fixture does — the getters, the spread, the
  logging — rather than what it asserts. The engine bug was in the *fixture*.
- Two doc comments told on themselves again. §27.1.4.1's said the sync `next` was read there,
  "which is what makes this an Iterator Record rather than a pair of lookups repeated per step" —
  it was the pair of lookups it described.

**`%Array.prototype.values%` is held by identity**, discovered after the built-ins run exactly as
`%Promise%` and `%ArrayBuffer%` are. The clause names the intrinsic, so replacing
`Array.prototype.values` leaves `[...arguments]` walking the one the realm was built with — and
reading it off the prototype at each call would pass every other test.

### Seven shipped library functions were simply absent, and `typeof` found them in one line

+146 runs. `Object.groupBy`, `Map.groupBy`, `RegExp.escape`, `String.prototype.isWellFormed` and
`toWellFormed`, `Promise.withResolvers` and `Error.isError` — ES2024, ES2025 and ES2026, all Stage 4,
none of them built. Each is between five and forty lines.

**Finding them cost one probe.** The `what was called is not a function` bucket is 905 runs and this
file called it "mostly proposals", which is true and was hiding these: a `typeof` over three dozen
names sorted the missing intrinsics into *shipped* and *proposal* in a single run, where the bucket's
paths sorted them into nothing. **When a bucket's reason is "there is no such function", ask the
engine what it has rather than reading the failing paths** — the paths say which tests use it and
never which edition it landed in.

Four things in the clauses that a plausible implementation gets wrong:

- **`RegExp.escape` refuses a non-String rather than coercing it.** `RegExp.escape(123)` is a
  TypeError. The whole value of the function is that its answer is safe to concatenate, and a silent
  `ToString` is how the mistake it exists to prevent gets back in.
- **Its first-code-point rule is about *position*.** An ASCII letter or a digit is escaped only where
  it would begin the answer, so `"B*B"` is `\x42\*B` — read as "escape every letter" it gives
  `\x42\*\x42` and passes nothing.
- **`Object.groupBy` answers an object with a null prototype**, because the keys are the program's
  own data: a group called `toString` has to be an ordinary property. And §10.1.11 then reorders it
  — a key that is an *array index* sorts ascending ahead of everything, so a callback answering
  numbers loses the discovery order entirely. That is the object's rule and not `groupBy`'s, and it
  is why `Map.groupBy` sits beside it.
- **`toWellFormed` replaces one lone code *unit* at a time**, so the answer is always the same
  length as the receiver: two leading surrogates in a row become two replacement characters.

**A second, wider sweep found three more and cost nothing**: the same `typeof` idiom over every
method of `Array.prototype`, `String.prototype`, `Object`, `Reflect`, `Promise`,
`Iterator.prototype`, `Math`, `Set.prototype` and `Number` named exactly four absences —
`Promise.try` (§27.2.4.9, 20 runs), `Number.parseFloat` and `parseInt` (§21.1.2.12 and .13, 8 runs),
`String.prototype.normalize`, and `Math.sumPrecise` and `JSON.rawJSON`, which are proposals. Both of
the cheap ones are built.

- **`Number.parseFloat` *is* `%parseFloat%`** — the same function object, so
  `Number.parseFloat === parseFloat`. A second native with the same body answers every other
  question identically and that one wrongly.
- **`Promise.try` exists for the *synchronous* throw.** `Promise.resolve().then(f)` also gives a
  promise and runs `f` a turn later; `Promise.try` runs it now and still turns a throw into a
  rejection, which is the one thing a bare call cannot do.

**`String.prototype.normalize` is 20 runs and is not cheap**: it needs the UCD's canonical
decompositions, combining classes, composition exclusions and compatibility mappings. Eleven of its
fourteen files test only the error paths, which is exactly the trap — a `normalize` that returns its
receiver would pass them and be a silently wrong answer for the other three. Left unbuilt on purpose.

**And every one of the seven throws a TypeError for a bad argument, which makes the type worthless
as an assertion.** Two `groupBy` guards survived mutation because the test asked only for
`TypeError`: with the guard removed the walk throws one anyway, a few steps later. What
distinguishes them is *what ran first* — a `[Symbol.iterator]` getter that must not fire, and a
message about `items` rather than about the engine's next move. **The fourth time this session that
a test passed against the bug it was written to catch.**

### A step that throws is a step that finished, and destructuring was closing anyway

+32 runs, and it is one flag written in a different place. §7.4.8 `IteratorStepValue` sets
`[[Done]]` to true in *three* of its steps — 2.a when `next` throws, 5.a when a `done` getter
throws, 9 when a `value` getter throws — and what that decides is whether the caller abandoning the
walk then calls `return`. It must not: an iterator that failed to produce has not been left
mid-walk, and §8.6.2 step 4 and §13.15.5.2 step 5 both close only while `[[Done]]` is false.

**The fix is where the flag is written, not a list of the ways out.** ViperJS set `done` on the two
ordinary paths — spent, and a value arrived — and a throw from anywhere in between left it false.
Setting it **before** the call and clearing it only on the path that really produced a value is the
clause exactly, with no handler around the call and nothing to keep in step: every way out except
"a value arrived" leaves it set. Written as a handler it would have been three catch sites that can
drift apart from the three steps.

**The next slice was the ordering that hid behind it, and it is +53 more.** §13.15.5.5 step 1
evaluates an assignment target's *reference* before step 2 steps the iterator, and §13.15.5.6 does
the same between the property name and the read — so `0, [{}[thrower()]] = iterable` calls `next`
**zero** times and closes once, and `({ [f()]: o[g()] } = src)` calls `f`, then `g`, then reads
`src`. ViperJS fetched the value first in both, above a doc saying evaluating the reference earlier
"is not an option", which was the clause read backwards.

The compiler change is `hoist_reference` and `store_hoisted`: a property reference is two stack
entries, or **three** for `super`, so it is parked in slots until the value turns up. `Reference`
already knew its own width for compound assignment; this is its second caller.

**And behind *that*, §7.4.9 step 4 — with one exception that cost four regressions to find.** On the
way out of a throw the original completion wins, so every failure of the close is discarded: the
`return` throwing, and the getter step 2 reads it with. ViperJS swallowed those only for an
**awaited** close, under a comment that already said "every failure of the close is discarded".
Broadening it to every unwinding close turned four `yield*` tests red, and they were right: §15.5.5
step 7.b.iii.4 closes a source with no `throw` method carrying a **normal** completion, so step 4
does not fire there, the close's own error is what the program sees, and step 6 examines what
`return` handed back. That call site is `Check::Plain`, not `Check::Unwind`.

**And a third slice behind that one, +50: a pattern's iterator was closed by a *throw* and by
nothing else.** §13.15.5.2 step 5 and §8.6.2 step 4 close on **any** abrupt completion; ViperJS armed
a handler, which catches a throw. A `return` is not a throw, and there is a way to write one —
a default inside the pattern may `yield`, so `[ {} = yield ] = iterable` resumed with `it.return()`
unwinds straight through a half-run pattern. The fix is the `Crossing::Iterator` entry a `for`-`of`
head already installs, pushed **after** the handler is armed because closing takes the handler down
as part of the same jump.

**Three slices in a row where the ratchet named the next one.** The `log.length` bucket was a real
bug about `[[Done]]`; fixing it moved 48 runs to a *different* failure, which was the reference
order; fixing that moved 36 more, which was §7.4.9 step 4; and behind that were these 50. None of
the four appears in a list of missing features, and each was invisible until the one in front of it
was gone. **When a slice reports "0 newly passing, N failing differently", that is the ratchet
naming the next slice, not a wasted change.**

**One clause, two callers, opposite answers about the same error — for the second time in this
file.** §7.4.9 was already recorded that way for the Iterator Helpers' `return`; this is the same
distinction reached from the other side, and the tell both times is *which completion the close is
carrying* rather than what kind of close it looks like.

### §9.1.1.2.5 is four steps, and a `with` write was doing one of them

+12 runs. Writing through a name a `with` resolved is `SetMutableBinding` on an Object Environment
Record, and it asks `HasProperty` **again** before it writes: everything between resolving the
reference and using it is a program, so `with (o) { x += 1 }` where `o`'s `x` getter deletes `x`
reads a binding that is gone by the time the write happens. Step 3 tells strict code so with a
ReferenceError; sloppy code is told nothing and step 4 makes the property again. Step 4's `S` also
makes a refused write a TypeError in strict code, which is §6.2.5.6's rule for every other reference
and was not applied here.

**The doc named its own gap and was out of date.** It said ViperJS "does not yet carry a store's
strictness as far as `[[Set]]`" — the strictness is an argument three lines above it. That is the
fourth comment this session that described a condition which had already passed.

**And it exposed something larger, which is now built: a direct `eval` inside a `with` could not
see the `with` at all.** `with (o) { eval('x') }` threw where it should read `o.x`, and
`with (o) { eval('x = 7') }` made a **global**. **+1 run**, and the number is the point — this is a
silently wrong *scope*, and almost nothing in test262 asks.

It was two independent faults that had to be fixed together, and each hid the other:

- **The call was not a direct eval at all.** §13.3.6.1 asks how the callee was *written*, and a bare
  `eval` inside a `with` is written the same way as anywhere else — what the `with` adds is
  §9.1.1.2.10's `WithBaseObject` under it. ViperJS decided the call's **shape** and its
  **directness** with one `match`, and `(true, _) => CallMethod` threw the second away. So the text
  ran as an *indirect* eval, in the global scope. The two questions are independent and the table
  has four rows now.
- **And the compiler could not have placed the names anyway.** DR-0018's chain is a list of *name
  lists*; an object environment has none, so it arrives as an empty level indistinguishable from a
  temporary's. `Heap::any_binding_object` answers the one question the chain cannot, and the eval
  compiles with `with_depth` set — every free name a run-time walk, exactly as code written inside
  the `with` already does.

**A behaviour-preserving flag cannot be killed by mutation coverage, and this is the second one.**
Forcing `any_binding_object` to `true` is transparent — the walk finds exactly the binding a slot
would have named — so no program tells the difference and three survivors sat there. What closes it
is a **structural** test: one on the emitted instructions (`compile::tests`) and one on the heap
walk itself, asserting the `false` side that only costs speed. `lab/`'s `name-resolution` measured
what that side is worth: 3.0× to 3.7× on local variable access.

### §27.2.4.1 step 8.a was missing from all four combinators

+24 runs, and it is the same clause the destructuring slices met: an abrupt walk closes the
iterator unless the iterator is where it went wrong. `Promise.all` over an iterable whose
`C.resolve` throws left it open — and a `resolve` that throws is the *first* thing that happens
after a value has been taken, so this is not an exotic path.

**Where the boundary sits took two rounds of mutation coverage to settle.** §7.4.2 step 4 reads `next`, and that
belongs to building the **record**, not to walking it: a `next` *getter* that throws is step 5
failing, so step 8 is never reached and nothing is closed — where a `next` *call* that throws
leaves a record that is already done, which also closes nothing, for a different reason. Reading
`next` inside the walk made the first of those close.

**And the `[[Done]]` flag turned out to be the wrong shape here, which the ratchet said twice.**
A `let mut done = false` at the call site survived mutation both before and after the boundary was
moved, because by then nothing between the initialisation and the loop's first write could throw —
an initial value no input could reach. The fix was to delete the flag: **the placement of the `?`
says it instead.** Everything from the step to the value arriving propagates untouched; everything
after a value has arrived goes through one `subscribe` call whose single error branch closes. One
branch at one place, and no state to keep in step with three `?`s.

That is the *opposite* conclusion from the compiler's version of the same clause two sections up,
where a flag set before the call is exactly right — because there the steps are emitted as bytecode
and there is no Rust `?` to place. **Same clause, two engines, two shapes.**

### §B.2.2's four helpers went round the internal methods instead of through them

+20 runs, and no new logic at all — three calls, each swapped for the mediated form.
`__defineGetter__` and `__defineSetter__` define with `DefinePropertyOrThrow`, which is §10.1.6
**or** §10.5.6; `__lookupGetter__` and `__lookupSetter__` walk with `[[GetOwnProperty]]` and
`[[GetPrototypeOf]]`, both `?`. ViperJS read and wrote the heap directly, which walks **past** a
Proxy — so a trap that threw was never called, one that refused was never heard, and one that
answered a descriptor of its own was never asked.

**This is DR-0020's shape from the other side.** The eleven internal methods were moved out of the
heap so a Proxy could mediate them; a built-in that reaches for `Heap::own_property` rather than
`Vm::own_property_through` quietly opts back out, and nothing in the types says so. Worth grepping
for when a slice touches a built-in that walks a prototype chain.

**And the ratchet had nothing to say about it — correctly.** The change is three call sites and no
branches, so mutation coverage reported `0/0 viable`, which this file already records as honest for
a slice with no condition in it. What pins it is that each of the three had a probe answering
`accepted` before and the trap's own throw after.

### A class stopped being a predicate on one code point

+54 runs, and every one came out of the *stopped* column rather than the failing one — the
arithmetic this file asks for held exactly: `not run` fell 374 to 320 and `failed` did not move.
§22.2.1's `ClassStringDisjunction` — `\q{abc|def}` — was the last buildable piece of the `v` flag,
and it is the one that breaks the shape the rest of `src/regexp` is built on.

**A class was a predicate `fn(code point) -> bool`, and this makes it something that consumes.** The
design that keeps the predicate is a split, and the split is forced rather than chosen: an
alternative exactly **one** code point long is an ordinary member of the character set, and every
other length is a sequence. So the predicate goes on answering for the first — which is what lets
`[[0-9]--\q{0|2|4}]` remove three digits without anything enumerating `[0-9]` — and only the other
lengths are resolved into a list. The code points *cannot* be enumerated (`\d`, `\p{L}`); the
strings are finite and written down, so their set algebra is computable by hand.

Four things it turns on, each of which a plausible implementation gets wrong:

- **`MayContainStrings` is syntactic, and the resolved set is not.** §22.2.1 refuses `[^…]` by the
  first, so `[^[\q{ab}--\q{ab}]]` is a Syntax Error although the difference is empty, and
  `[^[\q{ab}&&[a]]]` is a class although its first operand could. The rule is per-operation — any
  operand for a union, **every** one for an intersection, the **first** for a difference — and
  reading it as "is the resolved set non-empty" gets both of those backwards.
- **§22.2.2.7.2 backtracks.** Candidates are tried longest first and each is offered to the
  continuation in turn, so `/^[\q{ab|a}]b$/v` matches `ab` by taking `a` *after* `ab` has failed.
  A longest-match rule answers the same for most patterns and wrongly for that one.
- **The empty alternative sorts last** — after every longer candidate *and* after the ordinary
  character read, because descending length puts a zero-length candidate below a one-length one.
- **A class that consumes a sequence is not one code point wide**, so the iterative quantifier's
  fast path must refuse it or `[\q{ab}]+` takes one code point a turn.

**And the ratchet caught a guard the parser's own early error had already made unreachable**:
resolving a nested class's strings skipped a negated one, which cannot hold any because `class_set`
refuses one that could. Deleting it was the fix, for the fourth time this session.

### §23.2.5.1's step 7 and step 8 are alternatives, and ViperJS ran both

+24 runs. A view over a **resizable** buffer with no explicit length takes step 7 and tracks it;
everything else takes step 8. They are different branches, and step 7 has no modulo rule — so a
ten-byte resizable buffer is an `Int32Array` of two, where the same ten bytes fixed are a
RangeError. ViperJS worked the lengths out first and decided `tracking` afterwards, so step 8's
checks ran over a tracking view and refused it outright.

**Why step 7 needs no modulo rule is the part worth keeping**: a tracking view's length is
recomputed from the buffer at every read and rounded down to whole elements *there*, so a remainder
shorter than an element is simply not reported. There is nothing to refuse at the start because the
answer is never given at the start.

**And the stored length of a tracking view is dead data.** Two operators inside the arithmetic that
computed it flipped under mutation and nothing noticed — `any_view` recomputes it and
`view_out_of_bounds` asks `!tracking` before reading it. It stores a zero now: working out the
right number was a claim no program could check, which is the "dead data" shape this file already
names, met from a new direction.

### The shrink half, and a read that asked the buffer instead of the window

+90 runs, which closes the resizable-buffer area. Three faults, and the third is the one to carry.

- **A walk snapshotted its elements.** §23.2.3.7 step 3 caches the **length** and step 6.b re-reads
  each *element* with `Get(O, Pk)` — two decisions, not one. `fold` already spelled that out in its
  own doc; `walk` took a snapshot, above a comment saying the clause "carries on with what it had
  rather than turning the rest of the walk into `undefined`s", which is what a snapshot does and
  not what the clause says. So a callback that shrinks the buffer still gets the number of turns
  the array started with, and the turns past the new end are handed `undefined`.
- **`get byteOffset` answered the stored offset when the view was out of bounds.** §23.2.3.3 step 4
  returns `+0`, exactly as the two length getters do — and those already did, because
  `Heap::any_view` zeroes a *length*. An offset is never zeroed there, for the good reason that the
  offset is what a shrunk view is out of bounds **by**.
- **`Heap::numeric_at` checked the buffer's bytes and not the view's window.** A view that is out
  of bounds resolves to a count of zero while its bytes are still there, so a direct read returned
  what the window no longer covers — `t[2]` answered `undefined` while a walk handed the callback
  `4`. The property path asks `index_of` and gets the count; this one never did. **Two paths to the
  same element and only one of them bounded** is a shape worth grepping for.

**An existing unit test asserted the second bug**, and its comment read like a rule: "`length` and
`byteOffset` are *not* among them: the getters answer rather than throwing" — true, and silent about
*what* they answer. The conformance suite is what caught it, which is the division of labour this
file already claims: mutation coverage proves the branches are tested, the suite proves they are
the right branches.

### `api.rs` exists — and what an embedder could not do before it

DR-0021. **The conformance number does not move a hundredth of a percent, which is why it had not
been built.** A twenty-line program that runs a script and does something with the answer was
written against the public surface and compiled: four of its six lines did not, and there was no way
at all to bind a host function. Our own harness was the evidence — `conformance` binds `$262` by
*writing JavaScript source*, because no API existed to bind a Rust function instead.

`api::Engine` owns the `Heap` and the `Vm` together. That is the decision, not the convenience: they
are separate objects inside and every operation takes both, so two heaps and one machine compile and
answer *silently wrong* — a `Value` is an index, and the wrong arena has something else at it.
`api::Host` is the same surface borrowed for the duration of one native call.

**Three things caught it being wrong, none of them reasoning:**

- **A failing test corrected the decision record.** It claimed DR-0019's generations made a stale
  handle safe on its own. They stop a *wrong value*; they do not stop `[[Get]]` degrading to
  `undefined`, which is what an absent property gives too. So `Engine` checks liveness on every
  value the host passes in and answers `Error::Collected`. Written before it was measured, the
  record was wrong in the direction that reads as reassuring.
- **`examples/embed.rs` caught what fourteen unit tests could not.** A bound host function could not
  convert its own arguments — `Vm::to_string` is crate-private, and every test lived *inside* the
  crate. The surface let a host register I/O and not implement it. **A test in the crate cannot
  measure the crate's boundary**; an example outside it can, and that is what examples are for.
- **Mutation coverage found a real bug, not a missing test.** Two guards in the thrown-value
  description were indistinguishable because a *missing* property became the string `"undefined"` —
  non-empty, so it slipped past both. `throw ({})` read as `"undefined: undefined"`.

DR-0021 deliberately did not decide **stopping a script that will not stop**; DR-0022 does, and the
section below is what that cost.

### A run has a time budget, and it cannot be caught — DR-0022

`Vm::set_time_budget(Option<Duration>)`, off by default, per *run* rather than a fixed instant. The
decision the rest follows from is that exceeding it is **not a throw**: a budget a script can catch
is not a budget, because `try { while (true) {} } catch (e) {}` would swallow it and the loop would
resume. So `Vm::stopped` is set, the loop reads no further instruction, `Outcome::Interrupted` is a
third case beside `Value` and `Thrown`, and §9.5's jobs are not drained.

**The check rides the counter that was already there.** `execute` counts down to DR-0013's heap
check every thousand instructions; `Instant::now()` sits in the same branch for the same reason —
tens of nanoseconds, nothing once per thousand and not nothing per instruction. One counter
scheduling two checks is *not* the "one field answering two questions" mistake: both ask it the same
thing, which is whether it is time for housekeeping.

Three things worth carrying:

- **The flag is read before every instruction, and the nested case is why.** A stopped inner
  execution simply returns; the call it was serving keeps a frame it never popped and produces no
  value, so `call_value` hands back whatever is on the stack *as though the call had answered*.
  Without the check the caller runs on for another whole interval. A first test asserted a `catch`
  would run and passed with the check removed — because a stopped execution does not throw, so the
  `catch` was never reached either way. **What distinguishes them is an ordinary statement after the
  call.**
- **An equivalent mutant is a signal to change the code, not to write a test.** `now >= deadline`
  and `now > deadline` differ only at the nanosecond the clock reads the deadline exactly, which no
  test can arrange. It is written `deadline.saturating_duration_since(now).is_zero()` — the same
  question with no second spelling.
- **Adding a case to `Outcome` broke six `match`es** — `api`, `module`, the conformance harness, two
  lab experiments and `examples/evaluate.rs` — and each one is a boundary where somebody has to
  decide what an interrupt means there. `--workspace` on the clippy line is what caught the last
  three; without it they would have been red in CI and green here.

**What it does not stop, measured rather than asserted.** §22.2's matcher is its own loop and does
not read the flag: `/(a+)+b/` against 18, 20 and 22 `a`s took 52 ms, 210 ms and 689 ms against a
**10 ms** budget. That is a test now rather than a sentence, and if it ever fails the matcher has
gained a check and DR-0022's list wants updating. Also outside it: a single long-running built-in,
and a host function that blocks.

### The harness's own reach is part of the measurement, and it was hiding 68 runs

**`conformance` prepends its `$DONE` shim to the source and parses the two together, so for a
`flags: [module]` test the whole prologue is *inside the module*.** `var $__status` was then a module
binding, and the probe that reads the result afterwards is a separate Script — which cannot see one.
Every async module test therefore reported `the test's status could not be read`, a sentence about
the harness with nothing in it about the engine. Written onto `globalThis` instead — the one
spelling that means the same thing in a Script and in a module — and 68 runs came back, `$262.global`
with them (`this` is **undefined** at a module's top level, so that field was wrong there too).

**This is the third time a harness decision has been mistaken for an engine result**, after the
compile-error-as-a-skip that hid 194 failures and the timeout column that lost runs outright. The
tell is the same each time: **a reason string that describes the harness rather than the program.**
`could not be read`, `not run`, `did not finish` — when a bucket's reason is in the harness's voice,
suspect the harness before costing the feature. It also improves what the *remaining* failures say:
one entry now names `import.defer` as the proposal it is, where before it said `asyncTest called
without async flag` — `asyncHelpers.js` checks that `$DONE` is an **own property of the global
object**, which is why it refused rather than running.

### 81.26% to 82.16% in seven slices, six of which a comment told on

**Five of the seven were found by grepping an area's comments for what they *promise*, and that
now out-yields the failure buckets.** The buckets name what is missing by its symptom; a comment
that says a step "arrives when Symbols do" names it by its clause, and the condition it waited for
usually passed several milestones ago. Two of the seven were a comment that was simply *wrong*
about the specification, which is worse than stale and reads the same.

1. **§19.2.6's four URI functions** (+340) — missing entirely, and the largest thing in the buckets
   that was neither a proposal nor already costed. Decoding refuses more than reassembling the bits
   would accept: step 4.c.vii.7 says a *valid* UTF-8 encoding, which is RFC 3629's definition, so an
   overlong `%C0%80`, any encoding of a surrogate and anything above U+10FFFF are all URIErrors.
2. **§7.1.1 step 1.a's `@@toPrimitive`** (+294) — never implemented, so
   `String({[Symbol.toPrimitive]() { return "ok" }})` was `"[object Object]"`. The third hint is the
   part that is not mechanical: `preferredType` may be *absent*, and only a method can see the
   difference between absent and number — which is why `date + 1` concatenates.
3. **§B.2.1's `escape`/`unescape`** (+62) — code units where §19.2.6 escapes UTF-8 octets, so
   `escape("\u{1F600}")` is `"%uD83D%uDE00"`. The set is the ASCII **word** characters and `@*+-./`,
   and the underscore is the trap: written as alphanumeric plus punctuation it falls between the two.
4. **§20.2.3's `Function.prototype`** (+18) — a callable object missing §10.3.3's own `length` and
   `name`. Invisible from the object itself: the two are configurable on every built-in, so only a
   read arriving after `delete parseInt.length` can tell.
5. **§20.1.2.7's `Object.fromEntries`** (+22) — read a `length` where step 4 is §7.1.5.1's walk of
   the **iterator** protocol, so it answered `{}` for a `Map`. §7.4.2 `GetIterator` is `Walk::over`
   now; `direct` walks what it was handed and `over` asks the object what to walk.
6. **§13.15.2 step 5's `NamedEvaluation`** (+18) — `value &&= () => {}` names the arrow. §8.6.3's
   list is drawn per-production and not per-category, so reading "compound" as one category puts
   `||=` beside `+=` where the grammar does not. **A test row asserted the bug**, in the words of a
   rule.
7. **§7.1.19's `ToPropertyKey`, and §10.4.5.9 behind it** (+86) — see below.

**The seventh is the shape this file keeps meeting, in both halves.** `ToPropertyKey` existed
twice: `Vm::to_property_key` runs `ToPrimitive` and can call a method, and a second copy in
`builtins/object.rs` called the *value layer's* `to_string` and so threw for every object. Twelve
functions took keys through the copy, which is why `Object.defineProperty(o, [1, 2], {})` was a
TypeError rather than a define of `"1,2"`. Deleting the copy was the fix; there was nothing to
write.

Behind it, exactly as this file predicts of a slice that removes a refusal, **two tests turned red
that had been passing on the throw** — and what they needed was the same shape one layer down.
§10.4.5.9's index test is asked of a TypedArray's window *now*, and every **read** path resolved
the length first while `[[DefineOwnProperty]]` and `[[Delete]]` read the stored one. A resize makes
that stale, and a define is where it becomes observable without a method to refuse first: the key
conversion runs the program's own `toString`, which is free to shrink the buffer in between.

### 79.38% to 81.26% in sixteen slices, and what they have in common

None of them was a feature. Every one was a clause ViperJS had *nearly* right, and fifteen of the
sixteen were found by bucketing the failures rather than by reading a list of what is missing.

**Three of the twelve are one shape**, and it is the one worth carrying: *a clause names a
completion, a flag or a step that ViperJS collapsed into one path serving two callers.* §9.1.1.4.17's
`D`, §7.4.9's completion and §27.6.3.8's `Await` are all that — and each of the three cost between
22 and 116 runs while being one condition or one instruction.

1. **Annex B §B.3** (+571) — DR-0008's reversal, built. See the section below; it is the only one
   of the eight that was on anybody's list.
2. **§19.2.1.1's `D`** (+22) — `CreateGlobalVarBinding(N, D)` has one parameter and its two callers
   disagree about it. §16.1.7 passes `false` for a Script and §19.2.1.1 `true` for an `eval`, so
   `eval("var x = 1"); delete x` answered `false`. **When a spec operation takes a flag, check every
   caller passes its own.**
3. **§21.1.1.1's `ToNumeric`** (+164) — `Number(1n)` threw. Step 1.a is `ToNumeric` and not
   `ToNumber`, which is the only place in the language a BigInt crosses without a second word from
   the program. None of the 164 is in `built-ins/Number`: they are `Array` and `TypedArray` methods
   over a resizable buffer, whose harness reads elements back through `Number(n)`.
4. **Annex B §B.2.3** (+164) — the thirteen HTML methods, `"x".bold()`. One `CreateHTML` and a
   table, because the order of its two conversions is observable and would otherwise have to be got
   right thirteen times.
5. **§21.1.3 and §20.3.3's prototypes** (+158) — `Number.prototype` and `Boolean.prototype` **are**
   instances holding `+0` and `false`, so `Number.prototype.toString()` is `"0"`. §22.1.3 says the
   same of `String.prototype`; §21.4.4 and §22.2.6 say the *opposite* of `Date.prototype` and
   `RegExp.prototype`. The split is per-clause with nothing to derive it from — which is why the
   same slice had to give §22.2.6's ten accessors their prototype carve-out, and make
   `RegExp.prototype.flags` read the eight as **properties** in the clause's order.
6. **§22.1.3's `regexp is an Object`** (+46) — a 2025 normative change. The six pattern-taking
   methods look up `%Symbol.match%` and its siblings only for an Object, because `GetMethod` on a
   primitive goes through `ToObject` and lands on a wrapper prototype a script can write to.
7. **§6.2.5.5's settled key** (+96) — `GetValue` and `PutValue` both convert a property
   reference's key *and write it back*, so `o[p] += 1` converts once. And `ToObject` of the base
   comes **first**, so `null[p] += rhs()` throws before either `p.toString()` or `rhs()` runs.
8. **§22.2.1's `ClassSetExpression`** (+78 passing, +78 honestly refused) — a `v` pattern's class
   is a set expression, and its three operations are three quantifiers over the operands rather
   than sets to compute.
9. **§15.7.1 inside a direct `eval`** (+24) — `eval("this.#m")` in a method of the class declaring
   `#m` was refused. Nothing new is stored to allow it: a private name is a slot and DR-0018
   already made every running scope name its slots, so the classes the call is inside are written
   down in the chain the compiler is handed a moment later.
10. **§7.4.9 with a normal completion** (+28) — an Iterator Helper's `return` **reports** what
    closing its source found, where every other close here is abandoning a walk that already went
    wrong and the clause discards the close's own trouble. One clause, two callers, opposite
    answers about the same error.
11. **§27.6.3.8 step 5** (+116) — `yield` in an `async function*` **awaits** what it yields, which
    an ordinary generator does not. One instruction, and the largest slice since the `v` flag.
12. **§10.4.5.2 step 8's first disjunct** (+60) — a *tracking* view whose **offset** is past the
    shrunk buffer is out of bounds. The doc claimed one could never hang off the end, which is true
    of its end and false of its start.
13. **§14.2.2's `UpdateEmpty`** (+104) — eight statement forms begin "Let V be undefined" and are
    therefore never EMPTY, so `eval("1; if (true) ;")` is `undefined`. Three forms really are
    empty and had to stay so, which is why the list is exact rather than "most statements".
14. **§6.2.5.6 step 6** (+46) — strict code may not create a global by assigning to an undeclared
    name. The instruction's own comment said ViperJS "does not yet carry a strictness through to
    here"; it had for some time.
15. **§13.10.2 step 2** (+74) — `instanceof` asks the right operand what it means. Its doc said
    the step would arrive "when Symbols arrive"; they had.

**A second shape, and it cost three slices in a row to see: a doc comment that says what a clause
*will* need is a claim nothing checks.** Three of the sixteen were a comment stating the missing
step correctly — §13.10.2's `@@hasInstance` "when Symbols arrive", §6.2.5.6's strictness "not
carried through to here", §10.4.5.2's tracking view that "can never hang off the end". Each was
written by someone who had read the clause, and each outlived the condition it described. **Grep
the area's comments for what they promise before costing it as missing.**

**The shape worth carrying: a bucket spread evenly over an area's whole surface is one common path,
not many faults.** Slice 3 wore the name of every `Array.prototype` and `TypedArray.prototype`
method it stopped, and was one word in `Number`.

### §B.3 is built, and what it actually cost is worth reading before touching that area

DR-0008 was reversed on 2026-08-03 and B.3 landed the same day: **+571 runs, no regressions**.
§B.3.4's `if (x) function f() {}`, §B.3.2's `L: function f() {}`, §B.3.3's extra `var` binding and
§B.3.3.5's duplicate-function carve-out are all in — `src/compile/annex_b.rs` decides which
declarations earn the binding and its module doc is the long version. Four things around it cost
more than the clause did, and each is a shape that recurs:

- **§B.3.2 needs a *position* rule as well as strictness.** §14.6.1 and its five siblings make
  `IsLabelledFunction(Statement)` a Syntax Error wherever a body is a `Statement` rather than a
  `StatementList`, so `while (x) a: function f() {}` stays refused while `{ a: function f() {} }` is
  taken. It is `parser::LabelledFunction`, an argument to `parse_statement` rather than a field —
  a field would have had an initial value no input could reach, which mutation coverage caught.
- **A `switch` was instantiating no functions at all**, and neither was a labelled declaration in a
  block. §14.12.4 step 3 hands the whole `CaseBlock` to `BlockDeclarationInstantiation`; ViperJS
  opened the environment, made no slot, and left the statement doing nothing because hoisting was
  supposed to have done it. `switch (x) { case 1: function f() {} }` bound nothing, silently.
- **The global path is not `at_global_scope`.** That question answers two at once — the var scope is
  the global object, *and* no scope has been opened here — and the second half is false inside the
  block, which is the only place §B.3.3's copy is ever emitted. A first attempt asked it there and a
  script's `{ function f() {} }` stored into a slot a script does not have.
- **A name the variable scope already has needs no guard.** `Compiler::declare` hands back the slot
  a name has and `CreateGlobalVarBinding` leaves an existing property alone, so the clause's "if
  instantiatedVarNames does not contain F" is satisfied by the primitives. A guard in front of
  either was a branch no program could distinguish, and mutation coverage said so.

**One divergence is deliberate and recorded.** §B.3.3.5 lets `{ function f() {} function f() {} }`
parse, and read as written neither declaration is then eligible for the `var` binding — replacing
either with `var f` leaves the other lexically declaring `f` in the same list, which §14.2.1's
second rule refuses and B.3.3.5 does not relax. Every browser answers with the second function. No
test262 file measures it, so the letter is what is implemented; `src/compile/annex_b.rs`'s module
doc says which line to change if data ever arrives.

### What is left, in the order the numbers put it

- **A sloppy `var` or function declaration inside a direct `eval` in a function — 95 runs** (75 for
  a `var`, 20 for a function), and the largest thing ViperJS refuses by name. **Attempted 2026-08-04 and reverted: 0 fixed, 18
  regressed, net −103 runs.** Do not re-derive the design — it was built, it works by hand, and it
  is not the blocker. §19.2.1.1's `varEnv` was recorded on the frame and each var-declared name
  sorted by comparing its resolved depth against it; `function f(){ var x = 1; eval("var x = 2");
  return x }` gave 2 and `{ let y; eval("var y") }` was a SyntaxError, both correctly.
  **What it lost on is that ViperJS puts a function's parameters, its `arguments`, its `var`s *and*
  its body's `let`s in one environment, where §10.2.11 has up to three.** A depth comparison cannot
  then tell "bound in the variable environment" (nothing to create) from "bound in a scope between
  here and it" (§19.2.1.3 step 5.d.ii.1's SyntaxError) — both are the same number, and the
  `declare-arguments` family is exactly that distinction. **So the prerequisite is §10.2.11's
  environment split, and it is a bigger slice than the one it unblocks.** The growth case — a name
  bound nowhere, needing a slot in a frame sized at compile time — was never in scope and is still
  open. §B.3.3's own bindings in such an eval sidestep all of it: they go in the eval's own scope,
  because every test reads them from inside the eval, and `compile_direct_eval` says so.
- **What is left of the `v` flag is `\p{RGI_Emoji}` alone — 86 runs, and it is data.** §22.2.1's
  `ClassSetExpression` is built, and so is `\q{abc|def}` — see below; a class can consume a
  sequence now. The property of strings is the other operand that matches more than one code point
  and it needs the UCD's emoji sequence tables, which nothing else in the engine wants. It stays
  **refused by name rather than as bad syntax**, deliberately: it is a legal operand, so calling it
  a syntax error would pass every test asserting a pattern must be rejected.
- **Annex B's regexp grammar is done** — §B.1.2, see the section above and DR-0008's second
  amendment. What is left in `annexB/built-ins/RegExp/` is `legacy-accessors` (48 runs), which is
  the *Legacy RegExp Features* proposal and not §B.1.2 at all, and `prototype/compile` (10 runs),
  which is §B.2.4 and is real. Beside them sit four runs on **a lone surrogate in a pattern**, and
  this file called them "the cheapest real thing left in the area" — **which is wrong, measured
  2026-08-05.** `new RegExp("\\" + lone).source` answers the surrogate back correctly; the two
  tests build their pattern with `eval("/" + text + "/")`, and it is the **lexer** that loses it.
  Broader than RegExp, too: `eval("'" + lone + "'")` answers U+FFFD as well. So it is DR-0004's
  seam in the place that costs most — source text is a Rust `&str` and a lone surrogate is not a
  `char` — and four runs behind an architectural change is the opposite of cheap. **Probe the path
  a test actually uses**: `new RegExp` and a literal are two different front ends here.
- **The resizable-buffer area is done** — see the shrink section below. What remains beside it
  is `subarray` over an out-of-bounds source, which §23.2.5.1 refuses with a RangeError, and the
  files that use the immutable-`ArrayBuffer` harness, which is a proposal.
- **Four things sized and left unbuilt on purpose, so they are not re-costed.**
  - **`String.prototype.normalize` — 20 runs.** Needs the UCD's canonical decompositions, combining
    classes, composition exclusions and compatibility mappings. Eleven of its fourteen files test
    only the error paths, which is the trap: a `normalize` that returned its receiver would pass
    them and be a silently wrong answer for the other three.
  - **`Function.prototype.toString` of anything built from source — ~16 runs.** §20.2.3.5 wants the
    source text and ViperJS answers `function anonymous() { [native code] }` for every function there
    is, dynamic or not. It needs a span and the source retained on every `Chunk`.
  - **`[[IsHTMLDDA]]` — 50 runs.** §B.3.6's three carve-outs (`ToBoolean`, `IsLooselyEqual`,
    `typeof`) are small; what is not small is that the slot belongs to a *host* object, so the
    embedding surface has to be able to make one and `conformance` builds its `$262` by writing
    JavaScript source. **Half of that sentence expired on 2026-08-07**: `$262` is built in Rust now,
    through `api.rs`. What is still true is that a host cannot make an object with a slot the engine
    treats specially — see `api.rs`'s own list of what a host cannot bind, which has two more of the
    same kind on it.
  - **A host-bound *constructor*, and a `Uint8Array` a host can build — 0 runs and two real
    packages.** `Engine::bind` and `bind_namespace` make functions with no `[[Construct]]`, and the
    view constructors are crate-private, so `new TextEncoder()` cannot be offered and neither can
    what it would answer with. `pako` wants both. **`crypto.getRandomValues` cannot be offered by
    this crate at all** — OS entropy needs a dependency (DR-0001) or `unsafe` (DR-0002), and a
    clock-seeded generator under that name is worse than an absent one, because a library that finds
    it missing says so and one that finds a fake generates keys with it. It belongs to the embedder.
  - **`Atomics` is finished** — see the section below. 170 runs came out of it, and the 224 that
    remain are `$262.agent` to the last file (still 224 on 2026-08-08; `createRealm` was a different
    host function and touched none of them): 112 of the 127 failing files name it, and the other
    15 are proposals (the immutable-`ArrayBuffer` harness, `Atomics.pause`). The estimate that
    stood here — "~80 winnable" — was low by more than double, which is what came of costing it
    from the failing paths instead of asking the engine what it had.

- **`super(…)` inside a direct `eval` — 16 runs**, and the whole of what is left of that entry.
  `new.target` in an arrow's direct eval was the other half and is **built**: the fact travels on
  `Chunk::lexical_new_target`, an arrow written inside a function inherits it, and it moved **no
  test at all** — every one of the 16 is the `super()` half, which is refused by name
  (`super outside a derived constructor is not implemented yet`) and is a feature rather than a
  flag. `super.m()` through an arrow already worked.

**63.06% to 75.26% in five slices**, four of them §27.6 and its neighbourhood and the last §23.2's
missing two kinds: async generators themselves (+4,814), `yield*` inside one — §15.5.5 step 4's
`GetIterator(value, async)` (+1,590) — `GeneratorStart` becoming an instruction (+1,598), the
`BigInt` type itself, and `BigInt64Array` (+1,432). The `GeneratorStart` one is worth reading before
touching that area: a generator's parameters are **not** part of its body, so
`FunctionDeclarationInstantiation` runs at the call and only `GeneratorStart` parks what is left.
Deciding it in `enter` instead put the whole parameter list inside the parked body, where it ran at
the first `next` — invisible until a parameter can throw or be observed, and then 1,598 tests at
once, most of them in `dstr` directories that have nothing to do with generators.

**78.20% to 79.15% in two more**, and the second is not a feature. §13.3.10's `import()` is +831 on
its own — the largest single slice since the generators work — and what it cost was not the clause
but three things around it:

- **A module's registry outlives the call that made it.** §16.2.1.6's "each body once" is a fact
  about the execution, so the records moved onto the `Vm`, and the collector's root set with them.
- **A module body writes §14.2.2's completion register.** That is how `run_module_graph` answers,
  and a job runs between statements of a program that is still going — so `typeof import("m")`
  evaluated to whatever `m`'s last statement did until the job put it back.
- **"Has a record" is not "already placed".** A module gets its record before its dependencies are
  walked so a cycle can stop; reading that as "already in the order" ran a module before what it
  imports.

The second slice was the **first ratchet finding that it had been lying**. The mutation-coverage
configuration names the files it reads one at a time, and five engine files — `module.rs`,
`loader.rs`, `namespace.rs`, `dynamic.rs`, `eval.rs` — had never been added to it, so a green score
was being reported over code nothing was mutating. Probed, they scored 71.4%. **Check that list
whenever you add a file**; the audit is nine lines of Python comparing it against `src/`. A reported
survivor can also be **wrong** — one of these killed three tests when the mutation was applied by
hand, its sandbox having been quarantined mid-run. Hand-apply before believing a survivor, which is
the mirror of the rule this file already gives for believing a green.

**77.45% to 78.20% in four more**, and all four are §16.2. What they cost was not the linking — that
is two hundred lines — but three things that touch the rest of the engine:

- **An import is another environment's slot**, so `Heap::variable` and `set_variable` follow an
  alias. The table lives *beside* the environments rather than on them: a field on `Environment` is
  paid by every scope in every program, and a `Vec` there cost `TypedArray/prototype/sort/stability.js`
  its heap outright. §10.4.6's namespaces are beside the objects for the same reason.
- **`import * as n` bound `undefined` in silence** before this. The compiler recorded the entry, the
  linker had nothing to do with it, and a module doc-comment said it was refused. Nothing was
  refused; a wrong value was produced. Grep for what a doc claims is refused before believing it.
- **A namespace's dead zone reaches further than a read.** §10.4.6.5 builds the descriptor out of
  `[[Get]]`, so `Object.keys(ns)` throws inside a cycle — asking whether a name is enumerable
  cannot be answered without its value. That put a ReferenceError into `[[GetOwnProperty]]`, which
  had never been a completion.

**75.26% to 77.45% in seven more**, and the shape of them is worth knowing before picking the
next: two were one architectural piece (a name list on every running scope, then direct `eval`
resolving into it — +973), and the other five were small clauses that had been *hidden by a
refusal or by a crash*. Deleting the direct-eval refusal exposed §15.2.5's named function
expression (+36) and §10.2.4's restricted properties (+109 against 23); fixing a stack overflow
in `JSON.parse` exposed §25.5.1.1's array walk (+2); and §20.1.3.6's last two rows (+26) were
simply missing. **When a slice removes a refusal, budget for the ones behind it** — they are
cheap, they are core, and nothing on the failure-bucket list names them.

**A TypedArray's element is a `Value`, and that is what `BigInt64Array` cost.** Not the two kinds —
those are eight bytes and a sign — but the fact that §23.2.1 gives two of the eleven a
`[[ContentType]]` of BigInt, and a BigInt lives in the heap. So `Heap::element_property` had to
become `&mut self` to allocate one, and `own_property`, `find_own` and `has_property` above it with
it. Three things follow that are worth knowing before touching §23.2 again:

- **The destination chooses the conversion, never the value.** §10.4.5.16 runs `ToBigInt` for the
  two and `ToNumber` for the nine, so `fill`, `with`, `set`, `from`, `of` and `map` all ask the
  array they are writing *into* — which for `map` and `filter` is whatever `@@species` answered, not
  the receiver. §23.2.4.2 step 4 refuses a species of the other content type, and that one check is
  why the copies can move elements without each of them asking again.
- **A read for a copy must not go through a `Value`.** `Heap::numeric_at` is `&self` and allocates
  nothing; `element_at` is the one that makes a BigInt. `slice`, `copyWithin`, `reverse` and `set`
  use the first, and a walk that hands elements to a callback uses the second.
- **A mismatch writes nothing rather than truncating.** `Element::write` answers `None` for a
  BigInt kind and `write_big` `None` for a Number one. A Number reaching a `BigInt64Array` is not a
  value to squeeze into eight bytes — it is a program §7.1.13 should already have refused, and
  writing it would turn a TypeError into a silent conversion.

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

**The garbage collector's root set is settled; its schedule is not — and the reason it is not has
expired.** `Vm::collect` is the host's to call and the interpreter does not run one on a timer. That
was measured rather than deferred: `Heap::footprint` counts arena *slots*, a swept one was **never
reused**, so a collection reclaimed Strings, environments and buffers and could not reclaim what an
object took. Scheduled every eight mebibytes it cost 318 conformance files their time budget to buy
six passes; run once at the budget, 79 files to buy none. This file then said "the next step there
is slot reuse with generation-tagged handles — a decision record, not a patch, and after it the
timer is one line".

**That step is DR-0019, and DR-0023 has now built the schedule on top of it.** The interpreter
collects for itself: one mebibyte of *growth*, the allowance after each collection being the live
set, and **only when no native has been re-entered**. That last clause is the one to remember —
`Array.prototype.sort` holds its elements in a Rust `Vec` across a comparator call, and a collection
underneath it freed every one of them. DR-0011's re-entry counter is what says when it is safe.

`for (i = 0; i < 1e6; i++) s = f(s)` threw a RangeError before this and runs now; so does the same
loop at five million. **That is what the record is for, and not the conformance number** — which
moved by *four* stable runs. A run with the schedule on reports 310 to 476 newly passing and a
different figure each time, because 112 of the 116 that survive a three-run intersection are
`RegExp/property-escapes` sitting exactly on the ten-second budget. Blessing a lucky one put 198
unrepeatable passes into the ratchet before a re-run caught it. **Read that bucket's movement as
noise, never as progress**, and take the intersection of three runs before removing anything from
the expectations file.

What *is* settled is the part that cannot be left half-right. Four whole classes of reference were
untraced before this: a bound function's target and arguments, a revive closure's context, a
compiled chunk's constant table, and a queued job's payload. A collection with any of those missing
frees something a later instruction reads — silently, as a wrong value rather than a crash. The
root set lives in `Vm::roots` and is checked against the collector in `vm::tests::collecting`,
including the one case that distinguishes it: an intrinsic *nothing has reached yet*.

**A compile error is not automatically a skip, and treating it as one hid failures.** §22.2.1's
early errors are decided by the *compiler* — §12.9.5 reads a regular expression literal's shape and
its pattern only afterwards — so `conformance` used to drop every one of them into "not run". 560
runs came out of that column when it was fixed: 366 pass, and **194 fail and could not be seen
before**. The split it rests on is `ErrorKind::BadPattern` against `Unsupported`, and getting it
backwards is not symmetric: a *gap* recorded as an early error passes every test asserting "this
must be rejected", and a proposal's negative tests are exactly that shape. `(?i:…)` is the live
example — see `regexp::Error::unimplemented`.

**Two buckets are not ES2023 and must not be counted as cheap.** `Temporal` — now **8,316 runs and
the single largest failure bucket there is** — is a Stage 3 proposal with a surface larger than
`Date`, `Intl` and `RegExp` combined. Building it would raise the number while making the engine no
more of a JavaScript engine, and it will sit at the top of that list for as long as this file is
worth reading. The 170 runs that stop on `this is not a kind of group` are the same thing in
miniature: they are `built-ins/RegExp/regexp-modifiers` and the `(?i:…)` syntax is Stage 3 as well.
That bucket reads like a cheap 170 in a finished area, which is exactly why it is worth naming here.
**Check a bucket's directory before costing it.**

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
- **A refusal can be holding up passing tests, and removing it turns them red.** Direct eval threw a
  TypeError while it was unimplemented, and five tests asserting `assert.throws(TypeError, …)` were
  passing on it. That is the previous point in its most expensive form: the regression is real on the
  ratchet and is not a regression in the engine, and the only way to tell is to read each one and
  name what it *actually* needs. Those five needed §15.2.5's named-function-expression self-binding
  and a poisoned `caller` — two gaps the refusal had been hiding. **Expect this whenever a slice
  removes a throw**, and say in the commit message which missing feature each newly-listed test now
  genuinely needs.

**One flag answering two questions is the bug this file keeps meeting.** `Compiler::is_script` decided
both where a `var` goes and whether §14.2.2's completion value is kept; the two coincide for a script
and for a function body and come apart for a direct `eval`, which is Script code inside a function.
`at_global_scope` was `outer.is_empty()`, which meant both "the var scope is the global object" and
"no scope has been opened here". `[[GeneratorState]]` above is the same shape. When a slice arrives
that is the first thing to distinguish two meanings of one field, splitting it is the change — not a
special case at the call site that happens to know which meaning it wants.

The local loop is `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
**and** `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`, each by name. **The gate covers
none of the three** — this said it covered the first two and not the third, and that was measured
false on 2026-08-07: it checks no-unsafe, the boundary, the decision records, the architectural
constraints and the fail-open lint, and runs `cargo` for none of them. A public item's doc linking
to a private one is an error in CI and nowhere else, and so, it turns out, is an unformatted line.

**`--workspace` on the clippy line is load-bearing, and this file said otherwise until it cost a red
build.** Without it clippy checks the engine alone, so nothing in `lab/` or `conformance/` is linted
locally while CI lints all three — and a `println!` in a lab experiment took the public build red
after passing every check here. `cargo test` has the same split: CI runs `--workspace`, and
`cargo test --lib` is the engine's tests only. Run the workspace form before publishing, not the
short one.

Read [`GOAL.md`](GOAL.md) first — it is binding and it outranks this file — then `src/span.rs` to
calibrate on the bar. `cargo run --release --example parse -- --commonjs <dir>` over a real
repository is the fastest way to find something worth fixing; `examples/evaluate.rs` runs code.
