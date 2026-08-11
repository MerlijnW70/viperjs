//! UAX #15's four normalization forms, one row per decision the algorithm makes.
//!
//! Separate from [`super::string_methods`], where `normalize`'s *interface* is tested — the four
//! spellings, the RangeError for a fifth, the receiver checks. What is here is the algorithm
//! underneath: decomposition, canonical ordering, composition and the blocking rule, plus the
//! Hangul arithmetic that all three of those bypass a table for.
//!
//! **These exist because the file implementing them was not on the coverage list and had never been
//! probed.** When it was, twenty-seven mutations survived — including flipping NFD to behave as
//! NFKD, and every one of the six index calculations in the Hangul block. So each row below names
//! the decision it pins rather than merely asserting a normalized form, and every expected value
//! was checked against ICU before it was written down.
//!
//! A row is written with `\u` escapes on both sides. A pasted character cannot be trusted here: the
//! source file has itself been through an editor, and one of these test strings arrived already
//! decomposed the first time it was written, which turned an assertion about decomposition into an
//! assertion that decomposing twice is decomposing once.

use super::*;

/// `source.normalize(form)`, answered as the code points in `\u{…}` form.
///
/// Compared as *escapes* rather than as strings so that a failure names the code points that
/// differ. Two strings that differ only in combining order print identically in a terminal, which
/// is exactly the failure these rows are about.
fn normalized(source: &str, form: &str) -> String {
    run(&format!(
        "Array.from('{source}'.normalize('{form}')) \
         .map(function (c) {{ return c.codePointAt(0).toString(16).toUpperCase(); }}).join(' ')"
    ))
}

#[test]
fn the_four_forms_are_two_independent_decisions_and_all_four_differ() {
    // §22.1.3.13 step 5 — `compatibility` and `compose` are orthogonal, so a wrong pairing makes
    // one form behave as another and nothing else changes. Every one of the four flags could be
    // flipped without a test noticing until these rows existed.
    //
    // U+00C5 has a canonical decomposition and no compatibility one, so it separates C from D and
    // says nothing about K. U+00B2 is the mirror: compatibility only, so it separates K from
    // nothing else. Between them the two decisions are pinned independently, which one character
    // carrying both mappings could not do.
    for (input, form, expected) in [
        ("\\u00C5", "NFC", "C5"),
        ("\\u00C5", "NFD", "41 30A"),
        ("\\u00C5", "NFKC", "C5"),
        ("\\u00C5", "NFKD", "41 30A"),
        ("\\u00B2", "NFC", "B2"),
        ("\\u00B2", "NFD", "B2"),
        ("\\u00B2", "NFKC", "32"),
        ("\\u00B2", "NFKD", "32"),
    ] {
        assert_eq!(normalized(input, form), expected, "{input} under {form}");
    }
    // …and one character where all four answers are different, which is the strongest single
    // statement that the pairing is right: U+01C4 decomposes compatibly to three characters, and
    // the composing forms then put the caron back on the Z.
    for (form, expected) in [
        ("NFC", "1C4"),
        ("NFD", "1C4"),
        ("NFKC", "44 17D"),
        ("NFKD", "44 5A 30C"),
    ] {
        assert_eq!(normalized("\\u01C4", form), expected, "U+01C4 under {form}");
    }
    // A canonical decomposition that is a *singleton* — one character to one character — which is
    // why NFC is not the identity on anything already composed.
    assert_eq!(normalized("\\u2126", "NFC"), "3A9");
}

#[test]
fn a_hangul_syllable_decomposes_by_arithmetic_and_the_block_has_two_edges() {
    // UAX #15's Hangul Syllable Decomposition. Three divisions of one index, and each of the six
    // operators in them could be changed without any test complaining, because every syllable that
    // had been tested had a leading index and a vowel index of zero — U+AC00 is the *first*
    // syllable, so all three of its components are the base and no arithmetic is exercised at all.
    for (input, expected) in [
        // Leading 0, vowel 0, no trailing — the degenerate case the old rows used.
        ("\\uAC00", "1100 1161"),
        // …and a trailing, which is the only division U+AC00 does exercise.
        ("\\uAC01", "1100 1161 11A8"),
        // A nonzero *vowel* index, which nothing did.
        ("\\uAC1C", "1100 1162"),
        ("\\uAC1D", "1100 1162 11A8"),
        // A nonzero *leading* index.
        ("\\uB098", "1102 1161"),
        ("\\uB0B4", "1102 1162"),
        // All three nonzero, and the last syllable in the block.
        ("\\uD55C", "1112 1161 11AB"),
        ("\\uD7A3", "1112 1175 11C2"),
    ] {
        assert_eq!(normalized(input, "NFD"), expected, "{input} decomposed");
    }
    // Both edges of the block, because the guard is `>=` and an off-by-one in either direction
    // reads a code point that is not a syllable as one. U+D7A4 is exactly base + count.
    assert_eq!(normalized("\\uD7A4", "NFD"), "D7A4");
    assert_eq!(normalized("\\uABFF", "NFD"), "ABFF");
}

