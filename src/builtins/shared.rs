//! §25.2's `SharedArrayBuffer` and §25.4's `Atomics`.
//!
//! # What "shared" means
//!
//! Three things. A `SharedArrayBuffer` **cannot be detached** — §25.2 gives it no
//! `[[ArrayBufferDetachKey]]` and no `transfer`, so its bytes are there for as long as anything can
//! name it. It is a different **brand**: `ArrayBuffer.prototype.byteLength` requires an unshared
//! buffer and `SharedArrayBuffer.prototype.byteLength` a shared one, so neither answers about the
//! other however alike the bytes are. And its bytes are a [`Block`](crate::heap::Block): one
//! allocation that **more than one agent** may hold, which is the part that was missing here until
//! a host could start a second agent to hold it with.
//!
//! An engine embedded on its own still runs one agent, and for that one the memory model of
//! §25.4.1 has nothing to order. That never made `Atomics` decorative — the operations have
//! arithmetic and coercion of their own, they refuse the wrong element kinds, and they read and
//! write in ways `ta[i]` does not — but it did make the sharing notional, and it is not any more.
//!
//! # What makes the read-modify-writes atomic, which is not obvious from reading one
//!
//! [`Buffer::modify_bytes`](crate::heap::Buffer::modify_bytes), and nothing else. `add`, `and`,
//! `or`, `sub`, `xor`, `exchange` and `compareExchange` all go through [`read_modify_write`], which
//! reads the slot, decides the new value and writes it back **without letting the block's lock go**.
//! Reading and then writing is indistinguishable from that with one agent and loses updates with
//! two — and what it loses them to is not a wrong answer but a program that never finishes, because
//! test262 waits for agents by spinning until an `Atomics.add` counter reaches them.
//!
//! Every conversion that can run a program's own code happens *before* the section, which is also
//! the order §25.4.3.1's steps 1 to 3 put them in. Anything added here must keep both halves of
//! that: no user code inside, and nothing that reaches the block a second time.
//!
//! # Why `Atomics` accepts an ordinary `ArrayBuffer`
//!
//! Since ES2020 every operation here works on an unshared buffer too — only `wait` requires a
//! shared one. §25.4.3's `ValidateIntegerTypedArray` asks about the *element kind* rather than
//! about sharing, which is the check these actually make.
//!
//! # Waiting, where blocking and not blocking come apart
//!
//! The three waiting operations do not share a fate, and the line between them is whether the agent
//! has to stop.
//!
//! [`wait`] asks §9.7's `[[CanBlock]]` — DoWait step 12's `AgentCanSuspend()` — and that is the
//! **host's** answer rather than this module's. An engine embedded on its own says no: with nothing
//! else running, an agent that suspended could never be woken by anybody, which is why a browser's
//! main thread refuses too and what test262's `CanBlockIsFalse` flag describes. An agent a host
//! *started* says yes, and for it this parks the thread in the block's waiter list until a notify
//! from another agent or the timeout ends it.
//!
//! [`wait_async`] never suspends anything, whatever the host said. The agent parks a promise and
//! carries straight on, so it can reach [`notify`] a statement later and wake its *own* waiter.
//! test262's `undefined-for-timeout.js` is exactly that program.
//!
//! **Which is why there are two waiter lists, and it is worth being precise about the seam.** A
//! blocking waiter is a parked thread and lives in the block, where any agent can take it off the
//! list. An asynchronous one is a promise, and a promise can only be settled by running jobs on the
//! machine that made it — so it lives on that `Vm` and no other agent can reach it. [`notify`]
//! empties both and adds the counts. What it cannot do is interleave them in arrival order, and
//! what it cannot do at all is settle *another* agent's parked promise.
//!
//! **DR-0024's gap has narrowed to the asynchronous half.** A blocking wait times out now, because
//! a parked thread has a clock to be woken by. A `waitAsync` with a finite, non-zero timeout that
//! nothing notifies still stays parked, because settling it needs a job to run at a moment and the
//! queue has no notion of one. A timeout of zero is answered immediately without a promise, and a
//! notify settles a waiter whatever its timeout was, so what is left is that one shape.
//!
//! Around all three sits the part a test mostly measures: the *waitable* kind check ([`Kinds`]),
//! which admits `Int32Array` and `BigInt64Array` alone, the index, and the conversions of the
//! value, the count and the timeout — each of which may run the program's own `valueOf` and must
//! run in the clause's order.

