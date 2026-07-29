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
//! # Why the properties are a `Vec`, and what sits beside it
//!
//! Because §10.1.11 asks for insertion order and a `Vec` has it. Lookup was linear, which this
//! comment used to call the boring implementation and "wrong for an object with a thousand
//! properties — the fix is a map beside the order, or shapes, and both are M8 experiments that
//! need a benchmark first".
//!
//! The benchmark arrived. A linear scan makes *insertion* linear too, so filling an array element
//! by element is quadratic: `a[i] = 1` measured 270 ms for twenty thousand elements, 967 ms for
//! forty, and 3743 ms for eighty — four times the work for twice the elements. That is not a
//! slow engine, it is the wrong shape, and it was bad enough that such a test could not finish
//! inside the conformance harness's budget at all.
//!
//! So there is now a map beside the order, and the `Vec` still *is* the order. The map is not
//! built until an object has more properties than [`INDEXED_ABOVE`], because most objects never
//! do: a hash table on every one of them would cost an allocation each and buy nothing, and
//! DR-0013 counts those allocations. Shapes remain the other answer and remain an M8 experiment —
//! this one is smaller and needed no new representation to get the exponent right.

use crate::compile::Chunk;
use crate::heap::PropertyKind;
use crate::heap::arguments;
use crate::heap::define::{Validation, apply, validate};
use crate::heap::{
    ArgumentsMap, Bound, Callable, DefineOutcome, EnvironmentId, Heap, Native, Property,
    PropertyDescriptor, PropertyKey,
};
use crate::value::Value;
use std::collections::HashMap;
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
    call: Option<Callable>,
    /// The environment this function was *written* in — §10.2's `[[Environment]]`.
    ///
    /// A closure is this field. The call that made the function is long gone by the time the
    /// function runs, and the variables it could see are still here because this holds them.
    environment: Option<EnvironmentId>,
    /// The `this` an arrow was written beside — §10.2's `[[ThisMode]]` of `lexical`.
    ///
    /// `None` for every function that binds its own, which is all of them but arrows. Present, it
    /// is the same idea as `environment` one field up and for the same reason: the call that made
    /// the arrow is gone by the time the arrow runs, so the `this` it could see has to be *held*
    /// rather than looked for. §9.1.1.3 words it as a function environment with no `[[ThisBinding]]`
    /// whose `ResolveThisBinding` walks outward; the environment that walk arrives at is exactly
    /// the one running when the arrow was made, so recording its `this` here is that walk, done
    /// once and in advance.
    lexical_this: Option<Value>,
    /// Whether this is §10.4.2's exotic Array, whose `length` and indices move each other.
    pub(super) array: bool,
    /// §10.4.4's parameter map, if this is an arguments object.
    ///
    /// `None` for every other object, which is all but one per call that mentions the name. Boxed
    /// so that an object without one pays a pointer rather than a `Vec`: an `Object` sits inline
    /// in the arena, so its size is charged to every object ever made.
    pub(super) arguments: Option<Box<ArgumentsMap>>,
    /// The primitive this object *is* a wrapper for — §20.3's `[[BooleanData]]`, §21.1's
    /// `[[NumberData]]` and §22.1's `[[StringData]]`.
    ///
    /// One slot rather than three, because the value in it already says which: a `Value::Boolean`
    /// is a `[[BooleanData]]` and nothing else can be. That is what lets
    /// `Boolean.prototype.valueOf.call(new Number(1))` be the TypeError §20.3.3 asks for without
    /// three fields to keep apart.
    primitive: Option<Value>,
    /// The own properties, in the order they were created.
    ///
    /// The order is not incidental — §10.1.11 hands out string keys "in ascending chronological
    /// order of property creation", so this `Vec` *is* that answer for part of the result.
    properties: Vec<(PropertyKey, Property)>,
    /// Where each key sits in `properties`, once there are enough of them to be worth it.
    ///
    /// `None` means "few enough to scan", which is the common case and costs nothing. `Some` is
    /// an exact index: every key in `properties` is in it, mapped to its position. Anything that
    /// disturbs the positions — a delete, which shifts everything after it — either updates this
    /// or rebuilds it, because a stale index would find the wrong property rather than none.
    ///
    /// Boxed so that an object without one pays a pointer rather than a whole hash table. An
    /// `Object` sits inline in the heap's arena, so its size is charged to every object ever
    /// made, live or swept — see [`Heap::footprint`] and DR-0010.
    ///
    /// Measured, because clippy is right to ask: a `HashMap` here makes `Option<Object>` 144 bytes
    /// and a `Box` makes it 104. Most objects never build one, so the forty bytes would be paid by
    /// every object in the program to save a pointer hop for a few.
    #[allow(clippy::box_collection)] // 40 bytes an object, and every object pays — see above
    index: Option<Box<HashMap<PropertyKey, usize>>>,
}

