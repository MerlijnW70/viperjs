//! §25.2's `SharedArrayBuffer` and §25.4's `Atomics`.
//!
//! # What "shared" means to an engine with one agent
//!
//! Two things, and neither is about threads. A `SharedArrayBuffer` **cannot be detached** — §25.2
//! gives it no `[[ArrayBufferDetachKey]]` and no `transfer`, so its bytes are there for as long as
//! anything can name it. And it is a different **brand**: `ArrayBuffer.prototype.byteLength`
//! requires an unshared buffer and `SharedArrayBuffer.prototype.byteLength` a shared one, so
//! neither answers about the other however alike the bytes are.
//!
//! ViperJS runs one agent, so the memory model of §25.4.1 has nothing to order: every operation is
//! already the only one happening. That does **not** make `Atomics` decorative — the operations
//! have arithmetic and coercion of their own, they refuse the wrong element kinds, and they read
//! and write in ways `ta[i]` does not.
//!
//! # Why `Atomics` accepts an ordinary `ArrayBuffer`
//!
//! Since ES2020 every operation here works on an unshared buffer too — only `wait` requires a
//! shared one. §25.4.3's `ValidateIntegerTypedArray` asks about the *element kind* rather than
//! about sharing, which is the check these actually make.
//!
//! # `wait` and `notify` with one agent, which is not the same as without them
//!
//! Both are implemented, and neither is a stub. [`notify`] answers `+0` because that is what
//! waking a list with nothing on it returns, and [`wait`] throws a TypeError because DoWait step
//! 12 asks `AgentCanSuspend()` and this agent cannot — nothing could ever wake it. A browser's
//! main thread answers identically, which is what test262's `CanBlockIsFalse` flag is for.
//!
//! What that leaves is everything before those steps, and it is most of what a test can see: the
//! *waitable* kind check ([`Kinds`]), which admits `Int32Array` and `BigInt64Array` alone, the
//! index, and the conversions of the value, the count and the timeout — each of which may run the
//! program's own `valueOf` and must run in the clause's order.

use super::{define_method, define_value, key};
use crate::heap::{Element, Heap, Native, NativeCall, Numeric, ObjectId, PropertyDescriptor, View};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// The buffer `this` is, when it is shared — §25.2.4's brand check.
fn shared_buffer(heap: &Heap, this: Value) -> Completion<ObjectId> {
    let Value::Object(object) = this else {
        return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
    };
    match heap.object(object).and_then(crate::heap::Object::buffer) {
        Some(found) if found.shared() => Ok(object),
        _ => Err(Abrupt::type_error("this is not a SharedArrayBuffer")),
    }
}

/// §25.2.2.1 `SharedArrayBuffer(length)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        return Err(Abrupt::type_error(
            "SharedArrayBuffer must be called with new",
        ));
    }
    let asked = super::buffer::to_index(vm, heap, call.argument(0))?;
    // §25.2.2.1 step 2 — the same options bag §25.1.3.1 reads, under the same name. A growable
    // SharedArrayBuffer is the shared half of a resizable one, and the only difference is that it
    // may not shrink.
    let max = super::buffer::max_byte_length_option(vm, heap, call.argument(1))?;
    if max.is_some_and(|max| asked > max) {
        return Err(Abrupt::range_error(
            "this SharedArrayBuffer is longer than its maxByteLength allows",
        ));
    }
    super::array_methods::within_budget(heap)?;
    // DR-0013 — asked before the bytes are taken rather than noticed afterwards, because the
    // length is a number the program chose. `checked_sub` rather than a comparison for the reason
    // §25.1.3.1's version gives: the boundary of a comparison here is a number no test can reach,
    // since the allowance depends on what the heap already holds. Subtracting says the same thing
    // with nothing to be off by one about.
    if heap.allowance().checked_sub(asked).is_none() {
        return Err(Abrupt::range_error(
            "this SharedArrayBuffer is larger than this engine will allocate",
        ));
    }
    let prototype = super::prototype_from(heap, call, vm.realm().shared_buffer_prototype());
    let object = heap.new_object(Some(prototype));
    heap.charge_buffer(asked);
    if let Some(found) = heap.object_mut(object) {
        let mut made = crate::heap::Buffer::new_shared(asked);
        if let Some(max) = max {
            made.allow_resizing_to(max);
        }
        found.set_buffer(made);
    }
    Ok(Value::Object(object))
}