use super::{define_method, define_value, key};
use crate::heap::{
    Element, Heap, Native, NativeCall, Numeric, ObjectId, PropertyDescriptor, View, Wait,
};
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
    super::array_methods::within_budget(vm, heap)?;
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
    let prototype = super::prototype_from(vm, heap, call, Realm::shared_buffer_prototype)?;
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
/// The same arithmetic as §25.1.5.3's, and a different brand at each end: the receiver must be
/// shared, and so must what the species made.
///
/// The one place the two clauses genuinely differ is what they check afterwards. §25.1.5.3 asks
/// whether the new buffer has been **detached**, because constructing it can run a program;
/// §25.2.5.4 has no such step, because a shared buffer cannot be detached at all.
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
    // Step 10 — the *species* constructor, and it is **called**, which a subclass can observe.
    let default = vm.realm().shared_buffer_constructor();
    let species = super::promise::species_of(vm, heap, object, default)?;
    let made = vm.construct_value(Value::Object(species), &[Value::Number(taken as f64)], heap)?;
    // Steps 12 to 15 — three separate refusals, and the third is not symmetrical. What came back
    // must be a buffer and a **shared** one; it must not be the receiver, or the slice would copy
    // a buffer onto itself; and it may be **longer** than asked but never shorter. Only the short
    // case is refused, so a species answering ten bytes for a slice of eight is accepted and the
    // answer is ten bytes long — the clause hands back what the species made, not a trimmed copy.
    let Value::Object(id) = made else {
        return Err(Abrupt::type_error(
            "the species of this SharedArrayBuffer did not make one",
        ));
    };
    let room = match heap.object(id).and_then(crate::heap::Object::buffer) {
        Some(found) if found.shared() => found.byte_length(),
        _ => {
            return Err(Abrupt::type_error(
                "the species of this SharedArrayBuffer did not make one",
            ));
        }
    };
    if id == object {
        return Err(Abrupt::type_error(
            "the species of this SharedArrayBuffer answered the same one",
        ));
    }
    if room < taken {
        return Err(Abrupt::type_error(
            "the species of this SharedArrayBuffer made one too small to hold the slice",
        ));
    }
    // Step 16 — read **after** the species has run, because constructing can run a program and a
    // shared buffer's bytes are the one thing another agent could be writing to meanwhile.
    let bytes = heap
        .object(object)
        .and_then(crate::heap::Object::buffer)
        .map(|buffer| {
            buffer.with_bytes(|found| {
                let Some(found) = found else {
                    return Vec::new();
                };
                let from = (start as usize).min(found.len());
                found[from..(from + taken).min(found.len())].to_vec()
            })
        })
        .unwrap_or_default();
    // Two blocks and never one — step 14 refused a species that answered the receiver — so the
    // write below takes a second lock and the read above has already let go of the first.
    if let Some(buffer) = heap
        .object_mut(id)
        .and_then(crate::heap::Object::buffer_mut)
    {
        buffer.with_bytes_mut(|found| {
            if let Some(target) = found.and_then(|found| found.get_mut(..bytes.len())) {
                target.copy_from_slice(&bytes);
            }
        });
    }
    Ok(made)
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
    // The value is converted *after* the index is validated, and before anything is read: its
    // conversion may run user code, and §25.4.3.1's read-modify-write is a critical section that
    // must have no program in it. Which conversion is step 3's — `ToBigInt` for the two 64-bit
    // kinds and `ToNumber` for the six others, so `Atomics.add(new BigInt64Array(1), 0, 1)` is a
    // TypeError.
    let given = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    let Some(held) = read_modify_write(heap, view, element, at, |held| {
        // Never clamped: §25.4.3.4 refuses a `Uint8ClampedArray` before this is reached, so the
        // question §7.1.11 answers is one no operation here can ask.
        element.write_numeric(&what.apply(held, &given), false)
    }) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    // Every one of them answers the value that *was* there, which is what makes them
    // read-modify-writes rather than writes.
    Ok(heap.numeric_value(held))
}

