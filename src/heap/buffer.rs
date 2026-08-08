//! §25.1 — an `ArrayBuffer`'s bytes, and what it means for one to be detached.
//!
//! # Why the bytes are a `Vec<u8>` and not a `Vec<Value>`
//!
//! Because that is what the specification says they are. §25.1.3.1's data block is a sequence of
//! *bytes*, and everything above it — every TypedArray, every `DataView` — is a way of reading and
//! writing those bytes with a type imposed on the way past. A buffer holds no JavaScript values at
//! all, which is why it needs nothing from the collector: there is nothing in it to keep alive.
//!
//! # Detaching
//!
//! §25.1.3.3 `DetachArrayBuffer` throws the bytes away and leaves the object behind. Every view
//! onto it then throws on every access, and `byteLength` answers 0. It is not a way of freeing
//! memory that a program can rely on; it is what `transfer` and the host's own APIs do when the
//! bytes have gone somewhere else, and it is what makes "is this buffer still usable" a question a
//! specification has to ask before every read.
//!
//! `None` is detached and `Some(vec![])` is an empty buffer that is perfectly usable. The two are
//! different in every way that matters and identical in `byteLength`, which is exactly the sort of
//! distinction an `Option` is for.
//!
//! # Why a shared buffer's bytes are not a `Vec` at all
//!
//! Because §25.2 exists so that **more than one agent** can read and write the same memory, and two
//! agents are two threads with a heap each. A `Vec<u8>` sitting inside one heap's [`Buffer`] is
//! reachable from that agent and no other, so a `SharedArrayBuffer` built over it would be shared in
//! name only — which is what it was here until agents existed to share it with.
//!
//! So the bytes of a shared buffer live in a [`Block`]: one allocation behind an `Arc`, which any
//! number of [`Buffer`]s in any number of heaps may hold. Each agent still has its own object, its
//! own prototype and its own brand; what they have in common is the block underneath.
//!
//! **The lock covers the bytes and §25.4.1's waiter list together, and that is not tidiness.** A
//! blocking `Atomics.wait` compares the slot against a value and *then* joins the list, and if
//! another agent's store and notify could land between those two steps the waiter would park after
//! the wake it was waiting for and never be woken. One mutex over both makes the compare and the
//! enqueue a single critical section, which is exactly what §25.4.1 asks for. An ordinary
//! `ArrayBuffer` takes no lock at all, because there is nobody to take it against.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The bytes of a §25.2 `SharedArrayBuffer`, which more than one agent may hold.
///
/// Cloning one is an `Arc` bump: the clone names the *same* memory, which is the whole point. Two
/// blocks are the same block when [`Block::is`] says so, and no two allocations ever compare equal
/// however alike their bytes are — a buffer's identity is where it is, not what is in it.
#[derive(Debug, Clone)]
pub struct Block(Arc<Shared>);

/// What every holder of a [`Block`] shares.
#[derive(Debug)]
struct Shared {
    /// The bytes, the ceiling they may grow to, and the waiters — under one lock. See the module
    /// documentation for why those are not three locks.
    state: Mutex<State>,
    /// What a blocking wait parks on and [`Block::notify`] signals.
    ///
    /// One condition variable for the whole block rather than one per position. A notify wakes
    /// every parked thread and each re-checks whether *it* was the one taken off the list, which is
    /// what the loop in [`Block::wait`] is for — a spurious wake and a wake meant for another
    /// position are the same thing to a waiter, and both are handled by looking rather than by
    /// trusting that being woken means anything.
    woken: Condvar,
}

/// Everything about a block that changes, which is everything the lock protects.
#[derive(Debug)]
struct State {
    /// The bytes themselves. Never `None`: §25.2 gives a shared buffer no way to be detached.
    bytes: Vec<u8>,
    /// `[[ArrayBufferMaxByteLength]]`, kept here rather than on the [`Buffer`] because growing is
    /// something *one* agent does and every other agent must then see. A copy per holder would let
    /// two agents disagree about how large the same allocation is allowed to get.
    max_byte_length: Option<usize>,
    /// §25.4.1's list of agents parked on a position in this block, in the order they arrived.
    waiters: Vec<Waiter>,
    /// What the next waiter will be called.
    ///
    /// A waiter has to be able to ask "was *I* taken off the list", and its position is no answer:
    /// the list shifts as other waiters are woken. A number handed out once is.
    next: u64,
}

/// One agent parked in §25.4.1's waiter list.
#[derive(Debug)]
struct Waiter {
    /// Which **byte** offset into the block it is waiting on, per §25.4.1.
    ///
    /// A byte offset and not an element index, because a `BigInt64Array`'s slot 0 and an
    /// `Int32Array`'s slot 0 are one position in the block and an index would make them two.
    offset: usize,
    /// What this waiter is called, so that it can recognise its own removal.
    id: u64,
}

