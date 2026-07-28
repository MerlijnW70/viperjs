//! The ECMAScript language values (ECMA-262 §6.1), and every question one can be asked without
//! a heap.
//!
//! # Which types are here
//!
//! §6.1 names eight: Undefined, Null, Boolean, String, Symbol, Number, BigInt and Object. Four
//! of them are here. The other four are not missing so much as *homeless*: a String is a
//! sequence of code units, a Symbol has identity, a BigInt is unbounded, an Object has
//! properties — each needs somewhere to live, and where that is, is what `heap.rs` decides. This
//! module deliberately does not guess at it. Adding a variant later is a change the compiler
//! forces every `match` here to answer, which is the point of writing them exhaustively.
//!
//! So what is here is the part that is a *value* in the plainest sense: it fits in a register,
//! it is `Copy`, and nothing about it can fail.
//!
//! # Why there is no `PartialEq`
//!
//! Because ECMAScript has three equality relations and they disagree, on exactly the two values
//! anyone ever gets wrong:
//!
//! | | `NaN` vs `NaN` | `+0` vs `-0` |
//! | --- | --- | --- |
//! | [`Value::is_strictly_equal`] — `===` (§7.2.14) | false | **true** |
//! | [`Value::same_value`] — `Object.is` (§7.2.10) | **true** | false |
//! | [`Value::same_value_zero`] — `includes` (§7.2.11) | **true** | **true** |
//!
//! Rust's derived `==` on an `f64` happens to be the first of the three. Deriving it would
//! therefore be *correct today* and would read as no choice at all — so the next person to write
//! `a == b` would get `===` without having decided to, and the variant added after that could
//! quietly make the coincidence false. Each relation is a named method instead, and the name
//! says which question is being asked.
//!
//! # Where the conversions stop
//!
//! [`Value::to_number`] is total here and will not stay that way: §7.1.4 throws a **TypeError**
//! for a Symbol and for a BigInt, and reaches user code through `ToPrimitive` for an Object. All
//! three arrive with the types that need them, and the signature changes then. It is not fallible
//! *now* because a `Result` whose `Err` no input can produce is a branch no test could ever
//! reach — the same argument `src/span.rs` makes for `end.max(start)`.

/// An ECMAScript language value (§6.1), for the types that need no heap.
///
/// See the module documentation for which of §6.1's eight types are here and why the rest are
/// not, and for why this has no `PartialEq`.
#[derive(Debug, Clone, Copy)]
pub enum Value {
    /// `undefined` — §6.1.1, the value of a binding that has one and has not been given a value.
    Undefined,
    /// `null` — §6.1.2, the value that represents the intentional absence of an object.
    Null,
    /// `true` or `false` — §6.1.3.
    Boolean(bool),
    /// A Number — §6.1.6.1, an IEEE 754-2019 binary64 value.
    ///
    /// Every `f64` is a Number and every Number is an `f64`, with one wrinkle that costs work
    /// elsewhere: the specification has exactly **one** NaN, and IEEE 754 has 2^53 - 2 of them.
    /// Nothing here may let two NaNs be told apart, which is why [`Value::same_value`] asks
    /// `is_nan` of both rather than comparing bits.
    Number(f64),
}

