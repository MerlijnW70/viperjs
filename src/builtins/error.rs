//! §20.5 — `Error`, the five native error types, and the one method they share.
//!
//! # Why this is the first thing built in Rust
//!
//! Because the conformance suite asked for it. `TypeError` alone was named by 1,291 failing
//! tests, and every negative test in test262 checks the *constructor* of what was thrown — so
//! until `TypeError` is a value a script can reach, a whole category of test cannot be scored at
//! all. It is also the smallest complete builtin there is: seven constructors, one method, and no
//! iteration, no coercion table, no allocation strategy.
//!
//! # The shape §20.5 actually has
//!
//! `Error` is a constructor whose `prototype` carries `name`, `message` and `toString`. Each
//! native error — `TypeError`, `RangeError` and the rest — repeats that shape one level down: its
//! prototype inherits from `Error.prototype` and overrides `name`, and *the constructor itself*
//! inherits from `Error` (§20.5.6.2). That second inheritance is the one people forget, and
//! `TypeError.__proto__ === Error` is what test262 checks it with.

use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

use super::{define_method, define_value, key, text};

/// §20.5.1.1 `Error(message)`, and §20.5.6.1.1 for every native error.
///
/// One function for all seven, because the specification says the same thing seven times: make an
/// object inheriting from *this constructor's* `prototype`, and give it an own `message` if a
/// message was passed. Which constructor it is answers itself — the call knows which function
/// object it is running.
///
/// `Error("x")` and `new Error("x")` do the same thing, which is why §20.5.1.1 step 1 mentions
/// `NewTarget` only to pick a prototype and never to refuse. That is unusual and deliberate:
/// nearly every other constructor in the language throws when called without `new`.
pub fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §10.1.13 `GetPrototypeFromConstructor` — new.target's own `prototype`, and
    // `%Error.prototype%` when a script has replaced it with something that is not an object.
    let prototype = super::prototype_from(vm, heap, call, Realm::error_prototype)?;
    let error = heap.new_object(Some(prototype));
    // §20.5.1.1 step 2's `« [[ErrorData]] »`, which is the slot §20.1.3.6 step 7 asks for.
    if let Some(object) = heap.object_mut(error) {
        object.make_error();
    }

    // §20.5.1.1 step 3 — `undefined` is *absent*, not a message. `new Error(undefined)` has no
    // own `message` at all and inherits the empty one, while `new Error("")` has an own empty
    // string. Nothing observes the difference through `toString`; `hasOwnProperty` does.
    let message = call.argument(0);
    if !matches!(message, Value::Undefined) {
        let text = message.to_string(heap)?;
        // §20.5.1.1 step 4 uses `CreateNonEnumerableDataPropertyOrThrow`, so a message is
        // writable and configurable and hidden from enumeration — an error does not list its own
        // message in a `for...in`, which is what makes errors safe to log wholesale.
        define_value(heap, error, "message", Value::String(text));
    }
    Ok(Value::Object(error))
}

/// §20.5.7.1 — `new AggregateError(errors, message)`.
///
/// The **errors come first**, which is the whole reason this is not the ordinary error constructor
/// with a different prototype: `new AggregateError("oops")` is a message-less error whose `errors`
/// is the characters of `"oops"`, and that is not a mistake in the specification — the first
/// argument is an iterable of what went wrong and the second is what to say about it.
fn aggregate_construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let prototype = super::prototype_from(vm, heap, call, Realm::aggregate_error_prototype)?;
    let error = heap.new_object(Some(prototype));
    // §20.5.1.1 step 2's `« [[ErrorData]] »`, which is the slot §20.1.3.6 step 7 asks for.
    if let Some(object) = heap.object_mut(error) {
        object.make_error();
    }
    // Step 3 — the message, on the same terms as any other error's: `undefined` is *absent*.
    let message = call.argument(1);
    if !matches!(message, Value::Undefined) {
        let text = message.to_string(heap)?;
        define_value(heap, error, "message", Value::String(text));
    }
    // Step 5 — `IterableToList`, and then step 6 defines `errors` as writable, **not enumerable**
    // and configurable. Not enumerable because an error is a thing programs log wholesale, and a
    // list of causes in every `for...in` over one would be a surprise.
    let list = super::promise_group::iterable_to_list(vm, heap, call.argument(0))?;
    let errors = super::array::from_values(vm, heap, &list)?;
    define_value(heap, error, "errors", errors);
    Ok(Value::Object(error))
}

/// §20.5.3.4 `Error.prototype.toString`.
///
/// `"Error: something went wrong"`, or just the name when there is no message, or just the
/// message when the name is empty. The three cases are §20.5.3.4 steps 8–10 and they are the
/// reason this is not simply `name + ": " + message`.
///
/// It reads `name` and `message` off `this` through the prototype chain rather than off the
/// error itself, which is why an error made by `Object.create(Error.prototype)` prints as
/// `"Error"` and why assigning `e.name` changes what it prints.
pub fn to_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.this_value else {
        // §20.5.3.4 step 2. Reachable as `Error.prototype.toString.call(1)`, and the reason
        // §10.3.1 must not substitute a receiver: with the global object put here instead, this
        // would answer `"undefined"` rather than refusing.
        return Err(Abrupt::type_error(
            "Error.prototype.toString requires an object",
        ));
    };
    // §20.5.3.4 steps 3 and 5 — absent is `"Error"` for the name and `""` for the message, which
    // is what makes an object with neither print as `"Error"`.
    let name = inherited_string(heap, object, "name", "Error")?;
    let message = inherited_string(heap, object, "message", "")?;
    let joined = match (name.is_empty(), message.is_empty()) {
        (true, true) => String::new(),
        (true, false) => message,
        (false, true) => name,
        (false, false) => format!("{name}: {message}"),
    };
    Ok(text(heap, &joined))
}