/// §25.2.4.2 `get SharedArrayBuffer.prototype.growable`.
fn growable(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let can = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_some_and(|buffer| buffer.max_byte_length().is_some());
    Ok(Value::Boolean(can))
}

/// §25.2.4.4 `get SharedArrayBuffer.prototype.maxByteLength`.
///
/// A buffer that cannot grow answers its current length, exactly as §25.1.6.2 does for a fixed
/// `ArrayBuffer` — and there is no detached case to consider, because §25.2 has no way to detach.
fn max_byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, |buffer| {
            buffer
                .max_byte_length()
                .unwrap_or_else(|| buffer.byte_length())
        });
    Ok(Value::Number(length as f64))
}

/// §25.2.4.3 `SharedArrayBuffer.prototype.grow`.
///
/// §25.1.6.4's `resize` with one rule added and one removed: a shared buffer may only get
/// **bigger**, and it has no detached state to refuse. The growth-only rule is not tidiness — a
/// shrink would pull memory out from under a view another agent is reading through, and §25.2
/// exists precisely so that memory can be shared without that being possible.
fn grow(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let growable = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .is_some_and(|buffer| buffer.max_byte_length().is_some());
    if !growable {
        return Err(Abrupt::type_error("this SharedArrayBuffer is not growable"));
    }
    let length = super::buffer::to_index(vm, heap, call.argument(0))?;
    let Some(buffer) = heap
        .object_mut(object)
        .and_then(crate::heap::Object::buffer_mut)
    else {
        return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
    };
    let before = buffer.byte_length();
    // §25.2.4.3 step 8 — shrinking is a **RangeError** and not a silent no-op, so a program that
    // believes it shrank a shared buffer finds out rather than carrying on with a wrong length.
    if length < before {
        return Err(Abrupt::range_error("a SharedArrayBuffer cannot shrink"));
    }
    if !buffer.resize(length) {
        return Err(Abrupt::range_error(
            "this length is past the SharedArrayBuffer's maxByteLength",
        ));
    }
    heap.charge_buffer_delta(before, length);
    Ok(Value::Undefined)
}

/// §25.2.4.1 `get SharedArrayBuffer.prototype.byteLength`.
fn byte_length(_: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    Ok(Value::Number(length as f64))
}

/// §25.2.4.3 `SharedArrayBuffer.prototype.slice`.
///
/// The same arithmetic as §25.1.5.4's, and a different brand at each end: the receiver must be
/// shared, and so must what the species made.
fn slice(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = shared_buffer(heap, call.this_value)?;
    let length = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map_or(0, crate::heap::Buffer::byte_length);
    let start = super::array_methods::start_index(vm, heap, call.argument(0), length as u64)?;
    let end = match call.argument(1) {
        Value::Undefined => length as u64,
        given => super::array_methods::start_index(vm, heap, given, length as u64)?,
    };
    let taken = (end.saturating_sub(start)) as usize;
    let bytes = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::bytes)
        .map(|found| found[start as usize..start as usize + taken].to_vec())
        .unwrap_or_default();
    let prototype = vm.realm().shared_buffer_prototype();
    let made = heap.new_object(Some(prototype));
    heap.charge_buffer(taken);
    if let Some(found) = heap.object_mut(made) {
        let mut buffer = crate::heap::Buffer::new_shared(taken);
        if let Some(target) = buffer.bytes_mut() {
            target.copy_from_slice(&bytes);
        }
        found.set_buffer(buffer);
    }
    Ok(Value::Object(made))
}