/// §25.4.1.2 through the buffer: read the slot, decide the new value from it, write it back — and
/// answer what was there — without the lock being let go in between.
///
/// `None` when the buffer has been detached or the slot is not there, which every caller reports as
/// the detachment it is: the index was validated before whatever conversion ran, and a program's
/// own `valueOf` is the only thing that can have taken the bytes away since.
fn read_modify_write(
    heap: &mut Heap,
    view: View,
    element: Element,
    at: usize,
    change: impl FnOnce(&Numeric) -> Option<Vec<u8>>,
) -> Option<Numeric> {
    let offset = view.offset + at * element.width();
    let buffer = heap
        .object_mut(view.buffer)
        .and_then(crate::heap::Object::buffer_mut)?;
    let held = buffer.modify_bytes(offset, element.width(), |held| change(&element.read(held)))?;
    Some(element.read(&held))
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
    // §25.4.3.3 step 9 — the comparison is against the expected value **as this kind would store
    // it**, not against what was handed over: expecting 300 of a `Uint8Array` holding 44 is a
    // match, because 300 stored there *is* 44. Comparing the raw arguments would never match and
    // the write would never happen.
    //
    // One round trip and not two. What is held came out of the slot and re-encoding it could only
    // give the bytes it was already read from, so the two sides are not symmetrical however alike
    // they look — and a second encoding is a second place for §7.1.11's clamping to be asked for,
    // where no answer to it is observable.
    //
    // Encoded **before** the critical section, because it is the same for every slot and the
    // section is one no more work belongs in than has to be there.
    let stored = element
        .write_numeric(&expected, false)
        .map(|bytes| element.read(&bytes));
    // The comparison and the write are one step, which is the whole of what makes this a
    // *compare*-exchange: read separately, two agents can both find the expected value and both
    // write, and each is told it won.
    let Some(held) = read_modify_write(heap, view, element, at, |held| {
        match stored.as_ref() == Some(held) {
            true => element.write_numeric(&replacement, false),
            false => None,
        }
    }) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    Ok(heap.numeric_value(held))
}

/// §25.4.3.7 `Atomics.notify(typedArray, index, count)`.
///
/// Wakes the agent's **own** waiters, which is not the contradiction it sounds like: a
/// [`wait_async`] does not block, so the agent that parked one goes on running and reaches this.
/// Step 8 settles each woken promise with `"ok"` and the answer is how many were woken.
///
/// **The non-shared case is a `0` and not an error**, which is where this parts company with
/// [`wait`]: step 7 returns `+0` for an ordinary `ArrayBuffer` — nothing can be waiting on a
/// buffer no one else can reach. Refusing it would be a stricter engine, not a more correct one.
///
/// The waiters are settled **after** the list has been emptied, and that ordering is load-bearing:
/// resolving a promise runs the program's jobs eventually, and a `then` that calls `notify` again
/// must not find the waiter this call has already claimed.
fn notify(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, element, at) = target(vm, heap, call, Kinds::Waitable)?;
    // Step 3 — a missing count is **+∞** and not `ToIntegerOrInfinity(undefined)`'s zero, which is
    // the one argument here where absent and zero must not agree: `Atomics.notify(a, 0)` wakes
    // everything, and reading it as 0 would wake nothing and answer 0 while looking right.
    // `ToNumber(undefined)` is `NaN` and `to_integer_or_infinity` maps that to 0, so the case has
    // to be written out — and it is now observable, where before nothing was ever woken.
    let count = match call.argument(2) {
        Value::Undefined => f64::INFINITY,
        given => {
            let number = vm.to_number(given, heap)?;
            super::string::to_integer_or_infinity(number).max(0.0)
        }
    };
    // Step 7 — a buffer nobody else can reach can have nothing waiting on it. Asked after the
    // count, because the count's conversion is a step the program can see and this is not an error.
    let Some(block) = heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::block)
        .cloned()
    else {
        return Ok(Value::Number(0.0));
    };
    let byte = view.offset + at * element.width();
    // Both halves of §25.4.1's list, and the parked threads go first because they are the ones
    // another agent is waiting on: a `notify(…, 1)` that spent its one wake on a promise this agent
    // will settle at its leisure, while a thread in another agent stayed parked, would be a count
    // that is right and a program that hangs.
    let parked = block.notify(byte, count_of(count));
    let woken = vm.take_waiters(&block, byte, count - parked as f64);
    let total = parked + woken.len();
    let ok = super::text(heap, "ok");
    for capability in woken {
        vm.settle_capability(capability, crate::heap::ReactionKind::Fulfil, ok, heap)?;
    }
    Ok(Value::Number(total as f64))
}

/// §25.4.3.7's count as a number of waiters, which is a saturation and not a conversion.
///
/// The count is an `f64` because step 3 makes a missing one **+∞**, and `usize` has no such value —
/// so "all of them" becomes "more than any list will hold". `as` saturates rather than wrapping,
/// which is what makes that true rather than merely likely, and the caller has already clamped the
/// value to zero or above so there is no negative to consider.
fn count_of(count: f64) -> usize {
    count as usize
}

