//! What an object files under a key, and what a key is.
//!
//! Three types, and the distinctions between them are the specification's rather than
//! convenience:
//!
//! - [`PropertyKey`] — what a property is filed under. §6.1.7: "a property key is either a String
//!   or a Symbol", and here it is a String, interned.
//! - [`Property`] — what an object *stores*. Either a data property or an accessor property,
//!   with all four of its attributes present. §6.1.7.1's table.
//! - [`PropertyDescriptor`] — what is *passed around*. A record with **zero or more** fields, so
//!   every field is optional, and "has a `[[Value]]` field" is a different question from "the
//!   `[[Value]]` field is `undefined`". §6.2.6.
//!
//! Keeping the last two apart is worth a word. A stored property that is half a data property and
//! half an accessor is not a thing the specification can describe, and a descriptor that is
//! neither is (`{enumerable: true}` alone is a generic descriptor, and `Object.defineProperty`
//! takes one). Written as one type with six `Option` fields, both states would be representable
//! and the ones that cannot happen would need runtime checks nothing could reach. Written as two,
//! the compiler keeps them apart and the conversion between them is [`Property::to_descriptor`]
//! and — arriving with the object — `ValidateAndApplyPropertyDescriptor`.

use crate::heap::{Heap, StringId, SymbolId};
use crate::value::{Value, canonical_numeric_index};

/// A property key — §6.1.7, "either a String or a Symbol".
///
/// # Why this is not a `StringId`
///
/// Because a key's identity is its *contents* and a `StringId`'s is not. `o.a = 1; o.a` makes two
/// Strings spelled `a`, with two handles, and a map keyed by the handle would file the write and
/// the read under different properties. So a key holds an **interned** handle, which
/// [`Heap::intern`] guarantees is the same for every String with the same code units — and that
/// guarantee is why the field is private and the constructors are the only way in.
///
/// # Why it is an enum
///
/// §6.1.7 says a key is a String **or a Symbol**, and the two behave differently at nearly every
/// turn: a Symbol key is invisible to `for`-`in`, to `Object.keys` and to
/// `getOwnPropertyNames`, it sorts after every String in `[[OwnPropertyKeys]]`, and it cannot be
/// spelled. Every caller that takes a key apart therefore has to say what it does about a Symbol,
/// which is what the enum is for — [`PropertyKey::as_string`] answers `None` rather than a String
/// that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    /// An interned String — the ordinary kind, and every key that can be written down.
    String(StringId),
    /// A Symbol, which is its own identity and equal to nothing else.
    Symbol(SymbolId),
}

impl PropertyKey {
    /// The key `units` spells.
    ///
    /// Interns, so two calls with equal code units answer equal keys. Every String is a valid
    /// key, the empty one included — §6.1.7 says so in as many words, and `o[""]` is a property
    /// like any other.
    pub fn from_units(heap: &mut Heap, units: &[u16]) -> Self {
        Self::String(heap.intern(units))
    }

    /// The key a Symbol is used as — the other half of `ToPropertyKey` (§7.1.19 step 3).
    ///
    /// No interning: a Symbol is already its own identity, which is the property that makes it
    /// usable as a key at all.
    pub fn from_symbol(id: SymbolId) -> Self {
        Self::Symbol(id)
    }

    /// The key a String value is used as — most of `ToPropertyKey` (§7.1.19).
    ///
    /// Not all of it: §7.1.19 first takes `ToPrimitive` of whatever it was given and then
    /// `ToString` of that, so `o[{}]` is `o["[object Object]"]`. Those need objects and the
    /// operations that reach user code; this is the step underneath, and it is the only one that
    /// concerns the heap.
    pub fn from_string(heap: &mut Heap, id: StringId) -> Self {
        Self::String(heap.intern_id(id))
    }

    /// The Value that names this key — the inverse of §7.1.19, which needs no conversion.
    ///
    /// A key is a String or a Symbol and both are Values already, so this loses nothing and
    /// `ToPropertyKey` of what comes back is the key it came from. That round trip is what lets a
    /// settled key be left on the operand stack for a later instruction to use.
    #[must_use]
    pub fn to_value(self) -> crate::value::Value {
        match self {
            Self::String(id) => crate::value::Value::String(id),
            Self::Symbol(id) => crate::value::Value::Symbol(id),
        }
    }

    /// The String this key is, if it is one.
    ///
    /// `None` for a Symbol, and every caller has to say what it does about that — which is the
    /// point. A key that cannot be spelled is not a key `for`-`in` yields, nor one `Object.keys`
    /// lists, nor one a message can name.
    pub fn as_string(self) -> Option<StringId> {
        match self {
            Self::String(id) => Some(id),
            Self::Symbol(_) => None,
        }
    }

