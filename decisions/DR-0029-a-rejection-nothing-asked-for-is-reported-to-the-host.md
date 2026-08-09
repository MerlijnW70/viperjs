---
id: DR-0029
title: A rejection nothing asked for is reported to the host, because it is the engine's only signal that a job drain stopped early
status: prose-only
---

DR-0002 says no input may panic. DR-0022 says no run may hang. Between them they describe the two
ways an engine can fail that anybody thought to guard against, and on 2026-08-09 a third was found
that is neither: **the job queue emptying early**.

The shape, exactly. A promise chain re-arms itself — `p.then(step)` from inside `step`, which is
what every polling loop in JavaScript is and what test262's `atomicsHelper.js` builds its
`setTimeout` out of. It allocates, so eventually DR-0013's heap budget refuses. That refusal is a
catchable RangeError thrown wherever the allocation was, which here is **inside a job**, and §9.5
step 3 says a host discards a job's completion. So the rejection goes to the promise `then`
answered with, nothing is waiting on it, the queue empties, `Vm::run` returns `Outcome::Value`, and
the process exits zero.

The program stopped at 38,174 of 200,000 turns and every observable said it had succeeded.

## Why nothing could have caught it

Not for want of checking. `Vm::stopped` covers DR-0022, `Fault` covers a malformed chunk, and a
throw that escapes a *script* reaches the host as `Outcome::Thrown`. A job is the one execution
whose failure the specification instructs the host to drop, and it drops it into the one place
ViperJS did not keep: §27.2.6's `[[PromiseIsHandled]]`.

That slot was deliberately left out, and the note saying so was right about everything except how
long it would hold. `heap::promise` read: "§27.2.6 lists it and nothing in the language reads it.
Its one use is §27.2.1.7 step 7's `HostPromiseRejectionTracker`, and ViperJS has no such host hook,
so the slot would be written and never read." Every clause of that is true. What it missed is that
the tracker is not only a convenience for reporting a program's bugs — it is the **only** channel
through which the engine's own refusal can escape a job.

## What is decided

**§9.13's `HostPromiseRejectionTracker` is built, and `[[PromiseIsHandled]]` with it.** Two calls,
where the clauses put them:

- §27.2.1.7 step 7 — a promise is rejected while `[[PromiseIsHandled]]` is false. Recorded.
- §27.2.5.4.1 step 12 — `then` sets the slot, and if the promise was already a recorded rejection
  the record is taken back. This is §9.13's `"handle"` operation and it is not optional: without it
  `var p = Promise.reject(1); p.catch(f);` reports a rejection that the next statement handles, and
  two lines of ordinary JavaScript would make the list worthless.

**The question is `[[PromiseIsHandled]]`, not "is the reject list empty".** `p.then(f)` registers a
fulfil reaction *and* a pass-through reject one, so a promise with a handler and a promise without
one both have non-empty lists. Asking the list would report every link of every chain. Asking the
slot reports the chain's *end*, which is the one that actually lost the value.

**Recorded, not reported.** The host reads `Engine::unhandled_rejections()` after a run, because a
rejection is unhandled only once the queue has drained and nothing can still attach a handler. The
list is cleared when the next run begins, for the reason `Vm::stopped` is: it describes a run, and
the machine is reusable afterwards.

**It is a warning and never a status.** `viper` prints each to standard error and still exits zero.
An unhandled rejection is ordinary in a program that fires and forgets, and an engine that refused
those would refuse scripts every other engine runs. The exit status stays a claim about whether the
*script* completed.

## What this does not do

**It does not make the failure impossible, and it is not a substitute for not having it.** The
underlying bug was that the collector never ran during a job drain — DR-0023's amendment — and that
is fixed. This is the instrument that would have shown it on the first run rather than the
hundredth, and the reason to have it is that the next silent stop will have a different cause.

**It does not report a queue that empties for other reasons.** A drain that ends because every
promise is legitimately pending for ever — an `await` on something nothing will settle — leaves no
rejection and produces no warning, correctly: that is a program waiting, not a program stopped.
DR-0024's parked `waitAsync` with an infinite timeout is exactly this shape and must stay silent.

**It is not `unhandledrejection`.** There is no event, no `preventDefault`, and no second chance for
a host to mark one handled. Those are HTML's, not ECMA-262's, and GOAL.md §3 says the host provides
what a host provides. The list is enough to build any of them on.

## The reusable finding

**An engine can fail by succeeding, and this repository's three ratchets do not look for it.**
Mutation coverage asks whether a branch is tested. The conformance file asks whether a test that passed
still passes. The no-panic invariant asks whether an input crashes. A run that returns the right
kind of answer, having done a fraction of the work, satisfies all three — and it was found only
because a test262 test happened to assert something about a program that had to keep running.

Worth asking of any new refusal path: **if this fires inside a job, what says so?**
