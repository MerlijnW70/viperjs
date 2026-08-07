//! The objects that exist before any code runs — §9.3's realm, in the part that has one yet.
//!
//! A realm is the set of intrinsic objects a script can reach without making them: the prototypes
//! everything inherits from, the constructors, the global object. §9.3 lists about two hundred of
//! them; four are here, and they are the four an engine needs before it can *report* anything.
//!
//! # Why an error is an object at all
//!
//! Because `catch (e) { e.message }` has to work, and because `throw` takes a value rather than a
//! condition. [`crate::value::Abrupt`] says *which* error and why; this decides what object
//! stands for it, and that decision needs a prototype, which needs a realm. Keeping the two apart
//! is what lets `value/` stay a description of values rather than of a running engine.
//!
//! # What is here, and how you can tell what is not
//!
//! §9.3 lists about two hundred intrinsics and most of them are built. This section read "`Object`
//! and the error hierarchy are here; everything else — `Array`, `String`, `Number`,
//! `Function.prototype`'s methods — is not", which describes the engine as it stood around M3 and
//! has been wrong for the whole of §22 through §28.
//!
//! What is still absent is named by the conformance expectations rather than by a list here, for
//! the reason that list demonstrates: a file naming absences goes stale the moment one is filled,
//! and the expectations file cannot, because a test that starts passing fails the build until its
//! line is deleted.

use crate::heap::{Heap, ObjectId, PropertyDescriptor, PropertyKey};
use crate::value::Value;

