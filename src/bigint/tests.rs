//! §6.1.6.2's arithmetic, at the places where a width-free integer stops behaving like a `u64`.
//!
//! The interesting inputs are all about *carrying*: a sum that grows a limb, a difference that
//! shrinks one, a product whose partial sums overflow, a division whose trial digit is wrong. Every
//! test below picks values either side of a limb boundary rather than round decimal numbers,
//! because a bug in base 2^32 arithmetic hides perfectly at 1, 2 and 3.

use super::{BigInt, Error, MAX_LIMBS};

/// The BigInt this decimal string names, for tests that would otherwise be unreadable.
fn big(text: &str) -> BigInt {
    match text.strip_prefix('-') {
        Some(digits) => BigInt::from_digits(digits, 10)
            .unwrap_or_else(|| panic!("{text} is digits")) // a test's own input
            .negate(),
        None => BigInt::from_digits(text, 10).unwrap_or_else(|| panic!("{text} is digits")), // same
    }
}

/// What `left op right` came to, as a decimal string.
/// The same, in a radix of its own — §21.2.3.3's other half.
fn shown_in(value: &BigInt, radix: u32) -> String {
    value
        .to_digits(radix)
        .expect("a test's own values are spellable") // a test's own input
}

fn shown(value: &BigInt) -> String {
    value
        .to_digits(10)
        .expect("a test's own values are spellable")
}

#[test]
fn a_value_reads_back_as_the_digits_it_was_written_with() {
    for text in [
        "0",
        "1",
        "9",
        "4294967295",
        "4294967296",
        "4294967297",
        "18446744073709551615",
        "18446744073709551616",
        "-1",
        "-4294967296",
        "123456789012345678901234567890",
    ] {
        assert_eq!(shown(&big(text)), text, "round trip of {text}");
    }
    // Zero has one spelling, and a leading run of zeros is not a different number.
    assert_eq!(shown(&big("000")), "0");
    assert_eq!(shown(&big("007")), "7");
    // …and it is never negative, however it was arrived at.
    assert!(!big("0").negate().is_negative());
    assert_eq!(shown(&big("5").subtract(&big("5")).expect("finite")), "0"); // arithmetic in range
}

#[test]
fn the_other_radixes_read_and_write_the_same_value() {
    assert_eq!(shown(&BigInt::from_digits("ff", 16).expect("hex")), "255"); // a test's own input
    assert_eq!(
        shown(&BigInt::from_digits("1010", 2).expect("binary")),
        "10"
    ); // same
    assert_eq!(shown(&BigInt::from_digits("777", 8).expect("octal")), "511"); // same
    assert_eq!(shown_in(&big("255"), 16), "ff");
    assert_eq!(shown_in(&big("10"), 2), "1010");
    assert_eq!(shown_in(&big("-255"), 16), "-ff");
    // A character that is not a digit *in that radix* is refused rather than skipped, so a caller
    // cannot hand this a string with a space in it and get a number back.
    assert!(BigInt::from_digits("1 2", 10).is_none());
    assert!(BigInt::from_digits("fg", 16).is_none());
    assert!(BigInt::from_digits("2", 2).is_none());
}

#[test]
fn addition_carries_across_a_limb_and_subtraction_borrows_back() {
    // 2^32 - 1 plus one is the first sum that needs a second limb, and the first that a
    // single-limb implementation gets wrong.
    assert_eq!(
        shown(&big("4294967295").add(&big("1")).expect("finite")),
        "4294967296"
    ); // in range
    assert_eq!(
        shown(&big("4294967296").subtract(&big("1")).expect("finite")), // same
        "4294967295"
    );
    // Two limbs to three, and back.
    assert_eq!(
        shown(&big("18446744073709551615").add(&big("1")).expect("finite")), // same
        "18446744073709551616"
    );
    assert_eq!(
        shown(
            &big("18446744073709551616")
                .subtract(&big("1"))
                .expect("finite")
        ), // same
        "18446744073709551615"
    );
    // A borrow that runs through a whole limb of zeros — the case a per-limb borrow gets wrong if
    // it does not carry the borrow onward.
    assert_eq!(
        shown(
            &big("79228162514264337593543950336")
                .subtract(&big("1"))
                .expect("finite")
        ), // same
        "79228162514264337593543950335"
    );
}

