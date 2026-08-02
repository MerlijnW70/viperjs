//! §21.2 — the `BigInt` constructor, its prototype, and the two functions that give one a width.
//!
//! # Why the constructor is not constructible
//!
//! `new BigInt(1)` is a TypeError, and so is `new Symbol()`. Both types are primitives whose
//! wrapper object exists only so that a method call on one has somewhere to look — and neither has
//! a reason for a program to make a wrapper *on purpose*. §21.2.1 says so in step 1, before it
//! looks at the argument at all.
//!
//! # Why `BigInt(1.5)` throws where `Number("1.5")` does not
//!
//! §21.2.1 step 3 is `ToBigInt`, which is `ToNumeric` and then an exactness check: a Number that is
//! not an integer has no BigInt, so the conversion refuses rather than rounding. `Number` converts
//! by losing information and BigInt will not, which is the same argument the operators make.

use super::{define_function_metadata, define_method, define_value};
use crate::bigint::BigInt;
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build §21.2.1's constructor and §21.2.3's prototype.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.bigint_prototype();
    let constructor = heap.new_native_function(realm.function_prototype(), convert);
    define_function_metadata(heap, constructor, "BigInt", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(constructor));
    define_value(heap, global, "BigInt", Value::Object(constructor));

    // §21.2.2 — the two static functions, and the only place a BigInt has a width. They are what
    // makes the type usable for the thing it is usually reached for: a 64-bit integer from a
    // database, a hash, or a protocol.
    define_method(heap, realm, constructor, "asIntN", 2, as_int_n);
    define_method(heap, realm, constructor, "asUintN", 2, as_uint_n);

    define_method(heap, realm, prototype, "toString", 0, to_string);
    define_method(heap, realm, prototype, "toLocaleString", 0, to_string);
    define_method(heap, realm, prototype, "valueOf", 0, value_of);

    // §21.2.3.5 — `[object BigInt]` comes from here rather than from §20.1.3.6's table, which is
    // why deleting this property makes a BigInt wrapper tag as an ordinary object.
    if let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) {
        let name = PropertyKey::from_symbol(symbol);
        let units: Vec<u16> = "BigInt".encode_utf16().collect();
        let value = Value::String(heap.intern(&units));
        let _ = heap.define_own_property(
            prototype,
            name,
            &PropertyDescriptor {
                value: Some(value),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}

/// §21.2.1 `BigInt(value)` — the explicit conversion, which the operators refuse to do implicitly.
fn convert(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `new BigInt(…)` is a TypeError. A wrapper is what a method call makes for itself;
    // there is no reason for a program to ask for one.
    if call.constructing() {
        return Err(Abrupt::type_error("BigInt is not a constructor"));
    }
    let primitive = vm.to_primitive(call.argument(0), crate::value::Hint::Number, heap)?;
    to_bigint(primitive, heap)
}

/// §7.1.13 `ToBigInt` — every conversion *into* the type, in one place.
///
/// The asymmetry with `ToNumber` is the point. A Number becomes a BigInt only when it is already an
/// integer, because rounding would answer a question nobody asked; a String becomes one only when
/// it spells an integer, and a String that does not is a **SyntaxError** rather than a NaN — there
/// is no BigInt for "not a number" to be.
pub(crate) fn to_bigint(value: Value, heap: &mut Heap) -> Completion<Value> {
    let converted = match value {
        Value::BigInt(_) => return Ok(value),
        Value::Boolean(true) => BigInt::from_u64(1),
        Value::Boolean(false) => BigInt::zero(),
        // §7.1.13's table gives both of these a **TypeError**, unlike `ToNumber` where `null` is
        // `+0` and `undefined` is NaN. There is no BigInt NaN for the second to become, and the
        // first is refused beside it rather than being the one conversion that silently works.
        Value::Undefined | Value::Null => {
            return Err(Abrupt::type_error("this cannot be converted to a BigInt"));
        }
        Value::Symbol(_) => {
            return Err(Abrupt::type_error(
                "a Symbol cannot be converted to a BigInt",
            ));
        }
        Value::Object(_) => {
            return Err(Abrupt::type_error("this cannot be converted to a BigInt"));
        }
        Value::Number(number) => match BigInt::from_f64(number) {
            Some(value) => value,
            // §7.1.13 step 2 — a Number that is not an integer has no BigInt, so this refuses
            // rather than rounding. `BigInt(1.5)` and `BigInt(NaN)` fail for the same reason.
            None => {
                return Err(Abrupt::range_error(
                    "only an integer Number can become a BigInt",
                ));
            }
        },
        Value::String(id) => match crate::value::string_as_bigint(id, heap) {
            Some(value) => value,
            // §7.1.13 step 1's `StringToBigInt` — and its failure is a **SyntaxError**, which is
            // the only place a conversion throws one. `Number("x")` is NaN; there is no BigInt for
            // that to be, so the text is simply not a BigInt literal.
            None => {
                return Err(Abrupt::Raised(
                    crate::value::ErrorKind::Syntax,
                    "this string is not a BigInt",
                ));
            }
        },
    };
    Ok(Value::BigInt(heap.new_bigint(converted)))
}

/// §21.2.3.3 `BigInt.prototype.toString([radix])`.
fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = this_bigint(call.this_value, heap)?;
    let radix = match call.argument(0) {
        Value::Undefined => 10,
        given => {
            let number = vm.to_number(given, heap)?;
            // §21.2.3.3 step 3 — outside 2 to 36 is a RangeError, the same range `Number` uses and
            // the same error for leaving it.
            let radix = number as u32;
            if !(2.0..=36.0).contains(&number) || f64::from(radix) != number {
                return Err(Abrupt::range_error("a radix must be between 2 and 36"));
            }
            radix
        }
    };
    let text = value.to_digits(radix);
    Ok(Value::String(
        heap.new_string(text.encode_utf16().collect()),
    ))
}

/// §21.2.3.4 `BigInt.prototype.valueOf()`.
fn value_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = this_bigint(call.this_value, heap)?;
    Ok(Value::BigInt(heap.new_bigint(value)))
}

