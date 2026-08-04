//! What the *heap* does with an object — §10.1's internal methods, and the ways one is made.
//!
//! Split from [`super::object`], which holds the `Object` type and the accessors that answer for
//! one. The division is by who is asking. An `Object` answers about **itself**: what its prototype
//! is, whether it is callable, what it holds in a slot. A `Heap` answers about an object it *owns*
//! — and it has to, because §10.1's operations reach further than one object can see. Defining a
//! property consults the prototype chain, `[[Set]]` walks it looking for a setter, and a
//! constructor has to allocate before there is anything to hang a method on.
//!
//! # Two groups, and the second is why this is long
//!
//! The `new_*` constructors are one per exotic kind and are mostly short: a prototype, a slot, an
//! id. §10.1's internal methods are the rest and carry the weight — `define_own_property` alone is
//! §10.1.6's validation, which is a table of nine cases about what a descriptor may change into
//! what.

use super::arguments;
use super::arguments::Incoming;
use super::define::{Validation, apply, validate};
use super::object::{Lexical, MAX_PROTOTYPE_CHAIN, ObjectId, PrivateElement, Suspendable};
use super::string_object;
use super::typed;
use super::{
    ArgumentsMap, Bound, Callable, DefineOutcome, Element, EnvironmentId, Heap, Iteration, Native,
    Numeric, Object, Property, PropertyDescriptor, PropertyKey, PropertyKind, StringId, SymbolId,
};
use crate::compile::Chunk;
use crate::value::Value;
use std::rc::Rc;

