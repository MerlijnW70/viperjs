//! §19.2.6 — the four URI functions, and the two operations underneath them.
//!
//! # Why four functions and not two
//!
//! `encodeURI` and `encodeURIComponent` differ in exactly one thing: whether the ten characters
//! §19.2.6.1's grammar calls `uriReserved` — `;/?:@&=+$,` — plus `#` are left alone or escaped.
//! That is not a stylistic choice. Those are the characters that *delimit* the parts of a URI, so
//! a whole URI must keep them and a single component must not: `encodeURI` is for a string that
//! already is a URI, and `encodeURIComponent` is for a string that is about to be one query
//! parameter inside one. Using the first where the second belongs is the bug that lets a `&` in a
//! value invent a parameter, and it is why both exist under such similar names.
//!
//! `decodeURI` and `decodeURIComponent` mirror them, and the mirroring is not symmetry: decoding
//! *preserves* the escapes of the reserved set rather than skipping them, so `decodeURI("%3B")`
//! is the four characters `"%3B"` and not `";"`. Round-tripping is what that buys —
//! `decodeURI(encodeURI(s))` is `s`, because a `;` that was already there stayed a `;` and a
//! `%3B` that was already there stayed a `%3B`. Undoing more than `encodeURI` did would make the
//! two indistinguishable afterwards.
//!
//! # What is being escaped, and what a URIError means
//!
//! Percent-escaping is defined over **octets**, and a JavaScript String is UTF-16 code units, so
//! §19.2.6.5 goes through UTF-8: one code point becomes one to four octets and each octet becomes
//! `%XX`. Both directions can be handed something that is not a UTF-8 encoding of anything, and
//! that is the whole population of `URIError` — the only error type in §20.5.5 that nothing else
//! in the language raises.
//!
//! Encoding fails on an **unpaired surrogate**, because half of a pair is not a code point and
//! there is no UTF-8 for it. Decoding fails on a great deal more, and the reason is RFC 3629
//! rather than ECMA-262: a code point has exactly one UTF-8 encoding, so `%C0%80` — an overlong
//! spelling of NUL — is refused rather than decoded, as is any encoding of a surrogate and
//! anything above `U+10FFFF`. Accepting them is the classic way a filter that checks a decoded
//! string is bypassed by an encoded one, and [`from_utf8`] is where that is refused.
//!
//! # Why the two operations take their sets as bytes
//!
//! Every member of every one of the four sets is ASCII, in both directions, and always will be:
//! §19.2.6.1's grammar is written in ASCII characters and the escape syntax it defines is too.
//! So a set is `&[u8]` and a membership test is a byte comparison — which is also why
//! [`decode`]'s preserve set is checked against the *decoded* octet rather than against a
//! character, since the two are the same thing for everything the set can hold.

use super::define_method;
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The code unit `%`, which is what makes an escape an escape in both directions.
const PERCENT: u16 = 0x25;

/// §19.2.6.1's `uriReserved`, plus `#`.
///
/// The set `encodeURI` adds to what is always unescaped and the set `decodeURI` refuses to
/// decode — the same ten characters and the same `#` doing opposite jobs, which is what makes
/// the two round-trip. §19.2.6.2 and §19.2.6.4 pass an empty set here instead.
const RESERVED: &[u8] = b";/?:@&=+$,#";

/// The `uriMark` production of §19.2.6.1 without `_`, which is an ASCII word character already.
///
/// These are the punctuation characters RFC 2396 declared safe in a URI without escaping, so all
/// four functions leave them alone and no set may add or remove one.
const MARKS: &[u8] = b"-_.!~*'()";

/// Build §19.2.6's four functions onto the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    for (name, native) in [
        ("decodeURI", decode_uri as crate::heap::Native),
        ("decodeURIComponent", decode_uri_component),
        ("encodeURI", encode_uri),
        ("encodeURIComponent", encode_uri_component),
    ] {
        // Every one of the four takes exactly one argument, so §10.3.3's `length` is 1 for all of
        // them and naming it per row would be four chances to write a different number.
        define_method(heap, realm, global, name, 1, native);
    }
}

