//! §14.7.5.10 `EnumerateObjectProperties` — which names a `for`-`in` visits, and in what order.
//!
//! Its own file because it is the one operation that reads *across* the prototype chain rather
//! than along it, and the rule that makes it interesting is not about the object it starts from.
//!
//! # Why a non-enumerable property still matters
//!
//! The specification says the enumeration "must not visit a property more than once" and that a
//! property of a prototype is not visited "if it is shadowed by an earlier one" — and *shadowed*
//! means by name, not by visibility. So an own property that is **not** enumerable still hides a
//! prototype's property of the same name: it is passed over itself and takes the name with it.
//!
//! That is the whole reason this cannot be written as "collect the enumerable keys of each object
//! and concatenate". Every name met has to be remembered, whether or not it was yielded.

use crate::heap::{Heap, Object, ObjectId, PropertyDescriptor, PropertyKey};
use crate::value::Value;
use std::collections::HashSet;

impl Heap {
    /// The property names a `for`-`in` over `object` visits, in the order it visits them.
    ///
    /// A snapshot rather than a live walk. §14.7.5.10 lets an implementation decide what happens
    /// to properties *added* during the enumeration — they need not be visited — while a property
    /// **deleted** before it is reached must not be. Taking the names once settles the first, and
    /// the loop asks again about each name before using it, which settles the second.
    ///
    /// String keys only. §14.7.5.10 says so outright, and it is why `for`-`in` cannot be used to
    /// find a Symbol-keyed property.
    pub fn enumerable_keys(&mut self, object: ObjectId) -> Vec<PropertyKey> {
        let mut visited: HashSet<PropertyKey> = HashSet::new();
        let mut keys = Vec::new();
        let mut next = Some(object);
        while let Some(id) = next {
            // No guard for an id this heap has not got: its keys come back empty and its
            // prototype comes back `None`, so the walk ends on the next turn of its own accord.
            for key in self.own_property_keys(id) {
                // Met before, so it belongs to a nearer object and this one is shadowed —
                // whether or not the nearer one was yielded. See the module comment.
                if !visited.insert(key) {
                    continue;
                }
                if self
                    .own_property(id, key)
                    .is_some_and(|found| found.enumerable)
                {
                    keys.push(key);
                }
            }
            next = self.object(id).and_then(Object::prototype);
        }
        keys
    }

    /// The same names as an Array of Strings, which is what a `for`-`in` iterates over.
    ///
    /// An ordinary Array rather than a new kind of value: the collector already traces one, the
    /// interpreter already holds one in a slot, and a list of names is a list. Built here rather
    /// than in the interpreter because filling it uses the exotic define that keeps `length` in
    /// step, which belongs to the heap.
    pub fn new_enumeration(&mut self, prototype: ObjectId, object: ObjectId) -> ObjectId {
        let keys = self.enumerable_keys(object);
        self.enumeration_of(prototype, &keys)
    }