#[test]
fn a_sum_of_unlike_signs_takes_the_sign_of_the_larger_magnitude() {
    // The four sign combinations, and the one that is a subtraction in disguise both ways round.
    assert_eq!(shown(&big("5").add(&big("-3")).expect("finite")), "2"); // in range
    assert_eq!(shown(&big("3").add(&big("-5")).expect("finite")), "-2"); // same
    assert_eq!(shown(&big("-5").add(&big("3")).expect("finite")), "-2"); // same
    assert_eq!(shown(&big("-3").add(&big("5")).expect("finite")), "2"); // same
    assert_eq!(shown(&big("-3").add(&big("-5")).expect("finite")), "-8"); // same
    // Equal magnitudes cancel, and the result is not a negative zero — there is no such BigInt.
    let cancelled = big("-5").add(&big("5")).expect("finite"); // same
    assert!(cancelled.is_zero() && !cancelled.is_negative());
}

#[test]
fn multiplication_accumulates_across_limbs() {
    assert_eq!(
        shown(
            &big("4294967295")
                .multiply(&big("4294967295"))
                .expect("finite")
        ), // in range
        "18446744065119617025"
    );
    assert_eq!(shown(&big("0").multiply(&big("123")).expect("finite")), "0"); // same
    assert_eq!(
        shown(&big("-6").multiply(&big("7")).expect("finite")),
        "-42"
    ); // same
    assert_eq!(
        shown(&big("-6").multiply(&big("-7")).expect("finite")),
        "42"
    ); // same
    // A product wide enough that the partial sums overflow a limb several times over.
    assert_eq!(
        shown(
            &big("123456789012345678901234567890")
                .multiply(&big("987654321098765432109876543210"))
                .expect("finite")
        ), // same
        "121932631137021795226185032733622923332237463801111263526900"
    );
}

#[test]
fn division_truncates_towards_zero_and_the_remainder_follows_the_dividend() {
    // §6.1.6.2.5 and §6.1.6.2.6 — the pair that makes `(a / b) * b + (a % b)` equal `a`, which is
    // *not* what a floor division and a modulo would give for a negative.
    for (a, b, quotient, remainder) in [
        ("7", "2", "3", "1"),
        ("-7", "2", "-3", "-1"),
        ("7", "-2", "-3", "1"),
        ("-7", "-2", "3", "-1"),
        ("6", "3", "2", "0"),
        ("1", "2", "0", "1"),
    ] {
        let (q, r) = big(a)
            .divide_and_remainder(&big(b))
            .expect("non-zero divisor"); // the divisor is not zero
        assert_eq!(shown(&q), quotient, "{a} / {b}");
        assert_eq!(shown(&r), remainder, "{a} % {b}");
    }
    // Zero is a RangeError and not an infinity — the loudest difference from Number.
    assert_eq!(big("1").divide(&big("0")), Err(Error::DividedByZero));
    assert_eq!(big("1").remainder(&big("0")), Err(Error::DividedByZero));
}

#[test]
fn a_long_division_gets_its_trial_digits_right() {
    // Knuth's algorithm D, which the single-limb path above never reaches. The values are chosen so
    // that the estimated digit is *wrong* and has to be corrected — a divisor whose top limb is
    // just below a power of two is what forces it.
    for (a, b) in [
        (
            "340282366920938463463374607431768211455",
            "18446744073709551617",
        ),
        (
            "123456789012345678901234567890123456789",
            "987654321098765432109",
        ),
        ("18446744073709551616", "4294967297"),
        ("79228162514264337593543950335", "18446744073709551615"),
    ] {
        let (quotient, remainder) = big(a).divide_and_remainder(&big(b)).expect("non-zero"); // same
        // The identity is the check, because it is the whole contract and it needs no oracle: a
        // wrong trial digit breaks it however plausible the quotient looks.
        let rebuilt = quotient
            .multiply(&big(b))
            .and_then(|product| product.add(&remainder))
            .expect("finite"); // in range
        assert_eq!(shown(&rebuilt), a, "{a} / {b} does not rebuild");
        assert!(
            remainder.magnitude_of().compare(&big(b).magnitude_of()) == std::cmp::Ordering::Less,
            "{a} % {b} is not smaller than the divisor"
        );
    }
}

#[test]
fn exponentiation_squares_rather_than_repeating() {
    assert_eq!(
        shown(&big("2").exponentiate(&big("10")).expect("finite")),
        "1024"
    ); // in range
    assert_eq!(
        shown(&big("2").exponentiate(&big("0")).expect("finite")),
        "1"
    ); // same
    assert_eq!(
        shown(&big("0").exponentiate(&big("0")).expect("finite")),
        "1"
    ); // same
    assert_eq!(
        shown(&big("-2").exponentiate(&big("3")).expect("finite")),
        "-8"
    ); // same
    assert_eq!(
        shown(&big("-2").exponentiate(&big("2")).expect("finite")),
        "4"
    ); // same
    // A power large enough that repeated multiplication would be a different program.
    assert_eq!(
        shown(&big("2").exponentiate(&big("128")).expect("finite")), // same
        "340282366920938463463374607431768211456"
    );
    // §6.1.6.2.3 step 1 — a negative exponent is one half, and a BigInt is an integer.
    assert_eq!(
        big("2").exponentiate(&big("-1")),
        Err(Error::NegativeExponent)
    );
}

