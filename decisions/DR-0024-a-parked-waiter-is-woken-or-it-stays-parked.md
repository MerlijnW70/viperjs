---
id: DR-0024
title: A parked waiter is woken by a notify, by its timeout, or it stays parked
status: prose-only
---

## Amended again, 2026-08-09: the timeout elapses, and it is not one of the three fakes

**Everything below the next two sections is the record as it stood, and its conclusion is now
wrong.** `Atomics.waitAsync` with a finite timeout settles `"timed-out"`. Worth **+46 runs**, the
whole of test262's `waitAsync` timeout family.

The mistake was in the question. "What is missing is a clock the job queue can wait on" framed this
as needing a *timer* — something that fires while no JavaScript is running — and the three fakes
below are all attempts to build one. None is needed. §25.4.1.6's `TriggerTimeout` does not have to
run at the instant the timeout expires; it has to run before anything can observe that it has not.
The observation point is a **job boundary**, and there are two of them:

- **After each job**, because a program polling with promise jobs keeps the queue non-empty for the
  whole timeout. That is exactly what test262's `atomicsHelper.js` does — its `setTimeout` is a
  `Promise.resolve()` re-`then`ed until `Date.now()` passes — so for every asynchronous Atomics test
  in the suite the engine is already running when the deadline arrives. It only had to look.
- **When the queue empties**, because an agent that simply `await`s its own `waitAsync` has nothing
  to poll with. There the engine sleeps until the earliest deadline, which is neither a busy wait nor
  a delayed job: nothing is runnable, and the alternative is handing the host back a promise that
  provably can never settle. `Vm::wait_for_a_deadline`, and DR-0022's budget bounds it — a run with
  a deadline sleeps no further than that deadline and leaves the waiter parked.

So none of the three fakes applies. Nothing settles early, so no program can measure a lie with
`Date.now()`; nothing spins, so no core burns; and nothing blocks a queue that has work in it. What
the engine gained is not a clock but the habit of asking what time it is at a point where the answer
can change what runs next. The answer is late by at most one job and never early, which is the
direction that matters: `no-spurious-wakeup-*` asserts the elapsed time is *at least* the timeout,
and it measures 200.29 ms for a 200 ms wait.

`vm::tests::shared` is where this is enforceable, and the row that used to say `"nothing"` for a
1 ms timeout now says `"timed-out"` — deliberately, which is what the old record asked for. Beside it
are the polling path and an infinite timeout that still never settles, so the three are not one row
wearing three hats.

**What is still true**: a notify from *another* agent does not reach a promise parked here. That is
the second seam below and it is not a timer, so building this did not build it.

## The record as it stood

§25.4.3.15's `Atomics.waitAsync` answers one of three things: `"not-equal"` if the value has already
changed, `"timed-out"` if the timeout is zero, or a promise that settles when a notify arrives or
the timeout elapses. ViperJS answers the first two exactly and the third **partly**: the notify
works, and the elapsing does not, because there is no timer for it to elapse on.

This records what that costs, why it is bounded to one shape, and what would have to exist to close
it — so that the gap is a decision with a boundary rather than a bug someone finds later.

## Amended: the blocking half now has a clock, and this record is about the other one

When this was written the engine had one agent, so §25.4.3.14's blocking `Atomics.wait` was refused
outright and `waitAsync` was the only waiting there was. A host can start agents now — each its own
thread with its own heap, sharing a `heap::Block` — and for an agent whose `[[CanBlock]]` is true a
blocking wait **does** time out, because a parked thread is a thread a condition variable can wake at
a deadline. That is where `std::time::Instant` earns its third path in the architectural constraints
that confine the monotonic clock to the machine's own deadline.

None of the reasoning below changes, because none of it was ever about the blocking form. What a
blocking waiter holds is a thread, and a thread can be woken at a moment. What an asynchronous waiter
holds is a **promise**, and settling one means running a job — so it still needs something to happen
at a time when no JavaScript is running, and that is still the thing this engine does not have. The
three fakes are still the same three fakes.

Two consequences worth naming, both of them seams rather than gaps:

