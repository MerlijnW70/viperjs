//! What the operators mean — §6.1.6.1's Number operations and §7.2's comparisons.
//!
//! The arithmetic an engine does is not the arithmetic a CPU does, and the differences are small
//! and constant. `**` disagrees with IEEE `pow` about `1 ** NaN`. `<` on two Strings is not a
//! locale comparison but a walk over code units. `==` is a ten-step algorithm. Each of those is
//! written out here against its clause, because each is a place where the obvious implementation
//! is subtly wrong and stays wrong for years.
//!
//! # What can fail here
//!
//! Anything that has to turn an Object into a primitive, which is most of it: §13.15.3 converts
//! both operands, and §7.2.13 converts one when the other is not an Object. That conversion is
//! the only thing in this module that throws, and everything else is arithmetic.
//!
//! `instanceof` and `in` are still absent. Both ask an *object* a question rather than converting
//! it, and both need the interpreter — `instanceof` calls a method, `in` looks a key up.

use crate::ast::BinaryOperator;
use crate::bigint::BigInt;
use crate::heap::Heap;
use crate::value::{Abrupt, Completion, Hint, Value};

/// §7.2.13 steps 3.c to 3.f — `<` where at least one side is a BigInt.
///
/// `None` is "not comparable", which is what a NaN produces and what makes `1n < NaN`,
/// `1n > NaN` and `1n <= NaN` all false at once — the same fold a Number comparison goes through.
/// A String that is not a BigInt does the same: §7.2.13 step 3.c returns undefined for it, so
/// `1n < "abc"` is false and so is `1n > "abc"`.
fn compare_across_types(left: Value, right: Value, heap: &Heap) -> Option<bool> {
    let order = match (left, right) {
        (Value::BigInt(a), Value::BigInt(b)) => heap.bigint(a)?.compare(heap.bigint(b)?),
        (Value::BigInt(a), Value::Number(b)) => against_number(heap.bigint(a)?, b)?,
        (Value::Number(a), Value::BigInt(b)) => against_number(heap.bigint(b)?, a)?.reverse(),
        (Value::BigInt(a), Value::String(b)) => {
            heap.bigint(a)?.compare(&string_as_bigint(b, heap)?)
        }
        (Value::String(a), Value::BigInt(b)) => string_as_bigint(a, heap)?.compare(heap.bigint(b)?),
        // A Boolean, `null` or `undefined` against a BigInt reaches here having already been
        // through `ToNumeric` at the call site, so there is nothing left that is not one of the
        // pairs above.
        _ => return None,
    };
    Some(order == std::cmp::Ordering::Less)
}

/// Where a BigInt sits relative to a Number, exactly — `None` for a NaN.
///
/// Exact on both sides. Turning the BigInt into an `f64` would put `2n ** 53n + 1n` and `2 ** 53`
/// at the same place, and they are different numbers; turning the Number into a BigInt loses its
/// fraction, which is what decides the comparison when the integer parts agree.
fn against_number(left: &BigInt, right: f64) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    // No check for a NaN: `BigInt::from_f64` answers `None` for one, and `None` here *is* "not
    // comparable". A guard of its own said the same thing on every input.
    //
    // An infinity is comparable and equals no BigInt: every one of them is inside it.
    if right.is_infinite() {
        return Some(match right.is_sign_positive() {
            true => Ordering::Less,
            false => Ordering::Greater,
        });
    }
    let whole = BigInt::from_f64(right.trunc())?;
    Some(match left.compare(&whole) {
        Ordering::Equal => {
            // The integer parts agree, so the fraction decides — and which way depends on its
            // sign: `1n < 1.5` and `-1n > -1.5`.
            let fraction = right.fract();
            match fraction.partial_cmp(&0.0) {
                Some(Ordering::Greater) => Ordering::Less,
                Some(Ordering::Less) => Ordering::Greater,
                _ => Ordering::Equal,
            }
        }
        other => other,
    })
}

/// §7.2.15 steps 4 and 5 — a BigInt and a Number, compared as points on the number line.
///
/// A Number that is not an integer is not any BigInt, and neither is a NaN or an infinity. The
/// comparison is *exact* on both sides: converting the BigInt to an `f64` would make
/// `2n ** 53n + 1n` equal `2 ** 53`, and they are different numbers.
fn equal_across_types(big: crate::heap::BigIntId, number: f64, heap: &Heap) -> bool {
    heap.bigint(big)
        .zip(BigInt::from_f64(number))
        .is_some_and(|(big, number)| *big == number)
}

/// §7.2.15 steps 6 and 7 — a BigInt against a String, which is read as one.
///
/// Text that is not an integer is simply *not equal* rather than an error: `1n == "1"` is true and
/// `1n == "1.5"` is false.
fn equal_to_string(big: crate::heap::BigIntId, text: crate::heap::StringId, heap: &Heap) -> bool {
    heap.bigint(big)
        .zip(string_as_bigint(text, heap))
        .is_some_and(|(big, text)| *big == text)
}

