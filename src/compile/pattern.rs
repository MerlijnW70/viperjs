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
use super::expression::Reference;
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
        // §7.4.9's entry for this pattern's iterator, on the same terms a `for`-`of` head installs
        // one. The handler above catches a *throw*; a `return`, a `break` or a `continue` jumping
        // out of here does not throw and would leave the iterator open — and there is a way to
        // write one, because a default in the pattern may `yield`, and resuming that suspension
        // with `it.return()` unwinds straight through. `[ {} = yield ] = iterable` is exactly it.
        //
        // Recorded *after* the handler is armed, because `Check::Loop` takes the handler down as
        // part of closing: the jump has to undo both, in that order.
        let closes = self.unwinds.len();
        self.unwinds.push(crate::compile::Unwind {
            outer: self.breaks.len(),
            what: crate::compile::Crossing::Iterator(iterator, super::Closing::Sync),
        });
        let assigned = self.assign_elements(pattern, span, [iterator, next, done, current]);
        self.unwinds.truncate(closes);
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
            let Some(element) = element else {
                // An elision takes a turn of the iterator and writes nowhere, so there is no
                // reference to evaluate first.
                self.emit_step(iterator, next, done)?;
                self.chunk.emit(Instruction::Pop);
                continue;
            };
            // §13.15.5.5 step 1 — **before** step 2's `IteratorStepValue`.
            let hoisted = self.hoist_reference(&element.target)?;
            self.emit_step(iterator, next, done)?;
            self.apply_default(element.default.as_deref(), bound_name(&element.target))?;
            self.store_hoisted(hoisted, &element.target, span)?;
        }
        let Some(rest) = &pattern.rest else {
            return Ok(());
        };
        // §13.15.5.5's `AssignmentRestElement` step 1 — the same rule, and it comes before the
        // array is even created.
        let hoisted = self.hoist_reference(rest)?;
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
        self.store_hoisted(hoisted, rest, span)
    }

    /// §13.15.5.5 step 1 and §13.15.5.6 step 1 — evaluate a destructuring target's *reference*
    /// before the value it will be given has been fetched.
    ///
    /// `0, [{}[thrower()]] = iterable` must call `next` **zero** times: the target throws while the
    /// iterator is still untouched, so §13.15.5.2 step 5 closes it and nothing was ever asked of
    /// it. praxis stepped first, and the doc on [`Compiler::assign_target`] said evaluating the
    /// reference earlier "is not an option" — it is what the clause requires, and the two orders
    /// are told apart by any target with an effect in it.
    ///
    /// `None` for a nested pattern (step 1 excludes an ObjectLiteral and an ArrayLiteral) and for a
    /// plain name, whose reference is resolved where it is written and cannot be observed either
    /// way. Everything else is a property reference, which is between two and three stack entries
    /// wide, so it is parked in slots until the value turns up.
    pub(super) fn hoist_reference(
        &mut self,
        target: &AssignmentTarget,
    ) -> Result<Option<(Reference, Vec<u32>)>, CompileError> {
        let AssignmentTarget::Simple(expr) = target else {
            return Ok(None);
        };
        if !matches!(
            expr.kind,
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. }
        ) {
            return Ok(None);
        }
        let reference = self.property_reference(expr, Keep::Nothing)?;
        let slots: Vec<u32> = (0..reference.width())
            .map(|_| self.declare_hidden("reference"))
            .collect();
        debug_assert!(!slots.is_empty(), "a property reference is never empty");
        // Emptied from the top down, so the last slot holds what was pushed last — and filled back
        // in the same order below, which is the only reason a `super` reference's three entries
        // come back the way its `SetSuperProperty` wants them.
        for slot in slots.iter().rev() {
            self.chunk.emit(Instruction::StoreVariable(0, *slot));
            self.chunk.emit(Instruction::Pop);
        }
        Ok(Some((reference, slots)))
    }

    /// Write the value on top of the stack through a reference [`Compiler::hoist_reference`] parked.
    ///
    /// Falls back to [`Compiler::assign_target`] when nothing was parked, which is every nested
    /// pattern and every plain name — so steps 5 and 6 stay decided in one place rather than twice.
    pub(super) fn store_hoisted(
        &mut self,
        hoisted: Option<(Reference, Vec<u32>)>,
        target: &AssignmentTarget,
        span: Span,
    ) -> Result<(), CompileError> {
        let Some((reference, slots)) = hoisted else {
            return self.assign_target(target, span);
        };
        let held = self.declare_hidden("assigned");
        self.chunk.emit(Instruction::StoreVariable(0, held));
        self.chunk.emit(Instruction::Pop);
        for slot in &slots {
            self.chunk.emit(Instruction::LoadVariable(0, *slot));
        }
        self.chunk.emit(Instruction::LoadVariable(0, held));
        self.chunk.emit(reference.set());
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// Write the value on top of the stack to one target, consuming it.
    ///
    /// A property target is why the value goes into a slot first: `SetProperty` wants its base and
    /// its key *under* the value, and by the time this is reached the value is already on the
    /// stack. That is the shape for a target whose reference has **not** been hoisted — a nested
    /// pattern or a plain name. §13.15.5.5 step 1 says a property target's reference is evaluated
    /// *before* the value is fetched, and [`Compiler::hoist_reference`] is what does that; this doc
    /// used to say evaluating it earlier "is not an option", which was the clause read backwards.
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
    /// `depth` is how far out the loop's own slots are from where this runs.
    ///
    /// One rather than zero when the head declares lexically: §14.7.5.7 puts the head's binding in
    /// an environment made afresh for the pass, and the value being visited is held in a slot that
    /// belongs to the *loop* — which is therefore one hop further out than it was.
    pub(super) fn assign_enumerated(
        &mut self,
        target: &ForInOfTarget,
        binding: Option<u32>,
        current: u32,
        depth: u32,
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
                self.chunk.emit(Instruction::LoadVariable(depth, current));
                self.destructure(&declarator.binding, how, span)
            }
            // A head that declares: the binding is the loop's own, so this initialises it — which
            // for a `const` is the one write it ever gets.
            (ForInOfTarget::Declaration(_), Some(slot)) => {
                self.chunk.emit(Instruction::LoadVariable(depth, current));
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
                self.chunk.emit(Instruction::LoadVariable(depth, current));
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
                    self.chunk.emit(Instruction::LoadVariable(depth, current));
                    return self.assign_pattern(pattern, span);
                };
                match &target.kind {
                    ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                        let reference = self.property_reference(target, Keep::Nothing)?;
                        self.chunk.emit(Instruction::LoadVariable(depth, current));
                        self.chunk.emit(reference.set());
                    }
                    ExprKind::Identifier(name) => {
                        self.chunk.emit(Instruction::LoadVariable(depth, current));
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