/// A property of `object` or its prototypes, as Rust text, with a default when it is `undefined`.
fn inherited_string(
    heap: &mut Heap,
    object: ObjectId,
    name: &str,
    default: &str,
) -> Completion<String> {
    let key = key(heap, name);
    let found = heap.find_own(object, key).map(|(_, property)| property);
    let value = match found.map(|property| property.kind) {
        Some(crate::heap::PropertyKind::Data { value, .. }) => value,
        // An accessor would need its getter called, and nothing on these prototypes is one.
        // Reading it as absent is the same answer the default gives, and is not a guess.
        _ => Value::Undefined,
    };
    if matches!(value, Value::Undefined) {
        return Ok(default.to_string());
    }
    let id = value.to_string(heap)?;
    Ok(String::from_utf16_lossy(heap.string(id).unwrap_or(&[])))
}

/// Build `Error` and the five native errors into `heap`, and answer their prototypes.
///
/// Order matters twice. `Error.prototype` must exist before a native error's prototype can
/// inherit from it, and `Error` itself must exist before a native error's *constructor* can
/// inherit from it — §20.5.6.2, the second inheritance.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let error = constructor(heap, realm, "Error", realm.error_prototype(), None);
    for (name, prototype) in realm.native_error_prototypes() {
        let made = constructor(heap, realm, name, prototype, Some(error));
        define_value(heap, global, name, Value::Object(made));
    }
    define_value(heap, global, "Error", Value::Object(error));
    // §20.5.2.1 — `Error.isError`, which asks about the `[[ErrorData]]` slot and nothing else.
    // Not `instanceof` and not the `@@toStringTag`: a plain object with `Error.prototype` behind it
    // answers `false`, and an error from another realm answers `true`. That is the whole point of
    // it — the two questions a program could ask before this one both give the wrong answer across
    // a realm boundary, and neither can be fixed by the program.
    define_method(heap, realm, error, "isError", 1, is_error);

    // §20.5.7 — its own constructor, because its arguments are in a different order and its
    // instances carry a property no other error has. Its `[[Prototype]]` is `Error`, exactly as a
    // native error's is, so it inherits the constructor's properties in the same way.
    let aggregate = realm.aggregate_error_prototype();
    let made = heap.new_native_constructor(error, aggregate_construct, realm.id());
    super::define_function_metadata(heap, made, "AggregateError", 2);
    let key = key(heap, "prototype");
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(aggregate)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(made, key, &descriptor);
    define_value(heap, aggregate, "constructor", Value::Object(made));
    let name = super::text(heap, "AggregateError");
    define_value(heap, aggregate, "name", name);
    let message = super::text(heap, "");
    define_value(heap, aggregate, "message", message);
    define_value(heap, global, "AggregateError", Value::Object(made));

    // §20.5.3.4 — the one method, on `Error.prototype`, inherited by every native error's
    // prototype rather than repeated on each. That is why `new Abrupt::type_error("x") + ""` says
    // `"TypeError: x"` with no `TypeError.prototype.toString` anywhere.
    define_method(
        heap,
        realm,
        realm.error_prototype(),
        "toString",
        0,
        to_string,
    );
}

/// One constructor: the function, its `prototype` pair, and §10.3.3's `name` and `length`.
///
/// `inherits` is §20.5.6.2's second inheritance — a native error's constructor has `Error` as its
/// prototype, where `Error`'s own is `%Function.prototype%`. It is not decoration: `TypeError`
/// inherits `Error`'s properties through it, and test262 checks the link directly.
fn constructor(
    heap: &mut Heap,
    realm: &Realm,
    name: &str,
    prototype: ObjectId,
    inherits: Option<ObjectId>,
) -> ObjectId {
    let parent = inherits.unwrap_or_else(|| realm.function_prototype());
    let function = heap.new_native_constructor(parent, construct, realm.id());
    super::define_function_metadata(heap, function, name, 1);

    // §20.5.2 — `Error.prototype` on the constructor is **not** writable and **not**
    // configurable, unlike the `prototype` of a JavaScript function. A script may replace
    // `f.prototype` and may not replace `Error.prototype`.
    let key = key(heap, "prototype");
    let descriptor = PropertyDescriptor {
        value: Some(Value::Object(prototype)),
        writable: Some(false),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    let _ = heap.define_own_property(function, key, &descriptor);
    // …and the way back, which is what `assert.throws` compares and what
    // `new TypeError().constructor === TypeError` asks.
    define_value(heap, prototype, "constructor", Value::Object(function));
    function
}

/// §20.5.2.1 `Error.isError ( arg )`.
///
/// Three steps and no coercion: anything that is not an Object is `false`, an Object without
/// `[[ErrorData]]` is `false`, and everything else is `true`. It never throws and never reads a
/// property, so a Proxy over an error answers `false` — the slot is on the target and a Proxy has
/// none of its own.
fn is_error(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.argument(0) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(
        heap.object(object)
            .is_some_and(crate::heap::Object::is_error),
    ))
}
