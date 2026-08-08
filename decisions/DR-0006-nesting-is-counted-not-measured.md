---
id: DR-0006
title: The parser's nesting limit is a count, not a measurement of remaining stack
status: prose-only
---

The parser refuses to recurse past a fixed number of levels. The obvious alternative — and what
V8 and SpiderMonkey both do — is to measure how much stack has actually been used, by taking the
address of a local and comparing it against the address recorded when parsing began, then
refusing when the difference exceeds a budget. It is a handful of safe lines, it adapts to the
build, and it is the wrong choice here.

It is attractive for a real reason. A count has to be set for the most expensive configuration,
which is a debug build, where Rust gives every local its own stack slot and reuses nothing; a
release build costs several times less per level and is then limited far below what it could
afford. Worse, the count is not stable: every production the parser gains puts another frame
between one bracket and the next, so the number falls as the grammar grows. It has already
fallen from 512 to 128 to 64 across three slices, and it will fall again.

The reason to accept all of that is GOAL.md §4. Conformance is a *number*, measured against
test262 and visible every night, and the whole premise is that it cannot drift into "correct
according to us". A stack-based limit makes which programs parse depend on how the engine was
compiled — the same source accepted by a release build and refused by a debug one, the same
suite reporting two different scores depending on the profile it ran under. A conformance number
that means something requires that the answer not depend on the compiler's mood.

There is a second reason, smaller but real: a count is deterministic and therefore testable.
`parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` runs a full-depth parse in a thread with
exactly one mebibyte, which is a claim that can be checked and that fails loudly when a slice
makes nesting more expensive. A stack-headroom check has nothing equivalent — it cannot be
wrong, only differently generous, so nothing would ever fail and the cost of each production
would go unnoticed.

## Measured against real code on 2026-08-08, and the number is 77

Two published packages now sit past the cap, and the second one puts a figure on it. `three`'s
`draco_decoder.js` — 702 KiB of emscripten output, the WASM glue for its mesh decoder — needs
**77** levels; the cap is 64. Bisected by rebuilding, so it is the file's requirement and not an
estimate. `ajv` was the first, through `new Function` on a generated validator.

**And the default cannot simply rise to meet it.** At 77,
`parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` dies with `STATUS_STACK_OVERFLOW` in a
debug build on one mebibyte — which is the guard doing exactly what this record designed it to do,
and is the whole argument for a count restated as a measurement. 64 is not a cautious number; it is
the number.

So the route left is the third bullet below, which has never been built: **the limit is the
embedder's to set.** A host that knows it has eight mebibytes can afford 77 and the command line
cannot know that on its behalf. What must not move is the *default*, for the reason above the
bullets: which programs parse would otherwise depend on the build, and the conformance number would
mean less.

Two things worth carrying to whoever builds it:

- **Brace nesting in that file is only 38.** It is *expression* nesting that reaches 77 — one
  budget shared between the grammar's recursive paths, as this record says, and expressions are
  what spend it.
- **Raising it is a stack promise, not a flag.** An overflow aborts the process, which DR-0002 says
  no `Result` rescues, so the API has to say plainly that the number is a claim about the caller's
  stack and that the guard test measures only the default.

What follows from this decision:

- The number is measured before it is set, in a debug build, against the smallest stack in common
  use. It is a consequence, not a preference.
- Keeping the recursive path narrow is a correctness activity, not an optimisation. Every
  function removed from between one bracket and the next is nesting depth the parser can afford
  to accept, and the two slices so far that widened it both paid for it immediately.
- The limit becomes an embedder-set value at the M3 API boundary. The embedder is the one who
  knows how much stack they have, and giving them the number is better than guessing on their
  behalf — but the *default* stays conservative, and stays a count.
