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

use crate::heap::{Bound, Heap, NativeCall, Object, ObjectId, PropertyKey};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

use super::{define_method, define_value, key};

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
pub(super) fn list_from(vm: &mut Vm, heap: &mut Heap, list: Value) -> Completion<Vec<Value>> {
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

/// §20.2.3.2 `Function.prototype.bind`.
///
/// Answers a *new* function that calls this one with a receiver and some arguments already
/// decided — §10.4.1's bound function exotic object, which is not a function of its own but a
/// thing standing in front of one.
///
/// `length` and `name` are computed here rather than left off, because they are what a program
/// reads to tell a bound function from what it was bound to: §20.2.3.2 steps 5 to 8 make the
/// length what is *left* after the bound arguments, and the name the target's with `bound `
/// written in front of it.
pub fn bind(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(target) = call.this_value else {
        return Err(Abrupt::type_error("bind must be called on a function"));
    };
    if heap.object(target).and_then(Object::call).is_none() {
        return Err(Abrupt::type_error("bind must be called on a function"));
    }
    let this_value = call.argument(0);
    let arguments: Vec<Value> = call.arguments.iter().skip(1).copied().collect();
    let taken = arguments.len();

    let prototype = heap.object(target).and_then(Object::prototype);
    let bound = heap.new_bound_function(
        prototype,
        Bound {
            // §10.4.1.3 step 2 — settled now, from what the target *is*, because that is when the
            // specification asks. `Math.max.bind(null)` is not a constructor and never becomes one.
            constructs: heap
                .object(target)
                .and_then(crate::heap::Object::call)
                .is_some_and(crate::heap::Callable::constructs),
            target,
            this_value,
            arguments,
        },
    );

    // §20.2.3.2 steps 5 and 6 — the length is what a caller still has to supply, and it is only
    // asked for when the target has one of its own. A target without `length` gives 0, which is
    // what step 6.a says rather than a guess.
    let length_key = key(heap, "length");
    let remaining = match heap
        .object(target)
        .and_then(|found| found.get_own_property(length_key))
    {
        Some(property) => match property.kind {
            crate::heap::PropertyKind::Data {
                value: Value::Number(length),
                ..
            } => (length - taken as f64).max(0.0),
            _ => 0.0,
        },
        None => 0.0,
    };
    // §20.2.3.2 step 8 — `bound ` in front of the target's name, and in front of nothing when the
    // target has no name to speak of.
    let name_key = key(heap, "name");
    let target_name = match heap
        .object(target)
        .and_then(|found| found.get_own_property(name_key))
    {
        Some(property) => match property.kind {
            crate::heap::PropertyKind::Data {
                value: Value::String(name),
                ..
            } => String::from_utf16_lossy(heap.string(name).unwrap_or(&[])),
            _ => String::new(),
        },
        None => String::new(),
    };
    let name = crate::builtins::text(heap, &format!("bound {target_name}"));
    crate::builtins::define_metadata(heap, bound, Value::Number(remaining), name);
    let _ = vm;
    Ok(Value::Object(bound))
}

/// §20.2.1.1 `Function(...)` — building a function out of source text at run time.
///
/// Refused, and deliberately loudly. Everything else about `Function` is here: the object exists,
/// `Function.prototype` is reachable through it, and `instanceof Function` works. What is missing
/// is the part that compiles a String, which needs the parser and the compiler run from inside a
/// built-in and a global scope to compile against — a slice of its own.
///
/// A TypeError rather than nothing, because the alternative to saying so is answering with a
/// function that does not do what its source says.
fn construct(_vm: &mut Vm, _heap: &mut Heap, _call: &NativeCall<'_>) -> Completion<Value> {
    Err(Abrupt::type_error(
        "building a function from source text is not implemented yet",
    ))
}

/// Build `Function`, and `Function.prototype`'s methods, into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.function_prototype();
    define_method(heap, realm, prototype, "apply", 2, apply);
    define_method(heap, realm, prototype, "call", 1, call);
    define_method(heap, realm, prototype, "bind", 1, bind);

    // §20.2.2 — the constructor, and the `prototype` that every function in the realm already
    // inherits from. Not writable, not enumerable and not configurable, for the reason
    // `Object.prototype` is not: everything callable points at it.
    let function = heap.new_native_constructor(prototype, construct);
    crate::builtins::define_function_metadata(heap, function, "Function", 1);
    crate::builtins::define_fixed(heap, function, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, "Function", Value::Object(function));
}
