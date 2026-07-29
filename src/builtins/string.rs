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

use super::{define_function_metadata, define_method, define_value, string_edit, string_index};
use crate::heap::{Heap, NativeCall, Object, ObjectId, StringId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `String` and `String.prototype` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.string_prototype();
    let string = heap.new_native_constructor(realm.function_prototype(), make_string);
    define_function_metadata(heap, string, "String", 1);
    super::define_fixed(heap, string, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(string));
    define_value(heap, global, "String", Value::Object(string));

    for (name, length, native) in [
        ("fromCharCode", 1, from_char_code as crate::heap::Native),
        ("fromCodePoint", 1, from_code_point),
        ("raw", 1, raw),
    ] {
        define_method(heap, realm, string, name, length, native);
    }

    let own: [(&str, u32, crate::heap::Native); 6] = [
        ("toString", 0, string_to_string),
        ("valueOf", 0, value_of),
        ("toLowerCase", 0, to_lower_case),
        ("toUpperCase", 0, to_upper_case),
        ("toLocaleLowerCase", 0, to_lower_case),
        ("toLocaleUpperCase", 0, to_upper_case),
    ];
    for (name, length, native) in own
        .into_iter()
        .chain(string_index::METHODS)
        .chain(string_edit::METHODS)
    {
        define_method(heap, realm, prototype, name, length, native);
    }
    // B.2.2.14 and B.2.2.15 — the same function object under a second name, not a second function.
    for (alias, of) in string_edit::ALIASES {
        let Some(function) = read(heap, prototype, of) else {
            continue;
        };
        define_value(heap, prototype, alias, function);
    }
}

