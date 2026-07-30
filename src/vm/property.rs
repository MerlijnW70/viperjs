//! §10.1's internal methods, as a running program reaches them.
//!
//! `[[Get]]`, `[[Set]]`, `[[Delete]]` and `[[HasProperty]]` — the four that a property access
//! compiles to. They live with the interpreter rather than with [`crate::heap::Object`] for one
//! reason: each may **throw**, and what a throw is made of belongs to a realm. The heap's own
//! `define_own_property` answers a Boolean and needs none of that.

use super::Vm;
use crate::heap::{DefineOutcome, Heap, PropertyDescriptor, PropertyKey, PropertyKind, StringId};
use crate::value::{Abrupt, Completion, ErrorKind, Value};

/// What `[[Set]]` answers, out of what the define came to.
///
/// The Boolean is thrown away by sloppy code and turned into a TypeError by strict code, so a
/// refusal is not this function's business. §10.4.2.4 step 2 is: an array length that is not an
/// integer index **throws**, and it is the one assignment in the language that does — which is
/// exactly why a define answers three things rather than two.
fn stored(outcome: DefineOutcome) -> Completion<Value> {
    match outcome {
        DefineOutcome::Defined => Ok(Value::Boolean(true)),
        DefineOutcome::Refused => Ok(Value::Boolean(false)),
        DefineOutcome::BadLength => Err(Abrupt::Raised(
            ErrorKind::Range,
            "an array length must be an integer index",
        )),
    }
}

impl Vm {
    /// `ToPropertyKey` (§7.1.19), for the keys that exist.
    ///
    /// A Symbol is a key as it stands; everything else becomes the String `ToString` writes, which
    /// is why `o[1]` and `o["1"]` are one property and `o[1.0]` is the same one again.
    pub(super) fn property_key(&mut self, key: Value, heap: &mut Heap) -> Completion<PropertyKey> {
        self.to_property_key(key, heap)
    }
    /// `[[Get]]` (§10.1.8) — the value of `base`'s `key`, its prototypes included.
    ///
    /// A base that is not an object is a **TypeError**. That is right for `null` and `undefined`
    /// and is *temporary* for everything else: §7.3.2 wraps a primitive in its own object first,
    /// so `"abc".length` works by way of `String.prototype` — and there is no `String.prototype`
    /// yet. The message says "an object" rather than naming the type, so it does not have to
    /// change when that arrives.
    pub(crate) fn get_property(
        &mut self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let key = self.property_key(key, heap)?;
        self.get_property_key(base, key, heap)
    }

