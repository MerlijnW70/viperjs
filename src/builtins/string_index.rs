//! §22.1.3 — the methods that answer what is *at* a place in a string, or where something is.
//!
//! Split from [`super::string`] because the two halves ask different questions: these read the
//! characters that are there and answer a unit, a code point, a position or a yes. The ones in
//! [`super::string_edit`] make a new string out of the pieces.
//!
//! # The four ways of asking what is at a position
//!
//! `s[i]`, `charAt`, `charCodeAt` and `at` all read one place and answer four different things
//! when that place is outside the string — `undefined`, `""`, `NaN` and `undefined` again, and
//! `at` counts from the end where the others do not. Each is its own clause and none is a shorthand
//! for another.

use super::string::{argument_string, at, characters, clamp, to_integer_or_infinity};
use crate::heap::{Heap, NativeCall};
use crate::value::{Completion, Value};
use crate::vm::Vm;

/// §22.1.3.1 `String.prototype.charAt(pos)`.
///
/// A position outside the string is the *empty* string, not `undefined` — unlike `s[i]`, which is
/// the property read and answers `undefined`. The two look interchangeable and are not.
fn char_at(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let position = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let Some(unit) = at(&units, position) else {
        return Ok(Value::String(heap.intern(&[])));
    };
    Ok(Value::String(heap.intern(&[unit])))
}

/// §22.1.3.2 `String.prototype.charCodeAt(pos)`.
///
/// NaN outside the string, which is the one place a string method answers NaN rather than
/// `undefined` or `-1`. §22.1.3.2 step 5 says so and every other choice would be a guess.
fn char_code_at(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let position = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    Ok(Value::Number(
        at(&units, position).map_or(f64::NAN, f64::from),
    ))
}

/// §22.1.3.9 `String.prototype.indexOf(searchString[, position])`.
///
/// An empty needle is found at the clamped position rather than not found, which is why
/// `"abc".indexOf("")` is `0` and `"abc".indexOf("", 10)` is `3`. Falls out of the search below
/// rather than being a special case, because an empty slice matches at once.
fn index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let needle = argument_string(vm, heap, call, 0)?;
    let start = to_integer_or_infinity(vm.to_number(call.argument(1), heap)?);
    let from = clamp(start, units.len());
    Ok(Value::Number(search(
        &units,
        &needle,
        from..=units.len(),
        false,
    )))
}

/// §22.1.3.10 `String.prototype.lastIndexOf(searchString[, position])`.
///
/// The position argument is `ToNumber` and *not* `ToIntegerOrInfinity` first, because a NaN there
/// means "the end of the string" rather than zero — step 5 tests for NaN before truncating. So
/// `"aa".lastIndexOf("a", undefined)` is `1` while `"aa".indexOf("a", undefined)` is `0`.
fn last_index_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let needle = argument_string(vm, heap, call, 0)?;
    let asked = vm.to_number(call.argument(1), heap)?;
    let end = match asked.is_nan() {
        true => units.len(),
        false => clamp(to_integer_or_infinity(asked), units.len()),
    };
    Ok(Value::Number(search(&units, &needle, 0..=end, true)))
}

/// Where `needle` sits in `units`, searching only starts inside `range`.
///
/// One function for both directions because the two differ in nothing but which match they keep,
/// and written twice one of the two boundary rules would eventually be fixed in only one of them.
fn search(
    units: &[u16],
    needle: &[u16],
    range: std::ops::RangeInclusive<usize>,
    last: bool,
) -> f64 {
    let mut found = -1.0;
    for start in range {
        if units.len() - start < needle.len() {
            break;
        }
        if units[start..start + needle.len()] == *needle {
            found = start as f64;
            if !last {
                break;
            }
        }
    }
    found
}

/// §22.1.3.1 `String.prototype.at(index)`.
///
/// Counts from the end for a negative index, and answers `undefined` rather than `""` when there is
/// nothing there — the two ways it differs from `charAt`, and the reason it exists.
fn char_at_relative(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let asked = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let position = match asked < 0.0 {
        true => asked + units.len() as f64,
        false => asked,
    };
    let Some(unit) = at(&units, position) else {
        return Ok(Value::Undefined);
    };
    Ok(Value::String(heap.intern(&[unit])))
}

