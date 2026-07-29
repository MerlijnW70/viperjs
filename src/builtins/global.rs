//! §19.2 — the four functions that live on the global object and belong to nothing.
//!
//! # Why `parseInt` is not `Number`
//!
//! `Number("12abc")` is `NaN` and `parseInt("12abc")` is `12`. That is the whole distinction and
//! it is deliberate: `ToNumber` asks whether the *entire* string is a number, and `parseInt` reads
//! as far as it can and stops. So one is for validating and the other is for extracting, and a
//! program that reaches for the wrong one is usually wrong in a way that shows up on odd input
//! only.
//!
//! `parseFloat` is the same operation over a decimal literal instead of digits in a radix, with
//! the same "read what you can" rule — which is why `parseFloat("1.5.2")` is `1.5` rather than
//! `NaN`, and why neither of them ever throws.

use super::{define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Completion, Value};
use crate::vm::Vm;

/// Build §19.2's functions, and §19.1's two value properties, onto the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    for (name, length, native) in [
        ("isFinite", 1, is_finite as crate::heap::Native),
        ("isNaN", 1, is_nan),
        ("parseFloat", 1, parse_float),
        ("parseInt", 2, parse_int),
    ] {
        define_method(heap, realm, global, name, length, native);
    }
    // §19.1.1 to §19.1.3 — `undefined`, `NaN` and `Infinity` are properties of the global object
    // and not keywords, which is why `typeof undefined` works and `undefined = 1` does not. All
    // three are fixed in place: writable, enumerable and configurable are every one of them false.
    for (name, value) in [
        ("Infinity", Value::Number(f64::INFINITY)),
        ("NaN", Value::Number(f64::NAN)),
        ("undefined", Value::Undefined),
    ] {
        super::define_fixed(heap, global, name, value);
    }
    let _ = define_value;
}

/// §19.2.2 `isNaN(number)`.
///
/// `ToNumber` first, so it answers about what the argument *becomes*: `isNaN("abc")` is true
/// because that string is not a number, not because a string is not a number. `Number.isNaN` is
/// the one that does not convert, and the two disagree on every non-number.
fn is_nan(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let number = vm.to_number(call.argument(0), heap)?;
    Ok(Value::Boolean(number.is_nan()))
}

/// §19.2.3 `isFinite(number)`.
fn is_finite(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let number = vm.to_number(call.argument(0), heap)?;
    Ok(Value::Boolean(number.is_finite()))
}

/// §19.2.5 `parseInt(string, radix)`.
///
/// Reads a sign, an optional prefix and as many digits of the radix as it finds, and stops at the
/// first thing that is not one. Nothing at all is `NaN`, and that is the only way it fails.
///
/// The radix rules are the part worth reading. A radix of 0 or absent means 10 — *except* that a
/// `0x` prefix then means 16. A radix of 16 permits the prefix and does not require it. Anything
/// outside `[2, 36]` that is not 0 is `NaN` before a character is looked at.
fn parse_int(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    let radix = vm.to_number(call.argument(1), heap)?;
    Ok(Value::Number(integer_value(&units, as_radix(radix))))
}

/// §19.2.5 steps 6 to 8 — the radix a call asked for, as a number this can work in.
///
/// `ToInt32` and not `ToIntegerOrInfinity`: §19.2.5 step 6 says so, and it is why `parseInt("10",
/// 2 ** 32 + 2)` is 2 rather than a refusal. `None` is the "decide from the prefix" case.
fn as_radix(radix: f64) -> Option<u32> {
    let asked = to_int32(radix);
    match asked {
        0 => None,
        2..=36 => Some(asked as u32),
        // Out of range, and §19.2.5 step 8 makes that `NaN` rather than a fallback to 10. A radix
        // of 37 is a mistake and answering 10 would hide it.
        _ => Some(u32::MAX),
    }
}

/// §7.1.6 `ToInt32` — modulo 2^32, then read as signed.
fn to_int32(number: f64) -> i64 {
    // §7.1.6 steps 2 and 3 — a NaN or an infinity is `+0` — need no arm of their own.
    // `rem_euclid` answers NaN for both and a NaN cast to an integer is zero, so the arithmetic
    // arrives where the specification does and a guard here would be a branch no input could take.
    let wrapped = number.trunc().rem_euclid(4_294_967_296.0);
    let wrapped = wrapped as i64;
    match wrapped >= 2_147_483_648 {
        true => wrapped - 4_294_967_296,
        false => wrapped,
    }
}

