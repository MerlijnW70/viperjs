//! §20.1 — `Object`, and the surface every other object is described through.
//!
//! # Why this one, and why this much of it
//!
//! The conformance suite asked, and it asked precisely: `Object.defineProperty` was named by 742
//! failing tests and `Object.getOwnPropertyDescriptor` by 584, with `create` and
//! `defineProperties` behind them. Those four are one subject — a property descriptor as a
//! *value* — and the heap already does the hard half in §10.1.6.3's validation. What is here is
//! the translation either way: §6.2.6.5 turning an object into a descriptor, and §6.2.6.4 turning
//! a descriptor back into an object.
//!
//! # What is deliberately absent
//!
//! `Object(primitive)` should wrap — §7.1.18's `ToObject` makes a Number object out of `1` — and
//! there are no wrapper objects yet, so it refuses with a message saying so rather than answering
//! something that is not what the specification says.
//!
//! §20.1.3.6's `toString` reads a *builtin tag*, and most of that table is internal slots this
//! heap does not keep — so `[object Error]` and `[object Date]` are not distinguished. `Array`,
//! `Function` and the two that are not objects at all are, because those are answerable.

use crate::heap::{
    DefineOutcome, Heap, NativeCall, ObjectId, Property, PropertyDescriptor, PropertyKey,
    PropertyKind,
};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, ErrorKind, Value};
use crate::vm::Vm;

use super::{define_method, define_value, key, text};

/// §20.1.1.1 `Object(value)`.
pub fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §20.1.1.1 step 1 — the one constructor with a rule of its own about new.target: when it is
    // present and is *not* this function, the argument is ignored entirely and an ordinary object is
    // made from the target's `prototype`. That is what `class D extends Object {}` needs, and it is
    // why `new D(5)` is a `D` and not a Number wrapper.
    if let Value::Object(target) = call.new_target
        && target != call.function
    {
        let prototype = super::prototype_from(heap, call, vm.realm().object_prototype());
        return Ok(Value::Object(heap.new_object(Some(prototype))));
    }
    match call.argument(0) {
        // §20.1.1.1 step 3 — an object is handed back *as it stands*, which is what makes
        // `Object(o) === o` true and why the function is useless as a copy.
        Value::Object(object) => Ok(Value::Object(object)),
        // Steps 1 and 2 — `undefined` and `null` make a new ordinary object, the same one
        // `{}` makes.
        Value::Undefined | Value::Null => Ok(Value::Object(
            heap.new_object(Some(vm.realm().object_prototype())),
        )),
        // §20.1.1.1 step 3 — anything else is `ToObject` of it, which is a wrapper.
        primitive => vm.object_for(primitive, heap),
    }
}

