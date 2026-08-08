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
            // DR-0022 — before an instruction is read, which is what makes one stop reach every
            // execution there is. A coercion re-enters this loop from the middle of an instruction,
            // a native calls back into it, a job runs after it; each of them arrives here and
            // leaves at once. Nothing a script or a host function does can clear the flag, so there
            // is no `catch`, no `finally` and no handler that resumes.
            //
            // The nested case is the one that needs it. A stopped inner execution simply returns,
            // and the call it was serving is left with a frame it never popped and no value it
            // produced — so without this the *caller* runs on for another whole check interval,
            // which is a thousand instructions of a script that was supposed to have stopped.
            if self.stopped {
                return Ok(());
            }
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
            if self.until_check > 0 {
                self.until_check -= 1;
            } else {
                self.until_check = HEAP_CHECK_INTERVAL;
                // DR-0022's budget, asked on the same counter as the heap's. `Instant::now()` is
                // tens of nanoseconds — nothing once in a thousand instructions, and not nothing on
                // every one. A run with no budget never asks the clock at all.
                //
                // Set and return rather than throw. The counter has already been reset above, so
                // the trap the heap check documents below — a raise that allocates the error it is
                // about to throw, every pass, for ever — cannot arise here either; but the reason
                // this cannot is stronger, which is that the loop does not go round again.
                // `saturating_duration_since` answers zero when the deadline is not in the
                // future, which is exactly the question — and answers it with no comparison of its
                // own. Written as `now >= deadline` the two spellings of that operator differ only
                // at the nanosecond the clock reads the deadline exactly, which is a case no test
                // can arrange and mutation coverage is right to report as uncovered. Asking the
                // question a way that has no second spelling is the honest fix.
                if self.expires_at.is_some_and(|deadline| {
                    deadline
                        .saturating_duration_since(std::time::Instant::now())
                        .is_zero()
                }) {
                    self.stopped = true;
                    return Ok(());
                }
                // §7.1's abstract operations say nothing about memory, and DR-0013's budget is
                // what ViperJS answers with instead.
                //
                // The collector is **not** run from here, and that was a measurement rather than an
                // oversight — but the measurement's premise has since changed and it has not been
                // taken again. It read: `Heap::footprint` counts arena *slots*, a swept one is
                // never reused, so a collection reclaims Strings, environments and buffers and
                // cannot reclaim what an object took. Scheduled every eight mebibytes it cost 318
                // conformance files their time budget to buy six passes; run once at the budget,
                // 79 files to buy none. The conclusion was "until a slot can be reused, walking the
                // heap buys less than the walk costs".
                //
                // **DR-0019 reused the slot.** So the condition that conclusion waited on has
                // passed, and this comment is the standing note that the numbers above are stale
                // rather than a reason. Re-measure before changing anything here — `hot-shapes` in
                // `lab/` is the experiment, and it predates DR-0019 too.
                //
                // So the *policy* is the host's — [`Vm::collect`] — and what is settled here is
                // the part that cannot be left half-right: the root set, checked against the
                // collector in `vm::tests::collecting`.
                //
                // …and a host may now hand the policy back, with `Vm::set_collection_growth`. Off
                // unless asked, so nothing above changes for a caller that says nothing.
                //
                // Measured on **growth** rather than on the total, because `footprint` is a
                // high-water mark: a collection makes slots reusable and does not lower the number,
                // so a ceiling would fire at every check once crossed. This subtraction cannot
                // underflow — `collected_at` is a footprint this run already saw and the arena only
                // ever grows — but it is written saturating anyway, because the alternative to a
                // wrong answer here is a panic and DR-0002 does not allow one for any reason.
                // …and **only when no native is holding values in Rust locals**, which is what
                // `reentries` counts. This is the condition the whole schedule turns on, and it
                // cost a measurement to find rather than an argument: `Array.prototype.sort` reads
                // its elements into a `Vec`, calls a comparator that re-enters this loop, and
                // writes them back. A collection underneath that comparator frees every element,
                // because a root set is a claim about what a *program* can name and those elements
                // are named only by Rust. The suite reported it as `undefined` where an object had
                // been — a wrong value, not a crash, exactly as this file's collector warns.
                //
                // DR-0011 already counts the re-entries for its own bound, so the fact was on the
                // machine before this needed it. What it costs is real and belongs in DR-0023: a
                // program spending its time *inside* a native re-entry — a long `sort`, a
                // `JSON.parse` with a reviver — does not collect until it comes back out.
                // Written as "is there any allowance left" rather than as a comparison, for the
                // reason DR-0022 gives about its own deadline: `>=` and `>` differ only when the
                // growth lands exactly on the threshold, which is one byte value a script cannot
                // aim at, and mutation coverage is right to call that untestable. A saturating
                // subtraction asks the same question and has no second spelling — flipping the
                // `== 0` inverts the schedule and every test here says so.
                if let Some(growth) = self.collect_after_growth
                    && self.reentries == 0
                    && self
                        .collect_next
                        .saturating_sub(heap.footprint().saturating_sub(self.collected_at))
                        == 0
                {
                    self.collect_running(root, current.as_deref(), heap);
                    self.collected_at = heap.footprint();
                    // **Proportional to what survived, and never below the base.** The walk this
                    // just did costs what is *live*, so a program holding a great deal would
                    // otherwise pay that walk once per fixed step of growth. Measured on a loop
                    // holding 150,000 objects: a mebibyte step ran 3.56 s against 0.61 s for a
                    // sixteen-mebibyte one — six times the work for the same program, and all of it
                    // re-walking the same live set.
                    //
                    // So the next allowance is the live set itself, floored at the base a host
                    // asked for. A program with nothing live collects every `growth` bytes and
                    // stays small; one holding 30 MiB is allowed to grow by 30 MiB before being
                    // walked again, which is the standard proportional rule and is what stops the
                    // schedule turning a large heap into a quadratic one.
                    self.collect_next = growth.max(heap.live_footprint());
                }
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
                Instruction::ToNumeric => {
                    let operand = self.pop()?;
                    // §7.1.4, which may run the program's own `valueOf` — hence `settle`, exactly
                    // as the unary above does.
                    let value = self
                        .to_numeric_value(operand, heap)
                        .map(|numeric| heap.numeric_value(numeric));
                    match self.settle(value, heap, root, current, at)? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Step(operator) => {
                    let operand = self.pop()?;
                    // Asked again rather than assumed. `ToNumeric` has already run — the compiler
                    // emits the two together — so this is the identity on everything that can
                    // arrive, and asking makes the arm *total* instead of leaving a case no
                    // program can reach and no test can kill.
                    let value = self.to_numeric_value(operand, heap).and_then(|numeric| {
                        Ok(match numeric {
                            crate::heap::Numeric::Number(number) => Value::Number(match operator {
                                crate::ast::UpdateOperator::Increment => number + 1.0,
                                crate::ast::UpdateOperator::Decrement => number - 1.0,
                            }),
                            // §6.1.6.2 is unbounded, so this cannot overflow the way the Number
                            // case saturates at its precision — but it does allocate, which is
                            // why the Number path does not go through the heap at all.
                            crate::heap::Numeric::BigInt(big) => {
                                let one = crate::bigint::BigInt::from_u64(1);
                                let stepped = match operator {
                                    crate::ast::UpdateOperator::Increment => big.add(&one),
                                    crate::ast::UpdateOperator::Decrement => big.subtract(&one),
                                };
                                match stepped {
                                    Ok(answer) => Value::BigInt(heap.new_bigint(answer)),
                                    Err(_) => {
                                        return Err(crate::value::Abrupt::range_error(
                                            "this BigInt is larger than this engine will allocate",
                                        ));
                                    }
                                }
                            }
                        })
                    });
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
                    let Some(symbol) = heap.well_known(at as usize) else {
                        return Err(Fault::MissingConstant);
                    };
                    self.stack.push(Value::Symbol(symbol));
                }
                Instruction::CopyRest(count) => {
                    self.copy_rest_instruction(count, heap, root, current, at)?;
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
                    self.enumerate_properties(heap, root, current, at)?;
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
                    self.store_variable(depth, index, heap, root, current, at)?;
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
                    // Read before the borrow of `running` has to end, exactly as `SetProperty`
                    // reads it: raising wants the code pointer mutably.
                    let strict = running.is_strict();
                    // Peeked, not popped, for the same reason `StoreVariable` peeks.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let key = self.global_name(running, index, heap)?;
                    let global = self.realm.global();
                    // §6.2.5.6 `PutValue` step 6 — a name that resolves to nothing is created on
                    // the global object in **sloppy** code and is a ReferenceError in strict.
                    //
                    // `HasProperty` and not an own lookup, because §9.1.1.4.1's `HasBinding` on the
                    // global object record is `HasProperty`: `toString` resolves at the top level
                    // through `Object.prototype`, so assigning to it is not assigning to nothing.
                    //
                    // This also answers §9.1.1.4.5 step 2 without a second rule. A compound
                    // assignment whose getter *deletes* the property between the read and the write
                    // is a reference whose binding has gone, and the clause makes that the same
                    // ReferenceError — which is what asking now rather than at resolve time gives.
                    if strict && !heap.has_property(global, key) {
                        let thrown = self.realm.error(
                            heap,
                            NativeError::Reference,
                            "this name is not declared and strict code may not create it",
                        );
                        self.unwind(thrown, root, current, at)?;
                        continue;
                    }
                    let stored = self.set_property_key(Value::Object(global), key, value, heap);
                    // No `continue`: this is the end of the arm either way, so a handled throw
                    // and an ordinary store leave the loop in the same place.
                    self.settle(stored, heap, root, current, at)?;
                }
                Instruction::TypeofGlobal(index) => {
                    self.typeof_global(index, heap, root, current, at)?;
                }
                Instruction::DeclareGlobal { name, deletable } => {
                    let key = self.global_name(running, name, heap)?;
                    self.declare_global(key, deletable, heap);
                }
                Instruction::CheckGlobalVar(index) | Instruction::CheckGlobalFunction(index) => {
                    let key = self.global_name(running, index, heap)?;
                    let allowed = match instruction {
                        Instruction::CheckGlobalFunction(_) => {
                            self.can_declare_global_function(key, heap)
                        }
                        _ => self.can_declare_global_var(key, heap),
                    };
                    if !allowed {
                        // §16.1.7 throws before anything is instantiated, and these instructions
                        // are emitted before every `DeclareGlobal` for exactly that reason — so
                        // unwinding from here leaves a global object the script never touched.
                        let thrown = self.realm.error(
                            heap,
                            NativeError::Type,
                            "this name cannot be declared on the global object",
                        );
                        self.unwind(thrown, root, current, at)?;
                        continue;
                    }
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
                Instruction::CompletionUndefined => {
                    self.completion = Value::Undefined;
                }
                Instruction::MakeFunction(index) => {
                    let Some(body) = running.function(index).cloned() else {
                        return Err(Fault::MissingFunction);
                    };
                    let object = self.make_function(&body, heap);
                    self.stack.push(Value::Object(object));
                }
                Instruction::MakeClass {
                    body: index,
                    derived,
                } => {
                    let Some(body) = running.function(index).cloned() else {
                        return Err(Fault::MissingFunction);
                    };
                    let Some(object) = self.make_class(&body, derived, heap, root, current, at)?
                    else {
                        continue;
                    };
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
                    self.define_class_method(kind, heap, root, current, at)?;
                }
                Instruction::Call(count) | Instruction::CallMethod(count) => {
                    let method = matches!(instruction, Instruction::CallMethod(_));
                    let how = if method { Entry::Method } else { Entry::Plain };
                    self.enter(how, count, heap, root, current, at)?;
                }
                Instruction::CallDirectEval(count) | Instruction::CallDirectEvalMethod(count) => {
                    // Whether §9.1.1.2.10's `WithBaseObject` is sitting under the callee, which is
                    // the only thing the two spellings differ in — the *question* they carry is the
                    // same one.
                    let based = matches!(instruction, Instruction::CallDirectEvalMethod(_));
                    // §13.3.6.1 — the compiler saw the name `eval`; this is the other half of the
                    // question, which is whether that name holds `%eval%` *now*. The callee sits
                    // under its arguments, having been pushed first.
                    let callee_at = self
                        .stack
                        .len()
                        .checked_sub(count as usize + 1)
                        .ok_or(Fault::StackUnderflow)?;
                    let callee = self.stack[callee_at];
                    if self.realm.is_eval(callee) {
                        // §19.2.1.1 step 2 — anything but a String is answered *unchanged*, and the
                        // arguments past the first are evaluated and discarded.
                        let source = self.stack.get(callee_at + 1).copied();
                        // The caller's strictness, from the code that is running rather than from
                        // the function that holds it: §19.2.1.1 step 5 asks about the *call site*.
                        let strict = running.is_strict();
                        // The receiver goes with it. §19.2.1.1 never reads one — the evaluated
                        // text keeps the caller's `this` — so a `WithBaseObject` here is an
                        // operand of a call that is not going to happen.
                        let base = callee_at - usize::from(based);
                        self.stack.truncate(base);
                        let answer = self.perform_direct_eval(source, strict, heap);
                        match self.settle(answer, heap, root, current, at)? {
                            Some(value) => self.stack.push(value),
                            None => continue,
                        }
                        continue;
                    }
                    // Not `%eval%` after all, so it is an ordinary call — of whichever shape the
                    // stack is already in.
                    let how = if based { Entry::Method } else { Entry::Plain };
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
                    self.call_spread(how, heap, root, current, at)?;
                }
                Instruction::Construct(count) => {
                    self.enter(Entry::Construct, count, heap, root, current, at)?;
                }
                Instruction::Return => {
                    self.return_from_call(heap, current, at)?;
                }
                Instruction::Yield => {
                    self.yield_from_generator(heap, root, current, at)?;
                }
                Instruction::PushScope(index) => {
                    // §8.3.2's `NewDeclarativeEnvironment` — a child of what is running, which is
                    // what makes an inner block see the outer one's names one hop out.
                    //
                    // The names travel with it (DR-0018), since a direct `eval` written inside the
                    // block resolves against this environment and not against a compiler.
                    let scope = running.scope(index).ok_or(Fault::MissingScope)?;
                    let (slots, names) = (scope.slots as usize, Rc::clone(&scope.names));
                    self.environment =
                        heap.new_named_environment(Some(self.environment), slots, names);
                }
                Instruction::PushWithScope(index) => {
                    // §14.11.2 steps 2 to 5 — `ToObject` first, so `with (null)` is a TypeError
                    // before any scope is made and the body never runs.
                    // The scope is read out of the running chunk *before* anything that can throw,
                    // because `running` borrows the very chunk pointer `unwind` may rewrite.
                    let scope = running.scope(index).ok_or(Fault::MissingScope)?;
                    let (slots, names) = (scope.slots as usize, Rc::clone(&scope.names));
                    let value = self.pop()?;
                    let converted = self.object_for(value, heap);
                    let object = match self.settle(converted, heap, root, current, at)? {
                        Some(Value::Object(object)) => object,
                        // `ToObject` answers an object or throws, so the middle case is not one
                        // this heap produces; `None` is a handler having taken the throw.
                        Some(_) => return Err(Fault::NotAnObject),
                        None => continue,
                    };
                    self.environment =
                        heap.new_with_environment(Some(self.environment), slots, names, object);
                }
                Instruction::LoadName(index) | Instruction::LoadNameForCall(index) => {
                    let for_call = matches!(instruction, Instruction::LoadNameForCall(_));
                    self.load_name(index, for_call, heap, root, current, at)?;
                }
                Instruction::ResolveName(index) => {
                    self.resolve_name(index, heap, root, current, at)?;
                }
                Instruction::LoadThrough(index) => {
                    let key = self.global_name(running, index, heap)?;
                    let found = *self.references.last().ok_or(Fault::MissingReference)?;
                    match self.read_resolved(found, key, running.is_strict(), heap) {
                        Ok(Some(value)) => self.stack.push(value),
                        // §6.2.5.5 — nothing anywhere is the ReferenceError an ordinary
                        // unresolvable name gets, said in the same words by the same code.
                        Ok(None) => {
                            let message = self.missing_global(key, heap);
                            let thrown = self.realm.error(heap, NativeError::Reference, &message);
                            self.unwind(thrown, root, current, at)?;
                            continue;
                        }
                        Err(error) => {
                            let thrown = self.thrown_value(error, heap);
                            self.unwind(thrown, root, current, at)?;
                            continue;
                        }
                    }
                }
                Instruction::StoreThrough(index) => {
                    self.store_through(index, heap, root, current, at)?;
                }
                Instruction::StoreName(index) => {
                    self.store_name(index, heap, root, current, at)?;
                }
                // §13.3.10 `ImportCall` — answers a promise and does everything else in a job.
                Instruction::DynamicImport => {
                    let specifier = self.pop()?;
                    let promise = match self.begin_dynamic_import(specifier, heap) {
                        Ok(promise) => promise,
                        // Step 2's `NewPromiseCapability` is the one part that can fail outright,
                        // and only if the realm's `%Promise%` has been replaced with something that
                        // is not a constructor. There is no promise to reject with, so it throws.
                        Err(error) => {
                            let thrown = self.thrown_value(error, heap);
                            self.unwind(thrown, root, current, at)?;
                            continue;
                        }
                    };
                    self.stack.push(promise);
                }
                // §13.5.1.2 — the same walk a read of this name makes, and one of three answers
                // depending on where it lands. Emitted only inside a `with`: everywhere else the
                // compiler already knows which of the three applies.
                Instruction::DeleteName(index) => {
                    self.delete_name(index, heap, root, current, at)?;
                }
                Instruction::TypeofName(index) => {
                    self.typeof_name(index, heap, root, current, at)?;
                }
                Instruction::PopScope => {
                    // A block always has something outside it — the function's own environment at
                    // the very least — so a `None` here is a chunk that does not make sense rather
                    // than the end of the chain.
                    self.environment = heap
                        .environment_at(self.environment, 1)
                        .ok_or(Fault::UnmatchedPopScope)?;
                }
                Instruction::CopyScope(index) => {
                    // §14.7.4.7 — a *sibling*: the same parent, so a loop of a million iterations
                    // makes a million environments and not a chain a million deep. Each starts
                    // holding what the last one ended with, which is how `i++` carries forward
                    // while a closure made last time keeps the value it captured.
                    let scope = running.scope(index).ok_or(Fault::MissingScope)?;
                    let (slots, names) = (scope.slots, Rc::clone(&scope.names));
                    let parent = heap
                        .environment_at(self.environment, 1)
                        .ok_or(Fault::UnmatchedPopScope)?;
                    let fresh = heap.new_named_environment(Some(parent), slots as usize, names);
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
                    self.reject_from_async(heap, current, at)?;
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
                Instruction::SettleKey => {
                    let raw = self.pop()?;
                    // §7.1.19, and this is the *only* place it runs for this key: the define at the
                    // end of the property finds a String or a Symbol and converting one of those
                    // again calls nothing.
                    let key = match self.to_property_key(raw, heap) {
                        Ok(key) => key,
                        Err(error) => {
                            self.raise(error, heap, root, current, at)?;
                            continue;
                        }
                    };
                    self.stack.push(heap.key_value(key));
                }
                Instruction::NameFunction(prefix) => {
                    let function = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let key = *self
                        .stack
                        .get(self.stack.len().wrapping_sub(2))
                        .ok_or(Fault::StackUnderflow)?;
                    name_function(self, function, key, prefix, heap);
                }
                Instruction::ImportMeta => {
                    // §13.3.12 step 2 asserts the running code belongs to a Module, and the parser
                    // has already refused `import.meta` under any other goal — so `None` here is a
                    // chunk that did not come from this compiler.
                    let meta = self
                        .import_meta(heap)
                        .ok_or(Fault::ImportMetaOutsideModule)?;
                    self.stack.push(Value::Object(meta));
                }
                Instruction::RegExpLiteral => {
                    self.regexp_literal(heap, root, current, at)?;
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
                    self.set_super_property(heap, root, current, at)?;
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
                Instruction::BindThis { depth, index } => {
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let binding = heap
                        .environment_at(self.environment, depth)
                        .ok_or(Fault::MissingLocal)?;
                    // §10.2.2's `BindThisValue` step 2 — already bound is a **ReferenceError**, and
                    // that is what makes two `super()` calls in one constructor an error rather than
                    // two constructions. Asked of the slot rather than tracked separately, so the
                    // question and the answer cannot come apart.
                    match heap.variable(binding, index) {
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
                    if !heap.set_variable(binding, index, value) {
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
                    self.complete_derived_return(index, heap, root, current, at)?;
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
                    // `CreateDataPropertyOrThrow`, and the second half of the name earns its place
                    // in exactly one of this instruction's callers. Object and array literals and
                    // the spread targets define onto something the compiler made a moment ago, so
                    // a refusal there is unreachable; §15.7.10's **static** class field defines
                    // onto the constructor, which §10.2.10 has already given a `prototype` that is
                    // neither writable nor configurable. So `static ["prototype"] = 1` is refused
                    // by the ordinary rules rather than by a check for the name, and discarding
                    // the answer here is what let it through.
                    if !heap.define_own_property(base, key, &descriptor) {
                        let error = Abrupt::type_error("this property cannot be redefined");
                        self.raise(error, heap, root, current, at)?;
                        continue;
                    }
                }
                Instruction::DefineGetter | Instruction::DefineSetter => {
                    let getter = matches!(instruction, Instruction::DefineGetter);
                    self.define_accessor(getter, heap, root, current, at)?;
                }
                Instruction::PrepareKey { base } => {
                    // §6.2.5.5 step 3.a — `ToObject` of the base, which is where a nullish one
                    // becomes a TypeError. Peeked rather than popped: the reference stays where it
                    // is, and only the key is replaced.
                    let depth = usize::try_from(base).unwrap_or(usize::MAX);
                    let Some(under) = self.stack.len().checked_sub(depth + 1) else {
                        return Err(Fault::UnbalancedStack);
                    };
                    let Some(&receiver) = self.stack.get(under) else {
                        return Err(Fault::UnbalancedStack);
                    };
                    if matches!(receiver, Value::Undefined | Value::Null) {
                        let thrown = Err(Abrupt::type_error(
                            "cannot read a property of something that is not an object",
                        ));
                        match self.settle(thrown, heap, root, current, at)? {
                            Some(_) => continue,
                            None => continue,
                        }
                    }
                    // Step 3.b — and the key is written back, which is what makes the conversion
                    // happen once for a reference that is read and then written.
                    let key = self.pop()?;
                    // Spelled here rather than where the key was made — DR-0026's trade. This is
                    // §13.2.5.5's computed key waiting on the operand stack for the define below,
                    // which is once per property written rather than once per element.
                    let settled = self
                        .to_property_key(key, heap)
                        .map(|settled| settled.to_value(heap));
                    match self.settle(settled, heap, root, current, at)? {
                        Some(key) => self.stack.push(key),
                        None => continue,
                    }
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
                    self.set_property_instruction(heap, root, current, at)?;
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
                    references: self.references.len(),
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
    /// §10.2.3 `OrdinaryFunctionCreate` and §10.2.5 `MakeConstructor` — the object a `function`
    /// expression or declaration evaluates to.
    ///
    /// Everything here is settled *now*, where the function is written, and not where it is called:
    /// the environment it closes over, the `this` an arrow captures, and the `prototype` object a
    /// constructor will hand to its instances. A closure is the first of those and nothing more.
    fn make_function(&mut self, body: &Rc<Chunk>, heap: &mut Heap) -> crate::heap::ObjectId {
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
        // §27.3.3, §27.4.3, §27.7.3 — a function's `[[Prototype]]` is chosen by **both** of the
        // words in front of it, and each of the four has an object of its own that
        // `Object.getPrototypeOf` is the only route to. Asking only whether it is a generator sent
        // an `async function*` to %GeneratorFunction.prototype% and an `async function` to
        // %Function.prototype%, which is two of the four wrong — and invisible until something
        // asked `typeof` or a `@@toStringTag` of one.
        let inherits = match (body.is_async(), body.is_generator()) {
            (false, false) => self.realm.function_prototype(),
            (false, true) => self.realm.generator_function_prototype(),
            (true, false) => self.realm.async_function_prototype(),
            (true, true) => self.realm.async_generator_function_prototype(),
        };
        let object = heap.new_function(
            inherits,
            body.clone(),
            self.environment,
            lexical,
            self.realm.id(),
        );
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
        object
    }

    /// §15.7.14 `ClassDefinitionEvaluation`, as far as the constructor object — the heritage, the
    /// function, and the pair of references that make `new C() instanceof C` true.
    ///
    /// `None` when reading the heritage threw and a handler has taken it: the caller goes round the
    /// loop rather than pushing anything, which is the shape [`Vm::settle`] already answers with.
    ///
    /// What the class body puts *on* those two objects is a run of further instructions and not
    /// this — a method is compiled and defined one at a time, so that a computed key runs where it
    /// was written.
    /// [`Instruction::StoreThrough`] — a store through the reference a `with` head resolved.
    ///
    /// # Why this is out of line, and why `#[inline(never)]` is load-bearing
    ///
    /// [`Vm::execute`] is one function and its frame is the sum of every arm's locals — 18,568
    /// bytes in a debug build, measured from its own prologue, which uses `__chkstk` because it is
    /// past a page. A **re-entry pays that frame again per level**, so an arm that keeps a `String`
    /// and three `Value`s alive across four calls is charged to every nested execution in the
    /// program rather than to the one instruction that runs it. That is what
    /// `MAX_REENTRY_DEPTH` is spending, and moving an arm here is the only lever that moves it.
    ///
    /// Re-derived rather than passed: `running` borrows what `current` owns, and the calls below
    /// want `current` mutably. Reading it inside this frame ends the borrow here.
    #[inline(never)]
    fn store_through(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        // Peeked, not popped: an assignment is an expression and its value is the caller's. The
        // *reference* is popped, because it has now been used.
        let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
        let key = self.global_name(running, index, heap)?;
        let strict = running.is_strict();
        let found = self.references.pop().ok_or(Fault::MissingReference)?;
        let stored = self.store_dynamic(found, key, value, strict, heap);
        // A `None` is the throw already raised and the handler already taken, which in the loop
        // was a `continue`. Nothing followed the arm, so returning is the same thing.
        if let Some(Value::Boolean(false)) =
            self.settle(stored.map(Value::Boolean), heap, root, current, at)?
        {
            // §6.2.5.6 — nothing in the chain answered, so the global object takes it. The same
            // tail [`Vm::store_name`] has, and the reason both need it: `Resolved` has a `Global`
            // case meaning "ask the global object", and asking it can still come back `false`.
            let global = Value::Object(self.realm.global());
            let stored = self.set_property_key(global, key, value, heap);
            self.settle(stored, heap, root, current, at)?;
        }
        Ok(())
    }

    /// [`Instruction::SetProperty`] — an ordinary property assignment, §13.15.2 and §6.2.5.6.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn set_property_instruction(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // Read before the borrow of `running` has to end: `raise` wants the code pointer mutably,
        // and the chunk this instruction came from is a borrow of it.
        let strict = current.as_deref().unwrap_or(root).is_strict();
        let value = self.pop()?;
        let key = self.pop()?;
        let base = self.pop()?;
        let done = self.set_property(base, key, value, heap);
        match self.settle(done, heap, root, current, at)? {
            // §13.15.2 — the value of an assignment is the value assigned, whether or not the
            // object took it. §6.2.5.6 step 6.d decides whether "did not take it" is the end of
            // the matter: **a silent refusal is what sloppy mode is**, and strict code throws.
            Some(Value::Boolean(false)) if strict => {
                self.raise(
                    Abrupt::type_error("this property will not take a value"),
                    heap,
                    root,
                    current,
                    at,
                )?;
            }
            Some(_) => self.stack.push(value),
            None => {}
        }
        Ok(())
    }

    /// [`Instruction::CopyRest`] — §14.3.3.1's rest of an object pattern, minus what was named.
    ///
    /// Out of line for [`Vm::store_through`]'s reason, and this one keeps a `Vec` besides.
    #[inline(never)]
    fn copy_rest_instruction(
        &mut self,
        count: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let mut excluded = Vec::with_capacity(count as usize);
        for _ in 0..count {
            excluded.push(self.pop()?);
        }
        let source = self.pop()?;
        let copied = self.copy_rest(source, &excluded, heap);
        if let Some(value) = self.settle(copied, heap, root, current, at)? {
            self.stack.push(value);
        }
        Ok(())
    }

    /// [`Instruction::SetSuperProperty`] — a write through `super.x`, which has a receiver of its
    /// own.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn set_super_property(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let key = self.pop()?;
        let base = self.pop()?;
        let receiver = self.pop()?;
        match self.set_super(base, receiver, key, value, heap) {
            // An assignment is an expression, so its value stays behind.
            Ok(_) => self.stack.push(value),
            Err(error) => {
                self.raise(error, heap, root, current, at)?;
            }
        }
        Ok(())
    }

    /// [`Instruction::EnumerateProperties`] — §14.7.5.6's list for a `for`-`in` head.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn enumerate_properties(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let prototype = self.realm.array_prototype();
        // Step 2 — `undefined` and `null` are not an error here, they are simply nothing to
        // enumerate and the loop body never runs. Any other primitive is wrapped by `ToObject`,
        // and a wrapper has no enumerable own properties except a String's, which has one per
        // index (§10.4.3).
        let keys = match value {
            Value::Object(object) => {
                // §14.7.5.10's walk can throw, because a proxy anywhere in the chain answers it
                // with traps — so this goes through the handler search like any other operation
                // that may raise.
                let walked = self
                    .enumerable_keys_through(object, heap)
                    .map(|names| heap.enumeration_of(prototype, &names));
                match self.settle(walked.map(Value::Object), heap, root, current, at)? {
                    Some(list) => list,
                    None => return Ok(()),
                }
            }
            _ => Value::Object(heap.new_array(prototype, 0)),
        };
        self.stack.push(keys);
        Ok(())
    }

    /// [`Instruction::TypeofGlobal`] — §13.5.1.1 step 2, where no such global is `"undefined"`.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn typeof_global(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let read = match self.global_binding(key, heap) {
            Some(read) => read,
            None => Ok(Value::Undefined),
        };
        let Some(value) = self.settle(read, heap, root, current, at)? else {
            return Ok(());
        };
        let answer = value.type_of(heap);
        let id = heap.new_string(answer.encode_utf16().collect());
        self.stack.push(Value::String(id));
        Ok(())
    }

    /// [`Instruction::RegExpLiteral`] — §13.2.7.3 `InstantiateRegExpLiteral`.
    ///
    /// A new object each time, so a pattern written inside a loop does not carry `lastIndex`
    /// between turns. Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn regexp_literal(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let flags = self.pop()?;
        let source = self.pop()?;
        let (Value::String(source), Value::String(flags)) = (source, flags) else {
            // The compiler pushes two string constants and nothing else can reach here, so this is
            // a chunk that does not make sense as instructions.
            return Err(Fault::MissingFunction);
        };
        let source = String::from_utf16_lossy(heap.string(source).unwrap_or(&[]));
        let flags = String::from_utf16_lossy(heap.string(flags).unwrap_or(&[]));
        let made =
            crate::builtins::regexp::from_literal(self, heap, &source, &flags).map(Value::Object);
        if let Some(value) = self.settle(made, heap, root, current, at)? {
            self.stack.push(value);
        }
        Ok(())
    }

    /// [`Instruction::TypeofName`] — §13.5.1.1, the one read that answers for a name that is
    /// nowhere.
    ///
    /// Step 2 makes an unresolvable name `"undefined"` rather than the ReferenceError every other
    /// read gives, which is the whole of what distinguishes this from [`Vm::load_name`]. Out of
    /// line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn typeof_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        let read = self.read_resolved(found, key, strict, heap);
        let settled = self.settle(
            read.map(|found| found.unwrap_or(Value::Undefined)),
            heap,
            root,
            current,
            at,
        )?;
        let Some(value) = settled else {
            return Ok(());
        };
        let answer = value.type_of(heap);
        let id = heap.new_string(answer.encode_utf16().collect());
        self.stack.push(Value::String(id));
        Ok(())
    }

    /// [`Instruction::ResolveName`] — §9.4.2 `ResolveBinding`, once.
    ///
    /// Everything after this reads and writes through what it found, which is the whole of what
    /// §13.15.2 means by "the same reference": a getter that deletes the property between the read
    /// and the write must not send the write to whatever the name would resolve to now.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    fn resolve_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        if let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? {
            self.references.push(found);
        }
        Ok(())
    }

    /// [`Instruction::LoadName`] and [`Instruction::LoadNameForCall`] — read a name through the
    /// scope chain, and for the call form push §9.1.1.2.10's `WithBaseObject` under it.
    ///
    /// Out of line for [`Vm::store_through`]'s reason. `for_call` rather than the instruction,
    /// because the two spellings differ in exactly that and passing the enum would make this frame
    /// hold a copy of it.
    #[inline(never)]
    fn load_name(
        &mut self,
        index: u32,
        for_call: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        // Read before `settle_resolution`, which borrows the `current` that `running` came from —
        // the same hoist the two store sites make.
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        // Pushed *first* so the stack reads as a method call's — receiver, callee, arguments — and
        // only for the call form.
        if for_call {
            self.stack.push(Vm::with_base(found));
        }
        match self.read_resolved(found, key, strict, heap) {
            Ok(Some(value)) => self.stack.push(value),
            // §6.2.5.5 — nothing anywhere is the ReferenceError an ordinary unresolvable name
            // gets, said in the same words by the same code.
            Ok(None) => {
                let message = self.missing_global(key, heap);
                let thrown = self.realm.error(heap, NativeError::Reference, &message);
                self.unwind(thrown, root, current, at)?;
            }
            Err(error) => {
                let thrown = self.thrown_value(error, heap);
                self.unwind(thrown, root, current, at)?;
            }
        }
        Ok(())
    }

    /// [`Instruction::StoreName`] — a store to a name resolved through the scope chain.
    ///
    /// Out of line for [`Vm::store_through`]'s reason, which is written out there.
    #[inline(never)]
    fn store_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        let stored = self.store_dynamic(found, key, value, strict, heap);
        if let Some(Value::Boolean(false)) =
            self.settle(stored.map(Value::Boolean), heap, root, current, at)?
        {
            // Nowhere in the chain: §6.2.5.6 puts it on the global object, which is what
            // `StoreGlobal` does and is reached here by the same call.
            let global = Value::Object(self.realm.global());
            let stored = self.set_property_key(global, key, value, heap);
            self.settle(stored, heap, root, current, at)?;
        }
        Ok(())
    }

    fn make_class(
        &mut self,
        body: &Rc<Chunk>,
        derived: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Option<crate::heap::ObjectId>, Fault> {
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
                    return Ok(None);
                }
            },
        };
        let object = heap.new_function(
            inheritance.constructor,
            body.clone(),
            self.environment,
            None,
            self.realm.id(),
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
        Ok(Some(object))
    }

    /// §13.3.8 — a call whose argument count is not known until the spread has been iterated.
    ///
    /// The arguments arrive as one array and are expanded onto the stack here rather than by
    /// teaching `enter` about arrays, which keeps one calling convention: by the time the frame is
    /// built the stack looks exactly as it does for a call whose count was known all along.
    fn call_spread(
        &mut self,
        how: SpreadCall,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
                return Ok(());
            }
        };
        // The array is one this compiler built a moment ago — `NewArray` and a counter it bumped
        // once per argument written — so its `length` is a finite non-negative integer and no
        // script has been anywhere near it. Read through the ordinary path anyway: a second way of
        // reading an array is a second way to be wrong about one.
        //
        // A cast saturates in Rust, so a length that could not occur clamps rather than wrapping. A
        // guard naming those cases was here and is not any more: it decided between answers no
        // input can ask for, and a branch nothing can take is one no test can hold.
        let count = match length {
            Value::Number(number) => number as u32,
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
                        return Ok(());
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
        self.enter(how, count, heap, root, current, at)
    }

    /// §10.2.2 step 13 and §27.5.3.2 — leaving a call, and the three different things that means.
    ///
    /// An ordinary return answers with the value; a construction prefers the object it made unless
    /// the body returned one of its own; and a suspendable body answers the *resumption* that
    /// entered it rather than the caller — which for an async generator pushes nothing at all,
    /// because the promise went onto the stack when the request was enqueued.
    fn return_from_call(
        &mut self,
        heap: &mut Heap,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
            Some((_, Suspendable::Generator)) => Some(self.iterator_result(heap, answer, true)),
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
        self.realm = self.realm_by_id(frame.realm);
        *current = frame.code;
        *at = frame.at;
        Ok(())
    }

    /// §10.2.2 step 13 for a **derived** constructor, where all three of its cases differ.
    ///
    /// An object return still wins; `undefined` is answered with the bound `this` — and DR-0015's
    /// binding being unbound is how a constructor that never called `super()` becomes a
    /// ReferenceError rather than answering with nothing; and every other primitive is a TypeError
    /// where a base constructor would have ignored it.
    /// Raise as though the callee's execution context were already gone — §10.2.2 step 13.
    ///
    /// `[[Construct]]` removes the callee's context at step 13 and only *then* runs step 14's
    /// TypeError and step 15's `GetThisBinding`. So both of those belong to the **caller's** realm,
    /// which is a distinction with exactly one observable consequence: which realm's `ReferenceError`
    /// a class from another realm throws when its constructor returns before calling `super()`.
    ///
    /// Swapped rather than assigned, because the throw may be *caught* inside the constructor and
    /// the body would then carry on in the wrong realm. The frame is still on the stack here —
    /// `CompleteDerivedReturn` runs before `Return` pops it — which is why the caller's realm has to
    /// be read off it rather than simply being the one in force.
    fn raise_as_caller(
        &mut self,
        error: Abrupt,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let caller = self
            .frames
            .last()
            .map_or(self.realm, |frame| self.realm_by_id(frame.realm));
        let inside = std::mem::replace(&mut self.realm, caller);
        let outcome = self.raise(error, heap, root, current, at);
        self.realm = inside;
        outcome
    }

    fn complete_derived_return(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
                    self.raise_as_caller(
                        Abrupt::reference_error(
                            "a derived constructor returned before calling `super`",
                        ),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    return Ok(());
                }
            },
            // …step 13c — and every other primitive is a TypeError, where a base
            // constructor would have ignored it and answered with the object it made.
            _ => {
                self.raise_as_caller(
                    Abrupt::type_error(
                        "a derived constructor returned something that is not an object",
                    ),
                    heap,
                    root,
                    current,
                    at,
                )?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// §15.7.14 — put one method on a class or on its prototype.
    ///
    /// Writable and configurable, and *not* enumerable. That last one is the whole run-time
    /// difference between a class method and the same syntax in an object literal.
    fn define_class_method(
        &mut self,
        kind: crate::ast::MethodKind,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let key = self.pop()?;
        let Value::Object(target) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let key = match self.property_key(key, heap) {
            Ok(key) => key,
            Err(error) => {
                self.raise(error, heap, root, current, at)?;
                return Ok(());
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
        // §15.4.5's `DefinePropertyOrThrow`, and the throw is reachable for the same reason
        // §15.7.10's is: a **static** method defines onto the constructor, whose `prototype` is
        // neither writable nor configurable. `static ["prototype"]() {}` is refused here.
        if !heap.define_own_property(target, key, &descriptor) {
            let error = Abrupt::type_error("this property cannot be redefined");
            self.raise(error, heap, root, current, at)?;
        }
        Ok(())
    }

    /// §27.5.3.7 and §27.6.3.8 — park this generator's execution and answer whoever resumed it.
    ///
    /// Which generator is a fact about the *frame* rather than about the instruction: there is no
    /// other one a `yield` could mean, and the compiler emits none outside a generator body. An
    /// **async** generator settles a promise instead of answering a resumption, and then serves
    /// whatever was asked of it while it was busy — told apart by the brand on the object the frame
    /// names, which is the only thing that distinguishes the two bodies here.
    fn yield_from_generator(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        // §27.5.3.7 suspends *the generator's own execution context*, so which
        // generator this is belongs to the frame rather than to the instruction: there
        // is no other one a `yield` could mean, and the compiler emits none outside a
        // generator body.
        let Some(generator) = self.frames.last().and_then(|frame| frame.generator) else {
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
            return Ok(());
        }
        // Wrapped before the park, so that a park that is refused leaves nothing built.
        let result = self.iterator_result(heap, value, false);
        let parked = self.park(current, at)?;
        // The generator exists — the frame named it — so this cannot answer `false`.
        heap.park_into(generator, parked);
        // Where a `Return` would have left the returned value: the resumption that
        // entered this body is being answered, and it answers with an iterator result.
        self.stack.push(result);
        Ok(())
    }

    /// §15.4.5 — put one half of an accessor on the object under construction.
    ///
    /// Only the half that was written: §10.1.6.3 leaves an absent field alone, so a getter defined
    /// after a setter joins it rather than replacing it — which is what makes `{get a() {}, set
    /// a(v) {}}` one property with both halves.
    fn define_accessor(
        &mut self,
        getter: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
                return Ok(());
            }
        };
        // Only the half that was written. §10.1.6.3 leaves an absent field alone, so
        // a getter defined after a setter joins it rather than replacing it — which
        // is what makes `{get a() {}, set a(v) {}}` one property with both.
        let half = match getter {
            true => PropertyDescriptor {
                getter: Some(function),
                ..PropertyDescriptor::EMPTY
            },
            false => PropertyDescriptor {
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
        Ok(())
    }

    /// §27.7.5.2 step 3.b — a throw that escaped an `async` body **rejects its promise**.
    ///
    /// The promise is what the call answers with, so the frame is left exactly as `Return` leaves
    /// one. An async *generator* rejects the request in service instead and pushes nothing, for the
    /// reason [`Vm::return_from_call`] gives.
    fn reject_from_async(
        &mut self,
        heap: &mut Heap,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
                .map(|context| self.settle_async(context, ReactionKind::Reject, thrown, heap))
                .or(Some(Value::Undefined)),
        };
        if let Some(answer) = answer {
            self.stack.push(answer);
        }
        self.environment = frame.environment;
        self.this_value = frame.this_value;
        self.new_target = frame.new_target;
        self.realm = self.realm_by_id(frame.realm);
        *current = frame.code;
        *at = frame.at;
        Ok(())
    }

    /// §9.1.1.1.5 `SetMutableBinding` — assign to a slot the compiler placed.
    ///
    /// Step 2's dead zone is the whole of what is left for run time: assigning to a binding that is
    /// not initialised yet is a ReferenceError and not a way to initialise it. `let x = x` reads the
    /// dead zone and `x = 1; let x;` writes to it; both are errors, and only `Initialise` may fill
    /// an empty slot.
    fn store_variable(
        &mut self,
        depth: u32,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
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
            return Ok(());
        }
        if !heap.set_variable(target, index, value) {
            return Err(Fault::MissingLocal);
        }
        Ok(())
    }

    /// §13.5.1.2 — delete a bare name, when where it lives is only known at run time.
    ///
    /// The same walk a read of the name makes, and one of three answers depending on where it
    /// lands. Emitted only inside a `with`: everywhere else the compiler already knows which of the
    /// three applies and emits that answer as a constant.
    fn delete_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let found = match self.settle_resolution(&name, key, heap, root, current, at)? {
            Some(found) => found,
            None => return Ok(()),
        };
        let answer = match found {
            // §9.1.1.1.5 — a declarative binding is not deletable, whatever it is: a
            // `var`, a parameter, a `let`, a function's own slot. The one exception the
            // specification has is §19.2.1.1's direct eval, whose `var`s *are*, and
            // ViperJS does not make those deletable either — which is a gap, not this
            // instruction's business, and it is the same answer it gave before a `with`
            // could be written around it.
            crate::vm::dynamic::Resolved::Slot { .. } => false,
            // §9.1.1.2.7 `DeleteBinding` — `[[Delete]]` of the `with` object, which may
            // run a proxy's trap and so may throw. Own-only, like every `[[Delete]]`:
            // `with (o) { delete toString }` answers true and leaves
            // `Object.prototype.toString` where it is, because `o` never had it.
            crate::vm::dynamic::Resolved::Property(object) => {
                let gone = self.delete_property_key(Value::Object(object), key, heap);
                match self.settle(gone, heap, root, current, at)? {
                    Some(value) => value.to_boolean(heap),
                    None => return Ok(()),
                }
            }
            // §13.5.1.2 step 3 — nowhere in the chain, so the global object answers, and
            // §10.1.10.1 step 2 makes a property that is not there **true**.
            crate::vm::dynamic::Resolved::Global => {
                heap.delete_own_property(self.realm.global(), key)
            }
        };
        self.stack.push(Value::Boolean(answer));
        Ok(())
    }
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
        Some(found) => found.own_property_keys(),
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

