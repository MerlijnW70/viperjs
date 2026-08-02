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
//! - `arguments` — §10.4.4's parameter map, which makes `arguments[0]` and `a` one variable.
//! - `property` — [`PropertyKey`], and what an object files under one.
//! - `object` — the ordinary object (§10.1) and its internal methods.
//! - `define` — §10.1.6.3, which decides whether a property may change and then changes it.
//! - `enumerate` — §14.7.5.10, the names a `for`-`in` visits and the shadowing that decides them.
//! - `collect` — mark and sweep, and what counts as a root.
//! - `environment` — where a variable lives (§9.1), and what a closure holds on to.
//! - here — the arenas, their handles, and the intern table property keys need.

mod arguments;
mod array;
mod buffer;
mod callable;
mod collect;
mod collection;
mod define;
mod enumerate;
mod environment;
mod helper;
mod iteration;
mod matches;
mod object;
mod ordinary;
mod promise;
mod property;
mod proxy;
mod regexp;
mod string_object;
mod symbol;
mod typed;
mod weak_ref;

pub use self::arguments::{ArgumentsMap, Incoming};
pub use self::callable::{Bound, Callable, Native, NativeCall, Resumption};
pub use self::iteration::{Iterated, Iteration};
pub use self::symbol::{Symbol, SymbolId};

/// What `[[DefineOwnProperty]]` came to.
///
/// Three answers rather than two, because §10.4.2.4 step 2 has one that a Boolean cannot carry:
/// an array length that is not an integer index is a **RangeError**, where every other refusal is
/// a `false` that sloppy code ignores. Written as an enum so the difference cannot be lost by a
/// caller that was not thinking about arrays — which is what happened when the rule was written
/// twice, once here and once in a predicate the callers had to remember to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefineOutcome {
    /// The property is now what the descriptor asked for.
    Defined,
    /// §10.1.6.3's rules did not allow it. Sloppy code ignores this; strict code throws.
    Refused,
    /// §10.4.2.4 step 2 — the value is not a length, which throws rather than being refused.
    BadLength,
    /// §10.4.5.16 step 1 — a Number where a `BigInt64Array` wants a BigInt, or the reverse.
    ///
    /// A TypeError and not a refusal, because it is §7.1.4 and §7.1.13 declining to convert rather
    /// than §10.1.6.3 declining to redefine — so `Reflect.defineProperty` throws here where an
    /// out-of-range index on the same array answers `false`.
    WrongContent,
}

impl From<bool> for DefineOutcome {
    /// What an ordinary define answered, which is only ever two of the three.
    fn from(defined: bool) -> Self {
        match defined {
            true => Self::Defined,
            false => Self::Refused,
        }
    }
}
pub use self::buffer::{Buffer, Element, Numeric, View, clamp_if};
pub use self::collect::{Collected, Roots};
pub use self::collection::{Collection, CollectionKind};
pub use self::environment::{Environment, EnvironmentId};
pub use self::helper::{Helper, Step};
pub use self::matches::Matches;
pub(crate) use self::object::MAX_PROTOTYPE_CHAIN;
pub use self::object::{Lexical, Object, ObjectId, PrivateElement, Suspendable};
pub use self::promise::{
    Capability, Gather, Group, Promise, PromiseState, Reaction, ReactionKind, Request, Role,
    Settler,
};
pub use self::proxy::Proxy;
pub use self::regexp::RegExp;
pub use self::typed::KINDS;
pub use self::weak_ref::{Cell, Holdable, Registry, Weak};

impl Heap {
    /// §7.2.3 `IsCallable` — whether a *value* is something a call may reach.
    ///
    /// On the heap rather than beside the built-ins that ask, because the answer is a fact about an
    /// object and three modules were about to write it out again.
    pub fn is_callable(&self, value: crate::value::Value) -> bool {
        matches!(value, crate::value::Value::Object(id)
            if self.object(id).is_some_and(Object::is_callable))
    }

    /// §7.2.4 `IsConstructor` — whether `new` may reach it.
    pub fn is_constructor(&self, value: crate::value::Value) -> bool {
        matches!(value, crate::value::Value::Object(id)
            if self.object(id).is_some_and(Object::is_constructor))
    }
}
pub use self::property::{Property, PropertyDescriptor, PropertyKey, PropertyKind};

