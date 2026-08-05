---
id: DR-0024
title: A parked waiter is woken by a notify, or it stays parked — there is no timer to time it out
status: prose-only
---

§25.4.3.15's `Atomics.waitAsync` answers one of three things: `"not-equal"` if the value has already
changed, `"timed-out"` if the timeout is zero, or a promise that settles when a notify arrives or
the timeout elapses. ViperJS answers the first two exactly and the third **partly**: the notify
works, and the elapsing does not, because there is no timer for it to elapse on.

This records what that costs, why it is bounded to one shape, and what would have to exist to close
it — so that the gap is a decision with a boundary rather than a bug someone finds later.

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
notify for that position before the run ends.

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
