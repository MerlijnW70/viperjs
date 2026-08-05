---
id: DR-0014
title: The local time zone is UTC until a host supplies one
status: prose-only
---

`Date` is the first builtin that needs to know something only the operating system can tell it.
Every local-time operation in §21.4 goes through `LocalTZA(t, isUTC)`, and there is no way to
compute it from arithmetic: it is a property of the machine, and on most machines a function of the
date as well, because of daylight saving.

ViperJS cannot ask. `std::time::SystemTime` reports UTC and nothing else — that is the whole of what
the standard library offers. The local offset lives behind `GetTimeZoneInformation` on Windows and
`localtime_r` on Unix, and reaching either one costs a dependency (DR-0001 forbids it) or an
`extern "C"` declaration, which is `unsafe` (DR-0002 forbids that). There is no third door.

## So LocalTZA is zero, and that is conformant

This is not a shortfall dressed up as a decision. §21.4.1.7 makes the local time zone
**implementation-defined** and derived from the host environment; it does not require that the host
have any particular offset. An ECMAScript implementation whose host runs in UTC is a correct
ECMAScript implementation, and every engine agrees on what such a host observes: `getHours()` equals
`getUTCHours()`, `getTimezoneOffset()` is `0`, and `toString()` names `GMT+0000`.

So the engine behaves exactly as a UTC host behaves. What it must never do is *guess* — an offset
picked from a build-time constant, or a fixed number of hours because the author lives somewhere,
would make the same script answer differently on two machines with no way for either to be called
wrong. Zero is the one offset that is both defensible and reproducible.

## What this costs in conformance, precisely

The test262 `Date` tests are written to be timezone-agnostic, because they have to run on any host.
They either work in UTC, or compare local against UTC through the engine's own arithmetic, or state
their expectation in terms of `getTimezoneOffset()`. Those all pass here. What fails is the small
set that requires a *non-zero* offset to be observable at all — a test that only means something
where local time differs from UTC. Those are expectations entries with this record as the reason,
not silent failures.

Notably this is not the same as being wrong about daylight saving: with no offset there is no
transition, so there is no gap or repetition to get wrong. The arithmetic stays exact.

## The way out, when it is wanted

`LocalTZA` becomes a host hook on the embedding surface (`api.rs`, M4's remaining work), the same
shape M7 uses for a module resolver: the embedder supplies a function from a time value to an
offset in milliseconds, and the engine calls it. The default stays zero, so an embedder that does
not care is not obliged to care, and one that does gets real local time without the engine having
guessed anything. Nothing in this record has to be revisited to add that — the arithmetic already
routes every local operation through one function, which is the only structural requirement.

## The invariant

Every local-time operation goes through a single `local_tza` function. No other code in the engine
may consult the clock for an offset, and no offset may be baked in at build time. A test that would
only pass in a particular time zone is a test that must be rewritten, not accommodated.