/// How a blocking `Atomics.wait` ended — §25.4.3.14's three answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The slot did not hold what the caller expected, so nothing was waited for at all.
    NotEqual,
    /// A notify took this waiter off the list.
    Ok,
    /// The timeout elapsed with nobody notifying.
    TimedOut,
}

impl Wait {
    /// The string §25.4.3.14 answers with, which is the only thing a program sees of this.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::NotEqual => "not-equal",
            Self::Ok => "ok",
            Self::TimedOut => "timed-out",
        }
    }
}

impl Block {
    /// A block of `length` **zeroed** bytes.
    #[must_use]
    pub fn new(length: usize) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                bytes: vec![0; length],
                max_byte_length: None,
                waiters: Vec::new(),
                next: 0,
            }),
            woken: Condvar::new(),
        }))
    }

    /// Whether these two name the same allocation.
    ///
    /// The question `SharedArrayBuffer.prototype.slice` asks about its species and the one a host
    /// asks before handing a received block back to the agent it came from.
    #[must_use]
    pub fn is(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The lock, taking a poisoned one at its word.
    ///
    /// A `Mutex` is poisoned only by a thread panicking while it holds it, and the third ratchet
    /// says no input panics — so poisoning here would mean an engine bug that has already happened
    /// elsewhere. There is nothing a caller could do about it and no JavaScript error that would
    /// describe it, so the bytes are taken as they are rather than turning one bug into a second,
    /// stranger one. `unwrap()` would be a panic on a path that exists to avoid panicking.
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the bytes under the lock.
    fn with_bytes<R>(&self, read: impl FnOnce(&[u8]) -> R) -> R {
        read(&self.state().bytes)
    }

    /// Write them under the same lock.
    fn with_bytes_mut<R>(&self, write: impl FnOnce(&mut [u8]) -> R) -> R {
        write(&mut self.state().bytes)
    }

    /// How many bytes it holds at this moment — another agent may have grown it a moment ago.
    fn byte_length(&self) -> usize {
        self.state().bytes.len()
    }

    /// `[[ArrayBufferMaxByteLength]]`.
    fn max_byte_length(&self) -> Option<usize> {
        self.state().max_byte_length
    }

    /// Note that this block may grow to `max`.
    fn allow_resizing_to(&self, max: usize) {
        self.state().max_byte_length = Some(max);
    }

    /// §25.2.5.4 `grow`, once its caller has done the refusing. See [`Buffer::resize`].
    fn resize(&self, length: usize) -> bool {
        let mut state = self.state();
        if state.max_byte_length.is_none_or(|max| length > max) {
            return false;
        }
        state.bytes.resize(length, 0);
        true
    }

    /// §25.4.1's `DoWait` in synchronous mode, for an agent whose `[[CanBlock]]` is **true**.
    ///
    /// **The comparison is part of the wait and not a step before it.** `expected` is the value the
    /// caller wants the slot to still hold, already encoded as the element kind would store it, and
    /// it is compared here — inside the critical section that the enqueue also happens in. Reading
    /// the slot first and calling this afterwards would leave a window in which another agent
    /// stores the new value and notifies, and this agent then parks waiting for a wake that has
    /// already happened.
    ///
    /// `timeout` of `None` is §25.4.3.14's `+∞`: wait until notified, however long that is. A
    /// timeout that has already elapsed answers [`Wait::TimedOut`] without joining the list, which
    /// is why a zero timeout never blocks.
    ///
    /// An offset with fewer than `expected.len()` bytes after it answers [`Wait::NotEqual`] rather
    /// than waiting. No caller can present one — every one of them has validated the index against
    /// the view first — and of the two harmless answers this is the one that cannot park a thread
    /// on a position that does not exist.
    pub fn wait(&self, offset: usize, expected: &[u8], timeout: Option<Duration>) -> Wait {
        let mut state = self.state();
        if state.bytes.get(offset..offset + expected.len()) != Some(expected) {
            return Wait::NotEqual;
        }
        // A timeout so large that the clock cannot name the moment it ends is one no program will
        // outlive, so `checked_add` declining makes it the infinite wait rather than an error.
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        state.waiters.push(Waiter { offset, id });
        loop {
            // Being woken means nothing on its own — a condition variable may wake a thread for no
            // reason at all, and a notify wakes every waiter on the block whatever position it
            // named. Having been *taken off the list* is the fact, and it is asked for rather than
            // inferred.
            if !state.waiters.iter().any(|waiting| waiting.id == id) {
                return Wait::Ok;
            }
            let left = match deadline {
                None => None,
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) => Some(left),
                    None => {
                        // Still on the list at the deadline, so nobody is going to take it off.
                        state.waiters.retain(|waiting| waiting.id != id);
                        return Wait::TimedOut;
                    }
                },
            };
            // The two spellings answer differently poisoned — one a guard and one a guard beside a
            // "did it time out" — and neither answer is consulted: whether the deadline has passed
            // is asked of the clock at the top of the loop, so a timed-out flag would be a second
            // source for a fact this already has one of.
            state = match left {
                None => self
                    .0
                    .woken
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(left) => {
                    self.0
                        .woken
                        .wait_timeout(state, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    /// §25.4.3.7's half of `Atomics.notify` that the block owns — take up to `count` waiters off
    /// the list at `offset` and wake them, answering how many there were.
    ///
    /// In list order, which is §25.4.1's: the waiters that have been parked longest go first, so
    /// `notify(…, 1)` twice wakes two different agents rather than racing over one.
    ///
    /// The condition variable is signalled once for the whole batch and **every** parked thread on
    /// the block wakes to re-check, whatever position it named. One variable per position would
    /// wake fewer of them and would be a map to keep in step with the list; the waiters that were
    /// not meant find themselves still on it and park again, which is the same loop that already
    /// has to handle a spurious wake.
    ///
    /// And it is signalled whether or not anything was taken off, which is not an oversight: a wake
    /// that nobody was waiting for is what a condition variable is *defined* to be allowed to do, so
    /// a guard around this would be a branch no program could tell from its absence — and mutation
    /// coverage said exactly that, twice, before it went.
    pub fn notify(&self, offset: usize, count: usize) -> usize {
        let mut state = self.state();
        let mut woken = 0;
        state.waiters.retain(|waiting| {
            if waiting.offset == offset && woken < count {
                woken += 1;
                return false;
            }
            true
        });
        self.0.woken.notify_all();
        woken
    }
}

/// §25.1.3.1's data block — the bytes, or nothing if the buffer has been detached.
#[derive(Debug)]
pub struct Buffer {
    /// `[[ArrayBufferData]]`, in whichever of the two ways this buffer holds it.
    bytes: Bytes,
    /// `[[ArrayBufferMaxByteLength]]` for an **unshared** buffer — present exactly when it is
    /// resizable. A shared one keeps it on the block, where every agent sees the same answer.
    ///
    /// §25.1.6.4 and §25.2.5.4 both spell their first step `RequireInternalSlot(O,
    /// [[ArrayBufferMaxByteLength]])`, so "may this be resized" is not a flag beside a number: it
    /// *is* whether the number is there. An `Option` says that, and a `bool` next to a `usize`
    /// would let the two disagree.
    max_byte_length: Option<usize>,
}

/// Where a buffer's bytes are, which is the whole of the difference between §25.1 and §25.2.
///
/// Two variants rather than a `shared` flag beside an `Option<Vec<u8>>`, because the flag could
/// disagree with the storage and this cannot: a shared buffer has a [`Block`] and therefore no
/// detached state to represent, and an unshared one has bytes only this agent can reach and
/// therefore no lock to take. `transfer` refuses a shared buffer outright, because §25.2 has no
/// `[[ArrayBufferDetachKey]]` at all.
#[derive(Debug)]
enum Bytes {
    /// §25.1 — this agent's own bytes, and `None` once `DetachArrayBuffer` has done its work.
    Own(Option<Vec<u8>>),
    /// §25.2 — a block that any number of agents may hold, and that none of them can detach.
    Shared(Block),
}

impl Buffer {
    /// A buffer of `length` **zeroed** bytes — §25.1.3.1 step 2.
    ///
    /// Zeroed and not uninitialised, which is not an implementation detail: a program can read
    /// every byte of a fresh buffer and the answer has to be 0. Anything else would hand it
    /// whatever was in that memory before.
    #[must_use]
    pub fn new(length: usize) -> Self {
        Self {
            bytes: Bytes::Own(Some(vec![0; length])),
            max_byte_length: None,
        }
    }

    /// The same, as §25.2.2.1's `SharedArrayBuffer` — bytes that nothing can take away.
    #[must_use]
    pub fn new_shared(length: usize) -> Self {
        Self::over(&Block::new(length))
    }

    /// A `SharedArrayBuffer` over a block that already exists — what a **second agent** gets.
    ///
    /// The other half of §25.2's reason for existing: `$262.agent.broadcast` hands a block from one
    /// agent to another, and this is how the receiving agent's heap grows an object over it. The
    /// bytes are not allocated and not charged to the receiving heap's budget, because they were
    /// already allocated and already charged where they were made — a second name for one
    /// allocation is not a second allocation.
    #[must_use]
    pub fn over(block: &Block) -> Self {
        Self {
            bytes: Bytes::Shared(block.clone()),
            max_byte_length: None,
        }
    }

    /// Note that this buffer may be resized up to `max` — §25.1.3.1's `maxByteLength` option.
    ///
    /// The capacity is **not** reserved. §25.1.3.1 lets an implementation either allocate the
    /// maximum up front or grow on demand, and growing on demand is what DR-0013's budget wants:
    /// `new ArrayBuffer(0, { maxByteLength: 2 ** 30 })` is a line a program may write and reserving
    /// a gibibyte for it would refuse the program for memory it has not asked to use yet.
    pub fn allow_resizing_to(&mut self, max: usize) {
        match &self.bytes {
            Bytes::Own(_) => self.max_byte_length = Some(max),
            Bytes::Shared(block) => block.allow_resizing_to(max),
        }
    }

    /// `[[ArrayBufferMaxByteLength]]`, or `None` for a buffer fixed at its length.
    #[must_use]
    pub fn max_byte_length(&self) -> Option<usize> {
        match &self.bytes {
            Bytes::Own(_) => self.max_byte_length,
            Bytes::Shared(block) => block.max_byte_length(),
        }
    }

    /// §25.1.6.4 `resize` and §25.2.5.4 `grow`, once their callers have done the refusing.
    ///
    /// Answers whether the length was allowed — `false` for a buffer that was never resizable and
    /// for a length past its maximum, which are a TypeError and a RangeError at the two call sites.
    ///
    /// A **detached** buffer is not one of the answers, and deliberately: both callers refuse one
    /// before they get here (§25.1.6.4 step 4), and §25.2 has no way to detach at all, so a check
    /// here would be a branch no input could reach. What it does instead is nothing — the bytes are
    /// gone, so there is no `Vec` to resize and none is made, which is the one behaviour that
    /// cannot resurrect a detached buffer however carelessly this is called.
    ///
    /// Growth is zeroed, on §25.1.3.1's grounds: bytes a program can read must never be whatever
    /// was in that memory before. Shrinking drops the tail, and re-growing gives zeroes rather than
    /// what used to be there — `Vec::resize` in both directions says exactly that.
    pub fn resize(&mut self, length: usize) -> bool {
        match &mut self.bytes {
            Bytes::Shared(block) => block.resize(length),
            Bytes::Own(bytes) => {
                if self.max_byte_length.is_none_or(|max| length > max) {
                    return false;
                }
                if let Some(bytes) = bytes.as_mut() {
                    bytes.resize(length, 0);
                }
                true
            }
        }
    }

    /// Whether this is a `SharedArrayBuffer`.
    #[must_use]
    pub fn shared(&self) -> bool {
        matches!(self.bytes, Bytes::Shared(_))
    }

    /// The block these bytes are, if this buffer is a shared one.
    ///
    /// What a host needs to hand the same memory to another agent, and what §25.4's waiting and
    /// notifying are addressed to. `None` for an ordinary `ArrayBuffer`, which has no block because
    /// there is nobody to share it with.
    #[must_use]
    pub fn block(&self) -> Option<&Block> {
        match &self.bytes {
            Bytes::Own(_) => None,
            Bytes::Shared(block) => Some(block),
        }
    }

    /// Read the bytes — `None` once the buffer has been detached.
    ///
    /// A closure rather than a borrow, because a shared buffer's bytes are behind a lock and a
    /// `&[u8]` handed out would be one taken for as long as the caller kept it. The closure is the
    /// critical section, which is also the shape that makes it obvious a caller must not reach for
    /// the same block inside one.
    pub fn with_bytes<R>(&self, read: impl FnOnce(Option<&[u8]>) -> R) -> R {
        match &self.bytes {
            Bytes::Own(bytes) => read(bytes.as_deref()),
            Bytes::Shared(block) => block.with_bytes(|bytes| read(Some(bytes))),
        }
    }

    /// The same, to write through.
    pub fn with_bytes_mut<R>(&mut self, write: impl FnOnce(Option<&mut [u8]>) -> R) -> R {
        match &mut self.bytes {
            Bytes::Own(bytes) => write(bytes.as_deref_mut()),
            Bytes::Shared(block) => block.with_bytes_mut(|bytes| write(Some(bytes))),
        }
    }

    /// §25.4.1.2's read-modify-write: read `width` bytes at `offset`, hand them to `change`, and
    /// write back what it answers — **all inside one critical section**.
    ///
    /// Answers the bytes that *were* there, which is what every one of §25.4.3's arithmetic
    /// operations returns; `change` answering `None` leaves them alone, which is what
    /// `compareExchange` does when the slot does not hold what was expected.
    ///
    /// **This is what makes those operations atomic, and reading and then writing is not.** With one
    /// agent the two are indistinguishable, so this looked like a refactor right up until there was
    /// a second agent: `Atomics.add(i32a, 0, 1)` called by three agents through a read and a write
    /// loses updates, and test262's agent tests are built on exactly that counter —
    /// `atomicsHelper.js`'s `waitUntil` spins until it reaches the number of agents, so a lost
    /// update is not a wrong answer but a test that never finishes.
    ///
    /// `change` runs with the lock held, so it must not touch this block again. Every caller hands
    /// it a pure function of the bytes: [`Element::read`], some arithmetic, [`Element::write_numeric`].
    /// The **conversions** that can run a program's own code happen before this is called, which is
    /// also what §25.4.3.1's step order asks for.
    pub fn modify_bytes(
        &mut self,
        offset: usize,
        width: usize,
        change: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        self.with_bytes_mut(|bytes| {
            let slot = bytes?.get_mut(offset..offset + width)?;
            let held = slot.to_vec();
            if let Some(written) = change(&held)
                && let Some(target) = slot.get_mut(..written.len())
            {
                target.copy_from_slice(&written);
            }
            Some(held)
        })
    }

    /// `[[ArrayBufferByteLength]]` — **0** for a detached buffer, which §25.1.5.1 is explicit about.
    #[must_use]
    pub fn byte_length(&self) -> usize {
        match &self.bytes {
            Bytes::Own(bytes) => bytes.as_ref().map_or(0, Vec::len),
            Bytes::Shared(block) => block.byte_length(),
        }
    }

    /// Whether the bytes have gone — §25.1.3.2 `IsDetachedBuffer`.
    #[must_use]
    pub fn detached(&self) -> bool {
        matches!(self.bytes, Bytes::Own(None))
    }

    /// §25.1.3.3 `DetachArrayBuffer` — throw the bytes away and leave the object.
    ///
    /// Does nothing to a shared buffer, and that is §25.2 rather than an oversight: it has no
    /// `[[ArrayBufferDetachKey]]`, so there is no operation that could ask for this. Every caller
    /// refuses a shared buffer before it gets here, and taking the bytes away from one agent while
    /// another was reading them is precisely what a block exists to make impossible.
    pub fn detach(&mut self) {
        if let Bytes::Own(bytes) = &mut self.bytes {
            *bytes = None;
        }
    }
}

/// The ten numeric types §25.1.3.1's bytes can be read as.
///
/// Shared by §25.3's `DataView`, which asks for one per access, and by §23.2's TypedArrays, which
/// each have one for their whole length. That is the only difference between the two: a `DataView`
/// is a window with no type and a TypedArray is a window with one.
///
/// # Eight of them hold a Number and two hold a BigInt
///
/// §23.2.1 calls that a `[[ContentType]]`, and it is the one thing about a kind that a program
/// cannot coerce its way past: writing `1` to a `BigInt64Array` is a TypeError rather than a
/// conversion, because §6.1.6 gives Number and BigInt no arithmetic in common and an implicit
/// crossing between them would be exactly the silent precision loss the type exists to prevent.
/// [`Element::holds_big`] is that question, and every read and write below is decided by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    /// A signed byte.
    Int8,
    /// An unsigned byte.
    Uint8,
    /// A signed 16-bit integer.
    Int16,
    /// An unsigned 16-bit integer.
    Uint16,
    /// A signed 32-bit integer.
    Int32,
    /// An unsigned 32-bit integer.
    Uint32,
    /// A signed 64-bit integer, read and written as a BigInt.
    BigInt64,
    /// An unsigned 64-bit integer, read and written as a BigInt.
    BigUint64,
    /// An IEEE single.
    Float32,
    /// An IEEE double.
    Float64,
}

/// §6.1.6's two numeric types, as whichever one a slot holds.
///
/// The value a read of an [`Element`] answers and a write of one is given, and it exists because
/// those two are no longer both Numbers. Not a [`Value`](crate::value::Value): a BigInt in the heap
/// is an identifier, and the bytes of a buffer can be read without one — which is what lets
/// [`Element::read`] stay a function of the bytes alone and leaves the allocation to whoever wanted
/// a JavaScript value out of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Numeric {
    /// §6.1.6.1's Number, which eight of the ten kinds hold.
    Number(f64),
    /// §6.1.6.2's BigInt, which the two 64-bit integer kinds hold.
    BigInt(crate::bigint::BigInt),
}

impl Element {
    /// How many bytes one of these takes — §25.3.1.1's element size.
    #[must_use]
    pub fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::BigInt64 | Self::BigUint64 | Self::Float64 => 8,
        }
    }

    /// §23.2.1's `[[ContentType]]` — whether a slot of this kind holds a BigInt rather than a Number.
    ///
    /// The question every write asks first, because it chooses the conversion: §7.1.13 `ToBigInt`
    /// for the two, §7.1.4 `ToNumber` for the eight. It is also what §23.2.3.24's `set` and
    /// §23.2.5.1.2's copy-construction compare between two arrays, since bytes can be copied between
    /// kinds of one content type and never between the two.
    #[must_use]
    pub fn holds_big(self) -> bool {
        matches!(self, Self::BigInt64 | Self::BigUint64)
    }

    /// The name §25.3.4 gives the pair of methods that read and write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8",
            Self::Uint8 => "Uint8",
            Self::Int16 => "Int16",
            Self::Uint16 => "Uint16",
            Self::Int32 => "Int32",
            Self::Uint32 => "Uint32",
            Self::BigInt64 => "BigInt64",
            Self::BigUint64 => "BigUint64",
            Self::Float32 => "Float32",
            Self::Float64 => "Float64",
        }
    }

    /// §25.3.1.3 `RawBytesToNumeric` — the bytes, already in reading order, as a numeric value.
    ///
    /// Fewer bytes than the kind is wide reads the rest as zero rather than refusing, because every
    /// caller has already checked the bounds and a second refusal here would be one nothing could
    /// reach.
    #[must_use]
    pub fn read(self, bytes: &[u8]) -> Numeric {
        let mut eight = [0_u8; 8];
        eight[..bytes.len()].copy_from_slice(bytes);
        let number = match self {
            Self::Int8 => f64::from(eight[0] as i8),
            Self::Uint8 => f64::from(eight[0]),
            Self::Int16 => f64::from(i16::from_le_bytes([eight[0], eight[1]])),
            Self::Uint16 => f64::from(u16::from_le_bytes([eight[0], eight[1]])),
            Self::Int32 => f64::from(i32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            Self::Uint32 => f64::from(u32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            // §25.3.1.3 step 5 — the same eight bytes are two different BigInts depending on
            // whether the top bit is a sign, and that is the whole of the difference between the
            // two kinds. Nothing else about them differs, not even the write.
            Self::BigInt64 | Self::BigUint64 => {
                let bits = u64::from_le_bytes(eight);
                return Numeric::BigInt(crate::bigint::BigInt::from_bits(
                    bits,
                    self == Self::BigInt64,
                ));
            }
            // §6.1.6.1's Number is a double, so a float32 widens on the way out. Every float32 is
            // exactly representable as a double, so nothing is lost — but the *value* is the
            // rounded one, which is why `v.setFloat32(0, 0.1); v.getFloat32(0)` is not `0.1`.
            Self::Float32 => {
                f64::from(f32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]]))
            }
            Self::Float64 => f64::from_le_bytes(eight),
        };
        Numeric::Number(number)
    }

    /// §25.3.1.5 `NumericToRawBytes` for a Number — `None` if this kind holds a BigInt.
    ///
    /// The integer conversions are `ToIntN`/`ToUintN` (§7.1.7 and following), which **wrap** rather
    /// than clamp or throw: `setUint8(0, 256)` writes 0, and `setInt8(0, 200)` writes -56. That is
    /// modular arithmetic and not saturation, and it is what makes a TypedArray of bytes behave
    /// like memory rather than like a checked container.
    ///
    /// `None` rather than a wrap into the 64-bit slots, because a Number reaching a
    /// `BigInt64Array` is not a value to be truncated — it is a program that should already have
    /// been refused by §7.1.13, and writing it would turn a TypeError into a silent conversion.
    #[must_use]
    pub fn write(self, value: f64) -> Option<Vec<u8>> {
        Some(match self {
            Self::Int8 | Self::Uint8 => vec![wrap(value, 8) as u8],
            Self::Int16 | Self::Uint16 => (wrap(value, 16) as u16).to_le_bytes().to_vec(),
            Self::Int32 | Self::Uint32 => (wrap(value, 32) as u32).to_le_bytes().to_vec(),
            Self::Float32 => (value as f32).to_le_bytes().to_vec(),
            Self::Float64 => value.to_le_bytes().to_vec(),
            Self::BigInt64 | Self::BigUint64 => return None,
        })
    }

    /// §25.3.1.5 `NumericToRawBytes` for a BigInt — `None` if this kind holds a Number.
    ///
    /// §7.1.15 `ToBigInt64` and §7.1.16 `ToBigUint64`, which are one operation here: both take the
    /// low sixty-four bits and differ only in how a *read* interprets the top one. So there is no
    /// sign in this function, which is why `setBigInt64` and `setBigUint64` are the same native.
    #[must_use]
    pub fn write_big(self, value: &crate::bigint::BigInt) -> Option<Vec<u8>> {
        match self.holds_big() {
            true => Some(value.low_u64().to_le_bytes().to_vec()),
            false => None,
        }
    }

    /// §25.3.1.5 for whichever of the two a value is, with §7.1.11's clamping where it is asked for.
    ///
    /// `None` when the numeric and the kind disagree about their content type, which no caller can
    /// arrive at: the conversion that produced the value was itself chosen by this kind, and both
    /// §7.1.4 and §7.1.13 throw rather than answer the other type. Written as an absence rather
    /// than a truncation so that a caller that ever *did* skip the conversion writes nothing at all
    /// instead of writing the wrong sixty-four bits.
    #[must_use]
    pub fn write_numeric(self, value: &Numeric, clamped: bool) -> Option<Vec<u8>> {
        match value {
            Numeric::Number(number) => self.write(clamp_if(*number, clamped)),
            Numeric::BigInt(big) => self.write_big(big),
        }
    }
}

