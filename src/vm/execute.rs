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
use crate::compile::{Chunk, Instruction, ShortCircuit};
use crate::heap::{Heap, PropertyDescriptor};
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
                        Value::Object(object) => heap.new_enumeration(prototype, object),
                        _ => heap.new_array(prototype, 0),
                    };
                    self.stack.push(Value::Object(keys));
                }
                Instruction::EnumerateNext(keys, index) => {
                    let object = self.pop()?;
                    let next = self.enumerate_next(object, keys, index, heap)?;
                    self.stack.push(next);
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
                    let lexical_this = body.is_arrow().then_some(self.this_value);
                    let object = heap.new_function(
                        self.realm.function_prototype(),
                        body.clone(),
                        self.environment,
                        lexical_this,
                    );
                    // §10.2.5's `MakeConstructor`: every ordinary function gets a `prototype`
                    // object, and that object gets a `constructor` back. The pair is what makes
                    // `new f() instanceof f` true, and it is made eagerly because a function may
                    // be constructed with at any time — including before anything reads it.
                    //
                    // An arrow gets neither. §15.3 gives it no `[[Construct]]`, so a `prototype`
                    // would be an object nothing could ever inherit from.
                    if !body.is_arrow() {
                        self.realm.make_constructor(heap, object);
                    }
                    self.stack.push(Value::Object(object));
                }
                Instruction::Call(count) | Instruction::CallMethod(count) => {
                    let method = matches!(instruction, Instruction::CallMethod(_));
                    let how = if method { Entry::Method } else { Entry::Plain };
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
                    self.stack.push(answer);
                    self.environment = frame.environment;
                    self.this_value = frame.this_value;
                    *current = frame.code;
                    *at = frame.at;
                }
                Instruction::LoadThis => self.stack.push(self.this_value),
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
                Instruction::DuplicateTwo => {
                    let depth = self.stack.len();
                    let (Some(first), Some(second)) = (
                        depth
                            .checked_sub(2)
                            .and_then(|at| self.stack.get(at))
                            .copied(),
                        self.stack.last().copied(),
                    ) else {
                        return Err(Fault::StackUnderflow);
                    };
                    self.stack.push(first);
                    self.stack.push(second);
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
                    let value = self.pop()?;
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.set_property(base, key, value, heap);
                    match self.settle(done, heap, root, current, at)? {
                        // §13.15.2 — the value of an assignment is the value assigned, whether or
                        // not the object took it. A silent refusal is what sloppy mode is.
                        Some(_) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::DeleteProperty => {
                    let key = self.pop()?;
                    let base = self.pop()?;
                    let done = self.delete_property(base, key, heap);
                    match self.settle(done, heap, root, current, at)? {
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
