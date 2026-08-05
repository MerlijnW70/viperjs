# Lab notebook

One entry per experiment, newest first. **Write the question before you write the code** — an
experiment with no stated question produces a result you cannot interpret afterwards.

Failed experiments are the most valuable entries here. They are the only thing that stops the
same dead end being re-explored, and a dead end that is re-explored twice costs more than the
feature it was blocking.

Record the verdict here the moment it is known. A result nobody wrote down is a result
that gets re-derived, which is the one cost this notebook exists to prevent.

---

## Template — copy this

```
## <name> — <one-line question>

**Date:** YYYY-MM-DD
**Question:** the specific thing you did not know. Not "explore X" — "is a naive HashMap
property lookup fast enough to stay past M4, or does the shape table have to land first?"
**Setup:** what you built, what you measured, on what input. Enough that the number is
reproducible.
**Result:** the numbers or the observation. Raw, before interpretation.
**Verdict:** PROMOTE (and to which milestone) / PARK (and what would revive it) / DEAD (and why).
**Cost:** roughly how long it took, so the next estimate is better than a guess.
```

---

## somebody-elses-code — on twenty thousand real files, what breaks first?

**Date:** 2026-08-05
**Question:** `run-module` showed three.js links and runs, which is one library chosen for having no
host dependencies. The conformance number says what fraction of *test262* passes and test262 is
written to probe edges, not to resemble a program anybody ships. **Pointed at large repositories
written by people who have never heard of this engine, what fails — and is the first failure a
grammar gap, a library gap, or something else entirely?**
**Setup:** four checkouts, kept outside the working tree. `examples/parse --commonjs` over
`nodejs/node`'s `lib/` and `test/` (sparse checkout), `webpack/webpack` excluding `node_modules`, and
`ramda/ramda`'s ESM `source/`. Then `run-module` over ramda's entry, and a 36-row probe importing
across the library — currying, transducers, `equals` over Map/Set/`-0`/`NaN`, lenses, `sortWith`
comparators, global-flag regex with a replacer function, object algebra, numerics — run **from the
same file** under this engine and under node v22.22.1, and diffed. Finally a compute-bound
benchmark in both, and a bisect of the call-loop ceiling.

**Result — the parser found nothing, and that is the whole of the parser's result.**

| corpus | files | parsed | panicked | |
| --- | --- | --- | --- | --- |
| `node/lib` | 407 | 406 | 0 | 5.2 MB in 0.2 s |
| `node/test` | 8,235 | 8,165 | 0 | 29.2 MB in 1.2 s |
| `webpack` | 11,368 | 11,162 | 0 | 15.3 MB in 0.9 s |
| `ramda/source` | 369 | 369 | 0 | 0.4 MB |

Every one of the 277 failures was triaged and every one is a **correct refusal**: Stage 3 proposals
(`using` 41, `import defer` 26, `import source` 19, `import.source`/`import.defer` 13, the retired
`assert { type: "json" }` 15), JSX 11, webpack's `---` fixture separator which is not JavaScript 22,
deliberately-broken syntax-error fixtures, one file of V8 natives syntax (`%PrepareFunctionFor…`),
and one generated validator over `MAX_NESTING_DEPTH` — the same DR-0006 case three.js's draco blob
hit, at column 156,314 of a machine-written line.

**Two refusals were checked against node rather than against a reading, and both held.**

- `webpack/test/configCases/rebuild/finishModules/other-file.js` has `import foo from "./module"`
  and `export const foo = {…}` in one module. §16.2.1.1 makes that a duplicate lexical declaration.
  Node agrees to the line and column: `SyntaxError: Identifier 'foo' has already been declared`. **A
  real latent bug in webpack's own fixture, found by sweeping it.**
- `node/test/fixtures/utf8-bom-shebang-shebang.js` is a BOM, then two hashbangs. Refusing it looked
  like a BOM-handling bug; node's `test-module-loading.js` asserts a `SyntaxError` there. Right
  answer, reached more strictly — §12.5 wants the hashbang at the very start of the Source Text, and
  node's BOM-stripping is a host convention rather than a grammar rule.

**The runtime found nothing either, which is the stronger half.** Ramda's entry pulls **1,027
modules**; they link and evaluate in **82.7 ms**. The 36-row probe is **byte-identical** to node's
output — `diff` is empty. Deep `equals` over a `Set` inside an array inside an object, `-0` against
`0`, `NaN` against `NaN`, a lens composed three deep, and a global regex driven by a replacer
function all agree.

