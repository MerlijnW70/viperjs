//! The ordinary object — §10.1, the shape almost every object in a program has.
//!
//! An object is three things: a prototype, a flag saying whether properties may still be added,
//! and a collection of properties. Everything else about §10.1 is rules for changing them.
//!
//! # What is here and what is not
//!
//! Every ordinary internal method that does not reach user code. `[[Get]]` and `[[Set]]` are the
//! two that do — an accessor property's getter is a function, and calling it is the VM's job —
//! so they arrive with the interpreter. `[[HasProperty]]`, `[[Delete]]`, `[[GetOwnProperty]]`,
//! `[[DefineOwnProperty]]`, `[[OwnPropertyKeys]]` and the prototype and extensibility methods are
//! all here, and between them they are what the object model *is*.
//!
//! # Why the properties are a `Vec`, and what sits beside it
//!
//! Because §10.1.11 asks for insertion order and a `Vec` has it. Lookup was linear, which this
//! comment used to call the boring implementation and "wrong for an object with a thousand
//! properties — the fix is a map beside the order, or shapes, and both are M8 experiments that
//! need a benchmark first".
//!
//! The benchmark arrived. A linear scan makes *insertion* linear too, so filling an array element
//! by element is quadratic: `a[i] = 1` measured 270 ms for twenty thousand elements, 967 ms for
//! forty, and 3743 ms for eighty — four times the work for twice the elements. That is not a
//! slow engine, it is the wrong shape, and it was bad enough that such a test could not finish
//! inside the conformance harness's budget at all.
//!
//! So there is now a map beside the order, and the `Vec` still *is* the order. The map is not
//! built until an object has more properties than [`INDEXED_ABOVE`], because most objects never
//! do: a hash table on every one of them would cost an allocation each and buy nothing, and
//! DR-0013 counts those allocations. Shapes remain the other answer and remain an M8 experiment —
//! this one is smaller and needed no new representation to get the exponent right.

use crate::compile::Chunk;
use crate::heap::PropertyKind;
use crate::heap::arguments;
use crate::heap::arguments::Incoming;
use crate::heap::buffer::{Buffer, View};
use crate::heap::collection::Collection;
use crate::heap::define::{Validation, apply, validate};
use crate::heap::promise::{Promise, Role};
use crate::heap::string_object;
use crate::heap::typed;
use crate::heap::{
    ArgumentsMap, Bound, Callable, DefineOutcome, EnvironmentId, Heap, Helper, Iteration, Native,
    Property, PropertyDescriptor, PropertyKey, Proxy, StringId, SymbolId, Weak,
};
use crate::value::Value;
use std::collections::HashMap;
use std::rc::Rc;

/// An object on the heap.
///
/// Meaningful only to the [`Heap`] that issued it, on the same terms as [`crate::heap::StringId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub(super) usize);

/// Which kind of execution an object holds parked — §27.5.1's or §27.7's.
///
/// The two are the same record parked in the same slot, and they differ in three places: what the
/// call that made them answered with, what resumes them, and what a `return` from the body does.
/// Each of those asks this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspendable {
    /// §27.5.1's generator, resumed by `next`, `return` and `throw`.
    Generator,
    /// §27.7's `async` function context, resumed by a job when a promise settles.
    Async,
}

/// What an arrow reaches outward for, captured where it was written — §10.2.3 step 6.
///
/// Two values and not one, because §9.1.1.3's function environment holds both and an arrow gets
/// neither of its own: `this` and `new.target` are looked up through the same walk and answered by
/// the same environment, so an arrow that had captured one and resolved the other could disagree
/// with itself about which call it belongs to. One struct means the pair is captured at one moment
/// or not at all.
// No `PartialEq`: [`Value`] deliberately has none, because JavaScript has three different
// equalities and none of them is a derive.
#[derive(Debug, Clone, Copy)]
pub struct Lexical {
    /// The `this` in force where the arrow was written.
    pub this_value: Value,
    /// The `new.target` in force there — §13.3.12 reaches outward exactly as `this` does, which is
    /// why `() => new.target` inside a constructor answers that constructor and not `undefined`.
    pub new_target: Value,
    /// The `[[HomeObject]]` of the method the arrow was written in, if it was written in one.
    ///
    /// The third thing §9.1.1.3 answers by walking outward, so it is captured with the other two.
    /// `None` for an arrow written outside any method, where `super` is a Syntax Error anyway.
    pub home: Option<ObjectId>,
}

/// One entry in §7.3.28's `[[PrivateElements]]` — a field, a method, or an accessor.
///
/// The kind is carried rather than inferred, because §7.3.31 and §7.3.32 read it: a private *method*
/// refuses assignment where a field accepts it, and an accessor calls a function for both. Told apart
/// by the shape of the value they would otherwise be — a `Value` that happens to be callable is not
/// the same thing as a method, since `#x = function () {}` is a field holding a function.
// No `PartialEq`: [`Value`] has none, because JavaScript has three equalities and none is a derive.
#[derive(Debug, Clone, Copy)]
pub enum PrivateElement {
    /// `#x = 1` — per instance, and the only kind a write may reach.
    Field(Value),
    /// `#m() {}` — **one** function object shared by every instance, which each carries an entry for.
    ///
    /// Shared and not copied, so `new C().m === new C().m` for the function a private method holds; the
    /// per-instance entry is what makes `#m in o` a brand rather than a lookup on a prototype.
    Method(Value),
    /// `get #a() {}` and `set #a(v) {}` — one element with two halves, as §7.3.30 adds it.
    ///
    /// Either half may be `undefined`, and then that direction is a TypeError: a private accessor
    /// with only a getter refuses a write outright rather than silently doing nothing, which is where
    /// it differs from a public one.
    Accessor {
        /// The getter, or `undefined` if only a setter was written.
        getter: Value,
        /// The setter, or `undefined` if only a getter was written.
        setter: Value,
    },
}

impl PrivateElement {
    /// What a read answers with directly, if this kind answers without calling anything.
    ///
    /// `None` for an accessor, whose getter the interpreter has to call — the heap cannot, and that
    /// is the same division `[[Get]]` already makes for a property.
    pub fn value(self) -> Option<Value> {
        match self {
            Self::Field(value) | Self::Method(value) => Some(value),
            Self::Accessor { .. } => None,
        }
    }
}

