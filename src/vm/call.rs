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
        // Which ways in have a value under the callee. A method call has its receiver there, and
        // §28.1.2's has the `new.target` the caller named — a construction makes its own receiver,
        // so that slot is free and this is what it is free for.
        let method = matches!(how, Entry::Method | Entry::Named);
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
            self.raise(
                Abrupt::type_error("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        let lexical = heap.object(object).and_then(Object::lexical);
        let Some(callable) = heap.object(object).and_then(Object::call).cloned() else {
            self.raise(
                Abrupt::type_error("what was called is not a function"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        };
        // §7.3.13 `Construct` requires an `IsConstructor`, and being callable is not it. An arrow
        // (§15.3) and nearly every built-in (§10.3) have a `[[Call]]` and no `[[Construct]]`, so
        // `new` in front of one is a TypeError rather than an object that nothing could be an
        // instance of. Asked here, before the bound chain is flattened, because a bound function
        // answers for itself — it is a constructor exactly when its target is, and it recorded
        // that when it was made.
        if matches!(how, Entry::Construct | Entry::Super | Entry::Named) && !callable.constructs() {
            self.raise(
                Abrupt::type_error("what was used with `new` is not a constructor"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        }
        // §10.4.1 — a bound function is not a function of its own: it stands in front of another
        // one with a receiver and some arguments already decided. Resolved here, before anything
        // else, because what is actually being entered is the target.
        if matches!(callable, Callable::Bound(_)) {
            return self.enter_bound(
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
        // §10.3.1 — a built-in's `[[Call]]` does no receiver substitution and pushes no frame.
        // It runs to completion and leaves one value where the callee and its arguments were,
        // which is why it is answered here rather than joining the machinery below.
        let body = match callable {
            // Answered above; listed so that a third kind cannot arrive here unnoticed.
            Callable::Bound(_) => return Err(Fault::MissingFunction),
            // §27.5.1's three resumptions, which are not functions with bodies at all: what they
            // enter is a generator's parked execution. Answered here, beside the built-ins, because
            // like a built-in they push no frame of the callee's own — and unlike one, they may
            // leave the loop running inside a body.
            // §27.7.5.3's two closures. Like a resumption they enter the loop rather than running
            // in Rust, and unlike one they answer to nobody: the job that calls them discards the
            // completion, which is why nothing here is left on the stack for a caller.
            Callable::Revive { kind, context } => {
                return self.enter_revive(
                    kind,
                    context,
                    callee_at,
                    receiver_at,
                    count,
                    heap,
                    current,
                    at,
                );
            }
            // §27.6.1's three methods and §27.5.1's read the same and are not the same: one
            // answers a promise and the other an iterator result, and each refuses the other's
            // receiver. Which prototype the method came off is carried on the function object,
            // because by the time it is called there is nothing else left to ask.
            Callable::Resume { kind, asynchronous } if asynchronous => {
                return self.enter_async_resume(
                    kind,
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
            Callable::Resume { kind, .. } => {
                return self.enter_resume(
                    kind,
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
            Callable::Native { native, .. } => {
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

        // §15.7.14 — a class constructor has a `[[Construct]]` and its `[[Call]]` does nothing but
        // refuse. Checked here rather than at the class definition because the two are separated by
        // any amount of program: what arrives at a call site is a function object, and the body it
        // holds is the only thing that still remembers how it was written.
        if body.is_class_constructor()
            && !matches!(how, Entry::Construct | Entry::Super | Entry::Named)
        {
            self.raise(
                Abrupt::type_error("a class constructor cannot be called without `new`"),
                heap,
                chunk,
                current,
                at,
            )?;
            return Ok(());
        }

        // §9.1.1.3 — `[[NewTarget]]` is the constructor a `new` named, `undefined` for every other
        // way in, and *inherited* by a `super()`: §10.2.2 passes newTarget down rather than
        // replacing it, which is what makes a chain of `extends` produce an instance of the class
        // that was written after `new`. Computed before the receiver because the receiver is made
        // from it.
        //
        // An arrow has no function environment of its own, so §13.3.12's lookup walks outward and
        // arrives at whatever was in force where the arrow was written — the same walk, and the same
        // answer, as the `this` below.
        let new_target = match lexical {
            Some(captured) => captured.new_target,
            None => match how {
                Entry::Construct => callee,
                Entry::Super => self.new_target,
                // The receiver slot carries it: a construction makes its own receiver from
                // `new.target`, so the slot is free and this is what it is free for.
                Entry::Named => self.stack[receiver_at],
                Entry::Plain | Entry::Method => Value::Undefined,
            },
        };
        // §10.2.2 — a *derived* constructor is not given a receiver at all. `this` starts unbound
        // and `super()` creates it, which is DR-0015; making one here would be an object the
        // constructor could never see and the parent would make a second.
        let derived = body.derived_this().is_some();
        // §10.2.2 step 5's `OrdinaryCreateFromConstructor`: `new` *makes* the receiver, out of
        // **new.target's** `prototype` property. For a plain `new` new.target and the callee are
        // the same object, which is why this was invisible until `super()` existed: there the
        // callee is the parent and new.target is the class written after `new`, and only the
        // second has the prototype the instance must inherit from.
        //
        // Built here rather than in the `match` below because reading that property is a `[[Get]]`
        // — a getter, or a proxy's trap — and so may throw, which an arm producing a value cannot.
        let made = if matches!(how, Entry::Construct | Entry::Super | Entry::Named) && !derived {
            let from = match new_target {
                Value::Object(target) => target,
                // Unreachable from source — a construction always has one — and `object` is what
                // §10.2.2 would fall back to, since it is the constructor being entered.
                _ => object,
            };
            match self.prototype_property(from, heap) {
                Ok(prototype) => Some(Value::Object(heap.new_object(Some(prototype)))),
                Err(error) => {
                    self.raise(error, heap, chunk, current, at)?;
                    return Ok(());
                }
            }
        } else {
            None
        };

        // §10.2.1.2 and §10.2.2 — where the receiver comes from, and it comes from somewhere
        // different in each of the ways in.
        let receiver = match how {
            // §10.2.1.2 `OrdinaryCallBindThis` — the substitution belongs to the **function**
            // rather than to the shape of the call: a non-strict function is given the global
            // object whenever the receiver is `undefined` or `null`, however it was called. So
            // `f()` and `f.call()` and `f.call(null)` all agree, and a method call only differs
            // because its receiver is an object already.
            //
            // Step 6.b.i wraps a *primitive* receiver, so a sloppy function called on a number is
            // handed a **Number object**: `f.call(1)` writes `this.x` onto something that survives
            // the call and can be returned, and `this instanceof Number` is true. A strict one is
            // handed the primitive as it stands, which is what the rows below do.
            // A plain call has no receiver slot at all — `receiver_at` is the callee — so what
            // it passes is `undefined`, and the substitution then applies to that.
            // …and **strict mode keeps the `undefined`**, which is step 3 of the same operation and
            // the reason the flag has to reach the callee's body rather than the call site: a strict
            // function called from sloppy code is still strict, so the caller cannot answer for it.
            Entry::Plain if body.is_strict() => Value::Undefined,
            Entry::Plain => Value::Object(self.realm.global()),
            Entry::Method if body.is_strict() => self.stack[receiver_at],
            Entry::Method => {
                let given = self.stack[receiver_at];
                // Steps 6.a and 6.b in one question, because `ToObject` has no answer for exactly
                // the two values step 6.a is about. Asking "is it nullish" first and converting
                // after would be a branch no input could tell from this one.
                match self.wrapped(given, heap) {
                    Some(receiver) => receiver,
                    None => Value::Object(self.realm.global()),
                }
            }
            // §10.2.2 step 5's `OrdinaryCreateFromConstructor`: `new` *makes* the receiver, out
            // of the constructor's own `prototype` property. A `prototype` that is not an object
            // — a script may assign anything to it — falls back to `Object.prototype`, which is
            // what §10.1.13 says rather than an error.
            Entry::Construct | Entry::Super | Entry::Named if derived => Value::Undefined,
            Entry::Construct | Entry::Super | Entry::Named => match made {
                Some(receiver) => receiver,
                // `made` is filled in for exactly these entries, so this is a shape the types
                // cannot rule out and nothing produces.
                None => Value::Undefined,
            },
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
        let environment =
            heap.new_named_environment(Some(defined_in), body.locals(), Rc::clone(body.bindings()));
        for offset in 0..body.parameters().min(count) {
            let argument = self.stack[callee_at + 1 + offset];
            let index = u32::try_from(offset).unwrap_or(u32::MAX);
            heap.set_variable(environment, index, argument);
        }
        // §15.1 — a rest parameter takes every argument past the named ones, as an ordinary
        // Array. Built here because this is the only place they exist: the body sees slots, and
        // the arguments beyond the last slot are on the stack and nowhere else.
        if let Some(slot) = body.rest() {
            let from = callee_at + 1 + body.parameters().min(count);
            let extra: Vec<Value> =
                self.stack[from.min(self.stack.len())..callee_at + 1 + count].to_vec();
            let array = heap.new_array(self.realm.array_prototype(), 0);
            for (at, value) in extra.iter().enumerate() {
                let index = heap.index_key(u32::try_from(at).unwrap_or(u32::MAX));
                heap.define_own_property(
                    array,
                    index,
                    &crate::heap::PropertyDescriptor::data(*value),
                );
            }
            heap.set_variable(environment, slot, Value::Object(array));
        }
        // §10.2.11 step 22 `CreateMappedArgumentsObject`, and only when the body reads the name.
        // The values are every argument the call was given, and the map joins the first
        // `parameters` of them to the slots filled just above — which is what makes
        // `arguments[0]` and the first parameter one variable rather than two with equal values.
        //
        // **Two conditions and not one**, which is what step 22 actually says: the list must be
        // simple *and the code must be sloppy*. Asking only about the list gave a strict function
        // with plain parameters the mapped object, and the join is observable — `function f(a)
        // { 'use strict'; a = 2; return arguments[0]; }` answered 2 where §10.2.11 says 1, because
        // strict mode is where that link was taken away. It is also what a program uses to reach
        // %ThrowTypeError% at all: the poisoned `callee` is the unmapped object's.
        if let Some(slot) = body.arguments() {
            let values: Vec<Value> = self.stack[callee_at + 1..callee_at + 1 + count].to_vec();
            let prototype = self.realm.object_prototype();
            let arguments = heap.new_arguments(
                prototype,
                &crate::heap::Incoming {
                    environment,
                    values: &values,
                    parameters: body.parameters(),
                    callee: object,
                    thrower: self.realm.thrower(),
                    mapped: body.simple_parameters() && !body.is_strict(),
                    iteration: self
                        .realm
                        .well_known(crate::builtins::well_known_at("iterator"))
                        .map(|symbol| (symbol, self.realm.array_values())),
                },
            );
            heap.set_variable(environment, slot, Value::Object(arguments));
        }
        // §27.7.5.1 `AsyncFunctionStart` — a promise, a context to park into, and then the body
        // runs *now*: unlike a generator this pushes a frame and lets the loop carry on. What the
        // call answers with is decided by whatever stops the body, and both `Await` and `Return`
        // leave the same promise.
        // §27.6 has no context object of its own: the async generator *is* the thing a body parks
        // into and the thing that holds the promises, so making an `Await` context here would give
        // the body two places to be parked and one of them would win.
        let context = match body.is_async() && !body.is_generator() {
            true => self.begin_async(heap),
            false => None,
        };
        // §15.5.4 `EvaluateGeneratorBody` — everything above this line is
        // `FunctionDeclarationInstantiation`, which a generator performs exactly as an ordinary
        // function does, and so does everything below: a generator body is entered as an ordinary
        // call and parks itself at `Instruction::GeneratorStart`, which the compiler puts after the
        // parameters. Diverting here instead put the parameter list inside the parked body, where
        // it ran at the first `next` — see that instruction.
        self.stack.truncate(receiver_at);
        self.frames.push(Frame {
            code: (*current).take(),
            at: *at,
            this_value: self.this_value,
            new_target: self.new_target,
            environment: self.environment,
            stack_base: receiver_at,
            handlers_base: self.handlers.len(),
            // §10.2.2 step 13 — a constructor's call answers with the object it was given
            // unless its body returned an object of its own. A primitive `return` is ignored,
            // which is why `function F() { return 1; }` still constructs an `F`.
            // …and nothing for a derived one, whose answer §10.2.2 step 13 settles in the body:
            // `CompleteDerivedReturn` puts the object on the stack, so by the time `Return` runs
            // there is nothing left for the frame to prefer.
            constructed: match how {
                Entry::Construct | Entry::Super | Entry::Named if !derived => Some(receiver),
                _ => None,
            },
            function: Some(object),
            // An `async` function's context object, or nothing. A **generator's** own object is not
            // known yet and reaches the frame later: the body is entered as an ordinary call and
            // `Vm::start_generator` fills this in when `Instruction::GeneratorStart` runs, which is
            // after the parameters. This used to say a generator's frame was not pushed here at
            // all, which stopped being true when that instruction arrived — and contradicted the
            // comment twenty lines above it.
            generator: context,
        });
        self.environment = environment;
        // §10.2.1.2 step 1 — an arrow's `[[ThisMode]]` is `lexical`, so `OrdinaryCallBindThis`
        // returns without binding anything and the receiver computed above is discarded. What the
        // body sees instead is the `this` the arrow was *written* beside, which it has held since
        // it was made. This is why `f.call(other)` cannot move an arrow's `this` and why passing
        // one as a callback keeps it: neither is a place the arrow was written.
        self.this_value = match lexical {
            Some(captured) => captured.this_value,
            None => receiver,
        };
        self.new_target = new_target;
        *current = Some(body);
        *at = 0;
        Ok(())
    }
    /// Enter what a bound function stands in front of — §10.4.1.1 and §10.4.1.2.
    ///
    /// The chain is flattened rather than followed by recursing. `f.bind(a).bind(b)` is a bound
    /// function whose target is a bound function, and a program may write as many of those as it
    /// likes — so recursing here would put a Rust frame on the stack per `bind`, and DR-0002 does
    /// not allow a script to decide how deep the Rust stack goes.
    ///
    /// Walking outwards, each binding's arguments go in *front* of the ones collected so far,
    /// because the outermost `bind` is the one nearest the call. The receiver is the innermost
    /// binding's, for the same reason in reverse: `f.bind(a).bind(b)` calls `f` with `a`, since
    /// the second `bind` binds the already-bound function and §10.4.1.1 never looks past its own
    /// target.
    #[allow(clippy::too_many_arguments)] // the call's shape, threaded rather than shared
    fn enter_bound(
        &mut self,
        bound: crate::heap::ObjectId,
        how: Entry,
        callee_at: usize,
        receiver_at: usize,
        count: usize,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let given: Vec<Value> = self.stack[callee_at + 1..callee_at + 1 + count].to_vec();
        let mut prefix: Vec<Value> = Vec::new();
        let mut receiver = Value::Undefined;
        let mut target = bound;
        // Bounded because a hand-built heap could point a bound function at itself; no `bind` can
        // make such a cycle, since it binds a function that already exists.
        for _ in 0..MAX_CALL_DEPTH {
            let Some(Callable::Bound(binding)) =
                heap.object(target).and_then(Object::call).cloned()
            else {
                let mut arguments = prefix;
                arguments.extend_from_slice(&given);
                return self.enter_flattened(
                    target,
                    receiver,
                    &arguments,
                    how,
                    receiver_at,
                    heap,
                    chunk,
                    current,
                    at,
                );
            };
            let mut ahead = binding.arguments;
            ahead.extend_from_slice(&prefix);
            prefix = ahead;
            receiver = binding.this_value;
            target = binding.target;
        }
        let thrown = self
            .realm
            .error(heap, NativeError::Range, "too much recursion");
        self.unwind(thrown, chunk, current, at)?;
        Ok(())
    }

    /// Call `target` with the receiver and arguments a chain of bindings settled on.
    #[allow(clippy::too_many_arguments)] // as above
    fn enter_flattened(
        &mut self,
        target: crate::heap::ObjectId,
        receiver: Value,
        arguments: &[Value],
        how: Entry,
        receiver_at: usize,
        heap: &mut Heap,
        chunk: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let Ok(count) = u32::try_from(arguments.len()) else {
            let thrown = self
                .realm
                .error(heap, NativeError::Range, "too many arguments");
            self.unwind(thrown, chunk, current, at)?;
            return Ok(());
        };
        self.stack.truncate(receiver_at);
        // §10.4.1.2 — `new` on a bound function constructs the *target*, and the bound `this` is
        // not consulted at all: `new` makes its own receiver, so there is nothing for it to say.
        let how = match how {
            // A named `new.target` survives the bind, because §10.4.1.2 step 5 passes it through:
            // `Reflect.construct(f.bind(x), [], G)` makes an object from `G.prototype`, and the
            // bound receiver is no more consulted than it is for a plain `new`.
            Entry::Construct | Entry::Named => how,
            _ => {
                self.stack.push(receiver);
                Entry::Method
            }
        };
        self.stack.push(Value::Object(target));
        self.stack.extend_from_slice(arguments);
        self.enter(how, count, heap, chunk, current, at)
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
            Entry::Plain | Entry::Construct | Entry::Super | Entry::Named => Value::Undefined,
        };
        let arguments = self.stack[callee_at + 1..callee_at + 1 + count].to_vec();
        let call = crate::heap::NativeCall {
            function,
            this_value,
            arguments: &arguments,
            // §9.4's `[[NewTarget]]`, and for a `super()` it is the *inherited* one:
            // `class D extends Error {}` runs `Error` with a target of `D`, which is the only thing
            // that knows the object must inherit from `D.prototype`. A native is never an arrow, so
            // there is no captured target that could take precedence over these.
            new_target: match how {
                Entry::Construct => Value::Object(function),
                Entry::Super => self.new_target,
                // §28.1.2 — the third thing the caller named, carried in the receiver slot because
                // a construction has no other use for it. This is what a built-in's
                // `prototype_from` reads, so `Reflect.construct(Array, [], D)` really does make an
                // array that inherits from `D.prototype`.
                Entry::Named => self.stack[receiver_at],
                Entry::Plain | Entry::Method => Value::Undefined,
            },
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
    ) -> crate::value::Completion<crate::heap::ObjectId> {
        self.prototype_for(constructor, self.realm.object_prototype(), heap)
    }

    /// The same, with the intrinsic §10.1.13 falls back to named by the caller.
    ///
    /// `new f()` falls back to `%Object.prototype%` and a generator to `%GeneratorPrototype%`, and
    /// the clause is the same one both times: `GetPrototypeFromConstructor` takes the intrinsic as
    /// an argument. Two callers, one walk, and the fallback is the only thing that differs.
    pub(super) fn prototype_for(
        &mut self,
        constructor: crate::heap::ObjectId,
        fallback: crate::heap::ObjectId,
        heap: &mut Heap,
    ) -> crate::value::Completion<crate::heap::ObjectId> {
        let key = crate::heap::PropertyKey::from_units(
            heap,
            &"prototype".encode_utf16().collect::<Vec<_>>(),
        );
        // §10.1.13's `GetPrototypeFromConstructor` asks `Get(constructor, "prototype")`, which is
        // a full `[[Get]]`: a getter runs, and a proxy `new.target` answers with its trap. Reading
        // the property table instead finds nothing on a proxy, and every instance built through
        // one then inherited from `Object.prototype`.
        let value = self.get_property_key(Value::Object(constructor), key, heap)?;
        Ok(match value {
            Value::Object(prototype) => prototype,
            // A `prototype` that is not an object — a script may assign anything to it — falls
            // back to the intrinsic the caller named, which is what §10.1.13 says rather than an
            // error.
            _ => fallback,
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
    /// The object `new` made, if this call was a construction that made one.
    ///
    /// §10.2.2 step 13: a constructor answers with the object it was given unless its body
    /// returned an object of its own, so the answer has to be kept until the return decides.
    pub(super) constructed: Option<Value>,
    /// The function object this call entered — §10.2.2's *active function object*.
    ///
    /// `None` only for the script, which no function entered. What needs it is `super()`: §10.2.2's
    /// `GetSuperConstructor` reads the running function's `[[Prototype]]`, and it reads it *now*
    /// rather than at the class definition, because `Object.setPrototypeOf(D, Other)` changes what
    /// `super()` reaches. So the answer cannot be compiled in and the frame is the only thing that
    /// still knows which function is running.
    pub(super) function: Option<crate::heap::ObjectId>,
    /// The generator whose body this frame runs, if it runs one — §27.5.1's other direction.
    ///
    /// `None` for every ordinary call, which is nearly all of them. What needs it is `Return`: a
    /// generator's body answers with an iterator result rather than with the value it returned, and
    /// the generator has to be marked completed — and by then the frame is the only thing that
    /// still knows which object this execution belongs to.
    pub(super) generator: Option<crate::heap::ObjectId>,
    /// The `this` to go back to.
    pub(super) this_value: Value,
    /// The `new.target` to go back to.
    ///
    /// Saved beside the `this` because §9.1.1.3 keeps the two in one record: a call decides both,
    /// and a return has to put both back or a constructor that called a plain function would find
    /// its own `new.target` gone when the call came back.
    pub(super) new_target: Value,
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
    /// `super()` — a construction whose `new.target` is inherited rather than being the callee.
    ///
    /// Its own way in rather than `Construct` with a flag, because the two differ in the one thing
    /// that decides what object is made: §10.2.2 step 5 builds the receiver from **new.target's**
    /// `prototype`, and for a plain `new` that is the callee's own while for a `super()` it is the
    /// derived class's. That is what makes `new E()` an `E` however many `extends` clauses it passes
    /// through, and reading the parent's `prototype` instead would quietly produce a `B`.
    Super,
    /// §28.1.2's `Reflect.construct(target, args, newTarget)` — a construction whose `new.target`
    /// is neither the callee nor the running one, but a third thing the caller named.
    ///
    /// Its own way in for the reason `Super` is: what it changes is the one thing that decides what
    /// object is made. It is the only way in the language to build an X whose prototype came from a
    /// Y, and there is nowhere else the third value could come from — a plain `new` has two.
    Named,
}
