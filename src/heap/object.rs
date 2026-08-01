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

use crate::heap::buffer::{Buffer, View};
use crate::heap::collection::Collection;
use crate::heap::promise::{Promise, Role};
use crate::heap::{
    ArgumentsMap, Callable, EnvironmentId, Heap, Helper, Iteration, Property, PropertyKey, Proxy,
    StringId, SymbolId, Weak,
};
use crate::value::Value;
use std::collections::HashMap;

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
    /// §27.6's async generator, which is resumed by both — and is the only one a script can hold.
    ///
    /// A third brand rather than the two together, because it is not the conjunction: an `async`
    /// function's context object is internal and settles one promise, and an async generator is
    /// the object the script was handed and settles one promise *per request*. What they share is
    /// only that a body can be parked, and every object here has that.
    AsyncGenerator,
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
    pub(super) prototype: Option<ObjectId>,
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
    pub(super) call: Option<Callable>,
    /// The environment this function was *written* in — §10.2's `[[Environment]]`.
    ///
    /// A closure is this field. The call that made the function is long gone by the time the
    /// function runs, and the variables it could see are still here because this holds them.
    pub(super) environment: Option<EnvironmentId>,
    /// What an arrow was written beside — §10.2's `[[ThisMode]]` of `lexical`.
    ///
    /// `None` for every function that binds its own, which is all of them but arrows. Present, it
    /// is the same idea as `environment` one field up and for the same reason: the call that made
    /// the arrow is gone by the time the arrow runs, so what it could see has to be *held*
    /// rather than looked for. §9.1.1.3 words it as a function environment with no `[[ThisBinding]]`
    /// whose `ResolveThisBinding` walks outward; the environment that walk arrives at is exactly
    /// the one running when the arrow was made, so recording it here is that walk, done
    /// once and in advance.
    pub(super) lexical: Option<Lexical>,
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
    pub(super) primitive: Option<Value>,
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
    pub(super) suspension: Option<Box<crate::vm::Suspended>>,
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
    pub(super) suspendable: Option<Suspendable>,
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

    /// Whether this is §27.6's async generator — the `RequireInternalSlot` of its three methods.
    pub(crate) fn is_async_generator(&self) -> bool {
        self.suspendable == Some(Suspendable::AsyncGenerator)
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
    pub(super) fn insert(&mut self, key: PropertyKey, property: Property) {
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
    // Named here rather than at the top of the file: §10.1's internal methods moved next door to
    // `ordinary`, and these are the only rows left that build a descriptor by hand.
    use crate::heap::{PropertyDescriptor, PropertyKind};

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
