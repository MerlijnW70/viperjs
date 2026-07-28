//! The ordinary object — §10.1, the shape almost every object in a program has.
//!
//! An object is three things: a prototype, a flag saying whether properties may still be added,
//! and a collection of properties. Everything else about §10.1 is rules for changing them.
//!
//! # What is here and what is not
//!
//! Every ordinary internal method that does not reach user code. `[[Get]]` and `[[Set]]` are the
//! two that do — an accessor property's getter is a function, and calling it is the VM's job —
//! so they arrive with the interpreter. `[[HasProperty]]`, `[[Delete]]`, `[[GetOwnProperty]]`,
//! `[[DefineOwnProperty]]`, `[[OwnPropertyKeys]]` and the prototype and extensibility methods are
//! all here, and between them they are what the object model *is*.
//!
//! # Why the properties are a `Vec`
//!
//! Because §10.1.11 asks for insertion order and a `Vec` has it. Lookup is linear, which is the
//! boring implementation and is wrong for an object with a thousand properties — the fix is a map
//! beside the order, or shapes, and both are M8 experiments that need a benchmark first. Nothing
//! in the specification's behaviour depends on which is used, which is exactly why the choice can
//! wait for evidence.

use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, Property, PropertyDescriptor, PropertyKey, PropertyKind};
use crate::value::Value;
use std::rc::Rc;

/// An object on the heap.
///
/// Meaningful only to the [`Heap`] that issued it, on the same terms as [`crate::heap::StringId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(usize);

/// An ordinary object — §10.1.
#[derive(Debug, Default)]
pub struct Object {
    /// `[[Prototype]]` — "an Object or **null**", which is what the `Option` is.
    ///
    /// `None` is `null` and not "not set": an object whose prototype is null is the ordinary
    /// state of `Object.create(null)`, and the chain simply ends there.
    prototype: Option<ObjectId>,
    /// `[[Extensible]]` — whether properties may still be added.
    ///
    /// One-way: §10.1.4 can set it false and nothing sets it back. That is what makes
    /// `Object.preventExtensions` a guarantee rather than a suggestion, and it is why
    /// [`Object::prevent_extensions`] takes no argument.
    extensible: bool,
    /// The body this object runs when it is called — its `[[Call]]` internal method.
    ///
    /// `None` for an ordinary object, which is most of them. An object is *callable* exactly when
    /// this is present, which is the whole of what `typeof f === "function"` and "x is not a
    /// function" are asking about.
    ///
    /// Holding the code here rather than in an arena beside it is deliberate: a function object
    /// is the thing that owns its body, and the `Rc` is what lets a closure outlive the call that
    /// made it. See [`Chunk`] for why reference counting is safe for code where DR-0010 rejects
    /// it for values.
    call: Option<Rc<Chunk>>,
    /// The environment this function was *written* in — §10.2's `[[Environment]]`.
    ///
    /// A closure is this field. The call that made the function is long gone by the time the
    /// function runs, and the variables it could see are still here because this holds them.
    environment: Option<EnvironmentId>,
    /// The own properties, in the order they were created.
    ///
    /// The order is not incidental — §10.1.11 hands out string keys "in ascending chronological
    /// order of property creation", so this `Vec` *is* that answer for part of the result.
    properties: Vec<(PropertyKey, Property)>,
}

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
enum Validation {
    /// The change is not allowed. `[[DefineOwnProperty]]` answers `false`, and in strict code the
    /// caller turns that into a **TypeError**.
    Reject,
    /// The change is allowed and there is nothing to write.
    AcceptUnchanged,
    /// The change is allowed; apply it.
    Accept,
}

impl Object {
    /// An ordinary object with the given prototype, no properties, and extensible.
    ///
    /// `OrdinaryObjectCreate` (§10.1.12) in the part that concerns the object itself. The
    /// prototype is an argument rather than a default because there is no default: an object
    /// literal gets `Object.prototype`, `Object.create(null)` gets nothing, and neither is more
    /// ordinary than the other.
    pub fn new(prototype: Option<ObjectId>) -> Self {
        Self {
            prototype,
            extensible: true,
            call: None,
            environment: None,
            properties: Vec::new(),
        }
    }

    /// `[[GetPrototypeOf]]` (§10.1.1) — the prototype, or `None` for `null`.
    pub fn prototype(&self) -> Option<ObjectId> {
        self.prototype
    }

