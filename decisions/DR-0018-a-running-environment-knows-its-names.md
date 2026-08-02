---
id: DR-0018
title: A running environment knows its names, because direct `eval` asks at run time
status: prose-only
---

praxis resolves a name to a **slot** when it compiles. `binding("x")` walks the compiler's scope
chain and emits `LoadVariable(depth, index)`; the interpreter follows `depth` parent links and
indexes the slots. That is why an environment is a `Vec<Option<Value>>` and a parent link and
nothing else — the names were used up at compile time and never needed again.

Direct `eval` needs them again. §19.2.1.1 gives the evaluated source the **caller's** lexical
environment as its outer scope, and the source only exists at run time — so the compiler that
handles it cannot have seen the scopes it must resolve into. `(function () { let x = 1; return
eval("x"); })()` is the whole problem in one line: `x` is slot 0 of the running environment, the
compiler compiling `"x"` has no idea, and there is nothing in the environment to tell it.

Everything else about direct eval follows from having solved this. §19.2.1.1's fresh declarative
scope is `new_environment(Some(caller), …)`, which the heap already does; the completion value and
the re-entry are `Vm::run_script`, which indirect eval already needed.

## Three ways to answer it, and why this one

**Names on the environment.** Each environment carries the ordered names of its own slots. At a
direct eval the interpreter walks the parent chain, hands the compiler the names level by level,
and `binding` resolves into them exactly as it would for a scope it had compiled itself — because
`depth` counts enclosing scopes and the parent chain *is* those scopes. Chosen.

**Names looked up from the chunk at eval time.** An environment records which `PushScope` made it —
a chunk and an instruction index — and the names are read back from the chunk only when an eval
asks. Cheaper per scope by nothing measurable (both are one refcount bump) and dearer everywhere
else: an environment would hold an `Rc<Chunk>`, so the collector would have to trace *into* chunk
constants from environments as well as from frames, and a missed trace there frees a constant a
later instruction reads. Rejected because it moves a hot correctness question — what keeps a
chunk's constants alive — into a colder, rarer path.

**Dynamic lookup for everything.** Give up slot resolution and make every name a run-time search of
the chain, as an interpreter with no compiler would. This is what makes `with` easy and it costs
every program in the language to make one construct work. Rejected outright; DR-0010's whole shape
is that the common path pays nothing for the rare one.

## The invariant

**A name at index *i* is the name of slot *i*, in every environment that has names** — so a
compiler seeded from the chain emits the same `(depth, index)` the original compiler would have,
and there is no second resolution rule to keep in step with the first.

This was first written as "the same length and in the same order", and the length half turned out
to be a claim worth nothing and costing something. A compiled body's slot count is a *high-water
mark* across every scope inside it, so a function whose nested block needed more slots than it did
gets an environment with slots past its own last name. Making the lengths equal means padding those
with names no source can spell — and a padded entry carries a mutability flag that no input can
distinguish, which is what mutation coverage said by surviving a flip of it. A **prefix** gives the
resolver everything it needs: index *i* is slot *i*, and a slot past the end of the list has no
name and cannot be resolved to, which is the same answer the padding produced with one fewer thing
to be wrong about.

**And every name in the list is in scope for the whole life of the environment.** That is the half
this record originally left implicit and the half praxis did not satisfy — see below, and see the
note at the end of that section for how it was settled. A list that held a name which is in scope
for only part of the environment would need a position to be read against, and an eval has no
position to offer.

The `None` case is deliberate rather than a gap: environments made by the engine for its own
purposes — a bound function's, a job's — hold slots no source named, and a name list for them would
be a list of names no program can write. An eval that reaches one resolves nothing there and
carries on outwards, which is the same answer it would get for a scope that declares nothing.

## The invariant is not reachable from what the compiler holds today, and that is the finding

Written above as though a level's names could simply be read off the compiler's `locals` at the end
of compiling it. They cannot, and reading the compiler rather than assuming is what turned it up.

A lexical scope in praxis does not always get an environment. `leave_scope` "takes every local
declared since `mark` out of scope, **without giving its slot back**" — it sets `live = false` and
leaves the entry where it is. Compiled code is right about this because `resolve` consults `live`
*at the position it is compiling*: after `switch (1) { case 1: let a = 1; }` the name `a` no longer
resolves, and an outer `a` resolves again. Verified, not assumed.

A name list has no position and no `live` flag. Hand one built from that level to an eval and it
resolves `a` after the switch — a binding that is out of scope, answering with whatever the slot
still holds. The engine would be wrong in a way no existing test can see, because **the flattening
is invisible to everything except an eval**.

Eight constructs call `leave_scope`. Two of them open an environment of their own — a block (and
only when it declares something, which is §14.2.2's own rule) and a `for (let …; …; …)` head. The
other six do not: `for`-`in`, `for`-`of`, `switch`, `try`, a class body, and `for await`.

**So the implementation is to stop flattening, not to describe the flattening.** Each of those six
is a scope the specification already gives a record of its own — §14.12.4 for a switch, §14.15.3
for a catch, §15.7.1 for a class body, §14.7.5.7 for the two `for` heads — so praxis is already
non-conforming there in a way that happens to be unobservable, and eval is what would observe it.
Giving them environments is owed regardless; doing it first is what makes the name list truthful.

That reordering is the real cost of direct eval and it was not visible from the outside. Whoever
picks this up should land the six environments as their own slice, with the per-iteration and
per-entry semantics each one implies, and only then attach names.

### That is done, and here is what the names rest on

The six landed. `for`-`in` and `for`-`of` needed nothing — their heads take four `%` slots and put
a `let` in the §14.7.5.7 environment they already build — and `switch`, `catch`, `for await` and a
class body each got an environment of their own. So the pairing the name list depends on is:
**every scope that declares a name a source can spell opens an environment, and the ones that do
not declare only `%` slots.** A block, a `for` head, a `catch` and a class body open one exactly
when they declare something, which is the condition each of them already tested for its own reasons.

The name list therefore does *not* consult the compiler's `live` flag, and cannot: by the time an
environment is closed its level's own `leave_scope` has already run and marked everything in it
dead, so a list masked by that flag is an empty list. What keeps the invariant is the pairing
above and nothing else, which is why a construct that flattens a scope must not be added without
re-reading this.

## What this does not settle

**A sloppy `var` inside a direct eval still has nowhere to go.** §19.2.1.1 puts it in the caller's
*variable* environment, and at the top level of a script that is the global object — which praxis
already does, so it works. Inside a function it is a binding added to a scope whose slot count was
fixed when the function was compiled, and no name list makes a `Vec` longer. That case is refused
by name until environments can grow, and the refusal is checkable in advance: the compiler already
computes `VarDeclaredNames` for the source it is handed.

So this record buys the reads and the writes of bindings that already exist, which is most of what
direct eval is used for, and names the one shape that is still missing rather than guessing at it.

## Why `with` is the same decision

§14.11 needs a scope whose bindings are an object's properties, resolved by name at run time. It is
not this record — an object environment is a different kind of record, not a named slot list — but
it wants the same thing this one establishes: that a name can be resolved against a *running*
scope rather than only against a compiled one. Whoever builds `with` should read this first.