    /// The same list, already gathered — for §10.5, whose walk needs an interpreter.
    ///
    /// A proxy's enumerable keys come from its `ownKeys` and `getOwnPropertyDescriptor` traps, so
    /// [`Heap::enumerable_keys`] cannot gather them and [`crate::vm::Vm`] does it instead. The
    /// array is still built here, because filling it uses the exotic define that keeps `length` in
    /// step.
    pub fn enumeration_of(&mut self, prototype: ObjectId, keys: &[PropertyKey]) -> ObjectId {
        let array = self.new_array(prototype, 0);
        for (at, key) in keys.iter().enumerate() {
            // Bounded by how many properties an object has, which is bounded by what fits in
            // memory — and `u32::MAX` names would have exhausted DR-0013's budget long before.
            let index = u32::try_from(at).unwrap_or(u32::MAX);
            let slot = self.index_key(index);
            // Every key here is spellable: §14.7.5.10 says so, and `enumerable_keys` filtered the
            // Symbols out before this list was built. An index is spelled *here* rather than where
            // it was made, which is DR-0026's trade — per enumeration instead of per element.
            let Some(name) = key.spelling(self) else {
                continue;
            };
            let value = Value::String(name);
            self.define_own_property(array, slot, &PropertyDescriptor::data(value));
        }
        array
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(heap: &mut Heap, text: &str) -> PropertyKey {
        PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
    }

    /// A property with the attributes an assignment gives one.
    fn plain(value: f64) -> PropertyDescriptor {
        PropertyDescriptor {
            value: Some(Value::Number(value)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        }
    }

    /// The same, but not enumerable — what `Object.defineProperty` gives by default.
    fn hidden(value: f64) -> PropertyDescriptor {
        PropertyDescriptor {
            enumerable: Some(false),
            ..plain(value)
        }
    }

    fn names(heap: &mut Heap, object: ObjectId) -> Vec<String> {
        heap.enumerable_keys(object)
            .into_iter()
            .map(|key| key.describe(heap).unwrap_or_default())
            .collect()
    }

    #[test]
    fn the_chain_is_walked_from_the_object_outwards_and_a_name_is_visited_once() {
        let mut heap = Heap::new();
        let grandparent = heap.new_object(None);
        let parent = heap.new_object(Some(grandparent));
        let object = heap.new_object(Some(parent));
        for (holder, name) in [
            (grandparent, "far"),
            (parent, "near"),
            (object, "own"),
            (parent, "shared"),
            (object, "shared"),
        ] {
            let key = key(&mut heap, name);
            heap.define_own_property(holder, key, &plain(1.0));
        }
        // Own first, then each prototype in turn — and `shared` appears once, at the depth it was
        // first met, rather than again when the prototype is reached.
        assert_eq!(names(&mut heap, object), ["own", "shared", "near", "far"]);
    }

    #[test]
    fn a_property_that_is_not_enumerable_is_skipped_and_still_hides_the_one_behind_it() {
        // The rule the module comment is about, and the one a "filter then concatenate" version
        // gets wrong: `hidden` is not visited *and* the prototype's `hidden` is not visited
        // either, because the name was already spoken for.
        let mut heap = Heap::new();
        let parent = heap.new_object(None);
        let object = heap.new_object(Some(parent));
        let hidden_key = key(&mut heap, "hidden");
        let plain_key = key(&mut heap, "plain");
        heap.define_own_property(parent, hidden_key, &plain(1.0));
        heap.define_own_property(parent, plain_key, &plain(2.0));
        heap.define_own_property(object, hidden_key, &hidden(3.0));
        assert_eq!(names(&mut heap, object), ["plain"]);
    }

    #[test]
    fn indices_come_before_names_at_every_depth() {
        // §10.1.11's order applies per object, so the chain is index-then-name repeatedly rather
        // than every index in the chain before every name.
        let mut heap = Heap::new();
        let parent = heap.new_object(None);
        let object = heap.new_object(Some(parent));
        for (holder, name) in [(parent, "b"), (parent, "1"), (object, "a"), (object, "0")] {
            let key = key(&mut heap, name);
            heap.define_own_property(holder, key, &plain(1.0));
        }
        assert_eq!(names(&mut heap, object), ["0", "a", "1", "b"]);
    }

    #[test]
    fn an_object_with_nothing_of_its_own_and_no_prototype_enumerates_nothing() {
        let mut heap = Heap::new();
        let bare = heap.new_object(None);
        assert!(heap.enumerable_keys(bare).is_empty());
        // An accessor is enumerated on the same terms as a data property — what is read is the
        // attribute, not the kind.
        let getter = heap.new_object(None);
        let accessor = key(&mut heap, "a");
        heap.define_own_property(
            bare,
            accessor,
            &PropertyDescriptor {
                getter: Some(Value::Object(getter)),
                enumerable: Some(true),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
        assert_eq!(names(&mut heap, bare), ["a"]);
    }
}
