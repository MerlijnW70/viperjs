//! Where the values that do not fit in a register live.
//!
//! Four of §6.1's eight language types are `Copy` and need nothing from anyone; see
//! [`crate::value`]. The other four have to be *somewhere*, and this is it: an arena the `Heap`
//! owns, addressed by an index. DR-0010 has the argument for that shape and against the obvious
//! alternative — briefly, `Rc` cannot be a mark-sweep collector because it never frees a cycle,
//! and JavaScript makes cycles before user code runs.
//!
//! # What is here so far
//!
//! Strings. They come first because everything else needs them — a property key is a String or a
//! Symbol, so the object model cannot be built underneath one — and because they are the
//! simplest thing the heap will ever hold: a String has no prototype, no properties, and no
//! identity beyond its contents. Getting the arena right against them costs nothing extra.
//!
//! Objects, Symbols, BigInts and the collector itself are the slices after this one. Nothing
//! here is freed yet, which is why there is no free list and no generation counter on a handle:
//! until a sweep exists, no slot is ever reused and a stale handle cannot be made.
//!
//! # Why a handle is not a reference
//!
//! It would be pleasant to hand out `&[u16]` and be done. It is not possible: the next allocation
//! may reallocate the arena, so a borrow of one string would freeze the heap against every other
//! use of it. An index survives reallocation, which is the whole reason arenas are shaped this
//! way — and the reason reading one takes the `Heap` back as an argument.
//!
//! # How this module is laid out
//!
//! - `property` — [`PropertyKey`], and what an object files under one.
//! - here — the arena, [`StringId`], and the intern table property keys need.

mod property;

pub use self::property::{Property, PropertyDescriptor, PropertyKey, PropertyKind};

use crate::span::Span;
use std::collections::HashMap;

/// A String on the heap — a sequence of UTF-16 code units (DR-0004).
///
/// Not a `String` and not a `str`: `"\u{d800}"` is a legal ECMAScript string of one code unit,
/// and no Rust string type can hold it. The consequences are worked through in DR-0004; what
/// matters here is that the element type is `u16` and that nothing validates it.
///
/// Meaningful only to the [`Heap`] that issued it. See [`Heap::string`] for what happens when it
/// is given to another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(usize);

/// The arena every heap-allocated value lives in.
///
/// One `Heap` is one realm on one thread (GOAL.md §3), so there is no locking here and no plan
/// for any: an embedder that wants isolation runs a second engine, which is cheap when the engine
/// is small.
#[derive(Debug, Default)]
pub struct Heap {
    /// Every String ever allocated, in the order they were allocated.
    ///
    /// A `Box<[u16]>` and not a `Vec<u16>`: a String is immutable once made — §6.1.4 gives no way
    /// to change one — so the spare capacity a `Vec` keeps for growth would be paid for by every
    /// string in the program and used by none of them.
    strings: Vec<Box<[u16]>>,
    /// Where a given sequence of code units was interned, if it ever was.
    ///
    /// Only property keys go in here, and [`Heap::intern`] says why they must: two Strings with
    /// the same contents are two Strings, so `o.a` written twice makes two handles, and a
    /// property map keyed by a handle would file them under different properties.
    ///
    /// The units are held twice — once here as the key and once in `strings`. That is the boring
    /// implementation: a table that borrowed from the arena would have to hash through it, which
    /// is a hand-written map rather than the standard library's. Real engines do share the
    /// storage; doing so here is an M8 experiment with a measurement, not a guess.
    interned: HashMap<Box<[u16]>, StringId>,
}

impl Heap {
    /// An empty heap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `units` on the heap and answer where it went.
    ///
    /// Takes the code units by value because the heap is going to keep them, and because every
    /// caller either has just built them or is copying them out of somewhere that keeps its own.
    ///
    /// There is no failure case and no capacity check. The index is `Vec::len` before the push,
    /// so it is valid by construction — see DR-0010 for why the handle is a `usize` rather than
    /// something narrower that would need one.
    pub fn new_string(&mut self, units: Vec<u16>) -> StringId {
        let id = StringId(self.strings.len());
        self.strings.push(units.into_boxed_slice());
        id
    }

