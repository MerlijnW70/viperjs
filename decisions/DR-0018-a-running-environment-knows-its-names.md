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

**An environment's name list and its slot list are the same length and in the same order, or the
environment has no name list at all.** A name at index *i* is the name of slot *i*, in every
environment that has names — so a compiler seeded from the chain emits the same `(depth, index)`
the original compiler would have, and there is no second resolution rule to keep in step with the
first.

The `None` case is deliberate rather than a gap: environments made by the engine for its own
purposes — a bound function's, a job's — hold slots no source named, and a name list for them would
be a list of names no program can write. An eval that reaches one resolves nothing there and
carries on outwards, which is the same answer it would get for a scope that declares nothing.

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
