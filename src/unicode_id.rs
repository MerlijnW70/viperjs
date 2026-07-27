//! The identifier character classes of ECMA-262 §12.7, over the generated Unicode tables.
//!
//! The data lives in [`crate::unicode_id_table`] and is regenerated whole; everything here —
//! the two predicates, the search that backs them, and every test — is written by hand.
//!
//! Both predicates take a `u32` rather than a `char` on purpose. A `\uD800` escape in an
//! identifier hands the lexer a lone surrogate, and `\u{110000}` hands it a value past the last
//! code point; neither is a `char`, both must be *answered* rather than rejected by the type
//! system, because §12.7.1.1 makes them Syntax Errors and a Syntax Error is a diagnostic we owe
//! the user, not a conversion we quietly fail.

use crate::unicode_id_table::{ID_CONTINUE, ID_START};
use std::cmp::Ordering;

/// Whether `cp` may begin an identifier.
///
/// `IdentifierStartChar :: UnicodeIDStart | $ | _` (§12.7). Both additions are ECMAScript's own
/// — Note 1 says so explicitly — and neither is redundant: U+0024 has neither Unicode property,
/// and U+005F has `ID_Continue` but **not** `ID_Start`. That asymmetry is the whole reason the
/// grammar lists `_` here and omits it from `IdentifierPartChar`.
pub(crate) fn is_id_start(cp: u32) -> bool {
    cp == '$' as u32 || cp == '_' as u32 || in_table(ID_START, cp)
}

/// Whether `cp` may continue an identifier.
///
/// `IdentifierPartChar :: UnicodeIDContinue | $` (§12.7). Two things a reader will expect to
/// find here and should not add:
///
/// - **`_` is absent.** §12.7 Note 2: it derives via `UnicodeIDContinue`.
/// - **`<ZWNJ>` and `<ZWJ>` are absent.** Editions through ES2023 listed U+200C and U+200D as
///   explicit alternatives here. They were removed because Unicode gave both the `ID_Continue`
///   property, so the table now answers for them — adding them back would be harmless today and
///   wrong the moment the two disagree again.
pub(crate) fn is_id_continue(cp: u32) -> bool {
    cp == '$' as u32 || in_table(ID_CONTINUE, cp)
}

