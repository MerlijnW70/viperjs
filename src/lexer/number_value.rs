//! What a numeric literal *denotes* (ECMA-262 §12.9.3.2 `MV`, §12.9.3.3 `NumericValue`).
//!
//! Apart from the scanner in [`super::number`] because it answers a different question — that one
//! decides how far a literal reaches, this one decides what it is worth — and because it is a
//! pure function of the text, which is what lets it be checked against an oracle that shares
//! none of its machinery.
//!
//! §12.9.3.3 asks for two different things and gets two implementations. See [`numeric_value`].

use crate::span::Span;

/// The value a numeric literal denotes, or `None` if `span` does not cover one.
///
/// # How the value is computed, and why it is exactly right
///
/// §12.9.3.3 gives two different answers, and they need two different implementations:
///
/// - **`NonDecimalIntegerLiteral` and `LegacyOctalIntegerLiteral` denote `𝔽(MV)`** — the exact
///   mathematical value, rounded to the nearest `f64`. No approximation is licensed at all. All
///   three radixes are powers of two, so the digits *are* the binary expansion: the top 64 bits
///   are accumulated exactly, everything below is folded into a sticky bit, and the conversion
///   rounds once. See `power_of_two_value`.
/// - **A `DecimalLiteral` denotes `RoundMVResult(MV)`**, which returns `𝔽(MV)` when the decimal
///   has 20 or fewer significant digits and otherwise permits truncating at the 20th. We always
///   compute `𝔽(MV)` — correctly rounding the whole string — which is conformant for the longer
///   case too: the two options `RoundMVResult` offers bracket the true value, `𝔽` is monotonic,
///   and 20 digits is already more than the 17 that identify a `f64` uniquely, so `𝔽(MV)` is
///   necessarily one of them. It is also what every other engine does, which matters more, since
///   test262 asserts exact values.
///
/// Returns `None` for a span that is off a character boundary, covers no digits, or covers a
/// `BigInt` — the `n` suffix denotes a value of a type this engine does not have yet (M7), and
/// answering with the `f64` nearby would be worse than answering nothing.
///
/// ```
/// use praxis::lexer::{Goal, Lexer, TokenKind, numeric_value};
///
/// let source = "0x1_F";
/// let token = Lexer::new(source).next_token(Goal::Div).expect("this lexes");
/// assert_eq!(token.kind, TokenKind::Number { legacy: false });
/// assert_eq!(numeric_value(source, token.span), Some(31.0));
/// ```
pub fn numeric_value(source: &str, span: Span) -> Option<f64> {
    let text = span.slice(source)?;
    let after_prefix = text.get(2..);
    match text.as_bytes() {
        // `0b` / `0o` / `0x`, in either case (§12.9.3). One binary digit is one bit, an octal
        // digit three, a hex digit four — which is the whole reason these can be exact.
        [b'0', b'b' | b'B', ..] => power_of_two_value(after_prefix?, 1),
        [b'0', b'o' | b'O', ..] => power_of_two_value(after_prefix?, 3),
        [b'0', b'x' | b'X', ..] => power_of_two_value(after_prefix?, 4),
        // Annex B.1.1: a leading `0` followed only by octal digits is a
        // `LegacyOctalIntegerLiteral` and is read in base 8 — `0123` is 83, not 123. One `8` or
        // `9` anywhere in the run makes it a `NonOctalDecimalIntegerLiteral` instead, read in
        // base 10, so `0123` and `0128` differ in radix by their last digit. The distinction is
        // recoverable from the text alone, which is why this function needs no help from the
        // token to make it.
        [b'0', tail @ ..]
            if !tail.is_empty()
                && tail.iter().all(|b| b.is_ascii_digit())
                && !tail.iter().any(|b| matches!(b, b'8' | b'9')) =>
        {
            power_of_two_value(text.get(1..)?, 3)
        }
        _ => decimal_value(text),
    }
}