/// §20.1.3.6 `Object.prototype.toString`.
pub fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Steps 1 and 2 — the two values that have no object to ask, and the reason this method is
    // the idiomatic type test: it answers for `undefined` and `null` rather than throwing.
    let tag = match call.this_value {
        Value::Undefined => "Undefined",
        Value::Null => "Null",
        // Step 14's `Object` for a Symbol: a Symbol wrapper is an ordinary object with a
        // primitive inside and has no row of its own here. `[object Symbol]` comes from step 15
        // instead, out of the `@@toStringTag` §20.4.3.5 puts on `Symbol.prototype` — which is
        // why deleting that property makes a Symbol tag as an ordinary object again.
        Value::Symbol(_) => "Object",
        // §20.1.3.6 has no row for a BigInt either, and for the same reason: `[object BigInt]`
        // comes from the `@@toStringTag` on `BigInt.prototype` rather than from this table.
        Value::BigInt(_) => "Object",
        // Steps 4 to 14's table, in the rows this heap keeps enough state to answer. `IsArray`
        // is step 4 and is a real question about the object rather than about its prototype, so
        // `Object.prototype.toString.call([])` says `[object Array]` and one on an object merely
        // *given* `Array.prototype` does not. `Error`, `RegExp` and `Date` each read an internal
        // slot, and the order between them is the specification's rather than convenient.
        // Step 4's `IsArray` looks through a proxy to its target, so a proxy over an array tags as
        // `[object Array]` — and a *revoked* one throws here rather than tagging as anything.
        Value::Object(object) if heap.is_array_through(object)? => "Array",
        Value::Object(object) => match heap.object(object) {
            // Step 8 — an arguments object is tagged by its parameter map, which is the only
            // thing that tells it from an ordinary object with numeric keys.
            Some(found) if found.arguments_map().is_some() => "Arguments",
            Some(found) if found.call().is_some() => "Function",
            // Step 12 — a `[[DateValue]]` tags as Date, and it has to be asked *before* the
            // wrapper rows below: a time value is a Number, and a Date reaching those would be
            // tagged `[object Number]`.
            Some(found) if found.date_value().is_some() => "Date",
            // Step 7 — an `[[ErrorData]]` tags as Error. Asked of the object rather than of its
            // prototype, which is the distinction the slot exists to make: an ordinary object
            // *given* `Error.prototype` is `[object Object]`.
            Some(found) if found.is_error() => "Error",
            // Step 12 — and a `[[RegExpMatcher]]` as RegExp, on the same terms.
            Some(found) if found.regexp().is_some() => "RegExp",
            // Steps 9 and 10 — a wrapper is tagged by what it wraps, which is why
            // `Object.prototype.toString.call(new Number(1))` is `[object Number]` and not
            // `[object Object]`.
            Some(found) => match found.primitive() {
                Some(Value::Boolean(_)) => "Boolean",
                Some(Value::Number(_)) => "Number",
                Some(Value::String(_)) => "String",
                _ => "Object",
            },
            None => "Object",
        },
        // §7.1.18 — a primitive is wrapped first, and the wrapper's tag is what a wrapper of that
        // primitive would have. Read from the value directly rather than by making the object:
        // the tag is a question about the *kind*, and nothing else about the wrapper is used.
        Value::Boolean(_) => "Boolean",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
    };
    // Step 15 — `@@toStringTag` wins over the whole table above when it is a String, and is
    // ignored when it is anything else. It is why `Symbol()` says `[object Symbol]` despite its
    // wrapper being an ordinary object with no row of its own, and it is the supported way for a
    // script to name its own type here.
    if let Some(found) = tagged(vm, heap, call.this_value)? {
        return Ok(Value::String(found));
    }
    Ok(text(heap, &format!("[object {tag}]")))
}

/// §20.1.3.6 step 15 — what `@@toStringTag` says this object should be called, if it says a String.
///
/// A get and not an own-property read, so a tag inherited from a prototype counts — which is how
/// `Symbol.prototype[@@toStringTag]` tags every Symbol without anything being put on each one.
/// A primitive receiver reaches its prototype the same way any property read would.
fn tagged(
    vm: &mut Vm,
    heap: &mut Heap,
    receiver: Value,
) -> Completion<Option<crate::heap::StringId>> {
    if matches!(receiver, Value::Undefined | Value::Null) {
        return Ok(None);
    }
    let Some(symbol) = vm.realm().well_known(super::well_known_at("toStringTag")) else {
        return Ok(None);
    };
    let key = PropertyKey::from_symbol(symbol);
    let found = vm.get_property_key(receiver, key, heap)?;
    Ok(match found {
        Value::String(id) => Some(text_of(heap, id)),
        _ => None,
    })
}

/// `"[object " + tag + "]"`, for a tag that is already a String on the heap.
pub(super) fn text_of(heap: &mut Heap, tag: crate::heap::StringId) -> crate::heap::StringId {
    let mut units: Vec<u16> = "[object ".encode_utf16().collect();
    units.extend_from_slice(heap.string(tag).unwrap_or(&[]));
    units.push(u16::from(b']'));
    heap.intern(&units)
}

/// §20.1.3.4 `Object.prototype.toLocaleString`.
///
/// The whole of what the core language says: call the object's **own** `toString` and answer that.
/// It exists so that ECMA-402 and the built-ins that override it have somewhere to override, and
/// so that `[1, {}].toLocaleString()` reaches something on every element rather than a TypeError on
/// the ones with no locale-aware spelling of their own.
///
/// `Invoke`, so the `toString` the object has — inherited or given — is what runs, not the
/// intrinsic one.
fn to_locale_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let name = super::key(heap, "toString");
    let method = vm.get_property_key(call.this_value, name, heap)?;
    vm.call_value(method, call.this_value, &[], heap)
}

/// §20.1.3.7 `Object.prototype.valueOf` — `ToObject(this)`, which for an object is itself.
pub fn value_of(_vm: &mut Vm, _heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    match call.this_value {
        Value::Object(object) => Ok(Value::Object(object)),
        _ => Err(Abrupt::type_error(
            "Object.prototype.valueOf requires an object",
        )),
    }
}