/// §19.2.6.1 `decodeURI(encodedURI)`.
///
/// Undoes `encodeURI` and nothing more: an escape that spells a character of [`RESERVED`] is left
/// spelled that way, so the result can be encoded again and be the same string.
fn decode_uri(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| decode(units, RESERVED))
}

/// §19.2.6.2 `decodeURIComponent(encodedURIComponent)`.
///
/// Every escape is undone, `%3B` included — which is what makes this the wrong function to point
/// at a whole URI, since it would turn a component's encoded `/` into a path separator.
fn decode_uri_component(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| decode(units, &[]))
}

/// §19.2.6.3 `encodeURI(uri)`.
///
/// For a string that is already a whole URI: the characters that separate its parts survive.
fn encode_uri(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| encode(units, RESERVED))
}

/// §19.2.6.4 `encodeURIComponent(uriComponent)`.
///
/// For a string that is about to become one part of a URI: everything that could be read as a
/// separator is escaped, which is the whole point of the function.
fn encode_uri_component(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    transform(vm, heap, call, |units| encode(units, &[]))
}

/// `ToString` the argument, run `operation` over its units, and put the answer back on the heap.
///
/// The four functions are step 1 and step 3 of one algorithm with a different step 2, and this is
/// steps 1 and 3. Writing it once is not only brevity: the conversion **must** happen before
/// anything is inspected, so `encodeURI({toString(){throw 1}})` throws what the object threw and
/// never reaches a URIError, and four copies of that ordering is four chances to get it wrong.
fn transform(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    operation: impl FnOnce(&[u16]) -> Completion<Vec<u16>>,
) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    let built = operation(&units)?;
    let Some(id) = heap.new_string_checked(built) else {
        return Err(Abrupt::range_error(
            "the resulting string would be too long",
        ));
    };
    Ok(Value::String(id))
}

/// §19.2.6.5 `Encode(string, extraUnescaped)`.
///
/// Walks **code points** rather than code units, which is the only reason the surrogate rules
/// appear at all: a pair is one code point and gets one UTF-8 sequence, and a lone half is not a
/// code point and gets a URIError. Reading unit by unit instead would encode each half of a pair
/// separately and produce the six-octet CESU-8 spelling no decoder here would accept back.
fn encode(units: &[u16], extra: &[u8]) -> Completion<Vec<u16>> {
    let mut built = Vec::with_capacity(units.len());
    let mut at = 0;
    while at < units.len() {
        let unit = units[at];
        // Step 6.b — a character of the unescaped set is copied through as itself, and this is the
        // only place the two encoders differ.
        if unescaped(unit, extra) {
            built.push(unit);
            at += 1;
            continue;
        }
        // Step 6.c.i and 6.c.ii — §11.1.4 `CodePointAt`, whose `[[IsUnpairedSurrogate]]` is the
        // one thing that can fail here.
        let Some((code_point, width)) = code_point_at(units, at) else {
            return Err(Abrupt::uri_error(
                "a lone surrogate is not a code point and cannot be encoded",
            ));
        };
        at += width;
        let (octets, count) = utf8(code_point);
        for octet in &octets[..count] {
            let octet = *octet;
            built.push(PERCENT);
            // Step 6.c.v.1 — **uppercase**, and two digits always. A decoder here would take
            // either case and a leading zero is not optional, so only half of this is a
            // convention: `encodeURIComponent("\u{7}")` is `"%07"` and not `"%7"`.
            built.push(u16::from(hex_upper(octet >> 4)));
            built.push(u16::from(hex_upper(octet & 0x0F)));
        }
    }
    Ok(built)
}

