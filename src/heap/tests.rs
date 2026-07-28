//! What an object does when its properties are changed, and when they refuse to be.
//!
//! Most of these rows are about one rule — a non-configurable property refuses every change, and
//! restating what is already true is not a change — because that rule is where nearly all of
//! §10.1.6.3's length comes from.

use super::*;
use crate::heap::PropertyKind;
use crate::value::Value;

fn key(heap: &mut Heap, text: &str) -> PropertyKey {
    PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
}

/// The own property, through the heap — the tests all hold an `ObjectId` rather than an
/// `Object`, because that is what everything in the engine will hold.
fn own(heap: &Heap, object: ObjectId, key: PropertyKey) -> Option<Property> {
    heap.object(object)?.get_own_property(key).copied()
}

fn count(heap: &Heap, object: ObjectId) -> usize {
    heap.object(object).map_or(0, Object::property_count)
}

fn keys(heap: &Heap, object: ObjectId) -> Vec<PropertyKey> {
    heap.object(object)
        .map_or_else(Vec::new, |found| found.own_property_keys(heap))
}

fn prevent(heap: &mut Heap, object: ObjectId) {
    if let Some(found) = heap.object_mut(object) {
        found.prevent_extensions();
    }
}

fn delete(heap: &mut Heap, object: ObjectId, key: PropertyKey) -> bool {
    heap.object_mut(object)
        .is_some_and(|found| found.delete(key))
}

/// A descriptor for a plain writable, enumerable, configurable data property — what
/// assignment produces, and what `Object.defineProperty` notably does not.
fn data(value: f64) -> PropertyDescriptor {
    PropertyDescriptor {
        value: Some(Value::Number(value)),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    }
}

#[test]
fn a_new_property_takes_the_defaults_for_every_field_the_descriptor_omits() {
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    // `Object.defineProperty(o, "a", {value: 1})` — three attributes unstated, and every
    // default is `false`. This is the difference from `o.a = 1`, which states all three.
    let bare = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &bare));
    let property = own(&heap, object, a).expect("just defined"); // the test is about it
    assert!(!property.enumerable);
    assert!(!property.configurable);
    assert!(matches!(
        property.kind,
        PropertyKind::Data {
            writable: false,
            ..
        }
    ));
}

#[test]
fn nothing_may_be_added_to_a_non_extensible_object_and_what_is_there_may_still_change() {
    let mut heap = Heap::new();
    let (a, b) = (key(&mut heap, "a"), key(&mut heap, "b"));
    let object = heap.new_object(None);
    assert!(heap.define_own_property(object, a, &data(1.0)));
    prevent(&mut heap, object);

    assert!(!heap.object(object).is_some_and(Object::is_extensible));
    assert!(!heap.define_own_property(object, b, &data(2.0)));
    assert_eq!(count(&heap, object), 1);
    // …while the property that was already there is untouched by any of it: extensibility is
    // about *additions*, and `Object.preventExtensions` is not `Object.freeze`.
    assert!(heap.define_own_property(object, a, &data(3.0)));
    assert!(delete(&mut heap, object, a));
    assert_eq!(count(&heap, object), 0);
}

#[test]
fn a_non_configurable_property_refuses_every_change_but_the_ones_that_are_no_change() {
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    let frozen = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(false),
        enumerable: Some(true),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &frozen));

    // Making it configurable again — the one-way door.
    let becoming_configurable = PropertyDescriptor {
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &becoming_configurable));
    // Changing its enumerability.
    let becoming_hidden = PropertyDescriptor {
        enumerable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &becoming_hidden));
    // Changing its kind.
    let becoming_accessor = PropertyDescriptor {
        getter: Some(Value::Undefined),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &becoming_accessor));
    // Making it writable, or changing its value.
    let becoming_writable = PropertyDescriptor {
        writable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &becoming_writable));
    let other_value = PropertyDescriptor {
        value: Some(Value::Number(2.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &other_value));
    // …and deleting it.
    assert!(!delete(&mut heap, object, a));

    // Restating exactly what is already true is allowed, every time — that is a change of
    // nothing, and §10.1.6.3 only ever refuses changes.
    assert!(heap.define_own_property(object, a, &frozen));
    let restating_attributes = PropertyDescriptor {
        enumerable: Some(true),
        configurable: Some(false),
        writable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &restating_attributes));
    // …as is asking for nothing at all.
    assert!(heap.define_own_property(object, a, &PropertyDescriptor::EMPTY));
    assert_eq!(count(&heap, object), 1);
}