/// §20.1.3.2 `Object.prototype.hasOwnProperty`.
pub fn has_own_property(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let key = property_key(heap, call.argument(0))?;
    let object = this_object(call, "Object.prototype.hasOwnProperty requires an object")?;
    Ok(Value::Boolean(own_property(heap, object, key)?.is_some()))
}

/// §20.1.2.13 `Object.hasOwn(o, key)` — the same question without borrowing a method.
pub fn has_own(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let key = property_key(heap, call.argument(1))?;
    Ok(Value::Boolean(own_property(heap, object, key)?.is_some()))
}

/// §20.1.3.4 `Object.prototype.propertyIsEnumerable`.
pub fn property_is_enumerable(
    _vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let key = property_key(heap, call.argument(0))?;
    let object = this_object(
        call,
        "Object.prototype.propertyIsEnumerable requires an object",
    )?;
    // Own only, and `false` rather than an error when there is no such property — which is why
    // it cannot be used to ask whether a property exists at all.
    let answer = own_property(heap, object, key)?.is_some_and(|property| property.enumerable);
    Ok(Value::Boolean(answer))
}

/// §20.1.3.3 `Object.prototype.isPrototypeOf`.
pub fn is_prototype_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call, "Object.prototype.isPrototypeOf requires an object")?;
    // Step 1 — a primitive argument is `false` rather than an error, because the question is
    // about *its* chain and a primitive has none of its own.
    let Value::Object(mut walk) = call.argument(0) else {
        return Ok(Value::Boolean(false));
    };
    // Step 3's loop, iteratively: a chain is as long as a program makes it (DR-0002). Step 3.a is
    // `[[GetPrototypeOf]]`, so a proxy in the chain answers with its trap — and may throw, which is
    // why this walk returns a completion at all.
    loop {
        let Some(next) = vm.prototype_through(walk, heap)? else {
            return Ok(Value::Boolean(false));
        };
        if next == object {
            return Ok(Value::Boolean(true));
        }
        walk = next;
    }
}

/// §20.1.2.12 `Object.getPrototypeOf`.
pub fn get_prototype_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    Ok(match vm.prototype_through(object, heap)? {
        Some(prototype) => Value::Object(prototype),
        None => Value::Null,
    })
}

/// §20.1.2.4 `Object.defineProperty`.
pub fn define_property(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = object_argument(call.argument(0), "Object.defineProperty requires an object")?;
    let key = property_key(heap, call.argument(1))?;
    let descriptor = to_property_descriptor(vm, heap, call.argument(2))?;
    // §20.1.2.4 step 4 is `DefinePropertyOrThrow`: the heap answers what §10.1.6.3's rules made
    // of it, and a refusal here throws rather than doing nothing quietly. That is the difference
    // between `defineProperty` and `Reflect.defineProperty`.
    let outcome = vm.define_through(object, key, &descriptor, heap)?;
    defined(outcome)?;
    Ok(Value::Object(object))
}

/// §20.1.2.3 `Object.defineProperties`.
pub fn define_properties(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = object_argument(
        call.argument(0),
        "Object.defineProperties requires an object",
    )?;
    define_each(vm, heap, object, call.argument(1))?;
    Ok(Value::Object(object))
}

/// §20.1.2.2 `Object.create`.
pub fn create(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — the prototype may be `null`, and that is the whole reason `Object.create` exists:
    // it is the only way to make an object with no prototype at all.
    let prototype = match call.argument(0) {
        Value::Object(prototype) => Some(prototype),
        Value::Null => None,
        _ => {
            return Err(Abrupt::type_error(
                "the prototype given to Object.create must be an object or null",
            ));
        }
    };
    let object = heap.new_object(prototype);
    let properties = call.argument(1);
    if !matches!(properties, Value::Undefined) {
        define_each(vm, heap, object, properties)?;
    }
    Ok(Value::Object(object))
}

/// §20.1.2.8 `Object.getOwnPropertyDescriptor`.
pub fn get_own_property_descriptor(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let key = property_key(heap, call.argument(1))?;
    // §6.2.6.4 — a property that is not there is `undefined`, not an empty descriptor, which is
    // how a caller tells "absent" from "present and undefined".
    let Some(property) = vm.own_property_through(object, key, heap)? else {
        return Ok(Value::Undefined);
    };
    Ok(describe(heap, &vm.realm(), property))
}

