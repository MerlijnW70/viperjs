---
id: DR-0023
title: The interpreter may collect for itself, and only where it owns every live value
status: prose-only
---

DR-0013 gave the heap a budget and DR-0019 made a swept slot reusable. Between them they left one
thing undone, and it is the thing an embedder notices first: **nothing ever calls the collector**, so
a program that allocates steadily reaches the budget and is thrown a RangeError however little of
what it allocated is still live.

The measurement is in `lab/NOTES.md` under `somebody-elses-code`, and it is two lines of JavaScript:

```js
for (var i = 0; i < N; i++) { s = f(s) }
```

`N = 800,000` runs. `N = 1,000,000` does not. A call retains about 74 bytes of arena, DR-0019's note
predicted the wall at "about 900,000 calls", and that is where it is. An engine that cannot run that
loop is not one anybody can embed, whatever its conformance number says.

## The old measurement said not to, and its premise expired

`Vm::execute` carried a note for several milestones: a collection scheduled every eight mebibytes
cost **318 conformance files their time budget to buy six passes**, and one at the budget cost 79 to
buy none. The conclusion drawn was "until a slot can be reused, walking the heap buys less than the
walk costs".

That was correct and is now void. It was taken when a collection could not reclaim an object's slot
at all — the walk really did buy almost nothing. DR-0019 changed exactly that, and re-running the
suite with a schedule on now reports **no regressions and several hundred newly passing runs**,
measured three times because the count itself is not stable run to run.

**This is the second time a conclusion in this repository outlived its premise by three commits.**
The first was six comments claiming a swept slot is never reused. Both were found by re-measuring
rather than by reading, and both are the reason `Vm::execute`'s note now says *when* it was taken.

## What is decided

**The loop may collect for itself**, on a threshold a host sets with `Vm::set_collection_growth`.
Three properties, each of which cost a measurement:

**It triggers on growth, not on size.** `Heap::footprint` is a high-water mark for its slot terms: a
collection makes slots reusable rather than returning them, so the number does not fall. A threshold
on the total would fire once and then at every check for ever. Growth since the last collection is
self-limiting — a program whose live set is steady stops growing the arena and stops collecting.

**The next allowance is the live set, floored at the base.** A fixed threshold is pathological when a
program holds a great deal: the walk costs what is *live*, so it is repeated once per fixed step of
growth. Measured on a loop holding 150,000 objects, a one-mebibyte step ran **3.56 s** against
**0.61 s** for a sixteen-mebibyte one — six times the work, all of it re-walking the same live set.
Scaling the next allowance by `Heap::live_footprint` removes it: the same three thresholds then ran
0.576 s, 0.556 s and 0.547 s, against 0.466 s for not collecting at all.

**It collects only when no native is re-entered.** This is the property the whole thing turns on, and
it was found by the suite rather than by argument. `Array.prototype.sort` reads its elements into a
Rust `Vec`, calls a comparator that re-enters the interpreter, and writes them back. A collection
underneath that comparator freed every element, because a root set is a claim about what a *program*
can name and those elements were named only by Rust. The suite reported `undefined` where an object
had been — a wrong value, not a crash, which is precisely the failure `heap::collect`'s own header
warns a missing root produces. DR-0011 already counts the re-entries for its own bound, so the fact
was on the machine before this needed it.

**What that costs, stated rather than discovered later:** a program spending its time *inside* a
native re-entry — a long `sort`, a `JSON.parse` with a reviver, a `replace` with a function — does
not collect until it comes back out. Removing that restriction means rooting the natives' working
sets, which is a shadow stack, which is the same thing DR-0019 refused for compaction and for the
same reason.

## The default is on, and two bugs had to be found first

Turning it on failed exactly one test — a module with a top-level `await` — and that turned out to be
**two** faults stacked, neither of which was about `await`. A graph with no `await` at all failed the
same way once the schedule was aggressive enough.

- **A graph never started a collection window.** `Vm::run_module_graph` does not go through
  `Vm::run`'s preamble, so the base was zero and the realm's own footprint cleared any threshold a
  host set before a single module statement ran. Not "collects too often" — the schedule was never a
  schedule there. `Vm::begin_collection_window` is now the one place that starts one, and both
  entry points call it.
- **A graph's chunks were not roots.** This is the real one. A graph is several compiled bodies run
  one after another, so while the first executes, the ones that have not started are reachable from
  nothing `Vm::roots` walked — and their constant tables are Strings. `main`'s `'c'` was freed while
  `dep` was still running, and the answer came back `undefined`. `roots` now walks `self.resolved`.

**The second was invisible at any sensible threshold** and only ever appeared when collecting at
every check, which is why `vm::tests::collecting` forces the schedule rather than trusting a default
to exercise it.

## What it is worth, counted honestly

**+4 conformance runs.** Not the number the first run reported.

A run with the schedule on reports between 310 and 476 newly passing, and repeating it gives a
different figure each time. Taking the intersection of three runs leaves 116 — and **112 of those
116 are `built-ins/RegExp/property-escapes`**, the bucket this project has already parked as sitting
exactly on the ten-second per-test budget. The schedule moves those across the line in both
directions, so they pass sometimes and fail sometimes, and blessing a lucky run put 198 of them into
the ratchet as passes the engine could not repeat. That was caught by re-running rather than by
review, and the entries were put back.

So the conformance ledger reads: four runs genuinely fixed, and a large bucket made *noisier* than
it was. The ratchet holds — those entries stay listed as failures and a run that happens to pass
them reports "newly passing", never a regression — but **the headline percentage now varies by a
couple of hundred runs between invocations, and it did not before.** That is a real cost of this
decision and it is stated here rather than discovered by whoever next wonders why the number moved.

**The conformance number is not why this is on.** `for (i = 0; i < 1e6; i++) s = f(s)` is why. That
program threw before this and runs now, and so does the same loop at five million.

## What follows from this decision

- `Heap::live_footprint` exists beside `Heap::footprint` and answers what is still held rather than
  what has been paid for. DR-0013's budget deliberately goes on using `footprint`: what it bounds is
  a program that *allocates* without end, which is a claim about what has been taken.
- The conformance harness reads `VIPERJS_COLLECT_GROWTH`, so the suite can be run either way and the
  difference read off. The number it reports by default is the engine's default, deliberately — a
  conformance figure that depended on how the run was invoked is what DR-0006 refuses.
- `Vm::collect` is unchanged and stays the host's to call. A schedule is a convenience over it, not
  a replacement: an embedder still knows things the loop does not.
- The interval is DR-0022's — a thousand instructions, between them and never inside one. So this
  bounds a *program*'s growth and not any single operation's.