#[test]
fn restating_the_value_of_a_frozen_property_writes_nothing_which_matters_for_nan() {
    // §10.1.6.3's NOTE, and the reason [`Validation`] has three cases. Two NaNs are the same
    // value to `SameValue` and can be told apart by other means, so the specification is
    // careful to leave the stored one alone.
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    let stored = f64::from_bits(0x7ff8_0000_0000_0001);
    let other = f64::from_bits(0xfff8_0000_0000_0000);
    assert!(stored.is_nan() && other.is_nan() && stored.to_bits() != other.to_bits());

    let frozen = PropertyDescriptor {
        value: Some(Value::Number(stored)),
        writable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &frozen));
    // Accepted, because the two are the same *value*…
    let restated = PropertyDescriptor {
        value: Some(Value::Number(other)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &restated));
    // …and the bits that were there are still there.
    let PropertyKind::Data {
        value: Value::Number(kept),
        ..
    } = own(&heap, object, a).expect("still defined").kind
    // the test is about it
    else {
        panic!("still a data property")
    };
    assert_eq!(kept.to_bits(), stored.to_bits());
}

#[test]
fn a_frozen_accessor_keeps_both_of_its_functions_and_may_have_either_restated() {
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    let (getter, setter) = (Value::Number(1.0), Value::Number(2.0));
    let frozen = PropertyDescriptor {
        getter: Some(getter),
        setter: Some(setter),
        enumerable: Some(true),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &frozen));

    // §10.1.6.3 step 5.d, both halves — either function being different is a refusal, and
    // the setter is protected exactly as the getter is.
    let other_getter = PropertyDescriptor {
        getter: Some(Value::Number(9.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &other_getter));
    let other_setter = PropertyDescriptor {
        setter: Some(Value::Number(9.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &other_setter));
    // Restating the same pair is a change of nothing, and so is restating one of them.
    assert!(heap.define_own_property(object, a, &frozen));
    let same_setter = PropertyDescriptor {
        setter: Some(setter),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &same_setter));
    // …and a descriptor naming *no* kind is not asking for a change of kind, so step 5.c
    // lets it through even though this property is an accessor and the descriptor is not.
    let restating_configurable = PropertyDescriptor {
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &restating_configurable));
    // An accessor cannot become a data property while it is non-configurable, though.
    let becoming_data = PropertyDescriptor {
        value: Some(Value::Number(0.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &becoming_data));
}