#[test]
fn a_right_shift_rounds_towards_negative_infinity_and_a_division_does_not() {
    // The one place `>>` and `/ 2n` disagree, and the reason `shift_right` has a correction in it:
    // an arithmetic shift is a floor and a division truncates.
    assert_eq!(
        shown(&big("-1").shift_right(&big("1")).expect("finite")),
        "-1"
    ); // in range
    assert_eq!(shown(&big("-1").divide(&big("2")).expect("non-zero")), "0"); // same
    assert_eq!(
        shown(&big("-5").shift_right(&big("1")).expect("finite")),
        "-3"
    ); // same
    assert_eq!(
        shown(&big("-4").shift_right(&big("1")).expect("finite")),
        "-2"
    ); // same
    assert_eq!(
        shown(&big("5").shift_right(&big("1")).expect("finite")),
        "2"
    ); // same
    // Across a whole limb and past the end of the number.
    assert_eq!(
        shown(&big("4294967296").shift_right(&big("32")).expect("finite")),
        "1"
    ); // same
    assert_eq!(
        shown(&big("1").shift_right(&big("64")).expect("finite")),
        "0"
    ); // same
    assert_eq!(
        shown(&big("-1").shift_right(&big("64")).expect("finite")),
        "-1"
    ); // same
    // A left shift is a multiply by a power of two, and a negative count turns it round.
    assert_eq!(
        shown(&big("1").shift_left(&big("32")).expect("finite")),
        "4294967296"
    ); // same
    assert_eq!(
        shown(&big("-1").shift_left(&big("3")).expect("finite")),
        "-8"
    ); // same
    assert_eq!(
        shown(&big("8").shift_left(&big("-3")).expect("finite")),
        "1"
    ); // same
    assert_eq!(
        shown(&big("1").shift_right(&big("-3")).expect("finite")),
        "8"
    ); // same
}

#[test]
fn the_bitwise_operators_agree_with_two_s_complement() {
    // §6.1.6.2.17's operands are the infinite two's-complement expansions, so a negative operand is
    // an infinite run of leading ones — which is why `-1n & x` is `x` and `-1n | x` is `-1n`.
    assert_eq!(shown(&big("12").and(&big("10")).expect("finite")), "8"); // in range
    assert_eq!(shown(&big("12").or(&big("10")).expect("finite")), "14"); // same
    assert_eq!(shown(&big("12").xor(&big("10")).expect("finite")), "6"); // same
    assert_eq!(shown(&big("-1").and(&big("12")).expect("finite")), "12"); // same
    assert_eq!(shown(&big("-1").or(&big("12")).expect("finite")), "-1"); // same
    assert_eq!(shown(&big("-1").xor(&big("0")).expect("finite")), "-1"); // same
    // Two negatives, where both expansions have leading ones and the result does too.
    assert_eq!(shown(&big("-12").and(&big("-10")).expect("finite")), "-12"); // same
    assert_eq!(shown(&big("-12").or(&big("-10")).expect("finite")), "-10"); // same
    assert_eq!(shown(&big("-12").xor(&big("-10")).expect("finite")), "2"); // same
    // One of each sign, which is where a width chosen too narrow gives a plausible wrong answer.
    assert_eq!(
        shown(&big("-1").and(&big("4294967295")).expect("finite")),
        "4294967295"
    ); // same
    assert_eq!(
        shown(&big("-4294967296").and(&big("4294967295")).expect("finite")),
        "0"
    ); // same
    // §6.1.6.2.2 — `~x` is `-(x + 1)`, at every magnitude.
    assert_eq!(shown(&big("0").not().expect("finite")), "-1"); // same
    assert_eq!(shown(&big("-1").not().expect("finite")), "0"); // same
    assert_eq!(
        shown(&big("4294967295").not().expect("finite")),
        "-4294967296"
    ); // same
}

