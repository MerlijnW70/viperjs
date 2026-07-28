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
//! - `relations` — the three equalities that never convert, and so never throw.
//! - here — [`Value`] itself, the conversions and the three equality relations.
//!
//! The two directions are checked against each other rather than only against a table: reading
//! back what was written must give the same Number, for any Number at all.

mod number_to_string;
mod operators;
mod relations;
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

#[cfg(test)]
mod tests;
