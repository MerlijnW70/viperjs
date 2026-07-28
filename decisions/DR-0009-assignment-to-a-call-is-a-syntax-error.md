---
id: DR-0009
title: Assignment to a call expression is a Syntax Error, in sloppy code as in strict
status: prose-only
---

`f() = 1` is a Syntax Error here, and refusing it is a *choice* the specification offers rather
than the only reading of it. §8.6.4 states `AssignmentTargetType` for a `CallExpression` as:

```
1. If the host is a web browser or otherwise supports Runtime Errors for Function Call
   Assignment Targets, then
   a. If IsStrict(this CallExpression) is false, return ~web-compat~.
2. Return ~invalid~.
```

So a host has two conformant answers. One is `invalid` always, which makes `f() = 1` an early
error in both modes. The other is to support the legacy behaviour: in sloppy code the assignment
parses and throws a `ReferenceError` when it runs, which is what browsers have always done and
what the web depends on. Strict code is `invalid` either way — the host option does not reach it.

praxis takes the first, for the reason DR-0008 takes the same side of the same question: this is
not a web browser, and "unless the host is a web browser or otherwise supports X" is a sentence
whose default answer here is no. Taking the second would mean carrying a *runtime* error for a
shape that has no meaning — `f() = 1` cannot succeed under any circumstances, so the only thing
web-compat buys is that the failure arrives later and reads worse.

## What it costs, measured

It is not free, and it is worth being precise about who pays.

- **test262 agrees, and only about strict code.** Every test in
  `test/language/expressions/assignmenttargettype/` that is about a call target is flagged
  `onlyStrict` — `direct-callexpression.js`, the compound-assignment and update forms, and the
  `for`-`in`/`for`-`of` heads. There is no companion test asserting the sloppy case is accepted,
  because there could not be: it is host-dependent. So this decision costs nothing on the
  conformance run, in either direction.
- **Browsers and the engines that follow them disagree**, and one of them disagrees further than
  the specification allows: V8 accepts `f() = 1` in *strict* code too, where §8.6.4 gives it no
  choice. A sweep will therefore find real files this refuses — duktape's `tests/ecmascript/`
  has seven, all of them sloppy-mode tests written for an engine that took the other option.
- **Nothing else does.** Nineteen repositories swept and duktape is the only one that writes it,
  in tests about the behaviour itself.

## The invariant

`AssignmentTargetType` is a function of the expression alone: no strictness, no host flag, no
second answer for the same source. `is_simple_assignment_target` in `src/parser/operator.rs` is
that function, and it stays total and pure. The day this is revisited, it becomes an option on
the embedding surface and not a branch inside the parser — the same shape DR-0006 gives the
nesting limit and DR-0008 gives B.3.
