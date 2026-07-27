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
//! permitted for `0x…`, and only a 21st-significant-digit's worth for decimals. That half of
//! the work is [`super::number_value`]'s; this file decides only how far each literal reaches.

use super::{LexError, LexErrorKind, Lexer, TokenKind};
use crate::span::Span;
use crate::unicode_id::is_id_start;

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
    use crate::lexer::numeric_value;
    use crate::lexer::test_support::*;
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
