//! §14.7.5 with an `async` iterator hint — `for await`, and the two awaits an ordinary loop has not.
//!
//! The shape is `for`-`of`'s and the differences are three:
//!
//! - the iterator comes from §7.4.3's *async* path, which may wrap a sync one (see
//!   [`Instruction::GetAsyncIterator`]);
//! - every `next()` answers a **promise**, so the result is awaited before `done` is read;
//! - closing early awaits the `return()` too — §7.4.11 waits for the iterator to say it has
//!   finished before the loop may leave, and discards the close's own failure when a throw is
//!   already travelling.
//!
//! # What the close still costs a turn too many
//!
//! All three are here. What is not right yet is the *timing* of the third on the throw path:
//! `async-from-sync-iterator-continuation-abrupt-completion-get-constructor.js` sees the rejection
//! arrive one tick later than the specification's accounting says it should, and two test262 files
//! detect exactly that. The behaviour is right — the `catch` runs and with the right value — so
//! this is turn accounting somewhere between `Await`, `PromiseResolve` and the wrapper's own
//! `return`, and it wants counting rather than guessing.
//!
//! It is a separate file rather than a flag on `for_of_parts` because that function is already the
//! longest in its module and the two share no line that is not `for`-`of`'s shape. What they do
//! share is [`Compiler::for_in_binding`] and [`Compiler::assign_enumerated`], which decide what the
//! head binds and are the same question in both loops.

use super::LoopScope;
use super::statement::Check;
use super::{Closing, CompileError, Compiler, Instruction};
use crate::ast::ForInOfStatement;
use crate::span::Span;

impl Compiler<'_> {
    /// Compile `for await (… of …) …`, with the scope its head opens.
    pub(super) fn for_await_statement(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        let mark = self.enter_scope();
        self.loop_marks.push(LoopScope {
            mark,
            depth: self.outer.len(),
        });
        let compiled = self.for_await_parts(statement, span);
        self.loop_marks.pop();
        self.leave_scope(mark);
        compiled
    }

    /// The loop itself, once its scope is open.
    fn for_await_parts(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("iterator");
        let next = self.declare_hidden("next");
        let current = self.declare_hidden("current");
        let closable = self.declare_hidden("closable");

        // §7.4.3 with the async hint, which is one instruction because every step of it can throw
        // and the order the getters are read in is observable. It leaves the iterator and its
        // `next`, in that order.
        self.expression(&statement.right)?;
        self.chunk.emit(Instruction::GetAsyncIterator);
        self.chunk.emit(Instruction::StoreVariable(0, next));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::StoreVariable(0, iterator));
        self.chunk.emit(Instruction::Pop);

        let binding = self.for_in_binding(&statement.left, span)?;

        // §7.4.9 for the way out nothing jumps to: a throw from the body or from `next` itself.
        // The handler closes the iterator and throws the same thing onward.
        let unwind = self.chunk.emit_jump(Instruction::PushHandler);

        let top = self.here()?;
        // Cleared before every `next()`, so a throw from the step below is the shape that must not
        // close. Set again once a value is in hand, just above the binding.
        self.constant(crate::value::Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, closable));
        self.chunk.emit(Instruction::Pop);
        // §7.4.4 `IteratorNext`, then the await §14.7.5.7 step 3.d.iii puts in front of everything
        // that reads the result. Awaiting *after* reading `done` would read it off a promise, which
        // is an object and therefore truthy — a loop that ran once and stopped.
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::LoadVariable(0, next));
        self.chunk.emit(Instruction::CallMethod(0));
        self.chunk.emit(Instruction::Await);
        self.chunk.emit(Instruction::RequireObject);
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("done");
        self.constant(crate::value::Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        // Jumping on *false*, so the result object is dropped before the loop can be left — a
        // `break` lands where the done path lands and the two have to agree about the stack.
        let going = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::Pop);
        let out = self.chunk.emit_jump(Instruction::Jump);

        self.chunk.patch(going)?;
        let name = self.name_of("value");
        self.constant(crate::value::Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);

        // §14.7.5.7 step 3 — the `?` on `next()`, on `IteratorComplete` and on `IteratorValue`
        // propagates **without** closing: an iterator whose own `next` threw is not one to tell the
        // walk is over, and the specification only reaches `IteratorClose` from step 3.n and 3.q,
        // which are the binding and the body. A flag rather than moving the handler, because the
        // handler covers a region `continue` jumps back into and `break` crosses — arming it per
        // iteration would need both of those taught about it, where this needs neither.
        self.constant(crate::value::Value::Number(1.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, closable));
        self.chunk.emit(Instruction::Pop);

        self.assign_enumerated(&statement.left, binding, current, span)?;
        self.loop_body(
            &statement.body,
            Some((iterator, Closing::Awaited)),
            |compiler| {
                compiler.chunk.emit(Instruction::Jump(top));
                Ok(top)
            },
        )?;
        // Every `break` arrives having taken the handler down and closed for itself, so this is the
        // done path's business alone.
        let past = self.chunk.emit_jump(Instruction::Jump);

        // An iterator that has said it is done needs no closing — §7.4.5 is explicit about it.
        self.chunk.patch(out)?;
        self.chunk.emit(Instruction::PopHandler);
        let leaving = self.chunk.emit_jump(Instruction::Jump);

        self.chunk.patch(unwind)?;
        let thrown = self.declare_hidden("thrown");
        self.chunk.emit(Instruction::StoreVariable(0, thrown));
        self.chunk.emit(Instruction::Pop);
        // Only when a value had been handed over — see where `closable` is set.
        self.chunk.emit(Instruction::LoadVariable(0, closable));
        let unclosable = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.emit_close(iterator, Check::Unwind, Closing::Awaited)?;
        self.chunk.patch(unclosable)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);

        self.chunk.patch(past)?;
        self.chunk.patch(leaving)
    }
}