/// §20.1.2.17 `Object.keys` — own, enumerable, string-keyed, in creation order.
pub fn keys(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    own_keys(vm, heap, call, true)
}

/// §20.1.2.10 `Object.getOwnPropertyNames` — the same list without the enumerable filter.
pub fn get_own_property_names(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    own_keys(vm, heap, call, false)
}

/// §20.1.2.19 `Object.preventExtensions`.
pub fn prevent_extensions(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    let value = call.argument(0);
    // Step 1 — a primitive is handed straight back rather than refused, because it was never
    // extensible in the first place and the request is already satisfied.
    if let Value::Object(object) = value
        && !vm.prevent_through(object, heap)?
    {
        // Step 3 — a refusal is a TypeError, which only a proxy can produce: an ordinary object
        // always accepts.
        return Err(Abrupt::type_error(
            "Object.preventExtensions could not make this object non-extensible",
        ));
    }
    Ok(value)
}

/// §20.1.2.16 `Object.isExtensible`.
pub fn is_extensible(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — a primitive is `false` rather than an error: it is not extensible, which is a
    // true answer to the question asked.
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(vm.extensible_through(object, heap)?))
}

/// Build `Object` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.object_prototype();
    let function = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, function, "Object", 1);

    // §20.1.2.20 — `Object.prototype` is not writable, not enumerable and not configurable, for
    // the same reason `Error.prototype` is not: every object in the realm inherits from it.
    let key = key(heap, "prototype");
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(prototype)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(function, key, &descriptor);
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, "Object", Value::Object(function));

    for (name, length, native) in [
        ("toString", 0, to_string as crate::heap::Native),
        ("toLocaleString", 0, to_locale_string),
        ("valueOf", 0, value_of),
        ("hasOwnProperty", 1, has_own_property),
        ("isPrototypeOf", 1, is_prototype_of),
        ("propertyIsEnumerable", 1, property_is_enumerable),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    for (name, length, native) in [
        ("create", 2, create as crate::heap::Native),
        ("defineProperties", 2, define_properties),
        ("defineProperty", 3, define_property),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getOwnPropertyNames", 1, get_own_property_names),
        ("getPrototypeOf", 1, get_prototype_of),
        ("hasOwn", 2, has_own),
        ("isExtensible", 1, is_extensible),
        ("keys", 1, keys),
        ("preventExtensions", 1, prevent_extensions),
    ] {
        define_method(heap, realm, function, name, length, native);
    }
    super::object_state::install(heap, realm, function);
}

/// §20.1.2.3.1 `ObjectDefineProperties` — every own enumerable key of `properties`, in order.
///
/// The descriptors are all read *before* any is applied, which §20.1.2.3.1 step 4 is explicit
/// about: a second descriptor that is malformed must not leave the first one applied.
fn define_each(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    properties: Value,
) -> Completion<()> {
    let source = to_object(properties, "a property-descriptor list must be an object")?;
    let keys = vm.own_keys_through(source, heap)?;
    let mut pending = Vec::new();
    for key in keys {
        // The *attributes* are read from the table, because step 3.a asks
        // `GetOwnPropertyDescriptor` and only wants to know whether this key is enumerable.
        let Some(property) = vm.own_property_through(source, key, heap)? else {
            continue;
        };
        if !property.enumerable {
            continue;
        }
        // …and the descriptor itself is read with `Get` (step 3.b.i), which walks nothing here —
        // the key is already known to be own — but does call a getter. So a list may compute its
        // descriptors, which this used to refuse.
        let value = vm.get_property_key(Value::Object(source), key, heap)?;
        pending.push((key, to_property_descriptor(vm, heap, value)?));
    }
    for (key, descriptor) in pending {
        let outcome = vm.define_through(object, key, &descriptor, heap)?;
        defined(outcome)?;
    }
    Ok(())
}

