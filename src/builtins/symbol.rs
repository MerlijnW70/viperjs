//! §20.4 `Symbol` — the constructor that may not be constructed, and the well-known ones.
//!
//! # Why `new Symbol()` is an error
//!
//! §20.4.1 step 1 refuses it outright, and it is the only constructor in the language that refuses
//! itself. The reason is that a Symbol *wrapper* would be an object, and an object is never equal
//! to anything but itself — so `new Symbol("a")` would look like a Symbol, print like a Symbol, and
//! silently fail to be usable as the key it was made to be. The specification would rather refuse
//! than hand back something that behaves almost right.
//!
//! A wrapper still exists, because `ToObject` has to answer something for a Symbol receiver and
//! because `Object(sym)` is allowed. It just cannot be asked for directly.
//!
//! # The well-known Symbols
//!
//! §6.1.5.1's table: `Symbol.iterator`, `Symbol.toPrimitive`, and the rest. They are made here and
//! kept on the realm because the *engine* has to be able to find them — `for`-`of` looks for
//! `Symbol.iterator` by identity, not by name, and a script that shadows the property must not be
//! able to change what the loop reaches for.
//!
//! Every one of them is installed even though ViperJS acts on only some of them yet. A well-known
//! Symbol that exists but is never consulted is a property a script can put a method under and see
//! ignored; one that does not exist at all is a `TypeError` on the line that mentions it, which is
//! a worse answer to give and a harder one to grow out of.