/// Which arithmetic §25.4.3's read-modify-write operations do.
#[derive(Clone, Copy)]
enum Operation {
    /// §25.4.3.1 — `add`.
    Add,
    /// §25.4.3.2 — `and`.
    And,
    /// §25.4.3.11 — `or`.
    Or,
    /// §25.4.3.14 — `sub`.
    Sub,
    /// §25.4.3.15 — `xor`.
    Xor,
    /// §25.4.3.5 — `exchange`, which keeps the new value and answers the old.
    Exchange,
}

impl Operation {
    /// The new value, given what is there and what was asked for.
    ///
    /// The bitwise three are done on the *integer* form, because that is what they mean: `&` on a
    /// pair of doubles is not an operation, and every element kind these accept is an integer one.
    fn apply(self, held: &Numeric, given: &Numeric) -> Numeric {
        match (held, given) {
            (Numeric::Number(held), Numeric::Number(given)) => {
                let (left, right) = (*held as i64, *given as i64);
                Numeric::Number(match self {
                    Self::Add => held + given,
                    Self::And => (left & right) as f64,
                    Self::Or => (left | right) as f64,
                    Self::Sub => held - given,
                    Self::Xor => (left ^ right) as f64,
                    Self::Exchange => *given,
                })
            }
            // §25.4.1.2's read-modify-write on a sixty-four bit slot, done on the **bits**. A
            // BigInt element is stored two's complement and `low_u64` is exactly those bits, so
            // every one of the six is the machine operation — where §6.1.6.2's arithmetic would
            // allocate a magnitude on the way to being truncated back to the same eight bytes.
            //
            // Read back through `from_u64` and not through `from_bits`, because there is no sign
            // to decide: this answer's only use is the write below, which takes its low sixty-four
            // bits again, and those are the same eight bytes whichever way the top one was read.
            // Whether it is a sign is a question for the next *read* of the cell — the same reason
            // `setBigInt64` and `setBigUint64` are one native. Spelling it as a flag said
            // otherwise, and no program could tell the two spellings apart.
            (Numeric::BigInt(held), Numeric::BigInt(given)) => {
                let (left, right) = (held.low_u64(), given.low_u64());
                let bits = match self {
                    Self::Add => left.wrapping_add(right),
                    Self::And => left & right,
                    Self::Or => left | right,
                    Self::Sub => left.wrapping_sub(right),
                    Self::Xor => left ^ right,
                    Self::Exchange => right,
                };
                Numeric::BigInt(crate::bigint::BigInt::from_u64(bits))
            }
            // A Number against a BigInt, which no call can present: the value was converted by the
            // array's own content type and the element was read from the array. Answering what was
            // already there leaves the bytes alone, which is the only harmless thing to do with a
            // pair that cannot arise.
            _ => held.clone(),
        }
    }
}

/// §25.4.3.4's `waitable` argument, which decides *which* integer kinds an operation admits.
///
/// The two answers are not a subset relation worth collapsing: a waiter list is keyed on a
/// position in a buffer and the two kinds that may key one are exactly `Int32Array` and
/// `BigInt64Array`, where the arithmetic operations take every unclamped integer kind there is.
/// Passing the wrong one is not a missing error — it is `Atomics.wait` accepting a `Uint8Array`,
/// which no waiter could ever be woken from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kinds {
    /// `ValidateIntegerTypedArray(ta, false)` — every unclamped integer kind, floats and
    /// `Uint8ClampedArray` aside.
    Integer,
    /// `ValidateIntegerTypedArray(ta, true)` — `Int32Array` and `BigInt64Array`, and nothing else.
    Waitable,
    /// `ValidateSharedIntegerTypedArray(ta, true)` — the above, and the buffer must be **shared**.
    ///
    /// A third variant rather than a second argument because these are three named operations in
    /// the specification and every caller wants exactly one of them. It matters *where* the shared
    /// check sits: it is inside step 1, so it runs **before** the index is converted, and
    /// `Atomics.wait(unshared, poisoned, …)` must refuse the buffer without ever calling
    /// `poisoned.valueOf`. Checking it after the index reads identically and runs the program's
    /// code when the clause says nothing may run.
    SharedWaitable,
}

