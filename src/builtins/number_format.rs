//! §21.1.3's three spellings of a Number — `toFixed`, `toExponential` and `toPrecision`.
//!
//! # Why these cannot be a formatting call
//!
//! Two reasons, and both are about *exactness*.
//!
//! §21.1.3.3 asks for the integer `n` that makes `n / 10^f - x` closest to zero, **choosing the
//! larger `n` when two are equally close**. That is round-half-up on the value's true magnitude.
//! Rust's `{:.*}` rounds half-to-**even**, so `(0.5).toFixed(0)` would answer `"0"` where the
//! specification says `"1"`, and `(2.5).toFixed(0)` would answer `"2"` where it says `"3"`.
//!
//! And "the true magnitude" is not the number as written. `1.005` is not 1.005: the nearest double
//! is `1.00499999999999989341858963598497211933…`, so `(1.005).toFixed(2)` is `"1.00"` and every
//! engine agrees. An implementation that multiplied by `10^f` and rounded would answer `"1.01"`,
//! because the multiplication rounds first.
//!
//! # How the exactness is obtained without arbitrary-precision arithmetic
//!
//! Every finite `f64` **is** a finite decimal — a dyadic rational — and the longest one a double
//! can hold has 1074 fractional digits (the smallest subnormal). So asking Rust's formatter for
//! more places than that yields the exact expansion, with no rounding left to disagree about, and
//! the rest of the work is arithmetic on a string of digits. That is slower than a clever
//! algorithm and it is obviously right, which is the trade §21.1.3 is worth.

use super::define_method;
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// More fractional digits than any `f64` needs, so a fixed-point format of one is exact.
///
/// 1074 is the smallest subnormal's last place; the few extra are slack rather than significance.
const EXACT: usize = 1080;

/// The exact decimal expansion of a finite, non-negative double, as `integer.fraction`.
fn expand(value: f64) -> String {
    format!("{value:.EXACT$}")
}

/// Add one to a string of decimal digits, growing it at the front if it carries all the way.
///
/// `"999"` becomes `"1000"`, and that growth is what the callers have to notice: for a fixed-point
/// result it is one more integer digit, and for an exponential one it is the exponent going up.
fn increment(digits: &str) -> String {
    let mut out: Vec<u8> = digits.bytes().collect();
    for place in (0..out.len()).rev() {
        if out[place] < b'9' {
            out[place] += 1;
            // The rest were nines and are now zeroes, which the loop has already written.
            return String::from_utf8_lossy(&out).into_owned();
        }
        out[place] = b'0';
    }
    format!("1{}", String::from_utf8_lossy(&out))
}

/// Keep this many fractional digits of an exact expansion, rounding half **up**.
///
/// Half-up rather than half-even, and one comparison says both halves of it: a remainder of
/// exactly one half and a remainder of more than one half both round away from zero, so the digit
/// just past what is kept decides on its own.
fn round_fixed(exact: &str, keep: usize) -> String {
    let (whole, fraction) = exact.split_once('.').unwrap_or((exact, ""));
    let kept = &fraction[..keep.min(fraction.len())];
    let up = fraction
        .as_bytes()
        .get(keep)
        .is_some_and(|digit| *digit >= b'5');
    let digits = format!("{whole}{kept}");
    let digits = if up { increment(&digits) } else { digits };
    if keep == 0 {
        return trim_leading(&digits);
    }
    let point = digits.len() - keep;
    format!("{}.{}", trim_leading(&digits[..point]), &digits[point..])
}

/// Drop leading zeroes, leaving at least one digit — `"007"` is `"7"` and `"000"` is `"0"`.
fn trim_leading(digits: &str) -> String {
    let kept = digits.trim_start_matches('0');
    match kept.is_empty() {
        true => "0".to_string(),
        false => kept.to_string(),
    }
}

/// The significant digits of an exact expansion, and the exponent of the first of them.
///
/// `value = d[0].d[1]d[2]… × 10^exponent`. A value of zero has no significant digit at all, and is
/// reported as a single `0` at exponent 0 — which is what §21.1.3.2 step 9 and §21.1.3.5 step 10.a
/// each say in their own words.
fn significands(exact: &str) -> (String, i32) {
    let (whole, fraction) = exact.split_once('.').unwrap_or((exact, ""));
    let digits = format!("{whole}{fraction}");
    let Some(first) = digits.find(|digit| digit != '0') else {
        return ("0".to_string(), 0);
    };
    // The exponent counts from the decimal point, which sits after `whole`.
    let exponent = whole.len() as i32 - 1 - first as i32;
    (digits[first..].to_string(), exponent)
}

