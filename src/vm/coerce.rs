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
/// **What the cap costs a program is not nothing, and this comment claimed it was.** It read: "how
/// deeply `valueOf` may call something whose `valueOf` calls something else, and thirty-two of those
/// is already a program nobody wrote by hand." The counter is not about conversions. It rises for
/// **every** native that calls back into JavaScript — `map`, `forEach`, `sort`, `reduce`, a
/// `JSON.stringify` replacer, a `then` handler — so a recursive walk written the ordinary way,
///
/// ```js
/// function walk(node) { return node.children.map(walk); }
/// ```
///
/// stops at depth 33, where pure recursion reaches 5,000. Measured 2026-08-06 against real
/// packages: `ajv` hits it while compiling a schema, and the error it raises still says
/// *conversion*, which is the wrong word for most of what arrives here.
///
/// **And the margin is not what the paragraph above says.** Measured 2026-08-06 by
/// `lab`'s `reentry-cost`, which bisects the cliff with one child process per depth on a mebibyte
/// in a debug build, and again on 2026-08-07 after eight arms were moved out of the loop:
///
/// | shape | deepest, before | after 9 arms | after 12 | bytes per level now |
/// | --- | --- | --- | --- | --- |
/// | `valueOf` | 43 | 52 | 55 | 19.1 KiB |
/// | `map` | 38 | 45 | 48 | 21.8 KiB |
/// | `sort` | **35** | 41 | **43** | 24.4 KiB |
///
/// The margin at 32 was **1.09×** where this comment claimed "better than 2×", and is **1.34×**
/// now. The old number was never wrong for the shape it was measured against — a `toString` chain,
/// the cheapest of the three — and a cap has to hold for the dearest. A native's own frame rides
/// on top of the interpreter's, and `sort` carries a `Vec` of elements across the comparator call.
///
/// **So this number still cannot go up.** 1.34× is barely past the 1.3× that was recorded as thin
/// on Windows the last time, and macOS aborted on the next push anyway; the platform whose frames
/// are largest cannot be measured from here. What would justify a move is the cliff at 64 for
/// `sort`, which wants about 16 KiB a level.
///
/// The lever is measured now rather than guessed at. [`crate::vm::Vm::execute`] is one function
/// whose frame is the sum of every arm's locals — **18,568 bytes**, read from its own prologue,
/// which calls `__chkstk` because it is past a page. Moving twelve arms out of line took it to
/// **13,728**, and the cliff moved with it. `MakeClass` and `MakeFunction` were the documented
/// suspects and are *not* it: both hold an `Rc<Chunk>`, which is a pointer.
///
/// **The rate is falling and the next reader should know it.** Nine arms bought six levels of
/// `sort`; three more bought two. The arms left on the attribution list are 14 slots each against
/// the 20 the first ones held, so arm-by-arm reaches perhaps 50 and not 64. Past that the change
/// is structural — splitting the loop so the cold half is its own function, or boxing the widest
/// locals — and that is a design question rather than more of this.
///
/// Re-measure with `cargo rustc -p viperjs --lib -- --emit asm` and read `.seh_stackalloc` under
/// `Vm::execute` — seconds, against minutes for a bisection, so an arm can be moved and judged one
/// at a time. `lab`'s `reentry-cost` is what turns a frame figure back into a depth.
///
/// **Still 13,728 on 2026-08-08**, re-measured after DR-0025 gave a call a realm to carry. Worth
/// the minute it cost: adding a field to a `Frame` looks like it should move this and cannot, since
/// a frame is a record in a `Vec` and not a Rust stack frame. What *would* move it is a local live
/// across the `match`, and the realm switch is in `enter` rather than here. **Read the release
/// figure as a different measurement, not a better one** — the same prologue is 3,304 bytes
/// optimised, and the cap is set against a debug build because that is the one whose stack a test
/// can exhaust.
const MAX_REENTRY_DEPTH: usize = 32;