use super::{define_function_metadata, define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, StringId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §6.1.5.1 — every well-known Symbol, in the order the table lists them.
///
/// The `@@` spelling is the specification's own, and the description each carries is the one
/// §6.1.5.1 gives: `Symbol.iterator.toString()` really does answer `"Symbol(Symbol.iterator)"`.
pub const WELL_KNOWN: [&str; 13] = [
    "asyncIterator",
    "hasInstance",
    "isConcatSpreadable",
    "iterator",
    "match",
    "matchAll",
    "replace",
    "search",
    "species",
    "split",
    "toPrimitive",
    "toStringTag",
    "unscopables",
];

/// Build `Symbol` and `Symbol.prototype` into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.symbol_prototype();
    // §20.4.1 — callable and *not* constructible, which is the only constructor of which that is
    // true. See the module comment for why.
    let symbol = heap.new_native_function(realm.function_prototype(), make_symbol);
    define_function_metadata(heap, symbol, "Symbol", 0);
    super::define_fixed(heap, symbol, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(symbol));
    define_value(heap, global, "Symbol", Value::Object(symbol));

    // §20.4.2 — each well-known Symbol is a property of the constructor and is not writable, not
    // enumerable and not configurable. A script cannot replace `Symbol.iterator`, which is what
    // lets the engine trust the one it holds.
    for (at, name) in WELL_KNOWN.into_iter().enumerate() {
        let Some(well_known) = realm.well_known(at) else {
            continue;
        };
        super::define_fixed(heap, symbol, name, Value::Symbol(well_known));
    }

    for (name, length, native) in [
        ("for", 1, registered as crate::heap::Native),
        ("keyFor", 1, key_for),
    ] {
        define_method(heap, realm, symbol, name, length, native);
    }
    for (name, length, native) in [
        ("toString", 0, to_string as crate::heap::Native),
        ("valueOf", 0, value_of),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §20.4.3.5 — `Symbol.prototype[@@toStringTag]` is the String `"Symbol"`, and it is what makes
    // `Object.prototype.toString.call(sym)` say `[object Symbol]`. Not writable and not
    // enumerable, and *configurable*, which is the one of the three that surprises: a script may
    // delete it, and then a Symbol tags as an ordinary object again.
    if let Some(tag) = realm.well_known(super::well_known_at("toStringTag")) {
        let name = crate::heap::PropertyKey::from_symbol(tag);
        let units: Vec<u16> = "Symbol".encode_utf16().collect();
        let value = Value::String(heap.intern(&units));
        let _ = heap.define_own_property(
            prototype,
            name,
            &PropertyDescriptor {
                value: Some(value),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    // §20.4.3.5 — `Symbol.prototype[@@toPrimitive]` answers the Symbol itself, which is how a
    // Symbol survives a coercion that would otherwise reach `toString` and throw. It is why
    // `Object(sym) == sym` is true: the wrapper is asked for a primitive and gives back the very
    // Symbol it wraps, where §20.4.3.3's `toString` would have refused.
    if let Some(symbol) = realm.well_known(super::well_known_at("toPrimitive")) {
        let method = heap.new_native_function(realm.function_prototype(), to_primitive);
        define_function_metadata(heap, method, "[Symbol.toPrimitive]", 1);
        let _ = heap.define_own_property(
            prototype,
            crate::heap::PropertyKey::from_symbol(symbol),
            &PropertyDescriptor {
                value: Some(Value::Object(method)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    // §20.4.3.2 `description` is an accessor and not a value: it has to read the receiver, and a
    // data property could only hold one Symbol's answer.
    let getter = heap.new_native_function(realm.function_prototype(), description);
    define_function_metadata(heap, getter, "get description", 0);
    let key = super::key(heap, "description");
    let _ = heap.define_own_property(
        prototype,
        key,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            setter: Some(Value::Undefined),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §20.4.1.1 `Symbol([description])`.
///
/// No check for `new` here. §20.4.1 step 1's refusal is expressed by `Symbol` not having a
/// `[[Construct]]` at all — which is what §10.3.2 means and what [`install`] does by reaching for
/// [`Heap::new_native_function`] rather than its constructor-making sibling. The call never gets
/// this far, so a guard here would be a branch no input could take.
fn make_symbol(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 2 — *no argument* is no description, and an explicit `undefined` is the same. Both
    // differ from `Symbol("")`, which has one and it is empty.
    let description = match call.argument(0) {
        Value::Undefined => None,
        value => Some(vm.to_string(value, heap)?),
    };
    Ok(Value::Symbol(heap.new_symbol(description)))
}

/// §20.4.2.2 `Symbol.for(key)`.
///
/// The one way to ask for a Symbol that already exists. Everything else about the type is built on
/// two calls never agreeing; this is the exception, and it is a table rather than a change to what
/// a Symbol is.
fn registered(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let key = vm.to_string(call.argument(0), heap)?;
    Ok(Value::Symbol(heap.registered_symbol(key)))
}

/// §20.4.2.7 `Symbol.keyFor(sym)`.
///
/// `undefined` for a Symbol that was not made by `Symbol.for`, which is the only way to tell a
/// registered Symbol from an ordinary one — they are otherwise the same kind of thing.
fn key_for(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Symbol(symbol) = call.argument(0) else {
        return Err(Abrupt::type_error("Symbol.keyFor requires a Symbol"));
    };
    Ok(match heap.symbol_registry_key(symbol) {
        Some(key) => Value::String(key),
        None => Value::Undefined,
    })
}

/// `thisSymbolValue` (§20.4.3) — the Symbol the receiver *is*.
fn this_symbol(heap: &Heap, receiver: Value) -> Completion<crate::heap::SymbolId> {
    if let Value::Symbol(symbol) = receiver {
        return Ok(symbol);
    }
    if let Value::Object(object) = receiver
        && let Some(Value::Symbol(symbol)) =
            heap.object(object).and_then(crate::heap::Object::primitive)
    {
        return Ok(symbol);
    }
    Err(Abrupt::type_error(
        "this method requires a Symbol or a Symbol object",
    ))
}

/// §20.4.3.3 `Symbol.prototype.toString` — `SymbolDescriptiveString` (§20.4.3.3.1).
///
/// The one way a Symbol's text can be reached, and it must be asked for: `ToString` of a Symbol
/// throws, so this is never reached by accident.
fn to_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let symbol = this_symbol(heap, call.this_value)?;
    Ok(Value::String(descriptive(heap, symbol)))
}

/// `SymbolDescriptiveString` — `"Symbol("` , the description, `")"`.
///
/// A Symbol with no description spells `"Symbol()"`, and so does one described as the empty
/// String. The two are different Symbols with different `description`s that print alike, which is
/// §20.4.3.3.1 taking the description as text and nothing more.
pub(super) fn descriptive(heap: &mut Heap, symbol: crate::heap::SymbolId) -> StringId {
    let mut units: Vec<u16> = "Symbol(".encode_utf16().collect();
    if let Some(description) = heap.symbol_description(symbol) {
        units.extend_from_slice(heap.string(description).unwrap_or(&[]));
    }
    units.push(u16::from(b')'));
    heap.intern(&units)
}

/// §20.4.3.4 `Symbol.prototype.valueOf`.
fn value_of(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Symbol(this_symbol(heap, call.this_value)?))
}

/// §20.4.3.5 `Symbol.prototype[@@toPrimitive](hint)`.
///
/// Answers the Symbol, whatever the hint says — the one `@@toPrimitive` in the language that
/// ignores its argument, because a Symbol has no other primitive to become. That is not a shortcut
/// past §7.1.1: it is what makes `sym + ""` a TypeError from the **addition** rather than from
/// `toString`, and what lets a wrapper object compare equal to the Symbol it wraps.
fn to_primitive(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Symbol(this_symbol(heap, call.this_value)?))
}

/// §20.4.3.2 `get Symbol.prototype.description`.
///
/// `undefined` for a Symbol made without one, and `""` for one made with the empty String — the
/// distinction `toString` throws away and this keeps.
fn description(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let symbol = this_symbol(heap, call.this_value)?;
    Ok(match heap.symbol_description(symbol) {
        Some(text) => Value::String(text),
        None => Value::Undefined,
    })
}