use crate::span::Span;
use std::collections::HashMap;

/// The most 16-bit code units a String on this heap may hold — DR-0012.
///
/// §6.1.4 defines the String type as sequences "up to a maximum length of 2^53 - 1 elements", a
/// figure no implementation can reach: at two bytes an element it names sixteen petabytes. The
/// specification does not say what an engine with a smaller limit should do, and each of them
/// picks a different number — so this one is praxis's, and DR-0012 is where it is argued rather
/// than merely stated.
///
/// 2^28-1 units is 512 MiB of `u16` at the limit: past anything a program means to build, and
/// small enough that the allocation itself is not the next way to fall over.
pub const MAX_STRING_LENGTH: usize = (1 << 28) - 1;

/// Whether a String of this many code units is one that may exist — DR-0012's cap.
///
/// The same number [`Heap::new_string_checked`] enforces, asked *before* anything is built. A
/// caller about to compute a length has to be able to find out whether the answer could exist, and
/// finding out by trying it is exactly what DR-0002 forbids.
///
/// Compared in `f64` because that is what the callers are working in: a length that has not been
/// built yet is a product of two numbers and may be far past what a `usize` holds, and narrowing it
/// first would turn "an exabyte" into some small number that fits.
pub fn fits_in_a_string(units: f64) -> bool {
    units <= MAX_STRING_LENGTH as f64
}

/// Whether a String of `left` units followed by `right` more is one that may exist — DR-0012.
///
/// Separated from [`Heap::concat`] so that the decision can be *asked* at any size while joining
/// two Strings that big cannot be: proving the boundary is right would otherwise mean allocating
/// half a gigabyte in a unit test, and a limit nobody can afford to test is a limit nobody has
/// checked.
///
/// Saturating rather than checked: two lengths that overflow a `usize` are past the maximum by an
/// enormous margin, and answering `false` is what the caller does with that anyway. A `checked_add`
/// here would add a second way to say no that no input could ever reach.
const fn string_fits(left: usize, right: usize) -> bool {
    left.saturating_add(right) <= MAX_STRING_LENGTH
}

/// The most memory a heap may hand out before the engine refuses — DR-0013.
///
/// Not a number about machines: it is a number about *scripts*. `while (true) { ({}); }` allocates
/// forever, and until a collection policy exists (see [`Heap::collect`], which has the operation
/// and no caller) forever means until the process dies. An abort is the one failure DR-0002 has no
/// answer for, so the engine stops first and says so.
///
/// 64 MiB, and the number is chosen from a measurement rather than from taste. [`Heap::footprint`]
/// is an *estimate* that leaves out the storage an object's own properties take, and the gap is
/// widest for element-heavy programs: `while (true) { []; }` was measured at four times its
/// reported footprint. So the budget carries that factor as headroom, and what a runaway actually
/// costs before it is stopped is a few hundred megabytes rather than a few hundred gigabytes.
///
/// Generous for what praxis can currently run — an engine with no collection policy cannot execute
/// a long program under any budget — and the number to raise first when there is one. When there
/// is an embedding API this becomes something the host sets; a constant is what it can be while
/// there is nobody to ask.
pub const MAX_HEAP_BYTES: usize = 1 << 26;

/// A String on the heap — a sequence of UTF-16 code units (DR-0004).
///
/// Not a `String` and not a `str`: `"\u{d800}"` is a legal ECMAScript string of one code unit,
/// and no Rust string type can hold it. The consequences are worked through in DR-0004; what
/// matters here is that the element type is `u16` and that nothing validates it.
///
/// Meaningful only to the [`Heap`] that issued it. See [`Heap::string`] for what happens when it
/// is given to another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub(super) usize);