/// An ordinary object — §10.1.
#[derive(Debug, Default)]
pub struct Object {
    /// `[[Prototype]]` — "an Object or **null**", which is what the `Option` is.
    ///
    /// `None` is `null` and not "not set": an object whose prototype is null is the ordinary
    /// state of `Object.create(null)`, and the chain simply ends there.
    prototype: Option<ObjectId>,
    /// `[[Extensible]]` — whether properties may still be added.
    ///
    /// One-way: §10.1.4 can set it false and nothing sets it back. That is what makes
    /// `Object.preventExtensions` a guarantee rather than a suggestion, and it is why
    /// [`Object::prevent_extensions`] takes no argument.
    extensible: bool,
    /// The body this object runs when it is called — its `[[Call]]` internal method.
    ///
    /// `None` for an ordinary object, which is most of them. An object is *callable* exactly when
    /// this is present, which is the whole of what `typeof f === "function"` and "x is not a
    /// function" are asking about.
    ///
    /// Holding the code here rather than in an arena beside it is deliberate: a function object
    /// is the thing that owns its body, and the `Rc` is what lets a closure outlive the call that
    /// made it. See [`Chunk`] for why reference counting is safe for code where DR-0010 rejects
    /// it for values.
    call: Option<Callable>,
    /// The environment this function was *written* in — §10.2's `[[Environment]]`.
    ///
    /// A closure is this field. The call that made the function is long gone by the time the
    /// function runs, and the variables it could see are still here because this holds them.
    environment: Option<EnvironmentId>,
    /// What an arrow was written beside — §10.2's `[[ThisMode]]` of `lexical`.
    ///
    /// `None` for every function that binds its own, which is all of them but arrows. Present, it
    /// is the same idea as `environment` one field up and for the same reason: the call that made
    /// the arrow is gone by the time the arrow runs, so what it could see has to be *held*
    /// rather than looked for. §9.1.1.3 words it as a function environment with no `[[ThisBinding]]`
    /// whose `ResolveThisBinding` walks outward; the environment that walk arrives at is exactly
    /// the one running when the arrow was made, so recording it here is that walk, done
    /// once and in advance.
    lexical: Option<Lexical>,
    /// `[[PrivateElements]]` — §7.3.28's list, and **not** properties.
    ///
    /// A separate list rather than rows in the property table, because a private element is not a
    /// property by any test a program can make: `Object.keys`, `getOwnPropertyNames`,
    /// `getOwnPropertySymbols`, `for...in` and a Proxy trap must all fail to see it. Putting one in
    /// the table would mean teaching every one of those to skip it, and the one that was forgotten
    /// would leak `#x` to a script.
    ///
    /// Keyed by [`SymbolId`] because a Private Name (§6.2.12) needs exactly what a Symbol has and
    /// nothing more: an identity that is itself, freshly minted, with a description only a debugger
    /// would read. The Symbol never reaches the property table, so it cannot be a key by accident,
    /// and nothing a script can write reaches the slot that holds it.
    ///
    /// `None` for every object that has none, which is all of them but instances of a class with a
    /// private element. **Not** boxed, unlike the parameter map and the iteration state beside it:
    /// those box a struct, where the indirection genuinely shrinks the field, and a `Vec` is already
    /// a pointer — so boxing one buys three words back and costs a second allocation for every object
    /// that has any private state at all. `Option<Vec<_>>` also has no discriminant, the null pointer
    /// being the niche.
    pub(super) private: Option<Vec<(SymbolId, PrivateElement)>>,
    /// `[[HomeObject]]` — the object a method was defined *on*, if it is a method.
    ///
    /// `None` for every function that is not one, which is every function expression and every
    /// arrow. What needs it is `super.x`: §9.1.1.3's `GetSuperBase` reads this object's
    /// `[[Prototype]]`, so a method knows where to start looking that is one level above where it was
    /// defined. Unrelated to `this` and deliberately so — a method borrowed by another object keeps
    /// the home it was written in, which is why `super.x` there still reads the original parent.
    pub(super) home: Option<ObjectId>,
    /// Whether this is §10.4.2's exotic Array, whose `length` and indices move each other.
    pub(super) array: bool,
    /// §10.4.4's parameter map, if this is an arguments object.
    ///
    /// `None` for every other object, which is all but one per call that mentions the name. Boxed
    /// so that an object without one pays a pointer rather than a `Vec`: an `Object` sits inline
    /// in the arena, so its size is charged to every object ever made.
    pub(super) arguments: Option<Box<ArgumentsMap>>,
    /// Where this object has got to, if it is an iterator — §23.1.5.1 and §22.1.5.1.
    ///
    /// `None` for everything else. Boxed for the reason the parameter map is: an `Object` sits
    /// inline in the arena, so its size is charged to every object ever made.
    pub(super) iteration: Option<Box<Iteration>>,
    /// The primitive this object *is* a wrapper for — §20.3's `[[BooleanData]]`, §21.1's
    /// `[[NumberData]]` and §22.1's `[[StringData]]`.
    ///
    /// One slot rather than three, because the value in it already says which: a `Value::Boolean`
    /// is a `[[BooleanData]]` and nothing else can be. That is what lets
    /// `Boolean.prototype.valueOf.call(new Number(1))` be the TypeError §20.3.3 asks for without
    /// three fields to keep apart.
    primitive: Option<Value>,
    /// `[[DateValue]]` — the time value, in milliseconds since the epoch, if this is a Date.
    ///
    /// Its own field rather than a `Value::Number` in `primitive`, and that is not redundancy: the
    /// slot above is documented to say *which* wrapper it is by the type of the value in it, so a
    /// Number there means `[[NumberData]]` and can mean nothing else. Putting a time value there
    /// would make `new Date()` indistinguishable from `new Number()`, which would let
    /// `Number.prototype.valueOf.call(new Date())` answer instead of throwing, and would make
    /// `JSON.stringify(v, null, aDate)` indent by the epoch.
    ///
    /// Inline rather than boxed, unlike the two maps above, because the payload is one `f64` and a
    /// pointer to it would cost the same eight bytes plus an allocation. It widens every object by
    /// 16 — there is no niche in an `f64`, and NaN cannot stand in for absence because `new
    /// Date(NaN)` is a legal invalid date whose time value *is* NaN. Measured at 176 bytes before
    /// and 192 after; if that matters it is an M8 question, with a number in front of it.
    pub(super) date: Option<f64>,
    /// §25.1.3.1's data block, if this is an `ArrayBuffer`.
    ///
    /// Boxed like the others, and unlike the others it holds no `Value` at all: a buffer is bytes,
    /// so the collector has nothing to follow through one.
    buffer: Option<Box<Buffer>>,
    /// Whether this TypedArray is the one kind that saturates — `Uint8ClampedArray`.
    ///
    /// A flag rather than a tenth [`Element`], because it is not a different *type*: the bytes are
    /// a `Uint8`'s and every read of them is identical. What differs is one conversion on the way
    /// in, §7.1.11 rather than §7.1.9, so the difference belongs beside the write and not beside
    /// the storage.
    pub(super) clamped: bool,
    /// §25.3's view slots, if this is a `DataView`.
    ///
    /// Three `usize`s and an id, so it is inline rather than boxed: a pointer to it would cost as
    /// much as half of it.
    view: Option<View>,
    /// §24.1's `[[MapData]]` or §24.2's `[[SetData]]`, if this is one of those.
    ///
    /// Boxed for the reason the promise is: an `Object` sits inline in the arena, so its size is
    /// charged to every object ever made and a collection is a `Vec` and a count.
    collection: Option<Box<Collection>>,
    /// The six slots §27.2.6 gives a Promise, if this is one.
    ///
    /// Boxed, for the reason the two maps above are: an `Object` sits inline in the arena, so its
    /// size is charged to every object ever made and a promise's two reaction lists are three words
    /// each. `None` is every object that is not a promise, which is what §27.2.5.4 step 2 tests
    /// before it will do anything at all.
    promise: Option<Box<Promise>>,
    /// What a promise resolving function settles, or what a capability executor fills in.
    ///
    /// §27.2.1.3's resolve and reject functions have a `[[Promise]]`, and §27.2.1.5.1's executor has
    /// a `[[Capability]]`. Neither can be a captured variable: a built-in's body is a bare function
    /// pointer holding no state, deliberately, so what the specification captures in a closure is
    /// carried on the function object exactly as it says it is.
    role: Option<Box<Role>>,
    /// §26.1's `[[WeakRefTarget]]` or §26.2's `[[Cells]]`, if this is one of those.
    ///
    /// Boxed for the reason the collection beside it is: an `Object` sits inline in the arena, so
    /// whatever this costs is charged to every object ever made, and a registry is a `Vec`.
    weak: Option<Box<Weak>>,
    /// §27.1.5's Iterator Helper state, if this is one.
    ///
    /// Boxed like the collection beside it, and for the same reason: an `Object` sits inline in
    /// the arena, so whatever this costs is charged to every object ever made.
    helper: Option<Box<Helper>>,
    /// §10.5's `[[ProxyTarget]]` and `[[ProxyHandler]]`, if this is a Proxy.
    ///
    /// Two ids and a discriminant, so it sits inline rather than boxed — a pointer to it would
    /// cost as much as the thing itself.
    proxy: Option<Proxy>,
    /// §22.2.9's slots, if this is a RegExp String Iterator. Inline: five small fields.
    matches: Option<crate::heap::Matches>,
    /// §27.5.1's `[[GeneratorContext]]` — an execution parked in this object, if one is.
    ///
    /// `None` for every object that is not holding one, which is every object so far: nothing
    /// parks yet but a hand-written chunk. Boxed for the reason the collection beside it is — an
    /// `Object` sits inline in the arena, so whatever this costs is charged to every object ever
    /// made, and a suspension owns two `Vec`s and an `Rc`.
    ///
    /// The interpreter's, and deliberately opaque here: what is in it is frames and operands, and
    /// the heap's only business with it is holding it and letting the collector walk it.
    suspension: Option<Box<crate::vm::Suspended>>,
    /// Which kind of suspendable execution this object holds, if it is one.
    ///
    /// `None` is "neither a generator nor an `async` function's context", which is what
    /// §27.5.1.2 step 2's `RequireInternalSlot` asks about before it will do anything — so
    /// `Generator.prototype.next.call({})` is a TypeError rather than an answer about an object
    /// that merely looks similar. It stays set once set: a finished generator is still a generator.
    ///
    /// There is deliberately no `[[GeneratorState]]` beside it. Every one of §27.5.1's four states
    /// is a *question about somewhere else* — suspended is "it holds a parked execution", executing
    /// is "a live frame names it", completed is neither — and a field repeating those answers is a
    /// field that can disagree with them. It did: a throw that escaped a generator's body left the
    /// state saying `executing` for ever, because nothing on that path had anywhere to write.
    suspendable: Option<Suspendable>,
    /// §22.2.3's internal slots, if this is a regular expression.
    ///
    /// Boxed, because a compiled pattern owns a tree and every object would otherwise carry room
    /// for one.
    regexp: Option<Box<crate::heap::RegExp>>,
    /// The own properties, in the order they were created.
    ///
    /// The order is not incidental — §10.1.11 hands out string keys "in ascending chronological
    /// order of property creation", so this `Vec` *is* that answer for part of the result.
    properties: Vec<(PropertyKey, Property)>,
    /// Where each key sits in `properties`, once there are enough of them to be worth it.
    ///
    /// `None` means "few enough to scan", which is the common case and costs nothing. `Some` is
    /// an exact index: every key in `properties` is in it, mapped to its position. Anything that
    /// disturbs the positions — a delete, which shifts everything after it — either updates this
    /// or rebuilds it, because a stale index would find the wrong property rather than none.
    ///
    /// Boxed so that an object without one pays a pointer rather than a whole hash table. An
    /// `Object` sits inline in the heap's arena, so its size is charged to every object ever
    /// made, live or swept — see [`Heap::footprint`] and DR-0010.
    ///
    /// Measured, because clippy is right to ask: a `HashMap` here makes `Option<Object>` 144 bytes
    /// and a `Box` makes it 104. Most objects never build one, so the forty bytes would be paid by
    /// every object in the program to save a pointer hop for a few.
    #[allow(clippy::box_collection)] // 40 bytes an object, and every object pays — see above
    index: Option<Box<HashMap<PropertyKey, usize>>>,
}

/// How many properties an object may hold before its keys are worth indexing.
///
/// Below this a scan of a short `Vec` beats a hash: the keys are interned, so comparing two is
/// comparing two integers, and eight of those cost less than hashing one. Above it the scan is
/// what makes filling an array quadratic.
///
/// The exact number is not delicate — anything in this region trades the same way, and the cases
/// that hurt have thousands of properties rather than nine.
const INDEXED_ABOVE: usize = 8;