#[test]
fn comparison_orders_by_sign_first_and_then_by_magnitude() {
    use std::cmp::Ordering::{Equal, Greater, Less};
    assert_eq!(big("1").compare(&big("2")), Less);
    assert_eq!(big("2").compare(&big("1")), Greater);
    assert_eq!(big("2").compare(&big("2")), Equal);
    // A negative is below every non-negative however large its magnitude.
    assert_eq!(big("-100000000000000000000").compare(&big("0")), Less);
    assert_eq!(big("0").compare(&big("-1")), Greater);
    // …and among two negatives the bigger magnitude is the smaller number.
    assert_eq!(big("-2").compare(&big("-1")), Less);
    assert_eq!(big("-1").compare(&big("-2")), Greater);
    // Length decides before contents, which is what the no-trailing-zero invariant buys.
    assert_eq!(big("4294967296").compare(&big("4294967295")), Greater);
    // Equality is a comparison of the two fields, so two ways of reaching the same value agree.
    assert_eq!(
        big("4294967296"),
        big("4294967295").add(&big("1")).expect("finite")
    ); // in range
}

#[test]
fn a_result_past_what_this_engine_will_hold_is_refused_rather_than_attempted() {
    // §6.1.6.2 bounds nothing and no implementation can honour that, so the ceiling is ViperJS's —
    // and the answer at it is a refusal rather than an allocation that never returns.
    let huge = big("2").exponentiate(&big("100")).expect("finite"); // in range
    assert_eq!(big("2").exponentiate(&huge), Err(Error::TooLarge));
    assert_eq!(big("1").shift_left(&huge), Err(Error::TooLarge));
    // …and a shift *right* by an unrepresentable count is not an error: everything is shifted out,
    // which is a number rather than a refusal.
    assert_eq!(
        shown(&big("12345").shift_right(&huge).expect("finite")),
        "0"
    ); // same
    assert_eq!(
        shown(&big("-12345").shift_right(&huge).expect("finite")),
        "-1"
    ); // same
}