/// §25.4.3.15 `Atomics.waitAsync(typedArray, index, value, timeout)` — DoWait in `async` mode.
///
/// The half of DoWait that [`wait`] cannot reach. It never suspends the agent, so
/// `AgentCanSuspend()` is not asked and this works here where the blocking form cannot: the agent
/// carries on and may wake its own waiter with [`notify`] a statement later, which is exactly what
/// test262's `undefined-for-timeout.js` does.
///
/// Three answers, and the shape says which. Two of them settle **before returning** and are handed
/// back as a plain String with `async: false` — the value having changed already, and a timeout of
/// zero — because there is nothing left to wait for and a promise would only delay a known answer
/// by a turn. The third parks and answers `async: true` with the promise.
///
/// **A finite timeout does elapse**, which it did not when DR-0024 was written. The deadline goes
/// on the waiter and [`Vm::expire_waiters`] settles it `"timed-out"` at the next job boundary after
/// it passes — see that method and [`Vm::drain_jobs`] for why a job boundary is where a clause that
/// says *in parallel* lands in an engine that has no parallel. What is still missing is a notify
/// from **another** agent reaching a promise parked here; that one is not a timer and DR-0024's
/// second seam still names it.
fn wait_async(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, element, at) = target(vm, heap, call, Kinds::SharedWaitable)?;
    let expected = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    // Step 6's `ToNumber`, and NaN is +∞ rather than zero — the reverse of what `ToIndex` gives,
    // and why `waitAsync(a, 0, 0)` waits rather than answering at once.
    let timeout = vm.to_number(call.argument(3), heap)?;
    let timeout = match timeout.is_nan() {
        true => f64::INFINITY,
        false => timeout.max(0.0),
    };
    let Some(held) = heap.numeric_at(view, at) else {
        return Err(Abrupt::type_error("this ArrayBuffer has been detached"));
    };
    // Step 17 — the comparison is against the value as the element kind *stores* it, the same rule
    // `compareExchange` makes, so an expectation past the width matches the bits it would land as.
    let stored = element
        .write_numeric(&expected, false)
        .map(|bytes| element.read(&bytes));
    let settled = match stored.as_ref() == Some(&held) {
        false => Some("not-equal"),
        true if timeout == 0.0 => Some("timed-out"),
        true => None,
    };
    let outcome = heap.new_object(Some(vm.realm().object_prototype()));
    let (asynchronous, value) = match settled {
        Some(answer) => (false, super::text(heap, answer)),
        None => {
            let Some(block) = heap
                .object(view.buffer)
                .and_then(crate::heap::Object::buffer)
                .and_then(crate::heap::Buffer::block)
                .cloned()
            else {
                return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
            };
            let capability = vm.intrinsic_capability(heap);
            let promise = capability.promise;
            vm.park_waiter(
                block,
                view.offset + at * element.width(),
                capability,
                duration_of(timeout),
            );
            (true, promise)
        }
    };
    super::create_data_property(heap, outcome, "async", Value::Boolean(asynchronous));
    super::create_data_property(heap, outcome, "value", value);
    Ok(Value::Object(outcome))
}

