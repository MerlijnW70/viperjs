---
id: DR-0020
title: A module specifier is resolved against its importer, and the host says what it resolved to
status: prose-only
---

`ModuleLoader::load` takes a specifier and nothing else, and `Vm::run_module_graph` looks a
specifier up exactly as the source wrote it. Both are wrong in the same way, and pointing the
engine at somebody else's library is what showed it — see `lab/NOTES.md`'s `run-module`.

## What a real library does that this cannot express

three.js writes `import { clamp } from './MathUtils.js'` in `src/math/Vector3.js` and
`import { warn } from '../utils.js'` in `src/math/Quaternion.js`. A relative specifier means a
different file depending on **which module wrote it**, and two directories that both say
`./index.js` mean two different files. praxis has nowhere to put that:

- `load(specifier)` is not told who is importing, so a host cannot resolve a relative specifier
  even for a dynamic `import()`. It can only guess, and guessing consistently is impossible.
- `self.resolved` maps the specifier text to a chunk, so the second `./index.js` overwrites the
  first. There is no error; there is a wrong module.

three.js's math tree happens to be one directory, which is why it links at all. Anything larger
does not, and the target embedder GOAL.md §1 names — "an edge runtime, a plugin host, a game" —
is loading from directories on the first day.

**The specification already has the parameter praxis is missing.** §16.2.1.7 is
`HostLoadImportedModule(referrer, specifier, hostDefined, payload)`. The referrer is the first
argument, and it is there for exactly this reason.

## The decision

**The loader is handed the referrer, and answers with a key as well as a chunk.**

```
fn load(&mut self, referrer: Option<&str>, specifier: &str, heap: &mut Heap)
    -> Result<(String, Rc<Chunk>), String>;
```

`referrer` is the key of the module doing the importing, or `None` for an entry point and for a
`import()` written at the top level of a Script. The `String` the host answers with is the
module's **resolved identity** — an absolute path, a URL, a package name, whatever the host uses
to tell two modules apart. The engine does not parse it and does not care what it is; it only
compares them.

The engine then keeps two maps instead of one:

- `resolved: key -> chunk`, which is what a module *is*.
- `edges: (referrer key, specifier) -> key`, which is what a specifier *meant* where it was
  written.

Every lookup that today reads `resolved[specifier]` becomes `resolved[edges[(here, specifier)]]`,
and every one of those call sites already has the importing chunk in hand — that is why this is a
mechanical change to nine lines and not a redesign.

## A pre-supplied `Graph` keeps working, and that is not an accident

A host that hands over every chunk up front — which is what `Vm::run_module_graph` is for, and
what the engine's own tests use — registers each under the specifier it supplied, and every edge
from any referrer resolves a specifier to itself. That is precisely today's behaviour, so a flat
graph of unique names goes on linking exactly as it did.

So the two shapes coexist deliberately: **a `Graph` is for a host that already knows the whole
program**, and the **loader is for a host that discovers it**. Neither is a special case of the
other, and a host may use both — the graph seeds what it has, the loader answers for the rest.

## What this deliberately does not do

**No resolution algorithm.** The engine does not join paths, does not know what `..` means, does
not read `package.json`, and does not implement Node's algorithm or the browser's. Resolution is
the host's, because the host is the only thing that knows whether a specifier is a file, a URL, a
bundle entry or a name in a map. This record adds the *parameter* that makes resolution possible
and nothing more.

**No requirement that keys look like anything.** A host that wants today's behaviour answers with
the specifier unchanged, and it works.

## The invariant, stated as narrowly as it is true

Two specifiers that the host resolves to the same key name the same module, and two that resolve
to different keys name different ones. §16.2.1's "each body once" is a fact about the *key* and
never about the text, so a module reached by four different relative spellings still evaluates
once — which is the thing the flat map could not promise and the reason it was a wrong answer
rather than a missing feature.
