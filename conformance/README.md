# conformance/ — the oracle

Everything here exists to answer one question honestly: **how much of JavaScript does praxis
actually implement?** Not our test suite — TC39's, the same one V8, JavaScriptCore and
SpiderMonkey are measured by.

> Status: **not yet built.** This lands at M5 (see [../AGENTS.md](../AGENTS.md)). Until then the
> engine has unit tests only, and any claim about conformance is unsupported.

## Getting the suite

test262 is vendored, never committed:

```
git clone --depth 1 https://github.com/tc39/test262 conformance/test262
```

Record the exact commit you tested against in `expectations.txt`'s header. The suite moves; a
conformance number without a suite revision is not a number.

## The design (build it this way at M5)

**The runner** walks `test262/test/`, parses each file's YAML frontmatter, and honours it:

- `includes:` — harness files from `test262/harness/` that must be prepended (`assert.js` and
  `sta.js` always; `propertyHelper.js`, `compareArray.js` and friends on demand).
- `flags:` — `onlyStrict` / `noStrict` / `raw` / `module` / `async` / `CanBlockIsFalse`. A test
  with neither `onlyStrict` nor `noStrict` runs **twice**, once with a `"use strict"` prologue.
- `negative:` — the test must FAIL, with the named error type, at the named phase (`parse` or
  `runtime`). A negative test that passes is a failure. This is the half of test262 that catches
  a permissive parser, and it is also the half that is easiest to accidentally score backwards.
- `features:` — used for bucketing, not filtering. Never skip a test because it names a feature
  you have not built; that is what expectations are for.

**The expectations ratchet** is the point of the whole directory:

- `expectations.txt` lists every test currently known to fail, one per line, **each with a
  written reason**.
- A test that fails and is *not* listed → the run is RED. That is a regression.
- A test that passes but *is* listed → the run is RED, and the fix is to delete the line. The
  file may only shrink.
- The exit code is what CI reads. A summary printed to stdout is not a verdict.

The reason column is load-bearing. `expectations.txt` is the one place the conformance number
can be quietly laundered — "just add it to the list" is always available and always tempting.
Every entry says why (`# M6: generators not implemented`), and those reasons get
interrogated rather than taken on faith. A reason like "flaky" or "weird" is
not a reason.

## Reporting

The runner prints a summary that is safe to paste into a commit message:

```
test262 @ <suite-commit>   passed 12,481 / 51,208 (24.4%)   expected-fail 38,727   UNEXPECTED 0
```

Bucket failures by `features:` and by directory — that report is how you choose the next
milestone with data instead of intuition, and it is worth building on day one of M5.
