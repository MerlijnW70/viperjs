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
