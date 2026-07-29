//! §22.1.3 — the methods that build a new string out of an old one.
//!
//! Split from [`super::string`], which holds the constructor and the two methods that report what a
//! receiver *is*; the readers are in [`super::string_index`].
//!
//! # Nothing here changes a string
//!
//! §6.1.4 makes a String value immutable, so every one of these allocates. That is why the length
//! of a result is worked out before it is built rather than after — DR-0012 caps a String, and
//! `"a".repeat(1e18)` has to be refused rather than briefly attempted.

use super::string::{argument_string, characters, clamp, relative, to_integer_or_infinity};
use crate::heap::{Heap, NativeCall};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §22.1.3.4 `String.prototype.concat(...strings)`.
fn concat(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut units = characters(vm, heap, call)?;
    // Converted one at a time and in order, because each conversion can run a `toString` that sees
    // what the earlier ones did — so collecting them all first would be a different program.
    for at in 0..call.arguments.len() {
        units.extend(argument_string(vm, heap, call, at)?);
    }
    let Some(id) = heap.new_string_checked(units) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// §22.1.3.25 `String.prototype.slice(start, end)`.
///
/// Negative arguments count from the end, and a range that ends before it starts is empty rather
/// than reversed. That is the whole difference from `substring`, which swaps them instead.
fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let from = relative(vm.to_number(call.argument(0), heap)?, units.len());
    let to = match call.argument(1) {
        Value::Undefined => units.len(),
        value => relative(vm.to_number(value, heap)?, units.len()),
    };
    let taken = units.get(from..to.max(from)).unwrap_or(&[]).to_vec();
    Ok(Value::String(heap.intern(&taken)))
}

/// §22.1.3.24 `String.prototype.substring(start, end)`.
///
/// Clamps rather than counting from the end, and then puts the smaller first: `"abcd".substring(3,
/// 1)` is `"bc"`. §22.1.3.24 step 7 does the swap outright, and it is the reason this cannot share
/// its body with `slice`.
fn substring(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let first = clamp(
        to_integer_or_infinity(vm.to_number(call.argument(0), heap)?),
        units.len(),
    );
    let second = match call.argument(1) {
        Value::Undefined => units.len(),
        value => clamp(
            to_integer_or_infinity(vm.to_number(value, heap)?),
            units.len(),
        ),
    };
    let (from, to) = (first.min(second), first.max(second));
    let taken = units.get(from..to).unwrap_or(&[]).to_vec();
    Ok(Value::String(heap.intern(&taken)))
}

/// B.2.2.1 `String.prototype.substr(start, length)` — Annex B, and kept because the web is.
///
/// A negative start counts from the end and a *length* follows rather than an end position, which
/// is the whole difference from `slice` and `substring`. Annex B is normative for a browser and is
/// specified precisely enough to implement exactly.
fn substr(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let from = relative(vm.to_number(call.argument(0), heap)?, units.len());
    let wanted = match call.argument(1) {
        Value::Undefined => units.len(),
        value => clamp(
            to_integer_or_infinity(vm.to_number(value, heap)?),
            units.len(),
        ),
    };
    let to = from.saturating_add(wanted).min(units.len());
    let taken = units.get(from..to).unwrap_or(&[]).to_vec();
    Ok(Value::String(heap.intern(&taken)))
}