/// The intrinsic objects, and the prototypes everything else is given.
#[derive(Debug, Clone, Copy)]
pub struct Realm {
    object_prototype: ObjectId,
    global: ObjectId,
    function_prototype: ObjectId,
    error_prototype: ObjectId,
    array_prototype: ObjectId,
    /// What `new Boolean(true)` inherits from.
    boolean_prototype: ObjectId,
    /// What `new Number(1)` inherits from.
    ///
    /// An ordinary object and *not* a Number wrapper, so that
    /// `Number.prototype.valueOf.call(Number.prototype)` is a TypeError rather than zero.
    number_prototype: ObjectId,
    /// What `new Date()` inherits from.
    ///
    /// An ordinary object and *not* a Date, which §21.4.4 requires: it holds no `[[DateValue]]`, so
    /// `Date.prototype.getTime.call(Date.prototype)` is the TypeError §21.4.4.10 asks for rather
    /// than NaN. ES5 had it as a Date whose value was NaN; ES2015 changed it, and the change is
    /// observable exactly here.
    date_prototype: ObjectId,
    /// What `new String("a")` inherits from.
    ///
    /// A String exotic object over the empty String, and §22.1.3 is explicit that it is one — so
    /// `String.prototype.length` is `0` rather than absent, and `String.prototype[0]` is
    /// `undefined` because there is no character there rather than because it is not that kind of
    /// object. That is the difference from `Number.prototype`, which is deliberately ordinary.
    string_prototype: ObjectId,
    /// What every Symbol finds its three methods on.
    ///
    /// An ordinary object, like `Number.prototype` and unlike `String.prototype`: there is nothing
    /// a Symbol wrapper has of its own, so §20.4.3 needs no exotic object to hold it.
    symbol_prototype: ObjectId,
    /// Which realm this is, as the machine's table numbers them — its own `[[Realm]]` handle.
    ///
    /// Carried by the realm so that everything built during `Realm::new` can stamp it onto the
    /// functions it makes without the id being threaded separately beside the realm it belongs to.
    id: crate::heap::RealmId,
    /// How many objects the heap held **before** this realm began building — see
    /// [`Realm::intrinsics`].
    first_intrinsic: usize,
    /// How many it held once the realm was built — the other end of the same range.
    intrinsics: usize,
    /// %ArrayBuffer.prototype% — §25.1.5.
    array_buffer_prototype: ObjectId,
    /// %ArrayBuffer% itself, which `slice`'s `SpeciesConstructor` falls back to.
    array_buffer_constructor: ObjectId,
    /// %Array.prototype.values% — §10.4.4.4 step 16's `@@iterator` for every arguments object.
    ///
    /// Held by identity rather than read off `Array.prototype` at each call, because those are
    /// different questions: the clause names the *intrinsic*, so replacing `Array.prototype.values`
    /// leaves `[...arguments]` walking the one this realm was built with.
    array_values: ObjectId,
    /// %eval% — §19.2.1, held because §13.3.6.1 identifies a **direct** eval by object identity.
    ///
    /// Comparing against whatever the global object currently says under `eval` would answer
    /// `globalThis.eval = f; eval(x)` wrongly: that is an ordinary call to `f`, not a direct eval.
    /// The same reason `%Promise%` is kept here rather than looked up when it is needed.
    eval_function: ObjectId,
    /// %DataView.prototype% — §25.3.4.
    data_view_prototype: ObjectId,
    /// %TypedArray.prototype% — §23.2.3, which every one of the nine inherits from.
    typed_array_prototype: ObjectId,
    /// The concrete TypedArray constructors, in [`crate::heap::KINDS`] order.
    ///
    /// §23.2.4.2 step 1 wants "the intrinsic object associated with the constructor name in
    /// `exemplar.[[TypedArrayName]]`" as `SpeciesConstructor`'s default, and *intrinsic* is the
    /// operative word: it is `%Int8Array%` itself, not whatever the array's `constructor` property
    /// says. Reading the property instead made `sample.constructor = {}` decide what `map` built
    /// with, so a species of `undefined` fell back to a plain object and the copy tried to
    /// construct it.
    typed_constructors: [ObjectId; crate::heap::KINDS.len()],
    /// §23.2.6's nine concrete prototypes — `%Int8Array.prototype%` and its eight siblings.
    ///
    /// Held by identity beside the constructors above and for the same reason: §23.2.5.1's default
    /// prototype is the **concrete** one named by the constructor, so `Reflect.construct(Float64Array,
    /// …, other)` falls back to that realm's `Float64Array.prototype` and not to its
    /// `%TypedArray.prototype%`. Reading it off the constructor at the time would work — §23.2.6
    /// makes it neither writable nor configurable — but it would be a property lookup standing in
    /// for an intrinsic, which is the mistake `array_values` above records.
    typed_prototypes: [ObjectId; crate::heap::KINDS.len()],
    /// %Map.prototype% — §24.1.3.
    map_prototype: ObjectId,
    /// %RegExp.prototype% — §22.2.6.
    regexp_prototype: ObjectId,
    /// %RegExp% itself, which §22.2.6.8's and §22.2.6.14's `SpeciesConstructor` fall back to.
    regexp_constructor: ObjectId,
    /// %RegExpStringIteratorPrototype% — §22.2.9.3.
    regexp_string_iterator_prototype: ObjectId,
    /// %Set.prototype% — §24.2.3.
    set_prototype: ObjectId,
    /// %WeakMap.prototype% — §24.3.3.
    weak_map_prototype: ObjectId,
    /// %WeakSet.prototype% — §24.4.3.
    weak_set_prototype: ObjectId,
    /// %WeakRef.prototype% — §26.1.3.
    weak_ref_prototype: ObjectId,
    /// %FinalizationRegistry.prototype% — §26.2.3.
    finalization_registry_prototype: ObjectId,
    /// %SharedArrayBuffer.prototype% — §25.2.4.
    shared_buffer_prototype: ObjectId,
    /// %SharedArrayBuffer% itself, which `slice`'s `SpeciesConstructor` falls back to.
    shared_buffer_constructor: ObjectId,
    /// %IteratorHelperPrototype% — §27.1.5.1, which inherits from %IteratorPrototype%.
    iterator_helper_prototype: ObjectId,
    /// %WrapForValidIteratorPrototype% — §27.1.3.2.1, what `Iterator.from` wraps with.
    wrap_iterator_prototype: ObjectId,
    /// %MapIteratorPrototype% — §24.1.5, which inherits from %IteratorPrototype% and so is handed
    /// `[@@iterator]` by it.
    map_iterator_prototype: ObjectId,
    /// %SetIteratorPrototype% — §24.2.5.
    set_iterator_prototype: ObjectId,
    /// %AggregateError.prototype% — §20.5.7.3.
    ///
    /// Its own field and not a seventh entry in [`NATIVE_ERRORS`], because that table exists for
    /// the errors that are *the same but for a name*: made together, installed together, thrown by
    /// the engine. §20.5.7's constructor takes its arguments in a different order and gives its
    /// instances a property none of the others has, so it shares nothing with them but a prototype.
    aggregate_error_prototype: ObjectId,
    /// %Promise.prototype% — §27.2.5.
    promise_prototype: ObjectId,
    /// %Promise% itself — §27.2.4.
    ///
    /// Kept because `SpeciesConstructor` needs it as the *default*: a promise whose `constructor`
    /// is gone falls back to this one, and a fallback that read the global would answer with
    /// whatever a program had since assigned to `Promise`.
    promise_constructor: ObjectId,
    /// %AsyncIteratorPrototype% — §27.1.3, whose one method answers the receiver.
    ///
    /// The async half of %IteratorPrototype%, and it exists for the same reason: an async iterator
    /// is async-iterable, which is what lets `for await` be given one directly.
    async_iterator_prototype: ObjectId,
    /// %AsyncFromSyncIteratorPrototype% — §27.1.4.2.
    ///
    /// Not reachable by any name at all. `for await` over something with only a `[@@iterator]`
    /// makes one of these to stand in front of it, and that wrapper is the only way a script ever
    /// meets this object.
    async_from_sync_iterator_prototype: ObjectId,
    /// %GeneratorFunction.prototype% — §27.3.3, the `[[Prototype]]` of every generator *function*.
    ///
    /// Not a constructor and not reachable by name: §27.3 makes `GeneratorFunction` itself
    /// unreachable except through `Object.getPrototypeOf(function* () {}).constructor`, so this is
    /// held by the realm rather than by a property on the global object.
    generator_function_prototype: ObjectId,
    /// %AsyncGeneratorPrototype% — §27.6.1, what an async generator *object* inherits from.
    async_generator_prototype: ObjectId,
    /// %AsyncGeneratorFunction.prototype% — §27.4.3, what an async generator *function* does.
    async_generator_function_prototype: ObjectId,
    /// %AsyncFunction.prototype% — §27.7.3, the `[[Prototype]]` of every `async function`.
    ///
    /// Not reachable by name any more than the two above: §27.7 makes `AsyncFunction` itself
    /// unreachable, so `Object.getPrototypeOf(async function () {})` is the only route to it.
    async_function_prototype: ObjectId,
    /// %GeneratorPrototype% — §27.5.1, what every generator *object* inherits from.
    ///
    /// Its `[[Prototype]]` is %IteratorPrototype%, which is what makes a generator iterable: the
    /// `[@@iterator]` it inherits answers the generator itself, so `for (const x of gen())` walks
    /// the generator rather than looking for something else.
    generator_prototype: ObjectId,
    /// %BigInt.prototype% — §21.2.3.
    bigint_prototype: ObjectId,
    /// %IteratorPrototype% — §27.1.2, where `[@@iterator]` answers the receiver.
    ///
    /// Every iterator in the language inherits from this, however it was made, which is what makes
    /// one iterable and so usable wherever an iterable is wanted.
    iterator_prototype: ObjectId,
    /// %ArrayIteratorPrototype% — §23.1.5.2.
    array_iterator_prototype: ObjectId,
    /// %StringIteratorPrototype% — §22.1.5.2.
    ///
    /// Separate from the Array one because a script may replace either `next` without touching
    /// the other, and because §22.1.5.2.2 tags them differently.
    string_iterator_prototype: ObjectId,
    /// %ThrowTypeError% — §10.2.4.1, the function that exists only to refuse.
    ///
    /// One per realm and shared by every use, which the specification is explicit about: the same
    /// function object poisons `callee` on every unmapped arguments object, so a script comparing
    /// two of them finds them equal.
    thrower: ObjectId,
    /// §20.5.5's six native error prototypes, in the order [`NATIVE_ERRORS`] names them.
    ///
    /// An array rather than six fields because nothing here treats one differently from another:
    /// they are made the same way, they differ only in a `name`, and the code that installs them
    /// wants to walk them. [`NativeError`] indexes into it for the three the engine itself
    /// throws; the other three exist because a *script* can reach them.
    native_error_prototypes: [ObjectId; NATIVE_ERRORS.len()],
}

/// §20.5.5's native error types, in the order their prototypes are stored.
///
/// Alphabetical, which is also the order §20.5.5 lists them in, so a reader checking this against
/// the specification is reading down one column rather than hunting.
const NATIVE_ERRORS: [&str; 6] = [
    "EvalError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "TypeError",
    "URIError",
];

/// Which of §20.5.5's error types an engine-raised failure is.
///
/// Defined in [`crate::value`] because it is a *description* of a failure rather than an
/// intrinsic: the value layer can say "this is a RangeError" without knowing what one looks like.
/// Re-exported here under the name the realm uses, because this is where a kind becomes an object.
pub use crate::value::ErrorKind as NativeError;

impl NativeError {
    /// Where this kind's prototype sits in [`NATIVE_ERRORS`].
    fn at(self) -> usize {
        match self {
            Self::Eval => 0,
            Self::Range => 1,
            Self::Reference => 2,
            Self::Syntax => 3,
            Self::Type => 4,
            Self::Uri => 5,
        }
    }
}