/// The number `units` begins with, in `radix` — §19.2.5 steps 9 to 20.
fn integer_value(units: &[u16], radix: Option<u32>) -> f64 {
    let units = trim_leading_whitespace(units);
    let (negative, units) = sign(units);
    let (radix, units) = match radix {
        // A radix out of range, which step 8 refuses before reading anything.
        Some(radix) if !(2..=36).contains(&radix) => return f64::NAN,
        // 16 permits the prefix and does not require it; every other explicit radix forbids it.
        Some(16) => (16, strip_hex_prefix(units)),
        Some(radix) => (radix, units),
        // Absent, so the prefix decides — and this is the only place `0x` changes the radix.
        None => match strip_hex_prefix(units) {
            stripped if stripped.len() < units.len() => (16, stripped),
            _ => (10, units),
        },
    };
    let digits: Vec<u32> = units
        .iter()
        .map_while(|unit| digit_value(*unit, radix))
        .collect();
    // Step 16 — no digits at all is `NaN`, which is the one failure. `"abc"` in radix 10 has none;
    // `"12abc"` has two and stops at the third character.
    if digits.is_empty() {
        return f64::NAN;
    }
    // §19.2.5 step 19 asks for the *mathematical* value rounded once, and accumulating in `f64`
    // rounds at every digit instead. For thirty digits the two answers differ in the last place,
    // which is why base ten goes through the decimal parser: it is the one that rounds once, and
    // base ten is where a run long enough to notice actually gets written.
    //
    // Every other radix accumulates. A power of two is exact until the value is, and an odd radix
    // long enough to round twice is past anything a program means by a number.
    let value = match radix {
        10 => digits
            .iter()
            .map(|digit| char::from_digit(*digit, 10).unwrap_or('0'))
            .collect::<String>()
            .parse::<f64>()
            .unwrap_or(f64::NAN), // digits that came from `digit_value` always parse
        radix => digits.iter().fold(0.0, |value, digit| {
            value * f64::from(radix) + f64::from(*digit)
        }),
    };
    match negative {
        true => -value,
        false => value,
    }
}

/// §19.2.4 `parseFloat(string)`.
///
/// The longest prefix that is a `StrDecimalLiteral`, which is why `parseFloat("1.5.2")` is 1.5 and
/// `parseFloat("1e")` is 1 — the `e` begins an exponent that never arrives, so the literal ends
/// before it.
fn parse_float(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    Ok(Value::Number(decimal_value(&units)))
}

/// The longest leading `StrDecimalLiteral` of `units`, as a Number.
///
/// Found by trying and shortening rather than by a second parser: `crate::value` already knows
/// what a decimal literal is, and asking it about each prefix from the longest down gives the same
/// answer as reading the grammar forwards would. A String of `n` units asks at most `n` times, and
/// nothing here is on a path anything measures.
fn decimal_value(units: &[u16]) -> f64 {
    let units = trim_leading_whitespace(units);
    // `Infinity` is a `StrDecimalLiteral` and is not a number `ToNumber` would read from a prefix,
    // so it is worth naming rather than leaving to the loop below.
    for length in (1..=units.len()).rev() {
        let head = &units[..length];
        // §7.1.4.1's own reading, which refuses `0x…` and the empty string — both of which are
        // numbers to `ToNumber` and are not decimal literals.
        if starts_non_decimal(head) {
            continue;
        }
        let value = crate::value::string_to_number(head);
        if !value.is_nan() {
            return value;
        }
    }
    f64::NAN
}

/// Whether these units begin something `ToNumber` reads and `parseFloat` may not.
fn starts_non_decimal(units: &[u16]) -> bool {
    let (_, rest) = sign(units);
    let mut prefix = rest.iter().take(2);
    let Some(zero) = prefix.next() else {
        return false;
    };
    if *zero != u16::from(b'0') {
        return false;
    }
    prefix
        .next()
        .is_some_and(|kind| matches!(*kind, 0x62 | 0x42 | 0x6F | 0x4F | 0x78 | 0x58))
}

/// Drop the whitespace §7.1.4.1 allows before a numeric literal.
fn trim_leading_whitespace(units: &[u16]) -> &[u16] {
    let at = units
        .iter()
        .position(|unit| !is_space(*unit))
        .unwrap_or(units.len());
    &units[at..]
}

