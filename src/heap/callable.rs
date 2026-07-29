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
    Native(Native),
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
}

/// What a bound function was bound to — §10.4.1's three internal slots.
#[derive(Debug, Clone)]
pub struct Bound {
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
}

impl NativeCall<'_> {
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
        };
        assert!(matches!(call.argument(0), Value::Number(value) if value == 1.0));
        assert!(matches!(call.argument(1), Value::Null));
        assert!(matches!(call.argument(2), Value::Undefined));
        // …and far past the end is the same answer rather than a different one.
        assert!(matches!(call.argument(9_999), Value::Undefined));
    }
}
