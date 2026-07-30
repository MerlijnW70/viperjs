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
//! # What is missing, and how you can tell
//!
//! §9.3 lists about two hundred intrinsics. `Object` and the error hierarchy are here; everything
//! else — `Array`, `String`, `Number`, `Function.prototype`'s methods — is not, and the
//! conformance expectations name every test that notices.

use crate::heap::{Heap, ObjectId, PropertyDescriptor, PropertyKey, SymbolId};
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
    /// §6.1.5.1's well-known Symbols, in the order [`crate::builtins::WELL_KNOWN`] names them.
    ///
    /// Held by the realm because the *engine* consults them by identity: `for`-`of` reaches for
    /// this `Symbol.iterator` and not for whatever a script has since put under that name. A
    /// property on the constructor would be the script's to move; this is not.
    well_known: [SymbolId; crate::builtins::WELL_KNOWN.len()],
    /// %Map.prototype% — §24.1.3.
    map_prototype: ObjectId,
    /// %Set.prototype% — §24.2.3.
    set_prototype: ObjectId,
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
    pub fn new(heap: &mut Heap) -> Self {
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
        let boolean_prototype = heap.new_object(Some(object_prototype));
        let number_prototype = heap.new_object(Some(object_prototype));
        let date_prototype = heap.new_object(Some(object_prototype));
        let symbol_prototype = heap.new_object(Some(object_prototype));
        let aggregate_error_prototype = heap.new_object(Some(error_prototype));
        let promise_prototype = heap.new_object(Some(object_prototype));
        let iterator_prototype = heap.new_object(Some(object_prototype));
        let array_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let string_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let map_prototype = heap.new_object(Some(object_prototype));
        let set_prototype = heap.new_object(Some(object_prototype));
        let map_iterator_prototype = heap.new_object(Some(iterator_prototype));
        let set_iterator_prototype = heap.new_object(Some(iterator_prototype));
        // §10.2.4.1 %ThrowTypeError% — a function whose whole behaviour is to refuse, made here
        // rather than in a builtin module because it is not reachable by name from any script:
        // its only appearances are as an accessor pair the specification puts in place.
        let thrower = heap.new_native_function(function_prototype, refuse);
        // §6.1.5.1 — each is described as `Symbol.iterator` and so on, which is what makes
        // `String(Symbol.iterator)` answer `"Symbol(Symbol.iterator)"`.
        let well_known = crate::builtins::WELL_KNOWN.map(|name| {
            let units: Vec<u16> = format!("Symbol.{name}").encode_utf16().collect();
            let description = heap.intern(&units);
            heap.new_symbol(Some(description))
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
            well_known,
            map_prototype,
            set_prototype,
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
        realm
    }

    /// §20.5.5's native error prototypes, each with the name its `name` property carries.
    ///
    /// The pairing is what the built-ins need: a constructor is made per prototype, and the name
    /// is both the constructor's `name` and the global it is installed under.
    pub fn native_error_prototypes(&self) -> impl Iterator<Item = (&'static str, ObjectId)> + '_ {
        NATIVE_ERRORS.into_iter().zip(self.native_error_prototypes)
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

    /// %Map.prototype% — §24.1.3.
    pub fn map_prototype(&self) -> ObjectId {
        self.map_prototype
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

    /// %IteratorPrototype% — what every iterator in the realm inherits from.
    pub fn iterator_prototype(&self) -> ObjectId {
        self.iterator_prototype
    }

    /// %ArrayIteratorPrototype% — §23.1.5.2.
    pub fn array_iterator_prototype(&self) -> ObjectId {
        self.array_iterator_prototype
    }

    /// %StringIteratorPrototype% — §22.1.5.2.
    pub fn string_iterator_prototype(&self) -> ObjectId {
        self.string_iterator_prototype
    }

    /// The well-known Symbol at this position in [`crate::builtins::WELL_KNOWN`].
    ///
    /// By index rather than by name because the engine's uses are compile-time constants and a
    /// name lookup would be a string comparison on a path that has none. The names those indices
    /// have are the `WELL_KNOWN` table in `crate::builtins`, and `well_known_at` beside it turns
    /// one into the other for the callers that have a name and not a position.
    pub fn well_known(&self, at: usize) -> Option<SymbolId> {
        self.well_known.get(at).copied()
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
/// place praxis reaches for it so far.
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
        let realm = Realm::new(&mut heap);
        let environment = heap.new_environment(None, 0);
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let function = heap.new_function(realm.function_prototype(), body, environment, None);
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
        let realm = Realm::new(&mut heap);
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
        let realm = Realm::new(&mut heap);
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
        let realm = Realm::new(&mut heap);
        let Value::Object(error) = realm.error(&mut heap, NativeError::Type, "") else {
            panic!("an error is an object")
        };
        assert!(property(&heap, error, "message").is_none());
        let key = PropertyKey::from_units(&mut heap, &"message".encode_utf16().collect::<Vec<_>>());
        let (owner, _) = heap.find_own(error, key).expect("inherited"); // the test is about it
        assert_eq!(owner, realm.error_prototype());
    }

    #[test]
    fn a_built_in_property_is_not_enumerable() {
        // §17's convention, and it is observable: enumerating an error does not list `name`, so
        // `for (var k in e)` over a fresh TypeError finds nothing at all.
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap);
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
