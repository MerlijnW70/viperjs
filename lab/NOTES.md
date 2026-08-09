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

## dispatch-cost — what does one bytecode instruction actually cost?

**Date:** 2026-08-08
**Question:** `property-lookup` measured both of the property path's levers away and left one term
neither experiment touched — a read is 176 ns where an empty loop is ~122. `interpreter-speed` put
the engine at **"~20 ns per bytecode instruction"** against a good non-JIT interpreter's 2–5, and
concluded there was "roughly 10–20× of headroom inside the current architecture". **That figure came
from dividing one loop's time by an instruction count somebody estimated.** So: measure it.

**Setup:** `cargo run -p viperjs-lab --release -- dispatch-cost`. Six families, each a statement
repeated *k* = 0, 1, 2, 4, 8 times inside a loop of a million passes, best of five. Nothing is
estimated: the lab compiles, so each row's instruction count is read off `Chunk::code().len()`, and
every figure is a **slope** — Δtime over Δinstructions over passes. Adding a statement adds its
instructions to the chunk *and* to every pass, so the two differences are the same difference. The
`k = 0` row is the loop with an empty body, which makes the intercept the per-pass bookkeeping.

**Result.** `size_of::<Value>() = 16`, `size_of::<Instruction>() = 12`. Loop intercept 78–82 ns a
pass for the nine-or-so instructions a `for` header executes.

| family | what the statement is | ns per instruction |
| --- | --- | --- |
| `constant` | `t = 1` | **3–4** |
| `branch` | `if (s) { t = 1 }` | **3–4** |
| `copy-local` | `t = s` | **4–5** |
| `add-local` | `s = s + 1` | **7–8** |
| `compare` | `t = s < 2` | **7–8** |
| `member` | `t = o.x` | **13** |

**Verdict: the headline was wrong by three to six times, and the dispatch is not the lever.**

- **ViperJS runs an ordinary instruction in 3–8 ns**, which is the band `interpreter-speed` named as
  the target and said the engine was 4–10× away from. It is already there. The "10–20× of headroom
  inside the current architecture" claim rested entirely on the 20 ns figure and does not survive it.
- **Where the 20 came from:** that entry divided "71 ns for an increment" by "three or four
  instructions". An `i++` in a `for` header is not three or four instructions — the *header* is nine,
  and the 71 ns was the whole pass. Dividing a loop's time by a guess at its body is how an
  interpreter that is already fast gets ranked as slow.
- **The one outlier is the property read**, at 13 ns averaged over the five instructions
  `t = o.x` emits — so the `GetProperty` instruction alone is several times an add. That agrees with
  `property-lookup` from the other side, where the lookup was ~50 ns of a 176 ns read. **Two
  experiments built for different questions now say the same thing about the same instruction**,
  which is the strongest signal either of them has produced.

**And three suspects for that outlier are already eliminated**, each by short-circuiting it and
re-running rather than by reasoning:

| suspect | probe | result |
| --- | --- | --- |
| the linear scan | properties at position 1…64 | flat to 8, one step at 16, flat to 64 |
| the prototype walk | `is_namespace` forced false; a direct table fast path | no change |
| re-interning the key | `Value::String(id)` taken as a key without interning | 13 → 12 ns |

So the remaining ~50 ns is the *fixed* cost of the read — the pops, the `Completion`, `settle`, the
`Vm`→`Heap` hop through DR-0020's mediated internal methods — and none of it is a data structure.
**Whatever it is, it is not what any of the three names on the performance list said it was.**

**What this means for the ranking.** Nothing in the current architecture is 10× away from where it
should be. The gap to node is that node **stops dispatching** — it compiles hot code to machine code
— and GOAL.md §3 refuses that. What is left that is measurable and honest is one instruction's fixed
overhead, worth perhaps 25 ns on a read, which is 14%. That is a real slice and it is not a
milestone.

**Cost:** about an hour, and it would have been three without the self-calibration: reading the
instruction count off the compiled chunk is what turned "the slope looks about right" into a number,
and the whole point of the entry is that the previous figure was a division by a guess.

---

## property-lookup — is the 8-shape cost polymorphism, or is it the scan?

**Date:** 2026-08-08
**Status:** OPEN — question written before the code, per this file's own first line.

**Question.** `lab/NOTES.md`'s interpreter-speed entry ranks *shapes and inline caches* as the top
performance lever, on one number: a property read costs 174 ns with one shape and 275 ns with eight.
That number is being read as "polymorphism is expensive, so it wants an inline cache". **ViperJS has
no inline cache, so there is nothing for eight shapes to miss** — which means the row cannot be
measuring what it is being quoted for. Two other things vary in it:

