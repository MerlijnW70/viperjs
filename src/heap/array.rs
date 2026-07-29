//! §10.4.2 — the one exotic object that is not an ordinary one, and why it has to be.
//!
//! # What is exotic about an Array
//!
//! One property: `length`. It is not a count kept beside the elements, it is a *property*, and it
//! and the indices are wired to each other in both directions.
//!
//! - Writing an index at or past `length` raises `length` to one past it. That is why `a[5] = 1`
//!   on an empty array leaves `a.length` as 6 and four holes behind it.
//! - Writing `length` **deletes** every index at or past the new value, from the top down. That is
//!   why `a.length = 0` empties an array, and why it is the idiom for doing so.
//!
//! Neither can be done by an ordinary object, because both happen inside `[[DefineOwnProperty]]`
//! — so §10.4.2 overrides that method and everything else about an Array is ordinary. This module
//! is that override and nothing more.
//!
//! # The half that surprises people
//!
//! Shortening can **fail part way**. §10.4.2.4 deletes from the top down and stops at the first
//! index that refuses — a non-configurable one — leaving `length` at one past *that* index rather
//! than where it was asked to go, and answering `false`. So `a.length = 0` on an array with a
//! frozen element at 3 leaves `length` as 4, and every element above 3 gone. That is not a
//! quirk to be tidied away; it is what the algorithm says, and a test262 test asks.

use super::{DefineOutcome, Heap, ObjectId, PropertyDescriptor, PropertyKey, PropertyKind};
use crate::value::Value;

/// The largest index an Array may hold — §6.1.7's array index is below `2^32 - 1`.
///
/// Not `2^32`: the last value is reserved so that `length` itself always fits in a `u32`. An
/// object may hold a property named `"4294967295"`; it simply is not an *index*, and writing it
/// does not move `length`.
const MAX_INDEX: u32 = u32::MAX - 1;

/// The index a key names, if it names one — §6.1.7's "array index".
///
/// A key is an index only when it is the *canonical* spelling of one: `"01"`, `"1.0"` and `" 1"`
/// are ordinary property names on an array and do not touch its `length`. That is what
/// [`crate::value::canonical_numeric_index`] answers, and asking it here rather than parsing the
/// digits is what keeps the two definitions from drifting.
pub(super) fn array_index(heap: &Heap, key: PropertyKey) -> Option<u32> {
    // A Symbol is no index and has no digits — `as_string` answering `None` is that.
    let units = heap.string(key.as_string()?)?;
    let number = crate::value::canonical_numeric_index(units)?;
    // `is_sign_negative` rather than `< 0.0`, because **`-0.0 < 0.0` is false**. `"-0"` is a
    // canonical numeric index string — §7.1.21 says so outright — and it is *not* an array index,
    // since §6.1.7 asks for `ToString(ToUint32(P)) == P` and `ToUint32("-0")` writes back as
    // `"0"`. Written as a comparison it slipped straight through and `a["-0"] = 1` made an array
    // of length 1, which V8 disagreed with over a hundred generated cases.
    if number.is_sign_negative() || number.fract() != 0.0 {
        return None;
    }
    let index = number as u32;
    match f64::from(index) == number && index <= MAX_INDEX {
        true => Some(index),
        false => None,
    }
}

impl Heap {
    /// §10.4.2.1 `[[DefineOwnProperty]]` for an Array.
    ///
    /// Three cases, and the ordinary one is the last: `length`, an index, and everything else.
    pub(super) fn define_array_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> DefineOutcome {
        let length_key = self.length_key();
        if key == length_key {
            return self.set_array_length(object, descriptor);
        }
        let Some(index) = array_index(self, key) else {
            return DefineOutcome::from(self.define_ordinary_property(object, key, descriptor));
        };
        let (length, writable) = self.array_length(object);
        // §10.4.2.1 step 3.b — an index past the end needs `length` to move, and a `length` that
        // is not writable is what stops it. `Object.freeze` works on an array through this line.
        if index >= length && !writable {
            return DefineOutcome::Refused;
        }
        if !self.define_ordinary_property(object, key, descriptor) {
            return DefineOutcome::Refused;
        }
        if index >= length {
            self.write_length(object, index + 1);
        }
        DefineOutcome::Defined
    }

