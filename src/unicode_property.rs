//! §22.2.1's `\p{…}` — which sets of code points a pattern may name, and what they contain.
//!
//! The data is in [`crate::unicode_property_table`], which is generated; everything that decides
//! what a *name* means is here, and so is every test.
//!
//! # Two forms and no third
//!
//! `\p{Lu}` is a lone name, which §22.2.1 resolves against the binary properties **and** the
//! `General_Category` values — one namespace searched twice, not two spellings of one thing.
//! `\p{Script=Greek}` is a name and a value, and only three names take one: `General_Category`,
//! `Script` and `Script_Extensions`. Anything else is a Syntax Error, including a name that is a
//! real Unicode property but not one of those three: `\p{Age=9.0}` is invalid JavaScript however
//! meaningful it is to Unicode.
//!
//! # Why the names are matched exactly
//!
//! The specification's tables are of *exact* strings — `Lu`, `Uppercase_Letter` — and neither
//! case-folds nor ignores the underscores, where UTS #18 suggests an implementation may. So
//! `\p{uppercase_letter}` is a Syntax Error and `\p{Uppercase_Letter}` is not, and a lookup that
//! normalised first would accept a whole family of patterns no other engine does.

use crate::unicode_property_table::{BINARY, GENERAL_CATEGORY, SCRIPT, SCRIPT_EXTENSIONS};

/// A set of code points a `\p{…}` names, and whether the escape was the negated `\P{…}`.
///
/// The ranges are `'static` and shared with every other pattern naming the same property: a set is
/// a slice into the generated table rather than something a compiled pattern owns. That is what
/// makes `\p{Alphabetic}` free to write — the 1,400 ranges behind it are in the binary once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Property {
    /// The code points in it, as sorted, disjoint, inclusive ranges.
    ranges: &'static [(u32, u32)],
    /// Whether `\P{…}` was written rather than `\p{…}`.
    negated: bool,
}

impl Property {
    /// Whether `code` is in this set, taking the negation into account.
    ///
    /// A binary search, because the ranges are sorted and disjoint and some of these sets have
    /// well over a thousand of them. `partition_point` rather than `binary_search_by`: the answer
    /// wanted is "the last range starting at or below this", which is a boundary and not a hit.
    pub fn contains(&self, code: u32) -> bool {
        let at = self.ranges.partition_point(|(low, _)| *low <= code);
        let found = match at {
            0 => false,
            _ => self.ranges[at - 1].1 >= code,
        };
        found != self.negated
    }

    /// The same set with the opposite sense — what `\P{…}` is to `\p{…}`.
    pub fn negate(self) -> Self {
        Self {
            negated: !self.negated,
            ..self
        }
    }
}

/// §22.2.1's *properties of strings* — the `v`-flag names that match sequences, not code points.
///
/// Listed rather than resolved because ViperJS does not have them: `\p{RGI_Emoji}` matches a
/// *string* of several code points, which the matcher here has no shape for. Naming them keeps the
/// refusal honest — they are a feature this engine lacks, not a name the specification rejects,
/// and only the first of those may be judged as an early error.
pub const OF_STRINGS: &[&str] = &[
    "Basic_Emoji",
    "Emoji_Keycap_Sequence",
    "RGI_Emoji",
    "RGI_Emoji_Flag_Sequence",
    "RGI_Emoji_Modifier_Sequence",
    "RGI_Emoji_Tag_Sequence",
    "RGI_Emoji_ZWJ_Sequence",
];

/// Resolve what was written between the braces — `None` if it names nothing §22.2.1 allows.
///
/// The caller has already read the braces and decided the escape is `\p` rather than `\P`; this is
/// only about the name. A `None` is a Syntax Error at the call site and not an empty set: a
/// pattern naming a property that does not exist is not a pattern that matches nothing.
pub fn lookup(spelled: &str) -> Option<Property> {
    let ranges = match spelled.split_once('=') {
        // §22.2.1 `UnicodePropertyName=UnicodePropertyValue`, and exactly three names take a value.
        // `Script_Extensions` is not a superset of `Script` and is not interchangeable with it:
        // `\p{scx=Greek}` includes code points shared with other scripts that `\p{sc=Greek}` does
        // not, which is the whole reason Unicode publishes both.
        Some((name, value)) => match name {
            "General_Category" | "gc" => find(GENERAL_CATEGORY, value),
            "Script" | "sc" => find(SCRIPT, value),
            "Script_Extensions" | "scx" => find(SCRIPT_EXTENSIONS, value),
            _ => None,
        },
        // §22.2.1 `LoneUnicodePropertyNameOrValue` — a binary property, or a `General_Category`
        // value written without its name. The binary table is asked first, which is what the
        // specification's order says; nothing is in both, so the order is not load-bearing and is
        // worth stating for the day something is.
        None => find(BINARY, spelled).or_else(|| find(GENERAL_CATEGORY, spelled)),
    }?;
    Some(Property {
        ranges,
        negated: false,
    })
}

/// A linear scan of one table, matching the spelling exactly.
///
/// Linear rather than sorted-and-searched because it happens when a pattern is **compiled** and
/// never while one runs, and the longest table is a few hundred entries. A binary search would
/// need the generated table to be sorted by name, which is a property nothing else wants and one
/// more thing a regeneration could quietly get wrong.
fn find(
    table: &'static [(&'static str, &'static [(u32, u32)])],
    name: &str,
) -> Option<&'static [(u32, u32)]> {
    table
        .iter()
        .find(|(spelled, _)| *spelled == name)
        .map(|(_, ranges)| *ranges)
}

#[cfg(test)]
mod tests;
