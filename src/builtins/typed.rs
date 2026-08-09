//! §23.2 — the TypedArrays, which are one implementation with nine names.
//!
//! # What a TypedArray is
//!
//! A window onto an `ArrayBuffer` with a type. That is the whole of it: its elements are the
//! buffer's bytes read as that type, so two views over one buffer see each other's writes, and its
//! length can never change because the buffer's cannot.
//!
//! Everything that makes it feel like an Array is §10.4.5's exotic behaviour, which lives in the
//! heap next to the String object's: `ta[0]` is answered from the buffer rather than from a
//! property table, and `ta[0] = 1` writes into it. Nothing here is involved in that.
//!
//! # `%TypedArray%` is an abstract constructor
//!
//! §23.2.1 makes it a constructor that **always throws**. It exists to be the prototype of the nine
//! real ones and to hold every method they share — so `Int8Array.prototype.map` and
//! `Float64Array.prototype.map` are the *same function object*, and a program can reach it as
//! `Object.getPrototypeOf(Int8Array)`. There is no other way to name it.
//!
//! # `Uint8ClampedArray` is not a `Uint8Array`
//!
//! It differs in one operation and it is the only one of the nine that differs in any: §7.1.11
//! `ToUint8Clamp` **saturates** where §7.1.9 wraps, so writing 300 gives 255 rather than 44, and it
//! rounds halves to *even* rather than away from zero. That is what makes it right for pixel data
//! and wrong for everything else.

