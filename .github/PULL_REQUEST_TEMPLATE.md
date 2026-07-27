<!--
Describe what changed *behaviourally*, not which files moved. If the change is subtle, the
ECMA-262 section it implements is the most useful thing you can write here.
-->

## What changes

## Why, per the spec

<!-- The section number, e.g. §13.15.2. If you could not find one, say so — that is useful too. -->

## Checklist

- [ ] `cargo test --workspace` passes.
- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- [ ] New public items have doc comments, including what they do when the input is nonsense.
- [ ] Tests are named as sentences about behaviour, and each one *fails* if the logic is wrong.
- [ ] No new runtime dependency, and no `unsafe`.
- [ ] Any non-obvious behaviour carries a comment citing its ECMA-262 section.

<!--
Two things reviewers will look for and it is quicker to answer them up front:

  * A test that only passes is worth little. Would each new test fail if the branch it covers
    were wrong? If a branch is genuinely untestable, say so and why — that is a design signal.
  * A test that asserts what the code does, rather than what the spec says, pins a bug in place
    permanently. In spec-sensitive areas, cite the section rather than the observed output.
-->