/// §25.4.3.4 `ValidateIntegerTypedArray`, and the index — the opening every operation here shares.
///
/// Answers the view and the position, because getting one without the other is never useful. The
/// index is validated *after* the element kind, which is what §25.4.3.3 step 1 asks for: a
/// `Float64Array` is refused before its index is even looked at.
fn target(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    kinds: Kinds,
) -> Completion<(View, Element, usize)> {
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    let Some(view) = heap.typed_view(object) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    // §25.4.2.1 — `IsUnclampedIntegerElementType` or `IsBigIntElementType`, and nothing else. The
    // float kinds are refused because atomics are about bit patterns a CPU can exchange and a
    // double is not one of those however well it holds an integer; `Uint8ClampedArray` is refused
    // because §7.1.11's saturation is not a bit pattern either — an atomic exchange that quietly
    // wrote 255 for 300 would not be the exchange it was asked for. The `BigInt64` pair *is*
    // accepted, and is the reason the kind comes back rather than being checked and dropped.
    let Some(element) = view.element else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    let clamped = heap
        .object(object)
        .is_some_and(crate::heap::Object::is_clamped);
    if clamped || matches!(element, Element::Float32 | Element::Float64) {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    }
    // §25.4.3.4 step 3.a — the waitable check *replaces* the one above rather than adding to it,
    // and it is asked here, before the index, for the same reason: `Atomics.wait(new Uint8Array(8),
    // {valueOf(){ throw }})` refuses the array and never runs the getter.
    if kinds != Kinds::Integer && !matches!(element, Element::Int32 | Element::BigInt64) {
        return Err(Abrupt::type_error(
            "this is not an Int32Array or a BigInt64Array",
        ));
    }
    // `ValidateSharedIntegerTypedArray` step 9 — still inside step 1, and therefore still ahead of
    // the index. An unshared buffer cannot be reached by another agent, so a wait on one could
    // never be ended by anything and the clause refuses rather than hanging. §25.4.3.7's `notify`
    // takes either and answers `0`, which is why this is not asked for every waitable operation.
    if kinds == Kinds::SharedWaitable
        && !heap
            .object(view.buffer)
            .and_then(crate::heap::Object::buffer)
            .is_some_and(crate::heap::Buffer::shared)
    {
        return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
    }
    if heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .is_none_or(crate::heap::Buffer::detached)
    {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    }
    let asked = super::buffer::to_index(vm, heap, call.argument(1))?;
    // §25.4.3.3 step 3 — out of range is a **RangeError**, where an ordinary `ta[9]` is silently
    // `undefined`. These say so, because an atomic write that went nowhere is worse than an error.
    if asked >= view.count() {
        return Err(Abrupt::range_error("that index is outside the TypedArray"));
    }
    Ok((view, element, asked))
}

/// The five arithmetic operations and `exchange`, which differ only in [`Operation::apply`].
fn modify(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    what: Operation,
) -> Completion<Value> {
    let (view, element, at) = target(vm, heap, call, Kinds::Integer)?;
    // The value is converted *after* the index is validated, and its conversion may run user code
    // that detaches the buffer — so the write is checked again below rather than assumed. Which
    // conversion is §25.4.3.1 step 3's: `ToBigInt` for the two 64-bit kinds and `ToNumber` for the
    // six others, so `Atomics.add(new BigInt64Array(1), 0, 1)` is a TypeError.
    let given = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    let Some(held) = heap.numeric_at(view, at) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    heap.write_element(object, at, &what.apply(&held, &given));
    // Every one of them answers the value that *was* there, which is what makes them atomic
    // read-modify-writes rather than writes.
    Ok(heap.numeric_value(held))
}

/// §25.4.3.9 `Atomics.load`.
fn load(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, _, at) = target(vm, heap, call, Kinds::Integer)?;
    Ok(heap.element_at(view, at).unwrap_or(Value::Undefined))
}

