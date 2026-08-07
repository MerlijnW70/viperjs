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

use crate::heap::{Bound, Heap, NativeCall, Object, ObjectId, PropertyDescriptor, PropertyKey};
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

/// §20.2.3.5 `Function.prototype.toString`.
///
/// # Why every function answers the `[native code]` form
///
/// Step 2 answers a function's `[[SourceText]]` — but only when `HostHasSourceTextAvailable` says
/// so, and a host is allowed to say no. ViperJS says no for everything: a compiled chunk does not
/// keep the text it came from, and retaining it would mean holding every script alive for as long
/// as any function in it. So step 3's `NativeFunction` form is what comes back, for a function
/// written in JavaScript as much as for a built-in.
///
/// That is a real limitation and not a reading of the clause: a program using `String(f)` to
/// re-parse a function gets nothing useful. It is written down here because the alternative — a
/// plausible-looking reconstruction — would be worse, and because keeping the source is a change
/// to the compiler rather than to this function.
///
/// The name goes in as the `name` *property* reads it, so an accessor's `"get x"` produces
/// `function get x() { [native code] }` — which is what §20.2.3.5's grammar calls a
/// `NativeFunctionAccessor`, and is why the two spellings need no case of their own.
fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 5 — anything that is not callable is a TypeError. `Function.prototype.toString.call({})`
    // has nothing to answer about, and answering `"[object Object]"` would be a lie about what it
    // was asked.
    if !heap.is_callable(call.this_value) {
        return Err(Abrupt::type_error(
            "Function.prototype.toString requires a function",
        ));
    }
    let name = key(heap, "name");
    let held = vm.get_property_key(call.this_value, name, heap)?;
    let spelled = match held {
        Value::Undefined => Vec::new(),
        given => {
            let text = vm.to_string(given, heap)?;
            heap.string(text).unwrap_or(&[]).to_vec()
        }
    };
    let mut text: Vec<u16> = "function ".encode_utf16().collect();
    text.extend_from_slice(&spelled);
    text.extend("() { [native code] }".encode_utf16());
    Ok(Value::String(heap.intern(&text)))
}

/// §20.2.3.1 `Function.prototype.apply`.
///
/// The same, with the arguments in a list. `null` and `undefined` mean *no* arguments rather than
/// one — step 3 — which is why `f.apply(o)` and `f.apply(o, null)` both call `f` with none.
pub fn apply(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let arguments = match call.argument(1) {
        Value::Undefined | Value::Null => Vec::new(),
        list => list_from(
            vm,
            heap,
            list,
            "the arguments given to apply must be an object",
        )?,
    };
    vm.call_value(call.this_value, call.argument(0), &arguments, heap)
}

/// §7.3.19 `CreateListFromArrayLike` — the elements of anything with a `length`.
///
/// A hole reads as `undefined` here, unlike in most of §23.1.3: this is a plain `Get` of every
/// index, so `f.apply(null, [, 1])` passes two arguments and the first is `undefined`.
#[allow(clippy::manual_clamp)] // `clamp` answers NaN for NaN; §7.1.20 says a NaN length is 0
pub(crate) fn list_from(
    vm: &mut Vm,
    heap: &mut Heap,
    list: Value,
    wanted: &'static str,
) -> Completion<Vec<Value>> {
    let Value::Object(object) = list else {
        return Err(Abrupt::type_error(wanted));
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

/// §20.2.1.1 `CreateDynamicFunction` — a function built out of source text at run time.
///
/// # The scope it compiles against is the global one, and that is the whole reason this is
/// tractable
///
/// §20.2.1.1.1 step 30 gives the new function the *realm's* global environment, never the caller's.
/// So `function f() { var x = 1; return Function("return typeof x")(); }` answers `"undefined"` —
/// the `x` beside it is invisible, and a reader who expects a closure is reading `eval`. That is
/// what separates this from direct `eval`, which does need the caller's scope and is a slice of its
/// own: here the compiler is handed no outer scopes at all, so every free name compiles to a global
/// lookup, which is exactly what the clause asks for.
///
/// # Why the source is reassembled and reparsed rather than spliced
///
/// Steps 12 to 20 build one string — `function anonymous(P\n) {\nbody\n}` — and require the
/// *whole* of it to parse. That is not decoration: it is what makes `Function("a", "){ } , function
/// f2(") a SyntaxError instead of two functions, because the parameter text and the body text have
/// to agree about where the function ends. Parsing them separately would accept it.
///
/// The newlines are the specification's and are load-bearing too: a body ending in a `//` comment
/// would otherwise swallow the closing brace.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    dynamic_function(vm, heap, call, Kind::Ordinary)
}

/// §27.7.1.1 `AsyncFunction(...)` — the same operation, one word further along in the source.
///
/// §27.7 does not put `AsyncFunction` on the global object, so the only way a program reaches this
/// is `Object.getPrototypeOf(async function () {}).constructor` — which is exactly how test262's
/// `getWellKnownIntrinsicObject` finds it, and why the object has to exist even though nothing can
/// name it.
fn async_construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    dynamic_function(vm, heap, call, Kind::Async)
}

