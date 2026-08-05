---
id: DR-0012
title: A String has a maximum length, and exceeding it is a RangeError
status: prose-only
---

§6.1.4 defines the String type as "the set of all ordered sequences of zero or more 16-bit
unsigned integer values (*elements*) up to a maximum length of 2^53 - 1 elements". At two bytes an
element that maximum names sixteen petabytes, so it is not a limit any implementation enforces —
it is the point past which the *type* stops being defined. Every engine has a real limit far below
it, and no two agree: V8 allows about 2^29 elements, SpiderMonkey about 2^30, JavaScriptCore
2^31 - 1.

The specification does not say what an implementation with a smaller maximum should do when a
program asks for more. This record is that decision.

## What ViperJS does

**`MAX_STRING_LENGTH` is 2^28 - 1 code units**, and an operation that would make a longer String
throws a **RangeError** instead.

At two bytes an element the limit is 512 MiB of `u16`. That is chosen from both directions: far
past any string a program means to build, and small enough that an engine which permits a String
*at* the limit can still be expected to allocate it — a maximum whose own allocation is the next
thing to fail would not be a maximum, it would be a different crash.

A `RangeError` because that is what §20.5.5.2 is for — "a value that is not in the set or range of
allowable values" — and because it is what every other engine throws, so a program that handles
the condition at all handles it here unchanged.

## Why there is a limit at all

Not tidiness — DR-0002. Without one, `s = s + s` in a loop is an input that kills the process, and
it kills it in the worst available way: a Rust allocation failure is an **abort**, not a panic, so
it cannot be caught, no destructor runs, and nothing above gets to report what happened. DR-0002
says no input may panic; an abort is strictly worse than the thing DR-0002 forbids.

This was not hypothetical. It is how the finding arrived: the full test262 run aborted twice with
`memory allocation of N bytes failed`, at two consecutive doublings of one growing `Vec`, and
produced no report at all — so a single runaway test took the conformance number down with it.

## The invariant

> Every String on the heap holds at most `MAX_STRING_LENGTH` code units, and every operation that
> would produce a longer one refuses before allocating anything.

*Before allocating* is the operative half. Building the oversized String and then rejecting it
would allocate exactly the memory the limit exists to refuse.

## Where it is enforced, and where it deliberately is not

One door: `Heap::concat`. Concatenation is the only operation that makes a String longer than the
things it was made from — every other String on the heap is a piece of the source text or a
number's spelling, and neither can outgrow the program that asked for it. `Heap::new_string` is
therefore *not* a gate: putting the check there too would put a refusal on forty call sites that
no input can reach, which is a branch no test can kill and a maintenance cost with nothing behind
it. When a future operation can grow a String — template literals, `String.prototype.repeat`,
`Array.prototype.join` — it goes through `concat` or it gets the check, and this paragraph is the
reason that is a decision rather than an oversight.

The decision itself is a free function, `string_fits`, separate from the operation that acts on
it. That is not layering for its own sake: the boundary can then be *asked* at sizes that cannot
be *built*, so the tests that prove the comparison is the right way round cost nothing. A limit
nobody can afford to test is a limit nobody has checked.

## What this does not fix

A cap bounds one String. It does not make the engine's allocation infallible: a program can still
hold many Strings, or many objects, and Rust's `Vec` aborts on failure wherever it is used. Making
every allocation fallible means `try_reserve` and a `Completion` on every path that grows
anything, which is a much larger change and belongs behind a measurement rather than a guess.

What is claimed here is narrower and is the part that mattered: **unbounded** growth from a
bounded program is gone. `s = s + s` now throws where it used to abort.