/// §10.2.9 `SetFunctionName(F, name, prefix)` — what a computed key calls the function it names.
///
/// The compile-time half of this bakes the name into the body, which a literal key allows and a
/// computed one does not. So this is the same clause reached the other way, and the parts that are
/// *not* a string copy are the reason it is worth writing out:
///
/// - **A Symbol key names the function after its description in brackets** — step 2. `Symbol("t")`
///   gives `"[t]"`, and a Symbol with no description gives the **empty string** rather than `"[]"`,
///   because §20.4's `[[Description]]` distinguishes absent from empty and step 2.b says so.
/// - **The prefix is joined with a space** — step 5 concatenates prefix, U+0020 and the name, so an
///   accessor is `"get x"`. A getter on a Symbol key is `"get [t]"`, brackets and all.
/// - **The property is not writable, not enumerable and configurable**, which is §10.3.3's set and
///   the same one `length` beside it has.
///
/// Cannot fail and does not run any code: the key has already been settled by `SettleKey`, so
/// nothing here calls a `toString`.
fn name_function(
    vm: &mut Vm,
    function: Value,
    key: Value,
    prefix: crate::compile::NamePrefix,
    heap: &mut Heap,
) {
    let Value::Object(function) = function else {
        // The compiler emits this only after a function it has just made, so there is nothing else
        // this can be — and nothing to say if a hand-written chunk arranges otherwise.
        return;
    };
    let mut name: Vec<u16> = match key {
        // Step 2 — a Symbol's description in brackets, or nothing at all when it has none.
        Value::Symbol(id) => match heap.symbol(id).and_then(|symbol| symbol.description()) {
            Some(text) => {
                let mut units = vec![u16::from(b'[')];
                units.extend_from_slice(heap.string(text).unwrap_or(&[]));
                units.push(u16::from(b']'));
                units
            }
            None => Vec::new(),
        },
        Value::String(id) => heap.string(id).unwrap_or(&[]).to_vec(),
        // `SettleKey` leaves a String or a Symbol and nothing else can arrive here.
        _ => Vec::new(),
    };
    // Step 5, and the space belongs to the clause rather than to the caller.
    if let Some(word) = match prefix {
        crate::compile::NamePrefix::Plain => None,
        crate::compile::NamePrefix::Get => Some("get "),
        crate::compile::NamePrefix::Set => Some("set "),
    } {
        let mut prefixed: Vec<u16> = word.encode_utf16().collect();
        prefixed.append(&mut name);
        name = prefixed;
    }
    let named = Value::String(heap.intern(&name));
    let slot = property_name(heap, "name");
    let _ = vm;
    heap.define_own_property(
        function,
        slot,
        &PropertyDescriptor {
            value: Some(named),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
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
