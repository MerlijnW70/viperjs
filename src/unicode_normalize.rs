//! UAX #15's four normalization forms — the algorithm §22.1.3.13 hands its argument to.
//!
//! # The four forms are two decisions
//!
//! A form chooses **which decomposition** to apply — canonical, or canonical plus compatibility —
//! and **whether to compose afterwards**. NFD and NFKD stop after decomposing; NFC and NFKC put
//! back what canonical composition allows. So there is one algorithm here and not four, and the
//! form is two booleans rather than an enum with four arms doing similar things.
//!
//! # Why decomposition is a table lookup and Hangul is arithmetic
//!
//! The generated tables are already recursive — see [`crate::unicode_normalize_table`] — so a
//! decomposition is one lookup rather than a loop to a fixed point. Hangul is not in them because
//! UAX #15 defines it as arithmetic over a base, and 11,172 rows standing in for four
//! multiplications would be data pretending to be a decision.
//!
//! # What this operates on
//!
//! Code *points*, not code units. A String here is UTF-16 and normalization is defined over
//! scalar values, so the caller pairs surrogates on the way in and splits them on the way out —
//! an unpaired surrogate has no decomposition and passes through as itself, which is what keeps
//! `normalize` total on a String this engine can hold.

use crate::unicode_normalize_table::{
    CANONICAL_DECOMPOSITION, COMBINING_CLASS, COMPATIBILITY_DECOMPOSITION, COMPOSITION,
};

/// The base of the Hangul syllable block — UAX #15's `SBase`.
const HANGUL_SYLLABLE_BASE: u32 = 0xAC00;
/// `LBase`, `VBase` and `TBase`: the leading, vowel and trailing jamo blocks.
const HANGUL_LEADING_BASE: u32 = 0x1100;
const HANGUL_VOWEL_BASE: u32 = 0x1161;
const HANGUL_TRAILING_BASE: u32 = 0x11A7;
/// `VCount`, `TCount` and `NCount` — how many of each, and the size of one leading jamo's block.
const HANGUL_VOWEL_COUNT: u32 = 21;
const HANGUL_TRAILING_COUNT: u32 = 28;
const HANGUL_BLOCK: u32 = HANGUL_VOWEL_COUNT * HANGUL_TRAILING_COUNT;
/// `SCount` — every syllable there is.
const HANGUL_SYLLABLE_COUNT: u32 = 19 * HANGUL_BLOCK;

/// Which of §22.1.3.13's four forms — two independent decisions rather than four cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Form {
    /// Whether compatibility mappings apply as well as canonical ones — the `K` in the name.
    pub(crate) compatibility: bool,
    /// Whether canonical composition runs after decomposing — the `C` rather than the `D`.
    pub(crate) compose: bool,
}

impl Form {
    /// The form `name` spells, or `None` for anything §22.1.3.13 step 5 refuses.
    ///
    /// Exactly four spellings and they are case-sensitive: step 5 compares against a List of
    /// Strings, so `"nfc"` is a RangeError and not a synonym.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "NFC" => Some(Self {
                compatibility: false,
                compose: true,
            }),
            "NFD" => Some(Self {
                compatibility: false,
                compose: false,
            }),
            "NFKC" => Some(Self {
                compatibility: true,
                compose: true,
            }),
            "NFKD" => Some(Self {
                compatibility: true,
                compose: false,
            }),
            _ => None,
        }
    }
}

/// The Canonical_Combining_Class of a code point — zero for anything the table does not name.
///
/// Zero means **starter**, which is the property both later steps turn on: ordering may not move a
/// character across one, and composition only ever attaches to one.
fn combining_class(code: u32) -> u8 {
    COMBINING_CLASS
        .binary_search_by_key(&code, |(at, _)| *at)
        .ok()
        .and_then(|at| COMBINING_CLASS.get(at).map(|(_, class)| *class))
        .unwrap_or(0)
}

/// The decomposition of one code point under `form`, or nothing if it does not decompose.
fn decomposition(code: u32, form: Form) -> Option<&'static [u32]> {
    let table = match form.compatibility {
        true => COMPATIBILITY_DECOMPOSITION,
        false => CANONICAL_DECOMPOSITION,
    };
    table
        .binary_search_by_key(&code, |(at, _)| *at)
        .ok()
        .and_then(|at| table.get(at).map(|(_, parts)| *parts))
}