    /// The Symbol this key is, if it is one.
    pub fn as_symbol(self) -> Option<SymbolId> {
        match self {
            Self::Symbol(id) => Some(id),
            Self::String(_) => None,
        }
    }

    /// The array index this key is, if it is one — §6.1.7, "an integer index in `[+0, 2^32 - 2]`".
    ///
    /// The bound is one below `2^32 - 1` and that is not an off-by-one: `2^32 - 1` is the largest
    /// value `length` may take, so if it were also an index, an array could hold an element it
    /// could not describe the length of.
    ///
    /// `"-0"` is not an index. It is a canonical numeric String — §7.1.21 gives it a step of its
    /// own — and it is still not an index, because the interval starts at `+0` and `-0` is not in
    /// it. `a["-0"]` is a named property, and that is observable.
    pub fn as_array_index(self, heap: &Heap) -> Option<u32> {
        // A Symbol is no index and never will be, and falls out through `as_integer_index`.
        let index = self.as_integer_index(heap)?;
        // `2^32 - 2` is the last index; `as` cannot lose anything after the comparison, since the
        // value is an integer already inside `u32`'s range.
        (index <= f64::from(u32::MAX - 1)).then_some(index as u32)
    }

    /// The integer index this key is, if it is one — §6.1.7, `[+0, 2^53 - 1]`.
    ///
    /// Wider than an array index and used by the typed arrays, whose elements are addressed by
    /// any safe integer. Returned as an `f64` because that is the interval's type: `2^53 - 1` is
    /// exactly representable and the next integer is not, which is the whole reason the bound is
    /// there.
    pub fn as_integer_index(self, heap: &Heap) -> Option<f64> {
        let units = heap.string(self.as_string()?)?;
        let index = canonical_numeric_index(units)?;
        // "an integral Number in the inclusive interval from +0 to 2^53 - 1", in three tests.
        //
        // `fract` settles integrality and both infinities and NaN at once: theirs is NaN, which
        // is equal to nothing including zero. The sign bit settles the lower end, and it is the
        // sign rather than a comparison because `-0 <= x` is true of `-0` — the one value the
        // interval excludes and `<=` cannot see.
        let integral = index.fract() == 0.0;
        (integral && index.is_sign_positive() && index <= 9_007_199_254_740_991.0).then_some(index)
    }
}

/// What an object stores under a key — §6.1.7.1's four attributes, all present.
///
/// "Unless specified explicitly, the initial value of each attribute is its Default Value", and
/// every default is `false` or `undefined`: a property made without saying otherwise is invisible
/// to `for...in`, cannot be redefined, and cannot be written to. That is the opposite of what
/// assignment produces, and the difference is `CreateDataProperty` against `DefineProperty` —
/// which is why `Object.defineProperty(o, "x", {value: 1})` makes a property `o.x = 1` would not.
#[derive(Debug, Clone, Copy)]
pub struct Property {
    /// Which of the two kinds this is, with the attributes only that kind has.
    pub kind: PropertyKind,
    /// `[[Enumerable]]` — whether `for...in` and `Object.keys` see it.
    pub enumerable: bool,
    /// `[[Configurable]]` — whether it may be deleted or redefined.
    ///
    /// The attribute that makes the rest permanent: with this `false`, almost every change to the
    /// property is rejected, and that rule is where nearly all of
    /// `ValidateAndApplyPropertyDescriptor`'s length comes from.
    pub configurable: bool,
}

/// A data property or an accessor property — §6.1.7, and never both.
#[derive(Debug, Clone, Copy)]
pub enum PropertyKind {
    /// A value, and whether it may be replaced.
    Data {
        /// `[[Value]]` — what a get returns.
        value: Value,
        /// `[[Writable]]` — whether a set may change it.
        writable: bool,
    },
    /// One or two functions called instead of reading and writing a value.
    ///
    /// Both are `Value` rather than something narrower because §6.1.7.1 says "a function object
    /// **or `undefined`**", and `undefined` is how a getter-only property says it has no setter.
    /// That they must otherwise be callable is checked where a descriptor becomes a property, not
    /// here — this type is what the object holds after that check has passed.
    Accessor {
        /// `[[Getter]]` — called with no arguments on a get.
        getter: Value,
        /// `[[Setter]]` — called with the new value on a set.
        setter: Value,
    },
}

