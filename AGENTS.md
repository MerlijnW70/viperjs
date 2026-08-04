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

Conformance as of this commit is **82.38% of test262** — 76,745 of 93,161 runs. Treat that number as
perishable and re-measure rather than quoting it; the point of the figure is the work list under it.
Only 422 runs are now *stopped* before anything executes, and none of them is worth building:
`(?i:…)` 170 and a property of strings 110 are the RegExp **modifiers** and **strings** proposals,
`$262.agent` 18 is a one-thread engine's limit, and the rest is `import.meta` 6, `super` in an
arrow's direct `eval` 16, and two dozen module-beside-the-test parse failures that are proposals.

**The failure buckets are the whole work list now.** Sorted by reason the largest look actionable
and mostly are not, which is worth doing once and writing down rather than re-deriving:

| Runs | Reason | What it really is |
| --- | --- | --- |
| 8,316 | `Temporal is not defined` | a proposal — see below |
| ~939 | `what was called is not a function` | **mostly proposals**: `Array.fromAsync`, `Iterator.zip`/`zipKeyed`/`concat`, `Promise.allKeyed`/`allSettledKeyed`, `Map`/`WeakMap`'s `getOrInsert`, `Uint8Array` base64, `DataView`'s `getFloat16` |
| 846 | the heap budget | almost all are `RegExp/property-escapes`, and the lab has **parked** them — see below. The count shuffles between this row and the ten-second budget from run to run; it is one bucket wearing two names |
| 454 | `cannot read a property…` | **Atomics 316** (most needing `$262.agent`, so ~80 are winnable in a one-thread engine) and `Error.prototype.stack` 64, a proposal |
| 293 | `expected 'meta', found an identifier` | `import.defer` and `import.source` — two proposals, not `import.meta` |
| 238 | `Calling as constructor…` | all `Temporal` |
| 224 | `expected ';', found an identifier` | `using` / `await using` — explicit resource management, a proposal |
| 176 + 142 | `DisposableStack`, `AsyncDisposableStack` | the same proposal's library half |

**Two buckets have been costed and must not be re-costed.**

- **`RegExp/property-escapes` (878) is dead as a GC target**, and the recorded claim that it was
  blocked on DR-0010 slot reuse is **wrong**. `lab/NOTES.md`'s `gc-pressure` measured it: even with
  a zero-cost collector and simulated slot reuse, `ASCII.js` takes 21.8 s against a 10 s per-test
  budget. These need an interpreter several times faster, which is M8. The experiment's two real
  findings — a throwaway heap String per computed property key, and a timed-out run landing in no
  column — are both **fixed**; do not go looking for them again.
- **`Temporal` is a Stage 3 proposal with a surface larger than `Date`, `Intl` and `RegExp`
  combined.** Building it would raise the number while making the engine no more of a JavaScript
  engine, and it will sit at the top of that list for as long as this file is worth reading.

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
flag's `.indices` array, which praxis does not build at all. That is a next slice with its own
tests already listed against it.

### One `?`, and 109 runs: §15.8.4 rejects where the generator clauses throw

Three clauses evaluate a body, and they differ in one character. §15.5.4 and §15.6.5 — a generator
and an async generator — both begin `Perform ? FunctionDeclarationInstantiation(…)`, so a throw from
a parameter default reaches the **caller**. §15.8.4, a plain `async` function, runs the same
instantiation as a **Completion** and step 3 hands an abrupt one to the promise's `reject`. So
`async function f(x = x) {}` answers with a rejected promise where `function f(x = x) {}` throws at
the call, and `async function* g(x = x) {}` throws too.

praxis put the rejecting handler *below* the parameter prologue on the reading that "a throw from a
parameter default is the caller's to catch" — which is right for two of the three clauses and was
written in a comment as though it were a rule. Moving it above the prologue for a non-generator
`async` function is the whole change; the async generator keeps its handler where it was, and that
is the row that stops the fix being applied to every async body there is.

**The bucket undercounted it by three to one.** 36 runs wore the dead zone's reason string
(`a let or const was read before its declaration ran`); the other 73 were every other way a
parameter list can throw — a pattern against `null`, a default that calls something — each filed
under a reason of its own. **A clause about where a completion *goes* will never bucket cleanly,
because the bucket is keyed on what produced it.**

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

None of them was a feature. Every one was a clause praxis had *nearly* right, and fifteen of the
sixteen were found by bucketing the failures rather than by reading a list of what is missing.

**Three of the twelve are one shape**, and it is the one worth carrying: *a clause names a
completion, a flag or a step that praxis collapsed into one path serving two callers.* §9.1.1.4.17's
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
    name. The instruction's own comment said praxis "does not yet carry a strictness through to
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
  block. §14.12.4 step 3 hands the whole `CaseBlock` to `BlockDeclarationInstantiation`; praxis
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

