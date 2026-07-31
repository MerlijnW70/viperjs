//! A unit of compiled code, and the instruction set it is made of.
//!
//! Everything here is what the *interpreter* sees. The compiler's own working — how a name is
//! resolved, what it refuses — is next door; this is the surface between the two, and it is
//! deliberately small: an embedder holds a [`Chunk`], and a chunk holds instructions, constants,
//! a count of slots, and the bodies of the functions written inside it.

use crate::ast::{BinaryOperator, UnaryOperator};
use crate::compile::{CompileError, ErrorKind};
use crate::span::Span;
use crate::value::Value;
use std::rc::Rc;

/// One unit of compiled code — the instructions and the values they refer to.
///
/// Constants are held beside the code rather than inside it because a `Value` is 16 bytes and an
/// instruction should not be. It also means a String literal is put on the heap once, at compile
/// time, rather than each time the line runs.
#[derive(Debug, Default)]
pub struct Chunk {
    pub(super) code: Vec<Instruction>,
    pub(super) constants: Vec<Value>,
    pub(super) locals: usize,
    pub(super) parameters: usize,
    /// §10.2.3's `length` — the count *before* the first default or rest parameter.
    ///
    /// A second number because the two questions differ once a parameter list is not simple:
    /// `function f(a, b = 1, c)` has three slots to fill and a `length` of 1. `length` is what a
    /// caller is told to supply; `parameters` is how many places there are to put things.
    pub(super) length: usize,
    /// §10.2.9's `name`, if the compiler could tell what this function is called.
    ///
    /// The compiler and not the call, because a function has one name for its whole life and the
    /// position it was *written* in is what decides it: a declaration is named by its binding, a method
    /// by its key, and §8.6.3's `NamedEvaluation` gives an anonymous expression the name of whatever it
    /// is being assigned to. One syntactic position, one chunk, one name.
    ///
    /// `None` where §10.2.9 asks for the empty string — an anonymous function that is not in a named
    /// position — because a `None` and an interned `""` are the same thing to the only reader, and one
    /// of the two does not need a String on the heap per anonymous function in the program.
    pub(super) name: Option<crate::heap::StringId>,
    /// Whether this body is strict code — §11.2.1, as the parser computed it.
    ///
    /// Three things turn on it at run time and none of them can be decided anywhere else. §10.2.1.2
    /// step 3 does not substitute the global object for a strict function's `undefined` receiver;
    /// §6.2.5.6 step 6.d **throws** where a sloppy assignment silently does nothing; and §13.5.1.2
    /// does the same for a `delete` that was refused.
    ///
    /// On the chunk because strictness is a property of the *body*, which is what a call has to hand
    /// when it decides the receiver — and because a strict function called from sloppy code is still
    /// strict, so the caller cannot answer for it.
    pub(super) strict: bool,
    /// `IsSimpleParameterList` (§15.1.4) — whether every parameter is a plain name.
    ///
    /// Decides which arguments object §10.2.11 step 22 makes. A simple list gets the *mapped* one,
    /// where an index and its parameter are one variable; anything else gets an unmapped one,
    /// because a parameter initialised by running code is not a slot an index could stand for.
    pub(super) simple_parameters: bool,
    /// The slot a rest parameter's array goes in, if the list has one.
    ///
    /// Filled by the call rather than by the body, on the same terms as the arguments object: the
    /// arguments past the last named parameter are on the stack at entry and nowhere else.
    pub(super) rest: Option<u32>,
    /// Whether this body is an arrow's — §15.3.
    ///
    /// An arrow has no `this` of its own, no `prototype`, and no `[[Construct]]`. All three come
    /// from the same fact: §15.3 makes it a function *expression* over the scope it was written
    /// in rather than a thing you can be inside of, so `this` is whatever it was one line above.
    pub(super) arrow: bool,
    /// Whether this body is a `MethodDefinition` — §15.4.5 and §15.7.14.
    ///
    /// A method has **no `[[Construct]]`** and no `prototype`: `new o.m()` is a TypeError, and
    /// `'prototype' in o.m` is false. An arrow lacks both for a different reason (§15.3 gives it no
    /// `this` either), so the two flags are separate — a method has a `this` and is simply not
    /// constructible.
    ///
    /// A class *constructor* is not a method by this flag, whatever the grammar calls it: it is the
    /// one thing in a class body that constructs.
    pub(super) method: bool,
    /// Whether this body is a class constructor — §15.7.14.
    ///
    /// It has a `[[Construct]]` and no useful `[[Call]]`: written without `new` it is a TypeError.
    /// The flag lives on the body because by the time a call happens the chunk is the only thing
    /// left that could still know.
    pub(super) class_constructor: bool,
    /// Where a derived constructor's `this` lives, if this body is one — DR-0015.
    ///
    /// `None` for every other body, which is where `this` is the register the call set. Present, it
    /// is the slot §9.1.1.3's `[[ThisValue]]` occupies, and the *call* has to know: §10.2.2 does not
    /// create the receiver for a derived constructor, so what would otherwise be made on entry has
    /// to be left for `super()` to make.
    pub(super) derived_this: Option<u32>,
    /// The slot §10.4.4's arguments object goes in, if this body reaches for the name.
    ///
    /// `None` when it does not, and then no object is made: §10.2.11 makes one for every
    /// non-arrow function, and one nothing can read is one nothing can tell was never there. A
    /// call that made one anyway would allocate an object and its properties on every call in the
    /// program, which DR-0013 counts and a benchmark would notice.
    pub(super) arguments: Option<u32>,
    /// Whether this body reads the `arguments` of the function it is written *inside*.
    ///
    /// Only an arrow can: §15.3 gives it no `arguments` of its own, so the name resolves outward
    /// exactly as `this` does. The function around it has to know, because it is the one that has
    /// to build the object — and it finds out from here when the arrow's body comes back compiled.
    pub(super) outer_arguments: bool,
    /// The template objects' contents, one entry per tagged-template *site* in this chunk.
    ///
    /// Held here rather than built at compile time because the object is a frozen Array and belongs to
    /// a *realm*: the same chunk may run in two of them, and each needs its own. See
    /// [`Instruction::TemplateObject`], which builds one and caches it per site.
    pub(super) templates: Vec<Template>,
    /// The bodies of the functions written inside this one, in the order they were met.
    ///
    /// An `Rc` because a function object has to outlive the code that made it — `var f = g()`
    /// keeps a closure alive after `g` has returned — and because a chunk is immutable once
    /// compiled and holds only chunks *below* it. DR-0010 rejects reference counting for the
    /// *heap*, where cycles are made before user code runs; a tree of code has none, so the
    /// argument does not reach here.
    pub(super) functions: Vec<Rc<Chunk>>,
}