#[test]
fn division_rebuilds_its_dividend_over_a_few_thousand_shapes() {
    // The hand-picked cases above were chosen by a reader thinking about where a carry goes, which
    // is exactly the reasoning a bug survives. This walks a deterministic spread of magnitudes and
    // checks the only thing that has to hold: `(a / b) * b + (a % b) == a`, with `|a % b| < |b|`.
    //
    // A wrong trial digit in Knuth D produces a quotient that is off by one and a remainder that is
    // negative or larger than the divisor — neither of which looks wrong from the outside, and both
    // of which this catches on the first pair that reaches them.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        // xorshift64, written out because a test that needs randomness needs it to be the *same*
        // randomness on every run — a failure nobody can reproduce is not a failure anybody fixes.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // The shapes that force Knuth's step D6 — the add-back that only runs when the trial digit came
    // out one too large. Random limbs reach it about once in four billion, so a sweep of them says
    // nothing about it at all; a divisor whose top limb is 0x8000_0000 or 0xFFFF_FFFF and a
    // dividend just under a multiple of it is what makes the estimate overshoot.
    const AWKWARD: &[u32] = &[
        0,
        1,
        0xFFFF_FFFF,
        0xFFFF_FFFE,
        0x8000_0000,
        0x8000_0001,
        0x7FFF_FFFF,
    ];
    let build = |limbs: usize, next: &mut dyn FnMut() -> u64| {
        let mut value = BigInt::zero();
        let shift = BigInt::from_u64(32);
        for _ in 0..limbs {
            value = value
                .shift_left(&shift)
                .and_then(|moved| moved.add(&BigInt::from_u64(next() >> 32)))
                .expect("in range"); // a few limbs is not the ceiling
        }
        value
    };
    let mut checked = 0;
    for wide in 1..=6 {
        for narrow in 1..=wide {
            for _ in 0..40 {
                let a = build(wide, &mut next);
                let b = build(narrow, &mut next);
                if b.is_zero() {
                    continue;
                }
                for (a, b) in [
                    (a.clone(), b.clone()),
                    (a.negate(), b.clone()),
                    (a.clone(), b.negate()),
                    (a.negate(), b.negate()),
                ] {
                    let (quotient, remainder) =
                        a.divide_and_remainder(&b).expect("non-zero divisor"); // checked above
                    let rebuilt = quotient
                        .multiply(&b)
                        .and_then(|product| product.add(&remainder))
                        .expect("in range"); // same
                    assert_eq!(
                        rebuilt,
                        a,
                        "{} / {} rebuilt as {}",
                        shown(&a),
                        shown(&b),
                        shown(&rebuilt)
                    );
                    assert_eq!(
                        remainder.magnitude_of().compare(&b.magnitude_of()),
                        std::cmp::Ordering::Less,
                        "{} % {} = {} is not smaller than the divisor",
                        shown(&a),
                        shown(&b),
                        shown(&remainder)
                    );
                    // §6.1.6.2.6 — the remainder takes the sign of the *dividend*, always.
                    assert!(
                        remainder.is_zero() || remainder.is_negative() == a.is_negative(),
                        "{} % {} = {} has the wrong sign",
                        shown(&a),
                        shown(&b),
                        shown(&remainder)
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 3000, "only {checked} pairs were checked");

    // …and the same identity over the awkward shapes, in every arrangement of two and three limbs.
    // This is what reaches D6: the pairs are chosen for the estimate to overshoot rather than drawn
    // from a distribution where it almost never does.
    let limbs_of = |limbs: &[u32]| {
        let mut value = BigInt::zero();
        let shift = BigInt::from_u64(32);
        for limb in limbs.iter().rev() {
            value = value
                .shift_left(&shift)
                .and_then(|moved| moved.add(&BigInt::from_u64(u64::from(*limb))))
                .expect("in range"); // three limbs is not the ceiling
        }
        value
    };
    let mut awkward = 0;
    for high in AWKWARD {
        for low in AWKWARD {
            let divisor = limbs_of(&[*low, *high]);
            if divisor.is_zero() {
                continue;
            }
            for third in AWKWARD {
                for second in AWKWARD {
                    let dividend = limbs_of(&[*second, *third, *high]);
                    let (quotient, remainder) =
                        dividend.divide_and_remainder(&divisor).expect("non-zero"); // checked above
                    let rebuilt = quotient
                        .multiply(&divisor)
                        .and_then(|product| product.add(&remainder))
                        .expect("in range"); // same
                    assert_eq!(
                        rebuilt,
                        dividend,
                        "{} / {} rebuilt as {}",
                        shown(&dividend),
                        shown(&divisor),
                        shown(&rebuilt)
                    );
                    assert_eq!(
                        remainder.magnitude_of().compare(&divisor.magnitude_of()),
                        std::cmp::Ordering::Less,
                        "{} % {} is not smaller than the divisor",
                        shown(&dividend),
                        shown(&divisor)
                    );
                    awkward += 1;
                }
            }
        }
    }
    assert!(awkward > 1000, "only {awkward} awkward pairs were checked");
}

#[test]
fn a_shift_of_zero_and_a_shift_past_the_end_are_still_numbers() {
    // The edges of both shifts, which the arithmetic tests reach through and never *at*.
    assert_eq!(
        shown(&big("0").shift_left(&big("100")).expect("finite")),
        "0"
    ); // in range
    assert_eq!(
        shown(&big("0").shift_right(&big("100")).expect("finite")),
        "0"
    ); // same
    assert_eq!(shown(&big("0").shift_left(&big("0")).expect("finite")), "0"); // same
    assert_eq!(shown(&big("7").shift_left(&big("0")).expect("finite")), "7"); // same
    assert_eq!(
        shown(&big("7").shift_right(&big("0")).expect("finite")),
        "7"
    ); // same
    // Exactly as many places as there are bits, and one more — the boundary of "everything is
    // shifted out", which decides whether a limb of zeros is produced or none at all.
    assert_eq!(
        shown(&big("4294967295").shift_right(&big("32")).expect("finite")),
        "0"
    ); // same
    assert_eq!(
        shown(&big("4294967295").shift_right(&big("31")).expect("finite")),
        "1"
    ); // same
    assert_eq!(
        shown(&big("4294967296").shift_right(&big("33")).expect("finite")),
        "0"
    ); // same
    // A negative shifted entirely out is -1 and not 0, at the same two boundaries: an arithmetic
    // shift keeps the sign bit however far it goes.
    assert_eq!(
        shown(&big("-4294967295").shift_right(&big("32")).expect("finite")),
        "-1"
    ); // same
    assert_eq!(
        shown(&big("-4294967296").shift_right(&big("32")).expect("finite")),
        "-1"
    ); // same
    // …and one whose discarded bits were all zero does *not* round down, which is the correction
    // in `shift_right` and the only thing that tells the two apart.
    assert_eq!(
        shown(&big("-4294967296").shift_right(&big("31")).expect("finite")),
        "-2"
    ); // same
    assert_eq!(
        shown(&big("-4294967296").shift_right(&big("30")).expect("finite")),
        "-4"
    ); // same
    assert_eq!(
        shown(&big("-6").shift_right(&big("1")).expect("finite")),
        "-3"
    ); // same
    assert_eq!(
        shown(&big("-7").shift_right(&big("1")).expect("finite")),
        "-4"
    ); // same
}

#[test]
fn the_ceiling_refuses_one_limb_past_it_and_allows_the_last_one() {
    // Both sides of the limit, which is what a limit means — and the pair is what says which way
    // round the comparison goes. Written with `is_ok` rather than `assert_eq!`: a failure here
    // would otherwise print four megabytes of limbs.
    //
    // The shift lands a single set bit `MAX_LIMBS - 2` whole limbs up, so the magnitude that comes
    // back is one limb short of the ceiling — the top limb the shift reserved is zero and trimmed.
    let bits = BigInt::from_u64((MAX_LIMBS as u64 - 2) * 32);
    let at_the_edge = BigInt::from_u64(1)
        .shift_left(&bits)
        .expect("one limb inside the ceiling"); // the ceiling is the test
    assert!(!at_the_edge.is_zero());
    // A sum of two of those is *exactly* the ceiling, and exactly the ceiling is allowed.
    assert!(at_the_edge.add(&at_the_edge).is_ok());
    // One limb further **is** allowed, and this row used to say otherwise. The shift reserves a
    // limb for what spills out of the top, and that reservation was being counted against the
    // ceiling — so a magnitude that lands exactly on it was refused. Trimming takes the reserved
    // limb back when nothing spilled into it, and the ceiling applies to what is left.
    //
    // Not a tidying change: the refusal it produced was read as "cannot happen" by
    // `divide_magnitude`, swallowed with `unwrap_or_default`, and became an empty divisor that
    // indexed `divisor[n - 1]` at `usize::MAX`. GHSA-6976-qm5m-7mcj.
    assert!(at_the_edge.shift_left(&BigInt::from_u64(32)).is_ok());
    // Two limbs further is past the ceiling, and that is where the refusal belongs.
    assert_eq!(
        at_the_edge.shift_left(&BigInt::from_u64(64)).err(),
        Some(Error::TooLarge)
    );
    // …and a product of two near-ceiling values is far past it, by the same comparison.
    assert_eq!(
        at_the_edge.multiply(&at_the_edge).err(),
        Some(Error::TooLarge)
    );
    // Zero has no magnitude to grow, so no shift of it is ever a refusal — which is what the
    // emptiness check at the top of the shift is for.
    let far = BigInt::from_u64(u64::from(u32::MAX) * 32);
    assert_eq!(
        shown(&BigInt::zero().shift_left(&far).expect("zero has no width")),
        "0"
    ); // same
}

#[test]
fn a_shift_count_of_more_than_one_limb_is_read_whole() {
    // The count is a BigInt and is turned into a `u64` to be used. Reading only its low limb gives
    // a much smaller shift, which for a *right* shift is the difference between an answer and the
    // original number — 2^33 places is everything, and 2^33's low limb is zero.
    let far = BigInt::from_u64(1u64 << 33);
    assert_eq!(
        shown(&big("123456").shift_right(&far).expect("finite")),
        "0"
    ); // in range
    assert_eq!(
        shown(&big("-123456").shift_right(&far).expect("finite")),
        "-1"
    ); // same
    // …and the same count as a left shift is past the ceiling rather than a number.
    assert_eq!(big("1").shift_left(&far), Err(Error::TooLarge));
}

#[test]
fn a_two_s_complement_negate_carries_through_a_zero_limb() {
    // Inverting a magnitude whose low limb is zero produces `0xFFFF_FFFF + 1` there, and the carry
    // has to reach the limb above it. `-2^32` is the smallest value that has one, and every
    // bitwise operator goes through the conversion both ways.
    assert_eq!(
        shown(&big("-4294967296").or(&big("1")).expect("finite")),
        "-4294967295"
    ); // in range
    assert_eq!(
        shown(&big("-4294967296").xor(&big("1")).expect("finite")),
        "-4294967295"
    ); // same
    assert_eq!(
        shown(&big("-4294967296").and(&big("-1")).expect("finite")),
        "-4294967296"
    ); // same
    // Two limbs of zeros below the set bit, so the carry runs further than one place.
    assert_eq!(
        shown(&big("-18446744073709551616").or(&big("1")).expect("finite")), // same
        "-18446744073709551615"
    );
    // And back out again: a result that is negative is converted the other way, through the same
    // carry, which is what makes these two tests one claim rather than two.
    assert_eq!(
        shown(&big("-4294967296").not().expect("finite")),
        "4294967295"
    ); // same
}

#[test]
fn sixty_four_bits_go_out_and_come_back_the_way_the_sign_says() {
    // §25.3.1.2's eight bytes, both ways. The same bits are two different BigInts depending on
    // whether the top one is read as a sign, which is the whole of the difference between
    // `BigInt64Array` and `BigUint64Array`.
    assert_eq!(shown(&BigInt::from_bits(u64::MAX, true)), "-1");
    assert_eq!(
        shown(&BigInt::from_bits(u64::MAX, false)),
        "18446744073709551615"
    );
    assert_eq!(
        shown(&BigInt::from_bits(1 << 63, true)),
        "-9223372036854775808"
    );
    assert_eq!(
        shown(&BigInt::from_bits(1 << 63, false)),
        "9223372036854775808"
    );
    assert_eq!(shown(&BigInt::from_bits(0, true)), "0");
    assert_eq!(shown(&BigInt::from_bits(7, true)), "7");
    // Out again, where a value too large for the slot is taken modulo 2^64 rather than refused —
    // which is what a fixed-width write is.
    assert_eq!(big("-1").low_u64(), u64::MAX);
    assert_eq!(big("18446744073709551615").low_u64(), u64::MAX);
    assert_eq!(big("18446744073709551616").low_u64(), 0);
    assert_eq!(big("0").low_u64(), 0);
    assert_eq!(big("7").low_u64(), 7);
    // …and a round trip through both, at the edges where a sign is decided.
    for bits in [0u64, 1, 7, i64::MAX as u64, 1 << 63, u64::MAX] {
        assert_eq!(
            BigInt::from_bits(bits, true).low_u64(),
            bits,
            "signed {bits}"
        );
        assert_eq!(
            BigInt::from_bits(bits, false).low_u64(),
            bits,
            "unsigned {bits}"
        );
    }
}

#[test]
fn the_nearest_number_is_found_by_rounding_the_bits_and_never_by_accumulating_limbs() {
    // `𝔽(ℝ(x))`, which §21.1.1.1 is the only caller of. Exact while the value fits in a double's
    // fifty-three significant bits…
    for value in [0i64, 1, -1, 2, -2, 255, -255, 1 << 52, -(1 << 52)] {
        let big = match value < 0 {
            true => BigInt::from_u64(value.unsigned_abs()).negate(),
            false => BigInt::from_u64(value as u64),
        };
        assert_eq!(big.to_f64(), value as f64, "{value}");
    }
    // …and zero is `+0` rather than `-0`, §6.1.6.2 having only the one.
    assert!(BigInt::zero().to_f64().is_sign_positive());
    // Past fifty-three bits it rounds to nearest, ties to even — the three consecutive integers
    // above 2^53 are the whole of that rule, and each of them fails a different way when it is
    // written wrong. `+1` is a tie broken downwards, `+2` is exact, `+3` is a tie broken upwards.
    let two_53 = BigInt::from_u64(1 << 53);
    let one = BigInt::from_u64(1);
    let plus = |n: u64| two_53.add(&BigInt::from_u64(n)).expect("small enough");
    assert_eq!(two_53.to_f64(), 9_007_199_254_740_992.0);
    assert_eq!(
        plus(1).to_f64(),
        9_007_199_254_740_992.0,
        "ties to even, down"
    );
    assert_eq!(plus(2).to_f64(), 9_007_199_254_740_994.0, "exact");
    assert_eq!(
        plus(3).to_f64(),
        9_007_199_254_740_996.0,
        "ties to even, up"
    );
    assert_eq!(plus(4).to_f64(), 9_007_199_254_740_996.0, "exact");
    assert_eq!(plus(5).to_f64(), 9_007_199_254_740_996.0, "below halfway");
    // The sign is carried through the rounding rather than applied to a magnitude that was rounded
    // as though positive — which is the same answer here, and is not for a `low_u64` that
    // two's-complements.
    assert_eq!(plus(1).negate().to_f64(), -9_007_199_254_740_992.0);
    assert_eq!(BigInt::from_u64(1).negate().to_f64(), -1.0);
    // Rounding up out of fifty-three bits carries into the exponent: 2^53 - 1 is exact, and one
    // more than the largest odd value below a power of two is that power.
    let almost = two_53.subtract(&one).expect("no borrow past zero");
    assert_eq!(almost.to_f64(), 9_007_199_254_740_991.0);
    // A value with fifty-four significant bits whose low bits force a carry all the way up.
    let carry = BigInt::from_u64((1 << 54) - 1);
    assert_eq!(carry.to_f64(), 18_014_398_509_481_984.0, "2^54, carried");
    // Past the largest finite double there is no nearest Number, and §5.2 answers with an infinity
    // — with the sign, which is the one place the two infinities are told apart here.
    let two = BigInt::from_u64(2);
    let huge = two
        .exponentiate(&BigInt::from_u64(1024))
        .expect("in budget");
    assert_eq!(huge.to_f64(), f64::INFINITY);
    assert_eq!(huge.negate().to_f64(), f64::NEG_INFINITY);
    // …and just below it is finite, which is what says the boundary is in the right place rather
    // than merely somewhere.
    let large = two
        .exponentiate(&BigInt::from_u64(1023))
        .expect("in budget");
    assert!(large.to_f64().is_finite());
    assert_eq!(large.to_f64(), 2f64.powi(1023));
    // The largest finite double itself, which is 2^1024 - 2^971 and rounds to itself.
    let largest = huge
        .subtract(&two.exponentiate(&BigInt::from_u64(971)).expect("in budget"))
        .expect("no borrow");
    assert_eq!(largest.to_f64(), f64::MAX);
    // A round trip through the other direction, which is exact for every integer a double names.
    for value in [1.0f64, -1.0, 1e15, -1e15, 2f64.powi(60), f64::MAX] {
        let there = BigInt::from_f64(value).expect("an integer");
        assert_eq!(there.to_f64(), value, "{value}");
    }
    // Bits spanning three limbs, which is what the `u128` accumulator is for: a `u64` would drop
    // the most significant limb and answer with a tiny number instead of a huge one.
    let wide = two.exponentiate(&BigInt::from_u64(100)).expect("in budget");
    assert_eq!(wide.to_f64(), 2f64.powi(100));
    let offset = wide.add(&BigInt::from_u64(1)).expect("in budget");
    assert_eq!(
        offset.to_f64(),
        2f64.powi(100),
        "the one is below the precision"
    );
}

#[test]
fn rounding_reads_the_halfway_bit_and_everything_under_it_from_the_right_limbs() {
    // The rows above all round inside one limb. These are the same rule asked where the fifty-three
    // bits kept, the halfway bit and the sticky bits fall in *different* limbs — which is where a
    // shift by the wrong amount, or a limb index off by a factor of thirty-two, stops being
    // invisible. Every value here is 2^100 plus something, so `bit_length` is 101 and the halfway
    // bit is bit 47: limb one, bit fifteen.
    let two = BigInt::from_u64(2);
    let pow2 = |n: u64| {
        two.exponentiate(&BigInt::from_u64(n))
            .expect("well inside the limb ceiling")
    };
    let sum = |parts: &[u64]| {
        parts.iter().fold(BigInt::zero(), |total, bit| {
            total.add(&pow2(*bit)).expect("in budget")
        })
    };
    // Exactly halfway with an even mantissa: ties to even keeps it, so the 2^47 is dropped.
    assert_eq!(sum(&[100, 47]).to_f64(), 2f64.powi(100));
    // Exactly halfway with an **odd** mantissa: ties to even rounds up, and the step at this
    // magnitude is 2^48 — so the answer is 2^100 + 2^49, not 2^100 + 2^48.
    assert_eq!(sum(&[100, 48, 47]).to_f64(), 2f64.powi(100) + 2f64.powi(49));
    // Below halfway with an odd mantissa: nothing moves.
    assert_eq!(sum(&[100, 48]).to_f64(), 2f64.powi(100) + 2f64.powi(48));
    // Past halfway by one bit in a *lower limb*, with an even mantissa — the sticky bit is the
    // whole of what makes this differ from the first row, and it is thirty-seven bits below the
    // one that decides the tie.
    assert_eq!(sum(&[100, 47, 10]).to_f64(), 2f64.powi(100) + 2f64.powi(48));
    // …and a sticky bit *above* the halfway bit is not a sticky bit at all: it is part of the
    // mantissa, and reading it as one would round this up.
    assert_eq!(sum(&[100, 60, 47]).to_f64(), 2f64.powi(100) + 2f64.powi(60));
    // The overflow boundary with a fraction, which is the one case `>=` and `>` disagree about:
    // an exponent field of 0x7FF with a non-zero fraction is a NaN, not an infinity.
    let over = sum(&[1024, 1000]).to_f64();
    assert!(over.is_infinite(), "an infinity, not a NaN: {over}");
    assert!(over.is_sign_positive());
    assert!(sum(&[1024, 1000]).negate().to_f64().is_sign_negative());
    // …and the largest bit length that is still finite, so the boundary is pinned from both sides.
    assert!(sum(&[1023, 1000]).to_f64().is_finite());
}
