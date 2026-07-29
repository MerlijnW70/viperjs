//! §22.1 `String` — the constructor, and the prototype every string in the language reaches.
//!
//! # The two kinds of receiver
//!
//! Every method here can be called on a String primitive or on a String object, and most of them
//! do not care which: §22.1.3 starts nearly all of them with `RequireObjectCoercible` and then
//! `ToString`, which turns both into the same characters. Only `toString` and `valueOf` insist on
//! one of the two — they are the methods whose whole job is to say what the receiver *is*, so a
//! receiver that is neither is a TypeError rather than something to convert.
//!
//! That is why `String.prototype.charAt.call(42, 0)` answers `"4"`. It reads as a mistake and it is
//! the specification: `ToString(42)` is `"42"` and the method never asked what kind of thing it was
//! given. Tests here say so outright, because a reader who assumed otherwise would "fix" it.
//!
//! # Where the characters come from
//!
//! A String is a sequence of UTF-16 code units and these methods index it as one — §6.1.4. `charAt`
//! answers a unit and not a character, `length` counts units, and a surrogate pair is two of
//! everything. That is observable and required: `"😀".length` is `2`.

use super::{define_function_metadata, define_method, define_value};
use crate::heap::{Heap, NativeCall, Object, ObjectId, StringId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `String` and `String.prototype` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.string_prototype();
    let string = heap.new_native_function(realm.function_prototype(), make_string);
    define_function_metadata(heap, string, "String", 1);
    super::define_fixed(heap, string, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(string));
    define_value(heap, global, "String", Value::Object(string));

    define_method(heap, realm, string, "fromCharCode", 1, from_char_code);

    for (name, length, native) in [
        ("toString", 0, string_to_string as crate::heap::Native),
        ("valueOf", 0, value_of),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("indexOf", 1, index_of),
        ("lastIndexOf", 1, last_index_of),
        ("concat", 1, concat),
        ("slice", 2, slice),
        ("substring", 2, substring),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
}

/// §22.1.1.1 `String(value)` and `new String(value)`.
///
/// Called with nothing at all is the empty String and not `"undefined"`: step 1 tests for *no
/// argument* rather than for an `undefined` one, so `String()` and `String(undefined)` differ. The
/// only place in the language where they do, and it is worth the check.
fn make_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let data = match call.arguments.first() {
        Some(value) => vm.to_string(*value, heap)?,
        None => heap.intern(&[]),
    };
    match call.constructing {
        true => Ok(Value::Object(
            heap.new_string_object(vm.realm().string_prototype(), data),
        )),
        false => Ok(Value::String(data)),
    }
}

/// §22.1.2.1 `String.fromCharCode(...codeUnits)`.
///
/// `ToUint16` of each argument, which truncates rather than refuses: `fromCharCode(65.9)` is `"A"`
/// and `fromCharCode(65536)` is `"\0"`. Neither is an error, and a reader expecting a RangeError is
/// reading a different specification.
fn from_char_code(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut units = Vec::with_capacity(call.arguments.len());
    for argument in call.arguments {
        units.push(to_uint16(vm.to_number(*argument, heap)?));
    }
    let Some(id) = heap.new_string_checked(units) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// §7.1.7 `ToUint16` — modulo 2^16 after truncation towards zero.
fn to_uint16(number: f64) -> u16 {
    // `rem_euclid` rather than `as u16`, which saturates: §7.1.7 step 4 is a modulo, so 65536 is 0
    // and -1 is 65535. Casting would make them 65535 and 0 — wrong in both directions.
    //
    // Steps 2 and 3 — a NaN or an infinity is `+0` — need no test of their own. `rem_euclid`
    // answers NaN for both, and a NaN cast to an integer is zero, so the arithmetic arrives where
    // the specification does. A guard here would be a branch no input could take.
    number.trunc().rem_euclid(65_536.0) as u16
}

/// `thisStringValue` (§22.1.3) — the characters the receiver *is*, and a TypeError otherwise.
///
/// Not `ToString`. These two methods are the ones that report what the receiver is made of, so a
/// Number reaching `String.prototype.toString` is refused rather than converted — otherwise
/// `String.prototype.toString.call(42)` would answer `"42"` and there would be no way left to ask
/// whether something is a string.
fn this_string(heap: &Heap, receiver: Value) -> Completion<StringId> {
    if let Value::String(data) = receiver {
        return Ok(data);
    }
    if let Value::Object(object) = receiver
        && let Some(data) = heap.object(object).and_then(Object::string_data)
    {
        return Ok(data);
    }
    Err(Abrupt::type_error(
        "this method requires a String or a String object",
    ))
}

/// §22.1.3.29 `String.prototype.toString`.
fn string_to_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::String(this_string(heap, call.this_value)?))
}

/// §22.1.3.35 `String.prototype.valueOf` — the same operation, under the other name.
fn value_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::String(this_string(heap, call.this_value)?))
}

/// The characters a method should work on — `RequireObjectCoercible` then `ToString` (§22.1.3).
///
/// The order matters and is observable: `null` is refused before the argument is converted, so
/// `String.prototype.indexOf.call(null, {toString: f})` never calls `f`.
fn characters(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Vec<u16>> {
    if matches!(call.this_value, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "this method cannot be called on undefined or null",
        ));
    }
    let data = vm.to_string(call.this_value, heap)?;
    Ok(heap.string(data).unwrap_or(&[]).to_vec())
}

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