/// What a tagged template's object is made of — §13.2.8.3's two arrays.
///
/// The cooked strings and the raw ones, one of each per literal component. A cooked one is `None`
/// where the component holds a `NotEscapeSequence`: §12.9.6 leaves `TV` undefined there, which is
/// legal in a *tagged* template and a Syntax Error in an untagged one, and is why `String.raw` exists.
#[derive(Debug, Clone)]
pub struct Template {
    /// `TV` per component — `None` for one whose escape is not a valid escape.
    pub cooked: Vec<Option<crate::heap::StringId>>,
    /// `TRV` per component, escapes exactly as written.
    pub raw: Vec<crate::heap::StringId>,
}

/// One instruction.
///
/// Deliberately few. An operator is one instruction carrying which operator it is, rather than one
/// instruction per operator: the dispatch inside [`crate::value::apply_binary`] is a `match` on
/// the same value either way, and twenty opcodes would be twenty things to keep in step with the
/// abstract operations rather than one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    /// Push the constant at this index.
    Constant(u32),
    /// Replace a source and a flags string with a new `RegExp` object — §13.2.7.3.
    ///
    /// A **new** object every time, which is why a regular expression written inside a loop does
    /// not carry `lastIndex` from one turn to the next. ES3 shared one object per literal and the
    /// change is observable, so this is an instruction rather than a constant.
    RegExpLiteral,
    /// Replace the value on top of the stack with the result of a unary operator.
    Unary(UnaryOperator),
    /// Replace the top two values with the result of a binary operator, left below right.
    Binary(BinaryOperator),
    /// Continue at this instruction instead of the next one.
    Jump(u32),
    /// Take the top value; if it is falsy, continue at this instruction instead.
    ///
    /// The value is consumed either way. This is the conditional operator's jump, where the test
    /// is asked about and then thrown away.
    JumpIfFalse(u32),
    /// Look at the top value; if the condition holds, continue at this instruction and **leave it
    /// where it is**. Otherwise take it and carry on.
    ///
    /// The short circuits' jump, and the reason they are not `if` in disguise: `a || b` is not
    /// `a ? true : b`, it is *`a` itself* when `a` is truthy. So the value that decided has to
    /// survive being the answer.
    JumpKeeping(ShortCircuit, u32),
    /// Take the top value; if it is truthy, continue at this instruction instead.
    ///
    /// The mirror of [`Instruction::JumpIfFalse`], and what a `do`/`while` loop jumps back with:
    /// its test is at the *bottom*, so the sense of the jump is reversed.
    JumpIfTrue(u32),
    /// Discard the top value.
    Pop,
    /// Push one of §6.1.5.1's well-known Symbols, by its position in the table.
    ///
    /// Not a constant, because a chunk outlives the realm it was compiled against: the same code
    /// may run in two realms, and each has its own `Symbol.iterator`. Reading it from the realm at
    /// the moment it is needed is what makes `for`-`of` reach the right one — and reading it from
    /// the *realm* rather than from the `Symbol` object is what stops a script moving it.
    LoadWellKnown(u32),
    /// Throw a **TypeError** unless the value on top is an Object, and leave it there.
    ///
    /// §7.4.4 step 3 and §7.4.9 step 6. An iterator whose `next` answers a primitive would
    /// otherwise have its `done` read off that primitive's prototype as `undefined` — falsy — and
    /// the loop would never end. A check that turns a hang into an error.
    RequireObject,
    /// §7.3.25 `CopyDataProperties` — a new object holding what a pattern did not name.
    ///
    /// Pops that many excluded keys and then the source, and pushes the object. The keys are
    /// popped rather than named at compile time because a computed one is a value: `{[k]: v,
    /// ...rest}` excludes whatever `k` came to, and evaluating `k` a second time to find out
    /// would run it twice.
    ///
    /// Own *enumerable* properties only, and it **gets** each one — so a getter on the source runs
    /// and its answer is what is copied, which is the same reading `Object.assign` takes.
    CopyRest(u32),
    /// §7.1.2 `RequireObjectCoercible` — throw a **TypeError** for `undefined` or `null`, and
    /// leave anything else where it is.
    ///
    /// Not [`Instruction::RequireObject`], which is stricter: a primitive *is* coercible, so
    /// `var {length: n} = "ab"` reads through the String's object and works. The two are only
    /// both needed because destructuring asks the weaker question and the iterator protocol asks
    /// the stronger one.
    ///
    /// A pattern with properties in it would raise this at its first read anyway. An empty one —
    /// `var {} = null` — would not, and is a TypeError all the same, which is the case this exists
    /// for.
    RequireCoercible,
    /// §7.1.17 `ToString` of the value on top, replacing it.
    ///
    /// Not `+ ""`, and the difference is observable. Addition takes `ToPrimitive` with the
    /// *default* hint, so an object with a `valueOf` answers through that; `ToString` uses the
    /// **string** hint and reaches `toString` first. A template substitution is specified as
    /// `ToString` (§13.2.8.6), so an object with both methods stringifies differently inside a
    /// template than beside a `+`, and no arrangement of existing instructions says that.
    Stringify,
    /// Push the value of a variable: this many environments out, at this index.
    ///
    /// One instruction rather than one per scope kind. The compiler built the chain of scopes, so
    /// it knows how far out a name is and nothing here searches for it — §9.1's environment
    /// records, resolved at compile time.
    LoadVariable(u32, u32),
    /// Store the top value into a variable, **without** taking it off the stack.
    ///
    /// Assignment is an expression: `a = (b = 1)` works because the inner one leaves its value
    /// behind. A statement that only wants the effect follows this with a [`Instruction::Pop`].
    StoreVariable(u32, u32),
    /// Put a local binding into §9.1.1.1's uninitialised state — the temporal dead zone.
    ///
    /// Emitted where a block is *entered*, once for each `let` or `const` it declares, because
    /// §14.2.3 `BlockDeclarationInstantiation` creates those bindings there and leaves them
    /// uninitialised until their declaration runs. Reading one in between is a ReferenceError.
    ///
    /// An instruction rather than a state the environment starts in, because a slot is not new
    /// each time its block is entered: a loop body's bindings occupy the same slots on every pass,
    /// and the second pass must find the dead zone rather than the first pass's value.
    Uninitialise(u32),
    /// Give a local binding its first value, **without** taking it off the stack.
    ///
    /// `InitializeBinding` (§9.1.1.1.4), which is a different operation from an assignment and
    /// has to be: an assignment to an uninitialised binding is a ReferenceError, and this is what
    /// stops being one. Only a declaration emits it, and only for its own binding, which is why
    /// there is no depth — a declaration initialises the binding it is written in.
    Initialise(u32),
    /// Throw a TypeError, because a `const` was assigned to — §9.1.1.1.5 step 3.
    ///
    /// The compiler resolved the binding and so already knows the assignment cannot stand. What is
    /// left for run time is only the throw, and it happens *here* rather than at compile time
    /// because §13.15.2 evaluates the right-hand side first: `const c = 1; c = f()` calls `f`.
    ThrowImmutableAssignment,
    /// Replace a value with the list of names a `for`-`in` over it visits — §14.7.5.10.
    ///
    /// The list is an ordinary Array of Strings, so the collector already knows how to keep it and
    /// nothing new has to be a `Value`. A value that is `undefined` or `null` enumerates *nothing*
    /// rather than failing: §14.7.5.6 step 2 leaves the loop before its body ever runs, and an
    /// empty list is that, exactly.
    EnumerateProperties,
    /// Take the next name from an enumeration, or `undefined` when there are none left.
    ///
    /// The object is popped from the stack; the two operands are the slots holding the list and
    /// the position in it, which the compiler owns and no source can name.
    ///
    /// `undefined` as the end marker is safe rather than convenient: §14.7.5.10 enumerates String
    /// keys and nothing else, so no name it could yield is `undefined`.
    ///
    /// Skipping is part of the step. §14.7.5.10 requires that a property deleted before it is
    /// reached is not visited, so each name is asked about again — on the object, not on the list
    /// — before it is handed back. A name added during the enumeration is not visited, which the
    /// same clause explicitly allows.
    EnumerateNext(u32, u32),
    /// Push the value of a global, named by the String constant at this index.
    ///
    /// The other half of §9.4.2's `ResolveBinding`. A name the compiler could not place in any
    /// scope is not an error — it is a question for the global object, and one whose answer can
    /// change between one line and the next. A name that is not there when it is *read* is a
    /// **ReferenceError** (§6.2.5.5 `GetValue` on an unresolvable reference), which is the one
    /// thing that separates `missing` from `o.missing`.
    LoadGlobal(u32),
    /// Store the top value into a global, **without** taking it off the stack.
    ///
    /// §6.2.5.6 `PutValue`, in the sloppy-mode half: assigning to a name that is nowhere creates
    /// it on the global object. Strict code throws a ReferenceError instead, and nothing carries
    /// a strictness this far yet — so this is the answer that is right for a Script's default,
    /// and the conformance failures name the tests that want the other one.
    StoreGlobal(u32),
    /// Push the `typeof` of a global, or `"undefined"` when there is no such global.
    ///
    /// §13.5.1.1 step 2: `typeof` is the one operator that takes an unresolvable reference and
    /// answers rather than throwing. It is how a program asks whether a feature exists at all —
    /// `typeof JSON !== "undefined"` is in test262's own harness — so a plain
    /// [`Instruction::LoadGlobal`] would turn the question into the error it was asked to avoid.
    TypeofGlobal(u32),
    /// Give the global object this name as a `var` binding, if it has not got one.
    ///
    /// §9.1.1.4.17 `CreateGlobalVarBinding`: writable and enumerable like an ordinary property,
    /// and **not** configurable — a script's `var` cannot be deleted, which is exactly what makes
    /// `var x` different from `globalThis.x = 1`. An existing property is left alone: `var x`
    /// after `x` already holds something does not put `undefined` back, and that is hoisting.
    DeclareGlobal(u32),
    /// Push a new empty ordinary object, inheriting from `Object.prototype`.
    NewObject,
    /// Push a new empty Array of this length, inheriting from `Array.prototype`.
    ///
    /// The length is the literal's *element count*, holes included — §13.2.4.1 gives
    /// `[, , 1]` a length of 3 with one element in it. So the length is set once here rather
    /// than being raised element by element, which is also what makes a trailing hole count.
    NewArray(u32),
    /// Push a copy of the top two values, in the same order.
    ///
    /// A compound assignment to a property reads it and then writes it, and both need the base
    /// and the key. Evaluating them twice would call `f` twice in `o[f()] += 1`, so they are
    /// evaluated once and copied — which is what makes the once-only guarantee an instruction
    /// rather than a promise.
    DuplicateTop(u32),
    /// Take the top value and put it back this many places further down.
    ///
    /// `Bury(3)` turns `[a, b, c, v]` into `[v, a, b, c]`. There is one thing that needs it:
    /// `o.p++` has to keep the **old** value somewhere the store cannot reach, and the base, the
    /// key and the copy it is about to add one to are all sitting above it.
    ///
    /// Recovering the old value from the new one afterwards would need no instruction at all and
    /// would be wrong: `x = 2 ** 53; x++` leaves `x` unchanged, so `new - 1` is not the old
    /// value — and nothing is, once `NaN` is in play.
    Bury(u32),
    /// Take a value and a key and file the value under it, leaving the object where it is.
    ///
    /// `CreateDataPropertyOrThrow` (§7.3.5): an object literal *defines* its properties rather
    /// than assigning them, which is why `{__proto__: 1}` in a literal is special and
    /// `o.__proto__ = 1` is not, and why a literal can shadow a non-writable inherited property.
    DefineField,
    /// Take a function and a key and give the object under them a getter, keeping any setter.
    ///
    /// §15.4.5's `DefineMethod` half of an accessor. Not a [`Instruction::DefineField`] with a
    /// descriptor: a getter and a setter are two halves of *one* property, so defining one must
    /// leave the other where it is — which is why `{get a() {}, set a(v) {}}` is one property
    /// with both and not the second overwriting the first.
    DefineGetter,
    /// The same for a setter.
    DefineSetter,
    /// §15.7.14 `ClassDefinitionEvaluation` — the constructor and its prototype, made as a pair.
    ///
    /// One instruction rather than a sequence because neither half is observable on its own: the
    /// prototype cannot be reached until the constructor holds it, and the constructor is not usable
    /// until the prototype points back. There is no intermediate state worth naming in bytecode.
    ///
    /// The operand indexes the constructor's body. Leaves the constructor on the stack.
    /// Call with the arguments in the array on top of the stack — §13.3.8's spread.
    ///
    /// A separate instruction because [`Instruction::Call`] fixes its argument count at compile time
    /// and a spread does not have one until it has been iterated. The array is built first, by the
    /// same code an array literal uses, and this expands it: `f(...a, 1)` and `f.apply` differ only
    /// in who writes the array.
    /// §13.2.5 `...o` in an object literal — every own enumerable property of the value on top of
    /// the stack, added to the object beneath it.
    ///
    /// Pops the source and leaves the target, because a literal is built with one object on the stack
    /// throughout, the same way [`Instruction::DefineField`] does.
    SpreadProperties,
    /// Call with the arguments in the array on top of the stack — §13.3.8's spread.
    ///
    /// A separate instruction because [`Instruction::Call`] fixes its argument count at compile time
    /// and a spread does not have one until it has been iterated. The array is built first, by the
    /// same code an array literal uses, and this expands it.
    CallSpread(SpreadCall),
    /// §15.7.14 `ClassDefinitionEvaluation` — the constructor and its prototype, made as a pair.
    ///
    /// One instruction rather than a sequence because neither half is observable on its own: the
    /// prototype cannot be reached until the constructor holds it, and the constructor is not usable
    /// until the prototype points back.
    ///
    /// The operand indexes the constructor's body. Leaves the constructor on the stack.
    MakeClass {
        /// Which of this chunk's nested bodies is the constructor.
        body: u32,
        /// Whether the value of an `extends` clause is on the stack, waiting to be inherited from.
        ///
        /// §15.7.14 steps 8 to 11 read it three ways — `null` is a case of its own, a
        /// non-constructor is a TypeError, and otherwise both the constructor and its prototype are
        /// pointed at the parent's. A flag rather than a second instruction because everything after
        /// those steps is identical, and written twice one copy would be a branch no test reaches.
        derived: bool,
    },
    /// Replace the constructor on top of the stack with the prototype it was made with.
    ///
    /// Reads the object rather than the `prototype` property. At this point in a class definition the
    /// property is already non-writable and no getter could have been installed, so the two agree —
    /// and reading the object means a method definition cannot be intercepted.
    ClassPrototype,
    /// §15.7.14's method definition — a data property or one half of an accessor, **not** enumerable.
    ///
    /// The single runtime difference between a class body and an object literal: §15.4.5 makes a
    /// literal's methods enumerable and §15.7.14 does not, so `for (k in new C)` finds nothing. That
    /// one attribute is why this cannot be [`Instruction::DefineField`] with a flag.
    ///
    /// Pops the value, then the key, then the target.
    DefineClassMethod(crate::ast::MethodKind),
    /// `delete x` where `x` is a property of the global object — §13.5.1.2 step 5.
    ///
    /// The operand names it, exactly as [`Instruction::LoadGlobal`] does. A name the global object
    /// does not have answers **true**: §6.2.5.6 makes an unresolvable reference a delete of nothing,
    /// which succeeded vacuously. One it has answers whether the property was configurable.
    ///
    /// Only a *global* reaches this. A name the compiler could place is a declarative binding, and
    /// §9.1.1.1.5 makes every one of those non-deletable — so that answer is a constant `false` and
    /// needs no instruction at all.
    DeleteGlobal(u32),
    /// Take a key and a base and push the property's value — `[[Get]]`, §10.1.8.
    GetProperty,
    /// Take a value, a key and a base; store the value and leave it on the stack — §10.1.9.
    SetProperty,
    /// Take a constructor and a value and push whether the value is an instance of it.
    ///
    /// §13.10.2's `InstanceofOperator`. An instruction rather than a row in `apply_binary` for
    /// the same reason `in` is one: it asks an object a question instead of converting a value,
    /// and answering needs the prototype chain.
    Instanceof,
    /// Take a key and a base and push whether the property was there to remove — §13.5.1.
    DeleteProperty,
    /// Take a base and a key and push whether the base has it — §13.10.1's `in`.
    HasProperty,
    /// Take the top value and throw it — §14.14.
    ///
    /// Any value, not only an Error: `throw 1` is legal, and the specification never asks what
    /// was thrown. Where it lands is [`Instruction::PushHandler`]'s business.
    Throw,
    /// Remember that a throw from here until the matching pop should continue at this instruction.
    ///
    /// The operand stack's depth is remembered with it. A throw in the middle of an expression
    /// leaves whatever it had pushed behind, and unwinding has to put the stack back where the
    /// handler expects it — otherwise a caught exception would leave rubbish under every
    /// subsequent value.
    PushHandler(u32),
    /// Forget the innermost handler, because its protected region finished normally.
    PopHandler,
    /// Push a new function object over the nested body at this index.
    ///
    /// `InstantiateOrdinaryFunctionExpression` (§15.2.5) in the part that matters here: the
    /// object is made when the expression is *evaluated*, so two visits to the same `function`
    /// keyword make two objects, and `f !== f` across calls is the whole reason closures work.
    MakeFunction(u32),
    /// Take a callee and this many arguments and call it, leaving what it returned — §13.3.6.
    ///
    /// The callee gets no receiver, so its `this` is §10.2.1.2's substitution: the global object.
    Call(u32),
    /// Take a receiver, a callee and this many arguments, and call the callee *on* the receiver.
    ///
    /// A method call is not a plain call of a property's value. `o.m()` and `var f = o.m; f()`
    /// call the same function with different `this`, which is the whole reason the receiver
    /// travels with the call rather than with the function.
    CallMethod(u32),
    /// Take a constructor and this many arguments and construct — §13.3.5.
    ///
    /// Not a call with a different receiver: the receiver is *made* here, out of the
    /// constructor's own `prototype` property, and the result is that object unless the body
    /// returned one of its own.
    Construct(u32),
    /// Push the running function's `this`.
    LoadThis,
    /// `super(…)` — construct the parent and leave the object it made on the stack (§13.3.7.1).
    ///
    /// Not [`Instruction::Construct`] with a different callee, and the differences are the whole of
    /// what a derived construction is. The callee is not named in the source: it is the running
    /// function's `[[Prototype]]`, read now rather than captured at the class definition, because
    /// `Object.setPrototypeOf(D, Other)` changes what `super()` reaches. And `new.target` is
    /// *inherited* rather than becoming the parent, which is what makes `new E()` produce an `E`
    /// however many `extends` clauses it passes through.
    ///
    /// The operand is the argument count; the arguments are on the stack above nothing else.
    SuperCall(u32),
    /// §9.1.1.3's `MakeMethod` — tell the function on top of the stack which object it was defined on.
    ///
    /// Nothing is popped and nothing is observable: `[[HomeObject]]` is not a property and no script
    /// can read it. Only `super` consults it, which is why an ordinary function expression never gets
    /// one — `{ m: function () { return super.x; } }` is a Syntax Error, and the parser says so.
    ///
    /// The operand is how far below the top the home object sits, because the definition that follows
    /// needs its own operands in place: a method definition has `[home, key, function]` on the stack
    /// and a synthesised initialiser has `[home, function]`.
    MakeMethod(u32),
    /// §9.1.1.3's `GetSuperBase` — push the running method's home object's `[[Prototype]]`.
    ///
    /// One level *above* where the method was defined, which is the whole point: a method that read
    /// its own home would find itself and recurse. `undefined` when the running function has no home,
    /// which no `super` the parser accepts can reach.
    LoadSuperBase,
    /// `super.x` — read a property of the super base with `this` as the receiver (§13.3.7.1).
    ///
    /// Two objects, which is what makes this its own instruction. The property is *found* on the
    /// super base and an accessor is called with **`this`**, so a getter inherited from a parent sees
    /// the instance rather than the prototype it was found on. [`Instruction::GetProperty`] uses one
    /// value for both and cannot express it.
    ///
    /// Pops the key, the base and the receiver, in that order down the stack.
    GetSuperProperty,
    /// `super.x = v` — write through the super base with `this` as the receiver (§13.3.7.1).
    ///
    /// The receiver decides where the value *lands*: an inherited setter is called with `this`, and a
    /// write with no setter creates an own property of `this` rather than of the base. So
    /// `super.x = 1` in a method leaves an own `x` on the instance, which is the same rule as an
    /// ordinary assignment through a prototype and reads oddly only because the base is named.
    ///
    /// Pops the value, the key, the base and the receiver, leaving the value.
    SetSuperProperty,
    /// `delete super.x` — §13.5.1.1 step 3's unconditional **ReferenceError**.
    ///
    /// A run-time throw and not an early error, because the reference is evaluated first: `delete
    /// super[k]` runs `ToPropertyKey(k)` and only then refuses, so a `toString` on the key has
    /// already had its effect. The base and the key are on the stack and go with the throw.
    ThrowSuperDelete,
    /// Give the function on top of the stack the running function's `[[HomeObject]]`.
    ///
    /// For the bodies praxis synthesises to stand in for inline code — a derived class's instance
    /// field initialisers, which §15.7.14 runs from `super()` rather than on entry. Written inline
    /// those statements would see the constructor's home; as a body of their own they have none, so
    /// this is what an arrow's capture does, for a function that is not an arrow and needs its own
    /// `this`.
    InheritHome,
    /// Push a **fresh** Private Name — §9.2's `PrivateEnvironment`, one entry at a time.
    ///
    /// Fresh per execution, which is the whole reason it is an instruction rather than a constant: a
    /// class evaluated twice has two sets of private names, so an instance of one evaluation is not a
    /// brand of the other. That is what every `multiple-evaluations-of-class` test in the suite is
    /// about, and a constant in the chunk would make them all pass by accident and be wrong.
    ///
    /// The name is a Symbol, because §6.2.12 asks for exactly what a Symbol has — an identity that is
    /// itself and a description only a debugger reads — and for nothing a Symbol lacks. It goes into a
    /// compiler slot no source can spell and never reaches a property table.
    NewPrivateName(u32),
    /// `{__proto__: v}` — B.3.1, which sets the prototype instead of making a property.
    ///
    /// An **Annex B** rule and the one praxis implements, for the reasons in DR-0008: it is not
    /// conditioned on strictness, and leaving it out is a silent wrong answer rather than a refusal —
    /// the grammar already accepts `__proto__: x`, so there is nothing to reject.
    ///
    /// A value that is neither an Object nor `null` is **ignored**: `({__proto__: 1})` has no
    /// prototype-setting effect *and* no `__proto__` property, which is the one shape that surprises
    /// people. Pops the value and peeks the target, as a definition does.
    SetLiteralPrototype,
    /// §7.3.29 `PrivateFieldAdd` — add a private field to an object.
    ///
    /// A **TypeError** if the object already carries the name, which is step 3 rather than a
    /// defensive check: a constructor re-entered on an object it already initialised reaches it.
    ///
    /// Pops the value and the name, and *peeks* the target — an instance is given one field after
    /// another, exactly as [`Instruction::DefineField`] does it.
    DefinePrivateField,
    /// §7.3.30 `PrivateMethodOrAccessorAdd` — give an object a private method or accessor.
    ///
    /// One function object is shared by every instance and each instance carries an *entry* for it,
    /// which is what makes `#m in o` a brand rather than a lookup up a prototype chain. So the method
    /// is made once at the class definition and this is what runs per construction.
    ///
    /// A **TypeError** if the object already carries the name, on the same terms as §7.3.29.
    ///
    /// Pops the value — one function, or a getter and a setter for
    /// [`Instruction::AddPrivateAccessor`] — then the name, and *peeks* the target.
    AddPrivateMethod,
    /// The same for an accessor, whose two halves are **one** element (§7.3.30).
    ///
    /// Either half may be `undefined`, and then that direction is a TypeError rather than silently
    /// doing nothing — which is where a private accessor differs from a public one. Two halves written
    /// separately still make one element, so the second must merge into the first rather than replace
    /// it, and that is why this is not two adds.
    ///
    /// Pops the setter, the getter and the name, and peeks the target.
    AddPrivateAccessor,
    /// §7.3.31 `PrivateGet` — read a private field, or throw.
    ///
    /// A **TypeError** when the object does not carry the name, and that is what makes a private name
    /// a *brand*: there is no way to ask without risking the throw except [`Instruction::HasPrivate`],
    /// which exists for exactly that reason.
    ///
    /// Pops the name and the target.
    GetPrivate,
    /// §7.3.32 `PrivateSet` — write a private field that is already there, or throw.
    ///
    /// It does not create one. `this.#x = 1` on an object with no `#x` is a TypeError, which is what
    /// fixes an object's set of private names at construction.
    ///
    /// Pops the value, the name and the target, and leaves the value: an assignment is an expression.
    SetPrivate,
    /// `#x in o` — §13.10.1, the one way to ask without risking the throw.
    ///
    /// Pops the name and the target and pushes a Boolean. It exists because §7.3.31 throws: without
    /// it, asking whether an object is one of yours would mean catching a TypeError.
    HasPrivate,
    /// §10.2.2's `BindThisValue` — bind the derived constructor's `this` to the top value.
    ///
    /// Peeked rather than popped, because §13.3.7.1 step 8 makes the object the value of the
    /// `super()` expression as well. Binding a second time is a **ReferenceError**, which is what
    /// makes two `super()` calls in one constructor an error rather than two constructions — so this
    /// cannot be [`Instruction::Initialise`], which writes whatever it finds.
    BindThis(u32),
    /// Read a derived constructor's `this` — §9.1.1.3's `ResolveThisBinding` (DR-0015).
    ///
    /// Its own instruction rather than [`Instruction::LoadVariable`] for the message alone: the
    /// binding is in the same uninitialised state a `let` above its declaration is in, and reporting
    /// it as one would send a reader looking for a declaration there is not.
    LoadThisBinding {
        /// How many environments out — non-zero for an arrow written inside the constructor.
        depth: u32,
        /// Which slot, in that environment.
        index: u32,
    },
    /// Turn the top value into what a derived constructor returns — §10.2.2 step 13.
    ///
    /// Stricter than a base constructor's, which is why it cannot be left to
    /// [`Instruction::Return`]: an object is answered with, `undefined` is answered with the bound
    /// `this` — a ReferenceError if `super()` never ran — and **any other primitive is a
    /// TypeError**, where a base constructor ignores it and answers with the object it made.
    ///
    /// The operand is the `this` binding's slot, always in the running environment: a `return` is
    /// compiled inside the constructor whose binding it is, however many blocks deep.
    CompleteDerivedReturn(u32),
    /// Push §13.2.8.3's template object for the tagged template at this index.
    ///
    /// A frozen Array of the cooked strings with a frozen `raw` Array of the raw ones. **Cached per
    /// site**: the same tagged template evaluated twice hands the tag the *same* object, which is what
    /// lets a tag use it as a key and is the one thing about it a program can detect. So the identity
    /// is per site and per realm rather than per evaluation, and building a fresh one each time would
    /// pass every test about its contents and fail every test about its identity.
    TemplateObject(u32),
    /// Push the running function's `new.target` — §13.3.12.
    ///
    /// `undefined` unless the running call was a `new`, which is the whole of what the expression
    /// is for: a function cannot otherwise tell `f()` from `new f()`. Its own instruction rather
    /// than a property of anything, because it is a property of the *call* — the same function
    /// object answers differently on two successive calls, so there is nowhere else to read it
    /// from.
    LoadNewTarget,
    /// Push a copy of the top value.
    ///
    /// A method call needs the base twice — once to find the method on and once to call it with —
    /// and evaluating it twice would run its side effects twice.
    Duplicate,
    /// Leave the current function, taking the top value with it — §14.10.
    Return,
    /// Take the top value and make it the script's completion value.
    ///
    /// §14.2.2 — a Script evaluates to the value of its last *value-producing* statement, which
    /// is what makes `eval("1; 2")` be 2 and `eval("var x = 1")` be `undefined`. A register
    /// rather than the stack, because a statement in the middle of a block has to be able to
    /// replace it without anything below being disturbed.
    SetCompletion,
}