    /// What a String primitive has of its own — §10.4.3.5's characters and §10.4.3.4's `length`.
    ///
    /// `None` when the key is neither, and then the read goes on to `String.prototype` as any
    /// other would. The object §7.3.2 says to make is never made: it would have exactly these
    /// properties and be thrown away, and `"abc".length` is common enough that building one each
    /// time would be a cost paid by every program that touches a string.
    fn string_own_property(
        &mut self,
        data: StringId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Option<Value> {
        let units = heap.string(data)?.len();
        if key == self.length_key(heap) {
            return Some(Value::Number(f64::from(u32::try_from(units).ok()?))); // DR-0012 caps a String far below `u32`
        }
        let index = key.as_array_index(heap)?;
        let character = heap.intern_character(data, index)?;
        Some(Value::String(character))
    }

    /// The key `"length"`, interned.
    fn length_key(&mut self, heap: &mut Heap) -> PropertyKey {
        PropertyKey::from_units(heap, &"length".encode_utf16().collect::<Vec<_>>())
    }

    /// §7.3.25 `CopyDataProperties` — what a rest property collects.
    ///
    /// Own enumerable properties of `source` that `excluded` does not name, in the order
    /// `[[OwnPropertyKeys]]` gives them, as a new ordinary object. Symbol keys are copied like any
    /// other: §7.3.25 asks about enumerability and not about the kind of key, which is why
    /// `{...rest}` carries a Symbol-keyed property across where `Object.keys` would not list it.
    ///
    /// A **get** per property, so a getter on the source runs here and what it answered is what
    /// the rest object holds — the same reading `Object.assign` takes, and the reason the result
    /// is values rather than accessors.
    pub(crate) fn copy_rest(
        &mut self,
        source: Value,
        excluded: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        let built = heap.new_object(Some(self.realm.object_prototype()));
        self.copy_data_properties(built, source, excluded, heap)?;
        Ok(Value::Object(built))
    }

    /// §7.3.25 `CopyDataProperties` — every own enumerable property of `source`, onto `target`.
    ///
    /// Shared by §14.3.3's object rest, which wants a fresh object to put them in, and §13.2.5's
    /// object spread, which wants them added to the literal being built. Writing the walk twice is
    /// how the two come to disagree about an accessor or a non-enumerable property.
    ///
    /// Step 3 — `undefined` and `null` are *skipped* rather than refused, which is why `{...null}`
    /// is an empty object and `var {...a} = null` is a TypeError: the difference is a
    /// `RequireCoercible` the pattern emits and the literal does not, not anything here.
    pub(crate) fn copy_data_properties(
        &mut self,
        target: crate::heap::ObjectId,
        source: Value,
        excluded: &[Value],
        heap: &mut Heap,
    ) -> Completion<()> {
        if matches!(source, Value::Undefined | Value::Null) {
            return Ok(());
        }
        let Value::Object(from) = self.object_for(source, heap)? else {
            return Ok(());
        };
        let mut refused = Vec::with_capacity(excluded.len());
        for key in excluded {
            refused.push(self.property_key(*key, heap)?);
        }
        for key in heap.own_property_keys(from) {
            if refused.contains(&key) {
                continue;
            }
            if !heap
                .own_property(from, key)
                .is_some_and(|property| property.enumerable)
            {
                continue;
            }
            let value = self.get_property_key(Value::Object(from), key, heap)?;
            let descriptor = PropertyDescriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            };
            heap.define_own_property(target, key, &descriptor);
        }
        Ok(())
    }

    /// `ToObject` (§7.1.18) — the object a primitive stands for.
    ///
    /// Named `object_for` rather than `to_object`: a `to_*` method that takes `self` by value
    /// is a conversion, and this converts its *argument* while borrowing the machine.
    ///
    /// `undefined` and `null` have none, which is step 1 and 2's TypeError. A String has one and
    /// praxis cannot make it yet: §10.4.3's String exotic object has an own property per index,
    /// which is a second exotic object and a slice of its own.
    pub(crate) fn object_for(&mut self, value: Value, heap: &mut Heap) -> Completion<Value> {
        let wrapped = match value {
            Value::Object(_) => return Ok(value),
            Value::Boolean(_) => heap.new_wrapper(self.realm.boolean_prototype(), value),
            Value::Number(_) => heap.new_wrapper(self.realm.number_prototype(), value),
            Value::String(data) => heap.new_string_object(self.realm.string_prototype(), data),
            Value::Symbol(_) => heap.new_wrapper(self.realm.symbol_prototype(), value),
            Value::Undefined | Value::Null => {
                return Err(Abrupt::type_error(
                    "undefined and null cannot be converted to an object",
                ));
            }
        };
        Ok(Value::Object(wrapped))
    }