/// Whether `unit` is a character `Encode` copies through — step 4's `unescapedSet`.
///
/// The always-unescaped half is §19.2.6.5 step 3's "the ASCII word characters and `-.!~*'()`",
/// which is exactly §19.2.6.1's `uriUnescaped`; `extra` is what the caller adds. A unit above 255
/// is in neither and does not need asking, which is what the conversion decides.
fn unescaped(unit: u16, extra: &[u8]) -> bool {
    u8::try_from(unit).is_ok_and(|byte| {
        byte.is_ascii_alphanumeric() || MARKS.contains(&byte) || extra.contains(&byte)
    })
}

/// §11.1.4 `CodePointAt` — the code point at `at` and how many units it took, or `None` for an
/// unpaired surrogate.
///
/// Collapsing both of the clause's failure shapes into `None` is deliberate: a leading surrogate
/// with nothing after it and a trailing surrogate on its own are the same thing to every caller
/// here, which is a URIError either way.
fn code_point_at(units: &[u16], at: usize) -> Option<(u32, usize)> {
    let first = u32::from(*units.get(at)?);
    // Step 5 — not a surrogate at all, which is nearly every character of nearly every string.
    if !(0xD800..0xE000).contains(&first) {
        return Some((first, 1));
    }
    // Step 6 — a trailing surrogate here is unpaired by definition, because a pair's trailing half
    // is consumed with its leading one and is never arrived at.
    if first >= 0xDC00 {
        return None;
    }
    let second = u32::from(*units.get(at + 1)?);
    if !(0xDC00..0xE000).contains(&second) {
        return None;
    }
    // Step 9, and §11.1.3 `UTF16SurrogatePairToCodePoint`.
    Some(((first - 0xD800) * 0x400 + (second - 0xDC00) + 0x10000, 2))
}

/// The UTF-8 encoding of `code_point`, as the one to four octets it is and how many that was.
///
/// The four widths are the four ranges and nothing else decides: a code point is encoded in the
/// *shortest* form that fits it, which is what makes the encoding unique and what [`from_utf8`]
/// insists on when reading one back.
///
/// A fixed buffer rather than a `Vec` because this runs once per escaped character, and
/// `encodeURIComponent` of a paragraph of non-ASCII text escapes every character in it.
fn utf8(code_point: u32) -> ([u8; 4], usize) {
    /// The high bits a continuation octet carries, and the six bits of payload under them.
    fn continuation(bits: u32) -> u8 {
        0x80 | (bits & 0x3F) as u8
    }
    match code_point {
        0x0000..=0x007F => ([code_point as u8, 0, 0, 0], 1),
        0x0080..=0x07FF => (
            [
                0xC0 | (code_point >> 6) as u8,
                continuation(code_point),
                0,
                0,
            ],
            2,
        ),
        0x0800..=0xFFFF => (
            [
                0xE0 | (code_point >> 12) as u8,
                continuation(code_point >> 6),
                continuation(code_point),
                0,
            ],
            3,
        ),
        _ => (
            [
                0xF0 | (code_point >> 18) as u8,
                continuation(code_point >> 12),
                continuation(code_point >> 6),
                continuation(code_point),
            ],
            4,
        ),
    }
}

/// One hexadecimal digit of `nibble`, uppercase — step 6.c.v.1's formatting.
fn hex_upper(nibble: u8) -> u8 {
    match nibble < 10 {
        true => b'0' + nibble,
        false => b'A' + (nibble - 10),
    }
}