/// When a [`Instruction::JumpKeeping`] jumps — one per short-circuiting operator (§13.13, §13.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortCircuit {
    /// `&&` — stop at the first falsy operand, which becomes the answer.
    WhenFalsy,
    /// `||` — stop at the first truthy one.
    WhenTruthy,
    /// `??` — stop at the first that is neither `null` nor `undefined`.
    ///
    /// A different test from `||`, and the whole reason `??` exists: `0 || 1` is `1` and
    /// `0 ?? 1` is `0`, because `0` is falsy and is not nullish.
    WhenNotNullish,
}

/// A jump that has been emitted and does not know where it goes yet.
///
/// Exists to make forgetting one impossible rather than unlikely. It is `#[must_use]`, it is not
/// `Copy`, and [`Chunk::patch`] takes it by value — so the only way to obtain one is to emit a
/// jump and the only way to be rid of one is to patch it. A dangling jump becomes a build failure
/// under the gate's denied warnings, which is a stronger claim than any test could make.
#[must_use = "a jump that is never patched jumps nowhere"]
pub(super) struct Unpatched(usize);

impl Chunk {
    /// The instructions, in order.
    pub fn code(&self) -> &[Instruction] {
        &self.code
    }

    /// The constant at `index`, or `None` if there is none.
    ///
    /// Fallible because a `Chunk` can be built by hand, and a hand-built one may point anywhere.
    /// The compiler never produces such a chunk; the VM still has to answer for one, which is
    /// DR-0002 applied to the engine's own output rather than to a script's input.
    pub fn constant(&self, index: u32) -> Option<Value> {
        self.constants.get(index as usize).copied()
    }