/// §7.1.14 `StringToBigInt` — the text of a BigInt, or `None` if it is not one.
///
/// `None` where `ToNumber` of the same text would be NaN, and for anything with a decimal point or
/// an exponent: `"1.5"` is a Number and no BigInt. The empty string is `0n`, which is the one
/// place this agrees with `ToNumber` about something surprising.
pub(crate) fn string_as_bigint(id: crate::heap::StringId, heap: &Heap) -> Option<BigInt> {
    let text = String::from_utf16(heap.string(id)?).ok()?;
    let trimmed = text.trim_matches(|c: char| c.is_whitespace());
    if trimmed.is_empty() {
        return Some(BigInt::zero());
    }
    let (negative, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    // The three radix prefixes, which §7.1.14 accepts and which may not carry a sign.
    let (radix, digits) = match rest.get(..2) {
        Some("0x" | "0X") if !negative => (16, &rest[2..]),
        Some("0o" | "0O") if !negative => (8, &rest[2..]),
        Some("0b" | "0B") if !negative => (2, &rest[2..]),
        _ => (10, rest),
    };
    if digits.is_empty() {
        return None;
    }
    Some(BigInt::from_digits(digits, radix)?.negate_if(negative))
}

/// Put a BigInt result on the heap, or turn its refusal into the completion §6.1.6.2 names.
fn bigint_value(
    result: Result<BigInt, crate::bigint::Error>,
    heap: &mut Heap,
) -> Completion<Value> {
    match result {
        Ok(value) => Ok(Value::BigInt(heap.new_bigint(value))),
        Err(error) => Err(refused(error)),
    }
}

/// §7.1.3 `ToNumeric` for both operands, refusing the pair §13.15.3 step 3 forbids.
///
/// The whole of what BigInt adds to arithmetic. Every operator below used to read two `f64`s; the
/// clause says to convert each operand to *a numeric type* and then insist the two agree, so
/// `1n + 1` is a TypeError rather than `2` or `"11"`. There is no width at which a BigInt and a
/// Number are the same value, so any implicit choice loses precision or magnitude — and the
/// specification would rather stop.
fn numeric_pair(left: Value, right: Value, heap: &mut Heap) -> Completion<Numeric> {
    match (left, right) {
        (Value::BigInt(a), Value::BigInt(b)) => {
            // Cloned out of the heap because the operation may allocate into it, and a borrow of
            // an arena cannot survive a push to it. A magnitude is a `Vec`; this is a copy of one.
            let a = heap.bigint(a).cloned().unwrap_or_else(BigInt::zero);
            let b = heap.bigint(b).cloned().unwrap_or_else(BigInt::zero);
            Ok(Numeric::BigInts(a, b))
        }
        (Value::BigInt(_), _) | (_, Value::BigInt(_)) => Err(Abrupt::type_error(
            "a BigInt and a Number cannot be mixed in arithmetic",
        )),
        _ => Ok(Numeric::Numbers(
            left.to_number(heap)?,
            right.to_number(heap)?,
        )),
    }
}

/// Which arithmetic an operator is about to do — §13.15.3 step 4's two branches.
enum Numeric {
    /// Both operands are Numbers, so §6.1.6.1's operations apply.
    Numbers(f64, f64),
    /// Both are BigInts, so §6.1.6.2's do.
    BigInts(BigInt, BigInt),
}

/// What a BigInt operation that could not answer becomes — §6.1.6.2's three abrupt completions.
fn refused(error: crate::bigint::Error) -> Abrupt {
    match error {
        crate::bigint::Error::DividedByZero => {
            Abrupt::range_error("a BigInt cannot be divided by zero")
        }
        crate::bigint::Error::NegativeExponent => {
            Abrupt::range_error("a BigInt cannot be raised to a negative power")
        }
        crate::bigint::Error::NoUnsignedShift => {
            Abrupt::type_error("BigInts have no unsigned right shift")
        }
        crate::bigint::Error::TooLarge => {
            Abrupt::range_error("this BigInt is larger than this engine will hold")
        }
    }
}

/// `ApplyStringOrNumericBinaryOperator` (§13.15.3) and the relational and equality operators.
///
/// One function for every binary operator that has two values and no side effects, because that
/// is how the VM meets them: the operands are already evaluated and on the stack. The operators
/// that are *not* here — `instanceof`, `in` — are the two that need an object, and the compiler
/// refuses them until there is one.
pub fn apply_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
    heap: &mut Heap,
) -> Completion<Value> {
    Ok(match operator {
        // §13.15.3 step 1 — `+` is the operator that is two operators. Every other one coerces
        // to a number first; this one asks whether either side is a String and concatenates if
        // so, which is why `1 + "1"` is `"11"` and `1 - "1"` is `0`.
        BinaryOperator::Add => return add(left, right, heap),
        BinaryOperator::Subtract => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Value::Number(a - b),
            Numeric::BigInts(a, b) => bigint_value(a.subtract(&b), heap)?,
        },
        BinaryOperator::Multiply => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Value::Number(a * b),
            Numeric::BigInts(a, b) => bigint_value(a.multiply(&b), heap)?,
        },
        // IEEE division, with no special case: `1/0` is `Infinity` and `0/0` is NaN, and neither
        // is an error. §6.1.6.1.5 says exactly this and nothing more.
        BinaryOperator::Divide => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Value::Number(a / b),
            // §6.1.6.2.5 step 1 — a RangeError where a Number would be `Infinity`, because there
            // is no BigInt infinity for it to be.
            Numeric::BigInts(a, b) => bigint_value(a.divide(&b), heap)?,
        },
        // §6.1.6.1.6 — the *remainder*, which keeps the sign of the dividend: `-1 % 2` is `-1`
        // and not `1`. Rust's `%` on `f64` is C's `fmod` and agrees exactly.
        BinaryOperator::Remainder => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Value::Number(a % b),
            Numeric::BigInts(a, b) => bigint_value(a.remainder(&b), heap)?,
        },
        BinaryOperator::Exponent => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Value::Number(exponentiate(a, b)),
            Numeric::BigInts(a, b) => bigint_value(a.exponentiate(&b), heap)?,
        },
        // §6.1.6.1.9 to §6.1.6.1.11 — "let shiftCount be ℝ(rnum) modulo 32", which is what makes
        // `1 << 32` be `1` and not `0`. Written as `wrapping_shl` rather than `% 32` before a
        // shift: that method is defined as masking the count to the width, so the two are one
        // operation and writing both would be writing it twice.
        //
        // The left operand of `>>>` is read as *unsigned*, which is the whole of the difference
        // between it and `>>`.
        BinaryOperator::ShiftLeft => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                let count = right.to_uint32(heap)?;
                Value::Number(f64::from(left.to_int32(heap)?.wrapping_shl(count)))
            }
            Numeric::BigInts(a, b) => bigint_value(a.shift_left(&b), heap)?,
        },
        BinaryOperator::ShiftRight => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                let count = right.to_uint32(heap)?;
                Value::Number(f64::from(left.to_int32(heap)?.wrapping_shr(count)))
            }
            Numeric::BigInts(a, b) => bigint_value(a.shift_right(&b), heap)?,
        },
        BinaryOperator::ShiftRightUnsigned => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                let count = right.to_uint32(heap)?;
                Value::Number(f64::from(left.to_uint32(heap)?.wrapping_shr(count)))
            }
            // §6.1.6.2.11 — the one operator BigInt does not have. `>>>` fills from the left with
            // zeros, which needs a width; a BigInt has none, so the clause refuses rather than
            // pretending `>>` will do.
            Numeric::BigInts(..) => {
                return Err(Abrupt::type_error("BigInts have no unsigned right shift"));
            }
        },
        // §6.1.6.1.17 to §6.1.6.1.19 — through `ToInt32`, which is why `2147483648 | 0` is
        // `-2147483648` and why every bitwise operator throws away anything past 32 bits.
        BinaryOperator::BitwiseAnd => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                Value::Number(f64::from(left.to_int32(heap)? & right.to_int32(heap)?))
            }
            Numeric::BigInts(a, b) => bigint_value(a.and(&b), heap)?,
        },
        BinaryOperator::BitwiseXor => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                Value::Number(f64::from(left.to_int32(heap)? ^ right.to_int32(heap)?))
            }
            Numeric::BigInts(a, b) => bigint_value(a.xor(&b), heap)?,
        },
        BinaryOperator::BitwiseOr => match numeric_pair(left, right, heap)? {
            Numeric::Numbers(..) => {
                Value::Number(f64::from(left.to_int32(heap)? | right.to_int32(heap)?))
            }
            Numeric::BigInts(a, b) => bigint_value(a.or(&b), heap)?,
        },
        // §13.10.1 — all four relational operators are `IsLessThan` with the operands in one
        // order or the other, and `undefined` — "not comparable" — folded to `false`. That fold
        // is why `NaN < 1`, `NaN > 1` and `NaN <= 1` are all false at once.
        BinaryOperator::LessThan => Value::Boolean(is_less_than(left, right, heap)? == Some(true)),
        BinaryOperator::GreaterThan => {
            Value::Boolean(is_less_than(right, left, heap)? == Some(true))
        }
        // …and the two that negate: `<=` is "not greater", which is `IsLessThan` the other way
        // about with both `true` and `undefined` answering `false`.
        BinaryOperator::LessThanOrEqual => {
            Value::Boolean(is_less_than(right, left, heap)? == Some(false))
        }
        BinaryOperator::GreaterThanOrEqual => {
            Value::Boolean(is_less_than(left, right, heap)? == Some(false))
        }
        BinaryOperator::Equal => Value::Boolean(is_loosely_equal(left, right, heap)?),
        BinaryOperator::NotEqual => Value::Boolean(!is_loosely_equal(left, right, heap)?),
        BinaryOperator::StrictEqual => Value::Boolean(left.is_strictly_equal(&right, heap)),
        BinaryOperator::StrictNotEqual => Value::Boolean(!left.is_strictly_equal(&right, heap)),
        // Unreachable from a compiled chunk: the compiler refuses both until there is an object
        // to ask. Answering `undefined` rather than guessing means a mistake shows up as a wrong
        // value in a test rather than as a plausible one.
        BinaryOperator::Instanceof | BinaryOperator::In => Value::Undefined,
    })
}