/// §22.1.3.20 `String.prototype.repeat(count)`.
///
/// A negative or infinite count is a **RangeError** rather than a clamp — the one place a String
/// method refuses a number outright instead of bending it. Checked before anything is built,
/// because the whole point of the check is that the result would not fit.
fn repeat(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let count = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    if count < 0.0 || count.is_infinite() {
        return Err(Abrupt::range_error(
            "a string may be repeated a finite, non-negative number of times",
        ));
    }
    let Some(id) = grown(&units, count).and_then(|built| heap.new_string_checked(built)) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// `units` repeated `count` times, or `None` if that many could not be a String.
///
/// The length is worked out in `f64` before a byte is allocated. `"ab".repeat(1e18)` asks for two
/// exabytes, and a `Vec` that tries to grow that far aborts the process — which DR-0002 does not
/// permit a script to cause.
fn grown(units: &[u16], count: f64) -> Option<Vec<u16>> {
    let total = units.len() as f64 * count;
    if !crate::heap::fits_in_a_string(total) {
        return None;
    }
    // The *total* bounds the loop and not the count, which is not a tidying. `"".repeat(1e17)`
    // asks for a hundred quadrillion copies of nothing: the result fits easily, and counting to
    // it does not. Looping until the answer is long enough makes the empty case take no turns at
    // all, and a script cannot hang the engine by asking for it.
    let total = total as usize;
    let mut built = Vec::with_capacity(total);
    while built.len() < total {
        built.extend_from_slice(units);
    }
    Some(built)
}

/// §22.1.3.17 `String.prototype.padStart` and §22.1.3.16 `padEnd`.
///
/// The filler is repeated and then cut to the exact space left, so a two-unit filler in a gap of
/// three contributes one and a half of itself. An empty filler pads with nothing at all, which is
/// step 5 and needs no branch of its own — see below.
fn padded(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>, before: bool) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let wanted = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let filler = match call.argument(1) {
        // §22.1.3.17 step 4 — the default is one space, and an explicit `undefined` is the same.
        Value::Undefined => vec![u16::from(b' ')],
        _ => argument_string(vm, heap, call, 1)?,
    };
    let gap = match gap_to_fill(wanted, units.len()) {
        Pad::Nothing => return Ok(Value::String(heap.intern(&units))),
        Pad::TooLong => {
            return Err(Abrupt::range_error(
                "the resulting string would be too long",
            ));
        }
        Pad::Fill(gap) => gap,
    };
    // No guard for an empty filler. `cycle` over nothing answers nothing at once, so `take` ends
    // on its first turn and the result is the string unchanged — which is what §22.1.3.17 step 5
    // asks for, arrived at rather than tested for.
    let fill = filler.iter().copied().cycle().take(gap);
    let mut built = Vec::with_capacity(gap.saturating_add(units.len()));
    match before {
        true => {
            built.extend(fill);
            built.extend_from_slice(&units);
        }
        false => {
            built.extend_from_slice(&units);
            built.extend(fill);
        }
    }
    let Some(id) = heap.new_string_checked(built) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// What a pad has to do to reach `wanted` units, given it already has `have`.
///
/// Three answers and not two, because the two ways of having no gap to fill are not the same
/// thing. A target no longer than the string means the string is already long enough and is
/// answered unchanged — §22.1.3.17 step 5. A target longer than any String may be means the
/// answer cannot be built at all, and DR-0012 makes that a RangeError. Collapsing them would make
/// `"a".padStart(1e21)` quietly answer `"a"`.
fn gap_to_fill(wanted: f64, have: usize) -> Pad {
    if wanted <= have as f64 {
        return Pad::Nothing;
    }
    if !crate::heap::fits_in_a_string(wanted) {
        return Pad::TooLong;
    }
    Pad::Fill((wanted as usize).saturating_sub(have))
}

/// What [`gap_to_fill`] found there was to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pad {
    /// The string is already at least as long as was asked for.
    Nothing,
    /// This many units of filler go on the front or the back.
    Fill(usize),
    /// The length asked for is one no String may have.
    TooLong,
}

/// §22.1.3.17 `String.prototype.padStart(targetLength[, padString])`.
fn pad_start(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    padded(vm, heap, call, true)
}

/// §22.1.3.16 `String.prototype.padEnd(targetLength[, padString])`.
fn pad_end(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    padded(vm, heap, call, false)
}