    /// A chunk built by hand, out of instructions and constants that need not agree.
    ///
    /// The compiler does not use this — it emits as it goes — and nothing in the engine does. It
    /// is here so that a chunk the compiler would never produce can be *written*, which is the
    /// only way to reach [`crate::vm::Fault`] and therefore the only way to test that a malformed
    /// chunk is answered rather than crashed on.
    /// Everything but the code and the constants is left at its default, and *deliberately* not
    /// restated: a hand-built chunk is never a callee, so a field like `arrow` could be set to
    /// anything here without any test being able to tell. Written twice, the second copy would be
    /// a value no test can reach — so it is written once, where the compiler also gets it.
    pub fn from_parts(code: Vec<Instruction>, constants: Vec<Value>) -> Self {
        Self {
            code,
            constants,
            ..Self::default()
        }
    }

    /// How many named parameters the function this code belongs to declares.
    ///
    /// How many slots the parameters occupy — every named one, the rest parameter aside.
    ///
    /// What a call fills from its arguments. Not `length`: see [`Chunk::length`].
    pub fn parameters(&self) -> usize {
        self.parameters
    }

    /// §10.2.3's `length` — the count before the first default or rest parameter.
    ///
    /// `function f(a, b = 1, c)` has a length of 1, and a reader who expects 3 is reading the
    /// number of *slots*. What this reports is how many arguments the function says it needs.
    pub fn length(&self) -> usize {
        self.length
    }