#[test]
fn a_non_configurable_property_that_is_writable_may_still_be_written_to() {
    // The pair of attributes is not one attribute. `Object.seal` leaves properties writable
    // and non-configurable, and that is the common case rather than a corner: the value may
    // change, the property may not be redefined or deleted.
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    let sealed = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &sealed));

    let other_value = PropertyDescriptor {
        value: Some(Value::Number(2.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &other_value));
    // Restating writability is allowed while it is still writable…
    let staying_writable = PropertyDescriptor {
        writable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &staying_writable));
    // …and giving it up is allowed once, after which neither may come back.
    let freezing = PropertyDescriptor {
        writable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &freezing));
    assert!(!heap.define_own_property(object, a, &staying_writable));
    let third_value = PropertyDescriptor {
        value: Some(Value::Number(3.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(!heap.define_own_property(object, a, &third_value));
    // …while restating the value it now holds is still a change of nothing, and allowed.
    assert!(heap.define_own_property(object, a, &other_value));
    assert!(!delete(&mut heap, object, a));
}

#[test]
fn a_configurable_property_may_become_the_other_kind_and_forgets_what_it_was() {
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    assert!(heap.define_own_property(object, a, &data(1.0)));

    // Data to accessor naming only a *setter*: the old value is gone rather than kept as
    // the getter, which is step 6.a's "or to the attribute's default value otherwise".
    let setter_only = PropertyDescriptor {
        setter: Some(Value::Number(8.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &setter_only));
    assert!(matches!(
        own(&heap, object, a).expect("still defined").kind, // the test is about it
        PropertyKind::Accessor {
            getter: Value::Undefined,
            setter: Value::Number(8.0)
        }
    ));
    assert!(heap.define_own_property(object, a, &data(1.0)));

    // Data to accessor: the value is gone, the unstated accessor is `undefined`, and the two
    // shared attributes survive because the descriptor did not mention them.
    let getter_only = PropertyDescriptor {
        getter: Some(Value::Number(7.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &getter_only));
    let property = own(&heap, object, a).expect("still defined"); // the test is about it
    assert!(matches!(
        property.kind,
        PropertyKind::Accessor {
            getter: Value::Number(7.0),
            setter: Value::Undefined
        }
    ));
    assert!(property.enumerable && property.configurable);

    // …and back, symmetrically.
    let value_only = PropertyDescriptor {
        value: Some(Value::Number(2.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &value_only));
    let property = own(&heap, object, a).expect("still defined"); // same
    assert!(matches!(
        property.kind,
        PropertyKind::Data {
            value: Value::Number(2.0),
            writable: false
        }
    ));
    assert!(property.enumerable && property.configurable);
}

#[test]
fn changing_one_attribute_leaves_the_others_where_they_were() {
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    assert!(heap.define_own_property(object, a, &data(1.0)));
    // Step 6.c — a field the descriptor does not have is an attribute it is not asking about.
    let hide = PropertyDescriptor {
        enumerable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, a, &hide));
    let property = own(&heap, object, a).expect("still defined"); // the test is about it
    assert!(!property.enumerable);
    assert!(property.configurable);
    assert!(matches!(
        property.kind,
        PropertyKind::Data {
            value: Value::Number(1.0),
            writable: true
        }
    ));
}

#[test]
fn deleting_a_property_that_is_not_there_succeeds() {
    // `delete o.nothing` is `true`, and says nothing about whether there was anything.
    let mut heap = Heap::new();
    let a = key(&mut heap, "a");
    let object = heap.new_object(None);
    assert!(delete(&mut heap, object, a));
    assert!(heap.define_own_property(object, a, &data(1.0)));
    assert!(delete(&mut heap, object, a));
    assert!(delete(&mut heap, object, a));
    assert_eq!(count(&heap, object), 0);
}

#[test]
fn own_keys_put_the_array_indices_first_in_numeric_order_and_the_rest_in_creation_order() {
    let mut heap = Heap::new();
    let object = heap.new_object(None);
    // The example every article about this uses, and it is in the specification because
    // every engine had already agreed on it.
    for text in ["b", "2", "a", "1", "10", "0"] {
        let k = key(&mut heap, text);
        assert!(heap.define_own_property(object, k, &data(0.0)));
    }
    let names: Vec<String> = keys(&heap, object)
        .into_iter()
        .map(|k| String::from_utf16_lossy(heap.string(k.as_string()).unwrap_or(&[])))
        .collect();
    // Numeric order, not textual — "10" after "2" is the whole point of sorting on the index.
    assert_eq!(names, ["0", "1", "2", "10", "b", "a"]);
}

#[test]
fn a_key_too_large_to_be_an_array_index_is_ordered_as_a_name() {
    let mut heap = Heap::new();
    let object = heap.new_object(None);
    for text in ["4294967295", "5", "4294967294"] {
        let k = key(&mut heap, text);
        assert!(heap.define_own_property(object, k, &data(0.0)));
    }
    let names: Vec<String> = keys(&heap, object)
        .into_iter()
        .map(|k| String::from_utf16_lossy(heap.string(k.as_string()).unwrap_or(&[])))
        .collect();
    // 2^32 - 1 is a length, never an index, so it keeps its place among the names.
    assert_eq!(names, ["5", "4294967294", "4294967295"]);
}

#[test]
fn a_property_is_found_along_the_prototype_chain_and_the_nearest_one_wins() {
    let mut heap = Heap::new();
    let (a, b, missing) = (
        key(&mut heap, "a"),
        key(&mut heap, "b"),
        key(&mut heap, "z"),
    );
    let grandparent = heap.new_object(None);
    let parent = heap.new_object(Some(grandparent));
    let child = heap.new_object(Some(parent));

    assert!(heap.define_own_property(grandparent, a, &data(1.0)));
    assert!(heap.define_own_property(grandparent, b, &data(1.0)));
    assert!(heap.define_own_property(parent, b, &data(2.0)));

    assert!(heap.has_property(child, a));
    assert!(heap.has_property(child, b));
    assert!(!heap.has_property(child, missing));
    // Shadowing: the nearest object on the chain that has the key is the one that answers.
    assert_eq!(
        heap.find_own(child, b).map(|(owner, _)| owner),
        Some(parent)
    );
    assert_eq!(
        heap.find_own(child, a).map(|(owner, _)| owner),
        Some(grandparent)
    );
    // An object with a null prototype inherits nothing, which is what `Object.create(null)`
    // is for.
    let bare = heap.new_object(None);
    assert!(!heap.has_property(bare, a));
}

#[test]
fn a_prototype_may_not_be_made_to_point_back_at_itself() {
    let mut heap = Heap::new();
    let first = heap.new_object(None);
    let second = heap.new_object(Some(first));
    let third = heap.new_object(Some(second));

    // Directly, and at a distance: `first.__proto__ = third` would close a three-object loop.
    assert!(!heap.set_prototype_of(first, Some(first)));
    assert!(!heap.set_prototype_of(first, Some(third)));
    assert_eq!(heap.object(first).and_then(Object::prototype), None);
    // …while a chain that does not come back is fine.
    let elsewhere = heap.new_object(None);
    assert!(heap.set_prototype_of(first, Some(elsewhere)));
    assert_eq!(
        heap.object(first).and_then(Object::prototype),
        Some(elsewhere)
    );
}

#[test]
fn a_non_extensible_object_keeps_its_prototype_and_may_still_be_told_to_keep_it() {
    let mut heap = Heap::new();
    let prototype = heap.new_object(None);
    let object = heap.new_object(Some(prototype));
    prevent(&mut heap, object);

    assert!(!heap.set_prototype_of(object, None));
    // §10.1.2 step 2 comes *before* the extensibility test, so setting it to what it already
    // is succeeds even here — which is not the same as being able to change it.
    assert!(heap.set_prototype_of(object, Some(prototype)));
    assert_eq!(
        heap.object(object).and_then(Object::prototype),
        Some(prototype)
    );
}

#[test]
fn a_foreign_or_missing_object_handle_answers_rather_than_panicking() {
    // DR-0002 over the handles, on the same terms as `Heap::string`: no panic and no
    // out-of-range read, and no detection either.
    let mut heap = Heap::new();
    let mut other = Heap::new();
    let a = key(&mut heap, "a");
    let stranger = other.new_object(None);
    other.new_object(None);
    let past_the_end = other.new_object(None);

    assert!(heap.object(past_the_end).is_none());
    assert!(!heap.has_property(past_the_end, a));
    assert!(!heap.define_own_property(past_the_end, a, &data(1.0)));
    assert!(heap.find_own(past_the_end, a).is_none());
    assert!(!heap.set_prototype_of(past_the_end, None));
    // …and one that happens to be in range answers about *this* heap's object at that index.
    let mine = heap.new_object(None);
    assert_eq!(stranger, mine);
    assert!(heap.object(stranger).is_some());
}