/// The unit at a position that may be any number at all, including an infinite one.
fn at(units: &[u16], position: f64) -> Option<u16> {
    // A cast from `f64` saturates at both ends: a position past the last unit lands on `usize::MAX`
    // and `get` refuses it, so only the low end needs a comparison. Written as `>= 0.0` rather than
    // `< 0.0` negated because that is also the test a NaN fails, and a NaN cast to a `usize` would
    // otherwise be index zero — `"abc".charAt(NaN)` reading as `"a"` for the wrong reason.
    (position >= 0.0)
        .then(|| units.get(position as usize).copied())
        .flatten()
}

/// §7.1.5 `ToIntegerOrInfinity` — truncation towards zero, with NaN as zero.
fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() {
        return 0.0;
    }
    number.trunc()
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

/// A position argument turned into an offset inside a string of `length` units.
fn clamp(position: f64, length: usize) -> usize {
    // `max` first, because it answers the *other* operand for a NaN and so folds §7.1.5's "NaN is
    // zero" into the same expression. Then the saturating cast handles an infinity, and `min` the
    // upper end. Three rules, no branches — and nothing here a test could walk without reaching a
    // boundary that matters.
    (position.max(0.0) as usize).min(length)
}

/// The `ToString` of one argument, as units.
fn argument_string(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    at: usize,
) -> Completion<Vec<u16>> {
    let id = vm.to_string(call.argument(at), heap)?;
    Ok(heap.string(id).unwrap_or(&[]).to_vec())
}

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

/// A `slice`-style index: negative counts back from the end, and everything is clamped.
fn relative(position: f64, length: usize) -> usize {
    let position = to_integer_or_infinity(position);
    match position < 0.0 {
        true => clamp(position + length as f64, length),
        false => clamp(position, length),
    }
}

#[cfg(test)]
mod boundaries {
    use super::{at, clamp, to_integer_or_infinity, to_uint16};

    #[test]
    fn a_position_is_inside_a_string_only_up_to_its_last_unit() {
        let units = [b'a'.into(), b'b'.into()];
        assert_eq!(at(&units, 0.0), Some(u16::from(b'a')));
        assert_eq!(at(&units, 1.0), Some(u16::from(b'b')));
        // The two ends, and the reason each is refused: past the last unit there is nothing, and
        // below zero the cast would have wrapped round to the first one.
        assert_eq!(at(&units, 2.0), None);
        assert_eq!(at(&units, -1.0), None);
        // A NaN fails the same comparison a negative does, which is deliberate — see there.
        assert_eq!(at(&units, f64::NAN), None);
        assert_eq!(at(&units, f64::INFINITY), None);
        assert_eq!(at(&units, f64::NEG_INFINITY), None);
        assert_eq!(at(&[], 0.0), None);
    }

    #[test]
    fn a_position_argument_is_clamped_to_the_string_at_both_ends() {
        assert_eq!(clamp(0.0, 3), 0);
        assert_eq!(clamp(2.0, 3), 2);
        assert_eq!(clamp(3.0, 3), 3);
        // Past either end is the nearer end, and neither is an error: §22.1.3 clamps rather than
        // refusing, which is why `"abc".indexOf("", 10)` is `3` and not `-1`.
        assert_eq!(clamp(9.0, 3), 3);
        assert_eq!(clamp(-1.0, 3), 0);
        assert_eq!(clamp(f64::INFINITY, 3), 3);
        assert_eq!(clamp(f64::NEG_INFINITY, 3), 0);
        assert_eq!(clamp(f64::NAN, 3), 0);
        assert_eq!(clamp(1.0, 0), 0);
    }

    #[test]
    fn a_code_unit_argument_wraps_rather_than_being_refused() {
        assert_eq!(to_uint16(65.0), 65);
        assert_eq!(to_uint16(65.9), 65);
        assert_eq!(to_uint16(-65.9), 65_471);
        assert_eq!(to_uint16(65_535.0), 65_535);
        // §7.1.7 step 4 is a modulo and not a clamp, so these two wrap past each end rather than
        // sticking at it. A saturating cast would answer 65535 and 0 — the opposite of both.
        assert_eq!(to_uint16(65_536.0), 0);
        assert_eq!(to_uint16(-1.0), 65_535);
        assert_eq!(to_uint16(65_537.0), 1);
        assert_eq!(to_uint16(0.0), 0);
        assert_eq!(to_uint16(-0.0), 0);
        assert_eq!(to_uint16(f64::NAN), 0);
        assert_eq!(to_uint16(f64::INFINITY), 0);
        assert_eq!(to_uint16(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn an_integer_argument_truncates_towards_zero_and_a_nan_is_zero() {
        assert_eq!(to_integer_or_infinity(1.9), 1.0);
        assert_eq!(to_integer_or_infinity(-1.9), -1.0);
        assert_eq!(to_integer_or_infinity(0.0), 0.0);
        assert_eq!(to_integer_or_infinity(f64::NAN), 0.0);
        assert_eq!(to_integer_or_infinity(f64::INFINITY), f64::INFINITY);
        assert_eq!(to_integer_or_infinity(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }
}