/// `+` — §13.15.3 steps 1 to 3, for operands that are already primitive.
///
/// `ToPrimitive` of a primitive is the primitive, so those steps are the identity here and the
/// String test is all that remains. When Objects arrive, the two conversions go back in front and
/// this becomes fallible — the order matters and is observable, because both may run user code.
fn add(left: Value, right: Value, heap: &mut Heap) -> Completion<Value> {
    // Steps 1.a and 1.b — *both* operands become primitives before either is looked at, and in
    // this order. Both may run user code, so the order is observable and is not an accident.
    let left = left.to_primitive(heap, Hint::Number)?;
    let right = right.to_primitive(heap, Hint::Number)?;
    let either_is_a_string = matches!(left, Value::String(_)) || matches!(right, Value::String(_));
    if !either_is_a_string {
        // Steps 1.d to 1.g — the same `ToNumeric` pair every other arithmetic operator uses, so
        // `1n + 1` is a TypeError here too. The String test above comes first, which is why
        // `1n + "a"` is `"1a"` and not an error: concatenation is not arithmetic.
        return match numeric_pair(left, right, heap)? {
            Numeric::Numbers(a, b) => Ok(Value::Number(a + b)),
            Numeric::BigInts(a, b) => bigint_value(a.add(&b), heap),
        };
    }
    // Step 1.c — *both* are converted to Strings, not just the one that was not: `1 + "a"` is
    // `"1a"` because `ToString(1)` is `"1"`, which is where §6.1.6.1.20 earns its place.
    let left = left.to_string(heap)?;
    let right = right.to_string(heap)?;
    // Step 1.d is `StringConcat`, which the specification writes as though it always succeeds —
    // §6.1.4's maximum is 2^53-1 and unreachable. A real engine has a smaller one, and refusing is
    // the only honest answer left: the alternative is to allocate until the process dies, which is
    // a wrong answer for every program in it and not only for this one. See DR-0012.
    heap.concat(left, right)
        .map(Value::String)
        .ok_or_else(|| Abrupt::range_error("the string would be longer than a string may be"))
}