#[test]
fn canonical_ordering_sorts_each_run_of_marks_and_never_moves_a_starter() {
    // UAX #15's Canonical Ordering Algorithm — an insertion sort per run, not a sort of the string.
    //
    // The starter check is what makes it per-run, and it is invisible unless a starter follows a
    // mark of higher class: without it, the `A` here sorts in front of the grave, which changes
    // which character the mark belongs to.
    assert_eq!(normalized("\\u0300A", "NFD"), "300 41");
    // Two marks out of class order, at the very start of the string — the case that walks the
    // insertion all the way to index zero. A loop that tests `>=` instead of `>` reads before the
    // start here and nowhere else.
    assert_eq!(normalized("\\u0300\\u0316", "NFD"), "316 300");
    // Three, so the walk stops in the middle as well as at the end: U+0315 is class 232 and stays
    // where it is, being later than both.
    assert_eq!(normalized("\\u0300\\u0316\\u0315", "NFD"), "316 300 315");
    // Equal classes keep their written order, which is what "stable" means here and is the
    // difference between `<=` and `<` in the comparison that stops the walk.
    assert_eq!(normalized("\\u0316\\u0323", "NFD"), "316 323");
    assert_eq!(normalized("\\u0323\\u0316", "NFD"), "323 316");
    // A starter between two runs keeps them apart: the second run sorts on its own.
    assert_eq!(
        normalized("\\u0300\\u0316A\\u0300\\u0316", "NFD"),
        "316 300 41 316 300"
    );
}

#[test]
fn hangul_composes_in_two_steps_and_refuses_every_index_outside_the_block() {
    // The reverse arithmetic, which has its own four operators and four bounds. L+V first, then
    // LV+T — two steps rather than one, because a trailing jamo attaches to a syllable and not to
    // a pair.
    for (input, expected) in [
        ("\\u1100\\u1161", "AC00"),
        ("\\u1100\\u1162", "AC1C"),
        ("\\u1102\\u1161", "B098"),
        ("\\u1102\\u1162", "B0B4"),
        ("\\uAC00\\u11A8", "AC01"),
        ("\\u1100\\u1161\\u11A8", "AC01"),
        // An LV syllable that is **not the first one**, which is the whole of what the remainder
        // test says. Every LV syllable has an index divisible by 28; only U+AC00's is *zero*, so
        // a rule that multiplied where it should divide would still compose that one and nothing
        // else — and U+AC00 was the only syllable any row had ever handed a trailing jamo to.
        ("\\uAC1C\\u11A8", "AC1D"),
    ] {
        assert_eq!(normalized(input, "NFC"), expected, "{input} composed");
    }
    // Four bounds, each one past the end of what it admits. Modern Korean uses 19 leading jamo,
    // 21 vowels and 27 trailing — the archaic jamo beyond each are not part of the arithmetic and
    // must be left alone rather than folded into a syllable that does not exist.
    for (input, expected) in [
        // Leading index 19.
        ("\\u1113\\u1161", "1113 1161"),
        // Vowel index 21.
        ("\\u1100\\u1176", "1100 1176"),
        // Trailing index 28.
        ("\\uAC00\\u11C3", "AC00 11C3"),
        // Trailing index 0 is the filler and composes with nothing — `starter + 0` would answer
        // the syllable unchanged, which is the same string and a different reason.
        ("\\uAC00\\u11A7", "AC00 11A7"),
        // An LVT syllable already has a trailing jamo and takes no second one. This is the
        // remainder test, and it is the one bound that is not a comparison.
        ("\\uAC01\\u11A9", "AC01 11A9"),
        // One past the block, which the *bound* has to stop and the remainder does not: U+D7A4's
        // index is 11172, and 11172 is divisible by 28 — so it looks exactly like an LV syllable
        // to every test except the one that asks whether it is a syllable at all.
        ("\\uD7A4\\u11A8", "D7A4 11A8"),
    ] {
        assert_eq!(
            normalized(input, "NFC"),
            expected,
            "{input} must not compose"
        );
    }
}

#[test]
fn composition_is_blocked_by_a_mark_of_the_same_class_or_higher() {
    // UAX #15's blocking rule, and the reason composition cannot be "compose each pair": a
    // character composes with the last *starter* only when nothing between them sorts at or after
    // it. Both halves of that comparison are wrong in a way no ordinary word would show.
    //
    // Equal classes block. U+0305 and U+0301 are both 230, so the acute stays where it is and the
    // `a` is left bare — where a lone acute composes.
    assert_eq!(normalized("a\\u0305\\u0301", "NFC"), "61 305 301");
    assert_eq!(normalized("a\\u0301", "NFC"), "E1");
    // A *lower* class does not block, and the mark that did not block stays after the composite.
    // This is the row that separates `<` from `>`, and from a rule that looked only at the
    // character immediately before.
    assert_eq!(normalized("a\\u0316\\u0301", "NFC"), "E1 316");
    // …and the same shape where the first mark does compose, so the second is measured against a
    // class that a composition has already consumed.
    assert_eq!(normalized("a\\u0328\\u0301", "NFC"), "105 301");
    // A second starter begins again: the acute after `b` is measured against `b`, not against the
    // composite before it. Without the starter test, a mark would be taken as the new starter and
    // nothing after it could compose.
    assert_eq!(normalized("a\\u0301b\\u0301", "NFC"), "E1 62 301");
    // Ordering runs before composition, so a pair that arrives out of order composes as if it had
    // not: the dot below sorts first and neither combines with `q`.
    assert_eq!(normalized("q\\u0307\\u0323", "NFC"), "71 323 307");
}
