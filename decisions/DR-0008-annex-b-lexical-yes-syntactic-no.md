---
id: DR-0008
title: Annex B's lexical extensions are implemented and its syntactic ones are not
status: prose-only
---

Annex B is titled "Additional ECMAScript Features for Web Browsers" and is normative optional: a
conforming implementation may take it or leave it, and one that is not a web browser is expected to
leave it. praxis is not a web browser — GOAL.md names the hosts it is for, and none of them is one
— so the default answer to every "unless the host is a web browser or otherwise supports X" is no.

That answer is not applied uniformly, and the line is between B.1 and B.3.

**B.1 is implemented.** It extends the *lexical* grammar: `LegacyOctalIntegerLiteral` (`010`),
`NonOctalDecimalIntegerLiteral` (`08`), `LegacyOctalEscapeSequence` (`'\101'`), and HTML-like
comments. The lexer reads all four and flags the first three for the parser, which refuses them in
strict code where §12.9.3.1 says to.

**B.3 is not.** It extends the *syntax and semantics*: a `FunctionDeclaration` as the body of an
`if` (B.3.2), an initialiser in a `for`-`in` head (B.3.5), a `VariableStatement` naming a catch
parameter (B.3.4), a labelled `FunctionDeclaration` (§14.13.1's carve-out), and duplicate
`FunctionDeclaration`s in a block (§14.2.1's). All are refused.

The reason the line falls there rather than somewhere else is what the two kinds of extension cost
to leave out. Leaving out B.1 would mean refusing a *token* — `010` is a number in every engine and
in every JavaScript written before 2011, and a program containing one is not asking for a web
browser, it is old. Leaving out B.3 means refusing a *program shape*, and every one of those shapes
has a plain equivalent that has always been legal: write the function as a declaration, write the
initialiser as a statement, name the variable something else.

There is a second reason, and it is the one that decides the close cases. Every B.3 rule is
conditioned on strictness as well as on the host, so implementing one means implementing *two*
behaviours for the same source and choosing between them at run time. That is a conformance number
that depends on how the engine was configured, which is what DR-0006 refused for the nesting limit
and refuses here for the same reason.

## B.3.1 is the exception, and the two reasons above are what make it one

`__proto__:` in an object initializer — B.3.1, "The `__proto__` Property Name in Object Initializers"
— **is implemented**, and it is worth spelling out why, because "B.3 is not" reads as covering it.

Neither reason above reaches it.

It is not conditioned on strictness. Every other B.3 rule is, which is the argument that decides the
close cases: implementing one would mean two behaviours for the same source. `({__proto__: p})` sets
the prototype in strict and sloppy code alike, so there is one behaviour to implement and nothing to
choose between at run time.

And leaving it out is not a refusal. B.3.1 extends no grammar: `__proto__: x` is already a
well-formed property definition, so there is nothing to reject without rejecting a legal property
name. What praxis did instead was make an ordinary property called `__proto__` and carry on — a
**silent wrong answer**, which is the one outcome this project ranks below a refusal. Every other
B.3 shape is refused with a span; this one was mis-compiled.

There is a third, smaller reason. `test/annexB/` is excluded from the conformance run, and B.3.1's
tests are not in it: test262 files them under `test/language/expressions/object/`, by grammar
location rather than by clause. So they run, and forty-two of them were expectations entries — the
cost of this one was being paid in the ratchet rather than recorded in a decision.

The line is therefore not quite "B.1 yes, B.3 no". It is: **an Annex B rule that changes the meaning
of a program shape the core grammar already accepts is implemented; one that adds a shape is not.**
That is the same line, stated so that B.3.1 falls on the side it belongs to, and it leaves every
other B.3 rule exactly where it was.

What follows from this decision:

- `test/annexB/` is excluded from the conformance run at M5, in the same breath as `staging/` and
  for a stated reason rather than because it is inconvenient. The expectations file does not carry
  entries for tests we have decided not to run.
- Where a core rule and an Annex B carve-out disagree, the core rule is implemented and the test
  says which shape a web host would accept. Those tests are the record of what this decision costs.
- The decision is reversible with data. If M5's conformance run or an embedder's real code shows
  the cost is higher than this argues, the place to change it is here, and B.3 would then arrive
  behind a host flag — which is the shape the specification already gives it.
- `catch (e) { var e; }` was accepted in the try/catch slice on the narrower argument that
  test262's main tree cannot be asserting the refusal. That argument was sound and is not the
  question this decides; under this policy it is refused, and the slice that flips it says so.