/// The radix a `BigIntLiteral` asked for and the digits it is made of, or `None` if `span` does
/// not cover one.
///
/// This is the counterpart of [`numeric_value`] for the literals that one refuses: a BigInt
/// denotes a mathematical integer of no fixed width, and there is no such type here until M7.
/// What can be done without arithmetic is done — the `0b`/`0o`/`0x` prefix becomes a radix, and
/// the `NumericLiteralSeparator`s and the `n` come out, §12.9.3's MV rules giving separators no
/// value — so that `StringToBigInt` (§7.1.14) has nothing left to undo and the source never has
/// to be lexed twice.
///
/// The digits are returned as written otherwise: neither zero-stripped nor case-folded, because
/// `0x00Fn` and `0xfn` are the same number and only one of them is what the program said. That
/// is the same position [`numeric_value`] takes by keeping the span.
///
/// Ten is returned for a literal with no prefix. Eight never is: Annex B's
/// `LegacyOctalIntegerLiteral` has no `BigIntLiteralSuffix` alternative, so `0123n` has no
/// derivation, the scanner refuses it, and no span reaching here can hold one.
///
/// ```
/// use praxis::lexer::{Goal, Lexer, TokenKind, bigint_digits};
///
/// let source = "0x1_Fn";
/// let token = Lexer::new(source).next_token(Goal::Div).expect("this lexes");
/// assert_eq!(token.kind, TokenKind::BigInt);
/// assert_eq!(bigint_digits(source, token.span), Some((16, "1F".to_string())));
/// ```
pub fn bigint_digits(source: &str, span: Span) -> Option<(u32, String)> {
    // The suffix is what makes it a BigInt, so its absence means the span holds something else —
    // a `DecimalLiteral`, or nothing at all.
    let text = span.slice(source)?.strip_suffix('n')?;
    let (radix, digits) = match text.as_bytes() {
        [b'0', b'b' | b'B', ..] => (2, text.get(2..)?),
        [b'0', b'o' | b'O', ..] => (8, text.get(2..)?),
        [b'0', b'x' | b'X', ..] => (16, text.get(2..)?),
        _ => (10, text),
    };
    let digits: String = digits.chars().filter(|&ch| ch != '_').collect();
    // An empty run means the span was `0bn` or just `n` — neither is a literal, and a
    // `BigIntLiteral` always has at least one digit.
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    Some((radix, digits))
}

/// `𝔽(MV)` for a digit string in a radix that is a power of two, with `bits` bits per digit.
///
/// The digits of a base-2, base-8 or base-16 literal are exactly the bits of its value, so no
/// decimal conversion — and no second rounding — is involved. `top` accumulates the most
/// significant bits until one more digit would overflow it, at which point it holds at least 60
/// of them: more than the 53 an `f64` keeps plus the round bit. Every later digit can therefore
/// only influence the result through whether it is zero, which is what `sticky` records.
///
/// Returns `None` if any character is not a digit of the radix; `_` is skipped, since §12.9.3's
/// MV rules define separators to contribute nothing.
fn power_of_two_value(digits: &str, bits: u32) -> Option<f64> {
    let radix = 1u32 << bits;
    let mut top: u64 = 0;
    let mut dropped: u32 = 0;
    let mut sticky = false;
    let mut any = false;

    for ch in digits.chars() {
        if ch == '_' {
            continue;
        }
        let digit = ch.to_digit(radix)?;
        any = true;
        // "While it still fits" rather than a hand-written bound, because any bound past 54 bits
        // would do equally well and a comparison nothing can distinguish is a comparison that
        // should not be written. Checked arithmetic says the same thing with nothing left to
        // guard — and keeps the whole accumulator, which a threshold would not.
        match top
            .checked_mul(u64::from(radix))
            .and_then(|shifted| shifted.checked_add(u64::from(digit)))
        {
            Some(next) => top = next,
            None => {
                // `top` is full, so it already holds more significant bits than an `f64` keeps;
                // everything from here down can only matter through whether it is zero.
                dropped = dropped.saturating_add(bits);
                sticky |= digit != 0;
            }
        }
    }
    if !any {
        return None;
    }
    // Fold the discarded bits into the lowest bit of `top`. It sits at least ten places below
    // the point `f64` rounds at, so this cannot disturb a value that was not an exact tie — and
    // for one that was, it is what breaks the tie away from zero, as it must.
    if sticky {
        top |= 1;
    }
    // `u64` → `f64` rounds to nearest, ties to even: precisely 𝔽. That single rounding is the
    // only one, since scaling by a power of two afterwards is exact. `dropped` counts bits of a
    // literal whose length the source chooses, so it is clamped to an exponent `powi` can take —
    // 2^1024 is already past the largest finite `f64` and `top` is at least one, so clamping can
    // never turn a finite answer into a different finite answer, only into the infinity it
    // already was.
    Some((top as f64) * 2f64.powi(dropped.min(1024) as i32))
}

