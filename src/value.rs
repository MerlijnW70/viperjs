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
//! [`Value::type_of`] is the exception and stays free of it, because the variant already
//! answers — no operand is read. That asymmetry is worth keeping rather than smoothing over: it
//! says exactly which questions are about the *shape* of a value and which are about its
//! contents.
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

use crate::heap::{Heap, StringId};

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
            Self::String(_) => "string",
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
    pub fn to_number(&self, heap: &Heap) -> f64 {
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(true) => 1.0,
            Self::Boolean(false) => 0.0,
            Self::Number(number) => *number,
            // §7.1.4.1, which is a grammar and not a parse of convenience — see
            // [`string_to_number`] for the four ways it differs from the one the lexer reads.
            // A handle this heap does not know is NaN, which is what the operation says about
            // text that is not a `StrNumericLiteral`.
            Self::String(id) => heap.string(*id).map_or(f64::NAN, string_to_number),
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
    pub fn to_integer_or_infinity(&self, heap: &Heap) -> f64 {
        let number = self.to_number(heap);
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
    pub fn to_int32(&self, heap: &Heap) -> i32 {
        // §7.1.6 step 5: an `int32bit` at or above 2^31 comes back 2^32 lower. `as i32` on a
        // `u32` is that reinterpretation exactly, and is the one Rust cast that is defined to
        // wrap rather than saturate.
        self.to_uint32(heap) as i32
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
    pub fn to_uint32(&self, heap: &Heap) -> u32 {
        const MODULUS: f64 = 4_294_967_296.0; // 2^32

        // §7.1.7 step 2 sends every non-finite value and both zeroes to `+0`, and is not written
        // out: the arithmetic below already answers `0` for all five, so a step for them would be
        // a branch no input could tell from its absence. It rests on two facts rather than on
        // luck — `±∞ % y` and `NaN % y` are both NaN, and a float-to-integer `as` in Rust
        // saturates, which sends NaN to `0`. The behaviour is pinned by tests even though the
        // branch that would have stated it is gone.
        let remainder = self.to_number(heap).trunc() % MODULUS;
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
            _ => false,
        }
    }
}

/// `StringToNumber` (§7.1.4.1) — `ToNumber` applied to a String.
///
/// # Its grammar is not the source grammar, and the differences all surprise
///
/// §7.1.4.1 defines `StringNumericLiteral`, which resembles §12.9.3's `NumericLiteral` and is
/// not it. Four differences decide almost every awkward case, and each goes the opposite way to
/// the guess:
///
/// | | source | a String |
/// | --- | --- | --- |
/// | `""` | not a literal | **`+0`** |
/// | `"0123"` | legacy octal, 83 | **decimal, 123** |
/// | `"1_0"` | 10 | **NaN** — every production here is `[~Sep]` |
/// | `"Infinity"` | an identifier | **a literal**, and case-sensitively so |
/// | `"-0x10"` | `-(0x10)`, an operator | **NaN** |
///
/// The last needs a word. `StrNumericLiteral` is `StrDecimalLiteral` or
/// `NonDecimalIntegerLiteral`, and only the *decimal* alternative has the signed productions —
/// so a sign in front of `0x10` has no derivation. There is no unary minus here: this is a
/// grammar over a string, not an expression.
///
/// # Why the whitespace is the lexer's
///
/// `StrWhiteSpaceChar ::: WhiteSpace | LineTerminator` names §12.2's and §12.3's productions,
/// which the lexer already implements over the real Unicode sets. Sharing them is not
/// convenience: `"\u{feff}"` is `+0` and `"\u{85}"` is NaN, and a second copy of that table is
/// how the two answers drift apart.
fn string_to_number(units: &[u16]) -> f64 {
    let trimmed = trim_str_whitespace(units);
    // `StringNumericLiteral ::: StrWhiteSpace_opt` — a String of nothing but whitespace *is* a
    // literal, and its MV is 0. This is the row that catches everyone: `+[]` is `0`.
    if trimmed.is_empty() {
        return 0.0;
    }
    let text = decoded_text(trimmed);
    // `StrNumericLiteral ::: NonDecimalIntegerLiteral` is tried first because it is the narrower
    // alternative: only text starting `0x`, `0b` or `0o` can be one, and such text can be no
    // `StrDecimalLiteral` either.
    if let Some(value) = non_decimal_integer_value(&text) {
        return value;
    }
    str_decimal_literal_value(&text)
}

/// The String with `StrWhiteSpace` removed from both ends.
///
/// A lone surrogate is not a character and so is not whitespace, which `char::from_u32` says by
/// answering `None` — the one place this has to be careful, since a `u16` is not a `char`.
///
/// Counted from each end rather than searched for a first and a last, so that a String of
/// nothing but whitespace needs no case of its own: the two counts meet, and the slice between
/// them is empty. Written the other way it had an arm saying "a first exists but a last does
/// not", which is a state nothing can produce.
fn trim_str_whitespace(units: &[u16]) -> &[u16] {
    let is_str_whitespace = |unit: &&u16| {
        char::from_u32(u32::from(**unit)).is_some_and(|ch| {
            crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch)
        })
    };
    let start = units.iter().take_while(is_str_whitespace).count();
    let trailing = units[start..]
        .iter()
        .rev()
        .take_while(is_str_whitespace)
        .count();
    &units[start..units.len() - trailing]
}

