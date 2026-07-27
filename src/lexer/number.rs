//! Numeric literals (ECMA-262 §12.9.3): the four radixes, separators, `BigInt`, and Annex B.
//!
//! Two things make this longer than "read some digits".
//!
//! **The grammar is not greedy in the way it looks.** `1e` is not a malformed exponent — it is
//! the literal `1` followed by the name `e`, because `ExponentPart` requires at least one digit
//! and simply does not match. The same reading makes `3in` an error rather than `3` and `in`,
//! via the one rule §12.9.3 states in prose: *the SourceCharacter immediately following a
//! NumericLiteral must not be an IdentifierStart or DecimalDigit.* The scanner therefore looks
//! before it leaps, and never has to give a character back.
//!
//! **The value has to be right, not close.** §12.9.3.3 says a `DecimalLiteral` denotes
//! `RoundMVResult(MV)` and every other form denotes `𝔽(MV)` exactly — no approximation is
//! permitted for `0x…`, and only a 21st-significant-digit's worth for decimals. See
//! [`numeric_value`] for how each is computed and why each is correctly rounded.

use super::{LexError, LexErrorKind, Lexer, TokenKind};
use crate::span::Span;
use crate::unicode_id::is_id_start;

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
///   rounds once. See [`power_of_two_value`].
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
/// use praxis::lexer::{numeric_value, Lexer, TokenKind};
///
/// let source = "0x1_F";
/// let token = Lexer::new(source).next_token().expect("this lexes");
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

impl<'a> Lexer<'a> {
    /// Scan a `NumericLiteral`, and enforce the rule about what may follow it.
    ///
    /// Called only when the next character is a decimal digit, or a `.` with a decimal digit
    /// behind it — the two ways §12.9.3 lets a literal begin.
    pub(super) fn scan_number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.cursor.offset();
        let kind = self.scan_numeric_literal(start)?;