**What broke first was not a feature. It was the arena ceiling, on a two-line program.**

```
for (var i = 0; i < N; i++) { s = f(s) }

N = 100,000   ok        N = 500,000   ok        N = 800,000   ok
N = 1,000,000 RangeError: the heap has grown past what this engine will allocate
```

DR-0019's note predicted "about 900,000 calls before any program dies" from 74 B of arena per call.
The real boundary sits between 800,000 and 1,000,000, so the prediction was good to the digit that
matters — and the thing it predicts is now an ordinary loop rather than a row in this notebook. The
RangeError is catchable, and catching it is awkward for a reason worth knowing: **reporting the error
allocates**, so a handler that concatenates `e.message` can throw again on the way out.

**Speed, framed honestly, because one of the two numbers flatters us.** Linking and running the
1,028-module graph: **86–94 ms here against node's 101–107 ms** — and that comparison means very
little, being dominated by reading, parsing and compiling a thousand files rather than by executing
anything. Compute-bound is the real picture:

| | this engine | node v22 |
| --- | --- | --- |
| `arith-1e6` | 201 ms | 3 ms |
| `propget-1e6` | 269 ms | 1 ms |
| `array-1e5` (push then sum) | 318 ms | 2 ms |
| `string-1e5` | 82 ms | 2 ms |

An unoptimised interpreter against a JIT that can also prove most of these loops dead. The ratio is
not a finding; **that the loops complete at all and agree on every answer is.**

**Verdict: PROMOTE the collection schedule to the next M8 slice, and it is a decision record.** The
argument was abstract this morning — this run makes it concrete. Slot reuse landed with DR-0019 and
was re-measured today across all five arenas, so a collection now reclaims what a call took; nothing
schedules one, so `f` called a million times still dies. That is the last thing between this engine
and running ordinary code of ordinary size, and it is one condition in `Vm::execute` beside the
budget check that already exists.

Nothing else here is actionable. **PARK** the nesting limit — two machine-generated files in twenty
thousand, and DR-0006 wants `nesting-cost`'s stack figure before the number moves.

**Cost:** about an hour, most of it cloning and triaging buckets rather than diagnosing. The sweep
itself is seconds. Worth repeating after any parser slice, and worth repeating with a *different*
library after any runtime slice — the differential against node is four lines of shell and is the
only check here that can catch a silently wrong answer.

---

## direct-eval-var — is the 128-run refusal really three cases, one of them free?

**Date:** 2026-08-04
**Question:** `a sloppy var inside a direct eval in a function` is the largest thing praxis refuses
by name. A recorded plan said most of it needs **no** growable environment: a name the caller's
variable scope already has needs no slot, and a name shadowed by a lexical binding between
`lexEnv` and `varEnv` is a SyntaxError §19.2.1.3 owes anyway. **Is that split real?**
**Setup:** built it. `Vm::var_environment` — set when a call opens its environment, saved and
restored on `Frame` beside `environment`, untouched by `PushScope`/`PopScope`, because nothing in
a flat chain says which level is the variable one. Its depth goes to the compiler on
`EvalVars::Caller`, and each var-declared name is sorted by comparing its resolved depth against
it: shallower is a SyntaxError, equal is "already there", deeper or absent stays refused.

**Result: reverted. 0 fixed, 18 regressed, net -103 runs.**

The machinery works — `function f(){ var x = 1; eval("var x = 2"); return x }` answers 2 where it
threw before, a bare `var x` leaves the slot alone, and `{ let y; eval("var y") }` is a
SyntaxError. What it does **not** do is win a single test, and it breaks the `declare-arguments`
family outright.

**Why, and it is the part worth keeping:** praxis puts a function's parameters, its `arguments`,
its `var`s *and* its body's `let`s in **one environment**, where §10.2.11 has up to three. So a
depth comparison cannot tell "bound in the variable environment" from "bound in a scope between
here and it" — they are the same number. The `declare-arguments` tests are exactly that
distinction: they expect a SyntaxError because `arguments` is bound in a scope the walk passes
*through*, and with one environment the check reads it as the destination and accepts.

So the split is real in the specification and not expressible against praxis's environments. **The
prerequisite is §10.2.11's environment split — a parameter scope separate from the variable scope
when the parameter list has expressions — and that is a bigger slice than the one it unblocks.**

