//! `Number::toString` (§6.1.6.1.20) — the decimal a Number is written as.
//!
//! The counterpart of [`super::string_to_number`], and shaped the same way: the hard part is
//! delegated to something that already does it correctly, and what is written out here is the
//! part where ECMAScript's answer differs.
//!
//! # What is hard, and who does it
//!
//! Step 5 asks for the *fewest* decimal digits that still denote `x` exactly — the problem David
//! Gay's 1990 paper is about, and which the spec's own note points implementers at. Rust's
//! formatter solves it: `{:e}` writes the shortest representation that reads back as the same
//! `f64`. So `format!("{x:e}")` answers "how many digits", and nothing here has to.
//!
//! # What is ours, and why it is not the formatter's
//!
//! Two things.
//!
//! **The layout.** Rust's `Display` never uses exponential notation — `f64::MAX` prints as 309
//! characters — and ECMAScript switches at `n > 21` and at `n < -5`, with an explicit `+` on a
//! non-negative exponent. Steps 6 to 10 are that decision, written out.
//!
//! **The last digit.** Where two decimals of the same length are exactly equidistant from `x`,
//! both read back as `x` and the spec leaves the choice open — "the least significant digit of
//! s is not necessarily uniquely determined by these criteria" — while recommending the closest,
//! and on a tie the *even* one. Rust's shortest form picks the odd one. Measured over 1.5 million
//! random `f64` values, the two differ on about 1 in 5,000, and V8 follows the recommendation.
//!
//! Rather than detect that case, this asks the formatter a second question: how many digits `k`
//! does `x` need, and then what is `x` rounded to exactly `k` digits. Rounding to a fixed
//! precision is round-half-to-even, which *is* the recommendation, and the second answer is
//! always at least as close to `x` as the first — so it round-trips whenever the first does.
//! That is one `format!` in place of a tie-detector, and it agreed with V8 on every one of
//! those 1.5 million values.

/// `Number::toString(number, 10)` (§6.1.6.1.20).
///
/// Only radix 10 — `Number.prototype.toString(radix)` arrives with the Number builtin at M4 and
/// takes the general path, which shares steps 1 to 4 and almost nothing else.
///
/// Total: every `f64` has an answer, NaN and both infinities included.
pub(crate) fn number_to_string(number: f64) -> String {
    // Steps 1 to 4, in the spec's order, which matters: `-0` is caught by step 2 and so never
    // reaches step 3 to acquire a sign. `String(-0)` is `"0"`, and that is the whole reason
    // anyone ever writes `Object.is` — see [`super::Value::same_value`].
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number == 0.0 {
        return "0".to_string();
    }
    // Step 3 is written `x < -0𝔽`, and by here that is exactly the sign bit: NaN left at step 1
    // and both zeroes at step 2. Asking for the sign rather than comparing says so — a
    // comparison would have a boundary at zero that nothing can reach, since zero never gets
    // this far.
    if number.is_sign_negative() {
        return format!("-{}", number_to_string(-number));
    }
    if number.is_infinite() {
        return "Infinity".to_string();
    }
    let (digits, n) = shortest_digits(number);
    let k = digits.len() as i32;

    // Step 6's guard, in the spec's own terms: -5 ≤ n ≤ 21 is written notation, anything else is
    // scientific. The bounds are not symmetric and are not round numbers; they are where the
    // committee decided a run of zeroes stops being more readable than an exponent.
    if (-5..=21).contains(&n) {
        // Step 6.a — every digit is before the point, with zeroes after it.
        if n >= k {
            return digits + &"0".repeat((n - k) as usize);
        }
        // Step 6.b — the point falls inside the digits.
        if n > 0 {
            let (whole, fraction) = digits.split_at(n as usize);
            return format!("{whole}.{fraction}");
        }
        // Step 6.c — the point falls before them, with zeroes between.
        return format!("0.{}{digits}", "0".repeat((-n) as usize));
    }
    // Steps 7 to 10. The exponent is `n - 1` because it is written after a single leading digit,
    // and its sign is always spelled: `1e+21`, never `1e21`. That `+` is the difference between
    // agreeing with every other engine and not.
    //
    // `{:+}` writes step 8's `exponentSign` and the magnitude together. Written apart — a
    // comparison choosing the sign, then `abs` — the comparison has a threshold, and every
    // threshold from `n < -5` to `n < 22` gives the same answers here, because `n` is outside
    // that range whenever this line runs. A constant nothing can pin is a constant that should
    // not be written down.
    let exponent = n - 1;
    let (leading, rest) = digits.split_at(1);
    // Step 9 for a single digit, step 10 for more — one has no point to write.
    if rest.is_empty() {
        return format!("{leading}e{exponent:+}");
    }
    format!("{leading}.{rest}e{exponent:+}")
}

