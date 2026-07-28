# DR-0011 — A coercion re-enters the interpreter, and that re-entry is bounded separately

## The decision

`ToPrimitive` of an object calls a method, so a value operation must be able to run JavaScript.
It does that by starting a **nested execution**: a real Rust call into the same interpreter loop.
The depth of that nesting is counted in `Vm::reentries` and refused past `MAX_REENTRY_DEPTH`
(200), which is a different limit from `MAX_CALL_DEPTH` (10,000) and deliberately far below it.

A nested execution also carries a **floor** — an index into the handler stack and into the frame
stack — below which it may not unwind.

## Why re-entry at all

The interpreter's main loop does not recurse. A call pushes a `Frame` and the loop goes round
again, which is why ten thousand nested JavaScript calls cost ten thousand small structs and no
Rust stack. That is the whole reason the call limit can be a *number* rather than a guess about
the host's stack.

A coercion cannot work that way, because the answer is needed **in the middle of an instruction**.
`a + b` has one operand already on the stack and cannot finish until the other is a primitive.
Suspending and resuming the instruction would mean giving every operator a continuation — a
record of which method it had already tried, for which operand, at which stack slot — and the
alternative is one Rust call that returns a value. The second is the boring implementation.

## Why the limit is a different number

Because the resource is different. A JavaScript call costs a `Frame`, and ten thousand of those
are ordinary in ordinary code. A coercion costs a **Rust frame**, and the host's stack is not
ours to spend: praxis runs inside somebody else's binary, on a thread whose stack size it did not
choose. Two hundred is a depth no hand-written program reaches and a depth no plausible stack
cannot hold.

Sharing one limit would mean either allowing ten thousand Rust frames — a crash — or refusing
ordinary recursion at two hundred. Neither is acceptable, so there are two numbers.

The refusal is a **RangeError**, catchable like any other. DR-0002 says no input may panic, and a
program that nests conversions deeply is input.

## Why the floor

A method called by a coercion may throw. That throw has to come back to the operator that asked
for the conversion, because the Rust call which started the nested execution is still on the
stack waiting for an answer.

Without a floor it would not. `unwind` walks outwards looking for the innermost handler, and a
`try` in the *caller* is a perfectly good handler — so the throw would jump into the caller's code
and carry on executing there, inside the nested loop, with a Rust frame stranded beneath it. The
program would keep running and the interpreter would be wrong in a way nothing reports.

So a nested execution records how many handlers and frames existed when it began, and a throw
that reaches those indices stops. It travels the rest of the way as `Abrupt::Thrown`, and the
operator's ordinary `settle` hands it to the caller's handler from the outside, where the Rust
call has already returned.

## The invariant this implies

**Anything that re-enters the interpreter must set a floor and count the re-entry.** There is one
such place today, `Vm::call_value`, and everything that needs to call JavaScript from Rust — a
getter, `Array.prototype.map`, `Symbol.hasInstance`, a Proxy trap — goes through it rather than
entering the loop itself.

Two things are restored by hand around a nested execution: the environment and the `this`. A
`Return` restores them from the frame it pops, so the ordinary path needs no help; a throw that
nothing caught does not pop frames one at a time, and without putting them back the caller would
carry on running in the callee's scope. That was a real bug, found by a test that read a variable
after catching a conversion's throw.
