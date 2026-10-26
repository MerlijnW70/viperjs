---
id: DR-0032
title: A template object is cached on the code of its site, and lives exactly as long as that code
status: prose-only
---

§13.2.8.3 `GetTemplateObject` caches one object per realm and per Parse Node: the same tagged
template evaluated twice hands its tag the same frozen Array, and two sites hand out two, however
alike their text. That identity is the only thing about the object a program can see that its
contents do not already say, and a tag that uses it as a key into a table of its own depends on it.

## What was wrong

The machine kept the cache, a map from a `TemplateSite` — the running chunk's **address** and the
site's index — to the object, and rooted every value in it for the life of the machine. Both halves
of that were wrong, and the first was a wrong answer rather than a cost.

**An address names a chunk only while the chunk is alive.** An `eval`'s chunk is freed when the
`eval` returns, a `new Function`'s when the function is swept, and a host's whenever it lets go of
one. The next chunk is often allocated into the same block, and it was handed the earlier site's
object, with the earlier strings. Of 3,000 `eval`s in a loop, each of a tagged template with text
of its own, 2,999 came back with the wrong strings; a host compiling three scripts in a loop was
told `first` three times.

**And nothing ever left the map.** A program that keeps compiling sites — an `eval` in a loop, a
template engine built on `new Function` — kept two Arrays and their properties per site for ever,
and under a four-MiB budget ran out some seventy thousand sites in.

## What ViperJS does

**The cache is on the chunk.** `Chunk::reached` holds an entry per site and realm that has been
evaluated, and `Chunk::names` reports the objects in it. So the object is reachable exactly when
the code that can evaluate the site again is, whatever holds that code: a function object, a frame,
a module record, a host's `Rc`, or a `Chunk` on a host's stack. The chunk *is* the Parse Node, so
the identity cannot be reused while the entry exists — the entry is inside it.

When the code goes, its entries go with it and the next collection sweeps the objects with
everything else the code alone was holding. A program that still has an object keeps it, as it
keeps any other; it simply no longer has a site that could hand it out again.

The realm is named by its global object, not by `RealmId`: the id is an index into one machine's
table, and two machines may share a heap. DR-0019's generation means a handle is never issued twice,
so a realm that is gone cannot be mistaken for a later one.

## The two alternatives

**Keep the map and key it by an `Rc`'s identity, pruning entries whose code has died.** A `Weak`
would hold the allocation and so the address, and `strong_count` would say when to prune. But
`Vm::run` takes `&Chunk`, and a host's chunk need not be in an `Rc` at all — the tests run chunks
that are plain locals. A key that only some chunks can have is not a key.

**Bound the cache and charge it to the heap budget.** A bound has to do something when it is
reached. Evicting an entry whose site is still live hands that site a second object, which is the
identity bug again on a schedule; refusing makes a correct program fail; and charging a cache that
never shrinks to the budget turns a leak into a slower RangeError. Collection with the code is
exact, costs an empty `RefCell<Vec>` on a chunk that has no site, and asks nothing of the
collector that it did not already do for the chunk's constants.

## What it costs

A chunk is no longer immutable once compiled: running a site writes to it. The `RefCell` is the
whole of that, and neither borrow outlives the method that takes it, so two cannot overlap and no
input can make one panic. An entry is three words, and there is one per site per realm that ran it,
which is the bound.

A chunk the collector cannot see is a chunk whose template objects it frees, exactly as it frees
that chunk's constant Strings. That is the root set's existing contract — `Vm::collect` is told the
chunk about to run — and not a new one.
