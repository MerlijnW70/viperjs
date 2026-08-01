//! What an object does when it is called — the two kinds, and what a Rust one is handed.
//!
//! # Why there are two and not one
//!
//! A function written in JavaScript is a chunk of bytecode plus the environment it was written
//! in, and calling it means pushing a frame and going round the interpreter's loop again. A
//! built-in has neither: `Object.prototype.toString` is Rust, it runs to completion, and there is
//! nothing for a frame to come back to. Modelling them as one thing would mean giving every
//! built-in an empty chunk and an environment it never reads — a fiction the call path would then
//! have to see through.
//!
//! So [`Callable`] is an enum, and the difference is decided once, where the call is made.
//!
//! # Why this names `Vm`
//!
//! Because a built-in can call JavaScript. `Array.prototype.map` runs its callback, and `join`
//! reaches `ToString` of an element, which may run a `toString` — so a built-in needs the machine
//! and not merely the heap. It reaches the intrinsics through it as well.
//!
//! Naming it here points the heap at a module that points back, which is the shape `Object`
//! already has with [`crate::compile::Chunk`] and for the same reason: the type is a description
//! of what a function *is*, and the heap is where a function lives. Nothing here calls anything;
//! the interpreter does that.

use super::{Heap, ObjectId};
use crate::compile::Chunk;
use crate::value::{Completion, Value};
use crate::vm::Vm;
use std::rc::Rc;

/// What an object runs when it is called.
#[derive(Debug, Clone)]
pub enum Callable {
    /// A function written in JavaScript — §10.2's ordinary function objects.
    ///
    /// The environment it closed over is the object's own, because a closure is that field and
    /// nothing else. See [`Heap::new_function`].
    Bytecode(Rc<Chunk>),
    /// A function written in Rust — §10.3's built-in function objects.
    Native {
        /// The Rust function this runs.
        native: Native,
        /// Whether §10.3.2 gives it a `[[Construct]]` — see [`Callable::constructs`].
        constructs: bool,
    },
    /// What `bind` made — §10.4.1's bound function exotic objects.
    ///
    /// Not a function of its own: it holds another one and calls it with a `this` and a list of
    /// arguments decided when `bind` ran rather than when the call is made. §10.4.1.1's
    /// `[[Call]]` prepends the arguments and *replaces* the receiver; §10.4.1.2's `[[Construct]]`
    /// prepends the same arguments and does **not** replace anything, because `new` makes its own
    /// receiver and a bound `this` has nothing to say about it.
    ///
    /// A third variant rather than a Rust closure, because a [`Native`] is a plain `fn` pointer
    /// and has nowhere to keep the three things this needs.
    Bound(Bound),
    /// One of §27.5.1's three resumption methods — `next`, `return` and `throw`.
    ///
    /// **Not** a [`Native`], and that is the whole reason this variant exists. Resuming a generator
    /// means running its body, and a native runs inside DR-0011's nested execution — a Rust call
    /// waiting mid-instruction, which DR-0017 says a suspension may not be handed back to. So the
    /// body cannot be entered from Rust at all; it has to be entered by the interpreter's own loop,
    /// which is what [`crate::vm::Vm`]'s `enter` does when it meets one of these.
    ///
    /// A variant beside [`Callable::Bound`] rather than a flag on a native, for the same reason
    /// that one is: what is actually entered is not this function.
    Resume {
        /// Which of the three this is.
        kind: Resumption,
        /// Whether it is §27.6.1's method rather than §27.5.1's — the async generator's.
        ///
        /// The two sets read identically and neither accepts the other's receiver, so the
        /// difference has to travel on the function object: `Object.getOwnPropertyDescriptor`
        /// finds nothing that tells them apart, and by the time one is called the prototype it
        /// came off is no longer anywhere in the call.
        asynchronous: bool,
    },
    /// What a settled promise calls to put an `async` function's body back — §27.7.5.3.
    ///
    /// Two of these are made per `await`, one for each way the promise can settle, and each holds
    /// the execution it revives. A [`Native`] could not: its body is a bare `fn` pointer with
    /// nowhere to keep which execution this one is about, which is the same reason
    /// [`Callable::Bound`] is a variant rather than a closure.
    Revive {
        /// Which way the promise settled, and so whether the body carries on or throws.
        kind: crate::heap::ReactionKind,
        /// The object holding the parked execution — §27.7.5.1's context.
        context: ObjectId,
    },
}