- **There are two waiter lists now**, split by who is able to end a wait. A blocking waiter lives in
  the block, where any agent can reach it; an asynchronous one lives on the `Vm` that parked it,
  because only that machine can settle its promise. `Atomics.notify` empties both and adds the
  counts, and takes the blocking ones first — a count spent on a promise this agent will settle at
  its leisure, while a thread in another agent stayed parked, is a right number and a hung program.
- **One agent cannot settle another's `waitAsync`.** A notify from agent A leaves a promise parked in
  agent B untouched and uncounted. Closing that needs the same "run me again" hook as the timeout
  does, plus somewhere for agent A to leave the message — so it is the same piece of work and not a
  second one.

## A waiter list is meaningful with one agent, which is the surprising part

DR-0022's neighbourhood already establishes that this engine has a single agent, and §25.4.3.14's
blocking `Atomics.wait` is therefore refused outright: an agent that suspended could never be woken,
because there is nobody else to wake it. That reasoning does **not** carry over to `waitAsync`.

`waitAsync` does not suspend anything. The agent parks a promise and carries straight on, so it
reaches the next statement and may wake its own waiter:

```js
const p = Atomics.waitAsync(i32a, 0, 0).value;   // parks, infinite timeout
Atomics.notify(i32a, 0);                          // the same agent wakes it
```

That is test262's `undefined-for-timeout.js`, and it is why `Vm::waiters` is a real §25.4.1 waiter
list rather than a formality. It is also why `Atomics.notify` cannot answer a constant `+0`: it
counts what it woke, and with waiters parked that number is not zero.

**The list is keyed on a buffer and a *byte* offset**, per §25.4.1, so two views of different
element widths agree about a position: a `BigInt64Array`'s slot 0 and an `Int32Array`'s slot 0 are
the same eight-byte start. An element index would make those two separate lists and a notify through
one view would silently miss a waiter parked through the other.

## What is missing is a clock the job queue can wait on

A waiter with a *finite, non-zero* timeout that nothing notifies should settle `"timed-out"` after
that many milliseconds. Settling it needs something to happen at a time when no JavaScript is
running, and this engine has no such thing: §9.5's queue drains jobs that already exist, and
`run` returns when it is empty.

Three ways to fake it were considered and each is worse than the gap.

- **Settle immediately, on the job queue.** The answer would be right — with one agent a waiter
  that nothing notifies can only ever time out — and the *timing* would be a lie a program can
  measure with `Date.now()` across the `await`. A wrong answer that reads as right is the failure
  mode this repository is built to avoid.
- **Spin until the deadline.** Re-queueing a job until the clock passes is timing-accurate and
  burns a core for the duration; a one-second timeout becomes a one-second busy wait inside `run`,
  which no embedder would accept and DR-0022 exists to prevent.
- **Block the drain.** Sleeping inside the queue makes an *asynchronous* operation synchronous,
  which is the one thing `waitAsync` exists not to be.

So the waiter stays parked, and its promise never settles. A program that awaits it waits for ever
— which is what an infinite timeout does anyway, and is the same shape as any promise nobody
resolves.

## The boundary, stated exactly

Diverges only for: `waitAsync` with a matching value **and** a finite, non-zero timeout, **and** no
notify *from the parking agent* for that position before the run ends.

Everything else is conformant. A zero or negative timeout answers `"timed-out"` without a promise
at all, because `max(q, 0)` is 0 and the clause settles before returning. A mismatched value answers
`"not-equal"` the same way. A notify settles a waiter whatever its timeout was. The count, the
kind check and every coercion run in the clause's order.

`vm::tests::shared` pins both halves: a notified waiter settles with `"ok"`, and an un-notified one
with a timeout of 1 ms is still unsettled after the queue drains. **The second test is what makes
this record enforceable** — building a timer has to change that row deliberately.

## What would close it

A host-driven clock: an embedder-supplied "run me again at time T" hook, which `api::Engine` does
not have and which is the same shape a `setTimeout` would need. That is a larger decision than this
one — it is the difference between an engine that runs code and a runtime that owns an event loop,
and GOAL.md §3 says the host provides I/O. If it is ever built, the waiter list is where the first
caller is.
