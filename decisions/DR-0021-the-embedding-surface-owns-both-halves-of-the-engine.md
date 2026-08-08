---
id: DR-0021
title: The embedding surface owns both halves of the engine, and a value it hands out is not a root
status: prose-only
---

`AGENTS.md` has named `api.rs` — "the embedding surface" — in its module order since the plan was
written, and it has never existed. The engine's public surface is instead its own internals:
`heap`, `compile`, `parser`, `realm`, `vm`, each `pub` so that `examples/` and `conformance/` can
reach them. That was right while the only embedders were ours. It is not a surface anyone else can
build on, and this record settles what one looks like.

## What an embedder cannot do today, measured

A twenty-line program that runs a script and does something with the answer was written against the
public surface and compiled. Four of its six lines do not:

```
error[E0624]: method `to_string` is private          // a value out, as Rust text
error[E0624]: method `get_property_key` is private   // a property of a result
error[E0624]: method `call_value` is private         // a JavaScript function, called from Rust
error[E0599]: no method named `set_deadline`         // stopping a script that will not stop
```

And there is no way at all to **bind a host function**. `Native` is a public type and `NativeCall`
can already read its arguments, but every path that installs one — `builtins::define_method` and
its neighbours — is `pub(crate)`. The evidence that this is a real gap rather than an aesthetic one
is our own harness: `conformance` is a host, and it binds `$262.detachArrayBuffer` by *writing
JavaScript source*, because no API exists to bind a Rust function instead.

GOAL.md §1 names the target embedder — "an edge runtime, a plugin host, a game" — and §3 says "The
host provides I/O; we provide the language". Providing I/O is exactly the thing that cannot be done.

## The decision

**One type owns the `Heap` and the `Vm` together, and it is the whole public surface for running
code.** `api::Engine`, and the internals stay where they are.

Owning both is not tidiness, and it is the reason this is a decision rather than a patch. Today a
`Vm` and a `Heap` are separate objects and *every* operation takes both:

```rust
vm.run(&chunk, &mut heap)
```

Nothing says they belong together. Two heaps and one machine compile perfectly, and the result is
silently wrong rather than refused: a `Value::Object(id)` is an index, so handing a `Vm` the wrong
heap reads *some other object* at that index — or, since DR-0019, nothing at all. Neither is an
error anyone gets told about. Owning both halves makes the mismatch unrepresentable, which is a
safety property in the sense DR-0002 cares about even though no `unsafe` is involved.

It also settles the visibility question underneath the four errors above. `Vm::to_string`,
`get_property_key` and `call_value` stay `pub(crate)`; `Engine` wraps them. Making them `pub` would
export the two-object shape and the mismatch with it.

## A value handed to the embedder is not a root

`Value` is `Copy` and an object one is an arena index. `Engine::collect` roots what the *program*
can still reach — §9's execution contexts, the realm, the module registry — and a `Value` sitting in
a Rust local is none of those. So:

**A value outlives a collection only if something the program can reach also names it.**

**And the surface has to say so, because the heap does not.** This record's first draft claimed
DR-0019 made the rule safe on its own — a handle to a swept slot names nothing, so a read "answers
nothing rather than something else". The test written for that claim failed, and what it measured is
worse than either half:

```
read gave Ok: undefined
```

`[[Get]]` on an object that is no longer there degrades to `undefined`, which is *exactly* what an
absent property gives. DR-0019 does prevent a wrong value — the slot's generation has moved on, so
nothing else is read — but "no such property" and "the object you are asking about is gone" arrive
as the same answer, and a host cannot tell them apart. Silence is the wrong failure for a boundary.

So `Engine` checks liveness on every value the host hands *in* — the receiver, the callee, `this`,
and each argument — and answers `Error::Collected`. Each handle-carrying `Value` variant is asked,
not only objects: a String, a Symbol and a BigInt are swept on the same terms.

The measurement also killed a tempting shortcut. A value the host has just been handed *appears* to
survive a collection, because the machine still names it — §14.2.2's completion register is a root,
and so is the operand stack. Which of those holds a given value is an artefact of how the last
script happened to end. **Nothing in this record promises it, and a test that relied on it would be
pinning an accident**; the tests do fifty allocations first so that they measure the rule.

The escape hatch is the one the language already has, and the API exposes it rather than inventing a
second: put the value somewhere reachable — a property of the global object — and it is rooted for
as long as it is there. A rooted-handle type was considered and rejected for now: it is a second
lifetime discipline for the embedder to learn, and the measurement that would justify it (how often
an embedder holds a value across a collection) does not exist. `Engine::collect` is the embedder's
to call, so the window is theirs to choose.

## What this deliberately does not decide

**Interrupting a script that will not stop.** *(Decided by DR-0022 on 2026-08-05 — see the
amendment below. What follows is what this record said, and it named where the answer would hang.)*
There is no deadline, no fuel, no interrupt — a `while (true) {}` can be ended only by ending the
process, and GOAL.md §2.3's promise that "an embedder runs untrusted code inside their process" is
only half kept: the no-panic invariant stops a crash and nothing stops a hang. The heap has
DR-0013's budget; time has no equivalent. That is a change to the interpreter loop rather than to
its surface — a check per backward jump, and a decision about what the check costs — so it is its
own record and its own measurement, and this one does not prejudge it. `Engine` is where it will
hang when it arrives.

**Turning Rust values into JavaScript ones beyond the primitives.** No serialisation, no derive, no
struct mapping. The host builds what it needs from the operations here.

**A second realm, or more than one `Engine` sharing anything.** *(Half decided by DR-0025 on
2026-08-07 — see the amendment below.)* GOAL.md §3 says one realm, one thread, and isolation comes
from running more engines. Two `Engine`s share nothing, which falls out of each owning its own heap
and is worth saying because it is the property that makes that advice true.

## Amended twice, and both were things this record said it was not deciding

Recorded here rather than left to be inferred from a later file, because a "deliberately not
decided" that has since been decided reads as a live gap and sends the next reader looking for
work that is done.

**`Engine::set_time_budget` exists — DR-0022, 2026-08-05.** A run has a wall-clock budget and
exceeding it is **not a throw**, because a budget a script can `catch` is not a budget. So the
paragraph above is history: `while (true) {}` can be ended without ending the process, and §2.3's
promise is kept on both halves. What is still outside it, and measured rather than asserted, is a
§22.2 match already running and a host function that blocks.

**A second realm exists — DR-0025, 2026-08-07.** `Vm::create_realm` builds a whole second set of
§9.3 intrinsics on **one** `Engine`, sharing its heap. That is *half* of what the paragraph above
refuses and not the other half: two `Engine`s still share nothing, and that is still the property
that makes "run more engines" the isolation advice. A realm is not isolation — it shares a heap and
passes objects freely — which is why the two coexist rather than one replacing the other.

**And GOAL.md §3 moved with it.** The line the paragraph above cites read "One realm, one thread"
and now reads "One thread, and no parallelism inside it"; DR-0025 records why, and that the charter
was read after the work rather than before it.

## The invariant, stated as narrowly as it is true

An operation reachable from `Engine` reads and writes exactly one heap: the one that `Engine` owns.
No `Value` obtained from an `Engine` is meaningful to another, and DR-0019's generations mean the
consequence of trying is a refusal rather than a wrong answer.
