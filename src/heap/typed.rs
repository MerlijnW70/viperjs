//! §10.4.5 — the integer-indexed exotic object, which is what makes a TypedArray an array.
//!
//! # What is exotic about it
//!
//! A TypedArray has no stored properties for its elements. `ta[0]` is answered from the buffer
//! every time, and `ta[0] = 1` writes into the buffer rather than making a property — which is why
//! two views over one buffer see each other's writes and why a TypedArray's length can never
//! change.
//!
//! # The canonical numeric string, and why it is not "is this a number"
//!
//! §10.4.5 intercepts a key exactly when it is a **CanonicalNumericIndexString**: a String that
//! `ToString(ToNumber(it))` gives back unchanged. So `"0"` is one and `"00"` is not, `"1e3"` is not
//! but `"1000"` is, and `"-0"` is one — the only one that is not a valid index.
//!
//! That distinction is the whole of the exotic behaviour, and getting it wrong is invisible until
//! someone writes `ta["00"]`. A key that *is* canonical and out of range is **absent**: reading it
//! answers `undefined` without consulting the prototype, and writing it is discarded. A key that is
//! not canonical is an ordinary property, stored and read like any other — so `ta["00"] = 1` really
//! does make a property, and `ta[0] = 1` does not.

use crate::heap::{Heap, Numeric, ObjectId, Property, PropertyKey, PropertyKind, View};
use crate::value::Value;

/// §10.4.5.5 steps 1.b to 1.e — the attributes a define at an element may not ask for.
///
/// An element is a writable, enumerable, configurable data property and can be nothing else, so a
/// descriptor asking otherwise is refused before step 1.f converts anything. Shared rather than
/// written twice because the *order* is what it decides: `Vm::define_through` runs the conversion,
/// which can call a program's `valueOf`, and a descriptor these four steps refuse must not run it.
pub(crate) fn element_attributes_refused(descriptor: &crate::heap::PropertyDescriptor) -> bool {
    descriptor.getter.is_some()
        || descriptor.setter.is_some()
        || descriptor.writable == Some(false)
        || descriptor.enumerable == Some(false)
        || descriptor.configurable == Some(false)
}
/// §7.1.21 `CanonicalNumericIndexString`, as an index into a view of `count` elements.
///
/// Three answers, not two, and the middle one is the one that matters:
///
/// - `None` — not a canonical numeric string at all, so this key is an ordinary property.
/// - `Some(Err(()))` — canonical, and not an index this view has. §10.4.5's operations all treat
///   that as *absent*: the read answers `undefined` and the write is discarded, neither of them
///   consulting the prototype. `ta[99]` on a short array is not a prototype lookup.
/// - `Some(Ok(at))` — an element of this view.
pub(super) fn index_of(heap: &Heap, key: PropertyKey, count: usize) -> Option<Result<usize, ()>> {
    let PropertyKey::String(id) = key else {
        // §7.1.21 step 1 — a Symbol is never a numeric index, so it is always an ordinary property.
        return None;
    };
    let units = heap.string(id)?;
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|found| found.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    // §7.1.21 step 2 — `"-0"` is canonical and is never an index, which is the one case where the
    // two questions genuinely differ. It has to be checked by hand because `ToString(-0)` is `"0"`.
    if text == "-0" {
        return Some(Err(()));
    }
    let Ok(number) = text.parse::<f64>() else {
        // Not a number at all, so not a canonical numeric index — which makes it an ordinary
        // property. Said as a `let else` rather than an `ok()?` because the two `None`s mean the
        // same thing here and one of them would otherwise look like an error being swallowed.
        return None;
    };
    // Step 3 — canonical means `ToString(ToNumber(key))` gives the key back. `"00"`, `"1e3"`,
    // `" 1"` and `"0.0"` all parse and none of them comes back the same, so all four are ordinary
    // properties rather than indices.
    if crate::value::number_to_string(number) != text {
        return None;
    }
    // An index has to be a non-negative integer inside the view. Everything else is canonical and
    // absent — a fraction, a negative, `Infinity`, `NaN`, and anything past the end.
    if number < 0.0 || number.fract() != 0.0 || number >= count as f64 {
        return Some(Err(()));
    }
    Some(Ok(number as usize))
}