    /// The body this object runs when called, if it is callable at all.
    pub fn call(&self) -> Option<&Rc<Chunk>> {
        self.call.as_ref()
    }

    /// The environment this function was written in, if it is a function at all.
    pub fn environment(&self) -> Option<EnvironmentId> {
        self.environment
    }

    /// `[[IsExtensible]]` (§10.1.3).
    pub fn is_extensible(&self) -> bool {
        self.extensible
    }

    /// `[[PreventExtensions]]` (§10.1.4) — and there is no way back.
    ///
    /// Always succeeds, which is why it answers nothing: §10.1.4 returns `true` unconditionally.
    /// Existing properties are untouched — this stops *additions*, and a non-extensible object's
    /// configurable properties may still be deleted and redefined.
    pub fn prevent_extensions(&mut self) {
        self.extensible = false;
    }

    /// `[[GetOwnProperty]]` (§10.1.5) — the property filed under `key`, if there is one.
    ///
    /// Own only: nothing here walks the prototype chain, which is the difference between this and
    /// `[[Get]]`, and the difference `Object.hasOwn` exists to expose.
    pub fn get_own_property(&self, key: PropertyKey) -> Option<&Property> {
        self.properties
            .iter()
            .find(|(stored, _)| *stored == key)
            .map(|(_, property)| property)
    }

    /// File `property` under `key`, replacing whatever was there.
    ///
    /// The write half of `[[DefineOwnProperty]]`, and private because it is only correct after
    /// [`validate`] has agreed. A new key goes on the end, which is what makes the `Vec` the
    /// creation order §10.1.11 asks for.
    fn insert(&mut self, key: PropertyKey, property: Property) {
        match self
            .properties
            .iter_mut()
            .find(|(stored, _)| *stored == key)
        {
            Some((_, existing)) => *existing = property,
            None => self.properties.push((key, property)),
        }
    }

    /// `[[Delete]]` (§10.1.10) — remove the own property `key`, if it may be removed.
    ///
    /// A key that is not there answers `true`: deleting nothing succeeds, which is why
    /// `delete o.nothing` is `true` and says nothing about whether `o.nothing` existed.
    pub fn delete(&mut self, key: PropertyKey) -> bool {
        let Some(index) = self
            .properties
            .iter()
            .position(|(stored, _)| *stored == key)
        else {
            return true;
        };
        if !self.properties[index].1.configurable {
            return false;
        }
        self.properties.remove(index);
        true
    }

    /// `[[OwnPropertyKeys]]` (§10.1.11) — every own key, in the order the language guarantees.
    ///
    /// Array indices first in ascending numeric order, then every other String key in the order
    /// its property was created. That is why `{b: 1, 2: 2, a: 3, 1: 4}` enumerates as
    /// `1, 2, b, a`, and it is a guarantee rather than an implementation detail: the ordering was
    /// written into the specification in ES2015 because every engine already did it.
    ///
    /// Note *array* index, not integer index. `"4294967295"` is one too large to be an array
    /// index, so it sorts with the strings — the same boundary [`PropertyKey::as_array_index`]
    /// draws, and observable through this.
    pub fn own_property_keys(&self, heap: &Heap) -> Vec<PropertyKey> {
        let mut indices: Vec<(u32, PropertyKey)> = Vec::new();
        let mut names: Vec<PropertyKey> = Vec::new();
        for (key, _) in &self.properties {
            match key.as_array_index(heap) {
                Some(index) => indices.push((index, *key)),
                None => names.push(*key),
            }
        }
        // Ascending *numeric* order, which is why the index came back as a number: sorting the
        // keys as text would put "10" before "9".
        indices.sort_unstable_by_key(|(index, _)| *index);
        indices
            .into_iter()
            .map(|(_, key)| key)
            .chain(names)
            .collect()
    }

    /// How many own properties there are.
    ///
    /// For tests and for whatever reports on the heap. Counts every own property, enumerable or
    /// not — `Object.keys().length` is a different and smaller number.
    pub fn property_count(&self) -> usize {
        self.properties.len()
    }
}

