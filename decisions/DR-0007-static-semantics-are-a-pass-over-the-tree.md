---
id: DR-0007
title: Static semantics are computed over the finished tree, not tracked as the parser goes
status: prose-only
---

ECMAScript's early errors are stated in terms of syntax-directed operations — `BoundNames`,
`LexicallyDeclaredNames`, `VarDeclaredNames`, `ContainsDuplicateLabels`, and a few dozen more.
Each is defined piecewise over grammar productions, and each early error is a sentence about the
result. There are two ways to implement that, and they are not equally checkable.

The tempting one is to fold the work into parsing. The parser already walks the tree once, so it
can carry a scope stack, register every name as it binds it, and report a collision the moment it
happens. It is one pass instead of two, the diagnostic arrives at exactly the right token, and no
intermediate list is ever built.

We do the other one: the operations are functions over the AST, in `src/static_semantics.rs`, and
the parser calls them once a construct is complete.

The reason is that the specification is the oracle, and only one of these two can be compared
against it. `var_declared_names` is a function whose body can be read next to §8.2.8 line by line
— this production contributes its `BoundNames`, that one contributes nothing, this one descends
into both branches. A scope stack threaded through fifteen parse functions computes the same
answer by means that appear nowhere in the specification, and the day it disagrees there is
nothing to hold it against. A subtle divergence there is exactly the kind of bug AGENTS.md warns
about: cheap to introduce, and expensive to find three months later through a VM.

It also composes the way the specification does. §14.2.1 (Block), §16.1.1 (Script), §14.12.1
(switch) and §14.15.1 (catch) all state rules about the same two lists, so they are four callers
of two functions rather than four places to get a scope stack right. And `VarDeclaredNames` is
needed again at runtime by `GlobalDeclarationInstantiation` and `BlockDeclarationInstantiation`,
which the parser is long gone by the time of.

What it costs is real and accepted: a second walk of each statement list, and a walk of every
nested statement for `VarDeclaredNames`, once per enclosing block. That is `O(n × nesting depth)`
for a script, with nesting bounded at 48 — so it is linear in practice with a constant nobody
will notice next to a VM. If a benchmark ever says otherwise, the answer is a cache on the tree,
not a scope stack in the parser.

What follows from this decision:

- Early errors are checked when a construct is finished, not while it is being read. A diagnostic
  therefore points at the name it is about rather than at the parser's current position, which is
  why `Declarator` carries a `name_span` separate from its `span`.
- The operations take AST slices and return lists, exactly as the specification does. They do not
  take a parser, know about `ParseError`, or stop at the first problem — deciding which rule was
  broken is the caller's job, because the same list answers several rules.
- Walks over the tree are iterative. The tree's own destructor is already recursive over
  `Block`, and measured against a mebibyte it runs out near 3,500 levels; keeping the operations
  iterative means that stays the single depth limit rather than one appearing per operation.
- A construct's early errors get their own tests, separate from the operations'. The operations
  are tested for what they return; the rules are tested for what they refuse.
