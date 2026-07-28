# conformance/ — the oracle

Everything here exists to answer one question honestly: **how much of JavaScript does praxis
actually implement?** Not our test suite — TC39's, the same one V8, JavaScriptCore and
SpiderMonkey are measured by.

From here this is also what decides what gets built next. Before it existed, "what should I work
on" was a judgement call; now it is a number with a work list attached.

## Getting the suite

test262 is vendored, never committed:

```
git clone --depth 1 https://github.com/tc39/test262 conformance/test262
```

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
7679 passed, 539 failed, 84943 not run
93.44% of what ran — 8.24% of the whole suite
```

Both percentages, always. The first flatters an engine that declines most of the suite, and it
*falls* every time the engine learns to compile something new — a run where more tests execute is
a run where more of them fail. The second is the honest conformance figure and the one to quote.

Then the buckets:

```
what stopped the rest, commonest first:
   73357  a reference to an undeclared name is not implemented yet
   10737  an async test reports through $DONE, which needs a host function
     830  modules are M7
```

That list is the reason the directory exists. A bucket with seventy thousand runs behind it is
the next milestone; one with four is not.

## Passed, failed, and not run

A test **passes** when it did what its frontmatter said it would: ran to the end without throwing,
or — for a `negative` test — failed *in the phase it named* with the error it named. A `parse`
test that throws at run time has not passed, however loudly it failed; the program should never
have begun. This is the half of test262 that catches a permissive parser, and it is also the half
that is easiest to accidentally score backwards.

A test is **not run** when the engine declined it before anything executed: a construct the
compiler has not been taught, a module, an agent test. Nothing ran, so nothing can be said about
what it would have done. Counting those as failures would write the same sentence into the
expectations file tens of thousands of times and bury the entries that mean something.

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
Every entry says why — `:: it threw a RangeError` is a reason, and those reasons
get interrogated rather than taken on faith. A reason like "flaky" or "weird" is
not a reason. Which is why `--bless` rewrites the file wholesale and nothing else may add a line:
a harness that could write its own excuses would not be a ratchet.

## What is excluded, and why

- `staging/` — proposals that have not landed. Not normative, so failing them says nothing.
- `intl402/` — ECMA-402, a different specification. praxis implements ECMA-262.
- `*_FIXTURE.js` — imported *by* module tests rather than run. `INTERPRETING.md` names them.

## A test that will not stop

`while (true);` is a legal program, and Rust cannot stop a thread that will not stop. A worker
that runs past its budget is abandoned: its test is recorded as having timed out, a replacement
worker takes over the queue, and the old thread runs until the process ends. That is why the run
finishes by leaving rather than by joining — there may be threads that never join, and waiting for
them would be waiting forever for an answer already recorded.