impl Vm {
    /// §7.1.1 `ToPrimitive` — a value with no properties, out of one that may have them.
    ///
    /// A primitive is already one. An object is *asked*, and §7.1.1.1's `OrdinaryToPrimitive` says
    /// what asking means: two methods in an order the hint decides, and the first to answer with a
    /// primitive wins.
    ///
    /// §7.1.1 step 1.a looks for `@@toPrimitive` **first**, and it is the only step that can
    /// answer with something neither `valueOf` nor `toString` would: the method is handed the hint
    /// as a string and decides for itself. §21.4.4.45's is why `date + 1` concatenates where
    /// `date - 1` subtracts, and a class's own is how the whole ordinary walk is overridden.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn to_primitive(
        &mut self,
        value: Value,
        hint: Hint,
        heap: &mut Heap,
    ) -> Completion<Value> {
        if !matches!(value, Value::Object(_)) {
            return Ok(value);
        }
        if let Some(exotic) = self.exotic_to_primitive(value, heap)? {
            // Steps 1.b.i to 1.b.iii — the hint reaches the method as a **string**, which is the
            // only place in the language the three preferences are named rather than implied.
            let named = crate::builtins::text(heap, hint.spelling());
            let answer = self.call_value(exotic, value, &[named], heap)?;
            // Step 1.b.vi — an Object is **not** an answer here, and unlike §7.1.1.1's walk there
            // is nothing else to try: the object said how it wished to be converted and did not
            // convert. A fallback to `valueOf` would be a second chance the clause does not give.
            if matches!(answer, Value::Object(_)) {
                return Err(Abrupt::type_error(
                    "Symbol.toPrimitive did not answer with a primitive value",
                ));
            }
            return Ok(answer);
        }
        self.ordinary_to_primitive(value, hint, heap)
    }

    /// §7.1.1.1 `OrdinaryToPrimitive(O, hint)` — the two-method walk, without step 1's lookup.
    ///
    /// Apart from [`Vm::to_primitive`] because §21.4.4.45 needs exactly this and not the whole of
    /// §7.1.1: `Date.prototype[@@toPrimitive]` finishes by running the ordinary walk, and going
    /// back through the outer operation would find *itself* through the lookup and recur until the
    /// call stack ran out. The clause names the two operations separately for that reason, and
    /// they are two functions here for the same one.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn ordinary_to_primitive(
        &mut self,
        value: Value,
        hint: Hint,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Value::Object(object) = value else {
            return Ok(value);
        };
        // `valueOf` first for a Number hint, `toString` first for a String one. The order is the
        // whole of what the hint does: it is why `({}) + ""` is `"[object Object]"` and why a Date
        // in the same position is its own text.
        //
        // §7.1.1 step 1.c — an absent preference becomes **number** before this is reached, which
        // is what makes `Hint::Default` indistinguishable from `Hint::Number` for every object
        // that has no `@@toPrimitive`. That is most of them, and it is why this is one arm.
        let order: [&str; 2] = match hint {
            Hint::Number | Hint::Default => ["valueOf", "toString"],
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

    /// §7.1.13 `ToBigInt` of anything, including an object.
    ///
    /// The counterpart of [`Vm::to_number`], and deliberately not symmetrical with it: a **Number
    /// is refused**, integer or not. §7.1.13 is the conversion that happens without being asked
    /// for, and the whole point of §6.1.6.2's type is that crossing into it is something a program
    /// says explicitly with `BigInt(…)`.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn to_bigint(
        &mut self,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<crate::bigint::BigInt> {
        // Step 1 — `ToPrimitive` with the **Number** hint, so an object's `valueOf` is tried before
        // its `toString` exactly as it is for a Number.
        let primitive = self.to_primitive(value, Hint::Number, heap)?;
        let converted = crate::builtins::bigint::to_bigint(primitive, heap)?;
        crate::builtins::bigint::this_bigint(converted, heap)
    }

    /// §7.1.6 `ToNumeric` — the value's own numeric type, whichever of the two it turns out to be.
    ///
    /// Not [`Vm::to_numeric`], which is §10.4.5.16's: that one is told which type to produce by the
    /// TypedArray being written into, and refuses a value of the other. This one **asks the value**,
    /// and so is the operation an arithmetic operator starts with — `a * b` runs it on both sides
    /// and then refuses a pair that is one of each, which is not a refusal either conversion made.
    ///
    /// The one caller today is §21.1.1.1's `Number(x)`, which is the only place in the language a
    /// BigInt crosses to a Number without a second word from the program.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    pub(crate) fn to_numeric_value(
        &mut self,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<crate::heap::Numeric> {
        // Step 1 — the **Number** hint, so an object's `valueOf` is tried before its `toString`.
        let primitive = self.to_primitive(value, Hint::Number, heap)?;
        // Step 2 — a BigInt is already numeric and is answered with unchanged. A *wrapper* holding
        // one is not: `ToPrimitive` above has already unwrapped it, so what arrives here is the
        // primitive or something that was never a BigInt at all.
        if let Value::BigInt(id) = primitive {
            let Some(big) = heap.bigint(id) else {
                // A `BigIntId` naming nothing is a heap that has lost a value, not a program that
                // did anything — and `Fault` is the signal for that, so this is the honest error.
                return Err(Abrupt::type_error("a BigInt that the heap does not hold"));
            };
            return Ok(crate::heap::Numeric::BigInt(big.clone()));
        }
        // Step 3.
        Ok(crate::heap::Numeric::Number(primitive.to_number(heap)?))
    }

    /// §10.4.5.16 step 1 — the conversion a write to a TypedArray of this content type performs.
    ///
    /// The one place the two numeric types are chosen between by the *destination* rather than by
    /// the value: a `BigInt64Array` runs §7.1.13 and every other kind runs §7.1.4, so
    /// `bigOnes[0] = 1` and `bytes[0] = 1n` are both TypeErrors and neither is a truncation. Which
    /// conversion runs is observable beyond the refusal, because both can call a `valueOf`.
    #[allow(clippy::wrong_self_convention)] // same
    pub(crate) fn to_numeric(
        &mut self,
        holds_big: bool,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<crate::heap::Numeric> {
        match holds_big {
            true => Ok(crate::heap::Numeric::BigInt(self.to_bigint(value, heap)?)),
            false => Ok(crate::heap::Numeric::Number(self.to_number(value, heap)?)),
        }
    }

    /// The same, for a TypedArray that has to be asked which content type it is.
    ///
    /// Anything that is not a TypedArray answers as a Number one, which is not a guess: every
    /// caller has already established that it holds elements, and `holds_big` is a question only a
    /// kind can answer.
    #[allow(clippy::wrong_self_convention)] // same
    pub(crate) fn to_numeric_of(
        &mut self,
        object: crate::heap::ObjectId,
        value: Value,
        heap: &mut Heap,
    ) -> Completion<crate::heap::Numeric> {
        let holds_big = heap
            .typed_view(object)
            .and_then(|view| view.element)
            .is_some_and(crate::heap::Element::holds_big);
        self.to_numeric(holds_big, value, heap)
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

    /// The array index a Number *is*, if it is one — §6.1.7 asked of a value rather than of text.
    ///
    /// Must agree exactly with `heap::index_of`, which asks the same question of the spelling: a
    /// Number that answers `Some(n)` here has to be one whose `ToString` answers `Some(n)` there,
    /// or `a[0]` and `a["0"]` would be two keys.
    ///
    /// **`-0` is index zero, and the first version of this said otherwise.** `"-0"` is not an array
    /// index — §6.1.7 asks `ToString(ToUint32(P))` to be `P` and it is not — but the *Number* `-0`
    /// never arrives as that text: §7.1.19 spells it first, and `ToString(-0)` is `"0"`. So the two
    /// zeroes are one key here and two keys there, and a sign test written from the text rule sent
    /// `a[-0]` down the slow path to be told the same thing. Caught by
    /// `the_numeric_index_test_agrees_with_the_one_asked_of_the_spelling` — no program could see
    /// it, because the slow path was right.
    fn array_index_of(number: f64) -> Option<u32> {
        // `fract` settles integrality, both infinities and NaN at once: theirs is NaN, which is
        // equal to nothing including zero. `< 0.0` and not `!(>= 0.0)`, because NaN is already
        // gone by here and the remaining question really is "is it negative" — of which `-0` is
        // not one.
        if number.fract() != 0.0 || number < 0.0 {
            return None;
        }
        // `2^32 - 2` is the last index, so the cast cannot lose anything after the comparison —
        // and `-0.0 as u32` is `0`, which is the answer the spelling gives.
        (number <= f64::from(crate::heap::MAX_INDEX)).then_some(number as u32)
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
        // DR-0026's fast path, and the whole of the saving: a non-negative integral Number below
        // 2^32 - 1 *is* an array index, so the key is a cast. Asked before `ToPrimitive` because a
        // Number is already primitive — the call would answer it unchanged — and asked on the way
        // in because `a[i]` is where this is paid, once per element of every loop that walks an
        // array. What it skips is spelling the number, encoding it to UTF-16 and interning that.
        if let Value::Number(number) = value
            && let Some(index) = Self::array_index_of(number)
        {
            return Ok(PropertyKey::from_index(index));
        }
        let primitive = self.to_primitive(value, crate::value::Hint::String, heap)?;
        if let Value::Symbol(symbol) = primitive {
            return Ok(PropertyKey::from_symbol(symbol));
        }
        // Interned from the text rather than from a String made to hold it. Going through
        // `ToString` first would allocate an arena slot, hand it to the intern table, and have the
        // table answer with the copy it already had — leaving the new one dead on the first access
        // and every one after it. `a[i] = v` is this path, so that was a slot per element written.
        match primitive.spelled(heap) {
            Some(text) => Ok(PropertyKey::from_units(
                heap,
                &text.encode_utf16().collect::<Vec<_>>(),
            )),
            // A String, and only a String: `ToPrimitive` has answered so this is not an Object,
            // and a Symbol was taken above. Its units are already on the heap, so `ToString` hands
            // the handle straight back without allocating anything either.
            None => {
                let id = primitive.to_string(heap)?;
                Ok(PropertyKey::from_string(heap, id))
            }
        }
    }

    /// A callable property of `object`, or `None` when it is absent or is not callable.
    ///
    /// §7.1.1.1 step 3.b.i asks `IsCallable` and *skips* what is not, rather than throwing — so an
    /// object whose `valueOf` is a number still converts through `toString`.
    /// §7.1.1 step 1.a — `GetMethod(input, @@toPrimitive)`.
    ///
    /// `GetMethod` and not `Get`: `undefined` and `null` both mean "there is none" and everything
    /// else that is not callable is a **TypeError** rather than something to walk past. So
    /// `Date.prototype[Symbol.toPrimitive] = 1` breaks every coercion of every Date, which is what
    /// the clause says and is worth being able to see happen.
    fn exotic_to_primitive(&mut self, value: Value, heap: &mut Heap) -> Completion<Option<Value>> {
        let Some(symbol) = heap.well_known(crate::builtins::well_known_at("toPrimitive")) else {
            return Ok(None);
        };
        let found = self.get_property_key(value, PropertyKey::from_symbol(symbol), heap)?;
        if matches!(found, Value::Undefined | Value::Null) {
            return Ok(None);
        }
        if !heap.is_callable(found) {
            return Err(Abrupt::type_error("Symbol.toPrimitive is not a function"));
        }
        Ok(Some(found))
    }

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

    /// `Construct(callee, arguments, newTarget)` — §7.3.14 with the third argument given.
    ///
    /// `new X()` always passes `X` as its own `new.target`, so until §28.1.2 nothing in the engine
    /// needed to pass a different one. `Reflect.construct(X, [], Y)` is the only way to say it, and
    /// what it decides is the prototype of the object made: an X built through Y's `prototype`.
    pub(crate) fn construct_with_target(
        &mut self,
        callee: Value,
        new_target: Value,
        arguments: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        // The receiver slot carries it, because a construction makes its own receiver from
        // `new.target` and so leaves that slot free — see [`Entry::Named`].
        self.reach(Entry::Named, callee, new_target, arguments, heap)
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
                    // §28.1.2 — a built-in constructed through `Reflect.construct` is told the
                    // `new.target` the caller named, which is what its `prototype_from` reads.
                    Entry::Named => this_value,
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

    /// §19.2.1.1 `PerformEval` — run a whole compiled *script* here and answer its completion value.
    ///
    /// Not [`Vm::run`], which is the embedder's door and clears the stack, the frames and the
    /// handlers before it starts. This is a script running in the middle of an expression, so
    /// everything the caller had must still be there afterwards — the same promise
    /// [`Vm::call_value`] makes, kept by the same bookkeeping.
    ///
    /// The environment is the caller's to choose and is passed in, because that is the *whole*
    /// difference between §19.2.1.1's two modes: an indirect eval is given a fresh one over the
    /// global scope, and a direct one would be given a child of the running scope. Nothing here
    /// decides which.
    ///
    /// The completion value comes from `self.completion` rather than the stack. A script is not a
    /// call and leaves nothing behind: §14.2.2's value is whatever its last value-producing
    /// statement produced, which the loop records as it goes and which is why `eval("var x")` is
    /// `undefined` while `eval("1; ;")` is 1.
    pub(crate) fn run_script(
        &mut self,
        chunk: &Chunk,
        environment: crate::heap::EnvironmentId,
        heap: &mut Heap,
    ) -> Completion<Value> {
        if self.reentries >= MAX_REENTRY_DEPTH {
            return Err(Abrupt::Raised(
                ErrorKind::Range,
                "too much recursion in a conversion",
            ));
        }
        let floor = std::mem::replace(
            &mut self.floor,
            Floor {
                handlers: self.handlers.len(),
                frames: self.frames.len(),
            },
        );
        let saved_environment = std::mem::replace(&mut self.environment, environment);
        // Saved and **not replaced**, which is the one asymmetry in this pair. §19.2.1.1 step 12
        // hands a sloppy direct eval the *caller's* variable environment, so the eval's `var`s
        // belong where the caller's do and the field has to go on saying so while its chunk runs.
        // The other two modes never read it — `Vm::eval_vars` answers `Global` whenever the frame
        // count is back at the floor, which this call has just raised — so leaving it alone is the
        // right answer for all three rather than a direct-eval special case.
        //
        // Restored by hand for the reason `Vm::nested` gives about the field above: an uncaught
        // throw does not pop frames one at a time, so without this the caller carries on with a
        // callee's environment as its variable scope.
        let saved_var_environment = self.var_environment;
        let saved_completion = std::mem::replace(&mut self.completion, Value::Undefined);
        let base = self.stack.len();
        self.reentries += 1;

        let mut current: Option<Rc<Chunk>> = None;
        let mut at = 0_usize;
        let answer = self
            .execute(chunk, heap, &mut current, &mut at)
            .map_err(fault)
            .and_then(|()| match self.escaped.take() {
                // A throw the eval'd code did not catch is the caller's to see, and unchanged:
                // rebuilding it would hand the `catch` a different object than the `throw` made.
                Some(thrown) => Err(Abrupt::Thrown(thrown)),
                None => Ok(self.completion),
            });

        self.reentries -= 1;
        // Whatever happened. A throw leaves half-built operands and the eval's own scope behind,
        // and this is the one place that can put the caller back exactly as it was.
        self.stack.truncate(base);
        self.completion = saved_completion;
        self.environment = saved_environment;
        self.var_environment = saved_var_environment;
        self.floor = floor;
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
        // And the variable one with it, for exactly the reason this function's own documentation
        // gives: the throw that skips the frame-by-frame restore skips both.
        let var_environment = self.var_environment;
        let this_value = self.this_value;
        self.reentries += 1;
        let answer = self.nested_body(how, count, heap);
        self.reentries -= 1;
        self.environment = environment;
        self.var_environment = var_environment;
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
                // §7.2.15 steps 10 and 11 — `ToPrimitive(y)` with **no** preferred type, because
                // either kind of primitive is something a loose comparison can go on to use.
                match (convert(left, right), convert(right, left)) {
                    (true, _) => (self.to_primitive(left, Hint::Default, heap)?, right),
                    (_, true) => (left, self.to_primitive(right, Hint::Default, heap)?),
                    _ => (left, right),
                }
            }
            // §13.15.3 step 1.a — `+` is the one arithmetic operator that asks for a primitive
            // without saying which, because a String is an answer it can use: it decides between
            // concatenation and addition *after* seeing what it got. Every other operator here
            // goes through `ToNumeric`, which is §7.1.1 with the number preference.
            Op::Add => (
                self.to_primitive(left, Hint::Default, heap)?,
                self.to_primitive(right, Hint::Default, heap)?,
            ),
            // §13.15.3 steps 3 and 4 — `ToNumeric(lval)` **whole**, and only then `ToNumeric(rval)`.
            // Each is §7.1.1 with a number hint *followed by* the numeric conversion, so a left
            // operand whose `valueOf` answers a Symbol is refused before the right operand is
            // touched at all. Converting both to primitives first and refusing afterwards is the
            // shape of step 1, which belongs to `+` alone — and it ran the right's `valueOf` for
            // eleven operators that must not.
            //
            // Named one by one rather than left to a catch-all, because the arm below it is the
            // *relational* operators and they must not come here: §7.2.13 compares two Strings
            // lexicographically, so `"a" < "b"` is a comparison and not two NaNs.
            Op::Exponent
            | Op::Multiply
            | Op::Divide
            | Op::Remainder
            | Op::Subtract
            | Op::ShiftLeft
            | Op::ShiftRight
            | Op::ShiftRightUnsigned
            | Op::BitwiseAnd
            | Op::BitwiseOr
            | Op::BitwiseXor => (
                self.to_numeric_operand(left, heap)?,
                self.to_numeric_operand(right, heap)?,
            ),
            _ => (
                self.to_primitive(left, Hint::Number, heap)?,
                self.to_primitive(right, Hint::Number, heap)?,
            ),
        };
        crate::value::apply_binary(operator, left, right, heap)
    }

    /// §7.1.3 `ToNumeric` of one operand, answered as a `Value`.
    ///
    /// The conversion §13.15.3 steps 3 and 4 name, done to one operand so that the *order* of the
    /// two is the clause's. A BigInt passes through unchanged — it is already a numeric value, and
    /// making a new one would allocate a copy per arithmetic operation; everything else becomes a
    /// Number by the same `to_number` the pair below would have used, which is what refuses a
    /// Symbol. So this adds no rule: it moves an existing one to where the clause puts it.
    #[allow(clippy::wrong_self_convention)] // a conversion runs code, so it needs the machine
    fn to_numeric_operand(&mut self, value: Value, heap: &mut Heap) -> Completion<Value> {
        let primitive = self.to_primitive(value, Hint::Number, heap)?;
        match primitive {
            Value::BigInt(_) => Ok(primitive),
            other => Ok(Value::Number(other.to_number(heap)?)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numeric_index_test_agrees_with_the_one_asked_of_the_spelling() {
        // DR-0026's fast path is **transparent**: with it removed, `ToPropertyKey` spells the
        // Number, hands the text to `heap::index_of` and reaches the same key by the slow route.
        // So no program can tell the two apart and no behavioural test can pin this — which is
        // exactly the shape `lab/NOTES.md` records as needing a structural one instead.
        //
        // What it pins is the agreement the whole representation rests on: a Number this answers
        // `Some(n)` for must be one whose `ToString` the text version also answers `Some(n)` for,
        // or `a[i]` and `a[String(i)]` would be two keys for one property.
        let corpus = [
            0.0,
            -0.0,
            1.0,
            2.0,
            9.0,
            10.0,
            1.5,
            -1.0,
            -1.5,
            0.5,
            4_294_967_293.0,
            4_294_967_294.0,
            4_294_967_295.0,
            4_294_967_296.0,
            1e21,
            9_007_199_254_740_991.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for number in corpus {
            let spelled = crate::value::number_to_string(number);
            let units: Vec<u16> = spelled.encode_utf16().collect();
            assert_eq!(
                Vm::array_index_of(number),
                crate::heap::index_of(&units),
                "{number} is judged one way as a Number and another as {spelled:?}"
            );
        }
    }
}
