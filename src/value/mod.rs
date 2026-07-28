//! The ECMAScript language values (ECMA-262 §6.1), and every question one can be asked without
//! a heap.
//!
//! # Which types are here
//!
//! §6.1 names eight: Undefined, Null, Boolean, String, Symbol, Number, BigInt and Object. Five
//! of them are here. Symbol, BigInt and Object are not missing so much as *homeless* — a Symbol
//! has identity, a BigInt is unbounded, an Object has properties — and each arrives when the
//! heap has somewhere to put it. Adding a variant is a change the compiler forces every `match`
//! here to answer, which is the point of writing them exhaustively.
//!
//! # Why almost everything takes a `&Heap`
//!
//! A String is a handle (DR-0010), so reading one needs the heap it came from, and that
//! parameter spreads to every operation that could meet a String: truthiness asks whether it is
//! empty, equality compares its code units, and `ToNumber` reads a whole grammar out of it.
//!
//! [`Value::type_of`] used to be the exception, because the variant answered on its own. It is
//! not any more, and the reason is worth keeping: §13.5.3 answers `"function"` for an Object with
//! a `[[Call]]`, so `typeof` is the one question about a value's *shape* that an object can still
//! change the answer to.
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
//! # Which conversions can fail, and what they fail with
//!
//! [`Value::to_number`] and [`Value::to_string`] are fallible because an Object has to be turned
//! into a primitive first, and §7.1.1 throws a **TypeError** when it cannot be. [`Value::to_boolean`]
//! is not: §7.1.2's table has no row that throws, which is what makes `if (x)` unable to fail
//! however strange `x` is. The three equality relations are not either — they compare, and never
//! convert.
//!
//! What a failure carries is a [`TypeError`] and not an Error *object*. An object needs a
//! prototype, a prototype belongs to a realm, and a realm is a thing the interpreter has and the
//! value representation should not. So this layer says which error and why, and
//! [`crate::realm`] decides what object stands for it — which is also where the message will
//! acquire a stack trace it cannot have here.
//!
//! # How this module is laid out
//!
//! - `string_to_number` — §7.1.4.1's grammar, which is the only thing here that reads a value
//!   character by character rather than looking it up in a table.
//! - `number_to_string` — §6.1.6.1.20, the other direction, which decides how many digits a
//!   Number needs and where the point goes.
//! - `operators` — what the binary operators mean (§13.15.3, §7.2.12, §7.2.13).
//! - here — [`Value`] itself, the conversions and the three equality relations.
//!
//! The two directions are checked against each other rather than only against a table: reading
//! back what was written must give the same Number, for any Number at all.

mod number_to_string;
mod operators;
mod string_to_number;

pub use self::operators::{apply_binary, is_loosely_equal};

use self::number_to_string::number_to_string;
use self::string_to_number::string_to_number;
use crate::heap::{Heap, ObjectId, StringId};

