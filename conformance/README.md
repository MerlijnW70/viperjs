# conformance/ — the oracle

Everything here exists to answer one question honestly: **how much of JavaScript does ViperJS
actually implement?** Not our test suite — TC39's, the same one V8, JavaScriptCore and
SpiderMonkey are measured by.

From here this is also what decides what gets built next. Before it existed, "what should I work
on" was a judgement call; now it is a number with a work list attached.

## Getting the suite

test262 is vendored, never committed:

```
git clone --depth 1 https://github.com/tc39/test262 ../test262
export TEST262=../test262        # or pass --test262 on every run
```

**Clone it outside the working tree.** `conformance/test262/` is gitignored and the runner has
always taken a path, so either location works for the suite — but mutation testing copies the whole
working tree into up to nine throwaway worktrees, and test262 is 42,000 files. Measured on this
machine: 42.7 seconds to copy it with an eight-thread copy against 0.3 seconds for the whole of
`src/`, which is six to nine minutes of file copying before the first mutant compiles. Keeping it
outside is the difference between a mutation run you wait for and one you plan around.

The runner records the checkout's commit in `expectations.txt`'s header and says so when a later
run disagrees. The suite moves; a conformance number without a suite revision is not a number.

## Running it

```
cargo run --release -p conformance -- --test262 conformance/test262
```

`--release` is not a nicety: the suite is ~48,000 files, and a debug build spends its time in the
engine rather than in the harness.

| Option | What |
| --- | --- |
| `--test262 <path>` | The checkout. `TEST262` in the environment does the same. |
| `--only <substring>` | Run just the matching files, and do **not** judge the ratchet. |
| `--expectations <path>` | The ratchet file. Defaults to `conformance/expectations.txt`. |
| `--bless` | Rewrite the ratchet file from this run. A deliberate act — see below. |

## What a run says

```
78222 passed, 14619 failed, 320 not run
84.25% of what ran — 83.96% of the whole suite
```

Both percentages, always. The first flatters an engine that declines most of the suite, and it
*falls* every time the engine learns to compile something new — a run where more tests execute is
a run where more of them fail. The second is the honest conformance figure and the one to quote.

Then the buckets:

```
what stopped the rest, commonest first:
    170  the RegExp modifiers proposal is not implemented yet
     92  a property of strings is not implemented yet
     18  agents are not implemented
     16  `super` outside a derived constructor is not implemented yet
```

That list is the reason the directory exists. It used to be the whole work list — the top bucket
once had seventy thousand runs behind it and named the next milestone. It is now 320 runs in total,
and everything left in it is a proposal or something a single-threaded engine cannot do, so the
work list has moved to the **failures** instead. Sort those by reason to find the next slice:

```sh
grep -av '^#' expectations.txt | sed 's/.* :: //' | sort | uniq -c | sort -rn | head -25
```

## Passed, failed, and not run

A test **passes** when it did what its frontmatter said it would: ran to the end without throwing,
or — for a `negative` test — failed *in the phase it named* with the error it named. A `parse`
test that throws at run time has not passed, however loudly it failed; the program should never
have begun. This is the half of test262 that catches a permissive parser, and it is also the half
that is easiest to accidentally score backwards.

A test is **not run** when the engine declined it before anything executed: a construct the
compiler has not been taught, a syntax from a proposal, a test needing `$262.agent`. Nothing ran, so
nothing can be said about what it would have done. Counting those as failures would write the same
sentence into the expectations file and bury the entries that mean something.

This column used to hold tens of thousands of runs and now holds 320. **A shrinking "not run" is
not automatically progress** — a construct wrongly recorded as declined passes every negative test
that asserts it must be rejected, so moving a test out of this column and into `failed` is the
honest outcome and moving it into `passed` may not be.

A file with neither `onlyStrict` nor `noStrict` is **two** tests. §11.2.2's strict mode changes
what the same source means, and a name that did not say which mode it was would hide half of any
disagreement between them.

`features:` is read but never used to filter. Skipping a test because it names something you have
not built is what expectations are for.

## The expectations ratchet

`expectations.txt` is the point of the whole directory. Each line is one failing run and what went
wrong:

```
language/expressions/delete/11.4.1-5-a-28-s.js (strict) :: it threw the string oops
```

It is checked in both directions on every run:

- a test that fails and is **not** listed → the run is RED. That is a regression.
- a test that passes but **is** listed → the run is RED, and the fix is to delete the line.

The second half is the mechanism, and it is the half people find surprising. If passing tests
could stay listed, the file would only ever grow and the number it guards would stop being a
number about the engine. A line whose test no longer exists, or is now skipped, is reported the
same way — a line about nothing is a line to delete.

A failure whose **reason changed** is reported too, even though pass/fail did not move. A test
that started failing differently is a different fact, and the sentence written down is now about
something that is no longer happening.

The exit code is what CI reads. A summary printed to stdout is not a verdict.

The reason column is load-bearing. `expectations.txt` is the one place the conformance number
can be quietly laundered — "just add it to the list" is always available and always tempting.
Every entry says why, in the engine's own words: `:: it threw a RangeError` is a reason, and
"flaky" or "weird" is not one. **Nothing checks a reason automatically** — an earlier version of
this file claimed a tool interrogated them, and no tool can: they are prose in a `.txt`. What keeps
them honest is that `--bless` rewrites the file wholesale and nothing else may add a line, so every
*added* entry has to be read by whoever added it.

The check worth making by hand: an added line whose path was already listed is an honest reason
rewrite, and one with a genuinely new path is the ratchet moving the wrong way.

## The number is not stable to the last hundred runs, and that is not a bug in the engine

Roughly 900 files — most of `built-ins/RegExp/property-escapes` — take close to the ten-second
per-test budget, so they cross it in either direction depending on what else the machine is doing.
Consecutive runs of the same commit can differ by a few hundred in "newly passing".

Two rules follow, and both were learned by getting them wrong:

- **Take the intersection of three runs before deleting anything from `expectations.txt`.** Blessing
  a lucky run once put 198 unrepeatable passes into the file; the next run reported all 198 as
  regressions.
- **Never bless to make a shuffled reason string go away.** The entries are failures either way, and
  a bless that only rewrites reasons has changed nothing while looking like progress.

Regressions themselves are stable — a test that really breaks breaks on every run — so a red build
is still a red build. It is the *gains* that need corroborating.

## What is excluded, and why

- `staging/` — proposals that have not landed. Not normative, so failing them says nothing.
- `intl402/` — ECMA-402, a different specification. ViperJS implements ECMA-262.
- `*_FIXTURE.js` — imported *by* module tests rather than run. `INTERPRETING.md` names them.

## A test that will not stop

`while (true);` is a legal program, and Rust cannot stop a thread that will not stop. A worker
that runs past its budget is abandoned: its test is recorded as having timed out, a replacement
worker takes over the queue, and the old thread runs until the process ends. That is why the run
finishes by leaving rather than by joining — there may be threads that never join, and waiting for
them would be waiting forever for an answer already recorded.