use super::{define_value, key};
use crate::heap::{
    Buffer, Element, Heap, Native, NativeCall, Numeric, ObjectId, PropertyDescriptor, PropertyKey,
    View,
};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Build `%TypedArray%` and the nine concrete constructors into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.typed_array_prototype();
    let abstract_constructor =
        heap.new_native_constructor(realm.function_prototype(), construct_abstract, realm.id());
    super::define_function_metadata(heap, abstract_constructor, "TypedArray", 0);
    super::define_fixed(
        heap,
        abstract_constructor,
        "prototype",
        Value::Object(prototype),
    );
    define_value(
        heap,
        prototype,
        "constructor",
        Value::Object(abstract_constructor),
    );

    // §23.2.3 — the accessors every one of them shares. All four throw for a detached buffer
    // except `buffer`, which is about *which* buffer and not about its bytes.
    for (name, native) in [
        ("buffer", buffer as Native),
        ("byteLength", byte_length),
        ("byteOffset", byte_offset),
        ("length", length),
    ] {
        accessor(heap, realm, prototype, name, native);
    }
    // §23.2.3.32 — `get %TypedArray%.prototype[@@toStringTag]`, which is an accessor rather than a
    // string: it answers the *name of the kind*, so one function serves all nine, and answers
    // `undefined` for anything that is not a TypedArray at all rather than throwing.
    if let Some(symbol) = heap.well_known(super::well_known_at("toStringTag")) {
        let getter =
            heap.new_native_function(realm.function_prototype(), to_string_tag, realm.id());
        super::define_function_metadata(heap, getter, "get [Symbol.toStringTag]", 0);
        let _ = heap.define_own_property(
            prototype,
            PropertyKey::from_symbol(symbol),
            &PropertyDescriptor {
                getter: Some(Value::Object(getter)),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }

    super::typed_methods::install(heap, realm, prototype, abstract_constructor);

    // §23.2.5 — the nine, each inheriting from `%TypedArray%` in *both* directions: the constructor
    // from the abstract constructor, and its prototype from `%TypedArray%.prototype`. That second
    // link is what makes every method shared rather than copied nine times.
    for (name, element, _) in crate::heap::KINDS {
        let kind_prototype = heap.new_object(Some(prototype));
        let constructor =
            heap.new_native_constructor(abstract_constructor, construct_concrete, realm.id());
        super::define_function_metadata(heap, constructor, name, 3);
        super::define_fixed(
            heap,
            constructor,
            "prototype",
            Value::Object(kind_prototype),
        );
        define_value(
            heap,
            kind_prototype,
            "constructor",
            Value::Object(constructor),
        );
        // §23.2.6.2 — `BYTES_PER_ELEMENT`, on both the constructor and its prototype, and on
        // neither is it writable or configurable: it is the one fact about a kind that cannot
        // change, and a program reads it to decide how big a buffer to make.
        let width = Value::Number(element.width() as f64);
        super::define_fixed(heap, constructor, "BYTES_PER_ELEMENT", width);
        super::define_fixed(heap, kind_prototype, "BYTES_PER_ELEMENT", width);
        define_value(heap, global, name, Value::Object(constructor));
    }
}

/// One of §23.2.3's accessors, which are accessors so that they read the view each time.
fn accessor(heap: &mut Heap, realm: &Realm, prototype: ObjectId, name: &str, native: Native) {
    let getter = heap.new_native_function(realm.function_prototype(), native, realm.id());
    super::define_function_metadata(heap, getter, &format!("get {name}"), 0);
    let key = key(heap, name);
    let _ = heap.define_own_property(
        prototype,
        key,
        &PropertyDescriptor {
            getter: Some(Value::Object(getter)),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §23.2.1.1 — `%TypedArray%` itself, which always throws.
///
/// It is a constructor that cannot construct, and that is its whole behaviour: it exists to be the
/// prototype of the nine and to hold what they share. `new (Object.getPrototypeOf(Int8Array))()`
/// is the only way to reach it and it is a TypeError.
fn construct_abstract(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    Err(Abrupt::type_error(
        "the abstract TypedArray constructor cannot be called",
    ))
}

/// §23.2.5.1 — one of the nine, whose first argument decides which of four things it is.
fn construct_concrete(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error(
            "a TypedArray constructor must be called with new",
        ));
    }
    let Some((element, clamped)) = kind_of(heap, call) else {
        return Err(Abrupt::type_error("this is not a TypedArray constructor"));
    };
    // **Where `AllocateTypedArray` runs is decided per branch, and the two branches disagree.**
    // §23.2.5.1 step 6.b.i allocates *first* for an Object argument; step 6.c.ii runs `ToIndex` on
    // anything else and only then allocates at 6.c.iii. So `new Int8Array(Symbol())` is a TypeError
    // from the conversion and a `new.target` whose `prototype` getter throws never runs — where
    // `new Int8Array(someBuffer)` reads that getter before it looks at the buffer at all.
    //
    // Invisible while §10.1.13 read an own data property: nothing observed the order. It became a
    // regression the moment the read turned into a real `Get`, which is what the test that measures
    // it is called.
    match call.argument(0) {
        // §23.2.5.1 step 4 — a *number* is a length, and the buffer is made here. Every other case
        // is an object, and which kind of object decides everything else.
        Value::Object(source)
            if heap
                .object(source)
                .and_then(crate::heap::Object::buffer)
                .is_some() =>
        {
            let prototype = super::prototype_from(vm, heap, call, |realm| {
                realm
                    .typed_prototype(element, clamped)
                    .unwrap_or_else(|| realm.typed_array_prototype())
            })?;
            from_buffer(vm, heap, call, source, prototype, element, clamped)
        }
        Value::Object(source) => {
            let prototype = super::prototype_from(vm, heap, call, |realm| {
                realm
                    .typed_prototype(element, clamped)
                    .unwrap_or_else(|| realm.typed_array_prototype())
            })?;
            from_object(vm, heap, source, prototype, element, clamped)
        }
        length => {
            let count = super::buffer::to_index(vm, heap, length)?;
            let prototype = super::prototype_from(vm, heap, call, |realm| {
                realm
                    .typed_prototype(element, clamped)
                    .unwrap_or_else(|| realm.typed_array_prototype())
            })?;
            allocate(vm, heap, prototype, element, clamped, count)
        }
    }
}

/// Which kind this constructor makes, read off its own `BYTES_PER_ELEMENT` and name.
///
/// A `fn` pointer holds no state, so the nine share one body and it has to ask which it is. The
/// answer is on the function object, where §23.2.6.2 already puts it for the program's benefit.
fn kind_of(heap: &Heap, call: &NativeCall<'_>) -> Option<(Element, bool)> {
    let name = super::own_value(heap, call.function, "name")?;
    let Value::String(id) = name else {
        return None;
    };
    let units = heap.string(id)?;
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|found| found.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    crate::heap::KINDS
        .into_iter()
        .find(|(known, _, _)| *known == text)
        .map(|(_, element, clamped)| (element, clamped))
}

/// §23.2.5.1.1 `AllocateTypedArray` with a buffer of its own — the length form.
fn allocate(
    vm: &mut Vm,
    heap: &mut Heap,
    prototype: ObjectId,
    element: Element,
    clamped: bool,
    count: usize,
) -> Completion<Value> {
    let bytes = count.saturating_mul(element.width());
    super::array_methods::within_budget(vm, heap)?;
    if heap.allowance().checked_sub(bytes).is_none() {
        return Err(Abrupt::range_error(
            "this TypedArray is larger than this engine will allocate",
        ));
    }
    let buffer = heap.new_object(Some(vm.realm().array_buffer_prototype()));
    if let Some(found) = heap.object_mut(buffer) {
        found.set_buffer(Buffer::new(bytes));
    }
    heap.charge_buffer(bytes);
    // A buffer this made itself is fixed, so the view over it has nothing to track.
    let view = View {
        buffer,
        offset: 0,
        length: bytes,
        element: Some(element),
        tracking: false,
    };
    Ok(make(heap, prototype, view, clamped))
}

/// The object itself, once its buffer and window are decided.
fn make(heap: &mut Heap, prototype: ObjectId, view: View, clamped: bool) -> Value {
    let object = heap.new_object(Some(prototype));
    if let Some(found) = heap.object_mut(object) {
        found.set_view(view);
        if clamped {
            found.set_clamped();
        }
    }
    Value::Object(object)
}

/// §23.2.5.1 steps 5 to 7 — a view over a buffer somebody else made.
fn from_buffer(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    buffer: ObjectId,
    prototype: ObjectId,
    element: Element,
    clamped: bool,
) -> Completion<Value> {
    let width = element.width();
    let offset = super::buffer::to_index(vm, heap, call.argument(1))?;
    // Step 6.c — the offset must be a whole number of elements. A `Int32Array` cannot start at
    // byte 1, because its elements would then straddle the boundaries the format promised.
    if offset % width != 0 {
        return Err(Abrupt::range_error(
            "this offset is not a multiple of the element size",
        ));
    }
    let asked = match call.argument(2) {
        Value::Undefined => None,
        given => Some(super::buffer::to_index(vm, heap, given)?),
    };
    // Checked after both conversions, either of which can have detached it.
    // A buffer this far in is known to be one — the constructor matched on it — so the only
    // question left is whether its bytes are still there.
    let available = heap
        .object(buffer)
        .and_then(crate::heap::Object::buffer)
        .filter(|found| !found.detached())
        .map(crate::heap::Buffer::byte_length);
    let Some(available) = available else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    // §23.2.5.1 step 7 — a view over a *resizable* buffer with no explicit length **tracks** it.
    // Both halves are needed: an explicit length pins the window however the buffer moves, and a
    // fixed buffer has nothing to track. This is the only place in §23.2 where `auto` is decided.
    //
    // Asked **before** the lengths are worked out, because step 7 and step 8 are different
    // branches and not a flag on one. Deciding it afterwards ran step 8's checks over a tracking
    // view, and a ten-byte resizable buffer could then not be an `Int32Array` at all — where the
    // clause makes it a tracking view of two elements.
    let tracking = asked.is_none()
        && heap
            .object(buffer)
            .and_then(crate::heap::Object::buffer)
            .is_some_and(|found| found.max_byte_length().is_some());
    let length = match asked {
        // Step 8.a.i — an absent length means "to the end", and the *remainder* must itself be a
        // whole number of elements: a 5-byte buffer cannot be a fixed `Int32Array` even from
        // offset 0. Step 7 has no such rule, because a tracking view's length is recomputed from
        // the buffer at every read and rounded down to whole elements there — so a remainder that
        // is not a whole element is simply not reported, rather than being an error at the start.
        None if !tracking => {
            if available % width != 0 {
                return Err(Abrupt::range_error(
                    "this buffer is not a multiple of the element size",
                ));
            }
            available
                .checked_sub(offset)
                .ok_or_else(|| Abrupt::range_error("this offset is past the end of the buffer"))?
        }
        // Step 7.a — the one thing that can be wrong about a tracking view is beginning past the
        // end. `>` and not `>=`: an offset exactly at the end is a window on the empty remainder,
        // which is a view with no elements rather than a refusal.
        None => {
            if offset > available {
                return Err(Abrupt::range_error(
                    "this offset is past the end of the buffer",
                ));
            }
            // **Zero, and nothing is lost.** `Heap::any_view` recomputes a tracking view's length
            // from the buffer at every read and `view_out_of_bounds` asks `!tracking` before it
            // looks — so this field is dead for one of these, and working out the right number
            // here would be arithmetic no program could check. Mutation coverage said exactly
            // that: two operators flipped inside it and nothing noticed.
            0
        }
        Some(count) => count.saturating_mul(width),
    };
    // Step 8.b.ii, and step 7 has no equivalent: a tracking view cannot be longer than its buffer
    // because it has no length of its own to be too long.
    if !tracking && offset + length > available {
        return Err(Abrupt::range_error(
            "this TypedArray is longer than its buffer",
        ));
    }
    let view = View {
        buffer,
        offset,
        length,
        element: Some(element),
        tracking,
    };
    Ok(make(heap, prototype, view, clamped))
}

/// §23.2.5.1 step 5.b — another TypedArray, or anything iterable, or an array-like.
fn from_object(
    vm: &mut Vm,
    heap: &mut Heap,
    source: ObjectId,
    prototype: ObjectId,
    element: Element,
    clamped: bool,
) -> Completion<Value> {
    // §23.2.5.1.2 — from another TypedArray the elements are *copied by value*, converting each
    // one: a `Uint8Array` made from a `Float64Array` holds the truncated bytes and shares no
    // buffer with it. That is the difference from the buffer form above, and it is easy to
    // mistake one for the other.
    let values: Vec<Numeric> = match heap.typed_view(source) {
        Some(view) => {
            // §23.2.5.1.2 step 5 — the two content types are a **TypeError** and not a conversion:
            // `new BigInt64Array(new Int8Array(1))` throws, and so does the reverse. This is the
            // one copy in §23.2 that reads elements without going through `ToNumber` or `ToBigInt`
            // at all, so without this check the bytes of one type would be written as the other's.
            if view.element.is_some_and(Element::holds_big) != element.holds_big() {
                return Err(Abrupt::type_error(
                    "a BigInt TypedArray and a Number one cannot be copied into each other",
                ));
            }
            (0..view.count())
                .filter_map(|at| heap.numeric_at(view, at))
                .collect()
        }
        None => {
            // §23.2.5.1 step 6.b — `GetMethod(object, @@iterator)`, and the branch is on whether
            // there **is** one. Not on whether walking it succeeded: this was written
            // `iterable_to_list(…).or_else(|_| array_like(…))`, which caught every error the walk
            // could raise and answered with a different construction instead.
            //
            // So `new Float64Array(obj)` where `obj`'s `@@iterator` is a getter that throws built
            // an empty array — a function has a `length` of 0, so the fallback found an array-like
            // and the program's own error was discarded. Every step of 6.b and 6.c is a `?`, and a
            // fallback that fires on failure rather than on absence cannot be one.
            let taken = match super::array::iterator_method_of(vm, heap, Value::Object(source))? {
                Some(method) => super::promise_group::iterable_to_list_with(
                    vm,
                    heap,
                    Value::Object(source),
                    method,
                )?,
                None => array_like(vm, heap, source)?,
            };
            let mut numbers = Vec::with_capacity(taken.len());
            for value in taken {
                numbers.push(vm.to_numeric(element.holds_big(), value, heap)?);
            }
            numbers
        }
    };
    let made = allocate(vm, heap, prototype, element, clamped, values.len())?;
    if let Value::Object(id) = made {
        for (at, value) in values.into_iter().enumerate() {
            heap.write_element(id, at, &value);
        }
    }
    Ok(made)
}

/// §23.2.5.1.5 — an object with a `length` and no iterator.
fn array_like(vm: &mut Vm, heap: &mut Heap, source: ObjectId) -> Completion<Vec<Value>> {
    let name = key(heap, "length");
    let length = vm.get_property_key(Value::Object(source), name, heap)?;
    let count = super::array_methods::to_length(vm.to_number(length, heap)?);
    // DR-0013, and the bound is on what this is about to **produce** rather than on what was
    // asked for. The list below lives in Rust memory, which `within_budget` does not measure, so
    // `new Int8Array({ length: 2 ** 53 })` looped reading absent properties with nothing able to
    // refuse it — the heap never grew, so the check inside the loop never fired. Same shape as
    // `String.prototype.repeat`'s: a loop counted by a number a program chose.
    //
    // Asked as "could the heap hold the answer" rather than against a constant, because that is
    // the only honest ceiling — and a `length` past it cannot produce a TypedArray whatever the
    // elements turn out to be.
    //
    // Subtracted rather than compared, which is the idiom §25.1.3.1's allocation already uses here
    // and for the same reason: the boundary of a comparison is a number no test can reach, because
    // the allowance depends on what the heap already holds. Asking whether the room is *there*
    // leaves nothing to be off by one about.
    let room = usize::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(size_of::<Value>()))
        .and_then(|wanted| heap.allowance().checked_sub(wanted));
    if room.is_none() {
        return Err(Abrupt::range_error(
            "this array-like is longer than this engine will allocate",
        ));
    }
    let mut taken = Vec::new();
    for at in 0..count {
        let index = super::array_methods::index_key(heap, at);
        taken.push(vm.get_property_key(Value::Object(source), index, heap)?);
        super::array_methods::within_budget(vm, heap)?;
    }
    Ok(taken)
}

/// The view `this` is, or the TypeError §23.2.3 asks for.
fn view_of(heap: &Heap, this: Value) -> Completion<(ObjectId, View)> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not a TypedArray"));
    };
    heap.typed_view(object)
        .map(|view| (object, view))
        .ok_or_else(|| Abrupt::type_error("this is not a TypedArray"))
}

