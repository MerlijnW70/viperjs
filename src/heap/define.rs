//! `ValidateAndApplyPropertyDescriptor` — §10.1.6.3, in two halves.
//!
//! Split because the specification's own algorithm can accept *without writing*, and a reader
//! should be able to see that. Step 5.e.iii returns `SameValue(propertyDesc.[[Value]],
//! current.[[Value]])` from inside the algorithm, with a NOTE saying why: "SameValue returns true
//! for NaN values which may be distinguishable by other means. Returning here ensures that any
//! existing property of obj remains unmodified."
//!
//! Collapsing that into plain acceptance would write the descriptor's NaN over the property's.
//! Both are NaN, `SameValue` cannot tell them apart, and a `DataView` can.

use crate::heap::{Heap, Property, PropertyDescriptor, PropertyKind};
use crate::value::Value;

/// What [`validate`] concluded — §10.1.6.3's three outcomes, which are not two.
///
/// The interesting one is the middle. §10.1.6.3 step 5.e.iii returns `SameValue(propertyDesc.
/// [[Value]], current.[[Value]])` from inside the algorithm, so a `true` there means *accepted and
/// nothing written*, with a NOTE saying why: "SameValue returns true for NaN values which may be
/// distinguishable by other means. Returning here ensures that any existing property of obj
/// remains unmodified."
///
/// Collapsing that into plain acceptance would write the descriptor's NaN over the property's.
/// Both are NaN and `SameValue` cannot tell them apart, and a `DataView` can. So the outcome is
/// three-valued, and this type is the reason a reader can tell that was deliberate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Validation {
    /// The change is not allowed. `[[DefineOwnProperty]]` answers `false`, and in strict code the
    /// caller turns that into a **TypeError**.
    Reject,
    /// The change is allowed and there is nothing to write.
    AcceptUnchanged,
    /// The change is allowed; apply it.
    Accept,
}

/// `ValidateAndApplyPropertyDescriptor` (§10.1.6.3) — the half that decides.
///
/// Split from the half that writes because the specification's own algorithm can accept without
/// writing, and a reader should be able to see that. The two halves together are the algorithm
/// step for step; see [`Validation`] for the case that makes the split necessary rather than
/// merely tidy.
///
/// `current` absent is the specification's `undefined`: there is no such own property yet.
pub(super) fn validate(
    descriptor: &PropertyDescriptor,
    current: Option<&Property>,
    extensible: bool,
    heap: &Heap,
) -> Validation {
    // Step 2 — a new property. Nothing may be added to a non-extensible object, and anything may
    // be added to an extensible one: a brand-new property has no attributes to be inconsistent
    // with, so no other rule applies.
    let Some(current) = current else {
        return if extensible {
            Validation::Accept
        } else {
            Validation::Reject
        };
    };
    // Step 4 — "if propertyDesc does not have any fields, return true" — is not written here,
    // and its absence changes nothing. It is a shortcut: a descriptor with no fields asks for no
    // change, and every rule below refuses only changes, so it reaches step 6 and [`apply`] puts
    // each attribute back where it was. A branch whose two sides give the same answer for every
    // input is one no test could distinguish, which is the reason it is not written down.
    //
    // Step 5 — everything below is about a non-configurable property. A configurable one may be
    // changed in any way at all, including into the other kind.
    if current.configurable {
        return Validation::Accept;
    }
    // Step 5.a — configurability is one-way.
    if descriptor.configurable == Some(true) {
        return Validation::Reject;
    }
    // Step 5.b — enumerability is frozen too, but only against a *change*: restating it is fine.
    if descriptor
        .enumerable
        .is_some_and(|enumerable| enumerable != current.enumerable)
    {
        return Validation::Reject;
    }
    let current_is_accessor = matches!(current.kind, PropertyKind::Accessor { .. });
    // Step 5.c — the kind cannot change. A generic descriptor is exempt because it names no
    // kind, so it is not asking for a change of one.
    if !descriptor.is_generic_descriptor()
        && descriptor.is_accessor_descriptor() != current_is_accessor
    {
        return Validation::Reject;
    }
    match current.kind {
        // Step 5.d — an accessor's functions are frozen; restating the same ones is allowed.
        PropertyKind::Accessor { getter, setter } => {
            let same = |field: Option<Value>, existing: Value| {
                field.is_none_or(|given| given.same_value(&existing, heap))
            };
            if same(descriptor.getter, getter) && same(descriptor.setter, setter) {
                Validation::Accept
            } else {
                Validation::Reject
            }
        }
        // Step 5.e — a writable data property may still be written to and may still be made
        // non-writable, so only the non-writable case is constrained.
        PropertyKind::Data { value, writable } => {
            if writable {
                return Validation::Accept;
            }
            // Step 5.e.i — non-writable is one-way, like configurable.
            if descriptor.writable == Some(true) {
                return Validation::Reject;
            }
            // Steps 5.e.ii and 5.e.iii — restating the same value is allowed and writes nothing.
            // See [`Validation::AcceptUnchanged`] for the NOTE that requires the second half.
            match descriptor.value {
                Some(given) if !given.same_value(&value, heap) => Validation::Reject,
                Some(_) => Validation::AcceptUnchanged,
                None => Validation::Accept,
            }
        }
    }
}