    /// Put the source text `span` covers on the heap, as the code units it denotes.
    ///
    /// The bridge from the parser's world to this one: a `StringLiteral`'s value is already
    /// UTF-16 by the time the lexer is done with it, but an identifier or a raw span is still
    /// UTF-8 source, and this is where the conversion belongs rather than at each call.
    ///
    /// Answers `None` for a span that does not lie in `source` — off the end, or off a character
    /// boundary — which is what [`Span::slice`] already says about such a span.
    pub fn new_string_from_span(&mut self, source: &str, span: Span) -> Option<StringId> {
        let text = span.slice(source)?;
        Some(self.new_string(text.encode_utf16().collect()))
    }

    /// The code units of the String `id` refers to, or `None` if this heap has nothing there.
    ///
    /// A handle is meaningful only to the heap that issued it (DR-0010), and what is promised
    /// about a foreign one is narrower than it first looks: never a panic and never an
    /// out-of-range read, but *not* detection. A handle from another heap that happens to be in
    /// range answers with this heap's value at that index, which is a wrong string. Catching
    /// that needs an identifier on every handle, and one realm on one thread means no script can
    /// produce the situation — see DR-0010 for the whole of the argument.
    pub fn string(&self, id: StringId) -> Option<&[u16]> {
        self.strings.get(id.0).map(|units| &**units)
    }

    /// The one String on this heap with these contents, allocating it if there is not one yet.
    ///
    /// # Why anything is interned at all
    ///
    /// DR-0010 says nothing is, and for values that stays true — `"a" === "a"` compares code
    /// units, not handles. A property *key* is different: an object files its properties under
    /// keys, and `o.a = 1; o.a` produces two Strings with the same contents. A map keyed by a
    /// raw handle would file those as two properties, and the second read would find nothing.
    ///
    /// So keys are interned and values are not, and the two are different types for exactly that
    /// reason — see [`PropertyKey`], whose only constructors go through here.
    ///
    /// # What it costs
    ///
    /// A hash of the contents per key made, and one copy of the units kept in the table. Nothing
    /// is ever removed: until the collector exists, an interned key lives as long as the heap.
    /// That is a leak in the same sense that everything else here is one, and the sweep will
    /// treat this table the way engines do — weakly.
    pub fn intern(&mut self, units: &[u16]) -> StringId {
        if let Some(id) = self.interned.get(units) {
            return *id;
        }
        let id = self.new_string(units.to_vec());
        self.interned.insert(units.into(), id);
        id
    }

    /// The interned String with the same contents as `id`, which may be `id` itself.
    ///
    /// What `ToPropertyKey` needs: a String a script computed, filed under the one handle every
    /// equal String will be filed under. A handle this heap does not know interns as the empty
    /// String, which is the same answer [`Heap::string`] gives it — see there for why that
    /// situation is bounded rather than detected.
    pub fn intern_id(&mut self, id: StringId) -> StringId {
        let units = self.string(id).unwrap_or(&[]).to_vec();
        self.intern(&units)
    }

