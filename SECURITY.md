# Security policy

## Reporting

Report privately through
[GitHub Security Advisories](https://github.com/MerlijnW70/praxis/security/advisories/new).
Do not open a public issue for a vulnerability.

You can expect an acknowledgement within a few days and an assessment within two weeks. If a
fix is warranted you will be credited in the advisory unless you ask otherwise.

## What counts as a vulnerability here

praxis is an engine that runs **untrusted script inside someone else's process**. That framing
decides the answer to most questions:

- **A panic, abort, or stack overflow on any input** — including input no reasonable program
  would produce. Script text is data, never a trusted caller, so a crash is a denial of service
  in the embedder's process and is treated as a defect regardless of how absurd the input is.
- **Unbounded memory or time on small input** — the same reasoning.
- **Anything that escapes the sandbox the embedding API promises**, once that API exists.

Two things are deliberately *not* vulnerabilities:

- **A wrong answer that is merely wrong.** A conformance failure is a bug and belongs in an
  issue — it becomes a security matter only when an embedder's security decision could rest on
  it, in which case say so and it will be treated accordingly.
- **Resource use that the embedder asked for.** A script that legitimately allocates until the
  host's limit is the host's limit doing its job.

Memory-safety bugs of the classic kind are outside the scope of a report because they are
outside the scope of the language: the crate is `#![forbid(unsafe_code)]` and takes no runtime
dependencies, so there is no `unsafe` block and no third-party code to audit. If you find a way
around that — an unsound `std` interaction, a build that slips the attribute — it is very much
a report worth making.

## Supported versions

praxis is pre-1.0 and under active development. Only the latest released version is supported;
before 1.0 that means the tip of `master`.
