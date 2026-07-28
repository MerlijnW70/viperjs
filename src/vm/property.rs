//! §10.1's internal methods, as a running program reaches them.
//!
//! `[[Get]]`, `[[Set]]`, `[[Delete]]` and `[[HasProperty]]` — the four that a property access
//! compiles to. They live with the interpreter rather than with [`crate::heap::Object`] for one
//! reason: each may **throw**, and what a throw is made of belongs to a realm. The heap's own
//! `define_own_property` answers a Boolean and needs none of that.

use super::Vm;
use crate::heap::{Heap, PropertyDescriptor, PropertyKey, PropertyKind};
use crate::value::{Completion, TypeError, Value};

impl Vm {
    /// `ToPropertyKey` (§7.1.19), for the keys that exist.
    ///
    /// A Symbol is a key as it stands; everything else becomes the String `ToString` writes, which
    /// is why `o[1]` and `o["1"]` are one property and `o[1.0]` is the same one again.
    pub(super) fn property_key(&self, key: Value, heap: &mut Heap) -> Completion<PropertyKey> {
        let id = key.to_string(heap)?;
        Ok(PropertyKey::from_string(heap, id))
    }
    /// `[[Get]]` (§10.1.8) — the value of `base`'s `key`, its prototypes included.
    ///
    /// A base that is not an object is a **TypeError**. That is right for `null` and `undefined`
    /// and is *temporary* for everything else: §7.3.2 wraps a primitive in its own object first,
    /// so `"abc".length` works by way of `String.prototype` — and there is no `String.prototype`
    /// yet. The message says "an object" rather than naming the type, so it does not have to
    /// change when that arrives.
    pub(crate) fn get_property(
        &self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(TypeError(
                "cannot read a property of something that is not an object",
            ));
        };
        let key = self.property_key(key, heap)?;
        // §10.1.8.1 step 3 — a property that is nowhere on the chain is `undefined`, not an
        // error. That is the whole reason `o.missing` is a value and `missing` is a ReferenceError.
        let Some((_, property)) = heap.find_own(object, key) else {
            return Ok(Value::Undefined);
        };
        match property.kind {
            PropertyKind::Data { value, .. } => Ok(value),
            // §10.1.8.1 steps 5 and 6 — an accessor with no getter answers `undefined`, and one
            // with a getter has it called. Nothing is callable yet, so the second is a TypeError
            // for whatever was put there; both are reachable by defining the property directly.
            PropertyKind::Accessor {
                getter: Value::Undefined,
                ..
            } => Ok(Value::Undefined),
            PropertyKind::Accessor { .. } => Err(TypeError("a getter is not callable")),
        }
    }
    /// `[[Set]]` (§10.1.9) — put `value` under `key`, and answer whether it was allowed.
    ///
    /// The Boolean is thrown away by sloppy code and turned into a TypeError by strict code, which
    /// is why this answers rather than throwing: the caller knows which it is and this does not.
    pub(crate) fn set_property(
        &self,
        base: Value,
        key: Value,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(TypeError(
                "cannot set a property of something that is not an object",
            ));
        };
        let key = self.property_key(key, heap)?;
        // §10.1.9.2 — an *inherited* accessor is called, and an inherited non-writable data
        // property refuses the write. An inherited writable one does not: the value is filed on
        // the receiver, which is what makes a prototype's property shadowable.
        if let Some((owner, property)) = heap.find_own(object, key) {
            match property.kind {
                PropertyKind::Accessor {
                    setter: Value::Undefined,
                    ..
                } => {
                    return Ok(Value::Boolean(false));
                }
                PropertyKind::Accessor { .. } => {
                    return Err(TypeError("a setter is not callable"));
                }
                PropertyKind::Data {
                    writable: false, ..
                } => {
                    return Ok(Value::Boolean(false));
                }
                PropertyKind::Data { .. } if owner == object => {
                    // An own writable data property is changed in place, keeping its attributes:
                    // assignment never makes a property enumerable that was not.
                    let descriptor = PropertyDescriptor {
                        value: Some(value),
                        ..PropertyDescriptor::EMPTY
                    };
                    return Ok(Value::Boolean(heap.define_own_property(
                        object,
                        key,
                        &descriptor,
                    )));
                }
                PropertyKind::Data { .. } => {}
            }
        }
        // A new property, or one that shadows an inherited writable one. Either way it is created
        // on the receiver with the three attributes assignment always gives.
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        Ok(Value::Boolean(heap.define_own_property(
            object,
            key,
            &descriptor,
        )))
    }
    /// `[[Delete]]` (§10.1.10) through §13.5.1's operator.
    pub(crate) fn delete_property(
        &self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(TypeError(
                "cannot delete a property of something that is not an object",
            ));
        };
        let key = self.property_key(key, heap)?;
        // Own only: `delete` never reaches through a prototype, which is why deleting an
        // inherited property answers `true` and leaves it exactly where it was.
        let gone = heap
            .object_mut(object)
            .is_some_and(|found| found.delete(key));
        Ok(Value::Boolean(gone))
    }
    /// `[[HasProperty]]` (§10.1.7) through §13.10.1's `in`.
    pub(crate) fn has_property(
        &self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            // §13.10.1 step 5 — `in` is the one operator that names the requirement out loud
            // rather than converting: `1 in 2` is a TypeError and not `false`.
            return Err(TypeError("the right operand of in must be an object"));
        };
        let key = self.property_key(key, heap)?;
        Ok(Value::Boolean(heap.has_property(object, key)))
    }
}