- The benchmark builds its eight objects as `o["f" + j] = j` for `j` in `0..=k`, then `o.x = k`. So
  `x` sits at position 1 in the first object and position 8 in the last, and an own-property table
  is a `Vec<(PropertyKey, Property)>` walked in order. The average scan is 4.5 entries, not 1.
- It reads `os[i & 7]`, an array element, before the property. DR-0026 made that 4.4× cheaper two
  days ago and the 275 already includes the improvement.

**So: how much of the gap is the linear scan, how much is polymorphism proper, and how much is the
prototype chain?** The answer decides whether the next slice is a lookup structure (small, local,
no semantics at risk) or hidden classes with inline caches (large, and §10.1.11's key order is at
risk). Ranking them the wrong way round costs a milestone.

**Setup.** `cargo run -p viperjs-lab --release -- property-lookup`. Four axes, each a set of rows
differing in exactly one thing, so every interesting quantity is a subtraction between two rows:

- **A — scan length.** One shape, `x` at position 1, 2, 4, 8, 16, 32, 64. If the table is walked,
  this is linear in the position; if it is not the cost is flat.
- **B — polymorphism.** `k` distinct shapes at one site with `x` at the **same** position in every
  one, `k` = 1, 2, 4, 8. Prediction from reading the code: flat, because there is no cache to miss.
  A rise here is the thing an inline cache would buy.
- **C — prototype depth.** `x` found 0, 1, 2 and 4 levels up a chain. A miss on the own table is a
  scan *and* a walk, and a chain is the other linear thing in a lookup.
- **D — a miss.** `o.absent` against `o.x` on the same table, which is the whole scan plus the whole
  chain and is what `in` and a defaulting read pay.

Measured through the interpreter rather than by calling the heap directly, because that is what a
program pays; every row carries the same ~122 ns of dispatch, so the differences are clean.

**Result.** `cargo run -p viperjs-lab --release -- property-lookup`, best of five per row, on the
machine every other number in this file came from.

| axis | row | ns/read | over base |
| --- | --- | --- | --- |
| A scan | `x` found first | 176 | — |
| | one entry walked past | 176 | +0 |
| | seven walked past | 177 | +1 |
| | fifteen walked past | 190 | **+14** |
| | thirty-one walked past | 190 | +14 |
| | sixty-three walked past | 190 | +14 |
| B shapes | one shape (carries an array read) | 255 | — |
| | two shapes | 259 | +4 |
| | eight shapes, `x` first in each | 269 | **+14** |
| | **eight objects, one shape** | 268 | **+13** |
| | eight shapes with `x` moving — the original row | 276 | +21 |
| C proto | found on the object | 175 | — |
| | one level up | 181 | +6 |
| | two levels up | 190 | +14 |
| | four levels up | 208 | **+33** |
| D miss | a hit at the end of eight | 269 | — |
| | absent, walking to `Object.prototype` | 290 | **+21** |
| | absent, `Object.create(null)` | 269 | +0 |

**Verdict: DEAD for inline caches, and the premise that ranked them was two things at once.**

- **There is no polymorphism cost to cache.** Eight *distinct* shapes and eight objects of *one*
  shape cost the same to within a nanosecond — 269 against 268. So the +14 is the price of touching
  eight objects rather than one, which is cache lines and not hidden classes, and an inline cache
  removes none of it. A monomorphic site and an octomorphic site are already the same speed here.
- **The 101 ns that put shapes top of the list is mostly the benchmark's own array read.** The old
  row compared `os[i & 7].x` against `o.x` and attributed the whole difference to shapes. The array
  indexing alone is +79 (255 against 176); `x` moving through the table is +7; the eight objects are
  +14. Nothing is left for polymorphism. **Two rows that differ in two things cannot attribute
  either, and this one was quoted for a year of work.**
- **"Every lookup is a linear scan" stopped being true before it was written down.** `Object` keeps
  a `HashMap` index above `INDEXED_ABOVE = 8` properties, and axis A is that constant exactly: flat
  from 1 to 8, one step of +14 at 16, and flat again to 64. The step is the *hash* costing more than
  a scan of eight interned keys — which is what `INDEXED_ABOVE`'s own doc claims and nothing had
  measured. So the third item on the performance list is already built, and it is built the right
  way round.

**What the table does say is worth having**, and none of it is where anyone was looking:

- **A prototype level costs ~8 ns** and is linear in the depth. That is the largest per-unit cost
  found, and it is what every method call on a class instance pays — `o.m()` on a two-deep hierarchy
  is +14 before the call starts. **This was then recommended as the next slice and measured DEAD the
  same day — see the follow-up below.** The trend is real and the cause is memory rather than work,
  which no table of this kind can distinguish.
- **A miss costs +21 ns and all of it is the walk**: the same miss on an `Object.create(null)` costs
  nothing at all. So a defaulting read is priced by the chain and not by the miss.