    /// `[[Get]]` when the key is already a key.
    ///
    /// A global reference names its property in the bytecode, so it never had a *value* to
    /// convert. Splitting the conversion off means the two paths cannot drift on what a get is.
    pub(crate) fn get_property_key(
        &mut self,
        base: Value,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Value> {
        // §7.3.2 `GetV` — a primitive is not an error to read from. It is wrapped, and the read
        // goes to the wrapper. A wrapper's *own* properties are only ever the ones its kind gives
        // it, and Number and Boolean give none — so the prototype is consulted directly rather
        // than an object being made and thrown away on every `(1).toString()`.
        //
        // `undefined` and `null` are the two that really are errors, and §7.3.2 step 2 says so.
        let object = match base {
            Value::Object(object) => object,
            Value::Number(_) => self.realm.number_prototype(),
            Value::Boolean(_) => self.realm.boolean_prototype(),
            Value::Symbol(_) => self.realm.symbol_prototype(),
            // A String is the one primitive whose wrapper has own properties, so the shortcut has
            // to answer them itself before falling through to the prototype: §10.4.3.5's
            // characters, and the `length` §10.4.3.4 puts beside them.
            Value::String(data) => {
                if let Some(value) = self.string_own_property(data, key, heap) {
                    return Ok(value);
                }
                self.realm.string_prototype()
            }
            Value::Undefined | Value::Null => {
                return Err(Abrupt::type_error(
                    "cannot read a property of something that is not an object",
                ));
            }
        };
        // §10.1.8.1 step 3 — a property that is nowhere on the chain is `undefined`, not an
        // error. That is the whole reason `o.missing` is a value and `missing` is a ReferenceError.
        let Some((_, property)) = heap.find_own(object, key) else {
            return Ok(Value::Undefined);
        };
        match property.kind {
            PropertyKind::Data { value, .. } => Ok(value),
            // §10.1.8.1 steps 5 and 6 — an accessor with no getter answers `undefined`, and one
            // with a getter has it **called**, with the object the property was read *through*
            // as its receiver rather than the one it was found on. That is what makes an accessor
            // on a prototype see the instance, and it is the whole reason a getter is useful.
            PropertyKind::Accessor {
                getter: Value::Undefined,
                ..
            } => Ok(Value::Undefined),
            PropertyKind::Accessor { getter, .. } => self.call_value(getter, base, &[], heap),
        }
    }
    /// `[[Set]]` (§10.1.9) — put `value` under `key`, and answer whether it was allowed.
    ///
    /// The Boolean is thrown away by sloppy code and turned into a TypeError by strict code, which
    /// is why this answers rather than throwing: the caller knows which it is and this does not.
    pub(crate) fn set_property(
        &mut self,
        base: Value,
        key: Value,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let key = self.property_key(key, heap)?;
        self.set_property_key(base, key, value, heap)
    }

    /// `[[Set]]` when the key is already a key — see [`Vm::get_property_key`].
    pub(crate) fn set_property_key(
        &mut self,
        base: Value,
        key: PropertyKey,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot set a property of something that is not an object",
            ));
        };
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
                // §10.1.9.2 step 5 — the setter is called with the value, and its *answer* is
                // thrown away: a setter cannot refuse a write, it can only decline to record it.
                // The receiver is again the object the write went through.
                PropertyKind::Accessor { setter, .. } => {
                    self.call_value(setter, base, &[value], heap)?;
                    return Ok(Value::Boolean(true));
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
                    return stored(heap.define_property_outcome(object, key, &descriptor));
                }
                PropertyKind::Data { .. } => {}
            }
        }
        // A new property, or one that shadows an inherited writable one. Either way it is created
        // on the receiver with the three attributes assignment always gives.
        let descriptor = PropertyDescriptor::data(value);
        stored(heap.define_property_outcome(object, key, &descriptor))
    }
    /// §13.10.2's `InstanceofOperator`, by way of §7.3.22's `OrdinaryHasInstance`.
    ///
    /// # What it asks, and what it does not
    ///
    /// It walks `value`'s prototype chain looking for the *object* `target.prototype` holds. So it
    /// is a question about the chain and never about which constructor was called: reassign
    /// `C.prototype` and every object made before the reassignment stops being an instance of `C`,
    /// which is not a bug and is why `instanceof` is unreliable across frames.
    ///
    /// Three TypeErrors, and they are different sentences because they are different mistakes: a
    /// right operand that is not an object at all (§13.10.2 step 3), one that is an object but not
    /// callable (step 5), and a callable one whose `prototype` is not an object (§7.3.22 step 5) —
    /// the last being what `Object.create(null) instanceof f` after `f.prototype = 1` reaches.
    ///
    /// §13.10.2 step 4 looks for `@@hasInstance` first, which is how `Symbol.hasInstance` lets a
    /// class say what `instanceof` means for it. There are no Symbols yet, so every object takes
    /// the ordinary path; when they arrive this gains a step in front rather than changing.
    pub(crate) fn instance_of(
        &mut self,
        value: Value,
        target: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(constructor) = target else {
            return Err(Abrupt::type_error(
                "the right operand of instanceof must be an object",
            ));
        };
        if !heap
            .object(constructor)
            .is_some_and(|object| object.call().is_some())
        {
            return Err(Abrupt::type_error(
                "the right operand of instanceof is not callable",
            ));
        }
        // §7.3.22 step 3 — a primitive is an instance of nothing, and that is an *answer* rather
        // than an error. `1 instanceof Object` is `false`, not a mistake.
        let Value::Object(mut walk) = value else {
            return Ok(Value::Boolean(false));
        };
        let name = self.well_known("prototype", heap);
        let prototype = self.get_property(target, name, heap)?;
        let Value::Object(prototype) = prototype else {
            return Err(Abrupt::type_error(
                "the prototype of the right operand of instanceof is not an object",
            ));
        };
        // Iterative, because a prototype chain is as long as a program makes it and DR-0002 does
        // not let input decide how much Rust stack is used.
        loop {
            let Some(next) = heap.object(walk).and_then(|object| object.prototype()) else {
                return Ok(Value::Boolean(false));
            };
            if next == prototype {
                return Ok(Value::Boolean(true));
            }
            walk = next;
        }
    }

    /// A String value for a name the engine itself knows, for asking an object about it.
    fn well_known(&mut self, name: &str, heap: &mut Heap) -> Value {
        Value::String(heap.intern(&name.encode_utf16().collect::<Vec<_>>()))
    }

    /// `[[Delete]]` (§10.1.10) through §13.5.1's operator.
    pub(crate) fn delete_property(
        &mut self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot delete a property of something that is not an object",
            ));
        };
        let key = self.property_key(key, heap)?;
        self.delete_property_key(Value::Object(object), key, heap)
    }

    /// `[[Delete]]` when the key is already a key — see [`Vm::get_property_key`].
    pub(crate) fn delete_property_key(
        &mut self,
        base: Value,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot delete a property of something that is not an object",
            ));
        };
        // Own only: `delete` never reaches through a prototype, which is why deleting an
        // inherited property answers `true` and leaves it exactly where it was.
        // §10.4.4.5 step 4 — a deleted index stops being the parameter it was, whatever happens
        // to the property itself. Broken *before* the delete, because the delete may refuse and
        // the link is about the index rather than about what is at it.
        heap.unmap_argument(object, key);
        let gone = heap.delete_own_property(object, key);
        Ok(Value::Boolean(gone))
    }
    /// `[[HasProperty]]` (§10.1.7) through §13.10.1's `in`.
    pub(crate) fn has_property(
        &mut self,
        base: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            // §13.10.1 step 5 — `in` is the one operator that names the requirement out loud
            // rather than converting: `1 in 2` is a TypeError and not `false`.
            return Err(Abrupt::type_error(
                "the right operand of in must be an object",
            ));
        };
        let key = self.property_key(key, heap)?;
        let found = self.has_property_key(Value::Object(object), key, heap)?;
        Ok(Value::Boolean(found))
    }

    /// `[[HasProperty]]` when the key is already a key — see [`Vm::get_property_key`].
    ///
    /// Answers a Rust `bool` rather than a `Value`, because every caller with a key in hand is
    /// asking a question about the chain rather than evaluating `in`.
    pub(crate) fn has_property_key(
        &mut self,
        base: Value,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<bool> {
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "the right operand of in must be an object",
            ));
        };
        Ok(heap.has_property(object, key))
    }
}