/// `ValidateAndApplyPropertyDescriptor` (§10.1.6.3) — the half that decides.
///
/// Split from the half that writes because the specification's own algorithm can accept without
/// writing, and a reader should be able to see that. The two halves together are the algorithm
/// step for step; see [`Validation`] for the case that makes the split necessary rather than
/// merely tidy.
///
/// `current` absent is the specification's `undefined`: there is no such own property yet.
fn validate(
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
fn apply(descriptor: &PropertyDescriptor, current: Option<&Property>) -> Property {
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

impl Heap {
    /// Put a function object on the heap — `OrdinaryFunctionCreate` (§10.2.3), in the part that
    /// is about the object rather than about the environment.
    ///
    /// Ordinary in every way but one: it has a `[[Call]]`, which is what makes `typeof` say
    /// `"function"` and what a call expression looks for.
    pub fn new_function(
        &mut self,
        prototype: ObjectId,
        body: Rc<Chunk>,
        environment: EnvironmentId,
    ) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(body);
        object.environment = Some(environment);
        self.objects.push(object);
        id
    }

    /// Put an ordinary object on the heap — `OrdinaryObjectCreate` (§10.1.12).
    pub fn new_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let id = ObjectId(self.objects.len());
        self.objects.push(Object::new(prototype));
        id
    }

    /// The object `id` refers to, or `None` if this heap has nothing there.
    ///
    /// The same narrow promise [`Heap::string`] makes about a foreign handle, for the same
    /// reason: no panic and no out-of-range read, and no detection.
    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0)
    }

    /// The object `id` refers to, to be changed.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id.0)
    }

    /// How many objects this heap holds.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// `[[DefineOwnProperty]]` (§10.1.6) — apply `descriptor` to `object`'s `key`, if the rules
    /// allow it.
    ///
    /// Answers whether it was allowed. It does **not** throw: §10.1.6 returns a Boolean, and
    /// turning a `false` into a TypeError is the caller's decision — `Object.defineProperty`
    /// throws, `Reflect.defineProperty` hands the Boolean back, and an assignment in sloppy code
    /// does neither.
    ///
    /// Here rather than on [`Object`] because §10.1.6.3 compares values with `SameValue`, and two
    /// Strings are the same value when their code units are — which is a question only the heap
    /// can answer. An object cannot hold the heap it lives in, so the operation lives outside and
    /// takes both.
    pub fn define_own_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> bool {
        let Some(found) = self.object(object) else {
            return false;
        };
        // Copied out so the validation below may read the heap: a `Property` is `Copy` precisely
        // so that this costs nothing.
        let current = found.get_own_property(key).copied();
        let extensible = found.is_extensible();
        match validate(descriptor, current.as_ref(), extensible, self) {
            Validation::Reject => false,
            Validation::AcceptUnchanged => true,
            Validation::Accept => {
                let updated = apply(descriptor, current.as_ref());
                // The object was found above and an arena only grows, so this cannot be absent —
                // and the answer does not depend on it. Writing `None => false` here would be a
                // branch no input could take, and one that would report a refusal the rules did
                // not make.
                if let Some(found) = self.object_mut(object) {
                    found.insert(key, updated);
                }
                true
            }
        }
    }

    /// `OrdinaryHasProperty` (§10.1.7.1) — whether `object` or anything it inherits from has `key`.
    ///
    /// Walks the prototype chain, which is why it is here and not on [`Object`]: an object cannot
    /// see its own prototype's properties without the heap they both live in.
    ///
    /// The walk is bounded by the chain being acyclic, which
    /// [`Heap::set_prototype_of`] is what guarantees — and by a step count besides, because a
    /// guarantee that depends on every other path being correct is not one this may rely on.
    pub fn has_property(&self, object: ObjectId, key: PropertyKey) -> bool {
        self.find_own(object, key).is_some()
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// What `[[Get]]` will need once calling exists: the property *and* which object it came
    /// from, since an accessor's getter is called with that object as its receiver.
    pub fn find_own(&self, object: ObjectId, key: PropertyKey) -> Option<(ObjectId, Property)> {
        let mut cursor = Some(object);
        // The chain cannot be a cycle — nothing can build one — and this counts anyway. DR-0002
        // is not a claim about the code being right; it is a claim that being wrong does not
        // hang. See [`Heap::set_prototype_of`] for the check that makes the count unreachable.
        for _ in 0..MAX_PROTOTYPE_CHAIN {
            let current = self.object(cursor?)?;
            if let Some(property) = current.get_own_property(key) {
                return Some((cursor?, *property));
            }
            cursor = current.prototype();
        }
        None
    }

    /// `OrdinarySetPrototypeOf` (§10.1.2) — point `object` at `prototype`, if that is allowed.
    ///
    /// Two rules, and the second is the interesting one.
    ///
    /// A non-extensible object's prototype may not be changed — *changed*, not set: §10.1.2 step
    /// 2 returns `true` for setting it to what it already is, before extensibility is looked at
    /// at all. `Object.preventExtensions(o); Object.setPrototypeOf(o, Object.getPrototypeOf(o))`
    /// succeeds.
    ///
    /// And the chain may not become a cycle. §10.1.2 walks the proposed prototype's own chain
    /// looking for `object`, which is the check that makes every prototype walk in the engine
    /// terminate. The specification notes that this only holds while every object on the chain
    /// uses the ordinary method — a Proxy can lie, which is why the walks are bounded as well.
    pub fn set_prototype_of(&mut self, object: ObjectId, prototype: Option<ObjectId>) -> bool {
        // Step 2 — setting it to what it is always succeeds.
        let Some(current) = self.object(object) else {
            return false;
        };
        if current.prototype() == prototype {
            return true;
        }
        // Step 4.
        if !current.is_extensible() {
            return false;
        }
        // Steps 5 to 7 — walk the proposed chain and refuse if it comes back here.
        let mut cursor = prototype;
        for _ in 0..MAX_PROTOTYPE_CHAIN {
            let Some(id) = cursor else {
                break;
            };
            if id == object {
                return false;
            }
            match self.object(id) {
                Some(found) => cursor = found.prototype(),
                None => break,
            }
        }
        // Step 8. As in [`Heap::define_own_property`], the object was found at the top and the
        // arena only grows, so there is nothing here to fail and no refusal left to report.
        if let Some(found) = self.object_mut(object) {
            found.prototype = prototype;
        }
        true
    }
}

