//! §14 — statements, each of which leaves the stack exactly as it found it.
//!
//! Whatever a statement pushes it consumes. The one thing it may leave behind is a new completion
//! value, and that lives in a register rather than on the stack — precisely so that a statement
//! in the middle of a block can replace it without disturbing anything below.

use super::{CompileError, Compiler, Instruction, Unpatched, unsupported};
use crate::ast::{
    Binding, Declaration, DeclarationKind, ForInit, ForStatement, Stmt, StmtKind, TryStatement,
};
use crate::span::Span;
use crate::value::Value;

impl Compiler<'_> {
    /// Compile one statement, leaving the stack as it found it.
    ///
    /// Every statement is stack-neutral: whatever it pushes it consumes, and the one thing it may
    /// leave behind is a new completion value, which lives in a register rather than on the
    /// stack. That invariant is what [`crate::vm::Fault::UnbalancedStack`] checks at the end.
    pub(super) fn statement(&mut self, statement: &Stmt) -> Result<(), CompileError> {
        let span = statement.span;
        match &statement.kind {
            // §14.4 and §14.16 — neither does anything, and neither is value-producing, so the
            // completion value of `1; ;` is 1 rather than `undefined`.
            StmtKind::Empty | StmtKind::Debugger => Ok(()),
            // §14.5.1 — the only statement whose value is its own.
            StmtKind::Expression(expression) => {
                self.expression(expression)?;
                // §14.2.2's completion value is the *script's*. What a statement inside a
                // function evaluates to is nobody's business but `return`'s, so its value is
                // discarded rather than allowed to overwrite the script's.
                if self.is_script {
                    self.chunk.emit(Instruction::SetCompletion);
                } else {
                    self.chunk.emit(Instruction::Pop);
                }
                Ok(())
            }
            // §14.2 — a block is its statements. No scope of its own yet: a block only *has* one
            // when something lexical is declared inside it, and `let` and `const` are refused
            // below until they can throw on a use before their declaration.
            StmtKind::Block(body) => self.statements(body),
            StmtKind::Declaration(declaration) => self.declaration(declaration, span),
            // §14.6 — the test is thrown away and exactly one branch runs. An `if` with no `else`
            // still jumps over nothing, which is one wasted instruction and no special case.
            StmtKind::If(statement) => {
                self.expression(&statement.test)?;
                let to_alternate = self.chunk.emit_jump(Instruction::JumpIfFalse);
                self.statement(&statement.consequent)?;
                let past_the_alternate = self.chunk.emit_jump(Instruction::Jump);
                self.chunk.patch(to_alternate)?;
                if let Some(alternate) = &statement.alternate {
                    self.statement(alternate)?;
                }
                self.chunk.patch(past_the_alternate)
            }
            // §14.7.2 — the test is at the top, so `while (0) x` never runs its body, and
            // `continue` goes back to that test.
            StmtKind::While(statement) => {
                let top = self.here()?;
                self.expression(&statement.test)?;
                let out = self.chunk.emit_jump(Instruction::JumpIfFalse);
                self.loop_body(&statement.body, |compiler| {
                    compiler.chunk.emit(Instruction::Jump(top));
                    Ok(top)
                })?;
                self.chunk.patch(out)
            }
            // §14.7.1 — the test is at the *bottom*, so the body always runs once, the jump back
            // is the opposite sense, and `continue` goes to the test rather than to the top.
            StmtKind::DoWhile(statement) => {
                let top = self.here()?;
                self.loop_body(&statement.body, |compiler| {
                    let test = compiler.here()?;
                    compiler.expression(&statement.test)?;
                    compiler.chunk.emit(Instruction::JumpIfTrue(top));
                    Ok(test)
                })?;
                Ok(())
            }
            // §14.7.4 — four parts, any of which may be missing. A missing test is `true`, which
            // is what makes `for (;;)` the shortest infinite loop.
            StmtKind::For(statement) => self.for_statement(statement, span),
            // §14.9 and §14.8 — a jump out of, or back to, the innermost loop.
            StmtKind::Break(None) => self.leave_loop(true, span),
            StmtKind::Continue(None) => self.leave_loop(false, span),
            StmtKind::Break(Some(_)) | StmtKind::Continue(Some(_)) => {
                Err(unsupported("a labelled break or continue", span))
            }
            // §14.14 — throw. The value travels up until a handler wants it, or out of the
            // script; nothing looks at what it is.
            StmtKind::Throw(expression) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Throw);
                Ok(())
            }
            // §14.15 — try, in its three shapes.
            StmtKind::Try(statement) => self.try_statement(statement, span),
            StmtKind::Switch(_) => Err(unsupported("switch", span)),
            StmtKind::ForInOf(_) => Err(unsupported("for-in and for-of", span)),
            StmtKind::Labelled(_) => Err(unsupported("a labelled statement", span)),
            StmtKind::With(_) => Err(unsupported("with", span)),
            // Already made by [`Compiler::hoist_functions`] before the body ran, so the
            // declaration itself has nothing left to do. That it produces no completion value is
            // §14.2.2 as well: `function f() {}` alone evaluates to `undefined`.
            StmtKind::Function(_) => Ok(()),
            StmtKind::Class(_) => Err(unsupported("a class declaration", span)),
            // §14.10 — `return`, whose argument is optional and whose absence is `undefined`.
            StmtKind::Return(argument) => {
                if self.is_script {
                    return Err(unsupported("return outside a function", span));
                }
                match argument {
                    Some(argument) => self.expression(argument)?,
                    None => self.constant(Value::Undefined)?,
                }
                self.chunk.emit(Instruction::Return);
                Ok(())
            }
        }
    }
    /// §14.3 — `var`, `let` and `const`.
    ///
    /// Only `var` so far. `let` and `const` are refused rather than treated as `var`, because the
    /// difference between them is the temporal dead zone: reading one before its declaration is a
    /// **ReferenceError**, and nothing can throw one yet. Quietly making them behave like `var`
    /// would be a wrong answer no test of this engine would catch.
    pub(super) fn declaration(
        &mut self,
        declaration: &Declaration,
        span: Span,
    ) -> Result<(), CompileError> {
        if declaration.kind != DeclarationKind::Var {
            return Err(unsupported("let and const", span));
        }
        for declarator in &declaration.declarators {
            let Binding::Identifier(name) = &declarator.binding else {
                return Err(unsupported("a destructuring binding", declarator.span));
            };
            // A `var` with no initializer does nothing at all. The slot already holds `undefined`
            // from hoisting, and assigning it again would overwrite what an earlier `var x = 1`
            // put there: `var x = 1; var x;` leaves `x` as 1, which surprises people once.
            let Some(initializer) = &declarator.initializer else {
                continue;
            };
            let slot = self.declare(&name.name);
            self.expression(initializer)?;
            self.chunk.emit(Instruction::StoreVariable(0, slot));
            self.chunk.emit(Instruction::Pop);
            // A `var` always belongs to the function it is written in, so it is always a local —
            // even inside a function that also reads script-level names.
        }
        Ok(())
    }
    /// §14.7.4's four parts.
    fn for_statement(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
        match &statement.init {
            Some(ForInit::Expression(expression)) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Pop);
            }
            Some(ForInit::Declaration(declaration)) => self.declaration(declaration, span)?,
            None => {}
        }
        let top = self.here()?;
        // A missing test is `true` — `for (;;)` — so there is simply no jump out of the top.
        let out = match &statement.test {
            Some(test) => {
                self.expression(test)?;
                Some(self.chunk.emit_jump(Instruction::JumpIfFalse))
            }
            None => None,
        };
        let update = statement.update.as_ref();
        self.loop_body(&statement.body, |compiler| {
            // `continue` goes to the *update*, not to the test: `for (i = 0; i < 3; i = i + 1) {
            // continue; }` still increments, which is the whole reason the third part exists.
            let target = compiler.here()?;
            if let Some(update) = update {
                compiler.expression(update)?;
                compiler.chunk.emit(Instruction::Pop);
            }
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(target)
        })?;
        match out {
            Some(out) => self.chunk.patch(out),
            None => Ok(()),
        }
    }
    /// Compile a loop body with somewhere for `break` and `continue` to go.
    ///
    /// `after` compiles whatever follows the body — the jump back, and a `for` loop's update —
    /// and answers where `continue` should land, which is not the same place in all three loops.
    /// Every `break` collected while the body was compiled is patched to just past everything.
    fn loop_body(
        &mut self,
        body: &Stmt,
        after: impl FnOnce(&mut Self) -> Result<u32, CompileError>,
    ) -> Result<(), CompileError> {
        self.breaks.push(Vec::new());
        self.continues.push(Vec::new());
        let compiled = self.statement(body).and_then(|()| after(self));
        // The two stacks come back down even when compilation failed, so that a later loop does
        // not inherit this one's pending jumps and patch them into its own end.
        let continues = self.continues.pop().unwrap_or_default();
        let breaks = self.breaks.pop().unwrap_or_default();
        let continue_target = compiled?;
        for jump in continues {
            self.chunk.patch_to(jump, continue_target);
        }
        for jump in breaks {
            self.chunk.patch(jump)?;
        }
        Ok(())
    }
    /// `break` or `continue` — a jump whose target is not compiled yet.
    ///
    /// The parser refuses either outside a loop, so an empty stack here is unreachable from
    /// source; refusing rather than emitting a jump to nowhere is what keeps it unreachable.
    fn leave_loop(&mut self, leaving: bool, span: Span) -> Result<(), CompileError> {
        let pending = if leaving {
            self.breaks.last_mut()
        } else {
            self.continues.last_mut()
        };
        if pending.is_none() {
            return Err(unsupported("break or continue outside a loop", span));
        }
        // Leaving a `try` that has a `finally` is a third way out of it, and the finally has to
        // run on the way past. A loop written inside the `try` is unaffected, which is what the
        // depth comparison is for: the jump only crosses the finally when its loop began before
        // the `try` did.
        if self
            .finally_guards
            .last()
            .is_some_and(|depth| self.breaks.len() <= *depth)
        {
            return Err(unsupported(
                "break or continue out of a try with a finally",
                span,
            ));
        }
        let jump = self.chunk.emit_jump(Instruction::Jump);
        let pending = if leaving {
            self.breaks.last_mut()
        } else {
            self.continues.last_mut()
        };
        if let Some(pending) = pending {
            pending.push(jump);
        }
        Ok(())
    }
    /// §14.15 — `try`, with or without each of its two tails.
    ///
    /// # Why the `finally` block is compiled twice
    ///
    /// It has to run on the way out however the `try` was left, and there are two ways: normally,
    /// and carrying a thrown value. The alternative to two copies is a subroutine that both paths
    /// call and that returns to a variable address — the shape that gave the JVM `jsr`/`ret` and
    /// then a decade of verifier bugs. Two copies of a block are larger and are obviously right.
    ///
    /// # The shape
    ///
    /// ```text
    ///   PushHandler(UNWIND)    ; only when there is a finally
    ///   PushHandler(CATCH)     ; only when there is a catch
    ///   <the try block>
    ///   PopHandler             ; the inner one
    ///   Jump(AFTER)
    /// CATCH:                   ; the thrown value is on the stack
    ///   StoreLocal(parameter); Pop
    ///   <the catch block>
    /// AFTER:
    ///   PopHandler             ; the outer one
    ///   <the finally block>
    ///   Jump(END)
    /// UNWIND:                  ; the thrown value is on the stack
    ///   StoreLocal(saved); Pop
    ///   <the finally block>
    ///   LoadLocal(saved)
    ///   Throw                  ; on to the next handler out
    /// END:
    /// ```
    ///
    /// The outer handler is what makes a throw *inside the catch block* still run the finally,
    /// which is the case a single handler gets wrong.
    fn try_statement(&mut self, statement: &TryStatement, span: Span) -> Result<(), CompileError> {
        let has_finally = statement.finalizer.is_some();
        // `break` and `continue` out of a `try` would have to run the finally on the way past,
        // which is a third exit path and a larger design. Refusing the ones that leave the `try`
        // is narrow and honest; a loop written *inside* the try is unaffected, which is why this
        // records the loop depth rather than refusing outright.
        if has_finally {
            self.finally_guards.push(self.breaks.len());
        }
        let unwind = has_finally.then(|| self.chunk.emit_jump(Instruction::PushHandler));
        let to_catch = statement
            .handler
            .as_ref()
            .map(|_| self.chunk.emit_jump(Instruction::PushHandler));

        let compiled = self.try_body(statement, to_catch);
        if has_finally {
            self.finally_guards.pop();
        }
        compiled?;

        if let Some(unwind) = unwind {
            // The normal way out: forget the handler, then run the finally.
            self.chunk.emit(Instruction::PopHandler);
            if let Some(finalizer) = &statement.finalizer {
                self.statements(finalizer)?;
            }
            let end = self.chunk.emit_jump(Instruction::Jump);
            // …and the other way out, carrying whatever was thrown. The value is parked in a
            // slot no source text can name, because the finally block may use the stack.
            self.chunk.patch(unwind)?;
            let saved = self.declare_hidden("thrown");
            self.chunk.emit(Instruction::StoreVariable(0, saved));
            self.chunk.emit(Instruction::Pop);
            if let Some(finalizer) = &statement.finalizer {
                self.statements(finalizer)?;
            }
            self.chunk.emit(Instruction::LoadVariable(0, saved));
            self.chunk.emit(Instruction::Throw);
            self.chunk.patch(end)?;
        }
        let _ = span;
        Ok(())
    }
    /// The try block and its catch clause, up to the point where the finally would begin.
    fn try_body(
        &mut self,
        statement: &TryStatement,
        to_catch: Option<Unpatched>,
    ) -> Result<(), CompileError> {
        self.statements(&statement.block)?;
        let Some(to_catch) = to_catch else {
            // No catch clause, so the try block's protection ends here and a throw inside it has
            // already gone to the finally's handler.
            return Ok(());
        };
        // The try block finished without throwing, so its handler is no longer wanted and the
        // catch block must be jumped over.
        self.chunk.emit(Instruction::PopHandler);
        let past_the_catch = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(to_catch)?;

        let Some(handler) = &statement.handler else {
            // Unreachable: `to_catch` exists only when the handler does. Saying so with a jump
            // that lands here rather than an assertion keeps this total.
            return self.chunk.patch(past_the_catch);
        };
        // §14.15.3 — the catch parameter is a *block-scoped* binding of its own, so it is given a
        // slot for the duration of the catch block and taken away again. `catch { }` with no
        // parameter is ES2019's optional binding: the value is simply discarded.
        let outer_locals = self.locals.len();
        match &handler.parameter {
            Some(parameter) => {
                let Binding::Identifier(name) = &parameter.binding else {
                    return Err(unsupported("a destructuring catch parameter", handler.span));
                };
                let slot = self.declare_shadowing(&name.name);
                self.chunk.emit(Instruction::StoreVariable(0, slot));
                self.chunk.emit(Instruction::Pop);
            }
            None => self.chunk.emit(Instruction::Pop),
        }
        self.statements(&handler.body)?;
        self.locals.truncate(outer_locals);
        self.chunk.patch(past_the_catch)
    }
    /// Make the function objects a body's declarations describe, before any of it runs.
    ///
    /// §10.2.11's `FunctionDeclarationInstantiation`, in the part that separates a function
    /// declaration from every other kind. A `var` is *declared* early and assigned where it is
    /// written; a function is **initialised** early, which is why `f()` above `function f() {}`
    /// works and `g()` above `var g = function () {}` does not.
    ///
    /// Only the top level of the body. A function declared inside a block is Annex B's business
    /// and is refused, because its rules are a compatibility settlement rather than a semantics.
    pub(super) fn hoist_functions(&mut self, body: &[Stmt]) -> Result<(), CompileError> {
        for statement in body {
            let StmtKind::Function(function) = &statement.kind else {
                continue;
            };
            let Some(name) = &function.name else {
                // A declaration without a name is `export default function () {}`, which is a
                // module thing and has no name to hoist under.
                return Err(unsupported(
                    "an anonymous function declaration",
                    statement.span,
                ));
            };
            let slot = self.declare(&name.name);
            self.make_function(function, statement.span)?;
            self.chunk.emit(Instruction::StoreVariable(0, slot));
            self.chunk.emit(Instruction::Pop);
        }
        Ok(())
    }
    pub(super) fn statements(&mut self, statements: &[Stmt]) -> Result<(), CompileError> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }
}
