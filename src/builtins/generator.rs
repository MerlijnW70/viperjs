//! §27.3 and §27.5 — generator functions, generator objects, and the three ways to resume one.
//!
//! # Why `next` is not a built-in like every other method here
//!
//! Every other module in `builtins` installs a [`crate::heap::Native`]: a Rust `fn` that runs to
//! completion and answers a value. `Generator.prototype.next` cannot be one, because it does not
//! answer a value — it hands a *body* to the interpreter and the answer arrives whenever that body
//! next stops. A native would have to run the body itself, through DR-0011's nested execution, and
//! then it would be a Rust call per resumption in a program that may have millions.
//!
//! So the three methods are installed as [`crate::heap::Callable::Resume`], which the interpreter's
//! own `enter` recognises: resuming a generator is a way of *entering the loop*, alongside an
//! ordinary call. The function objects are otherwise built exactly as a built-in method is, and
//! nothing a script can ask distinguishes them — `typeof gen.next` is `"function"`, it has a `name`
//! and a `length`, and it is not a constructor.
//!
//! # The two prototypes
//!
//! %GeneratorFunction.prototype% is what a generator *function* inherits from, and
//! %GeneratorPrototype% is what a generator *object* does. The second is the first's `prototype`
//! property, which is the link `Object.getPrototypeOf(g()) === g.prototype`'s chain rests on —
//! and it inherits from %IteratorPrototype%, which is the whole of what makes a generator usable
//! in a `for`-`of`.
//!
//! Neither is reachable by name. §27.3 does not put `GeneratorFunction` on the global object, so a
//! script arrives at both only through `Object.getPrototypeOf(function* () {})`.

use super::{define_value, key};
use crate::heap::{Heap, PropertyDescriptor, PropertyKey, Resumption};
use crate::realm::Realm;
use crate::value::Value;

/// Build §27.3.3's and §27.5.1's prototypes and the links between them.
pub fn install(heap: &mut Heap, realm: &Realm) {
    let function_prototype = realm.generator_function_prototype();
    let prototype = realm.generator_prototype();

    // §27.5.1 — the three resumptions. `next` takes one argument and so do the other two, which is
    // what §27.5.1.2's `length` of 1 says.
    for (name, kind) in [
        ("next", Resumption::Next),
        ("return", Resumption::Return),
        ("throw", Resumption::Throw),
    ] {
        let function = heap.new_resume_function(realm.function_prototype(), kind, false);
        super::define_function_metadata(heap, function, name, 1);
        define_value(heap, prototype, name, Value::Object(function));
    }

    // §27.3.3.3 and §27.5.1.5 — the tags, which are what `Object.prototype.toString` prints and
    // the only thing that tells the two objects apart from a script.
    for (object, tag) in [
        (function_prototype, "GeneratorFunction"),
        (prototype, "Generator"),
    ] {
        tag_with(heap, object, tag);
    }

    // §27.3.3.2 — `%GeneratorFunction.prototype%.prototype` is %GeneratorPrototype%, and it is not
    // writable: everything else in the realm is built on where it is. §27.5.1.1's `constructor`
    // points back, which is what makes the pair a chain rather than two objects.
    fixed(
        heap,
        function_prototype,
        "prototype",
        Value::Object(prototype),
    );
    fixed(
        heap,
        prototype,
        "constructor",
        Value::Object(function_prototype),
    );
    install_async(heap, realm);
    // §27.7.3 — the plain async function's prototype, whose `@@toStringTag` is what makes
    // `Object.prototype.toString.call(async function () {})` answer `"[object AsyncFunction]"`.
    // §27.7.2's `constructor` is `%AsyncFunction%` and is written by `function::install`, beside
    // the two generator constructors and for the same reason: all three inherit from `%Function%`.
    // This comment used to say it "is not built", which stopped being true and read as a plan.
    tag_with(heap, realm.async_function_prototype(), "AsyncFunction");
}

/// §27.4.3 and §27.6.1 — the same four things again, for the async generator.
///
/// Written out rather than shared with a loop, because the two differ in more than the objects: the
/// async trio is `asynchronous`, which is what makes `next` answer a promise, and §27.6.1's methods
/// **refuse a synchronous generator** as their receiver just as §27.5.1's refuse an async one. A
/// helper taking both sets would have to carry that flag anyway, and the flag is the whole
/// difference.
fn install_async(heap: &mut Heap, realm: &Realm) {
    let function_prototype = realm.async_generator_function_prototype();
    let prototype = realm.async_generator_prototype();
    for (name, kind) in [
        ("next", Resumption::Next),
        ("return", Resumption::Return),
        ("throw", Resumption::Throw),
    ] {
        let function = heap.new_resume_function(realm.function_prototype(), kind, true);
        super::define_function_metadata(heap, function, name, 1);
        define_value(heap, prototype, name, Value::Object(function));
    }
    for (object, tag) in [
        (function_prototype, "AsyncGeneratorFunction"),
        (prototype, "AsyncGenerator"),
    ] {
        tag_with(heap, object, tag);
    }
    fixed(
        heap,
        function_prototype,
        "prototype",
        Value::Object(prototype),
    );
    fixed(
        heap,
        prototype,
        "constructor",
        Value::Object(function_prototype),
    );
}

/// A property that may be read and neither written nor enumerated — §27.3.3.2's attributes.
///
/// Not [`super::define_fixed`], which also refuses `configurable`: §27.3.3 gives both of these
/// `{ writable: false, enumerable: false, configurable: true }`, and the third is what lets a
/// script replace the link rather than merely being told it exists.
fn fixed(heap: &mut Heap, object: crate::heap::ObjectId, name: &str, value: Value) {
    let name = key(heap, name);
    let _ = heap.define_own_property(
        object,
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

/// §27.3.3.3's `[@@toStringTag]`, with the attributes that clause gives it.
fn tag_with(heap: &mut Heap, object: crate::heap::ObjectId, tag: &str) {
    let Some(symbol) = heap.well_known(super::well_known_at("toStringTag")) else {
        return;
    };
    let name = PropertyKey::from_symbol(symbol);
    let units: Vec<u16> = tag.encode_utf16().collect();
    let value = Value::String(heap.intern(&units));
    let _ = heap.define_own_property(
        object,
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
