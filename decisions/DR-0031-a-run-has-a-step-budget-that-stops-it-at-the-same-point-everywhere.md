---
id: DR-0031
title: A run has a step budget, which stops it at the same point on every machine
status: prose-only
---

DR-0022 bounds a run by a clock, and a clock is the wrong measure when the answer has to be
reproduced by somebody else. The case that asked for this is a judge: a host runs code it did not
write, against inputs, and publishes whether it passed — and a party who doubts the verdict must
be able to run the same thing and get the same answer. Under a time budget they may not. The same
loop finishes on a quiet machine and is stopped on a busy one, so "it ran out of time" is a fact
about the judge's afternoon rather than about the program.

## What ViperJS does

`Engine::set_step_budget` takes a count, and a run that spends it is stopped exactly as DR-0022
stops one: `Outcome::Interrupted`, nothing in the script can catch it, no `finally` and no job
runs. Only the measure differs.

The count is taken where the clock is asked, and nowhere else. The interpreter's check comes every
thousand instructions and spends a thousand; a built-in's own loop asks every ten thousand turns
and spends ten thousand. So a step is an instruction or a turn of a native walk, the budget is
honoured to within one check interval, and it reaches exactly what DR-0022 reaches — including the
array-like walks that never come back to the interpreter.

The two budgets are independent and may both be set: whichever is spent first stops the run.

## Why it is coarse

Spending one step per instruction would make the count exact and cost a subtraction on every
instruction of every program, including the ones with no budget at all. Spending at the existing
checks costs nothing new between them, and the granularity is the same thousand instructions the
heap and the clock already live with. What a host needs is that the stopping point is a function of
the program, and it is: a fresh engine running the same source stops after the same number of
checks.

## What it does not cover

What DR-0022 does not: the regular expression matcher, which has its own per-attempt bound; a
host function that blocks; and an `Atomics.wait`, which reads no instructions while it waits. A
host that needs a run to be reproducible should also fix what a run can observe of the world —
`Math.random`'s seed, and the clock `Date` reads — because a program that branches on either is
reproducible under no budget.

An engine reused across runs carries its check counters over, so the first check of a later run
comes sooner than in a fresh one. A host that needs identical stopping points runs each program in
a fresh engine, which is also what keeps one program's state from reaching the next.
