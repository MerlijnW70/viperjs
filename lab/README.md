# lab/ — experiments before commitment

The engine is held to a hard bar: zero dependencies, no panics, every branch proven tested, a
conformance number that may only go up. That bar is right for code that ships and **lethal for
thinking**. You cannot explore a value representation, race two parser designs, or find out how
slow a naive property lookup really is, if every keystroke owes a test.

So: the lab. It is a workspace member, not a dependency. Nothing here is probed, linted, or
released, and the engine cannot import it — the arrow points `lab -> praxis`, never back.

## The rules (there are only three)

1. **Anything goes in here.** Dependencies, `unsafe`, dead ends, hardcoded paths, `println!`
   debugging, code you would be embarrassed to show anyone. That is the point.
2. **Nothing leaves without a verdict.** When an experiment ends, write its entry in
   [`NOTES.md`](NOTES.md) — including, especially, the ones that failed. An experiment whose
   result is only in your head will be re-run by someone in three months, and that someone is
   probably you.
3. **Code is re-implemented on the way out, never copied.** A lab spike is evidence that an
   approach works. The engine version is written fresh, to the engine's bar, with its own
   tests. Copy-pasting a spike into `src/` is how unprobed logic gets in wearing a disguise.

## Workflow

```
cargo run -p praxis-lab                # list experiments
cargo run -p praxis-lab -- value-repr  # run one
cargo test -p praxis-lab               # lab has its own tests, held to no standard
```

`cargo test` at the repo root does **not** see any of this: the root package is
the engine alone. That separation is the whole design — check it stays true if you ever touch
the workspace layout.

## What belongs here

- **Spikes** — "can a bytecode VM do this at all?" Throwaway by construction.
- **Races** — two designs, one benchmark, one winner. Record the numbers in NOTES.md.
- **Reference oracles** — a slow, obviously-correct implementation to differential-test the
  fast engine version against. These are worth keeping around long-term.
- **Corpus tooling** — scripts that chew through test262 output, bucket failures by feature,
  and tell you what to build next. This is how you decide the next milestone with data
  instead of vibes.
- **Scratch JS** — put throwaway `.js` files in `lab/scratch/` (git-ignored).

## What does NOT belong here

Anything the engine needs in order to work. If the engine cannot run without it, it is not an
experiment — it is a feature, and it goes in `src/` with the tests to match.
