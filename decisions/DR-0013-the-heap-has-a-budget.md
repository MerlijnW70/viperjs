---
id: DR-0013
title: The heap has a budget, and exhausting it is a RangeError
status: prose-only
---

`while (true) { ({}); }` allocated until the process died. Not slowly and not subtly: measured at
193 bytes an iteration, it reached **86 GB** and took the machine with it.

That is the failure DR-0002 has no answer for. A Rust allocation failure is an `abort`, not a
panic: nothing catches it, no destructor runs, and no layer above gets to say what happened. The
engine has to stop before the allocator does.

## Why the collector is not the answer here

ViperJS has a mark-sweep collector. Nothing calls it — `collect.rs` says so, and defers the
question of *when* to a later milestone. The obvious reading is that this record should be that
decision instead. It should not, and the measurement is why.

DR-0010 buys never-dangling handles by **never reusing a slot**: a sweep empties a slot and leaves
the hole, and the arena only grows. `Option<Object>` is 96 bytes whether or not an object is in it.
So against 193 bytes an iteration, collecting perfectly would still leave 96 — a factor of two on
something that is unbounded, which is not a fix for something unbounded. For Strings it is better
(16 bytes a slot against 108) and still linear.

A collection policy is worth having and is not this. Bounding the arena needs slot reuse, slot
reuse needs the generation counter DR-0010 costed out, and none of that is required to stop a
script from killing the host.

## What ViperJS does

**`MAX_HEAP_BYTES` is 64 MiB.** Between instructions, the interpreter asks `Heap::footprint`, and
a script that has spent the budget is thrown a **RangeError** it can catch.

Checked between instructions rather than at each allocation. The allocating functions answer
handles, not completions; making forty of them fallible for a condition the loop can see from one
place would put a refusal on every one of their callers, for a case none of them can do anything
about. `MAX_CALL_DEPTH` is already this shape.

Asked once every thousand instructions rather than every one. The check is cheap but not free, and
a loop body of three instructions should not pay for it three times. A thousand objects is under a
hundred kilobytes of overshoot against a 64 MiB budget.

## The number, and why it is not rounder

`Heap::footprint` is an **estimate**: three arena lengths and a running total of String units, all
`O(1)` because the interpreter asks it between instructions. It leaves out the storage an object's
own properties take, and for element-heavy programs that is most of the cost — `while (true) { []; }`
was measured at four times its reported footprint.

So the budget carries that factor as headroom. At 64 MiB reported, the runaway shapes above were
measured at 98 MB to 241 MB of real memory before being stopped, rather than 86 GB.

Two things follow, and both are deliberate. The budget is a bound on the *shape* of failure —
a script that allocates in a loop is stopped — rather than a precise ceiling. And the number is
low compared to a real engine's heap, which is honest: without a collection policy ViperJS cannot
run a long program under any budget, and this is the first number to raise when it has one.

## The invariant

> A script cannot make the engine allocate without bound. Every path that allocates repeatedly
> runs through the interpreter's loop, and the loop refuses before the allocator does.

## What this does not fix

A single operation that allocates hugely in one step is bounded by its own rule, not this one —
that is what DR-0012 is for. A program that legitimately holds a great deal of live data meets the
budget too, and gets a RangeError where a collecting engine would carry on; that is the cost of
having no collection policy yet, stated plainly rather than discovered later.

A loop that allocates *nothing* — `while (true) { i = i + 1; }` — is not stopped and should not be.
It costs CPU and no memory, and an engine that refused it would be an engine that refused a loop.

## Note added 2026-08-05: the premise of the measurement above has changed

The argument in "It should not, and the measurement is why" rests on a sentence that is no longer
true: *"DR-0010 buys never-dangling handles by never reusing a slot: a sweep empties a slot and
leaves the hole, and the arena only grows."* **DR-0019 reuses the slot** — a free list plus a
generation on every handle, in `src/heap/arena.rs` — so "bounding the arena needs slot reuse, slot
reuse needs the generation counter DR-0010 costed out" names a prerequisite that has since been met.

**Nothing in "What ViperJS does" changes, and that is the point of writing this as a note rather than
an amendment.** `MAX_HEAP_BYTES`, the between-instructions check and the RangeError are all
untouched, because `Heap::footprint` counts `slots.len()` — a high-water mark that reuse stops
*growing* and does not refund. A budget measured that way answers the same for the same program.

What is now open, and was closed when this record was written, is whether the interpreter should run
a collection on a schedule. The numbers that said no — 318 conformance files losing their time budget
to buy six passes — were taken when a collection could not reclaim an object's slot at all. They have
not been taken since. `lab/NOTES.md`'s `hot-shapes` is the experiment and it predates DR-0019 as
well; the comment in `Vm::execute` beside the budget check says the same thing at the site.
