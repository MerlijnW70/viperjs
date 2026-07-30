//! §7.1.1 `ToPrimitive`, and the one place Rust re-enters the interpreter to get it.
//!
//! # Why this is not in `value/`
//!
//! Everything else in [`crate::value`] is a function of its arguments: `ToNumber` of a String is
//! arithmetic, `ToString` of a Number is formatting. `ToPrimitive` of an **object** is neither —
//! it calls a method, and the method is JavaScript. So the conversion cannot live where values are
//! described; it lives here, where there is an interpreter to run it.
//!
//! That is why `"" + {}` threw for so long. The addition was never wrong; it simply had no way to
//! ask the object what it was.
//!
//! # Re-entering, and why it is bounded on its own
//!
//! [`Vm::run`]'s loop does not recurse. A call pushes a frame and the same loop goes round again,
//! which is why ten thousand nested JavaScript calls cost ten thousand small structs and no Rust
//! stack at all.
//!
//! A coercion cannot do that, because the answer is needed *in the middle of an instruction*: the
//! `+` has one operand on the stack and cannot finish until the other is a primitive. So
//! [`Vm::call_value`] starts a nested execution, and that is a real Rust call. `valueOf` may
//! convert another object, whose `valueOf` converts another, so the depth is something a program
//! chooses — and it is counted and refused long before the host's stack runs out.

use super::call::Entry;
use super::{Floor, Vm};
use crate::compile::Chunk;
use crate::heap::{Callable, Heap, NativeCall, Object, ObjectId, PropertyKey};
use crate::value::{Abrupt, Completion, ErrorKind, Hint, Value};
use std::rc::Rc;

/// How deeply a coercion may re-enter the interpreter before it is refused.
///
/// Far below `MAX_CALL_DEPTH`, because each one of these is a Rust frame — a whole interpreter
/// loop and the call machinery under it — and the host's stack is not ours to spend. A program
/// that nests conversions sixty deep is doing something nobody wrote by hand; a program that
/// nests *calls* ten thousand deep is ordinary. That is why the two limits are different numbers
/// rather than one.
///
/// It was 200, and that number was never measured against a stack. Each re-entry costs about
/// 7 KiB in a debug build, so two hundred of them wanted more than a mebibyte — which is the
/// smallest thread stack in common use and the size DR-0006 measures the parser against. CI on a
/// platform with larger frames found it by aborting, which is precisely the failure DR-0002 says
/// no `Result` can rescue.
///
/// 64 cost about 450 KiB when it was set, and `a_conversion_at_the_cap_fits_in_the_stack_it_claims_
/// to_need` is the guard that makes it a measurement rather than a hope.
///
/// **The guard has since fired, and what moved was not this number.** Every slice that adds an arm to
/// [`crate::vm::Vm::execute`]'s `match` widens *one* Rust frame — the loop is one function, so its
/// frame is the sum of every arm's locals — and a re-entry pays that frame again per level. By the
/// private-element slice, 64 levels no longer fitted in a mebibyte at all: the guard overflowed, which
/// is precisely the abort DR-0002 says no `Result` can rescue.
///
/// The lever is the frame and not the cap. Moving three arms out of line brought 64 back inside a
/// mebibyte on Windows, at 12 to 16 KiB a level — a margin nearer 1.3× than the 2× above, recorded as
/// thin, and which is why the arms in [`crate::vm`] are `#[inline(never)]`.
///
/// **Thin was not enough.** macOS CI failed on the very next push: exit 101, output truncated
/// mid-run, no panic — the overflow signature, on a platform whose frames are larger and which
/// cannot be measured from here. So the number came down as well. 32 costs about 400 KiB by the same
/// measurement and leaves better than the 2× this comment has claimed twice; the guard runs at the
/// cap, so a future slice that fattens the frame again will fail it locally rather than in CI.
///
/// What the cap costs a program is nothing anyone will meet: it is how deeply `valueOf` may call
/// something whose `valueOf` calls something else, and thirty-two of those is already a program
/// nobody wrote by hand. The fattest arms left are `MakeClass`, `MakeFunction` and `CallSpread`, and
/// moving them is the lever to reach for before this number goes lower still.
const MAX_REENTRY_DEPTH: usize = 32;