/// Whether a unit is one §12.2's whitespace or a line terminator.
fn is_space(unit: u16) -> bool {
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

/// A leading `+` or `-`, and what follows it.
fn sign(units: &[u16]) -> (bool, &[u16]) {
    match units.first().map(|unit| u32::from(*unit)) {
        Some(0x2D) => (true, &units[1..]),
        Some(0x2B) => (false, &units[1..]),
        _ => (false, units),
    }
}

/// `0x` or `0X` removed, if it is there.
fn strip_hex_prefix(units: &[u16]) -> &[u16] {
    let mut prefix = units.iter();
    let looks = prefix.next() == Some(&u16::from(b'0'))
        && prefix
            .next()
            .is_some_and(|kind| *kind == 0x78 || *kind == 0x58);
    match looks {
        true => &units[2..],
        false => units,
    }
}

/// What one unit is worth as a digit in `radix`, if it is one.
fn digit_value(unit: u16, radix: u32) -> Option<u32> {
    let value = match u32::from(unit) {
        digit @ 0x30..=0x39 => digit - 0x30,
        upper @ 0x41..=0x5A => upper - 0x41 + 10,
        lower @ 0x61..=0x7A => lower - 0x61 + 10,
        _ => return None,
    };
    (value < radix).then_some(value)
}

#[cfg(test)]
mod parsing {
    use super::{digit_value, integer_value, starts_non_decimal, to_int32};

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_radix_wraps_at_two_to_the_thirty_second_and_is_read_as_signed() {
        assert_eq!(to_int32(10.0), 10);
        assert_eq!(to_int32(10.9), 10);
        assert_eq!(to_int32(-10.0), -10);
        assert_eq!(to_int32(0.0), 0);
        // The boundary, from both sides: 2^31 - 1 is the largest positive and 2^31 is the most
        // negative, which is the whole of what "read as signed" means.
        assert_eq!(to_int32(2_147_483_647.0), 2_147_483_647);
        assert_eq!(to_int32(2_147_483_648.0), -2_147_483_648);
        assert_eq!(to_int32(4_294_967_295.0), -1);
        assert_eq!(to_int32(4_294_967_296.0), 0);
        assert_eq!(to_int32(4_294_967_298.0), 2);
        // §7.1.6 steps 2 and 3, which the arithmetic answers without a branch — see there.
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
        assert_eq!(to_int32(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn a_digit_is_worth_its_place_in_the_radix_and_nothing_outside_it() {
        assert_eq!(digit_value(u16::from(b'0'), 10), Some(0));
        assert_eq!(digit_value(u16::from(b'9'), 10), Some(9));
        // Both cases of the letters, which is where an off-by-one in the arithmetic hides: `A` is
        // ten and `Z` is thirty-five, and the same for lower case.
        assert_eq!(digit_value(u16::from(b'A'), 16), Some(10));
        assert_eq!(digit_value(u16::from(b'a'), 16), Some(10));
        assert_eq!(digit_value(u16::from(b'F'), 16), Some(15));
        assert_eq!(digit_value(u16::from(b'Z'), 36), Some(35));
        assert_eq!(digit_value(u16::from(b'z'), 36), Some(35));
        // …and a digit past the radix is not one, which is what makes `parseInt` stop rather than
        // read the whole string.
        assert_eq!(digit_value(u16::from(b'9'), 2), None);
        assert_eq!(digit_value(u16::from(b'F'), 10), None);
        assert_eq!(digit_value(u16::from(b'Z'), 35), None);
        assert_eq!(digit_value(u16::from(b'-'), 10), None);
        assert_eq!(digit_value(0x3000, 10), None);
    }

    #[test]
    fn no_digits_at_all_is_the_one_way_reading_an_integer_fails() {
        assert!(integer_value(&units("abc"), Some(10)).is_nan());
        assert!(integer_value(&units(""), Some(10)).is_nan());
        assert!(integer_value(&units("-"), Some(10)).is_nan());
        // The same in every radix, and not zero: an empty run of digits and a run that says zero
        // are different answers, and a fold over nothing would give the second.
        assert!(integer_value(&units("9"), Some(2)).is_nan());
        assert!(integer_value(&units("zz"), Some(2)).is_nan());
        assert_eq!(integer_value(&units("0"), Some(2)), 0.0);
        // …and a radix outside the interval is refused before a character is read.
        assert!(integer_value(&units("10"), Some(37)).is_nan());
        assert!(integer_value(&units("10"), Some(1)).is_nan());
    }

    #[test]
    fn a_non_decimal_prefix_is_what_parse_float_may_not_read() {
        // The four §12.9.3 prefixes, with and without a sign in front of them.
        for prefix in ["0x", "0X", "0b", "0B", "0o", "0O", "+0x", "-0b"] {
            assert!(starts_non_decimal(&units(prefix)), "{prefix}");
        }
        // …and everything a decimal literal may begin with, which is everything else.
        // …including the ones whose *second* character is a prefix letter. `1x` is a one and then
        // something else, and reading only the second character would call it hexadecimal.
        for ordinary in [
            "0", "1", "", "-", ".5", "0.5", "0e1", "-1", "Infinity", "0y", "1x", "5b", "-2o",
        ] {
            assert!(!starts_non_decimal(&units(ordinary)), "{ordinary}");
        }
    }
}
