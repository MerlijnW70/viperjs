//! `UnicodeEscapeSequence`, and turning code points into the code units a String value is made of.
//!
//! This lives apart from both of its callers because the specification puts it that way round:
//! `UnicodeEscapeSequence` is defined in §12.9.4 among the string literals, and §12.7 borrows it
//! for identifiers — "the definitions of the nonterminal UnicodeEscapeSequence is given in
//! 12.9.4", as §12.7 says. One reader, two grammars, and no chance of the two drifting apart on
//! a detail like how many hex digits `\u` takes.

use super::{LexError, LexErrorKind, Lexer};
use crate::span::Span;

/// The code units one source construct contributes to a String value.
///
/// Three cases rather than a `Vec`, because there are only three: a `LineContinuation` gives
/// nothing, an astral code point gives a surrogate pair, and everything else gives one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodeUnits {
    /// Nothing at all — what a `LineContinuation` contributes (§12.9.4 SV).
    Nothing,
    /// Exactly one code unit. `\uD800` is a lone surrogate and stays one; see DR-0004.
    One(u16),
    /// A surrogate pair, high then low.
    Pair(u16, u16),
}

impl CodeUnits {
    /// Append these units to a String value under construction.
    pub(super) fn push_onto(self, out: &mut Vec<u16>) {
        match self {
            Self::Nothing => {}
            Self::One(unit) => out.push(unit),
            Self::Pair(high, low) => {
                out.push(high);
                out.push(low);
            }
        }
    }
}

/// `UTF16EncodeCodePoint` (§11.1.1): the code units a code point contributes to a String value.
///
/// Spelled out rather than delegated to [`char::encode_utf16`], because the input is a `u32` that
/// may not be a `char` at all: `\u{D800}` names a surrogate, which §12.9.4 permits and which this
/// operation passes through unchanged, being below 0x10000. A `char`-typed helper cannot express
/// that, and converting first would refuse a literal the grammar accepts.
///
/// A value above U+10FFFF cannot reach here — [`Lexer::read_unicode_escape`] rejects it as
/// `NotCodePoint` first — but if one did, it would be encoded as the pair its low bits describe
/// rather than panicking, which is the failure a diagnostic can survive.
pub(super) fn utf16_encode(code_point: u32) -> CodeUnits {
    if code_point <= 0xffff {
        return CodeUnits::One(code_point as u16);
    }
    // §11.1.1: subtract 0x10000, then split the remaining 20 bits into two 10-bit halves.
    let rest = code_point - 0x10000;
    CodeUnits::Pair(
        (0xd800 + (rest >> 10)) as u16,
        (0xdc00 + (rest & 0x3ff)) as u16,
    )
}

/// The value of one `HexDigit` (§12.9.3), or `None` if `ch` is not one.
///
/// `char::to_digit` is exactly right here and rarely is: it accepts only `0-9`, `a-z` and `A-Z`,
/// so it agrees with `HexDigit` on the ASCII range and — importantly — rejects the Arabic-Indic
/// and fullwidth digits that an `is_numeric`-based check would wave through.
pub(super) fn hex_value(ch: char) -> Option<u32> {
    ch.to_digit(16)
}