/// Keep this many significant digits, rounding half up, and say what it did to the exponent.
///
/// Rounding `99.5` to two significant digits gives `100`, which is three digits — so the caller is
/// told the exponent moved rather than being left to notice a string one longer than it asked for.
fn round_significant(digits: &str, exponent: i32, keep: usize) -> (String, i32) {
    if digits.len() <= keep {
        return (format!("{digits:0<keep$}"), exponent);
    }
    let up = digits.as_bytes()[keep] >= b'5';
    let kept = &digits[..keep];
    if !up {
        return (kept.to_string(), exponent);
    }
    let carried = increment(kept);
    match carried.len() > keep {
        // It grew, so the leading digit moved a place up and the last one is dropped again.
        true => (carried[..keep].to_string(), exponent + 1),
        false => (carried, exponent),
    }
}

/// `d.ddd` followed by `e+k` — the shape §21.1.3.2 step 12 describes.
fn exponential(digits: &str, exponent: i32) -> String {
    let mantissa = match digits.len() {
        1 => digits.to_string(),
        _ => format!("{}.{}", &digits[..1], &digits[1..]),
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{}", exponent.abs())
}

/// `this` as a Number — §21.1.3's `ThisNumberValue`, and the sign taken off it.
///
/// Answers the magnitude and the sign separately because every one of the three works on the
/// magnitude and puts the sign back at the end, exactly as §21.1.3.3 step 8 does. `-0` answers a
/// sign of `""`: its magnitude is zero and `(-0).toFixed(2)` is `"0.00"`, not `"-0.00"`.
fn this_number(heap: &Heap, this: Value, what: &'static str) -> Completion<(f64, &'static str)> {
    let value = match this {
        Value::Number(number) => number,
        Value::Object(object) => match heap.object(object).and_then(crate::heap::Object::primitive)
        {
            Some(Value::Number(number)) => number,
            _ => return Err(Abrupt::type_error(what)),
        },
        _ => return Err(Abrupt::type_error(what)),
    };
    match value < 0.0 {
        true => Ok((-value, "-")),
        // `-0` is **not** less than zero, so its sign is empty and `(-0).toFixed(2)` is `"0.00"`.
        // `abs` is still needed: negative zero formats with a minus of its own, and without this
        // that minus survives into the digits and comes back out in the answer.
        false => Ok((value.abs(), "")),
    }
}

/// §7.1.5 `ToIntegerOrInfinity` of the argument — the step that may run user code and throw.
///
/// Separate from the range check because the two do **not** happen together. All three methods
/// convert at step 2 or 3, before asking whether the value has any digits at all; `toExponential`
/// and `toPrecision` then range-check *after* that question, and `toFixed` before it. So
/// `(NaN).toExponential(101)` is `"NaN"` while `(NaN).toFixed(101)` is a RangeError, and a Symbol
/// argument is a TypeError in every one of them however finite the receiver is.
fn to_integer(vm: &mut Vm, heap: &mut Heap, given: Value) -> Completion<f64> {
    let number = vm.to_number(given, heap)?;
    Ok(if number.is_nan() { 0.0 } else { number.trunc() })
}

/// The count as a size, or the RangeError one outside the range earns.
///
/// One comparison for both ends and for both infinities: `-∞` and `+∞` each fall outside, which a
/// pair of separate bounds would have to say twice.
fn digit_count(integer: f64, lowest: f64, what: &'static str) -> Completion<usize> {
    match (lowest..=100.0).contains(&integer) {
        true => Ok(integer as usize),
        false => Err(Abrupt::range_error(what)),
    }
}

/// §21.1.3.3 `Number.prototype.toFixed`.
fn to_fixed(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (value, sign) = this_number(
        heap,
        call.this_value,
        "Number.prototype.toFixed requires a number",
    )?;
    let places = digit_count(
        to_integer(vm, heap, call.argument(0))?,
        0.0,
        "the number of digits after the decimal point must be between 0 and 100",
    )?;
    // Step 6 — a NaN or an infinity is spelled the ordinary way, and the sign comes back with it
    // because `number_to_string` writes its own.
    if !value.is_finite() {
        return Ok(super::text(
            heap,
            &crate::value::number_to_string(match sign {
                "-" => -value,
                _ => value,
            }),
        ));
    }
    // Step 9 — past 10^21 the fixed spelling *is* the ordinary one, which is why
    // `(1e21).toFixed(2)` answers `"1e+21"` and not a thousand digits.
    if value >= 1e21 {
        return Ok(super::text(
            heap,
            &format!("{sign}{}", crate::value::number_to_string(value)),
        ));
    }
    let text = round_fixed(&expand(value), places);
    Ok(super::text(heap, &format!("{sign}{text}")))
}

/// §21.1.3.2 `Number.prototype.toExponential`.
fn to_exponential(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (value, sign) = this_number(
        heap,
        call.this_value,
        "Number.prototype.toExponential requires a number",
    )?;
    let asked = call.argument(0);
    // Step 2 runs before everything below it, and it can throw — `ToNumber` of a Symbol is a
    // TypeError, and of an object may run its `valueOf`. Converting after the non-finite answer
    // would let `(NaN).toExponential(Symbol())` succeed.
    let converted = match asked {
        Value::Undefined => None,
        given => Some(to_integer(vm, heap, given)?),
    };
    // Step 4 comes *before* step 5's range check, so `(NaN).toExponential(101)` is `"NaN"` rather
    // than a RangeError — the one place the order of two guards is observable here.
    if !value.is_finite() {
        return Ok(super::text(
            heap,
            &crate::value::number_to_string(match sign {
                "-" => -value,
                _ => value,
            }),
        ));
    }
    // Step 10.b — an absent count means "as few digits as say the number exactly", which is what
    // the shortest round-trip spelling already answers.
    let places = match converted {
        None => None,
        Some(integer) => Some(digit_count(
            integer,
            0.0,
            "the number of digits after the decimal point must be between 0 and 100",
        )?),
    };
    let (digits, exponent) = significands(&expand(value));
    let (digits, exponent) = match places {
        Some(places) => round_significant(&digits, exponent, places + 1),
        None => shortest(value, exponent),
    };
    Ok(super::text(
        heap,
        &format!("{sign}{}", exponential(&digits, exponent)),
    ))
}

/// The fewest significant digits that still name this double — §21.1.3.2 step 10.b.
///
/// Taken from the ordinary spelling rather than computed again: `number_to_string` already answers
/// the shortest decimal that round-trips, which is exactly what "f as small as possible" asks for.
fn shortest(value: f64, exponent: i32) -> (String, i32) {
    let ordinary = crate::value::number_to_string(value);
    let digits: String = ordinary
        .chars()
        .take_while(|found| *found != 'e')
        .filter(char::is_ascii_digit)
        .collect();
    let digits = digits.trim_start_matches('0');
    let digits = digits.trim_end_matches('0');
    match digits.is_empty() {
        true => ("0".to_string(), 0),
        false => (digits.to_string(), exponent),
    }
}

/// §21.1.3.5 `Number.prototype.toPrecision`.
fn to_precision(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (value, sign) = this_number(
        heap,
        call.this_value,
        "Number.prototype.toPrecision requires a number",
    )?;
    // Step 2 — no argument at all is `ToString`, before anything else is looked at.
    if matches!(call.argument(0), Value::Undefined) {
        return Ok(super::text(
            heap,
            &crate::value::number_to_string(match sign {
                "-" => -value,
                _ => value,
            }),
        ));
    }
    // Step 3 converts before step 4 asks whether there are any digits, so a Symbol here is a
    // TypeError even for a NaN receiver.
    let converted = to_integer(vm, heap, call.argument(0))?;
    if !value.is_finite() {
        return Ok(super::text(
            heap,
            &crate::value::number_to_string(match sign {
                "-" => -value,
                _ => value,
            }),
        ));
    }
    let precision = digit_count(converted, 1.0, "the precision must be between 1 and 100")?;
    // No special case for zero. §21.1.3.5 step 10.a writes one, and the general path below already
    // produces it: `significands` answers a single `0` at exponent 0, `round_significant` pads it
    // to the precision asked for, and an exponent of 0 is neither below -6 nor at the precision —
    // so it takes the fixed spelling and lands on exactly the same digits. A branch whose absence
    // no input can detect is one the suite cannot test.
    let (digits, exponent) = significands(&expand(value));
    let (digits, exponent) = round_significant(&digits, exponent, precision);
    // Step 12 — far from the decimal point in either direction, the exponential spelling is the
    // one that fits. This is the boundary `(0.000001).toPrecision(1)` sits on one side of and
    // `(0.0000001).toPrecision(1)` on the other.
    if exponent < -6 || exponent >= precision as i32 {
        return Ok(super::text(
            heap,
            &format!("{sign}{}", exponential(&digits, exponent)),
        ));
    }
    // Otherwise it is written in full, with the point put back where the exponent says.
    let text = match exponent >= 0 {
        true => {
            let whole = exponent as usize + 1;
            match digits.len() > whole {
                true => format!("{}.{}", &digits[..whole], &digits[whole..]),
                false => digits.clone(),
            }
        }
        false => format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize)),
    };
    Ok(super::text(heap, &format!("{sign}{text}")))
}

/// Put §21.1.3's three onto `Number.prototype`.
pub(super) fn install(heap: &mut Heap, realm: &Realm, prototype: ObjectId) {
    define_method(heap, realm, prototype, "toFixed", 1, to_fixed);
    define_method(heap, realm, prototype, "toExponential", 1, to_exponential);
    define_method(heap, realm, prototype, "toPrecision", 1, to_precision);
}