impl Value {
    /// The string `typeof` gives for this value (§13.5.3).
    ///
    /// `typeof null` is `"object"`, which is not a bug being reproduced but the specification's
    /// own table: it was a mistake in 1995 and became load-bearing before anyone could fix it.
    /// The table is here rather than in the operator because it is a fact about the value.
    pub fn type_of(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "object",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
        }
    }

    /// `ToBoolean` (§7.1.2) — the value's truthiness.
    ///
    /// Total for every type, present and future: §7.1.2's table has no row that throws, which is
    /// what makes `if (x)` unable to fail however strange `x` is.
    pub fn to_boolean(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            // "If argument is +0𝔽, -0𝔽, or NaN, return false; otherwise return true." One
            // comparison covers both zeroes, since `-0.0 == 0.0`, and NaN fails every comparison
            // including this one — so `!= 0.0` would be wrong and `== 0.0 || is_nan` is not.
            Self::Number(number) => *number != 0.0 && !number.is_nan(),
        }
    }

    /// `ToNumber` (§7.1.4).
    ///
    /// `null` is `+0` and `undefined` is `NaN`, which is the difference behind `null + 1 === 1`
    /// and `undefined + 1` being `NaN`. See the module documentation for why this cannot fail
    /// yet and will.
    pub fn to_number(&self) -> f64 {
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(true) => 1.0,
            Self::Boolean(false) => 0.0,
            Self::Number(number) => *number,
        }
    }

    /// `ToIntegerOrInfinity` (§7.1.5) — the value truncated towards zero, or ±∞.
    ///
    /// Returns an `f64` because that is what the operation returns: "an integral Number, or
    /// +∞, or -∞". Callers that need a bounded integer clamp it themselves, which is what
    /// every caller in the specification does and with a different bound each time.
    ///
    /// The three values that collapse to `+0` are stated as one step in §7.1.5 and are worth
    /// naming: `NaN`, `+0` and `-0`. That is why `-0.5` gives `+0` and not `-0`.
    pub fn to_integer_or_infinity(&self) -> f64 {
        let number = self.to_number();
        if number.is_nan() {
            return 0.0;
        }
        // §7.1.5's steps 3 and 4 return the infinities as themselves, and are not written out:
        // `trunc` returns an infinity unchanged, so they leave by the last line already. NaN
        // above is not like them — `trunc` returns NaN too, and NaN is not the answer.
        let truncated = number.trunc();
        // Two of §7.1.5's steps in one branch, because they give one answer. Step 5 returns
        // `truncate(ℝ(number))` — a *mathematical* integer, which has no signed zero — so `-0.5`
        // truncates to 0 and not to `-0`, where Rust's `trunc` keeps the sign. Step 2's `+0` and
        // `-0` reach here truncating to zero and leave by the same door, so writing step 2 out
        // as well would be a branch no input could tell from its absence.
        if truncated == 0.0 { 0.0 } else { truncated }
    }

    /// `ToInt32` (§7.1.6) — the value as a signed 32-bit integer, wrapping.
    ///
    /// This is what every bitwise operator does to its operands, so `2147483648 | 0` is
    /// `-2147483648` and `4294967296 | 0` is `0`.
    pub fn to_int32(&self) -> i32 {
        // §7.1.6 step 5: an `int32bit` at or above 2^31 comes back 2^32 lower. `as i32` on a
        // `u32` is that reinterpretation exactly, and is the one Rust cast that is defined to
        // wrap rather than saturate.
        self.to_uint32() as i32
    }

    /// `ToUint32` (§7.1.7) — the value as an unsigned 32-bit integer, wrapping.
    ///
    /// # Why this is exact, and why the obvious version is not
    ///
    /// §7.1.7 asks for `truncate(ℝ(number)) modulo 2^32` — arithmetic on the *mathematical*
    /// value, which for a large `f64` is an integer of up to 309 digits. Casting through an
    /// integer type cannot do it: since Rust 1.45 a float-to-integer `as` saturates, so
    /// `1e300 as u32` is `u32::MAX` where the answer is `0`.
    ///
    /// Doing it in `f64` is exact all the same, for two reasons that hold together:
    ///
    /// - `trunc` is exact. Every `f64` of magnitude 2^52 or more is already an integer, so
    ///   `trunc` returns it unchanged; below that the truncation is representable.
    /// - `%` on `f64` is IEEE 754's `remainder` after truncated division — `fmod` — which the
    ///   standard requires to be **exact**, with no rounding at any magnitude.
    ///
    /// So the remainder is the mathematical one, and it lands in `(-2^32, 2^32)` where a single
    /// addition brings it into range and `as u32` is a lossless conversion of an integral `f64`.
    pub fn to_uint32(&self) -> u32 {
        const MODULUS: f64 = 4_294_967_296.0; // 2^32

        // §7.1.7 step 2 sends every non-finite value and both zeroes to `+0`, and is not written
        // out: the arithmetic below already answers `0` for all five, so a step for them would be
        // a branch no input could tell from its absence. It rests on two facts rather than on
        // luck — `±∞ % y` and `NaN % y` are both NaN, and a float-to-integer `as` in Rust
        // saturates, which sends NaN to `0`. The behaviour is pinned by tests even though the
        // branch that would have stated it is gone.
        let remainder = self.to_number().trunc() % MODULUS;
        // The specification's `modulo` takes the sign of the divisor and is therefore never
        // negative; `%` in Rust takes the sign of the dividend and so can be. One addition is
        // the whole difference between the two, and it is exact for the same reason `%` is.
        let in_range = if remainder < 0.0 {
            remainder + MODULUS
        } else {
            remainder
        };
        in_range as u32
    }

    /// `IsStrictlyEqual` (§7.2.14) — the `===` operator.
    ///
    /// Values of different types are never strictly equal, so this is the one relation where
    /// `NaN === NaN` is false: §6.1.6.1.13's `Number::equal` is IEEE comparison, under which a
    /// NaN equals nothing and the two zeroes equal each other.
    pub fn is_strictly_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Undefined, Self::Undefined) | (Self::Null, Self::Null) => true,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            (Self::Number(left), Self::Number(right)) => left == right,
            _ => false,
        }
    }

    /// `SameValue` (§7.2.10) — what `Object.is` asks.
    ///
    /// Differs from `===` on the two values that make the distinction worth having: `NaN` is the
    /// same value as itself, and `+0` is not the same value as `-0`. Both fall out of
    /// §6.1.6.1.14's `Number::sameValue`, which is written in terms of the mathematical values
    /// rather than in terms of IEEE comparison.
    pub fn same_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => same_value_number(*left, *right),
            _ => self.same_value_non_number(other),
        }
    }

    /// `SameValueZero` (§7.2.11) — what `Array.prototype.includes` and a `Map` key ask.
    ///
    /// `SameValue` with the zeroes put back together: it is the relation that wanted `NaN` to be
    /// findable without also wanting `-0` to be a different key from `+0`.
    pub fn same_value_zero(&self, other: &Self) -> bool {
        match (self, other) {
            // §6.1.6.1.15's `Number::sameValueZero`, whose only difference from `sameValue` is
            // that it says the zeroes are the same before it says anything else.
            (Self::Number(left), Self::Number(right)) => {
                (left.is_nan() && right.is_nan()) || left == right
            }
            _ => self.same_value_non_number(other),
        }
    }

    /// `SameValueNonNumber` (§7.2.12) — the part the three relations agree on.
    ///
    /// Every type but Number compares the same way in all three, which is why they share this
    /// and differ only in which `Number::` operation they reach for. Numbers are asked here
    /// too, and answer `false` for a mismatched type, so no caller has to check first.
    fn same_value_non_number(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Undefined, Self::Undefined) | (Self::Null, Self::Null) => true,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            _ => false,
        }
    }
}