/// §27.3.1.1 `GeneratorFunction(...)` — the same operation with `function*` in front.
///
/// Reached only through `Object.getPrototypeOf(function* () {}).constructor`, exactly as
/// `%AsyncFunction%` is: §27.3 keeps `GeneratorFunction` off the global object. Before this it was
/// not built at all, so that lookup walked past %GeneratorFunction.prototype% to `Function.prototype`
/// and found plain `%Function%` — which then assembled `function anonymous() { yield 1 }` and
/// refused it as a SyntaxError. A missing intrinsic answering as its own parent is worse than one
/// answering `undefined`, because the wrong object is callable and the error is about the wrong
/// thing.
fn generator_construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    dynamic_function(vm, heap, call, Kind::Generator)
}

/// §27.4.1.1 `AsyncGeneratorFunction(...)` — `async function*`.
fn async_generator_construct(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
) -> Completion<Value> {
    dynamic_function(vm, heap, call, Kind::AsyncGenerator)
}

/// Which of §20.2.1.1's four `CreateDynamicFunction` kinds is being built.
///
/// All four. The parameters and the body are assembled identically and the source differs by a
/// word, which is what the clause says: one operation with a `kind` argument. What the four do
/// *not* share is step 27 — the `prototype` property — and that is the one place they are told
/// apart below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// §20.2.1.1 — `Function`.
    Ordinary,
    /// §27.7.1.1 — `AsyncFunction`.
    Async,
    /// §27.3.1.1 — `GeneratorFunction`.
    Generator,
    /// §27.4.1.1 — `AsyncGeneratorFunction`.
    AsyncGenerator,
}

impl Kind {
    /// What goes in front of `anonymous` in the assembled source.
    fn prefix(self) -> &'static str {
        match self {
            Self::Ordinary => "function",
            Self::Async => "async function",
            Self::Generator => "function*",
            Self::AsyncGenerator => "async function*",
        }
    }

    /// The `[[Prototype]]` the built function gets when there is no `new.target` to take one from.
    fn prototype(self, realm: &crate::realm::Realm) -> crate::heap::ObjectId {
        match self {
            Self::Ordinary => realm.function_prototype(),
            Self::Async => realm.async_function_prototype(),
            Self::Generator => realm.generator_function_prototype(),
            Self::AsyncGenerator => realm.async_generator_function_prototype(),
        }
    }
}

