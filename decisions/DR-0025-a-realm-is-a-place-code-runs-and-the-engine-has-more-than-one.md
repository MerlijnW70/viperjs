---
id: DR-0025
title: A realm is a place code runs, and there is more than one of them
status: prose-only
---

`Vm` holds a `Realm` in a field. That is a claim — that there is exactly one set of intrinsics and
every operation may reach for it — and it is the reason **381 conformance runs** stop at
`$262.createRealm is not a function`. Measured 2026-08-07 by intersecting the expectations file with
every test262 file naming `createRealm`; 369 of the 381 fail on that call and nothing else.

That is the largest thing left in the suite that is neither a proposal nor a data table. It is also
the row that shows how a bucket lies: `what was called is not a function` is 776 runs, `AGENTS.md`
called it "now mostly proposals", and **almost half of it is this one absent host function**.

## What the tests actually ask, which is narrower than "two realms"

Nearly every one of the 381 is this program:

```js
var other = $262.createRealm().global;
var C = new other.Function();
C.prototype = null;
var o = Reflect.construct(SomeBuiltin, [], C);
assert.sameValue(Object.getPrototypeOf(o), other.SomeBuiltin.prototype);
```

§10.1.13 `GetPrototypeFromConstructor` step 3 reads `Get(constructor, "prototype")` and gets `null`,
so step 4 runs: `GetFunctionRealm(constructor)`, and the default prototype is **that** realm's
intrinsic rather than the running one's. The assertion is about *which* realm answered, and there is
no way to write it with one.

So what these tests measure is not that two realms can exist side by side. It is that **the engine
never reaches for "the" intrinsics.** Every clause that wants an intrinsic names whose it is — the
running execution context's, or a particular function's — and ViperJS has one place to get them from
and therefore cannot be asked the question.

## Three things in the engine assume there is one, and one of them is already wrong

**1. `Vm::realm` is a field, and it is read in 166 places.** Those reads are correct as *the running
realm* and wrong as *a global*. The distinction has never had to be made because the two coincide.
This is the shape `AGENTS.md` records as the recurring bug here — one field answering two questions —
met before it has cost anything, which is the cheap time to meet it.

**2. Well-known symbols are built per realm, and the Well-Known Symbols table says they are shared
by all of them.**
`Realm::new` calls `heap.new_symbol` for each entry of `builtins::WELL_KNOWN`, so a second realm
would get a *different* `Symbol.iterator`. An object carrying realm A's `@@iterator` would then not
be iterable in realm B — not an error, a silently missing method, which is the worst failure mode
this engine has. `built-ins/Symbol/asyncIterator/cross-realm.js` pins it in one line, quoting the
clause: "Unless otherwise specified, well-known symbols values are shared by all realms."

**This one is a live bug rather than a missing feature.** It is unobservable today for the single
reason that nothing can build a second realm, and it would become observable in the same commit that
lets something try.

**3. `Realm::intrinsics` is a heap watermark, not a set.** It is one `usize`, sealed to
`heap.object_count()` once the realm is built, and `Vm::roots` roots `0..that`. Built second, a
realm's watermark counts everything the *first* realm made **and everything the program allocated in
between** — so DR-0023's collector stays sound and becomes blind: every object older than the second
realm is permanently reachable. A leak proportional to how long the program ran before it called
`createRealm`, which for a test262 file is small and for an embedder is not.

## What is decided

**A realm is named by a `RealmId`, the running one lives on the `Vm`, and the previous one is saved
on the frame.** `Vm::realms: Vec<Realm>` with `RealmId(u32)` into it; `Vm::realm` becomes that index,
and `Frame` gains a `realm` field holding **the caller's**, put back on return.

That last part is the whole of the control-flow change, and it is deliberately the shape already
beside it: `Frame` saves `this_value`, `new_target` and `environment` for exactly this reason — a
call changes them and a return must restore them. A realm is a fourth thing of that kind, and
writing it any other way would be inventing a mechanism where one is in use three fields up.

The `Vec`-and-index is not incidental either. `Realm` is 47 `ObjectId`s and `Copy`; the 166 existing
reads stay one index and one field read, and nothing about them gets slower for a program that never
makes a second realm.

