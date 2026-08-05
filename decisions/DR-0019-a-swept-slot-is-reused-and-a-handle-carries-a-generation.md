---
id: DR-0019
title: A swept slot is reused, and a handle carries the generation that says which value it names
status: prose-only
---

DR-0010 made a heap value an index into an arena and accepted, explicitly, that a swept slot
stays as a `None` for as long as the arena does. It also said the rest was not its decision:
"When the sweep arrives it either compacts, keeps a free list, or leaves tombstones, and only
then is there evidence about what a stale handle would cost to detect." The sweep has arrived and
`lab/NOTES.md`'s `hot-shapes` is the evidence.

## What tombstones cost, measured

**A function call retains 74 bytes that nothing can give back.** That is one
`Option<Environment>` slot; the environment's own storage is freed on sweep and its Strings' code
units are returned to the budget, but `environments.len()` never falls. Against DR-0013's 64 MiB
that is about **900,000 calls before any program dies**, whatever it does with the results. A
`for (let …)` whose body closes over the loop variable retains 671 B a pass and dies at about
100,000 iterations.

The number that settles it is not the leak but the column beside it: `hot-shapes` reports arena
retained per pass *and* arena retained **after a full collection**, and the two are identical in
every row. A collection reclaims nothing here. So this is not a schedule that needs writing or a
root set that needs widening — both of those are done — and no further collector work can move
it. It is the slot.

## Compaction is not available, and the reason is DR-0011

Moving a live value and rewriting every handle to it requires enumerating every handle. ViperJS
cannot: a handle is `Copy`, it lives in `Value`s on the operand stack, in `Vec<Value>` arguments,
and — the case that closes the question — in **Rust locals of a native that re-entered the
interpreter**. DR-0011 makes that re-entry ordinary: a coercion calls `valueOf`, which runs a
program, which may allocate. A compacting collector would have to find the `ObjectId` sitting in
that native's stack frame, and nothing short of a shadow stack or a read barrier can. Both are
larger than the problem.

That leaves a free list, which is what this record decides on — with the part DR-0010 left open.

## A free list alone converts a missed root into a wrong value

Today a root the collector fails to trace is *invisible*: the slot is freed, nothing reuses it,
and a later read finds the same value still sitting there. The bug is real and its symptom is
nothing. With reuse the same bug hands back **a different object of the same shape**, silently —
which is the failure AGENTS.md already names as the worst one this engine can have, and which the
generator work met three separate times.

So the free list is not adopted on its own. **A handle carries the generation of the slot it was
issued for**, the sweep bumps a slot's generation when it puts it back, and a read whose
generation disagrees answers `None`.

The point is what that `None` is *not*: it is not a new failure path. `Heap::object`,
`Heap::string` and `Heap::environment` already answer `Option`, because DR-0010 already had to
answer something for an index past the end. Every caller in the engine already handles it. So a
generation check costs one comparison and adds no error type, no `Result`, and no branch that a
reader has to newly think about — the missed root stops being a wrong value and becomes the
answer an out-of-range handle has always given.

## Index and generation are packed into the word the handle already is

A handle is a `usize` today and `Value` is sixteen bytes. Both must stay that way: a second word
per handle is paid by every value in every program to detect a mistake in the collector. So the
word is split — 32 bits of index, 32 bits of generation.

Neither half is tight. DR-0013's budget cannot hold a million environments, so 2^32 slots is
headroom of four thousandfold, and 2^32 reuses of one slot is more collections than a budget of
this size can produce. But "cannot in practice" is not the guarantee this file is for: **a slot
whose generation would wrap is retired instead of freed** — not returned to the list, left as a
tombstone for the life of the arena. That is one comparison in the sweep, and it makes the
statement below true without an "unless".

## The consequence that is not in the collector

`Heap::interned` maps code units to a `StringId`, and the sweep prunes it with
`retain(|_, id| strings.get(id.index()).is_some())`. **That test becomes wrong the day a slot is
reused.** A swept string's slot comes back as `Some(different text)`, `retain` keeps the entry,
and the next `intern` of the old text hands back a handle to the new one — two different property
keys collapsed into one, silently, which is the exact failure this record exists to prevent
reappearing by another door. The prune must go through the same generation-checked accessor an
ordinary read uses; testing `is_some()` on the raw slot is what makes it a bug.

Any other table keyed by a handle acquires the same obligation. There are two today — the intern
table and §16.2.1.5.2's import bindings — and the rule for both is that a handle held outside an
arena is only meaningful through the accessor.

## The invariant, stated as narrowly as it is true

A handle names the value it was issued for, or it names nothing. It never names a *different*
value. That is what the generation buys and it is the whole of what it buys: a stale handle is
detected, not repaired, and the reader that gets `None` is the one that decides what to do about
it — exactly as for an index past the end.

What is still not guaranteed is what DR-0010 already said is not: a handle from *another* heap
that happens to be in range and to match a generation answers with this heap's value. One realm
and one thread (GOAL.md §3) keeps that out of a script's reach, and the fix if it ever escapes is
still an identifier per handle.

## What this does not fix, so that it is not re-measured

`hot-shapes`'s `element-store-growing` row retains 28 bytes a pass and is *live data* — an array
that really is getting longer. Slot reuse does not touch it and should not. The interpreter's
per-pass times, 156 ns to 1.8 us, are likewise untouched: this record is about a ceiling, not a
speed, and `lab/NOTES.md` has the numbers for both so that the two are not confused again.
