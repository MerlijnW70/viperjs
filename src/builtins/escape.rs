//! §B.2.1 — `escape` and `unescape`, the percent-escaping that predates §19.2.6's.
//!
//! # Why these are not `encodeURI` under an older name
//!
//! They escape **code units**, where §19.2.6 escapes the octets of a UTF-8 encoding. That is the
//! whole difference and it is visible immediately: `escape("\u{1F600}")` is `"%uD83D%uDE00"` —
//! the two halves of the surrogate pair, each spelled on its own — while
//! `encodeURIComponent` of the same string is `"%F0%9F%98%80"`, the one code point it is. So
//! `escape` has a third form that §19.2.6 has no need of, `%uXXXX`, and no notion of a code point
//! at all.
//!
//! Which is why neither of them can fail. An unpaired surrogate is just a code unit here and gets
//! its own `%uD800`; there is no encoding to be invalid, so `URIError` never arises and
//! [`unescape`] answers something for every string in the language.
//!
//! # What `unescape` does with what it cannot read
//!
//! Nothing — it copies it through. `unescape("%")` is `"%"`, `unescape("%zz")` is `"%zz"`, and
//! `unescape("%u12")` is `"%u12"`. That is not leniency bolted on: §B.2.1.2 has no failure
//! outcome, so a `%` that does not begin an escape is an ordinary character and the walk moves on
//! by one. `decodeURI` throws for every one of those, which is the sharpest way to see that these
//! two families are separate rather than related.
//!
//! The two length conditions are asymmetric in the specification and the asymmetry is real:
//! `%uXXXX` needs `k + 5 < len` and `%XX` needs `k + 3 ≤ len`. Both say "the escape fits", counted
//! once from the last index and once from the length — see [`unescape`], where they are written as
//! one kind of question so the difference cannot be a bug.
//!
//! # Annex B, and why they are here at all
//!
//! Normative optional, and every browser has them because the web does. DR-0008's position is that
//! praxis implements Annex B where strictness alone decides it; these need no host flag and no
//! strictness, so they cost two functions and a table.

use super::define_method;
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The code unit `%`.
const PERCENT: u16 = 0x25;

/// The code unit `u`, which is what tells the four-digit form from the two-digit one.
const LOWER_U: u16 = 0x75;

/// The punctuation §B.2.1.1 leaves alone, beside the ASCII word characters.
///
/// **Not** §19.2.6.1's `uriMark`, and the difference is the point: this set has `@`, `+` and `/`,
/// which are exactly the reserved characters a URI *component* escaper must escape, and lacks
/// `!~'()`, which a URI escaper leaves alone. Two sets chosen for two different jobs a decade
/// apart, overlapping in `-`, `.` and `*` by coincidence rather than by derivation.
///
/// The underscore is not here because it is an ASCII **word** character and is covered by
/// [`word_character`] — which is the clause's own phrasing, and worth keeping as such: written as
/// `is_ascii_alphanumeric` plus a punctuation list, `_` falls between the two and is escaped, and
/// the only thing that says so is test262's `unmodified.js` spelling the set out in full.
const UNESCAPED: &[u8] = b"@*+-./";

/// Build §B.2.1's two functions onto the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    define_method(heap, realm, global, "escape", 1, escape);
    define_method(heap, realm, global, "unescape", 1, unescape);
}

/// §B.2.1.1 `escape(string)`.
///
/// Three outcomes per code unit and no fourth: the unescaped set passes through, anything under
/// 256 becomes `%XX`, and everything else becomes `%uXXXX`. Nothing here can fail.
fn escape(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| {
        let mut built = Vec::with_capacity(units.len());
        for unit in units {
            let unit = *unit;
            // Step 6.b — the ASCII word characters and six punctuation marks.
            if u8::try_from(unit)
                .is_ok_and(|byte| word_character(byte) || UNESCAPED.contains(&byte))
            {
                built.push(unit);
                continue;
            }
            built.push(PERCENT);
            // Step 6.c.iii — a unit that does not fit in an octet keeps its `u` and all four of
            // its digits, which is the form §19.2.6 has no equivalent of.
            if unit > 0xFF {
                built.push(LOWER_U);
                built.push(u16::from(hex_upper((unit >> 12) as u8)));
                built.push(u16::from(hex_upper((unit >> 8) as u8)));
            }
            // Steps 6.c.ii.2 and 6.c.iii.2 — `StringPad(hex, 2, "0", start)`, so the leading zero
            // is written rather than dropped: `escape("\x07")` is `"%07"`.
            built.push(u16::from(hex_upper((unit >> 4) as u8)));
            built.push(u16::from(hex_upper(unit as u8)));
        }
        built
    })
}

