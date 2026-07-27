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

## (no experiments yet)

The first one will most likely be the value representation — see `AGENTS.md` M3, where the
choice between a plain `enum Value` and NaN-boxing has to be made with a number rather than an
opinion.