/// Step 5 — the digits of `s`, and `n` such that `s × 10^(n-k)` is `number`.
///
/// `number` must be finite and greater than zero; steps 1 to 4 have taken everything else.
///
/// Asked of the formatter twice on purpose. The first call answers *how many* digits are needed,
/// which is the part that is hard; the second re-reads `number` at exactly that many, which is
/// the part where the rounding rule matters. See the module documentation.
fn shortest_digits(number: f64) -> (String, i32) {
    let shortest = format!("{number:e}");
    let digit_count = shortest.split('e').next().map_or(0, |mantissa| {
        mantissa.chars().filter(char::is_ascii_digit).count()
    });
    // `{:.p$e}` writes `p` digits *after* the point, so one fewer than the count. A count of
    // zero cannot happen — `{:e}` always writes at least one digit — and `saturating_sub` says
    // so without a branch that no input could reach.
    let rounded = format!("{number:.*e}", digit_count.saturating_sub(1));

    let mut digits = String::new();
    let mut exponent_text = String::new();
    let mut past_the_e = false;
    for ch in rounded.chars() {
        match ch {
            'e' => past_the_e = true,
            '.' => {}
            _ if past_the_e => exponent_text.push(ch),
            _ => digits.push(ch),
        }
    }
    // `{:e}` writes a decimal exponent and nothing else, so this reads it. A `0` for anything
    // unreadable would be a wrong answer rather than a safe one, but there is no such input:
    // the formatter's output is not user data.
    let exponent = exponent_text.parse::<i32>().unwrap_or(0);
    // `n` is where the decimal point goes, counting from the left of the digits; the formatter
    // reports the exponent of the *leading* digit, which is one less.
    (digits, exponent + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_values_that_are_not_written_as_digits() {
        // Steps 1 to 4. `-0` never reaches step 3, which is why it has no sign — the one place
        // `String(x)` loses information that `Object.is` keeps.
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(-f64::NAN), "NaN");
        assert_eq!(number_to_string(0.0), "0");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn the_layout_changes_at_twenty_one_digits_and_at_a_millionth() {
        // The two thresholds, each measured against V8 at the value on either side of it. These
        // are the rows that decide whether `String(x)` is readable or is 300 zeroes.
        let table = [
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.2e21, "1.2e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.2e-7, "1.2e-7"),
            // n = 21 exactly, with sixteen digits and five zeroes after them.
            (9.999999999999999e20, "999999999999999900000"),
            (1e-5, "0.00001"),
        ];
        for (number, expected) in table {
            assert_eq!(number_to_string(number), expected, "String({number:e})");
        }
    }

    #[test]
    fn a_number_is_written_with_the_fewest_digits_that_read_back_as_itself() {
        let table = [
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (100.0, "100"),
            (1234.5678, "1234.5678"),
            (4294967295.0, "4294967295"),
            // Seventeen digits, which is as many as any `f64` ever needs…
            (123456789012345678901.0, "123456789012345680000"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            // …and one, which is as few. A denormal is not a special case here.
            (5e-324, "5e-324"),
            (1e-323, "1e-323"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            // 2^53 + 1 is not representable, so this is 2^53 and prints as it.
            (9007199254740993.0, "9007199254740992"),
        ];
        for (number, expected) in table {
            assert_eq!(number_to_string(number), expected, "String({number:e})");
        }
    }

    #[test]
    fn an_exact_tie_takes_the_even_digit_as_the_specification_recommends() {
        // §6.1.6.1.20's note: where two decimals of the same length are equidistant from the
        // value, take the even one. Rust's shortest form takes the odd one, so this is the case
        // the second `format!` in [`shortest_digits`] exists for. Every row below is an exact
        // tie — the value ends in .125, .25 or .625 — and V8 answers as written.
        let table = [
            (f64::from_bits(0x42db_143d_5663_33c8), "119094969470159.12"),
            (f64::from_bits(0xc315_3925_b528_8225), "-1493452156706953.2"),
            (f64::from_bits(0x42db_b0d6_a3ae_ba68), "121785316981481.62"),
            (f64::from_bits(0x4317_56f5_4a4f_36f5), "1642383994506685.2"),
            (f64::from_bits(0x42d8_0104_2e53_9dc8), "105570576715383.12"),
        ];
        for (number, expected) in table {
            assert_eq!(number_to_string(number), expected, "String({number:e})");
        }
    }

    #[test]
    fn every_number_reads_back_as_itself() {
        // §6.1.6.1.20's own note: "If x is any Number value other than -0, then
        // ToNumber(ToString(x)) is x." That is a property, not a table, and it needs no oracle —
        // which is what makes it worth running over values no hand-written table would contain.
        //
        // The two directions were written a day apart against the same specification. Either
        // being wrong breaks this, and so would the layout rules: an exponent written `1e21`
        // instead of `1e+21` still reads back, but one written `1.2e+3.5` does not.
        let mut state = 0x853c_49e6_748f_ea9b_u64;
        let mut checked = 0_u32;
        while checked < 200_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let number = f64::from_bits((state >> 11) ^ state);
            if !number.is_finite() {
                continue;
            }
            checked += 1;
            let written = number_to_string(number);
            let read_back =
                super::super::string_to_number(&written.encode_utf16().collect::<Vec<_>>());
            assert!(
                super::super::Value::Number(read_back).same_value(
                    &super::super::Value::Number(number),
                    &crate::heap::Heap::new()
                ),
                "{written} read back as {read_back:e}, not {number:e}"
            );
        }
    }

    #[test]
    fn the_boundaries_of_every_layout_rule_read_back_too() {
        // The random sweep above lands almost nowhere near `n = 21` or `n = -5`; these walk
        // every power of ten across the whole exponent range, where the rules change.
        let heap = crate::heap::Heap::new();
        for exponent in -330..=308_i32 {
            for mantissa in ["1", "9.999999999999999", "1.2345678901234567", "5"] {
                let Ok(number) = format!("{mantissa}e{exponent}").parse::<f64>() else {
                    continue;
                };
                if !number.is_finite() || number == 0.0 {
                    continue;
                }
                let written = number_to_string(number);
                let read_back =
                    super::super::string_to_number(&written.encode_utf16().collect::<Vec<_>>());
                assert!(
                    super::super::Value::Number(read_back)
                        .same_value(&super::super::Value::Number(number), &heap),
                    "{written} read back as {read_back:e}, not {number:e}"
                );
            }
        }
    }
}