impl Heap {
    /// The element at `at` of a view, as the property §10.4.5.1 describes.
    ///
    /// **Writable, enumerable and configurable** — all three, which is not what §10.4.3 gives a
    /// String object's characters. A TypedArray's elements can be written, which is the point of
    /// one; they are enumerable so that `Object.keys` and `for`-`in` list them; and they are
    /// configurable, which was a change in ES2021 and is what lets `Object.defineProperty` be used
    /// on them at all.
    /// `&mut` because of the two kinds that hold a BigInt: §6.1.6.2's value lives in the heap and
    /// reading one out of a buffer is therefore an allocation. Eight of the ten need nothing of the
    /// sort, and it is the two that decide the signature — which is why `[[GetOwnProperty]]` and
    /// everything above it takes a mutable heap for what reads like a pure question.
    pub(super) fn element_property(&mut self, view: View, at: usize) -> Option<Property> {
        let value = self.element_at(view, at)?;
        Some(Property {
            kind: PropertyKind::Data {
                value,
                writable: true,
            },
            enumerable: true,
            configurable: true,
        })
    }

    /// Write an element, if there is one there to write.
    ///
    /// Answers nothing, because §10.4.5.5 gives the caller nothing to do with an answer: a write to
    /// an index the view does not have, or to one whose buffer has since been detached, is
    /// **discarded** — in strict mode and sloppy alike. It is the one assignment in the language
    /// that fails silently by design, because a TypedArray's length cannot change and there is
    /// nowhere for the value to go.
    ///
    /// A value of the *other* content type is discarded in the same way, because there is no
    /// truncation that would be honest: §7.1.13 refuses a Number where a BigInt belongs, so a write
    /// that reached here with one is a caller that skipped the conversion rather than a value to be
    /// squeezed into eight bytes.
    pub(super) fn set_element(&mut self, view: View, at: usize, value: &Numeric, clamped: bool) {
        let Some(element) = view.element else {
            return;
        };
        let from = view.offset + at * element.width();
        let Some(bytes) = element.write_numeric(value, clamped) else {
            return;
        };
        if let Some(target) = self
            .object_mut(view.buffer)
            .and_then(super::Object::buffer_mut)
            .and_then(super::Buffer::bytes_mut)
            .and_then(|found| found.get_mut(from..from + element.width()))
        {
            target.copy_from_slice(&bytes);
        }
    }

    /// The view this object is, if it is a TypedArray rather than a `DataView` or anything else.
    #[must_use]
    pub fn typed_view(&self, object: ObjectId) -> Option<View> {
        let view = self.any_view(object)?;
        view.element.map(|_| view)
    }

    /// §10.4.5.2 `IsTypedArrayOutOfBounds` — whether a view's window no longer fits its buffer.
    ///
    /// Step 8 has **two** disjuncts and a tracking view is subject to the first of them. This doc
    /// used to say a tracking view "follows whatever length the buffer has and can never hang off
    /// the end", which is true of its *end* and false of its **start**: `new Uint8Array(rab, 4)`
    /// after `rab.resize(2)` begins past everything there is, and no length it could follow makes
    /// that a window. Step 6 gives such a view a `byteOffsetEnd` of the buffer's own length — so
    /// only the offset can put it out of bounds, and it does.
    ///
    /// The boundary is `>` and not `>=`: an offset landing exactly at the end is a window on the
    /// empty remainder, which is in bounds and has no elements. Those are different answers —
    /// see below.
    ///
    /// Out of bounds is not the same as empty and is treated like *detached*: every method that
    /// begins with `ValidateTypedArray` throws, where a view that merely has no elements walks
    /// nothing and answers. Detachment itself is asked separately — it is a question about the
    /// bytes, and this is a question about the window over them.
    #[must_use]
    pub fn view_out_of_bounds(&self, object: ObjectId) -> bool {
        // The two lookups share one `else`, which is not tidiness: an object that is not a view is
        // reachable and a view whose buffer is not a buffer is not, so a `return false` of its own
        // for the second would be one no input could flip. Mutation coverage said exactly that when
        // this was written as two.
        let Some((view, buffer)) =
            self.object(object)
                .and_then(super::Object::view)
                .and_then(|view| {
                    Some((
                        view,
                        self.object(view.buffer).and_then(super::Object::buffer)?,
                    ))
                })
        else {
            return false;
        };
        // A detached buffer is refused by the detach check rather than reported here, so that the
        // two reasons cannot both fire and disagree about which error to give.
        if buffer.detached() {
            return false;
        }
        let bytes = buffer.byte_length();
        // Step 8's first disjunct, which both kinds of view are subject to.
        if view.offset > bytes {
            return true;
        }
        // Step 8's second, which needs an end of the view's own — step 6 gives a tracking view the
        // buffer's, so for one of those this comparison is `bytes > bytes` and never fires.
        !view.tracking && view.offset + view.length > bytes
    }

