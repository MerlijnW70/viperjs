//! What `\p{…}` accepts, what it refuses, and what the sets behind it contain.

use super::lookup;

/// Whether `spelled` names a set containing `code` — the whole of what a test here asks.
fn holds(spelled: &str, code: char) -> bool {
    lookup(spelled)
        .unwrap_or_else(|| panic!("{spelled} should name a property")) // the lookup is the test
        .contains(code as u32)
}

#[test]
fn a_lone_name_is_looked_for_among_the_binary_properties_and_the_categories() {
    // §22.2.1 `LoneUnicodePropertyNameOrValue` — one namespace searched twice. `Lu` is a
    // `General_Category` value and `Alphabetic` is a binary property, and both are written the
    // same way.
    assert!(holds("Lu", 'A'));
    assert!(!holds("Lu", 'a'));
    assert!(holds("Alphabetic", '字'));
    assert!(!holds("Alphabetic", '1'));
    // A name that is neither is a Syntax Error rather than an empty set: a pattern naming a
    // property that does not exist is not a pattern that matches nothing.
    assert!(lookup("Nope").is_none());
    assert!(lookup("").is_none());
}

#[test]
fn a_named_value_takes_exactly_three_names() {
    // §22.2.1 `UnicodePropertyName=UnicodePropertyValue`, and the list is closed: `Age` is a real
    // Unicode property and `\p{Age=9.0}` is still invalid JavaScript.
    assert!(holds("Script=Greek", 'α'));
    assert!(holds("General_Category=Lu", 'A'));
    assert!(holds("Script_Extensions=Greek", 'α'));
    assert!(lookup("Age=9.0").is_none());
    assert!(lookup("Block=Cyrillic").is_none());
    // …and a value that does not belong to the name it is given.
    assert!(lookup("Script=Lu").is_none());
    assert!(lookup("General_Category=Greek").is_none());
}

#[test]
fn script_extensions_is_not_script_and_the_difference_is_the_point() {
    // U+0342 COMBINING GREEK PERISPOMENI has `Script=Inherited` and `Script_Extensions=Greek`.
    // Unicode publishes both because a combining mark belongs to the scripts it is *used with*
    // rather than to one of its own, and a `scx` table computed as a copy of `sc` would answer
    // this one wrongly and every unshared code point rightly — which is how it would go unnoticed.
    assert!(holds("Script_Extensions=Greek", '\u{342}'));
    assert!(!holds("Script=Greek", '\u{342}'));
    assert!(holds("Script=Inherited", '\u{342}'));
    // A code point that no `ScriptExtensions.txt` line mentions belongs to its own script's
    // extension set, which is the default the file's absences stand for.
    assert!(holds("Script_Extensions=Latin", 'a'));
    assert!(holds("Script=Latin", 'a'));
}

#[test]
fn the_short_and_long_spellings_name_the_same_set() {
    // §22.2.1's tables list both, exactly — so each is a separate entry pointing at the same
    // ranges rather than something normalised at the lookup.
    for (short, long) in [
        ("gc=Lu", "General_Category=Uppercase_Letter"),
        ("sc=Grek", "Script=Greek"),
        ("scx=Grek", "Script_Extensions=Greek"),
    ] {
        assert_eq!(lookup(short), lookup(long), "{short} against {long}");
    }
    assert_eq!(lookup("Alpha"), lookup("Alphabetic"));
    assert_eq!(lookup("Lu"), lookup("Uppercase_Letter"));
}

#[test]
fn the_names_are_matched_exactly_and_not_folded() {
    // UTS #18 suggests an implementation may ignore case and underscores; §22.2.1 does not, and
    // its tables are of exact strings. Accepting more would make praxis take patterns no other
    // engine does, which is a wrong answer in the direction nobody checks.
    assert!(lookup("uppercase_letter").is_none());
    assert!(lookup("ALPHABETIC").is_none());
    assert!(lookup("Uppercaseletter").is_none());
    assert!(lookup("script=Greek").is_none());
}

#[test]
fn a_grouped_category_is_the_union_of_the_specific_ones() {
    // `L` is `Lu`, `Ll`, `Lt`, `Lm` and `Lo` together, and no UCD file says so — it is derived
    // when the table is generated. So the assertion is that the derivation happened.
    for letter in ['A', 'a', 'ǅ', 'ʰ', '字'] {
        assert!(holds("L", letter), "L should hold {letter:?}");
    }
    assert!(!holds("L", '1'));
    assert!(holds("LC", 'A') && holds("LC", 'a'));
    assert!(!holds("LC", 'ʰ'), "a modifier letter is not a cased letter");
    assert!(holds("N", '1') && holds("Nd", '1'));
}

#[test]
fn assigned_is_everything_that_is_not_unassigned() {
    // Derived as the complement of `Cn`, which is the only way to get it: no file lists what is
    // assigned, only what is not.
    assert!(holds("Assigned", 'a'));
    assert!(!holds("Assigned", '\u{FFF0}'));
    assert!(holds("Cn", '\u{FFF0}'));
    // `Any` is every code point there is, including the ones nothing else holds.
    assert!(holds("Any", '\u{FFF0}'));
    assert!(holds("Any", '\u{10FFFF}'));
}

#[test]
fn negating_a_property_answers_the_other_way_round_and_twice_answers_back() {
    let upper = lookup("Lu").expect("Lu is a property"); // the lookup is not what this tests
    assert!(upper.contains('A' as u32) && !upper.contains('a' as u32));
    let not_upper = upper.negate();
    assert!(!not_upper.contains('A' as u32) && not_upper.contains('a' as u32));
    assert_eq!(not_upper.negate(), upper);
}

#[test]
fn membership_is_decided_at_the_edges_of_every_range() {
    // The binary search is the one piece of logic here, and off-by-one is the only way it fails.
    // ASCII is a single range, so its two edges and the code point past them say everything.
    let ascii = lookup("ASCII").expect("ASCII is a property"); // same
    assert!(ascii.contains(0));
    assert!(ascii.contains(0x7F));
    assert!(!ascii.contains(0x80));
    // …and a set with many ranges, either side of an interior gap. `Nd` holds the ASCII digits
    // and not the punctuation on both sides of them.
    let digits = lookup("Nd").expect("Nd is a property"); // same
    assert!(!digits.contains(u32::from(b'/')));
    assert!(digits.contains(u32::from(b'0')));
    assert!(digits.contains(u32::from(b'9')));
    assert!(!digits.contains(u32::from(b':')));
    // Beyond every range there is, which is where a search that read one entry too far would look.
    assert!(!digits.contains(0x10FFFF));
}

#[test]
fn a_property_of_strings_is_not_a_code_point_property() {
    // §22.2.1's `\p{RGI_Emoji}` and its six siblings match a *sequence* — with the `v` flag they
    // are valid and praxis has no shape for them, so the parser refuses them as unbuilt rather
    // than as invalid. Here the only claim is that `lookup` does not quietly answer with some
    // *code point* set of the same name, which would match one character and call it an emoji.
    for name in super::OF_STRINGS {
        assert!(
            lookup(name).is_none(),
            "{name} is not a code point property"
        );
    }
}
