# Security policy

## Reporting

Report privately through
[GitHub Security Advisories](https://github.com/MerlijnW70/viperjs/security/advisories/new).
Do not open a public issue for a vulnerability.

You can expect an acknowledgement within a few days and an assessment within two weeks. If a
fix is warranted you will be credited in the advisory unless you ask otherwise.

## What counts as a vulnerability here

ViperJS is an engine that runs **untrusted script inside someone else's process**. That framing
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

## Acknowledgements

People who have reported a vulnerability here, with thanks. A report that arrives privately, with
a reproduction that runs, is worth more to this project than almost anything else that lands in
the tracker — and this one did more than report.

| Reporter | What | Fixed in |
| --- | --- | --- |
| [@Zniece](https://github.com/Zniece) | [GHSA-6976-qm5m-7mcj](https://github.com/MerlijnW70/viperjs/security/advisories/GHSA-6976-qm5m-7mcj) — a `BigInt` division at the limb ceiling panicked in the embedder's process, and answered two other values wrongly in silence. Reported *and* fixed, with the silent wrong answers named rather than stopping at the crash. [The long version.](https://github.com/MerlijnW70/viperjs/discussions/4) | [0.2.2](https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.2) |

## Supported versions

ViperJS is pre-1.0 and under active development. Only the latest released version is supported;
before 1.0 that means the tip of `master`.