**Cost:** an hour, and the ratchet earned its keep: every hand-written probe answered correctly and
the suite still said no. A change that is right about the cases you thought of and wrong about the
ones you did not is exactly what a conformance number is for.


## run-module — does the engine run somebody else's library?

**Date:** 2026-08-04
**Question:** `examples/parse` sweeps a repository and answers "does it parse", which is the front
end only. §16.2's linker, the namespace objects and the live bindings are runtime, and had never
been pointed at a graph written by somebody who had never heard of praxis. **Does a real library
link and run, and what breaks first?**
**Setup:** `cargo run -p praxis-lab --release -- run-module <entry.js>`, against a three.js
checkout at 1,640 files and 736,000 lines. The host walks each chunk's `imports()`, reads and
compiles what they name, and hands the whole graph to `Vm::run_module_graph`.

**Result: it runs, and the numbers are right.**

The parse sweep first: **1,640 of 1,642 files parsed, 0 panicked, 21.4 MB in 1.1 s.** The two
failures are the same emscripten blob (`draco_decoder.js`) and are `MAX_NESTING_DEPTH`, not a
grammar gap — brace nesting there peaks at 38 against a limit of 64, so what it exceeds is
*expression* depth. It parses at a limit of 128. Raising it is DR-0006's business and wants
`nesting-cost`'s stack figure first, so the number is recorded and nothing was changed.

Then the graphs. Ten of three.js's math modules link and evaluate — `Frustum` pulls twelve
modules — and a probe that imports four of them computes correctly through both an interpreter
path and an independent one:

| | |
| --- | --- |
| `a.dot(b)` | 32 |
| `a.cross(b)` | -3, 6, -3 |
| `a.length()` | 3.741657 |
| `Matrix4.makeRotationY(π/2)` on +X | 0, 0, -1 |
| the same rotation as a `Quaternion` | 0, 0, -1 |
| `[...new Vector3(7,8,9)]` | 7, 8, 9 |

Twelve linked ES modules, 816 us. The matrix and quaternion routes agreeing is the row worth
having: they are different code in three.js and different instructions here.

**The finding is about the embedding surface, and it is real.** `Vm::run_module_graph` looks a
specifier up **exactly as written** — `self.resolved.get(&entry.specifier)` — and the
`ModuleLoader` hook is consulted only for §13.3.10's dynamic `import()`, never at link time. So a
host cannot supply a graph in which two directories both say `./MathUtils.js` meaning different
files; there is nowhere to put the resolution. three.js's math tree happens to be one directory
and survives, and anything larger will not. The experiment reports a clash rather than letting the
second file silently replace the first, which is what makes the limit visible instead of wrong.

**And an hour went into an engine bug that was not one.** The first version called
`Vm::run_module` — which runs *one* module chunk — and every imported binding read `undefined`.
`typeof fn` answered `"undefined"`, `fn()` threw "what was called is not a function", and it
reduced to a two-file repro that looked exactly like a broken linker. It was the wrong entry
point: linking is `run_module_graph` and it is handed every chunk up front. **The engine's own
`run_graph` test helper says so in four lines and reading it first would have saved the hour.**

**Fixed the same day — DR-0020.** The loader is handed the referrer and answers with a key, and
the experiment now supplies only the entry and lets the engine pull the rest: three.js's math
graph comes to **17 modules** rather than the 12 the hand-walked version found, because
`../utils.js` and `../constants.js` resolve as their own modules instead of colliding. The
computations are unchanged. A two-directory clash — `a/index.js` and `b/index.js` both importing
`./thing.js` — now answers `a says a, b says b`, which was not expressible before.

**Cost:** two hours. `Vm::to_string` being `pub(crate)` is worth knowing too — a thrown Error
cannot be printed from outside the crate, and `examples/evaluate.rs` settles for `[object]`. The
runner reads `name` and `message` off the object instead, which is the whole diagnosis and needs
no new API.


## hot-shapes — is the interpreter's ceiling speed, or is it something else?

**Date:** 2026-08-03
**Question:** "Optimise the bytecode compiler" and "refine the GC" were both proposed as the next
milestone, and `gc-pressure` had already left two numbers pointing at the interpreter without
chasing them: `for (let i …)` costing 4.2 us/iteration over `for (var i …)`, and `a[0] = i`
costing 1.9 us and 17 MiB/million. **Which of the two proposals do those numbers actually
support?**
**Setup:** `cargo run -p praxis-lab --release -- hot-shapes`. Twelve source shapes, 100,000
passes each, chosen so that every interesting quantity is a *difference between two rows that
differ in one thing*. Each row reports time per pass, arena retained per pass, and — the column
that settles it — arena retained per pass **after a full collection**.

