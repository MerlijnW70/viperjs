---
id: DR-0015
title: A derived constructor's `this` is a binding, not a register
status: prose-only
---

Every function in ViperJS gets its `this` from a register: the call decides it, `LoadThis` pushes it,
and a frame puts the caller's back on the way out. That works because `this` cannot change during a
call — it is decided once, at entry, and read as many times as the body likes.

`class D extends B` breaks that. §10.2.2 does **not** create the receiver for a derived
constructor: `this` starts out *uninitialised*, `super()` creates it by constructing the parent, and
until then §9.1.1.3's `ResolveThisBinding` throws a **ReferenceError**. Returning from a derived
constructor without having called `super()` is the same error. So there is now a call during which
`this` changes, and a state in which it has no value at all — neither of which a `Value` register can
hold.

## Two ways to represent the missing state

**A flag on the frame.** `this_value` stays a register, and a boolean beside it says whether it
means anything yet; `LoadThis` consults the flag and `super()` clears it. Small, local, and it gets
one case wrong that cannot be recovered afterwards:

```js
class D extends B {
  constructor() {
    const f = () => this;   // written before super()
    super();
    return f();             // must be the instance
  }
}
```

An arrow captures its `this` as a **value**, at the moment it is made (DR — see
`Object::lexical`), because that is the walk §9.1.1.3 would make and doing it once is cheaper than
doing it per call. An arrow written above the `super()` would therefore capture the placeholder and
hold it forever, and `f()` would answer `undefined` rather than the instance. That is a silent wrong
answer in code that looks entirely ordinary, which is the failure this project refuses to buy
cheaply.

**A binding.** A derived constructor declares a hidden `%this` slot in its own environment, left in
§9.1.1.1's uninitialised state at entry; `super()` initialises it; `this` inside that constructor
compiles to a read of it. That is what §9.1.1.3 actually describes — `[[ThisValue]]` lives in the
Function Environment Record, and `[[ThisBindingStatus]]` is a property of that record rather than of
the machine.

## The binding wins because three things already exist

Nothing here is new machinery, which is the whole argument:

- **The dead zone is already built.** `let` and `const` start uninitialised and reading one is a
  ReferenceError — §9.1.1.1 exactly, tested, and the error it produces is the error §9.1.1.3 wants.
  A derived constructor's `this` is not *like* a temporal dead zone; it is one.
- **Name resolution already crosses function boundaries.** An arrow nested to any depth resolves
  `%this` through the scope chain to a depth and a slot, at the moment it *runs*. So the case the
  flag gets wrong needs no code at all: the arrow reads the binding rather than a copy of it, which
  is what makes it see the `super()` that ran after it was written.
- **`Initialise` already means what is needed.** It is the instruction a `let` declaration emits.

The cost is that a derived constructor reads `this` from an environment rather than from a register.
Not measured, and deliberately not: the register is not slower than the binding, it is *wrong*, and
DR-0013's kind of number does not settle a question of correctness. M8 may revisit it with a
benchmark, on the same terms as everything else there.

## The price: two representations, and a flag that decides between them

Base constructors, methods and every plain function keep the register — giving them all a `%this`
slot would mean the call filling one per entry for a binding nothing reads. So `this` has two
representations in ViperJS, and the compiler picks. That is a genuine cost and it is bounded by being
one flag with one propagation rule, which mirrors `[[ThisMode]]`:

> A body is compiled with `this` as a binding if it is a derived constructor, **or** if it is an
> arrow written inside a body that was. A non-arrow function clears it, because §10.2.11 gives that
> function a `this` of its own and the enclosing `%this` is none of its business.

Getting that rule wrong in the permissive direction is the dangerous one: a *method* written inside
a derived constructor would resolve `%this` through the chain and answer the enclosing instance
instead of its own receiver. That is why the flag is cleared on a non-arrow boundary rather than the
lookup being trusted to stop somewhere.

## The invariant

Inside a derived constructor, and inside any arrow written within one, `this` is a read of the
`%this` binding and nothing else may push it. `super()` is the only thing that initialises that
binding, and initialising it twice is a ReferenceError — §10.2.2 again, and the reason a second
`super()` is an error rather than a second construction. Any future construct that changes `this`
during a call joins this mechanism rather than adding a third state to the register.