impl Realm {
    /// Build the intrinsics into `heap`.
    ///
    /// Order matters: every prototype has a prototype, and `Object.prototype` is where each chain
    /// ends. §20.5.5's error prototypes inherit from `Error.prototype`, which inherits from
    /// `Object.prototype` — so `e instanceof Error` will be true of a TypeError, once
    /// `instanceof` exists to ask.
    pub fn new(heap: &mut Heap, id: crate::heap::RealmId) -> Self {
        // Taken before the first allocation, so `intrinsics` is a range this realm owns rather than
        // a ceiling that swallows whatever came before it. Zero for the first realm — DR-0025.
        let first_intrinsic = heap.object_count();
        let object_prototype = heap.new_object(None);
        // §20.2.3 — every function inherits from this, and it is itself an ordinary object here.
        // It is callable in the specification, and callable with no arguments returning
        // `undefined`, which needs a native function and so waits for one.
        let function_prototype = heap.new_object(Some(object_prototype));
        // §9.3.4's global object, in the part that exists yet: an ordinary object with
        // `Object.prototype` behind it. It has no properties, because every property it should
        // have is a builtin. What it is *for* today is §10.2.1.2's substitution — a sloppy-mode
        // call with no receiver gets this rather than `undefined`.
        let global = heap.new_object(Some(object_prototype));
        let error_prototype = heap.new_object(Some(object_prototype));
        // §23.1.3 — `Array.prototype` is itself an Array, with a `length` of zero. Not a detail:
        // it is why `Array.prototype.length` is 0 rather than absent, and why `Array.isArray` of
        // it is true.
        let array_prototype = heap.new_array(object_prototype, 0);
        // §20.3.3 and §21.1.3 — these two are **not** ordinary objects: each is an instance of
        // its own kind, holding the primitive its methods then answer with. So
        // `Number.prototype.toString()` is `"0"` and `Boolean.prototype.valueOf()` is `false`,
        // where an ordinary object would make both a TypeError from `thisNumberValue`.
        //
        // Three of the five wrappers do this and two do not: §22.1.3 gives `String.prototype` the
        // empty String (it is made below, as a String exotic object), while §21.4.4 and §22.2.6
        // say in as many words that `Date.prototype` and `RegExp.prototype` are ordinary and are
        // *not* instances. The split is per-clause and there is no rule to derive it from.
        let boolean_prototype = heap.new_wrapper(object_prototype, Value::Boolean(false));
        let number_prototype = heap.new_wrapper(object_prototype, Value::Number(0.0));
        let date_prototype = heap.new_object(Some(object_prototype));
        let symbol_prototype = heap.new_object(Some(object_prototype));
        let aggregate_error_prototype = heap.new_object(Some(error_prototype));
        let promise_prototype = heap.new_object(Some(object_prototype));
        let iterator_prototype = heap.new_object(Some(object_prototype));
        let array_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let string_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let array_buffer_prototype = heap.new_object(Some(object_prototype));
        let data_view_prototype = heap.new_object(Some(object_prototype));
        let typed_array_prototype = heap.new_object(Some(object_prototype));
        let map_prototype = heap.new_object(Some(object_prototype));
        let regexp_prototype = heap.new_object(Some(object_prototype));
        let regexp_string_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let set_prototype = heap.new_object(Some(object_prototype));
        let weak_map_prototype = heap.new_object(Some(object_prototype));
        let weak_set_prototype = heap.new_object(Some(object_prototype));
        let weak_ref_prototype = heap.new_object(Some(object_prototype));
        let shared_buffer_prototype = heap.new_object(Some(object_prototype));
        let iterator_helper_prototype = heap.new_object(Some(iterator_prototype));
        let wrap_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let finalization_registry_prototype = heap.new_object(Some(object_prototype));
        let map_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let set_iterator_prototype = heap.new_object(Some(iterator_prototype));
        // §27.1.3 — the async iterators' shared prototype, and §27.1.4.2's wrapper inherits from
        // it exactly as a real async iterator does: the wrapper *is* one.
        let async_iterator_prototype = heap.new_object(Some(object_prototype));
        let async_from_sync_iterator_prototype = heap.new_object(Some(async_iterator_prototype));
        // §27.5.1 — a generator object inherits from %GeneratorPrototype%, which inherits from
        // %IteratorPrototype%. That second link is the whole of what makes a generator iterable.
        let generator_prototype = heap.new_object(Some(iterator_prototype));
        let bigint_prototype = heap.new_object(Some(object_prototype));
        // §27.3.3 — and a generator *function* inherits from this, which is an ordinary object
        // whose `[[Prototype]]` is %Function.prototype%: a generator function is still a function.
        let generator_function_prototype = heap.new_object(Some(function_prototype));
        // §27.6.1 — the async pair of the two above, and the link that differs is the interesting
        // one: an async generator object inherits from %AsyncIteratorPrototype%, not from
        // %IteratorPrototype%, which is what puts `Symbol.asyncIterator` rather than
        // `Symbol.iterator` on it and so what makes it a `for await` target and not a `for`-`of`
        // one.
        let async_generator_prototype = heap.new_object(Some(async_iterator_prototype));
        let async_generator_function_prototype = heap.new_object(Some(function_prototype));
        // §27.7.3 — and the plain async function's, which is the fourth of the four and was the
        // one ViperJS did not have. An ordinary object whose `[[Prototype]]` is %Function.prototype%,
        // exactly as the three above are: an `async function` is still a function.
        let async_function_prototype = heap.new_object(Some(function_prototype));
        // §10.2.4.1 %ThrowTypeError% — a function whose whole behaviour is to refuse, made here
        // rather than in a builtin module because it is not reachable by name from any script:
        // its only appearances are as an accessor pair the specification puts in place.
        let thrower = heap.new_native_function(function_prototype, refuse, id);
        // §10.2.4.1's own shape, and it is stricter than any other built-in's. `length` is 0 and
        // `name` is the **empty string** — not `"ThrowTypeError"`, which is the specification's
        // name for it and not a name any program may read — and both are non-writable *and*
        // non-configurable, where §17 makes every other built-in's configurable. Then the object
        // is sealed shut: `Object.isFrozen` of it is true, which is what stops a script attaching
        // anything to the one function every restricted property in the realm shares.
        crate::builtins::define_fixed(heap, thrower, "length", Value::Number(0.0));
        let empty_name = heap.intern(&[]);
        crate::builtins::define_fixed(heap, thrower, "name", Value::String(empty_name));
        if let Some(object) = heap.object_mut(thrower) {
            object.prevent_extensions();
        }
        // §6.1.5.1 — each is described as `Symbol.iterator` and so on, which is what makes
        // `String(Symbol.iterator)` answer `"Symbol(Symbol.iterator)"`. They live on the heap and
        // are built by whichever realm is built first, because the clause says they are shared by
        // all realms and a second set would be a second `Symbol.iterator` — DR-0025.
        heap.build_well_known(|heap| {
            crate::builtins::WELL_KNOWN
                .iter()
                .map(|name| {
                    let units: Vec<u16> = format!("Symbol.{name}").encode_utf16().collect();
                    let description = heap.intern(&units);
                    heap.new_symbol(Some(description))
                })
                .collect()
        });
        let empty = heap.intern(&[]);
        let string_prototype = heap.new_string_object(object_prototype, empty);
        // §20.5.3 — `Error.prototype` has a `name` of `"Error"` and an empty `message`, and both
        // are ordinary writable properties rather than anything special. That an error's message
        // usually comes from the *instance* and its name from the *prototype* is why
        // `e.message` is `""` for `new Error()` and `e.name` is never absent.
        let name = text(heap, "Error");
        define(heap, error_prototype, "name", name);
        let empty = text(heap, "");
        define(heap, error_prototype, "message", empty);

        // §20.5.6.3 — each native error's prototype inherits from `Error.prototype` and differs
        // from it in one property: its `name`. The `message` it does *not* override, which is why
        // `new TypeError().message` is the empty string that `Error.prototype` carries.
        let native_error_prototypes = NATIVE_ERRORS.map(|kind| {
            let prototype = heap.new_object(Some(error_prototype));
            let name = text(heap, kind);
            define(heap, prototype, "name", name);
            prototype
        });

        // §19.1's value properties of the global object — the four that are values rather than
        // functions, and so the four that can exist before anything is callable.
        //
        // §19.1.1's `globalThis` is an ordinary property: writable, not enumerable, configurable.
        // The other three are §19.1.2–4 and are none of those things. That `undefined` is a
        // read-only *property* rather than a keyword is why `var undefined = 1` is legal and does
        // nothing, and why a minifier can shorten it to `void 0` but not redefine it.
        define(heap, global, "globalThis", Value::Object(global));
        for (name, value) in [
            ("undefined", Value::Undefined),
            ("NaN", Value::Number(f64::NAN)),
            ("Infinity", Value::Number(f64::INFINITY)),
        ] {
            let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
            let descriptor = PropertyDescriptor {
                value: Some(value),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            };
            let _ = heap.define_own_property(global, key, &descriptor);
        }

        let mut realm = Self {
            object_prototype,
            global,
            function_prototype,
            error_prototype,
            array_prototype,
            boolean_prototype,
            number_prototype,
            date_prototype,
            string_prototype,
            symbol_prototype,
            id,
            first_intrinsic,
            intrinsics: 0,
            array_buffer_prototype,
            // Replaced by `builtins::buffer::install`, which is where the constructor is made.
            array_buffer_constructor: array_buffer_prototype,
            // Discovered below, once `array_methods::install` has made it. A callable placeholder
            // for the reason `promise_constructor` has one: a realm still being built has no
            // readers, and an `Option` would put a question at every use.
            array_values: function_prototype,
            // Replaced below, once `builtins::global::install` has made it.
            eval_function: global,
            data_view_prototype,
            typed_array_prototype,
            // Replaced below, once `builtins::typed::install` has made them.
            typed_constructors: [typed_array_prototype; crate::heap::KINDS.len()],
            typed_prototypes: [typed_array_prototype; crate::heap::KINDS.len()],
            map_prototype,
            regexp_prototype,
            // Replaced below, once the built-ins have made the real one. A prototype rather than a
            // placeholder of some other kind because every field here must name a live object.
            regexp_constructor: regexp_prototype,
            async_iterator_prototype,
            async_from_sync_iterator_prototype,
            generator_prototype,
            bigint_prototype,
            generator_function_prototype,
            async_function_prototype,
            async_generator_prototype,
            async_generator_function_prototype,
            regexp_string_iterator_prototype,
            set_prototype,
            weak_map_prototype,
            weak_set_prototype,
            weak_ref_prototype,
            shared_buffer_prototype,
            // Replaced below once the built-ins have made the real one, exactly as %RegExp% is.
            shared_buffer_constructor: shared_buffer_prototype,
            iterator_helper_prototype,
            wrap_iterator_prototype,
            finalization_registry_prototype,
            map_iterator_prototype,
            set_iterator_prototype,
            aggregate_error_prototype,
            promise_prototype,
            // Replaced by `builtins::promise::install`, which is where the constructor is made. A
            // placeholder rather than an `Option`, because every reader wants the real one and a
            // realm that has not finished being built has no readers.
            promise_constructor: promise_prototype,
            iterator_prototype,
            array_iterator_prototype,
            string_iterator_prototype,
            thrower,
            native_error_prototypes,
        };
        // The intrinsics are what a realm *is*, and §19 through §28 are intrinsics. Building them
        // here rather than at the first call is what makes `typeof Error` answer `"function"` in
        // a script that never mentions it.
        crate::builtins::install(heap, &realm);
        // §27.2.4.7's fallback is *the* `%Promise%`, not whatever a program later assigns to the
        // global `Promise`. It cannot be made before the built-ins run and the built-ins are handed
        // a finished realm, so it is filled in here — the one intrinsic that is discovered rather
        // than allocated.
        if let Some(found) = crate::builtins::promise::constructor_of(heap, &realm) {
            realm.set_promise_constructor(found);
        }
        if let Some(found) = crate::builtins::global_object(heap, &realm, "ArrayBuffer") {
            realm.array_buffer_constructor = found;
        }
        // §22.2.6.8 and §22.2.6.14 both fall back to *the* `%RegExp%`, and both hand it to
        // `Construct` — so this has to be the constructor and not the prototype, which is a
        // distinction nothing but a call would notice.
        if let Some(found) = crate::builtins::global_object(heap, &realm, "RegExp") {
            realm.regexp_constructor = found;
        }
        // §25.2.5.4 step 10's default. `SharedArrayBuffer` is a global here, so this is the same
        // discovery the two above are — and the same reason it cannot be allocated up there.
        if let Some(found) = crate::builtins::global_object(heap, &realm, "SharedArrayBuffer") {
            realm.shared_buffer_constructor = found;
        }
        // The same discovery for the nine, and for the same reason: they are made by the
        // built-ins, which are handed a finished realm. Taken from the global *now*, before a
        // script can run and reassign one — which is precisely the difference between an intrinsic
        // and a property, and the whole point of holding them here.
        // §13.3.6.1 needs this by identity, and it is discovered rather than allocated for the
        // same reason `%ArrayBuffer%` is: the built-ins are handed a finished realm.
        if let Some(found) = crate::builtins::global_object(heap, &realm, "eval") {
            realm.eval_function = found;
        }
        // §23.1.3.36 — `%Array.prototype.values%`, which §10.4.4.4 step 16 and §10.4.4.6 step 7
        // both give an arguments object under `%Symbol.iterator%`. Discovered here for the reason
        // `%ArrayBuffer%` is, and *taken now* for a second one: a script may replace
        // `Array.prototype.values`, and `[...arguments]` must go on walking the intrinsic.
        if let Some(crate::value::Value::Object(found)) =
            crate::builtins::own_value(heap, realm.array_prototype, "values")
        {
            realm.array_values = found;
        }
        for (at, (name, _, _)) in crate::heap::KINDS.into_iter().enumerate() {
            if let Some(found) = crate::builtins::global_object(heap, &realm, name) {
                realm.typed_constructors[at] = found;
                if let Some(crate::value::Value::Object(prototype)) =
                    crate::builtins::own_value(heap, found, "prototype")
                {
                    realm.typed_prototypes[at] = prototype;
                }
            }
        }
        // Last, so that everything above it is inside the ceiling — see `Realm::intrinsics`.
        realm.seal(heap);
        realm
    }

