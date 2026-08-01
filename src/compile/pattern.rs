//! §13.15.5 — destructuring *assignment*, which writes to references rather than making names.
//!
//! # The difference from a binding pattern
//!
//! `[a, b] = x` and `let [a, b] = x` parse alike and compile to different things. This half assigns
//! to targets that already exist, and a target may be any reference at all — `[o.a] = x` and
//! `[o[i]] = x` are both legal, and neither is anything a `let` could declare. So each target is
//! evaluated as a *reference* before the value that will be written to it, which §13.15.2 requires
//! and which is why these cannot share the binding half's code.
//!
//! That half is [`super::binding`]. `src/parser/` splits the same two ways.

use super::binding::{Bind, bound_name};
use super::function::Keep;
use super::statement::{Check, well_known};
use super::{CompileError, Compiler, Instruction, unsupported};
use crate::ast::{AssignmentTarget, BinaryOperator, Binding, ExprKind, ForInOfTarget};
use crate::span::Span;
use crate::value::Value;

impl Compiler<'_> {
    /// The array half, which drives an iterator exactly as a binding pattern does.
    pub(super) fn assign_array(
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
        self.emit_close(iterator, Check::Plain, super::Closing::Sync)?;
        self.chunk.patch(already)?;
        let past = self.chunk.emit_jump(Instruction::Jump);

        self.chunk.patch(unwind)?;
        let thrown = self.declare_hidden("thrown");
        self.chunk.emit(Instruction::StoreVariable(0, thrown));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let spent = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.emit_close(iterator, Check::Unwind, super::Closing::Sync)?;
        self.chunk.patch(spent)?;
        self.chunk.emit(Instruction::LoadVariable(0, thrown));
        self.chunk.emit(Instruction::Throw);
        self.chunk.patch(past)
    }

    /// The elements of an assignment array pattern, and its rest.
    pub(super) fn assign_elements(
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
            self.apply_default(element.default.as_deref(), bound_name(&element.target))?;
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
    pub(super) fn assign_target(
        &mut self,
        target: &AssignmentTarget,
        span: Span,
    ) -> Result<(), CompileError> {
        let target = match target {
            AssignmentTarget::Pattern(pattern) => return self.assign_pattern(pattern, span),
            AssignmentTarget::Simple(target) => target,
        };
        match &target.kind {
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                let held = self.declare_hidden("assigned");
                self.chunk.emit(Instruction::StoreVariable(0, held));
                self.chunk.emit(Instruction::Pop);
                let reference = self.property_reference(target, Keep::Nothing)?;
                self.chunk.emit(Instruction::LoadVariable(0, held));
                self.chunk.emit(reference.set());
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

    /// Put the name this pass is visiting where the head says it goes.
    pub(super) fn assign_enumerated(
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
                        let reference = self.property_reference(target, Keep::Nothing)?;
                        self.chunk.emit(Instruction::LoadVariable(0, current));
                        self.chunk.emit(reference.set());
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
}
