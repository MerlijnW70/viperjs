---
id: DR-0026
title: An array index is a kind of property key, not a String that spells one
status: prose-only
---

`a[i] = v` costs **+196 nanoseconds** over `a[j] = v` with a fixed `j`, measured on 2026-08-08 by a
ladder in which each row differs from the one above by exactly one thing — so that difference *is*
the cost of turning a varying Number into a key. Against a 122 ns interpreter baseline and a 412 ns
indexed read, it is the largest single line item any measurement here has produced.

What the 196 ns buys is a round trip through text that both ends already know is a number:

```
a[i]   →  Number  →  decimal text  →  Vec<u16>  →  intern (hash, probe, maybe allocate)
                                                        ↓
                              PropertyKey::String(StringId)
                                                        ↓
heap::array_index  →  read the units back  →  canonical_numeric_index  →  f64  →  u32
```

Six steps to get from a `u32` the compiler had to a `u32` the element store wants. Two of them
allocate. **A String key costs nothing extra** — the same ladder shows `o[k]` with a variable String
at exactly the price of `o.x`, because the key is already interned and the work is a hash. It is
only the *numeric* key that pays, and it pays on every element of every loop that walks an array.

## The decision

**`PropertyKey` gains a third variant, `Index(u32)`, and it is canonical.**

Canonical is the whole of the design and everything else follows from it: a key is `Index` **if and
only if** §6.1.7 says its spelling is an array index. `PropertyKey::from_units` and its neighbours
answer `Index` for `"0"`, and `String` for `"01"`, `"1.0"`, `" 1"`, `"-0"` and `"4294967295"` —
which are ordinary property names and not indices, as `heap::array_index` already decides today.

That is what keeps `a[0]` and `a["0"]` **one property**. §6.1.7's rule is that an array index is a
String key that happens to spell a number, not a second kind of key beside it — so two
representations for one key would make `Object.keys(a)` show `"0"` twice, and `a[0] = 1; a["0"]`
answer `undefined`. With exactly one representation per key, `PartialEq` and `Hash` may stay derived
and nothing has to remember to compare across the variants.

**What it buys.** The fast path allocates nothing and hashes nothing: `ToPropertyKey` of a
non-negative integral Number below 2^32-1 is a cast. And `heap::array_index` stops being a decode —
it becomes a match on the variant, which removes the *other* half of the round trip from every
element access, including the ones that go through `[[Get]]` rather than the fast path.

**What it costs, stated so nobody discovers it later.**

- **Every String key pays a canonical-index check on the way in.** It is a scan of at most ten ASCII
  digits and a bound, in front of an intern that already hashes the same units — small, and on a
  path that is not the one under measurement. If it ever shows up, the check can move inside
  interning, which walks the units anyway.
- **A key's *text* has to be materialised on demand.** `Object.keys`, `for`-`in`, `String(key)` and
  every error message naming a key need `"0"` rather than `0`, so those interns move from where the
  key was made to where it is spelled. That is the right way round: an element access happens per
  element and a key is spelled per enumeration.
- **`PropertyKey` is public**, so this is a breaking change to the embedding surface. The crate is
  pre-1.0 and `CHANGELOG.md` says the API is not stable; it is still a change an embedder sees.

## What it measured, once it was built

Same machine, same two benchmark files, three runs each and identical to within 3 ns. **The estimate
above was +196 ns for a varying index and the measurement is −200**, which is as close as this
instrument gets. `lab/NOTES.md` has the full table; three rows moved for reasons this record did not
name, and they are the part worth reading.

- **A TypedArray element read is 4.4× faster — 958 ns to 217 — and nothing here predicted it.**
  §7.1.21 `CanonicalNumericIndexString` was implemented in `src/heap/typed.rs` by decoding the key's
  UTF-16 into a Rust `String`, parsing an `f64` out of it and formatting that back to check the
  spelling round-trips, **per element access**. An `Index` key answers all of it with one comparison
  against the view's length. The largest single win in the slice was in the file this record does not
  mention, because the record was written from the benchmark and the benchmark's TypedArray row was
  not one of the deltas being explained. **A representation change pays wherever the representation
  was being re-derived, and the way to find those places is to grep for the decode rather than to
  reason from the hot path.**
- **A *literal* `a[0]` got 56 ns faster**, although its key is a compile-time constant and the
  interning was already paid. That is the "other half of the round trip" above, and it is worth
  naming as a measured thing rather than an aside: the saving is on both sides of the boundary.
- **`push`/`pop`, string growth, calls and `fib` did not move at all**, which is the control this
  slice needed and did not have by design. Four rows that should not change and did not is what
  says the other seven moved for the reason claimed.

**And the invariant test passed first time**, including the cases it was written to catch — `a["01"]`
staying a named property, `a["-0"]` not moving `length`, and §10.1.11 ordering `2` before `10`
before `"b"` from the number rather than from a spelling that is no longer stored.

## What it took away, which was not on the list either

**A per-index allocation is a termination argument, and this one was load-bearing.** DR-0013's heap
budget is checked once per index by every array-like walk, and its doc gave the reason: "each pass
interns a key — so a walk that is going nowhere is also a walk that is spending the budget." An
`Index` key allocates nothing. So a walk that reads absent elements out of a huge array-like now
spends no heap, the check never fires, and the walk does not end.

That surfaced as eight regressions, all `Array.prototype.slice`, and **all of them a bug that had
been there all along**: `slice` asked `ArraySpeciesCreate` for a zero-length array where §23.1.3.25
step 8 asks for `count`, so §10.4.2.2 step 1's RangeError never came and the walk ran instead. It
had always run; it had always been cut short by the heap budget, which throws a RangeError of its
own that `assert.throws(RangeError, …)` cannot tell apart. Fixed, with a test asking for the count
as the *constructor argument* a `Symbol.species` sees, which is the assertion that separates them.

**The remaining hang is correct and is deliberately left.** `Array.prototype.indexOf.call({length:
2 ** 53 - 1}, x)` loops because §23.1.3.17 says to, and node does not return from it either —
measured. The engine's answer to a program that will not stop is DR-0022's time budget, which is the
host's to set. What the old accident bought was termination for the wrong reason, at the cost of a
RangeError no clause asks for.

## What this deliberately does not decide

- **The linear property scan.** An object's own properties are a `Vec<(PropertyKey, Property)>`, so
  every lookup walks them. That is adjacent — it is part of why eight shapes cost 469 ns against a
  single shape's 170 — and it is a different change with a different risk. Shapes and inline caches
  are the same conversation and are behind it.
- **How an Array stores its elements.** This record is about the *key*; whether a dense array keeps
  a `Vec` beside its property table is untouched, and the win here does not depend on it.
- **Whether the interpreter's baseline can come down.** 122 ns for a loop of six instructions is
  ~20 ns an instruction against 2–5 for a good non-JIT interpreter, and nothing here addresses it.
  The stack frame was the first hypothesis and was **falsified** — see `lab/NOTES.md`, which records
  the frame inflated six-fold with no change in speed at all.

## The invariant

**One key, one representation.** A property named by an array index is `Index`, wherever it came
from and however it was spelled — from a Number, from a String, from a computed key, from
`Object.defineProperty`, from the parser's constant table. The moment two spellings of one key can
both exist, every table keyed on `PropertyKey` holds two entries for one property, and the failure
is not an error but a duplicate: `Object.keys` grows an entry, `in` disagrees with `[[Get]]`, and a
delete removes one of the pair. That is the test worth writing first.