        // §12.9.3: "The SourceCharacter immediately following a NumericLiteral must not be an
        // IdentifierStart or DecimalDigit", with the spec's own example — `3in` is an error and
        // not the two input elements `3` and `in`. The digit half is reachable too: `0b12` is
        // the literal `0b1` followed by `2`.
        //
        // A `\` is included because `IdentifierStart :: \ UnicodeEscapeSequence` — the escape is
        // how an identifier begins, even though the backslash alone is not an IdentifierStart.
        // Every engine rejects `3a`; test262 confirms or corrects this reading at M5.
        if let Some(ch) = self.cursor.peek()
            && (ch == '\\' || ch.is_ascii_digit() || is_id_start(ch as u32))
        {
            let at = self.cursor.offset();
            let _ = self.cursor.bump();
            return Err(LexError {
                kind: LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
                // The caret belongs under the offending character, not under the perfectly good
                // literal in front of it.
                span: Span::new(at, self.cursor.offset()),
            });
        }
        Ok(kind)
    }

    /// The literal itself, without the trailing-character rule.
    fn scan_numeric_literal(&mut self, start: u32) -> Result<TokenKind, LexError> {
        // `DecimalLiteral :: . DecimalDigits ExponentPart_opt`. The caller has already seen the
        // digit that distinguishes this from the `.` punctuator and from `...`.
        if self.cursor.peek() == Some('.') {
            // The dot, and the digit the dispatch already proved is behind it. Consuming that
            // digit here rather than leaving it to the run means the run is told something true
            // about its left neighbour, which is what makes `.5_5` legal and `._5` not.
            self.cursor.advance_ascii(2);
            self.scan_digits(10, true)?;
            self.scan_exponent()?;
            return Ok(TokenKind::Number { legacy: false });
        }

        if self.cursor.peek() == Some('0') {
            let _ = self.cursor.bump();
            if let Some(radix) = radix_prefix(self.cursor.peek()) {
                let _ = self.cursor.bump();
                if self.scan_digits(radix, false)? == 0 {
                    // Strictly, the grammar reaches the same verdict by another road: `0x` is
                    // not a HexIntegerLiteral, so the literal is `0` and the `x` that follows is
                    // an IdentifierStart. Both are a Syntax Error and nothing a script can
                    // observe distinguishes them, so this reports the one a human can act on.
                    return Err(LexError {
                        kind: LexErrorKind::MissingDigitsAfterRadixPrefix,
                        span: Span::new(start, self.cursor.offset()),
                    });
                }
                return Ok(self.finish_integer());
            }
            // Annex B.1.1, reachable only because a `0` was followed by another digit.
            if self.cursor.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                return self.scan_legacy_number();
            }
            // `DecimalIntegerLiteral :: 0` is complete on its own — deliberately not falling
            // into the digit loop below, so that `0_1` is diagnosed as the literal `0` followed
            // by the name `_1`, which is what the grammar says it is.
            if self.cursor.peek() == Some('n') {
                let _ = self.cursor.bump();
                return Ok(TokenKind::BigInt);
            }
            self.scan_fraction()?;
            self.scan_exponent()?;
            return Ok(TokenKind::Number { legacy: false });
        }

        // `DecimalIntegerLiteral :: NonZeroDigit NumericLiteralSeparator_opt DecimalDigits`. The
        // separator may sit directly after that first digit, which is why the run is told a
        // digit precedes it.
        let _ = self.cursor.bump();
        self.scan_digits(10, true)?;
        if self.cursor.peek() == Some('n') {
            let _ = self.cursor.bump();
            return Ok(TokenKind::BigInt);
        }
        self.scan_fraction()?;
        self.scan_exponent()?;
        Ok(TokenKind::Number { legacy: false })
    }

    /// `NonDecimalIntegerLiteral`, optionally carrying a `BigIntLiteralSuffix`.
    fn finish_integer(&mut self) -> TokenKind {
        if self.cursor.peek() == Some('n') {
            let _ = self.cursor.bump();
            return TokenKind::BigInt;
        }
        TokenKind::Number { legacy: false }
    }

    /// Annex B.1.1's two legacy forms, entered with the leading `0` consumed.
    ///
    /// Which one this is depends on a digit that may be anywhere in the run: all-octal makes it
    /// a `LegacyOctalIntegerLiteral`, and a single `8` or `9` makes it a
    /// `NonOctalDecimalIntegerLiteral` instead. That is not a detail — the second is a
    /// `DecimalIntegerLiteral` and so may carry a fraction and an exponent, while the first is a
    /// complete `NumericLiteral` that may not. It is why `018e2` is 1800 and `017e2` is a Syntax
    /// Error, the `e` there being an identifier starting immediately after a finished literal.
    ///
    /// Neither production takes the `[Sep]` parameter, so neither admits separators; `01_2` ends
    /// at `01` and trips the trailing-character rule on the `_`.
    fn scan_legacy_number(&mut self) -> Result<TokenKind, LexError> {
        let mut non_octal = false;
        while let Some(ch) = self.cursor.peek() {
            if !ch.is_ascii_digit() {
                break;
            }
            non_octal |= matches!(ch, '8' | '9');
            let _ = self.cursor.bump();
        }
        if non_octal {
            self.scan_fraction()?;
            self.scan_exponent()?;
        }
        Ok(TokenKind::Number { legacy: true })
    }

    /// A run of digits in `radix`, with `NumericLiteralSeparator` handled; returns how many
    /// digits were consumed.
    ///
    /// `preceded_by_digit` says whether the caller already consumed a digit immediately before
    /// the cursor, because that is exactly what decides whether a leading `_` is legal:
    /// §12.9.3's `[+Sep]` productions only ever place a separator *between* two digits. Checking
    /// both neighbours at the separator itself catches all three ways to get it wrong — leading
    /// (`0x_1`), doubled (`1__0`) and trailing (`1_`) — in one place, and means the flag needs
    /// updating only where a digit is consumed: after a separator, the next character has
    /// already been proved to be one.
    ///
    /// There is no "are separators allowed?" parameter, because every production reaching here
    /// takes `[+Sep]`. Annex B's two legacy forms, which do not, have a loop of their own.
    fn scan_digits(&mut self, radix: u32, preceded_by_digit: bool) -> Result<u32, LexError> {
        let mut count = 0;
        let mut prev_was_digit = preceded_by_digit;
        loop {
            match self.cursor.peek() {
                Some(ch) if ch.is_digit(radix) => {
                    let _ = self.cursor.bump();
                    count += 1;
                    prev_was_digit = true;
                }
                Some('_') => {
                    let at = self.cursor.offset();
                    let _ = self.cursor.bump();
                    let followed_by_digit = self.cursor.peek().is_some_and(|ch| ch.is_digit(radix));
                    if !prev_was_digit || !followed_by_digit {
                        return Err(LexError {
                            kind: LexErrorKind::MisplacedNumericSeparator,
                            span: Span::new(at, self.cursor.offset()),
                        });
                    }
                }
                _ => return Ok(count),
            }
        }
    }

    /// `. DecimalDigits[+Sep]opt` after a `DecimalIntegerLiteral`, if one is there.
    ///
    /// The digits are optional: `1.` is a complete `DecimalLiteral`, which is why `1..toString()`
    /// is the idiom it is.
    fn scan_fraction(&mut self) -> Result<(), LexError> {
        if self.cursor.peek() != Some('.') {
            return Ok(());
        }
        let _ = self.cursor.bump();
        // `false`, and load-bearing: a separator may not lead the fraction, so `1._5` is an
        // error while `1.2_5` is not.
        self.scan_digits(10, false)?;
        Ok(())
    }

    /// `ExponentPart[+Sep]`, if one is really there.
    ///
    /// Looks ahead past an optional sign for a digit *before* consuming anything, because an `e`
    /// with no digits after it is not a malformed exponent — `ExponentPart` just fails to match,
    /// leaving `1e` as the literal `1` followed by the name `e`. Consuming the `e` first and
    /// complaining afterwards would report a different error than the grammar describes, and
    /// would need the cursor to go backwards.
    fn scan_exponent(&mut self) -> Result<(), LexError> {
        if !matches!(self.cursor.peek(), Some('e' | 'E')) {
            return Ok(());
        }
        let signed = matches!(self.cursor.peek_byte(1), Some(b'+' | b'-'));
        let first_digit_at = if signed { 2 } else { 1 };
        if !self
            .cursor
            .peek_byte(first_digit_at)
            .is_some_and(|b| b.is_ascii_digit())
        {
            return Ok(());
        }
        // The `e`, the sign if there was one, and the digit just proved to be behind them — so
        // that the run below is told something true rather than something hopeful.
        self.cursor.advance_ascii(first_digit_at + 1);
        self.scan_digits(10, true)?;
        Ok(())
    }
}

