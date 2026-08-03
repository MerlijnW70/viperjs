//! §10.5's other seven internal methods — the ones about an object's *shape* rather than its
//! values.
//!
//! # Why these are separate from the four in [`crate::vm::proxy`]
//!
//! `[[Get]]`, `[[Set]]`, `[[HasProperty]]` and `[[Delete]]` are asked constantly and each has one
//! invariant to check. The seven here are asked rarely — by `Object.keys`, `for...in`,
//! `instanceof`, `Object.freeze` — and their invariants are the long ones. §10.5.11's
//! `[[OwnPropertyKeys]]` alone is a dozen steps of set arithmetic, because a list of keys can lie
//! in more ways than a single value can: by omitting a key the target cannot lose, by inventing one
//! on a target nothing may be added to, or by naming the same key twice.
//!
//! # What "the trap may lie" means here
//!
//! For a value, the rule is that a fixed property reads as itself. For a shape, it is stronger: a
//! non-extensible target's key list is *exactly* determined, so the trap must reproduce it — no
//! more and no fewer. That is the promise `Object.freeze` makes, and a proxy may not break it.

use super::proxy::Trapped;
use crate::heap::{
    DefineOutcome, Heap, ObjectId, Property, PropertyDescriptor, PropertyKey, PropertyKind,
};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

