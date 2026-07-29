//! §14 — statements, each of which leaves the stack exactly as it found it.
//!
//! Whatever a statement pushes it consumes. The one thing it may leave behind is a new completion
//! value, and that lives in a register rather than on the stack — precisely so that a statement
//! in the middle of a block can replace it without disturbing anything below.

use super::function::Keep;
use super::{CompileError, Compiler, Instruction, Unpatched, unsupported};
use crate::ast::{
    AssignmentTarget, BinaryOperator, Binding, Declaration, DeclarationKind, ExprKind, ForInOfKind,
    ForInOfStatement, ForInOfTarget, ForInit, ForStatement, LabelledStatement, Stmt, StmtKind,
    SwitchStatement, TryStatement,
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
                ForInOfKind::Of => Err(unsupported("for-of", span)),
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
                let Binding::Identifier(name) = &declarator.binding else {
                    return Err(unsupported("a destructuring binding", declarator.span));
                };
                let slot = self.declare_lexical(&name.name, immutable);
                self.chunk.emit(Instruction::Uninitialise(slot));
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
            let Binding::Identifier(name) = &declarator.binding else {
                return Err(unsupported("a destructuring binding", declarator.span));
            };
            // A `var` with no initializer does nothing at all. The slot already holds `undefined`
            // from hoisting, and assigning it again would overwrite what an earlier `var x = 1`
            // put there: `var x = 1; var x;` leaves `x` as 1, which surprises people once.
            let Some(initializer) = &declarator.initializer else {
                continue;
            };
            // A `var` belongs to whatever it is written in, and at the top level of a script that
            // is the *global object* rather than a scope — §9.1.1.4 again. The binding itself was
            // made before anything ran, either way; this is only the initializer running where it
            // was written, which is why `x` is `undefined` above its `var x = 1` and 1 below it.
            match self.at_global_scope() {
                true => {
                    let index = self.name(&name.name)?;
                    self.expression(initializer)?;
                    self.chunk.emit(Instruction::StoreGlobal(index));
                }
                false => {
                    let slot = self.declare(&name.name);
                    self.expression(initializer)?;
                    self.chunk.emit(Instruction::StoreVariable(0, slot));
                }
            }
            self.chunk.emit(Instruction::Pop);
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
        for declarator in &declaration.declarators {
            let Binding::Identifier(name) = &declarator.binding else {
                return Err(unsupported("a destructuring binding", declarator.span));
            };
            // §14.3.1.1 — `const` without an initializer is a Syntax Error the parser has already
            // refused, so anything here without one is a `let`.
            // Ordinarily the binding is already there: the block, body or case-block prologue
            // made it uninitialised on the way in. A `for` head has no prologue — §14.7.4.4 gives
            // the loop its own environment and this declaration is what creates the binding in it
            // — so one is made here if there is not one.
            let slot = match self.resolve_in_scope(&name.name) {
                Some(slot) => slot,
                None => {
                    let immutable = declaration.kind == DeclarationKind::Const;
                    let slot = self.declare_lexical(&name.name, immutable);
                    self.chunk.emit(Instruction::Uninitialise(slot));
                    slot
                }
            };
            match &declarator.initializer {
                Some(initializer) => self.expression(initializer)?,
                None => self.constant(Value::Undefined)?,
            }
            self.chunk.emit(Instruction::Initialise(slot));
            self.chunk.emit(Instruction::Pop);
        }
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
        let Binding::Identifier(name) = &declarator.binding else {
            return Err(unsupported("a destructuring binding", declarator.span));
        };
        if !declaration.kind.is_lexical() {
            // A `var` in the head is *not* the loop's. §14.7.5.5 gives it no per-iteration
            // binding and no scope of its own: it was hoisted with the rest of the body's, it
            // outlives the loop, and at the top level of a script it is a property of the global
            // object rather than a slot at all. So nothing is declared here and the assignment
            // goes by name, which is the one path that knows all of that.
            return Ok(None);
        }
        let immutable = declaration.kind == DeclarationKind::Const;
        let slot = self.declare_lexical(&name.name, immutable);
        self.chunk.emit(Instruction::Uninitialise(slot));
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
                    return Err(unsupported("a destructuring assignment", span));
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
            StmtKind::While(_) | StmtKind::DoWhile(_) | StmtKind::For(_)
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
                let Binding::Identifier(name) = &parameter.binding else {
                    return Err(unsupported("a destructuring catch parameter", handler.span));
                };
                let slot = self.declare_shadowing(&name.name);
                self.chunk.emit(Instruction::StoreVariable(0, slot));
                self.chunk.emit(Instruction::Pop);
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