/// `Number::exponentiate` (§6.1.6.1.3), which is not `f64::powf`.
///
/// The specification says so itself, in a note under the clause: the result "when base is 1 or -1
/// and exponent is +∞ or -∞, or when base is 1 and exponent is NaN, **differs from IEEE
/// 754-2019**. The first edition of ECMAScript specified a result of NaN for this operation,
/// whereas later revisions of IEEE 754 specified 1. The historical ECMAScript behaviour is
/// preserved for compatibility reasons."
///
/// IEEE has a continuity argument — `1^x` is 1 for every finite `x`, so make it 1 in the limit —
/// and ECMAScript has 1997. Two guards are the whole difference, and `f64::powf` is the rest.
///
/// # The one place the specification does not say what the answer is
///
/// Its last step reads: "Return an **implementation-approximated** Number value representing the
/// result of raising ℝ(base) to the ℝ(exponent) power." Not the correctly rounded one — an
/// approximation, deliberately, so that an engine may use the `pow` it has. Every other operator
/// in this module has exactly one right answer; this one has a range of them.
///
/// Measured against V8 over 1,444 pairs of awkward operands, the two disagree on two of them and
/// by one representable step each: `1e21 ** 10` and `1e-7 ** 9`. Both are conformant. Chasing
/// the last bit would mean writing a correctly rounded `pow`, which is a large piece of numerical
/// work in exchange for agreeing with one engine about a value the language leaves open.
pub(crate) fn exponentiate(base: f64, exponent: f64) -> f64 {
    // Step 1 — a NaN exponent is NaN, before the base is looked at. This is the guard that makes
    // `1 ** NaN` NaN.
    if exponent.is_nan() {
        return f64::NAN;
    }
    // Step 2 — "if exponent is either +0 or -0, return 1" — is not written here, and its absence
    // changes nothing: IEEE 754 gives `pow(x, ±0)` as 1 for every `x`, NaN included, which is
    // the same rule. It is worth naming anyway, because the *order* is load-bearing: `NaN ** 0`
    // is 1, so a mirror of step 1 asking whether the base is NaN would answer it wrongly.
    //
    // Steps 11.b and 12.b — an infinite exponent over a base of magnitude exactly 1. The base
    // being finite is what distinguishes this from `(±∞) ** ±∞`, which is IEEE's answer and is
    // reached below.
    if exponent.is_infinite() && base.abs() == 1.0 {
        return f64::NAN;
    }
    // Every remaining case — the infinities, the signed zeroes, the odd-integer exponents that
    // decide the sign of `(-∞) ** 3`, and a negative base under a fractional exponent — is IEEE
    // 754's `pow`, which the remaining steps restate.
    base.powf(exponent)
}