/// §20.1.2.17 and §20.1.2.10, which differ in one filter.
fn own_keys(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    enumerable_only: bool,
) -> Completion<Value> {
    let object = coerced(vm, heap, call.argument(0))?;
    let keys = vm.own_keys_through(object, heap)?;
    let mut names = Vec::new();
    for key in keys {
        // §20.1.2.11.1 `GetOwnPropertyKeys` filters by *type* and nothing else, so
        // `getOwnPropertyNames` never asks for a descriptor. §20.1.2.17's `Object.keys` goes
        // through §7.3.24 `EnumerableOwnProperties`, which does — and for a proxy that is a second
        // trap call, observably: an `ownKeys` trap that lists a key its
        // `getOwnPropertyDescriptor` trap then hides is left out of one listing and not the other.
        if enumerable_only
            && !vm
                .own_property_through(object, key, heap)?
                .is_some_and(|property| property.enumerable)
        {
            continue;
        }
        // §20.1.2.10 step 3 and §20.1.2.17 step 3 — String keys only. A Symbol key is not
        // *hidden* from the language, but it is not listed here: `getOwnPropertySymbols` is the
        // one that answers those, and keeping them apart is the whole reason for two functions.
        let Some(name) = key.as_string() else {
            continue;
        };
        names.push(Value::String(name));
    }
    // §20.1.2.17 answers an Array, and now there are some.
    let list = heap.new_array(vm.realm().array_prototype(), 0);
    for (at, name) in names.iter().enumerate() {
        let key = self::key(heap, &at.to_string());
        let descriptor = PropertyDescriptor {
            value: Some(*name),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(list, key, &descriptor);
    }
    Ok(Value::Object(list))
}

/// §6.2.6.5 `ToPropertyDescriptor` — an object read as a descriptor.
///
/// Every field is optional and *absence is not `undefined`*: `{value: undefined}` sets the value
/// to `undefined`, and `{}` sets nothing at all. That distinction is the whole reason
/// [`PropertyDescriptor`]'s fields are `Option`, and it is what makes
/// `Object.defineProperty(o, "k", {})` leave an existing property alone.
pub(crate) fn to_property_descriptor(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
) -> Completion<PropertyDescriptor> {
    let Value::Object(source) = value else {
        return Err(Abrupt::type_error(
            "a property descriptor must be an object",
        ));
    };
    let mut descriptor = PropertyDescriptor::EMPTY;
    // §6.2.6.5 steps 3 to 20, **in the order the specification reads them** — which is not the
    // order the fields are declared in anywhere else: `value` comes before `writable`. Nothing can
    // see that until a field is a getter, and then two of them with side effects can see it
    // exactly.
    for (name, at) in [
        ("enumerable", 0),
        ("configurable", 1),
        ("value", 2),
        ("writable", 3),
        ("get", 4),
        ("set", 5),
    ] {
        let key = self::key(heap, name);
        // `HasProperty` and then `Get`, which is what the specification says twice per field and
        // is not the same as reading the property table. `HasProperty` walks the prototype chain,
        // so a descriptor may be made by `Object.create({writable: true})`; `Get` **calls a
        // getter**, so a descriptor may compute its own fields — which a property-table read
        // cannot do and which this used to refuse outright.
        if !vm.has_property_key(Value::Object(source), key, heap)? {
            continue;
        }
        let value = vm.get_property_key(Value::Object(source), key, heap)?;
        match at {
            0 => descriptor.enumerable = Some(value.to_boolean(heap)),
            1 => descriptor.configurable = Some(value.to_boolean(heap)),
            2 => descriptor.value = Some(value),
            3 => descriptor.writable = Some(value.to_boolean(heap)),
            4 => descriptor.getter = Some(value),
            // §6.2.6.5 steps 17 and 20 — `get` and `set` must be callable or `undefined`, and
            // the check is below rather than in the heap because it is about what a *descriptor*
            // may say rather than about what an object may hold.
            _ => descriptor.setter = Some(value),
        }
    }
    for accessor in [descriptor.getter, descriptor.setter] {
        let Some(accessor) = accessor else { continue };
        let callable = matches!(accessor, Value::Undefined)
            || matches!(accessor, Value::Object(object)
                if heap.object(object).is_some_and(|found| found.call().is_some()));
        if !callable {
            return Err(Abrupt::type_error(
                "a getter or setter must be callable or undefined",
            ));
        }
    }
    // §6.2.6.5 step 21 — a descriptor may not be both, and saying so here means the heap never
    // has to answer what `{value: 1, get: f}` would mean.
    let data = descriptor.value.is_some() || descriptor.writable.is_some();
    let accessor = descriptor.getter.is_some() || descriptor.setter.is_some();
    if data && accessor {
        return Err(Abrupt::type_error(
            "a property descriptor may not have both a value and an accessor",
        ));
    }
    Ok(descriptor)
}

/// §6.2.6.4 `FromPropertyDescriptor` — a property written back out as an object.
///
/// A *complete* descriptor, which is why every field is present: this is the one place a
/// partial descriptor is filled in, and it is what makes `getOwnPropertyDescriptor` answer
/// `{value: 1, writable: false, enumerable: false, configurable: false}` for a property that was
/// defined with `{value: 1}` alone.
pub(super) fn describe(heap: &mut Heap, realm: &Realm, property: Property) -> Value {
    let object = heap.new_object(Some(realm.object_prototype()));
    let put = |heap: &mut Heap, name: &str, value: Value| {
        let key = self::key(heap, name);
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(object, key, &descriptor);
    };
    match property.kind {
        PropertyKind::Data { value, writable } => {
            put(heap, "value", value);
            put(heap, "writable", Value::Boolean(writable));
        }
        PropertyKind::Accessor { getter, setter } => {
            put(heap, "get", getter);
            put(heap, "set", setter);
        }
    }
    put(heap, "enumerable", Value::Boolean(property.enumerable));
    put(heap, "configurable", Value::Boolean(property.configurable));
    Value::Object(object)
}

/// §6.2.6.4 `FromPropertyDescriptor` — a *partial* descriptor written out as an object.
///
/// Unlike [`describe`], only the fields that are present are written. §10.5.6 needs exactly this:
/// a `defineProperty` trap is handed what the caller asked for, so a handler can tell
/// `defineProperty(p, "x", {value: 1})` from one that also asked for `{enumerable: false}`. Filling
/// the gaps in would make those two indistinguishable to every trap ever written.
pub(crate) fn from_property_descriptor(
    heap: &mut Heap,
    realm: &Realm,
    descriptor: &PropertyDescriptor,
) -> Value {
    let object = heap.new_object(Some(realm.object_prototype()));
    let put = |heap: &mut Heap, name: &str, value: Value| {
        let key = self::key(heap, name);
        let field = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(object, key, &field);
    };
    if let Some(value) = descriptor.value {
        put(heap, "value", value);
    }
    if let Some(writable) = descriptor.writable {
        put(heap, "writable", Value::Boolean(writable));
    }
    if let Some(getter) = descriptor.getter {
        put(heap, "get", getter);
    }
    if let Some(setter) = descriptor.setter {
        put(heap, "set", setter);
    }
    if let Some(enumerable) = descriptor.enumerable {
        put(heap, "enumerable", Value::Boolean(enumerable));
    }
    if let Some(configurable) = descriptor.configurable {
        put(heap, "configurable", Value::Boolean(configurable));
    }
    Value::Object(object)
}

/// `this` as an object, or the TypeError carrying `wanted`.
pub(super) fn this_object(call: &NativeCall<'_>, wanted: &'static str) -> Completion<ObjectId> {
    to_object(call.this_value, wanted)
}

/// §7.1.18 `ToObject`, in the part that does not need a wrapper.
///
/// §7.1.18 wraps a primitive and refuses only `undefined` and `null`. There is nothing to wrap
/// one in yet, so the two cases share a message — and the message is passed in, because "requires
/// an object" is useless without saying what did.
pub(super) fn coerced(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<ObjectId> {
    // §7.1.18 proper, which is not the same question as "is this an object". Most of §20.1.2's
    // statics begin with `ToObject` and so answer about a *primitive* rather than refusing it:
    // `Object.keys("ab")` is `["0", "1"]`, because the String object it stands for has those keys.
    // Only `undefined` and `null` have no object, and that is where the TypeError comes from.
    match vm.object_for(value, heap)? {
        Value::Object(object) => Ok(object),
        _ => Err(Abrupt::type_error("this value has no object to read")),
    }
}

/// The object a static was handed, refusing anything that is not one already.
///
/// For the statics that say "if Type(O) is not Object, throw" rather than `ToObject` — the ones
/// that would otherwise silently change a wrapper nobody else can see. `Object.defineProperty(1,
/// …)` is a TypeError for that reason and `Object.keys(1)` is not.
pub(super) fn to_object(value: Value, wanted: &'static str) -> Completion<ObjectId> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(Abrupt::Raised(ErrorKind::Type, wanted)),
    }
}

/// An argument §20.1 requires to be an object outright rather than coercing.
///
/// `defineProperty` and `defineProperties` are the two that refuse rather than wrap, because
/// defining a property on a throwaway wrapper would silently do nothing.
fn object_argument(value: Value, wanted: &'static str) -> Completion<ObjectId> {
    to_object(value, wanted)
}

/// Turn a define's outcome into the completion §20.1's `DefinePropertyOrThrow` wants.
///
/// Three different errors, because they are three different mistakes: a rule that would not allow
/// the property is a TypeError, a value that is not a length at all is §10.4.2.4 step 2's
/// RangeError, and a numeric of the wrong type for the array is §10.4.5.16's TypeError. Written
/// once so `defineProperty` and `defineProperties` cannot drift apart.
pub(crate) fn defined(outcome: DefineOutcome) -> Completion<()> {
    match outcome {
        DefineOutcome::Defined => Ok(()),
        DefineOutcome::Refused => Err(Abrupt::type_error("this property cannot be redefined")),
        DefineOutcome::BadLength => Err(Abrupt::Raised(
            ErrorKind::Range,
            "an array length must be an integer index",
        )),
        DefineOutcome::WrongContent => Err(Abrupt::type_error(
            "this TypedArray holds the other numeric type",
        )),
    }
}

/// The same outcome as the Boolean §28.1.3's `Reflect.defineProperty` and §10.1.9's `[[Set]]` want.
///
/// The difference from [`defined`] is only in the **refusal**: §10.1.6.3 declining to redefine a
/// property is what `Reflect.defineProperty` answers `false` for and `Object.defineProperty` throws
/// for. The other two are not refusals at all — §10.4.2.1's `ArraySetLength` and §10.4.5.16's
/// conversion *throw*, and a define that throws throws by every route to it. Written beside
/// [`defined`] so the two cannot drift about which of the four is which, which is exactly how
/// `Reflect.defineProperty([], "length", { value: -1 })` came to answer `false`.
pub(crate) fn define_answer(outcome: DefineOutcome) -> Completion<Value> {
    match outcome {
        DefineOutcome::Refused => Ok(Value::Boolean(false)),
        other => defined(other).map(|()| Value::Boolean(true)),
    }
}

/// An object's own property under `key`, if it has one — §10.1.5 `[[GetOwnProperty]]`.
///
/// A completion and not a plain answer, for one kind of object: §10.4.6.5 builds a namespace's
/// descriptor out of `[[Get]]`, so a binding still in its dead zone makes even *asking* about the
/// property a ReferenceError. Every caller therefore has to say what it does about that, which is
/// the point — `hasOwnProperty` on such a name throws where it would otherwise answer `true`.
///
/// This is the path that does **not** consult a Proxy's trap. Where the trap is wanted, the caller
/// uses [`Vm::own_property_through`] instead.
pub(super) fn own_property(
    heap: &mut Heap,
    object: ObjectId,
    key: PropertyKey,
) -> Completion<Option<Property>> {
    if let Some(crate::heap::Export::Uninitialised) = heap.namespace_export(object, key) {
        return Err(crate::value::Abrupt::reference_error(
            "a module binding was read before its module gave it a value",
        ));
    }
    // Through the heap rather than the object's own table, so that §10.4.4.1's substitution
    // happens: a joined argument index reports the *parameter's* value, which is what makes
    // `Object.getOwnPropertyDescriptor(arguments, '0')` follow an assignment to `a`.
    Ok(heap.own_property(object, key))
}

/// §7.1.19 `ToPropertyKey`.
pub(super) fn property_key(heap: &mut Heap, value: Value) -> Completion<PropertyKey> {
    // §7.1.19 step 3 — a Symbol is a key already and must not be spelled: `ToString` of one
    // throws, so without this `Object.getOwnPropertyDescriptor(o, sym)` would be an error rather
    // than an answer about a property that is really there.
    if let Value::Symbol(symbol) = value {
        return Ok(PropertyKey::from_symbol(symbol));
    }
    let id = value.to_string(heap)?;
    Ok(PropertyKey::from_string(heap, id))
}

/// The own keys of `object`, as a list that borrows nothing.
///
/// Every listing needs this, and each would otherwise hold a borrow across the `[[Get]]` that
/// follows — which can run a getter, which can change the object being listed.
pub(super) fn keys_of(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
) -> Completion<Vec<PropertyKey>> {
    vm.own_keys_through(object, heap)
}
