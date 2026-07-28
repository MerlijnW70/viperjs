---
id: DR-0010
title: A heap value is an index into an arena, not a reference count
status: prose-only
---

`Value` holds four types that fit in a register. The other four — String, Symbol, BigInt, Object
— do not, and something has to say how a `Value` refers to one. There are two shapes on offer and
they are not close.

**Reference counting is not available**, and the reason is not performance. M3 requires a
mark-sweep collector, and `Rc` cannot be one: it frees at a refcount of zero and therefore never
frees a cycle. In most languages cycles are a corner case. In JavaScript they are the *normal*
case — `function f() {}` gives `f.prototype.constructor === f` before a line of user code runs,
every closure that outlives its scope points back into it, and `globalThis.globalThis` is
`globalThis`. An engine that leaks cycles leaks approximately everything.

`Rc<RefCell<T>>` is worse than merely insufficient. Every field access becomes a runtime borrow
check, and a borrow check that fails **panics** — during a `[[Get]]` that re-entered the same
object through a getter, say, which is a shape a script can write deliberately. DR-0002 does not
permit a script to do that.

**So a heap value is an index into an arena the `Heap` owns.** `Value` stays `Copy` and stays
small; the collector can walk every object because the arena is the list of them; nothing is
freed until a sweep says so; and none of it needs `unsafe`, because an index is bounds-checked
and a `Vec` is a `Vec`.

## What that costs, stated plainly

Reading through a handle needs the `Heap`, so operations that touch one take `&Heap` and the
parameter spreads outward from wherever the first one is. That is the price, it is visible in
every signature, and it is the right way round: the heap is the context and a value is data.

## Three details the shape does not settle, and where each is settled

- **One arena per type, not one arena of an enum.** A `StringId` may not address an object, and
  the way to guarantee that is for the two to index different `Vec`s. An untyped handle into a
  tagged arena would make that a runtime question, and a wrong answer would be a type confusion
  the borrow checker cannot see.
- **The index is a `usize`, not a `u32`.** A `u32` halves the handle and would need a check on
  every allocation for an arena that has run out — a branch no test could reach, since four
  billion strings is tens of gigabytes and the allocator gives up long first. A `usize` index
  taken from `Vec::len` is valid by construction and needs no check at all. Narrowing it is an
  M8 experiment with a benchmark, next to NaN-boxing, and for the same reason: `Value` is 16
  bytes either way today, so the smaller handle buys nothing yet.
- **Whether a handle carries a generation is the collector's decision, not this one.** Nothing
  reuses a slot until there is a sweep, so a stale handle cannot exist yet. When the sweep
  arrives it either compacts, keeps a free list, or leaves tombstones, and only then is there
  evidence about what a stale handle would cost to detect.

## The invariant, stated as narrowly as it is true

A handle is meaningful only to the `Heap` that issued it, and giving one to another heap is a
programming error. What is guaranteed is the *bound* on what that error can do: never a panic,
never an out-of-range read, never `unsafe`. An index past the end answers `None`, which is the
position `Span::slice` takes for a span that does not lie in the source it is handed.

It is worth being exact about what is **not** guaranteed, because the pleasant-sounding version
of this sentence is false. A foreign handle that happens to be in range answers with *this*
heap's value at that index — a wrong string, not a detected error. Catching that needs an
identifier on every handle, which is a word per value to detect a mistake a script cannot make:
one realm and one thread (GOAL.md §3) means only an embedder running two engines can produce
one. If that ever stops being true, or an embedder reports it, the identifier is the fix and this
is the paragraph that says so.