/// `𝔽(MV)` for a `DecimalLiteral`, separators removed.
///
/// `f64::from_str` is correctly rounded — it is Eisel-Lemire with an exact fallback — and its
/// grammar accepts every shape §12.9.3 produces once the separators are gone, including the
/// trailing-dot `1.` and the leading-dot `.5` forms. It also accepts `inf` and `nan`, which is
/// harmless: nothing reaches this function that the scanner did not already recognise as digits.
fn decimal_value(text: &str) -> Option<f64> {
    // Separators come out unconditionally rather than behind a "does it contain one?" test:
    // §12.9.3's MV rules give them no value, the conversion that follows dominates the cost
    // either way, and a branch whose two arms produce the same answer is one no test can pin.
    let digits: String = text.chars().filter(|&ch| ch != '_').collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::test_support::*;
    #[test]
    fn a_literal_that_needs_rounding_is_rounded_correctly_and_not_merely_closely() {
        // §12.9.3.3: a DecimalLiteral is RoundMVResult(MV), which is 𝔽(MV) at these lengths.
        // Each of these separates a correctly-rounded conversion from a plausible one.
        assert_eq!(value("9007199254740993"), 9007199254740992.0); // 2^53+1, ties to even
        assert_eq!(value("0.1"), 0.1);
        assert_eq!(
            value("2.2250738585072011e-308").to_bits(),
            0x000f_ffff_ffff_ffff
        );
        assert_eq!(value("4.9e-324"), f64::from_bits(1)); // the smallest subnormal
        assert_eq!(value("1.7976931348623157e308"), f64::MAX);
        assert!(value("1.7976931348623159e308").is_infinite());
        assert!(value("1e309").is_infinite());
        assert_eq!(value("1e-400"), 0.0);
        // More than the 20 significant digits RoundMVResult may truncate at: correctly rounding
        // the whole string is one of the two options it permits, and is what other engines do.
        assert_eq!(
            value("123456789012345678901234567890"),
            1.2345678901234568e29
        );

        // §12.9.3.3 allows no approximation at all for the non-decimal forms — 𝔽(MV), exactly.
        assert_eq!(value("0x20000000000000"), 9007199254740992.0); // 2^53
        assert_eq!(value("0x20000000000001"), 9007199254740992.0); // 2^53+1 → down, to even
        assert_eq!(value("0x20000000000002"), 9007199254740994.0); // exact
        assert_eq!(value("0x20000000000003"), 9007199254740996.0); // 2^53+3 → up, to even
        // Past 64 bits the accumulator stops shifting and the rest becomes a sticky bit. 2^65+1
        // must round back down to 2^65 rather than drifting.
        assert_eq!(value("0x20000000000000000"), 36893488147419103232.0);
        assert_eq!(value("0x20000000000000001"), 36893488147419103232.0);

        // The sticky bit, isolated. Both of these fill the accumulator and then drop a digit,
        // and they differ only in whether that digit is zero — which is exactly the difference
        // between an exact tie (round to even, downwards) and a hair above one (round up).
        // Ignore the dropped digit and the second answer is wrong by 2^12.
        assert_eq!(value("0x10000000000000800"), 2f64.powi(64));
        assert_eq!(
            value("0x10000000000000801"),
            2f64.powi(64) + 2f64.powi(12),
            "a single dropped 1, eleven digits down, still moves the result"
        );
        assert_eq!(value("0b1"), 1.0);
        assert_eq!(value("0o1"), 1.0);
        // A literal too large for `f64` is infinity, not an error and not a wrapped value.
        let huge = format!("0x{}", "f".repeat(300));
        assert!(value(&huge).is_infinite());
        // …but "long" is not "too large": `0x1` followed by 200 zeros is 2^800, which is a
        // perfectly ordinary finite `f64` and must come back exact rather than saturating.
        assert_eq!(value(&format!("0x1{}", "0".repeat(200))), 2f64.powi(800));
        assert_eq!(value(&format!("0b1{}", "0".repeat(800))), 2f64.powi(800));
        // 2^1200 has no `f64`, so that one really is infinity.
        assert!(value(&format!("0x1{}", "0".repeat(300))).is_infinite());
        let padded = format!("0x{}1", "0".repeat(500));
        assert_eq!(value(&padded), 1.0, "leading zeros cost nothing");
        assert_eq!(value(&format!("0x{}", "0".repeat(500))), 0.0);
    }
    #[test]
    fn every_power_of_two_literal_agrees_with_an_exact_conversion() {
        // An independent oracle beats hand-computed expectations: a `u128` holds any literal of
        // up to 128 bits exactly, and `u128 as f64` rounds to nearest with ties to even — which
        // is 𝔽, by definition. So this compares the accumulator, the sticky bit and the scaling
        // against arithmetic that does none of them, over inputs long enough that most of the
        // sweep goes down the path where bits are dropped.
        //
        // The sequence is a fixed-seed xorshift rather than anything random: a test that probes
        // different inputs on different runs is a test that fails for someone else.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut rand = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut over_64_bits = 0;
        for _ in 0..10_000 {
            let bits = [1u32, 3, 4][(rand() % 3) as usize];
            let radix = 1u32 << bits;
            // 124 bits at most, so the oracle itself never overflows.
            let len = 1 + (rand() % u64::from(124 / bits)) as usize;
            let mut digits = String::with_capacity(len);
            for _ in 0..len {
                digits.push(DIGITS[(rand() % u64::from(radix)) as usize] as char);
            }
            let exact = match u128::from_str_radix(&digits, radix) {
                Ok(exact) => exact,
                Err(err) => panic!("oracle cannot read {digits:?} in radix {radix}: {err}"), // without the oracle there is nothing to compare against
            };
            if exact >= 1 << 64 {
                over_64_bits += 1;
            }
            assert_eq!(
                power_of_two_value(&digits, bits),
                Some(exact as f64),
                "radix {radix}, digits {digits:?}"
            );
        }
        assert!(
            over_64_bits > 1000,
            "only {over_64_bits} of the sweep exceeded 64 bits — the sticky path is barely tested"
        );
    }
    #[test]
    fn separators_change_the_spelling_of_a_value_and_never_the_value() {
        // §12.9.3's MV rules define a separator to contribute nothing, so these must be equal
        // rather than merely close.
        for (with, without) in [
            ("1_000", "1000"),
            ("1_0.2_5", "10.25"),
            ("1e1_0", "1e10"),
            ("0x1_2_3", "0x123"),
            ("0b1_0_1", "0b101"),
            ("0o1_7", "0o17"),
        ] {
            assert_eq!(value(with), value(without), "{with:?} vs {without:?}");
        }
    }
    #[test]
    fn numeric_value_answers_rather_than_panicking_on_a_span_it_was_not_given() {
        // Spans the lexer never produced: past the end, off a character boundary, empty, and
        // over text that is not a literal at all.
        assert_eq!(numeric_value("123", Span::new(0, 99)), None);
        assert_eq!(numeric_value("é1", Span::new(0, 1)), None);
        assert_eq!(numeric_value("123", Span::empty_at(1)), None);
        assert_eq!(numeric_value("abc", Span::new(0, 3)), None);
        assert_eq!(numeric_value("0xzz", Span::new(0, 4)), None);
        assert_eq!(numeric_value("", Span::empty_at(0)), None);
        // A radix prefix with no digits behind it has no value — and specifically not zero,
        // which is what a conversion that forgot to notice would return.
        assert_eq!(numeric_value("0x", Span::new(0, 2)), None);
        assert_eq!(numeric_value("0b", Span::new(0, 2)), None);
        assert_eq!(numeric_value("0o_", Span::new(0, 3)), None);
        // …whereas a digit that happens to be zero does have one.
        assert_eq!(numeric_value("0x0", Span::new(0, 3)), Some(0.0));
        // A valid span still works when it does not start at zero.
        assert_eq!(numeric_value("x = 0x10", Span::new(4, 8)), Some(16.0));
    }
    #[test]
    fn a_bigint_keeps_its_digits_and_the_radix_its_prefix_asked_for() {
        // §12.9.3: every `BigIntLiteral` alternative, and the radix each one denotes.
        assert_eq!(digits("1n"), (10, "1".to_string()));
        assert_eq!(digits("0n"), (10, "0".to_string()));
        assert_eq!(digits("0b1101n"), (2, "1101".to_string()));
        assert_eq!(digits("0o17n"), (8, "17".to_string()));
        assert_eq!(digits("0x1Fn"), (16, "1F".to_string()));
        // The prefix may be written either way and means the same thing.
        assert_eq!(digits("0B1n"), (2, "1".to_string()));
        assert_eq!(digits("0O1n"), (8, "1".to_string()));
        assert_eq!(digits("0X1n"), (16, "1".to_string()));
        // Separators contribute nothing to the MV, so they contribute nothing here either.
        assert_eq!(digits("1_2_3n"), (10, "123".to_string()));
        assert_eq!(digits("0x1_Fn"), (16, "1F".to_string()));
        // Neither zero-stripped nor case-folded: `0x00Fn` and `0xfn` are the same number, and
        // which one the program wrote is not this function's to discard.
        assert_eq!(digits("0x00Fn"), (16, "00F".to_string()));
        assert_eq!(digits("0xfn"), (16, "f".to_string()));
        // Longer than any `f64` — which is the entire reason the digits are kept as digits.
        assert_eq!(
            digits("123456789012345678901234567890n"),
            (10, "123456789012345678901234567890".to_string())
        );
    }
    #[test]
    fn the_two_numeric_readers_each_refuse_what_the_other_takes() {
        // The `n` is what tells them apart, and each answers `None` rather than guessing at a
        // value of the type it does not have.
        assert_eq!(numeric_value("1n", Span::new(0, 2)), None);
        assert_eq!(bigint_digits("1", Span::new(0, 1)), None);
        assert_eq!(bigint_digits("0x10", Span::new(0, 4)), None);
    }
    #[test]
    fn bigint_digits_answers_rather_than_panicking_on_a_span_it_was_not_given() {
        // Spans the lexer never produced: past the end, off a character boundary, empty, and
        // over text that is not a literal at all.
        assert_eq!(bigint_digits("123n", Span::new(0, 99)), None);
        assert_eq!(bigint_digits("én", Span::new(0, 1)), None);
        assert_eq!(bigint_digits("123n", Span::empty_at(1)), None);
        assert_eq!(bigint_digits("abcn", Span::new(0, 4)), None);
        assert_eq!(bigint_digits("", Span::empty_at(0)), None);
        // A radix prefix with no digits behind it has none — and specifically not `"0"`, which
        // is what a reader that forgot to look would return.
        assert_eq!(bigint_digits("0xn", Span::new(0, 3)), None);
        assert_eq!(bigint_digits("0b_n", Span::new(0, 4)), None);
        assert_eq!(bigint_digits("n", Span::new(0, 1)), None);
        // A digit outside the radix its prefix asked for. The scanner never produces one, so
        // this is about answering `None` rather than reading it in the wrong base.
        assert_eq!(bigint_digits("0b2n", Span::new(0, 4)), None);
        assert_eq!(bigint_digits("0o8n", Span::new(0, 4)), None);
        assert_eq!(bigint_digits("0xgn", Span::new(0, 4)), None);
        // A valid span still works when it does not start at zero.
        assert_eq!(
            bigint_digits("x = 0x10n", Span::new(4, 9)),
            Some((16, "10".to_string()))
        );
    }
}