/// §25.4.3.13 `Atomics.store`.
///
/// The one that answers what it was *given* rather than what it wrote or what was there. The two
/// differ: storing 300 into a `Uint8Array` writes 44 and answers 300.
fn store(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, element, at) = target(vm, heap, call, Kinds::Integer)?;
    let given = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    // §25.4.3.13 step 3 — a Number is put through `ToIntegerOrInfinity` *before* it is answered,
    // where a BigInt is answered as it came. Both are the value the call was given rather than the
    // bytes that were written, which differ: storing 300 into a `Uint8Array` writes 44 and
    // answers 300.
    let answer = match &given {
        Numeric::Number(number) => Numeric::Number(super::string::to_integer_or_infinity(*number)),
        big => big.clone(),
    };
    heap.write_element(object, at, &given);
    Ok(heap.numeric_value(answer))
}

/// §25.4.3.3 `Atomics.compareExchange`.
fn compare_exchange(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, element, at) = target(vm, heap, call, Kinds::Integer)?;
    let big = element.holds_big();
    let expected = vm.to_numeric(big, call.argument(1 + 1), heap)?;
    let replacement = vm.to_numeric(big, call.argument(3), heap)?;
    let Some(held) = heap.numeric_at(view, at) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    let Value::Object(object) = call.argument(0) else {
        return Err(Abrupt::type_error("this is not an integer TypedArray"));
    };
    // §25.4.3.3 step 9 — the comparison is against the expected value **as this kind would store
    // it**, not against what was handed over: expecting 300 of a `Uint8Array` holding 44 is a
    // match, because 300 stored there *is* 44. Comparing the raw arguments would never match and
    // the write would never happen.
    //
    // One round trip and not two. What is held came out of the slot and re-encoding it could only
    // give the bytes it was already read from, so the two sides are not symmetrical however alike
    // they look — and a second encoding is a second place for §7.1.11's clamping to be asked for,
    // where no answer to it is observable.
    let stored = element
        .write_numeric(&expected, false)
        .map(|bytes| element.read(&bytes));
    if stored.as_ref() == Some(&held) {
        heap.write_element(object, at, &replacement);
    }
    Ok(heap.numeric_value(held))
}

/// §25.4.3.7 `Atomics.notify(typedArray, index, count)`.
///
/// Always answers `+0` here, and that is the specification's own answer rather than a stub: step 8
/// wakes the waiters on a list, ViperJS has one agent, and an agent cannot be waiting while it is
/// running this. What the clause still asks for is every step *before* that — the waitable kind
/// check, the index, and the count's coercion — each of which can throw and each of which runs
/// user code the program can observe.
///
/// **The non-shared case is a `0` and not an error**, which is where this parts company with
/// [`wait`]: step 7 returns `+0` for an ordinary `ArrayBuffer`, because notifying nobody is what
/// notifying a buffer no one can share amounts to. Refusing it would be a stricter engine, not a
/// more correct one.
fn notify(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let _ = target(vm, heap, call, Kinds::Waitable)?;
    // Step 3 — the count is computed and then thrown away, because with nothing waiting there is
    // no number of woken agents for a program to see. It is still computed, and that is the point:
    // computing it is what runs the program's own `valueOf` and what lets that `valueOf`'s throw
    // out, both of which are observable where the number is not.
    //
    // **The clause's `undefined` → +∞ case is deliberately not branched on.** +∞ and
    // `ToIntegerOrInfinity(undefined)`'s 0 are both discarded, and `ToNumber(undefined)` is `NaN`
    // and cannot throw — so a branch there is one no program could ever distinguish. Written as a
    // case it survived mutation, which is the ratchet saying the branch is not a missing test but
    // a distinction this engine cannot have.
    let count = vm.to_number(call.argument(2), heap)?;
    let _ = super::string::to_integer_or_infinity(count).max(0.0);
    Ok(Value::Number(0.0))
}

