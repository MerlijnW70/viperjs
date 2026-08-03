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
and so are **async generators**, §27.6, in `src/vm/async_generator.rs`. **`BigInt` is in**: the
literal, the arithmetic, the object, and now `BigInt64Array` and `BigUint64Array`.

**And `eval` runs, both ways.** §19.2.1.1's indirect mode was already there; the **direct** mode
resolves into the scopes its caller is *running* in — see DR-0018, `src/vm/eval.rs` and
`compile_direct_eval`. That is what the environments' name lists are for, and it was worth 973 runs.

**`with` runs too, and so do modules — including `import()`.** §16.2 is whole apart from a top-level
`await`: `import` and `export` in every form, §16.2.1.5.2's live bindings, §10.4.6's namespace objects,
§16.2.1.6.3's `ResolveExport` across a graph, `export *` with its ambiguity rule, and §13.3.10's
dynamic `import()`. `src/vm/module.rs` is the linker, `src/heap/namespace.rs` the exotic object and
`src/vm/loader.rs` the host hook.

Conformance as of this commit is **79.38% of test262** — 73,949 of 93,161 runs. Treat that number as
perishable and re-measure rather than quoting it; the point of the figure is the work list under it.
Only 362 runs are now *stopped* before anything executes:

| Runs | What stops them |
| --- | --- |
| 170 | `(?i:…)` — the RegExp **modifiers proposal**, and not ES2023; see below |
| 110 | a property of strings |
| 18 | a destructuring rest parameter |
| 18 | `$262.agent`, which a one-thread engine has no answer for |

**The skip list is no longer where the work is, and neither are the biggest failure buckets.**
Sorted by reason the largest look actionable and mostly are not, which is worth doing once and
writing down rather than re-deriving. Bucketed by *path*, what they actually are:

| Runs | Reason | What it really is |
| --- | --- | --- |
| 8,316 | `Temporal is not defined` | a proposal — see below |
| ~960 | `what was called is not a function` | **mostly proposals**: `Array.fromAsync` 128, `Iterator.zip`/`zipKeyed`/`concat` 160, `Promise.allKeyed`/`allSettledKeyed` 92, `Map`/`WeakMap`'s `getOrInsert`, `Uint8Array` base64. The real remainder is `class` 45 and `DataView` 52 |
| 894 | the heap budget | 878 are `RegExp/property-escapes`, and the lab has **parked** them — see below |
| 471 | `a declaration may not stand where only a statement may` | Annex B syntactic — excluded by DR-0008, and see below |
| 454 | `cannot read a property…` | **Atomics 224** (of which most need `$262.agent`, so ~80 are winnable in a one-thread engine) and **`Error.prototype.stack` 64**, which is a proposal |
| 293 | `expected 'meta', found an identifier` | `import.defer` and `import.source` — two separate proposals, not `import.meta` |
| 216 | `expected ';', found an identifier` | `using` / `await using` — explicit resource management, a proposal |
| 238 | `Calling as constructor…` | all `Temporal` |

**Two buckets have been costed and must not be re-costed.**

- **`RegExp/property-escapes` (878) is dead as a GC target**, and the recorded claim that it was
  blocked on DR-0010 slot reuse is **wrong**. `lab/NOTES.md`'s `gc-pressure` measured it: even with
  a zero-cost collector and simulated slot reuse, `ASCII.js` takes 21.8 s against a 10 s per-test
  budget. These need an interpreter several times faster, which is M8. The experiment's two real
  findings — a throwaway heap String per computed property key, and a timed-out run landing in no
  column — are both **fixed**; do not go looking for them again.