    /// §10.2.9's `name`, or `None` where the specification asks for the empty string.
    pub fn name(&self) -> Option<crate::heap::StringId> {
        self.name
    }

    /// Whether this body is strict code — §11.2.1.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Whether the parameter list is simple — §15.1.4, and which arguments object to build.
    pub fn simple_parameters(&self) -> bool {
        self.simple_parameters
    }

    /// The slot a rest parameter's array goes in, if there is one.
    pub fn rest(&self) -> Option<u32> {
        self.rest
    }

    /// The template at this index, if there is one.
    pub fn template(&self, index: u32) -> Option<&Template> {
        self.templates.get(index as usize)
    }

    /// The body of the nested function at this index, if there is one.
    pub fn function(&self, index: u32) -> Option<&Rc<Chunk>> {
        self.functions.get(index as usize)
    }

    /// How many local-variable slots the code addresses.
    ///
    /// The VM gives a frame this many slots, each starting as `undefined` — which is what makes a
    /// `var` readable before its declaration and holding nothing.
    pub fn locals(&self) -> usize {
        self.locals
    }

    /// Where a derived constructor's `this` lives, if this body is one — DR-0015.
    ///
    /// `None` means the call binds `this` and makes the receiver, which is every other body. The
    /// call reads this to decide *not* to make one: §10.2.2 leaves that to `super()`.
    pub fn derived_this(&self) -> Option<u32> {
        self.derived_this
    }

