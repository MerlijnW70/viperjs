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

## The default is off, and what turns it on

`Vm::new` leaves it `None`. Turning it on fails exactly one test —
`modules::a_module_may_await_at_its_top_level_and_everything_importing_it_waits` — and passes the
other 1,556 plus the whole conformance suite with no regressions.

That one failure is not dismissible and is not yet diagnosed. A module graph is evaluated through
`link_and_evaluate`, which does **not** go through `Vm::run`'s preamble, so two things are true at
once and only one of them is the bug:

- the schedule's own state is never initialised there, so a graph evaluates with a base of zero and
  collects at every check rather than on a threshold;
- and the root set over a *partly evaluated* graph has never been established the way
  `vm::tests::collecting` establishes it for a script — a module body suspended at a top-level
  `await` is a parked execution reached from a record, and nothing yet proves that path.

**The default flips in the commit that answers which.** Shipping it on before then would be shipping
the failure mode this record exists to describe: a wrong value, silently, in the one area where the
tests are thinnest.

## What follows from this decision

- `Heap::live_footprint` exists beside `Heap::footprint` and answers what is still held rather than
  what has been paid for. DR-0013's budget deliberately goes on using `footprint`: what it bounds is
  a program that *allocates* without end, which is a claim about what has been taken.
- The conformance harness reads `PRAXIS_COLLECT_GROWTH`, so the suite can be run either way and the
  difference read off. The number it reports by default is the engine's default, deliberately — a
  conformance figure that depended on how the run was invoked is what DR-0006 refuses.
- `Vm::collect` is unchanged and stays the host's to call. A schedule is a convenience over it, not
  a replacement: an embedder still knows things the loop does not.
- The interval is DR-0022's — a thousand instructions, between them and never inside one. So this
  bounds a *program*'s growth and not any single operation's.
