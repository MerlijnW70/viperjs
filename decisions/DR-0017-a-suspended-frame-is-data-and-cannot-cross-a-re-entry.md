---
id: DR-0017
title: A suspended frame is data, and it carries no return address
status: prose-only
---

A generator and an `async` function both need one thing: an execution that can be **parked** and
revived later. praxis can do that because `Frame` (`src/vm/call.rs`) is a plain record — a chunk, an
instruction index, a `this`, a `new.target`, an environment id, and two stack marks. Nothing in it
borrows, so a parked execution is a value the heap can hold and the collector can trace. That is
`Suspended` (`src/vm/suspend.rs`): the record plus the two stack *slices* the marks already
delimited.

The invariant is what it does **not** hold:

> **A parked execution has no return address.** `Suspended` keeps where the body had got to and
> nothing about who was waiting for it. The caller's code, instruction and registers stay in the
> frame, which is discarded; `Vm::revive` writes a fresh one from wherever the revival happens. So a
> suspension is portable — it may be parked in one execution and revived in another, and the two
> need have nothing to do with each other.

## Why that is the whole of it

A `yield` leaves an iterator result where the *resumption's* answer goes, exactly as a `Return`
leaves a returned value: the call being answered is `gen.next()`, and it is answered. What happens
to the parked body afterwards is a separate question with a separate answer, which is why the two
can be separated at all.

The handlers are the one thing that has to be adjusted, and only because a [`Handler`] names an
absolute depth: they are stored relative to the frame's own two floors and rebased on the way back
in. Everything else in the record is already independent of where it was.

## Two things this record used to say, and neither was true

Recorded because both were written down with confidence and both cost a session.

**"A suspension may not be reached through a re-entry."** DR-0011's nested execution is a Rust call
waiting mid-instruction, and the fear was that parking through one would strand it. It does not: the
Rust call is waiting for a *value*, the suspension leaves one, and `nested_body` reads it and
returns normally. The parked body is revived later from somewhere else entirely, which it can be
because of the invariant above.

**"Only the frame a nested execution entered may not be parked."** The second attempt, narrower and
still wrong for the same reason. It was checked in code, and the check refused ordinary programs:

```js
const it = g();
[1].map(it.next.bind(it));      // TypeError, and should be the iterator result
arr.sort(it.next.bind(it));     // TypeError, and should sort
({ valueOf: it.next.bind(it) }) + 0;   // TypeError, and should be "[object Object]0"
```

All three work with the check removed, and the whole suite stays green. What misled both attempts
was reading `Frame` as one thing: it holds the *caller's* state and the *callee's* together, and
only the second half is the execution. Once the halves are separated the question answers itself.

## What this still rules out

An implementation of `yield` as a Rust-level coroutine, a thread, or a stack copy. All three make
the suspension a thing on the Rust stack rather than a value, which is what would make the return
address real and the portability above impossible — and all three cost either `unsafe` or a runtime
dependency, which GOAL.md does not allow. The frame stays data.

## What it costs

`Vm::park` is `O(1)` in the operands and handlers it takes with it, and a revival is `O(1)` in the
ones it puts back. `Suspended` owns two `Vec`s, so a generator that suspends inside a deep
expression pays for that expression's operands once per suspension — which is the same allocation an
engine that copied a stack segment would make, and unlike that one it is bounded by what the body
actually built rather than by the stack's size.