- **The baseline is the lever, and it is not a lookup at all.** A read is 176 ns where an empty loop
  is ~122, so the lookup is ~50 ns and the *dispatch* is the rest. `interpreter-speed` already put
  the engine at ~20 ns an instruction against a good non-JIT interpreter's 2–5, and this experiment
  is one more measurement saying the property path is not where the money is.

**Cost:** about two hours, of which most was two measurement faults worth recording because both
inflate a result rather than break it: measuring at **script top level**, where `o`, `s` and `i` are
properties of the global object and the yardstick reads 645 ns instead of 176; and a yardstick that
did not carry the `===` and the `?:` its own comparison row carried, which made a miss look like
+114 ns rather than +21.

### Follow-up the same day: the prototype walk is DEAD too, and the recommendation was mine

The verdict above named the prototype chain as the one measured cost that scales — ~8 ns a level,
linear — and recommended it as the next slice. **Measured, it is not a slice.** Two structural
suspects, both eliminated by short-circuiting them and re-running rather than by reasoning:

| probe | proto-1 | proto-2 | proto-4 |
| --- | --- | --- | --- |
| as it is | +6 | +14 | +33 |
| `is_namespace` forced to `false` | +7 | +18 | +34 |
| a direct table fast path *before* every exotic check | +6 | +14 | +36 |

- **`is_namespace` is a `HashMap` probe on every chain level of every property read in the language,
  and it costs nothing.** The table is empty in any program that imports no modules, and an empty
  `HashMap` answers before it hashes. That looked like the find of the day for about ten minutes:
  a side-table probe on the hottest path in the engine, with a free fix. **A structure being wrong
  in principle is not the same as it being slow, and only one of those is worth a commit.**
- **Skipping every exotic check does not help either.** A fast path that reads the table directly —
  no namespace, no view, no String object, no arguments map — leaves all three deltas where they
  were. So the per-level cost is not the *checks* and not the *lookup*.

**What is left is the walk itself: a different object fetched out of the arena at each level.** An
`Object` is around a hundred bytes, so four levels is four cache lines, and the only way to remove
that cost is to not walk — a cache. Which is where this ends: a per-object or per-site memory of
where a name was last found has a **wrong-value** failure mode (a stale entry answers a value rather
than an error) and an invalidation surface covering `[[Set]]`, `delete`, `Object.setPrototypeOf`,
`__proto__`, `defineProperty` and any Proxy in the chain — against a ceiling of **+6 ns on the
one-level chain that real code actually has**, which is 3% of a 176 ns read.

**Verdict: DEAD, and the same shape as the row above it.** Both of the property path's remaining
"next levers" turn out to be memory locality wearing a name that suggests an algorithm. The
measurable lever left is the one neither experiment touched: a read is 176 ns where an empty loop is
~122, so **the dispatch is the term, not the lookup**.

**And the recommendation that opened this section was mine, made from this file's own table.** It
was drawn from the one axis that showed a clean linear trend, which is exactly the shape that
invites an algorithmic fix — and the trend was real while the cause was not. A cost that is linear
in a count can be linear because of the work per item *or* because of the memory per item, and the
table cannot tell those apart. **Short-circuit the suspect and re-run; it takes two minutes and it
is the only thing that can.**

---

## reentry-cost — how much higher can `MAX_REENTRY_DEPTH` go?

**Date:** 2026-08-06
**Question:** `MAX_REENTRY_DEPTH` is 32, and a re-entry is *any* native calling back into the
interpreter — so `node.children.map(walk)` stops at depth 33 while pure recursion reaches 5,000.
`ajv` hits it compiling a schema. The cap's own comment said this was "nothing anyone will meet".
**How much higher can it go, and what is the lever?**
**Setup:** `cargo run -p viperjs-lab -- reentry-cost`, with `MAX_REENTRY_DEPTH` raised to a million
first so the stack is what refuses rather than the constant. One child process per candidate depth,
because an overflow aborts and cannot be bisected inside one process; a 1 MiB thread, the smallest
in common use, in a **debug** build, whose frames are largest. Three shapes, so a native's own
frame is reached by subtraction from the interpreter's.
**Result:**

| shape | deepest that survives | bytes per level |
| --- | --- | --- |
| `valueOf` | 43 | 24.4 KiB |
| `map` | 38 | 27.6 KiB |
| `sort` | **35** | 30.0 KiB |

**Verdict: PARK, and the question was the wrong way round.** The cap cannot go *up*: at 32 the
margin on the dearest shape is **1.09×**, where the constant's comment claimed "better than 2×".
The comment was not wrong about what it measured — a `toString` chain, the cheapest of the three —
and the cap has to hold for the dearest. So the engine has been one fattened arm away from the
macOS abort it already suffered once, and nothing would have caught it: the guard test
`a_conversion_at_the_cap_fits_in_the_stack_it_claims_to_need` used the cheapest shape too. **That
test now uses `sort`**, with a second row keeping the pair honest if they ever swap places.