- **Annex B block-level function declarations are 645 runs and are behind DR-0008**, which refuses
  B.3 deliberately and names the procedure for reversing itself ("the place to change it is here,
  and B.3 would then arrive behind a host flag"). ~480 of the 645 are the `if (x) function f() {}`
  shape, which is B.3's *syntactic* half — so this is a charter decision and not an
  implementation choice. **It is also the only remaining path to 80%**: every other item on this
  page together lands at about 79.5%.

**The skip list is all but empty, and 80% now turns on one decision.** §16.2 is finished; what is
left stopped is a proposal (`(?i:…)` 170, a property of strings 110), a one-thread engine's limit
(`$262.agent` 18), `import.meta` 6, and a destructuring rest parameter 18. Building every one of
those lands short of 80%. **The 645 Annex B runs below are what is between here and it.**

### Annex B §B.3 is now DECIDED and NOT BUILT — start here

DR-0008 was **reversed on 2026-08-03**: B.3 is implemented, conditioned on strictness alone and
**not** behind a host flag. Read the amendment at the foot of that record for why the flag was
dropped — strictness is static, and B.1's legacy octal already sets the precedent. 645 runs, and the
only path to 80%.

Three pieces, and the third is the one with the rules in it:

1. **§B.3.4 — `if (x) function f() {}`.** In `parse_if_branches`, when the code is sloppy and the
   branch begins with `function`, parse the declaration and wrap it in a `StmtKind::Block`. That is
   the specification's own framing ("evaluated as if it were `if (Expression) { FunctionDeclaration
   }`"), and wrapping means B.3.3 below applies to it with nothing extra. A **generator** is not a
   `FunctionDeclaration` and must still be refused, so this needs the one-token lookahead past
   `function` that `at_async_function` already shows how to do. `async function` is refused before
   this point and stays refused.
2. **§B.3.2 — `L: function f() {}`.** Same test in `parse_labelled_statement`, and **not** wrapped:
   §8.2.12 hands a `LabelledStatement` to `TopLevelVarDeclaredNames`, which `collect_var` already
   does by passing `direct` through, so a labelled function at a top level is var-scoped already.
3. **§B.3.3 — the extra `var` binding**, which is the semantics and the work. At each var scope, for
   every function declaration in a *nested* block, if replacing it with `var F` would raise no early
   error — no `let`/`const`/`class`/catch-parameter/lexical-`for`-head named F anywhere between the
   block and the var scope, and F not a parameter name — then create a var binding F initialised to
   `undefined`, and **at the point the declaration is evaluated** assign the block's binding to it.

**A first attempt got to 79.62% (+226, zero regressions) and was reverted.** It is worth reading
before the second, because three of the four things it found are not in the clause:

- **§B.3.2's labelled function needs a *position* rule, not just strictness.** §14.6.1, §14.7.x and
  §14.11.1 make `IsLabelledFunction(Statement)` a Syntax Error wherever a body is a `Statement`
  rather than a `StatementList` — so `while (x) a: function f() {}` stays refused while
  `{ a: function f() {} }` is taken. Without it, 18 `labelled-fn-stmt.js` tests regress. A flag set
  by the statement-list item parser and consumed by `parse_statement` carries it, with a label
  re-arming it for its own body so `a: b: function f() {}` inherits.
- **A generator is not a `FunctionDeclaration`** in either position, so both parse points need the
  one-token lookahead past `function`.
- **The name may already be a `var` or a top-level function**, and then B.3.3 step 2 must *not*
  create a second binding — "if instantiatedVarNames does not contain F". A separate slot makes
  reads resolve to one binding and the copy-out write to the other, which is what the 32
  `existing-fn-update` tests see. This is the piece the reverted attempt got wrong.
- **The global path did not store.** `{ function f() {} } typeof f` answered `undefined` at a
  *script's* top level while the same program inside a function worked. Diagnose that before
  anything else: the likely culprits are whether `global_vars` is set before the B.3.3 declaration
  runs, and whether a block at a script's top level pushes a scope `own_depth` counts.

Two things about (3) that the tests turn on:

- The assignment happens **whether or not** step 2 created the binding: `var f = 1` at the top and
  `{ function f() {} }` below leaves `f` as the function. `existing-var-update.js` is that row.
- Writing to the var binding from inside the block cannot go through ordinary name resolution — the
  block's own binding shadows it. The store needs the var scope's slot directly, which is
  `Compiler::own_depth` hops out (or `StoreGlobal` at a script's top level, since §16.1.7 makes that
  a property of the global object).

`test/annexB/language/{eval,function,global}-code/` is the whole of it, and each file's `info:`
frontmatter quotes the numbered steps. Read `func-skip-early-err.js` first: it is the condition in
(3) stated as a program.

### What is left after that, in the order the numbers put it

- **`import.meta` — 6 runs.** §16.2.1.9's host hook, and the registry it would hang off is built. It
  is also the **only compiler refusal left in the engine**: nothing a *script* can say is refused any
  more, which is why the four tests that assert a refusal by name now compile modules.
- **§13.15.2's order inside a `with` — 10 runs**, not the 79 an earlier note claimed. An assignment
  evaluates its target *reference* before the value; inside a `with` praxis evaluates the value
  first, which shows up as a proxy seeing one `has` where the specification asks for two.
- **`new.target` and `super(…)` inside a direct `eval` in an arrow — 16 runs.**

### Two small gaps are diagnosed and not built

Both are under a hundred runs, and each is written down because the diagnosis cost more than the
fix will.

- **`new.target` and `super(…)` inside a direct `eval` in an arrow — 16 runs.** Both are refused
  where §19.2.1.1 allows them, because whether an arrow was written inside a function is a
  *lexical* fact that a running arrow's chunk does not record — and the parser knows it, refusing
  `new.target` in a top-level arrow at compile time. Carrying that answer onto the chunk is the
  whole slice.
- **§13.15.2's order inside a `with` — 79 runs, all listed.** An assignment evaluates its target
  *reference* before the value, and inside a `with` praxis evaluates the value first. Observable
  when the right-hand side changes what the left resolves to.

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

The local loop is `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` **and**
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`, each by name. The gate does not cover
the third: a public item's doc linking to a private one is an error in CI and nowhere else.

Read [`GOAL.md`](GOAL.md) first — it is binding and it outranks this file — then `src/span.rs` to
calibrate on the bar. `cargo run --release --example parse -- --commonjs <dir>` over a real
repository is the fastest way to find something worth fixing; `examples/evaluate.rs` runs code.