/// §7.1.7 `ToIntN`/`ToUintN` — a Number as `bits` bits, wrapping.
///
/// `NaN`, both infinities and every fractional part go to zero or are truncated first (§7.1.5), and
/// only then does the value wrap. So `setUint8(0, NaN)` writes 0 rather than refusing, which is
/// what makes writing to a buffer total.
fn wrap(value: f64, bits: u32) -> u64 {
    let truncated = value.trunc();
    let modulus = 2_f64.powi(bits as i32);
    // `rem_euclid` rather than `%`, because `%` keeps the sign of the left operand and this has to
    // answer a *non-negative* residue: -1 as a byte is 255 and not -1.
    let wrapped = truncated.rem_euclid(modulus);
    // `NaN` and both infinities arrive here as `NaN` — `inf.rem_euclid(256)` is `NaN` — and a
    // `f64 as u64` cast **saturates**: `NaN` becomes 0, as does anything negative. So the guard
    // §7.1.5 writes out is already in the cast, and written twice it was a branch nothing could
    // tell from its absence.
    wrapped as u64
}

/// §7.1.11 `ToUint8Clamp`, applied only where a `Uint8ClampedArray` asks for it.
///
/// Saturating rather than wrapping, and rounding halves to **even** rather than away from zero.
/// Both are what pixel data wants: 300 is "as bright as it gets" rather than 44, and rounding half
/// to even keeps a long run of averages from drifting upwards.
///
/// Beside the wrapping this module's integer writes use, which is its counterpart: the two are §7.1
/// asking what a Number becomes at a fixed width. Clamping is the *only* thing that separates
/// `Uint8ClampedArray` from `Uint8Array` — every read of their bytes is identical, so the whole of
/// the difference between two of the eleven kinds is this function.
#[must_use]
pub fn clamp_if(value: f64, clamped: bool) -> f64 {
    if !clamped {
        return value;
    }
    // No case for `NaN`: `f64::clamp` answers `NaN` for one, and every arithmetic below carries it
    // through to the `write` that follows, where §7.1.9's cast turns it into 0 — which is the
    // answer §7.1.11 step 1 asks for. Written out as a case it was a branch nothing could tell
    // from its absence.
    let bounded = value.clamp(0.0, 255.0);
    let floor = bounded.floor();
    match bounded - floor {
        half if half > 0.5 => floor + 1.0,
        half if half < 0.5 => floor,
        // Exactly a half — to *even*, which `f64::round` does not do.
        _ if floor % 2.0 == 0.0 => floor,
        _ => floor + 1.0,
    }
}

