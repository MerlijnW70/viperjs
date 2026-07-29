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
pub mod global;
mod iterator;
mod math;
mod object;
mod object_state;
mod symbol;
pub use self::symbol::WELL_KNOWN;

/// Where a well-known Symbol sits in [`WELL_KNOWN`], by name.
///
/// A linear search over thirteen short strings, done once per call to whatever needs it. The
/// alternative is a constant per Symbol, which is thirteen names to keep in step with the table
/// instead of one; when a benchmark says this is on a hot path it becomes those constants and not
/// before.
#[must_use]
pub fn well_known_at(name: &str) -> usize {
    WELL_KNOWN
        .iter()
        .position(|known| *known == name)
        .unwrap_or(usize::MAX)
}
mod string;
mod string_edit;
mod string_index;
mod wrapper;

use crate::heap::{Heap, Native, ObjectId, PropertyDescriptor, PropertyKey, PropertyKind};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build every built-in into `heap`, on the realm's global object.
pub fn install(heap: &mut Heap, realm: &Realm) {
    let global = realm.global();
    // `Object` first: `Object.prototype` is where every chain ends, so a built-in installed
    // before it would inherit from a prototype with no methods on it yet.
    global::install(heap, realm, global);
    object::install(heap, realm, global);
    error::install(heap, realm, global);
    array::install(heap, realm, global);
    array_methods::install(heap, realm);
    function::install(heap, realm, global);
    math::install(heap, realm, global);
    wrapper::install(heap, realm, global);
    iterator::install(heap, realm);
    string::install(heap, realm, global);
    symbol::install(heap, realm, global);
}

/// A property key for a name the engine itself knows.
pub(crate) fn key(heap: &mut Heap, name: &str) -> PropertyKey {
    PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>())
}

/// A String on the heap, as a value.
pub(crate) fn text(heap: &mut Heap, contents: &str) -> Value {
    Value::String(heap.new_string(contents.encode_utf16().collect()))
}

/// Give `object` a property that cannot be written, seen or removed.
///
/// What §17 gives a *constant* — `Math.PI`, `Number.MAX_VALUE` — and what §20.2.2.2 gives a
/// constructor's `prototype`. The two have the same three answers for the same reason: a program
/// may read them and may not move them, because everything else in the realm is already built on
/// top of where they are.
///
/// Written once because it was written three times: in `Math`, in `Number`, and beside every
/// constructor. Three copies of three booleans is nine chances to disagree, and the copy nothing
/// reads is the one that would.
pub(crate) fn define_fixed(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
    let key = key(heap, name);
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(object, key, &descriptor);
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
    define_metadata(heap, function, Value::Number(f64::from(length)), named);
}

/// The same two properties, for a function whose length is arithmetic rather than a count.
///
/// §20.2.3.2 makes a bound function's `length` what is *left* after the arguments it was given,
/// which is a computed Number and not one the specification writes down. The attributes are the
/// same, and are written here once rather than beside each caller — three booleans repeated are
/// three booleans that can disagree, and the copy nothing reads is the one that will.
pub(crate) fn define_metadata(heap: &mut Heap, function: ObjectId, length: Value, name: Value) {
    for (key_name, value) in [("length", length), ("name", name)] {
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
        .find(|key| key.as_string().and_then(|id| heap.string(id)) == Some(&units[..]))?;
    match heap.object(object)?.get_own_property(key)?.kind {
        PropertyKind::Data { value, .. } => Some(value),
        PropertyKind::Accessor { .. } => None,
    }
}

/// `Set(O, key, value, true)` (§7.3.4) — the throwing form, which is the one §23.1.3 uses.
///
/// `[[Set]]` answers whether it was allowed and sloppy code throws that answer away; every Array
/// method instead passes `true` for `Throw`, so a refusal becomes a TypeError. It is the whole
/// difference between `Object.freeze(a); a[0] = 1` (silent) and `Object.freeze(a); a.push(1)`
/// (an error), and the reason it is a function is that there are twenty places to get it right.
pub(crate) fn set_or_throw(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    key: PropertyKey,
    value: Value,
) -> Completion<()> {
    match vm.set_property_key(Value::Object(object), key, value, heap)? {
        Value::Boolean(false) => Err(Abrupt::type_error(
            "this property cannot be set on this object",
        )),
        _ => Ok(()),
    }
}

/// `DeletePropertyOrThrow` (§7.3.9) — the same rule for a delete.
pub(crate) fn delete_or_throw(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    key: PropertyKey,
) -> Completion<()> {
    match vm.delete_property_key(Value::Object(object), key, heap)? {
        Value::Boolean(false) => Err(Abrupt::type_error(
            "this property cannot be deleted from this object",
        )),
        _ => Ok(()),
    }
}

/// Give an already-installed method a well-known Symbol as a second name.
///
/// §23.1.3.38 and its like say that `Array.prototype[@@iterator]` *is* `Array.prototype.values` —
/// the same function object, so `===` finds them equal. Installing a second native with the same
/// body would not satisfy that, which is why this copies the value across rather than defining
/// another function.
pub(crate) fn alias_to_symbol(
    heap: &mut Heap,
    realm: &Realm,
    object: ObjectId,
    from: &str,
    symbol: &str,
) {
    let Some(value) = read_method(heap, object, from) else {
        return;
    };
    define_under_symbol(heap, realm, object, symbol, value);
}

/// The same, but the String name is *removed* — the method only ever had a Symbol key.
///
/// §22.1.3.34's method has no String name at all: it is installed under one here because that is
/// how [`define_method`] gives a function its `name` and `length`, and then the name is taken
/// away. `String.prototype["[Symbol.iterator]"]` must not exist.
pub(crate) fn move_to_symbol(
    heap: &mut Heap,
    realm: &Realm,
    object: ObjectId,
    from: &str,
    symbol: &str,
) {
    alias_to_symbol(heap, realm, object, from, symbol);
    let name = key(heap, from);
    heap.delete_own_property(object, name);
}

/// The value of a method already installed under a String name.
fn read_method(heap: &mut Heap, object: ObjectId, name: &str) -> Option<Value> {
    let name = key(heap, name);
    match heap.own_property(object, name)?.kind {
        PropertyKind::Data { value, .. } => Some(value),
        PropertyKind::Accessor { .. } => None,
    }
}

/// Define `value` under a well-known Symbol, with §17's attributes for a method.
fn define_under_symbol(
    heap: &mut Heap,
    realm: &Realm,
    object: ObjectId,
    symbol: &str,
    value: Value,
) {
    let Some(found) = realm.well_known(well_known_at(symbol)) else {
        return;
    };
    let name = PropertyKey::from_symbol(found);
    let _ = heap.define_own_property(
        object,
        name,
        &PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}