    /// The view this object is — a TypedArray's or a `DataView`'s — with its length resolved.
    ///
    /// **The one place a stored `View` becomes a usable one.** A view that tracks a resizable
    /// buffer has no length of its own (§10.4.5's `auto`), so the stored number is stale the moment
    /// the buffer is resized. Resolving here rather than at each reader is what lets forty callers
    /// go on asking `view.count()` and get today's answer: the `View` is a `Copy` snapshot handed
    /// out by value, and this is where the snapshot is taken.
    ///
    /// A view whose buffer has been detached resolves to a length of zero, of **either** kind, and
    /// that is §10.4.5.1 `IsValidIntegerIndex` step 1 rather than a convenience. `view_out_of_bounds`
    /// deliberately answers `false` for a detached buffer so that the two reasons cannot both fire
    /// and disagree about which error to give — but a resolved view has no second question to ask,
    /// because every reader of one treats "no elements" as the whole answer. So detachment has to
    /// land here as well, and a view that kept its stored length said `delete ta[0]` was refused and
    /// `Object.defineProperty(ta, "0", …)` accepted, of an array with nothing in it at all.
    #[must_use]
    pub fn any_view(&self, object: ObjectId) -> Option<View> {
        let mut view = self.object(object)?.view()?;
        let attached = self
            .object(view.buffer)
            .and_then(super::Object::buffer)
            .filter(|buffer| !buffer.detached());
        let Some(available) = attached.map(super::Buffer::byte_length) else {
            view.length = 0;
            return Some(view);
        };
        if !view.tracking {
            // §10.4.5.1 — an out-of-bounds view has no elements at all, so a read finds nothing
            // rather than reaching bytes that are no longer inside the buffer. The methods that
            // refuse it outright ask `view_out_of_bounds`; this is what everything else sees.
            if self.view_out_of_bounds(object) {
                view.length = 0;
            }
            return Some(view);
        }
        // Rounded down to a whole number of elements, because a buffer resized to a length that is
        // not a multiple of the element width leaves a partial element at the end that §10.4.5 does
        // not make visible. A `DataView` has no element and keeps every byte.
        let width = view.element.map_or(1, super::Element::width);
        let usable = available.saturating_sub(view.offset);
        view.length = usable - usable % width;
        Some(view)
    }

    /// The value at `at` of a view, or `None` if there is nothing there.
    ///
    /// A Number for eight of the ten kinds and a BigInt for the other two, which is why this
    /// answers a [`Value`] rather than the `f64` it once did: an element's *type* is a property of
    /// its array, and a caller that assumed otherwise would read a `BigInt64Array` as empty.
    ///
    /// `None` for a `DataView`, which has no elements, for an index past the end, and for a
    /// detached buffer — three different reasons that every caller treats the same way, because
    /// §10.4.5 makes all three *absent* rather than any of them an error.
    pub fn element_at(&mut self, view: View, at: usize) -> Option<Value> {
        let numeric = self.numeric_at(view, at)?;
        Some(self.numeric_value(numeric))
    }

    /// The same element, without making a JavaScript value of it.
    ///
    /// What a caller that is about to write the value straight into another buffer wants — `slice`,
    /// `copyWithin`, `set` and the copy-constructor all move elements without a program ever seeing
    /// one. Going through [`Heap::element_at`] would allocate a BigInt per element and read it back
    /// out again, and this stays `&self` besides.
    #[must_use]
    pub fn numeric_at(&self, view: View, at: usize) -> Option<Numeric> {
        let element = view.element?;
        // Inside the **window**, and not merely inside the buffer. A view that is out of bounds
        // resolves to a count of zero while its bytes are still there, so checking only the slice
        // below reads what the window no longer covers: a `new Int8Array(rab, 0, 4)` over a buffer
        // shrunk to three answered `4` for index 2, where `ta[2]` answered `undefined` — the
        // property path asks `index_of` and gets the count, and this one never did.
        //
        // Written as a range test rather than `at >= view.count()`, which has a boundary no caller
        // reaches: every path here either validates the view first or asks `index_of` for the
        // count, so `at` is never *exactly* the count with readable bytes beyond it. `>=` and `>`
        // would then be two spellings of one question and mutation coverage could not tell them
        // apart — so the question is asked once.
        if !(0..view.count()).contains(&at) {
            return None;
        }
        let from = view.offset + at * element.width();
        Some(
            element.read(
                self.object(view.buffer)?
                    .buffer()?
                    .bytes()?
                    .get(from..from + element.width())?,
            ),
        )
    }