    /// Whether this body is a `MethodDefinition`, and so has no `[[Construct]]` — §15.4.5.
    pub fn is_method(&self) -> bool {
        self.method
    }

    /// Whether this body is a class constructor, and so refuses a call without `new`.
    pub fn is_class_constructor(&self) -> bool {
        self.class_constructor
    }

    /// Whether this body is an arrow's, and so has no `this` of its own.
    pub fn is_arrow(&self) -> bool {
        self.arrow
    }

    /// The slot a call should put §10.4.4's arguments object in, if this body reads the name.
    pub fn arguments(&self) -> Option<u32> {
        self.arguments
    }

    /// Point a jump at a target that is already known, which a backward jump's is.
    pub(super) fn patch_to(&mut self, jump: Unpatched, target: u32) {
        let Unpatched(at) = jump;
        if let Some(instruction) = self.code.get_mut(at) {
            *instruction = retarget(*instruction, target);
        }
    }

    /// Add an instruction.
    pub(super) fn emit(&mut self, instruction: Instruction) {
        self.code.push(instruction);
    }

    /// Emit a jump whose target is not known yet.
    ///
    /// The target of a forward jump is decided by code that has not been compiled — that is what
    /// makes it forward — so a placeholder goes in and [`Chunk::patch`] replaces it once the
    /// destination exists.
    ///
    /// The placeholder is never seen, and not by luck: [`Unpatched`] is `#[must_use]` and is
    /// consumed by `patch`, so a jump left dangling is a warning and the gate denies warnings.
    /// It is `u32::MAX` anyway, so that if that ever stops being true the answer is a loud
    /// [`crate::vm::Fault::JumpOutOfRange`] rather than a quiet jump to the beginning.
    pub(super) fn emit_jump(&mut self, make: impl FnOnce(u32) -> Instruction) -> Unpatched {
        let at = self.code.len();
        self.emit(make(u32::MAX));
        Unpatched(at)
    }