/// §25.4.3.14 `Atomics.wait(typedArray, index, value, timeout)` — DoWait in synchronous mode.
///
/// **Whether this throws is the host's answer and not the engine's.** DoWait step 12 asks
/// `AgentCanSuspend()`, which is §9.7's `[[CanBlock]]`, and an engine embedded on its own answers
/// `false`: with nothing else running, an agent that suspended here could never be woken. A
/// browser's main thread refuses for the same reason, and that is what test262's `CanBlockIsFalse`
/// flag describes. A host that starts other agents turns [`Vm::set_can_block`] on for the ones it
/// starts, and for those this parks the thread until a notify or the timeout ends it.
///
/// **The four arguments are converted first either way**, and that is most of what a test can see.
/// Steps 1 to 11 validate the array, the index, the value and the timeout in that order, so
/// `Atomics.wait(new Float64Array(4), 0, 0, 0)` is refused for its *kind* and never reaches the
/// suspend check, while `Atomics.wait(i32a, 0, 0, {valueOf(){ throw new Error() }})` throws the
/// program's own error rather than this one.
///
/// The comparison against `value` is **not** made here. It belongs inside the block's critical
/// section — see [`Block::wait`] — because reading the slot and joining the waiter list have to be
/// one step: between them, another agent could store the new value and notify, and this agent would
/// then park waiting for a wake that had already happened.
fn wait(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let (view, element, at) = target(vm, heap, call, Kinds::SharedWaitable)?;
    // Step 10 — the same `ToBigInt`/`ToNumber` split the arithmetic operations make, for the same
    // reason: the comparison is against a stored bit pattern, so the value has to become one.
    let expected = vm.to_numeric(element.holds_big(), call.argument(2), heap)?;
    // Step 11 — `ToNumber` and not `ToIndex`: a timeout is a duration and `-1` is not an error, it
    // is `max(q, 0)`. NaN is +∞ rather than 0, which is the reverse of what `ToIndex` would give
    // and is why `Atomics.wait(i32a, 0, 0, undefined)` waits for ever rather than not at all.
    let timeout = vm.to_number(call.argument(3), heap)?;
    // Step 12, and it is asked **after** both conversions, so a poisoned `valueOf` on either throws
    // the program's own error on a host that refuses to block as well as on one that does not.
    if !vm.can_block() {
        return Err(Abrupt::type_error("this agent cannot be suspended"));
    }
    let Some(block) = heap
        .object(view.buffer)
        .and_then(crate::heap::Object::buffer)
        .and_then(crate::heap::Buffer::block)
        .cloned()
    else {
        return Err(Abrupt::type_error("this is not a SharedArrayBuffer"));
    };
    // The same encoding step 17 makes for `waitAsync`: the comparison is against the value as this
    // element kind *stores* it, so an expectation past the width matches the bits it would land as.
    // The `Option` cannot be an absence, because the conversion above was chosen by this same kind
    // — and were it ever one, a value no slot can hold is a value the slot does not hold, which
    // ends the wait at once rather than parking a thread on a comparison that can never be made.
    let outcome = match element.write_numeric(&expected, false) {
        None => Wait::NotEqual,
        Some(expected) => block.wait(
            view.offset + at * element.width(),
            &expected,
            duration_of(timeout),
        ),
    };
    Ok(super::text(heap, outcome.name()))
}

/// §25.4.3.14 step 11's timeout as a duration — `None` for the infinite wait.
///
/// NaN and infinity are the same answer here — step 11 maps NaN to `+∞` — and a negative timeout is
/// zero, because `t` is `max(q, 0)`. What is left is a finite non-negative number of milliseconds,
/// and building a duration from one fails only when it is too large to represent. A wait longer than
/// the clock can name is the infinite one rather than an error, so that failure is `None` too.
fn duration_of(timeout: f64) -> Option<std::time::Duration> {
    match timeout.is_nan() || timeout.is_infinite() {
        true => None,
        false => std::time::Duration::try_from_secs_f64(timeout.max(0.0) / 1000.0).ok(),
    }
}

/// §25.4.3.8 `Atomics.isLockFree`.
///
/// Answers about a *width* rather than about a buffer. Only `4` is settled by the clause — step 5
/// says `true` outright — and 1, 2 and 8 are the Agent Record's `[[IsLockFree1]]`, `[[IsLockFree2]]`
/// and `[[IsLockFree8]]`, which a host chooses. The one rule about those is that the answer may not
/// *change*: once any agent has observed one, every agent sees the same for ever.
///
/// **This doc said "ViperJS has one agent, so every width it supports is lock-free in the only
/// sense the question has", and that stopped being true when there was more than one agent.** A
/// block's operations go through a mutex now, so nothing here is lock-free in the sense §25.4.3.8's
/// note means by it — "as fast as ordinary memory access". The answer is unchanged all the same,
/// and deliberately: this is an *optimisation* primitive, a program reads it to decide whether to
/// use `Atomics` or build a lock of its own out of them, and `false` would send it round a longer
/// way to reach the same mutex. `4` would have to be `true` regardless.
fn is_lock_free(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let width = vm.to_number(call.argument(0), heap)?;
    Ok(Value::Boolean(matches!(width as i64, 1 | 2 | 4 | 8)))
}

/// Build §25.2's `SharedArrayBuffer` and §25.4's `Atomics` onto the global.
pub(super) fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.shared_buffer_prototype();
    let constructor =
        heap.new_native_constructor(realm.function_prototype(), construct, realm.id());
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
        let getter = heap.new_native_function(realm.function_prototype(), native, realm.id());
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
    super::collection::tag_with(heap, prototype, "SharedArrayBuffer");

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
        ("waitAsync", 4, wait_async),
        ("xor", 3, xor),
    ] {
        define_method(heap, realm, atomics, name, length, native);
    }
    super::collection::tag_with(heap, atomics, "Atomics");
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