/// §22.1.3.22 `String.prototype.split(separator, limit)`.
///
/// Four answers that read as special cases and are not, being steps 10 to 14 taken plainly:
///
/// - no separator at all is the whole string in one piece, nothing having been asked to split on;
/// - a limit of zero is the empty array, no pieces having been asked for;
/// - an empty separator cuts between every unit, so `"abc"` becomes three pieces;
/// - and the empty string split on an empty separator is the empty array rather than one empty
///   piece, there being no units to cut between.
///
/// A regular-expression separator is step 2, and arrives with `RegExp`.
fn split(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    // §22.1.3.22 step 6 — `ToUint32` and not a clamp, so a limit of -1 wraps to 2^32-1 and means
    // "every piece" rather than "none". `"a,b".split(",", -1)` has two pieces, and a clamp here
    // would answer zero.
    let limit = match call.argument(1) {
        Value::Undefined => usize::MAX,
        value => to_uint32(vm.to_number(value, heap)?) as usize,
    };
    if limit == 0 {
        return super::array::from_values(vm, heap, &[]);
    }
    let pieces = match call.argument(0) {
        Value::Undefined => vec![units],
        _ => {
            let separator = argument_string(vm, heap, call, 0)?;
            cut(&units, &separator, limit)
        }
    };
    let values: Vec<Value> = pieces
        .into_iter()
        .map(|piece| Value::String(heap.intern(&piece)))
        .collect();
    super::array::from_values(vm, heap, &values)
}