    /// Point a jump at wherever the next instruction will go.
    pub(super) fn patch(&mut self, jump: Unpatched) -> Result<(), CompileError> {
        let target = u32::try_from(self.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })?;
        self.patch_to(jump, target);
        Ok(())
    }

    /// Add a constant and answer where it went.
    ///
    /// Does not look for an existing equal constant. Deduplicating would need `SameValue`, which
    /// needs the heap, and would save a few words per chunk — an M8 experiment with a measurement,
    /// not a guess.
    pub(super) fn add_constant(&mut self, value: Value) -> Result<u32, CompileError> {
        let index = u32::try_from(self.constants.len());
        self.constants.push(value);
        // A chunk with more than four billion constants is not a program anyone wrote, and the
        // index has to fit somewhere. Refusing is the only answer that is neither a panic nor a
        // wrong constant.
        index.map_err(|_| CompileError {
            kind: ErrorKind::TooManyConstants,
            span: Span::new(0, 0),
        })
    }
}

/// Which of §13.3's three ways in a spread call takes.
///
/// One operand rather than a flag per way: `f(...a)`, `o.m(...a)` and `new f(...a)` differ only in
/// where the receiver comes from, and a second boolean beside the first would leave a fourth
/// combination that means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadCall {
    /// `f(...a)` — no receiver.
    Plain,
    /// `o.m(...a)` — the receiver sits under the callee.
    Method,
    /// `new f(...a)` — the receiver is made by the call.
    Construct,
    /// `super(...a)` — the callee is the running function's `[[Prototype]]`, not on the stack.
    Super,
}