/// §21.2.3.1's `ThisBigIntValue` — the receiver, or the BigInt a wrapper holds.
///
/// Both, because a method reached through a wrapper has the wrapper as its receiver: `1n.toString()`
/// wraps and `Object(1n).toString()` was already wrapped, and the two must answer the same.
fn this_bigint(receiver: Value, heap: &Heap) -> Completion<BigInt> {
    let held = match receiver {
        Value::BigInt(id) => Some(id),
        Value::Object(id) => match heap.object(id).and_then(crate::heap::Object::primitive) {
            Some(Value::BigInt(id)) => Some(id),
            _ => None,
        },
        _ => None,
    };
    match held.and_then(|id| heap.bigint(id)) {
        Some(value) => Ok(value.clone()),
        None => Err(Abrupt::type_error("this method requires a BigInt")),
    }
}

/// §21.2.2.1 `BigInt.asIntN(bits, bigint)` — the value modulo 2^bits, read as signed.
fn as_int_n(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (bits, value) = truncation_arguments(vm, heap, call)?;
    let wrapped = wrap_to_width(&value, bits).map_err(refused)?;
    // Step 4 — a value at or above half the width is that much *below* the width, which is what
    // reading the top bit as a sign means. `BigInt.asIntN(8, 255n)` is `-1n`.
    let half = BigInt::from_u64(1).shift_left(&BigInt::from_u64(bits.saturating_sub(1)));
    let signed = match (bits, half) {
        (0, _) => wrapped,
        (_, Ok(half)) if wrapped.compare(&half) != std::cmp::Ordering::Less => {
            let whole = BigInt::from_u64(1)
                .shift_left(&BigInt::from_u64(bits))
                .map_err(refused)?;
            wrapped.subtract(&whole).map_err(refused)?
        }
        _ => wrapped,
    };
    Ok(Value::BigInt(heap.new_bigint(signed)))
}

/// §21.2.2.2 `BigInt.asUintN(bits, bigint)` — the value modulo 2^bits, read as unsigned.
fn as_uint_n(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (bits, value) = truncation_arguments(vm, heap, call)?;
    let wrapped = wrap_to_width(&value, bits).map_err(refused)?;
    Ok(Value::BigInt(heap.new_bigint(wrapped)))
}

/// The two arguments both of §21.2.2's functions take, in the order the clause reads them.
fn truncation_arguments(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<(u64, BigInt)> {
    // `ToIndex` of the width first, then `ToBigInt` of the value — and the order is observable,
    // because either may run user code through a `valueOf`.
    let bits = super::buffer::to_index(vm, heap, call.argument(0))?;
    let primitive = vm.to_primitive(call.argument(1), crate::value::Hint::Number, heap)?;
    let Value::BigInt(id) = to_bigint(primitive, heap)? else {
        return Err(Abrupt::type_error("this cannot be converted to a BigInt"));
    };
    let value = heap.bigint(id).cloned().unwrap_or_else(BigInt::zero);
    Ok((bits as u64, value))
}

/// `value` modulo 2^`bits`, always in `0 ..= 2^bits - 1`.
///
/// A remainder and then a correction, because §6.1.6.2.6's remainder keeps the sign of the dividend
/// and a modulo does not: `-1n % 256n` is `-1n` where this wants `255n`.
fn wrap_to_width(value: &BigInt, bits: u64) -> Result<BigInt, crate::bigint::Error> {
    if bits == 0 {
        return Ok(BigInt::zero());
    }
    let width = BigInt::from_u64(1).shift_left(&BigInt::from_u64(bits))?;
    let remainder = value.remainder(&width)?;
    match remainder.is_negative() {
        true => remainder.add(&width),
        false => Ok(remainder),
    }
}

/// What a BigInt operation that could not answer becomes here.
fn refused(error: crate::bigint::Error) -> Abrupt {
    match error {
        crate::bigint::Error::TooLarge => {
            Abrupt::range_error("this BigInt is larger than this engine will hold")
        }
        _ => Abrupt::range_error("this BigInt operation has no answer"),
    }
}