/// Where a BigInt lives on the heap.
///
/// A handle for the reason a [`StringId`] is one: §6.1.6.2's integer has no width, so its size is
/// the program's to choose and it cannot sit in a register. Unlike an [`ObjectId`] this addresses a
/// *value* — two handles to equal digits are the same BigInt, and every comparison reads through
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BigIntId(pub(super) usize);

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
    strings: Vec<Option<Box<[u16]>>>,
    /// The BigInts, by the handle that addresses one.
    ///
    /// Not interned, and deliberately not: two BigInts with the same digits are the same *value*
    /// and every relation says so by reading them, so sharing a slot would buy nothing an equality
    /// does not already give. A table keyed by digits would also have to hash a magnitude of
    /// arbitrary length on every literal.
    bigints: Vec<Option<crate::bigint::BigInt>>,
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
    /// Every object ever allocated, in the order they were allocated.
    ///
    /// A separate arena from the strings, so that an [`ObjectId`] cannot address a String and the
    /// compiler says so — one arena per type is DR-0010's second consequence, after the shape of
    /// the handle itself.
    objects: Vec<Option<Object>>,
    /// Every environment ever made — one per call, plus the script's.
    ///
    /// On the heap rather than on a stack because a closure outlives the call that made it: the
    /// frame is gone and the variables are not. See [`Environment`].
    environments: Vec<Option<Environment>>,
    /// Every Symbol ever made — §6.1.5, where a handle is the value rather than a name for one.
    ///
    /// Its own arena for the reason the others have theirs: a [`SymbolId`] cannot address a String
    /// and the compiler says so. Nothing is ever removed while the collector does not reach here,
    /// on the same terms as the rest.
    symbols: Vec<Option<Symbol>>,
    /// §20.4.2.2's global Symbol registry — what `Symbol.for` has already handed out.
    ///
    /// Keyed by the *interned* String, so `Symbol.for("a")` twice is one Symbol. The registry
    /// outlives every realm by design: the specification says so in as many words, and it is the
    /// reason two frames can agree on a key without sharing an object.
    registry: HashMap<StringId, SymbolId>,
    /// How many code units every String on this heap holds between them.
    ///
    /// Tracked rather than summed because [`Heap::footprint`] is asked in the interpreter's loop
    /// and walking every String to answer it would make the check cost more than the work it
    /// guards. The arenas' own lengths are already `O(1)`; this is the one part that is not.
    string_units: usize,
    /// How many bytes every `ArrayBuffer` on this heap holds between them.
    ///
    /// Counted, and counted at all because DR-0013's footprint is what stops a runaway: without
    /// this, `while (true) { new ArrayBuffer(1 << 20); }` allocates until the process dies, since
    /// none of the other three terms grows by anything like what one costs.
    buffer_bytes: usize,
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
        self.string_units += units.len();
        self.strings.push(Some(units.into_boxed_slice()));
        id
    }

    /// Roughly how many bytes this heap has taken, and the number DR-0013's budget is against.
    ///
    /// An estimate, and deliberately a cheap one: three arena lengths and a running total of code
    /// units, all `O(1)`, because the interpreter asks this between instructions. What it counts
    /// is what a runaway loop actually grows — a slot per allocation, which DR-0010 never gives
    /// back, plus the contents of every String.
    ///
    /// What it does not count is the storage an object's own properties take. A program can
    /// therefore hold more than this says, and the budget is a bound on the shape of failure
    /// rather than a precise ceiling: a loop that allocates is stopped, which is the case that
    /// ends in an abort.
    pub fn footprint(&self) -> usize {
        // Slots rather than live values. A swept slot still costs its place in the arena — DR-0010
        // trades that for handles that never dangle — so what has been *allocated* is the honest
        // measure of what the heap has cost, not what is still reachable.
        self.objects.len() * size_of::<Option<Object>>()
            + self.environments.len() * size_of::<Option<Environment>>()
            + self.strings.len() * size_of::<Option<Box<[u16]>>>()
            + self.string_units * size_of::<u16>()
            + self.buffer_bytes
    }

    /// Note that a buffer of `length` bytes now exists — see [`Heap::allowance`].
    pub fn charge_buffer(&mut self, length: usize) {
        self.buffer_bytes = self.buffer_bytes.saturating_add(length);
    }

    /// Note that a buffer that held `before` bytes now holds `after` — §25.1.6.4's resize.
    ///
    /// The one place this total goes **down**. Everything else that allocates only adds, because
    /// DR-0010 does not give a slot back and a detached buffer's bytes are not returned either —
    /// but a resizable buffer shrinking really has given the memory up, and charging it as a fresh
    /// allocation would make `for (;;) { ab.resize(0); ab.resize(n); }` read as a runaway and be
    /// refused for memory it is not using.
    pub fn charge_buffer_delta(&mut self, before: usize, after: usize) {
        self.buffer_bytes = self
            .buffer_bytes
            .saturating_sub(before)
            .saturating_add(after);
    }

    /// How much more DR-0013 will allow, in bytes.
    ///
    /// What `ArrayBuffer` asks before it allocates rather than after. Every other allocation here
    /// is small enough that noticing afterwards is soon enough; a buffer is asked for by a number a
    /// program chose, so `new ArrayBuffer(2 ** 40)` has to be refused *before* the memory is taken
    /// rather than reported once it has been.
    pub fn allowance(&self) -> usize {
        MAX_HEAP_BYTES.saturating_sub(self.footprint())
    }

    /// Whether this heap has taken more than DR-0013 allows.
    pub fn is_exhausted(&self) -> bool {
        self.footprint() > MAX_HEAP_BYTES
    }

    /// A new String from `units`, or `None` if there are more of them than a String may hold.
    ///
    /// The check [`Heap::concat`] makes, for the callers that build their units themselves rather
    /// than by joining two Strings — `String.fromCharCode` and the methods that assemble one.
    pub fn new_string_checked(&mut self, units: Vec<u16>) -> Option<StringId> {
        string_fits(units.len(), 0).then(|| self.new_string(units))
    }

    /// Join two Strings, or refuse because the result would be longer than one may be.
    ///
    /// §6.1.4 puts the String type's maximum at 2^53-1 elements and says nothing about what an
    /// implementation with a smaller one should do; every engine imposes a smaller one and every
    /// engine throws a `RangeError`. DR-0012 records praxis's, and this is the door it is enforced
    /// at — the *only* door, because concatenation is the only operation that makes a String longer
    /// than the two things it was made from. Every other String on this heap is a piece of the
    /// source text or a number's spelling, and neither can outgrow the program that asked for it.
    ///
    /// The length is checked *before* anything is allocated, so an overlong result is refused
    /// rather than briefly built. That is the whole point: a `Vec` that grows past what is
    /// available aborts the process, and DR-0002 does not permit a script to do that.
    pub fn concat(&mut self, left: StringId, right: StringId) -> Option<StringId> {
        // A missing handle reads as empty, on the same terms as `Heap::string` — a foreign handle
        // is a wrong string, never a panic.
        let left_units = self.string(left).map_or(0, <[u16]>::len);
        let right_units = self.string(right).map_or(0, <[u16]>::len);
        if !string_fits(left_units, right_units) {
            return None;
        }
        let mut units = Vec::with_capacity(left_units + right_units);
        units.extend_from_slice(self.string(left).unwrap_or(&[]));
        units.extend_from_slice(self.string(right).unwrap_or(&[]));
        Some(self.new_string(units))
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
        self.strings.get(id.0)?.as_deref()
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

    /// Put a BigInt on the heap and answer the handle that addresses it.
    pub fn new_bigint(&mut self, value: crate::bigint::BigInt) -> BigIntId {
        let id = BigIntId(self.bigints.len());
        self.bigints.push(Some(value));
        id
    }

    /// The BigInt a handle addresses, or `None` for a handle to a swept slot.
    pub fn bigint(&self, id: BigIntId) -> Option<&crate::bigint::BigInt> {
        self.bigints.get(id.0)?.as_ref()
    }

    /// Put a Symbol on the heap — §20.4.1.1 `SymbolDescriptiveString`'s subject, and §6.1.5's value.
    ///
    /// A fresh Symbol every time, which is the entire contract: `Symbol("a") === Symbol("a")` is
    /// false, and there is no way to ask for one that already exists except through the registry.
    pub fn new_symbol(&mut self, description: Option<StringId>) -> SymbolId {
        let id = SymbolId(self.symbols.len());
        self.symbols.push(Some(Symbol {
            description,
            registered: None,
        }));
        id
    }

    /// The Symbol `key` names in §20.4.2.2's registry, made and filed if it is not there yet.
    ///
    /// The half of `Symbol.for` that concerns the heap. `key` must already be interned, which
    /// [`PropertyKey`] guarantees and every caller here goes through.
    pub fn registered_symbol(&mut self, key: StringId) -> SymbolId {
        let key = self.intern_id(key);
        if let Some(found) = self.registry.get(&key) {
            return *found;
        }
        let id = self.new_symbol(Some(key));
        if let Some(Some(symbol)) = self.symbols.get_mut(id.index()) {
            symbol.registered = Some(key);
        }
        self.registry.insert(key, id);
        id
    }

    /// The Symbol `id` refers to, or `None` if this heap has nothing there.
    ///
    /// The same narrow promise [`Heap::string`] makes about a foreign handle, for the same reason.
    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id.index())?.as_ref()
    }

    /// What a Symbol was described as, if anything — §20.4.3.2's `[[Description]]`.
    pub fn symbol_description(&self, id: SymbolId) -> Option<StringId> {
        self.symbol(id)?.description
    }

    /// The registry key a Symbol was made under — §20.4.2.7 `Symbol.keyFor`.
    pub fn symbol_registry_key(&self, id: SymbolId) -> Option<StringId> {
        self.symbol(id)?.registered
    }

    /// How many Symbols this heap holds.
    pub fn symbol_count(&self) -> usize {
        self.symbols.iter().filter(|slot| slot.is_some()).count()
    }

    /// The interned String with these contents, if this heap has already interned one.
    ///
    /// The read-only half of [`Heap::intern`], and the only half a shared borrow can use. Answers
    /// `None` for contents that are on the heap but were never interned: this is a question about
    /// the intern table, not about what strings exist.
    pub fn find_string(&self, units: &[u16]) -> Option<StringId> {
        self.interned.get(units).copied()
    }

    /// The interned String with the same contents as `id`, which may be `id` itself.
    ///
    /// What `ToPropertyKey` needs: a String a script computed, filed under the one handle every
    /// equal String will be filed under. A handle this heap does not know interns as the empty
    /// String, which is the same answer [`Heap::string`] gives it — see there for why that
    /// situation is bounded rather than detected.
    pub fn intern_id(&mut self, id: StringId) -> StringId {
        // Asked first without copying anything. The common case is a name that has been used
        // before — `o[k]` in a loop, or the same key on a thousand objects — and answering it
        // needs only to *read* the units, where the copy below exists solely to release the borrow
        // on `strings` before `intern` takes the heap mutably.
        let Some(units) = self.strings.get(id.0).and_then(Option::as_ref) else {
            // A handle this heap never issued has no contents, so it interns as the empty String —
            // the same answer [`Heap::string`] gives it, and for the same reason.
            return self.intern(&[]);
        };
        // Asked without copying anything. The common case is a name that has been used before —
        // `o[k]` in a loop, or the same key on a thousand objects — and answering it needs only to
        // *read* the units.
        if let Some(found) = self.interned.get(&**units) {
            return *found;
        }
        // Not filed yet, and **this** handle becomes the one it is filed under. `intern` allocates
        // a String on a miss because its caller holds units that are not on the heap; here they
        // already are, so making a second String to hold the same text would leave the first one
        // dead the moment the key is used — which is the whole shape this call is on the hot path
        // of. The map still owns its key, which is the one copy that cannot be avoided.
        let key = units.clone();
        self.interned.insert(key, id);
        id
    }

    /// How many Strings this heap holds.
    ///
    /// For tests and for whatever reports on the heap later. It counts allocations rather than
    /// live values, which is the same number until something sweeps.
    pub fn string_count(&self) -> usize {
        self.strings.iter().filter(|slot| slot.is_some()).count()
    }
}