impl Vm {
    /// §10.5.1 `[[GetPrototypeOf]]` through a proxy.
    ///
    /// The outer `Option` is "not a proxy"; the inner one is the prototype, which may be null.
    pub(crate) fn proxy_prototype(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Option<Option<ObjectId>>> {
        let Some(trapped) = self.proxy_trap(object, "getPrototypeOf", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return Ok(Some(self.prototype_through(target, heap)?));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let answer = self.call_value(trap, handler, &[Value::Object(target)], heap)?;
        // Step 7 — a prototype is an object or null and nothing else. `undefined` is the common
        // mistake (a trap that forgot to return), and the specification refuses it rather than
        // reading it as null.
        let answered = match answer {
            Value::Object(id) => Some(id),
            Value::Null => None,
            _ => {
                return Err(Abrupt::type_error(
                    "a proxy getPrototypeOf trap answered something that is not an object or null",
                ));
            }
        };
        // Step 8 — an extensible target's prototype may be reported as anything, because it could
        // genuinely become that. A non-extensible one's is fixed, and so is what the trap may say.
        if self.extensible_through(target, heap)? {
            return Ok(Some(answered));
        }
        if answered != self.prototype_through(target, heap)? {
            return Err(Abrupt::type_error(
                "a proxy getPrototypeOf trap answered a prototype the target cannot have",
            ));
        }
        Ok(Some(answered))
    }

    /// §10.5.2 `[[SetPrototypeOf]]` through a proxy.
    pub(crate) fn proxy_set_prototype(
        &mut self,
        object: ObjectId,
        prototype: Option<ObjectId>,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "setPrototypeOf", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return Ok(Some(self.set_prototype_through(target, prototype, heap)?));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = prototype.map_or(Value::Null, Value::Object);
        let answer = self.call_value(trap, handler, &[Value::Object(target), named], heap)?;
        if !answer.to_boolean(heap) {
            return Ok(Some(false));
        }
        // Step 8 — the same rule as the getter's, from the other side: a trap may report that it
        // moved a non-extensible target's prototype only if it moved it to where it already is.
        if self.extensible_through(target, heap)? {
            return Ok(Some(true));
        }
        if prototype != self.prototype_through(target, heap)? {
            return Err(Abrupt::type_error(
                "a proxy setPrototypeOf trap reported moving a prototype that cannot move",
            ));
        }
        Ok(Some(true))
    }

    /// §10.5.3 `[[IsExtensible]]` through a proxy.
    pub(crate) fn proxy_extensible(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "isExtensible", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => return Ok(Some(self.extensible_through(target, heap)?)),
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let answer = self.call_value(trap, handler, &[Value::Object(target)], heap)?;
        // Step 9 — this trap has no freedom at all: it must agree with the target. It exists so a
        // program can *observe* the question being asked, not so it can answer differently.
        if answer.to_boolean(heap) != self.extensible_through(target, heap)? {
            return Err(Abrupt::type_error(
                "a proxy isExtensible trap disagreed with its target",
            ));
        }
        Ok(Some(answer.to_boolean(heap)))
    }

    /// §10.5.4 `[[PreventExtensions]]` through a proxy.
    pub(crate) fn proxy_prevent(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "preventExtensions", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                if let Some(found) = heap.object_mut(target) {
                    found.prevent_extensions();
                }
                return Ok(Some(true));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let answer = self.call_value(trap, handler, &[Value::Object(target)], heap)?;
        if !answer.to_boolean(heap) {
            return Ok(Some(false));
        }
        // Step 8 — claiming success while the target is still extensible would make
        // `Object.isExtensible` and `Object.preventExtensions` disagree about the same object.
        if self.extensible_through(target, heap)? {
            return Err(Abrupt::type_error(
                "a proxy preventExtensions trap reported success on a target that is still extensible",
            ));
        }
        Ok(Some(true))
    }

    /// §10.5.5 `[[GetOwnProperty]]` through a proxy.
    ///
    /// The outer `Option` is "not a proxy"; the inner one is the property, which may be absent.
    pub(crate) fn proxy_own_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Option<Option<Property>>> {
        let Some(trapped) = self.proxy_trap(object, "getOwnPropertyDescriptor", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return Ok(Some(self.own_property_through(target, key, heap)?));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        let answer = self.call_value(trap, handler, &[Value::Object(target), named], heap)?;
        // Step 6 — the answer's *type* is judged before the target is consulted at all, so a trap
        // that answers a number is refused without `[[GetOwnProperty]]` ever being asked of the
        // target. The order is observable whenever the target is itself a proxy.
        if !matches!(answer, Value::Object(_) | Value::Undefined) {
            return Err(Abrupt::type_error(
                "a proxy getOwnPropertyDescriptor trap answered neither a descriptor nor undefined",
            ));
        }
        let held = self.own_property_through(target, key, heap)?;
        // Step 8 — an *absent* answer. §10.5.5 refuses two ways of lying about absence: a property
        // the target cannot delete is still there, and a non-extensible target's key list is fixed
        // so a property it has cannot be reported missing. `IsExtensible` is asked at step 8.c,
        // *after* the two cheaper answers, so a trap on it does not run for an absent property.
        if matches!(answer, Value::Undefined) {
            let Some(held) = held else {
                return Ok(Some(None));
            };
            if !held.configurable {
                return Err(Abrupt::type_error(
                    "a proxy getOwnPropertyDescriptor trap hid a property the target cannot lose",
                ));
            }
            if self.extensible_through(target, heap)? {
                return Ok(Some(None));
            }
            return Err(Abrupt::type_error(
                "a proxy getOwnPropertyDescriptor trap hid a property of a non-extensible target",
            ));
        }
        let extensible = self.extensible_through(target, heap)?;
        let described = crate::builtins::object::to_property_descriptor(self, heap, answer)?;
        // §6.2.6.6 `CompletePropertyDescriptor` — the trap may answer a partial descriptor, and
        // the missing fields are the defaults a *fresh* property would have, not the target's.
        let complete = Self::complete(&described);
        if !heap.is_compatible_descriptor(&described, held.as_ref(), extensible) {
            return Err(Abrupt::type_error(
                "a proxy getOwnPropertyDescriptor trap described a property the target could not have",
            ));
        }
        // Step 17 — a trap may only call a property permanent if the target's really is. Otherwise
        // a program could freeze a property that the target is free to change underneath it.
        if !complete.configurable {
            match held {
                None => {
                    return Err(Abrupt::type_error(
                        "a proxy getOwnPropertyDescriptor trap reported a non-configurable property the target does not have",
                    ));
                }
                Some(held) if held.configurable => {
                    return Err(Abrupt::type_error(
                        "a proxy getOwnPropertyDescriptor trap reported a configurable property as non-configurable",
                    ));
                }
                Some(held) => {
                    if matches!(described.writable, Some(false))
                        && matches!(held.kind, PropertyKind::Data { writable: true, .. })
                    {
                        return Err(Abrupt::type_error(
                            "a proxy getOwnPropertyDescriptor trap reported a writable property as non-writable",
                        ));
                    }
                }
            }
        }
        Ok(Some(Some(complete)))
    }

    /// §6.2.6.6 `CompletePropertyDescriptor` — a partial descriptor as the property it describes.
    ///
    /// Absent fields are `undefined` and `false`, which is what a property created from an empty
    /// descriptor would have. A descriptor mentioning neither a value nor an accessor is a data
    /// property holding `undefined`: §6.2.6.6 step 4 treats generic and data descriptors alike.
    fn complete(descriptor: &PropertyDescriptor) -> Property {
        let accessor = descriptor.getter.is_some() || descriptor.setter.is_some();
        let kind = if accessor {
            PropertyKind::Accessor {
                getter: descriptor.getter.unwrap_or(Value::Undefined),
                setter: descriptor.setter.unwrap_or(Value::Undefined),
            }
        } else {
            PropertyKind::Data {
                value: descriptor.value.unwrap_or(Value::Undefined),
                writable: descriptor.writable.unwrap_or(false),
            }
        };
        Property {
            kind,
            enumerable: descriptor.enumerable.unwrap_or(false),
            configurable: descriptor.configurable.unwrap_or(false),
        }
    }

    /// §10.5.6 `[[DefineOwnProperty]]` through a proxy.
    pub(crate) fn proxy_define(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
        heap: &mut Heap,
    ) -> Completion<Option<DefineOutcome>> {
        let Some(trapped) = self.proxy_trap(object, "defineProperty", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return Ok(Some(self.define_through(target, key, descriptor, heap)?));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        // Step 7 — the trap is handed the descriptor as an *object*, so a handler can read the
        // fields the caller actually supplied. That is why this is not `describe`: a partial
        // descriptor must arrive partial, or `defineProperty(p, "x", {value: 1})` would look to
        // the trap like a request to make the property non-enumerable too.
        let object_form = crate::builtins::object::from_property_descriptor(
            heap,
            &self.realm().clone(),
            descriptor,
        );
        let answer = self.call_value(
            trap,
            handler,
            &[Value::Object(target), named, object_form],
            heap,
        )?;
        if !answer.to_boolean(heap) {
            return Ok(Some(DefineOutcome::Refused));
        }
        let held = self.own_property_through(target, key, heap)?;
        let extensible = self.extensible_through(target, heap)?;
        let making_permanent = matches!(descriptor.configurable, Some(false));
        let Some(held) = held else {
            // Step 16 — nothing may be added to a non-extensible target, and a property that does
            // not exist cannot be made permanent: there would be nothing for the promise to be
            // about.
            if !extensible {
                return Err(Abrupt::type_error(
                    "a proxy defineProperty trap added a property to a non-extensible target",
                ));
            }
            if making_permanent {
                return Err(Abrupt::type_error(
                    "a proxy defineProperty trap reported a non-configurable property the target does not have",
                ));
            }
            return Ok(Some(DefineOutcome::Defined));
        };
        if !heap.is_compatible_descriptor(descriptor, Some(&held), extensible) {
            return Err(Abrupt::type_error(
                "a proxy defineProperty trap accepted a change the target could not have made",
            ));
        }
        if making_permanent && held.configurable {
            return Err(Abrupt::type_error(
                "a proxy defineProperty trap reported a configurable property as non-configurable",
            ));
        }
        // Step 17.c — a non-configurable *writable* property may not be reported as having become
        // non-writable, because non-configurable and non-writable together is the one combination
        // that can never be undone.
        if matches!(descriptor.writable, Some(false))
            && !held.configurable
            && matches!(held.kind, PropertyKind::Data { writable: true, .. })
        {
            return Err(Abrupt::type_error(
                "a proxy defineProperty trap made a permanent property non-writable",
            ));
        }
        Ok(Some(DefineOutcome::Defined))
    }

    /// §10.5.11 `[[OwnPropertyKeys]]` through a proxy.
    pub(crate) fn proxy_own_keys(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Option<Vec<PropertyKey>>> {
        let Some(trapped) = self.proxy_trap(object, "ownKeys", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => return Ok(Some(self.own_keys_through(target, heap)?)),
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let answer = self.call_value(trap, handler, &[Value::Object(target)], heap)?;
        let listed = crate::builtins::function::list_from(
            self,
            heap,
            answer,
            "a proxy ownKeys trap answered something that is not a list",
        )?;
        let mut answered = Vec::with_capacity(listed.len());
        for value in listed {
            // Step 6 — the list may hold only Strings and Symbols. A number in it is a mistake and
            // not a key to be coerced: `ownKeys: () => [0]` is a TypeError, not `["0"]`.
            let key = match value {
                Value::String(text) => PropertyKey::from_string(heap, text),
                Value::Symbol(symbol) => PropertyKey::from_symbol(symbol),
                _ => {
                    return Err(Abrupt::type_error(
                        "a proxy ownKeys trap listed something that is not a property key",
                    ));
                }
            };
            // Step 7 — duplicates. §10.5.11 forbids them outright, because a caller iterating the
            // list would see the same property twice and `Object.keys` is not allowed to do that.
            if answered.contains(&key) {
                return Err(Abrupt::type_error(
                    "a proxy ownKeys trap listed the same key twice",
                ));
            }
            answered.push(key);
        }
        let extensible = self.extensible_through(target, heap)?;
        let held = self.own_keys_through(target, heap)?;
        let mut permanent = Vec::new();
        let mut removable = Vec::new();
        for key in held {
            match self.own_property_through(target, key, heap)? {
                Some(found) if !found.configurable => permanent.push(key),
                _ => removable.push(key),
            }
        }
        // Step 15's shortcut for an extensible target with nothing permanent is not written here.
        // Its absence changes nothing: with `permanent` empty the first loop below does nothing,
        // and step 18 then returns the same list it would have. A branch whose two sides give the
        // same answer for every input is one no test could distinguish.
        let mut unchecked = answered.clone();
        let take = |key: PropertyKey, unchecked: &mut Vec<PropertyKey>| {
            unchecked
                .iter()
                .position(|held| *held == key)
                .map(|at| unchecked.remove(at))
        };
        // Step 17 — every key the target cannot lose must be in the list.
        for key in permanent {
            if take(key, &mut unchecked).is_none() {
                return Err(Abrupt::type_error(
                    "a proxy ownKeys trap omitted a key the target cannot lose",
                ));
            }
        }
        if extensible {
            return Ok(Some(answered));
        }
        // Steps 19 and 20 — a non-extensible target's keys are exactly these, so the list must
        // account for every one of them and invent none.
        for key in removable {
            if take(key, &mut unchecked).is_none() {
                return Err(Abrupt::type_error(
                    "a proxy ownKeys trap omitted a key of a non-extensible target",
                ));
            }
        }
        if !unchecked.is_empty() {
            return Err(Abrupt::type_error(
                "a proxy ownKeys trap invented a key on a non-extensible target",
            ));
        }
        Ok(Some(answered))
    }
}

/// §6.1.7.2's internal methods, asked of anything — a proxy or an ordinary object.
///
/// # Why every one of these exists twice
///
/// The heap answers each of these directly and always could. What it cannot do is ask a *trap*,
/// because a trap is JavaScript. So each operation the language performs on an unknown object now
/// goes through one of these: try the proxy, and fall back to what the heap has always done. The
/// fallback is the ordinary path and costs one `Option` check.
///
/// A `Completion` on operations that never used to fail is the visible price. `Object.keys(o)` can
/// now throw, because `o` may be a proxy whose `ownKeys` trap throws — and that is not an
/// implementation detail leaking out, it is what §10.5 says happens.
impl Vm {
    /// §10.1.1 / §10.5.1 `[[GetPrototypeOf]]`.
    pub(crate) fn prototype_through(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Option<ObjectId>> {
        if let Some(answer) = self.proxy_prototype(object, heap)? {
            return Ok(answer);
        }
        Ok(heap.object(object).and_then(crate::heap::Object::prototype))
    }

    /// §10.1.2 / §10.5.2 `[[SetPrototypeOf]]`.
    pub(crate) fn set_prototype_through(
        &mut self,
        object: ObjectId,
        prototype: Option<ObjectId>,
        heap: &mut Heap,
    ) -> Completion<bool> {
        if let Some(answer) = self.proxy_set_prototype(object, prototype, heap)? {
            return Ok(answer);
        }
        Ok(heap.set_prototype_of(object, prototype))
    }

    /// §10.1.3 / §10.5.3 `[[IsExtensible]]`.
    pub(crate) fn extensible_through(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<bool> {
        if let Some(answer) = self.proxy_extensible(object, heap)? {
            return Ok(answer);
        }
        Ok(heap
            .object(object)
            .is_some_and(crate::heap::Object::is_extensible))
    }

    /// §10.1.4 / §10.5.4 `[[PreventExtensions]]`.
    pub(crate) fn prevent_through(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<bool> {
        if let Some(answer) = self.proxy_prevent(object, heap)? {
            return Ok(answer);
        }
        if let Some(found) = heap.object_mut(object) {
            found.prevent_extensions();
        }
        Ok(true)
    }

    /// §10.1.5 / §10.5.5 `[[GetOwnProperty]]`.
    pub(crate) fn own_property_through(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Option<Property>> {
        if let Some(answer) = self.proxy_own_property(object, key, heap)? {
            return Ok(answer);
        }
        // §10.4.6.5 step 4 — a namespace's descriptor is built from `[[Get]]`, so a binding still in
        // its dead zone makes *asking about the property* a ReferenceError and not only reading it.
        // That is why `Object.keys(ns)` throws inside a cycle: it asks each name whether it is
        // enumerable, and the answer cannot be given without the value.
        if let Some(crate::heap::Export::Uninitialised) = heap.namespace_export(object, key) {
            return Err(crate::value::Abrupt::reference_error(
                "a module binding was read before its module gave it a value",
            ));
        }
        Ok(heap.own_property(object, key))
    }

    /// §10.1.6 / §10.5.6 `[[DefineOwnProperty]]`.
    pub(crate) fn define_through(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
        heap: &mut Heap,
    ) -> Completion<DefineOutcome> {
        // §10.4.6.6 — a namespace accepts a define only when it changes nothing: the descriptor
        // has to match the export exactly, attributes and all. Everything else is refused, which is
        // what makes `Object.defineProperty(ns, "a", {value: 1})` fail even for the value it holds
        // — the descriptor it would have to match is `configurable: false`, and a bare `value` is
        // read as configurable.
        if heap.is_namespace(object) {
            let Some(export) = heap.namespace_export(object, key) else {
                // Not an export, and a namespace has no other data property to redefine. Its
                // `@@toStringTag` is non-configurable, so this refuses that too.
                return Ok(DefineOutcome::Refused);
            };
            let crate::heap::Export::Value(value) = export else {
                return Err(crate::value::Abrupt::reference_error(
                    "a module binding was read before its module gave it a value",
                ));
            };
            let unchanged = descriptor.getter.is_none()
                && descriptor.setter.is_none()
                && descriptor.configurable != Some(true)
                && descriptor.enumerable != Some(false)
                && descriptor.writable != Some(false)
                && descriptor
                    .value
                    .is_none_or(|asked| asked.same_value(&value, heap));
            return Ok(match unchanged {
                true => DefineOutcome::Defined,
                false => DefineOutcome::Refused,
            });
        }

        if let Some(answer) = self.proxy_define(object, key, descriptor, heap)? {
            return Ok(answer);
        }
        Ok(heap.define_property_outcome(object, key, descriptor))
    }

    /// §14.7.5.10's `EnumerateObjectProperties` — the names a `for`-`in` visits.
    ///
    /// The heap's own version of this walk cannot be used once a proxy may be anywhere in the
    /// chain, because every step of it — the keys, each key's attributes, the next prototype — is
    /// an internal method a trap may answer. Shadowing is decided by *name*: a key met on a nearer
    /// object hides the same key further along whether or not the nearer one was enumerable, which
    /// is why the visited set is filled before the enumerable test rather than after it.
    pub(crate) fn enumerable_keys_through(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Vec<PropertyKey>> {
        let mut visited: std::collections::HashSet<PropertyKey> = std::collections::HashSet::new();
        let mut names = Vec::new();
        let mut next = Some(object);
        while let Some(id) = next {
            for key in self.own_keys_through(id, heap)? {
                // §14.7.5.10 — String keys only, which is why `for`-`in` cannot find a Symbol-keyed
                // property. Filtered before the visited set, so a Symbol does not shadow anything.
                if key.as_string().is_none() {
                    continue;
                }
                if !visited.insert(key) {
                    continue;
                }
                if self
                    .own_property_through(id, key, heap)?
                    .is_some_and(|found| found.enumerable)
                {
                    names.push(key);
                }
            }
            next = self.prototype_through(id, heap)?;
        }
        Ok(names)
    }

    /// §10.1.11 / §10.5.11 `[[OwnPropertyKeys]]`.
    pub(crate) fn own_keys_through(
        &mut self,
        object: ObjectId,
        heap: &mut Heap,
    ) -> Completion<Vec<PropertyKey>> {
        if let Some(answer) = self.proxy_own_keys(object, heap)? {
            return Ok(answer);
        }
        Ok(heap.own_property_keys(object))
    }
}