/// Whether `cp` falls in one of `table`'s sorted, disjoint, inclusive ranges.
///
/// Binary search rather than a bitset or a two-stage trie: 684 and 799 ranges make this ten
/// comparisons at worst, and the boring version is the one that is obviously correct. Speed
/// here is an M8 question with a benchmark attached, not a guess made now.
fn in_table(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                Ordering::Less
            } else if lo > cp {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tables must be sorted, disjoint, and *maximally merged* — adjacent ranges collapsed.
    ///
    /// Sortedness is what `binary_search_by` requires to be correct at all; a table that lost it
    /// would answer "not an identifier character" for arbitrary letters, and only for some of
    /// them. Maximal merging is not required for correctness, but a gap of zero between two
    /// ranges is the signature of a generator that failed to merge, which is exactly the kind of
    /// silent data rot this test exists to catch.
    #[test]
    fn both_tables_are_sorted_disjoint_and_maximally_merged() {
        for (name, table) in [("ID_START", ID_START), ("ID_CONTINUE", ID_CONTINUE)] {
            assert!(!table.is_empty(), "{name} is empty");
            for &(lo, hi) in table {
                assert!(lo <= hi, "{name} has a reversed range {lo:#x}..{hi:#x}");
                assert!(
                    hi <= 0x10ffff,
                    "{name} range {hi:#x} exceeds the last code point"
                );
            }
            for pair in table.windows(2) {
                let [(_, prev_hi), (next_lo, _)] = pair else {
                    continue;
                };
                assert!(
                    prev_hi < next_lo,
                    "{name}: {prev_hi:#x} and {next_lo:#x} overlap or are unmerged"
                );
                assert!(
                    *prev_hi + 1 < *next_lo,
                    "{name}: {prev_hi:#x} and {next_lo:#x} are adjacent and should be one range"
                );
            }
            // A surrogate has no `ID_Start`/`ID_Continue` property and can never appear in
            // well-formed source; a range covering one would mean the generator read the file
            // wrong. It also matters for `\uD800` escapes, which reach these predicates as raw
            // `u32` precisely because they are not characters.
            for &(lo, hi) in table {
                assert!(hi < 0xd800 || lo > 0xdfff, "{name} covers a surrogate");
            }
        }
    }

    /// The checked-in shape of Unicode 17.0.0. These four numbers are the table's checksum.
    ///
    /// They are not computed from the tables they check — they come from the UCD as counted at
    /// generation time. A regeneration that moves them is a Unicode version bump, and it should
    /// arrive as a commit that says so rather than as a diff nobody read.
    #[test]
    fn the_tables_match_the_unicode_version_they_claim() {
        assert_eq!(ID_START.len(), 684);
        assert_eq!(ID_CONTINUE.len(), 799);
        let count = |table: &[(u32, u32)]| table.iter().map(|&(lo, hi)| hi - lo + 1).sum::<u32>();
        assert_eq!(count(ID_START), 145_916);
        assert_eq!(count(ID_CONTINUE), 149_240);
    }

    /// Every `ID_Start` code point is an `ID_Continue` code point.
    ///
    /// Unicode guarantees this, and identifier scanning silently depends on it: the lexer tests
    /// the first character against `ID_Start` and every later one against `ID_Continue`, so a
    /// letter that started an identifier but could not continue one would make `aa` a syntax
    /// error while `a` was fine.
    #[test]
    fn every_id_start_code_point_also_continues_an_identifier() {
        for &(lo, hi) in ID_START {
            let covered = ID_CONTINUE.iter().any(|&(clo, chi)| clo <= lo && hi <= chi);
            assert!(
                covered,
                "ID_Start range {lo:#x}..{hi:#x} escapes ID_Continue"
            );
        }
    }

    /// The search must include both ends of a range and exclude both neighbours.
    ///
    /// Written against ranges at the front, middle and very end of each table, because a
    /// comparison that is wrong by one still finds most characters — and a binary search whose
    /// last range is unreachable fails only for astral code points nobody tests by accident.
    #[test]
    fn the_range_search_includes_both_endpoints_and_excludes_their_neighbours() {
        // 'A'..'Z', the first ID_Start range.
        assert!(!in_table(ID_START, 0x40)); // '@', one before
        assert!(in_table(ID_START, 0x41)); // 'A', the low end
        assert!(in_table(ID_START, 0x4d)); // 'M', the middle
        assert!(in_table(ID_START, 0x5a)); // 'Z', the high end
        assert!(!in_table(ID_START, 0x5b)); // '[', one after

        // '0'..'9', the first ID_Continue range — and the low end of the whole table, where an
        // off-by-one in the search's lower bound shows up first.
        assert!(!in_table(ID_CONTINUE, 0x2f));
        assert!(in_table(ID_CONTINUE, 0x30));
        assert!(in_table(ID_CONTINUE, 0x39));
        assert!(!in_table(ID_CONTINUE, 0x3a));

        // The last range of each table: U+31350..U+33479 and U+E0100..U+E01EF. Nothing after
        // them is in either table, up to the last code point there is.
        assert!(in_table(ID_START, 0x31350));
        assert!(in_table(ID_START, 0x33479));
        assert!(!in_table(ID_START, 0x3347a));
        assert!(in_table(ID_CONTINUE, 0xe0100));
        assert!(in_table(ID_CONTINUE, 0xe01ef));
        assert!(!in_table(ID_CONTINUE, 0xe01f0));
        assert!(!in_table(ID_CONTINUE, 0x10ffff));
        assert!(!in_table(ID_START, 0));
    }

    /// The two ECMAScript additions, and the asymmetry between them.
    #[test]
    fn dollar_and_underscore_are_ecmascripts_own_additions() {
        // `$` has neither Unicode property. Without §12.7's explicit alternative, jQuery would
        // not be expressible.
        assert!(!in_table(ID_START, '$' as u32));
        assert!(!in_table(ID_CONTINUE, '$' as u32));
        assert!(is_id_start('$' as u32));
        assert!(is_id_continue('$' as u32));

        // `_` is `ID_Continue` but NOT `ID_Start` — which is why the grammar names it in
        // `IdentifierStartChar` and leaves it out of `IdentifierPartChar` (Note 2). Drop the
        // explicit alternative and `_` stops being a legal variable name.
        assert!(!in_table(ID_START, '_' as u32));
        assert!(in_table(ID_CONTINUE, '_' as u32));
        assert!(is_id_start('_' as u32));
        assert!(is_id_continue('_' as u32));
    }

    /// Spot checks across scripts and categories, each chosen because it separates a plausible
    /// wrong implementation from the right one.
    #[test]
    fn the_predicates_answer_the_unicode_standard_across_scripts() {
        // Letters that start identifiers, from the BMP and beyond. An implementation that
        // stopped at `char::is_alphabetic` would pass all of these — which is why the negatives
        // below matter more.
        for (cp, what) in [
            (0x0041, "LATIN CAPITAL A"),
            (0x00aa, "FEMININE ORDINAL INDICATOR"),
            (0x00e9, "LATIN SMALL E WITH ACUTE"),
            (0x03a9, "GREEK CAPITAL OMEGA"),
            (0x05d0, "HEBREW ALEF"),
            (0x3042, "HIRAGANA A"),
            (0x4e00, "CJK ONE"),
            (0x1d49c, "MATHEMATICAL SCRIPT CAPITAL A"),
            // Other_ID_Start: neither is a letter by general category, and both are in ID_Start
            // only because Unicode grandfathered them. §12.7 Note 3 requires them.
            (0x2118, "SCRIPT CAPITAL P"),
            (0x212e, "ESTIMATED SYMBOL"),
        ] {
            assert!(
                is_id_start(cp),
                "{what} (U+{cp:04X}) must start an identifier"
            );
            assert!(is_id_continue(cp), "{what} must also continue one");
        }

        // Continues but does not start. Getting these wrong makes `x1` illegal or `1x` legal.
        for (cp, what) in [
            (0x0030, "DIGIT ZERO"),
            (0x0660, "ARABIC-INDIC DIGIT ZERO"),
            (0x0301, "COMBINING ACUTE ACCENT"),
            (0x00b7, "MIDDLE DOT — Other_ID_Continue"),
            (0x200c, "ZERO WIDTH NON-JOINER"),
            (0x200d, "ZERO WIDTH JOINER"),
            (0xe0100, "VARIATION SELECTOR-17"),
        ] {
            assert!(
                !is_id_start(cp),
                "{what} (U+{cp:04X}) must not start an identifier"
            );
            assert!(is_id_continue(cp), "{what} must continue one");
        }

        // Neither. The rocket is the one people expect to work; U+200B is the one that looks
        // like it already does. Surrogates and out-of-range values arrive here from `\u`
        // escapes and must simply answer "no" rather than blowing up.
        for (cp, what) in [
            (0x0020, "SPACE"),
            (0x002d, "HYPHEN-MINUS"),
            (0x0021, "EXCLAMATION MARK"),
            (0x200b, "ZERO WIDTH SPACE"),
            (0x2029, "PARAGRAPH SEPARATOR"),
            (0x1f680, "ROCKET"),
            (0xd800, "a lone high surrogate"),
            (0xdfff, "a lone low surrogate"),
            (0x110000, "one past the last code point"),
            (u32::MAX, "not a code point at all"),
        ] {
            assert!(
                !is_id_start(cp),
                "{what} (U+{cp:04X}) must not start an identifier"
            );
            assert!(!is_id_continue(cp), "{what} must not continue one");
        }
    }
}
