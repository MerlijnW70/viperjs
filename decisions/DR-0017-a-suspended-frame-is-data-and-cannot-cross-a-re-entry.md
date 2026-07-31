---
id: DR-0017
title: A suspended frame is data, and the frame a re-entry entered may not be parked
status: prose-only
---

A generator and an `async` function both need one thing: an execution that can be **parked** and
revived later. praxis can do that because `Frame` (`src/vm/call.rs`) is a plain record — a chunk,
an instruction index, a `this`, a `new.target`, an environment id, and two stack marks. Nothing in
it borrows, so a parked execution is a value the heap can hold and the collector can trace. That
is `Suspended` (`src/vm/suspend.rs`), which is that record plus the two stack *slices* the marks
already delimited.

The invariant this record fixes is the other half of that, and it is the one a reader will not
guess:

> **The frame a nested execution entered may not be parked.** A nested execution (DR-0011's
> re-entry) is a Rust call waiting mid-instruction for an answer — a coercion, a proxy trap, a
> `sort` comparator. Parking the frame it entered hands control straight back to that call, which
> then reads the suspension's value as though the function had *returned* it.

## It is one frame, not the whole nested execution

The first way this was written down said no suspension may be reached through a re-entry at all,
and that is too strong: it refuses ordinary programs. `[1].map(() => gen.next())` reaches a
generator's body through `map`'s callback, which is a nested execution — but the generator's frame
sits *above* the callback's, so parking it returns to the callback in the usual way, `next`
answers, and the callback returns to `map` having really returned. Nothing is stranded.

What the rule refuses is `arr.sort(gen.next.bind(gen))`, where the resumed body **is** what the
comparator call entered. There the park pops the only frame the nested execution has, the loop
falls off the end of its empty root chunk, and the comparator receives the yielded value as its
answer. That is reachable from ordinary JavaScript, which is why the check has to exist rather than
being a property of the grammar.

## Why the language mostly guarantees it anyway

§27.5.3.7's `GeneratorYield` suspends *the generator's own execution context*, and a `yield` is
syntactically only ever in the body of the `function*` that owns it. The same holds for `await`. So
the specification never asks for a suspension inside a coercion's own body: a nested execution is
entered from a *native* — `ToPrimitive`, a trap, a callback — and a native's body is Rust.

What the grammar does **not** rule out is the case above, where the thing a native calls is itself
the resumption of a generator. That is the gap the check closes.

## What has to be true in the code

`Vm::reentries` is non-zero exactly when a nested execution is running, and `Vm::floor.frames` is
the frame depth it started at. Together they say whether the frame just popped was the one that
execution entered:

```rust
if self.reentries > 0 && self.frames.len() == self.floor.frames { … }
```

Refusing is a `Fault` — an engine bug the types cannot encode — rather than a JavaScript error,
because nothing a program can write should reach it once the machinery above answers first: §27.5's
state machine can refuse a resumption, and a comparator that resumes a generator will get an
iterator result rather than a stranded frame.

Note what the fault does *not* do. A nested execution answers with a completion, so a fault met
inside one becomes a TypeError on the way out rather than escaping past the Rust call that is
waiting. That is deliberate and it is why the check has to be here: by the time the fault would be
visible as a fault, the frame it was about is gone.

Writing the check down is the point. Without it the invariant is a sentence in a commit message,
and the first native that grows a callback into generator code breaks it silently.

## What this rules out

An implementation of `yield` as a Rust-level coroutine, a thread, or a stack copy. All three would
make the suspension work *through* a re-entry and so would remove the reason to state this — and
all three cost either `unsafe` or a runtime dependency, which GOAL.md does not allow. The frame
stays data.