impl Property {
    /// The fully populated descriptor for this property — what `[[GetOwnProperty]]` answers.
    ///
    /// Populated because it comes from a property, which has every attribute of its kind; the
    /// fields of the *other* kind stay absent, which is what makes the result a data or an
    /// accessor descriptor rather than a generic one.
    pub fn to_descriptor(&self) -> PropertyDescriptor {
        let mut descriptor = PropertyDescriptor {
            enumerable: Some(self.enumerable),
            configurable: Some(self.configurable),
            ..PropertyDescriptor::EMPTY
        };
        match self.kind {
            PropertyKind::Data { value, writable } => {
                descriptor.value = Some(value);
                descriptor.writable = Some(writable);
            }
            PropertyKind::Accessor { getter, setter } => {
                descriptor.getter = Some(getter);
                descriptor.setter = Some(setter);
            }
        }
        descriptor
    }
}

/// A Property Descriptor — §6.2.6, "a Record with **zero or more** fields".
///
/// # Why every field is an `Option`
///
/// Because "has a `[[Value]]` field" is a question the specification asks, and it is not the same
/// question as "the `[[Value]]` field is `undefined`". `{value: undefined}` has one and
/// `{writable: true}` does not, and the two behave differently everywhere:
/// `Object.defineProperty(o, "x", {get: undefined})` defines an **accessor** property with no
/// getter, while `{}` alone leaves an existing property's kind alone.
///
/// `Some(Value::Undefined)` and `None` are therefore different descriptors, and no shorter
/// encoding can say that.
#[derive(Debug, Clone, Copy)]
pub struct PropertyDescriptor {
    /// `[[Value]]`.
    pub value: Option<Value>,
    /// `[[Writable]]`.
    pub writable: Option<bool>,
    /// `[[Getter]]` — named as §6.2.6 names it.
    ///
    /// The specification calls this `[[Getter]]` and not `[[Get]]` deliberately: `[[Get]]` is an
    /// object's internal method, and the two used to share a name and be confused for each other.
    pub getter: Option<Value>,
    /// `[[Setter]]`.
    pub setter: Option<Value>,
    /// `[[Enumerable]]`.
    pub enumerable: Option<bool>,
    /// `[[Configurable]]`.
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    /// The descriptor with no fields at all — a generic descriptor, and the base every other one
    /// is built from.
    ///
    /// A `const` rather than a `Default` implementation, because `Default` would read as "the
    /// default descriptor" and there is no such thing: §6.1.7.1's defaults are what
    /// [`PropertyDescriptor::complete`] fills in, and they are not this.
    pub const EMPTY: Self = Self {
        value: None,
        writable: None,
        getter: None,
        setter: None,
        enumerable: None,
        configurable: None,
    };

    /// A plain data property holding `value` — writable, enumerable and configurable.
    ///
    /// The three attributes an *assignment* gives a property it creates, per §10.1.9.2 step 4's
    /// `CreateDataProperty`, and the shape almost every property in a running program has.
    ///
    /// Here rather than spelled out at each site, because written twice the copies can disagree —
    /// and one of them is always the one nothing looks at. That is not hypothetical: the list a
    /// `for`-`in` walks is built with this, and its own copy of the three booleans could be set to
    /// anything without a single test noticing, since a script never sees that list.
    pub const fn data(value: Value) -> Self {
        Self {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..Self::EMPTY
        }
    }

    /// `IsAccessorDescriptor` (§6.2.6.1) — whether it has a `[[Getter]]` or a `[[Setter]]` field.
    pub fn is_accessor_descriptor(&self) -> bool {
        self.getter.is_some() || self.setter.is_some()
    }

