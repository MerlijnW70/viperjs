---
id: DR-0003
title: Unicode identifier tables are generated data, pinned to a version and never hand-edited
status: prose-only
---

ECMA-262 §12.7 defines identifiers in terms of the Unicode properties `ID_Start` and
`ID_Continue`. There is no way to answer "may this code point begin a variable name?" without
that data, and DR-0001 forbids the crate that would hand it to us. So we carry it: 684 and 799
inclusive ranges, generated from the UCD and checked into `src/unicode_id_table.rs`.

Three consequences, and the reason this record exists is that each one is easy to violate by
accident.

**The data is pinned to a Unicode version, and that is conformant.** `src/unicode_id_table.rs`
names Unicode 17.0.0 and the exact file it came from. §12.7 says implementations "may recognize
identifier code points defined in later editions of the Unicode Standard" — so lagging is legal,
and the only real failure mode is not knowing which edition you are on. The version lives in the
file's header, and `the_tables_match_the_unicode_version_they_claim` asserts four counts taken
from the UCD at generation time. A regeneration that moves those numbers is a version bump and
must arrive as a commit that says so.

**Regeneration replaces the file whole.** Nothing hand-written lives in
`src/unicode_id_table.rs` — no predicate, no test, no local fix for one awkward character. The
logic that reads the tables is in `src/unicode_id.rs`, which regeneration never touches. A
one-character patch to the data would be invisible to review and permanent; if a code point
seems wrong, the UCD is the thing to check, and if the UCD is right then our reading of it is
the bug.

**No build script, no download, no `OUT_DIR`.** The tables are source. An embedder building
offline, in an air-gapped CI, or from a vendored crate gets byte-identical behaviour to one who
is not, and `cargo build` never reaches the network. That is the same promise DR-0001 makes
about dependencies, applied to data.

The alternative we rejected was restricting identifiers to ASCII and deferring Unicode. It is
not merely incomplete — it is wrong in a way that spreads: §12.7.1.1 makes it a Syntax Error
when a `\u` escape in an identifier resolves to a code point that is not an `IdentifierStartChar`
or `IdentifierPartChar`, so an ASCII-only predicate rejects `\u{e9}` as well as `é`, and the
escape validator would have had to be rewritten rather than extended once the real tables
arrived.