    /// §20.5.5's native error prototypes, each with the name its `name` property carries.
    ///
    /// The pairing is what the built-ins need: a constructor is made per prototype, and the name
    /// is both the constructor's `name` and the global it is installed under.
    pub fn native_error_prototypes(&self) -> impl Iterator<Item = (&'static str, ObjectId)> + '_ {
        NATIVE_ERRORS.into_iter().zip(self.native_error_prototypes)
    }

    /// §20.5.5's prototype for the native error *named* `name`, or `None` for anything else.
    ///
    /// By name because that is what a built-in has: the six share one Rust body, so the only thing
    /// telling `TypeError` from `URIError` at call time is the `name` §10.3.3 put on the function
    /// object — the same trick §23.2's nine TypedArray constructors use.
    #[must_use]
    pub fn native_error_prototype(&self, name: &str) -> Option<ObjectId> {
        let at = NATIVE_ERRORS.iter().position(|known| *known == name)?;
        self.native_error_prototypes.get(at).copied()
    }

    /// `%Object.prototype%` — what an object literal inherits from.
    pub fn object_prototype(&self) -> ObjectId {
        self.object_prototype
    }

    /// The global object — `globalThis`.
    ///
    /// Bare so far. A script's `this` is this object (§16.1.7), and so is the `this` of a
    /// sloppy-mode function called with no receiver. What it does *not* do yet is hold the
    /// script's `var` declarations, which §9.3's Global Environment Record puts here — those live
    /// in the script's declarative environment, and moving them is the slice that gives an
    /// undeclared name a ReferenceError instead of a refusal to compile.
    pub fn global(&self) -> ObjectId {
        self.global
    }

    /// `%Function.prototype%` — what every function inherits from.
    pub fn function_prototype(&self) -> ObjectId {
        self.function_prototype
    }

    /// `%Error.prototype%`.
    pub fn error_prototype(&self) -> ObjectId {
        self.error_prototype
    }

    /// `%Array.prototype%` — itself an Array, per §23.1.3.
    pub fn array_prototype(&self) -> ObjectId {
        self.array_prototype
    }

    /// `%Boolean.prototype%`.
    pub fn boolean_prototype(&self) -> ObjectId {
        self.boolean_prototype
    }

    /// `%Number.prototype%`.
    pub fn number_prototype(&self) -> ObjectId {
        self.number_prototype
    }

    /// `%Date.prototype%`.
    pub fn date_prototype(&self) -> ObjectId {
        self.date_prototype
    }

    /// What every String object and every String primitive finds its methods on.
    pub fn string_prototype(&self) -> ObjectId {
        self.string_prototype
    }

    /// What every Symbol finds its methods on.
    pub fn symbol_prototype(&self) -> ObjectId {
        self.symbol_prototype
    }

    /// %ThrowTypeError% — the function that throws whatever it is asked.
    pub fn thrower(&self) -> ObjectId {
        self.thrower
    }

    /// %ArrayBuffer.prototype% — §25.1.5.
    pub fn array_buffer_prototype(&self) -> ObjectId {
        self.array_buffer_prototype
    }

    /// %Array.prototype.values% — what an arguments object's `@@iterator` is.
    #[must_use]
    pub fn array_values(&self) -> ObjectId {
        self.array_values
    }

    /// %ArrayBuffer% — the default `slice`'s `SpeciesConstructor` falls back to.
    pub fn array_buffer_constructor(&self) -> ObjectId {
        self.array_buffer_constructor
    }

    /// %DataView.prototype% — §25.3.4.
    pub fn data_view_prototype(&self) -> ObjectId {
        self.data_view_prototype
    }

    /// Whether `callee` is §19.2.1's `eval` itself — §13.3.6.1's test for a **direct** eval.
    ///
    /// Identity and not a name, which is the whole of what that clause asks: the call is direct
    /// when the thing being called *is* this function, however it was spelled and whatever the
    /// global object currently says under `eval`. So `var eval = f; eval(x)` is an ordinary call,
    /// and `globalThis.eval = f; eval(x)` is one too.
    #[must_use]
    pub fn is_eval(&self, callee: crate::value::Value) -> bool {
        matches!(callee, crate::value::Value::Object(id) if id == self.eval_function)
    }

    /// %TypedArray.prototype% — §23.2.3.
    pub fn typed_array_prototype(&self) -> ObjectId {
        self.typed_array_prototype
    }

    /// The intrinsic constructor for one of §23.2's eleven kinds — §23.2.4.2 step 1's default.
    ///
    /// Named by the pair that identifies a kind rather than by a string: `Uint8Array` and
    /// `Uint8ClampedArray` read the same eight bits and are told apart only by the clamping, so an
    /// `Element` alone would answer the wrong one of the two for half the programs that ask.
    ///
    /// `None` for a pair [`crate::heap::KINDS`] does not list, which no view can hold — a view's
    /// element came from that table in the first place.
    /// §23.2.6's prototype for one of §23.2's eleven kinds, by identity.
    ///
    /// Named by the same pair `typed_constructor` below is, and for the same reason. `None` for a
    /// pair [`crate::heap::KINDS`] does not list.
    #[must_use]
    pub fn typed_prototype(
        &self,
        element: crate::heap::Element,
        clamped: bool,
    ) -> Option<ObjectId> {
        let at = crate::heap::KINDS
            .into_iter()
            .position(|(_, known, known_clamped)| known == element && known_clamped == clamped)?;
        self.typed_prototypes.get(at).copied()
    }

    /// The intrinsic constructor for one of §23.2's eleven kinds — §23.2.4.2 step 1's default.
    ///
    /// Named by the pair that identifies a kind rather than by a string: `Uint8Array` and
    /// `Uint8ClampedArray` read the same eight bits and are told apart only by the clamping, so an
    /// `Element` alone would answer the wrong one of the two for half the programs that ask.
    ///
    /// `None` for a pair [`crate::heap::KINDS`] does not list, which no view can hold — a view's
    /// element came from that table in the first place.
    pub fn typed_constructor(
        &self,
        element: crate::heap::Element,
        clamped: bool,
    ) -> Option<ObjectId> {
        let at = crate::heap::KINDS
            .into_iter()
            .position(|(_, known, known_clamped)| known == element && known_clamped == clamped)?;
        self.typed_constructors.get(at).copied()
    }

    /// %RegExp.prototype% — §22.2.6.
    pub fn regexp_prototype(&self) -> ObjectId {
        self.regexp_prototype
    }

    /// %RegExp% — the default `SpeciesConstructor` falls back to in §22.2.6.8 and §22.2.6.14.
    ///
    /// The constructor, not the prototype: both clauses `Construct` what this answers, so the two
    /// differ by a TypeError rather than by anything subtle.
    pub fn regexp_constructor(&self) -> ObjectId {
        self.regexp_constructor
    }

    /// §22.2.9.3 — `%RegExpStringIteratorPrototype%`, which inherits from `%IteratorPrototype%`
    /// and so is iterable itself.
    pub fn regexp_string_iterator_prototype(&self) -> ObjectId {
        self.regexp_string_iterator_prototype
    }

    /// §22.2.5 — `%RegExp.prototype%`, an ordinary object and *not* a regular expression itself.
    pub fn map_prototype(&self) -> ObjectId {
        self.map_prototype
    }

    /// %WeakMap.prototype% — §24.3.3.
    #[must_use]
    pub fn weak_map_prototype(&self) -> ObjectId {
        self.weak_map_prototype
    }

    /// %WeakSet.prototype% — §24.4.3.
    #[must_use]
    pub fn weak_set_prototype(&self) -> ObjectId {
        self.weak_set_prototype
    }

    /// %WeakRef.prototype% — §26.1.3.
    #[must_use]
    pub fn weak_ref_prototype(&self) -> ObjectId {
        self.weak_ref_prototype
    }

    /// %SharedArrayBuffer% — §25.2.5.4 step 10's default for `SpeciesConstructor`.
    #[must_use]
    pub fn shared_buffer_constructor(&self) -> ObjectId {
        self.shared_buffer_constructor
    }

    /// %SharedArrayBuffer.prototype% — §25.2.4.
    #[must_use]
    pub fn shared_buffer_prototype(&self) -> ObjectId {
        self.shared_buffer_prototype
    }

    /// %IteratorHelperPrototype% — §27.1.5.1.
    #[must_use]
    pub fn iterator_helper_prototype(&self) -> ObjectId {
        self.iterator_helper_prototype
    }

    /// %WrapForValidIteratorPrototype% — §27.1.3.2.1.
    #[must_use]
    pub fn wrap_iterator_prototype(&self) -> ObjectId {
        self.wrap_iterator_prototype
    }

    /// %FinalizationRegistry.prototype% — §26.2.3.
    #[must_use]
    pub fn finalization_registry_prototype(&self) -> ObjectId {
        self.finalization_registry_prototype
    }

    /// %Set.prototype% — §24.2.3.
    pub fn set_prototype(&self) -> ObjectId {
        self.set_prototype
    }

    /// %MapIteratorPrototype% — §24.1.5.
    pub fn map_iterator_prototype(&self) -> ObjectId {
        self.map_iterator_prototype
    }

    /// %SetIteratorPrototype% — §24.2.5.
    pub fn set_iterator_prototype(&self) -> ObjectId {
        self.set_iterator_prototype
    }

    /// %AggregateError.prototype% — §20.5.7.3.
    pub fn aggregate_error_prototype(&self) -> ObjectId {
        self.aggregate_error_prototype
    }

    /// %Promise.prototype% — §27.2.5.
    pub fn promise_prototype(&self) -> ObjectId {
        self.promise_prototype
    }

    /// %Promise% — the default `SpeciesConstructor` falls back to.
    pub fn promise_constructor(&self) -> ObjectId {
        self.promise_constructor
    }

    /// Record the constructor `builtins::promise::install` made.
    pub(crate) fn set_promise_constructor(&mut self, constructor: ObjectId) {
        self.promise_constructor = constructor;
    }

    /// %AsyncFunction.prototype% — §27.7.3, the `[[Prototype]]` of every `async function`.
    #[must_use]
    pub fn async_function_prototype(&self) -> ObjectId {
        self.async_function_prototype
    }

    /// %IteratorPrototype% — what every iterator in the realm inherits from.
    pub fn iterator_prototype(&self) -> ObjectId {
        self.iterator_prototype
    }

    /// %AsyncIteratorPrototype% — §27.1.3.
    pub fn async_iterator_prototype(&self) -> ObjectId {
        self.async_iterator_prototype
    }

    /// %AsyncFromSyncIteratorPrototype% — §27.1.4.2.
    pub fn async_from_sync_iterator_prototype(&self) -> ObjectId {
        self.async_from_sync_iterator_prototype
    }

    /// %BigInt.prototype% — §21.2.3, what a BigInt wrapper inherits from.
    pub fn bigint_prototype(&self) -> ObjectId {
        self.bigint_prototype
    }

    /// %GeneratorPrototype% — §27.5.1, what a generator object inherits from.
    pub fn generator_prototype(&self) -> ObjectId {
        self.generator_prototype
    }

    /// %AsyncGeneratorPrototype% — §27.6.1, what an async generator object inherits from.
    pub fn async_generator_prototype(&self) -> ObjectId {
        self.async_generator_prototype
    }

    /// %AsyncGeneratorFunction.prototype% — §27.4.3.
    pub fn async_generator_function_prototype(&self) -> ObjectId {
        self.async_generator_function_prototype
    }

    /// %GeneratorFunction.prototype% — §27.3.3, what a generator function inherits from.
    pub fn generator_function_prototype(&self) -> ObjectId {
        self.generator_function_prototype
    }

    /// %ArrayIteratorPrototype% — §23.1.5.2.
    pub fn array_iterator_prototype(&self) -> ObjectId {
        self.array_iterator_prototype
    }

    /// %StringIteratorPrototype% — §22.1.5.2.
    pub fn string_iterator_prototype(&self) -> ObjectId {
        self.string_iterator_prototype
    }

    /// Which realm this is — what a function created here records as its `[[Realm]]`.
    #[must_use]
    pub fn id(&self) -> crate::heap::RealmId {
        self.id
    }

    /// Every object this realm built, as roots for the collector.
    ///
    /// A *range* rather than a list of the forty-odd intrinsic fields, and deliberately: a list
    /// written out by hand is one an intrinsic added later is left out of, and being left out of
    /// the root set does not fail to compile — it frees `%GeneratorPrototype%` while a generator
    /// is still inheriting from it. Nothing this realm built is outside the range, because both
    /// ends are taken by `Realm::new` around the building.
    ///
    /// It over-approximates by whatever `Realm::new` allocated and threw away, which is a handful
    /// of objects that will never be collected. That is the price, and it is the right way round:
    /// the alternative fails by freeing something live.
    ///
    /// **A range and not a ceiling, which matters as soon as there are two realms.** A ceiling
    /// taken by the second realm counts everything the first one made *and everything the program
    /// allocated in between*, so DR-0023's collector would stay sound and go blind — a leak the
    /// size of whatever ran before `create_realm`. The floor is `0` for the first realm, which is
    /// why this is the same set it always was. DR-0025.
    pub fn intrinsics(&self) -> impl Iterator<Item = crate::heap::ObjectId> {
        (self.first_intrinsic..self.intrinsics).map(crate::heap::ObjectId)
    }

    /// Record the far end of that range, once everything the realm builds is built.
    fn seal(&mut self, heap: &Heap) {
        self.intrinsics = heap.object_count();
    }

    /// Give a function the `prototype` object its instances will inherit from — §10.2.5.
    ///
    /// The pair is mutual: the function's `prototype` points at the object, and the object's
    /// `constructor` points back at the function. That is what makes `new F().constructor === F`
    /// true, and it is why neither can be made lazily without the other noticing.
    ///
    /// The attributes are §10.2.5's and they are not the ones assignment gives. `prototype` is
    /// writable and *not* configurable — a script may replace it and may not delete it — while
    /// `constructor` is writable and configurable and hidden from enumeration, so `for...in` over
    /// an instance finds nothing.
    pub fn make_constructor(&self, heap: &mut Heap, function: ObjectId) {
        let prototype = heap.new_object(Some(self.object_prototype));
        let constructor =
            PropertyKey::from_units(heap, &"constructor".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Object(function)),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(prototype, constructor, &descriptor);

        let key = PropertyKey::from_units(heap, &"prototype".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Object(prototype)),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(function, key, &descriptor);
    }

    /// §15.5.4's `prototype` for a generator function, which is not `MakeConstructor`'s.
    ///
    /// Two differences and both matter. The object inherits from %GeneratorPrototype% rather than
    /// from %Object.prototype%, which is what puts `next` on every generator this function makes.
    /// And there is **no `constructor` back-pointer**: a generator function is not a constructor,
    /// so a property saying it was would be a lie a script can read.
    pub fn make_generator_function(&self, heap: &mut Heap, function: ObjectId, asynchronous: bool) {
        // §27.6.2 does the same with %AsyncGeneratorPrototype%, and getting this wrong is not
        // subtle: the object every instance inherits from is where `next` comes from, so an async
        // generator built on the synchronous prototype answers its own `next` with §27.5.1's,
        // which then refuses the receiver for not being a generator.
        let inherits = match asynchronous {
            true => self.async_generator_prototype,
            false => self.generator_prototype,
        };
        let prototype = heap.new_object(Some(inherits));
        let key = PropertyKey::from_units(heap, &"prototype".encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(Value::Object(prototype)),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        let _ = heap.define_own_property(function, key, &descriptor);
    }

    /// A new error object of this kind, carrying `message`.
    ///
    /// §20.5.1.1 in the part that is not about `new`: an ordinary object with the right prototype
    /// and an own `message`. The message is an *own* property because it belongs to this error,
    /// while `name` stays on the prototype because it belongs to the kind.
    ///
    /// An empty message leaves the property off entirely, which is what §20.5.1.1 step 4 says —
    /// `new TypeError()` has no own `message` and inherits the empty one.
    pub fn error(&self, heap: &mut Heap, kind: NativeError, message: &str) -> Value {
        let prototype = self.native_error_prototypes[kind.at()];
        let object = heap.new_object(Some(prototype));
        // The same `[[ErrorData]]` §20.5.1.1 gives one a script constructs. An engine's own throw
        // is not a lesser error: `Object.prototype.toString.call(caught)` says `[object Error]`
        // whichever side made it, and a program cannot tell them apart by anything else either.
        if let Some(found) = heap.object_mut(object) {
            found.make_error();
        }
        if !message.is_empty() {
            let message = text(heap, message);
            define(heap, object, "message", message);
        }
        Value::Object(object)
    }
}

/// A String on the heap, as a value.
fn text(heap: &mut Heap, contents: &str) -> Value {
    Value::String(heap.new_string(contents.encode_utf16().collect()))
}

/// Give `object` an ordinary writable, non-enumerable, configurable property.
///
/// The attributes every built-in property has, and they are not the ones assignment produces:
/// §17's convention is that a built-in is invisible to `for...in`, which is why enumerating an
/// error does not list its `name`.
fn define(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
    let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
    let descriptor = PropertyDescriptor {
        value: Some(value),
        writable: Some(true),
        enumerable: Some(false),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    // The object was made here and is extensible with nothing in the way, so the rules cannot
    // refuse this. Ignoring the answer rather than asserting on it keeps the constructor total.
    let _ = heap.define_own_property(object, key, &descriptor);
}

/// %ThrowTypeError% (§10.2.4.1) — throws whatever it is asked, and answers nothing.
///
/// The specification's own name for it is a percent-delimited intrinsic, because no script can
/// name it: it appears only where a clause puts it, as the getter and setter of a property that
/// exists to be unreadable. `arguments.callee` on a function with a default parameter is the one
/// place ViperJS reaches for it so far.
fn refuse(
    _vm: &mut crate::vm::Vm,
    _heap: &mut Heap,
    _call: &crate::heap::NativeCall<'_>,
) -> crate::value::Completion<crate::value::Value> {
    Err(crate::value::Abrupt::type_error(
        "this property may not be read or written",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::PropertyKind;

    fn property(heap: &Heap, object: ObjectId, name: &str) -> Option<Value> {
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

    fn text_of(heap: &Heap, value: Value) -> String {
        match value {
            Value::String(id) => String::from_utf16_lossy(heap.string(id).unwrap_or(&[])),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn a_constructors_pair_of_properties_have_the_attributes_ten_two_five_gives_them() {
        // §10.2.5's `prototype` and `constructor`, and their attributes are not the same as each
        // other's — which is a distinction nothing in the language can see yet, because seeing it
        // needs `for...in` or `getOwnPropertyDescriptor`.
        //
        // `prototype` is writable and **not configurable**: a script may replace it and may not
        // delete it. `constructor` is writable *and* configurable. Both are hidden from
        // enumeration, which is why `for (var k in new F())` finds nothing.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));
        let environment = heap.new_environment(None, 0);
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let function = heap.new_function(
            realm.function_prototype(),
            body,
            environment,
            None,
            realm.id(),
        );
        realm.make_constructor(&mut heap, function);

        let on_the_function = attributes(&heap, function, "prototype").expect("made"); // the test is about it
        assert_eq!(on_the_function, (true, false, false));

        let Some(Value::Object(prototype)) = property(&heap, function, "prototype") else {
            panic!("the prototype is an object")
        };
        let on_the_prototype = attributes(&heap, prototype, "constructor").expect("made"); // same
        assert_eq!(on_the_prototype, (true, false, true));

        // …and the pair points both ways, which is what makes `new F().constructor === F` true.
        assert!(matches!(
            property(&heap, prototype, "constructor"),
            Some(Value::Object(back)) if back == function
        ));
    }

    /// The `(writable, enumerable, configurable)` of an own data property.
    fn attributes(heap: &Heap, object: ObjectId, name: &str) -> Option<(bool, bool, bool)> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let key = heap
            .object(object)?
            .own_property_keys(heap)
            .into_iter()
            .find(|key| key.as_string().and_then(|id| heap.string(id)) == Some(&units[..]))?;
        let found = heap.object(object)?.get_own_property(key)?;
        let PropertyKind::Data { writable, .. } = found.kind else {
            return None;
        };
        Some((writable, found.enumerable, found.configurable))
    }

    #[test]
    fn every_prototype_chain_ends_at_object_prototype() {
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));
        // §20.5.5 — a native error's prototype inherits from `Error.prototype`, which inherits
        // from `Object.prototype`. Three links, and the last one ends: `Object.prototype` has a
        // null prototype, which is where every chain in the language stops.
        let Value::Object(error) = realm.error(&mut heap, NativeError::Type, "") else {
            panic!("an error is an object")
        };
        let type_error_prototype = realm.native_error_prototypes[NativeError::Type.at()];
        let prototype = heap.object(error).and_then(crate::heap::Object::prototype);
        assert_eq!(prototype, Some(type_error_prototype));
        let grandparent = heap
            .object(type_error_prototype)
            .and_then(crate::heap::Object::prototype);
        assert_eq!(grandparent, Some(realm.error_prototype()));
        let root = heap
            .object(realm.error_prototype())
            .and_then(crate::heap::Object::prototype);
        assert_eq!(root, Some(realm.object_prototype()));
        assert_eq!(
            heap.object(realm.object_prototype())
                .and_then(crate::heap::Object::prototype),
            None
        );
    }

    #[test]
    fn the_name_comes_from_the_kind_and_the_message_from_the_error() {
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));
        let Value::Object(error) = realm.error(&mut heap, NativeError::Range, "out of range")
        else {
            panic!("an error is an object")
        };
        // The message is the error's own, because it belongs to this one…
        assert_eq!(
            text_of(&heap, property(&heap, error, "message").expect("a message")),
            "out of range"
        ); // the test is about it
        // …and the name is not, because it belongs to the kind. Every RangeError shares it.
        assert!(property(&heap, error, "name").is_none());
        let key = PropertyKey::from_units(&mut heap, &"name".encode_utf16().collect::<Vec<_>>());
        let (_, inherited) = heap
            .find_own(error, key)
            .expect("inherited from the prototype"); // same
        let PropertyKind::Data { value, .. } = inherited.kind else {
            panic!("a data property")
        };
        assert_eq!(text_of(&heap, value), "RangeError");
    }

    #[test]
    fn an_error_with_nothing_to_say_has_no_message_of_its_own() {
        // §20.5.1.1 step 4 — the property is only made when there is a message, so
        // `new TypeError()` inherits the empty one rather than owning it.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));
        let Value::Object(error) = realm.error(&mut heap, NativeError::Type, "") else {
            panic!("an error is an object")
        };
        assert!(property(&heap, error, "message").is_none());
        let key = PropertyKey::from_units(&mut heap, &"message".encode_utf16().collect::<Vec<_>>());
        let (owner, _) = heap.find_own(error, key).expect("inherited"); // the test is about it
        assert_eq!(owner, realm.error_prototype());
    }

    #[test]
    fn each_native_error_names_its_own_prototype_and_not_the_one_they_share() {
        // §20.5.6.2's intrinsic default is `%NativeError.prototype%` — `URIError.prototype` for a
        // `URIError`. The six share one Rust body, so this lookup is the only thing telling them
        // apart, and getting it wrong is invisible until a `new.target` whose `prototype` is not an
        // object makes the fallback run at all.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));

        let uri = realm
            .native_error_prototype("URIError")
            .expect("a realm has URIError"); // the test is which prototype
        let kind = realm
            .native_error_prototype("TypeError")
            .expect("a realm has TypeError"); // same
        assert_ne!(uri, kind);
        assert_eq!(uri, realm.native_error_prototypes[NativeError::Uri.at()]);
        assert_eq!(kind, realm.native_error_prototypes[NativeError::Type.at()]);

        // …and a name that is not one of the six is `None` rather than the nearest match, which is
        // what makes the caller fall back to `%Error.prototype%` instead of to a wrong sibling.
        assert!(realm.native_error_prototype("Error").is_none());
        assert!(realm.native_error_prototype("URIErro").is_none());
    }

    #[test]
    fn a_typed_arrays_prototype_is_named_by_its_element_and_its_clamping_together() {
        // `Uint8Array` and `Uint8ClampedArray` read the same eight bits, so the element alone
        // answers the wrong one of the two for half the programs that ask — which is why the lookup
        // is a conjunction and why this asserts the pair rather than either half.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));

        let plain = realm
            .typed_prototype(crate::heap::Element::Uint8, false)
            .expect("a realm has Uint8Array"); // the test is which prototype
        let clamped = realm
            .typed_prototype(crate::heap::Element::Uint8, true)
            .expect("a realm has Uint8ClampedArray"); // same
        let wider = realm
            .typed_prototype(crate::heap::Element::Int32, false)
            .expect("a realm has Int32Array"); // same
        assert_ne!(plain, clamped);
        assert_ne!(plain, wider);

        // Each is the object its own constructor carries, which is what makes it the intrinsic
        // rather than an object that merely looks like one.
        for (element, clamp) in [
            (crate::heap::Element::Uint8, false),
            (crate::heap::Element::Uint8, true),
            (crate::heap::Element::Int32, false),
        ] {
            let constructor = realm
                .typed_constructor(element, clamp)
                .expect("a realm has it"); // same
            let held = match property(&heap, constructor, "prototype") {
                Some(Value::Object(id)) => Some(id),
                _ => None,
            };
            assert_eq!(held, realm.typed_prototype(element, clamp));
        }
    }

    #[test]
    fn a_built_in_property_is_not_enumerable() {
        // §17's convention, and it is observable: enumerating an error does not list `name`, so
        // `for (var k in e)` over a fresh TypeError finds nothing at all.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap, crate::heap::RealmId(0));
        let prototype = realm.error_prototype();
        let keys = heap
            .object(prototype)
            .map_or_else(Vec::new, |found| found.own_property_keys(&heap));
        // §20.5.3's four: the `name` and `message` an error inherits, the `toString` that prints
        // it, and the `constructor` that points back at `Error`. Named rather than counted, so
        // that adding a fifth is a decision rather than a number going up.
        let names: Vec<String> = keys
            .iter()
            .map(|key| {
                String::from_utf16_lossy(
                    key.as_string()
                        .and_then(|id| heap.string(id))
                        .unwrap_or(&[]),
                )
            })
            .collect();
        assert_eq!(names, ["name", "message", "constructor", "toString"]);
        for key in keys {
            let property = heap
                .object(prototype)
                .and_then(|found| found.get_own_property(key))
                .copied()
                .expect("just listed"); // the test is about it
            assert!(!property.enumerable);
            // Writable and configurable, which is §17's other half: a built-in property is
            // hidden from enumeration and is *not* frozen. `Error.prototype.name = "Oops"` works,
            // and every engine lets it, which is why the two attributes differ from the one.
            assert!(property.configurable);
            assert!(matches!(
                property.kind,
                PropertyKind::Data { writable: true, .. }
            ));
        }
    }
}