/// §25.3's `DataView` slots — which buffer, where in it, and how much of it.
///
/// A view is a *window*: it holds no bytes of its own, and two views over one buffer see each
/// other's writes. The offset and the length are fixed when the view is made, which is what makes
/// a view a promise about a region rather than a pointer into one.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// `[[ViewedArrayBuffer]]`.
    pub buffer: crate::heap::ObjectId,
    /// `[[ByteOffset]]` — where the window starts.
    pub offset: usize,
    /// `[[ByteLength]]` — how wide it is.
    pub length: usize,
    /// The type its bytes are read as, or `None` for a `DataView`.
    ///
    /// The whole difference between §25.3 and §23.2: a `DataView` is a window with no type, which
    /// asks for one at every access, and a TypedArray is a window that has one for its whole
    /// length. Everything else about them — the buffer, the offset, the width, the detaching — is
    /// the same, so they are the same record with this field answered differently.
    pub element: Option<Element>,
    /// Whether this window runs to the end of its buffer *whatever length that is* — §10.4.5's
    /// `[[ArrayLength]]` of `auto`.
    ///
    /// A view made over a resizable buffer with no explicit length **tracks** it: resizing the
    /// buffer changes the view's length, with no notification and nothing to re-derive. So `length`
    /// above is not the answer for one of these and must not be read directly — [`crate::heap::Heap::typed_view`]
    /// and [`crate::heap::Heap::any_view`] resolve it against the buffer on every call, which is what makes every
    /// existing reader see the current length without knowing this field exists.
    ///
    /// False for every view over a fixed buffer, including one made without a length: there is
    /// nothing to track when the buffer cannot change size, and saying otherwise would make
    /// `byteLength` a computation where it is a constant.
    pub tracking: bool,
}

