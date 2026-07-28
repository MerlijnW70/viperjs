//! The objects a script can reach without making them — §19 through §28, as far as they go.
//!
//! # What a built-in is, mechanically
//!
//! An ordinary object with a `[[Call]]` that is Rust rather than bytecode. Nothing else about it
//! is special: it has properties, a prototype, and attributes like anything else, and the
//! interpreter reaches it through the same call instruction. [`crate::heap::Callable`] is where
//! the two kinds part company, and it is the only place they do.
//!
//! # Why the shared helpers are here rather than on `Heap`
//!
//! Because they encode §17's *conventions* rather than the heap's mechanics. "A built-in property
//! is writable, not enumerable and configurable" is a rule about built-ins, and the day a rule has
//! an exception it should be visible next to the built-ins it governs — not buried in a heap
//! method that also serves object literals.

pub mod array;
pub mod array_edit;
pub mod array_iterate;
pub mod array_methods;
pub mod error;
pub mod function;
pub mod object;

use crate::heap::{Heap, Native, ObjectId, PropertyDescriptor, PropertyKey, PropertyKind};
use crate::realm::Realm;
use crate::value::Value;

/// Build every built-in into `heap`, on the realm's global object.
pub fn install(heap: &mut Heap, realm: &Realm) {
    let global = realm.global();
    // `Object` first: `Object.prototype` is where every chain ends, so a built-in installed
    // before it would inherit from a prototype with no methods on it yet.
    object::install(heap, realm, global);
    error::install(heap, realm, global);
    array::install(heap, realm, global);
    array_methods::install(heap, realm);
    function::install(heap, realm, global);
}

/// A property key for a name the engine itself knows.
pub(crate) fn key(heap: &mut Heap, name: &str) -> PropertyKey {
    PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>())
}

/// A String on the heap, as a value.
pub(crate) fn text(heap: &mut Heap, contents: &str) -> Value {
    Value::String(heap.new_string(contents.encode_utf16().collect()))
}

/// Give `object` a property with §17's attributes: writable, not enumerable, configurable.
///
/// The convention every built-in property follows, and it is not what assignment produces —
/// which is why enumerating an error does not list its `message` and why `for...in` over any
/// built-in object finds nothing at all.
pub(crate) fn define_value(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
    let key = key(heap, name);
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(false),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    // The object was made here and nothing on it is non-configurable, so the rules cannot refuse
    // this. Ignoring the answer rather than asserting keeps installation total.
    let _ = heap.define_own_property(object, key, &descriptor);
}

/// Give `object` a method: a built-in function with §10.3.3's `name` and `length`.
pub(crate) fn define_method(
    heap: &mut Heap,
    realm: &Realm,
    object: ObjectId,
    name: &str,
    length: u32,
    native: Native,
) {
    let function = heap.new_native_function(realm.function_prototype(), native);
    define_function_metadata(heap, function, name, length);
    define_value(heap, object, name, Value::Object(function));
}

/// §10.3.3's two own properties, which every built-in function has and no two share.
///
/// `length` is how many arguments the specification *writes*, not how many it will accept, and
/// `name` is what a diagnostic prints. Both are non-writable and configurable — a script may
/// delete them and redefine them, and may not simply assign over them. `assert.throws` reads
/// `name` off a constructor to say which error it wanted, so this is load-bearing for the suite
/// rather than decoration.
pub(crate) fn define_function_metadata(
    heap: &mut Heap,
    function: ObjectId,
    name: &str,
    length: u32,
) {
    let named = text(heap, name);
    for (key_name, value) in [
        ("length", Value::Number(f64::from(length))),
        ("name", named),
    ] {
        let key = key(heap, key_name);
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(function, key, &descriptor);
    }
}

/// An object's own property value, if it has one that is a value rather than an accessor.
///
/// Own rather than inherited, and data rather than accessor. A built-in reading its own
/// `prototype` wants the one it was given, not one it inherited from a constructor it happens to
/// be written beneath.
pub(crate) fn own_value(heap: &Heap, object: ObjectId, name: &str) -> Option<Value> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let key = heap
        .object(object)?
        .own_property_keys(heap)
        .into_iter()
        .find(|key| heap.string(key.as_string()) == Some(&units[..]))?;
    match heap.object(object)?.get_own_property(key)?.kind {
        PropertyKind::Data { value, .. } => Some(value),
        PropertyKind::Accessor { .. } => None,
    }
}