/// Which of §27.5.1's three ways a generator is resumed.
///
/// The three differ in the *completion* the body is resumed with, and in nothing else: `next`
/// resumes with a normal one, `return` with a return, and `throw` with a throw. §27.5.3.2's
/// `GeneratorResume` and §27.5.3.4's `GeneratorResumeAbrupt` are that difference and no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resumption {
    /// §27.5.1.2 `next` — carry on, with the argument as the value of the `yield`.
    Next,
    /// §27.5.1.3 `return` — carry on as though a `return` had been written at the `yield`.
    Return,
    /// §27.5.1.4 `throw` — carry on as though a `throw` had been written there.
    Throw,
}

impl Callable {
    /// Whether `new` may be written in front of this — its `[[Construct]]`.
    ///
    /// Being callable and being constructible are two properties and not one, and *which* two
    /// depends on the kind. A function written in the language is a constructor unless it is an
    /// arrow (§15.3), which the body already knows. A built-in has a `[[Construct]]` only where
    /// §10.3.2 gives it one, which nothing about the Rust function could say. And a bound function
    /// borrows its target's answer (§10.4.1.3 step 2), which was settled when it was made.
    ///
    /// Asked of the callable rather than of the object because a thing that cannot be called
    /// cannot be constructed either, and there is no third answer for an object that is neither.
    pub fn constructs(&self) -> bool {
        match self {
            // §15.3 for an arrow and §15.4.5 for a method, and they are two facts rather than one:
            // an arrow has no `this` either, where a method has one and simply does not construct.
            // …and §15.5.3 for a generator, which is a third reason and not the same as either:
            // a generator function has a `this` and is written like an ordinary declaration, and
            // `new g()` is still a TypeError because what it would construct is an object nothing
            // ever inherits from — a generator's instances come from calling it, not from `new`.
            // …and §15.8.3 for an `async` function, which is a fourth: what `new` would construct
            // is a promise, and a promise is not an instance of anything the call could name.
            Self::Bytecode(body) => {
                !body.is_arrow() && !body.is_method() && !body.is_generator() && !body.is_async()
            }
            Self::Native { constructs, .. } => *constructs,
            Self::Bound(bound) => bound.constructs,
            // §27.5.1 gives none of the three resumptions a `[[Construct]]`, as it gives none to
            // any method: `new gen.next()` is a TypeError. §27.7.5.3's two are the same answer for
            // the same reason and are written with them rather than beside them — one of the pair
            // is reachable from a script and the other is not, so a separate arm for the second
            // would be a claim nothing could ever check.
            Self::Resume { .. } | Self::Revive { .. } => false,
        }
    }
}

/// What a bound function was bound to — §10.4.1's three internal slots.
#[derive(Debug, Clone)]
pub struct Bound {
    /// Whether the target had a `[[Construct]]` — §10.4.1.3 step 2.
    ///
    /// Copied at binding time rather than looked up at call time, because §10.4.1.3 settles it
    /// then: what the bound function is depends on what its target *was*.
    pub constructs: bool,
    /// `[[BoundTargetFunction]]` — what is actually called.
    pub target: ObjectId,
    /// `[[BoundThis]]` — the receiver a call uses, and that `new` ignores.
    pub this_value: Value,
    /// `[[BoundArguments]]` — put in front of whatever the call supplies.
    pub arguments: Vec<Value>,
}