/// §B.2.1.2 `unescape(string)`.
///
/// Reads `%uXXXX` and `%XX`, and copies through every `%` that begins neither. There is no third
/// outcome and no error: what cannot be read is a character like any other.
fn unescape(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| {
        let mut built = Vec::with_capacity(units.len());
        let mut at = 0;
        while at < units.len() {
            let unit = units[at];
            if unit != PERCENT {
                built.push(unit);
                at += 1;
                continue;
            }
            // Step 6.b.i and 6.b.ii — the four-digit form is tried first, so `%u0041` is `"A"`
            // rather than the `%u0` a two-digit reading would fail to make of it. Trying them the
            // other way round would read `%u00` as `%XX` with `u` as a digit, which it is not —
            // but `%uXX` where the last two are *not* digits must still fall through to the
            // two-digit form finding nothing, which is what makes this an `or` and not a branch.
            let read = digits(units, at + 2, 4)
                .filter(|_| units.get(at + 1) == Some(&LOWER_U))
                .map(|value| (value, 5))
                .or_else(|| digits(units, at + 1, 2).map(|value| (value, 2)));
            match read {
                Some((value, width)) => {
                    built.push(value);
                    at += width;
                }
                // Step 6.c reached with `c` still the `%` — an ordinary character, and the walk
                // advances by the one unit it consumed rather than skipping what follows. That is
                // why `unescape("%%41")` is `"%A"`: the first `%` is copied, and the second one
                // then begins an escape that reads.
                None => built.push(unit),
            }
            at += 1;
        }
        built
    })
}

/// The value of `count` hexadecimal digits at `position`, or `None` if they are not all there.
///
/// One function for both forms, which is what makes the specification's two length conditions —
/// `k + 5 < len` for four digits and `k + 3 ≤ len` for two — the same question asked once. Written
/// as two comparisons they differ by where they count from and would be two chances to be off by
/// one; written as "is there a slice of this length" they cannot.
fn digits(units: &[u16], position: usize, count: usize) -> Option<u16> {
    let asked = units.get(position..position + count)?;
    asked.iter().try_fold(0u16, |value, unit| {
        // Shifting rather than multiplying so that four digits cannot overflow: the widest thing
        // this reads is `%uFFFF`, which is exactly a `u16`, and a fifth digit is never asked for.
        Some((value << 4) | u16::from(hex_digit(*unit)?))
    })
}

/// An **ASCII word character** — a letter, a digit or `_`.
///
/// Its own function because `is_ascii_alphanumeric` is not it: the underscore is the difference,
/// and it is the one member of the set that no reader checks and every escaper needs. §22.2.2.9's
/// `\w` is the same set, spelled the same way.
fn word_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The value of one hexadecimal digit, in either case.
fn hex_digit(unit: u16) -> Option<u8> {
    match unit {
        0x30..=0x39 => Some((unit - 0x30) as u8),
        0x41..=0x46 => Some((unit - 0x41 + 10) as u8),
        0x61..=0x66 => Some((unit - 0x61 + 10) as u8),
        _ => None,
    }
}

/// The low nibble of `byte` as an uppercase hexadecimal digit.
///
/// Takes a whole byte and keeps four bits of it so that every caller can shift and hand it the
/// result, rather than masking at six call sites and getting one of them wrong.
fn hex_upper(byte: u8) -> u8 {
    match byte & 0x0F {
        nibble if nibble < 10 => b'0' + nibble,
        nibble => b'A' + (nibble - 10),
    }
}

/// `ToString` the argument, walk its units, and put the answer back on the heap.
///
/// Step 1 for both, and it is the only step either of them can fail at: neither walk has a failure
/// outcome, so a throw out of here is the argument's `toString` and nothing else.
fn transform(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    operation: impl FnOnce(&[u16]) -> Vec<u16>,
) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    let Some(id) = heap.new_string_checked(operation(&units)) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}