/// §19.2.6.6 `Decode(string, preserveEscapeSet)`.
///
/// Everything that is not a `%` is copied through untouched — including a `%` that is *part of*
/// what an earlier escape decoded to, since the walk moves past a whole escape sequence at once
/// and never re-reads its own output.
fn decode(units: &[u16], preserve: &[u8]) -> Completion<Vec<u16>> {
    let mut built = Vec::with_capacity(units.len());
    let mut at = 0;
    while at < units.len() {
        let unit = units[at];
        // Step 4.b — anything but a `%` is itself, and that is most of most strings.
        if unit != PERCENT {
            built.push(unit);
            at += 1;
            continue;
        }
        let first = hex_octet(units, at + 1)?;
        // Step 4.c.v — the leading 1 bits of the first octet say how many octets the sequence has,
        // which is the property that makes UTF-8 self-synchronising.
        let width = first.leading_ones() as usize;
        if width == 0 {
            // Step 4.c.vi.2 — a one-octet sequence is an ASCII character, and it is put *back* as
            // the escape it came from when the set says so. Copying the source units rather than
            // re-spelling them keeps `decodeURI("%3b")` lowercase, which round-tripping needs.
            match preserve.contains(&first) {
                true => built.extend_from_slice(&units[at..at + 3]),
                false => built.push(u16::from(first)),
            }
            at += 3;
            continue;
        }
        // Step 4.c.vii.1 — a first octet of `10xxxxxx` is a continuation with nothing to continue,
        // and one of `111110xx` or longer is a width UTF-8 has not had since RFC 3629.
        if width == 1 || width > 4 {
            return Err(Abrupt::uri_error(
                "this is not the first octet of a UTF-8 sequence",
            ));
        }
        // Plain rather than pre-sized: at most three octets go in here, so a capacity hint buys
        // nothing measurable and is arithmetic no program could observe being wrong.
        let mut rest = Vec::new();
        // Step 4.c.vii.5 — every octet after the first must be its own `%XX`, so a sequence
        // announced as three octets and given two raw bytes is refused rather than read.
        for index in 1..width {
            let escape = at + 3 * index;
            if units.get(escape) != Some(&PERCENT) {
                return Err(Abrupt::uri_error(
                    "a UTF-8 sequence is missing one of its escapes",
                ));
            }
            rest.push(hex_octet(units, escape + 1)?);
        }
        let code_point = from_utf8(first, &rest)?;
        // Step 4.c.vii.9 — §11.1.1 `UTF16EncodeCodePoint`, which is where a code point above the
        // basic plane becomes the two units a JavaScript String actually holds.
        match u16::try_from(code_point) {
            Ok(unit) => built.push(unit),
            Err(_) => {
                let offset = code_point - 0x10000;
                built.push(0xD800 + (offset >> 10) as u16);
                built.push(0xDC00 + (offset & 0x3FF) as u16);
            }
        }
        at += 3 * width;
    }
    Ok(built)
}

/// §19.2.6.7 `ParseHexOctet(string, position)` — the two units at `position`, as one octet.
///
/// Refuses rather than reading a shorter escape, which is what makes `decodeURI("%")` and
/// `decodeURI("%A")` URIErrors instead of the characters they look like.
fn hex_octet(units: &[u16], position: usize) -> Completion<u8> {
    let refusal = Abrupt::uri_error("a `%` needs two hexadecimal digits after it");
    let Some(pair) = units.get(position..position + 2) else {
        return Err(refusal);
    };
    match (hex_digit(pair[0]), hex_digit(pair[1])) {
        (Some(high), Some(low)) => Ok(high * 16 + low),
        _ => Err(refusal),
    }
}

/// The value of one hexadecimal digit, in either case — the `HexDigit` production of §12.9.3.
fn hex_digit(unit: u16) -> Option<u8> {
    match unit {
        0x30..=0x39 => Some((unit - 0x30) as u8),
        0x41..=0x46 => Some((unit - 0x41 + 10) as u8),
        0x61..=0x66 => Some((unit - 0x61 + 10) as u8),
        _ => None,
    }
}

