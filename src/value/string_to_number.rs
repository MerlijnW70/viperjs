//! `StringToNumber` (§7.1.4.1) — the grammar a String is read as when a Number is wanted.
//!
//! Separate from the values themselves because it is a *parser*, and the only one here: three
//! productions, a whitespace rule that belongs to the lexer, and one condition standing in for a
//! grammar `f64::from_str` already implements. Everything else in [`super`] is a table lookup or
//! a comparison; this is the one place a value is taken apart character by character.

use crate::lexer::{is_line_terminator, is_whitespace, power_of_two_value};

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
pub(crate) fn string_to_number(units: &[u16]) -> f64 {
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
        char::from_u32(u32::from(**unit))
            .is_some_and(|ch| is_whitespace(ch) || is_line_terminator(ch))
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
    Some(power_of_two_value(digits, bits).unwrap_or(f64::NAN))
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

#[cfg(test)]
mod tests {
    use crate::heap::Heap;
    use crate::value::Value;

    // Driven through `Value::String` rather than by calling [`string_to_number`], because the
    // handle lookup is part of what is being asserted: a String's numeric value is what a
    // *script* gets, and that path goes through the heap.

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
            let actual = Value::String(id)
                .to_number(&heap)
                .expect("a primitive converts");
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
            let value = Value::String(id)
                .to_number(&heap)
                .expect("a primitive converts");
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
            let value = Value::String(id)
                .to_number(&heap)
                .expect("a primitive converts");
            assert!(
                value == 0.0 && value.is_sign_positive(),
                "ToNumber of {text:?} should be +0, was {value}"
            );
        }
        let id = heap.new_string("-0x0".encode_utf16().collect());
        assert!(
            Value::String(id)
                .to_number(&heap)
                .expect("a primitive converts")
                .is_nan()
        );
    }
}