    /// `IsDataDescriptor` (§6.2.6.2) — whether it has a `[[Value]]` or a `[[Writable]]` field.
    pub fn is_data_descriptor(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// `IsGenericDescriptor` (§6.2.6.3) — whether it is neither of the other two.
    ///
    /// Not "has no fields": `{enumerable: true}` is generic and has one. A descriptor may be
    /// neither a data nor an accessor descriptor, and may not be both — the second half is an
    /// invariant callers uphold rather than one this type can enforce, since the fields are
    /// public and a descriptor is assembled a field at a time.
    pub fn is_generic_descriptor(&self) -> bool {
        !self.is_accessor_descriptor() && !self.is_data_descriptor()
    }

    /// `CompletePropertyDescriptor` (§6.2.6.5) — fill in every absent field with its default.
    ///
    /// The defaults are §6.1.7.1's, and *which* ones are filled in depends on what the descriptor
    /// already is: a data descriptor gains `[[Value]]` and `[[Writable]]` and never gains
    /// accessors, an accessor descriptor the other way about, and a generic one is completed as
    /// a **data** descriptor — which is why `Object.getOwnPropertyDescriptor` never answers
    /// something with all six fields.
    pub fn complete(&self) -> Self {
        let mut completed = *self;
        if self.is_generic_descriptor() || self.is_data_descriptor() {
            completed.value = completed.value.or(Some(Value::Undefined));
            completed.writable = completed.writable.or(Some(false));
        } else {
            completed.getter = completed.getter.or(Some(Value::Undefined));
            completed.setter = completed.setter.or(Some(Value::Undefined));
        }
        completed.enumerable = completed.enumerable.or(Some(false));
        completed.configurable = completed.configurable.or(Some(false));
        completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(heap: &mut Heap, text: &str) -> PropertyKey {
        PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn two_keys_spelled_the_same_are_one_key_though_their_strings_were_two() {
        // The reason keys are interned at all. Without this, `o.a = 1` and `o.a` would file
        // under different properties and the read would find nothing.
        let mut heap = Heap::new();
        let written = key(&mut heap, "a");
        let read = key(&mut heap, "a");
        assert_eq!(written, read);
        assert_eq!(written.as_string(), read.as_string());
        assert_ne!(written, key(&mut heap, "b"));
        // …and a key made from a String a script computed is the same key again, even though
        // that String is its own allocation.
        let computed = heap.new_string("a".encode_utf16().collect());
        assert_ne!(Some(computed), written.as_string());
        assert_eq!(PropertyKey::from_string(&mut heap, computed), written);
    }

    #[test]
    fn every_string_is_a_key_including_the_empty_one_and_a_lone_surrogate() {
        let mut heap = Heap::new();
        let empty = PropertyKey::from_units(&mut heap, &[]);
        let surrogate = PropertyKey::from_units(&mut heap, &[0xd800]);
        assert_ne!(empty, surrogate);
        assert_eq!(empty, PropertyKey::from_units(&mut heap, &[]));
        assert_eq!(
            empty.as_string().and_then(|id| heap.string(id)),
            Some(&[][..])
        );
        // Neither is an index, and neither may be turned into one by accident.
        assert_eq!(empty.as_array_index(&heap), None);
        assert_eq!(surrogate.as_integer_index(&heap), None);
    }

    #[test]
    fn only_the_canonical_spelling_of_a_number_is_an_index() {
        let mut heap = Heap::new();
        // The left column is an index; the right column reads as the same Number and is not,
        // because `ToString` does not write it that way. This is what keeps `a["01"]` a named
        // property, and it is observable through `Object.keys` ordering.
        for text in ["0", "1", "42", "4294967294", "9007199254740991"] {
            assert!(
                key(&mut heap, text).as_integer_index(&heap).is_some(),
                "{text:?} should be an integer index"
            );
        }
        for text in [
            "01", "1.0", "1e0", " 1", "1 ", "+1", "0x1", "1_0", "", "a", "1.5", "NaN",
        ] {
            assert_eq!(
                key(&mut heap, text).as_integer_index(&heap),
                None,
                "{text:?} should not be an integer index"
            );
        }
    }

    #[test]
    fn minus_zero_is_canonical_and_is_still_not_an_index() {
        // §7.1.21 gives `"-0"` a step of its own, so it *is* a canonical numeric String — and
        // §6.1.7's interval starts at `+0`, so it is not an index. Both halves matter: the first
        // makes typed arrays treat it as numeric, the second makes `a["-0"]` a named property.
        let mut heap = Heap::new();
        assert_eq!(
            canonical_numeric_index(&"-0".encode_utf16().collect::<Vec<_>>()),
            Some(-0.0)
        );
        let minus_zero = key(&mut heap, "-0");
        assert_eq!(minus_zero.as_integer_index(&heap), None);
        assert_eq!(minus_zero.as_array_index(&heap), None);
        // …while `"0"` is both, and the two keys are not the same key.
        let plus_zero = key(&mut heap, "0");
        assert_eq!(plus_zero.as_array_index(&heap), Some(0));
        assert_ne!(minus_zero, plus_zero);
    }

    #[test]
    fn an_array_index_stops_one_short_of_the_largest_length() {
        let mut heap = Heap::new();
        // 2^32 - 2 is the last index; 2^32 - 1 is a length and not an index, which is the one
        // value where the two intervals part company.
        assert_eq!(
            key(&mut heap, "4294967294").as_array_index(&heap),
            Some(4_294_967_294)
        );
        assert_eq!(key(&mut heap, "4294967295").as_array_index(&heap), None);
        // …and it is still an *integer* index, which is the wider interval typed arrays use.
        assert_eq!(
            key(&mut heap, "4294967295").as_integer_index(&heap),
            Some(4_294_967_295.0)
        );
        // 2^53 - 1 is the last of those; the next integer is not representable, which is why the
        // interval ends there.
        assert_eq!(
            key(&mut heap, "9007199254740991").as_integer_index(&heap),
            Some(9_007_199_254_740_991.0)
        );
        assert_eq!(
            key(&mut heap, "9007199254740992").as_integer_index(&heap),
            None
        );
        // A negative one is neither, and `-1` is the key `at(-1)` exists to avoid needing.
        assert_eq!(key(&mut heap, "-1").as_integer_index(&heap), None);
        assert_eq!(key(&mut heap, "Infinity").as_integer_index(&heap), None);
    }

    #[test]
    fn a_descriptor_is_classified_by_which_fields_it_has_not_by_their_values() {
        // §6.2.6's three predicates, and the distinction the whole type exists for: a field that
        // is present and `undefined` is present.
        let undefined_getter = PropertyDescriptor {
            getter: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(undefined_getter.is_accessor_descriptor());
        assert!(!undefined_getter.is_data_descriptor());
        assert!(!undefined_getter.is_generic_descriptor());

        let undefined_value = PropertyDescriptor {
            value: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(undefined_value.is_data_descriptor());
        assert!(!undefined_value.is_accessor_descriptor());

        // `[[Writable]]` alone makes it a data descriptor, and `[[Setter]]` alone an accessor
        // one — neither needs the field one would name the kind after.
        let writable_only = PropertyDescriptor {
            writable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        assert!(writable_only.is_data_descriptor());
        let setter_only = PropertyDescriptor {
            setter: Some(Value::Undefined),
            ..PropertyDescriptor::EMPTY
        };
        assert!(setter_only.is_accessor_descriptor());

        // Generic is not "empty": a descriptor with attributes and no kind is what
        // `Object.defineProperty(o, "x", {enumerable: true})` passes.
        let attributes_only = PropertyDescriptor {
            enumerable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(attributes_only.is_generic_descriptor());
        assert!(PropertyDescriptor::EMPTY.is_generic_descriptor());
    }

    #[test]
    fn completing_a_descriptor_fills_in_one_kind_and_never_both() {
        // §6.2.6.5. A generic descriptor completes as a *data* descriptor, which is why nothing
        // ever comes back with all six fields.
        let completed = PropertyDescriptor::EMPTY.complete();
        assert!(matches!(completed.value, Some(Value::Undefined)));
        assert_eq!(completed.writable, Some(false));
        assert!(completed.getter.is_none());
        assert!(completed.setter.is_none());
        assert_eq!(completed.enumerable, Some(false));
        assert_eq!(completed.configurable, Some(false));

        // An accessor descriptor gains the other pair, and gains no value.
        let accessor = PropertyDescriptor {
            getter: Some(Value::Undefined),
            enumerable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        let completed = accessor.complete();
        assert!(matches!(completed.setter, Some(Value::Undefined)));
        assert!(completed.value.is_none());
        assert!(completed.writable.is_none());
        // …and an attribute that was already there is not overwritten by its default.
        assert_eq!(completed.enumerable, Some(true));
        assert_eq!(completed.configurable, Some(false));
    }

    #[test]
    fn a_stored_property_describes_itself_with_the_fields_of_its_own_kind() {
        let data = Property {
            kind: PropertyKind::Data {
                value: Value::Number(1.0),
                writable: true,
            },
            enumerable: true,
            configurable: false,
        };
        let descriptor = data.to_descriptor();
        assert!(descriptor.is_data_descriptor());
        assert!(!descriptor.is_accessor_descriptor());
        assert_eq!(descriptor.writable, Some(true));
        assert_eq!(descriptor.enumerable, Some(true));
        assert_eq!(descriptor.configurable, Some(false));
        assert!(descriptor.getter.is_none());

        let accessor = Property {
            kind: PropertyKind::Accessor {
                getter: Value::Number(0.0),
                setter: Value::Undefined,
            },
            enumerable: false,
            configurable: true,
        };
        let descriptor = accessor.to_descriptor();
        assert!(descriptor.is_accessor_descriptor());
        assert!(!descriptor.is_data_descriptor());
        // A setter that is `undefined` is a setter *field* that is present — the property has no
        // setter and the descriptor says so by having the field, not by omitting it.
        assert!(matches!(descriptor.setter, Some(Value::Undefined)));
        assert!(descriptor.value.is_none());
        // Already fully populated, so completing it changes nothing.
        let completed = descriptor.complete();
        assert!(completed.value.is_none());
        assert_eq!(completed.configurable, Some(true));
    }
}
