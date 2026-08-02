//! §14 — statements, each of which leaves the stack exactly as it found it.
//!
//! Whatever a statement pushes it consumes. The one thing it may leave behind is a new completion
//! value, and that lives in a register rather than on the stack — precisely so that a statement
//! in the middle of a block can replace it without disturbing anything below.
//!
//! # What is next door
//!
//! Two chapters used to be in here and are not: [`super::binding`] has §8.6.2's binding patterns
//! and [`super::pattern`] has §13.15.5's destructuring *assignment*. They read alike and are
//! different operations — one makes names, the other writes to references that already exist —
//! and `src/parser/` has always split them exactly this way. All three are one `impl Compiler`,
//! so what a statement compiles to is unchanged by where its helpers live.

use super::Environment;
use super::binding::Bind;
use super::{
    Closing, CompileError, Compiler, Crossing, Instruction, Unpatched, Unwind, unsupported,
};
use crate::ast::{
    BinaryOperator, Declaration, DeclarationKind, ForInOfKind, ForInOfStatement, ForInOfTarget,
    ForInit, ForStatement, LabelledStatement, Stmt, StmtKind, SwitchStatement, TryStatement,
};
use crate::span::Span;
use crate::value::Value;
use std::rc::Rc;

/// Which jump is leaving, and what it leaves.
///
/// The three abrupt completions that travel *out* of a statement without being an exception. They
/// differ in one place only — whether the target loop's own iterator is closed — and this is where
/// that one difference is written down, so that no caller has to remember it.
#[derive(Clone, Copy)]
pub(super) enum Exit {
    /// `break` — the statement whose break list is at this index is left, and everything in it.
    Break(usize),
    /// `continue` — the loop at this index is *not* left, so its iterator stays open.
    Continue(usize),
    /// `return` — every enclosing statement is left, whatever it is and however deep.
    Return,
}

