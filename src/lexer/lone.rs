//! Source text that was a String first, and the unpaired surrogates in it.
//!
//! # Why the lexer needs to know
//!
//! Every source a host hands over is Rust text, and Rust text cannot hold an unpaired surrogate. A
//! source a *program* hands over can: §19.2.1.1's `eval` and §20.2.1.1's `Function` take a String,
//! and a String is any sequence of code units (DR-0004). `eval("'" + "\ud800" + "'")` is a string
//! literal whose value is that one unit, and `eval("/" + "\ud800" + "/").source` is that one unit
//! too — test262 asks both, for every code unit there is.
//!
//! The lexer reads `&str`, and teaching it to read code units instead would touch every production
//! for a character that can only ever stand in four places: a string literal, a template, a regular
//! expression literal and a comment. Everywhere else it is a Syntax Error, as it is for any
//! character that starts no token. So the text the lexer reads has each unpaired surrogate replaced
//! by U+FFFD — one character that starts no token, is no identifier part and no line terminator,
//! and takes the same three bytes — and [`UnitSource::lone`] records where each one was and which
//! unit it stood for. The places that turn a span of source back into code units put the unit back.
//!
//! A U+FFFD the program wrote itself is not in the table, so it stays a U+FFFD: the table is keyed
//! by position, never by character.

use super::escape::utf16_encode;
use crate::span::Span;

/// One unpaired surrogate of a source that was handed over as code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoneSurrogate {
    /// The byte offset, in [`UnitSource::text`], of the U+FFFD that stands in for it.
    pub offset: u32,
    /// The code unit itself, `0xD800..=0xDFFF`.
    pub unit: u16,
}

/// A source handed over as code units, as text the lexer can read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitSource {
    /// The units as text, with U+FFFD for each unpaired surrogate.
    pub text: String,
    /// Each unpaired surrogate, in order of offset.
    pub lone: Box<[LoneSurrogate]>,
}

impl UnitSource {
    /// Read `units` as source text.
    ///
    /// A surrogate pair is the one character it encodes; a surrogate that is not half of a pair is
    /// recorded and replaced. Never fails: every sequence of code units has exactly one reading.
    ///
    /// ```
    /// use viperjs::lexer::{LoneSurrogate, UnitSource};
    ///
    /// let read = UnitSource::new(&[0x27, 0xd800, 0x27]);
    /// assert_eq!(read.text, "'\u{fffd}'");
    /// assert_eq!(&*read.lone, &[LoneSurrogate { offset: 1, unit: 0xd800 }]);
    /// ```
    #[must_use]
    pub fn new(units: &[u16]) -> Self {
        let mut text = String::with_capacity(units.len());
        let mut lone = Vec::new();
        for decoded in char::decode_utf16(units.iter().copied()) {
            match decoded {
                Ok(ch) => text.push(ch),
                Err(unpaired) => {
                    // A String is at most `MAX_STRING_LENGTH` units, and each becomes at most three
                    // bytes, so the offset fits; saturating keeps a fact the types cannot see from
                    // being a panic.
                    lone.push(LoneSurrogate {
                        offset: u32::try_from(text.len()).unwrap_or(u32::MAX),
                        unit: unpaired.unpaired_surrogate(),
                    });
                    text.push(char::REPLACEMENT_CHARACTER);
                }
            }
        }
        Self {
            text,
            lone: lone.into_boxed_slice(),
        }
    }
}

/// The unit the U+FFFD at `offset` stands for, if it stands for one.
pub(crate) fn unit_at(lone: &[LoneSurrogate], offset: u32) -> Option<u16> {
    lone.binary_search_by_key(&offset, |found| found.offset)
        .ok()
        .and_then(|at| lone.get(at))
        .map(|found| found.unit)
}

/// The code units the text at `span` stands for: each character's UTF-16 encoding, with every
/// unpaired surrogate put back.
///
/// What a span of source *is* as a String, which is what §13.2.7.3's `BodyText` and §20.2.3.5's
/// `[[SourceText]]` both are. `None` for a span off a character boundary.
///
/// ```
/// use viperjs::lexer::{UnitSource, source_units};
/// use viperjs::span::Span;
///
/// let read = UnitSource::new(&[0x2f, 0xdc00, 0x2f]);
/// assert_eq!(source_units(&read.text, Span::new(1, 4), &read.lone), Some(vec![0xdc00]));
/// ```
#[must_use]
pub fn source_units(source: &str, span: Span, lone: &[LoneSurrogate]) -> Option<Vec<u16>> {
    let text = span.slice(source)?;
    let mut units = Vec::with_capacity(text.len());
    for (at, ch) in text.char_indices() {
        let offset = span
            .start
            .saturating_add(u32::try_from(at).unwrap_or(u32::MAX));
        match unit_at(lone, offset) {
            Some(unit) => units.push(unit),
            None => utf16_encode(ch as u32).push_onto(&mut units),
        }
    }
    Some(units)
}

#[cfg(test)]
mod tests {
    use super::{LoneSurrogate, UnitSource, source_units, unit_at};
    use crate::span::Span;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_pair_is_one_character_and_only_an_unpaired_half_is_recorded() {
        let read = UnitSource::new(&[0xd83d, 0xde00, 0xd800, 0x61, 0xdc00]);
        assert_eq!(read.text, "\u{1f600}\u{fffd}a\u{fffd}");
        assert_eq!(
            &*read.lone,
            &[
                LoneSurrogate {
                    offset: 4,
                    unit: 0xd800
                },
                LoneSurrogate {
                    offset: 8,
                    unit: 0xdc00
                },
            ]
        );
    }

    #[test]
    fn a_trail_before_its_lead_is_two_unpaired_surrogates_and_not_a_pair() {
        let read = UnitSource::new(&[0xdc00, 0xd800]);
        assert_eq!(read.text, "\u{fffd}\u{fffd}");
        assert_eq!(read.lone.len(), 2);
        assert_eq!(unit_at(&read.lone, 0), Some(0xdc00));
        assert_eq!(unit_at(&read.lone, 3), Some(0xd800));
    }

    #[test]
    fn a_replacement_character_the_program_wrote_is_not_put_back() {
        let read = UnitSource::new(&units("\u{fffd}x"));
        assert!(read.lone.is_empty());
        assert_eq!(
            source_units(&read.text, Span::new(0, 4), &read.lone),
            Some(vec![0xfffd, 0x78])
        );
    }

    #[test]
    fn a_span_of_source_is_its_units_with_every_unpaired_surrogate_restored() {
        let mut given = units("a");
        given.extend([0xdbff, 0x62, 0xdfff]);
        let read = UnitSource::new(&given);
        let whole = Span::new(0, read.text.len() as u32);
        assert_eq!(source_units(&read.text, whole, &read.lone), Some(given));
        // Offsets are absolute, so a span starting part way still finds the unit it covers.
        assert_eq!(
            source_units(&read.text, Span::new(4, 8), &read.lone),
            Some(vec![0x62, 0xdfff])
        );
        assert_eq!(source_units(&read.text, Span::new(2, 4), &read.lone), None);
    }

    #[test]
    fn a_source_with_no_surrogates_reads_as_itself() {
        let read = UnitSource::new(&units("let x = 'é';"));
        assert_eq!(read.text, "let x = 'é';");
        assert!(read.lone.is_empty());
        assert_eq!(UnitSource::new(&[]), UnitSource::default());
    }
}