Two things ruled out, so they are not re-tried. `MakeClass` and `MakeFunction` were the documented
suspects for the fat frame and are **not** it — both hold an `Rc<Chunk>`, which is a pointer, not a
copy. And raising the cap while shrinking nothing is not a trade: it converts a `RangeError` a
program can catch into the abort DR-0002 forbids.

**What would revive it:** the frame profiled *per arm* rather than guessed at. The loop is one
function and its frame is the sum of every arm's locals, so a level costs 24 KiB before any native
adds to it — and 24 KiB of locals in one `match` is a number somebody should be able to attribute.
Halving it buys 64 with a real margin, which is what `ajv` needs and roughly where this was before
the macOS failure.

**Cost:** about two hours, most of it the bisection running rather than being written — each trial
is a process and a cliff at 35 takes a dozen of them per shape. The experiment stays: it is the
only thing here that can answer "did that slice cost us stack?", and the answer is now needed after
any change to the interpreter loop rather than after any change to the cap.

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
**Question:** `a sloppy var inside a direct eval in a function` is the largest thing ViperJS refuses
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

**Why, and it is the part worth keeping:** ViperJS puts a function's parameters, its `arguments`,
its `var`s *and* its body's `let`s in **one environment**, where §10.2.11 has up to three. So a
depth comparison cannot tell "bound in the variable environment" from "bound in a scope between
here and it" — they are the same number. The `declare-arguments` tests are exactly that
distinction: they expect a SyntaxError because `arguments` is bound in a scope the walk passes
*through*, and with one environment the check reads it as the destination and accepts.

So the split is real in the specification and not expressible against ViperJS's environments. **The
prerequisite is §10.2.11's environment split — a parameter scope separate from the variable scope
when the parameter list has expressions — and that is a bigger slice than the one it unblocks.**

**Cost:** an hour, and the ratchet earned its keep: every hand-written probe answered correctly and
the suite still said no. A change that is right about the cases you thought of and wrong about the
ones you did not is exactly what a conformance number is for.


## run-module — does the engine run somebody else's library?

**Date:** 2026-08-04
**Question:** `examples/parse` sweeps a repository and answers "does it parse", which is the front
end only. §16.2's linker, the namespace objects and the live bindings are runtime, and had never
been pointed at a graph written by somebody who had never heard of ViperJS. **Does a real library
link and run, and what breaks first?**
**Setup:** `cargo run -p viperjs-lab --release -- run-module <entry.js>`, against a three.js
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
**Setup:** `cargo run -p viperjs-lab --release -- hot-shapes`. Twelve source shapes, 100,000
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
million passes at all. This limits every ViperJS program, not the `property-escapes` tests
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
the number that record was missing: without it, ViperJS cannot call a function a million times.

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
**Setup:** `cargo run -p viperjs-lab -- gc-pressure <test262 file>`, which prepends the harness
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
**Setup:** `cargo run -p viperjs-lab -- nesting-cost`. For each of eight shapes it bisects the
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

---

## alternation-width — does the matcher try every alternative at every position?

**Date:** 2026-08-09
**Question:** `he` — the HTML entity library under `htmlparser2` and `cheerio` — runs 7.8× slower
than node on a real workload, where every other library measured this session runs at parity or
faster. `interpreter-speed` says the general gap on *bundle* work is about 2.2×, so 7.8× wants its
own explanation. Is this the interpreter, or is it §22.2's matcher specifically?
**Setup:** `lab/scratch/bench-alternation.js`. `he.decode` matches against an alternation of roughly
two thousand named entities, so the first instrument split `he`'s own cost
(`H:/tmp/ghsweep/probes/he-split.mjs`) and the second varies **branch count alone** against a fixed
eight-character subject that shares no first character with any alternative — so every attempt fails
at its first byte and the number is the cost of *reaching* the branches rather than of matching
inside them. A length ladder on a single alternative is the control for the other axis.

**Result.** `he`'s cost is almost entirely `decode`: 697 ms of 776. The row that says why is
`decode` of a string containing **no entities at all** — 138 µs for eight characters, where 20,000
bare alternation tests cost 2 µs each.

| branches | µs/call | ns per branch per character |
| --- | --- | --- |
| 1 | 2.8 | 344 |
| 10 | 3.0 | 37.5 |
| 100 | 6.8 | 8.44 |
| 500 | 22.5 | 5.63 |
| 1000 | 41.3 | 5.16 |
| 2000 | 83.3 | 5.20 |

**The last column is flat from 500 upwards.** Cost is `branches × positions × 5.2 ns`, exactly. The
length control is linear in the other axis. Node on the same table is **0.03 ns** per branch per
character — about 170× less — because Irregexp discriminates on the first character before it tries
an alternative.