/// §20.2.1.1 `CreateDynamicFunction`, for whichever kind asked.
fn dynamic_function(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kind: Kind,
) -> Completion<Value> {
    // Steps 5 to 11 — the last argument is the body and everything before it is a parameter, so
    // `Function()` is a function of no arguments with an empty body rather than an error.
    let (parameters, body) = match call.arguments.split_last() {
        Some((body, parameters)) => (parameters, *body),
        None => (&[] as &[Value], Value::Undefined),
    };
    let mut written = String::new();
    for (at, parameter) in parameters.iter().enumerate() {
        if at > 0 {
            written.push(',');
        }
        written.push_str(&text_of(vm, heap, *parameter)?);
    }
    let body = match call.arguments.is_empty() {
        true => String::new(),
        false => text_of(vm, heap, body)?,
    };
    let source = format!("{} anonymous({written}\n) {{\n{body}\n}}", kind.prefix());

    // Step 20's assertion, as a real check. A refusal here is a **SyntaxError**, which is what the
    // clause says and is also the one a program tests for.
    let script = crate::parser::parse_script(&source).map_err(|_| {
        Abrupt::Raised(
            crate::value::ErrorKind::Syntax,
            "the source of a dynamic function does not parse",
        )
    })?;
    // Steps 19 and 21 parse the parameters and the body *separately* as well as together, and this
    // is what that catches: a body of `return }{` closes the function early, so the combined text
    // parses happily as a function **and a block beside it**. One statement is the whole of the
    // check — anything the body smuggled out of the braces shows up as a second one.
    if script.body.len() != 1 {
        return Err(Abrupt::Raised(
            crate::value::ErrorKind::Syntax,
            "the source of a dynamic function does not parse as one function",
        ));
    }
    let compiled = crate::compile::compile_script(&script, heap)
        // Not a SyntaxError: the text *is* a program and this engine cannot compile it yet.
        // Reporting it as one would tell a program its own source is malformed.
        .map_err(|error| Abrupt::type_error(leaked(error.message())))?;
    let Some(inner) = compiled.function(0) else {
        // The script above is one function declaration and nothing else, so it has one nested body.
        return Err(Abrupt::type_error("a dynamic function compiled to nothing"));
    };
    let inner = std::rc::Rc::clone(inner);

    // §20.2.1.1.1 step 28 — the prototype comes from `new.target` when there was one, which is what
    // makes a subclass of `Function` produce instances of itself.
    let prototype = super::prototype_from(vm, heap, call, |realm| kind.prototype(realm))?;
    // Step 30 — the global environment, and an empty one of its own so that the body's own slots
    // have somewhere to live. Its parent is `None`: there is nothing outside a dynamic function.
    let environment = heap.new_environment(None, 0);
    let object = heap.new_function(prototype, inner, environment, None, vm.realm().id());
    // §20.2.1.1.1 steps 31 to 33 — `length` is how many parameters were written, `name` is
    // `"anonymous"` whatever the source says, and it is a constructor like any ordinary function.
    let length = u32::try_from(parameters.len()).unwrap_or(u32::MAX);
    super::define_function_metadata(heap, object, "anonymous", length);
    // §20.2.1.1.1 step 27 — the `prototype` property, and this is where the four kinds differ.
    //
    // Only the ordinary kind is a **constructor**: §27.7.4, §27.3.4 and §27.4.4 all say an async
    // function, a generator and an async generator have no `[[Construct]]`, so
    // `new (async function () {})` is a TypeError and a dynamic one must be no different. A plain
    // `async function` gets nothing at all; the two generator kinds get §15.5.4's `prototype`,
    // which is not `MakeConstructor`'s — it inherits from %GeneratorPrototype% and has no
    // `constructor` back-pointer, because a property saying a generator function is a constructor
    // would be a lie a script can read.
    match kind {
        Kind::Ordinary => vm.realm().make_constructor(heap, object),
        Kind::Async => {}
        Kind::Generator => vm.realm().make_generator_function(heap, object, false),
        Kind::AsyncGenerator => vm.realm().make_generator_function(heap, object, true),
    }
    Ok(Value::Object(object))
}

/// A compiler refusal, worded for a script rather than for this repository.
///
/// [`crate::compile::CompileError::message`] is written for a reader of the engine. What reaches a
/// program is the same sentence, and it says what was not built rather than what to do about it.
fn leaked(message: String) -> &'static str {
    let _ = message;
    "building this function from source text is not implemented yet"
}

