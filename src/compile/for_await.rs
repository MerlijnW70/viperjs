//! §14.7.5 with an `async` iterator hint — `for await`, and the two awaits an ordinary loop has not.
//!
//! The shape is `for`-`of`'s and the differences are three:
//!
//! - the iterator comes from §7.4.3's *async* path, which may wrap a sync one (see
//!   [`Instruction::GetAsyncIterator`]);
//! - every `next()` answers a **promise**, so the result is awaited before `done` is read;
//! - closing early should await the `return()` too — §7.4.11 waits for the iterator to say it has
//!   finished before the loop may leave.
//!
//! # What is missing: §7.4.11's await on the close
//!
//! The third of those is **not** here yet. Leaving the loop early calls the iterator's `return` and
//! does not await what it answers, so a close that rejects is dropped and one that answers a
//! non-object is not checked. The sync iterator underneath is still closed — the wrapper's `return`
//! calls it before answering — which is why the observable half works and the tests pass.
//!
//! It is not a small fix, and that is the reason it is recorded rather than done: the close for a
//! `break` or a `return` is emitted by `loop_body` through the same `Crossing` machinery every
//! loop uses, so awaiting it means threading "this loop is async" through all of it. Two survivors
//! in `builtins::async_iterator` are waiting on it — the `return` lookup's callable check and the
//! `done` of the result built when there is no `return` — because neither can be seen from a
//! script until the close's answer is looked at.
//!
//! It is a separate file rather than a flag on `for_of_parts` because that function is already the
//! longest in its module and the two share no line that is not `for`-`of`'s shape. What they do
//! share is [`Compiler::for_in_binding`] and [`Compiler::assign_enumerated`], which decide what the
//! head binds and are the same question in both loops.

use super::statement::Check;
use super::{CompileError, Compiler, Instruction};
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
        self.loop_marks.push(mark);
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

        self.assign_enumerated(&statement.left, binding, current, span)?;
        self.loop_body(&statement.body, Some(iterator), |compiler| {
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(top)
        })?;
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
        self.emit_close(iterator, Check::Unwind)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);

        self.chunk.patch(past)?;
        self.chunk.patch(leaving)
    }
}