/// An ECMAScript language value (§6.1).
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
    /// A String — §6.1.4, a sequence of UTF-16 code units living on the heap.
    ///
    /// The handle and not the units: a String is immutable and often shared, and DR-0010 has
    /// the argument for why a heap value is an index. Two Strings with the same contents are
    /// two Strings — nothing is interned — so every comparison here reads the heap.
    String(StringId),
    /// An Object — §6.1.7, a collection of properties with a prototype.
    ///
    /// The only type here whose identity is not its contents: two objects with the same
    /// properties are two objects, and `{} === {}` is false. That is why every equality relation
    /// compares the handle for this variant and the code units for a String.
    Object(ObjectId),
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
    pub fn type_of(&self, heap: &Heap) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "object",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            // §13.5.3's table again, and the one row that has to look inside: an Object is
            // `"object"` unless it has a `[[Call]]`, in which case it is `"function"`. A handle
            // this heap does not know is `"object"`, which is what an object with nothing
            // callable about it would be.
            Self::Object(id) => match heap.object(*id).and_then(crate::heap::Object::call) {
                Some(_) => "function",
                None => "object",
            },
        }
    }

    /// `ToBoolean` (§7.1.2) — the value's truthiness.
    ///
    /// Total for every type, present and future: §7.1.2's table has no row that throws, which is
    /// what makes `if (x)` unable to fail however strange `x` is.
    pub fn to_boolean(&self, heap: &Heap) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            // "If argument is the empty String, return false; otherwise return true." Only the
            // length is asked about, so `"0"` and `"false"` are both true — the two strings
            // every list of JavaScript surprises begins with.
            //
            // A handle this heap does not know is `false`, which is the same answer an empty
            // String gets. See [`Heap::string`] for why that situation is bounded rather than
            // detected, and why no script can produce it.
            Self::String(id) => heap.string(*id).is_some_and(|units| !units.is_empty()),
            // Every object is truthy, without exception and without asking the object. The one
            // famous counter-example — `document.all` — is a host object with an [[IsHTMLDDA]]
            // slot, which §7.1.2 mentions and which no engine outside a browser has.
            Self::Object(_) => true,
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
    pub fn to_number(&self, heap: &Heap) -> Completion<f64> {
        Ok(match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(true) => 1.0,
            Self::Boolean(false) => 0.0,
            Self::Number(number) => *number,
            // §7.1.4 step 1 — an Object is converted to a primitive first, and that is what can
            // fail. `ToPrimitive` of the result is never an Object, so this cannot recur.
            Self::Object(_) => return self.to_primitive(heap, Hint::Number)?.to_number(heap),
            // §7.1.4.1, which is a grammar and not a parse of convenience — see
            // [`string_to_number`] for the four ways it differs from the one the lexer reads.
            // A handle this heap does not know is NaN, which is what the operation says about
            // text that is not a `StrNumericLiteral`.
            Self::String(id) => heap.string(*id).map_or(f64::NAN, string_to_number),
        })
    }

    /// `ToPrimitive` (§7.1.1) — the value as something that is not an Object.
    ///
    /// A primitive is already one, which is every case that succeeds today. For an Object,
    /// §7.1.1 looks for a `Symbol.toPrimitive` method and then §7.1.1.1 for `valueOf` and
    /// `toString`, calls whichever is callable, and throws a **TypeError** if none is.
    ///
    /// Nothing is callable yet — there are no functions — so no property lookup could find a
    /// method to call, and the answer for every Object is the TypeError at the end. Writing the
    /// lookup now would be writing three branches no input could take. It arrives with `[[Call]]`,
    /// and the answer for `{}` changes on its own once `Object.prototype` has a `toString`.
    pub fn to_primitive(&self, heap: &Heap, hint: Hint) -> Completion<Self> {
        let _ = (heap, hint);
        match self {
            Self::Object(_) => Err(TypeError("cannot convert an object to a primitive value")),
            primitive => Ok(*primitive),
        }
    }

    /// `ToString` (§7.1.17) — the String a value is written as.
    ///
    /// Takes the heap by `&mut` because the answer is a String and a String has to live
    /// somewhere; this is the first operation here that *makes* a value rather than reading one.
    /// A String argument is returned unchanged rather than copied, which §7.1.17 says by
    /// returning the argument itself and which is why this may hand back the handle it was given.
    ///
    /// Total for the types that are here, and it will not stay that way for the same reason
    /// [`Value::to_number`] will not: §7.1.17 throws a **TypeError** for a Symbol, and reaches
    /// user code for an Object.
    pub fn to_string(&self, heap: &mut Heap) -> Completion<StringId> {
        // The four constants are spelled out rather than shared, because §7.1.17's table is a
        // table: `String(null)` is `"null"` and not `typeof null`, which is `"object"`.
        let text = match self {
            Self::Undefined => "undefined".to_string(),
            Self::Null => "null".to_string(),
            Self::Boolean(true) => "true".to_string(),
            Self::Boolean(false) => "false".to_string(),
            Self::Number(number) => number_to_string(*number),
            Self::String(id) => return Ok(*id),
            // §7.1.17 step 1 — the same conversion `ToNumber` does, with the other hint. `"" + x`
            // and `1 * x` therefore ask an object two different questions.
            Self::Object(_) => return self.to_primitive(heap, Hint::String)?.to_string(heap),
        };
        // Every one of those is ASCII, so the UTF-16 encoding is a widening and cannot fail.
        Ok(heap.new_string(text.encode_utf16().collect()))
    }

    /// `ToIntegerOrInfinity` (§7.1.5) — the value truncated towards zero, or ±∞.
    ///
    /// Returns an `f64` because that is what the operation returns: "an integral Number, or
    /// +∞, or -∞". Callers that need a bounded integer clamp it themselves, which is what
    /// every caller in the specification does and with a different bound each time.
    ///
    /// The three values that collapse to `+0` are stated as one step in §7.1.5 and are worth
    /// naming: `NaN`, `+0` and `-0`. That is why `-0.5` gives `+0` and not `-0`.
    pub fn to_integer_or_infinity(&self, heap: &Heap) -> Completion<f64> {
        let number = self.to_number(heap)?;
        if number.is_nan() {
            return Ok(0.0);
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
        Ok(if truncated == 0.0 { 0.0 } else { truncated })
    }

    /// `ToInt32` (§7.1.6) — the value as a signed 32-bit integer, wrapping.
    ///
    /// This is what every bitwise operator does to its operands, so `2147483648 | 0` is
    /// `-2147483648` and `4294967296 | 0` is `0`.
    pub fn to_int32(&self, heap: &Heap) -> Completion<i32> {
        // §7.1.6 step 5: an `int32bit` at or above 2^31 comes back 2^32 lower. `as i32` on a
        // `u32` is that reinterpretation exactly, and is the one Rust cast that is defined to
        // wrap rather than saturate.
        Ok(self.to_uint32(heap)? as i32)
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
    pub fn to_uint32(&self, heap: &Heap) -> Completion<u32> {
        const MODULUS: f64 = 4_294_967_296.0; // 2^32

        // §7.1.7 step 2 sends every non-finite value and both zeroes to `+0`, and is not written
        // out: the arithmetic below already answers `0` for all five, so a step for them would be
        // a branch no input could tell from its absence. It rests on two facts rather than on
        // luck — `±∞ % y` and `NaN % y` are both NaN, and a float-to-integer `as` in Rust
        // saturates, which sends NaN to `0`. The behaviour is pinned by tests even though the
        // branch that would have stated it is gone.
        let remainder = self.to_number(heap)?.trunc() % MODULUS;
        // The specification's `modulo` takes the sign of the divisor and is therefore never
        // negative; `%` in Rust takes the sign of the dividend and so can be. One addition is
        // the whole difference between the two, and it is exact for the same reason `%` is.
        let in_range = if remainder < 0.0 {
            remainder + MODULUS
        } else {
            remainder
        };
        Ok(in_range as u32)
    }

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
            _ => false,
        }
    }
}