**Verdict: PROMOTE to M8, and it is not the JIT gap.** This is a missing *prefilter*, not a missing
compiler: every backtracking engine narrows the alternatives at a position by the character sitting
there, usually as a first-code-point set per alternative and a bitmap test before the attempt.
Semantically it is pure pruning — an alternative whose first-set excludes the current character
cannot match — so "none may cost a single conformance test" is checkable rather than hoped for.

### The diagnosis above was wrong, and the table was right — 2026-08-09, same day

**Built the prefilter and it changed nothing: 5.20 ns/branch/char before, 5.42 after.** The
measurement was sound and the reading of it was not, and the benchmark said so if it had been read
properly — every alternative in it begins with `q`, and they sit behind a literal `&` that the
subject `zzzzzzzz` never matches. **The disjunction was never entered.** A cost that scales with
branch count in a pattern whose branches are never reached is not a matching cost at all.

It was `builtins/regexp.rs` doing `found.pattern().clone()` — a **deep copy of the whole parsed
tree, on every match operation**. That is why cost tracked pattern size rather than the work done,
why an eight-character subject cost 138 µs, and why `he.decode` of a string with no entities in it
was expensive. `[[RegExpMatcher]]` never changes after the object is built, so nothing needed a
copy; the field is an `Rc<Pattern>` now and the call site takes a handle.

| | µs/call at 2,000 branches |
| --- | --- |
| before | 86.8 |
| the `Rc` alone | 2.5 |

`he` end to end: **776 ms → 114 ms** against node's 97, from that one line.

**And the prefilter still earns its place, measured on a benchmark that can see it** — branches with
distinct first characters, behind a `&` the subject *does* match, so the disjunction is entered and
every branch attempted:

| branches | with prefilter | without |
| --- | --- | --- |
| 500 | 3.00 µs | 4.50 µs |
| 2,000 | **4.70 µs** | **10.60 µs** |

and `he` goes 114 → 97 ms, which is parity with node. So both land, and the order they were found in
is the wrong way round: **the cheap structural fix was worth 6.8× and the clever one 1.2×.**

**The transferable part is the benchmark, not the fix.** A ladder that varies one thing is only
evidence if the thing it varies is on the path being measured; this one varied branch count in a
pattern whose branches were unreachable, and it produced a beautifully linear table of the cost of
*cloning* them. **Check that the work you think you are timing actually runs** — the cheapest way
here would have been to make the subject match and watch the number move.

**Two things it is worth being careful about before building it**, both of which decide how much of
the table it actually recovers:

- A first-set is only sound where an alternative *must* consume a character to match. An alternative
  that can match empty, or that opens with a lookaround or a backreference, has no first-set and
  must always be tried.
- The `i` flag, and `u`/`v`'s case folding, make the first-set a set of *folded* code points. Getting
  that wrong turns a pruning step into a wrong answer, which is the one failure mode that matters
  here.

**And it may reach further than `he`.** `gc-pressure` established that
`RegExp/property-escapes` is time-bound rather than memory-bound and left M8 as its answer without
saying which part of M8. A property escape compiles to a large class rather than a large
alternation, so this table does not measure it — but it is the first evidence that the matcher's
per-attempt constant, rather than the interpreter's, is what that bucket is spending.

**Cost:** about half an hour, most of it writing the two instruments.

---

## regexp-bucket-cost — is the slowest directory in the suite slow because of the matcher?

**Date:** 2026-08-09
**Question:** `RegExp/CharacterClassEscapes` runs 24 tests in 57 s — 2.4 s each, against 2.7 ms for
an average `Array/prototype` run. AGENTS.md has pointed at this family as M8's job since M5. Is it
the matcher, and if so which part?

**Setup:** `lab/scratch/bench-find-scan.js` and `lab/scratch/bench-array-growth.js`, each timing the
halves separately, run on `viper --release` and on node with a `print` shim.

### It is not the matcher, and it is not close

These tests do two things: `buildString` assembles a subject holding every code point in a set —
often a million of them — and then one `regExp.test(subject)` runs over it. Timed apart, on a
400,000-unit subject:

| | ViperJS |
| --- | --- |
| build the subject | **722 ms** |
| scan all 400,000 positions for `/\d/` (no match) | **1 ms** |
| anchored match consuming the whole subject | 1 ms |

**The search is a thousandth of the run.** I had a prefilter half-designed for `Matcher::find`'s
position loop — the same first-character trick `alternation-width` put on a disjunction — and this
killed it before a line was written. A pattern that fails at every position over 400,000 positions
costs one millisecond; there is nothing there to win.

### Where the 722 ms goes, bisected

Three variants of the build, each adding one thing to the one above:

| | ViperJS | node |
| --- | --- | --- |
| the loop and `chunk.push(n)` alone, no strings | **726 ms** | 3 ms |
| …and `String.fromCodePoint.apply` per 10,000-chunk | 763 ms | 4 ms |
| …and accumulating with `+=` | 768 ms | 5 ms |

**All of it is in the first row**, and `fromCodePoint` and the concatenation together add 6%. So the
slowest directory in the conformance suite is slow because of `Array.prototype.push`, and the regular
expression it exists to test is free.

### The element-store ladder

`bench-array-growth.js`, 200,000 turns each, ViperJS against node:

| | ViperJS ns/turn | node | over the empty loop |
| --- | --- | --- | --- |
| empty loop | 185 | 5 | — |
| `o.p = i` — a named property, no array | 245 | 0 | **+60** |
| `a[i % 64] = i` — an array that never grows | 505 | 5 | **+320** |
| `a[i] = i` into `new Array(N)` — pre-sized | 675 | 5 | +490 |
| `a[i] = i` — growing | 775 | 10 | +590 |
| `a[a.length] = i` | 850 | 10 | +665 |
| `a.push(i)` | 1,595 | 10 | **+1,410** |

Three things this says that the earlier `interpreter-speed` table could not:

- **Growth is not the cost.** Pre-sizing with `new Array(N)` saves 100 ns of 590, and an array that
  never grows past 64 elements still costs 320 ns. So it is not reallocation and it is not the
  index map crossing `INDEXED_ABOVE`.
- **An indexed store costs five times a named one** — 320 ns against 60 — for the *same* work in a
  smaller key space. Every element is a `(PropertyKey, Property)` in the object's own property
  vector, so `a[7] = x` is the whole of §10.1.9's `[[Set]]` into a general property table, plus
  §10.4.2's `length` maintenance on top.
- **`push` is 2.4× a plain indexed store**, and reading §23.1.3.23 says why without any guessing:
  it is a native call, a `Get` of `length` with `ToLength`, the element `Set`, and a second `Set` of
  `length` — three property operations where `a[i] =` is one. Nothing in the implementation is
  wasteful; there is simply more of it. This answers `interpreter-speed`'s open item 5 —
  "`push`/`pop` at 2.4 µs wants its own look before anyone guesses at it" — and the answer is that
  it is the element store underneath it, times three.

### The verdict

**M8's array work is a dense element store, and it is the lever with the widest reach.** Not the
matcher, not `property-escapes`, and not `push` on its own. An array whose keys are a contiguous
run of integers should hold its elements in a `Vec<Value>` beside the property table rather than
inside it, with the property table used only for the sparse and the exotic.

**Not started here, deliberately.** It touches §10.4.2's `[[DefineOwnProperty]]`, every internal
method that can see an element, §10.1.11's key order, holes and `length` truncation, and the
collector — which is a decision record and several commits, not the tail of an afternoon. What this
experiment establishes is that it is the right one to spend them on, and that two other candidates
are not.

**And one stale claim is now retired.** AGENTS.md said what remained of `RegExp/property-escapes`
was "genuinely slow and M8 is genuinely its answer". Measured the same day: the directory passes
**1,206 of 1,226 runs**, the expectations file lists **six**, and all six are `Script=Unknown` — a
UCD default the table generator never emitted. It is a data gap of six runs. That paragraph has now
been corrected three times, every time downwards.

**Cost:** about forty minutes, most of it in the two conformance directory timings.

## interpreter-speed — how far is ViperJS from node, and what would close it?

**Date:** 2026-08-08
**Question:** The ask was "as fast as or faster than node.js". What is the real gap, where does the
time go, and which of it is reachable without a JIT?
**Setup:** `lab/scratch/bench-vs-node.js` — twenty cases, each returning a checksum so an engine
that optimises the work away is caught rather than credited. Run on ViperJS with `viper`, and on
node with a three-line `print` shim. **Node was re-run at fifty times the iteration count** because
its first numbers were 1–3 ms, which is timer resolution rather than a measurement; the ratios below
rest on the 50M run. `lab/scratch/bench-access-ladder.js` is the second instrument: each row differs
from the one above by exactly one thing, so a difference between two rows is the cost of that thing.

### The gap, measured

| case | node ns/op | ViperJS ns/op | ratio |
| --- | --- | --- | --- |
| empty loop (`var`) | 0.4 | 82 | 205× |
| empty loop (`let`) | 0.4 | 172 | 430× |
| arithmetic | 2.3 | 207 | 90× |
| property read, fixed key | 0.4 | 170 | 425× |
| property read, 8 shapes | 2.3 | 469 | 204× |
| array read `a[i & 7]` | 0.5 | 407 | 814× |
| array write `a[i & 7] = i` | 0.6 | 886 | 1,477× |
| `push` + `pop` | 1.0 | 2,819 | 2,819× |
| call, two arguments | 0.4 | 301 | 753× |
| method call | 0.6 | 482 | 803× |
| closure creation | 1.0 | 1,540 | 1,540× |
| string `+=` | 50 | 11,000 | 220× |
| `fib(24)` | ~1,000,000 | 26,000,000 | 26× |