impl Lexer<'_> {
    /// Consume `\ UnicodeEscapeSequence` (§12.9.4) and return the code point it denotes.
    ///
    /// Two forms: `\u` followed by exactly four hex digits, or `\u{` HexDigits `}` where the
    /// value must not exceed U+10FFFF (the spec's `CodePoint`, against `NotCodePoint`). The
    /// braced form takes `HexDigits[~Sep]` — **no numeric separators**, and any number of
    /// digits, so `\u{00000000000061}` is a perfectly ordinary `a`.
    ///
    /// The returned value is deliberately a `u32` and not a `char`: `\uD800` and `\u{10FFFF}`
    /// are both well-formed escapes whose acceptability depends on where they appear. In an
    /// identifier the first is a Syntax Error (§12.7.1.1) and in a string it is a lone surrogate
    /// (DR-0004) — so the caller decides, and this reader does not pre-judge.
    pub(super) fn read_unicode_escape(&mut self) -> Result<u32, LexError> {
        let start = self.cursor.offset();
        // Every ill-formed exit reports the same span: from the backslash to wherever the
        // sequence stopped making sense.
        macro_rules! malformed {
            () => {
                LexError {
                    kind: LexErrorKind::InvalidUnicodeEscape,
                    span: Span::new(start, self.cursor.offset()),
                }
            };
        }

        self.cursor.advance_ascii(1); // the `\`
        if self.cursor.peek() != Some('u') {
            return Err(malformed!());
        }
        self.cursor.advance_ascii(1);

        if self.cursor.peek() == Some('{') {
            self.cursor.advance_ascii(1);
            let mut value: u32 = 0;
            let mut digits = 0usize;
            while let Some(digit) = self.cursor.peek().and_then(hex_value) {
                let _ = self.cursor.bump();
                digits += 1;
                // Saturating, not wrapping: the digit count is chosen by whoever wrote the
                // source, so `\u{FFFFFFFFFFFFFFFF}` is an input, and an input may not overflow
                // (DR-0002). Saturation lands far above U+10FFFF, which is the answer anyway.
                value = value.saturating_mul(16).saturating_add(digit);
            }
            if digits == 0 || self.cursor.peek() != Some('}') {
                return Err(malformed!());
            }
            self.cursor.advance_ascii(1);
            if value > 0x10ffff {
                return Err(LexError {
                    kind: LexErrorKind::CodePointOutOfRange,
                    span: Span::new(start, self.cursor.offset()),
                });
            }
            return Ok(value);
        }

        // `Hex4Digits :: HexDigit HexDigit HexDigit HexDigit` — exactly four. A fifth digit is
        // simply the next character of whatever contains the escape, which is what makes
        // `a` name `a0`.
        let mut value: u32 = 0;
        for _ in 0..4 {
            let Some(digit) = self.cursor.peek().and_then(hex_value) else {
                return Err(malformed!());
            };
            let _ = self.cursor.bump();
            // Bounded by construction: four hex digits cannot exceed 0xFFFF.
            value = value * 16 + digit;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The units `text` denotes, appended in order.
    fn units(value: CodeUnits) -> Vec<u16> {
        let mut out = vec![0xffff]; // a sentinel, so "appends" is distinguishable from "replaces"
        value.push_onto(&mut out);
        out
    }

    #[test]
    fn utf16_encoding_matches_the_specs_own_operation() {
        // §11.1.1. The boundary is 0x10000: below it a code point is one unit, at and above it
        // exactly two. Both sides of that boundary, and both ends of the astral range.
        assert_eq!(utf16_encode(0x0000), CodeUnits::One(0x0000));
        assert_eq!(utf16_encode(0x0061), CodeUnits::One(0x0061));
        assert_eq!(utf16_encode(0xffff), CodeUnits::One(0xffff));
        assert_eq!(utf16_encode(0x10000), CodeUnits::Pair(0xd800, 0xdc00));
        assert_eq!(utf16_encode(0x10ffff), CodeUnits::Pair(0xdbff, 0xdfff));
        // U+1F680 ROCKET, the pair every UTF-16 bug is first noticed with.
        assert_eq!(utf16_encode(0x1f680), CodeUnits::Pair(0xd83d, 0xde80));
        // A surrogate passes through unchanged rather than being re-encoded or refused — the
        // whole reason this takes a `u32` (DR-0004).
        assert_eq!(utf16_encode(0xd800), CodeUnits::One(0xd800));
        assert_eq!(utf16_encode(0xdfff), CodeUnits::One(0xdfff));
        // Cross-checked against `char::encode_utf16` wherever a `char` exists to check against,
        // which is everywhere except the surrogates.
        for code_point in [0x61u32, 0xe9, 0x2028, 0xffff, 0x10000, 0x1f680, 0x10ffff] {
            let Some(ch) = char::from_u32(code_point) else {
                continue;
            };
            let mut buffer = [0u16; 2];
            let expected = ch.encode_utf16(&mut buffer).to_vec();
            let mut actual = Vec::new();
            utf16_encode(code_point).push_onto(&mut actual);
            assert_eq!(actual, expected, "U+{code_point:04X}");
        }
    }

    #[test]
    fn pushing_code_units_appends_in_order_and_appends_nothing_for_nothing() {
        assert_eq!(units(CodeUnits::Nothing), vec![0xffff]);
        assert_eq!(units(CodeUnits::One(0x41)), vec![0xffff, 0x41]);
        // High then low, in that order: a pair written backwards is a different string.
        assert_eq!(
            units(CodeUnits::Pair(0xd83d, 0xde80)),
            vec![0xffff, 0xd83d, 0xde80]
        );
    }

    #[test]
    fn hex_digits_are_the_ascii_ones_and_nothing_that_merely_looks_like_them() {
        assert_eq!(hex_value('0'), Some(0));
        assert_eq!(hex_value('9'), Some(9));
        assert_eq!(hex_value('a'), Some(10));
        assert_eq!(hex_value('f'), Some(15));
        assert_eq!(hex_value('A'), Some(10));
        assert_eq!(hex_value('F'), Some(15));
        // One past each end of each run.
        assert_eq!(hex_value('g'), None);
        assert_eq!(hex_value('G'), None);
        assert_eq!(hex_value('/'), None);
        assert_eq!(hex_value(':'), None);
        // Digits from other scripts are not `HexDigit`s, however numeric they look.
        assert_eq!(hex_value('\u{0665}'), None); // ARABIC-INDIC DIGIT FIVE
        assert_eq!(hex_value('\u{ff15}'), None); // FULLWIDTH DIGIT FIVE
    }
}