/// How far any prototype walk goes before giving up.
///
/// Not a limit the language has: an acyclic chain of a million objects is legal and this would
/// answer wrongly about it. It is a backstop for DR-0002 — a walk that cannot terminate is a hang,
/// and "the cycle check is correct" is exactly the kind of claim that should not be the only thing
/// standing between an engine and one. Every chain a program actually builds is a handful long;
/// the figure is deliberately far above that and deliberately not a guess about correctness.
const MAX_PROTOTYPE_CHAIN: usize = 100_000;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(heap: &mut Heap, text: &str) -> PropertyKey {
        PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
    }

    /// The own property, through the heap — the tests all hold an `ObjectId` rather than an
    /// `Object`, because that is what everything in the engine will hold.
    fn own(heap: &Heap, object: ObjectId, key: PropertyKey) -> Option<Property> {
        heap.object(object)?.get_own_property(key).copied()
    }

    fn count(heap: &Heap, object: ObjectId) -> usize {
        heap.object(object).map_or(0, Object::property_count)
    }

    fn keys(heap: &Heap, object: ObjectId) -> Vec<PropertyKey> {
        heap.object(object)
            .map_or_else(Vec::new, |found| found.own_property_keys(heap))
    }

    fn prevent(heap: &mut Heap, object: ObjectId) {
        if let Some(found) = heap.object_mut(object) {
            found.prevent_extensions();
        }
    }

    fn delete(heap: &mut Heap, object: ObjectId, key: PropertyKey) -> bool {
        heap.object_mut(object)
            .is_some_and(|found| found.delete(key))
    }

    /// A descriptor for a plain writable, enumerable, configurable data property — what
    /// assignment produces, and what `Object.defineProperty` notably does not.
    fn data(value: f64) -> PropertyDescriptor {
        PropertyDescriptor {
            value: Some(Value::Number(value)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        }
    }

    #[test]
    fn a_new_property_takes_the_defaults_for_every_field_the_descriptor_omits() {
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        // `Object.defineProperty(o, "a", {value: 1})` — three attributes unstated, and every
        // default is `false`. This is the difference from `o.a = 1`, which states all three.
        let bare = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &bare));
        let property = own(&heap, object, a).expect("just defined"); // the test is about it
        assert!(!property.enumerable);
        assert!(!property.configurable);
        assert!(matches!(
            property.kind,
            PropertyKind::Data {
                writable: false,
                ..
            }
        ));
    }

    #[test]
    fn nothing_may_be_added_to_a_non_extensible_object_and_what_is_there_may_still_change() {
        let mut heap = Heap::new();
        let (a, b) = (key(&mut heap, "a"), key(&mut heap, "b"));
        let object = heap.new_object(None);
        assert!(heap.define_own_property(object, a, &data(1.0)));
        prevent(&mut heap, object);

        assert!(!heap.object(object).is_some_and(Object::is_extensible));
        assert!(!heap.define_own_property(object, b, &data(2.0)));
        assert_eq!(count(&heap, object), 1);
        // …while the property that was already there is untouched by any of it: extensibility is
        // about *additions*, and `Object.preventExtensions` is not `Object.freeze`.
        assert!(heap.define_own_property(object, a, &data(3.0)));
        assert!(delete(&mut heap, object, a));
        assert_eq!(count(&heap, object), 0);
    }

    #[test]
    fn a_non_configurable_property_refuses_every_change_but_the_ones_that_are_no_change() {
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        let frozen = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(false),
            enumerable: Some(true),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &frozen));

        // Making it configurable again — the one-way door.
        let becoming_configurable = PropertyDescriptor {
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &becoming_configurable));
        // Changing its enumerability.
        let becoming_hidden = PropertyDescriptor {
            enumerable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &becoming_hidden));
        // Changing its kind.
        let becoming_accessor = PropertyDescriptor {
            getter: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &becoming_accessor));
        // Making it writable, or changing its value.
        let becoming_writable = PropertyDescriptor {
            writable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &becoming_writable));
        let other_value = PropertyDescriptor {
            value: Some(Value::Number(2.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &other_value));
        // …and deleting it.
        assert!(!delete(&mut heap, object, a));

        // Restating exactly what is already true is allowed, every time — that is a change of
        // nothing, and §10.1.6.3 only ever refuses changes.
        assert!(heap.define_own_property(object, a, &frozen));
        let restating_attributes = PropertyDescriptor {
            enumerable: Some(true),
            configurable: Some(false),
            writable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &restating_attributes));
        // …as is asking for nothing at all.
        assert!(heap.define_own_property(object, a, &PropertyDescriptor::EMPTY));
        assert_eq!(count(&heap, object), 1);
    }

    #[test]
    fn restating_the_value_of_a_frozen_property_writes_nothing_which_matters_for_nan() {
        // §10.1.6.3's NOTE, and the reason [`Validation`] has three cases. Two NaNs are the same
        // value to `SameValue` and can be told apart by other means, so the specification is
        // careful to leave the stored one alone.
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        let stored = f64::from_bits(0x7ff8_0000_0000_0001);
        let other = f64::from_bits(0xfff8_0000_0000_0000);
        assert!(stored.is_nan() && other.is_nan() && stored.to_bits() != other.to_bits());

        let frozen = PropertyDescriptor {
            value: Some(Value::Number(stored)),
            writable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &frozen));
        // Accepted, because the two are the same *value*…
        let restated = PropertyDescriptor {
            value: Some(Value::Number(other)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &restated));
        // …and the bits that were there are still there.
        let PropertyKind::Data {
            value: Value::Number(kept),
            ..
        } = own(&heap, object, a).expect("still defined").kind
        // the test is about it
        else {
            panic!("still a data property")
        };
        assert_eq!(kept.to_bits(), stored.to_bits());
    }

    #[test]
    fn a_frozen_accessor_keeps_both_of_its_functions_and_may_have_either_restated() {
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        let (getter, setter) = (Value::Number(1.0), Value::Number(2.0));
        let frozen = PropertyDescriptor {
            getter: Some(getter),
            setter: Some(setter),
            enumerable: Some(true),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &frozen));

        // §10.1.6.3 step 5.d, both halves — either function being different is a refusal, and
        // the setter is protected exactly as the getter is.
        let other_getter = PropertyDescriptor {
            getter: Some(Value::Number(9.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &other_getter));
        let other_setter = PropertyDescriptor {
            setter: Some(Value::Number(9.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &other_setter));
        // Restating the same pair is a change of nothing, and so is restating one of them.
        assert!(heap.define_own_property(object, a, &frozen));
        let same_setter = PropertyDescriptor {
            setter: Some(setter),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &same_setter));
        // …and a descriptor naming *no* kind is not asking for a change of kind, so step 5.c
        // lets it through even though this property is an accessor and the descriptor is not.
        let restating_configurable = PropertyDescriptor {
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &restating_configurable));
        // An accessor cannot become a data property while it is non-configurable, though.
        let becoming_data = PropertyDescriptor {
            value: Some(Value::Number(0.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &becoming_data));
    }

    #[test]
    fn a_non_configurable_property_that_is_writable_may_still_be_written_to() {
        // The pair of attributes is not one attribute. `Object.seal` leaves properties writable
        // and non-configurable, and that is the common case rather than a corner: the value may
        // change, the property may not be redefined or deleted.
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        let sealed = PropertyDescriptor {
            value: Some(Value::Number(1.0)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &sealed));

        let other_value = PropertyDescriptor {
            value: Some(Value::Number(2.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &other_value));
        // Restating writability is allowed while it is still writable…
        let staying_writable = PropertyDescriptor {
            writable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &staying_writable));
        // …and giving it up is allowed once, after which neither may come back.
        let freezing = PropertyDescriptor {
            writable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &freezing));
        assert!(!heap.define_own_property(object, a, &staying_writable));
        let third_value = PropertyDescriptor {
            value: Some(Value::Number(3.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(!heap.define_own_property(object, a, &third_value));
        // …while restating the value it now holds is still a change of nothing, and allowed.
        assert!(heap.define_own_property(object, a, &other_value));
        assert!(!delete(&mut heap, object, a));
    }

    #[test]
    fn a_configurable_property_may_become_the_other_kind_and_forgets_what_it_was() {
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        assert!(heap.define_own_property(object, a, &data(1.0)));

        // Data to accessor naming only a *setter*: the old value is gone rather than kept as
        // the getter, which is step 6.a's "or to the attribute's default value otherwise".
        let setter_only = PropertyDescriptor {
            setter: Some(Value::Number(8.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &setter_only));
        assert!(matches!(
            own(&heap, object, a).expect("still defined").kind, // the test is about it
            PropertyKind::Accessor {
                getter: Value::Undefined,
                setter: Value::Number(8.0)
            }
        ));
        assert!(heap.define_own_property(object, a, &data(1.0)));

        // Data to accessor: the value is gone, the unstated accessor is `undefined`, and the two
        // shared attributes survive because the descriptor did not mention them.
        let getter_only = PropertyDescriptor {
            getter: Some(Value::Number(7.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &getter_only));
        let property = own(&heap, object, a).expect("still defined"); // the test is about it
        assert!(matches!(
            property.kind,
            PropertyKind::Accessor {
                getter: Value::Number(7.0),
                setter: Value::Undefined
            }
        ));
        assert!(property.enumerable && property.configurable);

        // …and back, symmetrically.
        let value_only = PropertyDescriptor {
            value: Some(Value::Number(2.0)),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &value_only));
        let property = own(&heap, object, a).expect("still defined"); // same
        assert!(matches!(
            property.kind,
            PropertyKind::Data {
                value: Value::Number(2.0),
                writable: false
            }
        ));
        assert!(property.enumerable && property.configurable);
    }

    #[test]
    fn changing_one_attribute_leaves_the_others_where_they_were() {
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        assert!(heap.define_own_property(object, a, &data(1.0)));
        // Step 6.c — a field the descriptor does not have is an attribute it is not asking about.
        let hide = PropertyDescriptor {
            enumerable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, a, &hide));
        let property = own(&heap, object, a).expect("still defined"); // the test is about it
        assert!(!property.enumerable);
        assert!(property.configurable);
        assert!(matches!(
            property.kind,
            PropertyKind::Data {
                value: Value::Number(1.0),
                writable: true
            }
        ));
    }

    #[test]
    fn deleting_a_property_that_is_not_there_succeeds() {
        // `delete o.nothing` is `true`, and says nothing about whether there was anything.
        let mut heap = Heap::new();
        let a = key(&mut heap, "a");
        let object = heap.new_object(None);
        assert!(delete(&mut heap, object, a));
        assert!(heap.define_own_property(object, a, &data(1.0)));
        assert!(delete(&mut heap, object, a));
        assert!(delete(&mut heap, object, a));
        assert_eq!(count(&heap, object), 0);
    }

    #[test]
    fn own_keys_put_the_array_indices_first_in_numeric_order_and_the_rest_in_creation_order() {
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        // The example every article about this uses, and it is in the specification because
        // every engine had already agreed on it.
        for text in ["b", "2", "a", "1", "10", "0"] {
            let k = key(&mut heap, text);
            assert!(heap.define_own_property(object, k, &data(0.0)));
        }
        let names: Vec<String> = keys(&heap, object)
            .into_iter()
            .map(|k| String::from_utf16_lossy(heap.string(k.as_string()).unwrap_or(&[])))
            .collect();
        // Numeric order, not textual — "10" after "2" is the whole point of sorting on the index.
        assert_eq!(names, ["0", "1", "2", "10", "b", "a"]);
    }

    #[test]
    fn a_key_too_large_to_be_an_array_index_is_ordered_as_a_name() {
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        for text in ["4294967295", "5", "4294967294"] {
            let k = key(&mut heap, text);
            assert!(heap.define_own_property(object, k, &data(0.0)));
        }
        let names: Vec<String> = keys(&heap, object)
            .into_iter()
            .map(|k| String::from_utf16_lossy(heap.string(k.as_string()).unwrap_or(&[])))
            .collect();
        // 2^32 - 1 is a length, never an index, so it keeps its place among the names.
        assert_eq!(names, ["5", "4294967294", "4294967295"]);
    }

    #[test]
    fn a_property_is_found_along_the_prototype_chain_and_the_nearest_one_wins() {
        let mut heap = Heap::new();
        let (a, b, missing) = (
            key(&mut heap, "a"),
            key(&mut heap, "b"),
            key(&mut heap, "z"),
        );
        let grandparent = heap.new_object(None);
        let parent = heap.new_object(Some(grandparent));
        let child = heap.new_object(Some(parent));

        assert!(heap.define_own_property(grandparent, a, &data(1.0)));
        assert!(heap.define_own_property(grandparent, b, &data(1.0)));
        assert!(heap.define_own_property(parent, b, &data(2.0)));

        assert!(heap.has_property(child, a));
        assert!(heap.has_property(child, b));
        assert!(!heap.has_property(child, missing));
        // Shadowing: the nearest object on the chain that has the key is the one that answers.
        assert_eq!(
            heap.find_own(child, b).map(|(owner, _)| owner),
            Some(parent)
        );
        assert_eq!(
            heap.find_own(child, a).map(|(owner, _)| owner),
            Some(grandparent)
        );
        // An object with a null prototype inherits nothing, which is what `Object.create(null)`
        // is for.
        let bare = heap.new_object(None);
        assert!(!heap.has_property(bare, a));
    }

    #[test]
    fn a_prototype_may_not_be_made_to_point_back_at_itself() {
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(Some(first));
        let third = heap.new_object(Some(second));

        // Directly, and at a distance: `first.__proto__ = third` would close a three-object loop.
        assert!(!heap.set_prototype_of(first, Some(first)));
        assert!(!heap.set_prototype_of(first, Some(third)));
        assert_eq!(heap.object(first).and_then(Object::prototype), None);
        // …while a chain that does not come back is fine.
        let elsewhere = heap.new_object(None);
        assert!(heap.set_prototype_of(first, Some(elsewhere)));
        assert_eq!(
            heap.object(first).and_then(Object::prototype),
            Some(elsewhere)
        );
    }

    #[test]
    fn a_non_extensible_object_keeps_its_prototype_and_may_still_be_told_to_keep_it() {
        let mut heap = Heap::new();
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        prevent(&mut heap, object);

        assert!(!heap.set_prototype_of(object, None));
        // §10.1.2 step 2 comes *before* the extensibility test, so setting it to what it already
        // is succeeds even here — which is not the same as being able to change it.
        assert!(heap.set_prototype_of(object, Some(prototype)));
        assert_eq!(
            heap.object(object).and_then(Object::prototype),
            Some(prototype)
        );
    }

    #[test]
    fn a_foreign_or_missing_object_handle_answers_rather_than_panicking() {
        // DR-0002 over the handles, on the same terms as `Heap::string`: no panic and no
        // out-of-range read, and no detection either.
        let mut heap = Heap::new();
        let mut other = Heap::new();
        let a = key(&mut heap, "a");
        let stranger = other.new_object(None);
        other.new_object(None);
        let past_the_end = other.new_object(None);

        assert!(heap.object(past_the_end).is_none());
        assert!(!heap.has_property(past_the_end, a));
        assert!(!heap.define_own_property(past_the_end, a, &data(1.0)));
        assert!(heap.find_own(past_the_end, a).is_none());
        assert!(!heap.set_prototype_of(past_the_end, None));
        // …and one that happens to be in range answers about *this* heap's object at that index.
        let mine = heap.new_object(None);
        assert_eq!(stranger, mine);
        assert!(heap.object(stranger).is_some());
    }
}
