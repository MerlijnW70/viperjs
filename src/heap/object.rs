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
use crate::heap::define::{Validation, apply, validate};
use crate::heap::{EnvironmentId, Heap, Property, PropertyDescriptor, PropertyKey};
use std::rc::Rc;

/// An object on the heap.
///
/// Meaningful only to the [`Heap`] that issued it, on the same terms as [`crate::heap::StringId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub(super) usize);

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
        self.objects.push(Some(object));
        id
    }

    /// Put an ordinary object on the heap — `OrdinaryObjectCreate` (§10.1.12).
    pub fn new_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let id = ObjectId(self.objects.len());
        self.objects.push(Some(Object::new(prototype)));
        id
    }

    /// The object `id` refers to, or `None` if this heap has nothing there.
    ///
    /// The same narrow promise [`Heap::string`] makes about a foreign handle, for the same
    /// reason: no panic and no out-of-range read, and no detection.
    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0)?.as_ref()
    }

    /// The object `id` refers to, to be changed.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id.0)?.as_mut()
    }

    /// How many objects this heap holds.
    pub fn object_count(&self) -> usize {
        self.objects.iter().filter(|slot| slot.is_some()).count()
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
#[path = "tests.rs"]
mod object_tests;
