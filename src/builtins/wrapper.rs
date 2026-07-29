//! §20.3 `Boolean` and §21.1 `Number` — the two constructors that wrap a primitive.
//!
//! # What a wrapper is for
//!
//! `ToObject` (§7.1.18). A primitive has no properties of its own, so every operation that needs
//! an object out of one makes a wrapper: `Object(1)`, a method called with a primitive receiver in
//! sloppy code, a `for`-`in` over a String. The wrapper is an ordinary object with an ordinary
//! prototype that happens to remember what it was made from, and the prototype's methods are the
//! only things that read it.
//!
//! # Why one slot serves both
//!
//! §20.3 calls it `[[BooleanData]]` and §21.1 calls it `[[NumberData]]`, and a method of one may
//! not read the other's — `Boolean.prototype.valueOf.call(new Number(1))` is a TypeError. praxis
//! keeps one slot holding the primitive, and *the value says which*: a `Value::Boolean` is a
//! `[[BooleanData]]` and can be nothing else. So the rule is enforced by matching on what is
//! there rather than by three fields that could each be set wrongly.

use super::{define_function_metadata, define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value, number_to_string};
use crate::vm::Vm;

/// Build `Boolean` and `Number` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let boolean_prototype = realm.boolean_prototype();
    let boolean = constructor(
        heap,
        realm,
        global,
        "Boolean",
        1,
        boolean_prototype,
        make_boolean,
    );
    let _ = boolean;
    define_method(
        heap,
        realm,
        boolean_prototype,
        "toString",
        0,
        boolean_to_string,
    );
    define_method(
        heap,
        realm,
        boolean_prototype,
        "valueOf",
        0,
        boolean_value_of,
    );

    let number_prototype = realm.number_prototype();
    let number = constructor(
        heap,
        realm,
        global,
        "Number",
        1,
        number_prototype,
        make_number,
    );
    define_method(
        heap,
        realm,
        number_prototype,
        "toString",
        1,
        number_to_string_method,
    );
    define_method(heap, realm, number_prototype, "valueOf", 0, number_value_of);

    // §21.1.2 — the constants, none of them writable, enumerable or configurable.
    for (name, value) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", 9_007_199_254_740_991.0),
        ("MAX_VALUE", f64::MAX),
        ("MIN_SAFE_INTEGER", -9_007_199_254_740_991.0),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("POSITIVE_INFINITY", f64::INFINITY),
    ] {
        super::define_fixed(heap, number, name, Value::Number(value));
    }
    for (name, length, native) in [
        ("isFinite", 1, is_finite as crate::heap::Native),
        ("isInteger", 1, is_integer),
        ("isNaN", 1, is_nan),
        ("isSafeInteger", 1, is_safe_integer),
    ] {
        define_method(heap, realm, number, name, length, native);
    }
}

/// One constructor, with its `prototype` and that prototype's `constructor` back.
fn constructor(
    heap: &mut Heap,
    realm: &Realm,
    global: ObjectId,
    name: &str,
    length: u32,
    prototype: ObjectId,
    native: crate::heap::Native,
) -> ObjectId {
    // §20.3.2 and §21.1.2 — `Boolean` and `Number` are constructors; everything this file
    // installs beside them is a method and is not.
    let function = heap.new_native_constructor(realm.function_prototype(), native);
    define_function_metadata(heap, function, name, length);
    // §20.3.2.1 and §21.1.2.15 — the `prototype` of a wrapper constructor is fixed in place, as
    // every constructor's is: an instance already inherits from it by the time a script could
    // move it.
    super::define_fixed(heap, function, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, name, Value::Object(function));
    function
}

/// §20.3.1.1 `Boolean(value)` — and `new Boolean(value)`, which differ in what they answer.
///
/// Called, it converts; constructed, it wraps. That is the whole of the difference, and it is why
/// `if (new Boolean(false))` takes the branch: the wrapper is an object, and every object is
/// truthy however it was made.
fn make_boolean(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = Value::Boolean(call.argument(0).to_boolean(heap));
    Ok(wrap_or_convert(
        vm,
        heap,
        call,
        value,
        vm.realm().boolean_prototype(),
    ))
}

/// §21.1.1.1 `Number(value)` and `new Number(value)`.
fn make_number(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §21.1.1.1 step 1 — with no argument at all the answer is `+0`, which is not the same as
    // `ToNumber(undefined)` and is why `Number()` is `0` where `Number(undefined)` is `NaN`.
    let value = match call.arguments.first() {
        Some(argument) => Value::Number(argument.to_number(heap)?),
        None => Value::Number(0.0),
    };
    Ok(wrap_or_convert(
        vm,
        heap,
        call,
        value,
        vm.realm().number_prototype(),
    ))
}

/// The shape both constructors share: a wrapper when constructed, the primitive when called.
fn wrap_or_convert(
    _vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    value: Value,
    prototype: ObjectId,
) -> Value {
    match call.constructing {
        true => Value::Object(heap.new_wrapper(prototype, value)),
        false => value,
    }
}

/// `thisBooleanValue` (§20.3.3) and `thisNumberValue` (§21.1.3) — the receiver's own primitive.
///
/// A primitive receiver answers itself; a wrapper answers what it wraps; anything else is a
/// TypeError. The `matches` on the value is what keeps the two kinds apart — a Number wrapper
/// reaching `Boolean.prototype.valueOf` finds a `Value::Number` where it wanted a `Value::Boolean`
/// and is refused, which is what §20.3.3 asks for.
fn this_primitive(
    heap: &Heap,
    receiver: Value,
    wanted: fn(&Value) -> bool,
    complaint: &'static str,
) -> Completion<Value> {
    if wanted(&receiver) {
        return Ok(receiver);
    }
    if let Value::Object(object) = receiver
        && let Some(primitive) = heap.object(object).and_then(crate::heap::Object::primitive)
        && wanted(&primitive)
    {
        return Ok(primitive);
    }
    Err(Abrupt::type_error(complaint))
}