**And yet 1.2 MB of prettier, preact, valibot and d3-array runs in 447 ms against node's 200 — 2.2×.**
That contrast is the most useful thing here and it is not a contradiction: **module initialisation
is one-shot work, where V8's JIT never warms up.** A bundle is mostly closure creation, object
literals and a single pass of each function, and ViperJS is within a small factor there. Hot loops
are where a JIT wins by three orders of magnitude, and that is what these micro-benchmarks measure.

### Where the time goes

Reading the ladder downwards; each delta is the cost of one added thing.

| step | ns/op | delta | what the delta buys |
| --- | --- | --- | --- |
| `s += i` — the loop alone | 122 | — | dispatch, stack, `Value` |
| local variable read | 122 | 0 | free |
| `o.x` — literal key | 170 | +48 | one property lookup |
| `o[k]` — variable String key | 170 | 0 | **nothing**: the key is already interned |
| `a[0]` — literal index | 214 | +44 | the array's element path |
| `a[i & 7]` — varying index | 412 | **+196** | turning a varying Number into a key |
| `a[i & 7] = i` | 895 | **+483** | the write path on top of the read |
| `Int32Array` read | 958 | — | §10.4.5's checks, *slower* than a plain Array |
| `push` + `pop` | 2,406 | — | |
| `s += "x"` | 11,000 | — | a copy of the whole String per append |

**~20 ns per bytecode instruction** is the headline number, from `i++` amortised over four per loop
pass: 71 ns for an increment of three or four instructions. A good non-JIT interpreter is 2–5 ns.

> **Wrong by three to six times, measured 2026-08-08 — see `dispatch-cost`.** An `i++` in a `for`
> header is not three or four instructions; the header is nine, and the 71 ns was the whole pass.
> Measured by slope against instruction counts read off the compiled chunk, an ordinary instruction
> is **3–8 ns** — already inside the band this sentence names as the target. **This is what dividing
> a loop's time by a guess at its body produces**, and it ranked a fast interpreter as slow for two
> days of planning.

### The verdict

**"Faster than node" is not reachable without a JIT, and GOAL.md §3 refuses one.** V8 compiles hot
code to machine code through three tiers; a bytecode interpreter cannot meet that, and no amount of
tuning changes the kind of thing it is. What *is* reachable is the range good non-JIT engines
occupy — QuickJS sits around 30–50× V8 on this sort of loop, where ViperJS is at 200–800×. **So
there is roughly 10–20× of headroom inside the current architecture**, and it is worth having.

> **The headroom claim does not survive `dispatch-cost` either**, because it rested entirely on
> the 20 ns figure above. An ordinary instruction is 3–8 ns and the remaining honest target is
> one instruction's fixed overhead — a property read, worth perhaps 14% of a read. The gap to
> node is that node stops dispatching, which GOAL.md §3 refuses.

Ranked by what the measurements support, not by what sounds promising:

1. ~~**The interpreter loop's stack frame.**~~ **Falsified the same day — see the correction
   below.** The frame costs no speed at all; it remains a `MAX_REENTRY_DEPTH` problem and is not a
   performance one.
2. ~~**An integer variant for `PropertyKey` — a measured +196 ns per varying index.**~~ **Built,
   and measured at −200 ns — see the result section below.** `a[i]` used to turn the Number into
   decimal text, encode that to UTF-16 and intern it; DR-0026 removed all three from every indexed
   access, and took a TypedArray read down by 741 ns as well.
3. ~~**Shapes and inline caches.**~~ **DEAD — see `property-lookup` above, which took this row
   apart.** The eight-shape figure quoted here compares `os[i & 7].x` against `o.x` and charges the
   whole difference to shapes; the array indexing is most of it, and eight objects of *one* shape
   cost the same as eight of eight. There is no polymorphism cost here to cache.
4. **String concatenation is quadratic.** 11 µs per append at 100k appends is a copy of the whole
   String each time. A rope or cons-string representation is the usual answer.
5. **`push`/`pop` at 2.4 µs** wants its own look before anyone guesses at it.

**Ruled out by the charter, so nobody should re-cost them:** a JIT (GOAL.md §3), NaN-boxing (needs
`unsafe`, DR-0002 — and the lab already measured it at 1.4× and parked it), and any dependency
(DR-0001).

**Cost:** about an hour, most of it in the two benchmark runs and one re-run of node at a size where
its numbers mean something.

### Correction, an hour later: the frame is not the speed lever

