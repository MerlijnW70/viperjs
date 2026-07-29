//! §14 — statements, each of which leaves the stack exactly as it found it.
//!
//! Whatever a statement pushes it consumes. The one thing it may leave behind is a new completion
//! value, and that lives in a register rather than on the stack — precisely so that a statement
//! in the middle of a block can replace it without disturbing anything below.

use super::function::Keep;
use super::{CompileError, Compiler, Instruction, Unpatched, unsupported};
use crate::ast::PropertyKey as AstPropertyKey;
use crate::ast::{
    AssignmentTarget, BinaryOperator, Binding, BindingPattern, Declaration, DeclarationKind, Expr,
    ExprKind, ForInOfKind, ForInOfStatement, ForInOfTarget, ForInit, ForStatement,
    LabelledStatement, Stmt, StmtKind, SwitchStatement, TryStatement,
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
                self.loop_marks.push(self.locals.len());
                let compiled = self.loop_body(&statement.body, |compiler| {
                    compiler.chunk.emit(Instruction::Jump(top));
                    Ok(top)
                });
                self.loop_marks.pop();
                compiled?;
                self.chunk.patch(out)
            }
            // §14.7.1 — the test is at the *bottom*, so the body always runs once, the jump back
            // is the opposite sense, and `continue` goes to the test rather than to the top.
            StmtKind::DoWhile(statement) => {
                let top = self.here()?;
                self.loop_marks.push(self.locals.len());
                let compiled = self.loop_body(&statement.body, |compiler| {
                    let test = compiler.here()?;
                    compiler.expression(&statement.test)?;
                    compiler.chunk.emit(Instruction::JumpIfTrue(top));
                    Ok(test)
                });
                self.loop_marks.pop();
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
            StmtKind::Labelled(statement) => self.labelled_statement(statement, span),
            StmtKind::With(_) => Err(unsupported("with", span)),
            // Already made by [`Compiler::hoist_functions`] before the body ran, so a declaration
            // at a body's top level has nothing left to do — and produces no completion value,
            // which is §14.2.2: `function f() {}` alone evaluates to `undefined`.
            //
            // One inside a *block* was not hoisted, so doing nothing here would leave the name
            // unbound and say nothing about it. §14.1 block-scopes it and Annex B.3.3 hoists it
            // in sloppy code; both need block scoping, so it is refused until that exists.
            StmtKind::Function(_) => match self.hoisted.contains(&span) {
                true => Ok(()),
                false => Err(unsupported("a function declaration inside a block", span)),
            },
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
                // §7.4.9 — a `return` leaves *every* enclosing `for`-`of`, so each of their
                // iterators is told, innermost first. The value is already on the stack and the
                // closing is stack-balanced, so it is still there afterwards — and if a `return`
                // method throws, that throw wins over the return, which is step 7.
                for at in (0..self.closes.len()).rev() {
                    let iterator = self.closes[at];
                    self.emit_close(iterator, Check::Loop)?;
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
        let mark = self.enter_scope();
        self.declare_lexical_names(body)?;
        // Deliberately *not* `hoist_functions`. §14.1 block-scopes a function declaration and
        // Annex B.3.3 hoists it besides, and neither is implemented — so one written here is
        // refused, which is what `Compiler::hoisted` is for. Hoisting it now that a block is a
        // scope would give it the scope and none of Annex B, which is the silent wrong answer the
        // refusal was added to stop.
        self.statements(body)?;
        self.leave_scope(mark);
        Ok(())
    }

    /// Create every `let` and `const` a body declares, uninitialised — §14.2.3 and §10.2.11.
    ///
    /// Only the *top level* of the body, because that is what §8.2.6's `LexicallyDeclaredNames`
    /// is: a `let` inside a nested block belongs to that block and is created when it is entered.
    pub(super) fn declare_lexical_names(&mut self, body: &[Stmt]) -> Result<(), CompileError> {
        for statement in body {
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
            self.expression(initializer)?;
            self.destructure(&declarator.binding, Bind::Var, declarator.span)?;
        }
        Ok(())
    }
    /// §14.3.1 — what a `let` or `const` declaration *runs*.
    ///
    /// The binding already exists: [`Compiler::declare_lexical_names`] made it when the block was
    /// entered, and made it uninitialised. So all this does is give it its first value, which is
    /// `InitializeBinding` (§9.1.1.1.4) and is the moment the dead zone ends.
    ///
    /// A `let` with no initializer is initialised to `undefined` — and that is not the same as
    /// being left alone: `let x; x` is `undefined` where `x; let x;` is a ReferenceError, and the
    /// difference is exactly this instruction having run or not.
    fn lexical_declaration(&mut self, declaration: &Declaration) -> Result<(), CompileError> {
        let immutable = declaration.kind == DeclarationKind::Const;
        for declarator in &declaration.declarators {
            // §14.3.1.1 — `const` without an initializer is a Syntax Error the parser has already
            // refused, so anything here without one is a `let`.
            match &declarator.initializer {
                Some(initializer) => self.expression(initializer)?,
                None => self.constant(Value::Undefined)?,
            }
            self.destructure(
                &declarator.binding,
                Bind::Lexical(immutable),
                declarator.span,
            )?;
        }
        Ok(())
    }

    /// §14.3.3 — bind a pattern to the value on top of the stack, consuming it.
    ///
    /// Recursive, because a pattern nests: `{a: {b}}` reads `a` and then takes *that* apart the
    /// same way. Every level leaves the stack as it found it, which is what lets the recursion be
    /// written without a count of what is on it.
    ///
    /// Only object patterns so far. An array one is `GetIterator` and a step per element, and the
    /// sequencing that needs — a `done` that latches, a rest element that collects the remainder,
    /// an `IteratorClose` when the pattern ran out before the iterator did — is a slice of its own.
    fn destructure(
        &mut self,
        binding: &Binding,
        how: Bind,
        span: Span,
    ) -> Result<(), CompileError> {
        match binding {
            Binding::Identifier(name) => self.bind_name(&name.name, how),
            Binding::Pattern(BindingPattern::Object(pattern)) => {
                if pattern.rest.is_some() {
                    return Err(unsupported("a rest property in a binding pattern", span));
                }
                // §14.3.3.7 step 1 — `undefined` and `null` are refused before any property is
                // read, which is why `var {} = null` throws despite reading nothing.
                self.chunk.emit(Instruction::RequireCoercible);
                for property in &pattern.properties {
                    self.chunk.emit(Instruction::Duplicate);
                    self.property_key(&property.key)?;
                    self.chunk.emit(Instruction::GetProperty);
                    self.apply_default(property.value.default.as_deref())?;
                    self.destructure(&property.value.target, how, span)?;
                }
                // The source, which every property was read from and nothing wants now.
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            Binding::Pattern(BindingPattern::Array(pattern)) => {
                self.destructure_array(pattern, how, span)
            }
        }
    }

    /// Take a parameter's pattern apart, from the value on top of the stack.
    ///
    /// A parameter's names are the function's own bindings, made where its `var`s are made — so
    /// they are assigned rather than initialised, which is what [`Bind::Var`] means. The
    /// difference from a `let` is the dead zone, and a parameter has none: it holds `undefined`
    /// from the moment the call begins.
    pub(super) fn destructure_parameter(
        &mut self,
        binding: &Binding,
        span: Span,
    ) -> Result<(), CompileError> {
        self.destructure(binding, Bind::Var, span)
    }

    /// Take a catch parameter apart, from the thrown value on top of the stack.
    ///
    /// [`Bind::Local`] and not [`Bind::Var`]: a catch binding is the block's own wherever the
    /// block is written, and a `var` at the top level of a script is a property of the global
    /// object. Asking for the wrong one there put the thrown value on the global object and left
    /// the catch block reading a slot nothing had filled.
    fn destructure_catch(&mut self, binding: &Binding, span: Span) -> Result<(), CompileError> {
        self.destructure(binding, Bind::Local, span)
    }

    /// §13.15.5 `DestructuringAssignmentEvaluation` — take a value apart into things that already
    /// exist.
    ///
    /// The twin of [`Compiler::destructure`], and the difference is only what happens at the
    /// leaves. A binding pattern makes names; an assignment pattern writes to *references* — a
    /// name, a property, a computed one — so `[o.a, b[i]] = pair` is as ordinary here as
    /// `[x, y] = pair`. Everything above the leaf is the same walk, and the two are written apart
    /// because the syntax trees are: §13.15.5 refines a literal into a pattern, and the refinement
    /// keeps the expression types rather than becoming bindings.
    pub(super) fn assign_pattern(
        &mut self,
        pattern: &crate::ast::Pattern,
        span: Span,
    ) -> Result<(), CompileError> {
        match pattern {
            crate::ast::Pattern::Object(pattern) => {
                if pattern.rest.is_some() {
                    return Err(unsupported(
                        "a rest property in an assignment pattern",
                        span,
                    ));
                }
                self.chunk.emit(Instruction::RequireCoercible);
                for property in &pattern.properties {
                    self.chunk.emit(Instruction::Duplicate);
                    self.property_key(&property.key)?;
                    self.chunk.emit(Instruction::GetProperty);
                    self.apply_default(property.value.default.as_deref())?;
                    self.assign_target(&property.value.target, span)?;
                }
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            crate::ast::Pattern::Array(pattern) => self.assign_array(pattern, span),
        }
    }

    /// The array half, which drives an iterator exactly as a binding pattern does.
    fn assign_array(
        &mut self,
        pattern: &crate::ast::ArrayPattern,
        span: Span,
    ) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("iterator");
        let next = self.declare_hidden("next");
        let done = self.declare_hidden("done");
        let current = self.declare_hidden("current");

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
        self.constant(Value::Boolean(false))?;
        self.chunk.emit(Instruction::StoreVariable(0, done));
        self.chunk.emit(Instruction::Pop);

        let unwind = self.chunk.emit_jump(Instruction::PushHandler);
        let assigned = self.assign_elements(pattern, span, [iterator, next, done, current]);
        self.chunk.emit(Instruction::PopHandler);
        assigned?;

        self.chunk.emit(Instruction::LoadVariable(0, done));
        let already = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.emit_close(iterator, Check::Plain)?;
        self.chunk.patch(already)?;
        let past = self.chunk.emit_jump(Instruction::Jump);

        self.chunk.patch(unwind)?;
        let thrown = self.declare_hidden("thrown");
        self.chunk.emit(Instruction::StoreVariable(0, thrown));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let spent = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.emit_close(iterator, Check::Unwind)?;
        self.chunk.patch(spent)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);
        self.chunk.patch(past)
    }

    /// The elements of an assignment array pattern, and its rest.
    fn assign_elements(
        &mut self,
        pattern: &crate::ast::ArrayPattern,
        span: Span,
        [iterator, next, done, current]: [u32; 4],
    ) -> Result<(), CompileError> {
        for element in &pattern.elements {
            self.emit_step(iterator, next, done)?;
            let Some(element) = element else {
                self.chunk.emit(Instruction::Pop);
                continue;
            };
            self.apply_default(element.default.as_deref())?;
            self.assign_target(&element.target, span)?;
        }
        let Some(rest) = &pattern.rest else {
            return Ok(());
        };
        let collected = self.declare_hidden("rest");
        let at = self.declare_hidden("at");
        self.chunk.emit(Instruction::NewArray(0));
        self.chunk.emit(Instruction::StoreVariable(0, collected));
        self.chunk.emit(Instruction::Pop);
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        let top = self.here()?;
        self.emit_step(iterator, next, done)?;
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let out = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.chunk.emit(Instruction::LoadVariable(0, collected));
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.chunk.emit(Instruction::LoadVariable(0, current));
        self.chunk.emit(Instruction::SetProperty);
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.constant(Value::Number(1.0))?;
        self.chunk.emit(Instruction::Binary(BinaryOperator::Add));
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Jump(top));
        self.chunk.patch(out)?;
        self.chunk.emit(Instruction::LoadVariable(0, collected));
        self.assign_target(rest, span)
    }

    /// Write the value on top of the stack to one target, consuming it.
    ///
    /// A property target is why the value goes into a slot first: `SetProperty` wants its base and
    /// its key *under* the value, and the value arrived before either of them could be evaluated.
    /// Evaluating the reference earlier is not an option — §13.15.5.3 evaluates it here, after the
    /// element it belongs to has been taken from the iterator.
    fn assign_target(&mut self, target: &AssignmentTarget, span: Span) -> Result<(), CompileError> {
        let target = match target {
            AssignmentTarget::Pattern(pattern) => return self.assign_pattern(pattern, span),
            AssignmentTarget::Simple(target) => target,
        };
        match &target.kind {
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                let held = self.declare_hidden("assigned");
                self.chunk.emit(Instruction::StoreVariable(0, held));
                self.chunk.emit(Instruction::Pop);
                self.property_reference(target, Keep::Nothing)?;
                self.chunk.emit(Instruction::LoadVariable(0, held));
                self.chunk.emit(Instruction::SetProperty);
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            ExprKind::Identifier(name) => {
                self.store_name(name)?;
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            _ => Err(unsupported(
                "an assignment to something that is not a name",
                span,
            )),
        }
    }

    /// §8.6.2 `IteratorBindingInitialization` — take an array pattern apart, one step per element.
    ///
    /// An array pattern is not a shorter object pattern. It drives an *iterator*, so the source
    /// need not be an Array and need not have a `length`: anything with an `@@iterator` works, and
    /// the elements come in the order that iterator gives them.
    ///
    /// Three things follow from that and none is optional. An iterator that runs out leaves the
    /// remaining names `undefined` rather than failing — and must not be asked again, which is
    /// what the latching `done` is for. A pattern that finishes while the iterator has not is a
    /// §7.4.9 `IteratorClose`, because the iterator was told to produce and is being abandoned.
    /// And an error while binding abandons it too, which is what the handler is for.
    fn destructure_array(
        &mut self,
        pattern: &crate::ast::ArrayBindingPattern,
        how: Bind,
        span: Span,
    ) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("iterator");
        let next = self.declare_hidden("next");
        let done = self.declare_hidden("done");
        let current = self.declare_hidden("current");

        // §7.4.2 `GetIterator`, on the value already on the stack.
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
        // One `Pop` and not two: the source was the *receiver* of the `@@iterator` call, so the
        // call consumed it. Only `next` is left to drop.
        self.chunk.emit(Instruction::Pop);
        self.constant(Value::Boolean(false))?;
        self.chunk.emit(Instruction::StoreVariable(0, done));
        self.chunk.emit(Instruction::Pop);

        let unwind = self.chunk.emit_jump(Instruction::PushHandler);
        let bound = self.destructure_elements(pattern, how, span, [iterator, next, done, current]);
        self.chunk.emit(Instruction::PopHandler);
        bound?;

        // §8.6.2 step 4 — the pattern is finished and the iterator may not be.
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let already = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.emit_close(iterator, Check::Plain)?;
        self.chunk.patch(already)?;
        let past = self.chunk.emit_jump(Instruction::Jump);

        // …and an error while binding abandons it too, which step 4 covers with the same call.
        self.chunk.patch(unwind)?;
        let thrown = self.declare_hidden("thrown");
        self.chunk.emit(Instruction::StoreVariable(0, thrown));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let spent = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.emit_close(iterator, Check::Unwind)?;
        self.chunk.patch(spent)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);
        self.chunk.patch(past)
    }

    /// The elements of an array pattern, and its rest, once the iterator is in hand.
    ///
    /// The four slots travel together because they are one iterator record spelled out: which
    /// iterator, its `next`, whether it has run out, and where the last step put its value.
    fn destructure_elements(
        &mut self,
        pattern: &crate::ast::ArrayBindingPattern,
        how: Bind,
        span: Span,
        [iterator, next, done, current]: [u32; 4],
    ) -> Result<(), CompileError> {
        for element in &pattern.elements {
            self.emit_step(iterator, next, done)?;
            let Some(element) = element else {
                // An elision — `[, a]` — takes a turn of the iterator and binds nothing. That is
                // not the same as a name that gets `undefined`: the step happens either way.
                self.chunk.emit(Instruction::Pop);
                continue;
            };
            self.apply_default(element.default.as_deref())?;
            self.destructure(&element.target, how, span)?;
        }
        let Some(rest) = &pattern.rest else {
            return Ok(());
        };
        // §8.6.2's `BindingRestElement` — every remaining step, as an Array. The count is a slot
        // rather than the array's `length`, because reading the length back each turn would ask
        // the array a question the loop already knows the answer to.
        let collected = self.declare_hidden("rest");
        let at = self.declare_hidden("at");
        self.chunk.emit(Instruction::NewArray(0));
        self.chunk.emit(Instruction::StoreVariable(0, collected));
        self.chunk.emit(Instruction::Pop);
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        let top = self.here()?;
        self.emit_step(iterator, next, done)?;
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let out = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.chunk.emit(Instruction::LoadVariable(0, collected));
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.chunk.emit(Instruction::LoadVariable(0, current));
        self.chunk.emit(Instruction::SetProperty);
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.constant(Value::Number(1.0))?;
        self.chunk.emit(Instruction::Binary(BinaryOperator::Add));
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Jump(top));
        self.chunk.patch(out)?;
        self.chunk.emit(Instruction::LoadVariable(0, collected));
        self.destructure(rest, how, span)
    }

    /// One turn of the iterator: the value it gave, or `undefined` once it has run out.
    ///
    /// The `done` slot latches. §8.6.2 asks a spent iterator nothing further, so `[a, b]` over a
    /// one-element iterable calls `next` twice and not three times — which a `next` that counts
    /// its own calls can see.
    fn emit_step(&mut self, iterator: u32, next: u32, done: u32) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let spent = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.chunk.emit(Instruction::LoadVariable(0, iterator));
        self.chunk.emit(Instruction::LoadVariable(0, next));
        self.chunk.emit(Instruction::CallMethod(0));
        self.chunk.emit(Instruction::RequireObject);
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("done");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        let going = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::Pop);
        self.constant(Value::Boolean(true))?;
        self.chunk.emit(Instruction::StoreVariable(0, done));
        self.chunk.emit(Instruction::Pop);
        let ran_out = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(going)?;
        let name = self.name_of("value");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::GetProperty);
        let got = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(spent)?;
        self.chunk.patch(ran_out)?;
        self.constant(Value::Undefined)?;
        self.chunk.patch(got)
    }

    /// Push the key a binding property reads, computed or written down.
    fn property_key(&mut self, key: &AstPropertyKey) -> Result<(), CompileError> {
        match key {
            AstPropertyKey::Identifier(name) => {
                let id = self.name_of(name);
                self.constant(Value::String(id))
            }
            AstPropertyKey::String(text) => {
                // Already code units, and already the key: a String literal key is not re-cooked.
                let id = self.heap.intern(text);
                self.constant(Value::String(id))
            }
            AstPropertyKey::Number(number) => {
                let text = crate::value::number_to_string(*number);
                let id = self.name_of(&text);
                self.constant(Value::String(id))
            }
            AstPropertyKey::Computed(expression) => self.expression(expression),
            AstPropertyKey::BigInt(_) => Err(unsupported("a BigInt literal", Span::new(0, 0))),
            AstPropertyKey::Private(_) => Err(unsupported("a private name", Span::new(0, 0))),
        }
    }

    /// §14.3.3 — replace the value on top with `default` when it is `undefined`.
    ///
    /// The default is evaluated only when it is needed, which is observable: `{a = f()}` does not
    /// call `f` when `a` was there. Compared against `undefined` and not against absence, so a
    /// property that is present and `undefined` takes the default too.
    fn apply_default(&mut self, default: Option<&Expr>) -> Result<(), CompileError> {
        let Some(default) = default else {
            return Ok(());
        };
        self.chunk.emit(Instruction::Duplicate);
        self.constant(Value::Undefined)?;
        self.chunk
            .emit(Instruction::Binary(BinaryOperator::StrictEqual));
        let given = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::Pop);
        self.expression(default)?;
        self.chunk.patch(given)
    }

    /// Give one name the value on top of the stack, consuming it.
    fn bind_name(&mut self, name: &str, how: Bind) -> Result<(), CompileError> {
        match how {
            // A `var` at the top level of a script is a property of the global object, not a slot.
            Bind::Var if self.at_global_scope() => {
                let index = self.name(name)?;
                self.chunk.emit(Instruction::StoreGlobal(index));
            }
            Bind::Var => {
                let slot = self.declare(name);
                self.chunk.emit(Instruction::StoreVariable(0, slot));
            }
            Bind::Made => {
                let Some(slot) = self.resolve_in_scope(name) else {
                    // The head declared it a moment ago, so this is a compiler that has lost
                    // track of its own scope rather than anything a program can write.
                    return Err(unsupported(
                        "a binding the head declared and the body cannot find",
                        Span::new(0, 0),
                    ));
                };
                self.chunk.emit(Instruction::Initialise(slot));
            }
            Bind::Local => {
                let slot = match self.resolve_in_scope(name) {
                    Some(slot) => slot,
                    None => self.declare_shadowing(name),
                };
                self.chunk.emit(Instruction::StoreVariable(0, slot));
            }
            Bind::Lexical(immutable) => {
                let slot = match self.resolve_in_scope(name) {
                    Some(slot) => slot,
                    None => {
                        let slot = self.declare_lexical(name, immutable);
                        self.chunk.emit(Instruction::Uninitialise(slot));
                        slot
                    }
                };
                self.chunk.emit(Instruction::Initialise(slot));
            }
        }
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// §14.7.4's four parts.
    fn for_statement(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
        // §14.7.4.4 — the head's `let` and `const` belong to the *loop*, not to the statement
        // around it, so the scope opens before the head is compiled and closes after the body.
        let mark = self.enter_scope();
        self.loop_marks.push(mark);
        let compiled = self.for_parts(statement, span);
        self.loop_marks.pop();
        self.leave_scope(mark);
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
        self.loop_marks.push(mark);
        let compiled = self.for_in_parts(statement, span);
        self.loop_marks.pop();
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
        let binding = self.for_in_binding(&statement.left, span)?;

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

        self.assign_enumerated(&statement.left, binding, current, span)?;
        self.loop_body(&statement.body, |compiler| {
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(top)
        })?;
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
        let mark = self.enter_scope();
        self.loop_marks.push(mark);
        let compiled = self.for_of_parts(statement, span);
        self.loop_marks.pop();
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

        let binding = self.for_in_binding(&statement.left, span)?;

        // §7.4.9 again, for the way out nothing can jump to: a throw from the body, or from
        // `next` itself. The handler closes the iterator and throws the same thing onward — and
        // does not look at what `return` answered, because step 5 has already decided.
        let unwind = self.chunk.emit_jump(Instruction::PushHandler);

        let top = self.here()?;
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

        self.assign_enumerated(&statement.left, binding, current, span)?;
        self.closes.push(iterator);
        let body = self.loop_body(&statement.body, |compiler| {
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(top)
        });
        self.closes.pop();
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
        self.emit_close(iterator, Check::Unwind)?;
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
    fn emit_close(&mut self, iterator: u32, check: Check) -> Result<(), CompileError> {
        // A deliberate exit is leaving the loop, so its handler comes down *first*. Left armed,
        // it would catch a `return` method that threw and close the same iterator a second time —
        // which is one call too many and is observable.
        if check == Check::Loop {
            self.chunk.emit(Instruction::PopHandler);
        }
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
        self.chunk.patch(done)
    }

    /// The interned name of a property this compiler emits for itself.
    fn name_of(&mut self, name: &str) -> crate::heap::StringId {
        self.heap.intern(&name.encode_utf16().collect::<Vec<_>>())
    }

    /// Make the head's binding, if the head declares one, and answer where it lives.
    fn for_in_binding(
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

    /// Put the name this pass is visiting where the head says it goes.
    fn assign_enumerated(
        &mut self,
        target: &ForInOfTarget,
        binding: Option<u32>,
        current: u32,
        span: Span,
    ) -> Result<(), CompileError> {
        match (target, binding) {
            // A pattern head, lexical or not: the value this pass is visiting is taken apart the
            // same way a declaration's initializer would be. Which kind of binding its names get
            // is the only thing the two cases differ in — a `let` initialises bindings the head
            // made, a `var` assigns ones the body already hoisted.
            (ForInOfTarget::Declaration(declaration), held)
                if matches!(
                    declaration.declarators.first().map(|first| &first.binding),
                    Some(Binding::Pattern(_))
                ) =>
            {
                let Some(declarator) = declaration.declarators.first() else {
                    return Err(unsupported("a `for` head with no binding", span));
                };
                let how = match held {
                    Some(_) => Bind::Made,
                    None => Bind::Var,
                };
                self.chunk.emit(Instruction::LoadVariable(0, current));
                self.destructure(&declarator.binding, how, span)
            }
            // A head that declares: the binding is the loop's own, so this initialises it — which
            // for a `const` is the one write it ever gets.
            (ForInOfTarget::Declaration(_), Some(slot)) => {
                self.chunk.emit(Instruction::LoadVariable(0, current));
                self.chunk.emit(Instruction::Initialise(slot));
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            // A `var` head: assigned by name, exactly as `var k = …` would be.
            (ForInOfTarget::Declaration(declaration), None) => {
                let Some(Binding::Identifier(name)) =
                    declaration.declarators.first().map(|first| &first.binding)
                else {
                    return Err(unsupported("a destructuring binding", span));
                };
                self.chunk.emit(Instruction::LoadVariable(0, current));
                self.store_name(&name.name)?;
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
            // A head that does not: an ordinary assignment to whatever it names, each pass.
            (ForInOfTarget::Expression(target), _) => {
                let AssignmentTarget::Simple(target) = &**target else {
                    // §14.7.5.5 — a head that is a pattern rather than a declaration. The value
                    // this pass is visiting is taken apart into things that already exist, which
                    // is the same operation `[a, b] = pair` is.
                    let AssignmentTarget::Pattern(pattern) = &**target else {
                        return Err(unsupported("a destructuring assignment", span));
                    };
                    self.chunk.emit(Instruction::LoadVariable(0, current));
                    return self.assign_pattern(pattern, span);
                };
                match &target.kind {
                    ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                        self.property_reference(target, Keep::Nothing)?;
                        self.chunk.emit(Instruction::LoadVariable(0, current));
                        self.chunk.emit(Instruction::SetProperty);
                    }
                    ExprKind::Identifier(name) => {
                        self.chunk.emit(Instruction::LoadVariable(0, current));
                        self.store_name(name)?;
                    }
                    _ => {
                        return Err(unsupported(
                            "an assignment to something that is not a name",
                            target.span,
                        ));
                    }
                }
                self.chunk.emit(Instruction::Pop);
                Ok(())
            }
        }
    }

    /// The rest of `for`, once its scope has been opened.
    fn for_parts(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
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
        let mark = self.enter_scope();
        for case in statement.cases.iter() {
            self.declare_lexical_names(&case.body)?;
        }
        // A `break` inside a switch leaves the switch, so it is a breakable statement like a
        // loop — but not a *continuable* one, which is why only the break stack is pushed.
        self.breaks.push(Vec::new());

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
        for (entry, start) in entries.into_iter().zip(&starts) {
            if let Some(entry) = entry {
                self.chunk.patch_to(entry, *start);
            }
        }
        match to_default.and_then(|at| starts.get(at)) {
            Some(start) => self.chunk.patch_to(fallback, *start),
            None => self.chunk.patch(fallback)?,
        }

        // The discriminant is still under everything, and a `break` jumps here — so it is
        // discarded after the breaks land rather than before, or a break would leave it behind.
        let breaks = self.breaks.pop().unwrap_or_default();
        for jump in breaks {
            self.chunk.patch_to(jump, after);
        }
        self.chunk.emit(Instruction::Pop);
        self.leave_scope(mark);
        Ok(())
    }

    /// §14.13 — a labelled statement.
    ///
    /// A label is a name attached to the statement that follows, and what it is *for* is being
    /// aimed at: `break outer` leaves the statement the label is on, and `continue outer` goes
    /// round it again. So the label is remembered for as long as that statement is being
    /// compiled, and a `break` that names it patches into the same list the statement's own
    /// breaks do.
    fn labelled_statement(
        &mut self,
        statement: &LabelledStatement,
        span: Span,
    ) -> Result<(), CompileError> {
        // §14.13.1 — a label may not be nested inside one of the same name. The parser refuses
        // that, so this is not a duplicate check; it is what makes the innermost match the right
        // one when the names differ.
        let breakable = matches!(
            statement.body.kind,
            StmtKind::While(_) | StmtKind::DoWhile(_) | StmtKind::For(_) | StmtKind::ForInOf(_)
        );
        if !breakable {
            // `outer: { break outer; }` is legal and needs a break target with no loop under it.
            // A block is not a loop, so there is nowhere for the jump to land yet.
            return Err(unsupported("a label on something that is not a loop", span));
        }
        self.labels
            .push((statement.label.name.clone(), self.breaks.len()));
        let compiled = self.statement(&statement.body);
        self.labels.pop();
        compiled
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
        // `depth` is the *index* of the loop's break list, so the loop is `depth + 1` deep —
        // and a guard recorded at that same number was raised *inside* this loop rather than
        // outside it. Comparing the index against the count directly refuses a break that
        // crosses nothing.
        if self
            .finally_guards
            .last()
            .is_some_and(|guard| depth < *guard)
        {
            return Err(unsupported(
                "break or continue out of a try with a finally",
                span,
            ));
        }
        // §7.4.9 — a labelled jump may cross several loops, and every `for`-`of` it leaves has
        // to be told, innermost first. `depth` is the index of the target loop's break list, so
        // the loops being left are the ones at that index and above — and a `continue` stays
        // inside the target, so it leaves one fewer.
        let staying = usize::from(!leaving);
        for at in (depth + staying..self.closes.len()).rev() {
            let iterator = self.closes[at];
            self.emit_close(iterator, Check::Loop)?;
        }
        // No "is there a list" check, because there always is: [`Compiler::labelled_statement`]
        // records a label only for a loop, and a loop pushes both lists before its body is
        // compiled. Written with a guard, that guard was a branch nothing could take.
        let jump = self.chunk.emit_jump(Instruction::Jump);
        let pending = if leaving {
            self.breaks.get_mut(depth)
        } else {
            self.continues.get_mut(depth)
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
        // `depth` is the *index* of the loop's break list, so the loop is `depth + 1` deep —
        // and a guard recorded at that same number was raised *inside* this loop rather than
        // outside it. Comparing the index against the count directly refuses a break that
        // crosses nothing.
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
        // §7.4.9 — a `break` leaves the innermost loop, so an iterator it was driving has to be
        // told. A `continue` stays in the loop and has nothing to tell it.
        if leaving && self.closes.len() == self.breaks.len() {
            let iterator = self.closes[self.closes.len() - 1];
            self.emit_close(iterator, Check::Loop)?;
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
                    self.make_function(function, statement.span)?;
                    self.chunk.emit(Instruction::StoreGlobal(index));
                }
                false => {
                    let slot = self.declare(&name.name);
                    self.make_function(function, statement.span)?;
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
fn well_known(name: &str) -> u32 {
    u32::try_from(crate::builtins::well_known_at(name)).unwrap_or(u32::MAX)
}

/// What an `IteratorClose` has to do besides closing — §7.4.9 steps 5 and 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
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

/// Which kind of binding a pattern's names are being given — §14.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bind {
    /// A `var`, which was hoisted and is being assigned — and which at the top level of a script
    /// is a property of the global object rather than a slot.
    Var,
    /// A `let` or `const` binding that already exists — initialise it and nothing else.
    ///
    /// A `for`-`of` head's pattern is the case: [`Compiler::for_in_binding`] declared every name
    /// in it, with the head's own mutability, before the loop began. Carrying the mutability here
    /// as well would be a second copy of an answer already given, and one nothing could tell was
    /// wrong.
    Made,
    /// A binding that is always a slot in the current scope, whatever the scope is.
    ///
    /// §14.15.3's catch parameter is the one of these: it belongs to the catch block and to
    /// nothing wider, so a `catch ({a})` written at the top level of a script must *not* reach
    /// the global object the way a `var` there would.
    Local,
    /// A `let` or `const`, which exists uninitialised and is being initialised. The flag is
    /// whether it is a `const`, for the case where the binding has to be made here.
    Lexical(bool),
}