**Result:** neither proposal is the fix, and the second is refuted outright.

| shape | ns/pass | retained | after gc |
| --- | --- | --- | --- |
| `fn-empty-var` | 156 | 2 B | 2 B |
| `fn-empty-let` | 213 | **74 B** | **74 B** |
| `empty-var` (script level) | 353 | 2 B | 2 B |
| `empty-let-captured` | out of heap | **671 B** | **671 B** |
| `named-store` | 549 | 2 B | 2 B |
| `element-store` | 814 | 2 B | 2 B |
| `element-store-growing` | 1777 | 28 B | 28 B |
| `call` | 677 | **74 B** | **74 B** |

**The after-gc column is identical to the retained column in every row.** A full collection
reclaims *nothing*. What these shapes retain is not garbage a schedule would catch — it is arena
slots DR-0010 declines to reuse, and no collector and no schedule can give them back. That is the
GC proposal answered for the second time and by a different route than `gc-pressure` took.

**The binding constraint is a ceiling, not a speed.** A function call retains 74 B of
unreclaimable arena. Against DR-0013's 64 MiB budget that is **about 900,000 calls before any
program dies**, whatever it does with the results — and a `for (let …)` whose body closes over the
binding retains 671 B/pass, so it dies at about 100,000. Four of the twelve shapes cannot run a
million passes at all. This limits every praxis program, not the `property-escapes` tests
specially, and it is why that bucket is the shape it is.

**Two corrections to this notebook's own record.** `gc-pressure`'s "`for (let i …)` vs
`for (var i …)`: 4.79 s against 0.63 s, 4.2 us/iteration" is a **wrong attribution**. Measured
inside a function, where both are slots, the per-iteration environment costs **57 ns** — two
orders of magnitude less. Most of the old gap was that a script-level `var` is a *global-object
property*: the same empty loop is 353 ns/pass at script level and 156 ns/pass inside a function,
a 197 ns penalty per access that has nothing to do with `let`. Any future measurement of scope
cost must run inside a function or it measures the global object instead.

**What the compiler proposal is worth, exactly.** Eliding §14.7.4.7's per-iteration copy when
nothing captures the binding saves 57 ns and 74 B per iteration. It is real and it is worth doing;
it is not the fix. It cannot touch `call`'s 74 B, and it must **not** be applied to the capturing
case — that is the 671 B row, and the copy is what makes it correct.

**The one honest allocation finding** is `element-store-growing`: 28 B/pass and 963 ns over a
fixed index. That is the array's backing growth, not a per-store object, and it is the only row
where the memory is doing work.

**Verdict: PARKED as an optimisation question; ESCALATED as a limit.** Neither "optimise the
compiler" nor "improve the GC" is the next thing. **DR-0010 slot reuse with generation-tagged
handles** is, and AGENTS.md already calls it "a decision record, not a patch". This experiment is
the number that record was missing: without it, praxis cannot call a function a million times.

**Cost:** about an hour. Most of it was one harness bug worth repeating — `Outcome::Thrown` is
`Ok`, so the first run reported the four shapes that *exhausted the heap* as the four fastest.
A benchmark that does not check what it measured will report a crash as a speed-up.

**Implemented the same day — DR-0019, environments only.** A free list per arena plus a generation
in the handle's spare 32 bits. The reuse check this experiment gained afterwards is the number:
100,000 calls, collect, 100,000 calls again — the first run grows the arena by **7,458,300 B** and
the second by **816 B**. Nine thousandfold, and the ceiling this entry was written about is gone
for environments.

Two things the table could *not* show, and it took a run each to see why:

- **A collection at the end of a loop moves nothing**, so the "after gc" column stayed at 74 B
  even once reuse worked. `footprint` counts `environments.len()`, and freeing a slot does not
  shorten the `Vec` — it makes the slot available. Only a **second** loop distinguishes reuse from
  tombstones, which is what `reuse_check` runs.
- **Conformance did not move at all**: 76,456 passing before and after, byte for byte. Right, and
  worth stating — `Vm::collect` is the host's to call and the interpreter runs none on a timer, so
  reuse changes what a program *can* do rather than what any current test does. The objects,
  strings, symbols and BigInt arenas are still tombstoned; they are the same change and are next.