/// The radix `0b`, `0o` or `0x` introduces, in either case.
fn radix_prefix(ch: Option<char>) -> Option<u32> {
    match ch {
        Some('b' | 'B') => Some(2),
        Some('o' | 'O') => Some(8),
        Some('x' | 'X') => Some(16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::test_support::*;

    /// The value of the one literal in `source`.
    fn value(source: &str) -> f64 {
        let token = first(source);
        numeric_value(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should have a numeric value")) // a test about the value cannot proceed without one
    }

    /// The error `source` fails with, or a panic naming what it produced instead.
    fn error(source: &str) -> LexError {
        match Lexer::new(source).tokens() {
            Err(err) => err,
            Ok(tokens) => panic!("{source:?} should not lex, got {tokens:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn a_decimal_literal_takes_every_shape_the_grammar_allows() {
        // Each of the six `DecimalLiteral` alternatives, plus the pieces they are made of. A
        // scanner that requires digits after the dot loses `1.`; one that requires them before
        // it loses `.5`; one that treats the exponent as mandatory-if-`e`-present loses `1e`
        // (see the trailing-character test, where `1e` is an error for a quite different
        // reason).
        for (source, expected) in [
            ("0", 0.0),
            ("1", 1.0),
            ("42", 42.0),
            ("1.", 1.0),
            ("1.5", 1.5),
            (".5", 0.5),
            ("0.5", 0.5),
            ("1e3", 1000.0),
            ("1E3", 1000.0),
            ("1e+3", 1000.0),
            ("1e-3", 0.001),
            ("1.5e3", 1500.0),
            ("1.e3", 1000.0),
            (".5e1", 5.0),
            ("0e0", 0.0),
        ] {
            assert_eq!(value(source), expected, "value of {source:?}");
            assert_eq!(
                kinds(source),
                [NUMBER, TokenKind::Eof],
                "kinds of {source:?}"
            );
        }
        // `0` alone is a plain DecimalIntegerLiteral and must NOT be flagged legacy — only a
        // `0` with digits after it is Annex B.
        assert_eq!(kinds("0"), [NUMBER, TokenKind::Eof]);
        // A literal ends where it ends: the punctuator after it is a separate token.
        assert_eq!(
            kinds("1+2"),
            [NUMBER, TokenKind::Plus, NUMBER, TokenKind::Eof]
        );
        assert_eq!(first("1.5;").span, Span::new(0, 3));
        // `.` without a digit is still the punctuator, and `...` still wins over it.
        assert_eq!(kinds("."), [TokenKind::Dot, TokenKind::Eof]);
        assert_eq!(kinds("..."), [TokenKind::DotDotDot, TokenKind::Eof]);
        assert_eq!(
            kinds("1..5"),
            [NUMBER, NUMBER, TokenKind::Eof],
            "`1.` then `.5`, which is why `1..toString()` parses"
        );
    }

    #[test]
    fn a_non_decimal_literal_reads_in_its_own_radix_and_rejects_foreign_digits() {
        for (source, expected) in [
            ("0b0", 0.0),
            ("0b101", 5.0),
            ("0B101", 5.0),
            ("0o0", 0.0),
            ("0o17", 15.0),
            ("0O17", 15.0),
            ("0x0", 0.0),
            ("0x1f", 31.0),
            ("0X1F", 31.0),
            ("0xAbCdEf", 11259375.0),
            ("0xffffffff", 4294967295.0),
        ] {
            assert_eq!(value(source), expected, "value of {source:?}");
            assert_eq!(kinds(source), [NUMBER, TokenKind::Eof], "{source:?}");
        }
        // A prefix with nothing after it, and one whose next character is not a digit of the
        // radix it announced.
        for source in ["0x", "0X", "0b", "0o", "0xg", "0b2", "0o8"] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::MissingDigitsAfterRadixPrefix,
                "on {source:?}"
            );
        }
        // …but a foreign digit AFTER a valid one ends the literal instead, and is then caught by
        // the trailing rule. `0b12` is the literal `0b1` followed by `2`.
        assert_eq!(
            error("0b12").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit
        );
        assert_eq!(error("0o18").span, Span::new(3, 4));
        // The prefix error spans the prefix, so the caret sits under `0x` rather than nothing.
        assert_eq!(error("0x").span, Span::new(0, 2));
    }

    #[test]
    fn a_numeric_separator_must_sit_between_two_digits() {
        // §12.9.3 places `NumericLiteralSeparator` only ever between digits, in every radix and
        // in every part of a decimal literal.
        for (source, expected) in [
            ("1_0", 10.0),
            ("1_000_000", 1000000.0),
            ("1_0.2_5", 10.25),
            ("1.2_5", 1.25),
            (".2_5", 0.25),
            ("1e1_0", 1e10),
            ("1e-1_0", 1e-10),
            (".5_5", 0.55),
            ("0x1_2", 18.0),
            ("0b1_0", 2.0),
            ("0o1_7", 15.0),
            ("0x1_2_3", 291.0),
        ] {
            assert_eq!(value(source), expected, "value of {source:?}");
            assert_eq!(kinds(source), [NUMBER, TokenKind::Eof], "{source:?}");
        }
        // Leading, doubled and trailing — the three ways to misplace one — in the several
        // places each can occur.
        for source in [
            "1__0", "1_", "1_.5", "1._5", "1.5_", "1e1_", "0x_1", "0x1_", "0x1__2", "0b_1", "0o_7",
            ".5_",
        ] {
            let err = error(source);
            assert_eq!(
                err.kind,
                LexErrorKind::MisplacedNumericSeparator,
                "on {source:?}"
            );
            assert_eq!(
                err.span.len(),
                1,
                "the caret belongs on the `_` in {source:?}"
            );
        }
        // `0` is a complete DecimalIntegerLiteral, so `0_1` is not a misplaced separator at all
        // — it is the literal `0` followed by the identifier `_1`, exactly as the grammar says.
        assert_eq!(
            error("0_1").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit
        );
    }

    #[test]
    fn a_numeric_literal_may_not_be_followed_by_an_identifier_or_a_digit() {
        // §12.9.3's prose rule, with its own example: "3in is an error and not the two input
        // elements 3 and in". Without it a lexer happily produces `3` `in` and the parser then
        // accepts a relational expression nobody wrote.
        // `3_` and `0b1_2` are deliberately absent: those are misplaced separators, diagnosed
        // by the rule above rather than this one.
        for source in [
            "3in",
            "3abc",
            "1e",
            "1e+",
            "1E",
            "1e_1",
            "1e+_1",
            "3n_",
            "0x1g",
            "1.x",
            "3.toString",
            "1_0a",
            "3$",
            "017e2",
            "0x1n2",
        ] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
                "on {source:?}"
            );
        }
        // A backslash counts: `IdentifierStart :: \ UnicodeEscapeSequence`, so `3a` is `3`
        // followed by the start of a name.
        assert_eq!(
            error("3\\u0061").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit
        );
        // The caret sits under the offending character, not under the literal before it.
        assert_eq!(error("3in").span, Span::new(1, 2));
        assert_eq!(error("42abc").span, Span::new(2, 3));
        // What is allowed to follow: anything that starts no name and is no digit.
        for source in ["3+4", "3;", "3)", "3,4", "3 in x", "3.5;", "1..x", "3\n"] {
            assert!(Lexer::new(source).tokens().is_ok(), "{source:?} should lex");
        }
    }

    #[test]
    fn annex_b_legacy_octal_is_read_in_base_eight_and_flagged_for_the_parser() {
        // Annex B.1.1. §12.9.3.1 makes both forms a Syntax Error in strict code, and the lexer
        // cannot know whether the code is strict — so it records the fact and leaves the verdict
        // to the parser, exactly as it does for an escaped keyword.
        for (source, expected) in [
            ("00", 0.0),
            ("01", 1.0),
            ("0123", 83.0),  // base 8: 1×64 + 2×8 + 3
            ("0777", 511.0), // the largest three-digit one
            ("07", 7.0),
        ] {
            assert_eq!(value(source), expected, "value of {source:?}");
            assert_eq!(kinds(source), [LEGACY, TokenKind::Eof], "{source:?}");
        }
        // One `8` or `9` anywhere in the run switches the radix to ten — `0123` is 83 while
        // `0128` is 128, a difference decided by the final digit.
        for (source, expected) in [
            ("08", 8.0),
            ("09", 9.0),
            ("0128", 128.0),
            ("0189", 189.0),
            ("0789", 789.0),
        ] {
            assert_eq!(value(source), expected, "value of {source:?}");
            assert_eq!(kinds(source), [LEGACY, TokenKind::Eof], "{source:?}");
        }
        // A NonOctalDecimalIntegerLiteral IS a DecimalIntegerLiteral, so it takes a fraction and
        // an exponent. A LegacyOctalIntegerLiteral is a complete NumericLiteral and takes
        // neither, which is the whole reason these two differ:
        assert_eq!(value("018e2"), 1800.0);
        assert_eq!(value("08.5"), 8.5);
        assert_eq!(
            error("017e2").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
            "`017` is complete, so the `e` starts a name"
        );
        // Neither form admits separators, neither takes a BigInt suffix, and a plain `0` is not
        // legacy at all.
        assert_eq!(
            error("01_2").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit
        );
        assert_eq!(
            error("01n").kind,
            LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit
        );
        assert_eq!(kinds("0"), [NUMBER, TokenKind::Eof]);
        assert_eq!(kinds("0.5"), [NUMBER, TokenKind::Eof]);
        assert_eq!(kinds("0e1"), [NUMBER, TokenKind::Eof]);
        assert_eq!(kinds("0n"), [TokenKind::BigInt, TokenKind::Eof]);
    }

    #[test]
    fn a_bigint_suffix_makes_a_different_token_and_forbids_a_fraction() {
        // `BigIntLiteralSuffix :: n`. The value is a BigInt, a type this engine reaches at M7 —
        // so the lexer records the token form now, because getting it wrong today would make
        // `123n` lex as `123` and the name `n`, which is silently valid nonsense.
        for source in ["0n", "1n", "123n", "1_000n", "0x1fn", "0b101n", "0o17n"] {
            assert_eq!(
                kinds(source),
                [TokenKind::BigInt, TokenKind::Eof],
                "{source:?}"
            );
            assert_eq!(first(source).span, Span::new(0, source.len() as u32));
        }
        // `DecimalBigIntegerLiteral` has no fraction and no exponent alternative, so the `n`
        // there is simply a name starting after a finished literal.
        for source in ["1.5n", "1.n", "1e3n", ".5n"] {
            assert_eq!(
                error(source).kind,
                LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit,
                "on {source:?}"
            );
        }
        // The `f64` value of a BigInt is not a thing to guess at: M7 owns it.
        let token = first("123n");
        assert_eq!(numeric_value("123n", token.span), None);
    }

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
    fn no_numeric_literal_however_long_or_absurd_can_panic() {
        // DR-0002: the digit count is chosen by whoever wrote the source, so every accumulator
        // here takes attacker-chosen input. Overflow must saturate into infinity, never wrap and
        // never panic.
        let cases = [
            "9".repeat(5000),
            format!("0.{}", "9".repeat(5000)),
            format!("0x{}", "f".repeat(5000)),
            format!("0b{}", "1".repeat(5000)),
            format!("0o{}", "7".repeat(5000)),
            format!("1e{}", "9".repeat(400)),
            format!("1e-{}", "9".repeat(400)),
            "0".repeat(5000), // legacy octal zero, five thousand times
            format!("1{}", "_0".repeat(2000)),
            "1e999999999".to_string(),
            "1e-999999999".to_string(),
        ];
        for source in &cases {
            // The verdict does not matter; not unwinding does. Where it lexes, the value must
            // still be a number we can name.
            if let Ok(tokens) = Lexer::new(source).tokens() {
                let value = numeric_value(source, tokens[0].span);
                assert!(
                    value.is_some(),
                    "{} … lexed but has no value",
                    &source[..12]
                );
            }
        }
        assert!(value(&format!("1e{}", "9".repeat(400))).is_infinite());
        assert_eq!(value(&format!("1e-{}", "9".repeat(400))), 0.0);
        assert_eq!(value(&"0".repeat(5000)), 0.0);
        assert_eq!(value(&format!("1{}", "_0".repeat(2000))), f64::INFINITY);
    }
}