/// §7.1.17 `ToString`, for one of the arguments a dynamic function is assembled from.
fn text_of(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<String> {
    let text = vm.to_string(value, heap)?;
    Ok(String::from_utf16_lossy(heap.string(text).unwrap_or(&[])))
}

/// Build `Function`, and `Function.prototype`'s methods, into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.function_prototype();
    // §20.2.3 — `Function.prototype` **is itself a built-in function object**, not an ordinary one:
    // `typeof Function.prototype` is `"function"`, calling it answers `undefined` whatever it is
    // given, and it has no `[[Construct]]`. The realm makes it before any native exists, so the
    // `[[Call]]` is attached here rather than at construction — which is what `make_callable` is
    // for, and the second thing in the engine to need it.
    //
    // Not a curiosity. §7.3.22 step 1 answers `false` for a receiver that is not callable, so
    // `[] instanceof Function.prototype` reached that `false` and answered instead of running
    // step 4, which is where reading a `prototype` of `""` is the TypeError test262 asks for.
    heap.make_callable(prototype, returns_undefined, false, realm.id());
    // …and being a function object, it has §10.3.3's two own properties like any other: a `length`
    // of +0 and a `name` of the **empty string**. Both are what §20.2.3 writes down rather than a
    // consequence of anything, which is why they are stated here and not derived.
    //
    // What they are for is the lookup that lands here after a delete. `length` and `name` are
    // configurable on every built-in, so `delete decodeURI.length` succeeds — and what
    // `decodeURI.length` then answers is whatever the prototype chain has, which is this. Without
    // them it answered `undefined`, and the difference is only ever visible one step up the chain.
    crate::builtins::define_function_metadata(heap, prototype, "", 0);
    define_method(heap, realm, prototype, "toString", 0, to_string);
    define_method(heap, realm, prototype, "apply", 2, apply);
    define_method(heap, realm, prototype, "call", 1, call);
    define_method(heap, realm, prototype, "bind", 1, bind);

    // §20.2.2 — the constructor, and the `prototype` that every function in the realm already
    // inherits from. Not writable, not enumerable and not configurable, for the reason
    // `Object.prototype` is not: everything callable points at it.
    let function = heap.new_native_constructor(prototype, construct, realm.id());
    crate::builtins::define_function_metadata(heap, function, "Function", 1);
    crate::builtins::define_fixed(heap, function, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(function));
    define_value(heap, global, "Function", Value::Object(function));

    // §20.2.3.6 — the method `instanceof` looks up on every use, and the **only** one on
    // `Function.prototype` that is neither writable nor configurable. §17's usual attributes would
    // let a program replace it and change what the operator means for every function in the realm
    // at once; they would also make `Vm::is_default_has_instance` a guess rather than a fact.
    let has_instance = heap.new_native_function(prototype, has_instance, realm.id());
    crate::builtins::define_function_metadata(heap, has_instance, "[Symbol.hasInstance]", 1);
    if let Some(symbol) = heap.well_known(crate::builtins::well_known_at("hasInstance")) {
        let _ = heap.define_own_property(
            prototype,
            crate::heap::PropertyKey::from_symbol(symbol),
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(has_instance)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
    }

    // §27.7.2 — `%AsyncFunction%`, which §27.7 deliberately does not put on the global object:
    // the only route to it is `Object.getPrototypeOf(async function () {}).constructor`, and it
    // has to exist for that route to lead anywhere. Its own `[[Prototype]]` is `%Function%`,
    // which is what makes `AsyncFunction instanceof Function` true.
    let async_prototype = realm.async_function_prototype();
    let async_function = heap.new_native_constructor(function, async_construct, realm.id());
    crate::builtins::define_function_metadata(heap, async_function, "AsyncFunction", 1);
    crate::builtins::define_fixed(
        heap,
        async_function,
        "prototype",
        Value::Object(async_prototype),
    );
    // §27.7.3.1 — writable false, enumerable false, **configurable true**, which is the shape
    // every `constructor` on a prototype has and is not the shape `prototype` itself has.
    let name = crate::builtins::key(heap, "constructor");
    let _ = heap.define_own_property(
        async_prototype,
        name,
        &crate::heap::PropertyDescriptor {
            value: Some(Value::Object(async_function)),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..crate::heap::PropertyDescriptor::EMPTY
        },
    );

    // §27.3.1 and §27.4.1 — `%GeneratorFunction%` and `%AsyncGeneratorFunction%`, kept off the
    // global object by their clauses exactly as `%AsyncFunction%` is. Built here rather than beside
    // the generator prototypes because `%Function%` is what they inherit from and it is in scope
    // here; the prototypes themselves belong to the realm and exist before any of this runs.
    for (name, target, build) in [
        (
            "GeneratorFunction",
            realm.generator_function_prototype(),
            generator_construct as crate::heap::Native,
        ),
        (
            "AsyncGeneratorFunction",
            realm.async_generator_function_prototype(),
            async_generator_construct as crate::heap::Native,
        ),
    ] {
        let made = heap.new_native_constructor(function, build, realm.id());
        crate::builtins::define_function_metadata(heap, made, name, 1);
        // §27.3.2.1 — `prototype` here is **not configurable**, which is the one attribute that
        // differs from the `constructor` pointing the other way. Two links between the same pair of
        // objects with different shapes, and `propertyHelper.js` checks both.
        crate::builtins::define_fixed(heap, made, "prototype", Value::Object(target));
        // §27.3.3.1 — writable false, enumerable false, configurable **true**.
        let key = crate::builtins::key(heap, "constructor");
        let _ = heap.define_own_property(
            target,
            key,
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(made)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
    }

    restrict(heap, realm, prototype);
}

/// §20.2.3 — what calling `Function.prototype` itself does.
///
/// "Accepts any arguments and returns undefined." There is nothing else to it, and the reason it
/// exists at all is compatibility: `Function.prototype` was a function in every edition, and code
/// that reaches for `typeof` on it or hands it somewhere callable predates any of the alternatives.
fn returns_undefined(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Undefined)
}

/// §20.2.3.6 `Function.prototype [ %Symbol.hasInstance% ] ( V )`.
///
/// Two steps and no checks of its own: the receiver is whatever `instanceof` was written against,
/// and §7.3.22 answers `false` for one that is not callable rather than refusing it. So
/// `Function.prototype[Symbol.hasInstance].call(1, {})` is `false` and not a TypeError.
fn has_instance(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    vm.ordinary_has_instance(call.this_value, call.argument(0), heap)
}

/// §10.2.4 `AddRestrictedFunctionProperties` — the two names a function may not answer for.
///
/// `caller` and `arguments` were ES5's way to walk the call stack from inside a function, and
/// ES5's strict mode closed it off. What replaced them is not their absence: they are **accessor**
/// properties on `Function.prototype` whose getter and setter are both §10.2.4.1's
/// %ThrowTypeError%, so reaching for either through any function is a TypeError rather than
/// `undefined`. The difference matters to a program that asks — `f.caller` throwing is what says
/// the language refuses, where `undefined` would say this engine has not got round to it.
///
/// On the *prototype* and on no individual function, which is where ES2015 moved them: ES5 put a
/// pair on every strict function, and a test that asks
/// `Object.prototype.hasOwnProperty.call(f, "caller")` can tell the two apart.
///
/// One %ThrowTypeError% for the realm and not one per property. §10.2.4.1 makes it a single object,
/// and a program can see that: both halves of both accessors are the same function, and it is the
/// same one an unmapped arguments object's `callee` is poisoned with.
fn restrict(heap: &mut Heap, realm: &Realm, prototype: ObjectId) {
    let thrower = realm.thrower();
    for name in ["caller", "arguments"] {
        let key = key(heap, name);
        let _ = heap.define_own_property(
            prototype,
            key,
            &PropertyDescriptor {
                getter: Some(Value::Object(thrower)),
                setter: Some(Value::Object(thrower)),
                enumerable: Some(false),
                // §10.2.4 step 2 — configurable, so a host or a script may replace them. That is
                // the one attribute the two restricted properties do not share with §17's usual
                // shape, and it is deliberate.
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}