/// A built-in's body.
///
/// # What it may throw
///
/// [`Completion`] carries a [`crate::value::Abrupt`], which names an error kind and a message
/// written in the source — or carries a value that has already been thrown. A built-in that needs
/// a message built at *run time* still cannot have one; that is the next thing this type will
/// grow, and it will grow it when a built-in needs it rather than a fortnight before.
pub type Native = fn(&mut Vm, &mut Heap, &NativeCall<'_>) -> Completion<Value>;

/// What a built-in is told about the call it is answering.
#[derive(Debug)]
pub struct NativeCall<'a> {
    /// The function object being called, so a built-in can read its own properties.
    ///
    /// §20.5.1.1 needs exactly this: `Error("x")` and `new Error("x")` both make an object
    /// inheriting from the constructor's `prototype`, and a built-in that did not know which
    /// object it *is* would have to be told by a closure it cannot have — a `fn` pointer holds
    /// no captured state, which is deliberate and is what keeps a function object `Copy`-cheap.
    pub function: ObjectId,
    /// `this`, exactly as the call passed it.
    ///
    /// §10.3.1 performs **no** substitution: a built-in called with no receiver sees `undefined`
    /// where a sloppy-mode JavaScript function would see the global object. That difference is
    /// observable — `Error.prototype.toString.call(undefined)` throws — so the two call paths
    /// deliberately disagree here rather than sharing one rule.
    pub this_value: Value,
    /// The arguments, in order, and only the ones that were written.
    ///
    /// §10.4.4's `arguments` object is a different thing; this is the list itself. A built-in
    /// reads past the end with [`NativeCall::argument`], because §10.3's built-ins are all
    /// specified in terms of "if `x` is absent" meaning `undefined`.
    pub arguments: &'a [Value],
    /// §9.4's `[[NewTarget]]` — the constructor a `new` named, or `undefined` for a plain call.
    ///
    /// Two questions at once, and they used to be two fields' worth of answer. *Whether* this is a
    /// construction is what the wrapper constructors need — `Number(1)` is a Number and
    /// `new Number(1)` is an object — and [`NativeCall::constructing`] asks it.
    ///
    /// *Which* constructor is what §10.3.2 needs, and it was a flag until `super()` existed: a
    /// built-in reached through `class D extends Error {}` must make an object inheriting from
    /// `D.prototype`, and the only thing that knows about `D` is this. With a flag here the answer
    /// was `Error.prototype` and `new D() instanceof D` was false — right for every construction a
    /// program could write before, and wrong for every one it can write now.
    pub new_target: Value,
}

impl NativeCall<'_> {
    /// Whether this is `new f(…)` rather than `f(…)` — §10.3.2's `[[Construct]]`.
    ///
    /// Derived from the target rather than stored beside it, so the two cannot disagree about
    /// whether a construction is happening.
    pub fn constructing(&self) -> bool {
        !matches!(self.new_target, Value::Undefined)
    }

    /// The argument at this position, or `undefined` when there was none.
    ///
    /// Every built-in in §20 through §28 is written as though the list were infinite and padded
    /// with `undefined`, so this is what "let `message` be the first argument" actually means.
    pub fn argument(&self, at: usize) -> Value {
        self.arguments.get(at).copied().unwrap_or(Value::Undefined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_that_was_not_written_reads_as_undefined() {
        // §20 through §28 are written as though the list were infinite: "let `message` be the
        // first argument" means `undefined` when there is no first argument, and a built-in that
        // had to check the length itself would get it wrong somewhere.
        let mut heap = Heap::new();
        let arguments = [Value::Number(1.0), Value::Null];
        let call = NativeCall {
            function: heap.new_object(None),
            this_value: Value::Undefined,
            arguments: &arguments,
            new_target: Value::Undefined,
        };
        assert!(matches!(call.argument(0), Value::Number(value) if value == 1.0));
        assert!(matches!(call.argument(1), Value::Null));
        assert!(matches!(call.argument(2), Value::Undefined));
        // …and far past the end is the same answer rather than a different one.
        assert!(matches!(call.argument(9_999), Value::Undefined));
    }

    #[test]
    fn a_call_is_a_construction_exactly_when_it_has_a_new_target() {
        // §10.3.2 — the two questions the field answers, and the reason it is one field. Derived
        // rather than stored beside the target, so a call cannot claim to be constructing while
        // naming nobody, or name a target while claiming not to be.
        let mut heap = Heap::new();
        let target = heap.new_object(None);
        let plain = NativeCall {
            function: target,
            this_value: Value::Undefined,
            arguments: &[],
            new_target: Value::Undefined,
        };
        assert!(!plain.constructing());
        let constructing = NativeCall {
            new_target: Value::Object(target),
            ..plain
        };
        assert!(constructing.constructing());
    }
}