/// The same jump, pointed somewhere else.
///
/// Exhaustive on purpose. Written with a catch-all arm it silently did nothing to a `PushHandler`,
/// whose target then stayed at the unpatched placeholder — so every `try` jumped off the end of
/// its chunk. Listing every instruction means the next one that carries a target cannot be
/// forgotten here: leaving it out is a compile error.
pub(super) fn retarget(instruction: Instruction, target: u32) -> Instruction {
    match instruction {
        Instruction::Jump(_) => Instruction::Jump(target),
        Instruction::JumpIfFalse(_) => Instruction::JumpIfFalse(target),
        Instruction::JumpIfTrue(_) => Instruction::JumpIfTrue(target),
        Instruction::JumpKeeping(condition, _) => Instruction::JumpKeeping(condition, target),
        Instruction::PushHandler(_) => Instruction::PushHandler(target),
        // Not a jump. An `Unpatched` can only ever name one, since `emit_jump` is the only way
        // to make one — so these are unreachable, and are listed rather than swept into a
        // catch-all so that a new jump cannot hide among them.
        Instruction::Constant(_)
        | Instruction::RegExpLiteral
        | Instruction::Unary(_)
        | Instruction::Binary(_)
        | Instruction::Pop
        | Instruction::Stringify
        | Instruction::LoadWellKnown(_)
        | Instruction::RequireObject
        | Instruction::RequireCoercible
        | Instruction::CopyRest(_)
        | Instruction::LoadVariable(_, _)
        | Instruction::StoreVariable(_, _)
        | Instruction::Uninitialise(_)
        | Instruction::Initialise(_)
        | Instruction::ThrowImmutableAssignment
        | Instruction::EnumerateProperties
        | Instruction::EnumerateNext(_, _)
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::TypeofGlobal(_)
        | Instruction::DeclareGlobal(_)
        | Instruction::DeleteGlobal(_)
        | Instruction::SetCompletion
        | Instruction::Throw
        | Instruction::PopHandler
        | Instruction::MakeFunction(_)
        | Instruction::Call(_)
        | Instruction::Construct(_)
        | Instruction::CallMethod(_)
        | Instruction::LoadThis
        | Instruction::LoadNewTarget
        | Instruction::TemplateObject(_)
        | Instruction::SuperCall(_)
        | Instruction::MakeMethod(_)
        | Instruction::NewPrivateName(_)
        | Instruction::SetLiteralPrototype
        | Instruction::DefinePrivateField
        | Instruction::AddPrivateMethod
        | Instruction::AddPrivateAccessor
        | Instruction::GetPrivate
        | Instruction::SetPrivate
        | Instruction::HasPrivate
        | Instruction::ThrowSuperDelete
        | Instruction::InheritHome
        | Instruction::LoadSuperBase
        | Instruction::GetSuperProperty
        | Instruction::SetSuperProperty
        | Instruction::BindThis(_)
        | Instruction::LoadThisBinding { .. }
        | Instruction::CompleteDerivedReturn(_)
        | Instruction::Duplicate
        | Instruction::Return
        | Instruction::NewObject
        | Instruction::NewArray(_)
        | Instruction::DuplicateTop(_)
        | Instruction::Bury(_)
        | Instruction::DefineField
        | Instruction::DefineGetter
        | Instruction::DefineSetter
        | Instruction::SpreadProperties
        | Instruction::CallSpread(_)
        | Instruction::MakeClass { .. }
        | Instruction::ClassPrototype
        | Instruction::DefineClassMethod(_)
        | Instruction::GetProperty
        | Instruction::SetProperty
        | Instruction::DeleteProperty
        | Instruction::HasProperty
        | Instruction::Instanceof => instruction,
    }
}
