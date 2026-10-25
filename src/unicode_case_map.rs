//! The Unicode Default Case Conversion — what §22.1.3.28 and §22.1.3.30 hand a String's code points
//! to.
//!
//! # Full mappings, and one that looks around
//!
//! §22.1.3.30 names "the locale-insensitive case mappings in the Unicode Character Database", and
//! says that includes `SpecialCasing.txt` as well as `UnicodeData.txt`. The unconditional entries
//! there are a table — [`crate::unicode_case_map_table`] — and they are why a result may be longer
//! than its input: `ß` uppercases to `SS` and `İ` lowercases to `i` and a combining dot.
//!
//! One entry is conditional and still locale-insensitive: **`Final_Sigma`**. `Σ` lowercases to `ς`
//! at the end of a word and to `σ` everywhere else, so `"ΟΔΟΣ"` lowers to `"οδος"`. Whether it is at
//! the end of a word is a question about the code points on either side, which no table indexed by
//! one code point can answer, so it is asked here.
//!
//! # What this operates on
//!
//! Code *points*, as [`crate::unicode_normalize`] does: the caller pairs surrogates on the way in and
//! splits them on the way out. An unpaired surrogate has no mapping, is neither `Cased` nor
//! `Case_Ignorable`, and passes through as itself.

use crate::unicode_case_map_table::{LOWERCASE, UPPERCASE};
use crate::unicode_property::Property;

/// GREEK CAPITAL LETTER SIGMA — the one code point whose lowercase depends on its neighbours.
const CAPITAL_SIGMA: u32 = 0x03A3;
/// GREEK SMALL LETTER FINAL SIGMA — what `Final_Sigma` maps it to.
const FINAL_SIGMA: u32 = 0x03C2;

/// `toLowercase` or `toUppercase` of `points`, as the Unicode Standard's §3.13 defines them.
///
/// Total: every input, unpaired surrogates included, has an answer, and nothing here allocates
/// more than the result. The result is at most three code points per input code point, which is
/// the longest row in `SpecialCasing.txt`.
pub(crate) fn recase(points: &[u32], upwards: bool) -> Vec<u32> {
    let table = match upwards {
        true => UPPERCASE,
        false => LOWERCASE,
    };
    // Looked up once and only if a sigma is lowered: most strings never ask.
    let mut context: Option<SigmaContext> = None;
    let mut out = Vec::with_capacity(points.len());
    for (at, &point) in points.iter().enumerate() {
        if !upwards && point == CAPITAL_SIGMA {
            let context = context.get_or_insert_with(SigmaContext::new);
            if context.is_final(points, at) {
                out.push(FINAL_SIGMA);
                continue;
            }
        }
        match table.binary_search_by_key(&point, |(from, _)| *from) {
            Ok(found) => out.extend_from_slice(table[found].1),
            Err(_) => out.push(point),
        }
    }
    out
}

/// The two properties `Final_Sigma` is defined in terms of.
///
/// From [`crate::unicode_property_table`], so the context is read from the same Unicode edition as
/// the mappings and as `\p{Cased}` in a pattern.
struct SigmaContext {
    /// Unicode's `Cased`, D135.
    cased: Option<Property>,
    /// Unicode's `Case_Ignorable`, D136.
    ignorable: Option<Property>,
}

impl SigmaContext {
    /// Both sets, resolved by the names `\p{…}` would use.
    fn new() -> Self {
        Self {
            cased: crate::unicode_property::lookup("Cased"),
            ignorable: crate::unicode_property::lookup("Case_Ignorable"),
        }
    }

    /// Whether `point` is `Cased`.
    fn cased(&self, point: u32) -> bool {
        self.cased.is_some_and(|set| set.contains(point))
    }

    /// Whether `point` is `Case_Ignorable`.
    fn ignorable(&self, point: u32) -> bool {
        self.ignorable.is_some_and(|set| set.contains(point))
    }

    /// The Unicode Standard §3.13, Table 3-17 `Final_Sigma`: `points[at]` is preceded by a cased
    /// letter and then zero or more case-ignorable characters, and is **not** followed by zero or
    /// more case-ignorable characters and then a cased letter.
    ///
    /// **Ignorable is asked before cased.** A few code points are both — U+0345 COMBINING GREEK
    /// YPOGEGRAMMENI is the one test262 asks about — and Table 3-17's regular expressions read
    /// literally would let such a code point be the cased letter. ECMAScript engines skip it as
    /// ignorable instead, and test262's `special_casing_conditional.js` pins that reading in both
    /// directions: `"ͅΣ"` lowers to `σ` and `"AΣͅ"` to `ς`.
    ///
    /// Linear over a whole string however many sigmas it holds: a scan stops at the first code point
    /// that is not ignorable, so each run of ignorables is crossed by at most the sigma on either
    /// side of it.
    fn is_final(&self, points: &[u32], at: usize) -> bool {
        let before = points[..at]
            .iter()
            .rev()
            .find(|&&point| !self.ignorable(point))
            .is_some_and(|&point| self.cased(point));
        let after = points[at + 1..]
            .iter()
            .find(|&&point| !self.ignorable(point))
            .is_some_and(|&point| self.cased(point));
        before && !after
    }
}

#[cfg(test)]
mod tests;