/// §20.3.3.2 `Boolean.prototype.toString`.
fn boolean_to_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = this_primitive(
        heap,
        call.this_value,
        |value| matches!(value, Value::Boolean(_)),
        "Boolean.prototype.toString requires a boolean",
    )?;
    let text = matches!(value, Value::Boolean(true));
    Ok(super::text(heap, if text { "true" } else { "false" }))
}

/// §20.3.3.3 `Boolean.prototype.valueOf`.
fn boolean_value_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    this_primitive(
        heap,
        call.this_value,
        |value| matches!(value, Value::Boolean(_)),
        "Boolean.prototype.valueOf requires a boolean",
    )
}

/// §21.1.3.7 `Number.prototype.valueOf`.
fn number_value_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    this_primitive(
        heap,
        call.this_value,
        |value| matches!(value, Value::Number(_)),
        "Number.prototype.valueOf requires a number",
    )
}

/// §21.1.3.6 `Number.prototype.toString([radix])`.
fn number_to_string_method(
    _vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let value = this_primitive(
        heap,
        call.this_value,
        |value| matches!(value, Value::Number(_)),
        "Number.prototype.toString requires a number",
    )?;
    let Value::Number(number) = value else {
        return Err(Abrupt::type_error(
            "Number.prototype.toString requires a number",
        ));
    };
    // Step 2 — a missing radix is 10, and `undefined` is *also* 10, which is why the argument is
    // asked about rather than converted blindly.
    let radix = match call.argument(0) {
        Value::Undefined => 10.0,
        given => given.to_integer_or_infinity(heap)?,
    };
    // Step 3 — anything outside 2 to 36 is a RangeError, including a fractional radix, which the
    // conversion above has already flattened.
    if !(2.0..=36.0).contains(&radix) {
        return Err(Abrupt::range_error("the radix must be between 2 and 36"));
    }
    let text = match radix as u32 {
        // §6.1.6.1.20's `Number::toString`, which is a different algorithm from the general one
        // and the only one that is exactly specified.
        10 => number_to_string(number),
        radix => in_radix(number, radix),
    };
    Ok(super::text(heap, &text))
}

/// A Number written in a radix other than ten — §21.1.3.6's implementation-approximated half.
///
/// The specification leaves this to the implementation past the point where a value is exactly
/// representable, and says only that it should be the digits of a value "for which x is closest".
/// Written the boring way: the integer part by repeated division, the fraction by repeated
/// multiplication, stopping when nothing is left or when a `f64` has no more to give.
fn in_radix(value: f64, radix: u32) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        // `==` and not `>`: only an infinity reaches here, so a comparison against zero would
        // be one no input could answer differently either way.
        return if value == f64::INFINITY {
            "Infinity"
        } else {
            "-Infinity"
        }
        .into();
    }
    // No case for zero: it falls through with nothing before the point and nothing after, and the
    // loop below writes the `0` that leaves. A branch for it would be one no input could tell from
    // its absence.
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let negative = value < 0.0;
    let value = value.abs();
    let mut integer = value.trunc();
    let mut fraction = value.fract();

    let mut whole = Vec::new();
    while integer >= 1.0 {
        let digit = (integer % f64::from(radix)) as usize;
        whole.push(DIGITS[digit.min(35)]);
        integer = (integer / f64::from(radix)).trunc();
    }
    if whole.is_empty() {
        whole.push(b'0');
    }
    whole.reverse();
    let mut out = String::from_utf8_lossy(&whole).into_owned();

    if fraction > 0.0 {
        out.push('.');
        // Twenty digits is past what a `f64` can distinguish in any radix — the mantissa is 53
        // bits, and even in base 2 the fraction runs out well before this.
        for _ in 0..1100 {
            if fraction == 0.0 {
                break;
            }
            fraction *= f64::from(radix);
            let digit = fraction.trunc() as usize;
            out.push(char::from(DIGITS[digit.min(35)]));
            fraction = fraction.fract();
        }
    }
    if negative { format!("-{out}") } else { out }
}

/// §21.1.2.2 `Number.isFinite` — with **no** conversion, which is the whole point of it.
///
/// `isFinite("1")` is true and `Number.isFinite("1")` is false: the global one converts and this
/// one asks. That is why these four cannot share an implementation with anything.
fn is_finite(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Boolean(matches!(
        call.argument(0),
        Value::Number(value) if value.is_finite()
    )))
}

/// §21.1.2.3 `Number.isInteger`.
fn is_integer(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Boolean(matches!(
        call.argument(0),
        Value::Number(value) if value.is_finite() && value.trunc() == value
    )))
}

/// §21.1.2.4 `Number.isNaN`.
fn is_nan(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Boolean(matches!(
        call.argument(0),
        Value::Number(value) if value.is_nan()
    )))
}

/// §21.1.2.5 `Number.isSafeInteger` — an integer every `f64` can tell from its neighbours.
fn is_safe_integer(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Boolean(matches!(
        call.argument(0),
        Value::Number(value)
            if value.is_finite() && value.trunc() == value && value.abs() <= 9_007_199_254_740_991.0
    )))
}