#[cfg(test)]
mod cap {
    use super::{MAX_STRING_LENGTH, fits_in_a_string};

    #[test]
    fn the_longest_string_that_may_exist_is_the_cap_itself() {
        // The boundary, from both sides. Asked rather than walked: proving it by building a String
        // this long would cost half a gigabyte, and a limit nobody can afford to test is a limit
        // nobody has checked.
        assert!(fits_in_a_string(MAX_STRING_LENGTH as f64));
        assert!(fits_in_a_string(MAX_STRING_LENGTH as f64 - 1.0));
        assert!(!fits_in_a_string(MAX_STRING_LENGTH as f64 + 1.0));
        assert!(fits_in_a_string(0.0));
        // The sizes the callers actually arrive with, none of which a `usize` would hold.
        assert!(!fits_in_a_string(1e18));
        assert!(!fits_in_a_string(f64::INFINITY));
        // A NaN fits nothing: every comparison against it is false, and answering `false` is what
        // a caller that cannot build a length wants anyway.
        assert!(!fits_in_a_string(f64::NAN));
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
    fn joining_two_strings_lays_them_end_to_end() {
        let mut heap = Heap::new();
        let left = heap.new_string(units("foo"));
        let right = heap.new_string(units("bar"));
        let joined = heap.concat(left, right).expect("well under the maximum"); // the test is about the contents
        assert_eq!(heap.string(joined), Some(&units("foobar")[..]));
        // The operands are untouched — a join makes a third String rather than growing the first.
        assert_eq!(heap.string(left), Some(&units("foo")[..]));
        assert_eq!(heap.string(right), Some(&units("bar")[..]));
        // The empty String is an identity on both sides, and joining two of them is still a
        // String rather than nothing.
        let empty = heap.new_string(Vec::new());
        let same = heap.concat(left, empty).expect("no longer than `left`"); // same
        assert_eq!(heap.string(same), Some(&units("foo")[..]));
        let nothing = heap.concat(empty, empty).expect("empty fits"); // same
        assert_eq!(heap.string(nothing), Some(&[][..]));
        // A handle this heap has nothing at reads as empty rather than refusing — `Heap::string`'s
        // promise, which a join must not quietly narrow into a panic.
        //
        // Out of range, and deliberately not merely foreign: a handle from another heap that
        // happens to be *in* range names this heap's String at that index, which is a wrong string
        // and not a missing one. `Heap::string` says so, and a join inherits it rather than fixing
        // it — there is no realm in which a script can hold two heaps' handles at once.
        let mut elsewhere = Heap::new();
        for _ in 0..heap.string_count() + 1 {
            elsewhere.new_string(units("elsewhere"));
        }
        let past_the_end = elsewhere.new_string(units("last"));
        assert_eq!(heap.string(past_the_end), None);
        let absent = heap
            .concat(left, past_the_end)
            .expect("a missing operand is empty"); // same
        assert_eq!(heap.string(absent), Some(&units("foo")[..]));
    }

    #[test]
    fn a_string_may_not_grow_past_the_maximum() {
        // The number itself, written out. Every other row here is phrased *relative* to the
        // constant and would therefore pass whatever it was set to — so without this line the
        // limit could be changed to anything and the suite would agree, while DR-0012 went on
        // naming a figure the engine no longer used.
        assert_eq!(MAX_STRING_LENGTH, 268_435_455);
        // DR-0012's boundary, asked rather than built: the lengths here name half a gigabyte and
        // more, and no allocation happens because `string_fits` is a decision about two numbers.
        assert!(string_fits(0, 0));
        assert!(string_fits(MAX_STRING_LENGTH, 0));
        assert!(string_fits(0, MAX_STRING_LENGTH));
        // Exactly the maximum fits; one past it does not. Both sides of the boundary, because a
        // limit written with the wrong comparison passes every test that only asks one.
        assert!(string_fits(MAX_STRING_LENGTH - 1, 1));
        assert!(!string_fits(MAX_STRING_LENGTH, 1));
        assert!(!string_fits(1, MAX_STRING_LENGTH));
        assert!(!string_fits(MAX_STRING_LENGTH, MAX_STRING_LENGTH));
        // Two lengths that overflow a `usize` are refused rather than wrapping to something small
        // and being allowed — which is what a plain `+` would do in release.
        assert!(!string_fits(usize::MAX, 1));
        assert!(!string_fits(usize::MAX, usize::MAX));
    }

    #[test]
    fn joining_past_the_maximum_answers_none_instead_of_allocating() {
        // The boundary reached through the real operation rather than through the predicate, so
        // that the wiring between them is a tested thing and not an assumed one.
        //
        // Affordable because nothing here is ever *written*: the operand is one zeroed allocation
        // the operating system may back lazily, `concat` only reads its length, and the join it
        // refuses is the one that would have cost half a gigabyte.
        let mut heap = Heap::new();
        let half = heap.new_string(vec![0; MAX_STRING_LENGTH / 2 + 1]);
        assert!(heap.concat(half, half).is_none());
        // …while the same operand joined to an empty String is still under the maximum and is
        // therefore made — the refusal is about the length, not about the operand being large.
        let empty = heap.new_string(Vec::new());
        assert!(heap.concat(half, empty).is_some());
    }

    #[test]
    fn the_footprint_counts_a_slot_for_every_allocation_and_the_units_of_every_string() {
        // DR-0013's estimate, term by term. Each row allocates one thing and asks what the number
        // moved by, so a term that was dropped, doubled or divided is a different answer rather
        // than a smaller one — a test that only checked "it went up" would agree with all three.
        let mut heap = Heap::new();
        assert_eq!(heap.footprint(), 0);

        heap.new_string(units("hello"));
        let a_string = size_of::<Option<Box<[u16]>>>() + 5 * size_of::<u16>();
        assert_eq!(heap.footprint(), a_string);

        heap.new_object(None);
        assert_eq!(heap.footprint(), a_string + size_of::<Option<Object>>());

        // An environment's slots are not counted, only its place in the arena — so two
        // environments of very different sizes cost the same here, which is what makes the
        // estimate an estimate.
        heap.new_environment(None, 0);
        let so_far = a_string + size_of::<Option<Object>>() + size_of::<Option<Environment>>();
        assert_eq!(heap.footprint(), so_far);
        heap.new_environment(None, 64);
        assert_eq!(heap.footprint(), so_far + size_of::<Option<Environment>>());
    }

    #[test]
    fn the_budget_is_spent_only_once_it_is_passed() {
        // Both sides of DR-0013's comparison, hit exactly. A heap whose footprint is precisely the
        // budget has not exceeded it; one unit more has. Written as an exact landing rather than
        // an approach, because `>` and `>=` differ on this one value and nowhere else.
        //
        // One String does it: the slot plus two bytes a unit, solved for the unit count. Nothing
        // reads the units, so the allocation may never be backed.
        let mut heap = Heap::new();
        let units = (MAX_HEAP_BYTES - size_of::<Option<Box<[u16]>>>()) / size_of::<u16>();
        heap.new_string(vec![0; units]);
        assert_eq!(heap.footprint(), MAX_HEAP_BYTES);
        assert!(!heap.is_exhausted());

        heap.new_string(vec![0; 1]);
        assert!(heap.footprint() > MAX_HEAP_BYTES);
        assert!(heap.is_exhausted());
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
    fn interning_a_handle_answers_the_same_string_whether_the_table_had_it_or_not() {
        // Two paths through `intern_id` and they must not disagree. The first call files the
        // contents and answers the handle it was given; the second is asked about a *different*
        // handle spelling the same thing and answers the first one — that is what makes a key a
        // key. The second is also the path that reads the units where they lie instead of copying
        // them out, so a fast path that looked in the wrong place would show up here as two
        // different keys for one name.
        let mut heap = Heap::new();
        let first = heap.new_string(units("name"));
        let second = heap.new_string(units("name"));
        assert_ne!(first, second);
        let filed = heap.intern_id(first);
        assert_eq!(filed, first);
        assert_eq!(heap.intern_id(second), first);
        // Idempotent, which is the property the fast path is *for*: asking again about a handle
        // already filed under itself allocates nothing and answers itself.
        let before = heap.string_count();
        assert_eq!(heap.intern_id(filed), first);
        assert_eq!(heap.string_count(), before);
        // A handle this heap never issued has no contents, so it interns as the empty String —
        // the same answer `Heap::string` gives it, rather than a panic on the way past.
        let elsewhere = StringId(heap.string_count() + 100);
        let interned = heap.intern_id(elsewhere);
        assert_eq!(heap.string(interned), Some(&[][..]));
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