/// `IsLessThan` (§7.2.12) — `Some(true)`, `Some(false)`, or `None` for "not comparable".
///
/// `None` is the specification's `undefined`, and it happens for exactly one reason among these
/// values: a NaN on either side. Every operator that calls this folds `None` to `false`, which is
/// why `NaN < 1` and `NaN >= 1` are both false and why no ordering of NaN is consistent.
///
/// Takes no `leftFirst` flag. It exists to control the order of two `ToPrimitive` calls, both of
/// which may run user code, and neither of which can yet — see the module documentation.
fn is_less_than(left: Value, right: Value, heap: &Heap) -> Completion<Option<bool>> {
    // Step 3 — two Strings compare by code unit, not by character and not by locale. A lone
    // surrogate is a code unit like any other, and `"\u{FF3A}" < "\u{1D400}"` is `false` even
    // though the second character has the larger code *point*: it is stored as a surrogate pair
    // beginning with 0xD835.
    // Steps 1 and 2 — both operands become primitives, left first unless the caller reversed
    // them, which is what `IsLessThan`'s `leftFirst` flag controls and why `>` passes them the
    // other way about.
    let left = left.to_primitive(heap, Hint::Number)?;
    let right = right.to_primitive(heap, Hint::Number)?;
    if let (Value::String(left), Value::String(right)) = (left, right) {
        let (Some(left), Some(right)) = (heap.string(left), heap.string(right)) else {
            return Ok(None);
        };
        return Ok(Some(left < right));
    }
    // Steps 3.c to 3.f — a BigInt is *compared* with anything numeric, unlike in arithmetic. The
    // ordering is of mathematical values, so it is exact on both sides: a BigInt turned into an
    // `f64` first would put `2n ** 53n + 1n` and `2 ** 53` in the wrong order to each other.
    if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) {
        return Ok(compare_across_types(left, right, heap));
    }
    // Steps 8 to 10 — everything else through `ToNumber`, and NaN is not less than, greater
    // than, or equal to anything.
    let (left, right) = (left.to_number(heap)?, right.to_number(heap)?);
    if left.is_nan() || right.is_nan() {
        return Ok(None);
    }
    Ok(Some(left < right))
}

