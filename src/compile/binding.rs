//! §8.6.2 — binding patterns, and the declarations that hold one.
//!
//! # Why this is not §13.15.5's destructuring
//!
//! The two look identical in source and are different operations. A *binding* pattern creates
//! bindings — `let {a} = o` declares `a` — and the compiler knows their slots as it goes. A
//! destructuring *assignment* writes to references that already exist and may be arbitrary
//! expressions, which is why `[o.a] = x` is legal and `let [o.a] = x` is not. That half is in
//! [`super::pattern`], and `src/parser/` makes the same split for the same reason.
//!
//! These are one `impl Compiler` spread across three files, so what they compile to is unchanged
//! by living here. `pub(super)` means "within `compile`", which is where the callers are.

use super::statement::{Check, well_known};
use super::{CompileError, Compiler, Instruction, unsupported};
use crate::ast::PropertyKey as AstPropertyKey;
use crate::ast::{
    AssignmentTarget, BinaryOperator, Binding, BindingPattern, Declaration, DeclarationKind, Expr,
    ExprKind,
};
use crate::span::Span;
use crate::value::Value;

impl Compiler<'_> {
    /// §14.3.1 — what a `let` or `const` declaration *runs*.
    ///
    /// The binding already exists: [`Compiler::declare_lexical_names`] made it when the block was
    /// entered, and made it uninitialised. So all this does is give it its first value, which is
    /// `InitializeBinding` (§9.1.1.1.4) and is the moment the dead zone ends.
    ///
    /// A `let` with no initializer is initialised to `undefined` — and that is not the same as
    /// being left alone: `let x; x` is `undefined` where `x; let x;` is a ReferenceError, and the
    /// difference is exactly this instruction having run or not.
    pub(super) fn lexical_declaration(
        &mut self,
        declaration: &Declaration,
    ) -> Result<(), CompileError> {
        let immutable = declaration.kind == DeclarationKind::Const;
        for declarator in &declaration.declarators {
            // §14.3.1.1 — `const` without an initializer is a Syntax Error the parser has already
            // refused, so anything here without one is a `let`.
            match &declarator.initializer {
                Some(initializer) => self.initialiser(&declarator.binding, initializer)?,
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
    pub(super) fn destructure(
        &mut self,
        binding: &Binding,
        how: Bind,
        span: Span,
    ) -> Result<(), CompileError> {
        match binding {
            Binding::Identifier(name) => self.bind_name(&name.name, how),
            Binding::Pattern(BindingPattern::Object(pattern)) => {
                // §14.3.3.7 step 1 — `undefined` and `null` are refused before any property is
                // read, which is why `var {} = null` throws despite reading nothing.
                self.chunk.emit(Instruction::RequireCoercible);
                let held = match pattern.rest.is_some() {
                    true => self.stash_keys(pattern.properties.len()),
                    false => Vec::new(),
                };
                for (at, property) in pattern.properties.iter().enumerate() {
                    self.chunk.emit(Instruction::Duplicate);
                    self.push_key(&property.key, held.get(at).copied())?;
                    self.chunk.emit(Instruction::GetProperty);
                    self.apply_default(
                        property.value.default.as_deref(),
                        bound_name(&property.value.target),
                    )?;
                    self.destructure(&property.value.target, how, span)?;
                }
                match &pattern.rest {
                    // §14.3.3 — the rest takes the source with it, since §7.3.25 needs it.
                    Some(name) => {
                        self.emit_rest(&held)?;
                        self.bind_name(&name.name, how)
                    }
                    // Nothing wants the source now.
                    None => {
                        self.chunk.emit(Instruction::Pop);
                        Ok(())
                    }
                }
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
    pub(super) fn destructure_catch(
        &mut self,
        binding: &Binding,
        span: Span,
    ) -> Result<(), CompileError> {
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
                self.chunk.emit(Instruction::RequireCoercible);
                let held = match pattern.rest.is_some() {
                    true => self.stash_keys(pattern.properties.len()),
                    false => Vec::new(),
                };
                for (at, property) in pattern.properties.iter().enumerate() {
                    self.chunk.emit(Instruction::Duplicate);
                    self.push_key(&property.key, held.get(at).copied())?;
                    self.chunk.emit(Instruction::GetProperty);
                    self.apply_default(
                        property.value.default.as_deref(),
                        bound_name(&property.value.target),
                    )?;
                    self.assign_target(&property.value.target, span)?;
                }
                match &pattern.rest {
                    Some(target) => {
                        self.emit_rest(&held)?;
                        let target = AssignmentTarget::Simple((**target).clone());
                        self.assign_target(&target, span)
                    }
                    None => {
                        self.chunk.emit(Instruction::Pop);
                        Ok(())
                    }
                }
            }
            crate::ast::Pattern::Array(pattern) => self.assign_array(pattern, span),
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
    pub(super) fn destructure_array(
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
        self.emit_close(iterator, Check::Plain, super::Closing::Sync)?;
        self.chunk.patch(already)?;
        let past = self.chunk.emit_jump(Instruction::Jump);

        // …and an error while binding abandons it too, which step 4 covers with the same call.
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

    /// The elements of an array pattern, and its rest, once the iterator is in hand.
    ///
    /// The four slots travel together because they are one iterator record spelled out: which
    /// iterator, its `next`, whether it has run out, and where the last step put its value.
    pub(super) fn destructure_elements(
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
            self.apply_default(element.default.as_deref(), bound_name(&element.target))?;
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
    pub(super) fn emit_step(
        &mut self,
        iterator: u32,
        next: u32,
        done: u32,
    ) -> Result<(), CompileError> {
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

    /// Slots for a pattern's keys, when a rest property means they will be needed twice.
    ///
    /// Once to read the property, and once to tell §7.3.25 which keys not to copy. A *computed*
    /// key is a value, so evaluating it a second time would run the expression twice — `{[f()]: v,
    /// ...rest}` calls `f` once, and this is what makes that true. With no rest there is nothing
    /// to stash and the keys are pushed where they are used.
    pub(super) fn stash_keys(&mut self, count: usize) -> Vec<u32> {
        (0..count).map(|_| self.declare_hidden("key")).collect()
    }

    /// Push one property key, keeping a copy in `held` when the rest will want it back.
    pub(super) fn push_key(
        &mut self,
        key: &AstPropertyKey,
        held: Option<u32>,
    ) -> Result<(), CompileError> {
        self.property_key(key)?;
        let Some(slot) = held else {
            return Ok(());
        };
        self.chunk.emit(Instruction::StoreVariable(0, slot));
        Ok(())
    }

    /// §7.3.25 — turn the source on the stack into the object a rest property collects.
    pub(super) fn emit_rest(&mut self, held: &[u32]) -> Result<(), CompileError> {
        for slot in held {
            self.chunk.emit(Instruction::LoadVariable(0, *slot));
        }
        let count = u32::try_from(held.len()).map_err(|_| CompileError {
            kind: crate::compile::ErrorKind::TooLong,
            span: Span::new(0, 0),
        })?;
        self.chunk.emit(Instruction::CopyRest(count));
        Ok(())
    }

    /// Push the key a binding property reads, computed or written down.
    pub(super) fn property_key(&mut self, key: &AstPropertyKey) -> Result<(), CompileError> {
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
            // §13.2.5.1 — a numeric `PropertyName` is its *ToString*, and a BigInt's is its digits
            // without the `n`. So `{ 1n: 'a' }` and `{ '1': 'a' }` are the same property, which is
            // what makes `({1n: 'a'})[1]` answer `'a'`.
            AstPropertyKey::BigInt(literal) => {
                let Some(value) =
                    crate::bigint::BigInt::from_digits(&literal.digits, literal.radix)
                else {
                    return Err(unsupported(
                        "a BigInt property key this large",
                        Span::new(0, 0),
                    ));
                };
                let id = self.name_of(&value.to_digits(10));
                self.constant(Value::String(id))
            }
            // A private name is not a `PropertyName` at all — §15.7's `ClassElementName` is one *or*
            // a `PrivateIdentifier`. A private *field* never reaches here, because its Private Name
            // is minted at the class definition instead; what does is a private method, accessor or
            // static, each of which needs §7.3.30's `PrivateMethodOrAccessorAdd`.
            AstPropertyKey::Private(_) => Err(unsupported(
                "a private method, accessor or static",
                Span::new(0, 0),
            )),
        }
    }

    /// §8.6.3 — a binding's initialiser, named after the binding when it is anonymous.
    ///
    /// One of the closed list of positions `NamedEvaluation` applies to, and the commonest: it is what
    /// makes `var f = function () {}` and `const f = () => {}` both answer `"f"` for `f.name`.
    pub(super) fn initialiser(
        &mut self,
        target: &Binding,
        value: &Expr,
    ) -> Result<(), CompileError> {
        match target {
            // Only a plain name is a named position. A pattern binds several names and none of them is
            // *the* name — `var [a] = [function () {}]` leaves the function unnamed, and the element's
            // own default is where §8.6.3 reaches instead.
            Binding::Identifier(name) => self.named_evaluation(&name.name, value),
            Binding::Pattern(_) => self.expression(value),
        }
    }

    /// §14.3.3 — replace the value on top with `default` when it is `undefined`.
    ///
    /// The default is evaluated only when it is needed, which is observable: `{a = f()}` does not
    /// call `f` when `a` was there. Compared against `undefined` and not against absence, so a
    /// property that is present and `undefined` takes the default too.
    ///
    /// `target` is what the default is standing in for, when that is a plain name: §8.6.3 reaches a
    /// destructuring default too, so `let [x = function () {}] = []` gives the function the name `x`.
    pub(super) fn apply_default(
        &mut self,
        default: Option<&Expr>,
        target: Option<&str>,
    ) -> Result<(), CompileError> {
        let Some(default) = default else {
            return Ok(());
        };
        self.chunk.emit(Instruction::Duplicate);
        self.constant(Value::Undefined)?;
        self.chunk
            .emit(Instruction::Binary(BinaryOperator::StrictEqual));
        let given = self.chunk.emit_jump(Instruction::JumpIfFalse);
        self.chunk.emit(Instruction::Pop);
        match target {
            Some(name) => self.named_evaluation(name, default)?,
            None => self.expression(default)?,
        }
        self.chunk.patch(given)
    }

    /// Give one name the value on top of the stack, consuming it.
    pub(super) fn bind_name(&mut self, name: &str, how: Bind) -> Result<(), CompileError> {
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
}

/// Which kind of binding a pattern's names are being given — §14.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bind {
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

/// The name a destructuring target binds, when it binds exactly one — §8.6.3's named position.
///
/// `None` for a pattern, which binds several and none of them is *the* name, and for a property
/// reference, because §13.15.5 asks for an `IsIdentifierRef` and `o.p` is not one.
pub(super) trait BoundName {
    /// The single name, if there is one.
    fn bound_name(&self) -> Option<&str>;
}

impl BoundName for Binding {
    fn bound_name(&self) -> Option<&str> {
        match self {
            Self::Identifier(name) => Some(&name.name),
            Self::Pattern(_) => None,
        }
    }
}

impl BoundName for AssignmentTarget {
    fn bound_name(&self) -> Option<&str> {
        match self {
            Self::Simple(target) => match &target.kind {
                ExprKind::Identifier(name) => Some(name),
                _ => None,
            },
            Self::Pattern(_) => None,
        }
    }
}

/// The single name a target binds, for the two shapes a destructuring element's target can have.
pub(super) fn bound_name(target: &impl BoundName) -> Option<&str> {
    target.bound_name()
}
