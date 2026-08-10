---
id: DR-0022
title: A run has a time budget, and exceeding it stops the machine rather than throwing
status: prose-only
---

GOAL.md §2.3 says an embedder "runs untrusted code inside their process", and DR-0002 keeps half of
that promise: no input may panic. Nothing kept the other half. `while (true) {}` ended with the
process, and `run_prepared` said so in as many words — "Nothing bounds how long this runs: a
backward jump is how a loop will be built, and a script that loops forever is a script that loops
forever. DR-0002 is about panics, not about halting."

DR-0021 built the embedding surface and named this as the thing it deliberately did not decide.
This is that record.

## The budget is time, and it belongs to a run

`Vm::set_time_budget(Option<Duration>)`, and each `run` computes its own deadline from it. Not a
deadline the host sets once — that would be an engine that dies at a wall-clock instant rather than
a limit on what a script may take — and not a step count.

**Not steps, though steps are the reproducible thing.** A step budget is not a promise about
wall-clock, and wall-clock is the only question a host actually has: an edge runtime has a request
deadline, a game has a frame. Two scripts that execute the same number of instructions can differ by
orders of magnitude in time — one allocating, one arithmetic — so a step budget generous enough for
the second is no bound at all on the first. The cost of asking the clock is dealt with below.

**Off by default.** `None` is no budget and is what every existing caller gets, so the conformance
suite, the examples and the tests are unaffected. A host opts in.

## Exceeding it cannot be caught

This is the decision the rest follows from. A budget a script can catch is not a budget for
untrusted code: `try { while (true) {} } catch (e) {}` would swallow the refusal and the loop would
resume, and the check meant to stop a runaway would be run again, and again, for ever.

So exceeding the budget is **not a throw**. `Vm::stopped` is set, the interpreter loop reads no
further instruction, and every execution nested inside — a `valueOf` re-entered from a coercion, a
native's callback, a job — returns at once because the flag is checked before an instruction is
read. Nothing a script or a host function does can clear it. `Vm::run` answers `Outcome::Interrupted`
and, because the flag is cleared when a run *begins*, the machine is usable again afterwards.

**Jobs do not run.** §9.5's queue is drained at the end of a run, and a job is code like any other:
a `then` handler that loops forever is the same problem wearing a promise. An interrupted run
answers without draining, and the queue is left as it stands.

This is deliberately unlike DR-0013's heap budget, which *does* throw a catchable RangeError. The
difference is what the two protect: memory is a resource a script can reasonably be told it has run
out of and recover from, and time is the resource that decides whether the host gets control back at
all. A script that catches the memory error still reaches the next instruction, where the check
fires again against a heap it cannot grow. A script that caught this one would reach the next
instruction with time it does not have.

## The check rides the counter that is already there

`execute` already counts down to a periodic check for DR-0013's budget — `HEAP_CHECK_INTERVAL`,
every thousand instructions — because `Heap::footprint` is cheap and not free. `Instant::now()` is
in exactly the same position: tens of nanoseconds, which is nothing once per thousand instructions
and is not nothing on every one.

So there is **one counter scheduling two checks**, renamed to say so. That is not the "one flag
answering two questions" mistake this project keeps meeting: both checks ask the same question of
the counter — *is it time for periodic housekeeping* — and neither reads it for a second meaning.
Two counters would be two intervals that could drift apart, and the second one would be the one
nobody reads.

Resolution follows from the interval: a thousand instructions is single-digit microseconds in a
tight loop, so a budget of a millisecond is honoured to well within itself, and the guarantee stated
below is a bound and not a promise of exactness.

## What this does not stop, stated so nobody assumes otherwise

**The regular expression matcher.** §22.2's backtracking is its own loop and does not read this flag,
so a pattern like `/(a+)+b/` against a long non-matching subject still runs to completion. That is a
real hang a hostile script can cause and this record does not close it. It is a second check in a
second loop, and it wants its own measurement because the matcher's inner loop is far tighter than
the interpreter's — the counter that is free here may not be free there.

**A single built-in that takes a long time.** Sorting a large array, or building a large string,
runs inside one instruction. DR-0013's heap budget bounds the ones that allocate, which is most of
them, and nothing bounds the rest.

> **Amended 2026-08-10: the ones that *walk* are bounded now, and this paragraph was hiding a hole
> rather than describing a limit.** "Nothing bounds the rest" was true and read as a small residue.
> What it covered was every method in §23.1.3 and its neighbours: they are written against an
> array-like's `length`, so `Array.prototype.join.call({length: 2 ** 32 - 1})` is four billion turns
> inside Rust with no instruction boundary in it, and a host that set fifty milliseconds waited out
> all of them. Measured before the fix: thirty seconds for a walk of two hundred million, whatever
> the budget said. After it: under one.
>
> `Vm::interrupted` asks the deadline from inside those loops, on a counter of its own —
> `NATIVE_CHECK_INTERVAL`, ten thousand turns, larger than the interpreter's thousand because a turn
> there is cheaper than an instruction. Every such walk already passed through one function,
> `builtins::array_methods::within_budget`, which is what made this a single change rather than
> thirty-seven — and that function's own doc had named this budget as the answer to a program that
> will not stop, above a check that asked only about the heap.
>
> The stop is still uncatchable: `interrupted` sets the flag, and a `try` around the walk sees
> nothing because the loop's next pass returns before the handler it unwound to can run.
>
> **Found while chasing something the fuzzer turned up on its first real run** — an *abort* rather
> than a hang, a mutated file asking the allocator for 64 GiB in one go, which killed the process
> where `catch_unwind` could not see it.
>
> **That abort is not known to be fixed, and this record said it was.** The sentence here read "the
> same seed completes now, because the allocation was inside a walk the deadline reaches", on the
> evidence that seed 1 aborted before the change and did not after. Checked properly the next hour
> by disabling the new check by hand: **seed 1 does not abort with the fix disabled either.** The
> seed fixes the fuzzer's *inputs* and not the engine's behaviour — `Math.random` is seeded from the
> clock at every `Engine::new` — so the two runs were never the comparison they were read as. The
> abort is an open finding with no reproduction, which is why a finding now writes the offending
> source to disk instead of trusting the seed.
>
> The time-budget hole *is* verified, and by the harder method: disabling the check by hand takes a
> walk of two hundred million from 0.6 s back to 53 s with a 500 ms budget set.
>
> What is still unbounded is a built-in that takes a long time **without** walking a length: a
> sort's comparator loop, a single enormous string build. Those are bounded by the heap budget when
> they allocate and by nothing when they do not.

**A host function.** A `Native` that blocks is the host's own code and the host's own problem; the
engine cannot interrupt Rust it did not write.

**A thread other than the running one.** GOAL.md §3 says one realm, one thread, and this adds no way
for another thread to interrupt a running script. A host that wants that wants a flag it can set
across threads, which is a different shape — an `Arc<AtomicBool>` the loop reads — and a different
record. The time budget covers the case that motivated this, which is a script that will not stop
rather than a host that changed its mind.

## The invariant, stated as narrowly as it is true

While a time budget is set, an interpreter loop reads at most `HEAP_CHECK_INTERVAL` instructions
after the deadline has passed, and then no more until the next `run` — whatever the code does, and
whatever any handler, native or job it reaches would otherwise do.