impl Heap {
    /// Put a function object on the heap — `OrdinaryFunctionCreate` (§10.2.3), in the part that
    /// is about the object rather than about the environment.
    ///
    /// Ordinary in every way but one: it has a `[[Call]]`, which is what makes `typeof` say
    /// `"function"` and what a call expression looks for.
    ///
    /// `lexical` is `Some` only for an arrow, and holds what was in force where the arrow
    /// was written — §10.2.3 step 6's `[[ThisMode]]` of `lexical`, captured rather than resolved.
    /// Every other function is handed its `this` by the call, so it passes `None`.
    pub fn new_function(
        &mut self,
        prototype: ObjectId,
        body: Rc<Chunk>,
        environment: EnvironmentId,
        lexical: Option<Lexical>,
    ) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Bytecode(body));
        object.environment = Some(environment);
        // An arrow's home comes from the same capture as its `this`, so the three cannot be captured
        // separately and disagree about which method the arrow was written in. A method's own home is
        // set afterwards by §9.1.1.3's `MakeMethod`, which is a different moment and a different
        // object — see [`Heap::set_home_object`].
        object.home = lexical.and_then(|captured| captured.home);
        object.lexical = lexical;
        self.objects.place(object)
    }

    /// Put one of §27.5.1's three resumption methods on the heap.
    ///
    /// A function object in every respect a script can ask about — it has a `[[Call]]`, `typeof`
    /// answers `"function"`, and it is not a constructor. What it does *not* have is a Rust body:
    /// see [`Callable::Resume`] for why resuming a generator cannot be one.
    pub(crate) fn new_resume_function(
        &mut self,
        prototype: ObjectId,
        kind: crate::heap::Resumption,
        asynchronous: bool,
    ) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Resume { kind, asynchronous });
        self.objects.place(object)
    }

    /// One of §27.7.5.3's two resumption closures, as a function object.
    ///
    /// Reachable from nothing a script can name: it exists to be handed to `PerformPromiseThen` and
    /// called once by a job. It has no `name` and no `length`, which nothing can ask for.
    pub(crate) fn new_revive_function(
        &mut self,
        context: ObjectId,
        kind: crate::heap::ReactionKind,
    ) -> ObjectId {
        let mut object = Object::new(None);
        object.call = Some(Callable::Revive { kind, context });
        self.objects.place(object)
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
        self.built_in(prototype, native, false)
    }

    /// The same, for a built-in that §10.3.2 gives a `[[Construct]]` — a *constructor*.
    ///
    /// Separate from [`Heap::new_native_function`] rather than a flag at every call site, because
    /// the two are unequal in number: nearly every built-in is a method and cannot be constructed,
    /// and defaulting the other way would make `new Math.max()` an object rather than the
    /// TypeError §10.3 asks for.
    pub fn new_native_constructor(&mut self, prototype: ObjectId, native: Native) -> ObjectId {
        self.built_in(prototype, native, true)
    }

    /// `CreateBuiltinFunction` (§10.3.4), for both kinds.
    fn built_in(&mut self, prototype: ObjectId, native: Native, constructs: bool) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Native { native, constructs });
        self.objects.place(object)
    }

    /// Give an object that already exists a `[[Call]]` running `native`.
    ///
    /// For §10.5 alone. Every other callable is *made* callable, because what it runs is decided
    /// with it; a proxy is made first and then finds out whether its target was a function, and
    /// §10.5 says it has a `[[Call]]` exactly when the target did.
    pub fn make_callable(&mut self, object: ObjectId, native: Native, constructs: bool) {
        if let Some(found) = self.object_mut(object) {
            found.call = Some(Callable::Native { native, constructs });
        }
    }

    /// Put a bound function on the heap — `BoundFunctionCreate` (§10.4.1.3).
    ///
    /// Its prototype is the *target's*, not `Function.prototype`: §10.4.1.3 step 1 takes it from
    /// the function being bound, so `f.bind(o)` inherits from whatever `f` did.
    ///
    /// No environment and no code of its own. A bound function has nothing to close over — what
    /// it holds is another function and the two things a call to it is already decided about.
    pub fn new_bound_function(&mut self, prototype: Option<ObjectId>, bound: Bound) -> ObjectId {
        let mut object = Object::new(prototype);
        object.call = Some(Callable::Bound(bound));
        self.objects.place(object)
    }

    /// Put a wrapper for a primitive on the heap — §20.3.1.1, §21.1.1.1 and §22.1.1.1.
    ///
    /// Ordinary in every way but one: it remembers a primitive, and the methods of the matching
    /// prototype are the only things that read it. Nothing about the *object* changes — a wrapper
    /// has ordinary properties, an ordinary prototype and no exotic behaviour, which is why
    /// `new Number(1).x = 2` works exactly as it does on `{}`.
    pub fn new_wrapper(&mut self, prototype: ObjectId, primitive: Value) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.primitive = Some(primitive);
        self.objects.place(object)
    }

    /// Put a Date on the heap — §21.4.2.1's `OrdinaryCreateFromConstructor` with `[[DateValue]]`.
    ///
    /// `time` may be NaN, and that is a Date rather than a failure: §21.4.1.31's `TimeClip` answers
    /// NaN for anything out of range, and the object it lands in is a perfectly ordinary Date whose
    /// every getter reports NaN. There is no separate "invalid" state to represent.
    pub fn new_date(&mut self, prototype: ObjectId, time: f64) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.date = Some(time);
        self.objects.place(object)
    }

    /// §10.4.3.4 `StringCreate` — a String exotic object over `data`.
    ///
    /// `length` is put there for real, because it is an ordinary property that never changes; the
    /// characters are not: those are answered from `data` itself, every time they are asked for.
    ///
    /// Every character is interned on the way past. That is what lets a *read* of `s[0]` be a read:
    /// the one-character String it must answer with already exists, so no shared borrow ever has to
    /// make one. There are at most 65,536 distinct one-unit Strings, so what this can add to the
    /// heap over a whole program is bounded however many String objects are made.
    pub fn new_string_object(&mut self, prototype: ObjectId, data: StringId) -> ObjectId {
        let units = self.string(data).unwrap_or(&[]).to_vec();
        for unit in &units {
            self.intern(&[*unit]);
        }
        let id = self.new_wrapper(prototype, Value::String(data));
        let units16: Vec<u16> = "length".encode_utf16().collect();
        let length = PropertyKey::from_units(self, &units16);
        let count = f64::from(u32::try_from(units.len()).unwrap_or(u32::MAX));
        // §10.4.3.4 step 5 — all three attributes false, which is why `s.length = 9` is refused
        // and `delete s.length` answers false. The define cannot be refused on an object made a
        // moment ago, so its answer is not worth asking about.
        self.define_own_property(
            id,
            length,
            &PropertyDescriptor {
                value: Some(Value::Number(count)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        );
        id
    }

    /// `[[OwnPropertyKeys]]` — §10.1.11, and §10.4.3.1 when the object is a String.
    ///
    /// Everything a program can see, which is more than [`Object::own_property_keys`] can answer:
    /// a String object's characters are own keys and naming one means making the String `"0"`, so
    /// this is where the question is asked from and why it needs the heap by exclusive reference.
    pub fn own_property_keys(&mut self, object: ObjectId) -> Vec<PropertyKey> {
        // §10.4.6.10 — the sorted export names ahead of the object's own, which is the one
        // enumeration order in the language that is not the order things were written in.
        if let Some(keys) = self.namespace_keys(object) {
            return keys;
        }
        let stored = self
            .object(object)
            .map_or_else(Vec::new, |found| found.own_property_keys(self));
        // §10.4.5.6 — a TypedArray's indices, which nothing stored, ahead of everything that was.
        // In order and complete: §10.1.11 wants the integer indices first and ascending, and no
        // stored key can sort in among them because a define at an index never stores anything.
        if let Some(view) = self.object(object).and_then(Object::view)
            && view.element.is_some()
        {
            let count = view.count();
            let mut keys = Vec::new();
            for index in 0..u32::try_from(count).unwrap_or(u32::MAX) {
                keys.push(self.index_key(index));
            }
            keys.extend(stored);
            return keys;
        }
        let Some(data) = self.object(object).and_then(Object::string_data) else {
            return stored;
        };
        // Ahead of the stored keys and in order, which is §10.1.11's ascending run of indices: a
        // String object's own stored indices are all past its last character, because a define
        // *at* a character is refused, so nothing stored can sort in among these.
        let count = u32::try_from(string_object::length(self, data)).unwrap_or(u32::MAX);
        let mut keys = Vec::with_capacity(count as usize + stored.len());
        for index in 0..count {
            keys.push(self.index_key(index));
        }
        keys.extend(stored);
        keys
    }

    /// `[[Delete]]` — §10.1.10, and §10.4.3.6 when the object is a String.
    ///
    /// A String object's characters are not configurable, so deleting one answers `false` and
    /// removes nothing. [`Object::delete`] cannot tell: it looks for a stored property, finds none,
    /// and says `true` on the grounds that what is not there cannot be in the way.
    pub fn delete_own_property(&mut self, object: ObjectId, key: PropertyKey) -> bool {
        // §10.4.5.4 — an index the view *has* cannot be deleted, and one it has not is already
        // gone. Both answers come from the same place and they are opposite: `delete ta[0]` is
        // false on a non-empty array and `delete ta[99]` is true, because deleting nothing
        // succeeded vacuously.
        // Through [`Heap::typed_view`] and never the stored `View`: §10.4.5.9's index test is
        // asked of the window the buffer has **now**, and a view over a resizable buffer that has
        // been shrunk holds a length that is no longer true. Reading the stale one says an index
        // past the end is still there, and answers `false` to a `delete` of a property §10.4.5.1
        // no longer describes.
        if let Some(view) = self.typed_view(object)
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            return index.is_err();
        }
        if let Some(data) = self.object(object).and_then(Object::string_data)
            && string_object::character(self, data, key).is_some()
        {
            return false;
        }
        self.object_mut(object)
            .is_some_and(|found| found.delete(key))
    }

    /// The one-character String at `index` of `data`, interned so a later read can find it.
    ///
    /// §10.4.3.5's value, for the reader that has a String *primitive* rather than an object and
    /// so has nowhere the characters were interned from.
    pub fn intern_character(&mut self, data: StringId, index: u32) -> Option<StringId> {
        string_object::intern_character(self, data, index)
    }

    /// §23.1.5.1 `CreateArrayIterator` and §22.1.5.1 `CreateStringIterator` — an iterator object.
    ///
    /// Ordinary but for the position it remembers, which is a slot rather than a property so that
    /// nothing in the language can move it. See [`crate::heap::Iteration`].
    pub fn new_iterator(&mut self, prototype: ObjectId, iteration: Iteration) -> ObjectId {
        let mut object = Object::new(Some(prototype));
        object.iteration = Some(Box::new(iteration));
        self.objects.place(object)
    }

    /// Put an ordinary object on the heap — `OrdinaryObjectCreate` (§10.1.12).
    pub fn new_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        self.objects.place(Object::new(prototype))
    }

    /// The object `id` refers to, or `None` if this heap has nothing there.
    ///
    /// The same narrow promise [`Heap::string`] makes about a foreign handle, for the same
    /// reason: no panic and no out-of-range read, and no detection.
    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id)
    }

    /// The object `id` refers to, to be changed.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id)
    }

    /// Park `parked` in `holder` — §27.5.1's `[[GeneratorContext]]`, filled in.
    ///
    /// An `ObjectId` and no answer, because there is nothing to answer. It used to take a `Value`
    /// and report whether that value could hold an execution, which was worth asking while
    /// `Suspend` took its holder off the operand stack; now every caller has an object it either
    /// just made or read off the running frame. What was left was a `false` no input could reach.
    ///
    /// Whatever was there is replaced. A holder that is already parked is not a state the
    /// generator machinery above this can produce — §27.5.1.2 refuses to resume one twice — so
    /// there is nothing here to refuse.
    pub(crate) fn park_into(&mut self, holder: ObjectId, parked: crate::vm::Suspended) {
        if let Some(object) = self.object_mut(holder) {
            object.suspension = Some(Box::new(parked));
        }
    }

    /// Take the execution parked in `holder`, leaving it holding none.
    ///
    /// `None` for a value that is not an object and for an object that has nothing parked — which
    /// includes one that was parked and has already been revived, since a suspension is *moved*
    /// out. That is the property the state machine above this rests on: an execution cannot be
    /// entered twice, because after the first entry it is no longer anywhere to be found.
    pub(crate) fn take_parked(&mut self, holder: Value) -> Option<crate::vm::Suspended> {
        let parked = self.holder_mut(holder)?.suspension.take()?;
        Some(*parked)
    }

    /// Mark `object` as holding a suspendable execution — given once and never taken away.
    pub(crate) fn brand_suspendable(&mut self, object: ObjectId, kind: Suspendable) {
        if let Some(object) = self.object_mut(object) {
            object.suspendable = Some(kind);
        }
    }

    /// The object a value names, to be changed — `None` if it names none.
    fn holder_mut(&mut self, holder: Value) -> Option<&mut Object> {
        match holder {
            Value::Object(id) => self.object_mut(id),
            _ => None,
        }
    }

    /// How many objects this heap holds.
    pub fn object_count(&self) -> usize {
        self.objects.live()
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

    /// §7.2.2 `IsArray` — an Array, or a proxy standing in front of one.
    ///
    /// The one question about a proxy that needs no interpreter: §7.2.2 does not consult the
    /// handler at all, it looks straight through to `[[ProxyTarget]]`. So `Array.isArray` of a
    /// proxy over an array is `true` however the handler is written, and there is no trap that can
    /// change it — which is what lets `JSON.stringify` tell an array from an object safely.
    ///
    /// A revoked proxy is a TypeError rather than `false`, because there is no target left to ask.
    /// Iterative, because a proxy's target may be another proxy and a program chooses how many.
    pub fn is_array_through(&self, object: ObjectId) -> crate::value::Completion<bool> {
        let mut walk = object;
        loop {
            // No guard for an id this heap has not got: it is not an array, and it is not a proxy
            // standing in front of one either, so both answers below are already right.
            let Some(proxy) = self.object(walk).and_then(Object::proxy) else {
                return Ok(self.object(walk).is_some_and(Object::is_array));
            };
            let Some(target) = proxy.target() else {
                return Err(crate::value::Abrupt::type_error(
                    "Array.isArray cannot ask a revoked proxy what it stands in front of",
                ));
            };
            walk = target;
        }
    }

    /// `IsCompatiblePropertyDescriptor` (§6.2.6.4) — would this change be allowed, without making it?
    ///
    /// §6.2.6.4 is `ValidateAndApplyPropertyDescriptor` with no object to write to, and it exists
    /// for §10.5 alone: a proxy trap describes a property the *target* does not have to hold, and
    /// the only question is whether that description could have been true of the target. Nothing
    /// else in the language needs to ask a question about a property it is not about to change.
    #[must_use]
    pub fn is_compatible_descriptor(
        &self,
        descriptor: &PropertyDescriptor,
        current: Option<&Property>,
        extensible: bool,
    ) -> bool {
        !matches!(
            crate::heap::define::validate(descriptor, current, extensible, self),
            crate::heap::define::Validation::Reject
        )
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
        // §10.4.5.3 — a define at a canonical numeric index of a TypedArray. An index the view
        // does not have is **refused** rather than stored, which is where this differs from every
        // ordinary object: `Object.defineProperty(ta, "99", …)` on a short array fails, because a
        // TypedArray's length cannot change and a property there would be a length that lied.
        //
        // Through [`Heap::typed_view`], so the length is the one the buffer has now. §10.4.5.9
        // step 2 refuses an index of a view that is *out of bounds*, and a define is where that
        // becomes observable without a method to throw first: `Object.defineProperty` converts its
        // key by running the program's own `toString`, which is free to resize the buffer between
        // the view being handed over and the index being tested.
        if let Some(view) = self.typed_view(object)
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            let Ok(at) = index else {
                return DefineOutcome::Refused;
            };
            // An element is a writable, enumerable, configurable data property and can be nothing
            // else, so a descriptor asking for an accessor or for any other attributes is refused.
            // One that asks only for a *value* is the ordinary write.
            if descriptor.getter.is_some()
                || descriptor.setter.is_some()
                || descriptor.writable == Some(false)
                || descriptor.enumerable == Some(false)
                || descriptor.configurable == Some(false)
            {
                return DefineOutcome::Refused;
            }
            if let Some(value) = descriptor.value {
                // §10.4.5.3 step 1.b.v hands the value to §10.4.5.16, whose conversion is chosen by
                // the array's `[[ContentType]]` — and the two numeric types refuse each other there
                // unconditionally, whatever the value is. §7.1.4 `ToNumber` throws for *every*
                // BigInt and §7.1.13 `ToBigInt` for *every* Number, so this is a question about the
                // two types and not about the two values, and it can be asked without an
                // interpreter to run a coercion with.
                //
                // A **throw** and not a refusal, which a program can tell apart:
                // `Reflect.defineProperty(new BigInt64Array(1), 0, {value: 1})` raises a TypeError
                // where the same call at an out-of-range index quietly answers `false`.
                let holds_big = view.element.is_some_and(Element::holds_big);
                let crossed = match value {
                    Value::BigInt(_) => !holds_big,
                    Value::Number(_) => holds_big,
                    // Every other type has a conversion to *both*, so neither is refused here.
                    _ => false,
                };
                if crossed {
                    return DefineOutcome::WrongContent;
                }
                // A define carries a value that is already a Value, so there is no conversion to
                // run here — anything that is neither Number nor BigInt writes as `NaN` would,
                // which is what `ToNumber` of it would give for the types a define can carry
                // without a coercion step of its own.
                let numeric = self.as_numeric(value).unwrap_or(Numeric::Number(f64::NAN));
                let clamped = self.object(object).is_some_and(Object::is_clamped);
                self.set_element(view, at, &numeric, clamped);
            }
            return DefineOutcome::Defined;
        }
        // §10.4.3.3 — a define at one of a String object's characters never stores anything. It
        // is allowed only when it describes the property that is already there, and refused
        // otherwise, which is what makes `s[0] = "z"` do nothing at all.
        if let Some(data) = self.object(object).and_then(Object::string_data)
            && let Some(current) = string_object::character(self, data, key)
        {
            return DefineOutcome::from(string_object::define_is_allowed(
                self, &current, descriptor,
            ));
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
    pub fn has_property(&mut self, object: ObjectId, key: PropertyKey) -> bool {
        self.find_own(object, key).is_some()
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// The property *and* which object it came from, because that is what `[[Get]]` needs: an
    /// accessor's getter is called with the object it was found on as its receiver.
    /// An object's own property, with §10.4.4's map consulted — `[[GetOwnProperty]]`.
    ///
    /// The same answer as the object's own table for everything but a joined argument index,
    /// where the *value* comes from the parameter instead. §10.4.4.1 says exactly this: the
    /// descriptor is the ordinary one with its value replaced, which is why
    /// `Object.getOwnPropertyDescriptor(arguments, 0)` reports a data property and not the
    /// accessor the specification's own note implements the map with.
    ///
    /// `&mut` for the sake of one kind of object: a `BigInt64Array`'s element is a BigInt, which
    /// lives in the heap, so reading one out of a buffer allocates. Every caller of this and of
    /// [`Heap::find_own`] therefore needs a mutable heap for what is otherwise a pure question.
    pub fn own_property(&mut self, object: ObjectId, key: PropertyKey) -> Option<Property> {
        // §10.4.5.1 — a TypedArray's elements are answered from the buffer and are never stored, so
        // this comes *before* the table rather than after it: a canonical numeric index is an
        // element whatever the table happens to hold, and one out of range is absent.
        //
        // The view is copied out before anything else is asked of the object, because reading an
        // element needs the heap mutably and a borrow of the object would still be alive.
        // `any_view` and not the stored one: a view that tracks a resizable buffer has no length
        // of its own, and reading the stale number would answer elements past the end of a buffer
        // that has been shrunk — an out-of-range read that looks exactly like a valid one.
        // §10.4.6.5 — a namespace's exported names are answered from the exporting module's slots
        // and nothing is ever stored for them, so this comes before the table for the reason a
        // TypedArray's elements do. A Symbol falls through to the object, which is where
        // `@@toStringTag` is.
        if self.is_namespace(object) && key.as_string().is_some() {
            return self.namespace_property(object, key);
        }
        let element_view = self.any_view(object).filter(|view| view.element.is_some());
        if let Some(view) = element_view
            && let Some(at) = typed::index_of(self, key, view.count())
        {
            return at.ok().and_then(|at| self.element_property(view, at));
        }
        let found = self.object(object)?;
        let Some(property) = found.get_own_property(key).copied() else {
            // §10.4.3.5 — nothing stored, which for a String object is where its characters are.
            return string_object::character(self, found.string_data()?, key);
        };
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
    pub fn new_arguments(&mut self, prototype: ObjectId, call: &Incoming<'_>) -> ObjectId {
        let &Incoming {
            environment,
            values,
            parameters,
            callee,
            thrower,
            mapped,
        } = call;
        let object = self.new_object(Some(prototype));
        for (at, value) in values.iter().enumerate() {
            let index = u32::try_from(at).unwrap_or(u32::MAX);
            let key = self.index_key(index);
            self.define_own_property(object, key, &PropertyDescriptor::data(*value));
        }
        // §10.4.4.4 step 14 — `length` is an ordinary §17 property: writable and configurable,
        // and never enumerable, so `for`-`in` over `arguments` walks the indices and nothing else.
        let key = PropertyKey::from_units(self, &"length".encode_utf16().collect::<Vec<_>>());
        self.define_own_property(
            object,
            key,
            &PropertyDescriptor {
                enumerable: Some(false),
                ..PropertyDescriptor::data(Value::Number(values.len() as f64))
            },
        );
        let key = PropertyKey::from_units(self, &"callee".encode_utf16().collect::<Vec<_>>());
        let callee = match mapped {
            // §10.4.4.4 step 15 — the function itself, on a mapped object.
            true => PropertyDescriptor {
                enumerable: Some(false),
                ..PropertyDescriptor::data(Value::Object(callee))
            },
            // §10.4.4.6 step 6 — and on an *unmapped* one it is poisoned: an accessor pair of
            // %ThrowTypeError% for both halves, so reading it or writing it throws. That is a
            // deliberate refusal rather than an omission — a function with a default parameter is
            // ES2015 code, and `arguments.callee` is the idiom ES2015 was closing off.
            false => PropertyDescriptor {
                getter: Some(Value::Object(thrower)),
                setter: Some(Value::Object(thrower)),
                enumerable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        };
        self.define_own_property(object, key, &callee);
        // §10.2.11 step 22 — the map is only made for a *simple* parameter list. Anything else
        // gets §10.4.4.4's unmapped object: a parameter that a default filled in is not a slot an
        // index could stand for, and joining them would make `arguments[0] = 1` reach past the
        // code that decided what the parameter was.
        //
        // Joined *after* the properties are made, because making them goes through the define
        // below — and a define on a joined index writes through to a parameter instead.
        //
        // The slot is present either way, and that is not a detail: §20.1.3.6 step 8 tags an
        // object `Arguments` because it *has* a `[[ParameterMap]]`, not because the map joins
        // anything. An unmapped object gets one that joins nothing, which is what §10.4.4.6's
        // "set to undefined" behaves as.
        let joined = match mapped {
            true => parameters.min(values.len()),
            false => 0,
        };
        if let Some(found) = self.object_mut(object) {
            found.arguments = Some(Box::new(ArgumentsMap::new(environment, joined)));
        }
        object
    }

    /// §10.4.5.4 — whether a prototype walk for `key` must stop at this object.
    ///
    /// True only for a canonical numeric index of a TypedArray, which is an *element* whether or
    /// not the array has one: `ta[99]` on a short array is `undefined` and never the property
    /// somebody put at `Int32Array.prototype[99]`. Every walk in the engine has to know this, and
    /// the ones in [`crate::vm::Vm`] cannot use [`Heap::find_own`] to learn it because they have a
    /// proxy trap to ask at each step.
    #[must_use]
    pub fn walk_stops_here(&self, object: ObjectId, key: PropertyKey) -> bool {
        self.any_view(object).is_some_and(|view| {
            view.element.is_some() && typed::index_of(self, key, view.count()).is_some()
        })
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// The property *and* which object it came from, since an accessor's getter is called with
    /// that object as its receiver.
    ///
    /// Asked through [`Heap::own_property`] rather than the object's own table, so that a joined
    /// argument index answers with its parameter's value however the read arrived.
    pub fn find_own(&mut self, object: ObjectId, key: PropertyKey) -> Option<(ObjectId, Property)> {
        // §10.4.5.4 — a canonical numeric index of a TypedArray never reaches the prototype, even
        // when the array does not have it. `ta[99]` on a short array is `undefined` and not an
        // inherited property, which is the whole reason this stops here rather than answering
        // `None` and letting the walk continue: a program that puts something at
        // `Int32Array.prototype[9]` must not have it show up as an element.
        // `any_view` and not the stored one: a view that tracks a resizable buffer has no length
        // of its own, and reading the stale number would answer elements past the end of a buffer
        // that has been shrunk — an out-of-range read that looks exactly like a valid one.
        let element_view = self.any_view(object).filter(|view| view.element.is_some());
        if let Some(view) = element_view
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            return index
                .ok()
                .and_then(|at| self.element_property(view, at))
                .map(|property| (object, property));
        }
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

    /// §7.3.29 `PrivateFieldAdd` — add a private field, answering whether it was not already there.
    ///
    /// `false` means the object already carries this Private Name, which §7.3.29 makes a TypeError.
    /// Reachable from source in exactly one way: a constructor that calls itself on the same object,
    /// as in `class C { #x; constructor() { C.prototype.constructor.call(this); } }` — so the guard is
    /// not defensive, it is the specification's step 3.
    pub fn add_private_field(&mut self, object: ObjectId, name: SymbolId, value: Value) -> bool {
        self.add_private_element(object, name, PrivateElement::Field(value))
    }

    /// §7.3.30 `PrivateMethodOrAccessorAdd` — add a method or accessor, answering as §7.3.29 does.
    ///
    /// The same operation and the same one failure, which is why they share a body: the *kind* is all
    /// that differs, and §7.3.30's own step 2 refuses an existing name in the same words §7.3.29 does.
    pub fn add_private_element(
        &mut self,
        object: ObjectId,
        name: SymbolId,
        element: PrivateElement,
    ) -> bool {
        let Some(object) = self.objects.get_mut(object) else {
            return false;
        };
        let elements = object.private.get_or_insert_with(Vec::new);
        if elements.iter().any(|(key, _)| *key == name) {
            return false;
        }
        elements.push((name, element));
        true
    }

    /// §7.3.32 `PrivateSet` — write a private field that is already there, answering whether it was.
    ///
    /// It does **not** create. That is the whole difference from a property: `this.#x = 1` on an
    /// object with no `#x` is a TypeError rather than a new field, which is what makes the set of
    /// private names an object carries fixed at construction and usable as a brand.
    pub fn set_private_field(&mut self, object: ObjectId, name: SymbolId, value: Value) -> bool {
        let Some(object) = self.objects.get_mut(object) else {
            return false;
        };
        let Some(elements) = object.private.as_mut() else {
            return false;
        };
        match elements.iter_mut().find(|(key, _)| *key == name) {
            // §7.3.32 step 3 — a *field* is the only kind a write may reach through here. A method
            // refuses outright, and an accessor is the interpreter's business because its setter has
            // to be called; both are answered before this is reached, so this arm is a field or the
            // caller has not read the kind.
            Some((_, PrivateElement::Field(held))) => {
                *held = value;
                true
            }
            _ => false,
        }
    }

    /// §9.1.1.3's `MakeMethod` — record which object a function was defined on.
    ///
    /// Not a property and not observable: no script can read `[[HomeObject]]` by any means, and the
    /// only thing that consults it is `super`.
    ///
    /// A handle to nothing is ignored rather than reported. Every caller passes a function it made a
    /// moment earlier, so there is no state in which the answer could be acted on — and a `bool`
    /// nobody could ever see be `false` is a branch no test can pin, which mutation coverage said by
    /// surviving a flip of it.
    pub fn set_home_object(&mut self, function: ObjectId, home: ObjectId) {
        if let Some(object) = self.objects.get_mut(function) {
            object.home = Some(home);
        }
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