    /// How many Strings this heap holds.
    ///
    /// For tests and for whatever reports on the heap later. It counts allocations rather than
    /// live values, which is the same number until something sweeps.
    pub fn string_count(&self) -> usize {
        self.strings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code units of a `str`, which is what most tests want to put in.
    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_string_comes_back_exactly_as_it_went_in() {
        let mut heap = Heap::new();
        let hello = heap.new_string(units("hello"));
        assert_eq!(heap.string(hello), Some(&units("hello")[..]));
        // The empty string is a string, and is not the absence of one.
        let empty = heap.new_string(Vec::new());
        assert_eq!(heap.string(empty), Some(&[][..]));
        assert_eq!(heap.string_count(), 2);
    }

    #[test]
    fn a_lone_surrogate_survives_the_round_trip() {
        // DR-0004's example, and the reason none of this is a Rust `String`: 0xD800 is a legal
        // ECMAScript string of one code unit and is not a Unicode scalar value, so `String` and
        // `char` both refuse it. Nothing here validates, so nothing here can lose it.
        let mut heap = Heap::new();
        let lone = heap.new_string(vec![0xd800]);
        assert_eq!(heap.string(lone), Some(&[0xd800][..]));
        // …including an unpaired *trailing* surrogate, and a pair in the wrong order, which is
        // two code units that no encoder would produce and a script may still write down.
        let reversed = heap.new_string(vec![0xdc00, 0xd800]);
        assert_eq!(heap.string(reversed), Some(&[0xdc00, 0xd800][..]));
    }

    #[test]
    fn two_strings_with_the_same_contents_are_two_strings() {
        // Nothing is interned. Two allocations give two handles, and the handles differ even
        // though the contents do not — which is why string equality has to read the heap rather
        // than compare handles. Interning is an optimisation with a measurement behind it, and
        // there is no measurement yet.
        let mut heap = Heap::new();
        let first = heap.new_string(units("same"));
        let second = heap.new_string(units("same"));
        assert_ne!(first, second);
        assert_eq!(heap.string(first), heap.string(second));
    }

    #[test]
    fn a_foreign_handle_is_bounded_rather_than_detected() {
        // The narrow claim DR-0010 makes, tested in both directions so that neither half can be
        // read as the other. A script cannot reach any of this — one realm, one thread — and an
        // embedder running two engines can.
        let mut one = Heap::new();
        let mut other = Heap::new();
        one.new_string(units("first in one"));
        let same_index = other.new_string(units("first in other"));

        // In range: the answer is *this* heap's value at that index. A wrong string, and no
        // detection. Writing the pleasant version of this assertion — `None` — would be
        // claiming a guarantee the handle does not carry.
        assert_eq!(one.string(same_index), Some(&units("first in one")[..]));

        // Out of range: `None`, and that is the whole of what is promised — no panic, no
        // out-of-range read.
        other.new_string(units("second in other"));
        let past_the_end = other.new_string(units("third in other"));
        assert_eq!(one.string(past_the_end), None);
    }

    #[test]
    fn a_span_becomes_the_code_units_it_denotes() {
        let mut heap = Heap::new();
        let source = "let name = 'value';";
        let id = heap
            .new_string_from_span(source, Span::new(4, 8))
            .expect("the span lies in the source"); // a test about the contents needs them
        assert_eq!(heap.string(id), Some(&units("name")[..]));

        // Text outside the Basic Multilingual Plane becomes the surrogate pair it is stored as,
        // so a span of one character can be two code units — which is what `.length` will say.
        let emoji = "let x = 🚀;";
        let id = heap
            .new_string_from_span(emoji, Span::new(8, 12))
            .expect("the span lies in the source"); // same
        assert_eq!(heap.string(id), Some(&[0xd83d, 0xde80][..]));
    }

    #[test]
    fn a_span_that_is_not_in_the_source_allocates_nothing() {
        let mut heap = Heap::new();
        // Past the end, and off a character boundary — the two ways `Span::slice` answers `None`,
        // and the heap has to leave no half-made string behind for either.
        assert_eq!(heap.new_string_from_span("abc", Span::new(0, 99)), None);
        assert_eq!(heap.new_string_from_span("é", Span::new(0, 1)), None);
        assert_eq!(heap.string_count(), 0);
    }

    #[test]
    fn no_sequence_of_code_units_can_make_the_heap_panic() {
        // DR-0002 reaches here too: these are the values a script computed, and a string is the
        // one heap type whose contents a script chooses byte for byte.
        let mut heap = Heap::new();
        let awkward: [Vec<u16>; 7] = [
            Vec::new(),
            vec![0],                      // an interior NUL
            vec![0xd800],                 // a lone leading surrogate
            vec![0xdfff],                 // a lone trailing surrogate
            vec![0xdc00, 0xd800],         // a reversed pair
            vec![0xffff, 0xfffe, 0xfeff], // a non-character, a BOM, and a reversed BOM
            vec![0x41; 100_000],          // long enough to have reallocated on the way in
        ];
        for units in awkward {
            let expected = units.clone();
            let id = heap.new_string(units);
            assert_eq!(heap.string(id), Some(&expected[..]));
        }
    }
}