### Re-run 2026-08-05 — the rollout finished, and the table's headline is a property of the measure

**Question:** the entry above says the other four arenas "are next". Are they still tombstoned, and
does the 74 B-per-call ceiling still stand?

**Result: all of them reuse, and the ceiling is conditional rather than absolute.** `reuse_check`
grew a row per arena, each running its loop twice with a collection between:

| arena | run 1 kept | run 2 kept | |
| --- | --- | --- | --- |
| environments (a call) | 7,462,922 B | 816 B | REUSED |
| objects (a literal) | 41,062,106 B | 0 B | REUSED |
| strings (a concatenation) | 3,462,118 B | 0 B | REUSED |
| bigints (an addition) | 262,078 B | 0 B | REUSED |

`src/heap/mod.rs` has all five fields as one `Arena<T>` and `collect.rs` sweeps every one through
it, so this confirms a reading rather than discovering anything — but the reading was three commits
old and the entry above still said otherwise, which is why it was worth the run.

**What has *not* changed is the timing table, and the two must not be confused.** `call` is still
705 ns and 74 B a pass; `empty-let-captured` still reaches DR-0013's budget and stops. Those rows
allocate and never collect, because nothing collects unless a host asks. So **"about 900,000 calls
before any program dies" is still true of a program that never calls `Vm::collect`, and false of one
that does.** DR-0019 turned an absolute ceiling into a schedule question, and the schedule is
genuinely still open — which is now the *only* thing standing between this notebook and M8.

**The `after gc` column cannot answer any of this, and that is the correction worth carrying.** It
is identical to the `leak/pass` column in every row and always will be: `footprint` is a high-water
mark, so a freed slot goes on being counted whether or not the next allocation takes it. Read as
"what a collection cannot reclaim" it produced this entry's original headline — *a full collection
reclaims nothing* — which was a statement about the measure and not about the collector. Only a
**second loop** distinguishes reuse from tombstones.

**And the first version of the per-arena check got Strings wrong**, which is worth recording because
the failure was subtle and the number looked decisive. Measuring "growth in run 1 vs growth in run 2"
reported Strings as `tombstoned — paid again`: 5,617,700 B then 2,155,560 B. Both numbers were real
and the verdict was wrong, because `footprint` is slots **plus** `string_units`, and a String's units
are memory the sweep genuinely returns and the next allocation genuinely buys again. That component
is correct behaviour and says nothing about the slot. Taking both readings **after a collection**
removes what legitimately comes and goes, and Strings then read 3,462,118 B / 0 B — reuse, and the
arithmetic of the two versions agrees to twenty-two bytes. **A mixed-unit metric will hand you a
confident wrong verdict for whichever term dominates.**


## name-resolution — what would it cost to resolve every name at run time?

**Date:** 2026-08-03
**Question:** §14.11 forced a second way to reach a variable — a walk of the running scopes by
name — and DR-0018's name lists make that walk find *exactly* the binding the compiled slot was
chosen for. The two are therefore indistinguishable by any program, so the compile-time switch
between them is a branch mutation coverage cannot pin, and it duly survived. AGENTS.md's answer to
a branch nothing can pin is to remove it. Removing this one means every name in every program
resolved at run time. **Is that affordable?**
**Setup:** five loops of 300,000 iterations, each doing nothing but read and write names — locals,
a name one scope out, one four scopes out, and a global. Run against the engine as it is, then
against the same engine with `Compiler::names_are_dynamic` forced `true`, which is exactly the
mutant. Release build, one warm-up run discarded.
**Result:**

| | placed | dynamic | |
| --- | --- | --- | --- |
| local reads | 56.1 ms | 183.6 ms | **3.3×** |
| local writes | 38.2 ms | 114.3 ms | **3.0×** |
| one scope out | 38.1 ms | 133.1 ms | **3.5×** |
| four scopes out | 38.1 ms | 141.0 ms | **3.7×** |
| globals | 162.9 ms | 226.2 ms | 1.4× |

**Verdict:** PROMOTE the *number*, not the code — the branch stays and is now justified by a
measurement rather than an intuition. Three to four times on local variable access is the whole
hot path of the interpreter, and a global read is dearer in absolute terms only because it was
already a property lookup.

