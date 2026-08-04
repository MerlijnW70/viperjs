//! §27.5.3.7 step 7 — `yield*`, compiled as the loop the clause describes.
//!
//! A `yield*` is not a `yield` with a flag. It is a loop that stands between the generator's caller
//! and another iterator and passes every message through in both directions: what the inner
//! iterator produces goes out, and what the caller sends back — a value, or a throw — goes in. The
//! clause is written as a loop over a *received completion*, and this compiles exactly that.
//!
//! # Why the result object is passed through whole
//!
//! §27.5.3.7 step 7.a.vii yields `innerResult` itself, not `innerResult.value` rewrapped. That is
//! observable: an inner iterator whose result object carries extra properties hands them to the
//! outer caller unchanged, and a `done` that is truthy-but-not-`true` stays as it was. So the
//! delegating yield needs an instruction of its own — see [`Instruction::YieldDelegated`].
//!
//! # What is missing, and why it is not here
//!
//! Step 7.c is the *return* completion: `gen.return(v)` on a generator suspended inside a `yield*`
//! forwards to the inner iterator's `return`. praxis has no return completion to forward — see the
//! divergence recorded beside `yield` — so this compiles steps 7.a and 7.b, and 7.c arrives with
//! the machinery it needs.

use super::statement::well_known;
use super::{CompileError, Compiler, Instruction};
use crate::ast::Expr;
use crate::value::Value;

/// Which message the loop is about to pass inward.
///
/// A number in a slot rather than two copies of the loop, because the two paths differ in one call
/// and rejoin immediately: what follows a `next` and what follows a `throw` is the same question
/// about the same result object.
const SENDING_NEXT: f64 = 0.0;
/// …and what a caught throw sets it to before going round again.
const SENDING_THROW: f64 = 1.0;
/// …and what a `return` resumption sets it to — §27.5.3.7 step 7.c.
///
/// A third value rather than a second flag, because the three are one question with three answers
/// and the loop rejoins immediately: what follows all of them is the same check on the same result
/// object. Only the *done* branch has to ask again, and it asks this.
const SENDING_RETURN: f64 = 2.0;

