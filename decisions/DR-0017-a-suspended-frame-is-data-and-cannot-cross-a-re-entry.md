---
id: DR-0017
title: A suspended frame is data, and a suspension may not cross a re-entry
status: prose-only
---

A generator and an `async` function both need one thing: an execution that can be **parked** and
revived later. praxis can do that because `Frame` (`src/vm/call.rs`) is a plain record — a chunk,
an instruction index, a `this`, a `new.target`, an environment id, and two stack marks. Nothing in
it borrows, so a parked execution is a value the heap can hold and the collector can trace.

The invariant this record fixes is the other half of that, and it is the one a reader will not
guess:

> **A suspension point may only occur where the Rust stack below it belongs to the interpreter
> loop.** No `yield` and no `await` may be reached through a nested execution (DR-0011's
> re-entry). If one ever could, parking the JavaScript frame would leave a Rust frame behind that
> nothing can revive.

## Why the language already guarantees it

§27.5.3.7's `GeneratorYield` suspends *the generator's own execution context*, and a `yield` is
syntactically only ever in the body of the `function*` that owns it. The same holds for `await`.
So the specification never asks for a suspension inside a coercion, a proxy trap, or a
`sort` comparator — every one of which praxis reaches through a re-entry.

That is a claim about the grammar rather than about this implementation, which is what makes it
safe to rely on. The parser refuses `yield` outside a generator body and `await` outside an async
one; a nested execution is entered from a *native* — `ToPrimitive`, a trap, a callback — and a
native's own body is Rust.

## What has to be true in the code

`Vm::reentries` is non-zero exactly when a nested execution is running. A suspend instruction is
therefore only reachable with `reentries` at the value it had when the generator's frame was
pushed. That is not currently checked anywhere, and the check is cheap: refusing to suspend across
a re-entry is a `Fault` — an engine bug the types cannot encode — rather than a JavaScript error,
because a program cannot write one.

Writing the check down is the point. Without it the invariant is a sentence in a commit message,
and the first native that grows a callback into generator code breaks it silently: the generator
appears to suspend, the Rust frame under it returns, and the revival resumes into a stack that is
no longer there.

## What this rules out

An implementation of `yield` as a Rust-level coroutine, a thread, or a stack copy. All three would
make the suspension work *through* a re-entry and so would remove the reason to state this — and
all three cost either `unsafe` or a runtime dependency, which GOAL.md does not allow. The frame
stays data.
