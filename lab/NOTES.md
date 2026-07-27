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

The instrument stays. It is the thing that answers "did that slice make a level more expensive",
and the cap is going to be argued about again.

**Cost:** about an hour, most of it waiting on bisections — each shape is roughly twenty child
processes and a debug parse of a very large file.

---

## (nothing else yet)

The first one will most likely be the value representation — see `AGENTS.md` M3, where the
choice between a plain `enum Value` and NaN-boxing has to be made with a number rather than an
opinion.