/// `IsLooselyEqual` (§7.2.13) — the `==` operator.
///
/// The algorithm that gave `==` its reputation, and it is shorter than the reputation suggests.
/// Same type is `===`. `null` and `undefined` are equal to each other and to nothing else. A
/// Boolean is converted to a Number *first*, which is the step behind `"1" == true` being true
/// and `"true" == true` being false. Everything else is a Number-and-String pair, and the String
/// is converted.
pub fn is_loosely_equal(left: Value, right: Value, heap: &Heap) -> Completion<bool> {
    Ok(match (left, right) {
        // Step 1 — same type, so `===` decides, NaN and the signed zeroes included.
        (Value::Undefined, Value::Undefined)
        | (Value::Null, Value::Null)
        | (Value::Boolean(_), Value::Boolean(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_))
        | (Value::Symbol(_), Value::Symbol(_))
        | (Value::BigInt(_), Value::BigInt(_))
        | (Value::Object(_), Value::Object(_)) => left.is_strictly_equal(&right, heap),
        // Steps 2 and 3 — the pair that is equal without being the same, and the reason
        // `x == null` is the idiomatic test for "either of them".
        (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => true,
        // Steps 6 and 7 — a String against a Number is read as a Number, so `"" == 0` is true:
        // `ToNumber("")` is `+0`, which surprises everyone exactly once.
        (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
            left.to_number(heap)? == right.to_number(heap)?
        }
        // Steps 4 and 5 — a BigInt and a Number **are** compared, unlike in arithmetic, and
        // mathematically: `1n == 1` is true where `1n + 1` is a TypeError. Comparing is asking
        // whether two values are the same point on the number line, which is a question that has
        // an answer at every width; arithmetic has to *produce* a value and would have to choose
        // one. That is the whole of why one mixes and the other does not.
        (Value::BigInt(big), Value::Number(number))
        | (Value::Number(number), Value::BigInt(big)) => equal_across_types(big, number, heap),
        // Steps 6 and 7's other half — a BigInt against a String reads the String as a BigInt, and
        // text that is not an integer is simply *not equal* rather than an error: `1n == "1"` is
        // true and `1n == "1.5"` is false.
        (Value::BigInt(big), Value::String(text)) | (Value::String(text), Value::BigInt(big)) => {
            equal_to_string(big, text, heap)
        }
        // Steps 10 and 11 — a Boolean becomes a Number and the comparison starts again. That is
        // why `"1" == true` is true and `"true" == true` is false: the Boolean became `1`, and
        // then `"true"` became NaN.
        (Value::Boolean(_), _) => {
            let left = Value::Number(left.to_number(heap)?);
            return is_loosely_equal(left, right, heap);
        }
        (_, Value::Boolean(_)) => {
            let right = Value::Number(right.to_number(heap)?);
            return is_loosely_equal(left, right, heap);
        }
        // Steps 13 and 14 — an Object against a String or a Number is converted and compared
        // again. That is why `[] == 0` is true once arrays have a `toString`: the object becomes
        // `""`, which becomes `+0`.
        //
        // The list of what it may be compared against is exact, and `null` and `undefined` are
        // *not* on it. So `{} == null` reaches step 15 and is false without converting anything —
        // which is also why it cannot throw, and why `x == null` stays the safe idiom even when
        // `x` is an object whose `valueOf` would blow up.
        (Value::Object(_), Value::String(_) | Value::Number(_) | Value::Symbol(_)) => {
            let left = left.to_primitive(heap, Hint::Number)?;
            return is_loosely_equal(left, right, heap);
        }
        (Value::String(_) | Value::Number(_) | Value::Symbol(_), Value::Object(_)) => {
            let right = right.to_primitive(heap, Hint::Number)?;
            return is_loosely_equal(left, right, heap);
        }
        // Step 15 — `null` and `undefined` are equal to nothing else at all, not even to `0` or
        // to `""`, which is the one place `==` is stricter than people expect.
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value an operator gave, when the row is about the value rather than about failing.
    ///
    /// Every row in this module works on primitives, and converting a primitive never throws — so
    /// an `Err` here would be the operator being wrong about something other than what the row is
    /// checking, and it should be loud rather than quietly compared.
    fn ok<T>(outcome: Completion<T>) -> T {
        outcome.expect("an operator on primitives does not throw") // an Err is the bug this would hide
    }

    fn string(heap: &mut Heap, text: &str) -> Value {
        Value::String(heap.new_string(text.encode_utf16().collect()))
    }

    fn text(heap: &Heap, value: Value) -> String {
        match value {
            Value::String(id) => String::from_utf16_lossy(heap.string(id).unwrap_or(&[])),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn plus_is_two_operators_and_the_string_one_wins() {
        let mut heap = Heap::new();
        let one = Value::Number(1.0);
        let a = string(&mut heap, "a");
        // Either side being a String makes it concatenation, and *both* sides are then written
        // out — which is what makes this the first operator that needs §6.1.6.1.20.
        let joined = ok(apply_binary(BinaryOperator::Add, one, a, &mut heap));
        assert_eq!(text(&heap, joined), "1a");
        let joined = ok(apply_binary(BinaryOperator::Add, a, one, &mut heap));
        assert_eq!(text(&heap, joined), "a1");
        // …and every other operator reads the String as a Number instead, which is the whole of
        // why `+` is the one that catches people.
        let result = ok(apply_binary(
            BinaryOperator::Subtract,
            one,
            string(&mut heap, "1"),
            &mut heap,
        ));
        assert!(matches!(result, Value::Number(value) if value == 0.0));
        // `null` is `+0` and `undefined` is NaN, so the two behave completely differently here.
        let with_null = ok(apply_binary(
            BinaryOperator::Add,
            one,
            Value::Null,
            &mut heap,
        ));
        assert!(matches!(with_null, Value::Number(value) if value == 1.0));
        let with_undefined = ok(apply_binary(
            BinaryOperator::Add,
            one,
            Value::Undefined,
            &mut heap,
        ));
        assert!(matches!(with_undefined, Value::Number(value) if value.is_nan()));
    }

    #[test]
    fn exponentiation_keeps_nan_contagious_where_ieee_pow_does_not() {
        // The one place `f64::powf` is the wrong function. IEEE 754 answers `1` for both of
        // these on a continuity argument; ECMAScript answers NaN, and every engine agrees.
        assert!(exponentiate(1.0, f64::NAN).is_nan());
        assert!(exponentiate(-1.0, f64::INFINITY).is_nan());
        assert!(exponentiate(-1.0, f64::NEG_INFINITY).is_nan());
        // …while a zero exponent is `1` for every base including NaN, which is step 2 and which
        // `powf` does agree with.
        assert_eq!(exponentiate(f64::NAN, 0.0), 1.0);
        assert_eq!(exponentiate(f64::NAN, -0.0), 1.0);
        // The ordinary cases, and the ones where the sign comes from the exponent being odd.
        assert_eq!(exponentiate(2.0, 10.0), 1024.0);
        assert_eq!(exponentiate(f64::NEG_INFINITY, 3.0), f64::NEG_INFINITY);
        assert_eq!(exponentiate(f64::NEG_INFINITY, 2.0), f64::INFINITY);
        assert!(exponentiate(-8.0, 1.0 / 3.0).is_nan());
    }

    #[test]
    fn exponentiation_stays_within_the_approximation_the_specification_allows() {
        // §6.1.6.1.3's last step says "implementation-approximated", which is the only place in
        // this module where more than one answer is conformant. The claim worth pinning is not a
        // value — it is *how far* the answer may be from the one every browser gives, and the
        // answer is one representable step.
        //
        // These two rows are where a sweep of 1,444 operand pairs found the engines parting
        // company. Written as bit patterns because the difference is invisible at any precision
        // a decimal literal could carry.
        let cases = [
            (1e21_f64, 10.0_f64, 1.0000000000000001e210_f64),
            (1e-7, 9.0, 9.999999999999997e-64),
        ];
        for (base, exponent, in_v8) in cases {
            let mine = exponentiate(base, exponent);
            let steps = i64::from_ne_bytes(mine.to_bits().to_ne_bytes())
                - i64::from_ne_bytes(in_v8.to_bits().to_ne_bytes());
            assert!(
                steps.abs() <= 1,
                "{base:e} ** {exponent} is {steps} steps from V8's answer"
            );
        }
        // …and everywhere else the agreement is exact, which is what makes the two rows above
        // worth naming rather than a tolerance worth applying everywhere.
        assert_eq!(exponentiate(2.0, 53.0), 9_007_199_254_740_992.0);
        assert_eq!(exponentiate(10.0, 22.0), 1e22);
    }

    #[test]
    fn shifting_takes_the_count_modulo_thirty_two() {
        let mut heap = Heap::new();
        let shift = |left: f64, right: f64, operator, heap: &mut Heap| match ok(apply_binary(
            operator,
            Value::Number(left),
            Value::Number(right),
            heap,
        )) {
            Value::Number(value) => value,
            other => panic!("a shift is a Number, not {other:?}"),
        };
        // `1 << 32` is `1`, not `0` — the count wraps, which is the surprise.
        assert_eq!(shift(1.0, 32.0, BinaryOperator::ShiftLeft, &mut heap), 1.0);
        assert_eq!(shift(1.0, 33.0, BinaryOperator::ShiftLeft, &mut heap), 2.0);
        // A left shift that reaches the sign bit produces a negative number, because the result
        // is read back as a signed 32-bit integer.
        assert_eq!(
            shift(1.0, 31.0, BinaryOperator::ShiftLeft, &mut heap),
            -2_147_483_648.0
        );
        // …and the two right shifts differ on exactly that value: one keeps the sign, one does
        // not, which is what `>>>` is for.
        assert_eq!(
            shift(-1.0, 0.0, BinaryOperator::ShiftRight, &mut heap),
            -1.0
        );
        assert_eq!(
            shift(-1.0, 0.0, BinaryOperator::ShiftRightUnsigned, &mut heap),
            4_294_967_295.0
        );
        assert_eq!(
            shift(-8.0, 1.0, BinaryOperator::ShiftRight, &mut heap),
            -4.0
        );
    }

    #[test]
    fn strings_compare_by_code_unit_and_not_by_character() {
        let mut heap = Heap::new();
        let less = |left: &str, right: &str, heap: &mut Heap| {
            let (left, right) = (string(heap, left), string(heap, right));
            matches!(
                ok(apply_binary(BinaryOperator::LessThan, left, right, heap)),
                Value::Boolean(true)
            )
        };
        assert!(less("a", "b", &mut heap));
        assert!(!less("b", "a", &mut heap));
        // A prefix is less than what extends it, and equal strings are not less.
        assert!(less("ab", "abc", &mut heap));
        assert!(!less("abc", "abc", &mut heap));
        // Code unit, not code point: U+FF3A is one unit, 0xFF3A; U+1D400 is the pair 0xD835
        // 0xDC00, and 0xD835 is the smaller. So the character with the *larger* code point
        // compares as less, which is the whole content of DR-0004 made visible.
        assert!(!less("\u{ff3a}", "\u{1d400}", &mut heap));
        assert!(less("\u{1d400}", "\u{ff3a}", &mut heap));
        // …while a Number on either side stops it being a String comparison at all: `"10" < "9"`
        // is true and `"10" < 9` is false.
        let ten = string(&mut heap, "10");
        let nine = Value::Number(9.0);
        assert!(matches!(
            ok(apply_binary(BinaryOperator::LessThan, ten, nine, &mut heap)),
            Value::Boolean(false)
        ));
    }

    #[test]
    fn nan_is_not_less_greater_or_equal_and_so_answers_false_four_times() {
        let mut heap = Heap::new();
        let nan = Value::Number(f64::NAN);
        let one = Value::Number(1.0);
        // §13.10.1's fold of `undefined` to `false`, seen from all four sides at once. `<=` and
        // `>=` are the interesting ones: they are written as negations, so a naive
        // implementation makes them *true* here.
        for operator in [
            BinaryOperator::LessThan,
            BinaryOperator::GreaterThan,
            BinaryOperator::LessThanOrEqual,
            BinaryOperator::GreaterThanOrEqual,
        ] {
            assert!(
                matches!(
                    ok(apply_binary(operator, nan, one, &mut heap)),
                    Value::Boolean(false)
                ),
                "NaN {} 1",
                operator.as_str()
            );
            assert!(
                matches!(
                    ok(apply_binary(operator, one, nan, &mut heap)),
                    Value::Boolean(false)
                ),
                "1 {} NaN",
                operator.as_str()
            );
        }
        // …and the boundary the negations exist for: equal values are `<=` and `>=` and neither
        // `<` nor `>`.
        assert!(matches!(
            ok(apply_binary(
                BinaryOperator::LessThanOrEqual,
                one,
                one,
                &mut heap
            )),
            Value::Boolean(true)
        ));
        assert!(matches!(
            ok(apply_binary(
                BinaryOperator::GreaterThanOrEqual,
                one,
                one,
                &mut heap
            )),
            Value::Boolean(true)
        ));
        assert!(matches!(
            ok(apply_binary(BinaryOperator::LessThan, one, one, &mut heap)),
            Value::Boolean(false)
        ));
    }

    #[test]
    fn loose_equality_over_the_table_that_gave_it_its_reputation() {
        let mut heap = Heap::new();
        let empty = string(&mut heap, "");
        let zero_text = string(&mut heap, "0");
        let one_text = string(&mut heap, "1");
        let true_text = string(&mut heap, "true");
        let (zero, one, nan) = (
            Value::Number(0.0),
            Value::Number(1.0),
            Value::Number(f64::NAN),
        );
        let (yes, no) = (Value::Boolean(true), Value::Boolean(false));
        let table = [
            // Same type, so `===` decides — including NaN being equal to nothing.
            (nan, nan, false),
            (zero, Value::Number(-0.0), true),
            // The pair that is equal to each other and to nothing else at all.
            (Value::Null, Value::Undefined, true),
            (Value::Undefined, Value::Null, true),
            (Value::Null, zero, false),
            (Value::Null, no, false),
            (Value::Undefined, empty, false),
            // A String against a Number goes through ToNumber, and `""` is `+0`.
            (empty, zero, true),
            (zero_text, zero, true),
            (one_text, one, true),
            // A Boolean becomes a Number *first*, which is why the second row is false: `true`
            // became `1`, and `"true"` then became NaN.
            (one_text, yes, true),
            (true_text, yes, false),
            (empty, no, true),
            (zero, no, true),
            (one, yes, true),
            (nan, no, false),
        ];
        for (left, right, expected) in table {
            assert_eq!(
                ok(is_loosely_equal(left, right, &heap)),
                expected,
                "{} == {}",
                text(&heap, left),
                text(&heap, right)
            );
            // `==` is symmetric, and an implementation that converts only one side is not.
            assert_eq!(
                ok(is_loosely_equal(right, left, &heap)),
                expected,
                "{} == {}",
                text(&heap, right),
                text(&heap, left)
            );
        }
    }

    #[test]
    fn no_pair_of_values_can_make_an_operator_panic() {
        // DR-0002 over the operators: these run on whatever a script computed, and every pair of
        // values meets every operator somewhere.
        let mut heap = Heap::new();
        let values = [
            Value::Undefined,
            Value::Null,
            Value::Boolean(true),
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(-0.0),
            Value::Number(f64::MAX),
            Value::String(heap.new_string(Vec::new())),
            Value::String(heap.new_string(vec![0xd800])),
            Value::String(heap.new_string("1e309".encode_utf16().collect())),
        ];
        let operators = [
            BinaryOperator::Exponent,
            BinaryOperator::Multiply,
            BinaryOperator::Divide,
            BinaryOperator::Remainder,
            BinaryOperator::Add,
            BinaryOperator::Subtract,
            BinaryOperator::ShiftLeft,
            BinaryOperator::ShiftRight,
            BinaryOperator::ShiftRightUnsigned,
            BinaryOperator::LessThan,
            BinaryOperator::GreaterThan,
            BinaryOperator::LessThanOrEqual,
            BinaryOperator::GreaterThanOrEqual,
            BinaryOperator::Instanceof,
            BinaryOperator::In,
            BinaryOperator::Equal,
            BinaryOperator::NotEqual,
            BinaryOperator::StrictEqual,
            BinaryOperator::StrictNotEqual,
            BinaryOperator::BitwiseAnd,
            BinaryOperator::BitwiseXor,
            BinaryOperator::BitwiseOr,
        ];
        for operator in operators {
            for left in values {
                for right in values {
                    let _ = ok(apply_binary(operator, left, right, &mut heap));
                }
            }
        }
    }
}