    /// Where `key` points in this object, if it is a TypedArray and `key` is a numeric index.
    ///
    /// Three answers, and the middle one is the one that matters. `None` means this key is an
    /// ordinary property. `Some(Err)` means it is a canonical numeric index the view does not have,
    /// which §10.4.5 treats as *absent*: the read answers `undefined` and the write is discarded,
    /// neither of them consulting the prototype. `Some(Ok)` is an element.
    pub fn typed_index(&self, object: ObjectId, key: PropertyKey) -> Option<Result<usize, ()>> {
        let view = self.typed_view(object)?;
        index_of(self, key, view.count())
    }

    /// Write `value` at an index a lookup already found.
    ///
    /// Nothing happens if the buffer has since been detached, which is what §10.4.5.5 step 1.b.i
    /// means by "return unused": the write is discarded rather than refused, because a TypedArray's
    /// elements are the buffer's bytes and there are none.
    pub fn write_element(&mut self, object: ObjectId, at: usize, value: &Numeric) {
        let Some(view) = self.typed_view(object) else {
            return;
        };
        let clamped = self.object(object).is_some_and(super::Object::is_clamped);
        self.set_element(view, at, value, clamped);
    }

    /// A [`Numeric`] as a JavaScript value, allocating the BigInt when it is one.
    ///
    /// The step that makes reading a `BigInt64Array` need a mutable heap: §6.1.6.2's value has
    /// identity in the heap where §6.1.6.1's is the bits themselves.
    pub fn numeric_value(&mut self, numeric: Numeric) -> Value {
        match numeric {
            Numeric::Number(number) => Value::Number(number),
            Numeric::BigInt(value) => Value::BigInt(self.new_bigint(value)),
        }
    }

    /// The numeric a value already of one of the two numeric types names — `None` for anything else.
    ///
    /// The other direction, for a caller holding elements it read out earlier and is about to write
    /// back. It performs **no conversion**: §7.1.4 and §7.1.13 can both run a program, and neither
    /// belongs in a heap that has no interpreter to run one with.
    pub fn as_numeric(&self, value: Value) -> Option<Numeric> {
        match value {
            Value::Number(number) => Some(Numeric::Number(number)),
            Value::BigInt(id) => Some(Numeric::BigInt(self.bigint(id)?.clone())),
            _ => None,
        }
    }
}

/// The eleven concrete kinds, with the name each constructor carries — §23.2.5.
///
/// In the order §23.2 lists them, which is by width and then by signedness. `Uint8Clamped` is the
/// odd one and is here rather than in [`Element`](crate::heap::Element) because it differs only in
/// *how a value is written*: it is a `Uint8` that saturates instead of wrapping, and every other
/// operation on it is identical.
///
/// Two of the eleven — the `BigInt64` pair — hold a BigInt rather than a Number, which is a
/// difference of a wholly different order: it changes what a write converts with and makes the two
/// unassignable from the other nine in either direction. That question is
/// [`Element::holds_big`](crate::heap::Element::holds_big) and not a column here, because it
/// belongs to the *kind* and every one of its answers follows from the kind alone.
pub const KINDS: [(&str, super::Element, bool); 11] = [
    ("Int8Array", super::Element::Int8, false),
    ("Uint8Array", super::Element::Uint8, false),
    ("Uint8ClampedArray", super::Element::Uint8, true),
    ("Int16Array", super::Element::Int16, false),
    ("Uint16Array", super::Element::Uint16, false),
    ("Int32Array", super::Element::Int32, false),
    ("Uint32Array", super::Element::Uint32, false),
    ("BigInt64Array", super::Element::BigInt64, false),
    ("BigUint64Array", super::Element::BigUint64, false),
    ("Float32Array", super::Element::Float32, false),
    ("Float64Array", super::Element::Float64, false),
];