/// How many properties an object may hold before its keys are worth indexing.
///
/// Below this a scan of a short `Vec` beats a hash: the keys are interned, so comparing two is
/// comparing two integers, and eight of those cost less than hashing one. Above it the scan is
/// what makes filling an array quadratic.
///
/// The exact number is not delicate — anything in this region trades the same way, and the cases
/// that hurt have thousands of properties rather than nine.
const INDEXED_ABOVE: usize = 8;

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
            array: false,
            arguments: None,
            primitive: None,
            call: None,
            environment: None,
            lexical_this: None,
            properties: Vec::new(),
            index: None,
        }
    }

    /// The parameter map this object joins, if it is an arguments object.
    pub(crate) fn arguments_map(&self) -> Option<&ArgumentsMap> {
        self.arguments.as_deref()
    }

    /// The primitive this object wraps, if it wraps one.
    ///
    /// `None` for an ordinary object, which is most of them. What is in it says which kind of
    /// wrapper this is, so a method that requires its own kind matches on the value rather than
    /// asking a flag.
    pub fn primitive(&self) -> Option<Value> {
        self.primitive
    }

    /// Whether this is an Array — §10.4.2's exotic object, and the only one there is.
    pub fn is_array(&self) -> bool {
        self.array
    }

    /// `[[GetPrototypeOf]]` (§10.1.1) — the prototype, or `None` for `null`.
    pub fn prototype(&self) -> Option<ObjectId> {
        self.prototype
    }

    /// What this object runs when it is called, if it is callable at all.
    ///
    /// `None` is what `typeof` reads to answer anything but `"function"`, and what a call
    /// expression checks before it does anything else.
    pub fn call(&self) -> Option<&Callable> {
        self.call.as_ref()
    }

    /// The environment this function was written in, if it is a function at all.
    pub fn environment(&self) -> Option<EnvironmentId> {
        self.environment
    }

    /// The `this` this function took from around it, if it is an arrow.
    ///
    /// `None` means the function binds `this` from the call, which is every function but an arrow
    /// — and also every non-function, which has no `this` to speak of either way.
    pub fn lexical_this(&self) -> Option<Value> {
        self.lexical_this
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
        let at = self.position(key)?;
        self.properties.get(at).map(|(_, property)| property)
    }

    /// Where `key` sits in `properties`, by whichever means this object has.
    ///
    /// The one place that decides how a key is found. Written twice — once for the scan and once
    /// for the map — the two could disagree about a key and only one of them would be right.
    fn position(&self, key: PropertyKey) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(&key).copied(),
            None => self
                .properties
                .iter()
                .position(|(stored, _)| *stored == key),
        }
    }

    /// Build the index of every key's position, or rebuild one whose positions have moved.
    fn reindex(&mut self) {
        self.index = Some(Box::new(
            self.properties
                .iter()
                .enumerate()
                .map(|(at, (key, _))| (*key, at))
                .collect(),
        ));
    }

    /// File `property` under `key`, replacing whatever was there.
    ///
    /// The write half of `[[DefineOwnProperty]]`, and private because it is only correct after
    /// [`validate`] has agreed. A new key goes on the end, which is what makes the `Vec` the
    /// creation order §10.1.11 asks for.
    fn insert(&mut self, key: PropertyKey, property: Property) {
        if let Some(at) = self.position(key) {
            // A key that is already here keeps its place: §10.1.11's order is *creation* order,
            // so writing to a property again must not move it to the end.
            if let Some((_, existing)) = self.properties.get_mut(at) {
                *existing = property;
            }
            return;
        }
        let at = self.properties.len();
        self.properties.push((key, property));
        match &mut self.index {
            // Appending disturbs no existing position, so the index only gains an entry.
            Some(index) => {
                index.insert(key, at);
            }
            None if self.properties.len() > INDEXED_ABOVE => self.reindex(),
            None => {}
        }
    }

    /// `[[Delete]]` (§10.1.10) — remove the own property `key`, if it may be removed.
    ///
    /// A key that is not there answers `true`: deleting nothing succeeds, which is why
    /// `delete o.nothing` is `true` and says nothing about whether `o.nothing` existed.
    pub fn delete(&mut self, key: PropertyKey) -> bool {
        let Some(at) = self.position(key) else {
            return true;
        };
        if !self.properties[at].1.configurable {
            return false;
        }
        self.properties.remove(at);
        // Removing shifts every position after it, so the index is now wrong about all of them —
        // and wrong here means finding a *neighbouring* property rather than finding none, which
        // is the kind of error that reads as a plausible value. Rebuilding costs what the removal
        // already cost.
        if self.index.is_some() {
            self.reindex();
        }
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
    ///
    /// `lexical_this` is `Some` only for an arrow, and holds the `this` in force where the arrow
    /// was written — §10.2.3 step 6's `[[ThisMode]]` of `lexical`, captured rather than resolved.
    /// Every other function is handed its `this` by the call, so it passes `None`.
    pub fn new_function(
        &mut self,
        prototype: ObjectId,
        body: Rc<Chunk>,
        environment: EnvironmentId,
        lexical_this: Option<Value>,
    ) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Bytecode(body));
        object.environment = Some(environment);
        object.lexical_this = lexical_this;
        self.objects.push(Some(object));
        id
    }

    /// Put a built-in function object on the heap — `CreateBuiltinFunction` (§10.3.4).
    ///
    /// No environment, because there is nothing lexical about it: a built-in's behaviour is Rust
    /// and closes over nothing. That is the field a JavaScript function needs and this one does
    /// not, and leaving it empty is what says so.
    ///
    /// The `name` and `length` §10.3.3 requires are properties like any others and are given by
    /// the caller, because only the caller knows them.
    pub fn new_native_function(&mut self, prototype: ObjectId, native: Native) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Native(native));
        self.objects.push(Some(object));
        id
    }

    /// Put a bound function on the heap — `BoundFunctionCreate` (§10.4.1.3).
    ///
    /// Its prototype is the *target's*, not `Function.prototype`: §10.4.1.3 step 1 takes it from
    /// the function being bound, so `f.bind(o)` inherits from whatever `f` did.
    ///
    /// No environment and no code of its own. A bound function has nothing to close over — what
    /// it holds is another function and the two things a call to it is already decided about.
    pub fn new_bound_function(&mut self, prototype: Option<ObjectId>, bound: Bound) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(prototype);
        object.call = Some(Callable::Bound(bound));
        self.objects.push(Some(object));
        id
    }

    /// Put a wrapper for a primitive on the heap — §20.3.1.1, §21.1.1.1 and §22.1.1.1.
    ///
    /// Ordinary in every way but one: it remembers a primitive, and the methods of the matching
    /// prototype are the only things that read it. Nothing about the *object* changes — a wrapper
    /// has ordinary properties, an ordinary prototype and no exotic behaviour, which is why
    /// `new Number(1).x = 2` works exactly as it does on `{}`.
    pub fn new_wrapper(&mut self, prototype: ObjectId, primitive: Value) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.primitive = Some(primitive);
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
        self.define_property_outcome(object, key, descriptor) == DefineOutcome::Defined
    }

    /// §10.4.4.2 — what a define does to an argument index that is joined to a parameter.
    ///
    /// Three rules, and each is about the link rather than about the property. A value written to
    /// a joined index goes to the *parameter*. Making the index an accessor breaks the link,
    /// because a parameter is not an accessor and could not stand in for one. Making it
    /// non-writable breaks the link too, and §10.4.4.2 is careful about the order: the value is
    /// written first, so `Object.defineProperty(arguments, '0', {value: 2, writable: false})`
    /// leaves the parameter at 2 and *then* stops following it.
    fn settle_argument(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) {
        if self
            .object(object)
            .and_then(Object::arguments_map)
            .is_none()
        {
            return;
        }
        if descriptor.is_accessor_descriptor() {
            self.unmap_argument(object, key);
            return;
        }
        if let Some(value) = descriptor.value {
            self.write_through(object, key, value);
        }
        if descriptor.writable == Some(false) {
            self.unmap_argument(object, key);
        }
    }

    /// `[[DefineOwnProperty]]`, with the one answer a Boolean cannot carry.
    ///
    /// §10.4.2.4 step 2's bad array length is a **RangeError** and every other refusal is a
    /// `false` that sloppy code ignores. A caller that can throw asks this; one that only wants
    /// to know whether the property is now there asks [`Heap::define_own_property`].
    pub fn define_property_outcome(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> DefineOutcome {
        // §10.4.2.1 — an Array's is not the ordinary one. Dispatching here rather than at every
        // call site is what makes `a[0] = 1` and `Object.defineProperty(a, "0", …)` agree about
        // what happens to `length`, which they must.
        if self.object(object).is_some_and(Object::is_array) {
            return self.define_array_property(object, key, descriptor);
        }
        let defined = self.define_ordinary_property(object, key, descriptor);
        // §10.4.4.2 steps 3 to 5 — only when the define was allowed. A refused define changes
        // nothing, and must not break a link either.
        if defined {
            self.settle_argument(object, key, descriptor);
        }
        DefineOutcome::from(defined)
    }

    /// §10.1.6.3 `OrdinaryDefineOwnProperty` — the rules every object but an Array uses whole,
    /// and the ones an Array uses after it has moved its `length`.
    pub(super) fn define_ordinary_property(
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
    /// An object's own property, with §10.4.4's map consulted — `[[GetOwnProperty]]`.
    ///
    /// The same answer as the object's own table for everything but a joined argument index,
    /// where the *value* comes from the parameter instead. §10.4.4.1 says exactly this: the
    /// descriptor is the ordinary one with its value replaced, which is why
    /// `Object.getOwnPropertyDescriptor(arguments, 0)` reports a data property and not the
    /// accessor the specification's own note implements the map with.
    pub fn own_property(&self, object: ObjectId, key: PropertyKey) -> Option<Property> {
        let found = self.object(object)?;
        let property = *found.get_own_property(key)?;
        let Some(map) = found.arguments_map() else {
            return Some(property);
        };
        let Some(slot) = arguments::index_of(self, key).and_then(|index| map.slot(index)) else {
            return Some(property);
        };
        // A joined index is never uninitialised: a parameter is given its value when the call
        // begins, and nothing can put one back into the dead zone.
        let Some(Some(value)) = self.variable(map.environment, slot) else {
            return Some(property);
        };
        Some(Property {
            kind: match property.kind {
                PropertyKind::Data { writable, .. } => PropertyKind::Data { value, writable },
                accessor => accessor,
            },
            ..property
        })
    }

    /// Break the link between an argument index and its parameter — §10.4.4.2 and §10.4.4.5.
    ///
    /// Answers whether there was one, so that a caller which has just changed a property can say
    /// what it did without asking twice.
    pub(crate) fn unmap_argument(&mut self, object: ObjectId, key: PropertyKey) {
        let Some(index) = arguments::index_of(self, key) else {
            return;
        };
        if let Some(map) = self
            .object_mut(object)
            .and_then(|found| found.arguments.as_deref_mut())
        {
            map.unmap(index);
        }
    }

    /// Write through to the parameter an argument index is joined to, if it is joined to one.
    ///
    /// Answers nothing. Whether it wrote is not a question anyone asks — the caller has just been
    /// told the define was allowed, and a key that is not a joined index simply has no parameter
    /// behind it. A return value nobody reads is one no test could be wrong about.
    fn write_through(&mut self, object: ObjectId, key: PropertyKey, value: Value) {
        let Some(index) = arguments::index_of(self, key) else {
            return;
        };
        let Some(map) = self.object(object).and_then(Object::arguments_map) else {
            return;
        };
        let (Some(slot), environment) = (map.slot(index), map.environment) else {
            return;
        };
        self.set_variable(environment, slot, value);
    }

    /// Put an arguments object on the heap — §10.4.4.4 `CreateMappedArgumentsObject`.
    ///
    /// The values are the arguments the call was given, all of them; the map joins the first
    /// `parameters` of them to the slots of `environment`. `callee` is the function itself, which
    /// §10.4.4.4 step 15 gives a mapped arguments object and an unmapped one refuses to.
    pub fn new_arguments(
        &mut self,
        prototype: ObjectId,
        environment: EnvironmentId,
        values: &[Value],
        parameters: usize,
        callee: ObjectId,
    ) -> ObjectId {
        let object = self.new_object(Some(prototype));
        for (at, value) in values.iter().enumerate() {
            let index = u32::try_from(at).unwrap_or(u32::MAX);
            let key = self.index_key(index);
            self.define_own_property(object, key, &PropertyDescriptor::data(*value));
        }
        // §10.4.4.4 steps 14 and 15 — `length` and `callee` are ordinary §17 properties: writable
        // and configurable, and never enumerable, so `for`-`in` over `arguments` walks the
        // indices and nothing else.
        for (name, value) in [
            ("length", Value::Number(values.len() as f64)),
            ("callee", Value::Object(callee)),
        ] {
            let key = PropertyKey::from_units(self, &name.encode_utf16().collect::<Vec<_>>());
            self.define_own_property(
                object,
                key,
                &PropertyDescriptor {
                    enumerable: Some(false),
                    ..PropertyDescriptor::data(value)
                },
            );
        }
        // Joined *after* the properties are made, because making them goes through the define
        // below — and a define on a joined index writes through to a parameter instead.
        if let Some(found) = self.object_mut(object) {
            found.arguments = Some(Box::new(ArgumentsMap::new(
                environment,
                parameters.min(values.len()),
            )));
        }
        object
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// The property *and* which object it came from, since an accessor's getter is called with
    /// that object as its receiver.
    ///
    /// Asked through [`Heap::own_property`] rather than the object's own table, so that a joined
    /// argument index answers with its parameter's value however the read arrived.
    pub fn find_own(&self, object: ObjectId, key: PropertyKey) -> Option<(ObjectId, Property)> {
        let mut cursor = Some(object);
        // The chain cannot be a cycle — nothing can build one — and this counts anyway. DR-0002
        // is not a claim about the code being right; it is a claim that being wrong does not
        // hang. See [`Heap::set_prototype_of`] for the check that makes the count unreachable.
        for _ in 0..MAX_PROTOTYPE_CHAIN {
            let at = cursor?;
            if let Some(property) = self.own_property(at, key) {
                return Some((at, property));
            }
            cursor = self.object(at)?.prototype();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::PropertyKind;

    fn key(heap: &mut Heap, text: &str) -> PropertyKey {
        PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
    }

    fn data(value: f64) -> PropertyDescriptor {
        PropertyDescriptor {
            value: Some(Value::Number(value)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        }
    }

    /// Whether `object` is keeping an index of its keys.
    fn indexed(heap: &Heap, object: ObjectId) -> bool {
        heap.object(object)
            .is_some_and(|found| found.index.is_some())
    }

    #[test]
    fn keys_are_indexed_only_once_there_are_more_of_them_than_it_costs_to_scan() {
        // This test looks at a private field, which the rest of this file's tests are careful not
        // to do — and the reason is the point. The index changes no answer: every question about
        // a property has the same answer whether it was found by a scan or by a hash. So no test
        // written in JavaScript can say when one is built, and a policy nothing can observe is a
        // policy nothing is holding in place.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        for at in 0..INDEXED_ABOVE {
            let key = key(&mut heap, &format!("k{at}"));
            heap.define_own_property(object, key, &data(at as f64));
            assert!(
                !indexed(&heap, object),
                "{} properties is still few enough to scan",
                at + 1
            );
        }
        // One more than the threshold, and not one fewer: an object holding exactly
        // `INDEXED_ABOVE` is on the cheap side of the trade.
        let over = key(&mut heap, "one-too-many");
        heap.define_own_property(object, over, &data(99.0));
        assert!(indexed(&heap, object));

        // A small object that has something deleted does not acquire one on the way past — the
        // rebuild after a delete is for an index that already exists, not a reason to build one.
        let small = heap.new_object(None);
        let only = key(&mut heap, "only");
        heap.define_own_property(small, only, &data(1.0));
        assert!(
            heap.object_mut(small)
                .is_some_and(|found| found.delete(only))
        );
        assert!(!indexed(&heap, small));
    }

    #[test]
    fn an_indexed_object_finds_every_key_it_still_has_after_a_delete() {
        // The failure this guards against is not "cannot find it" — it is finding the *wrong*
        // one. Removing a property shifts every position after it, so an index left unrebuilt
        // answers each of those keys with its neighbour: a plausible value, not a crash.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        let count = INDEXED_ABOVE * 3;
        let keys: Vec<_> = (0..count)
            .map(|at| {
                let key = key(&mut heap, &format!("k{at}"));
                heap.define_own_property(object, key, &data(at as f64));
                key
            })
            .collect();
        assert!(indexed(&heap, object));

        // Delete from the front, where the most positions move.
        assert!(
            heap.object_mut(object)
                .is_some_and(|found| found.delete(keys[0]))
        );
        for (at, key) in keys.iter().enumerate().skip(1) {
            let found = heap
                .object(object)
                .and_then(|found| found.get_own_property(*key))
                .map(|property| property.kind);
            assert!(
                matches!(
                    found,
                    Some(PropertyKind::Data {
                        value: Value::Number(number),
                        ..
                    }) if number == at as f64
                ),
                "k{at} answered with the wrong property after a delete shifted it"
            );
        }
        assert!(
            heap.object(object)
                .and_then(|found| found.get_own_property(keys[0]))
                .is_none()
        );
    }
}