impl Compiler<'_> {
    /// Compile `yield* operand` — §27.5.3.7 step 7, as one expression leaving one value.
    ///
    /// The shape, with the four slots it needs:
    ///
    /// ```text
    ///   <operand>; GetIterator; read `next` once      ; §7.4.2
    ///   sending := next, sent := undefined
    /// top:
    ///   if sending is throw: call the inner `throw`, refusing if it has none
    ///   otherwise:           call the inner `next`
    ///   the answer must be an object                  ; §7.4.4
    ///   if it is done: the expression's value is its `value`
    ///   otherwise: yield it whole, inside a handler
    ///     resumed normally: sent := what was sent, sending := next
    ///     resumed by a throw: sent := what was thrown, sending := throw
    ///   go round
    /// ```
    pub(super) fn yield_delegated(&mut self, operand: &Expr) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("delegate");
        let next = self.declare_hidden("next");
        let sent = self.declare_hidden("sent");
        let sending = self.declare_hidden("sending");
        // §15.5.5 step 3's `GetGeneratorKind`, asked of the body being compiled. Inside a generator
        // this can only be an async generator, and it changes three things: which iterator is
        // fetched, that every answer is awaited, and what the suspension hands outward.
        let asynchronous = self.chunk.is_async;

        // §7.4.2 `GetIterator(value, generatorKind)` and then `next` read **once**, exactly as
        // `for`-`of` reads it: a `next` replaced on the inner iterator part-way through is not the
        // one this loop calls.
        //
        // The `async` hint is not a detail. §7.4.3 asks `[@@asyncIterator]` and falls back to
        // §27.1.4's wrapper around `[@@iterator]`, and a `yield*` that asked for the synchronous
        // one instead would *read a property the specification says it must not even look at* —
        // which is observable, and is what a whole bucket of test262 checks by putting a throwing
        // getter on `Symbol.iterator`.
        self.expression(operand)?;
        if asynchronous {
            self.chunk.emit(Instruction::GetAsyncIterator);
            self.chunk.emit(Instruction::StoreVariable(0, next));
            self.chunk.emit(Instruction::Pop);
            self.chunk.emit(Instruction::StoreVariable(0, iterator));
            self.chunk.emit(Instruction::Pop);
        } else {
            self.chunk.emit(Instruction::Duplicate);
            self.chunk
                .emit(Instruction::LoadWellKnown(well_known("iterator")));
            self.chunk.emit(Instruction::GetProperty);
            self.chunk.emit(Instruction::CallMethod(0));
            self.chunk.emit(Instruction::RequireObject);
            self.chunk.emit(Instruction::StoreVariable(0, iterator));
            let name = self.name_of("next");
            self.constant(Value::String(name))?;
            self.chunk.emit(Instruction::GetProperty);
            self.chunk.emit(Instruction::StoreVariable(0, next));
            self.chunk.emit(Instruction::Pop);
        }

        // Step 3 — the first message inward is a normal completion carrying `undefined`, which is
        // why the inner iterator's first `next` is called with no useful argument however the outer
        // generator was resumed.
        self.set_slot(sent, Value::Undefined)?;
        self.set_slot(sending, Value::Number(SENDING_NEXT))?;

        let top = self.here()?;
        self.chunk.emit(Instruction::LoadVariable(0, sending));
        let sending_next = self.chunk.emit_jump(Instruction::JumpIfFalse);
        // Not next, so it is one of the two abrupt ones and they are told apart here.
        self.chunk.emit(Instruction::LoadVariable(0, sending));
        self.constant(Value::Number(SENDING_RETURN))?;
        self.chunk
            .emit(Instruction::Binary(crate::ast::BinaryOperator::StrictEqual));
        let sending_throw = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.forward_return(iterator, sent)?;
        let returned = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(sending_throw)?;
        self.forward_throw(iterator, sent)?;
        let joined = self.chunk.emit_jump(Instruction::Jump);

        // Step 7.a — the ordinary turn: `next.call(iterator, sent)`.
        self.chunk.patch(sending_next)?;
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::LoadVariable(0, next));
        self.chunk.emit(Instruction::LoadVariable(0, sent));
        self.chunk.emit(Instruction::CallMethod(1));

        // Steps 7.a.ii to 7.a.vi — the answer must be an object, and `done` decides whether this
        // expression is finished or the loop goes round again.
        self.chunk.patch(joined)?;
        self.chunk.patch(returned)?;
        // §15.5.5 steps 7.a.iii, 7.b.iii and 7.c.v — an async delegation awaits *the answer*, and
        // here rather than at each of the three calls because all three arrive at this point. It
        // has to be before `done` is read: awaiting afterwards would read `done` off a promise,
        // which is always absent and so always falsy, and the loop would never end.
        if asynchronous {
            self.chunk.emit(Instruction::Await);
        }
        self.chunk.emit(Instruction::RequireObject);
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("done");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        // Truthy rather than `true`, which is what §7.4.4 `IteratorComplete` asks and what the
        // jump already does.
        let going = self.chunk.emit_jump(Instruction::JumpIfFalse);
        // Done: §7.4.5 `IteratorValue` of it is what `yield*` evaluates to — *unless* the message
        // that produced it was a `return`. Step 7.c.viii answers with a **return completion**, so
        // the outer generator leaves with that value rather than carrying on with it as an
        // expression. Same result object, two meanings, and only the mode says which.
        let name = self.name_of("value");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::LoadVariable(0, sending));
        self.constant(Value::Number(SENDING_RETURN))?;
        self.chunk
            .emit(Instruction::Binary(crate::ast::BinaryOperator::StrictEqual));
        let as_a_value = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.unwind_across(super::statement::Exit::Return)?;
        self.chunk.emit(Instruction::Return);
        self.chunk.patch(as_a_value)?;
        let finished = self.chunk.emit_jump(Instruction::Jump);

        // Step 7.a.vii — not done, so the result object goes *out*, whole. The handler is armed
        // across the suspension and is parked with it, so a `throw` from the outside lands here
        // rather than escaping the delegation.
        self.chunk.patch(going)?;
        let caught = self.chunk.emit_jump(Instruction::PushHandler);
        // §27.6.3.8 `AsyncGeneratorYield` takes a *value* and wraps it itself, where §27.5.3.7 step
        // 7.a.vii hands the inner result object straight out to keep its identity. So the async one
        // reads `value` off the result and yields that: wrapping twice would put an iterator result
        // inside an iterator result.
        if asynchronous {
            let name = self.name_of("value");
            self.constant(Value::String(name))?;
            self.chunk.emit(Instruction::GetProperty);
            self.chunk.emit(Instruction::Yield);
        } else {
            self.chunk.emit(Instruction::YieldDelegated);
        }
        self.chunk.emit(Instruction::PopHandler);
        // §27.5.3.7 step 7.c — a `return` resumption reaches the delegation here, and it must not
        // be mistaken for a value sent inward. What is emitted is the same exit a `return` written
        // on this line would take, so the outer generator's `finally` blocks run and its open
        // iterators are closed.
        //
        // Step 7.c's own forwarding — telling the *inner* iterator to return, and carrying on if it
        // says it is not done — is not here, and is the remaining divergence. Without this check at
        // all the resumption was read as an ordinary `next`, which sent the returned value inward
        // and carried on yielding: worse than not forwarding, because it answers.
        self.chunk.emit(Instruction::ResumeMode);
        let carry_on = self.chunk.emit_jump(Instruction::JumpIfFalse);
        // Step 7.c — the value goes *inward*, as the argument to the inner iterator's `return`.
        // Leaving here directly was the previous shape and it skipped the inner iterator entirely.
        self.chunk.emit(Instruction::StoreVariable(0, sent));
        self.chunk.emit(Instruction::Pop);
        self.set_slot(sending, Value::Number(SENDING_RETURN))?;
        self.chunk.emit(Instruction::Jump(top));
        self.chunk.patch(carry_on)?;
        self.chunk.emit(Instruction::StoreVariable(0, sent));
        self.chunk.emit(Instruction::Pop);
        self.set_slot(sending, Value::Number(SENDING_NEXT))?;
        self.chunk.emit(Instruction::Jump(top));

        // Step 7.b — the caller threw into this generator while it was delegating. The value is
        // not this loop's to handle: it goes inward on the next turn, as a throw.
        self.chunk.patch(caught)?;
        self.chunk.emit(Instruction::StoreVariable(0, sent));
        self.chunk.emit(Instruction::Pop);
        self.set_slot(sending, Value::Number(SENDING_THROW))?;
        self.chunk.emit(Instruction::Jump(top));

        self.chunk.patch(finished)
    }

    /// Step 7.c.i to 7.c.viii — hand a `return` to the inner iterator, or leave without it.
    ///
    /// §7.3.10's `GetMethod` again, and the absent case is the one that differs from a throw: an
    /// iterator with no `return` has nothing to be told and that is not an error, so step 7.c.iii
    /// simply leaves with the value. A throw in the same position is a TypeError, because the
    /// caller asked for something to be thrown and nothing threw it.
    fn forward_return(&mut self, iterator: u32, sent: u32) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("return");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        let has_return = self.chunk.emit_jump(|target| {
            Instruction::JumpKeeping(super::chunk::ShortCircuit::WhenNotNullish, target)
        });
        // Step 7.c.iii — nothing to call, so the outer generator leaves with what it was given.
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, sent));
        self.unwind_across(super::statement::Exit::Return)?;
        self.chunk.emit(Instruction::Return);

        self.chunk.patch(has_return)?;
        self.chunk.emit(Instruction::LoadVariable(0, sent));
        self.chunk.emit(Instruction::CallMethod(1));
        Ok(())
    }

    /// Step 7.b.i to 7.b.iii — hand a throw to the inner iterator, or refuse for it.
    ///
    /// `GetMethod` (§7.3.10) treats `undefined` and `null` alike, and an inner iterator with
    /// neither is one that cannot be told: §27.5.3.7 step 7.b.iii closes it and throws a TypeError,
    /// because swallowing the throw would lose it and rethrowing it outward would skip the close.
    ///
    /// Leaves the inner iterator's answer on the stack, which the loop then checks like any other.
    fn forward_throw(&mut self, iterator: u32, sent: u32) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("throw");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        // Nullish means "there is no `throw`", which is one test and not two — §7.3.10 says so.
        let has_throw = self.chunk.emit_jump(|target| {
            Instruction::JumpKeeping(super::chunk::ShortCircuit::WhenNotNullish, target)
        });
        // No `throw` on it: close it and refuse. The receiver and the nullish method both go with
        // the throw, on the same terms as any operand left behind by one.
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Pop);
        // `Check::Plain` and **not** `Check::Unwind`, which is the whole of the difference §7.4.9
        // step 4 turns on. §15.5.5 step 7.b.iii.4 closes with a *normal* completion — there is no
        // throw travelling yet, the TypeError below has not been raised — so step 4 does not fire,
        // the close's own failure is the answer, and step 6 examines what `return` handed back.
        // Every other close in the engine is abandoning a walk that already went wrong, and for
        // those the original completion wins; this one is the exception the clause writes down.
        self.emit_close(
            iterator,
            super::statement::Check::Plain,
            super::Closing::Sync,
        )?;
        self.chunk.emit(Instruction::ThrowNoThrowMethod);

        self.chunk.patch(has_throw)?;
        // The stack is the receiver and the method, which is the shape `CallMethod` wants.
        self.chunk.emit(Instruction::LoadVariable(0, sent));
        self.chunk.emit(Instruction::CallMethod(1));
        Ok(())
    }

    /// Put a constant in a slot, leaving the stack as it was.
    ///
    /// Three instructions written four times otherwise, and each copy a chance to forget the `Pop`
    /// — which unbalances the expression somewhere else entirely.
    fn set_slot(&mut self, slot: u32, value: Value) -> Result<(), CompileError> {
        self.constant(value)?;
        self.chunk.emit(Instruction::StoreVariable(0, slot));
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }
}