impl Object {
    /// An ordinary object with the given prototype, no properties, and extensible.
    ///
    /// `OrdinaryObjectCreate` (§10.1.12) in the part that concerns the object itself. The
    /// prototype is an argument rather than a default because there is no default: an object
    /// literal gets `Object.prototype`, `Object.create(null)` gets nothing, and neither is more
    /// ordinary than the other.
    pub fn new(prototype: Option<ObjectId>) -> Self {
        Self {
            prototype,
            extensible: true,
            array: false,
            arguments: None,
            iteration: None,
            primitive: None,
            date: None,
            buffer: None,
            view: None,
            clamped: false,
            collection: None,
            promise: None,
            role: None,
            weak: None,
            helper: None,
            proxy: None,
            matches: None,
            suspension: None,
            suspendable: None,
            regexp: None,
            call: None,
            environment: None,
            lexical: None,
            private: None,
            home: None,
            properties: Vec::new(),
            index: None,
        }
    }

    /// The parameter map this object joins, if it is an arguments object.
    pub(crate) fn arguments_map(&self) -> Option<&ArgumentsMap> {
        self.arguments.as_deref()
    }

    /// Where this object has got to, if it is an iterator.
    pub fn iteration(&self) -> Option<&Iteration> {
        self.iteration.as_deref()
    }

    /// The same, to be moved on by a step.
    pub fn iteration_mut(&mut self) -> Option<&mut Iteration> {
        self.iteration.as_deref_mut()
    }

    /// Whether this object is something a call may reach — §7.2.3 `IsCallable`, for an object.
    pub fn is_callable(&self) -> bool {
        self.call.is_some()
    }

    /// Whether `new` may reach it — §7.2.4 `IsConstructor`.
    ///
    /// Not the same question: every constructor is callable and most callables are not
    /// constructors. An arrow, a method and a getter each have a `[[Call]]` and no `[[Construct]]`,
    /// which is why `new ({ m() {} }).m` is a TypeError.
    pub fn is_constructor(&self) -> bool {
        self.call.as_ref().is_some_and(super::Callable::constructs)
    }

    /// Whether this is a `Uint8ClampedArray`.
    pub fn is_clamped(&self) -> bool {
        self.clamped
    }

    /// Say that it is — §23.2.5's `Uint8ClampedArray` and nothing else.
    ///
    /// Takes no argument, so it is only ever called for the kind that *is* clamped. Written to take
    /// a Boolean it was called for all nine, and the eight that passed `false` were writing the
    /// value that was already there — which made the field's own default unreachable by any test.
    pub fn set_clamped(&mut self) {
        self.clamped = true;
    }

    /// The window this object is, if it is a `DataView`.
    pub fn view(&self) -> Option<View> {
        self.view
    }

    /// Make this object a `DataView` — §25.3.2.1, which is the only caller.
    pub fn set_view(&mut self, view: View) {
        self.view = Some(view);
    }