/// §22.1.3.4 `String.prototype.codePointAt(pos)`.
///
/// The whole code point when a leading surrogate is followed by a trailing one, and the lone unit
/// otherwise — so reading the *second* half of a pair answers that half's own value rather than the
/// pair's. That asymmetry is §11.1.5 `CodePointAt`, and it is the only reason this is not
/// `charCodeAt` under another name.
fn code_point_at(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let position = to_integer_or_infinity(vm.to_number(call.argument(0), heap)?);
    let Some(first) = at(&units, position) else {
        return Ok(Value::Undefined);
    };
    Ok(Value::Number(f64::from(code_point(
        &units, position, first,
    ))))
}

/// The code point beginning at `position`, which is one unit unless a surrogate pair starts there.
fn code_point(units: &[u16], position: f64, first: u16) -> u32 {
    if !(0xD800..0xDC00).contains(&first) {
        return u32::from(first);
    }
    let Some(second) = at(units, position + 1.0) else {
        return u32::from(first);
    };
    if !(0xDC00..0xE000).contains(&second) {
        return u32::from(first);
    }
    // §11.1.3 `UTF16SurrogatePairToCodePoint`.
    (u32::from(first) - 0xD800) * 0x400 + (u32::from(second) - 0xDC00) + 0x10000
}

/// §22.1.3.8 `String.prototype.includes`, §22.1.3.23 `startsWith`, §22.1.3.7 `endsWith`.
///
/// One function, because the three differ only in which starting positions they will accept a match
/// at: anywhere from a point, exactly at a point, or exactly at a point counted from the end. Their
/// shared steps — `RequireObjectCoercible`, `IsRegExp`, `ToString`, the clamp — are the part that
/// would drift if this were written three times.
///
/// `IsRegExp` is not among them yet, because there are no regular expressions to detect. When there
/// are, these three gain the TypeError that stops `"a".includes(/a/)` — and it belongs here, once.
fn matches_at(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    where_from: Anchor,
) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let needle = argument_string(vm, heap, call, 0)?;
    let found = match where_from {
        // §22.1.3.7 step 6 — an `undefined` end position is the length, and every other value is
        // clamped. That is why `"abc".endsWith("abc", undefined)` is true.
        Anchor::End => {
            let end = match call.argument(1) {
                Value::Undefined => units.len(),
                value => clamp(
                    to_integer_or_infinity(vm.to_number(value, heap)?),
                    units.len(),
                ),
            };
            end.checked_sub(needle.len())
                .is_some_and(|start| units[start..end] == *needle)
        }
        anchor => {
            let start = clamp(
                to_integer_or_infinity(vm.to_number(call.argument(1), heap)?),
                units.len(),
            );
            let last = match anchor {
                Anchor::Anywhere => units.len(),
                _ => start,
            };
            (start..=last).any(|from| {
                units
                    .get(from..from + needle.len())
                    .is_some_and(|window| window == needle)
            })
        }
    };
    Ok(Value::Boolean(found))
}

/// Which starting positions one of the three "does it match" methods will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
    /// `includes` — any position from the one given onwards.
    Anywhere,
    /// `startsWith` — the position given, and no other.
    Start,
    /// `endsWith` — the position given, counted as where the match must *end*.
    End,
}

/// §22.1.3.8 `String.prototype.includes(searchString[, position])`.
pub(super) fn includes(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    matches_at(vm, heap, call, Anchor::Anywhere)
}

/// §22.1.3.23 `String.prototype.startsWith(searchString[, position])`.
pub(super) fn starts_with(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    matches_at(vm, heap, call, Anchor::Start)
}

/// §22.1.3.7 `String.prototype.endsWith(searchString[, endPosition])`.
pub(super) fn ends_with(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    matches_at(vm, heap, call, Anchor::End)
}

/// §22.1.3.12 `String.prototype.localeCompare(that)`.
///
/// Code-unit order, which §22.1.3.12 permits outright: an implementation without a locale database
/// may compare in any consistent way, and this is the one that agrees with `<`. The sign is all
/// that is specified — the magnitude is not — so this answers -1, 0 or 1 and nothing else.
pub(super) fn locale_compare(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let other = argument_string(vm, heap, call, 0)?;
    Ok(Value::Number(match units.cmp(&other) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

/// The two methods this module installs under names that are not their function names.
pub(super) const METHODS: [(&str, u32, crate::heap::Native); 10] = [
    ("at", 1, char_at_relative),
    ("charAt", 1, char_at),
    ("charCodeAt", 1, char_code_at),
    ("codePointAt", 1, code_point_at),
    ("endsWith", 1, ends_with),
    ("includes", 1, includes),
    ("indexOf", 1, index_of),
    ("lastIndexOf", 1, last_index_of),
    ("localeCompare", 1, locale_compare),
    ("startsWith", 1, starts_with),
];