The interesting part is what it says about the *method*. A semantically transparent optimisation
is invisible to behavioural mutation testing **by construction**: if flipping it changed an
answer, it would not be transparent. So the ratchet can never kill such a branch, and no
restructuring helps — moving the decision into `binding`, into the chunk, or into the interpreter
leaves the same equivalent pair with the same switch. What closes it is a *structural* test that
asserts the design rather than the behaviour: `a_name_is_a_slot_the_compiler_chose_and_only_a_with
_makes_it_a_walk` reads the emitted instructions, and it is the second such claim in
`compile/tests.rs` for the same reason the first one is there.

**Cost:** about an hour, most of it establishing that no restructuring could work before accepting
that the test was the answer.


## gc-pressure — is the `property-escapes` bucket a memory problem or a time problem?

**Date:** 2026-08-02
**Question:** 878 of the 894 tests failing on DR-0013's RangeError are
`built-ins/RegExp/property-escapes`. The recorded plan said they were blocked on the GC schedule,
which was blocked on DR-0010 slot reuse. Is that true — would a collector plus reusable slots make
them pass?
**Setup:** `cargo run -p praxis-lab -- gc-pressure <test262 file>`, which prepends the harness
includes and runs a whole file with a wall clock. Measured on a 9950X (32 threads, 64 GB DDR5),
`--release`. Three engine builds: as-is; collect-when-exhausted; and collect-when-exhausted with
`footprint` counting only *live* slots, which simulates slot reuse without building it. Per-test
harness budget is 10 s.
**Result:**

| Build | `ASCII.js` | Peak |
| --- | --- | --- |
| as-is (throws at 64 MiB) | — | refused |
| collect on exhaustion | 40.8 s, completed | 54 MiB |
| + simulated slot reuse | (same policy) | 54 MiB |
| no collector, unlimited budget | **21.8 s**, completed | 303 MiB |

Over the whole bucket, simulated reuse + collection took it from 884 failures to 445 — but the run
total fell from 1226 to 787, and the missing 439 are **timeouts the harness drops into no column**.
They did not pass; they vanished.

Instrumenting the exhaustion point showed where the memory goes: a collection reclaims the string
units perfectly (40 MB -> 1 MB, the `result +=` garbage), and 457,392 of 457,397 environments are
garbage but their *slots* stay. So the floor ratchets up — 26, 35, 41, 45 MiB — until slots alone
exceed the budget.

Per-1M-iteration micro-benchmarks isolated the cost, `var` loops throughout (empty loop 0.62 s):

| Shape | Time | Over baseline |
| --- | --- | --- |
| `o.x = i` — fixed key | 0.62 s | 0 |
| `a[0] = i` — one slot, same index | 2.55 s | 1.9 us/store, **17 MiB** |
| `a[len++] = i` — varying index | 4.37 s | 3.8 us/store, 23 MiB |
| `for (let i …)` vs `for (var i …)` | 4.79 s vs 0.63 s | 4.2 us/iteration |

**Verdict:** PARK the bucket — DEAD as a GC target. Even a zero-cost collector leaves `ASCII.js` at
21.8 s against a 10 s budget, so no amount of collector work wins these tests; they need an
interpreter several times faster, which is M8. The recorded claim that the GC schedule unblocks 894
runs is **wrong**, and the 894 should not be costed as GC work.

Two findings worth more than the bucket was:

- **A computed property key allocates a throwaway heap String per access.** `to_property_key` calls
  `to_string` (which `new_string`s a permanent arena slot), then `intern_id` copies the units back
  out and interns them, abandoning the slot just made. `a[0] = i` a million times writes one element
  and costs 17 MiB. That is a DR-0013 leak on every indexed or computed access, and ~2 us of the
  cost. `PropertyKey` has no integer variant, which is the deeper version of the same thing.
- **The harness drops a timed-out run into no column.** `Worker::ask` answers `None` on
  `recv_timeout` and the file's runs are counted as neither passed, failed, nor not-run — so the
  totals silently shrink and a slice that makes tests slower reads as a slice that fixed them.

**Cost:** about an hour, most of it in the three engine builds.

---

## nesting-cost — can the array literal be made cheap enough to raise the cap past 64?