    /// §10.4.2.4 `ArraySetLength`.
    ///
    /// The one place step 2's rule is written, which is why this answers a [`DefineOutcome`]
    /// rather than a Boolean: a length that is not an integer index is a **RangeError**, and a
    /// Boolean cannot say that. `false` means "not allowed", which sloppy code drops on the
    /// floor — and a bad length must not be dropped.
    fn set_array_length(
        &mut self,
        object: ObjectId,
        descriptor: &PropertyDescriptor,
    ) -> DefineOutcome {
        let Some(value) = descriptor.value else {
            // No value, so only the attributes are being changed — `Object.defineProperty(a,
            // "length", {writable: false})`, which is how an array is made fixed-length.
            let key = self.length_key();
            return DefineOutcome::from(self.define_ordinary_property(object, key, descriptor));
        };
        // §10.4.2.4 step 2 — `ToUint32` and `ToNumber` must agree, or the value is not a length
        // at all. `is_sign_negative` rather than `< 0.0`, for the reason `array_index` gives.
        let Ok(number) = value.to_number(self) else {
            return DefineOutcome::BadLength;
        };
        let wanted = number as u32;
        if f64::from(wanted) != number || number.is_sign_negative() {
            return DefineOutcome::BadLength;
        }
        let (current, writable) = self.array_length(object);
        // §10.4.2.4 step 12 — the comparison is with the *current* length, not a blanket
        // refusal. `a.length = a.length` is allowed on a fixed-length array; anything else is not.
        if !writable && wanted != current {
            return DefineOutcome::Refused;
        }
        // Growing needs no work: the indices between are simply absent, which is what a hole
        // *is*. Written without a `wanted < current` guard in front of it, because there is
        // nothing for the guard to save — no index can be at or above an array's own length, so
        // deleting from `wanted` upwards when growing walks an empty list and answers `wanted`.
        // A guard no input can tell from its absence is one to leave out.
        let reached = self.delete_above(object, wanted);
        let length = PropertyDescriptor {
            value: Some(Value::Number(f64::from(reached))),
            ..*descriptor
        };
        let key = self.length_key();
        let stored = self.define_ordinary_property(object, key, &length);
        // §10.4.2.4 step 17 — a shortening that could not finish is refused even though `length`
        // did move. The array is left in the state the deletions reached, which is a state no
        // other operation can produce.
        DefineOutcome::from(stored && reached == wanted)
    }

    /// Delete every index at or above `floor`, from the top down, and answer where it stopped.
    ///
    /// Downwards because §10.4.2.4 step 15 says so, and the order is observable: it stops at the
    /// first index that refuses, so the elements *below* a frozen one survive and the ones above
    /// it are already gone.
    fn delete_above(&mut self, object: ObjectId, floor: u32) -> u32 {
        let mut indices: Vec<u32> = self
            .object(object)
            .map(|found| found.own_property_keys(self))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|key| array_index(self, key))
            .filter(|index| *index >= floor)
            .collect();
        indices.sort_unstable();
        for index in indices.into_iter().rev() {
            let key = self.index_key(index);
            let deletable = self
                .object(object)
                .and_then(|found| found.get_own_property(key))
                .is_none_or(|property| property.configurable);
            if !deletable {
                return index + 1;
            }
            if let Some(found) = self.object_mut(object) {
                found.delete(key);
            }
        }
        floor
    }

    /// The `length` an array holds, and whether it may be changed.
    ///
    /// Every array has one — it is created with it and it cannot be deleted — so an array without
    /// one is not a state this heap can be in. Answering zero rather than asserting keeps the
    /// method total, and a wrong answer here would be a wrong length rather than a crash.
    fn array_length(&mut self, object: ObjectId) -> (u32, bool) {
        let key = self.length_key();
        let found = self
            .object(object)
            .and_then(|found| found.get_own_property(key));
        match found.map(|property| property.kind) {
            Some(PropertyKind::Data {
                value: Value::Number(length),
                writable,
            }) => (length as u32, writable),
            _ => (0, true),
        }
    }

    /// Put a new `length` in place, keeping its attributes.
    fn write_length(&mut self, object: ObjectId, length: u32) {
        let descriptor = PropertyDescriptor {
            value: Some(Value::Number(f64::from(length))),
            ..PropertyDescriptor::EMPTY
        };
        let key = self.length_key();
        self.define_ordinary_property(object, key, &descriptor);
    }

    /// The interned key `length`, which every array has and every array shares.
    fn length_key(&mut self) -> PropertyKey {
        PropertyKey::from_units(self, &"length".encode_utf16().collect::<Vec<_>>())
    }

    /// The key an index is filed under, which is the decimal spelling and nothing else.
    pub(crate) fn index_key(&mut self, index: u32) -> PropertyKey {
        PropertyKey::from_units(self, &index.to_string().encode_utf16().collect::<Vec<_>>())
    }

    /// Put a new Array on the heap — §10.4.2.2 `ArrayCreate`.
    ///
    /// `length` is writable and neither enumerable nor configurable, which is §10.4.2.2 step 6 and
    /// is what makes `delete a.length` answer `false` on every array in the language.
    pub fn new_array(&mut self, prototype: ObjectId, length: u32) -> ObjectId {
        let object = self.new_object(Some(prototype));
        if let Some(found) = self.object_mut(object) {
            found.array = true;
        }
        let descriptor = PropertyDescriptor {
            value: Some(Value::Number(f64::from(length))),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        let key = self.length_key();
        self.define_ordinary_property(object, key, &descriptor);
        object
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_array_that_somehow_lost_its_length_is_read_as_an_empty_one() {
        // `new_array` gives every array a `length` and §10.4.2.2 makes it non-configurable, so
        // this is a state no program can reach. The *heap* can hold it, and a guard that answered
        // differently would put a wrong length on an array rather than crashing — so it is built
        // by hand here, which is the only way to reach it at all.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        if let Some(found) = heap.object_mut(object) {
            found.array = true;
        }
        // Read as empty and writable, so the first index written behaves exactly as it would on
        // an array made the ordinary way: it lands, and the length follows it.
        let key = PropertyKey::from_units(&mut heap, &"3".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert_eq!(
            heap.define_property_outcome(object, key, &descriptor),
            super::DefineOutcome::Defined
        );
        // …and the `length` that appeared is one `write_length` created rather than one it
        // updated, so it carries §6.1.7.1's defaults instead of §10.4.2.2's. That is only
        // reachable from here: on an array made the ordinary way there is always one to update.
        assert_eq!(heap.array_length(object), (4, false));
    }
}
