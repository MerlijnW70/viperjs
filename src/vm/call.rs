//! Entering a function — §10.2.1's `[[Call]]`, as the interpreter performs it.
//!
//! Three things are decided here and none of them belongs to the function object: the environment
//! the call runs in, the `this` it sees, and the frame that says how to get back. A function
//! object holds only the two halves that *are* its own — the code, and the environment it was
//! written in.

use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{Callable, EnvironmentId, Heap, Object};
use crate::realm::NativeError;
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    /// Call whatever is on the stack, leaving the interpreter running inside it.
    ///
    /// The callee sits under its arguments, because it was pushed first — and a method call has
    /// its receiver under that again. Nothing recurses: the frame is pushed, the code is swapped,
    /// and [`Vm::run`]'s loop goes round again in the callee.
    pub(super) fn enter(
        &mut self,
        how: Entry,
        count: u32,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let count = count as usize;
        let method = how == Entry::Method;
        // The callee sits under its arguments, because it was pushed first — and a
        // method call has its receiver under *that*.
        let Some(callee_at) = self.stack.len().checked_sub(count + 1) else {
            return Err(Fault::StackUnderflow);
        };
        let receiver_at = if method {
            match callee_at.checked_sub(1) {
                Some(at) => at,
                None => return Err(Fault::StackUnderflow),
            }
        } else {
            callee_at
        };
        let callee = self.stack[callee_at];

        let Value::Object(object) = callee else {
            self.throw_type_error(
                Abrupt::type_error("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        let Some(callable) = heap.object(object).and_then(Object::call).cloned() else {
            self.throw_type_error(
                Abrupt::type_error("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        // §10.3.1 — a built-in's `[[Call]]` does no receiver substitution and pushes no frame.
        // It runs to completion and leaves one value where the callee and its arguments were,
        // which is why it is answered here rather than joining the machinery below.
        let body = match callable {
            Callable::Native(native) => {
                return self.enter_native(
                    native,
                    object,
                    how,
                    callee_at,
                    receiver_at,
                    count,
                    heap,
                    chunk,
                    current,
                    at,
                );
            }
            Callable::Bytecode(body) => body,
        };
        // §10.2.1.2 and §10.2.2 — where the receiver comes from, and it comes from somewhere
        // different in each of the three ways in.
        let receiver = match how {
            // §10.2.1.2 `OrdinaryCallBindThis` — the substitution belongs to the **function**
            // rather than to the shape of the call: a non-strict function is given the global
            // object whenever the receiver is `undefined` or `null`, however it was called. So
            // `f()` and `f.call()` and `f.call(null)` all agree, and a method call only differs
            // because its receiver is an object already.
            //
            // Strict mode keeps the `undefined`, and telling the two apart needs the flag the
            // parser already computes. §7.1.18 also says a *primitive* receiver is wrapped,
            // which waits for wrapper objects.
            // A plain call has no receiver slot at all — `receiver_at` is the callee — so what
            // it passes is `undefined`, and the substitution then applies to that.
            Entry::Plain => Value::Object(self.realm.global()),
            Entry::Method => match self.stack[receiver_at] {
                Value::Undefined | Value::Null => Value::Object(self.realm.global()),
                given => given,
            },
            // §10.2.2 step 5's `OrdinaryCreateFromConstructor`: `new` *makes* the receiver, out
            // of the constructor's own `prototype` property. A `prototype` that is not an object
            // — a script may assign anything to it — falls back to `Object.prototype`, which is
            // what §10.1.13 says rather than an error.
            Entry::Construct => {
                let prototype = self.prototype_property(object, heap)?;
                Value::Object(heap.new_object(Some(prototype)))
            }
        };
        if self.frames.len() >= MAX_CALL_DEPTH {
            let thrown = self
                .realm
                .error(heap, NativeError::Range, "too much recursion");
            self.unwind(thrown, chunk, current, at)?;
            return Ok(());
        }
        // §10.2.11 — a new environment per call, written inside the one the function
        // was *defined* in. That parent is the whole of what a closure is: the
        // caller's environment has nothing to do with it, which is the difference
        // between lexical scope and dynamic scope.
        let Some(defined_in) = heap.object(object).and_then(Object::environment) else {
            return Err(Fault::MissingFunction);
        };
        let environment = heap.new_environment(Some(defined_in), body.locals());
        for offset in 0..body.parameters().min(count) {
            let argument = self.stack[callee_at + 1 + offset];
            let index = u32::try_from(offset).unwrap_or(u32::MAX);
            heap.set_variable(environment, index, argument);
        }
        self.stack.truncate(receiver_at);
        self.frames.push(Frame {
            code: (*current).take(),
            at: *at,
            this_value: self.this_value,
            environment: self.environment,
            stack_base: receiver_at,
            handlers_base: self.handlers.len(),
            // §10.2.2 step 13 — a constructor's call answers with the object it was given
            // unless its body returned an object of its own. A primitive `return` is ignored,
            // which is why `function F() { return 1; }` still constructs an `F`.
            constructed: if how == Entry::Construct {
                Some(receiver)
            } else {
                None
            },
        });
        self.environment = environment;
        self.this_value = receiver;
        *current = Some(body);
        *at = 0;
        Ok(())
    }
    /// Run a built-in and leave its answer where the call was — §10.3.1 and §10.3.2.
    ///
    /// Nothing is suspended. A built-in is Rust: it runs, it answers, and the interpreter carries
    /// on at the next instruction, so there is no frame to push and none to come back to. That is
    /// also why the recursion limit does not apply — a built-in cannot recurse into the
    /// interpreter, because it has no way to reach it.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    fn enter_native(
        &mut self,
        native: crate::heap::Native,
        function: crate::heap::ObjectId,
        how: Entry,
        callee_at: usize,
        receiver_at: usize,
        count: usize,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // §10.3.1 step 3 passes `thisArgument` straight through — no global-object substitution,
        // which is why `Error.prototype.toString.call(undefined)` throws where a sloppy-mode
        // JavaScript function would have been handed the global object instead.
        //
        // §10.3.2's `[[Construct]]` does not make the receiver either: a built-in constructor
        // makes its own object, out of its own `prototype`, which is the whole reason
        // `Error("x")` and `new Error("x")` come to the same thing.
        let this_value = match how {
            Entry::Method => self.stack[receiver_at],
            Entry::Plain | Entry::Construct => Value::Undefined,
        };
        let arguments = self.stack[callee_at + 1..callee_at + 1 + count].to_vec();
        let call = crate::heap::NativeCall {
            function,
            this_value,
            arguments: &arguments,
        };
        let answer = native(self, heap, &call);
        // The callee, its receiver and its arguments all go, and the answer takes their place —
        // exactly what a return from a JavaScript function leaves behind.
        self.stack.truncate(receiver_at);
        // `None` is a throw a handler took, and it has already moved the program counter — so
        // there is nothing to push and nothing else to do.
        if let Some(value) = self.settle(answer, heap, chunk, current, at)? {
            self.stack.push(value);
        }
        Ok(())
    }

    /// The object a constructor's instances inherit from — §10.2.2 step 5.
    ///
    /// A function's `prototype` is an ordinary writable property, so a script may put anything
    /// there. §10.1.13 says an instance falls back to `%Object.prototype%` when it is not an
    /// object, rather than the construction failing — which is why `F.prototype = 1; new F()`
    /// works and gives an ordinary object.
    fn prototype_property(
        &mut self,
        constructor: crate::heap::ObjectId,
        heap: &mut Heap,
    ) -> Result<crate::heap::ObjectId, Fault> {
        let key = crate::heap::PropertyKey::from_units(
            heap,
            &"prototype".encode_utf16().collect::<Vec<_>>(),
        );
        let found = heap
            .find_own(constructor, key)
            .map(|(_, property)| property);
        let value = match found.map(|property| property.kind) {
            Some(crate::heap::PropertyKind::Data { value, .. }) => value,
            _ => Value::Undefined,
        };
        Ok(match value {
            Value::Object(prototype) => prototype,
            _ => self.realm.object_prototype(),
        })
    }
}

/// One suspended call — where to come back to, and what to put back when we do.
///
/// A call does **not** recurse into the interpreter. The loop stays one loop and a frame is a
/// record, which is why a thousand-deep JavaScript recursion costs a thousand small structs
/// rather than a thousand Rust stack frames — and why the limit on it can be a number rather than
/// a guess about the host's stack.
#[derive(Debug)]
pub(super) struct Frame {
    /// The code that was running, and the instruction to come back to.
    pub(super) code: Option<Rc<Chunk>>,
    pub(super) at: usize,
    /// The object `new` made, if this call was a construction.
    ///
    /// §10.2.2 step 13: a constructor answers with the object it was given unless its body
    /// returned an object of its own, so the answer has to be kept until the return decides.
    pub(super) constructed: Option<Value>,
    /// The `this` to go back to.
    pub(super) this_value: Value,
    /// The environment to go back to.
    ///
    /// Not the callee's — that one may outlive the call, if the callee made a closure over it.
    pub(super) environment: EnvironmentId,
    /// Where this frame's operands begin.
    ///
    /// A floor rather than a count: returning truncates back to it, which is what makes a
    /// `return` from the middle of an expression leave nothing of that expression behind.
    pub(super) stack_base: usize,
    /// How many handlers were installed when the call began.
    ///
    /// A `try` inside the callee must not catch on the caller's behalf, and a throw that escapes
    /// the callee must find the caller's handlers intact — so unwinding pops frames and handlers
    /// together, down to this mark.
    pub(super) handlers_base: usize,
}

/// How many calls may be waiting at once before a further one is a **RangeError**.
///
/// Every engine has one and none of them is in the specification: §9.4's note says an
/// implementation may limit recursion and should report it as a RangeError, which is the
/// "Maximum call stack size exceeded" every browser prints.
///
/// The number is about memory rather than about the host's stack, because a call here is a frame
/// *record* and not a Rust frame — the interpreter's loop stays one loop however deep the
/// JavaScript goes. Ten thousand is deeper than any recursion a program means to make and
/// shallow enough that overrunning it costs a few hundred kilobytes rather than the machine.
pub(super) const MAX_CALL_DEPTH: usize = 10_000;

/// The three ways into a function, and they differ only in where the receiver comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// `f()` — no receiver, so §10.2.1.2 substitutes the global object.
    Plain,
    /// `o.m()` — the object the method was found on.
    Method,
    /// `new f()` — a fresh object, made from the constructor's `prototype`.
    Construct,
}
