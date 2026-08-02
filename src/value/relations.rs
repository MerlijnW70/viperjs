//! The three equality relations, and why there are three.
//!
//! §7.2.10, §7.2.11 and §7.2.14 disagree on exactly the two values everyone gets wrong:
//!
//! | | `NaN` vs `NaN` | `+0` vs `-0` |
//! | --- | --- | --- |
//! | [`Value::is_strictly_equal`] — `===` | false | **true** |
//! | [`Value::same_value`] — `Object.is` | **true** | false |
//! | [`Value::same_value_zero`] — `includes` | **true** | **true** |
//!
//! None of them converts anything, which is why none of them can throw — and why they are here
//! rather than with the conversions. `==` is a fourth relation and *does* convert, so it lives
//! with the operators.

use crate::heap::Heap;
use crate::value::Value;

impl Value {
    /// `IsStrictlyEqual` (§7.2.14) — the `===` operator.
    ///
    /// Values of different types are never strictly equal, so this is the one relation where
    /// `NaN === NaN` is false: §6.1.6.1.13's `Number::equal` is IEEE comparison, under which a
    /// NaN equals nothing and the two zeroes equal each other.
    pub fn is_strictly_equal(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left == right,
            _ => self.same_value_non_number(other, heap),
        }
    }

    /// `SameValue` (§7.2.10) — what `Object.is` asks.
    ///
    /// Differs from `===` on the two values that make the distinction worth having: `NaN` is the
    /// same value as itself, and `+0` is not the same value as `-0`. Both fall out of
    /// §6.1.6.1.14's `Number::sameValue`, which is written in terms of the mathematical values
    /// rather than in terms of IEEE comparison.
    pub fn same_value(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => same_value_number(*left, *right),
            _ => self.same_value_non_number(other, heap),
        }
    }

    /// `SameValueZero` (§7.2.11) — what `Array.prototype.includes` and a `Map` key ask.
    ///
    /// `SameValue` with the zeroes put back together: it is the relation that wanted `NaN` to be
    /// findable without also wanting `-0` to be a different key from `+0`.
    pub fn same_value_zero(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            // §6.1.6.1.15's `Number::sameValueZero`, whose only difference from `sameValue` is
            // that it says the zeroes are the same before it says anything else.
            (Self::Number(left), Self::Number(right)) => {
                (left.is_nan() && right.is_nan()) || left == right
            }
            _ => self.same_value_non_number(other, heap),
        }
    }

    /// `SameValueNonNumber` (§7.2.12) — the part the three relations agree on.
    ///
    /// Every type but Number compares the same way in all three, which is why they share this
    /// and differ only in which `Number::` operation they reach for. Numbers are asked here
    /// too, and answer `false` for a mismatched type, so no caller has to check first.
    fn same_value_non_number(&self, other: &Self, heap: &Heap) -> bool {
        match (self, other) {
            (Self::Undefined, Self::Undefined) | (Self::Null, Self::Null) => true,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            // §7.2.12: two Strings are the same value when they are the same *sequence of code
            // units*, which is a comparison of contents and not of handles — nothing is
            // interned. Two handles this heap does not know are not equal to each other either,
            // since `None == None` is not what is being asked: the units are.
            (Self::String(left), Self::String(right)) => {
                match (heap.string(*left), heap.string(*right)) {
                    (Some(left), Some(right)) => left == right,
                    _ => false,
                }
            }
            // §7.2.12 again, and the other way about: an Object is the same value as itself and
            // as nothing else. Two objects with identical properties are two objects, which is
            // why `{} === {}` is false — so this compares the handle where the String arm above
            // compares contents, and that difference *is* the difference between the two types.
            (Self::Object(left), Self::Object(right)) => left == right,
            // …and a Symbol is compared the same way, for the same reason and to a different end.
            // §7.2.12 says two Symbols are the same value when they *are* the same Symbol, which
            // is what makes `Symbol("a") === Symbol("a")` false and what makes a Symbol usable as
            // a key nothing else can collide with. The description takes no part.
            (Self::Symbol(left), Self::Symbol(right)) => left == right,
            // §7.2.12's `BigInt::sameValue` — the *digits*, like a String and unlike an Object. A
            // BigInt is a primitive whose identity is its value, so two handles to equal
            // magnitudes are equal however they were arrived at.
            (Self::BigInt(left), Self::BigInt(right)) => heap
                .bigint(*left)
                .zip(heap.bigint(*right))
                .is_some_and(|(left, right)| left == right),
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
