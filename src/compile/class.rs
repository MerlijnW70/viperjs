//! §15.7 `ClassDefinitionEvaluation` — a class body, compiled.
//!
//! # What a class is, once the syntax is gone
//!
//! Two objects made together: a constructor function and the prototype its instances inherit from,
//! each holding a reference to the other. Everything else in §15.7.14 is putting methods on one or
//! the other. That pairing is why [`Instruction::MakeClass`] is one instruction rather than a
//! sequence — neither half is observable on its own, so there is no intermediate state worth
//! spelling out in bytecode.
//!
//! # The one runtime difference from an object literal
//!
//! A method here is **not enumerable**. §15.4.5 makes an object literal's methods enumerable and
//! §15.7.14 does not, so `for (k in new C)` finds nothing at all. That single attribute is the whole
//! of what [`Instruction::DefineClassMethod`] adds over [`Instruction::DefineField`], and it is why
//! the two cannot share an instruction.
//!
//! # What is not here yet
//!
//! Fields and static blocks, `extends` and `super`, and private names. Each is refused by name
//! rather than mis-compiled, because a class that silently dropped its fields would be worse than
//! one that would not compile.

use super::function::{Body, Lexical};
use super::{CompileError, ErrorKind, unsupported};
use crate::ast::{Class, ClassElement, FormalParameters, PropertyKey as AstPropertyKey, Stmt};
use crate::compile::Compiler;
use crate::compile::chunk::{Chunk, Instruction};
use crate::span::Span;
use std::rc::Rc;