    /// The bytes this object holds, if it is an `ArrayBuffer`.
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_deref()
    }

    /// The same, to write through or to detach.
    pub fn buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_deref_mut()
    }

    /// Make this object an `ArrayBuffer` — §25.1.3.1, which is the only caller.
    pub fn set_buffer(&mut self, buffer: Buffer) {
        self.buffer = Some(Box::new(buffer));
    }

    /// The entries this object holds, if it is a `Map` or a `Set`.
    ///
    /// `None` for every other object, which is the test every one of §24's methods makes first:
    /// `Map.prototype.get.call({})` is a TypeError rather than `undefined`, because the method is
    /// about the internal slot and not about a shape.
    pub fn collection(&self) -> Option<&Collection> {
        self.collection.as_deref()
    }

    /// The same, to change.
    pub fn collection_mut(&mut self) -> Option<&mut Collection> {
        self.collection.as_deref_mut()
    }

    /// Make this object a `Map` or a `Set` — §24.1.1.1 and §24.2.1.1 step 4.
    pub fn set_collection(&mut self, collection: Collection) {
        self.collection = Some(Box::new(collection));
    }

    /// What this object holds weakly, if it is a `WeakRef` or a `FinalizationRegistry`.
    pub fn weak(&self) -> Option<&Weak> {
        self.weak.as_deref()
    }

    /// The same, to change.
    pub fn weak_mut(&mut self) -> Option<&mut Weak> {
        self.weak.as_deref_mut()
    }

    /// Make this object one of §26's two, which nothing undoes.
    pub fn set_weak(&mut self, weak: Weak) {
        self.weak = Some(Box::new(weak));
    }

    /// The §27.1.5 helper state this object holds, if it is an Iterator Helper.
    pub fn helper(&self) -> Option<&Helper> {
        self.helper.as_deref()
    }

    /// The same, to change — a helper is state that moves as it is walked.
    pub fn helper_mut(&mut self) -> Option<&mut Helper> {
        self.helper.as_deref_mut()
    }

    /// Make this object an Iterator Helper, which nothing undoes.
    pub fn set_helper(&mut self, helper: Helper) {
        self.helper = Some(Box::new(helper));
    }

    /// §22.2.9's walk, if this object is a RegExp String Iterator.
    pub fn matches(&self) -> Option<crate::heap::Matches> {
        self.matches
    }

    /// The same, to move it on.
    pub fn matches_mut(&mut self) -> Option<&mut crate::heap::Matches> {
        self.matches.as_mut()
    }

    /// Make this object one — §22.2.9.1 `CreateRegExpStringIterator`.
    pub fn set_matches(&mut self, matches: crate::heap::Matches) {
        self.matches = Some(matches);
    }

    /// The compiled pattern this object holds, if it is a regular expression — §22.2.3.
    pub fn regexp(&self) -> Option<&crate::heap::RegExp> {
        self.regexp.as_deref()
    }

    /// Make this object a regular expression, or replace the pattern it already held.
    ///
    /// Replacing is what §22.2.3.1 does: `RegExp.prototype.compile` re-initialises an object that
    /// is already one, and nothing else in the language changes a pattern after it is made.
    pub fn set_regexp(&mut self, regexp: crate::heap::RegExp) {
        self.regexp = Some(Box::new(regexp));
    }

    /// The execution parked in this object, if one is — §27.5.1's `[[GeneratorContext]]`.
    pub(crate) fn suspension(&self) -> Option<&crate::vm::Suspended> {
        self.suspension.as_deref()
    }

    /// Which kind of suspendable this object is, if it is one — and not where it has got to.
    pub(crate) fn suspendable(&self) -> Option<Suspendable> {
        self.suspendable
    }

    /// Whether it is a *generator* — §27.5.1's brand, which is what the three resumptions want.
    pub(crate) fn is_generator(&self) -> bool {
        self.suspendable == Some(Suspendable::Generator)
    }

    /// The target and handler this object proxies, if it is a Proxy — §10.5.
    pub fn proxy(&self) -> Option<Proxy> {
        self.proxy
    }

    /// The same, to revoke.
    pub fn proxy_mut(&mut self) -> Option<&mut Proxy> {
        self.proxy.as_mut()
    }

    /// Make this object a Proxy, which nothing undoes — revoking empties it rather than removing it.
    pub fn set_proxy(&mut self, proxy: Proxy) {
        self.proxy = Some(proxy);
    }

    /// The promise state this object holds, if it is a promise.
    pub fn promise(&self) -> Option<&Promise> {
        self.promise.as_deref()
    }

    /// The same, to settle.
    pub fn promise_mut(&mut self) -> Option<&mut Promise> {
        self.promise.as_deref_mut()
    }

    /// Make this object a pending promise — §27.2.3.1 step 3, which is the only caller.
    pub(super) fn set_promise(&mut self, promise: Promise) {
        self.promise = Some(Box::new(promise));
    }

    /// What this function object settles or fills in, if it is one of §27.2's.
    pub fn role(&self) -> Option<&Role> {
        self.role.as_deref()
    }

    /// The same, for a capability executor to write into when it is called.
    pub fn role_mut(&mut self) -> Option<&mut Role> {
        self.role.as_deref_mut()
    }

    /// Give this function object the state §27.2 describes as captured.
    pub fn set_role(&mut self, role: Role) {
        self.role = Some(Box::new(role));
    }

    /// The primitive this object wraps, if it wraps one.
    ///
    /// `None` for an ordinary object, which is most of them. What is in it says which kind of
    /// wrapper this is, so a method that requires its own kind matches on the value rather than
    /// asking a flag.
    pub fn primitive(&self) -> Option<Value> {
        self.primitive
    }

    /// The time value this object holds, if it is a Date — `[[DateValue]]`.
    ///
    /// `Some(NaN)` is a Date that is present and invalid, which is a different answer from `None`
    /// for something that is not a Date at all. Every `Date.prototype` method needs to tell those
    /// two apart: the first produces `NaN` or `"Invalid Date"`, the second a TypeError.
    pub fn date_value(&self) -> Option<f64> {
        self.date
    }

    /// Move a Date to another instant — `Date.prototype.setTime` and every other setter.
    ///
    /// The caller must already have established that this *is* a Date, which every setter in §21.4.4
    /// has: each one reads `thisTimeValue` before it computes anything, and a receiver without the
    /// slot has thrown by then. So there is no guard here — a branch no input can reach is a branch
    /// no test can pin, and one written defensively would only look tested.
    pub fn set_date_value(&mut self, value: f64) {
        self.date = Some(value);
    }

    /// The characters this object *is*, if it is a String exotic object — `[[StringData]]`.
    ///
    /// A wrapper around any other primitive answers `None`, which is what keeps `new Number(1)`
    /// from growing a property per digit.
    pub fn string_data(&self) -> Option<StringId> {
        match self.primitive {
            Some(Value::String(data)) => Some(data),
            _ => None,
        }
    }

    /// Whether this is an Array — §10.4.2's exotic object, and the only one there is.
    pub fn is_array(&self) -> bool {
        self.array
    }

    /// `[[GetPrototypeOf]]` (§10.1.1) — the prototype, or `None` for `null`.
    pub fn prototype(&self) -> Option<ObjectId> {
        self.prototype
    }

    /// What this object runs when it is called, if it is callable at all.
    ///
    /// `None` is what `typeof` reads to answer anything but `"function"`, and what a call
    /// expression checks before it does anything else.
    pub fn call(&self) -> Option<&Callable> {
        self.call.as_ref()
    }

    /// The environment this function was written in, if it is a function at all.
    pub fn environment(&self) -> Option<EnvironmentId> {
        self.environment
    }

    /// §7.3.28 `PrivateElementFind` — what this object holds under a Private Name, if anything.
    ///
    /// `None` covers both "no private elements at all" and "none under this name", because nothing
    /// distinguishes them: §7.3.31 `PrivateGet` throws for either, and it is the *same* TypeError.
    pub fn private_element(&self, name: SymbolId) -> Option<PrivateElement> {
        self.private
            .as_ref()?
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }

    /// Every private element, for the collector — a value here is reachable and nothing else holds it.
    pub(super) fn private_elements(&self) -> &[(SymbolId, PrivateElement)] {
        match &self.private {
            Some(elements) => elements,
            None => &[],
        }
    }

    /// The object this method was defined on — `[[HomeObject]]`, which is the super base minus a step.
    pub fn home_object(&self) -> Option<ObjectId> {
        self.home
    }

    /// What this function took from around it, if it is an arrow.
    ///
    /// `None` means the function binds `this` from the call, which is every function but an arrow
    /// — and also every non-function, which has no `this` to speak of either way.
    pub fn lexical(&self) -> Option<Lexical> {
        self.lexical
    }

    /// `[[IsExtensible]]` (§10.1.3).
    pub fn is_extensible(&self) -> bool {
        self.extensible
    }

    /// `[[PreventExtensions]]` (§10.1.4) — and there is no way back.
    ///
    /// Always succeeds, which is why it answers nothing: §10.1.4 returns `true` unconditionally.
    /// Existing properties are untouched — this stops *additions*, and a non-extensible object's
    /// configurable properties may still be deleted and redefined.
    pub fn prevent_extensions(&mut self) {
        self.extensible = false;
    }

    /// `[[GetOwnProperty]]` (§10.1.5) — the property filed under `key`, if there is one.
    ///
    /// Own only: nothing here walks the prototype chain, which is the difference between this and
    /// `[[Get]]`, and the difference `Object.hasOwn` exists to expose.
    pub fn get_own_property(&self, key: PropertyKey) -> Option<&Property> {
        let at = self.position(key)?;
        self.properties.get(at).map(|(_, property)| property)
    }

    /// Where `key` sits in `properties`, by whichever means this object has.
    ///
    /// The one place that decides how a key is found. Written twice — once for the scan and once
    /// for the map — the two could disagree about a key and only one of them would be right.
    fn position(&self, key: PropertyKey) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(&key).copied(),
            None => self
                .properties
                .iter()
                .position(|(stored, _)| *stored == key),
        }
    }

    /// Build the index of every key's position, or rebuild one whose positions have moved.
    fn reindex(&mut self) {
        self.index = Some(Box::new(
            self.properties
                .iter()
                .enumerate()
                .map(|(at, (key, _))| (*key, at))
                .collect(),
        ));
    }

    /// File `property` under `key`, replacing whatever was there.
    ///
    /// The write half of `[[DefineOwnProperty]]`, and private because it is only correct after
    /// [`validate`] has agreed. A new key goes on the end, which is what makes the `Vec` the
    /// creation order §10.1.11 asks for.
    fn insert(&mut self, key: PropertyKey, property: Property) {
        if let Some(at) = self.position(key) {
            // A key that is already here keeps its place: §10.1.11's order is *creation* order,
            // so writing to a property again must not move it to the end.
            if let Some((_, existing)) = self.properties.get_mut(at) {
                *existing = property;
            }
            return;
        }
        let at = self.properties.len();
        self.properties.push((key, property));
        match &mut self.index {
            // Appending disturbs no existing position, so the index only gains an entry.
            Some(index) => {
                index.insert(key, at);
            }
            None if self.properties.len() > INDEXED_ABOVE => self.reindex(),
            None => {}
        }
    }

    /// `[[Delete]]` (§10.1.10) — remove the own property `key`, if it may be removed.
    ///
    /// A key that is not there answers `true`: deleting nothing succeeds, which is why
    /// `delete o.nothing` is `true` and says nothing about whether `o.nothing` existed.
    pub fn delete(&mut self, key: PropertyKey) -> bool {
        let Some(at) = self.position(key) else {
            return true;
        };
        if !self.properties[at].1.configurable {
            return false;
        }
        self.properties.remove(at);
        // Removing shifts every position after it, so the index is now wrong about all of them —
        // and wrong here means finding a *neighbouring* property rather than finding none, which
        // is the kind of error that reads as a plausible value. Rebuilding costs what the removal
        // already cost.
        if self.index.is_some() {
            self.reindex();
        }
        true
    }

    /// `[[OwnPropertyKeys]]` (§10.1.11) — every own key, in the order the language guarantees.
    ///
    /// Array indices first in ascending numeric order, then every other String key in the order
    /// its property was created. That is why `{b: 1, 2: 2, a: 3, 1: 4}` enumerates as
    /// `1, 2, b, a`, and it is a guarantee rather than an implementation detail: the ordering was
    /// written into the specification in ES2015 because every engine already did it.
    ///
    /// Note *array* index, not integer index. `"4294967295"` is one too large to be an array
    /// index, so it sorts with the strings — the same boundary [`PropertyKey::as_array_index`]
    /// draws, and observable through this.
    /// A String object's characters are *not* among these, because they are not stored and this
    /// cannot make the keys that would name them. [`Heap::own_property_keys`] is the whole answer;
    /// this is the part of it the collector and the engine's own walks need, and neither of those
    /// cares about a character.
    pub fn own_property_keys(&self, heap: &Heap) -> Vec<PropertyKey> {
        let mut indices: Vec<(u32, PropertyKey)> = Vec::new();
        let mut names: Vec<PropertyKey> = Vec::new();
        // §10.1.11 step 4 — every Symbol key comes after every String one, in the order they were
        // added. A third list rather than a sort, because the order *within* each group is
        // insertion order and a sort would have to be told not to disturb it.
        let mut symbols: Vec<PropertyKey> = Vec::new();
        for (key, _) in &self.properties {
            match (key.as_symbol(), key.as_array_index(heap)) {
                (Some(_), _) => symbols.push(*key),
                (None, Some(index)) => indices.push((index, *key)),
                (None, None) => names.push(*key),
            }
        }
        // Ascending *numeric* order, which is why the index came back as a number: sorting the
        // keys as text would put "10" before "9".
        indices.sort_unstable_by_key(|(index, _)| *index);
        indices
            .into_iter()
            .map(|(_, key)| key)
            .chain(names)
            .chain(symbols)
            .collect()
    }

    /// How many own properties there are.
    ///
    /// For tests and for whatever reports on the heap. Counts every own property, enumerable or
    /// not — `Object.keys().length` is a different and smaller number.
    pub fn property_count(&self) -> usize {
        self.properties.len()
    }
}