/// §25.4.3.14 `Atomics.wait(typedArray, index, value, timeout)`.
///
/// Throws a TypeError, always — and this is a conformant answer rather than a gap. DoWait step 12
/// asks `AgentCanSuspend()`, and ViperJS's agent cannot: there is no second agent to notify it, so
/// an agent that suspended here would never be woken by anything. A browser's main thread answers
/// the same way for the same reason, which is what test262's `CanBlockIsFalse` flag exists to say.
///
/// **The four arguments are still converted first**, and that is the whole of what a test can see.
/// Steps 1 to 11 validate the array, the index, the value and the timeout in that order, so
/// `Atomics.wait(new Float64Array(4), 0, 0, 0)` is refused for its *kind* and never reaches the
/// suspend check, while `Atomics.wait(i32a, 0, 0, {valueOf(){ throw new Error() }})` throws the
/// program's own error rather than this one.
fn wait(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (_, element, _) = target(vm, heap, call, Kinds::SharedWaitable)?;
    // Step 10 — the same `ToBigInt`/`ToNumber` split the arithmetic operations make, for the same
    // reason: the comparison is against a stored bit pattern, so the value has to become one.
    let _ = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    // Step 11 — `ToNumber` and not `ToIndex`: a timeout is a duration and `-1` is not an error, it
    // is `max(q, 0)`. NaN is +∞ rather than 0, which is the reverse of what `ToIndex` would give
    // and is why `Atomics.wait(i32a, 0, 0, undefined)` waits for ever rather than not at all.
    let _ = vm.to_number(call.argument(3), heap)?;
    Err(Abrupt::type_error("this agent cannot be suspended"))
}

/// §25.4.3.8 `Atomics.isLockFree`.
///
/// Answers about a *width* rather than about a buffer. ViperJS has one agent, so every width it
/// supports is lock-free in the only sense the question has — but the answer must still be the
/// same for a given width every time it is asked, which §25.4.3.8's note requires.
fn is_lock_free(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let width = vm.to_number(call.argument(0), heap)?;
    Ok(Value::Boolean(matches!(width as i64, 1 | 2 | 4 | 8)))
}

/// Build §25.2's `SharedArrayBuffer` and §25.4's `Atomics` onto the global.
pub(super) fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.shared_buffer_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "SharedArrayBuffer", 1);
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(
        heap,
        global,
        "SharedArrayBuffer",
        Value::Object(constructor),
    );
    define_value(heap, prototype, "constructor", Value::Object(constructor));
    define_method(heap, realm, prototype, "slice", 2, slice);
    define_method(heap, realm, prototype, "grow", 1, grow);
    for (name, native) in [
        ("byteLength", byte_length as Native),
        ("growable", growable),
        ("maxByteLength", max_byte_length),
    ] {
        let getter = heap.new_native_function(realm.function_prototype(), native);
        super::define_function_metadata(heap, getter, &format!("get {name}"), 0);
        let name = key(heap, name);
        let _ = heap.define_own_property(
            prototype,
            name,
            &PropertyDescriptor {
                getter: Some(Value::Object(getter)),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    super::buffer::define_species(heap, realm, constructor);
    super::collection::tag_with(heap, realm, prototype, "SharedArrayBuffer");

    // §25.4 — an ordinary object rather than a constructor, like `Math` and `JSON`.
    let atomics = heap.new_object(Some(realm.object_prototype()));
    define_value(heap, global, "Atomics", Value::Object(atomics));
    for (name, length, native) in [
        ("add", 3, add as Native),
        ("and", 3, and),
        ("compareExchange", 4, compare_exchange),
        ("exchange", 3, exchange),
        ("isLockFree", 1, is_lock_free),
        ("load", 2, load),
        ("notify", 3, notify),
        ("or", 3, or),
        ("store", 3, store),
        ("sub", 3, sub),
        ("wait", 4, wait),
        ("xor", 3, xor),
    ] {
        define_method(heap, realm, atomics, name, length, native);
    }
    super::collection::tag_with(heap, realm, atomics, "Atomics");
}

/// §25.4.3.1 `Atomics.add`.
fn add(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Add)
}

/// §25.4.3.2 `Atomics.and`.
fn and(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::And)
}

/// §25.4.3.11 `Atomics.or`.
fn or(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Or)
}

/// §25.4.3.14 `Atomics.sub`.
fn sub(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Sub)
}

/// §25.4.3.15 `Atomics.xor`.
fn xor(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Xor)
}

/// §25.4.3.5 `Atomics.exchange`.
fn exchange(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    modify(vm, heap, call, Operation::Exchange)
}