/// `units` cut wherever `separator` occurs, to at most `limit` pieces.
///
/// A pure function so that its four boundaries can be asked about directly, rather than through a
/// call and an array that would have to be read back to find out what happened.
fn cut(units: &[u16], separator: &[u16], limit: usize) -> Vec<Vec<u16>> {
    if separator.is_empty() {
        return units.iter().take(limit).map(|unit| vec![*unit]).collect();
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at + separator.len() <= units.len() {
        if units[at..at + separator.len()] != *separator {
            at += 1;
            continue;
        }
        pieces.push(units[start..at].to_vec());
        if pieces.len() == limit {
            return pieces;
        }
        at += separator.len();
        start = at;
    }
    // The tail is a piece even when it is empty, which is why `"a,".split(",")` has two of them and
    // why a string with no separator in it splits into one rather than none.
    pieces.push(units[start..].to_vec());
    pieces
}

/// §7.1.6 `ToUint32` — modulo 2^32 after truncation towards zero.
///
/// The same shape as §7.1.7's `ToUint16` in [`super::string`], and for the same reason: a NaN or an
/// infinity is `+0`, and `rem_euclid` followed by a saturating cast arrives there without a branch
/// that no input could take.
fn to_uint32(number: f64) -> u32 {
    number.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// §22.1.3.32 `String.prototype.trim`.
fn trim(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    trimmed(vm, heap, call, true, true)
}

/// §22.1.3.34 `String.prototype.trimStart`.
fn trim_start(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    trimmed(vm, heap, call, true, false)
}

/// §22.1.3.33 `String.prototype.trimEnd`.
fn trim_end(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    trimmed(vm, heap, call, false, true)
}

/// §22.1.3.31 `TrimString`, from either end or both.
fn trimmed(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    start: bool,
    end: bool,
) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let (from, to) = edges(&units, start, end);
    let taken = units.get(from..to).unwrap_or(&[]).to_vec();
    Ok(Value::String(heap.intern(&taken)))
}

/// The first and last positions a trim would keep.
fn edges(units: &[u16], start: bool, end: bool) -> (usize, usize) {
    let mut from = 0;
    let mut to = units.len();
    if start {
        while from < to && is_trimmable(units[from]) {
            from += 1;
        }
    }
    if end {
        while to > from && is_trimmable(units[to - 1]) {
            to -= 1;
        }
    }
    (from, to)
}

/// Whether a code unit is one §22.1.3.31 trims — `White_Space`, or a line terminator.
///
/// By unit rather than by code point, and that is exact rather than approximate: every code point
/// in the set is below `U+10000`, so no surrogate can be mistaken for one and no astral character
/// can be half-trimmed.
fn is_trimmable(unit: u16) -> bool {
    matches!(
        unit,
        0x09..=0x0D
            | 0x20
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
            | 0xFEFF
    )
}

/// Every method this module defines, with the `length` §22.1.3 gives it.
///
/// A table rather than a call per method at the install site, so that adding one here is one line
/// and cannot be written without being installed.
pub(super) const METHODS: [(&str, u32, crate::heap::Native); 11] = [
    ("concat", 1, concat),
    ("padEnd", 1, pad_end),
    ("padStart", 1, pad_start),
    ("repeat", 1, repeat),
    ("slice", 2, slice),
    ("split", 2, split),
    ("substr", 2, substr),
    ("substring", 2, substring),
    ("trim", 0, trim),
    ("trimEnd", 0, trim_end),
    ("trimStart", 0, trim_start),
];

/// The Annex B names for two of the trimmers — B.2.2.14 and B.2.2.15.
///
/// The specification says `trimLeft` **is** `trimStart` — the same function object — so a test
/// comparing them with `===` passes. Installing a second native with the same body would not
/// satisfy that, which is why these are aliases of an already-installed property rather than two
/// more entries above.
pub(super) const ALIASES: [(&str, &str); 2] = [("trimLeft", "trimStart"), ("trimRight", "trimEnd")];

#[cfg(test)]
mod pieces {
    use super::{Pad, cut, edges, gap_to_fill, grown, is_trimmable, to_uint32};

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn joined(pieces: Vec<Vec<u16>>) -> String {
        pieces
            .into_iter()
            .map(|piece| String::from_utf16_lossy(&piece))
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn a_separator_cuts_between_its_occurrences_and_leaves_the_tail() {
        assert_eq!(joined(cut(&units("a,b,c"), &units(","), 9)), "a|b|c");
        assert_eq!(joined(cut(&units("aXXb"), &units("XX"), 9)), "a|b");
        // The tail is a piece even when empty, and so is a leading one — which is what makes
        // `",a,".split(",")` three pieces rather than one.
        assert_eq!(joined(cut(&units(",a,"), &units(","), 9)), "|a|");
        assert_eq!(joined(cut(&units("abc"), &units(","), 9)), "abc");
        assert_eq!(cut(&units(""), &units(","), 9).len(), 1);
        // An empty separator cuts between every unit, and the empty string has nothing to cut
        // between — the one case that answers no pieces at all.
        assert_eq!(joined(cut(&units("abc"), &[], 9)), "a|b|c");
        assert_eq!(cut(&units(""), &[], 9).len(), 0);
        // The limit stops the cutting rather than trimming the answer afterwards, so the last
        // piece is *not* the rest of the string.
        assert_eq!(joined(cut(&units("a,b,c"), &units(","), 2)), "a|b");
        assert_eq!(joined(cut(&units("abc"), &[], 2)), "a|b");
        assert_eq!(cut(&units("a,b"), &units(","), 1).len(), 1);
        // A separator longer than the string is not found, and the loop that looks for it does not
        // run off the end while failing to find it.
        assert_eq!(joined(cut(&units("a"), &units("abc"), 9)), "a");
    }

    #[test]
    fn a_repeat_is_refused_before_it_is_built_when_it_could_not_fit() {
        assert_eq!(grown(&units("ab"), 3.0), Some(units("ababab")));
        assert_eq!(grown(&units("ab"), 0.0), Some(Vec::new()));
        assert_eq!(grown(&units("ab"), 1.0), Some(units("ab")));
        // Nothing repeated any number of times is still nothing — and must not be *counted* to,
        // either. This asked for a hundred quadrillion turns of the loop before the loop was
        // bounded by the length of the answer instead of by the count.
        assert_eq!(grown(&[], 1e17), Some(Vec::new()));
        assert_eq!(grown(&[], 1e300), Some(Vec::new()));
        // The number that matters: two exabytes asked for, and nothing allocated to find out.
        assert_eq!(grown(&units("ab"), 1e18), None);
        assert_eq!(
            grown(&units("a"), crate::heap::MAX_STRING_LENGTH as f64 + 1.0),
            None
        );
        // The cap itself is *allowed* and is deliberately not asked for here. A String that long
        // is within DR-0012's limit, so this would answer `Some` after half a gigabyte and a
        // quarter of a billion turns of the loop — which is what asking it here did. Where the
        // refusal begins is the only part of the boundary a caller can observe.
        assert_eq!(grown(&units("ab"), 134_217_728.0), None);
    }

    #[test]
    fn a_pad_asks_for_its_gap_before_allocating_it() {
        assert_eq!(gap_to_fill(5.0, 2), Pad::Fill(3));
        assert_eq!(gap_to_fill(3.0, 0), Pad::Fill(3));
        // Already long enough: the string comes back unchanged, and a target *shorter* than it is
        // the same answer rather than a negative gap.
        assert_eq!(gap_to_fill(2.0, 2), Pad::Nothing);
        assert_eq!(gap_to_fill(1.0, 2), Pad::Nothing);
        // …and a target no String could reach is the other answer entirely. Collapsing these two
        // is what made `"a".padStart(1e21)` quietly answer `"a"`.
        assert_eq!(gap_to_fill(f64::INFINITY, 0), Pad::TooLong);
        assert_eq!(gap_to_fill(1e21, 1), Pad::TooLong);
        assert_eq!(
            gap_to_fill(crate::heap::MAX_STRING_LENGTH as f64 + 1.0, 0),
            Pad::TooLong
        );
        assert_eq!(
            gap_to_fill(crate::heap::MAX_STRING_LENGTH as f64, 0),
            Pad::Fill(crate::heap::MAX_STRING_LENGTH)
        );
    }

    #[test]
    fn a_split_limit_wraps_rather_than_clamping() {
        assert_eq!(to_uint32(2.0), 2);
        assert_eq!(to_uint32(2.9), 2);
        // §22.1.3.22 step 6 is `ToUint32`, so a negative limit is very nearly unlimited. A clamp
        // would make `"a,b".split(",", -1)` answer no pieces at all.
        assert_eq!(to_uint32(-1.0), 4_294_967_295);
        assert_eq!(to_uint32(4_294_967_296.0), 0);
        assert_eq!(to_uint32(4_294_967_297.0), 1);
        assert_eq!(to_uint32(0.0), 0);
        assert_eq!(to_uint32(f64::NAN), 0);
        assert_eq!(to_uint32(f64::INFINITY), 0);
        assert_eq!(to_uint32(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn trimming_stops_at_the_first_unit_that_is_not_whitespace() {
        assert_eq!(edges(&units("  ab  "), true, true), (2, 4));
        assert_eq!(edges(&units("  ab  "), true, false), (2, 6));
        assert_eq!(edges(&units("  ab  "), false, true), (0, 4));
        assert_eq!(edges(&units("ab"), true, true), (0, 2));
        // All whitespace: the two ends meet rather than crossing, which is what the `from < to` in
        // each loop is there for.
        assert_eq!(edges(&units("   "), true, true), (3, 3));
        assert_eq!(edges(&[], true, true), (0, 0));
        // §22.1.3.31's set is longer than a reader expects, and these are the members easiest to
        // leave out — and the two near-misses that must not be in it.
        assert!(is_trimmable(0x0B));
        assert!(is_trimmable(0xFEFF));
        assert!(is_trimmable(0x3000));
        assert!(is_trimmable(0x2028));
        assert!(is_trimmable(0x1680));
        assert!(!is_trimmable(u16::from(b'a')));
        assert!(!is_trimmable(0x200B));
        assert!(!is_trimmable(0x1FFF));
        assert!(!is_trimmable(0x0E));
    }
}