/// `Number::sameValue` (§6.1.6.1.14), which is not `==` and not `total_cmp` either.
///
/// Written out rather than reached for, because both of the obvious shortcuts are wrong in a way
/// that shows up rarely: `==` says two NaNs differ, and comparing bit patterns says two NaNs
/// differ *from each other* — the specification has one NaN where IEEE 754 has millions, and a
/// `0.0 / 0.0` may not carry the same payload as an `f64::NAN` written down.
fn same_value_number(left: f64, right: f64) -> bool {
    // Asked of the left alone. Asking both — `left.is_nan() || right.is_nan()` guarding a
    // `left.is_nan() && right.is_nan()` — gives the same answer for every pair, because a
    // non-NaN falling through compares unequal to a NaN anyway. Two conditions that cannot
    // disagree are one condition written twice.
    if left.is_nan() {
        return right.is_nan();
    }
    // The zeroes are equal under `==` and are not the same value, so the sign settles it. Asked
    // only here: `is_sign_negative` is true of `-NaN` as well, which is why NaN left first.
    if left == 0.0 && right == 0.0 {
        return left.is_sign_negative() == right.is_sign_negative();
    }
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four values that every table in this module has a row for.
    const UNDEFINED: Value = Value::Undefined;
    const NULL: Value = Value::Null;

    fn number(value: f64) -> Value {
        Value::Number(value)
    }

    fn boolean(value: bool) -> Value {
        Value::Boolean(value)
    }

    #[test]
    fn typeof_null_is_object_and_the_rest_say_what_they_are() {
        assert_eq!(UNDEFINED.type_of(), "undefined");
        assert_eq!(boolean(true).type_of(), "boolean");
        assert_eq!(number(1.0).type_of(), "number");
        // §13.5.3's table, and the one entry that surprises everyone who has not met it.
        assert_eq!(NULL.type_of(), "object");
    }

    #[test]
    fn to_boolean_is_false_for_both_zeroes_and_for_nan_and_true_for_everything_else() {
        assert!(!UNDEFINED.to_boolean());
        assert!(!NULL.to_boolean());
        assert!(boolean(true).to_boolean());
        assert!(!boolean(false).to_boolean());
        // The three numbers §7.1.2 names, and the sign is not one of the things it asks about.
        assert!(!number(0.0).to_boolean());
        assert!(!number(-0.0).to_boolean());
        assert!(!number(f64::NAN).to_boolean());
        // …and everything else is true, including the values that look like nothing.
        assert!(number(1.0).to_boolean());
        assert!(number(-1.0).to_boolean());
        assert!(number(f64::MIN_POSITIVE).to_boolean());
        assert!(number(f64::INFINITY).to_boolean());
        assert!(number(f64::NEG_INFINITY).to_boolean());
    }

    #[test]
    fn to_number_gives_null_a_zero_and_undefined_a_nan() {
        // The pair behind `null + 1 === 1` and `undefined + 1` being NaN, which is the whole of
        // why the two are not interchangeable in arithmetic.
        assert_eq!(NULL.to_number(), 0.0);
        assert!(UNDEFINED.to_number().is_nan());
        assert_eq!(boolean(true).to_number(), 1.0);
        assert_eq!(boolean(false).to_number(), 0.0);
        // A Number is returned unchanged, including the one that is not equal to itself.
        assert_eq!(number(1.5).to_number(), 1.5);
        assert!(number(f64::NAN).to_number().is_nan());
        // …and including the sign of zero, which `to_integer_or_infinity` then discards.
        assert!(number(-0.0).to_number().is_sign_negative());
    }

    #[test]
    fn to_integer_or_infinity_truncates_towards_zero_and_keeps_the_infinities() {
        assert_eq!(number(3.9).to_integer_or_infinity(), 3.0);
        assert_eq!(number(-3.9).to_integer_or_infinity(), -3.0);
        assert_eq!(number(3.0).to_integer_or_infinity(), 3.0);
        // §7.1.5 collapses NaN and both zeroes to `+0`, so a fraction that truncates to zero
        // comes back *positive* zero however it was signed.
        assert!(!number(-0.5).to_integer_or_infinity().is_sign_negative());
        assert!(!number(-0.0).to_integer_or_infinity().is_sign_negative());
        assert_eq!(number(f64::NAN).to_integer_or_infinity(), 0.0);
        assert!(!number(f64::NAN).to_integer_or_infinity().is_nan());
        // The infinities are returned as themselves — the operation is named for it.
        assert_eq!(
            number(f64::INFINITY).to_integer_or_infinity(),
            f64::INFINITY
        );
        assert_eq!(
            number(f64::NEG_INFINITY).to_integer_or_infinity(),
            f64::NEG_INFINITY
        );
        // The other types go through `ToNumber` first.
        assert_eq!(boolean(true).to_integer_or_infinity(), 1.0);
        assert_eq!(UNDEFINED.to_integer_or_infinity(), 0.0);
    }

    #[test]
    fn to_uint32_wraps_by_the_mathematical_modulo_at_every_magnitude() {
        assert_eq!(number(0.0).to_uint32(), 0);
        assert_eq!(number(1.0).to_uint32(), 1);
        assert_eq!(number(4_294_967_295.0).to_uint32(), 4_294_967_295);
        // One past the modulus wraps to zero, which is the whole of the operation.
        assert_eq!(number(4_294_967_296.0).to_uint32(), 0);
        assert_eq!(number(4_294_967_297.0).to_uint32(), 1);
        // A negative comes back as its positive residue: the specification's `modulo` takes the
        // sign of the divisor where Rust's `%` takes the sign of the dividend.
        assert_eq!(number(-1.0).to_uint32(), 4_294_967_295);
        assert_eq!(number(-4_294_967_296.0).to_uint32(), 0);
        // The fraction goes before the modulo, not after.
        assert_eq!(number(-1.5).to_uint32(), 4_294_967_295);
        assert_eq!(number(1.9).to_uint32(), 1);
        // §7.1.7 step 2 sends every non-finite value to zero rather than to a saturated bound,
        // which is what a cast through an integer type would have produced.
        assert_eq!(number(f64::NAN).to_uint32(), 0);
        assert_eq!(number(f64::INFINITY).to_uint32(), 0);
        assert_eq!(number(f64::NEG_INFINITY).to_uint32(), 0);
        // Far past anything an integer type could hold, where the exactness argument is the
        // only thing keeping the answer right. 1e300 is a multiple of 2^32 and so is zero;
        // `1e300 as u32` in Rust is `u32::MAX`.
        assert_eq!(number(1e300).to_uint32(), 0);
        assert_eq!(number(f64::MAX).to_uint32(), 0);
        // 2^53 is the last integer with a neighbour, and 2^53 + 2 the next one representable.
        assert_eq!(number(9_007_199_254_740_992.0).to_uint32(), 0);
        assert_eq!(number(9_007_199_254_740_994.0).to_uint32(), 2);
    }

    #[test]
    fn to_int32_is_to_uint32_read_as_signed() {
        assert_eq!(number(1.0).to_int32(), 1);
        assert_eq!(number(-1.0).to_int32(), -1);
        // The boundary the two operations differ at, and the reason `2147483648 | 0` is negative.
        assert_eq!(number(2_147_483_647.0).to_int32(), 2_147_483_647);
        assert_eq!(number(2_147_483_648.0).to_int32(), -2_147_483_648);
        assert_eq!(number(4_294_967_295.0).to_int32(), -1);
        assert_eq!(number(4_294_967_296.0).to_int32(), 0);
        assert_eq!(number(f64::NAN).to_int32(), 0);
        assert_eq!(number(f64::INFINITY).to_int32(), 0);
        assert_eq!(number(1e300).to_int32(), 0);
    }

    #[test]
    fn the_three_equality_relations_disagree_on_nan_and_on_the_signed_zeroes() {
        let nan = number(f64::NAN);
        let plus_zero = number(0.0);
        let minus_zero = number(-0.0);

        // `===` is IEEE comparison: a NaN equals nothing, and the zeroes equal each other.
        assert!(!nan.is_strictly_equal(&nan));
        assert!(plus_zero.is_strictly_equal(&minus_zero));
        // `Object.is` is the other way round on both.
        assert!(nan.same_value(&nan));
        assert!(!plus_zero.same_value(&minus_zero));
        // …and `SameValueZero` takes one from each.
        assert!(nan.same_value_zero(&nan));
        assert!(plus_zero.same_value_zero(&minus_zero));

        // Two NaNs need not share a bit pattern — IEEE 754 has millions and §6.1.6.1 has one —
        // so a relation that compared bits would call these two different values. This one is
        // negative and quiet where `f64::NAN` is positive and quiet; all three relations are
        // asked, and none of them notices.
        let other_nan = number(f64::from_bits(0xfff8_0000_0000_0000));
        assert!(other_nan.same_value(&nan));
        assert!(other_nan.same_value_zero(&nan));
        assert!(!other_nan.is_strictly_equal(&nan));
    }

    #[test]
    fn the_three_relations_over_every_kind_of_number_pair() {
        // The narrative test above says *why* the three differ; this one says what each answers
        // for every shape of pair, including the ordinary ones. Those are the rows that matter
        // most: a relation that got `NaN` and the zeroes right and `1 === 1` wrong would pass
        // every interesting-looking test ever written for it.
        let nan = f64::NAN;
        let inf = f64::INFINITY;
        let table = [
            //  left      right     ===     SameValue  SameValueZero
            (1.0, 1.0, true, true, true),
            (1.0, 2.0, false, false, false),
            (-1.0, -1.0, true, true, true),
            (1.0, -1.0, false, false, false),
            // A NaN on one side only, which is where a condition asked of the wrong operand
            // stops agreeing with one asked of both.
            (nan, 1.0, false, false, false),
            (1.0, nan, false, false, false),
            (nan, nan, false, true, true),
            // The zeroes, together and apart, and against something that is not a zero.
            (0.0, -0.0, true, false, true),
            (-0.0, 0.0, true, false, true),
            (0.0, 0.0, true, true, true),
            (-0.0, -0.0, true, true, true),
            (0.0, 1.0, false, false, false),
            (-0.0, 1.0, false, false, false),
            // The infinities are ordinary values to all three, and are only equal to themselves.
            (inf, inf, true, true, true),
            (inf, -inf, false, false, false),
            (inf, nan, false, false, false),
            (inf, f64::MAX, false, false, false),
        ];
        for (left, right, strict, same, same_zero) in table {
            let left = number(left);
            let right = number(right);
            assert_eq!(
                left.is_strictly_equal(&right),
                strict,
                "=== of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value(&right),
                same,
                "SameValue of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value_zero(&right),
                same_zero,
                "SameValueZero of {left:?} and {right:?}"
            );
        }
    }

    #[test]
    fn every_relation_agrees_about_the_types_that_are_not_numbers() {
        let cases = [
            (UNDEFINED, UNDEFINED, true),
            (NULL, NULL, true),
            (UNDEFINED, NULL, false),
            (boolean(true), boolean(true), true),
            (boolean(true), boolean(false), false),
            // A different type is a different value under all three, and `false` is not `+0`
            // however much `==` would like it to be — that is `IsLooselyEqual`, which is not
            // one of these and arrives with the operator that needs it.
            (boolean(false), number(0.0), false),
            (NULL, number(0.0), false),
            (UNDEFINED, number(f64::NAN), false),
        ];
        for (left, right, expected) in cases {
            assert_eq!(
                left.is_strictly_equal(&right),
                expected,
                "=== of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value(&right),
                expected,
                "SameValue of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value_zero(&right),
                expected,
                "SameValueZero of {left:?} and {right:?}"
            );
        }
    }

    #[test]
    fn no_number_can_make_a_conversion_panic() {
        // DR-0002 applies to a value as much as to source text: these run on whatever a script
        // computed, and every one of them is total.
        let awkward = [
            0.0,
            -0.0,
            f64::NAN,
            -f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN,
            f64::MAX,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            f64::EPSILON,
            9_007_199_254_740_993.0,
            -9_007_199_254_740_993.0,
            1e-323,
            f64::from_bits(0x7ff0_0000_0000_0001), // a signalling NaN
            f64::from_bits(0xfff8_0000_0000_0000), // a negative quiet NaN
        ];
        for value in awkward {
            let value = number(value);
            let _ = value.to_boolean();
            let _ = value.to_number();
            let _ = value.to_integer_or_infinity();
            let _ = value.to_int32();
            let _ = value.to_uint32();
            let _ = value.type_of();
            let _ = value.same_value(&value);
            let _ = value.same_value_zero(&value);
            let _ = value.is_strictly_equal(&value);
        }
    }
}
