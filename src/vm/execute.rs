//! The interpreter's loop — one `match`, and the two limits a running script can meet.
//!
//! Split from [`super`] because it is one long function and the things around it are not: the
//! types, the frames and the two kinds of failure are what a reader wants beside the loop, not
//! inside it. Nothing here is reachable except through [`Vm::run`].
//!
//! `Vm`'s fields are private to `vm`, and this is a module inside it — so the loop reaches them
//! directly, as it did when the two lived in one file.

use super::call::Entry;
use super::{Fault, HEAP_CHECK_INTERVAL, Handler, Vm, jump_to};
use crate::compile::{Chunk, Instruction, ShortCircuit, SpreadCall};
use crate::heap::{Heap, PropertyDescriptor, ReactionKind, Suspendable};
use crate::realm::NativeError;
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    /// Run instructions until the code runs out, or until a nested call has returned.
    ///
    /// One loop rather than two, so the two paths into it can never disagree about what an
    /// instruction does. A nested execution stops without being told to: its root chunk is empty,
    /// so the moment `Return` points the program counter back at it there is nothing to read.
    pub(super) fn execute(
        &mut self,
        root: &Chunk,
        heap: &mut Heap,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        loop {
            // DR-0013 — the heap has a budget, and this is where a script that has spent it finds
            // out. Between instructions rather than at each allocation: the allocating functions
            // answer handles rather than completions, and making forty of them fallible for a
            // condition the loop can see from here would put a refusal on every one of their
            // callers. It is the shape `MAX_CALL_DEPTH` already uses.
            //
            // Counted down rather than asked every time. `Heap::footprint` is cheap but it is not
            // free, and a loop body of three instructions should not pay for it three times.
            //
            // The counter is reset *before* the throw rather than after it. Written the other way
            // round — as one assignment whose `None` arm throws — the `continue` leaves the
            // expression before the assignment happens, the counter stays at zero, and every pass
            // through the loop raises the error again. Each of those raises allocates the Error
            // object it is about to throw, so the check meant to stop a runaway becomes one.
            if self.until_heap_check > 0 {
                self.until_heap_check -= 1;
            } else {
                self.until_heap_check = HEAP_CHECK_INTERVAL;
                // §7.1's abstract operations say nothing about memory, and DR-0013's budget is
                // what praxis answers with instead.
                //
                // The collector is **not** run from here, and that is a measurement rather than an
                // oversight. `Heap::footprint` counts arena *slots*, and DR-0010 does not reuse a
                // swept one — so a collection reclaims Strings, environments and buffers and
                // cannot reclaim what an object took. Scheduled every eight mebibytes it cost 318
                // conformance files their time budget to buy six passes; run once at the budget,
                // 79 files to buy none. Until a slot can be reused, walking the heap buys less
                // than the walk costs.
                //
                // So the *policy* is the host's — [`Vm::collect`] — and what is settled here is
                // the part that cannot be left half-right: the root set, checked against the
                // collector in `vm::tests::collecting`.
                if heap.is_exhausted() {
                    let thrown = self.realm.error(
                        heap,
                        NativeError::Range,
                        "the heap has grown past what this engine will allocate",
                    );
                    // Nothing catches it in the usual case, and then `unwind` points the program
                    // counter past the end of the code — so the loop reads no instruction and
                    // stops, which is what makes this a refusal rather than a spin.
                    self.unwind(thrown, root, current, at)?;
                    continue;
                }
            }
            let running: &Chunk = current.as_deref().unwrap_or(root);
            let code = running.code();
            let Some(instruction) = code.get(*at).copied() else {
                return Ok(());
            };
            *at += 1;
            match instruction {
                Instruction::Constant(index) => {
                    // The *running* chunk's table, not the root's. A callee has its own
                    // constants numbered from zero, and reading the caller's by mistake is the
                    // kind of bug that gives a plausible wrong value rather than a crash.
                    let value = running.constant(index).ok_or(Fault::MissingConstant)?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let operand = self.pop()?;
                    let value = self.unary(operator, operand, heap);
                    match self.settle(value, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Binary(operator) => {
                    // Right first: it was pushed second, so it is on top. Getting this backwards
                    // would make every subtraction and comparison silently mirror itself.
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let value = self.binary(operator, left, right, heap);
                    match self.settle(value, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Jump(target) => *at = jump_to(target, code.len())?,
                Instruction::JumpIfFalse(target) => {
                    // The test is consumed either way — this is the conditional operator's jump,
                    // and `a ? b : c` evaluates to `b` or `c` and never to `a`.
                    if !self.pop()?.to_boolean(heap) {
                        *at = jump_to(target, code.len())?;
                    }
                }
                Instruction::JumpKeeping(condition, target) => {
                    // Peeked, not popped: if the short circuit fires, this value *is* the answer.
                    let deciding = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let stop = match condition {
                        ShortCircuit::WhenFalsy => !deciding.to_boolean(heap),
                        ShortCircuit::WhenTruthy => deciding.to_boolean(heap),
                        ShortCircuit::WhenNotNullish => {
                            !matches!(deciding, Value::Undefined | Value::Null)
                        }
                    };
                    if stop {
                        *at = jump_to(target, code.len())?;
                    } else {
                        // It did not decide, so it is not the answer and the right operand's
                        // value will take its place.
                        self.pop()?;
                    }
                }
                Instruction::JumpIfTrue(target) => {
                    if self.pop()?.to_boolean(heap) {
                        *at = jump_to(target, code.len())?;
                    }
                }
                Instruction::Pop => {
                    self.pop()?;
                }
                Instruction::LoadWellKnown(at) => {
                    let Some(symbol) = self.realm.well_known(at as usize) else {
                        return Err(Fault::MissingConstant);
                    };
                    self.stack.push(Value::Symbol(symbol));
                }
                Instruction::CopyRest(count) => {
                    let mut excluded = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        excluded.push(self.pop()?);
                    }
                    let source = self.pop()?;
                    let copied = self.copy_rest(source, &excluded, heap);
                    match self.settle(copied, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::RequireCoercible => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    if matches!(value, Value::Undefined | Value::Null) {
                        let thrown = Err(Abrupt::type_error(
                            "undefined and null cannot be destructured",
                        ));
                        match self.settle(thrown, heap, root, current, at)? {
                            Some(value) => self.stack.push(value),
                            None => continue,
                        }
                    }
                }
                Instruction::RequireObject => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    if !matches!(value, Value::Object(_)) {
                        let thrown = Err(Abrupt::type_error("an iterator must answer an object"));
                        match self.settle(thrown, heap, root, current, at)? {
                            Some(value) => self.stack.push(value),
                            None => continue,
                        }
                    }
                }
                Instruction::Stringify => {
                    let value = self.pop()?;
                    // May run user code — a `toString` on the value — and so may throw or unwind,
                    // which is why it goes through `settle` like a call rather than being a
                    // conversion done in place.
                    let text = self.to_string(value, heap).map(Value::String);
                    match self.settle(text, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::LoadVariable(depth, index) => {
                    let slot = heap
                        .environment_at(self.environment, depth)
                        .and_then(|at| heap.variable(at, index))
                        .ok_or(Fault::MissingLocal)?;
                    match slot {
                        Some(value) => self.stack.push(value),
                        // §9.1.1.1.6 `GetBindingValue` step 3 — the binding is there and is not
                        // initialised, which is the whole of §14.3.1's temporal dead zone. The
                        // message does not name the variable because this instruction does not
                        // carry the name; the span the parser kept is what a diagnostic would use.
                        None => {
                            self.raise(
                                Abrupt::reference_error(
                                    "a `let` or `const` was read before its declaration ran",
                                ),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::EnumerateProperties => {
                    let value = self.pop()?;
                    let prototype = self.realm.array_prototype();
                    // §14.7.5.6 step 2 — `undefined` and `null` are not an error here, they are
                    // simply nothing to enumerate, and the loop body never runs. Any other
                    // primitive would be wrapped by `ToObject`; a wrapper has no enumerable own
                    // properties and §17 makes every prototype's non-enumerable, so an empty list
                    // is the same answer for all of them but a String — whose wrapper has an own
                    // enumerable property per index (§10.4.3), and which waits for wrappers.
                    let keys = match value {
                        Value::Object(object) => {
                            // §14.7.5.10's walk can now throw, because a proxy anywhere in the
                            // chain answers it with traps — so this goes through the handler
                            // search like any other operation that may raise.
                            let walked = self
                                .enumerable_keys_through(object, heap)
                                .map(|names| heap.enumeration_of(prototype, &names));
                            match self.settle(walked.map(Value::Object), heap, root, current, at)? {
                                Some(list) => list,
                                None => continue,
                            }
                        }
                        _ => Value::Object(heap.new_array(prototype, 0)),
                    };
                    self.stack.push(keys);
                }
                Instruction::EnumerateNext(keys, index) => {
                    let object = self.pop()?;
                    let next = self.enumerate_next(object, keys, index, heap)?;
                    match self.settle(next, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Uninitialise(index) => {
                    // Always the running scope's own binding, so no depth: a block puts *its*
                    // declarations into the dead zone, never somebody else's.
                    if !heap.uninitialise(self.environment, index) {
                        return Err(Fault::MissingLocal);
                    }
                }
                Instruction::Initialise(index) => {
                    // §9.1.1.1.4 `InitializeBinding` — peeked rather than popped, on the same
                    // terms as a store: `let a = b = 1` leaves the value behind for the `b`.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    if !heap.set_variable(self.environment, index, value) {
                        return Err(Fault::MissingLocal);
                    }
                }
                Instruction::ThrowImmutableAssignment => {
                    // §9.1.1.1.5 step 3. The right-hand side is on the stack and is discarded with
                    // the rest of the expression when the throw unwinds.
                    self.raise(
                        Abrupt::type_error("assignment to a constant"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    continue;
                }
                Instruction::StoreVariable(depth, index) => {
                    // Peeked, not popped: assignment is an expression, and `a = (b = 1)` needs
                    // the inner one to leave its value behind.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let target = heap
                        .environment_at(self.environment, depth)
                        .ok_or(Fault::MissingLocal)?;
                    // §9.1.1.1.5 `SetMutableBinding` step 2 — assigning to a binding that is not
                    // initialised yet is a ReferenceError, not a way to initialise it. `let x = x`
                    // reads the dead zone; `x = 1; let x;` writes to it, and both are errors.
                    // Only `Initialise` above may fill an empty slot.
                    if heap
                        .variable(target, index)
                        .ok_or(Fault::MissingLocal)?
                        .is_none()
                    {
                        self.raise(
                            Abrupt::reference_error(
                                "a `let` or `const` was assigned to before its declaration ran",
                            ),
                            heap,
                            root,
                            current,
                            at,
                        )?;
                        continue;
                    }
                    if !heap.set_variable(target, index, value) {
                        return Err(Fault::MissingLocal);
                    }
                }
                Instruction::LoadGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    // §6.2.5.5 `GetValue` — an unresolvable reference is a ReferenceError, and
                    // this is the line that makes `missing` differ from `o.missing`.
                    let Some(read) = self.global_binding(key, heap) else {
                        let message = self.missing_global(key, heap);
                        let thrown = self.realm.error(heap, NativeError::Reference, &message);
                        self.unwind(thrown, root, current, at)?;
                        continue;
                    };
                    match self.settle(read, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::StoreGlobal(index) => {
                    // Peeked, not popped, for the same reason `StoreVariable` peeks.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let key = self.global_name(running, index, heap)?;
                    let global = Value::Object(self.realm.global());
                    // §6.2.5.6 `PutValue`: a name that resolves to nothing is created on the
                    // global object. That is the *sloppy* answer — strict code throws a
                    // ReferenceError instead — and this engine does not yet carry a strictness
                    // through to here, so it gives the one that is right for a Script's default.
                    let stored = self.set_property_key(global, key, value, heap);
                    // No `continue`: this is the end of the arm either way, so a handled throw
                    // and an ordinary store leave the loop in the same place.
                    self.settle(stored, heap, root, current, at)?;
                }
                Instruction::TypeofGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    // §13.5.1.1 step 2 — no such global is `"undefined"`, not a throw.
                    let read = match self.global_binding(key, heap) {
                        Some(read) => read,
                        None => Ok(Value::Undefined),
                    };
                    let answer = match self.settle(read, heap, root, current, at)? {
                        Some(value) => value.type_of(heap),
                        None => continue,
                    };
                    let id = heap.new_string(answer.encode_utf16().collect());
                    self.stack.push(Value::String(id));
                }
                Instruction::DeclareGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    self.declare_global(key, heap);
                }
                Instruction::DeleteGlobal(index) => {
                    let key = self.global_name(running, index, heap)?;
                    // §9.1.1.2.7 `DeleteBinding` is `[[Delete]]` of the binding object, and §10.1.10.1
                    // step 2 answers **true** for a property that is not there — which is also what
                    // §13.5.1.2 step 3 says about a name that resolves nowhere. The two paths the
                    // specification separates give the same answer here, so one call serves both.
                    //
                    // `[[Delete]]` is own-only while `HasBinding` walks the prototype chain, and that
                    // difference is deliberate on both sides: `delete toString` at the top level answers
                    // true and leaves `Object.prototype.toString` exactly where it was.
                    //
                    // Straight to the heap rather than through `delete_property_key`, because the
                    // global object is ordinary and §10.1.10.1 is total for one: there is no
                    // completion to settle and so no branch here that nothing can reach. The parser
                    // makes `delete x` an early error in strict code (§13.5.1.1), so the throw a
                    // `false` answer would earn there is unreachable from this instruction too.
                    let gone = heap.delete_own_property(self.realm.global(), key);
                    self.stack.push(Value::Boolean(gone));
                }
                Instruction::Bury(depth) => {
                    // Removed from the top and inserted lower down rather than swapped along:
                    // everything it passes keeps its order, and only this value moves.
                    let value = self.pop()?;
                    let at = self
                        .stack
                        .len()
                        .checked_sub(depth as usize)
                        .ok_or(Fault::StackUnderflow)?;
                    self.stack.insert(at, value);
                }
                Instruction::SetCompletion => {
                    self.completion = self.pop()?;
                }
                Instruction::MakeFunction(index) => {
                    let Some(body) = running.function(index) else {
                        return Err(Fault::MissingFunction);
                    };
                    // The environment the function is *written* in, not the one it will run
                    // in. That is the whole of a closure: `counter()` returns a function that
                    // still holds the environment `counter`'s call made, after the call is gone.
                    //
                    // An arrow captures the `this` in force here for exactly the same reason and
                    // at exactly the same moment (§10.2.3 step 6). Reading it at *call* time
                    // instead would be dynamic `this` wearing a lexical name: the two agree only
                    // while the arrow is called from inside the call that made it.
                    // §15.3 — an arrow reaches outward for `super` on exactly the terms it reaches
                    // outward for `this` and `new.target`: it has no `[[HomeObject]]` of its own, so
                    // §9.1.1.3's walk arrives at the method it was written in. All three are captured
                    // here rather than walked at use time, which is the same argument for each — by
                    // the time the arrow runs, the frame it was written in may be long gone.
                    let home = self
                        .frames
                        .last()
                        .and_then(|frame| frame.function)
                        .and_then(|function| heap.object(function))
                        .and_then(crate::heap::Object::home_object);
                    let lexical = body.is_arrow().then_some(crate::heap::Lexical {
                        this_value: self.this_value,
                        new_target: self.new_target,
                        home,
                    });
                    // §27.3.3 — a generator function's `[[Prototype]]` is
                    // %GeneratorFunction.prototype% rather than %Function.prototype%, which is
                    // where `Object.getPrototypeOf(function* () {})` lands and the only way a
                    // script can reach that object at all.
                    let inherits = match body.is_generator() {
                        true => self.realm.generator_function_prototype(),
                        false => self.realm.function_prototype(),
                    };
                    let object =
                        heap.new_function(inherits, body.clone(), self.environment, lexical);
                    // §20.2.4.1 — `length` is what the function says it needs, which stops at the
                    // first default and never counts a rest parameter. Not writable and not
                    // enumerable, and *configurable*, which is what lets a decorator replace it.
                    let key = crate::heap::PropertyKey::from_units(
                        heap,
                        &"length".encode_utf16().collect::<Vec<_>>(),
                    );
                    heap.define_own_property(
                        object,
                        key,
                        &crate::heap::PropertyDescriptor {
                            value: Some(Value::Number(body.length() as f64)),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                    );
                    // §10.2.9 `SetFunctionName` — not writable, not enumerable, and *configurable*,
                    // which is the set §10.3.3 gives `length` beside it. An unnamed function gets the
                    // empty string rather than no property at all: `(function () {}).name` is `""`, and
                    // `'name' in f` is true for every function.
                    let named = match body.name() {
                        Some(text) => Value::String(text),
                        None => Value::String(heap.intern(&[])),
                    };
                    let key = property_name(heap, "name");
                    heap.define_own_property(
                        object,
                        key,
                        &PropertyDescriptor {
                            value: Some(named),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..PropertyDescriptor::EMPTY
                        },
                    );
                    // §10.2.5's `MakeConstructor`: every ordinary function gets a `prototype`
                    // object, and that object gets a `constructor` back. The pair is what makes
                    // `new f() instanceof f` true, and it is made eagerly because a function may
                    // be constructed with at any time — including before anything reads it.
                    //
                    // An arrow gets neither. §15.3 gives it no `[[Construct]]`, so a `prototype`
                    // would be an object nothing could ever inherit from.
                    // …and a **method** gets neither, for the same reason it has no `[[Construct]]`:
                    // §15.4.5 makes one that is not a constructor, so a `prototype` would be an object
                    // nothing could ever inherit from. `Object.getOwnPropertyNames(o.m)` is exactly
                    // `length` and `name`, which test262 checks by name.
                    // …and a **generator** gets neither, for a third reason: §15.5.3 gives it no
                    // `[[Construct]]` either. What it gets instead is a `prototype` whose object
                    // inherits from %GeneratorPrototype% and which points back at nothing — every
                    // generator this function makes inherits from it, and §15.5.4 gives it no
                    // `constructor`, so `g().constructor` finds %GeneratorFunction.prototype% up
                    // the chain rather than `g` itself.
                    // …and an **async** function gets neither either, and not even a `prototype`:
                    // §15.8.4 gives an async function object exactly `length` and `name`, because
                    // what it answers with is a promise and nothing ever inherits from it.
                    if body.is_generator() {
                        self.realm
                            .make_generator_function(heap, object, body.is_async());
                    } else if !body.is_arrow() && !body.is_method() && !body.is_async() {
                        self.realm.make_constructor(heap, object);
                    }
                    self.stack.push(Value::Object(object));
                }
                Instruction::MakeClass {
                    body: index,
                    derived,
                } => {
                    let Some(body) = running.function(index) else {
                        return Err(Fault::MissingFunction);
                    };
                    // §15.7.14 steps 9 to 11 — the heritage read three ways. `extends null` is a
                    // class whose instances inherit from nothing, and whose constructor still
                    // inherits from `Function.prototype`; anything that is not a constructor is a
                    // TypeError, and `extends {}` is caught by that rather than by the step below.
                    let inheritance = match derived {
                        false => Inheritance {
                            prototype: Some(self.realm.object_prototype()),
                            constructor: self.realm.function_prototype(),
                        },
                        true => match self.inheritance(heap) {
                            Ok(found) => found,
                            Err(error) => {
                                self.raise(error, heap, root, current, at)?;
                                continue;
                            }
                        },
                    };
                    let object = heap.new_function(
                        inheritance.constructor,
                        body.clone(),
                        self.environment,
                        None,
                    );
                    let key = property_name(heap, "length");
                    heap.define_own_property(
                        object,
                        key,
                        &crate::heap::PropertyDescriptor {
                            value: Some(Value::Number(body.length() as f64)),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                    );
                    // §10.2.9 `SetFunctionName` — not writable, not enumerable, and *configurable*,
                    // which is the set §10.3.3 gives `length` beside it. An unnamed function gets the
                    // empty string rather than no property at all: `(function () {}).name` is `""`, and
                    // `'name' in f` is true for every function.
                    let named = match body.name() {
                        Some(text) => Value::String(text),
                        None => Value::String(heap.intern(&[])),
                    };
                    let key = property_name(heap, "name");
                    heap.define_own_property(
                        object,
                        key,
                        &PropertyDescriptor {
                            value: Some(named),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..PropertyDescriptor::EMPTY
                        },
                    );
                    // §15.7.14 steps 12 to 14 — the prototype, and the pair of references that make
                    // `new C() instanceof C` true. `prototype` is **not writable** here, which is the
                    // difference from §10.2.5's `MakeConstructor` for an ordinary function: a class
                    // may not be pointed at a different prototype after the fact.
                    let prototype = heap.new_object(inheritance.prototype);
                    // §15.7.14 step 17 `MakeMethod(F, proto)` — the constructor is a method of the
                    // prototype, which is what lets `super.x` be written in it. Set here rather than
                    // by an instruction because both objects are only in one place at this moment.
                    heap.set_home_object(object, prototype);
                    let key = property_name(heap, "constructor");
                    heap.define_own_property(
                        prototype,
                        key,
                        &crate::heap::PropertyDescriptor {
                            value: Some(Value::Object(object)),
                            writable: Some(true),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                    );
                    let key = property_name(heap, "prototype");
                    heap.define_own_property(
                        object,
                        key,
                        &crate::heap::PropertyDescriptor {
                            value: Some(Value::Object(prototype)),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(false),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                    );
                    self.stack.push(Value::Object(object));
                }
                Instruction::ClassPrototype => {
                    let Value::Object(constructor) = self.pop()? else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = property_name(heap, "prototype");
                    // Read rather than `[[Get]]`: `MakeClass` defined this a moment ago as
                    // non-writable and non-configurable, so nothing could have replaced it with an
                    // accessor, and a method definition must not be interceptable.
                    let found = heap.own_property(constructor, key).and_then(|property| {
                        match property.kind {
                            crate::heap::PropertyKind::Data { value, .. } => Some(value),
                            crate::heap::PropertyKind::Accessor { .. } => None,
                        }
                    });
                    let Some(value @ Value::Object(_)) = found else {
                        return Err(Fault::NotAnObject);
                    };
                    self.stack.push(value);
                }
                Instruction::DefineClassMethod(kind) => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let Value::Object(target) = self.pop()? else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = match self.property_key(key, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    // §15.7.14 — writable and configurable, and *not* enumerable. The last of those
                    // is the whole runtime difference from an object literal's method.
                    let descriptor = match kind {
                        crate::ast::MethodKind::Get => crate::heap::PropertyDescriptor {
                            getter: Some(value),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                        crate::ast::MethodKind::Set => crate::heap::PropertyDescriptor {
                            setter: Some(value),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                        crate::ast::MethodKind::Normal => crate::heap::PropertyDescriptor {
                            value: Some(value),
                            writable: Some(true),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..crate::heap::PropertyDescriptor::EMPTY
                        },
                    };
                    let _ = heap.define_own_property(target, key, &descriptor);
                }
                Instruction::Call(count) | Instruction::CallMethod(count) => {
                    let method = matches!(instruction, Instruction::CallMethod(_));
                    let how = if method { Entry::Method } else { Entry::Plain };
                    self.enter(how, count, heap, root, current, at)?;
                }
                Instruction::SpreadProperties => {
                    let source = self.pop()?;
                    let target = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let Value::Object(target) = target else {
                        return Err(Fault::NotAnObject);
                    };
                    let copied = self.copy_data_properties(target, source, &[], heap);
                    if self
                        .settle(copied.map(|()| Value::Undefined), heap, root, current, at)?
                        .is_none()
                    {
                        continue;
                    }
                }
                Instruction::CallSpread(how) => {
                    // The arguments arrived as one array, because §13.3.8's spread has no count until
                    // it has been iterated. Expanding it here rather than teaching `enter` about
                    // arrays keeps one calling convention: by the time the frame is built the stack
                    // looks exactly as it does for a call whose count was known all along.
                    let Value::Object(list) = self.pop()? else {
                        return Err(Fault::NotAnObject);
                    };
                    let name = property_name(heap, "length");
                    let length = match self.get_property_key(Value::Object(list), name, heap) {
                        Ok(value) => value,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    // The array is one this compiler built a moment ago, so its length is an integer
                    // and its elements are plain data. Read through the ordinary path anyway: a
                    // second way of reading an array is a second way to be wrong about one.
                    let count = match length {
                        // A float cast saturates in Rust, so an absurd length clamps rather than
                        // wrapping — and the length here was written by this compiler anyway.
                        Value::Number(number) if number >= 0.0 && number.is_finite() => {
                            number as u32
                        }
                        _ => 0,
                    };
                    for index in 0..count {
                        let key = property_name(heap, &index.to_string());
                        let value = match self.get_property_key(Value::Object(list), key, heap) {
                            Ok(value) => value,
                            Err(error) => {
                                self.raise(error, heap, root, current, at)?;
                                continue;
                            }
                        };
                        self.stack.push(value);
                    }
                    let how = match how {
                        SpreadCall::Plain => Entry::Plain,
                        SpreadCall::Method => Entry::Method,
                        SpreadCall::Construct => Entry::Construct,
                        // §13.3.7 — the one call whose callee is not on the stack, because the source
                        // never named it. Pushed under the arguments now, which is where every other
                        // call had put it before its arguments were evaluated.
                        SpreadCall::Super => {
                            let parent = match self.super_constructor(heap) {
                                Ok(parent) => parent,
                                Err(error) => {
                                    self.raise(error, heap, root, current, at)?;
                                    continue;
                                }
                            };
                            let callee_at = self
                                .stack
                                .len()
                                .checked_sub(count as usize)
                                .ok_or(Fault::StackUnderflow)?;
                            self.stack.insert(callee_at, parent);
                            Entry::Super
                        }
                    };
                    self.enter(how, count, heap, root, current, at)?;
                }
                Instruction::Construct(count) => {
                    self.enter(Entry::Construct, count, heap, root, current, at)?;
                }
                Instruction::Return => {
                    let value = self.pop()?;
                    let Some(frame) = self.frames.pop() else {
                        return Err(Fault::ReturnWithNoCall);
                    };
                    // Everything the callee left behind goes with it: its operands, its locals,
                    // and any handler it installed and did not take down.
                    self.stack.truncate(frame.stack_base);
                    self.handlers.truncate(frame.handlers_base);
                    // §10.2.2 step 13 — a construction answers with the object it made, unless
                    // the body returned an object of its own. A primitive `return` is *ignored*,
                    // which is why `function F() { return 1; }` still constructs an `F`.
                    let answer = match (frame.constructed, value) {
                        (Some(_), Value::Object(_)) | (None, _) => value,
                        (Some(made), _) => made,
                    };
                    // §27.5.3.2 step 5 — a generator's body does not answer with what it returned:
                    // the resumption that entered it answers with `{ value, done: true }`. Nothing
                    // is marked finished, because nothing needs to be — the execution was taken out
                    // of the generator to be run and is not going back, and *that* is what being
                    // completed means.
                    // Which of the three a return from this frame is: an ordinary one, a
                    // generator's — wrapped as `{ value, done: true }` — or an `async` function's,
                    // which *resolves* its promise and answers with the promise.
                    let answer = match Self::suspendable_of(&frame, heap) {
                        Some((_, Suspendable::Generator)) => {
                            Some(self.iterator_result(heap, answer, true))
                        }
                        Some((context, Suspendable::Async)) => {
                            Some(self.settle_async(context, ReactionKind::Fulfil, answer, heap))
                        }
                        // §27.6.3.2 and then §27.6.3.6 — the request being served is answered with
                        // `{ value, done: true }`, and every request behind it with the same, the
                        // body being gone. **Nothing is pushed**: the promise this resumption
                        // answers with went onto the stack when the request was enqueued, which is
                        // the one way an async generator's body differs from every other.
                        Some((generator, Suspendable::AsyncGenerator)) => {
                            self.answer_step(generator, answer, true, heap);
                            self.drain(generator, heap);
                            None
                        }
                        None => Some(answer),
                    };
                    if let Some(answer) = answer {
                        self.stack.push(answer);
                    }
                    self.environment = frame.environment;
                    self.this_value = frame.this_value;
                    self.new_target = frame.new_target;
                    *current = frame.code;
                    *at = frame.at;
                }
                Instruction::Yield => {
                    let value = self.pop()?;
                    // §27.5.3.7 suspends *the generator's own execution context*, so which
                    // generator this is belongs to the frame rather than to the instruction: there
                    // is no other one a `yield` could mean, and the compiler emits none outside a
                    // generator body.
                    let Some(generator) = self.frames.last().and_then(|frame| frame.generator)
                    else {
                        return Err(Fault::YieldOutsideGenerator);
                    };
                    // §27.6.3.8 — an async generator's `yield` *settles a promise* instead of
                    // answering a resumption, and then serves whatever was asked while it was
                    // busy. Told apart by the brand on the object the frame names, which is the
                    // only thing that distinguishes the two bodies at this instruction.
                    if heap
                        .object(generator)
                        .is_some_and(crate::heap::Object::is_async_generator)
                    {
                        let parked = self.park(current, at)?;
                        heap.park_into(generator, parked);
                        self.answer_step(generator, value, false, heap);
                        self.serve_queued(generator, heap, root, current, at)?;
                        continue;
                    }
                    // Wrapped before the park, so that a park that is refused leaves nothing built.
                    let result = self.iterator_result(heap, value, false);
                    let parked = self.park(current, at)?;
                    // The generator exists — the frame named it — so this cannot answer `false`.
                    heap.park_into(generator, parked);
                    // Where a `Return` would have left the returned value: the resumption that
                    // entered this body is being answered, and it answers with an iterator result.
                    self.stack.push(result);
                }
                Instruction::PushScope(slots) => {
                    // §8.3.2's `NewDeclarativeEnvironment` — a child of what is running, which is
                    // what makes an inner block see the outer one's names one hop out.
                    self.environment = heap.new_environment(Some(self.environment), slots as usize);
                }
                Instruction::PopScope => {
                    // A block always has something outside it — the function's own environment at
                    // the very least — so a `None` here is a chunk that does not make sense rather
                    // than the end of the chain.
                    self.environment = heap
                        .environment_at(self.environment, 1)
                        .ok_or(Fault::UnmatchedPopScope)?;
                }
                Instruction::CopyScope(slots) => {
                    // §14.7.4.7 — a *sibling*: the same parent, so a loop of a million iterations
                    // makes a million environments and not a chain a million deep. Each starts
                    // holding what the last one ended with, which is how `i++` carries forward
                    // while a closure made last time keeps the value it captured.
                    let parent = heap
                        .environment_at(self.environment, 1)
                        .ok_or(Fault::UnmatchedPopScope)?;
                    let fresh = heap.new_environment(Some(parent), slots as usize);
                    for index in 0..slots {
                        // §14.7.4.7 copies the *binding's value*, and an uninitialised one stays
                        // uninitialised: a `let` in the temporal dead zone at the moment the loop
                        // turns over is still in it on the next pass.
                        let held = heap.variable(self.environment, index).flatten();
                        let _ = match held {
                            Some(value) => heap.set_variable(fresh, index, value),
                            None => heap.uninitialise(fresh, index),
                        };
                    }
                    self.environment = fresh;
                }
                Instruction::GeneratorStart => {
                    // Which kind is a property of the code being run and of nothing else: the
                    // compiler emits this instruction only into a generator body, and `is_async`
                    // on that body is what tells §27.6's apart from §27.5's.
                    let asynchronous = running.is_async();
                    self.start_generator(asynchronous, heap, root, current, at)?;
                    continue;
                }
                Instruction::ResumeMode => {
                    // Read once and cleared: the next resumption sets it again, and a body that
                    // asked twice would be asking about a revival that had already happened.
                    let returning = std::mem::take(&mut self.resume_returns);
                    self.stack.push(Value::Boolean(returning));
                }
                Instruction::YieldDelegated => {
                    // §27.5.3.7 step 7.a.vii — what is on the stack is already the inner
                    // iterator's result object, so it goes out as it is. Everything else is the
                    // same park.
                    let result = self.pop()?;
                    let Some(generator) = self.frames.last().and_then(|frame| frame.generator)
                    else {
                        return Err(Fault::YieldOutsideGenerator);
                    };
                    let parked = self.park(current, at)?;
                    heap.park_into(generator, parked);
                    self.stack.push(result);
                }
                Instruction::ThrowNoThrowMethod => {
                    // §27.5.3.7 step 7.b.iii. The iterator has already been closed by the
                    // instructions in front of this one, which is the order the clause has: it is
                    // told the delegation is over *before* the caller is told it went wrong.
                    self.raise(
                        Abrupt::type_error("the iterator being delegated to has no `throw` method"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    continue;
                }
                Instruction::Await => {
                    let value = self.pop()?;
                    let Some(context) = self.frames.last().and_then(|frame| frame.generator) else {
                        return Err(Fault::YieldOutsideGenerator);
                    };
                    self.await_value(context, value, heap, root, current, at)?;
                }
                Instruction::AsyncReject => {
                    let thrown = self.pop()?;
                    let Some(frame) = self.frames.pop() else {
                        return Err(Fault::ReturnWithNoCall);
                    };
                    self.stack.truncate(frame.stack_base);
                    self.handlers.truncate(frame.handlers_base);
                    // §27.7.5.2 step 3.b — the promise is rejected with what was thrown, and the
                    // *promise* is what the call answers with. A `Frame` with no context is a chunk
                    // that does not make sense: only an `async` body has this instruction in it.
                    let answer = match Self::suspendable_of(&frame, heap) {
                        // §27.6.3.2 with a throw completion, then the drain: the request in service
                        // is *rejected* with what escaped, and the queue behind it answers as a
                        // completed generator does. Nothing is pushed, for the reason `Return` gives.
                        Some((generator, Suspendable::AsyncGenerator)) => {
                            self.reject_step(generator, thrown, heap);
                            self.drain(generator, heap);
                            None
                        }
                        _ => frame
                            .generator
                            .map(|context| {
                                self.settle_async(context, ReactionKind::Reject, thrown, heap)
                            })
                            .or(Some(Value::Undefined)),
                    };
                    if let Some(answer) = answer {
                        self.stack.push(answer);
                    }
                    self.environment = frame.environment;
                    self.this_value = frame.this_value;
                    self.new_target = frame.new_target;
                    *current = frame.code;
                    *at = frame.at;
                }
                Instruction::GetAsyncIterator => {
                    let iterable = self.pop()?;
                    let got =
                        crate::builtins::async_iterator::get_async_iterator(self, heap, iterable);
                    let (iterator, next) = match got {
                        Ok(pair) => pair,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    self.stack.push(iterator);
                    self.stack.push(next);
                }
                Instruction::LoadThis => self.stack.push(self.this_value),
                Instruction::LoadNewTarget => self.stack.push(self.new_target),
                Instruction::RegExpLiteral => {
                    // §13.2.7.3 `InstantiateRegExpLiteral` — a new object each time, so a pattern
                    // written inside a loop does not carry `lastIndex` between turns.
                    let flags = self.pop()?;
                    let source = self.pop()?;
                    let (Value::String(source), Value::String(flags)) = (source, flags) else {
                        // The compiler pushes two string constants and nothing else can reach here,
                        // so this is a chunk that does not make sense as instructions.
                        return Err(Fault::MissingFunction);
                    };
                    let source = String::from_utf16_lossy(heap.string(source).unwrap_or(&[]));
                    let flags = String::from_utf16_lossy(heap.string(flags).unwrap_or(&[]));
                    let made = crate::builtins::regexp::from_literal(self, heap, &source, &flags)
                        .map(Value::Object);
                    match self.settle(made, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::TemplateObject(index) => {
                    let site = TemplateSite {
                        chunk: std::ptr::from_ref(running) as usize,
                        index,
                    };
                    // §13.2.8.3 — **cached per site**, so the same tagged template hands the tag the
                    // same object every time it is evaluated. That identity is the only thing about
                    // the object a program can detect that its contents do not already say, and it is
                    // what lets a tag use it as a key into a table of its own.
                    let object = match self.templates.get(&site) {
                        Some(held) => *held,
                        None => {
                            let Some(template) = running.template(index) else {
                                return Err(Fault::MissingConstant);
                            };
                            let built = self.build_template_object(&template.clone(), heap);
                            self.templates.insert(site, built);
                            built
                        }
                    };
                    self.stack.push(Value::Object(object));
                }
                Instruction::SuperCall(count) => {
                    let parent = match self.super_constructor(heap) {
                        Ok(parent) => parent,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    // Pushed *under* the arguments, which are already on the stack, because `enter`
                    // expects the callee first — it was written that way by every other call.
                    let callee_at = self
                        .stack
                        .len()
                        .checked_sub(count as usize)
                        .ok_or(Fault::StackUnderflow)?;
                    self.stack.insert(callee_at, parent);
                    self.enter(Entry::Super, count, heap, root, current, at)?;
                }
                Instruction::MakeMethod(depth) => {
                    let below = self
                        .stack
                        .len()
                        .checked_sub(depth as usize + 1)
                        .ok_or(Fault::StackUnderflow)?;
                    let (Some(&Value::Object(function)), Value::Object(home)) =
                        (self.stack.last(), self.stack[below])
                    else {
                        return Err(Fault::NotAnObject);
                    };
                    heap.set_home_object(function, home);
                }
                Instruction::LoadSuperBase => {
                    // §9.1.1.3 `GetSuperBase` — the home object's prototype, one level above where the
                    // method was defined. A method with no home cannot be reached from source: the
                    // parser makes `super` outside a method a Syntax Error.
                    let home = self
                        .frames
                        .last()
                        .and_then(|frame| frame.function)
                        .and_then(|function| heap.object(function))
                        .and_then(crate::heap::Object::home_object);
                    let base = home
                        .and_then(|home| heap.object(home))
                        .and_then(crate::heap::Object::prototype);
                    // `null` for a class whose parent has none, and `undefined` for no home at all.
                    // Both refuse the read below, and neither is a fault: `class D extends null`
                    // reaches the first honestly.
                    self.stack.push(match (home, base) {
                        (Some(_), Some(prototype)) => Value::Object(prototype),
                        (Some(_), None) => Value::Null,
                        (None, _) => Value::Undefined,
                    });
                }
                Instruction::GetSuperProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let receiver = self.pop()?;
                    match self.get_super(base, receiver, key, heap) {
                        Ok(value) => self.stack.push(value),
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    }
                }
                Instruction::SetSuperProperty => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let receiver = self.pop()?;
                    match self.set_super(base, receiver, key, value, heap) {
                        // An assignment is an expression, so its value stays behind.
                        Ok(_) => self.stack.push(value),
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    }
                }
                Instruction::ThrowSuperDelete => {
                    // The base and the key are dropped with the rest of the expression as the throw
                    // unwinds, on the same terms as `ThrowImmutableAssignment`.
                    self.raise(
                        Abrupt::reference_error("a property of `super` cannot be deleted"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    continue;
                }
                Instruction::InheritHome => {
                    let Some(&Value::Object(function)) = self.stack.last() else {
                        return Err(Fault::NotAnObject);
                    };
                    let home = self
                        .frames
                        .last()
                        .and_then(|frame| frame.function)
                        .and_then(|running| heap.object(running))
                        .and_then(crate::heap::Object::home_object);
                    // Nothing to inherit means the running function is not a method, which no source
                    // reaches: this is only emitted inside a class constructor.
                    if let Some(home) = home {
                        heap.set_home_object(function, home);
                    }
                }
                Instruction::NewPrivateName(index) => {
                    // The description is §6.2.12's `[[Description]]`, which only a debugger reads —
                    // it takes no part in identity, so two names with the same description are two
                    // names. That is exactly a Symbol's contract.
                    let description = match running.constant(index) {
                        Some(Value::String(text)) => Some(text),
                        _ => return Err(Fault::MissingConstant),
                    };
                    let name = heap.new_symbol(description);
                    self.stack.push(Value::Symbol(name));
                }
                Instruction::SetLiteralPrototype => {
                    let value = self.pop()?;
                    // Peeked, so the literal goes on defining properties after this.
                    let Some(&Value::Object(target)) = self.stack.last() else {
                        return Err(Fault::NotAnObject);
                    };
                    // B.3.1 step 2 — an Object or `null` is set, and **anything else is ignored**: no
                    // prototype change and no property either. The answer is discarded for the same
                    // reason §10.1.2's is here: a literal's object was made a moment ago and is
                    // extensible, so nothing can refuse.
                    match value {
                        Value::Object(prototype) => {
                            heap.set_prototype_of(target, Some(prototype));
                        }
                        Value::Null => {
                            heap.set_prototype_of(target, None);
                        }
                        _ => {}
                    }
                }
                Instruction::DefinePrivateField => {
                    let value = self.pop()?;
                    let Value::Symbol(name) = self.pop()? else {
                        return Err(Fault::NotAnObject);
                    };
                    // Peeked, so one target takes field after field.
                    let Some(&Value::Object(target)) = self.stack.last() else {
                        return Err(Fault::NotAnObject);
                    };
                    if !heap.add_private_field(target, name, value) {
                        self.raise(
                            Abrupt::type_error("this object already has that private field"),
                            heap,
                            root,
                            current,
                            at,
                        )?;
                        continue;
                    }
                }
                Instruction::AddPrivateMethod | Instruction::AddPrivateAccessor => {
                    self.add_private(instruction, heap, root, current, at)?;
                }
                Instruction::GetPrivate => {
                    self.get_private(heap, root, current, at)?;
                }
                Instruction::SetPrivate => {
                    self.set_private(heap, root, current, at)?;
                }
                Instruction::HasPrivate => {
                    let Value::Symbol(name) = self.pop()? else {
                        return Err(Fault::NotAnObject);
                    };
                    let target = self.pop()?;
                    // §13.10.1 **step 3** — a non-object right-hand side is a TypeError, exactly as it
                    // is for an ordinary `in`. This read the other way for one commit, on the guess
                    // that the production existed to make the question always safe; it exists to make
                    // it safe for an *object* that lacks the name, where §7.3.31 would throw.
                    let Value::Object(object) = target else {
                        self.raise(
                            Abrupt::type_error("`in` requires an object on the right"),
                            heap,
                            root,
                            current,
                            at,
                        )?;
                        continue;
                    };
                    let held = heap
                        .object(object)
                        .is_some_and(|held| held.private_element(name).is_some());
                    self.stack.push(Value::Boolean(held));
                }
                Instruction::BindThis(index) => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    // §10.2.2's `BindThisValue` step 2 — already bound is a **ReferenceError**, and
                    // that is what makes two `super()` calls in one constructor an error rather than
                    // two constructions. Asked of the slot rather than tracked separately, so the
                    // question and the answer cannot come apart.
                    match heap.variable(self.environment, index) {
                        None => return Err(Fault::MissingLocal),
                        Some(Some(_)) => {
                            self.raise(
                                Abrupt::reference_error("`super` was already called"),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                        Some(None) => {}
                    }
                    if !heap.set_variable(self.environment, index, value) {
                        return Err(Fault::MissingLocal);
                    }
                }
                Instruction::LoadThisBinding { depth, index } => {
                    let slot = heap
                        .environment_at(self.environment, depth)
                        .and_then(|at| heap.variable(at, index))
                        .ok_or(Fault::MissingLocal)?;
                    match slot {
                        Some(value) => self.stack.push(value),
                        // §9.1.1.3 `ResolveThisBinding` on a record whose `[[ThisBindingStatus]]` is
                        // still `uninitialized` — which is every use of `this` in a derived
                        // constructor above its `super()`.
                        None => {
                            self.raise(
                                Abrupt::reference_error(
                                    "`this` was read before `super` was called",
                                ),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::CompleteDerivedReturn(index) => {
                    let value = self.pop()?;
                    match value {
                        // §10.2.2 step 13a — an object return wins, exactly as in a base constructor.
                        Value::Object(_) => self.stack.push(value),
                        // …step 13b — `undefined` is answered with the bound `this`, and the binding
                        // being unbound is how a constructor that never called `super()` becomes a
                        // ReferenceError rather than answering with nothing.
                        Value::Undefined => match heap.variable(self.environment, index) {
                            None => return Err(Fault::MissingLocal),
                            Some(Some(bound)) => self.stack.push(bound),
                            Some(None) => {
                                self.raise(
                                    Abrupt::reference_error(
                                        "a derived constructor returned before calling `super`",
                                    ),
                                    heap,
                                    root,
                                    current,
                                    at,
                                )?;
                                continue;
                            }
                        },
                        // …step 13c — and every other primitive is a TypeError, where a base
                        // constructor would have ignored it and answered with the object it made.
                        _ => {
                            self.raise(
                                Abrupt::type_error(
                                    "a derived constructor returned something that is not an object",
                                ),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                    }
                }
                Instruction::Duplicate => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    self.stack.push(value);
                }
                Instruction::NewArray(length) => {
                    let prototype = self.realm.array_prototype();
                    let array = heap.new_array(prototype, length);
                    self.stack.push(Value::Object(array));
                }
                Instruction::NewObject => {
                    let object = heap.new_object(Some(self.realm.object_prototype()));
                    self.stack.push(Value::Object(object));
                }
                Instruction::DuplicateTop(count) => {
                    // A whole *reference* at once, whatever its width: two values for `o.x`, three for
                    // `super.x`, and a compound assignment needs the lot twice over — once for the read
                    // and once for the write, because §13.15.2 evaluates the reference only once.
                    let count = count as usize;
                    let from = self
                        .stack
                        .len()
                        .checked_sub(count)
                        .ok_or(Fault::StackUnderflow)?;
                    // Collected before pushing: extending a `Vec` from a slice of itself is not a thing
                    // Rust will allow, and the borrow is what says so.
                    let copied: Vec<Value> = self.stack[from..].to_vec();
                    self.stack.extend_from_slice(&copied);
                }
                Instruction::DefineField => {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    // Peeked: an object literal defines one property after another and the object
                    // is the expression's value, so it stays where it is until the last one.
                    let base = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let Value::Object(base) = base else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = match self.property_key(key, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    let descriptor = PropertyDescriptor {
                        value: Some(value),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..PropertyDescriptor::EMPTY
                    };
                    // `CreateDataProperty` on a fresh, extensible object cannot be refused.
                    let _ = heap.define_own_property(base, key, &descriptor);
                }
                Instruction::DefineGetter | Instruction::DefineSetter => {
                    let function = self.pop()?;
                    let key = self.pop()?;
                    let base = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let Value::Object(base) = base else {
                        return Err(Fault::NotAnObject);
                    };
                    let key = match self.property_key(key, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    // Only the half that was written. §10.1.6.3 leaves an absent field alone, so
                    // a getter defined after a setter joins it rather than replacing it — which
                    // is what makes `{get a() {}, set a(v) {}}` one property with both.
                    let half = match instruction {
                        Instruction::DefineGetter => PropertyDescriptor {
                            getter: Some(function),
                            ..PropertyDescriptor::EMPTY
                        },
                        _ => PropertyDescriptor {
                            setter: Some(function),
                            ..PropertyDescriptor::EMPTY
                        },
                    };
                    // §15.4.5 gives an accessor made this way `[[Enumerable]]` and
                    // `[[Configurable]]`, the same two an ordinary literal property gets.
                    let descriptor = PropertyDescriptor {
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..half
                    };
                    let _ = heap.define_own_property(base, key, &descriptor);
                }
                Instruction::GetProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let got = self.get_property(base, key, heap);
                    match self.settle(got, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::SetProperty => {
                    // Read before the borrow of `running` has to end: `raise` wants the code pointer
                    // mutably, and the chunk this instruction came from is a borrow of it.
                    let strict = running.is_strict();
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.set_property(base, key, value, heap);
                    match self.settle(done, heap, root, current, at)? {
                        // §13.15.2 — the value of an assignment is the value assigned, whether or not
                        // the object took it. §6.2.5.6 step 6.d decides whether "did not take it" is
                        // the end of the matter: **a silent refusal is what sloppy mode is**, and
                        // strict code throws instead.
                        Some(Value::Boolean(false)) if strict => {
                            self.raise(
                                Abrupt::type_error("this property will not take a value"),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                        Some(_) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::DeleteProperty => {
                    let strict = running.is_strict();
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.delete_property(base, key, heap);
                    match self.settle(done, heap, root, current, at)? {
                        // §13.5.1.2 step 5.b — the same rule as an assignment's, from the other side:
                        // `delete` answers `false` for a non-configurable property, and strict code
                        // turns that answer into a throw.
                        Some(Value::Boolean(false)) if strict => {
                            self.raise(
                                Abrupt::type_error("this property cannot be deleted"),
                                heap,
                                root,
                                current,
                                at,
                            )?;
                            continue;
                        }
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::HasProperty => {
                    let base = self.pop()?;
                    let key = self.pop()?;
                    let has = self.has_property(base, key, heap);
                    match self.settle(has, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Instanceof => {
                    // Right first: it was pushed second, so it is on top — the same order every
                    // binary operator here pops in.
                    let target = self.pop()?;
                    let value = self.pop()?;
                    let answer = self.instance_of(value, target, heap);
                    match self.settle(answer, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Throw => {
                    // §6.2.4 — a throw completion travels up until something wants it. Here that
                    // is the innermost handler; with nothing to want it, it leaves the script.
                    let thrown = self.pop()?;
                    self.unwind(thrown, root, current, at)?;
                }
                Instruction::PushHandler(target) => self.handlers.push(Handler {
                    target,
                    frames: self.frames.len(),
                    depth: self.stack.len(),
                    environment: self.environment,
                }),
                Instruction::PopHandler => {
                    // A pop with nothing to pop is a chunk that does not make sense: the compiler
                    // emits these in pairs.
                    self.handlers.pop().ok_or(Fault::UnmatchedPopHandler)?;
                }
            }
        }
    }
}

/// Which tagged template a cached object belongs to — §13.2.8.3's "same Parse Node".
///
/// The chunk's address stands for the Parse Node, which is exactly as stable as it needs to be: a
/// chunk is immutable once compiled and is held alive by the function object that owns it, so two
/// evaluations of one site are two runs of the same address. The index distinguishes the sites within
/// it. Not a `Chunk` reference, because the map outlives any one borrow of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TemplateSite {
    /// The chunk the site is in, by address.
    chunk: usize,
    /// Which site in that chunk.
    index: u32,
}

impl Vm {
    /// §13.2.8.3 `GetTemplateObject` — the frozen pair of Arrays a tag is handed.
    ///
    /// Frozen and with a frozen `raw` beside it, which is what makes the object safe to hand out and
    /// keep: a tag cannot change what a later evaluation of the same site will see. A cooked component
    /// that is `None` becomes `undefined` — §12.9.6 leaves `TV` undefined for an escape that is not
    /// one, which only a *tagged* template may contain.
    #[inline(never)]
    fn build_template_object(
        &mut self,
        template: &crate::compile::Template,
        heap: &mut Heap,
    ) -> crate::heap::ObjectId {
        let prototype = self.realm.array_prototype();
        let count = u32::try_from(template.raw.len()).unwrap_or(u32::MAX); // bounded by the source length
        let cooked = heap.new_array(prototype, count);
        let raw = heap.new_array(prototype, count);
        for (at, (one, other)) in template.cooked.iter().zip(&template.raw).enumerate() {
            let key = heap.index_key(u32::try_from(at).unwrap_or(u32::MAX)); // same
            let value = match one {
                Some(text) => Value::String(*text),
                None => Value::Undefined,
            };
            heap.define_own_property(cooked, key, &PropertyDescriptor::data(value));
            heap.define_own_property(raw, key, &PropertyDescriptor::data(Value::String(*other)));
        }
        // §13.2.8.3 steps 10 and 11 — both arrays are frozen, `raw` before it is attached.
        freeze(heap, raw);
        let key = property_name(heap, "raw");
        // §13.2.8.3 step 9 asks for all three attributes false. Only `enumerable` is stated here: the
        // freeze on the next line sets the other two a moment later, so writing them would be two
        // values no input could tell from their absence — which mutation coverage duly reported.
        heap.define_own_property(
            cooked,
            key,
            &PropertyDescriptor {
                value: Some(Value::Object(raw)),
                enumerable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        );
        freeze(heap, cooked);
        cooked
    }
}

/// `SetIntegrityLevel(o, frozen)` for an array this engine has just built — §7.3.15.
///
/// Every property is made non-writable and non-configurable and the object non-extensible. Narrower
/// than the general operation because the object is one made a moment ago: there are no accessors
/// among its properties and nothing can have refused, so there is no answer to report.
fn freeze(heap: &mut Heap, object: crate::heap::ObjectId) {
    let keys = match heap.object(object) {
        Some(found) => found.own_property_keys(heap),
        None => return,
    };
    for key in keys {
        heap.define_own_property(
            object,
            key,
            &PropertyDescriptor {
                writable: Some(false),
                configurable: Some(false),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
    if let Some(found) = heap.object_mut(object) {
        found.prevent_extensions();
    }
}

/// One built-in property name, interned.
///
/// Written out four times inside `MakeClass` it was the same three lines of UTF-16 encoding each
/// time, which said nothing about what the property was for.
fn property_name(heap: &mut Heap, name: &str) -> crate::heap::PropertyKey {
    crate::heap::PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>())
}

/// The two prototypes a class definition points its halves at — §15.7.14 steps 9 to 11.
///
/// A pair rather than two values because the three cases decide them *together*: `extends null` sets
/// one to nothing and the other to `Function.prototype`, and nothing sets only one of them.
struct Inheritance {
    /// What instances inherit from — `[[Prototype]]` of the class's `prototype` object.
    ///
    /// `None` for `extends null`, which is the whole reason it is an `Option`: a class whose
    /// instances inherit from nothing at all is legal, and its instances have no `toString`.
    prototype: Option<crate::heap::ObjectId>,
    /// What the constructor itself inherits from, which is how a static method is inherited.
    constructor: crate::heap::ObjectId,
}

impl Vm {
    /// §7.3.30 `PrivateMethodOrAccessorAdd` — give an object a private method or accessor.
    ///
    /// Out of line, and every method in this block is out of line for one reason: [`Vm::execute`] is a
    /// single `match`, so its Rust frame is the sum of every arm's locals — and §7.1.1's conversions
    /// re-enter the interpreter, paying that frame again per level. `MAX_REENTRY_DEPTH` is a
    /// *measured* number against a one-mebibyte stack, and writing these three inline was enough to
    /// break its margin: `a_conversion_at_the_cap_fits_in_the_stack_it_claims_to_need` found it by
    /// overflowing, which is exactly what that guard is for. `inline` is refused for the same reason,
    /// because a release build that folded them back in would put the frame back with them.
    #[inline(never)]
    fn add_private(
        &mut self,
        instruction: Instruction,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let element = match instruction {
            Instruction::AddPrivateAccessor => {
                let setter = self.pop()?;
                let getter = self.pop()?;
                crate::heap::PrivateElement::Accessor { getter, setter }
            }
            // Listed rather than defaulted, so a third kind cannot arrive here unnoticed.
            _ => crate::heap::PrivateElement::Method(self.pop()?),
        };
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        // Peeked, so one target takes element after element.
        let Some(&Value::Object(target)) = self.stack.last() else {
            return Err(Fault::NotAnObject);
        };
        // §7.3.30 step 2 — an existing name is a TypeError, with no exception for an accessor. Its two
        // halves are **one** element built by §15.7.14 at the class definition, so by the time this
        // runs there is one add per name; merging here instead let the same accessor be added to one
        // object twice, which the specification refuses and a re-entered constructor reaches.
        if !heap.add_private_element(target, name, element) {
            self.raise(
                Abrupt::type_error("this object already has that private element"),
                heap,
                root,
                current,
                at,
            )?;
        }
        Ok(())
    }

    /// §7.3.31 `PrivateGet` — read a private field, method or accessor, or throw.
    ///
    /// Out of line; see [`Vm::add_private`].
    #[inline(never)]
    fn get_private(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let target = self.pop()?;
        // §7.3.31 step 1 — a primitive carries no private elements, so it fails the same way an
        // object without the name does. No wrapper is made: a wrapper would have none either.
        let found = match target {
            Value::Object(object) => heap
                .object(object)
                .and_then(|held| held.private_element(name)),
            _ => None,
        };
        let Some(element) = found else {
            self.raise(
                Abrupt::type_error("this object has no such private field"),
                heap,
                root,
                current,
                at,
            )?;
            return Ok(());
        };
        // §7.3.31 step 4 — a field or a method answers directly; an accessor's getter is **called**,
        // with the object as its receiver, which is why this cannot be the heap's business alone. A
        // getter-less accessor is a TypeError where a public one would have answered `undefined`.
        let value = match element {
            crate::heap::PrivateElement::Accessor { getter, .. } => {
                if matches!(getter, Value::Undefined) {
                    self.raise(
                        Abrupt::type_error("this private accessor has no getter"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    return Ok(());
                }
                match self.call_value(getter, target, &[], heap) {
                    Ok(value) => value,
                    Err(error) => {
                        self.raise(error, heap, root, current, at)?;
                        return Ok(());
                    }
                }
            }
            // A field and a method both hold one value, and an accessor was answered above.
            held => match held.value() {
                Some(value) => value,
                None => return Err(Fault::NotAnObject),
            },
        };
        self.stack.push(value);
        Ok(())
    }

    /// §7.3.32 `PrivateSet` — write a private field or call a private setter, or throw.
    ///
    /// Out of line; see [`Vm::add_private`].
    #[inline(never)]
    fn set_private(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let target = self.pop()?;
        let element = match target {
            Value::Object(object) => heap
                .object(object)
                .and_then(|held| held.private_element(name)),
            _ => None,
        };
        // §7.3.32 reads the kind before it writes anything, and two of the three never reach the heap:
        // a **method** refuses assignment outright, which is what makes `#m` unlike a field holding a
        // function, and an accessor's setter is called.
        match element {
            Some(crate::heap::PrivateElement::Accessor { setter, .. }) => {
                if matches!(setter, Value::Undefined) {
                    self.raise(
                        Abrupt::type_error("this private accessor has no setter"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    return Ok(());
                }
                if let Err(error) = self.call_value(setter, target, &[value], heap) {
                    self.raise(error, heap, root, current, at)?;
                    return Ok(());
                }
            }
            Some(crate::heap::PrivateElement::Method(_)) => {
                self.raise(
                    Abrupt::type_error("a private method cannot be assigned to"),
                    heap,
                    root,
                    current,
                    at,
                )?;
                return Ok(());
            }
            Some(crate::heap::PrivateElement::Field(_)) => {
                let Value::Object(object) = target else {
                    return Err(Fault::NotAnObject);
                };
                if !heap.set_private_field(object, name, value) {
                    return Err(Fault::NotAnObject);
                }
            }
            None => {
                self.raise(
                    Abrupt::type_error("this object has no such private field"),
                    heap,
                    root,
                    current,
                    at,
                )?;
                return Ok(());
            }
        }
        self.stack.push(value);
        Ok(())
    }

    /// §10.2.2's `GetSuperConstructor` — the running function's `[[Prototype]]`.
    ///
    /// Read now rather than captured when the class was defined, because it is *mutable*:
    /// `Object.setPrototypeOf(D, Other)` changes which constructor `super()` reaches, and a class
    /// definition that had recorded the answer would go on calling the old one.
    fn super_constructor(&mut self, heap: &Heap) -> Result<Value, crate::value::Abrupt> {
        let running = self.frames.last().and_then(|frame| frame.function);
        // Unreachable from source: the parser makes `super(…)` outside a derived constructor a
        // Syntax Error, and a constructor is always entered through a frame. A hand-written chunk
        // can ask, and this is the honest answer rather than a panic.
        let Some(running) = running else {
            return Err(crate::value::Abrupt::type_error(
                "`super` was called outside a constructor",
            ));
        };
        let parent = heap
            .object(running)
            .and_then(crate::heap::Object::prototype);
        // §10.2.2 step 3 — the parent must be a constructor. `class D extends null {}` arrives here
        // with `Function.prototype`, which is callable and *not* a constructor, so this is where
        // `new D()` on such a class becomes the TypeError §15.7.14 promised at step 9.
        let constructs = parent.is_some_and(|parent| {
            heap.object(parent)
                .and_then(crate::heap::Object::call)
                .is_some_and(crate::heap::Callable::constructs)
        });
        match (parent, constructs) {
            (Some(parent), true) => Ok(Value::Object(parent)),
            _ => Err(crate::value::Abrupt::type_error(
                "the superclass is not a constructor",
            )),
        }
    }

    /// Read the `extends` value on top of the stack as §15.7.14 steps 9 to 11 read it.
    ///
    /// Three cases, and the middle one is the reason this is not a property access: `extends {}` and
    /// `extends 1` are TypeErrors because the value is not a **constructor**, which is a question
    /// about `[[Construct]]` and not about being callable — so `extends Math.max` fails here too,
    /// where `Math.max.prototype` would simply have been `undefined`.
    fn inheritance(&mut self, heap: &mut Heap) -> Result<Inheritance, crate::value::Abrupt> {
        // A missing operand is a chunk that does not make sense rather than a throw, and there is
        // nothing to inherit from either way — the compiler emits the heritage before this.
        let heritage = self.stack.pop().unwrap_or(Value::Undefined);
        // §15.7.14 step 9 — `extends null` is not an error and not the same as no `extends` at all:
        // the class is still *derived*, so its constructor must call `super()`, and `super()` will
        // then find `null` where a constructor should be. That is a run-time TypeError per
        // construction rather than a definition-time one, which is what the specification says.
        if matches!(heritage, Value::Null) {
            return Ok(Inheritance {
                prototype: None,
                constructor: self.realm.function_prototype(),
            });
        }
        let constructs = match heritage {
            Value::Object(parent) => heap
                .object(parent)
                .and_then(crate::heap::Object::call)
                .is_some_and(crate::heap::Callable::constructs),
            _ => false,
        };
        let (Value::Object(parent), true) = (heritage, constructs) else {
            return Err(crate::value::Abrupt::type_error(
                "a class may only extend a constructor or null",
            ));
        };
        // §15.7.14 step 11 — the parent's `prototype` is read with `[[Get]]`, so a getter runs and a
        // Proxy would be consulted. It must be an Object or null; a parent whose `prototype` was
        // replaced with a number is a TypeError, and this is the one place that check lives.
        let key = property_name(heap, "prototype");
        let found = self.get_property_key(Value::Object(parent), key, heap)?;
        let prototype = match found {
            Value::Object(prototype) => Some(prototype),
            Value::Null => None,
            _ => {
                return Err(crate::value::Abrupt::type_error(
                    "the `prototype` of an extended constructor is neither an object nor null",
                ));
            }
        };
        Ok(Inheritance {
            prototype,
            constructor: parent,
        })
    }
}