/// Whether the buffer behind a view has gone.
fn detached(heap: &Heap, view: View) -> bool {
    heap.object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached)
}

/// §23.2.3.1 — `get buffer`, which answers even for a detached one.
fn buffer(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, view) = view_of(heap, call.this_value)?;
    Ok(Value::Object(view.buffer))
}

/// §23.2.3.2 — `get byteLength`, which is **0** for a detached buffer rather than a throw.
fn byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, view) = view_of(heap, call.this_value)?;
    let answer = if detached(heap, view) { 0 } else { view.length };
    Ok(Value::Number(answer as f64))
}

/// §23.2.3.3 — `get byteOffset`, which is **0** for a window that is no longer one.
///
/// Step 4 asks `IsTypedArrayOutOfBounds` and answers `+0` for it, which is wider than the detached
/// case its neighbours above can get away with: `byteLength` and `length` read a length
/// [`Heap::any_view`] has already zeroed, and an *offset* is never zeroed there because the offset
/// is what a shrunk view is out of bounds **by**. So this asks the question itself.
///
/// Both halves are needed. `view_out_of_bounds` deliberately answers `false` for a detached buffer
/// — §10.4.5.2 asserts the buffer is attached, and its callers ask detachment separately so the two
/// reasons cannot disagree about which error to give — where §23.2.3.3 wants one answer for both.
fn byte_offset(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (object, view) = view_of(heap, call.this_value)?;
    let gone = detached(heap, view) || heap.view_out_of_bounds(object);
    let answer = if gone { 0 } else { view.offset };
    Ok(Value::Number(answer as f64))
}