The recommendation above was a *deduction* — "everything rides on the 122 ns baseline, and a
13,728-byte frame means the hot loop keeps nothing in registers" — and it is wrong. Falsified by
inflating the frame on purpose rather than by arguing about it:

| | release frame | ns/iteration |
| --- | --- | --- |
| as it is | 3,272 | 144.5 |
| with a 16 KiB `black_box`'d array live across the loop | **19,704** | **144.5** |

Six times the frame, no difference. Obvious afterwards: those bytes are *unused* on the hot path,
the compiler keeps the handful of live locals in registers regardless, and `__chkstk` is paid once
per `execute` **call** rather than per instruction.

Two smaller things came out of the same hour and are worth keeping:

- **`yield_from_generator` was not `#[inline(never)]`** and owned 105 stack slots, the largest
  single contributor to the release frame. Marking it and two neighbours took the frame from 3,272
  to 3,080 — **6%**, and no measurable speed. Reverted: it buys nothing anything can see.
- **The debug frame did not move at all**, because a debug build inlines nothing to begin with. The
  two frames have different causes: debug is the sum of the locals the `match` arms declare, release
  is whatever survives sharing. `coerce.rs` already said the compiler shares slots across arms; it
  was right and reading it first would have saved the experiment.

**So the ranking above stands with item 1 struck out**, and the integer `PropertyKey` — +196 ns per
varying index, isolated by construction rather than deduced — is the first item that rests on a
measurement. DR-0026 is that.

**And a third cost turned up while reading for it:** an object's own properties are a
`Vec<(PropertyKey, Property)>`, so every lookup is a **linear scan**. That is adjacent to the key
work and separate from it, and it is part of why eight shapes cost 469 ns against a single shape's
170.

> **Both halves of that paragraph are wrong, measured 2026-08-08 — see `property-lookup`.** The scan
> stops at `INDEXED_ABOVE = 8` and a `HashMap` takes over, so it was never linear for the objects
> that would suffer; and the eight-shape figure is the benchmark's own array read rather than
> anything about shapes. Left in place because the correction is the more useful record: **a cost
> "turned up while reading" is a hypothesis, and this file printed it as a finding.**

### Result, two days later: DR-0026 is built, and it beat its own estimate on the row nobody costed

`PropertyKey::Index(u32)` is in. Same machine, same two benchmark files, three runs each and
identical to within 3 ns.

| step | before | after | delta |
| --- | --- | --- | --- |
| `s += i` — the loop alone | 122 | 122 | — |
| `o.x` — literal key | 170 | 174 | — |
| `a[0]` — literal index | 214 | **158** | −56 |
| `a[i & 7]` — varying index | 412 | **212** | **−200** |
| `a[i & 7] = i` — write | 895 | **439** | **−456** |
| `Int32Array` read | 958 | **217** | **−741** |
| property read, 8 shapes | 469 | **275** | −194 |

**The estimate was +196 ns for a varying index and the measurement is −200, which is as close as
this instrument gets.** Three of the seven rows moved for reasons the estimate did not name, and
each is worth keeping:

- **The TypedArray read is 4.4× faster, and that row was never costed.** §7.1.21 was implemented by
  decoding the key's UTF-16 into a Rust `String`, parsing an `f64` out of it and formatting it back
  to check the spelling round-trips — per element access. An `Index` key answers all of that with
  one comparison against the view's length. **The largest single win in the slice was in the file
  the record does not mention.**
- **A literal `a[0]` got 56 ns faster although its key is a *constant*.** The interning was already
  done at compile time, so the estimate said zero; what it missed is the other half of the round
  trip — every access still read the units back out of the heap and decoded ten digits to find the
  element. A representation change pays on both sides of the boundary, not only where it is made.
- **Eight shapes fell by 194 ns**, which is the array read inside that benchmark's own `os[i & 7]`
  rather than anything about shapes. **A benchmark row is only as isolated as its body**, and this
  one is two operations wearing one name — the ladder is the instrument that separates them.

**One thing got cheaper that is not on the list:** an `Index` key holds no String, so the collector
has nothing to mark for it and a program that fills an array no longer interns one String per
element.

**And the conformance suite regressed by eight, which was worth more than the measurement.** All
eight were `Array.prototype.slice` timing out, because `slice` asked `ArraySpeciesCreate` for a
zero-length array where §23.1.3.25 step 8 asks for `count` — a bug it had always had, whose walk had
always been stopped by the heap budget the per-index interning spent. Take the allocation away and
the walk stops ending. **A speed experiment can remove a termination argument nobody wrote down**,
and the place it was written down here was a doc comment explaining why the budget check worked.

**Ranking after this:** item 3 (shapes and inline caches) is now the top of the list, and the linear
scan noted just above is the cheaper half of it — a `Vec` lookup is what both a shape and a hash
would replace, and measuring the scan alone would say which is worth building.

