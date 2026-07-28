//! The objects that exist before any code runs — §9.3's realm, in the part that has one yet.
//!
//! A realm is the set of intrinsic objects a script can reach without making them: the prototypes
//! everything inherits from, the constructors, the global object. §9.3 lists about two hundred of
//! them; four are here, and they are the four an engine needs before it can *report* anything.
//!
//! # Why an error is an object at all
//!
//! Because `catch (e) { e.message }` has to work, and because `throw` takes a value rather than a
//! condition. [`crate::value::TypeError`] says *which* error and why; this decides what object
//! stands for it, and that decision needs a prototype, which needs a realm. Keeping the two apart
//! is what lets `value/` stay a description of values rather than of a running engine.
//!
//! # What is missing, and how you can tell
//!
//! `Error.prototype` has a `name` and a `message` and no methods, because a method is a function
//! and there are none. So `String(e)` on an error does not yet say `"TypeError: …"` — it throws,
//! since `ToPrimitive` finds no `toString` to call. That is the correct answer for the object as
//! it stands, and it changes on its own when `Object.prototype.toString` arrives.

use crate::heap::{Heap, ObjectId, PropertyDescriptor, PropertyKey};
use crate::value::Value;

/// The intrinsic objects, and the prototypes everything else is given.
#[derive(Debug, Clone, Copy)]
pub struct Realm {
    object_prototype: ObjectId,
    function_prototype: ObjectId,
    error_prototype: ObjectId,
    type_error_prototype: ObjectId,
    range_error_prototype: ObjectId,
    reference_error_prototype: ObjectId,
}

/// Which of §20.5.5's five native error types this is.
///
/// `Error` itself is not among them: it is the one a *program* throws, and an engine throws one
/// of these. `EvalError` and `URIError` join the list when the builtins that produce them do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeError {
    /// The operand is the wrong kind of thing — §20.5.5.5. By far the commonest.
    Type,
    /// A number is outside the interval something allows — §20.5.5.2.
    Range,
    /// A name has no binding, or is used before it has one — §20.5.5.3.
    Reference,
}

impl NativeError {
    /// The `name` property of the prototype, which is what `e.name` answers.
    fn name(self) -> &'static str {
        match self {
            Self::Type => "TypeError",
            Self::Range => "RangeError",
            Self::Reference => "ReferenceError",
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
        let error_prototype = heap.new_object(Some(object_prototype));
        // §20.5.3 — `Error.prototype` has a `name` of `"Error"` and an empty `message`, and both
        // are ordinary writable properties rather than anything special. That an error's message
        // usually comes from the *instance* and its name from the *prototype* is why
        // `e.message` is `""` for `new Error()` and `e.name` is never absent.
        let name = text(heap, "Error");
        define(heap, error_prototype, "name", name);
        let empty = text(heap, "");
        define(heap, error_prototype, "message", empty);

        let mut native = |kind: NativeError| {
            let prototype = heap.new_object(Some(error_prototype));
            let name = text(heap, kind.name());
            define(heap, prototype, "name", name);
            prototype
        };
        let type_error_prototype = native(NativeError::Type);
        let range_error_prototype = native(NativeError::Range);
        let reference_error_prototype = native(NativeError::Reference);

        Self {
            object_prototype,
            function_prototype,
            error_prototype,
            type_error_prototype,
            range_error_prototype,
            reference_error_prototype,
        }
    }

    /// `%Object.prototype%` — what an object literal inherits from.
    pub fn object_prototype(&self) -> ObjectId {
        self.object_prototype
    }

    /// `%Function.prototype%` — what every function inherits from.
    pub fn function_prototype(&self) -> ObjectId {
        self.function_prototype
    }

    /// `%Error.prototype%`.
    pub fn error_prototype(&self) -> ObjectId {
        self.error_prototype
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
        let prototype = match kind {
            NativeError::Type => self.type_error_prototype,
            NativeError::Range => self.range_error_prototype,
            NativeError::Reference => self.reference_error_prototype,
        };
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
            .find(|key| heap.string(key.as_string()) == Some(&units[..]))?;
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
    fn every_prototype_chain_ends_at_object_prototype() {
        let mut heap = Heap::new();
        let realm = Realm::new(&mut heap);
        // §20.5.5 — a native error's prototype inherits from `Error.prototype`, which inherits
        // from `Object.prototype`. Three links, and the last one ends: `Object.prototype` has a
        // null prototype, which is where every chain in the language stops.
        let Value::Object(error) = realm.error(&mut heap, NativeError::Type, "") else {
            panic!("an error is an object")
        };
        let prototype = heap.object(error).and_then(crate::heap::Object::prototype);
        assert_eq!(prototype, Some(realm.type_error_prototype));
        let grandparent = heap
            .object(realm.type_error_prototype)
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
        assert_eq!(keys.len(), 2);
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