/// §23.2.3.19 — `get length`, in **elements** rather than bytes.
///
/// Where §23.2 differs from §25.3: a `DataView` has a `byteLength` and no `length` at all, because
/// it has no elements. This is the same window divided by the width of its type.
fn length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, view) = view_of(heap, call.this_value)?;
    let answer = if detached(heap, view) {
        0
    } else {
        view.count()
    };
    Ok(Value::Number(answer as f64))
}

/// §23.2.3.32 — `get [@@toStringTag]`, which answers the *kind's* name.
///
/// One accessor for all nine, and `undefined` for anything that is not a TypedArray — where every
/// other method here throws. It has to answer rather than throw because
/// `Object.prototype.toString` reads it off whatever it was given.
fn to_string_tag(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(object) = call.this_value else {
        return Ok(Value::Undefined);
    };
    let Some(view) = heap.typed_view(object) else {
        return Ok(Value::Undefined);
    };
    let clamped = heap
        .object(object)
        .is_some_and(crate::heap::Object::is_clamped);
    let Some(element) = view.element else {
        return Ok(Value::Undefined);
    };
    let name = crate::heap::KINDS
        .into_iter()
        .find(|(_, known, known_clamped)| *known == element && *known_clamped == clamped)
        .map_or("", |(name, _, _)| name);
    let _ = vm;
    Ok(super::text(heap, name))
}