/// The code units as text, so the productions below can be read as a `&str`.
///
/// Total, and deliberately not fallible. A `u16` is not always a character — a lone surrogate is
/// no character at all — but nothing here needs to know: every production in §7.1.4.1 is ASCII,
/// so a unit that is not ASCII makes the whole String a NaN whatever it is decoded to. U+FFFD
/// stands in for the units that are not characters because it appears in no production either,
/// which is the only property being asked of it.
///
/// The alternative — answering `None` for anything outside ASCII — was written first and had a
/// branch no input could distinguish: both arms end at the same NaN.
fn decoded_text(units: &[u16]) -> String {
    units
        .iter()
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// `NonDecimalIntegerLiteral[~Sep]` (§12.9.3), or `None` if the text is not one.
///
/// Answers `None` rather than NaN for text that is not one at all, so the caller can go on to
/// try the decimal alternative; a `0x` with no digits after it is not "not one", it is a
/// malformed one, and that is NaN.
fn non_decimal_integer_value(text: &str) -> Option<f64> {
    let bits = match text.as_bytes() {
        [b'0', b'b' | b'B', ..] => 1,
        [b'0', b'o' | b'O', ..] => 3,
        [b'0', b'x' | b'X', ..] => 4,
        _ => return None,
    };
    let digits = &text[2..];
    // `[~Sep]` — this production has no `NumericLiteralSeparator`, and the evaluator below skips
    // one because in *source* the same production may have them. That is the whole of what has
    // to be said here: every other way the digits can be wrong, including there being none at
    // all, that function already answers `None` to, and `None` is this operation's NaN.
    if digits.contains('_') {
        return Some(f64::NAN);
    }
    // The same evaluator §12.9.3.3 uses, because this is the same production. It is exact for a
    // power-of-two radix: the digits *are* the bits.
    Some(crate::lexer::power_of_two_value(digits, bits).unwrap_or(f64::NAN))
}

/// `StrDecimalLiteral` (§7.1.4.1), or NaN if the text is not one.
///
/// # Why this is a condition and not a parser
///
/// `f64::from_str`'s documented grammar and `StrUnsignedDecimalLiteral` are the **same language**
/// once two things are set aside, and the condition below is exactly those two:
///
/// | | `f64::from_str` | §7.1.4.1 |
/// | --- | --- | --- |
/// | `"1"`, `"1."`, `".5"`, `"1.5e-3"`, `"1E5"` | accepted | accepted, and identically |
/// | `"inf"`, `"infinity"`, `"nan"` — any case | accepted | **NaN**, no such production |
/// | a sign | accepted | taken already, so a *second* one is not |
///
/// The grammar was written out here first, and every branch of it turned out to be one no input
/// could distinguish: `f64::from_str` rejects `"1abc"`, `"1.2.3"`, `"1e"` and `"."` for itself,
/// so each hand-written rule was a second opinion that could never differ from the first. A
/// branch nothing can pin is a branch that should not exist (DR-0002), so what remains is the
/// difference alone. `the_two_grammars_accept_the_same_language` is the test that keeps the
/// claim honest, over every string of the shape this can meet.
fn str_decimal_literal_value(text: &str) -> f64 {
    let (sign, magnitude) = match text.as_bytes().first() {
        Some(b'+') => (1.0, &text[1..]),
        Some(b'-') => (-1.0, &text[1..]),
        _ => (1.0, text),
    };
    // `StrUnsignedDecimalLiteral ::: Infinity`, spelled exactly so. `f64::from_str` would take
    // `inf`, `infinity` and `nan` in any case, none of which this grammar has.
    if magnitude == "Infinity" {
        return sign * f64::INFINITY;
    }
    // The one line that separates the two languages — see the doc comment above for why it is
    // one line. `inf`, `infinity` and `nan` all begin with a letter, a second sign begins with a
    // sign, and no `StrUnsignedDecimalLiteral` begins with anything but a digit or a `.`.
    let starts_a_literal = matches!(
        magnitude.as_bytes().first(),
        Some(byte) if byte.is_ascii_digit() || *byte == b'.'
    );
    if !starts_a_literal {
        return f64::NAN;
    }
    // Correctly rounded, and by the same argument §12.9.3.3 makes for a `DecimalLiteral`:
    // `f64::from_str` is Eisel-Lemire with an exact fallback. The sign is applied afterwards
    // rather than parsed, which is what gives `"-0"` a negative zero and `"-1e-400"` one too.
    //
    // The `Err` is `"1abc"`, `"1.2.3"`, `"1e"`, `"."` — everything shaped like a literal and not
    // being one — and NaN is what §7.1.4.1 says about all of it.
    magnitude
        .parse::<f64>()
        .map_or(f64::NAN, |value| sign * value)
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
        assert_eq!(NULL.to_number(&heap), 0.0);
        assert!(UNDEFINED.to_number(&heap).is_nan());
        assert_eq!(boolean(true).to_number(&heap), 1.0);
        assert_eq!(boolean(false).to_number(&heap), 0.0);
        // A Number is returned unchanged, including the one that is not equal to itself.
        assert_eq!(number(1.5).to_number(&heap), 1.5);
        assert!(number(f64::NAN).to_number(&heap).is_nan());
        // …and including the sign of zero, which `to_integer_or_infinity` then discards.
        assert!(number(-0.0).to_number(&heap).is_sign_negative());
    }

    #[test]
    fn to_integer_or_infinity_truncates_towards_zero_and_keeps_the_infinities() {
        let heap = Heap::new();
        assert_eq!(number(3.9).to_integer_or_infinity(&heap), 3.0);
        assert_eq!(number(-3.9).to_integer_or_infinity(&heap), -3.0);
        assert_eq!(number(3.0).to_integer_or_infinity(&heap), 3.0);
        // §7.1.5 collapses NaN and both zeroes to `+0`, so a fraction that truncates to zero
        // comes back *positive* zero however it was signed.
        assert!(
            !number(-0.5)
                .to_integer_or_infinity(&heap)
                .is_sign_negative()
        );
        assert!(
            !number(-0.0)
                .to_integer_or_infinity(&heap)
                .is_sign_negative()
        );
        assert_eq!(number(f64::NAN).to_integer_or_infinity(&heap), 0.0);
        assert!(!number(f64::NAN).to_integer_or_infinity(&heap).is_nan());
        // The infinities are returned as themselves — the operation is named for it.
        assert_eq!(
            number(f64::INFINITY).to_integer_or_infinity(&heap),
            f64::INFINITY
        );
        assert_eq!(
            number(f64::NEG_INFINITY).to_integer_or_infinity(&heap),
            f64::NEG_INFINITY
        );
        // The other types go through `ToNumber` first.
        assert_eq!(boolean(true).to_integer_or_infinity(&heap), 1.0);
        assert_eq!(UNDEFINED.to_integer_or_infinity(&heap), 0.0);
    }

    #[test]
    fn to_uint32_wraps_by_the_mathematical_modulo_at_every_magnitude() {
        let heap = Heap::new();
        assert_eq!(number(0.0).to_uint32(&heap), 0);
        assert_eq!(number(1.0).to_uint32(&heap), 1);
        assert_eq!(number(4_294_967_295.0).to_uint32(&heap), 4_294_967_295);
        // One past the modulus wraps to zero, which is the whole of the operation.
        assert_eq!(number(4_294_967_296.0).to_uint32(&heap), 0);
        assert_eq!(number(4_294_967_297.0).to_uint32(&heap), 1);
        // A negative comes back as its positive residue: the specification's `modulo` takes the
        // sign of the divisor where Rust's `%` takes the sign of the dividend.
        assert_eq!(number(-1.0).to_uint32(&heap), 4_294_967_295);
        assert_eq!(number(-4_294_967_296.0).to_uint32(&heap), 0);
        // The fraction goes before the modulo, not after.
        assert_eq!(number(-1.5).to_uint32(&heap), 4_294_967_295);
        assert_eq!(number(1.9).to_uint32(&heap), 1);
        // §7.1.7 step 2 sends every non-finite value to zero rather than to a saturated bound,
        // which is what a cast through an integer type would have produced.
        assert_eq!(number(f64::NAN).to_uint32(&heap), 0);
        assert_eq!(number(f64::INFINITY).to_uint32(&heap), 0);
        assert_eq!(number(f64::NEG_INFINITY).to_uint32(&heap), 0);
        // Far past anything an integer type could hold, where the exactness argument is the
        // only thing keeping the answer right. 1e300 is a multiple of 2^32 and so is zero;
        // `1e300 as u32` in Rust is `u32::MAX`.
        assert_eq!(number(1e300).to_uint32(&heap), 0);
        assert_eq!(number(f64::MAX).to_uint32(&heap), 0);
        // 2^53 is the last integer with a neighbour, and 2^53 + 2 the next one representable.
        assert_eq!(number(9_007_199_254_740_992.0).to_uint32(&heap), 0);
        assert_eq!(number(9_007_199_254_740_994.0).to_uint32(&heap), 2);
    }

    #[test]
    fn to_int32_is_to_uint32_read_as_signed() {
        let heap = Heap::new();
        assert_eq!(number(1.0).to_int32(&heap), 1);
        assert_eq!(number(-1.0).to_int32(&heap), -1);
        // The boundary the two operations differ at, and the reason `2147483648 | 0` is negative.
        assert_eq!(number(2_147_483_647.0).to_int32(&heap), 2_147_483_647);
        assert_eq!(number(2_147_483_648.0).to_int32(&heap), -2_147_483_648);
        assert_eq!(number(4_294_967_295.0).to_int32(&heap), -1);
        assert_eq!(number(4_294_967_296.0).to_int32(&heap), 0);
        assert_eq!(number(f64::NAN).to_int32(&heap), 0);
        assert_eq!(number(f64::INFINITY).to_int32(&heap), 0);
        assert_eq!(number(1e300).to_int32(&heap), 0);
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
    fn string_to_number_over_the_table_v8_answers() {
        // Every row measured against V8 rather than reasoned about, because §7.1.4.1's grammar
        // resembles the one the lexer reads closely enough to be guessed at wrongly. `None`
        // stands for NaN, which no equality would compare.
        let mut heap = Heap::new();
        let table: &[(&str, Option<f64>)] = &[
            ("", Some(0_f64)),
            (" ", Some(0_f64)),
            ("\t\n\r ", Some(0_f64)),
            ("\u{a0}", Some(0_f64)),
            ("\u{2028}", Some(0_f64)),
            ("\u{2029}", Some(0_f64)),
            ("\u{feff}", Some(0_f64)),
            ("1", Some(1_f64)),
            ("1.5", Some(1.5_f64)),
            (".5", Some(0.5_f64)),
            ("5.", Some(5_f64)),
            ("-1", Some(-1_f64)),
            ("+1", Some(1_f64)),
            ("1e3", Some(1000_f64)),
            ("1E3", Some(1000_f64)),
            ("1e+3", Some(1000_f64)),
            ("1e-3", Some(0.001_f64)),
            ("-1.5e-3", Some(-0.0015_f64)),
            ("0", Some(0_f64)),
            ("-0", Some(0_f64)),
            ("+0", Some(0_f64)),
            ("  42  ", Some(42_f64)),
            ("Infinity", Some(f64::INFINITY)),
            ("-Infinity", Some(f64::NEG_INFINITY)),
            ("+Infinity", Some(f64::INFINITY)),
            ("infinity", None),
            ("INFINITY", None),
            (" Infinity ", Some(f64::INFINITY)),
            ("0x10", Some(16_f64)),
            ("0X10", Some(16_f64)),
            ("0b11", Some(3_f64)),
            ("0B11", Some(3_f64)),
            ("0o17", Some(15_f64)),
            ("0O17", Some(15_f64)),
            ("-0x10", None),
            ("+0x10", None),
            (" 0x10 ", Some(16_f64)),
            ("0x", None),
            ("0b", None),
            ("0o", None),
            ("0xg", None),
            ("0123", Some(123_f64)),
            ("0888", Some(888_f64)),
            ("00", Some(0_f64)),
            ("09", Some(9_f64)),
            ("1_0", None),
            ("0x1_0", None),
            ("1_000.5", None),
            ("1e1_0", None),
            ("1 2", None),
            ("1,2", None),
            ("abc", None),
            ("1abc", None),
            ("--1", None),
            ("1-", None),
            (".", None),
            ("-.", None),
            ("e3", None),
            ("1e", None),
            ("1e+", None),
            ("+-1", None),
            ("1.2.3", None),
            ("1n", None),
            ("0x1n", None),
            ("1e309", Some(f64::INFINITY)),
            ("-1e309", Some(f64::NEG_INFINITY)),
            ("1e-400", Some(0_f64)),
            ("9007199254740993", Some(9007199254740992_f64)),
        ];
        for (text, expected) in table {
            let id = heap.new_string(text.encode_utf16().collect());
            let actual = Value::String(id).to_number(&heap);
            match expected {
                Some(expected) => assert_eq!(actual, *expected, "ToNumber of {text:?}"),
                None => assert!(actual.is_nan(), "ToNumber of {text:?} should be NaN"),
            }
        }
    }

    /// `StrUnsignedDecimalLiteral` other than `Infinity`, §7.1.4.1's grammar written out.
    ///
    /// This is the reference the shipped condition is checked against — obviously the grammar,
    /// and slow enough that nobody would run it per conversion. Keeping it here rather than in
    /// `src/` is the point: the claim "`f64::from_str` accepts exactly this" is a claim about
    /// two implementations agreeing, and a test is where such a claim belongs.
    fn reference_str_unsigned_decimal_literal(text: &str) -> bool {
        fn digits(bytes: &[u8], at: &mut usize) -> usize {
            let start = *at;
            while matches!(bytes.get(*at), Some(byte) if byte.is_ascii_digit()) {
                *at += 1;
            }
            *at - start
        }
        let bytes = text.as_bytes();
        let mut at = 0;
        // `DecimalDigits . DecimalDigits_opt` | `. DecimalDigits` | `DecimalDigits` — a `.` may
        // have digits on either side or both, and must have them on at least one.
        let mut has_digits = digits(bytes, &mut at) > 0;
        if bytes.get(at) == Some(&b'.') {
            at += 1;
            has_digits |= digits(bytes, &mut at) > 0;
        }
        if !has_digits {
            return false;
        }
        // `ExponentPart ::: ExponentIndicator SignedInteger`, and a `SignedInteger` has digits.
        if matches!(bytes.get(at), Some(b'e' | b'E')) {
            at += 1;
            if matches!(bytes.get(at), Some(b'+' | b'-')) {
                at += 1;
            }
            if digits(bytes, &mut at) == 0 {
                return false;
            }
        }
        at == bytes.len()
    }

    #[test]
    fn the_two_grammars_accept_the_same_language() {
        // The load-bearing claim of `str_decimal_literal_value`: past the sign and `Infinity`,
        // "starts with a digit or a `.`, and `f64::from_str` takes it" accepts exactly
        // `StrUnsignedDecimalLiteral`. Checked exhaustively rather than argued, over an alphabet
        // holding every character that could possibly matter — the digits, the two exponent
        // indicators, both signs, the point, the letters of `infinity` and `nan`, a separator,
        // an `x`, and a space.
        //
        // Five characters is enough to reach `1e+1`, `.5e5`, `1.2.3`, `infin`, `+-1.5` and
        // `1_000`; the shapes that go wrong are all short. A sixth would cost sixteen times as
        // much for nothing new.
        let alphabet = b"01.eE+-nifaty_x ";
        let mut checked = 0_u32;
        let mut text = String::new();
        for length in 0..=5_u32 {
            for encoded in 0..16_u32.pow(length) {
                text.clear();
                let mut rest = encoded;
                for _ in 0..length {
                    text.push(char::from(alphabet[(rest % 16) as usize]));
                    rest /= 16;
                }
                let shipped = matches!(
                    text.as_bytes().first(),
                    Some(byte) if byte.is_ascii_digit() || *byte == b'.'
                ) && text.parse::<f64>().is_ok();
                assert_eq!(
                    shipped,
                    reference_str_unsigned_decimal_literal(&text),
                    "the two grammars disagree about {text:?}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 1_118_481);
    }

    #[test]
    fn string_to_number_keeps_the_sign_of_a_zero_the_table_cannot_see() {
        // The table above compares with `==`, under which `-0.0 == 0.0` — so every row that
        // answers a negative zero is silently unpinned there. These are the same measurement
        // taken with `Object.is`, and the sign matters: `1 / Number("-0")` is `-Infinity`.
        let mut heap = Heap::new();
        let negative = ["-0", "-0.0", "-0e5", " -0 ", "-.0", "-1e-400"];
        for text in negative {
            let id = heap.new_string(text.encode_utf16().collect());
            let value = Value::String(id).to_number(&heap);
            assert!(
                value == 0.0 && value.is_sign_negative(),
                "ToNumber of {text:?} should be -0, was {value}"
            );
        }
        // `-1e-400` underflows to a *negative* zero because the sign is applied after the
        // parse, not read as part of it. The unsigned spellings stay positive, and `-0x0` is
        // NaN rather than either zero — a sign has no derivation before a non-decimal literal.
        for text in ["0", "+0", "0.0", "0x0"] {
            let id = heap.new_string(text.encode_utf16().collect());
            let value = Value::String(id).to_number(&heap);
            assert!(
                value == 0.0 && value.is_sign_positive(),
                "ToNumber of {text:?} should be +0, was {value}"
            );
        }
        let id = heap.new_string("-0x0".encode_utf16().collect());
        assert!(Value::String(id).to_number(&heap).is_nan());
    }

    #[test]
    fn a_string_is_typeof_string_and_is_true_unless_it_is_empty() {
        let mut heap = Heap::new();
        let empty = Value::String(heap.new_string(Vec::new()));
        let zero = Value::String(heap.new_string("0".encode_utf16().collect()));
        let space = Value::String(heap.new_string(" ".encode_utf16().collect()));
        assert_eq!(empty.type_of(), "string");
        // §7.1.2 asks about the length and nothing else, which is why `"0"` and `"false"` are
        // true while `Number("0")` is false — the two operations are not the same question.
        assert!(!empty.to_boolean(&heap));
        assert!(zero.to_boolean(&heap));
        assert!(space.to_boolean(&heap));
        assert_eq!(zero.to_number(&heap), 0.0);
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
            assert_eq!(value.to_int32(&heap), as_int32, "ToInt32 of {text:?}");
            assert_eq!(value.to_uint32(&heap), as_uint32, "ToUint32 of {text:?}");
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
        assert!(foreign.to_number(&mine).is_nan());
        assert_eq!(foreign.type_of(), "string");
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
            let _ = value.to_number(&heap);
            let _ = value.to_integer_or_infinity(&heap);
            let _ = value.to_int32(&heap);
            let _ = value.to_uint32(&heap);
            let _ = value.type_of();
            let _ = value.same_value(&value, &heap);
            let _ = value.same_value_zero(&value, &heap);
            let _ = value.is_strictly_equal(&value, &heap);
        }
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
            let _ = value.to_number(&heap);
            let _ = value.to_integer_or_infinity(&heap);
            let _ = value.to_int32(&heap);
            let _ = value.to_uint32(&heap);
            let _ = value.type_of();
            let _ = value.same_value(&value, &heap);
            let _ = value.same_value_zero(&value, &heap);
            let _ = value.is_strictly_equal(&value, &heap);
        }
    }
}
