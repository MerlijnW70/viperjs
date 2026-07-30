---
id: DR-0016
title: Jobs run inside `run`, and nowhere else
status: prose-only
---

§9.5 says a promise job runs "when there is no running execution context". That is a statement
about the *host*, not about the engine: the specification hands the job back and the host decides
when the stack is empty. praxis has to make that decision, and this is it.

**A script's jobs are drained at the end of `Vm::run`, before it returns.** After the last
instruction, before the stack-balance check, before the completion value is handed back.

## Why not leave it to the embedder

Because then `Promise.resolve(1).then(f)` would never call `f` in the common case, and an engine
whose promises silently do nothing is worse than one with no promises at all. An embedder that
wants to interleave jobs with its own event loop will need a way to ask for one turn at a time;
that is an addition, and it can be added without changing what `run` does today.

## What this makes true, and what it costs

A script's **completion value is decided before any job has run**. §14.2.2 settles it with the last
statement, and jobs are after that — so nothing a `then` handler does can be seen through the value
`run` answers with. That is not a limitation of the implementation, it is what the ordering means:
by the time a handler runs, the script that would have observed it is over.

The consequence for tests is that observing a promise takes two scripts in one realm — one that
sets things going and one that asks afterwards. `run_settled` in the VM tests is exactly that, and
test262's async tests are the same shape wearing a different hat: they report through `$DONE`,
which is a side effect and not a completion value, for this reason.

## The invariant

- Jobs run only between the end of a script and the answer `run` gives for it. Never during
  compiled code, never during a native built-in, never across an embedder's call boundary.
- A job's abrupt completion is discarded (§9.5 step 3). Everything a promise was waiting for has
  already been settled by the job before it could throw, so the only completions dropped are ones
  nothing was waiting for.
- The queue belongs to the `Vm` and not to the `Realm`. A job is work in progress, like a frame; a
  fresh machine over the same heap starts with nothing waiting.
- **A waiting job is not yet a collector root.** Nothing collects while one waits — DR-0013 refuses
  a heap that has grown too far rather than collecting — so this is latent and not a bug. It is the
  first thing that must become a root when the collector is wired to the interpreter, and a
  reaction holds a handler and a capability that nothing else need be holding.
