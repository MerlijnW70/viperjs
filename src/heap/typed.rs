//! §10.4.5 — the integer-indexed exotic object, which is what makes a TypedArray an array.
//!
//! # What is exotic about it
//!
//! A TypedArray has no stored properties for its elements. `ta[0]` is answered from the buffer
//! every time, and `ta[0] = 1` writes into the buffer rather than making a property — which is why
//! two views over one buffer see each other's writes and why a TypedArray's length can never
//! change.
//!
//! # The canonical numeric string, and why it is not "is this a number"
//!
//! §10.4.5 intercepts a key exactly when it is a **CanonicalNumericIndexString**: a String that
//! `ToString(ToNumber(it))` gives back unchanged. So `"0"` is one and `"00"` is not, `"1e3"` is not
//! but `"1000"` is, and `"-0"` is one — the only one that is not a valid index.
//!
//! That distinction is the whole of the exotic behaviour, and getting it wrong is invisible until
//! someone writes `ta["00"]`. A key that *is* canonical and out of range is **absent**: reading it
//! answers `undefined` without consulting the prototype, and writing it is discarded. A key that is
//! not canonical is an ordinary property, stored and read like any other — so `ta["00"] = 1` really
//! does make a property, and `ta[0] = 1` does not.

use crate::heap::{Element, Heap, ObjectId, Property, PropertyKey, PropertyKind, View};
use crate::value::Value;

/// §7.1.21 `CanonicalNumericIndexString`, as an index into a view of `count` elements.
///
/// Three answers, not two, and the middle one is the one that matters:
///
/// - `None` — not a canonical numeric string at all, so this key is an ordinary property.
/// - `Some(Err(()))` — canonical, and not an index this view has. §10.4.5's operations all treat
///   that as *absent*: the read answers `undefined` and the write is discarded, neither of them
///   consulting the prototype. `ta[99]` on a short array is not a prototype lookup.
/// - `Some(Ok(at))` — an element of this view.
pub(super) fn index_of(heap: &Heap, key: PropertyKey, count: usize) -> Option<Result<usize, ()>> {
    let PropertyKey::String(id) = key else {
        // §7.1.21 step 1 — a Symbol is never a numeric index, so it is always an ordinary property.
        return None;
    };
    let units = heap.string(id)?;
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|found| found.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    // §7.1.21 step 2 — `"-0"` is canonical and is never an index, which is the one case where the
    // two questions genuinely differ. It has to be checked by hand because `ToString(-0)` is `"0"`.
    if text == "-0" {
        return Some(Err(()));
    }
    let Ok(number) = text.parse::<f64>() else {
        // Not a number at all, so not a canonical numeric index — which makes it an ordinary
        // property. Said as a `let else` rather than an `ok()?` because the two `None`s mean the
        // same thing here and one of them would otherwise look like an error being swallowed.
        return None;
    };
    // Step 3 — canonical means `ToString(ToNumber(key))` gives the key back. `"00"`, `"1e3"`,
    // `" 1"` and `"0.0"` all parse and none of them comes back the same, so all four are ordinary
    // properties rather than indices.
    if crate::value::number_to_string(number) != text {
        return None;
    }
    // An index has to be a non-negative integer inside the view. Everything else is canonical and
    // absent — a fraction, a negative, `Infinity`, `NaN`, and anything past the end.
    if number < 0.0 || number.fract() != 0.0 || number >= count as f64 {
        return Some(Err(()));
    }
    Some(Ok(number as usize))
}

impl Heap {
    /// The element at `at` of a view, as the property §10.4.5.1 describes.
    ///
    /// **Writable, enumerable and configurable** — all three, which is not what §10.4.3 gives a
    /// String object's characters. A TypedArray's elements can be written, which is the point of
    /// one; they are enumerable so that `Object.keys` and `for`-`in` list them; and they are
    /// configurable, which was a change in ES2021 and is what lets `Object.defineProperty` be used
    /// on them at all.
    pub(super) fn element_property(&self, view: View, at: usize) -> Option<Property> {
        let element = view.element?;
        let from = view.offset + at * element.width();
        let bytes = self
            .object(view.buffer)?
            .buffer()?
            .bytes()?
            .get(from..from + element.width())?;
        Some(Property {
            kind: PropertyKind::Data {
                value: Value::Number(element.read(bytes)),
                writable: true,
            },
            enumerable: true,
            configurable: true,
        })
    }