/// UAX #15's Hangul decomposition, which is arithmetic rather than a table.
fn decompose_hangul(code: u32, into: &mut Vec<u32>) -> bool {
    let Some(index) = code.checked_sub(HANGUL_SYLLABLE_BASE) else {
        return false;
    };
    if index >= HANGUL_SYLLABLE_COUNT {
        return false;
    }
    into.push(HANGUL_LEADING_BASE + index / HANGUL_BLOCK);
    into.push(HANGUL_VOWEL_BASE + (index % HANGUL_BLOCK) / HANGUL_TRAILING_COUNT);
    let trailing = index % HANGUL_TRAILING_COUNT;
    // A syllable with no trailing jamo decomposes to two, not to three with a placeholder.
    if trailing != 0 {
        into.push(HANGUL_TRAILING_BASE + trailing);
    }
    true
}

/// Steps 1 and 2 — decompose everything, then put the combining marks in canonical order.
fn decompose(points: &[u32], form: Form) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(points.len());
    for point in points {
        if decompose_hangul(*point, &mut out) {
            continue;
        }
        match decomposition(*point, form) {
            Some(parts) => out.extend_from_slice(parts),
            None => out.push(*point),
        }
    }
    order_canonically(&mut out);
    out
}

/// The canonical ordering algorithm — a stable sort of each run of non-starters by combining class.
///
/// Written as the insertion sort UAX #15 describes rather than as a sort over the whole string,
/// because the ordering is **per run**: a starter is a wall that nothing may move across, and a
/// single sort of everything would reorder marks around one.
fn order_canonically(points: &mut [u32]) {
    for at in 1..points.len() {
        let class = combining_class(points[at]);
        if class == 0 {
            continue;
        }
        let mut here = at;
        while here > 0 {
            let before = combining_class(points[here - 1]);
            // Stop at a starter, and at anything that already sorts no later than this one —
            // which is what makes the sort stable and keeps equal classes in written order.
            if before == 0 || before <= class {
                break;
            }
            points.swap(here - 1, here);
            here -= 1;
        }
    }
}

/// Whether `starter` and `second` are a primary composite, and which.
fn composed(starter: u32, second: u32) -> Option<u32> {
    // Hangul first, for the reason it is absent from the table: it is arithmetic.
    if let Some(leading) = starter.checked_sub(HANGUL_LEADING_BASE)
        && leading < 19
        && let Some(vowel) = second.checked_sub(HANGUL_VOWEL_BASE)
        && vowel < HANGUL_VOWEL_COUNT
    {
        return Some(
            HANGUL_SYLLABLE_BASE + (leading * HANGUL_VOWEL_COUNT + vowel) * HANGUL_TRAILING_COUNT,
        );
    }
    if let Some(index) = starter.checked_sub(HANGUL_SYLLABLE_BASE)
        && index < HANGUL_SYLLABLE_COUNT
        && index % HANGUL_TRAILING_COUNT == 0
        && let Some(trailing) = second.checked_sub(HANGUL_TRAILING_BASE)
        && trailing < HANGUL_TRAILING_COUNT
        && trailing > 0
    {
        return Some(starter + trailing);
    }
    COMPOSITION
        .binary_search_by(|(first, next, _)| (*first, *next).cmp(&(starter, second)))
        .ok()
        .and_then(|at| COMPOSITION.get(at).map(|(_, _, made)| *made))
}

/// Step 3 — canonical composition, which NFC and NFKC run over the decomposed form.
///
/// The rule that is easy to get wrong is **blocking**: a character can only compose with the last
/// starter before it when nothing between them has a combining class greater than or equal to its
/// own. Without that, `A` + `ring above` + `acute` would put the ring on and then the acute, where
/// the acute is blocked by the ring and must stay where it is.
fn compose(points: Vec<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(points.len());
    // Where the last starter sits in `out`, and the highest class seen since it.
    let mut starter: Option<usize> = None;
    let mut last_class: Option<u8> = None;
    for point in points {
        let class = combining_class(point);
        if let Some(at) = starter
            && last_class.is_none_or(|seen| seen < class)
            && let Some(made) = out.get(at).and_then(|held| composed(*held, point))
        {
            out[at] = made;
            continue;
        }
        if class == 0 {
            starter = Some(out.len());
            last_class = None;
        } else {
            last_class = Some(class);
        }
        out.push(point);
    }
    out
}

/// §22.1.3.13 step 6 — the normalized form of `points`.
pub(crate) fn normalize(points: &[u32], form: Form) -> Vec<u32> {
    let decomposed = decompose(points, form);
    match form.compose {
        true => compose(decomposed),
        false => decomposed,
    }
}