/// Which primitive an object is being asked for — §7.1.1's `preferredType`.
///
/// Only ever a preference: an object answers with whatever its methods return, and the hint
/// merely decides which method is tried first. `Date` is the one built-in that treats the absence
/// of a hint as `String` rather than `Number`, which is why `date + 1` concatenates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// Prefer a Number — what every arithmetic operator asks for.
    Number,
    /// Prefer a String — what `ToString` and `ToPropertyKey` ask for.
    String,
}

/// A **TypeError** that has happened, named by what it was.
///
/// Not an Error object: an object needs a prototype, a prototype belongs to a realm, and a realm
/// is not something the value representation should know about. [`crate::realm::Realm`] turns one
/// of these into the object a `catch` block receives.
///
/// The message is `&'static str` because every message this layer produces is written in it.
/// Anything that needs to name a value — "x is not a function" — is produced where the value is,
/// which is the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeError(pub &'static str);

/// The result of an operation that may throw — §6.2.4's normal and throw completions.
///
/// `break`, `continue` and `return` are not here: a bytecode compiler turns those into jumps,
/// because it knows at compile time where they go. Only a throw has to travel.
pub type Completion<T> = Result<T, TypeError>;

/// `CanonicalNumericIndexString` (§7.1.21) — the Number `units` spells, if it spells one exactly.
///
/// Answers `None` for a String that is not the *canonical* spelling of any Number. That word is
/// the whole operation: `"1"` is canonical and `"1.0"`, `"01"`, `"1e0"` and `" 1"` are not, because
/// none of them is what `ToString` writes for the Number they read as. It is what keeps `a["01"]`
/// an ordinary named property while `a["1"]` is an element.
///
/// `"-0"` is the one exception the specification writes out, and it is not an accident: `-0` is a
/// Number whose `ToString` is `"0"`, so without step 1 no String would denote it. It is
/// deliberately *not* an index — see [`crate::heap::PropertyKey`] — but it is canonical.
///
/// Here rather than with the keys because it is a conversion, and because it is the one operation
/// that needs both directions of the Number/String correspondence at once: the round trip closing
/// is precisely the question it asks.
pub fn canonical_numeric_index(units: &[u16]) -> Option<f64> {
    // Step 1.
    if units == "-0".encode_utf16().collect::<Vec<_>>() {
        return Some(-0.0);
    }
    // Steps 2 and 3. `ToString(ToNumber(arg)) is arg` — an identity, not a comparison of value:
    // a String that reads as a Number and is written back differently is not canonical, and NaN
    // is caught by the same test rather than by a rule of its own, since `"NaN"` is written back
    // as `"NaN"` and so *is* canonical.
    let number = string_to_number(units);
    let written = number_to_string(number);
    if written.encode_utf16().eq(units.iter().copied()) {
        return Some(number);
    }
    None
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
    use crate::value::apply_binary;

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
        let heap = Heap::new();
        assert_eq!(UNDEFINED.type_of(&heap), "undefined");
        assert_eq!(boolean(true).type_of(&heap), "boolean");
        assert_eq!(number(1.0).type_of(&heap), "number");
        // §13.5.3's table, and the one entry that surprises everyone who has not met it.
        assert_eq!(NULL.type_of(&heap), "object");
    }

    #[test]
    fn to_boolean_is_false_for_both_zeroes_and_for_nan_and_true_for_everything_else() {
        let heap = Heap::new();
        assert!(!UNDEFINED.to_boolean(&heap));
        assert!(!NULL.to_boolean(&heap));
        assert!(boolean(true).to_boolean(&heap));
        assert!(!boolean(false).to_boolean(&heap));
        // The three numbers §7.1.2 names, and the sign is not one of the things it asks about.
        assert!(!number(0.0).to_boolean(&heap));
        assert!(!number(-0.0).to_boolean(&heap));
        assert!(!number(f64::NAN).to_boolean(&heap));
        // …and everything else is true, including the values that look like nothing.
        assert!(number(1.0).to_boolean(&heap));
        assert!(number(-1.0).to_boolean(&heap));
        assert!(number(f64::MIN_POSITIVE).to_boolean(&heap));
        assert!(number(f64::INFINITY).to_boolean(&heap));
        assert!(number(f64::NEG_INFINITY).to_boolean(&heap));
    }

    #[test]
    fn to_number_gives_null_a_zero_and_undefined_a_nan() {
        let heap = Heap::new();
        // The pair behind `null + 1 === 1` and `undefined + 1` being NaN, which is the whole of
        // why the two are not interchangeable in arithmetic.
        assert_eq!(NULL.to_number(&heap).expect("a primitive converts"), 0.0);
        assert!(
            UNDEFINED
                .to_number(&heap)
                .expect("a primitive converts")
                .is_nan()
        );
        assert_eq!(
            boolean(true)
                .to_number(&heap)
                .expect("a primitive converts"),
            1.0
        );
        assert_eq!(
            boolean(false)
                .to_number(&heap)
                .expect("a primitive converts"),
            0.0
        );
        // A Number is returned unchanged, including the one that is not equal to itself.
        assert_eq!(
            number(1.5).to_number(&heap).expect("a primitive converts"),
            1.5
        );
        assert!(
            number(f64::NAN)
                .to_number(&heap)
                .expect("a primitive converts")
                .is_nan()
        );
        // …and including the sign of zero, which `to_integer_or_infinity` then discards.
        assert!(
            number(-0.0)
                .to_number(&heap)
                .expect("a primitive converts")
                .is_sign_negative()
        );
    }

    #[test]
    fn to_integer_or_infinity_truncates_towards_zero_and_keeps_the_infinities() {
        let heap = Heap::new();
        assert_eq!(
            number(3.9)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            3.0
        );
        assert_eq!(
            number(-3.9)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            -3.0
        );
        assert_eq!(
            number(3.0)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            3.0
        );
        // §7.1.5 collapses NaN and both zeroes to `+0`, so a fraction that truncates to zero
        // comes back *positive* zero however it was signed.
        assert!(
            !number(-0.5)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts")
                .is_sign_negative()
        );
        assert!(
            !number(-0.0)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts")
                .is_sign_negative()
        );
        assert_eq!(
            number(f64::NAN)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            0.0
        );
        assert!(
            !number(f64::NAN)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts")
                .is_nan()
        );
        // The infinities are returned as themselves — the operation is named for it.
        assert_eq!(
            number(f64::INFINITY)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            f64::INFINITY
        );
        assert_eq!(
            number(f64::NEG_INFINITY)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            f64::NEG_INFINITY
        );
        // The other types go through `ToNumber` first.
        assert_eq!(
            boolean(true)
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            1.0
        );
        assert_eq!(
            UNDEFINED
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts"),
            0.0
        );
    }

    #[test]
    fn to_uint32_wraps_by_the_mathematical_modulo_at_every_magnitude() {
        let heap = Heap::new();
        assert_eq!(
            number(0.0).to_uint32(&heap).expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(1.0).to_uint32(&heap).expect("a primitive converts"),
            1
        );
        assert_eq!(
            number(4_294_967_295.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            4_294_967_295
        );
        // One past the modulus wraps to zero, which is the whole of the operation.
        assert_eq!(
            number(4_294_967_296.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(4_294_967_297.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            1
        );
        // A negative comes back as its positive residue: the specification's `modulo` takes the
        // sign of the divisor where Rust's `%` takes the sign of the dividend.
        assert_eq!(
            number(-1.0).to_uint32(&heap).expect("a primitive converts"),
            4_294_967_295
        );
        assert_eq!(
            number(-4_294_967_296.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        // The fraction goes before the modulo, not after.
        assert_eq!(
            number(-1.5).to_uint32(&heap).expect("a primitive converts"),
            4_294_967_295
        );
        assert_eq!(
            number(1.9).to_uint32(&heap).expect("a primitive converts"),
            1
        );
        // §7.1.7 step 2 sends every non-finite value to zero rather than to a saturated bound,
        // which is what a cast through an integer type would have produced.
        assert_eq!(
            number(f64::NAN)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(f64::INFINITY)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(f64::NEG_INFINITY)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        // Far past anything an integer type could hold, where the exactness argument is the
        // only thing keeping the answer right. 1e300 is a multiple of 2^32 and so is zero;
        // `1e300 as u32` in Rust is `u32::MAX`.
        assert_eq!(
            number(1e300)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(f64::MAX)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        // 2^53 is the last integer with a neighbour, and 2^53 + 2 the next one representable.
        assert_eq!(
            number(9_007_199_254_740_992.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(9_007_199_254_740_994.0)
                .to_uint32(&heap)
                .expect("a primitive converts"),
            2
        );
    }

    #[test]
    fn to_int32_is_to_uint32_read_as_signed() {
        let heap = Heap::new();
        assert_eq!(
            number(1.0).to_int32(&heap).expect("a primitive converts"),
            1
        );
        assert_eq!(
            number(-1.0).to_int32(&heap).expect("a primitive converts"),
            -1
        );
        // The boundary the two operations differ at, and the reason `2147483648 | 0` is negative.
        assert_eq!(
            number(2_147_483_647.0)
                .to_int32(&heap)
                .expect("a primitive converts"),
            2_147_483_647
        );
        assert_eq!(
            number(2_147_483_648.0)
                .to_int32(&heap)
                .expect("a primitive converts"),
            -2_147_483_648
        );
        assert_eq!(
            number(4_294_967_295.0)
                .to_int32(&heap)
                .expect("a primitive converts"),
            -1
        );
        assert_eq!(
            number(4_294_967_296.0)
                .to_int32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(f64::NAN)
                .to_int32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(f64::INFINITY)
                .to_int32(&heap)
                .expect("a primitive converts"),
            0
        );
        assert_eq!(
            number(1e300).to_int32(&heap).expect("a primitive converts"),
            0
        );
    }

    #[test]
    fn the_three_equality_relations_disagree_on_nan_and_on_the_signed_zeroes() {
        let heap = Heap::new();
        let nan = number(f64::NAN);
        let plus_zero = number(0.0);
        let minus_zero = number(-0.0);

        // `===` is IEEE comparison: a NaN equals nothing, and the zeroes equal each other.
        assert!(!nan.is_strictly_equal(&nan, &heap));
        assert!(plus_zero.is_strictly_equal(&minus_zero, &heap));
        // `Object.is` is the other way round on both.
        assert!(nan.same_value(&nan, &heap));
        assert!(!plus_zero.same_value(&minus_zero, &heap));
        // …and `SameValueZero` takes one from each.
        assert!(nan.same_value_zero(&nan, &heap));
        assert!(plus_zero.same_value_zero(&minus_zero, &heap));

        // Two NaNs need not share a bit pattern — IEEE 754 has millions and §6.1.6.1 has one —
        // so a relation that compared bits would call these two different values. This one is
        // negative and quiet where `f64::NAN` is positive and quiet; all three relations are
        // asked, and none of them notices.
        let other_nan = number(f64::from_bits(0xfff8_0000_0000_0000));
        assert!(other_nan.same_value(&nan, &heap));
        assert!(other_nan.same_value_zero(&nan, &heap));
        assert!(!other_nan.is_strictly_equal(&nan, &heap));
    }

    #[test]
    fn the_three_relations_over_every_kind_of_number_pair() {
        let heap = Heap::new();
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
                left.is_strictly_equal(&right, &heap),
                strict,
                "=== of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value(&right, &heap),
                same,
                "SameValue of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value_zero(&right, &heap),
                same_zero,
                "SameValueZero of {left:?} and {right:?}"
            );
        }
    }

    #[test]
    fn every_relation_agrees_about_the_types_that_are_not_numbers() {
        let heap = Heap::new();
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
                left.is_strictly_equal(&right, &heap),
                expected,
                "=== of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value(&right, &heap),
                expected,
                "SameValue of {left:?} and {right:?}"
            );
            assert_eq!(
                left.same_value_zero(&right, &heap),
                expected,
                "SameValueZero of {left:?} and {right:?}"
            );
        }
    }

    #[test]
    fn to_string_writes_each_type_the_way_the_table_says_and_not_the_way_typeof_does() {
        let mut heap = Heap::new();
        let table = [
            (UNDEFINED, "undefined"),
            // §7.1.17's row, not §13.5.3's: `String(null)` is `"null"` where `typeof null` is
            // `"object"`. The two tables disagree here and nowhere else.
            (NULL, "null"),
            (boolean(true), "true"),
            (boolean(false), "false"),
            (number(1.5), "1.5"),
            (number(-0.0), "0"),
            (number(f64::NAN), "NaN"),
            (number(f64::NEG_INFINITY), "-Infinity"),
            (number(1e21), "1e+21"),
        ];
        for (value, expected) in table {
            let id = value.to_string(&mut heap).expect("a primitive converts");
            let units: Vec<u16> = expected.encode_utf16().collect();
            assert_eq!(heap.string(id), Some(units.as_slice()), "String({value:?})");
        }
    }

    #[test]
    fn to_string_of_a_string_hands_back_the_same_string_rather_than_a_copy() {
        // §7.1.17 returns the argument itself for a String, and this is where that is visible:
        // no second String appears on the heap, and the handle that comes back is the one that
        // went in. A copy would be correct in every observable way and would still be wrong —
        // `String(s)` in a loop would grow the heap without bound.
        let mut heap = Heap::new();
        let original = heap.new_string("abc".encode_utf16().collect());
        let before = heap.string_count();
        let returned = Value::String(original)
            .to_string(&mut heap)
            .expect("a primitive converts");
        assert_eq!(returned, original);
        assert_eq!(heap.string_count(), before);
        // …while every other type does allocate, since its text has to live somewhere.
        let _ = NULL.to_string(&mut heap).expect("a primitive converts");
        assert_eq!(heap.string_count(), before + 1);
    }

    #[test]
    fn a_string_is_typeof_string_and_is_true_unless_it_is_empty() {
        let mut heap = Heap::new();
        let empty = Value::String(heap.new_string(Vec::new()));
        let zero = Value::String(heap.new_string("0".encode_utf16().collect()));
        let space = Value::String(heap.new_string(" ".encode_utf16().collect()));
        assert_eq!(empty.type_of(&heap), "string");
        // §7.1.2 asks about the length and nothing else, which is why `"0"` and `"false"` are
        // true while `Number("0")` is false — the two operations are not the same question.
        assert!(!empty.to_boolean(&heap));
        assert!(zero.to_boolean(&heap));
        assert!(space.to_boolean(&heap));
        assert_eq!(zero.to_number(&heap).expect("a primitive converts"), 0.0);
    }

    #[test]
    fn the_integer_conversions_read_a_string_through_to_number() {
        let mut heap = Heap::new();
        let cases = [
            ("4294967297", 1_i32, 1_u32),
            ("-1", -1, 4_294_967_295),
            ("abc", 0, 0),
        ];
        for (text, as_int32, as_uint32) in cases {
            let value = Value::String(heap.new_string(text.encode_utf16().collect()));
            assert_eq!(
                value.to_int32(&heap).expect("a primitive converts"),
                as_int32,
                "ToInt32 of {text:?}"
            );
            assert_eq!(
                value.to_uint32(&heap).expect("a primitive converts"),
                as_uint32,
                "ToUint32 of {text:?}"
            );
        }
    }

    #[test]
    fn two_strings_are_the_same_value_when_their_units_match_not_their_handles() {
        let mut heap = Heap::new();
        let first = Value::String(heap.new_string("ab".encode_utf16().collect()));
        let again = Value::String(heap.new_string("ab".encode_utf16().collect()));
        let other = Value::String(heap.new_string("ac".encode_utf16().collect()));
        let prefix = Value::String(heap.new_string("a".encode_utf16().collect()));
        // Nothing is interned, so `first` and `again` are distinct handles — §7.2.12 compares
        // the sequences, and a relation that compared handles would answer `false` here.
        for (left, right, expected) in [
            (first, again, true),
            (first, first, true),
            (first, other, false),
            (first, prefix, false),
            (first, number(0.0), false),
            (first, NULL, false),
        ] {
            assert_eq!(left.is_strictly_equal(&right, &heap), expected);
            assert_eq!(left.same_value(&right, &heap), expected);
            assert_eq!(left.same_value_zero(&right, &heap), expected);
        }
    }

    #[test]
    fn a_handle_the_heap_does_not_know_answers_as_if_it_were_nothing() {
        // No script can produce one — DR-0010 has the argument — but the branches exist, and
        // what they do is a choice worth pinning: every operation is total and none of them
        // reads another heap's memory. `false` for the equalities is the important one: two
        // unknown handles are *not* equal, since it is their units being compared and there
        // are none.
        let mut mine = Heap::new();
        let mut theirs = Heap::new();
        let _ = theirs.new_string("a".encode_utf16().collect());
        let foreign = Value::String(theirs.new_string("b".encode_utf16().collect()));
        let known = Value::String(mine.new_string("b".encode_utf16().collect()));
        assert!(!foreign.to_boolean(&mine));
        assert!(foreign.to_number(&mine).is_ok_and(f64::is_nan));
        assert_eq!(foreign.type_of(&mine), "string");
        assert!(!foreign.same_value(&foreign, &mine));
        assert!(!foreign.same_value(&known, &mine));
        assert!(!known.same_value(&foreign, &mine));
    }

    #[test]
    fn no_string_can_make_a_conversion_panic() {
        // DR-0002 over the code units, which are `u16` and so need not be text at all: a lone
        // surrogate is not a `char`, and the whitespace test has to survive meeting one.
        let mut heap = Heap::new();
        let mut awkward: Vec<Vec<u16>> = vec![
            Vec::new(),
            vec![0xd800],               // a lone high surrogate
            vec![0xdfff],               // a lone low surrogate
            vec![0xd800, 0x20, 0xdc00], // a pair split by a space
            vec![0x2d, 0xd800],         // a sign then nothing readable
            vec![0x30, 0x78, 0xd800],   // `0x` then nothing readable
            vec![0xfeff, 0x31, 0xfeff], // whitespace the Rust table disagrees about
            vec![0x2e],                 // a lone `.`
            vec![0x2d],                 // a lone sign
            vec![0x65],                 // a lone exponent indicator
            vec![0x30; 4096],           // long enough to overflow a naive accumulator
            vec![0x39; 4096],           // …and the same length of nines
        ];
        awkward.push("1e999999999999999999999".encode_utf16().collect());
        awkward.push("0x".to_string().repeat(2048).encode_utf16().collect());
        for units in awkward {
            let value = Value::String(heap.new_string(units));
            let _ = value.to_boolean(&heap);
            let _ = value.to_number(&heap).expect("a primitive converts");
            let _ = value
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts");
            let _ = value.to_int32(&heap).expect("a primitive converts");
            let _ = value.to_uint32(&heap).expect("a primitive converts");
            let _ = value.type_of(&heap);
            let _ = value.same_value(&value, &heap);
            let _ = value.same_value_zero(&value, &heap);
            let _ = value.is_strictly_equal(&value, &heap);
        }
    }

    #[test]
    fn an_object_is_truthy_and_is_typeof_object_and_is_equal_only_to_itself() {
        let mut heap = Heap::new();
        let first = Value::Object(heap.new_object(None));
        let second = Value::Object(heap.new_object(None));
        assert_eq!(first.type_of(&heap), "object");
        // Every object is truthy — an empty one, one with a null prototype, any of them. The
        // famous counter-example is a host object with an [[IsHTMLDDA]] slot, which is a browser
        // thing and not a language thing.
        assert!(first.to_boolean(&heap));
        // Identity, not contents: two objects with the same properties are two objects.
        assert!(first.is_strictly_equal(&first, &heap));
        assert!(!first.is_strictly_equal(&second, &heap));
        assert!(first.same_value(&first, &heap));
        assert!(!first.same_value(&second, &heap));
        assert!(!first.is_strictly_equal(&NULL, &heap));
        assert!(!first.is_strictly_equal(&UNDEFINED, &heap));
    }

    #[test]
    fn an_object_that_cannot_be_made_primitive_throws_rather_than_answering() {
        // §7.1.1.1 — `valueOf` and `toString` are looked for and neither is callable, because
        // nothing is callable yet. The end of that algorithm is a TypeError, so that is the
        // answer for every object today. It changes on its own when `Object.prototype` has
        // methods; what will not change is that the answer is a *throw* and not a guess.
        let mut heap = Heap::new();
        let object = Value::Object(heap.new_object(None));
        assert_eq!(
            object.to_number(&heap),
            Err(TypeError("cannot convert an object to a primitive value"))
        );
        assert!(object.to_string(&mut heap).is_err());
        assert!(object.to_int32(&heap).is_err());
        assert!(object.to_uint32(&heap).is_err());
        assert!(object.to_integer_or_infinity(&heap).is_err());
        // …while the operations that do not convert still answer, which is the line §7.1.2 draws.
        assert!(object.to_boolean(&heap));
        assert_eq!(object.type_of(&heap), "object");
        // `ToPrimitive` of anything that is already primitive is itself, under either hint.
        assert!(matches!(
            Value::Number(1.0).to_primitive(&heap, Hint::Number),
            Ok(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            NULL.to_primitive(&heap, Hint::String),
            Ok(Value::Null)
        ));
    }

    #[test]
    fn an_operator_with_an_object_operand_throws_from_wherever_the_conversion_was() {
        let mut heap = Heap::new();
        let object = Value::Object(heap.new_object(None));
        let one = Value::Number(1.0);
        // Arithmetic converts, so it throws…
        for operator in [
            crate::ast::BinaryOperator::Add,
            crate::ast::BinaryOperator::Subtract,
            crate::ast::BinaryOperator::Multiply,
            crate::ast::BinaryOperator::LessThan,
            crate::ast::BinaryOperator::BitwiseAnd,
            crate::ast::BinaryOperator::ShiftLeft,
        ] {
            assert!(
                apply_binary(operator, object, one, &mut heap).is_err(),
                "{} should throw",
                operator.as_str()
            );
            assert!(apply_binary(operator, one, object, &mut heap).is_err());
        }
        // …and `===` does not, because it compares rather than converting. Neither does `==`
        // against `null`, which §7.2.13 answers before it reaches any conversion.
        assert!(matches!(
            apply_binary(
                crate::ast::BinaryOperator::StrictEqual,
                object,
                one,
                &mut heap
            ),
            Ok(Value::Boolean(false))
        ));
        assert!(matches!(
            apply_binary(
                crate::ast::BinaryOperator::Equal,
                object,
                Value::Null,
                &mut heap
            ),
            Ok(Value::Boolean(false))
        ));
        assert!(matches!(
            apply_binary(crate::ast::BinaryOperator::Equal, object, object, &mut heap),
            Ok(Value::Boolean(true))
        ));
    }

    #[test]
    fn no_number_can_make_a_conversion_panic() {
        let heap = Heap::new();
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
            let _ = value.to_boolean(&heap);
            let _ = value.to_number(&heap).expect("a primitive converts");
            let _ = value
                .to_integer_or_infinity(&heap)
                .expect("a primitive converts");
            let _ = value.to_int32(&heap).expect("a primitive converts");
            let _ = value.to_uint32(&heap).expect("a primitive converts");
            let _ = value.type_of(&heap);
            let _ = value.same_value(&value, &heap);
            let _ = value.same_value_zero(&value, &heap);
            let _ = value.is_strictly_equal(&value, &heap);
        }
    }
}