/// The code point `octets` spells, or a URIError because they spell nothing.
///
/// Step 4.c.vii.7's "a valid UTF-8 encoding of a Unicode code point" is RFC 3629's definition and
/// not merely "the bits reassemble", which is the whole of this function. Three things are
/// refused that a naive reassembly would accept, and every one of them has been a security
/// advisory somewhere:
///
/// - An **overlong** encoding — `%C0%80` for NUL, `%E0%80%AF` for `/`. A code point has exactly
///   one encoding, so a longer one is not another spelling of it; it is a way to smuggle a
///   character past anything that inspected the escaped form.
/// - A **surrogate** — `%ED%A0%80`. Those code points exist only to encode the other planes in
///   UTF-16 and are not characters, so UTF-8 has no encoding for one.
/// - Anything **above `U+10FFFF`**, which four octets have room for and Unicode does not.
///
/// Takes its first octet apart from the rest because there is no such thing as a sequence with no
/// first octet: written over one slice this would need an arm for the empty case that no call
/// could reach, and an unreachable arm is a branch no test can pin.
fn from_utf8(first: u8, rest: &[u8]) -> Completion<u32> {
    let refusal = Abrupt::uri_error("these octets are not a UTF-8 encoding of any code point");
    let width = 1 + rest.len();
    // The payload of the first octet is what is left under its width marker: five bits of a
    // two-octet sequence, four of a three, three of a four.
    let mut code_point = u32::from(first & (0x7F >> width));
    for octet in rest {
        // Step 4.c.vii.7 again — every octet after the first is `10xxxxxx`, and one that is not
        // ends the sequence early rather than contributing its bits.
        if octet & 0xC0 != 0x80 {
            return Err(refusal);
        }
        code_point = (code_point << 6) | u32::from(octet & 0x3F);
    }
    // The shortest form that fits: two octets carry 11 bits and must use the eighth, three carry
    // 16 and must use the twelfth, four carry 21 and must use the seventeenth.
    let smallest = match width {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    let surrogate = (0xD800..0xE000).contains(&code_point);
    if code_point < smallest || code_point > 0x10FFFF || surrogate {
        return Err(refusal);
    }
    Ok(code_point)
}

/// The one thing about this module a script cannot ask, and why it is asked here instead.
///
/// [`code_point_at`]'s arithmetic is checked directly because [`utf8`] hides it: the four-octet
/// arm casts `code_point >> 18` to a `u8` and then ORs it under `0xF0`, so a code point wrong by
/// certain large multiples encodes to the very same four octets. `0x6C00000` is one such —
/// exactly `432 << 18`, which leaves every observable bit alone — and it is what
/// `first + 0xD800` in place of `first - 0xD800` produces. No `encodeURI` call can tell the two
/// apart, so the contract is pinned where it is stated rather than where it is used.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surrogate_pair_is_the_code_point_the_two_halves_spell() {
        // §11.1.3 `UTF16SurrogatePairToCodePoint` at both ends of the range it covers, and one in
        // the middle that a reader can check against a character they have seen.
        assert_eq!(code_point_at(&[0xD800, 0xDC00], 0), Some((0x10000, 2)));
        assert_eq!(code_point_at(&[0xDBFF, 0xDFFF], 0), Some((0x10FFFF, 2)));
        assert_eq!(code_point_at(&[0xD83D, 0xDE00], 0), Some((0x1F600, 2)));
        // …and a unit that is not part of a pair is itself, in one unit rather than two.
        assert_eq!(code_point_at(&[0x41], 0), Some((0x41, 1)));
        assert_eq!(code_point_at(&[0xFFFF], 0), Some((0xFFFF, 1)));
        // The three shapes that are not a code point: a lone trailing half, a leading half with
        // nothing after it, and a leading half followed by something that is not a trailing one.
        assert_eq!(code_point_at(&[0xDC00], 0), None);
        assert_eq!(code_point_at(&[0xD800], 0), None);
        assert_eq!(code_point_at(&[0xD800, 0x41], 0), None);
        assert_eq!(code_point_at(&[0xD800, 0xD800], 0), None);
    }
}