impl Vm {
    /// §7.1.1 `ToPrimitive` — a value with no properties, out of one that may have them.
    ///
    /// A primitive is already one. An object is *asked*, and §7.1.1.1's `OrdinaryToPrimitive` says
    /// what asking means: two methods in an order the hint decides, and the first to answer with a
    /// primitive wins.
    ///
    /// §7.1.1 step 2 looks for `@@toPrimitive` first, which is how a `Date` says it prefers a
    /// string and how a class overrides the whole thing. There are no Symbols yet, so every object
    /// takes the ordinary path; when they arrive this gains a step in front rather than changing.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn to_primitive(
        &mut self,
        value: Value,
        hint: Hint,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = value else {
            return Ok(value);
        };
        // §7.1.1.1 — `valueOf` first for a Number hint, `toString` first for a String one. The
        // order is the whole of what the hint does: it is why `({}) + ""` is `"[object Object]"`
        // and why a Date in the same position is its own text.
        let order: [&str; 2] = match hint {
            Hint::Number => ["valueOf", "toString"],
            Hint::String => ["toString", "valueOf"],
        };
        for name in order {
            let Some(method) = self.method(object, name, heap)? else {
                continue;
            };
            let answer = self.call_value(method, value, &[], heap)?;
            // §7.1.1.1 step 3.b.iii — an object is *not* an answer, and the other method is tried.
            // That is why `({valueOf: function () { return {} }}) + ""` still reaches `toString`
            // rather than giving up at the first attempt.
            if !matches!(answer, Value::Object(_)) {
                return Ok(answer);
            }
        }
        // §7.1.1.1 step 4. `Object.create(null)` reaches this, and so does an object whose two
        // methods both answer with objects — the only ways in the language to have no primitive.
        Err(Abrupt::type_error(
            "cannot convert an object to a primitive value",
        ))
    }

    /// §7.1.3 `ToNumber` of anything, including an object.
    ///
    /// The value layer answers for every primitive; an object is made primitive first and asked
    /// again. `ToNumber` uses the **Number** hint, which is why `({valueOf: () => 1}) * 2` is 2.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn to_number(&mut self, value: Value, heap: &mut Heap) -> Completion<f64> {
        let primitive = self.to_primitive(value, Hint::Number, heap)?;
        primitive.to_number(heap)
    }

    /// §7.1.17 `ToString` of anything, including an object.
    ///
    /// The **String** hint, so `toString` is tried before `valueOf` — which is what makes
    /// `String({})` say `"[object Object]"` rather than reaching for a number that is not there.
    #[allow(clippy::wrong_self_convention)] // same: `ToString` of an object calls a method
    pub(crate) fn to_string(
        &mut self,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<crate::heap::StringId> {
        let primitive = self.to_primitive(value, Hint::String, heap)?;
        primitive.to_string(heap)
    }

    /// §7.1.19 `ToPropertyKey` of anything, including an object.
    #[allow(clippy::wrong_self_convention)] // same
    pub(crate) fn to_property_key(
        &mut self,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<PropertyKey> {
        // §7.1.19 step 3 — a Symbol *is* a key and is taken as one. Checked after `ToPrimitive`,
        // which is what lets an object with a `Symbol.toPrimitive` answer a Symbol and have it
        // used as the key rather than converted — and it has to be before `ToString`, which
        // throws for a Symbol.
        let primitive = self.to_primitive(value, crate::value::Hint::String, heap)?;
        if let Value::Symbol(symbol) = primitive {
            return Ok(PropertyKey::from_symbol(symbol));
        }
        let id = primitive.to_string(heap)?;
        Ok(PropertyKey::from_string(heap, id))
    }

    /// A callable property of `object`, or `None` when it is absent or is not callable.
    ///
    /// §7.1.1.1 step 3.b.i asks `IsCallable` and *skips* what is not, rather than throwing — so an
    /// object whose `valueOf` is a number still converts through `toString`.
    fn method(
        &mut self,
        object: ObjectId,
        name: &str,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
        let found = self.get_property_key(Value::Object(object), key, heap)?;
        let Value::Object(function) = found else {
            return Ok(None);
        };
        Ok(
            match heap.object(function).and_then(Object::call).is_some() {
                true => Some(found),
                false => None,
            },
        )
    }

    /// §7.3.14 `Call` — call `callee` from Rust and wait for its answer.
    ///
    /// A built-in answers without an interpreter at all: it is Rust, and calling it is calling it.
    /// A JavaScript function needs the loop, so this starts a nested execution and runs until the
    /// frame it pushed has come back. Everything else about that execution is ordinary — the same
    /// `enter`, the same instructions, the same frames.
    pub(crate) fn call_value(
        &mut self,
        callee: Value,
        this_value: Value,
        arguments: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        self.reach(Entry::Method, callee, this_value, arguments, heap)
    }

    /// `Construct(callee, arguments)` — §7.3.14, with `newTarget` the callee itself.
    ///
    /// The same machinery with a different [`Entry`], which is the whole difference: a construction
    /// takes its receiver from `new.target`'s `prototype` rather than from the stack, and refuses a
    /// callee that has no `[[Construct]]`. §27.2.1.5 is the first thing in the engine that needs to
    /// construct from Rust — every other construction is a `new` that the compiler saw.
    pub(crate) fn construct_value(
        &mut self,
        callee: Value,
        arguments: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        self.reach(Entry::Construct, callee, Value::Undefined, arguments, heap)
    }

    /// Call or construct `callee` from Rust, and run until it comes back.
    fn reach(
        &mut self,
        how: Entry,
        callee: Value,
        this_value: Value,
        arguments: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(function) = callee else {
            return Err(Abrupt::type_error("what was called is not a function"));
        };
        let Some(callable) = heap.object(function).and_then(Object::call).cloned() else {
            return Err(Abrupt::type_error("what was called is not a function"));
        };
        if let Callable::Native { native, .. } = callable {
            let call = NativeCall {
                function,
                this_value,
                arguments,
                // `Vm::call_value` is how a built-in calls a function — a callback, a `valueOf`,
                // a `toString`. None of those is a construction.
                // §7.1.1 calls `valueOf` and `toString`; neither is a construction.
                new_target: match how {
                    Entry::Construct => callee,
                    _ => Value::Undefined,
                },
            };
            return native(self, heap, &call);
        }
        if self.reentries >= MAX_REENTRY_DEPTH {
            // A program chose this depth, so it is a run-time error like any other recursion that
            // went too far — not a [`super::Fault`], which is about a chunk that does not parse as
            // instructions.
            return Err(Abrupt::Raised(
                ErrorKind::Range,
                "too much recursion in a conversion",
            ));
        }

        // The caller's stack must come back untouched, so the call is built on top of it exactly
        // as a compiled one would be: the receiver, the callee, then the arguments.
        let base = self.stack.len();
        self.stack.push(this_value);
        self.stack.push(callee);
        self.stack.extend_from_slice(arguments);
        let count = u32::try_from(arguments.len()).unwrap_or(u32::MAX);
        let answer = self.nested(how, count, heap);
        // Whatever happened, the caller's stack is what it was. A throw leaves half-built operands
        // behind, and this is where they go.
        self.stack.truncate(base);
        answer
    }

    /// Enter a compiled callee and run until it returns, with the floors and the count set.
    ///
    /// The environment and the `this` are saved and put back by hand. A `Return` restores them
    /// from the frame it pops, so the ordinary path needs no help — but a **throw** that nothing
    /// caught does not pop frames one at a time, and would leave the caller running in the
    /// callee's scope. The next variable it read would be a slot that is not there.
    fn nested(&mut self, how: Entry, count: u32, heap: &mut Heap) -> Completion<Value> {
        let floor = std::mem::replace(
            &mut self.floor,
            Floor {
                handlers: self.handlers.len(),
                frames: self.frames.len(),
            },
        );
        let environment = self.environment;
        let this_value = self.this_value;
        self.reentries += 1;
        let answer = self.nested_body(how, count, heap);
        self.reentries -= 1;
        self.environment = environment;
        self.this_value = this_value;
        self.floor = floor;
        answer
    }

    /// The nested execution itself, with the bookkeeping already done around it.
    fn nested_body(&mut self, how: Entry, count: u32, heap: &mut Heap) -> Completion<Value> {
        // A chunk with no instructions, standing in for "the code that started this" — which is
        // Rust. Nothing executes from it: `enter` records it as the return address, and the loop
        // stops the moment the callee returns, before an instruction could be read here.
        let root = Chunk::from_parts(Vec::new(), Vec::new());
        let mut current: Option<Rc<Chunk>> = None;
        let mut at = 0_usize;

        // §10.2.1.2's receiver is decided by the caller, and here the caller is Rust — so it is
        // *passed* rather than substituted. `Entry::Method` is the shape that takes one from the
        // stack, which is where `call_value` put it.
        self.enter(how, count, heap, &root, &mut current, &mut at)
            .map_err(fault)?;
        // `enter` throws rather than faulting when it refuses — the callee is not callable, or
        // the call is too deep — and a throw with nothing above the floor to catch it lands in
        // `escaped`. So there is no separate "did a frame get pushed" question to ask: if none
        // did, the loop below reads nothing and the check after it says what happened.
        self.execute(&root, heap, &mut current, &mut at)
            .map_err(fault)?;
        if let Some(thrown) = self.escaped.take() {
            // This is what `Abrupt::Thrown` exists for. The value is the one the program raised
            // and it travels back through Rust unchanged; rebuilding an error from its parts
            // would hand the `catch` a different object than the `throw` created.
            return Err(Abrupt::Thrown(thrown));
        }
        // A return leaves exactly one value where the call began. Nothing means the callee fell
        // off the end of its own chunk, which no compiled body does.
        self.stack
            .last()
            .copied()
            .ok_or(Abrupt::type_error("a call answered with nothing"))
    }
}

/// A malformed chunk met inside a conversion.
///
/// A [`super::Fault`] is not a thrown value and must not become one — it says the *compiler* is
/// wrong, not the program. Nothing that reaches here can produce one: the callee was compiled by
/// this engine. It is mapped rather than propagated because `Completion` is what a conversion
/// answers with, and a fault arriving as a TypeError is still louder than a fault ignored.
fn fault(fault: super::Fault) -> Abrupt {
    match fault {
        super::Fault::StackUnderflow => Abrupt::type_error("a conversion ran out of operands"),
        _ => Abrupt::type_error("the code of a conversion did not make sense"),
    }
}

impl Vm {
    /// §13.15.3 and §7.2.13 — a binary operator, with its operands made primitive first.
    ///
    /// Which operands are converted is not the same question for every operator, and getting it
    /// wrong is silent:
    ///
    /// - **Strict** equality converts nothing. It compares types, and a conversion would erase the
    ///   very difference it exists to report.
    /// - **Loose** equality converts an object only when the other side is a String, a Number or a
    ///   Boolean. §7.2.15's list is exact and `null` and `undefined` are not on it, so `{} == null`
    ///   is `false` without asking the object anything — which is why `x == null` stays safe even
    ///   when `x` has a `valueOf` that throws. Two objects are compared by identity, so
    ///   `({}) == ({})` is `false`; converting both would make it `true`.
    /// - Everything else converts both, left first, because §13.15.3 evaluates them in that order
    ///   and a `valueOf` with a side effect can tell.
    pub(crate) fn binary(
        &mut self,
        operator: crate::ast::BinaryOperator,
        left: Value,
        right: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        use crate::ast::BinaryOperator as Op;
        let (left, right) = match operator {
            Op::StrictEqual | Op::StrictNotEqual => (left, right),
            Op::Equal | Op::NotEqual => {
                let convert = |one: Value, other: Value| {
                    matches!(one, Value::Object(_))
                        && matches!(
                            other,
                            Value::String(_)
                                | Value::Number(_)
                                | Value::Boolean(_)
                                | Value::Symbol(_)
                        )
                };
                match (convert(left, right), convert(right, left)) {
                    (true, _) => (self.to_primitive(left, Hint::Number, heap)?, right),
                    (_, true) => (left, self.to_primitive(right, Hint::Number, heap)?),
                    _ => (left, right),
                }
            }
            _ => (
                self.to_primitive(left, Hint::Number, heap)?,
                self.to_primitive(right, Hint::Number, heap)?,
            ),
        };
        crate::value::apply_binary(operator, left, right, heap)
    }

    /// §13.5 — a unary operator, with its operand made primitive when the operator reads one.
    ///
    /// `typeof` asks what a value *is* and `!` asks whether it is truthy; neither converts, and
    /// neither can throw. The three that produce a number do, which is why `-{}` is `NaN` and
    /// `-({valueOf: function () { return 2 }})` is `-2`.
    pub(crate) fn unary(
        &mut self,
        operator: crate::ast::UnaryOperator,
        operand: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        use crate::ast::UnaryOperator as Op;
        let operand = match operator {
            Op::Plus | Op::Minus | Op::BitwiseNot => {
                self.to_primitive(operand, Hint::Number, heap)?
            }
            _ => operand,
        };
        super::apply_unary(operator, operand, heap)
    }
}
