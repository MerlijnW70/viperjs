//! §20.2.3 — `Function.prototype`, in the two methods that decide who `this` is.
//!
//! # Why these two and not `bind`
//!
//! Because `call` and `apply` are how the rest of the language is *reached*. Almost every method
//! in §20 through §28 is written against a shape rather than a type —
//! `Array.prototype.join.call({0: "a", length: 1})` is the specified reading, not a trick — and
//! without these there is no way to say so from a script. test262's own harness leans on them:
//! `Object.prototype.toString.call` and `Array.prototype.map.call` are both in `assert.js`.
//!
//! `bind` makes a *new function object* with its own internal slots, which is a different thing
//! and belongs with whatever else needs one.

use crate::heap::{Heap, NativeCall, ObjectId, PropertyKey};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

use super::{define_method, key};

/// §20.2.3.3 `Function.prototype.call`.
///
/// The first argument is the receiver and the rest are the arguments, which is the whole
/// difference from an ordinary call: `f.call(o, 1)` is `o.f(1)` for an `f` that `o` never had.
pub fn call(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let arguments: Vec<Value> = call.arguments.iter().skip(1).copied().collect();
    vm.call_value(call.this_value, call.argument(0), &arguments, heap)
}

/// §20.2.3.1 `Function.prototype.apply`.
///
/// The same, with the arguments in a list. `null` and `undefined` mean *no* arguments rather than
/// one — step 3 — which is why `f.apply(o)` and `f.apply(o, null)` both call `f` with none.
pub fn apply(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let arguments = match call.argument(1) {
        Value::Undefined | Value::Null => Vec::new(),
        list => list_from(vm, heap, list)?,
    };
    vm.call_value(call.this_value, call.argument(0), &arguments, heap)
}

/// §7.3.19 `CreateListFromArrayLike` — the elements of anything with a `length`.
///
/// A hole reads as `undefined` here, unlike in most of §23.1.3: this is a plain `Get` of every
/// index, so `f.apply(null, [, 1])` passes two arguments and the first is `undefined`.
#[allow(clippy::manual_clamp)] // `clamp` answers NaN for NaN; §7.1.20 says a NaN length is 0
fn list_from(vm: &mut Vm, heap: &mut Heap, list: Value) -> Completion<Vec<Value>> {
    let Value::Object(object) = list else {
        return Err(Abrupt::type_error(
            "the arguments given to apply must be an object",
        ));
    };
    let name = key(heap, "length");
    let value = vm.get_property_key(Value::Object(object), name, heap)?;
    let length = vm.to_number(value, heap)?;
    // §7.1.20's clamp, and then the argument list's own: a call with more arguments than a
    // machine could hold is one no program wrote. `max` before `min` because `f64::max` answers
    // the other operand for NaN, which is what turns an absent or unreadable `length` into zero.
    let count = length.max(0.0).min(65_535.0) as u64;
    let mut arguments = Vec::new();
    for index in 0..count {
        let at =
            PropertyKey::from_units(heap, &index.to_string().encode_utf16().collect::<Vec<_>>());
        arguments.push(vm.get_property_key(Value::Object(object), at, heap)?);
    }
    Ok(arguments)
}

/// Build `Function.prototype`'s methods into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, _global: ObjectId) {
    let prototype = realm.function_prototype();
    define_method(heap, realm, prototype, "apply", 2, apply);
    define_method(heap, realm, prototype, "call", 1, call);
}
