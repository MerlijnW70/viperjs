---
id: DR-0033
title: A module record is filed under its module's environment, found through its code, and lives exactly as long as the module can run
status: prose-only
---

§16.2.1.4 gives every module one record, and §16.2.1.6 reads it to run each body once: a module
evaluated already is not evaluated again, a module that threw throws the same value again, and the
namespace and `import.meta` objects it hands out are the same objects every time. That is the whole
of a record's job, and both of the ways it can go wrong are wrong answers: two modules sharing one
record, and one module given two.

## What was wrong

The machine kept a map from a chunk's **address** to its record, filled by `Vm::run_module` and by
the linker, and never removed anything from it. It is DR-0032's bug a second time.

**An address names a chunk only while the chunk is alive.** A host that runs one module chunk after
another from the same local compiles the second into the bytes the first one had, and the second was
handed the first one's record: already evaluated, holding an environment its own body was not
running in, so `import.meta` was `Fault::ImportMetaOutsideModule`. A graph whose chunk landed on a
freed module's address was found evaluated and skipped, and its importers were bound to the freed
module's environment. The linker's own key was described as sound because a `Graph` holds every
chunk for the whole link; the map outlived the link, and `run_module_graph` replaces a chunk filed
under a specifier it is handed again.

**And nothing ever left the map.** Every module a machine ran kept its environment, its namespace,
its `import.meta` object and everything those reached, for the life of the machine. Two thousand
modules holding a 256-element Array each ran out of a four-MiB budget before the hundredth, however
often the host collected.

## What ViperJS does

**The record is on the heap, filed under the module's environment** — `heap::ModuleRecord`, in a
table beside the environments like §16.2.1.5.2's import bindings and §10.4.6's namespaces. The
environment is what everything still able to reach a module already names: its functions close over
it, its namespace reads it, an importer's binding reaches into it, and a body parked at a top-level
`await` resumes in it. The collector marks a record's objects when it marks the environment, and
drops the record in the sweep that frees it. `import.meta` finds the record from the running code's
outermost environment, as it found the module before, so a function a module made answers with that
module's object after the host has let go of the chunk — as §10.2.1.1 says it must.

**The chunk names its record's environment** — `Chunk::modules`, one entry per machine, beside
DR-0032's template objects and for the same reason: the chunk is the only identity for a module's
code that lasts exactly as long as the code can run, and no other code can be mistaken for it while
the entry exists, because the entry is inside it. Running a chunk again is evaluating the same
module again, which runs nothing. The root set walks a chunk's environments wherever it walks its
constants, so the machine's memo of resolved specifiers keeps a graph's modules for the life of the
machine — §16.2.1.7 requires a specifier to answer the same module every time it is asked — and a
host's chunk keeps its module while the host tells the collector about it.

**The key is the record's.** DR-0020's resolved identity was a second map keyed by address; it is a
field of the record now, set when a graph first reaches the module under a key. A module a host ran
on its own has none until then.

**The machine is named by the global object of its first realm.** The records were the machine's
before and still are — one graph, linked once, however many realms the machine builds — and a second
machine on the same heap keeps records of its own, as it kept a map of its own. A handle is never
issued twice (DR-0019), so a machine that is gone cannot be mistaken for a later one.

## The audit

Every other map and set in the engine keyed by an address or a reusable handle, and why each is
sound — the linker's walks are the only ones that changed.

- **The linker's `seen` lists** (`visit`, `exported_names`, `ResolveExport`'s `resolveSet`) compared
  addresses. They hold the `Rc` now and compare allocations: an allocation cannot be freed while a
  list holding it remembers it.
- **The heap's import bindings and namespaces** are keyed by generation-tagged handles, which cannot
  name a later environment or object. Neither table was ever pruned, which cost nothing while no
  module could be collected and a table growing per module once one could; the sweep prunes both
  now. `heap::collect`'s tests assert it.
- **The intern table** is pruned through the generation-checked accessor already (DR-0019).
- **The symbol registry** is a root, so its keys are never swept while it holds them.
- **An object's property index** is keyed by its own property keys, which the collector marks with
  the object — `everything_an_object_leads_to_survives_with_it` asserts the names survive.
- **The weak collections and finalization registries** prune entries whose key or target was swept,
  in the same sweep.
- **The template-object cache** is on the chunk — DR-0032.
- **The collector's `settled` set** names slots by index, during one mark phase that borrows the
  heap immutably: nothing can be freed or allocated while it exists, and the compiler says so.
- **`for`-`in`'s visited sets** hold property keys for one enumeration. The heap's walk cannot
  collect, and the proxy walk calls traps in a nested execution, where the schedule never collects —
  `a_sort_with_a_comparator_survives_a_collection_in_the_middle_of_it` is that rule under test.
- **A waiter's `Block`** is compared by `Arc` identity, and the waiter holds the `Arc`.
- **The machine's realm table** is indexed by `RealmId` and never shrinks (DR-0025).

## The alternatives

**Keep the map on the machine and key it by environment, pruning dead entries after a collection.**
Then the machine cannot root the records, or they never die, and it cannot leave them unrooted, or a
module's `import.meta` object — which often nothing else names — is freed while the module is still
reachable. The record's objects have to be marked exactly when its environment is, and that is a
thing only the collector can do. It is what filing the record under the environment is.

**Key the records by an `Rc`'s identity, holding a `Weak`.** DR-0032's reason: `Vm::run_module`
takes `&Chunk`, and a host's chunk need not be in an `Rc` at all.

**Bound the map.** Evicting a record whose module can still run hands it a second record, which is
the identity bug on a schedule.

## What it costs

A record found by a chunk the collector was never told about is gone after a collection, and running
that chunk again is a new module. That is the root set's existing contract — DR-0032 states it for a
chunk's constants and template objects — and `Vm::collect` is told the chunk about to run.

`Engine::eval_module` coins a specifier per call and the machine's memo keeps each one, so a long
series of `eval_module` calls on one engine still keeps every module it ran. That is §16.2.1.7's
memo working as specified over a host that never reuses a name, and it is bounded by the number of
specifiers the host hands over rather than by anything this record could decide.
