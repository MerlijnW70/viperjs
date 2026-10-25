//! The case tables' shape, and `Final_Sigma` asked of code points directly.
//!
//! What a script sees — the same rules through `toLowerCase` and `toUpperCase`, and a corpus
//! checked against V8 — is in `vm/tests/case_mapping.rs`.

use super::{SigmaContext, recase};
use crate::unicode_case_map_table::{LOWERCASE, UPPERCASE};

/// `recase` over a `&str`, answered as a `String` — only for inputs that are all characters.
fn lowered(text: &str) -> String {
    cased(text, false)
}

/// The same, upwards.
fn uppered(text: &str) -> String {
    cased(text, true)
}

/// `recase` over the characters of `text`, either way.
fn cased(text: &str, upwards: bool) -> String {
    let points: Vec<u32> = text.chars().map(u32::from).collect();
    recase(&points, upwards)
        .into_iter()
        .map(|point| char::from_u32(point).expect("a character maps to characters")) // the test
        .collect()
}

/// The checked-in shape of Unicode 17.0.0. These numbers are the tables' checksum.
///
/// They come from the UCD as counted at generation time, not from the tables they check: a
/// regeneration that moves them is a Unicode version bump, and it should arrive as a commit that
/// says so.
#[test]
fn the_tables_match_the_unicode_version_they_claim() {
    assert_eq!(LOWERCASE.len(), 1488);
    assert_eq!(UPPERCASE.len(), 1580);
    let longer = |table: &[(u32, &[u32])]| table.iter().filter(|(_, to)| to.len() > 1).count();
    assert_eq!(longer(LOWERCASE), 1);
    assert_eq!(longer(UPPERCASE), 102);
}

#[test]
fn every_row_is_sorted_changes_something_and_names_only_characters() {
    for (name, table) in [("LOWERCASE", LOWERCASE), ("UPPERCASE", UPPERCASE)] {
        // Sorted and unique, because a lookup is a binary search and a duplicate would be found or
        // not depending on where the search happened to land.
        assert!(
            table.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "{name} is not sorted"
        );
        for &(from, to) in table {
            assert!(char::from_u32(from).is_some(), "{name} maps {from:#x}");
            assert!(
                to.iter().all(|&point| char::from_u32(point).is_some()),
                "{name} maps {from:#x} to a non-character"
            );
            // `SpecialCasing.txt`'s longest row, and the bound `recase` documents.
            assert!((1..=3).contains(&to.len()), "{name} row {from:#x}");
            assert_ne!(to, [from], "{name} maps {from:#x} to itself");
        }
    }
}

#[test]
fn both_properties_final_sigma_reads_resolve() {
    // A misspelt name would answer `None`, and the context would then call nothing cased and every
    // sigma medial — a silent wrong answer rather than a failure. This is where it fails instead.
    let context = SigmaContext::new();
    assert!(context.cased.is_some());
    assert!(context.ignorable.is_some());
    assert!(context.cased('A' as u32) && !context.ignorable('A' as u32));
    assert!(context.ignorable('\'' as u32) && !context.cased('\'' as u32));
    // U+0345 is both, which is why `is_final` has to choose an order.
    assert!(context.cased(0x0345) && context.ignorable(0x0345));
}

#[test]
fn a_sigma_is_final_after_a_cased_letter_and_before_none() {
    assert_eq!(lowered("ΟΔΟΣ"), "οδος");
    assert_eq!(lowered("ΟΔΟΣ ΟΔΟΣ"), "οδος οδος");
    assert_eq!(lowered("ΣΑΣ"), "σας");
    // Medial: a cased letter follows.
    assert_eq!(lowered("ΑΣΑ"), "ασα");
    // Alone: nothing cased precedes it.
    assert_eq!(lowered("Σ"), "σ");
    assert_eq!(lowered(" Σ "), " σ ");
    assert_eq!(lowered("1Σ"), "1σ");
}

#[test]
fn case_ignorable_code_points_are_looked_through_on_both_sides() {
    // An apostrophe and a full stop are `Case_Ignorable` (MidLetter, MidNumLet).
    assert_eq!(lowered("Α'Σ"), "α'ς");
    assert_eq!(lowered("ΑΣ'Α"), "ασ'α");
    assert_eq!(lowered("ΑΣ'"), "ας'");
    assert_eq!(lowered("'Σ"), "'σ");
    // U+0345 is cased *and* ignorable, and is read as ignorable on either side.
    assert_eq!(lowered("\u{0345}Σ"), "\u{0345}σ");
    assert_eq!(lowered("Α\u{0345}Σ"), "α\u{0345}ς");
    assert_eq!(lowered("ΑΣ\u{0345}"), "ας\u{0345}");
    assert_eq!(lowered("ΑΣ\u{0345}Α"), "ασ\u{0345}α");
    // A space is neither, so it ends the scan in both directions.
    assert_eq!(lowered("Α Σ"), "α σ");
    assert_eq!(lowered("ΑΣ Α"), "ας α");
}

#[test]
fn uppercasing_never_produces_a_final_sigma_and_keeps_both_small_forms_apart() {
    assert_eq!(uppered("οδος"), "ΟΔΟΣ");
    assert_eq!(uppered("σς"), "ΣΣ");
    assert_eq!(lowered("ς"), "ς");
    assert_eq!(lowered("σ"), "σ");
}

#[test]
fn an_unconditional_special_casing_row_changes_the_length_either_way() {
    assert_eq!(uppered("ß"), "SS");
    assert_eq!(uppered("ŉ"), "ʼN");
    assert_eq!(uppered("ﬃ"), "FFI");
    assert_eq!(uppered("\u{0390}"), "\u{0399}\u{0308}\u{0301}");
    assert_eq!(uppered("ᾳ"), "ΑΙ");
    assert_eq!(lowered("İ"), "i\u{0307}");
    // The simple mapping is the one used where `SpecialCasing.txt` says nothing: `ẞ` lowercases to
    // `ß`, which does not uppercase back to `ẞ`.
    assert_eq!(lowered("ẞ"), "ß");
    assert_eq!(uppered(&lowered("ẞ")), "SS");
}

#[test]
fn an_unpaired_surrogate_passes_through_and_is_no_letter_to_a_sigma() {
    assert_eq!(recase(&[0xD800], true), [0xD800]);
    assert_eq!(
        recase(&[0x41, 0xDC00, 0x03A3], false),
        [0x61, 0xDC00, 0x03C3]
    );
    assert_eq!(
        recase(&[0x41, 0x03A3, 0xD800], false),
        [0x61, 0x03C2, 0xD800]
    );
}