/// `ValidateAndApplyPropertyDescriptor` (§10.1.6.3) — the half that writes.
///
/// Step 6, which has three shapes: a kind change in either direction keeps the two attributes the
/// descriptor did not mention and takes its own defaults for the rest, and a change within a kind
/// sets only the fields the descriptor has. `current` absent is a new property, which is step 2's
/// "or to the attribute's default value otherwise".
pub(super) fn apply(descriptor: &PropertyDescriptor, current: Option<&Property>) -> Property {
    // Steps 2.c and 2.d — a new property takes §6.1.7.1's default for each field the descriptor
    // does not have. That is what [`PropertyDescriptor::complete`] does, and calling it here and
    // then reading the fields would need a second default for each one, in a place the first had
    // already made unreachable. The defaults are written once, here.
    let Some(current) = current else {
        return Property {
            kind: if descriptor.is_accessor_descriptor() {
                PropertyKind::Accessor {
                    getter: descriptor.getter.unwrap_or(Value::Undefined),
                    setter: descriptor.setter.unwrap_or(Value::Undefined),
                }
            } else {
                PropertyKind::Data {
                    value: descriptor.value.unwrap_or(Value::Undefined),
                    writable: descriptor.writable.unwrap_or(false),
                }
            },
            enumerable: descriptor.enumerable.unwrap_or(false),
            configurable: descriptor.configurable.unwrap_or(false),
        };
    };
    // Steps 6.a.i and 6.b.i — the two attributes both kinds share survive a change of kind unless
    // the descriptor replaces them.
    let enumerable = descriptor.enumerable.unwrap_or(current.enumerable);
    let configurable = descriptor.configurable.unwrap_or(current.configurable);
    let kind = match (descriptor.is_accessor_descriptor(), current.kind) {
        // Step 6.a — data becomes accessor. The old value is *gone*, not remembered: the new
        // property takes `undefined` for whichever accessor the descriptor did not name.
        (true, PropertyKind::Data { .. }) => PropertyKind::Accessor {
            getter: descriptor.getter.unwrap_or(Value::Undefined),
            setter: descriptor.setter.unwrap_or(Value::Undefined),
        },
        // Step 6.b — accessor becomes data, and symmetrically.
        (false, PropertyKind::Accessor { .. }) if descriptor.is_data_descriptor() => {
            PropertyKind::Data {
                value: descriptor.value.unwrap_or(Value::Undefined),
                writable: descriptor.writable.unwrap_or(false),
            }
        }
        // Step 6.c — no change of kind, so each field the descriptor has replaces its attribute
        // and each field it lacks leaves the attribute alone.
        (_, PropertyKind::Accessor { getter, setter }) => PropertyKind::Accessor {
            getter: descriptor.getter.unwrap_or(getter),
            setter: descriptor.setter.unwrap_or(setter),
        },
        (_, PropertyKind::Data { value, writable }) => PropertyKind::Data {
            value: descriptor.value.unwrap_or(value),
            writable: descriptor.writable.unwrap_or(writable),
        },
    };
    Property {
        kind,
        enumerable,
        configurable,
    }
}