/// The value of a property just installed, for giving it a second name.
///
/// Ignores a miss rather than asserting: installation is total, and a name that is not there yet
/// means an alias for a method this build does not have — which is a table that needs correcting,
/// not a reason for a realm to fail to come up.
fn read(heap: &mut Heap, object: ObjectId, name: &str) -> Option<Value> {
    let key = super::key(heap, name);
    match heap.own_property(object, key)?.kind {
        crate::heap::PropertyKind::Data { value, .. } => Some(value),
        crate::heap::PropertyKind::Accessor { .. } => None,
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
pub(super) fn characters(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Vec<u16>> {
    if matches!(call.this_value, Value::Undefined | Value::Null) {
        return Err(Abrupt::type_error(
            "this method cannot be called on undefined or null",
        ));
    }
    let data = vm.to_string(call.this_value, heap)?;
    Ok(heap.string(data).unwrap_or(&[]).to_vec())
}

/// The unit at a position that may be any number at all, including an infinite one.
pub(super) fn at(units: &[u16], position: f64) -> Option<u16> {
    // A cast from `f64` saturates at both ends: a position past the last unit lands on `usize::MAX`
    // and `get` refuses it, so only the low end needs a comparison. Written as `>= 0.0` rather than
    // `< 0.0` negated because that is also the test a NaN fails, and a NaN cast to a `usize` would
    // otherwise be index zero — `"abc".charAt(NaN)` reading as `"a"` for the wrong reason.
    (position >= 0.0)
        .then(|| units.get(position as usize).copied())
        .flatten()
}

/// §7.1.5 `ToIntegerOrInfinity` — truncation towards zero, with NaN as zero.
pub(super) fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() {
        return 0.0;
    }
    number.trunc()
}

/// A position argument turned into an offset inside a string of `length` units.
pub(super) fn clamp(position: f64, length: usize) -> usize {
    // `max` first, because it answers the *other* operand for a NaN and so folds §7.1.5's "NaN is
    // zero" into the same expression. Then the saturating cast handles an infinity, and `min` the
    // upper end. Three rules, no branches — and nothing here a test could walk without reaching a
    // boundary that matters.
    (position.max(0.0) as usize).min(length)
}

/// The `ToString` of one argument, as units.
pub(super) fn argument_string(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    at: usize,
) -> Completion<Vec<u16>> {
    let id = vm.to_string(call.argument(at), heap)?;
    Ok(heap.string(id).unwrap_or(&[]).to_vec())
}

/// A `slice`-style index: negative counts back from the end, and everything is clamped.
pub(super) fn relative(position: f64, length: usize) -> usize {
    let position = to_integer_or_infinity(position);
    match position < 0.0 {
        true => clamp(position + length as f64, length),
        false => clamp(position, length),
    }
}

/// §22.1.2.2 `String.fromCodePoint(...codePoints)`.
///
/// Each argument must be an integer in `[0, 0x10FFFF]` — a fraction, a negative, or anything past
/// the last code point is a **RangeError**. That is the whole difference from `fromCharCode`, which
/// takes any number at all and wraps it: this one builds *code points*, and there is no code point
/// to build from `1.5`.
fn from_code_point(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut units = Vec::with_capacity(call.arguments.len());
    for argument in call.arguments {
        let number = vm.to_number(*argument, heap)?;
        let Some(point) = code_point_of(number) else {
            return Err(Abrupt::range_error(
                "a code point must be an integer from 0 to 0x10FFFF",
            ));
        };
        encode(point, &mut units);
    }
    let Some(id) = heap.new_string_checked(units) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// The code point a number names, if it names one — §22.1.2.2 steps 5.b to 5.d.
///
/// Refuses a fraction, a negative, and anything past `U+10FFFF`, and accepts everything else —
/// including a lone surrogate, which is a code point that Rust's `char` cannot hold.
/// `String.fromCodePoint(0xD800)` is a one-unit string and not an error, so this answers a `u32`
/// and [`encode`] does the part `char::encode_utf16` would otherwise have done.
pub(super) fn code_point_of(number: f64) -> Option<u32> {
    if number.trunc() != number || !(0.0..=1_114_111.0).contains(&number) {
        return None;
    }
    Some(number as u32)
}

/// Append a code point to `units` as UTF-16 — one unit, or a surrogate pair.
///
/// Written out rather than deferred to `char::encode_utf16`, because half of what this is asked to
/// encode is a surrogate that is already a code unit and has no `char`.
fn encode(point: u32, units: &mut Vec<u16>) {
    let Some(above) = point.checked_sub(0x10000) else {
        // Below the astral planes, which includes the surrogates themselves — §11.1.1 lets a String
        // hold one, and this is the only way one can be put there deliberately.
        units.push(point as u16);
        return;
    };
    units.push(0xD800 + (above >> 10) as u16);
    units.push(0xDC00 + (above & 0x3FF) as u16);
}

/// §22.1.2.4 `String.raw(template, ...substitutions)`.
///
/// Reads `template.raw` as an array-like and joins its elements with the substitutions between
/// them, which is what makes it work on a hand-made object and not only on a tagged template. There
/// is one more raw piece than there are substitutions, so the last piece has nothing after it.
fn raw(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let template = vm.object_for(call.argument(0), heap)?;
    let raw_key = super::key(heap, "raw");
    let pieces = vm.get_property_key(template, raw_key, heap)?;
    let pieces = vm.object_for(pieces, heap)?;
    let length_key = super::key(heap, "length");
    let length = vm.get_property_key(pieces, length_key, heap)?;
    // §22.1.2.4 step 4 is `ToLength`, which clamps rather than refusing — and clamps a NaN to
    // zero, which is why this is not `f64::clamp`. The one already written for the Array methods,
    // because a second copy is a second place for that NaN rule to be got wrong.
    let count = super::array_methods::to_length(vm.to_number(length, heap)?);
    let mut units: Vec<u16> = Vec::new();
    for at in 0..count {
        let key = super::array_methods::index_key(heap, at);
        let piece = vm.get_property_key(pieces, key, heap)?;
        let piece = vm.to_string(piece, heap)?;
        units.extend_from_slice(heap.string(piece).unwrap_or(&[]));
        // The substitutions go *between* the pieces, so the last one is not followed by anything —
        // step 8.e stops one short rather than the loop doing.
        if at + 1 == count {
            break;
        }
        let Some(value) = call.arguments.get(at as usize + 1) else {
            continue;
        };
        let filled = vm.to_string(*value, heap)?;
        units.extend_from_slice(heap.string(filled).unwrap_or(&[]));
    }
    let Some(id) = heap.new_string_checked(units) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// §22.1.3.30 `String.prototype.toLowerCase`, and §22.1.3.26 `toLocaleLowerCase` with it.
fn to_lower_case(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    recased(vm, heap, call, false)
}

/// §22.1.3.28 `String.prototype.toUpperCase`, and §22.1.3.27 `toLocaleUpperCase` with it.
fn to_upper_case(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    recased(vm, heap, call, true)
}

/// §22.1.3.29 `ToLowerCase`/`ToUpperCase` — the Unicode Default Case Conversion.
///
/// Not a per-unit table. The mapping is defined over code *points* and is not one-to-one: `ß`
/// uppercases to `SS`, and `İ` lowercases to two code points. So the units are decoded, mapped,
/// and encoded again, and the result may be longer than what went in.
///
/// The locale-sensitive variants share this. §22.1.3.26 permits an implementation with no locale
/// data to answer the locale-independent mapping, and praxis has none — so `toLocaleUpperCase` is
/// `toUpperCase` under a second name rather than a second, subtly different answer.
fn recased(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    upwards: bool,
) -> Completion<Value> {
    let units = characters(vm, heap, call)?;
    let mut built: Vec<u16> = Vec::with_capacity(units.len());
    let mut encoded = [0u16; 2];
    for point in char::decode_utf16(units.iter().copied()) {
        // A lone surrogate has no case and no `char` — it is copied through, which is what keeps
        // `"\ud800".toUpperCase().length` at one rather than replacing it with U+FFFD.
        let Ok(point) = point else {
            built.push(point.unwrap_err().unpaired_surrogate());
            continue;
        };
        let mapped = match upwards {
            true => Cased::Up(point.to_uppercase()),
            false => Cased::Down(point.to_lowercase()),
        };
        for point in mapped {
            built.extend_from_slice(point.encode_utf16(&mut encoded));
        }
    }
    let Some(id) = heap.new_string_checked(built) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// One code point's case mapping, either way — the two have different types and the same shape.
enum Cased {
    /// What §22.1.3.28 maps a code point to.
    Up(std::char::ToUppercase),
    /// What §22.1.3.30 maps it to.
    Down(std::char::ToLowercase),
}

impl Iterator for Cased {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::Up(points) => points.next(),
            Self::Down(points) => points.next(),
        }
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
