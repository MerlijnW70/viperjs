//! §10.1's internal methods, as a running program reaches them.
//!
//! `[[Get]]`, `[[Set]]`, `[[Delete]]` and `[[HasProperty]]` — the four that a property access
//! compiles to. They live with the interpreter rather than with [`crate::heap::Object`] for one
//! reason: each may **throw**, and what a throw is made of belongs to a realm. The heap's own
//! `define_own_property` answers a Boolean and needs none of that.

use super::Vm;
use crate::heap::{DefineOutcome, Heap, PropertyDescriptor, PropertyKey, PropertyKind, StringId};
use crate::value::{Abrupt, Completion, Value};

/// What `[[Set]]` answers, out of what the define came to.
///
/// The Boolean is thrown away by sloppy code and turned into a TypeError by strict code, so a
/// refusal is not this function's business. §10.4.2.4 step 2 is: an array length that is not an
/// integer index **throws**, and it is the one assignment in the language that does. §10.4.5.16 is
/// the other — which is why a define answers four things rather than two.
///
/// The same table `Reflect.defineProperty` needs, and shared with it rather than written twice.
fn stored(outcome: DefineOutcome) -> Completion<Value> {
    crate::builtins::object::define_answer(outcome)
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

    /// `super.x` — §13.3.7.1, which finds on one object and calls back on another.
    ///
    /// The lookup walks from `base`, and an accessor found anywhere along it is called with
    /// `receiver` as its `this`. That is the whole of what makes this different from
    /// [`Vm::get_property`], which passes the base for both — and the difference is observable
    /// exactly when a parent's getter reads a field of the instance.
    ///
    /// No primitive case, unlike the ordinary read: a super base is `home.[[GetPrototypeOf]]()`, so it
    /// is an object or it is `null`, and `null` is the TypeError below.
    pub(crate) fn get_super(
        &mut self,
        base: Value,
        receiver: Value,
        key: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let key = self.property_key(key, heap)?;
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot read a property of `super` when there is no superclass",
            ));
        };
        // Absent is `undefined` rather than an error, as any read is: `super.nothing` in a class whose
        // parent has no such property is not a mistake the language reports.
        let Some((_, property)) = heap.find_own(object, key) else {
            return Ok(Value::Undefined);
        };
        match property.kind {
            PropertyKind::Data { value, .. } => Ok(value),
            // §10.1.8.1 step 8 — a getter with nothing behind it answers `undefined`, which is not
            // the same as the property being absent and is reached by a different route.
            PropertyKind::Accessor {
                getter: Value::Undefined,
                ..
            } => Ok(Value::Undefined),
            PropertyKind::Accessor { getter, .. } => self.call_value(getter, receiver, &[], heap),
        }
    }

    /// `super.x = v` — §13.3.7.1's write, which lands on the receiver rather than on the base.
    ///
    /// Two rules, and both follow from the receiver being `this`. An inherited setter is called with
    /// `this`, so it writes wherever it means to. With no setter the value is defined on **`this`** and
    /// not on the base — so `super.x = 1` in a method leaves an own `x` on the instance and the parent
    /// prototype is untouched, which is the same rule an ordinary assignment through a prototype
    /// follows.
    pub(crate) fn set_super(
        &mut self,
        base: Value,
        receiver: Value,
        key: Value,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let key = self.property_key(key, heap)?;
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot write a property of `super` when there is no superclass",
            ));
        };
        if let Some((_, property)) = heap.find_own(object, key) {
            match property.kind {
                // A property with a getter and no setter refuses the write, silently in sloppy code.
                PropertyKind::Accessor {
                    setter: Value::Undefined,
                    ..
                } => return Ok(value),
                PropertyKind::Accessor { setter, .. } => {
                    self.call_value(setter, receiver, &[value], heap)?;
                    return Ok(value);
                }
                // A non-writable data property refuses it too, on the same terms.
                PropertyKind::Data {
                    writable: false, ..
                } => return Ok(value),
                PropertyKind::Data { .. } => {}
            }
        }
        // §10.1.9.2 step 4 — the value is filed on the *receiver*, through the ordinary path so that
        // an exotic receiver (an Array's `length`, an arguments object's map) behaves as it should.
        self.set_property_key(receiver, key, value, heap)
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
        for key in self.own_keys_through(from, heap)? {
            if refused.contains(&key) {
                continue;
            }
            if !self
                .own_property_through(from, key, heap)?
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
            Value::BigInt(_) => heap.new_wrapper(self.realm.bigint_prototype(), value),
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
        self.get_through(base, key, base, heap)
    }

    /// The same read, with the receiver named separately — §10.1.8.1's third argument.
    ///
    /// Every ordinary read passes the same value twice: the object being read *is* the one a getter
    /// should see as `this`. §28.1.5's `Reflect.get` is the one place they differ, and a `Proxy`'s
    /// `get` trap is why — it forwards to its target and must not tell the getter that the target
    /// was what the program asked about.
    pub(crate) fn get_through(
        &mut self,
        base: Value,
        key: PropertyKey,
        receiver: Value,
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
            Value::BigInt(_) => self.realm.bigint_prototype(),
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
        // §10.1.8.1 step 3 — the chain, walked here rather than in the heap, because §10.5.8
        // says a proxy *anywhere* on it answers the whole `[[Get]]` with its trap. The heap's own
        // walk cannot ask a trap, and a proxy is invisible to it: it has no own properties, so a
        // chain running through one would simply read past it.
        //
        // A loop rather than the specification's recursion into the parent's `[[Get]]`, because a
        // chain is as long as a program makes it (DR-0002). The receiver does not change as it
        // goes, which is what makes the two shapes the same.
        let mut walk = object;
        // Counted for the same reason [`crate::heap::Heap::find_own`] counts: the chain cannot be
        // a cycle, and DR-0002 is a claim that being wrong about that does not hang.
        for _ in 0..crate::heap::MAX_PROTOTYPE_CHAIN {
            if let Some(answer) = self.proxy_get(walk, key, receiver, heap)? {
                return Ok(answer);
            }
            // §10.4.6.8 — a namespace's exported name is read out of the exporting module's slot,
            // and a binding still in its dead zone is a **ReferenceError** rather than `undefined`.
            // That is the one thing about a namespace only the interpreter can answer, which is why
            // the read is here and the descriptor is in the heap.
            if let Some(export) = heap.namespace_export(walk, key) {
                return match export {
                    crate::heap::Export::Value(value) => Ok(value),
                    crate::heap::Export::Uninitialised => Err(Abrupt::reference_error(
                        "a module binding was read before its module gave it a value",
                    )),
                };
            }
            if let Some(property) = heap.own_property(walk, key) {
                return match property.kind {
                    PropertyKind::Data { value, .. } => Ok(value),
                    // §10.1.8.1 steps 5 and 6 — an accessor with no getter answers `undefined`,
                    // and one with a getter has it **called**, with the object the property was
                    // read *through* as its receiver rather than the one it was found on. That is
                    // what makes an accessor on a prototype see the instance.
                    PropertyKind::Accessor {
                        getter: Value::Undefined,
                        ..
                    } => Ok(Value::Undefined),
                    PropertyKind::Accessor { getter, .. } => {
                        self.call_value(getter, receiver, &[], heap)
                    }
                };
            }
            // §10.4.5.4 — a canonical numeric index of a TypedArray never reaches the prototype,
            // even when the array does not have it.
            if heap.walk_stops_here(walk, key) {
                return Ok(Value::Undefined);
            }
            // A property that is nowhere on the chain is `undefined`, not an error. That is the
            // whole reason `o.missing` is a value and `missing` is a ReferenceError.
            let Some(next) = heap.object(walk).and_then(crate::heap::Object::prototype) else {
                return Ok(Value::Undefined);
            };
            walk = next;
        }
        Ok(Value::Undefined)
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
        self.set_through(base, key, value, base, heap)
    }

    /// §10.4.2.4 `ArraySetLength` steps 3 to 5 — the conversions, run where an interpreter is.
    ///
    /// `Heap::set_array_length` cannot do this. `ToUint32` and `ToNumber` both run a script's own
    /// `valueOf`, and the heap has no machine to re-enter — DR-0011's seam. So the value is settled
    /// here and the heap is handed a Number, which is what its existing check was always written
    /// against.
    ///
    /// **Both conversions, on the same value.** Steps 3 and 4 each convert `Desc.[[Value]]`, so a
    /// `valueOf` runs *twice* — which is observable and is what the clause says. Doing it once and
    /// reusing the answer would be a tidier engine and a different language.
    ///
    /// The RangeError is step 5's: a value that survives both conversions and disagrees with itself
    /// is not a length. A value that survives neither throws whatever the conversion threw, which
    /// is how `[].length = 1n` becomes §7.1.6's **TypeError** rather than a refusal.
    ///
    /// `None` when this is not an array's `length` at all, which is every other write.
    pub(super) fn settled_array_length(
        &mut self,
        object: crate::heap::ObjectId,
        key: PropertyKey,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        if !heap
            .object(object)
            .is_some_and(crate::heap::Object::is_array)
            || key != heap.length_key()
        {
            return Ok(None);
        }
        // §7.1.6 `ToUint32`, which is `ToNumber` and then the modulo — so it throws for a BigInt
        // exactly as `ToNumber` does, which is where `[].length = 1n`'s TypeError comes from.
        let length = self.to_number(value, heap)?;
        let length = Value::Number(length).to_uint32(heap)?;
        let number = self.to_number(value, heap)?;
        // `SameValueZero`, which for two Numbers is equality with `NaN` equal to itself — and a
        // `NaN` cannot survive `ToUint32` anyway, so what this really refuses is `1.5`, `-1` and
        // anything past 2^32-1.
        if f64::from(length) != number {
            return Err(Abrupt::range_error(
                "an array length must be an integer index",
            ));
        }
        Ok(Some(Value::Number(f64::from(length))))
    }

    /// The same write, with the receiver named separately — §10.1.9.2's fourth argument.
    ///
    /// Two things read it: a setter is called with it as `this`, and a property that shadows an
    /// inherited one is created *on it*. Every ordinary assignment passes the same value twice;
    /// §28.1.9's `Reflect.set` is the one place they differ, and it is what lets a `Proxy` forward
    /// a write to its target while the property lands on the proxy.
    pub(crate) fn set_through(
        &mut self,
        base: Value,
        key: PropertyKey,
        value: Value,
        receiver: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = base else {
            return Err(Abrupt::type_error(
                "cannot set a property of something that is not an object",
            ));
        };
        // §10.5.9 — the same, and the answer is whether the write was accepted rather than the
        // value, which is what §6.2.5.5's `Set` reports.
        if let Some(accepted) = self.proxy_set(object, key, value, receiver, heap)? {
            return Ok(Value::Boolean(accepted));
        }
        // §10.4.6.9 — a namespace refuses every write, whatever the key and whatever it holds. Its
        // exports report `writable: true` all the same: the attribute describes the *binding*, which
        // the exporting module may still assign to, not what may be done through this object.
        if heap.is_namespace(object) {
            return Ok(Value::Boolean(false));
        }
        // Whether this write is going to the object it was looked up on — §10.4.5.4 step 2.b.i's
        // `SameValue(O, Receiver)` and §10.1.9.2 step 2's "the receiver is the target". For every
        // assignment the program can write they are the same object; `Reflect.set` and a write
        // reaching a prototype are where they come apart, and both clauses below turn on it.
        let to_itself = matches!(receiver, Value::Object(id) if id == object);
        // §10.4.2.4 — an array's `length` is settled before the heap sees it, for the same reason
        // the TypedArray write below is: the conversion runs a script's `valueOf` and the heap has
        // no interpreter to run one with. Only when the write lands here: with another receiver
        // §10.1.9.2 step 3.f files the value on *that* object, and `ArraySetLength` never runs.
        let value = match self.settled_array_length(object, key, value, heap)? {
            Some(settled) if to_itself => settled,
            _ => value,
        };
        // §10.4.5.4 — a write to a canonical numeric index goes into the buffer, and one that is
        // out of range is **discarded**: not an error, in strict mode or sloppy, because a
        // TypedArray's length cannot change and there is nowhere for the value to go. It is the one
        // assignment in the language that fails silently by design.
        //
        // This used to be reached before the receiver was consulted at all, on the reading that
        // "the element belongs to the buffer and no receiver can move it elsewhere" — written as a
        // rule, and citing §10.4.5.5, which is the *define*. Step 2.b.i of the `[[Set]]` clause says
        // `SameValue(O, Receiver)`, so `Reflect.set(ta, 0, v, {})` leaves `ta` alone and gives the
        // plain object the property. Reading it the other way also converted `v`, which step 2.b.ii
        // does not.
        if let Some(index) = heap.typed_index(object, key) {
            if to_itself {
                // §10.4.5.16 step 1 — *which* conversion is chosen by the array's `[[ContentType]]`:
                // §7.1.13 `ToBigInt` for the two 64-bit kinds and §7.1.4 `ToNumber` for the other
                // nine. That is where the two numeric types stop mixing, and it is a throw rather
                // than a truncation: `new BigInt64Array(1)[0] = 1` is a TypeError.
                //
                // Run **before** the index is judged, and for an out-of-range index too, because
                // §10.4.5.16 step 1 converts first: `ta[99] = {valueOf(){ throw 0 }}` throws even
                // though the write itself would have gone nowhere.
                let numeric = self.to_numeric_of(object, value, heap)?;
                // The conversion can detach the buffer, so the write is attempted afterwards and
                // simply finds nothing to write to — which is the same answer as an out-of-range
                // index and is what §10.4.5.16 step 3 means by leaving the buffer alone.
                if let Ok(at) = index {
                    heap.write_element(object, at, &numeric);
                }
                return Ok(Value::Boolean(true));
            }
            // Step 2.b.ii — an index this array does not have, written through some other receiver,
            // is reported as accepted and goes **nowhere**: the receiver is never told about it and
            // the value is not converted. A valid index falls through to the ordinary walk below,
            // which files it on the receiver like any other shadowable data property.
            if index.is_err() {
                return Ok(Value::Boolean(true));
            }
        }
        // §10.1.9.2 — an *inherited* accessor is called, and an inherited non-writable data
        // property refuses the write. An inherited writable one does not: the value is filed on
        // the receiver, which is what makes a prototype's property shadowable.
        // The chain is walked here for the same reason `get_through` walks it: §10.5.9 says a
        // proxy on it answers the whole `[[Set]]`, and the heap's walk would read straight past
        // one. `find_own`'s answer, gathered with the traps in the way.
        let mut found = None;
        let mut walk = object;
        for _ in 0..crate::heap::MAX_PROTOTYPE_CHAIN {
            if let Some(accepted) = self.proxy_set(walk, key, value, receiver, heap)? {
                return Ok(Value::Boolean(accepted));
            }
            if let Some(property) = heap.own_property(walk, key) {
                found = Some((walk, property));
                break;
            }
            if heap.walk_stops_here(walk, key) {
                break;
            }
            let Some(next) = heap.object(walk).and_then(crate::heap::Object::prototype) else {
                break;
            };
            walk = next;
        }
        if let Some((owner, property)) = found {
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
                    self.call_value(setter, receiver, &[value], heap)?;
                    return Ok(Value::Boolean(true));
                }
                PropertyKind::Data {
                    writable: false, ..
                } => {
                    return Ok(Value::Boolean(false));
                }
                // An own writable data property is changed in place, keeping its attributes:
                // assignment never makes a property enumerable that was not. Only when the write
                // is going *here* — §10.1.9.2 step 2 looks the property up on the target and then
                // writes to the **receiver**, and for every ordinary assignment those are the same
                // object. `Reflect.set` is where they are not.
                PropertyKind::Data { .. } if owner == object && to_itself => {
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
        // **on the receiver**, which for every ordinary assignment is the object itself and for
        // `Reflect.set` may be something else entirely — §10.1.9.2 step 3, and the reason a write
        // forwarded to a target lands where the program asked rather than where it was looked up.
        let Value::Object(landing) = receiver else {
            return Ok(Value::Boolean(false));
        };
        // §10.1.9.2 steps 3.d and 3.f both go through the *receiver's own* internal methods, and a
        // TypedArray receiver answers those from its buffer rather than from a property table. So
        // `Reflect.set(short, 0, v, longer)` writes an element into `longer`, and against a receiver
        // too short for the index §10.4.5.5 step 1.a.i refuses the `CreateDataProperty` — which is
        // the write returning `false`, with the value never converted. Asking `get_own_property`
        // below would find neither, and the value would land in the table beside the elements where
        // nothing can ever read it again.
        if let Some(index) = heap.typed_index(landing, key) {
            let Ok(at) = index else {
                return Ok(Value::Boolean(false));
            };
            let numeric = self.to_numeric_of(landing, value, heap)?;
            heap.write_element(landing, at, &numeric);
            return Ok(Value::Boolean(true));
        }
        // §10.1.9.2 step 2.c — what the *receiver* already has decides how it is written. An
        // accessor or a non-writable property there refuses the write outright: the value came
        // looking for a home and that one is taken. Anything else keeps its attributes, so a write
        // through a receiver never makes a property enumerable that was not.
        let existing = heap
            .object(landing)
            .and_then(|found| found.get_own_property(key));
        let descriptor = match existing.map(|property| property.kind) {
            Some(
                PropertyKind::Accessor { .. }
                | PropertyKind::Data {
                    writable: false, ..
                },
            ) => {
                return Ok(Value::Boolean(false));
            }
            Some(PropertyKind::Data { .. }) => PropertyDescriptor {
                value: Some(value),
                ..PropertyDescriptor::EMPTY
            },
            // A new property, or one shadowing an inherited writable one. Either way it is created
            // with the three attributes assignment always gives.
            None => PropertyDescriptor::data(value),
        };
        stored(heap.define_property_outcome(landing, key, &descriptor))
    }
    /// §13.10.2's `InstanceofOperator` — the operator, which is mostly a lookup.
    ///
    /// Step 2 asks the right operand for `%Symbol.hasInstance%` and, finding one, **calls it and
    /// believes it**. That is how a class says what `instanceof` means for it, and it is not a rare
    /// path: §20.2.3.6 puts the default method on `Function.prototype`, so every ordinary function
    /// goes through it too and the walk below is reached by way of a call rather than directly.
    ///
    /// The doc here used to say the step would be added "when Symbols arrive". They arrived; the
    /// step had not, so `1 instanceof {[Symbol.hasInstance]: f}` threw instead of asking `f`.
    ///
    /// Two TypeErrors of its own, and they are different sentences because they are different
    /// mistakes: a right operand that is not an object at all (step 1), and one that is an object,
    /// has no `@@hasInstance`, and is not callable (step 4).
    pub(crate) fn instance_of(
        &mut self,
        value: Value,
        target: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        // Step 1.
        let Value::Object(constructor) = target else {
            return Err(Abrupt::type_error(
                "the right operand of instanceof must be an object",
            ));
        };
        // Steps 2 and 3.
        if let Some(handler) = self.has_instance_handler(target, heap)? {
            let answered = self.call_value(handler, target, &[value], heap)?;
            return Ok(Value::Boolean(answered.to_boolean(heap)));
        }
        // Step 4 — reached by an object with no `@@hasInstance` anywhere on its chain, which for a
        // function means one whose prototype chain does not reach `Function.prototype`.
        if !heap
            .object(constructor)
            .is_some_and(|object| object.call().is_some())
        {
            return Err(Abrupt::type_error(
                "the right operand of instanceof is not callable",
            ));
        }
        // Step 5.
        self.ordinary_has_instance(target, value, heap)
    }

    /// §7.3.22 `OrdinaryHasInstance` — the walk `instanceof` means when nothing overrides it.
    ///
    /// # What it asks, and what it does not
    ///
    /// It walks `value`'s prototype chain looking for the *object* `target.prototype` holds. So it
    /// is a question about the chain and never about which constructor was called: reassign
    /// `C.prototype` and every object made before the reassignment stops being an instance of `C`,
    /// which is not a bug and is why `instanceof` is unreliable across frames.
    ///
    /// # A bound function answers for its target, and why that is a loop
    ///
    /// Step 2 hands a bound function's `[[BoundTargetFunction]]` back to §13.10.2 — so the
    /// target's own `@@hasInstance` decides, and `x instanceof f.bind()` is `x instanceof f`.
    /// Written as the specification writes it that is mutual recursion, and a chain of ten
    /// thousand `bind` calls is ten thousand Rust frames, which DR-0002 does not allow input to
    /// ask for. So the chain is unwound in a loop instead, and the one thing the loop has to keep
    /// is the reason the recursion existed: at each target, a `@@hasInstance` that is **not** the
    /// default is called and believed. The default is this function, so unwinding past it is the
    /// same answer without the regress.
    ///
    /// Bounded for the reason `Vm::enter_bound` is bounded: no `bind` can make a cycle, since it
    /// binds a function that already exists, but a hand-built heap can.
    pub(crate) fn ordinary_has_instance(
        &mut self,
        target: Value,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let mut target = target;
        // A `for` over a constant rather than a counter and a comparison, which is the shape
        // `Vm::enter_bound` already uses for the same guard: the bound is not a rule about
        // programs — DR-0013's heap gives out at a few thousand bound functions long before this
        // — it is the answer for a hand-built heap pointing a bound function at itself. Written as
        // a comparison it is a branch nothing can take, and mutation coverage duly survived it.
        let constructor = 'unwind: {
            for _ in 0..super::call::MAX_CALL_DEPTH {
                // Step 1 — a non-callable answers **false** rather than throwing, because §13.10.2
                // step 4 has already thrown for the one route that could reach it with one. This is
                // reachable as `Function.prototype[Symbol.hasInstance].call(1, x)`.
                let Value::Object(object) = target else {
                    return Ok(Value::Boolean(false));
                };
                let Some(callable) = heap
                    .object(object)
                    .and_then(crate::heap::Object::call)
                    .cloned()
                else {
                    return Ok(Value::Boolean(false));
                };
                let crate::heap::Callable::Bound(bound) = callable else {
                    break 'unwind object;
                };
                let next = Value::Object(bound.target);
                match self.has_instance_handler(next, heap)? {
                    Some(handler) if !self.is_default_has_instance(handler, heap) => {
                        let answered = self.call_value(handler, next, &[value], heap)?;
                        return Ok(Value::Boolean(answered.to_boolean(heap)));
                    }
                    _ => target = next,
                }
            }
            return Err(Abrupt::type_error(
                "this bound function's chain of targets does not end",
            ));
        };
        // Step 3 — a primitive is an instance of nothing, and that is an *answer* rather than an
        // error. `1 instanceof Object` is `false`, not a mistake.
        let Value::Object(mut walk) = value else {
            return Ok(Value::Boolean(false));
        };
        let name = self.well_known("prototype", heap);
        // Step 4 reads it as a *property*, so a getter runs and may throw.
        let prototype = self.get_property(Value::Object(constructor), name, heap)?;
        // Step 5 — what `Object.create(null) instanceof f` reaches after `f.prototype = 1`.
        let Value::Object(prototype) = prototype else {
            return Err(Abrupt::type_error(
                "the prototype of the right operand of instanceof is not an object",
            ));
        };
        // Step 6, iteratively: a prototype chain is as long as a program makes it and DR-0002 does
        // not let input decide how much Rust stack is used.
        loop {
            let Some(next) = self.prototype_through(walk, heap)? else {
                return Ok(Value::Boolean(false));
            };
            if next == prototype {
                return Ok(Value::Boolean(true));
            }
            walk = next;
        }
    }

    /// §7.3.11 `GetMethod(target, %Symbol.hasInstance%)`.
    ///
    /// `undefined` and null both mean absent; anything else that is not callable is a TypeError
    /// rather than a silent fall through to the ordinary walk, which would make a misspelled
    /// override look as though it had worked.
    fn has_instance_handler(
        &mut self,
        target: Value,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        let Some(symbol) = self
            .realm
            .well_known(crate::builtins::well_known_at("hasInstance"))
        else {
            return Ok(None);
        };
        let found = self.get_property_key(target, PropertyKey::from_symbol(symbol), heap)?;
        if matches!(found, Value::Undefined | Value::Null) {
            return Ok(None);
        }
        if !heap.is_callable(found) {
            return Err(Abrupt::type_error("Symbol.hasInstance is not a function"));
        }
        Ok(Some(found))
    }

    /// Whether this handler is §20.2.3.6's, the one `Function.prototype` carries.
    ///
    /// Read off `Function.prototype` rather than remembered in the realm, and that is exact rather
    /// than convenient: §20.2.3.6 makes the property **neither writable nor configurable**, so no
    /// program can put anything else there and the value found is the intrinsic by construction.
    fn is_default_has_instance(&mut self, handler: Value, heap: &mut Heap) -> bool {
        // One chain rather than an early return for the missing Symbol: a realm always has the
        // well-known ones, so a `false` of its own would be a branch no input could take.
        self.realm
            .well_known(crate::builtins::well_known_at("hasInstance"))
            .map(PropertyKey::from_symbol)
            .and_then(|key| {
                heap.object(self.realm.function_prototype())
                    .and_then(|object| object.get_own_property(key))
            })
            .and_then(|property| match property.kind {
                crate::heap::PropertyKind::Data { value, .. } => Some(value),
                crate::heap::PropertyKind::Accessor { .. } => None,
            })
            .is_some_and(|default| matches!((default, handler), (Value::Object(a), Value::Object(b)) if a == b))
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
        // §10.5.10 — a proxy's `deleteProperty` trap, before the ordinary delete. Its answer is a
        // boolean, which is what `delete` evaluates to anyway.
        if let Value::Object(object) = base
            && let Some(answer) = self.proxy_delete(object, key, heap)?
        {
            return Ok(Value::Boolean(answer));
        }
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
        // §10.4.6.11 — an exported name may not be deleted. A key that is *not* an export is
        // absent, and deleting an absent property answers true, so only the export needs saying.
        if heap.namespace_export(object, key).is_some() {
            return Ok(Value::Boolean(false));
        }
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
        // §10.1.7.1's chain, walked here so that §10.5.7's trap is reached wherever the proxy is:
        // as the object asked about, or as something further along its prototype chain.
        let mut walk = object;
        for _ in 0..crate::heap::MAX_PROTOTYPE_CHAIN {
            if let Some(answer) = self.proxy_has(walk, key, heap)? {
                return Ok(answer);
            }
            // §10.4.6.7 — an export is present whether or not its module has reached the line
            // that gives it a value, so this asks the export list rather than for a descriptor:
            // asking for one in the dead zone throws, and `"x" in ns` does not.
            if heap.namespace_has(walk, key) {
                return Ok(true);
            }
            if heap.own_property(walk, key).is_some() {
                return Ok(true);
            }
            // §10.4.5.2 — the same stop as `[[Get]]`'s: an index a TypedArray does not have is
            // absent rather than inherited.
            if heap.walk_stops_here(walk, key) {
                return Ok(false);
            }
            let Some(next) = heap.object(walk).and_then(crate::heap::Object::prototype) else {
                return Ok(false);
            };
            walk = next;
        }
        Ok(false)
    }
}