- **A sloppy `var` or function declaration inside a direct `eval` in a function — 128 runs**, and
  the largest thing praxis refuses by name. **Attempted 2026-08-04 and reverted: 0 fixed, 18
  regressed, net −103 runs.** Do not re-derive the design — it was built, it works by hand, and it
  is not the blocker. §19.2.1.1's `varEnv` was recorded on the frame and each var-declared name
  sorted by comparing its resolved depth against it; `function f(){ var x = 1; eval("var x = 2");
  return x }` gave 2 and `{ let y; eval("var y") }` was a SyntaxError, both correctly.
  **What it lost on is that praxis puts a function's parameters, its `arguments`, its `var`s *and*
  its body's `let`s in one environment, where §10.2.11 has up to three.** A depth comparison cannot
  then tell "bound in the variable environment" (nothing to create) from "bound in a scope between
  here and it" (§19.2.1.3 step 5.d.ii.1's SyntaxError) — both are the same number, and the
  `declare-arguments` family is exactly that distinction. **So the prerequisite is §10.2.11's
  environment split, and it is a bigger slice than the one it unblocks.** The growth case — a name
  bound nowhere, needing a slot in a frame sized at compile time — was never in scope and is still
  open. §B.3.3's own bindings in such an eval sidestep all of it: they go in the eval's own scope,
  because every test reads them from inside the eval, and `compile_direct_eval` says so.
- **A class that matches *strings* — 78 runs, and what is left of the `v` flag.** §22.2.1's
  `ClassSetExpression` is built: `[[a-z]--[aeiou]]`, `[\d&&[0-4]]` and nesting all work, and the
  three operations turned out to be three quantifiers over the operands rather than sets to build.
  What remains is the two operands that match more than one code point — `\q{abc|def}` and
  `\p{RGI_Emoji}` — and **both are refused by name rather than as bad syntax**, deliberately: they
  are legal, so calling them a syntax error would pass every test asserting a pattern must be
  rejected. Building them is a matcher change (a class stops being a code-point predicate), not a
  parser one.
- **A coercion can detach or resize the buffer under a TypedArray method — what is left of ~58.**
  Every one coerces an argument and then works from a length read *before* it, where the clause
  says to look again. **The rules differ per method and that is the whole cost:** `copyWithin`,
  `fill` and `slice` **throw**, while `includes`, `indexOf` and `lastIndexOf` set the length to
  zero and answer `-1`/`false`, and `subarray` goes on to build a view that §23.2.5.1 then refuses
  with a *RangeError*. Several of these files also use the immutable-`ArrayBuffer` harness, so
  check what a row is really asking before counting it — and re-measure, because §10.4.5.2's
  offset fix took 60 runs out of this area already.
- **§13.15.2's order inside a `with` — 49 runs.** An assignment resolves its target *reference*
  before reading the value, and a compound one writes back through the **same** reference. praxis
  resolves the name twice, so a getter that deletes the property between them writes to a different
  binding. `src/vm/dynamic.rs` already has `Resolved`, which is §9.4.2's Reference; what is missing
  is a way to keep one across two instructions, and the right-hand side can throw between them —
  so it needs the unwind discipline the operand stack has, not a field.
- **`ArraySetLength` cannot run a `valueOf` — ~30 runs.** `[].length = {valueOf(){return 3}}` throws
  and `[].length = 1n` is a RangeError where §10.4.2.4 propagates `ToUint32`'s TypeError.
  `set_array_length` is on `Heap`, which has no interpreter to re-enter — that is DR-0011's seam.
- **A computed key does not name its method — 36 runs.** §15.4.5 runs `SetFunctionName(closure,
  propKey)` with the *evaluated* key, so `({ ["id"]() {} }).id.name` is `"id"` and a Symbol key
  gives `"[description]"`; praxis answers `""` for both, and the accessors are missing their
  `get `/`set ` prefix with it. Most of the 36 are `language/expressions/object`, not classes.
  `src/compile/class.rs`'s `Naming` decides it at compile time, where the key is not yet known — so
  the slice is naming at run time from the key already on the stack. **Found by grepping doc
  comments, not by bucketing**: the comment there described the empty string as a choice.
- **The `d` flag's `.indices` array — 34 runs.** §22.2.7.8 `MakeMatchIndicesIndexPairArray`, and
  praxis builds none of it: the flag parses and `RegExp.prototype.hasIndices` answers, so a script
  can ask for `d` and then read `undefined`. The match record already holds every span the array
  needs — `found.span` and `found.captures` are pairs — so this is the array and its `groups`
  object and nothing else. Six of the 34 arrived with the duplicate-named-groups slice.
- **`import.meta` — 6 runs**, §16.2.1.9's host hook, and the registry it would hang off is built.
- **`new.target` and `super(…)` inside a direct `eval` in an arrow — 16 runs.** Whether an arrow was
  written inside a function is a *lexical* fact a running arrow's chunk does not record, and the
  parser knows it. Carrying that answer onto the chunk is the whole slice.

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
**and** `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`, each by name. The gate does not
cover the third: a public item's doc linking to a private one is an error in CI and nowhere else.

**`--workspace` on the clippy line is load-bearing, and this file said otherwise until it cost a red
build.** Without it clippy checks the engine alone, so nothing in `lab/` or `conformance/` is linted
locally while CI lints all three — and a `println!` in a lab experiment took the public build red
after passing every check here. `cargo test` has the same split: CI runs `--workspace`, and
`cargo test --lib` is the engine's tests only. Run the workspace form before publishing, not the
short one.

Read [`GOAL.md`](GOAL.md) first — it is binding and it outranks this file — then `src/span.rs` to
calibrate on the bar. `cargo run --release --example parse -- --commonjs <dir>` over a real
repository is the fastest way to find something worth fixing; `examples/evaluate.rs` runs code.