    /// Write an element, if there is one there to write.
    ///
    /// Answers nothing, because §10.4.5.5 gives the caller nothing to do with an answer: a write to
    /// an index the view does not have, or to one whose buffer has since been detached, is
    /// **discarded** — in strict mode and sloppy alike. It is the one assignment in the language
    /// that fails silently by design, because a TypedArray's length cannot change and there is
    /// nowhere for the value to go.
    pub(super) fn set_element(&mut self, view: View, at: usize, value: f64, clamped: bool) {
        let Some(element) = view.element else {
            return;
        };
        let from = view.offset + at * element.width();
        let bytes = element.write(clamp_if(value, clamped));
        if let Some(target) = self
            .object_mut(view.buffer)
            .and_then(super::Object::buffer_mut)
            .and_then(super::Buffer::bytes_mut)
            .and_then(|found| found.get_mut(from..from + element.width()))
        {
            target.copy_from_slice(&bytes);
        }
    }

    /// The view this object is, if it is a TypedArray rather than a `DataView` or anything else.
    #[must_use]
    pub fn typed_view(&self, object: ObjectId) -> Option<View> {
        let view = self.object(object)?.view()?;
        view.element.map(|_| view)
    }

    /// The number at `at` of a view, or `None` if there is nothing there.
    #[must_use]
    pub fn element_at(&self, view: View, at: usize) -> Option<f64> {
        match self.element_property(view, at)?.kind {
            PropertyKind::Data {
                value: Value::Number(number),
                ..
            } => Some(number),
            _ => None,
        }
    }

    /// Where `key` points in this object, if it is a TypedArray and `key` is a numeric index.
    ///
    /// Three answers, and the middle one is the one that matters. `None` means this key is an
    /// ordinary property. `Some(Err)` means it is a canonical numeric index the view does not have,
    /// which §10.4.5 treats as *absent*: the read answers `undefined` and the write is discarded,
    /// neither of them consulting the prototype. `Some(Ok)` is an element.
    pub fn typed_index(&self, object: ObjectId, key: PropertyKey) -> Option<Result<usize, ()>> {
        let view = self.typed_view(object)?;
        index_of(self, key, view.count())
    }

    /// Write `value` at an index a lookup already found.
    ///
    /// Nothing happens if the buffer has since been detached, which is what §10.4.5.5 step 1.b.i
    /// means by "return unused": the write is discarded rather than refused, because a TypedArray's
    /// elements are the buffer's bytes and there are none.
    pub fn write_element(&mut self, object: ObjectId, at: usize, value: f64) {
        let Some(view) = self.typed_view(object) else {
            return;
        };
        let clamped = self.object(object).is_some_and(super::Object::is_clamped);
        self.set_element(view, at, value, clamped);
    }
}

/// The eight concrete kinds, with the name each constructor carries — §23.2.5.
///
/// In the order §23.2 lists them, which is by width and then by signedness. `Uint8Clamped` is the
/// odd one and is here rather than in [`Element`] because it differs only in *how a value is
/// written*: it is a `Uint8` that saturates instead of wrapping, and every other operation on it is
/// identical.
pub const KINDS: [(&str, Element, bool); 9] = [
    ("Int8Array", Element::Int8, false),
    ("Uint8Array", Element::Uint8, false),
    ("Uint8ClampedArray", Element::Uint8, true),
    ("Int16Array", Element::Int16, false),
    ("Uint16Array", Element::Uint16, false),
    ("Int32Array", Element::Int32, false),
    ("Uint32Array", Element::Uint32, false),
    ("Float32Array", Element::Float32, false),
    ("Float64Array", Element::Float64, false),
];

/// §7.1.11 `ToUint8Clamp`, applied only where a `Uint8ClampedArray` asks for it.
///
/// Saturating rather than wrapping, and rounding halves to **even** rather than away from zero.
/// Both are what pixel data wants: 300 is "as bright as it gets" rather than 44, and rounding half
/// to even keeps a long run of averages from drifting upwards.
///
/// Here rather than beside the write it modifies, because it is the *only* thing that separates
/// `Uint8ClampedArray` from `Uint8Array` — every read of their bytes is identical, so the whole of
/// the difference between two of the nine kinds is this function.
#[must_use]
pub fn clamp_if(value: f64, clamped: bool) -> f64 {
    if !clamped {
        return value;
    }
    // No case for `NaN`: `f64::clamp` answers `NaN` for one, and every arithmetic below carries it
    // through to the `write` that follows, where §7.1.9's cast turns it into 0 — which is the
    // answer §7.1.11 step 1 asks for. Written out as a case it was a branch nothing could tell
    // from its absence.
    let bounded = value.clamp(0.0, 255.0);
    let floor = bounded.floor();
    match bounded - floor {
        half if half > 0.5 => floor + 1.0,
        half if half < 0.5 => floor,
        // Exactly a half — to *even*, which `f64::round` does not do.
        _ if floor % 2.0 == 0.0 => floor,
        _ => floor + 1.0,
    }
}
