---
id: DR-0004
title: A string value is a sequence of UTF-16 code units, not of characters
status: prose-only
---

`"\uD800"` is a valid ECMAScript string literal. Its value is one code unit, 0xD800, which is an
unpaired surrogate — and therefore not a Unicode scalar value, not a `char`, and not something a
Rust `String` can hold. `String::from_utf8` would reject it; `char::from_u32` returns `None` for
it. This is not an edge case we may round off: `"\uD800".length` is 1, `"\uD800" === "\uD800"` is
true, and both are things a script can observe.

So the lexer's string values are `Vec<u16>`, and the engine's `String` type will be a sequence of
code units when it arrives at M3. Not `String`, not `&str`, not "UTF-8 with a fixup".

The pressure to do otherwise is real and worth naming, because it will come back. UTF-8 is what
the source text already is; borrowing a slice of it would make the common literal free, and
almost every string in almost every program is well-formed UTF-16 that would survive the round
trip. The counter is simply that "almost every" is not "every", and the failure is silent: a
lexer that returns `String` has to do *something* with a lone surrogate, and every available
something — replacement character, error, dropping it — is observably wrong in a way no test of
ordinary strings would catch.

§12.9.4 makes the point in three separate places, which is a fair sign it is deliberate:

- The SV of `\ UnicodeEscapeSequence :: u Hex4Digits` is "the code unit whose numeric value is
  the MV of Hex4Digits" — a code *unit*, with no well-formedness condition attached.
- `\u{D800}` is likewise legal: `CodePoint` admits any value up to 0x10FFFF, surrogates included,
  and `UTF16EncodeCodePoint` (§11.1.1) passes anything below 0x10000 through unchanged.
- Astral characters are defined as contributing *two* code units, so the code-unit view is the
  one the specification counts in throughout.

The cost is an allocation per string literal and a representation twice the size of UTF-8 for
Latin text. Both are performance problems, which is the kind of problem this project prefers to
have (GOAL.md §1). A compact representation — WTF-8, small-string optimisation, interning — is an
M8 experiment with a benchmark in front of it, and it changes the engine's internals rather than
this decision: whatever the bytes look like in memory, what a string *is* stays a sequence of
code units.
