---
id: DR-0030
title: A class field's initialiser is a function of its own, made where the class is defined
status: prose-only
---

§15.7.10 makes every class field initialiser a **function**: `ClassFieldDefinitionEvaluation` builds
one per field, with the class's environment and the class prototype as its home object, and
`DefineField` calls it once per construction with the new object as its receiver. ViperJS did that
for a `static` field and compiled an **instance** field's initialiser *inline into the constructor's
prologue*.

Inline is the same thing only if nothing can tell the two apart. Three things can, and the first two
are wrong **values** rather than missing errors — which is why this was worth doing and why the note
that costed it at "twenty runs" had the wrong measure.

## What inline cost

- **`new.target`.** A field initialiser is entered by a *call*, so §9.4.4 gives it `undefined`.
  Inline it read the constructor's: `class C { x = new.target }` answered with `C`.
- **The constructor's parameters were in scope.** Inline code closes over the constructor's
  environment, so `class C { x = typeof p; constructor(p) {} }` answered `"number"` and
  `class C { x = p; constructor(p) {} }` read the argument. A field initialiser has no business
  seeing a parameter, and nothing in the clause puts one there.
- **`arguments` in a direct `eval` was not refused.** §15.7.1 forbids the word and the parser refuses
  every spelling it can see; a direct `eval` is the one it cannot, so the fact travels on the
  compiled body. Inline there was no body to carry it, and the running frame was the constructor's —
  which may legitimately have `arguments`.

## What is decided

**Each instance field's initialiser is compiled where the class is defined**, into a function stored
in a compiler temporary of the class scope — `%class field init N`, beside the `%class field name N`
that already held the evaluated key. The constructor's prologue loads it and calls it with the new
object as receiver. One function per field, made once and shared by every construction, which is
what `[[Fields]]` holds in the specification.

The home object is the class **prototype**, so `super.x` in an initialiser reads the parent's
prototype and not the parent class — the same distinction §15.7.14 draws for an instance method
against a static one.

## Three things it uncovered, each a bug of its own

**A slot that is not declared falls back to a global.** The first attempt reserved a name slot per
field and no initialiser slot, and `store_name` for an unknown name resolves to a property of the
global object. Sloppy code stores it and works; strict code throws. Measured: **844 conformance
runs**, every regression a `(strict)` one. The comment above the existing reservation loop had
already written this failure down — "a missing slot does not fail loudly, it falls back to a
*global* of the same name and keeps working" — which is a warning that was read only after it fired.

**`Strict::Inherited` is wrong for a class body.** §11.2.2 makes *all* parts of a ClassBody strict
code, and all three initialiser sites inherited strictness from whatever enclosed the class instead.
For a class in a sloppy script that made the initialiser sloppy, so a direct `eval` in one took the
sloppy `var` path into a refusal the engine has not built —
`class C { static x = eval("var v = 1") }` was a SyntaxError before this and is not now. The
constructor said `Strict::Yes` outright, so inline code had been strict by borrowing it.

**The field-initialiser flag was set too late for an arrow.** It was written onto the chunk *after*
`compile_nested` returned, and an arrow written inside the initialiser reads it off the enclosing
chunk while that arrow is being compiled — so it had already been read as `false`.
`x = () => eval('arguments')` was accepted, in a static field as much as in an instance one. It is
set from the shape of the body now, which is knowable before the body is compiled.

## And a naming bug in the opposite direction

Moving the instance side to a wrapper would have **lost** §8.6.3's naming, which the inline path got
by reaching `named_evaluation` on the way past. Putting it back showed that the *static* side never
had it: `class C { static x = () => {} }` left the arrow unnamed, and a computed static key named
nothing either.

So the naming is now where the clause puts it — `Body::FieldInitializer` carries the written name
into the body, which is §15.7.10 step 2.g's `[[ClassFieldInitializerName]]`, and a **computed** key
is named by the define sequence from the key already on the stack. A written name has to go inside
because a private field's is `#q`, and the key on the stack there is a Private Name that renders as
`[q]`.

`Body::FieldInitializer` is a variant rather than an `Option` on `Body::Expression`, because the two
differ in the one thing that matters: `var f = x => y` names the *arrow* `f` and must not name `y`,
where `class C { x = () => {} }` names the arrow and never the wrapper.

## What it is worth

**+40 conformance runs**, three identical runs, no regressions. The number is the smaller half: the
wrong `new.target` and the visible constructor parameter are the kind of fault that makes a program
quietly do something else, and neither had a test in the suite that ViperJS was reaching.