impl View {
    /// How many elements this window holds — its byte length over its element's width.
    ///
    /// Zero for a `DataView`, which has no elements at all: `length` there is a count of *bytes*,
    /// and §25.3 never divides it.
    #[must_use]
    pub fn count(&self) -> usize {
        self.element
            .map_or(0, |element| self.length / element.width())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bigint::BigInt;

    /// Every kind and the width §25.3.1.1's table gives it.
    ///
    /// Ten where [`KINDS`](crate::heap::KINDS) has eleven, because `Uint8ClampedArray` is a
    /// `Uint8` that saturates rather than a kind of its own.
    const WIDTHS: [(Element, usize); 10] = [
        (Element::Int8, 1),
        (Element::Uint8, 1),
        (Element::Int16, 2),
        (Element::Uint16, 2),
        (Element::Int32, 4),
        (Element::Uint32, 4),
        (Element::BigInt64, 8),
        (Element::BigUint64, 8),
        (Element::Float32, 4),
        (Element::Float64, 8),
    ];

    #[test]
    fn a_kind_writes_only_the_numeric_type_it_holds() {
        // The content-type line, at the one place it is a *type* question rather than a
        // conversion. Every write above this has already run §7.1.4 or §7.1.13 — whichever the
        // kind asked for — so a mismatched pair reaching here is a caller that skipped the
        // conversion. Answering an absence rather than truncating is what makes that a write
        // that does not happen instead of the wrong sixty-four bits.
        for (element, width) in WIDTHS {
            let number = element.write(1.0);
            let big = element.write_big(&BigInt::from_u64(1));
            match element.holds_big() {
                true => {
                    assert_eq!(number, None, "{} took a Number", element.name());
                    assert_eq!(
                        big.map(|bytes| bytes.len()),
                        Some(width),
                        "{} refused a BigInt",
                        element.name()
                    );
                }
                false => {
                    assert_eq!(big, None, "{} took a BigInt", element.name());
                    assert_eq!(
                        number.map(|bytes| bytes.len()),
                        Some(width),
                        "{} refused a Number",
                        element.name()
                    );
                }
            }
        }
    }

    #[test]
    fn write_numeric_picks_the_half_that_matches_and_has_none_for_the_other() {
        // The dispatch [`Heap::set_element`] and the `DataView` setters both go through, so a
        // mistake in it is a mistake in every write in the engine. Checked in both directions:
        // the pair that agrees produces bytes and the pair that does not produces nothing.
        let one = Numeric::Number(1.0);
        let big_one = Numeric::BigInt(BigInt::from_u64(1));
        assert_eq!(
            Element::Int8.write_numeric(&one, false),
            Some(vec![1]),
            "a Number into a Number kind"
        );
        assert_eq!(
            Element::BigInt64.write_numeric(&big_one, false),
            Some(vec![1, 0, 0, 0, 0, 0, 0, 0]),
            "a BigInt into a BigInt kind"
        );
        assert_eq!(
            Element::BigInt64.write_numeric(&one, false),
            None,
            "a Number into a BigInt kind"
        );
        assert_eq!(
            Element::Int8.write_numeric(&big_one, false),
            None,
            "a BigInt into a Number kind"
        );
        // …and the clamping flag still reaches the Number half through it, which is the one thing
        // `write_numeric` does beyond choosing: §7.1.11 saturates where §7.1.9 wraps.
        assert_eq!(
            Element::Uint8.write_numeric(&Numeric::Number(300.0), true),
            Some(vec![255]),
            "clamped"
        );
        assert_eq!(
            Element::Uint8.write_numeric(&Numeric::Number(300.0), false),
            Some(vec![44]),
            "unclamped"
        );
    }

    #[test]
    fn the_two_bigint_kinds_are_the_same_eight_bytes_read_two_ways() {
        // §25.3.1.3 step 5 — the whole of the difference between `BigInt64` and `BigUint64` is
        // whether the top bit is a sign. Written through one and read through both says that in a
        // way two separate round trips would not.
        let bytes = Element::BigInt64
            .write_big(&BigInt::from_u64(1).negate())
            .expect("a BigInt kind takes a BigInt"); // the test is about the bytes
        assert_eq!(bytes, vec![0xFF; 8]);
        assert_eq!(
            Element::BigInt64.read(&bytes),
            Numeric::BigInt(BigInt::from_u64(1).negate()),
            "read as signed"
        );
        assert_eq!(
            Element::BigUint64.read(&bytes),
            Numeric::BigInt(BigInt::from_u64(u64::MAX)),
            "read as unsigned"
        );
        // And the *write* consults no sign at all, which is why one native serves both setters.
        assert_eq!(
            Element::BigUint64.write_big(&BigInt::from_u64(1).negate()),
            Some(bytes),
            "the sign is a reader's question"
        );
    }

    #[test]
    fn a_kinds_width_is_what_its_bytes_take_and_its_name_is_its_accessors() {
        // The table §25.3.1.1 and §25.3.4 share. A width that disagreed with what `write`
        // produces would put every element of an array at the wrong offset, and a name that
        // disagreed would build `DataView.prototype` with a method under the wrong spelling.
        for (element, width) in WIDTHS {
            assert_eq!(element.width(), width, "{}", element.name());
            let bytes = match element.holds_big() {
                true => element.write_big(&BigInt::zero()),
                false => element.write(0.0),
            };
            assert_eq!(
                bytes.map(|bytes| bytes.len()),
                Some(width),
                "{} wrote its width",
                element.name()
            );
        }
        assert_eq!(Element::BigInt64.name(), "BigInt64");
        assert_eq!(Element::BigUint64.name(), "BigUint64");
    }
}
