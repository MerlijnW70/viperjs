---
id: DR-0028
title: A sloppy function has its own `caller` and `arguments`, which ECMA-262 does not describe
status: prose-only
---

ECMA-262 §10.2.4 `AddRestrictedFunctionProperties` puts `caller` and `arguments` on
**`Function.prototype`** as accessors whose getter and setter are both §10.2.4.1's %ThrowTypeError%.
There is nothing else in the specification about either name. Read literally — and ViperJS did read
it literally — `f.caller` therefore throws for *every* function `f`, because every function inherits
that pair.

This record says why ViperJS now shadows that pair on one kind of function, what the shadowing does,
and exactly where it stops. **It is a deliberate divergence from the specification**, taken on the
user's instruction after being costed and declined once; the case for declining is in
`notes/FINDINGS.md` and is not repeated here.

## What the tests want, and why the row looked like an engine fault

23 test262 runs failed with **ViperJS's own** message, `this property may not be read or written` —
which is the poisoned accessor doing exactly what the clause says. A bucket that fails with the
engine's own error string normally means a bug; here it meant the opposite, and that is the trap
worth recording.

All 23 carry `features: [caller]` and date from ES5, whose §15.3.5.4 gave Function objects a variant
`[[Get]]`: reading `"caller"` threw only when the **value** was a strict function, so a sloppy
function's `.caller` answered with its caller. ES2015 deleted that clause. Every shipping engine kept
the behaviour anyway, and test262 kept the tests behind a feature flag.

Three facts settle that this is an extension rather than a gap:

- `features.txt` files `caller` under *Standard language features*, which is why it reads as
  normative. It is there because the flag is old.
- **The tests say so themselves.** `language/arguments-object/10.6-13-a-2.js` contains
  `if (arguments.callee.caller === undefined) { called = true; // Extension not supported - fake it }`.
  So `undefined` is an accepted answer and a **throw** is not.
- Nothing in the suite requires a *sloppy* function's `.caller` to throw.

## What is built

A sloppy function gets its **own** `caller` and `arguments`, which shadow the inherited pair.

- `caller` answers the function whose call is directly beneath the running invocation of the
  receiver — `null` when the receiver is not executing, when the call came from a script or module
  body, and when the caller is **strict**. The last is ES5 §15.3.5.4's rule kept on purpose: handing
  out a strict function's identity is the hole the clause was deleted around, and `null` rather than
  a throw is what the tests require, since `15.3.5.4_2-75gs.js` calls a sloppy function from a strict
  one and asserts only that nothing was raised.
- `arguments` answers **`null`, always**. See below.
- Both are accessors with a getter and **no setter**, not enumerable, and configurable — so a sloppy
  write is discarded, a strict write throws, and `delete f.caller` uncovers §10.2.4's pair
  underneath. The standard behaviour stays reachable, which is the one thing a divergence like this
  must not take away.
- One getter of each per realm, shared by every function that has the pair, exactly as
  %ThrowTypeError% is shared. A program comparing two functions' descriptors sees one host facility
  rather than a property each function invented.

## Where it stops, and why each boundary is a test rather than a preference

Only an ordinary **sloppy** function declaration or expression. Excluded:

| Kind | What says so |
| --- | --- |
| strict | `built-ins/Function/StrictFunction_restricted-properties.js` — no own property, and both names throw |
| a generator | `built-ins/GeneratorFunction/instance-restricted-properties.js` — the same, and it asks it of `new GeneratorFunction()`, which is **sloppy** |
| bound, built-in, proxy | no body of their own; `built-ins/Function/prototype/bind/S15.3.4.5_A2.js` is what would notice |
| an arrow, `async`, a `MethodDefinition` | untested either way — excluded because the extension is ES5's and none of the three existed to have it |

The generator row is the one that matters most: it is why strictness alone is not the question, and a
host that asked only about strictness would fail that file at the `new Function` site.

The method row has a second witness. `src/vm/tests/names.rs` already asserted that a concise method's
own property names are exactly `length` and `name` — a claim about §15.4.5 — and including methods
broke it. **An extension that breaks a claim about the grammar has been drawn too wide.**

## `arguments` answers `null`, and that is the honest shape

The extension's `arguments` is the arguments object of the executing call. ViperJS cannot name one
from outside the call: the object lives in a slot of the **callee's** function environment, and a
frame records the environment to go *back* to rather than the one it entered. What is reachable is
whatever environment was current when the next call was made, which is a *block* environment as
often as not, so reading a slot out of it would answer with some other variable.

Three answers were considered. `undefined` is what test262 reads as "this host has not got the
extension", which would be a lie now that half of it exists. A best-effort object would be right for
the functions whose bodies happen to mention `arguments` — ViperJS only materialises one then — and
wrong for the rest, which is worse than uniform. `null` is the extension's own answer for a function
with no activation to describe, and this engine genuinely cannot describe one.

## What would close it

A frame that recorded the environment it *entered* as well as the one to return to. That is eight
bytes on every call, and DR-0019 measured a call's retained cost at 74 — so it is a real trade and
not an oversight. Nothing in test262 asks for it.

## What it costs

Two property-table entries on every sloppy function object, and two `define_own_property` calls when
one is made. Strict code — every module, every class body — pays nothing. If that ever shows up in a
benchmark, the alternative is synthesising the pair on read the way a String object's indices already
are, which costs nothing per function and needs the getter reachable from the heap.