impl Exit {
    /// The jump a `break` or a `continue` aimed at the statement at `depth` makes.
    fn of(leaving: bool, depth: usize) -> Self {
        match leaving {
            true => Self::Break(depth),
            false => Self::Continue(depth),
        }
    }
}

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
            StmtKind::Block(body) => self.block(body),
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
                let compiled = self.loop_body(&statement.body, None, |compiler| {
                    compiler.chunk.emit(Instruction::Jump(top));
                    Ok(top)
                });
                compiled?;
                self.chunk.patch(out)
            }
            // §14.7.1 — the test is at the *bottom*, so the body always runs once, the jump back
            // is the opposite sense, and `continue` goes to the test rather than to the top.
            StmtKind::DoWhile(statement) => {
                let top = self.here()?;
                let compiled = self.loop_body(&statement.body, None, |compiler| {
                    let test = compiler.here()?;
                    compiler.expression(&statement.test)?;
                    compiler.chunk.emit(Instruction::JumpIfTrue(top));
                    Ok(test)
                });
                compiled?;
                Ok(())
            }
            // §14.7.4 — four parts, any of which may be missing. A missing test is `true`, which
            // is what makes `for (;;)` the shortest infinite loop.
            StmtKind::For(statement) => self.for_statement(statement, span),
            // §14.9 and §14.8 — a jump out of, or back to, the innermost loop.
            StmtKind::Break(None) => self.leave_loop(true, span),
            StmtKind::Continue(None) => self.leave_loop(false, span),
            StmtKind::Break(Some(label)) => self.leave_labelled(&label.name, true, span),
            StmtKind::Continue(Some(label)) => self.leave_labelled(&label.name, false, span),
            // §14.14 — throw. The value travels up until a handler wants it, or out of the
            // script; nothing looks at what it is.
            StmtKind::Throw(expression) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Throw);
                Ok(())
            }
            // §14.15 — try, in its three shapes.
            StmtKind::Try(statement) => self.try_statement(statement, span),
            // §14.12 — switch.
            StmtKind::Switch(statement) => self.switch_statement(statement),
            StmtKind::ForInOf(statement) => match statement.kind {
                ForInOfKind::In => self.for_in_statement(statement, span),
                // §14.7.5.7 drives an iterator, which needs `Symbol.iterator` and the protocol
                // around it. Named apart from `for`-`in` so the conformance buckets say which of
                // the two is being waited on.
                ForInOfKind::Of => self.for_of_statement(statement, span),
            },
            // §14.13 — a label, which is a name a `break` or a `continue` can aim at.
            StmtKind::Labelled(statement) => self.labelled_statement(statement),
            StmtKind::With(_) => Err(unsupported("with", span)),
            // Already made by [`Compiler::hoist_functions`] before the body ran, so a declaration
            // at a body's top level has nothing left to do — and produces no completion value,
            // which is §14.2.2: `function f() {}` alone evaluates to `undefined`.
            //
            // One inside a *block* was not hoisted, so doing nothing here would leave the name
            // unbound and say nothing about it. §14.1 block-scopes it and Annex B.3.3 hoists it
            // in sloppy code; both need block scoping, so it is refused until that exists.
            // Already made by [`Compiler::hoist_functions`] before the body ran — at a body's top
            // level and, since §14.1 arrived, at a block's too. So there is nothing left to do,
            // and no completion value either: `function f() {}` alone evaluates to `undefined`.
            StmtKind::Function(_) => Ok(()),
            // §15.7.11 — a declaration evaluates the class and initialises the binding its name
            // already has: the name is hoisted and left uninitialised, so a reference before this
            // point is the temporal dead zone rather than `undefined`.
            StmtKind::Class(class) => {
                // §10.2.9 — a class declaration names its constructor after its own binding.
                self.class(
                    class,
                    match &class.name {
                        Some(name) => super::function::Naming::of(&name.name),
                        None => super::function::Naming::default(),
                    },
                    span,
                )?;
                match &class.name {
                    Some(name) => self.bind_name(&name.name, Bind::Made),
                    None => {
                        self.chunk.emit(Instruction::Pop);
                        Ok(())
                    }
                }
            }
            // §14.10 — `return`, whose argument is optional and whose absence is `undefined`.
            StmtKind::Return(argument) => {
                if self.is_script {
                    return Err(unsupported("return outside a function", span));
                }
                match argument {
                    Some(argument) => self.expression(argument)?,
                    None => self.constant(Value::Undefined)?,
                }
                // §14.15.3 and §7.4.9 — a `return` leaves *every* enclosing statement, so each
                // `finally` runs and each `for`-`of` iterator is told, innermost first. If a
                // `return` method throws, that throw wins over the return, which is §7.4.9 step 7;
                // if a `finally` returns something of its own, its `Return` runs and this one
                // never does, which is §14.15.3's `UpdateEmpty` seen from the other side.
                //
                // The value stays on the stack under whatever those blocks put above it, and
                // nothing saves it first. `Return` truncates to the frame it is leaving, so the one
                // case where this is not stack-neutral — a `finally` that jumps away instead of
                // falling through, abandoning the value — abandons it into a frame that is about to
                // go. Saving it in a slot was written first and then taken out: four mutants said
                // the slot decided nothing, and no test could be written that told them apart.
                self.unwind_across(Exit::Return)?;
                // §10.2.2 step 13 — a derived constructor's `return` is stricter than every other
                // one: an object is answered with, `undefined` is answered with the bound `this`, and
                // any other primitive is a **TypeError** where a base constructor would ignore it.
                // Emitted after the iterators are closed, because a `return` inside a `for`-`of` has
                // to close it whatever the value turns out to be.
                if let Some(slot) = self.chunk.derived_this {
                    self.chunk.emit(Instruction::CompleteDerivedReturn(slot));
                }
                self.chunk.emit(Instruction::Return);
                Ok(())
            }
        }
    }
    /// §14.2 — a block, which is a scope of its own because §14.3.1 puts `let` and `const` in one.
    ///
    /// The bindings are created here, all of them, before any statement runs — that is
    /// §14.2.3 `BlockDeclarationInstantiation` — and each is left uninitialised, which is what
    /// makes reading one above its declaration a ReferenceError rather than `undefined`.
    pub(super) fn block(&mut self, body: &[Stmt]) -> Result<(), CompileError> {
        // §14.2.2 makes a Declarative Environment Record for a block **only if it declares
        // something**. Not an optimisation: `{ }` and `if (c) { x; }` are most of the blocks in
        // any program, and an environment each would be an allocation per `if` for a scope holding
        // nothing. §8.3.2's chain would be longer everywhere and hold the same names.
        let lexical = Self::declares_something_lexical(body);
        let opened = lexical.then(|| self.enter_environment());
        let mark = self.enter_scope();
        self.declare_lexical_names(body)?;
        // §14.1 `BlockDeclarationInstantiation` step 3.a.ii — a function declaration in a block is
        // created *and initialised* before the block's first statement, so `{ f(); function f() {}
        // }` calls it. The slot goes in the block's own level, which since block environments
        // arrived is the block's own environment: a second entry makes a second function.
        //
        // Annex B.3.3's extra `var` binding in the enclosing function is **not** here — DR-0008
        // leaves Annex B out — so `{ function f() {} } f;` is a ReferenceError rather than the
        // function, which is what a strict-mode engine does and what §14.1 alone says.
        self.hoist_functions(body)?;
        self.statements(body)?;
        self.leave_scope(mark);
        if let Some(opened) = opened {
            self.leave_environment(opened)?;
        }
        Ok(())
    }

    /// Whether this body's *top level* declares anything a block scope has to keep — §8.2.6.
    ///
    /// The same question [`Compiler::declare_lexical_names`] answers by doing it, asked before any
    /// of it happens: an environment has to be pushed before the names go into it, and whether to
    /// push one depends on whether there are any. Kept beside that function so the two cannot
    /// drift — a `let` form it learned about and this did not would silently lose its scope.
    fn declares_something_lexical(body: &[Stmt]) -> bool {
        body.iter().any(|statement| match &statement.kind {
            StmtKind::Class(class) => class.name.is_some(),
            StmtKind::Declaration(declaration) => declaration.kind.is_lexical(),
            // §14.1 — a function declaration in a block belongs to the *block*, which is what
            // makes it the one declaration that is hoisted and lexical at once: created and
            // **initialised** when the block is entered, where a `let` is created and left in the
            // dead zone. Without it here the block gets no environment and the name would be a
            // slot in the function, shared by every entry.
            StmtKind::Function(_) => true,
            _ => false,
        })
    }

    /// Create every `let` and `const` a body declares, uninitialised — §14.2.3 and §10.2.11.
    ///
    /// Only the *top level* of the body, because that is what §8.2.6's `LexicallyDeclaredNames`
    /// is: a `let` inside a nested block belongs to that block and is created when it is entered.
    pub(super) fn declare_lexical_names(&mut self, body: &[Stmt]) -> Result<(), CompileError> {
        for statement in body {
            // §15.7.11 — a class declaration is a lexical declaration: its name is hoisted and left
            // uninitialised, so a reference above it is the temporal dead zone rather than
            // `undefined`. That is the difference from a function declaration, which is hoisted
            // *initialised* and callable before its own line.
            if let StmtKind::Class(class) = &statement.kind {
                if let Some(name) = &class.name {
                    let slot = self.declare_lexical(&name.name, false);
                    self.chunk.emit(Instruction::Uninitialise(slot));
                }
                continue;
            }
            let StmtKind::Declaration(declaration) = &statement.kind else {
                continue;
            };
            if !declaration.kind.is_lexical() {
                continue;
            }
            let immutable = declaration.kind == DeclarationKind::Const;
            for declarator in &declaration.declarators {
                // §8.2.1 `BoundNames` of the binding, which for a pattern is every name inside it
                // — `let {a, b: [c]} = x` puts three in the dead zone, not one. The walk is the
                // static-semantics one rather than a second copy here, because a name it missed
                // would be a binding that never existed and a `ReferenceError` no source explains.
                for name in crate::static_semantics::bound_names(&declarator.binding) {
                    let slot = self.declare_lexical(name.name, immutable);
                    self.chunk.emit(Instruction::Uninitialise(slot));
                }
            }
        }
        Ok(())
    }

    /// §14.3 — `var`, `let` and `const`.
    pub(super) fn declaration(
        &mut self,
        declaration: &Declaration,
        _span: Span,
    ) -> Result<(), CompileError> {
        if declaration.kind.is_lexical() {
            return self.lexical_declaration(declaration);
        }
        for declarator in &declaration.declarators {
            // A `var` with no initializer does nothing at all. The slot already holds `undefined`
            // from hoisting, and assigning it again would overwrite what an earlier `var x = 1`
            // put there: `var x = 1; var x;` leaves `x` as 1, which surprises people once.
            //
            // A *pattern* with no initializer cannot happen: §14.3.2.1 makes it a Syntax Error,
            // because there would be nothing to take apart.
            let Some(initializer) = &declarator.initializer else {
                continue;
            };
            // The value is evaluated first and the binding takes it apart — which is what makes
            // a `var` at the top level of a script a property of the global object rather than a
            // slot, decided per name inside [`Compiler::bind_name`] rather than here.
            self.initialiser(&declarator.binding, initializer)?;
            self.destructure(&declarator.binding, Bind::Var, declarator.span)?;
        }
        Ok(())
    }
    /// §14.7.4's four parts.
    fn for_statement(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
        // §14.7.4.4 — the head's `let` and `const` belong to the *loop*, not to the statement
        // around it, so the scope opens before the head is compiled and closes after the body.
        // §14.7.4.4 — and only a *lexical* head gets an environment: `for (var i = …)` declares
        // one binding for the whole function and `for (i = 0; …)` declares none at all, so an
        // environment for either would be a scope with nothing in it that has to be copied on
        // every pass.
        let lexical = matches!(
            &statement.init,
            Some(ForInit::Declaration(declaration)) if declaration.kind.is_lexical()
        );
        let mut opened = lexical.then(|| self.enter_environment());
        let mark = self.enter_scope();
        // §14.7.4.2 — the initialiser runs once, before anything else, and outside the loop it
        // starts. It is compiled here rather than inside `for_parts` because the environment it
        // fills has to exist first and the copies below have to come after it: `for (let i = f();
        // …)` calls `f` once, whatever the loop then does with `i`.
        let compiled = self
            .for_init(statement, span)
            .and_then(|()| self.for_parts(statement, opened.as_mut()));
        self.leave_scope(mark);
        if let Some(opened) = opened {
            self.leave_environment(opened)?;
        }
        compiled
    }

    /// §14.7.5 — `for (x in o)`, over the enumerable property names of `o` and its prototypes.
    ///
    /// The names are taken once, before the body runs, and then walked. §14.7.5.10 asks for
    /// exactly that: a property added while the loop runs need not be visited, and one deleted
    /// before it is reached must not be — so the list settles the first and
    /// [`Instruction::EnumerateNext`] asks the object again about each name, which settles the
    /// second.
    ///
    /// The name is put in a slot of its own before it is assigned anywhere. That is what lets one
    /// shape of code serve all three targets — a fresh binding, an existing name, a property —
    /// rather than each having to reach a value buried under whatever it needs on the stack.
    fn for_in_statement(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        // §14.7.5.5 — a `let` or `const` in the head belongs to the loop, so the scope opens
        // before the binding is made and closes after the body.
        let mark = self.enter_scope();
        let compiled = self.for_in_parts(statement, span);
        self.leave_scope(mark);
        compiled
    }

    /// The rest of `for`-`in`, once its scope has been opened.
    fn for_in_parts(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        let keys = self.declare_hidden("keys");
        let object = self.declare_hidden("object");
        let index = self.declare_hidden("index");
        let current = self.declare_hidden("current");

        // §14.7.5.6 `ForIn/OfHeadEvaluation` — the object is evaluated once, and kept, because
        // every step has to ask it whether the name it is about to visit is still there.
        self.expression(&statement.right)?;
        self.chunk.emit(Instruction::Duplicate);
        self.chunk.emit(Instruction::EnumerateProperties);
        self.chunk.emit(Instruction::StoreVariable(0, keys));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::StoreVariable(0, object));
        self.chunk.emit(Instruction::Pop);
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, index));
        self.chunk.emit(Instruction::Pop);

        // The head's binding, if it declares one. Made once and given its value on each pass —
        // §14.7.5.5 makes it a *fresh* binding per iteration, which is only observable through a
        // closure, and `Compiler::loop_marks` refuses that rather than getting it wrong.
        let top = self.here()?;
        self.chunk.emit(Instruction::LoadVariable(0, object));
        self.chunk.emit(Instruction::EnumerateNext(keys, index));
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);
        // `undefined` is the end of the enumeration and cannot be a name — §14.7.5.10 yields
        // Strings and nothing else.
        self.chunk.emit(Instruction::LoadVariable(0, current));
        self.constant(Value::Undefined)?;
        self.chunk
            .emit(Instruction::Binary(BinaryOperator::StrictEqual));
        let out = self.chunk.emit_jump(Instruction::JumpIfTrue);

        // §14.7.5.7 step 3.g again, and simpler here: a `for`-`in` closes nothing, so there is no
        // iterator entry for the environment to have to sit inside.
        let lexical = matches!(
            &statement.left,
            ForInOfTarget::Declaration(declaration) if declaration.kind.is_lexical()
        );
        let opened = lexical.then(|| self.enter_iteration_environment());
        let binding = self.for_in_binding(&statement.left, span)?;
        let deep = u32::from(lexical);
        self.assign_enumerated(&statement.left, binding, current, deep, span)?;
        let body = self.loop_body(&statement.body, None, |compiler| {
            if lexical {
                compiler.chunk.emit(Instruction::PopScope);
            }
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(top)
        });
        if let Some(opened) = opened {
            self.leave_environment_already_popped(opened)?;
        }
        body?;
        self.chunk.patch(out)
    }

    /// §14.7.5.7 — `for`-`of`, which drives an iterator rather than walking a list of keys.
    ///
    /// The difference from `for`-`in` is not the shape of the loop; it is that every step runs
    /// user code. `next` may throw, may answer something that is not an object, and may have side
    /// effects — so the loop is compiled out of ordinary calls and property reads rather than out
    /// of an instruction that walks something the engine already holds.
    fn for_of_statement(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        // §14.7.5.7 — `for await` walks an **async** iterator, which is a different loop and not a
        // flag on this one: every step is awaited before it is read.
        if statement.is_await {
            return self.for_await_statement(statement, span);
        }
        let mark = self.enter_scope();
        let compiled = self.for_of_parts(statement, span);
        self.leave_scope(mark);
        compiled
    }

    /// The rest of `for`-`of`, once its scope has been opened.
    fn for_of_parts(
        &mut self,
        statement: &ForInOfStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("iterator");
        let next = self.declare_hidden("next");
        let current = self.declare_hidden("current");
        let closable = self.declare_hidden("closable");

        // §7.4.2 `GetIterator` — ask the iterable for its iterator, then read `next` **once**.
        // The handler goes on *after* the iterator exists, because a throw from the head has no
        // iterator to close — and comes off at every deliberate way out, which is why `break`
        // lands past it and `return` unwinds the frame that holds it.
        // Reading it once is observable: a `next` replaced on the iterator part-way through the
        // loop is not the one the rest of the loop calls.
        self.expression(&statement.right)?;
        self.chunk.emit(Instruction::Duplicate);
        self.chunk
            .emit(Instruction::LoadWellKnown(well_known("iterator")));
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::CallMethod(0));
        // Step 5 — what came back must be an object, or the loop would read `next` off a
        // primitive's prototype and call something that was never there.
        self.chunk.emit(Instruction::RequireObject);
        self.chunk.emit(Instruction::StoreVariable(0, iterator));
        let name = self.name_of("next");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::StoreVariable(0, next));
        self.chunk.emit(Instruction::Pop);

        // §7.4.9 again, for the way out nothing can jump to: a throw from the body, or from
        // `next` itself. The handler closes the iterator and throws the same thing onward — and
        // does not look at what `return` answered, because step 5 has already decided.
        //
        // Emitted *above* the per-iteration environment below, which is what makes the throw path
        // simple: unwinding restores the environment a handler was installed in, so the closing
        // code here runs with the loop's own slots at depth zero however many passes had gone by.
        let unwind = self.chunk.emit_jump(Instruction::PushHandler);

        let top = self.here()?;
        // Cleared before every `next()`, so a throw from the step below is the shape that must not
        // close. Set again once a value is in hand, just above the binding.
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, closable));
        self.chunk.emit(Instruction::Pop);
        // §7.4.5 `IteratorStep` — `next.call(iterator)`, and the result must be an object.
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::LoadVariable(0, next));
        self.chunk.emit(Instruction::CallMethod(0));
        self.chunk.emit(Instruction::RequireObject);
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("done");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        // `ToBoolean` of it, which is what the jump already does — §7.4.5 step 4 asks whether it
        // is truthy and not whether it is `true`.
        //
        // Jumping on *false* rather than on true, so that the result object is dropped before the
        // loop can be left. A `break` lands where the done path lands, and the two have to agree
        // about what is on the stack — leaving the result there for the done path to pop made a
        // `break` pop something that was never pushed.
        let going = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::Pop);
        let out = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(going)?;
        let name = self.name_of("value");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);

        // §14.7.5.7 step 3 — the `?` on `next()`, on `IteratorComplete` and on `IteratorValue`
        // propagates **without** closing: an iterator whose own `next` threw is not one to tell the
        // walk is over, and the specification only reaches `IteratorClose` from step 3.n and 3.q,
        // which are the binding and the body. A flag rather than moving the handler, because the
        // handler covers a region `continue` jumps back into and `break` crosses — arming it per
        // iteration would need both of those taught about it, where this needs neither.
        self.constant(Value::Number(1.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, closable));
        self.chunk.emit(Instruction::Pop);

        // §14.7.5.7 step 3.g — `NewDeclarativeEnvironment` **per pass**, holding the head's binding
        // and nothing else. Made here rather than around the whole loop because the loop's own
        // slots — the iterator, its `next`, the flag saying whether it may still be closed — have
        // to outlive one pass and be reachable from the handler above at a fixed depth.
        //
        // A closure made in one pass therefore keeps that pass's binding, which is the difference
        // between `for (const x of [1, 2, 3])` handing three closures three values and three ones.
        let lexical = matches!(
            &statement.left,
            ForInOfTarget::Declaration(declaration) if declaration.kind.is_lexical()
        );
        // §7.4.9's entry for this loop's iterator, pushed here rather than left to `loop_body`,
        // because the environment below sits **inside** it and the two have to be in that order.
        // `unwind_across` stops at the first entry a jump does not cross, and a `continue`
        // deliberately does not close the iterator — so with the iterator on top, a `continue`
        // would stop there and never reach the environment it is certainly leaving.
        let closes = self.unwinds.len();
        self.unwinds.push(Unwind {
            outer: self.breaks.len(),
            what: Crossing::Iterator(iterator, Closing::Sync),
        });
        let opened = lexical.then(|| self.enter_iteration_environment());
        let binding = self.for_in_binding(&statement.left, span)?;
        let deep = u32::from(lexical);
        self.assign_enumerated(&statement.left, binding, current, deep, span)?;
        let body = self.loop_body(&statement.body, None, |compiler| {
            // Falling off the end of the pass leaves its environment, exactly as `break` and
            // `continue` do — those emit their own on the way past, which is what recording it as
            // an iteration environment arranges.
            if lexical {
                compiler.chunk.emit(Instruction::PopScope);
            }
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(top)
        });
        if let Some(opened) = opened {
            self.leave_environment_already_popped(opened)?;
        }
        self.unwinds.truncate(closes);
        body?;
        // Every `break` arrives here having already taken the handler down and closed for itself,
        // so this is the *done* path's business alone.
        let past = self.chunk.emit_jump(Instruction::Jump);

        // An iterator that has said it is done needs no closing — §7.4.5 is explicit that a done
        // iterator is already finished with, and this is where both ways out arrive.
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
        self.emit_close(iterator, Check::Unwind, Closing::Sync)?;
        self.chunk.patch(unclosable)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);
        self.chunk.patch(past)?;
        self.chunk.patch(leaving)
    }

    /// §7.4.9 `IteratorClose` — tell an iterator the loop is leaving early.
    ///
    /// Emitted before every jump that leaves a `for`-`of` from inside it: a `break`, a labelled
    /// one crossing this loop, and a `return`. Not before a `continue`, which stays in the loop
    /// and has nothing to tell it.
    ///
    /// An iterator with no `return` is simply left; one that has it gets it called and the answer
    /// checked. §7.4.9 step 6 makes a non-object answer a **TypeError**, which is the one way
    /// closing an iterator can fail for a reason of its own.
    pub(super) fn emit_close(
        &mut self,
        iterator: u32,
        check: Check,
        closing: Closing,
    ) -> Result<(), CompileError> {
        // A deliberate exit is leaving the loop, so its handler comes down *first*. Left armed,
        // it would catch a `return` method that threw and close the same iterator a second time —
        // which is one call too many and is observable.
        if check == Check::Loop {
            self.chunk.emit(Instruction::PopHandler);
        }
        // §7.4.11 step 4 — on the way out of a *throw* the original completion wins, so every
        // failure of the close is discarded: the rejection the `Await` below raises, and the method
        // lookup step 2 can fail at. A handler around the whole close is the only way to say that,
        // because by then the throw is a value travelling and not a flag to consult.
        let swallow = (closing == Closing::Awaited && check == Check::Unwind)
            .then(|| self.chunk.emit_jump(Instruction::PushHandler));

        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("return");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        self.chunk.emit(Instruction::Duplicate);
        // `undefined` and `null` are both falsy and both mean "there is nothing to call", which
        // is exactly the pair step 4 tests for.
        let absent = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::CallMethod(0));
        // §7.4.11 step 3.d — an async iterator answers with a promise, and the loop may not leave
        // until it settles. Before the check below, because what has to be an object is what the
        // promise *settled with* and not the promise.

        if closing == Closing::Awaited {
            self.chunk.emit(Instruction::Await);
        }
        // §7.4.9 step 5 answers a throw completion *before* it looks at what `return` gave back,
        // so on the way out of a throw the answer is not examined at all. Only a deliberate exit
        // checks it.
        if check != Check::Unwind {
            self.chunk.emit(Instruction::RequireObject);
        }
        self.chunk.emit(Instruction::Pop);
        let done = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(absent)?;
        // The receiver and the absent method are still there; neither is wanted.
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Pop);
        self.chunk.patch(done)?;
        let Some(swallow) = swallow else {
            return Ok(());
        };
        self.chunk.emit(Instruction::PopHandler);
        let past = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(swallow)?;
        self.chunk.emit(Instruction::Pop);
        self.chunk.patch(past)
    }

    /// The interned name of a property this compiler emits for itself.
    pub(super) fn name_of(&mut self, name: &str) -> crate::heap::StringId {
        self.heap.intern(&name.encode_utf16().collect::<Vec<_>>())
    }

    /// Make the head's binding, if the head declares one, and answer where it lives.
    pub(super) fn for_in_binding(
        &mut self,
        target: &ForInOfTarget,
        span: Span,
    ) -> Result<Option<u32>, CompileError> {
        let ForInOfTarget::Declaration(declaration) = target else {
            return Ok(None);
        };
        let Some(declarator) = declaration.declarators.first() else {
            // `ForBinding` is singular and the parser produces exactly one, so this is a tree
            // nobody can write.
            return Err(unsupported("a `for`-`in` head with no binding", span));
        };
        if !declaration.kind.is_lexical() {
            // A `var` in the head is *not* the loop's. §14.7.5.5 gives it no per-iteration
            // binding and no scope of its own: it was hoisted with the rest of the body's, it
            // outlives the loop, and at the top level of a script it is a property of the global
            // object rather than a slot at all. So nothing is declared here and the assignment
            // goes by name, which is the one path that knows all of that.
            return Ok(None);
        }
        // §8.2.1 `BoundNames` — a pattern in the head declares every name inside it, and each
        // starts in the dead zone exactly as a single name does.
        let immutable = declaration.kind == DeclarationKind::Const;
        let mut first = None;
        for name in crate::static_semantics::bound_names(&declarator.binding) {
            let slot = self.declare_lexical(name.name, immutable);
            self.chunk.emit(Instruction::Uninitialise(slot));
            first.get_or_insert(slot);
        }
        let slot = first.unwrap_or_default();
        Ok(Some(slot))
    }

    /// The rest of `for`, once its scope has been opened.
    fn for_init(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
        match &statement.init {
            Some(ForInit::Expression(expression)) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Pop);
            }
            Some(ForInit::Declaration(declaration)) => self.declaration(declaration, span)?,
            None => {}
        }
        Ok(())
    }

    /// The rest of `for`, once its head has run — §14.7.4.7 `ForBodyEvaluation`.
    fn for_parts(
        &mut self,
        statement: &ForStatement,
        mut per_iteration: Option<&mut Environment>,
    ) -> Result<(), CompileError> {
        // §14.7.4.7 step 2 — before the first test, so the initialiser's own environment is never
        // the one the body runs in. Without it the first pass shares its bindings with the
        // initialiser and every later pass does not, which is a difference no reader would expect
        // and only a closure made on the first pass can see.
        if let Some(environment) = per_iteration.as_mut() {
            self.copy_environment(environment);
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
        self.loop_body(&statement.body, None, |compiler| {
            // `continue` goes to the *update*, not to the test: `for (i = 0; i < 3; i = i + 1) {
            // continue; }` still increments, which is the whole reason the third part exists.
            let target = compiler.here()?;
            // §14.7.4.7 step 3.d — **before** the update and at the `continue` target, because a
            // `continue` reaches step 3.d too. The update then runs against the next pass's copy,
            // so `i++` carries the count forward while the closure the last pass made keeps its
            // own `i`.
            if let Some(environment) = per_iteration.as_mut() {
                compiler.copy_environment(environment);
            }
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
    /// Emit everything between here and the statement being left.
    ///
    /// The one place a `break`, a `continue` and a `return` agree: each leaves some run of
    /// enclosing statements, and each of those may have a `finally` to run (§14.15.3), an iterator
    /// to close (§7.4.9) or a handler to take down. They come off innermost first, which is what
    /// makes a throw inside a `finally` escape the `try` that owns it rather than being caught by
    /// its own `catch` — the handlers go down before the block that might throw runs.
    ///
    /// [`Exit`] says which jump this is and what it leaves — one argument rather than an index and
    /// a flag, because those two together can spell a `return` that leaves only *some* of them,
    /// which is not a thing that exists. A mutant that flipped that flag survived every test there
    /// was, for the good reason that nothing it changed was ever read.
    pub(super) fn unwind_across(&mut self, exit: Exit) -> Result<(), CompileError> {
        let stack = std::mem::take(&mut self.unwinds);
        let mut outcome = Ok(());
        for at in (0..stack.len()).rev() {
            let entry = &stack[at];
            let iterator = matches!(entry.what, Crossing::Iterator(..));
            let operand = matches!(entry.what, Crossing::Operand);
            let crossed = match exit {
                // A `return` leaves everything — except an *operand*. The value being returned is
                // already on the stack above it, so a `Pop` here would discard that instead; and
                // the frame's whole stack goes when the call returns, so what is left under it was
                // never a leak. The two other jumps stay inside the frame and do have to tidy.
                Exit::Return => !operand,
                // An iterator belongs to its own loop rather than sitting inside it, which is the
                // one place the two jumps differ: a `break` closes the target loop's own iterator
                // and a `continue` does not, while a `finally` at that same depth was written in
                // the body and runs for either.
                Exit::Break(depth) => entry.outer > depth || (iterator && entry.outer == depth),
                Exit::Continue(depth) => entry.outer > depth,
            };
            if !crossed {
                break;
            }
            // While this entry is being emitted, it and everything above it is already dealt with.
            // Without that, a `return` written inside a `finally` would emit the same `finally`
            // again, and again, for as long as the compiler had memory.
            self.unwinds = stack[..at].to_vec();
            outcome = match &stack[at].what {
                Crossing::Handlers(count) => {
                    for _ in 0..*count {
                        self.chunk.emit(Instruction::PopHandler);
                    }
                    Ok(())
                }
                Crossing::Operand => {
                    self.chunk.emit(Instruction::Pop);
                    Ok(())
                }
                Crossing::Scope => {
                    self.chunk.emit(Instruction::PopScope);
                    Ok(())
                }
                Crossing::Finally(body) => self.block(body),
                Crossing::Iterator(slot, closing) => self.emit_close(*slot, Check::Loop, *closing),
            };
            if outcome.is_err() {
                break;
            }
        }
        self.unwinds = stack;
        outcome
    }
    /// Compile a loop body with somewhere for `break` and `continue` to go.
    ///
    /// `after` compiles whatever follows the body — the jump back, and a `for` loop's update —
    /// and answers where `continue` should land, which is not the same place in all three loops.
    /// Every `break` collected while the body was compiled is patched to just past everything.
    pub(super) fn loop_body(
        &mut self,
        body: &Stmt,
        iterator: Option<(u32, Closing)>,
        after: impl FnOnce(&mut Self) -> Result<u32, CompileError>,
    ) -> Result<(), CompileError> {
        self.breaks.push(Vec::new());
        self.continues.push(Some(Vec::new()));
        // §7.4.9 — the iterator this loop drives, if it drives one, recorded against this loop's
        // own index rather than against its body: a `continue` of *this* loop does not close it,
        // which is the one place an iterator differs from everything else on the stack. Taken as an
        // argument rather than pushed by the caller beforehand: the caller would have to know not to
        // push twice, and *that* was a branch nothing could pin.
        let mark = self.unwinds.len();
        if let Some((slot, closing)) = iterator {
            self.unwinds.push(Unwind {
                outer: self.breaks.len() - 1,
                what: Crossing::Iterator(slot, closing),
            });
        }
        let compiled = self.statement(body).and_then(|()| after(self));
        // The stacks come back down even when compilation failed, so that a later loop does
        // not inherit this one's pending jumps and patch them into its own end.
        let continues = self.continues.pop().flatten().unwrap_or_default();
        let breaks = self.breaks.pop().unwrap_or_default();
        self.unwinds.truncate(mark);
        let continue_target = compiled?;
        for jump in continues {
            self.chunk.patch_to(jump, continue_target);
        }
        for jump in breaks {
            self.chunk.patch(jump)?;
        }
        Ok(())
    }
    /// §14.12 — `switch`.
    ///
    /// # Why the tests come first and the bodies afterwards
    ///
    /// Because a `switch` is not a chain of `if`s. §14.12.4 evaluates the case tests **in order**
    /// until one is strictly equal to the discriminant, and *then* runs every statement from that
    /// case to the end of the switch — through the other cases, not into them. Fall-through is
    /// not a quirk of the syntax; it is what the algorithm says, and compiling the bodies as one
    /// run of statements with entry points into it is the shape that makes it true for free.
    ///
    /// `default` is not tried in that first pass. §14.12.4 runs the tests, and only if none
    /// matched does it come back to the default — so `switch (x) { default: a; case 1: b; }` with
    /// `x` of 1 runs `b` alone, and with `x` of 2 runs `a` and then `b`.
    fn switch_statement(&mut self, statement: &SwitchStatement) -> Result<(), CompileError> {
        self.expression(&statement.discriminant)?;
        // §14.12.4 — the `CaseBlock` is *one* scope over all of the cases, not one per case. So a
        // `let` in one case is in scope in the next, and its dead zone runs from the top of the
        // whole block: `switch (x) { case 1: y; break; case 2: let y; }` throws rather than
        // reading `undefined`. That is why the bindings are created here, before any test runs.
        //
        // And *one environment* over all of them, for the same reason — §14.12.4 step 3's
        // `NewDeclarativeEnvironment` around the whole `CaseBlock`. Entered **before** the switch's
        // own break list is pushed, which is what decides whether a `break` pops it: `unwind_across`
        // crosses a scope entry only when its `outer` is *greater* than the exit's depth, so an
        // entry recorded at the switch's own depth is left to the one `PopScope` below — where
        // falling off the last case, a `break`, and no case matching at all all converge. A
        // `continue` out to an enclosing loop has a smaller depth and does cross it, which is right.
        let lexical = statement
            .cases
            .iter()
            .any(|case| Self::declares_something_lexical(&case.body));
        let opened = lexical.then(|| self.enter_environment());
        // The discriminant stays on the stack for the whole `CaseBlock`, so a jump that leaves the
        // switch without landing on the convergence below has to drop it. Recorded at the same
        // depth as the environment and for the same reason: a `break` lands where it is popped
        // anyway, and a `continue` or a `return` jumps clean past — which left the value on the
        // stack and faulted the *next* pass of the enclosing loop with `UnbalancedStack`.
        let unwinds = self.unwinds.len();
        self.unwinds.push(Unwind {
            outer: self.breaks.len(),
            what: Crossing::Operand,
        });
        let mark = self.enter_scope();
        for case in statement.cases.iter() {
            self.declare_lexical_names(&case.body)?;
        }
        // A `break` inside a switch leaves the switch, so it is a breakable statement like a
        // loop — but not a *continuable* one. Both stacks are pushed all the same, with `None` for
        // the second: an exit's depth is one number indexing both, so a switch that pushed only one
        // of them would make that number mean something different on either side of itself.
        self.breaks.push(Vec::new());
        self.continues.push(None);

        // The tests, in order, each jumping to where its body begins.
        let mut entries = Vec::new();
        let mut to_default = None;
        for case in statement.cases.iter() {
            let Some(test) = &case.test else {
                to_default = Some(entries.len());
                entries.push(None);
                continue;
            };
            // The discriminant is compared once per test and has to survive each comparison.
            self.chunk.emit(Instruction::Duplicate);
            self.expression(test)?;
            self.chunk
                .emit(Instruction::Binary(BinaryOperator::StrictEqual));
            entries.push(Some(self.chunk.emit_jump(Instruction::JumpIfTrue)));
        }
        // Nothing matched: to the default if there is one, past everything if there is not.
        let fallback = self.chunk.emit_jump(Instruction::Jump);

        // The bodies, as one run of statements with an entry point at each case.
        let mut starts = Vec::new();
        for case in statement.cases.iter() {
            starts.push(self.here()?);
            self.statements(&case.body)?;
        }
        let after = self.here()?;
        // Every way out of the case bodies arrives here, so this is where the case block's
        // environment is left — once, on a path all three of them share.
        self.leave_scope(mark);
        self.unwinds.truncate(unwinds);
        if let Some(opened) = opened {
            self.leave_environment(opened)?;
        }
        for (entry, start) in entries.into_iter().zip(&starts) {
            if let Some(entry) = entry {
                self.chunk.patch_to(entry, *start);
            }
        }
        match to_default.and_then(|at| starts.get(at)) {
            Some(start) => self.chunk.patch_to(fallback, *start),
            // Aimed at `after` by name rather than at wherever the compiler has got to, because
            // the `PopScope` above now sits between the two and a no-match path must run it.
            None => self.chunk.patch_to(fallback, after),
        }

        // The discriminant is still under everything, and a `break` jumps here — so it is
        // discarded after the breaks land rather than before, or a break would leave it behind.
        let breaks = self.breaks.pop().unwrap_or_default();
        self.continues.pop();
        for jump in breaks {
            self.chunk.patch_to(jump, after);
        }
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// §14.13 — a labelled statement.
    ///
    /// A label is a name attached to the statement that follows, and what it is *for* is being
    /// aimed at: `break outer` leaves the statement the label is on, and `continue outer` goes
    /// round it again. So the label is remembered for as long as that statement is being
    /// compiled, and a `break` that names it patches into the same list the statement's own
    /// breaks do.
    fn labelled_statement(&mut self, statement: &LabelledStatement) -> Result<(), CompileError> {
        // §14.13.1 — a label may not be nested inside one of the same name. The parser refuses
        // that, so this is not a duplicate check; it is what makes the innermost match the right
        // one when the names differ.
        let breakable = matches!(
            statement.body.kind,
            StmtKind::While(_) | StmtKind::DoWhile(_) | StmtKind::For(_) | StmtKind::ForInOf(_)
        );
        self.labels
            .push((statement.label.name.clone(), self.breaks.len()));
        if breakable {
            let compiled = self.statement(&statement.body);
            self.labels.pop();
            return compiled;
        }
        // §14.13.4 — a label on anything else is a break target and nothing more: `outer: { break
        // outer; }` leaves the block, and there is no loop under it for the jump to land in. So the
        // statement gets a break list of its own, exactly as a `switch` does, and the jumps patch to
        // the end of it. `continue outer` is a Syntax Error the parser has already refused, which is
        // why there is no continue list to go with it.
        self.breaks.push(Vec::new());
        let compiled = self.statement(&statement.body);
        let breaks = self.breaks.pop().unwrap_or_default();
        self.labels.pop();
        compiled?;
        let after = self.here()?;
        for jump in breaks {
            self.chunk.patch_to(jump, after);
        }
        Ok(())
    }

    /// `break name` or `continue name` — §14.9 and §14.8 with a label.
    ///
    /// The label names a *statement*, and the jump goes to that statement's end or its next turn
    /// — so the only thing that differs from an unlabelled one is which list the jump joins.
    fn leave_labelled(
        &mut self,
        name: &str,
        leaving: bool,
        span: Span,
    ) -> Result<(), CompileError> {
        let Some((_, depth)) = self.labels.iter().rev().find(|(label, _)| &**label == name) else {
            // The parser refuses a label that is not in scope, so this is unreachable from
            // source — and refusing rather than guessing keeps it that way.
            return Err(unsupported("a break or continue to an unknown label", span));
        };
        let depth = *depth;
        // A labelled jump may cross several statements at once, and each gets what it is owed on
        // the way past — which is the whole of what is special about it. An ordinary break leaves
        // one statement; this one leaves as many as the label is above.
        self.unwind_across(Exit::of(leaving, depth))?;
        // No "is there a list" check, because there always is: [`Compiler::labelled_statement`]
        // records a label only for a loop, and a loop pushes both lists before its body is
        // compiled. Written with a guard, that guard was a branch nothing could take.
        let jump = self.chunk.emit_jump(Instruction::Jump);
        let pending = if leaving {
            self.breaks.get_mut(depth)
        } else {
            self.continues.get_mut(depth).and_then(Option::as_mut)
        };
        if let Some(pending) = pending {
            pending.push(jump);
        }
        Ok(())
    }

    /// `break` or `continue` — a jump whose target is not compiled yet.
    ///
    /// The parser refuses either outside a loop, so an empty stack here is unreachable from
    /// source; refusing rather than emitting a jump to nowhere is what keeps it unreachable.
    fn leave_loop(&mut self, leaving: bool, span: Span) -> Result<(), CompileError> {
        // The target's depth, in the one scale everything else is measured in. A `break` leaves the
        // innermost breakable statement whatever it is; a `continue` leaves the innermost
        // *continuable* one, which is not the same thing the moment a switch is in between — and
        // taking the innermost breakable for both is what unwound a `continue` to the switch's
        // depth and left its discriminant behind.
        let depth = match leaving {
            true => self.breaks.len().checked_sub(1),
            false => self.continues.iter().rposition(Option::is_some),
        };
        let Some(depth) = depth else {
            return Err(unsupported("break or continue outside a loop", span));
        };
        // Everything between here and it is crossed: a `finally` in between runs, an iterator in
        // between is closed, a handler in between comes down, a scope in between is left.
        self.unwind_across(Exit::of(leaving, depth))?;
        let jump = self.chunk.emit_jump(Instruction::Jump);
        let pending = match leaving {
            true => self.breaks.get_mut(depth),
            false => self.continues.get_mut(depth).and_then(Option::as_mut),
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
        let outer = self.breaks.len();
        let mark = self.unwinds.len();
        // §14.15.3 — the `finally` goes on first, so that a jump out of here meets it *after* the
        // handlers. A throw inside a `finally` is not caught by the `try` the `finally` belongs to,
        // and taking the handlers down before running the block is what makes that true.
        if let Some(finalizer) = &statement.finalizer {
            self.unwinds.push(Unwind {
                outer,
                what: Crossing::Finally(Rc::from(&**finalizer)),
            });
        }
        let unwind = has_finally.then(|| self.chunk.emit_jump(Instruction::PushHandler));
        let to_catch = statement
            .handler
            .as_ref()
            .map(|_| self.chunk.emit_jump(Instruction::PushHandler));
        // Both of them are armed over the try block, and a `break` out of it has to take down
        // exactly the ones it jumps past — a count rather than a flag, because the catch block
        // below is inside one of these two and not the other.
        let armed = u32::from(unwind.is_some()) + u32::from(to_catch.is_some());
        self.unwinds.push(Unwind {
            outer,
            what: Crossing::Handlers(armed),
        });

        let compiled = self.try_body(statement, to_catch);
        // Down before the `finally` is emitted below: the block belongs to whatever encloses this
        // `try` rather than to the `try`, so a `break` inside it does not run it a second time.
        self.unwinds.truncate(mark);
        compiled?;

        if let Some(unwind) = unwind {
            // The normal way out: forget the handler, then run the finally.
            self.chunk.emit(Instruction::PopHandler);
            if let Some(finalizer) = &statement.finalizer {
                self.block(finalizer)?;
            }
            let end = self.chunk.emit_jump(Instruction::Jump);
            // …and the other way out, carrying whatever was thrown. The value is parked in a
            // slot no source text can name, because the finally block may use the stack.
            self.chunk.patch(unwind)?;
            let saved = self.declare_hidden("thrown");
            self.chunk.emit(Instruction::StoreVariable(0, saved));
            self.chunk.emit(Instruction::Pop);
            if let Some(finalizer) = &statement.finalizer {
                self.block(finalizer)?;
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
        self.block(&statement.block)?;
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
        // From here on one fewer handler is armed: getting to a catch block means the throw found
        // this handler, and a handler that fires is taken off the stack by the throw that found it.
        // So a `break` out of the *catch* block owes one less than one out of the try block.
        if let Some(entry) = self.unwinds.last_mut()
            && let Crossing::Handlers(armed) = &mut entry.what
        {
            *armed = u32::from(statement.finalizer.is_some());
        }

        let Some(handler) = &statement.handler else {
            // Unreachable: `to_catch` exists only when the handler does. Saying so with a jump
            // that lands here rather than an assertion keeps this total.
            return self.chunk.patch(past_the_catch);
        };
        // §14.15.3 — the catch parameter is a *block-scoped* binding of its own, so it is given a
        // slot for the duration of the catch block and taken away again. `catch { }` with no
        // parameter is ES2019's optional binding: the value is simply discarded.
        let outer_locals = self.enter_scope();
        match &handler.parameter {
            Some(parameter) => {
                // §14.15.3 makes every name in the parameter a binding of the catch block, so a
                // pattern declares all of them and then takes the thrown value apart. Declared
                // with `declare_shadowing` for the same reason a name is: the catch's binding is
                // its own, and an outer one of the same name is hidden rather than written to.
                for name in crate::static_semantics::bound_names(&parameter.binding) {
                    self.declare_shadowing(name.name);
                }
                self.destructure_catch(&parameter.binding, handler.span)?;
            }
            None => self.chunk.emit(Instruction::Pop),
        }
        self.block(&handler.body)?;
        self.leave_scope(outer_locals);
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
            // Remembered so the statement itself knows it was hoisted. One inside a block never
            // reaches here, and doing nothing for it would leave the name unbound in silence.
            self.hoisted.push(statement.span);
            // §9.1.1.4.16 `CreateGlobalFunctionBinding` at the top level of a script, and an
            // ordinary slot everywhere else. The declaration comes first so the property exists
            // with a script's attributes — writable, enumerable, not configurable — and the store
            // then puts the function in it. Assigning to a name that does not exist yet would
            // create it *configurable*, and `delete f` would then work on a function declaration.
            match self.at_global_scope() {
                true => {
                    let index = self.name(&name.name)?;
                    self.chunk.emit(Instruction::DeclareGlobal(index));
                    self.make_function(
                        function,
                        // §10.2.9 — a declaration is named by its own binding.
                        match &function.name {
                            Some(written) => super::function::Naming::of(&written.name),
                            None => super::function::Naming::default(),
                        },
                        statement.span,
                    )?;
                    self.chunk.emit(Instruction::StoreGlobal(index));
                }
                false => {
                    let slot = self.declare(&name.name);
                    self.make_function(
                        function,
                        // §10.2.9 — a declaration is named by its own binding.
                        match &function.name {
                            Some(written) => super::function::Naming::of(&written.name),
                            None => super::function::Naming::default(),
                        },
                        statement.span,
                    )?;
                    self.chunk.emit(Instruction::StoreVariable(0, slot));
                }
            }
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

/// Where a well-known Symbol sits in the realm's table — see `crate::builtins::well_known_at`.
pub(super) fn well_known(name: &str) -> u32 {
    u32::try_from(crate::builtins::well_known_at(name)).unwrap_or(u32::MAX)
}

/// What an `IteratorClose` has to do besides closing — §7.4.9 steps 5 and 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Check {
    /// Leaving a `for`-`of` deliberately: the loop's handler comes down first, and a non-object
    /// answer is a TypeError.
    Loop,
    /// Leaving one on the way out of a throw: no handler to take down, because the throw already
    /// consumed it, and step 5 has already decided what the answer is.
    Unwind,
    /// Closing an iterator a pattern is finished with, where there is no handler in the way and
    /// the answer is examined as step 6 asks.
    Plain,
}
