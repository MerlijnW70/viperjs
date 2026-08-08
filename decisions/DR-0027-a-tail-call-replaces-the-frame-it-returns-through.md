---
id: DR-0027
title: A tail call replaces the frame it would have returned through
status: prose-only
---

`function f(n) { "use strict"; return n === 0 ? "done" : f(n - 1) }` throws a RangeError at
`MAX_CALL_DEPTH`, which is 10,000. §15.10 says it must not: a call in tail position discards the
calling context before it makes the call, so the depth does not grow and the recursion is bounded
only by what the program itself holds. test262 measures it at 100,000 iterations across 33 files,
and ViperJS fails 31 of them with one reason — `too much recursion`.

**The bucket is why this record exists.** Proper tail calls appear nowhere on AGENTS.md's work list,
in any of its revisions; the area was found by sorting the expectations file by reason and reading
what `too much recursion` turned out to be. Everything above it in that sort is a proposal.

## Why this is small here and large in most engines

The usual difficulty is that a JavaScript call is a *host* stack frame, so eliminating one means
teaching a compiler to reuse machine frames. ViperJS has never had that problem and
`MAX_CALL_DEPTH`'s own doc says why: "a call here is a frame *record* and not a Rust frame — the
interpreter's loop stays one loop however deep the JavaScript goes." So the whole of the mechanism
is that `Vm::frames` gains one entry and does not lose one, and a tail call is the arrangement where
it loses one first.

`return_from_call` already does exactly the teardown a tail call needs, for exactly the reasons a
tail call needs it: truncate the operand stack to `frame.stack_base`, truncate the handlers to
`frame.handlers_base`, and put back the environment, the `this`, the `new.target`, the realm and the
code position the frame saved. **A tail call is that teardown followed immediately by an ordinary
entry**, rather than followed by pushing a value.

## The decision

**A `return` whose argument is a call is compiled as a tail call when the source text alone says it
is safe, and the machine refuses it in the two cases only the machine can see.**

### What the compiler decides

Four conditions, and each is a fact about the text:

1. **The code is strict.** §15.10.3 requires a tail call only for strict function code, and every
   one of the 33 test files is `onlyStrict` or turns strictness on in the function it measures.
   `self.chunk.strict` is the whole of the question.
2. **It is not a derived constructor's body.** §10.2.2 step 13 makes a derived `return` run
   `CompleteDerivedReturn` *after* the value is in hand, and code that runs after the call is code a
   tail call has thrown away the frame for.
3. **Nothing is pending that must run on the way out.** This is the interesting one, and it is
   already computed: `unwind_across(Exit::Return)` walks the `Crossing` entries a return leaves.
   `Scope` and `Operand` vanish with the frame and cost nothing; **`Finally` and `Iterator` emit
   code that runs after the argument is evaluated**, and a tail call cannot have any.

   **`Handlers` is the count and not the entry, and this record said otherwise until it was
   built.** The first reading was "a `try` was written here, so refuse", which refuses
   `try { throw 0 } catch (e) { return f(n - 1) }` — and §14.6.1 makes a catch block a tail
   position, with a test262 file that says so. The entry outlives the try block: entering a catch
   sets its count to what is *still armed* there, which for a `try`/`catch` with no `finally` is
   **zero**, because a handler that fires is taken off the stack by the throw that found it. So the
   condition is `armed > 0`, and it separates the try block from the catch block in one comparison
   without either being written down as a case. **A crossing that carries a number is telling you
   something the variant alone is not.**
4. **The argument *is* a plain or method call**, and not merely one somewhere inside it. `?:`,
   `,`, `&&`, `||` and `??` are tail positions in §15.10.2 and are **not** taken here — see the
   omissions below; each needs its branch compiled knowing one arm may never return, which is a
   change to three expression forms rather than to the `return` statement. The condition is
   deliberately the narrow one: `return f(g())` marks `f` and never `g`, because `g` answers into a
   frame that is still needed, and asking whether the argument *is* the call is the reading that
   cannot get that wrong.

**Condition 3 is the whole reason this fits.** §15.10.2's `HasCallInTailPosition` is thirty
productions of static semantics; what it computes, operationally, is "is there anything left to do
in this function after the call answers". ViperJS already keeps that as a list, because a `return`
has to emit it. Deriving the answer from the crossings rather than from the grammar means the two
cannot disagree — and it gets `try { } finally { return f() }` right, which is a tail call, and
`try { return f() } finally { }`, which is not, without either being written down as a case.

### What the machine decides

Two conditions the text cannot answer, because they are about how the *frame* was entered rather
than about what the body says:

- **A construction.** The same body runs for `f(1)` and for `new f(1)`, and only the second has
  `frame.constructed` — §10.2.2 step 13 has to answer with the object that was made, which means
  there is something to do after the call.
- **A suspendable body.** A generator, an `async` function and an async generator all answer with
  something other than the value returned — an iterator result, a resolved promise, a settled
  request — and `suspendable_of` is what tells them apart from an ordinary frame.

In both cases the tail call **degrades to an ordinary call**. That is observable only as the stack
growing, which is what every engine that does not implement §15.10 at all does everywhere.

## What this deliberately does not decide

- **A tail call across a `for`-`of`.** §15.10.2 makes an iteration statement's body a tail position,
  and §7.4.9 makes a `return` out of a `for`-`of` close its iterator — which is work after the call.
  test262 tests `for (;;)` and never `for`-`of`, so the conflict is not measured, and condition 3
  refuses it. A tail call that skipped `IteratorClose` would be trading a clause the language does
  have for one it is unclear about.
- **`CallSpread`.** `return f(...args)` builds its argument list at run time through a different
  instruction. Nothing in test262 asks, and adding a second tail-call path for it would double the
  surface for no measurement.
- **A call written as the bare name `eval` — four runs.** The compiler cannot know whether that name
  holds `%eval%`, and if it does the text runs in *this* frame's scopes, which a tail call has just
  taken down. Refusing it is the honest reading of what the compiler knows; making it work means
  asking the same question the interpreter already asks and only then taking the frame down, which
  is a second teardown site. `language/expressions/call/tco-non-eval-*` is what it costs.
- **The tail positions that are not the whole argument — seven runs.** §15.10.2 makes the branches
  of `?:`, the right of `&&`, `||` and `??`, and the last operand of a comma tail positions too.
  Each needs its branch compiled knowing that one arm may end in a call that never returns while the
  other falls through to the shared `Return`, which is a change to three expression forms rather
  than to the `return` statement. Left for a second slice on purpose: this one is a `return` whose
  argument *is* a call, and that is 21 of the 32 runs.
- **Lowering `MAX_CALL_DEPTH`, or raising it.** The cap is unchanged and still means what its doc
  says. This makes one shape of recursion unbounded; it does not make recursion cheap.
- **Anything about `arguments`, `caller` or a stack trace.** §15.10.4's note is that a tail call
  discards the context, so a trace cannot show it. ViperJS has no traces to lose.

## The invariant

**A tail call leaves `Vm::frames` exactly as long as it found it.** Not shorter, which would return
through a frame that is still needed, and not longer, which is the bug this exists to remove. That
is a structural fact rather than a behavioural one — a program cannot see the length, only the
RangeError that eventually comes of it — so what holds it is a test that reads the depth across a
tail call and not a test that recurses a hundred thousand times and hopes.

The behavioural half is the second invariant and it is the one that says the optimisation is
*correct*: **a tail call answers exactly what the same call would have answered without it.** The
frame it replaces is the one whose return value it becomes, so the caller cannot tell which
happened — and if it ever can, the tail call has skipped something the crossings should have named.
