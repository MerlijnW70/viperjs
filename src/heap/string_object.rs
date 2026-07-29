//! §10.4.3 — the String exotic object, and the properties it has without storing any.
//!
//! # What is exotic about it
//!
//! `new String("abc")` has four own properties — `0`, `1`, `2` and `length` — and only one of them
//! is written down. The three indices are answered from the characters every time they are asked
//! for, which is why
//!
//! ```text
//! var s = new String("abc"); s[0] = "z"; s[0]
//! ```
//!
//! still answers `"a"`. There is nowhere to put the `"z"`: §10.4.3.5 gives every index a property
//! that is not writable and not configurable, so the assignment is refused and the character stands.
//!
//! # Why they are computed and not stored
//!
//! Because they cannot change and there can be a great many of them. A String object's characters
//! are fixed when it is made — nothing in the language can alter one — so a stored property could
//! only ever hold what this function computes. Storing them would make `new String(s)` cost a
//! property per character of `s`, which for a string near [`crate::heap::MAX_STRING_LENGTH`] is an
//! allocation the program did not ask for.
//!
//! # Where else this is used
//!
//! §7.3.2 says a property read from a *primitive* string reads it from the object the primitive
//! stands for. Rather than build that object and throw it away on every `"abc".length`, the reader
//! in [`crate::vm`] asks these same two functions about the characters directly. One definition of
//! what a String has, consulted from both sides, is what keeps `"abc"[0]` and `new String("abc")[0]`
//! from being able to disagree.

use crate::heap::{Heap, Property, PropertyDescriptor, PropertyKey, PropertyKind, StringId};
use crate::value::Value;

/// §10.4.3.5 `StringGetOwnProperty` — the property a string's characters give a key, if any.
///
/// `None` for `length`, which is an ordinary stored property put there when the object is made, and
/// for every key that is not an index inside the string. An index *outside* it is `None` too, which
/// is how `"ab"[5]` becomes `undefined` by the ordinary route of finding nothing anywhere.
pub(crate) fn character(heap: &Heap, data: StringId, key: PropertyKey) -> Option<Property> {
    let index = key.as_array_index(heap)?;
    let units = heap.string(data)?;
    let unit = *units.get(index as usize)?;
    // Interned when the object was made — see [`Heap::new_string_object`], which is the only way
    // one comes into existence. A miss here would make a character read as `undefined`, so the
    // interning is the invariant this depends on rather than a cache it hopes for.
    let value = heap.find_string(&[unit])?;
    Some(Property {
        // §10.4.3.5 step 8 — writable and configurable are both false, and this is the whole
        // reason a String object's characters cannot be assigned over or deleted.
        kind: PropertyKind::Data {
            value: Value::String(value),
            writable: false,
        },
        enumerable: true,
        configurable: false,
    })
}

/// The one-character string at `index`, made if this heap has not got one already.
///
/// Separate from [`character`] because that answers a question and this changes the heap: a reader
/// holding a shared borrow can ask what a character *is* only if the answer already exists, and
/// this is what makes sure it does.
pub(crate) fn intern_character(heap: &mut Heap, data: StringId, index: u32) -> Option<StringId> {
    let unit = *heap.string(data)?.get(index as usize)?;
    Some(heap.intern(&[unit]))
}

/// How many characters a String object's data has.
pub(crate) fn length(heap: &Heap, data: StringId) -> usize {
    heap.string(data).map_or(0, <[u16]>::len)
}

/// Whether a define at a character's key is allowed — §10.4.3.3, and nothing is ever stored.
///
/// §10.1.6.3 `ValidateAndApplyPropertyDescriptor` in the one shape it can take here. The current
/// property is fixed: a data property, not writable, enumerable, not configurable. Everything that
/// rule does with a *configurable* current property is unreachable, so what is left is the short
/// list of ways to describe the property that is already there — and a define that describes it is
/// allowed and changes nothing, which is why there is no branch here that writes.
pub(crate) fn define_is_allowed(
    heap: &Heap,
    current: &Property,
    descriptor: &PropertyDescriptor,
) -> bool {
    // Step 4 — a non-configurable data property cannot become an accessor.
    if descriptor.getter.is_some() || descriptor.setter.is_some() {
        return false;
    }
    // Steps 4.a and 6.a — nor gain writability, lose enumerability, or become configurable.
    if descriptor.writable == Some(true)
        || descriptor.enumerable == Some(false)
        || descriptor.configurable == Some(true)
    {
        return false;
    }
    // Step 6.b — and its value may only be set to the one it already has.
    let (Some(asked), PropertyKind::Data { value, .. }) = (descriptor.value, current.kind) else {
        return true;
    };
    asked.same_value(&value, heap)
}