**Date:** 2026-07-27
**Question:** `MAX_NESTING_DEPTH` is 64 because the array literal cliffs at 71 levels in one
mebibyte, against 152 for a parenthesis and 327 for a block. Is the literal's parse expensive
enough to be worth restructuring, and would doing so buy a materially higher cap?
**Setup:** `cargo run -p praxis-lab -- nesting-cost`. For each of eight shapes it bisects the
deepest nesting that survives a 1 MiB thread, one child process per candidate — a stack overflow
aborts, so an in-process bisection is not possible. The engine cannot be instrumented from the
lab, so per-function cost is reached by subtraction: each shape walks a known segment of the call
graph, and `!!!!1` (which recurses inside `parse_unary` alone) is the yardstick for one frame.
`MAX_NESTING_DEPTH` has to be raised out of the way first or the cap is all that is measured; the
instrument detects that and prints `cap-limited` rather than a number that means nothing.

**Result:**

```
shape              debug            release
unary              409  2.5 KiB     1673  0.6 KiB
block              327  3.1 KiB     1153  0.9 KiB
group              152  6.7 KiB     1110  0.9 KiB
conditional        170  6.0 KiB      718  1.4 KiB
array               71 14.4 KiB      392  2.6 KiB
array-pattern       70 14.6 KiB      390  2.6 KiB
computed-member     70 14.6 KiB      390  2.6 KiB
object              41 25.0 KiB      289  3.5 KiB   (two levels per repeat: a paren and a brace)
```

Two things fall out of that table and neither was the expected one.

*The array literal is not the expensive part.* `computed-member` costs the same 14.6 KiB and never
touches `parse_array_literal`: `a[0][0]…` descends the same operand ladder and stops at
`parse_member`. What both pay for is the descent `parse_assignment -> parse_binary -> parse_unary
-> parse_member -> parse_primary`, about six frames at the 2.5 KiB the yardstick says one frame
costs. `(` is cheap for exactly the complementary reason: `parse_arrow_or_group` intercepts it at
the assignment level and it never enters the ladder at all.

*The cost is a debug artefact.* Release is 5.5× cheaper across the board, and the array cliff moves
from 71 to 392. At the same 1.09× margin the cap could be about 359 in release; at a comfortable
1.5× it could be 261.

**Candidate A, measured and rejected.** Skip `parse_arrow_or_group` for a token no arrow may begin
with, saving two frames per level on the `[` path. Result: array 71 -> **70**, computed-member 70
-> **69**. It made things *worse*: the two frames it saves are early returns with almost nothing
in them, and the guard it adds to `parse_assignment` — which is on the path — costs more than they
did. Frame-shaving on this ladder is not the lever; the frames that matter are the ones doing
work, and there is no fat one to split.

**Verdict:** DEAD, for the question as asked. The array literal is not where the stack goes, so
making it cheaper would buy nothing, and the ladder's six frames are each ordinary-sized — there is
no restructuring here worth 50%, which is what a cap of 96 would need.

PARK, for the cap: the binding constraint is not the parser's shape but the stack test's decision
to assert a *debug* build against one mebibyte. That decision is right today — DR-0006 wants a
constant that does not depend on how the engine was compiled, so it has to be safe in the hungriest
build. What would revive this is M3's embedder-set limit, where somebody knows how much stack there
actually is: at that point the release figure is the one that matters and it is six times larger.

**The number the experiment was missing, added later.** This measured what a level *costs* and
never what real code *needs*, so the cap's adequacy was an argument rather than a figure. A sweep
of 4,733 minified files — 120 MB of published npm bundles, plus every built library WordPress and
Moodle vendor — supplies it: two files exceed the cap, both copies of the same Emscripten-generated
Draco decoder, and bisecting `MAX_NESTING_DEPTH` against one says it needs **77**.

That sharpens the verdict rather than changing it. 77 is thirteen past the cap and seven past the
70 levels the narrowest path survives in the build the stack test asserts against, so taking that
file still needs the operand ladder to get cheaper — which is the thing this experiment looked for
and did not find. What it does settle is the size of the gap: not "deeper than anything reasonable"
but *seven levels* beyond what the debug build affords, and comfortably inside what release
already does. That is an argument for M3's embedder-set limit and not for moving the constant.

The instrument stays. It is the thing that answers "did that slice make a level more expensive",
and the cap is going to be argued about again.

**Cost:** about an hour, most of it waiting on bisections — each shape is roughly twenty child
processes and a debug parse of a very large file.

---

## (nothing else yet)

The first one will most likely be the value representation — see `AGENTS.md` M3, where the
choice between a plain `enum Value` and NaN-boxing has to be made with a number rather than an
opinion.