impl Compiler<'_> {
    /// Evaluate a class and leave its constructor on the stack.
    ///
    /// The elements are walked once, in source order, because that order is observable: a computed
    /// key runs when it is reached, and two methods with the same key leave the later one in place.
    pub(super) fn class(&mut self, class: &Class, span: Span) -> Result<(), CompileError> {
        if class.heritage.is_some() {
            return Err(unsupported("a class with `extends`", span));
        }
        let mut fields: Vec<&crate::ast::ClassField> = Vec::new();
        for element in &class.elements {
            match element {
                ClassElement::Field(field) if field.is_static => {
                    // A static field's initialiser is evaluated with `this` bound to the
                    // *constructor*, at definition time. Emitting it inline here would give it
                    // whatever `this` the surrounding scope has, which is a wrong answer rather than
                    // a missing one — so it is refused until there is somewhere to bind it.
                    return Err(unsupported("a static class field", field.key_span));
                }
                ClassElement::Field(field) => match &field.key {
                    AstPropertyKey::Computed(_) => {
                        // A computed name is evaluated once, at definition time, while the
                        // initialiser runs per construction. Keeping that one value where the
                        // constructor can find it needs §15.7.14's `[[Fields]]` on the object.
                        return Err(unsupported("a computed class field name", field.key_span));
                    }
                    _ => fields.push(field),
                },
                ClassElement::StaticBlock(block) => {
                    return Err(unsupported("a class static block", block.span));
                }
                ClassElement::Method(_) => {}
            }
        }

        self.constructor(class, &fields, span)?;

        for element in &class.elements {
            let ClassElement::Method(method) = element else {
                continue;
            };
            if element.is_constructor() {
                continue;
            }
            // The target: a copy of the constructor for a static method, its prototype otherwise.
            // Duplicated rather than reloaded, because the class is not bound to any name yet — the
            // stack is the only place it exists.
            self.chunk.emit(Instruction::Duplicate);
            if !method.is_static {
                self.chunk.emit(Instruction::ClassPrototype);
            }
            self.property_key(&method.key)?;
            self.make_function(&method.function, span)?;
            self.chunk.emit(Instruction::DefineClassMethod(method.kind));
        }
        Ok(())
    }

    /// Compile the constructor and emit the instruction that makes the class from it.
    ///
    /// A class without one still has a constructor — §15.7.14 gives it an empty one — and that is
    /// synthesised here as an AST rather than special-cased in the machine. One path through
    /// [`Compiler::compile_nested`] means the implicit constructor is scoped, counted and bounded
    /// exactly as a written one is, instead of being a second implementation that has to be kept in
    /// step with the first.
    fn constructor(
        &mut self,
        class: &Class,
        fields: &[&crate::ast::ClassField],
        span: Span,
    ) -> Result<(), CompileError> {
        let written = class.elements.iter().find_map(|element| match element {
            ClassElement::Method(method) if element.is_constructor() => Some(&*method.function),
            _ => None,
        });
        // A class with no constructor written still has one — §15.7.14 gives it an empty one — and
        // what the body compiler actually needs is a parameter list and a statement list, not a
        // whole `Function`. Synthesising one meant carrying `is_async` and `is_generator` that
        // nothing reads: §15.7.1 makes `async constructor` and `*constructor` early errors, so the
        // parser refuses both before a compiler sees them, and mutation coverage duly reported the
        // two literals as untestable. Two references instead, and there is nothing left to be wrong.
        let empty = FormalParameters {
            items: Box::new([]),
            rest: None,
            span,
        };
        let (parameters, statements): (&FormalParameters, &[Stmt]) = match written {
            Some(function) => (&function.parameters, &function.body),
            None => (&empty, &[]),
        };
        let mut body = self.compile_nested(
            parameters,
            Body::Constructor { fields, statements },
            Lexical::No,
            span,
        )?;
        // §15.7.14 — a class constructor has a `[[Construct]]` and no useful `[[Call]]`: written
        // without `new` it is a TypeError. The body has to carry that, because by the time a call
        // happens the only thing left is the chunk.
        body.class_constructor = true;
        self.emit_class(body, span)
    }

    /// Push a constructor's body and emit [`Instruction::MakeClass`] for it.
    ///
    /// The sibling of [`Compiler::emit_function`], and separate for one reason: the instruction
    /// differs. Both have to carry an inner arrow's reach for `arguments` outward, which is what the
    /// `|=` is doing.
    fn emit_class(&mut self, body: Chunk, span: Span) -> Result<(), CompileError> {
        let index = u32::try_from(self.chunk.functions.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        self.uses_arguments |= body.outer_arguments;
        self.chunk.functions.push(Rc::new(body));
        self.chunk.emit(Instruction::MakeClass(index));
        Ok(())
    }

    /// §15.7.14's `InitializeInstanceElements`, as the prologue of a constructor.
    ///
    /// `CreateDataPropertyOrThrow` and **not** an assignment. That is the whole reason these are
    /// instructions rather than synthesised `this.x = e` statements: an assignment is `[[Set]]`, which
    /// calls an inherited setter, and a field must shadow one instead. A class whose field shares a
    /// name with a prototype setter is where the two answers differ.
    ///
    /// `this` is loaded once and left in place, because `DefineField` peeks its base — an object
    /// literal defines one property after another the same way.
    pub(super) fn instance_fields(
        &mut self,
        fields: &[&crate::ast::ClassField],
    ) -> Result<(), CompileError> {
        // No early return for a class without fields. Skipping the two instructions would be an
        // optimisation and not a semantic: `LoadThis` followed by `Pop` leaves the stack as it was,
        // so no input can tell the shortcut from its absence — mutation coverage reported exactly
        // that. Two instructions per construction is not a cost a benchmark has complained about,
        // and a branch nothing can pin is worse than one that was never written.
        self.chunk.emit(Instruction::LoadThis);
        for field in fields {
            self.property_key(&field.key)?;
            match &field.initializer {
                Some(expression) => self.expression(expression)?,
                // §15.7.14 — a field written without one is `undefined`, which is not the same as
                // the field being absent: `x;` makes an own property and `for...in` finds it.
                None => self.constant(crate::value::Value::Undefined)?,
            }
            self.chunk.emit(Instruction::DefineField);
        }
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }
}