**A function's `[[Realm]]` lives on its `Callable`, not on its object and not in a side table.** It
is a property of code, and code is where it comes from: a `Chunk` is compiled in one realm and every
closure over it belongs to that realm, and a `Native` is installed into one realm by the same call
that builds it. `Bound` and the resumption methods need none of their own: `GetFunctionRealm` step 2
reads the `[[Realm]]` slot and step 3 says a **bound function exotic object** answers by recursing
into its `[[BoundTargetFunction]]` — a Proxy the same way, into its target. Step 4's fallback for
everything else is the *current* realm, which is the running frame's and needs nothing stored.

Not a field on `Object`: that is paid by every ordinary object in every program for a fact only
functions have. Not a `BTreeMap` beside the arena either, which is what DR-0020's imports and
§10.4.6's namespaces do — those are sparse, and this is not: **every** function has a realm, so the
map would be one entry per function and the lookup would sit on `GetFunctionRealm`'s path.

**Well-known symbols move to the `Heap`, made once.** They stop being a `Realm` field. The engine
gains no ability from this; it loses the ability to be wrong about it.

**Each realm records its own slot range rather than a ceiling.** `intrinsics: usize` becomes the
pair the seal actually knows — where the realm's allocation began and where it ended — and
`Vm::roots` walks each realm's own range. Sound and precise for one realm as well as for two, and
the one-realm case is byte-identical to today because its range starts at zero.

**`Vm::create_realm()` is the engine's, and `$262.createRealm` is the harness's.** The engine builds
a realm and answers a `RealmId` and its global; deciding that a `$262` goes on that global is
INTERPRETING.md's business and belongs in `conformance`, exactly as `detachArrayBuffer` and
`evalScript` already do. `api::Engine` gets the same pair, because an embedder who wants a sandbox
per tenant wants precisely this and DR-0021 left it out.

## What is deliberately not decided

- **`ShadowRealm` is not this.** It is a separate proposal with a membrane between the two sides;
  what is here is §9.3's realm, which shares one heap and passes objects freely. The 118 runs that
  say `ShadowRealm is not defined` are not part of the 381 and this record does not reach them.
- **Cross-realm `Proxy` — 37 files — is not costed here.** Recursing into a Proxy's target is one
  line of this design, but whether anything else in `src/vm/proxy.rs` assumes one realm has not been
  measured, and guessing at it is how a record acquires a claim nobody checked.
- **Which realm a *job* runs in.** §9.5 queues a job with the realm that made it, and ViperJS's
  queue holds no realm. **194 of the 195 files are synchronous**, so this is off their path — the
  one that is not is `harness/asyncHelpers-throwsAsync-same-realm.js`, a self-test of the harness's
  own `throwsAsync`, and it is the whole of what this omission costs today. The first *engine* test
  that settles a promise across a realm will cost more, and this is where to start looking.
- **An intrinsic reached through a `Value` that outlived its realm.** Realms are never destroyed
  here, so the question does not arise yet. It would if `Engine` ever grew a `drop_realm`.

## The charter said "one realm", and it was read after the fact

**GOAL.md §3 read "No threads inside the engine. One realm, one thread" when this was built, and
nobody checked it first.** That is the process error, and it is worth recording plainly: GOAL.md is
binding and outranks every other document here, so it is the *first* thing a slice of this size
should be measured against rather than the last.

The line moved on 2026-08-08, and the reasoning is the charter's own rather than this record's
convenience:

- **The bullet refuses threads.** Its bold lead says so and its rationale — "embedders get isolation
  by running more engines" — is about isolation. A second realm shares one heap on one thread: it
  adds no parallelism and grants no isolation, so nothing the bullet exists to prevent is happening.
- **A realm is not ours to refuse.** Everything else §3 names — JIT, Node compatibility, Intl,
  threads, FFI — is *outside* ECMA-262 or a bet about complexity. §9.3 is inside it, and §4 makes
  test262 the arbiter: 381 runs require `$262.createRealm`. A charter cannot appoint an arbiter and
  forbid what it asks for.
- **The five non-negotiables are untouched.** No dependency, no `unsafe`, no panic, no threads, and
  the suite still decides.

**What was rejected: reverting this record.** It would have removed 383 runs of conformant behaviour
to satisfy a phrase whose own bullet is about something else, and left the engine less of a
JavaScript engine than it is — the inverse of the test this repository applies to `Temporal`.
Reverting also would not have fixed the actual mistake, which was the reading order.

## The invariant

**No operation reaches for "the" intrinsics.** Every clause that wants one names whose it is, and
the engine answers from the running frame's realm or from a named function's — never from a field
that means "the engine's". Where the specification does not say which, that is a clause left to
read, not a licence to pick.