impl Heap {
    /// Put a function object on the heap — `OrdinaryFunctionCreate` (§10.2.3), in the part that
    /// is about the object rather than about the environment.
    ///
    /// Ordinary in every way but one: it has a `[[Call]]`, which is what makes `typeof` say
    /// `"function"` and what a call expression looks for.
    ///
    /// `lexical` is `Some` only for an arrow, and holds what was in force where the arrow
    /// was written — §10.2.3 step 6's `[[ThisMode]]` of `lexical`, captured rather than resolved.
    /// Every other function is handed its `this` by the call, so it passes `None`.
    pub fn new_function(
        &mut self,
        prototype: ObjectId,
        body: Rc<Chunk>,
        environment: EnvironmentId,
        lexical: Option<Lexical>,
    ) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Bytecode(body));
        object.environment = Some(environment);
        // An arrow's home comes from the same capture as its `this`, so the three cannot be captured
        // separately and disagree about which method the arrow was written in. A method's own home is
        // set afterwards by §9.1.1.3's `MakeMethod`, which is a different moment and a different
        // object — see [`Heap::set_home_object`].
        object.home = lexical.and_then(|captured| captured.home);
        object.lexical = lexical;
        self.objects.push(Some(object));
        id
    }

    /// Put one of §27.5.1's three resumption methods on the heap.
    ///
    /// A function object in every respect a script can ask about — it has a `[[Call]]`, `typeof`
    /// answers `"function"`, and it is not a constructor. What it does *not* have is a Rust body:
    /// see [`Callable::Resume`] for why resuming a generator cannot be one.
    pub(crate) fn new_resume_function(
        &mut self,
        prototype: ObjectId,
        kind: crate::heap::Resumption,
    ) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Resume(kind));
        self.objects.push(Some(object));
        id
    }

    /// One of §27.7.5.3's two resumption closures, as a function object.
    ///
    /// Reachable from nothing a script can name: it exists to be handed to `PerformPromiseThen` and
    /// called once by a job. It has no `name` and no `length`, which nothing can ask for.
    pub(crate) fn new_revive_function(
        &mut self,
        context: ObjectId,
        kind: crate::heap::ReactionKind,
    ) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(None);
        object.call = Some(Callable::Revive { kind, context });
        self.objects.push(Some(object));
        id
    }

    /// Put a built-in function object on the heap — `CreateBuiltinFunction` (§10.3.4).
    ///
    /// No environment, because there is nothing lexical about it: a built-in's behaviour is Rust
    /// and closes over nothing. That is the field a JavaScript function needs and this one does
    /// not, and leaving it empty is what says so.
    ///
    /// The `name` and `length` §10.3.3 requires are properties like any others and are given by
    /// the caller, because only the caller knows them.
    pub fn new_native_function(&mut self, prototype: ObjectId, native: Native) -> ObjectId {
        self.built_in(prototype, native, false)
    }

    /// The same, for a built-in that §10.3.2 gives a `[[Construct]]` — a *constructor*.
    ///
    /// Separate from [`Heap::new_native_function`] rather than a flag at every call site, because
    /// the two are unequal in number: nearly every built-in is a method and cannot be constructed,
    /// and defaulting the other way would make `new Math.max()` an object rather than the
    /// TypeError §10.3 asks for.
    pub fn new_native_constructor(&mut self, prototype: ObjectId, native: Native) -> ObjectId {
        self.built_in(prototype, native, true)
    }

    /// `CreateBuiltinFunction` (§10.3.4), for both kinds.
    fn built_in(&mut self, prototype: ObjectId, native: Native, constructs: bool) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.call = Some(Callable::Native { native, constructs });
        self.objects.push(Some(object));
        id
    }

    /// Give an object that already exists a `[[Call]]` running `native`.
    ///
    /// For §10.5 alone. Every other callable is *made* callable, because what it runs is decided
    /// with it; a proxy is made first and then finds out whether its target was a function, and
    /// §10.5 says it has a `[[Call]]` exactly when the target did.
    pub fn make_callable(&mut self, object: ObjectId, native: Native, constructs: bool) {
        if let Some(found) = self.object_mut(object) {
            found.call = Some(Callable::Native { native, constructs });
        }
    }

    /// Put a bound function on the heap — `BoundFunctionCreate` (§10.4.1.3).
    ///
    /// Its prototype is the *target's*, not `Function.prototype`: §10.4.1.3 step 1 takes it from
    /// the function being bound, so `f.bind(o)` inherits from whatever `f` did.
    ///
    /// No environment and no code of its own. A bound function has nothing to close over — what
    /// it holds is another function and the two things a call to it is already decided about.
    pub fn new_bound_function(&mut self, prototype: Option<ObjectId>, bound: Bound) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(prototype);
        object.call = Some(Callable::Bound(bound));
        self.objects.push(Some(object));
        id
    }

    /// Put a wrapper for a primitive on the heap — §20.3.1.1, §21.1.1.1 and §22.1.1.1.
    ///
    /// Ordinary in every way but one: it remembers a primitive, and the methods of the matching
    /// prototype are the only things that read it. Nothing about the *object* changes — a wrapper
    /// has ordinary properties, an ordinary prototype and no exotic behaviour, which is why
    /// `new Number(1).x = 2` works exactly as it does on `{}`.
    pub fn new_wrapper(&mut self, prototype: ObjectId, primitive: Value) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.primitive = Some(primitive);
        self.objects.push(Some(object));
        id
    }

    /// Put a Date on the heap — §21.4.2.1's `OrdinaryCreateFromConstructor` with `[[DateValue]]`.
    ///
    /// `time` may be NaN, and that is a Date rather than a failure: §21.4.1.31's `TimeClip` answers
    /// NaN for anything out of range, and the object it lands in is a perfectly ordinary Date whose
    /// every getter reports NaN. There is no separate "invalid" state to represent.
    pub fn new_date(&mut self, prototype: ObjectId, time: f64) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.date = Some(time);
        self.objects.push(Some(object));
        id
    }

    /// §10.4.3.4 `StringCreate` — a String exotic object over `data`.
    ///
    /// `length` is put there for real, because it is an ordinary property that never changes; the
    /// characters are not: those are answered from `data` itself, every time they are asked for.
    ///
    /// Every character is interned on the way past. That is what lets a *read* of `s[0]` be a read:
    /// the one-character String it must answer with already exists, so no shared borrow ever has to
    /// make one. There are at most 65,536 distinct one-unit Strings, so what this can add to the
    /// heap over a whole program is bounded however many String objects are made.
    pub fn new_string_object(&mut self, prototype: ObjectId, data: StringId) -> ObjectId {
        let units = self.string(data).unwrap_or(&[]).to_vec();
        for unit in &units {
            self.intern(&[*unit]);
        }
        let id = self.new_wrapper(prototype, Value::String(data));
        let units16: Vec<u16> = "length".encode_utf16().collect();
        let length = PropertyKey::from_units(self, &units16);
        let count = f64::from(u32::try_from(units.len()).unwrap_or(u32::MAX));
        // §10.4.3.4 step 5 — all three attributes false, which is why `s.length = 9` is refused
        // and `delete s.length` answers false. The define cannot be refused on an object made a
        // moment ago, so its answer is not worth asking about.
        self.define_own_property(
            id,
            length,
            &PropertyDescriptor {
                value: Some(Value::Number(count)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        );
        id
    }

    /// `[[OwnPropertyKeys]]` — §10.1.11, and §10.4.3.1 when the object is a String.
    ///
    /// Everything a program can see, which is more than [`Object::own_property_keys`] can answer:
    /// a String object's characters are own keys and naming one means making the String `"0"`, so
    /// this is where the question is asked from and why it needs the heap by exclusive reference.
    pub fn own_property_keys(&mut self, object: ObjectId) -> Vec<PropertyKey> {
        let stored = self
            .object(object)
            .map_or_else(Vec::new, |found| found.own_property_keys(self));
        // §10.4.5.6 — a TypedArray's indices, which nothing stored, ahead of everything that was.
        // In order and complete: §10.1.11 wants the integer indices first and ascending, and no
        // stored key can sort in among them because a define at an index never stores anything.
        if let Some(view) = self.object(object).and_then(Object::view)
            && view.element.is_some()
        {
            let count = view.count();
            let mut keys = Vec::new();
            for index in 0..u32::try_from(count).unwrap_or(u32::MAX) {
                keys.push(self.index_key(index));
            }
            keys.extend(stored);
            return keys;
        }
        let Some(data) = self.object(object).and_then(Object::string_data) else {
            return stored;
        };
        // Ahead of the stored keys and in order, which is §10.1.11's ascending run of indices: a
        // String object's own stored indices are all past its last character, because a define
        // *at* a character is refused, so nothing stored can sort in among these.
        let count = u32::try_from(string_object::length(self, data)).unwrap_or(u32::MAX);
        let mut keys = Vec::with_capacity(count as usize + stored.len());
        for index in 0..count {
            keys.push(self.index_key(index));
        }
        keys.extend(stored);
        keys
    }

    /// `[[Delete]]` — §10.1.10, and §10.4.3.6 when the object is a String.
    ///
    /// A String object's characters are not configurable, so deleting one answers `false` and
    /// removes nothing. [`Object::delete`] cannot tell: it looks for a stored property, finds none,
    /// and says `true` on the grounds that what is not there cannot be in the way.
    pub fn delete_own_property(&mut self, object: ObjectId, key: PropertyKey) -> bool {
        // §10.4.5.4 — an index the view *has* cannot be deleted, and one it has not is already
        // gone. Both answers come from the same place and they are opposite: `delete ta[0]` is
        // false on a non-empty array and `delete ta[99]` is true, because deleting nothing
        // succeeded vacuously.
        if let Some(view) = self.object(object).and_then(Object::view)
            && view.element.is_some()
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            return index.is_err();
        }
        if let Some(data) = self.object(object).and_then(Object::string_data)
            && string_object::character(self, data, key).is_some()
        {
            return false;
        }
        self.object_mut(object)
            .is_some_and(|found| found.delete(key))
    }

    /// The one-character String at `index` of `data`, interned so a later read can find it.
    ///
    /// §10.4.3.5's value, for the reader that has a String *primitive* rather than an object and
    /// so has nowhere the characters were interned from.
    pub fn intern_character(&mut self, data: StringId, index: u32) -> Option<StringId> {
        string_object::intern_character(self, data, index)
    }

    /// §23.1.5.1 `CreateArrayIterator` and §22.1.5.1 `CreateStringIterator` — an iterator object.
    ///
    /// Ordinary but for the position it remembers, which is a slot rather than a property so that
    /// nothing in the language can move it. See [`crate::heap::Iteration`].
    pub fn new_iterator(&mut self, prototype: ObjectId, iteration: Iteration) -> ObjectId {
        let id = ObjectId(self.objects.len());
        let mut object = Object::new(Some(prototype));
        object.iteration = Some(Box::new(iteration));
        self.objects.push(Some(object));
        id
    }

    /// Put an ordinary object on the heap — `OrdinaryObjectCreate` (§10.1.12).
    pub fn new_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let id = ObjectId(self.objects.len());
        self.objects.push(Some(Object::new(prototype)));
        id
    }

    /// The object `id` refers to, or `None` if this heap has nothing there.
    ///
    /// The same narrow promise [`Heap::string`] makes about a foreign handle, for the same
    /// reason: no panic and no out-of-range read, and no detection.
    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0)?.as_ref()
    }

    /// The object `id` refers to, to be changed.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id.0)?.as_mut()
    }

    /// Park `parked` in whatever `holder` names, and answer whether anything could hold it.
    ///
    /// `false` for a value that is not an object, which is the only way the answer is interesting:
    /// nothing in the language can name an object this heap does not have, so the other half of
    /// the lookup is a shape only a hand-written chunk reaches.
    ///
    /// Whatever was there is replaced. A holder that is already parked is not a state the
    /// generator machinery above this can produce — §27.5.1.2 refuses to resume one twice — so
    /// there is nothing here to refuse.
    pub(crate) fn park_into(&mut self, holder: Value, parked: crate::vm::Suspended) -> bool {
        let Some(object) = self.holder_mut(holder) else {
            return false;
        };
        object.suspension = Some(Box::new(parked));
        true
    }

    /// Take the execution parked in `holder`, leaving it holding none.
    ///
    /// `None` for a value that is not an object and for an object that has nothing parked — which
    /// includes one that was parked and has already been revived, since a suspension is *moved*
    /// out. That is the property the state machine above this rests on: an execution cannot be
    /// entered twice, because after the first entry it is no longer anywhere to be found.
    pub(crate) fn take_parked(&mut self, holder: Value) -> Option<crate::vm::Suspended> {
        let parked = self.holder_mut(holder)?.suspension.take()?;
        Some(*parked)
    }

    /// Mark `object` as holding a suspendable execution — given once and never taken away.
    pub(crate) fn brand_suspendable(&mut self, object: ObjectId, kind: Suspendable) {
        if let Some(object) = self.object_mut(object) {
            object.suspendable = Some(kind);
        }
    }

    /// The object a value names, to be changed — `None` if it names none.
    fn holder_mut(&mut self, holder: Value) -> Option<&mut Object> {
        match holder {
            Value::Object(id) => self.object_mut(id),
            _ => None,
        }
    }

    /// How many objects this heap holds.
    pub fn object_count(&self) -> usize {
        self.objects.iter().filter(|slot| slot.is_some()).count()
    }

    /// `[[DefineOwnProperty]]` (§10.1.6) — apply `descriptor` to `object`'s `key`, if the rules
    /// allow it.
    ///
    /// Answers whether it was allowed. It does **not** throw: §10.1.6 returns a Boolean, and
    /// turning a `false` into a TypeError is the caller's decision — `Object.defineProperty`
    /// throws, `Reflect.defineProperty` hands the Boolean back, and an assignment in sloppy code
    /// does neither.
    ///
    /// Here rather than on [`Object`] because §10.1.6.3 compares values with `SameValue`, and two
    /// Strings are the same value when their code units are — which is a question only the heap
    /// can answer. An object cannot hold the heap it lives in, so the operation lives outside and
    /// takes both.
    pub fn define_own_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> bool {
        self.define_property_outcome(object, key, descriptor) == DefineOutcome::Defined
    }

    /// §10.4.4.2 — what a define does to an argument index that is joined to a parameter.
    ///
    /// Three rules, and each is about the link rather than about the property. A value written to
    /// a joined index goes to the *parameter*. Making the index an accessor breaks the link,
    /// because a parameter is not an accessor and could not stand in for one. Making it
    /// non-writable breaks the link too, and §10.4.4.2 is careful about the order: the value is
    /// written first, so `Object.defineProperty(arguments, '0', {value: 2, writable: false})`
    /// leaves the parameter at 2 and *then* stops following it.
    fn settle_argument(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) {
        if self
            .object(object)
            .and_then(Object::arguments_map)
            .is_none()
        {
            return;
        }
        if descriptor.is_accessor_descriptor() {
            self.unmap_argument(object, key);
            return;
        }
        if let Some(value) = descriptor.value {
            self.write_through(object, key, value);
        }
        if descriptor.writable == Some(false) {
            self.unmap_argument(object, key);
        }
    }

    /// §7.2.2 `IsArray` — an Array, or a proxy standing in front of one.
    ///
    /// The one question about a proxy that needs no interpreter: §7.2.2 does not consult the
    /// handler at all, it looks straight through to `[[ProxyTarget]]`. So `Array.isArray` of a
    /// proxy over an array is `true` however the handler is written, and there is no trap that can
    /// change it — which is what lets `JSON.stringify` tell an array from an object safely.
    ///
    /// A revoked proxy is a TypeError rather than `false`, because there is no target left to ask.
    /// Iterative, because a proxy's target may be another proxy and a program chooses how many.
    pub fn is_array_through(&self, object: ObjectId) -> crate::value::Completion<bool> {
        let mut walk = object;
        loop {
            // No guard for an id this heap has not got: it is not an array, and it is not a proxy
            // standing in front of one either, so both answers below are already right.
            let Some(proxy) = self.object(walk).and_then(Object::proxy) else {
                return Ok(self.object(walk).is_some_and(Object::is_array));
            };
            let Some(target) = proxy.target() else {
                return Err(crate::value::Abrupt::type_error(
                    "Array.isArray cannot ask a revoked proxy what it stands in front of",
                ));
            };
            walk = target;
        }
    }

    /// `IsCompatiblePropertyDescriptor` (§6.2.6.4) — would this change be allowed, without making it?
    ///
    /// §6.2.6.4 is `ValidateAndApplyPropertyDescriptor` with no object to write to, and it exists
    /// for §10.5 alone: a proxy trap describes a property the *target* does not have to hold, and
    /// the only question is whether that description could have been true of the target. Nothing
    /// else in the language needs to ask a question about a property it is not about to change.
    #[must_use]
    pub fn is_compatible_descriptor(
        &self,
        descriptor: &PropertyDescriptor,
        current: Option<&Property>,
        extensible: bool,
    ) -> bool {
        !matches!(
            crate::heap::define::validate(descriptor, current, extensible, self),
            crate::heap::define::Validation::Reject
        )
    }

    /// `[[DefineOwnProperty]]`, with the one answer a Boolean cannot carry.
    ///
    /// §10.4.2.4 step 2's bad array length is a **RangeError** and every other refusal is a
    /// `false` that sloppy code ignores. A caller that can throw asks this; one that only wants
    /// to know whether the property is now there asks [`Heap::define_own_property`].
    pub fn define_property_outcome(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> DefineOutcome {
        // §10.4.2.1 — an Array's is not the ordinary one. Dispatching here rather than at every
        // call site is what makes `a[0] = 1` and `Object.defineProperty(a, "0", …)` agree about
        // what happens to `length`, which they must.
        if self.object(object).is_some_and(Object::is_array) {
            return self.define_array_property(object, key, descriptor);
        }
        // §10.4.5.3 — a define at a canonical numeric index of a TypedArray. An index the view
        // does not have is **refused** rather than stored, which is where this differs from every
        // ordinary object: `Object.defineProperty(ta, "99", …)` on a short array fails, because a
        // TypedArray's length cannot change and a property there would be a length that lied.
        if let Some(view) = self.object(object).and_then(Object::view)
            && view.element.is_some()
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            let Ok(at) = index else {
                return DefineOutcome::Refused;
            };
            // An element is a writable, enumerable, configurable data property and can be nothing
            // else, so a descriptor asking for an accessor or for any other attributes is refused.
            // One that asks only for a *value* is the ordinary write.
            if descriptor.getter.is_some()
                || descriptor.setter.is_some()
                || descriptor.writable == Some(false)
                || descriptor.enumerable == Some(false)
                || descriptor.configurable == Some(false)
            {
                return DefineOutcome::Refused;
            }
            if let Some(value) = descriptor.value {
                let number = match value {
                    crate::value::Value::Number(number) => number,
                    // A define carries a value that is already a Value, so there is no conversion
                    // to run here and nothing that could throw — anything that is not a Number
                    // writes as `NaN` would, which is what `ToNumber` of it would give for the
                    // types a define can carry without a coercion step of its own.
                    _ => f64::NAN,
                };
                let clamped = self.object(object).is_some_and(Object::is_clamped);
                self.set_element(view, at, number, clamped);
            }
            return DefineOutcome::Defined;
        }
        // §10.4.3.3 — a define at one of a String object's characters never stores anything. It
        // is allowed only when it describes the property that is already there, and refused
        // otherwise, which is what makes `s[0] = "z"` do nothing at all.
        if let Some(data) = self.object(object).and_then(Object::string_data)
            && let Some(current) = string_object::character(self, data, key)
        {
            return DefineOutcome::from(string_object::define_is_allowed(
                self, &current, descriptor,
            ));
        }
        let defined = self.define_ordinary_property(object, key, descriptor);
        // §10.4.4.2 steps 3 to 5 — only when the define was allowed. A refused define changes
        // nothing, and must not break a link either.
        if defined {
            self.settle_argument(object, key, descriptor);
        }
        DefineOutcome::from(defined)
    }

    /// §10.1.6.3 `OrdinaryDefineOwnProperty` — the rules every object but an Array uses whole,
    /// and the ones an Array uses after it has moved its `length`.
    pub(super) fn define_ordinary_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        descriptor: &PropertyDescriptor,
    ) -> bool {
        let Some(found) = self.object(object) else {
            return false;
        };
        // Copied out so the validation below may read the heap: a `Property` is `Copy` precisely
        // so that this costs nothing.
        let current = found.get_own_property(key).copied();
        let extensible = found.is_extensible();
        match validate(descriptor, current.as_ref(), extensible, self) {
            Validation::Reject => false,
            Validation::AcceptUnchanged => true,
            Validation::Accept => {
                let updated = apply(descriptor, current.as_ref());
                // The object was found above and an arena only grows, so this cannot be absent —
                // and the answer does not depend on it. Writing `None => false` here would be a
                // branch no input could take, and one that would report a refusal the rules did
                // not make.
                if let Some(found) = self.object_mut(object) {
                    found.insert(key, updated);
                }
                true
            }
        }
    }

    /// `OrdinaryHasProperty` (§10.1.7.1) — whether `object` or anything it inherits from has `key`.
    ///
    /// Walks the prototype chain, which is why it is here and not on [`Object`]: an object cannot
    /// see its own prototype's properties without the heap they both live in.
    ///
    /// The walk is bounded by the chain being acyclic, which
    /// [`Heap::set_prototype_of`] is what guarantees — and by a step count besides, because a
    /// guarantee that depends on every other path being correct is not one this may rely on.
    pub fn has_property(&self, object: ObjectId, key: PropertyKey) -> bool {
        self.find_own(object, key).is_some()
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// What `[[Get]]` will need once calling exists: the property *and* which object it came
    /// from, since an accessor's getter is called with that object as its receiver.
    /// An object's own property, with §10.4.4's map consulted — `[[GetOwnProperty]]`.
    ///
    /// The same answer as the object's own table for everything but a joined argument index,
    /// where the *value* comes from the parameter instead. §10.4.4.1 says exactly this: the
    /// descriptor is the ordinary one with its value replaced, which is why
    /// `Object.getOwnPropertyDescriptor(arguments, 0)` reports a data property and not the
    /// accessor the specification's own note implements the map with.
    pub fn own_property(&self, object: ObjectId, key: PropertyKey) -> Option<Property> {
        let found = self.object(object)?;
        // §10.4.5.1 — a TypedArray's elements are answered from the buffer and are never stored, so
        // this comes *before* the table rather than after it: a canonical numeric index is an
        // element whatever the table happens to hold, and one out of range is absent.
        if let Some(view) = found.view()
            && view.element.is_some()
            && let Some(at) = typed::index_of(self, key, view.count())
        {
            return at.ok().and_then(|at| self.element_property(view, at));
        }
        let Some(property) = found.get_own_property(key).copied() else {
            // §10.4.3.5 — nothing stored, which for a String object is where its characters are.
            return string_object::character(self, found.string_data()?, key);
        };
        let Some(map) = found.arguments_map() else {
            return Some(property);
        };
        let Some(slot) = arguments::index_of(self, key).and_then(|index| map.slot(index)) else {
            return Some(property);
        };
        // A joined index is never uninitialised: a parameter is given its value when the call
        // begins, and nothing can put one back into the dead zone.
        let Some(Some(value)) = self.variable(map.environment, slot) else {
            return Some(property);
        };
        Some(Property {
            kind: match property.kind {
                PropertyKind::Data { writable, .. } => PropertyKind::Data { value, writable },
                accessor => accessor,
            },
            ..property
        })
    }

    /// Break the link between an argument index and its parameter — §10.4.4.2 and §10.4.4.5.
    ///
    /// Answers whether there was one, so that a caller which has just changed a property can say
    /// what it did without asking twice.
    pub(crate) fn unmap_argument(&mut self, object: ObjectId, key: PropertyKey) {
        let Some(index) = arguments::index_of(self, key) else {
            return;
        };
        if let Some(map) = self
            .object_mut(object)
            .and_then(|found| found.arguments.as_deref_mut())
        {
            map.unmap(index);
        }
    }

    /// Write through to the parameter an argument index is joined to, if it is joined to one.
    ///
    /// Answers nothing. Whether it wrote is not a question anyone asks — the caller has just been
    /// told the define was allowed, and a key that is not a joined index simply has no parameter
    /// behind it. A return value nobody reads is one no test could be wrong about.
    fn write_through(&mut self, object: ObjectId, key: PropertyKey, value: Value) {
        let Some(index) = arguments::index_of(self, key) else {
            return;
        };
        let Some(map) = self.object(object).and_then(Object::arguments_map) else {
            return;
        };
        let (Some(slot), environment) = (map.slot(index), map.environment) else {
            return;
        };
        self.set_variable(environment, slot, value);
    }

    /// Put an arguments object on the heap — §10.4.4.4 `CreateMappedArgumentsObject`.
    ///
    /// The values are the arguments the call was given, all of them; the map joins the first
    /// `parameters` of them to the slots of `environment`. `callee` is the function itself, which
    /// §10.4.4.4 step 15 gives a mapped arguments object and an unmapped one refuses to.
    pub fn new_arguments(&mut self, prototype: ObjectId, call: &Incoming<'_>) -> ObjectId {
        let &Incoming {
            environment,
            values,
            parameters,
            callee,
            thrower,
            mapped,
        } = call;
        let object = self.new_object(Some(prototype));
        for (at, value) in values.iter().enumerate() {
            let index = u32::try_from(at).unwrap_or(u32::MAX);
            let key = self.index_key(index);
            self.define_own_property(object, key, &PropertyDescriptor::data(*value));
        }
        // §10.4.4.4 step 14 — `length` is an ordinary §17 property: writable and configurable,
        // and never enumerable, so `for`-`in` over `arguments` walks the indices and nothing else.
        let key = PropertyKey::from_units(self, &"length".encode_utf16().collect::<Vec<_>>());
        self.define_own_property(
            object,
            key,
            &PropertyDescriptor {
                enumerable: Some(false),
                ..PropertyDescriptor::data(Value::Number(values.len() as f64))
            },
        );
        let key = PropertyKey::from_units(self, &"callee".encode_utf16().collect::<Vec<_>>());
        let callee = match mapped {
            // §10.4.4.4 step 15 — the function itself, on a mapped object.
            true => PropertyDescriptor {
                enumerable: Some(false),
                ..PropertyDescriptor::data(Value::Object(callee))
            },
            // §10.4.4.6 step 6 — and on an *unmapped* one it is poisoned: an accessor pair of
            // %ThrowTypeError% for both halves, so reading it or writing it throws. That is a
            // deliberate refusal rather than an omission — a function with a default parameter is
            // ES2015 code, and `arguments.callee` is the idiom ES2015 was closing off.
            false => PropertyDescriptor {
                getter: Some(Value::Object(thrower)),
                setter: Some(Value::Object(thrower)),
                enumerable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        };
        self.define_own_property(object, key, &callee);
        // §10.2.11 step 22 — the map is only made for a *simple* parameter list. Anything else
        // gets §10.4.4.4's unmapped object: a parameter that a default filled in is not a slot an
        // index could stand for, and joining them would make `arguments[0] = 1` reach past the
        // code that decided what the parameter was.
        //
        // Joined *after* the properties are made, because making them goes through the define
        // below — and a define on a joined index writes through to a parameter instead.
        //
        // The slot is present either way, and that is not a detail: §20.1.3.6 step 8 tags an
        // object `Arguments` because it *has* a `[[ParameterMap]]`, not because the map joins
        // anything. An unmapped object gets one that joins nothing, which is what §10.4.4.6's
        // "set to undefined" behaves as.
        let joined = match mapped {
            true => parameters.min(values.len()),
            false => 0,
        };
        if let Some(found) = self.object_mut(object) {
            found.arguments = Some(Box::new(ArgumentsMap::new(environment, joined)));
        }
        object
    }

    /// §10.4.5.4 — whether a prototype walk for `key` must stop at this object.
    ///
    /// True only for a canonical numeric index of a TypedArray, which is an *element* whether or
    /// not the array has one: `ta[99]` on a short array is `undefined` and never the property
    /// somebody put at `Int32Array.prototype[99]`. Every walk in the engine has to know this, and
    /// the ones in [`crate::vm::Vm`] cannot use [`Heap::find_own`] to learn it because they have a
    /// proxy trap to ask at each step.
    #[must_use]
    pub fn walk_stops_here(&self, object: ObjectId, key: PropertyKey) -> bool {
        self.object(object).is_some_and(|found| {
            found.view().is_some_and(|view| {
                view.element.is_some() && typed::index_of(self, key, view.count()).is_some()
            })
        })
    }

    /// The object along `object`'s prototype chain that owns `key`, if any.
    ///
    /// The property *and* which object it came from, since an accessor's getter is called with
    /// that object as its receiver.
    ///
    /// Asked through [`Heap::own_property`] rather than the object's own table, so that a joined
    /// argument index answers with its parameter's value however the read arrived.
    pub fn find_own(&self, object: ObjectId, key: PropertyKey) -> Option<(ObjectId, Property)> {
        // §10.4.5.4 — a canonical numeric index of a TypedArray never reaches the prototype, even
        // when the array does not have it. `ta[99]` on a short array is `undefined` and not an
        // inherited property, which is the whole reason this stops here rather than answering
        // `None` and letting the walk continue: a program that puts something at
        // `Int32Array.prototype[9]` must not have it show up as an element.
        if let Some(view) = self.object(object)?.view()
            && view.element.is_some()
            && let Some(index) = typed::index_of(self, key, view.count())
        {
            return index
                .ok()
                .and_then(|at| self.element_property(view, at))
                .map(|property| (object, property));
        }
        let mut cursor = Some(object);
        // The chain cannot be a cycle — nothing can build one — and this counts anyway. DR-0002
        // is not a claim about the code being right; it is a claim that being wrong does not
        // hang. See [`Heap::set_prototype_of`] for the check that makes the count unreachable.
        for _ in 0..MAX_PROTOTYPE_CHAIN {
            let at = cursor?;
            if let Some(property) = self.own_property(at, key) {
                return Some((at, property));
            }
            cursor = self.object(at)?.prototype();
        }
        None
    }

    /// §7.3.29 `PrivateFieldAdd` — add a private field, answering whether it was not already there.
    ///
    /// `false` means the object already carries this Private Name, which §7.3.29 makes a TypeError.
    /// Reachable from source in exactly one way: a constructor that calls itself on the same object,
    /// as in `class C { #x; constructor() { C.prototype.constructor.call(this); } }` — so the guard is
    /// not defensive, it is the specification's step 3.
    pub fn add_private_field(&mut self, object: ObjectId, name: SymbolId, value: Value) -> bool {
        self.add_private_element(object, name, PrivateElement::Field(value))
    }

    /// §7.3.30 `PrivateMethodOrAccessorAdd` — add a method or accessor, answering as §7.3.29 does.
    ///
    /// The same operation and the same one failure, which is why they share a body: the *kind* is all
    /// that differs, and §7.3.30's own step 2 refuses an existing name in the same words §7.3.29 does.
    pub fn add_private_element(
        &mut self,
        object: ObjectId,
        name: SymbolId,
        element: PrivateElement,
    ) -> bool {
        let Some(object) = self.objects.get_mut(object.0).and_then(Option::as_mut) else {
            return false;
        };
        let elements = object.private.get_or_insert_with(Vec::new);
        if elements.iter().any(|(key, _)| *key == name) {
            return false;
        }
        elements.push((name, element));
        true
    }

    /// §7.3.32 `PrivateSet` — write a private field that is already there, answering whether it was.
    ///
    /// It does **not** create. That is the whole difference from a property: `this.#x = 1` on an
    /// object with no `#x` is a TypeError rather than a new field, which is what makes the set of
    /// private names an object carries fixed at construction and usable as a brand.
    pub fn set_private_field(&mut self, object: ObjectId, name: SymbolId, value: Value) -> bool {
        let Some(object) = self.objects.get_mut(object.0).and_then(Option::as_mut) else {
            return false;
        };
        let Some(elements) = object.private.as_mut() else {
            return false;
        };
        match elements.iter_mut().find(|(key, _)| *key == name) {
            // §7.3.32 step 3 — a *field* is the only kind a write may reach through here. A method
            // refuses outright, and an accessor is the interpreter's business because its setter has
            // to be called; both are answered before this is reached, so this arm is a field or the
            // caller has not read the kind.
            Some((_, PrivateElement::Field(held))) => {
                *held = value;
                true
            }
            _ => false,
        }
    }

    /// §9.1.1.3's `MakeMethod` — record which object a function was defined on.
    ///
    /// Not a property and not observable: no script can read `[[HomeObject]]` by any means, and the
    /// only thing that consults it is `super`.
    ///
    /// A handle to nothing is ignored rather than reported. Every caller passes a function it made a
    /// moment earlier, so there is no state in which the answer could be acted on — and a `bool`
    /// nobody could ever see be `false` is a branch no test can pin, which mutation coverage said by
    /// surviving a flip of it.
    pub fn set_home_object(&mut self, function: ObjectId, home: ObjectId) {
        if let Some(object) = self.objects.get_mut(function.0).and_then(Option::as_mut) {
            object.home = Some(home);
        }
    }

    /// `OrdinarySetPrototypeOf` (§10.1.2) — point `object` at `prototype`, if that is allowed.
    ///
    /// Two rules, and the second is the interesting one.
    ///
    /// A non-extensible object's prototype may not be changed — *changed*, not set: §10.1.2 step
    /// 2 returns `true` for setting it to what it already is, before extensibility is looked at
    /// at all. `Object.preventExtensions(o); Object.setPrototypeOf(o, Object.getPrototypeOf(o))`
    /// succeeds.
    ///
    /// And the chain may not become a cycle. §10.1.2 walks the proposed prototype's own chain
    /// looking for `object`, which is the check that makes every prototype walk in the engine
    /// terminate. The specification notes that this only holds while every object on the chain
    /// uses the ordinary method — a Proxy can lie, which is why the walks are bounded as well.
    pub fn set_prototype_of(&mut self, object: ObjectId, prototype: Option<ObjectId>) -> bool {
        // Step 2 — setting it to what it is always succeeds.
        let Some(current) = self.object(object) else {
            return false;
        };
        if current.prototype() == prototype {
            return true;
        }
        // Step 4.
        if !current.is_extensible() {
            return false;
        }
        // Steps 5 to 7 — walk the proposed chain and refuse if it comes back here.
        let mut cursor = prototype;
        for _ in 0..MAX_PROTOTYPE_CHAIN {
            let Some(id) = cursor else {
                break;
            };
            if id == object {
                return false;
            }
            match self.object(id) {
                Some(found) => cursor = found.prototype(),
                None => break,
            }
        }
        // Step 8. As in [`Heap::define_own_property`], the object was found at the top and the
        // arena only grows, so there is nothing here to fail and no refusal left to report.
        if let Some(found) = self.object_mut(object) {
            found.prototype = prototype;
        }
        true
    }
}

/// How far any prototype walk goes before giving up.
///
/// Not a limit the language has: an acyclic chain of a million objects is legal and this would
/// answer wrongly about it. It is a backstop for DR-0002 — a walk that cannot terminate is a hang,
/// and "the cycle check is correct" is exactly the kind of claim that should not be the only thing
/// standing between an engine and one. Every chain a program actually builds is a handful long;
/// the figure is deliberately far above that and deliberately not a guess about correctness.
pub(crate) const MAX_PROTOTYPE_CHAIN: usize = 100_000;

#[cfg(test)]
#[path = "tests.rs"]
mod object_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::PropertyKind;

    fn key(heap: &mut Heap, text: &str) -> PropertyKey {
        PropertyKey::from_units(heap, &text.encode_utf16().collect::<Vec<_>>())
    }

    fn data(value: f64) -> PropertyDescriptor {
        PropertyDescriptor {
            value: Some(Value::Number(value)),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        }
    }

    /// Whether `object` is keeping an index of its keys.
    fn indexed(heap: &Heap, object: ObjectId) -> bool {
        heap.object(object)
            .is_some_and(|found| found.index.is_some())
    }

    #[test]
    fn keys_are_indexed_only_once_there_are_more_of_them_than_it_costs_to_scan() {
        // This test looks at a private field, which the rest of this file's tests are careful not
        // to do — and the reason is the point. The index changes no answer: every question about
        // a property has the same answer whether it was found by a scan or by a hash. So no test
        // written in JavaScript can say when one is built, and a policy nothing can observe is a
        // policy nothing is holding in place.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        for at in 0..INDEXED_ABOVE {
            let key = key(&mut heap, &format!("k{at}"));
            heap.define_own_property(object, key, &data(at as f64));
            assert!(
                !indexed(&heap, object),
                "{} properties is still few enough to scan",
                at + 1
            );
        }
        // One more than the threshold, and not one fewer: an object holding exactly
        // `INDEXED_ABOVE` is on the cheap side of the trade.
        let over = key(&mut heap, "one-too-many");
        heap.define_own_property(object, over, &data(99.0));
        assert!(indexed(&heap, object));

        // A small object that has something deleted does not acquire one on the way past — the
        // rebuild after a delete is for an index that already exists, not a reason to build one.
        let small = heap.new_object(None);
        let only = key(&mut heap, "only");
        heap.define_own_property(small, only, &data(1.0));
        assert!(
            heap.object_mut(small)
                .is_some_and(|found| found.delete(only))
        );
        assert!(!indexed(&heap, small));
    }

    #[test]
    fn an_indexed_object_finds_every_key_it_still_has_after_a_delete() {
        // The failure this guards against is not "cannot find it" — it is finding the *wrong*
        // one. Removing a property shifts every position after it, so an index left unrebuilt
        // answers each of those keys with its neighbour: a plausible value, not a crash.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        let count = INDEXED_ABOVE * 3;
        let keys: Vec<_> = (0..count)
            .map(|at| {
                let key = key(&mut heap, &format!("k{at}"));
                heap.define_own_property(object, key, &data(at as f64));
                key
            })
            .collect();
        assert!(indexed(&heap, object));

        // Delete from the front, where the most positions move.
        assert!(
            heap.object_mut(object)
                .is_some_and(|found| found.delete(keys[0]))
        );
        for (at, key) in keys.iter().enumerate().skip(1) {
            let found = heap
                .object(object)
                .and_then(|found| found.get_own_property(*key))
                .map(|property| property.kind);
            assert!(
                matches!(
                    found,
                    Some(PropertyKind::Data {
                        value: Value::Number(number),
                        ..
                    }) if number == at as f64
                ),
                "k{at} answered with the wrong property after a delete shifted it"
            );
        }
        assert!(
            heap.object(object)
                .and_then(|found| found.get_own_property(keys[0]))
                .is_none()
        );
    }
}
