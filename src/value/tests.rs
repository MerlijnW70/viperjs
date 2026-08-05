//! What a value answers, said as sentences about behaviour.
//!
//! Measured rather than reasoned about wherever another engine could be asked — §7.1.4.1's
//! grammar and §6.1.6.1.20's digits have their own files and their own measurements. What is left
//! is the part where the specification's own table is the authority: which conversions throw, and
//! what `typeof` says.

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
fn a_value_that_is_already_text_or_refuses_to_be_has_nothing_to_spell() {
    // The three `spelled` answers `None` for, and the reason each is not a piece of fresh text: a
    // String already is one, a Symbol refuses, and an Object is not a primitive. Anything else has
    // an answer, and it is the same one §7.1.17 gives — so a caller may intern it directly.
    let mut heap = Heap::new();
    let text = heap.new_string("abc".encode_utf16().collect());
    let symbol = heap.new_symbol(None);
    let object = heap.new_object(None);
    assert_eq!(Value::String(text).spelled(&heap), None);
    assert_eq!(Value::Symbol(symbol).spelled(&heap), None);
    assert_eq!(Value::Object(object).spelled(&heap), None);
    assert_eq!(NULL.spelled(&heap).as_deref(), Some("null"));
    assert_eq!(UNDEFINED.spelled(&heap).as_deref(), Some("undefined"));
    assert_eq!(Value::Boolean(true).spelled(&heap).as_deref(), Some("true"));
    assert_eq!(
        Value::Boolean(false).spelled(&heap).as_deref(),
        Some("false")
    );
    assert_eq!(Value::Number(1.5).spelled(&heap).as_deref(), Some("1.5"));
}

#[test]
fn spelling_a_value_puts_no_string_on_the_heap() {
    // The whole point of the function. `ToString` allocates because its answer *is* a String; this
    // answers with the text instead, so a caller about to intern it does not leave a dead slot
    // behind for every property access. DR-0010 never gives such a slot back.
    let heap = Heap::new();
    let before = heap.string_count();
    for value in [UNDEFINED, NULL, Value::Boolean(true), Value::Number(42.0)] {
        assert!(value.spelled(&heap).is_some());
    }
    assert_eq!(heap.string_count(), before);
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
    // Matched rather than compared: an abrupt completion may carry a thrown `Value`, and `Value`
    // has no equality of its own — comparing two of them would be asking a question the language
    // answers with `===` and Rust would answer differently.
    assert!(matches!(
        object.to_number(&heap),
        Err(Abrupt::Raised(
            ErrorKind::Type,
            "cannot convert an object to a primitive value"
        ))
    ));
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

#[test]
fn adding_two_strings_that_would_be_too_long_throws_a_range_error() {
    // §6.1.4 puts the String type's maximum at 2^53-1 and says nothing about an implementation
    // with a smaller one; DR-0012 is ViperJS's, and this is `+` meeting it. Every engine answers a
    // `RangeError` here, and the alternative is not a longer String — it is allocating until the
    // process dies, which is a wrong answer for every program in it.
    //
    // Cheap for the reason DR-0012 gives: the operand is one zeroed allocation nothing writes to,
    // and the join that would have cost half a gigabyte is refused before it is made.
    let mut heap = Heap::new();
    let half = Value::String(heap.new_string(vec![0; crate::heap::MAX_STRING_LENGTH / 2 + 1]));
    assert!(matches!(
        apply_binary(crate::ast::BinaryOperator::Add, half, half, &mut heap),
        Err(Abrupt::Raised(
            ErrorKind::Range,
            "the string would be longer than a string may be"
        ))
    ));
    // …while a join that stays under the maximum is made, so the refusal is about the length and
    // not about `+` having stopped concatenating.
    let empty = Value::String(heap.new_string(Vec::new()));
    assert!(matches!(
        apply_binary(crate::ast::BinaryOperator::Add, half, empty, &mut heap),
        Ok(Value::String(_))
    ));
}
