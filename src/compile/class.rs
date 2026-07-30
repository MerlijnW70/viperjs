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
use super::CompileError;
use crate::ast::{Class, ClassElement, FormalParameters, Stmt};
use crate::compile::Compiler;
use crate::compile::chunk::{Chunk, Instruction};
use crate::span::Span;

impl Compiler<'_> {
    /// Evaluate a class and leave its constructor on the stack.
    ///
    /// The elements are walked once, in source order, because that order is observable: a computed
    /// key runs when it is reached, and two methods with the same key leave the later one in place.
    pub(super) fn class(&mut self, class: &Class, span: Span) -> Result<(), CompileError> {
        let mut fields: Vec<&crate::ast::ClassField> = Vec::new();
        // §15.7.14 keeps the static elements in one list, because a field and a block run in the
        // order they were written and nothing distinguishes them at that point.
        let mut statics: Vec<StaticElement<'_>> = Vec::new();
        for element in &class.elements {
            match element {
                // Taken in the element walk below, where the specification evaluates its name.
                ClassElement::Field(field) if field.is_static => {}
                ClassElement::Field(field) => fields.push(field),
                // Taken in the element walk below, in order with the static fields.
                ClassElement::StaticBlock(_) => {}
                ClassElement::Method(_) => {}
            }
        }

        // §15.7.14 steps 4 to 7 — the class body is a scope of its own, holding an **immutable**
        // binding for the class's own name. That is a different binding from the one a declaration
        // also creates outside, and the difference is observable: reassigning the outer name does not
        // change what a method sees, because the method closed over this one.
        //
        // It is also what lets a named class *expression* see itself, since an expression has no
        // outer binding at all.
        let mark = self.enter_scope();
        let mut inner = None;
        if let Some(name) = &class.name {
            let slot = self.declare_lexical(&name.name, true);
            self.chunk.emit(Instruction::Uninitialise(slot));
            inner = Some(slot);
        }

        // A computed instance-field name is evaluated once, at definition time, while its initialiser
        // runs per construction — so the one value has to outlive the definition. It goes in a
        // compiler temporary in *this* scope, and the constructor reaches it the way any closure
        // reaches an outer variable: the constructor is compiled inside this scope, so ordinary name
        // resolution finds the slot at whatever depth it turns out to be.
        //
        // The slots are reserved before the constructor is compiled, because the prologue inside it
        // has to resolve them; the values are stored during the element walk below, which is where
        // §15.7.14 evaluates the names. The class definition finishes before anything can construct,
        // so every slot is filled before a prologue reads it.
        // A slot for every field, not only the computed ones. Reserving them conditionally left a
        // branch no test could pin: with the condition inverted a missing slot does not fail loudly,
        // it falls back to a *global* of the same name and keeps working. One slot per field costs
        // nothing measurable and there is nothing left to get wrong.
        for at in 0..fields.len() {
            self.declare_shadowing(&field_name_slot(at));
        }

        // §15.7.14 step 8 — the heritage is evaluated **inside** the class scope and after the inner
        // name binding has been made but before it holds anything, so `class C extends C {}` is a
        // ReferenceError from the dead zone rather than a reference to an outer `C`. Its value is
        // left on the stack for `MakeClass`, which is where steps 9 to 11 read it three ways.
        if let Some(heritage) = &class.heritage {
            self.expression(heritage)?;
        }

        self.constructor(class, &fields, span)?;

        // Initialised to the constructor before any element is defined — which is the whole reason
        // the scope exists, and why the binding is uninitialised until now: a computed key evaluated
        // above this point is in the class name's temporal dead zone.
        // No `Duplicate` first: `Initialise` *peeks* its value rather than popping it — the same
        // terms as a store, so that `let a = b = 1` leaves the value for the outer binding — so the
        // constructor is written into the slot and stays on the stack for the methods below.
        if let Some(slot) = inner {
            self.chunk.emit(Instruction::Initialise(slot));
        }

        for element in &class.elements {
            if let ClassElement::Field(field) = element
                && !field.is_static
            {
                // Its name, now, where the specification evaluates it — every field's, not only a
                // computed one's, for the reason the slots are reserved unconditionally. A plain
                // name is a constant either way, so storing it changes nothing but removes a branch.
                let at = fields
                    .iter()
                    .position(|other| std::ptr::eq(*other, field))
                    .unwrap_or_default();
                self.property_key(&field.key)?;
                self.store_name(&field_name_slot(at))?;
                self.chunk.emit(Instruction::Pop);
                continue;
            }
            if let ClassElement::Field(field) = element
                && field.is_static
            {
                // The name, now, where the specification evaluates it — interleaved with the methods
                // and in source order. It is kept in a compiler temporary because the initialiser
                // that will use it does not run until every element is defined. The name has a space
                // in it so that no source can name the same slot.
                self.property_key(&field.key)?;
                let slot = self.declare_shadowing("static field name");
                self.chunk.emit(Instruction::StoreVariable(0, slot));
                self.chunk.emit(Instruction::Pop);
                statics.push(StaticElement::Field(field, slot));
                continue;
            }
            if let ClassElement::StaticBlock(block) = element {
                statics.push(StaticElement::Block(block));
                continue;
            }
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
            // §15.7.14's `MethodDefinitionEvaluation` calls `MakeMethod` with the object the method
            // is being put on — the prototype for an instance method and the constructor for a
            // static one, which is exactly what is under the key here. That is what makes `super.x`
            // in a static method read the *parent class* rather than its prototype.
            self.chunk.emit(Instruction::MakeMethod(2));
            self.chunk.emit(Instruction::DefineClassMethod(method.kind));
        }
        for element in &statics {
            match element {
                StaticElement::Field(field, slot) => self.static_field(field, *slot, span)?,
                StaticElement::Block(block) => self.static_block(block, span)?,
            }
        }
        self.leave_scope(mark);
        Ok(())
    }

    /// §15.7.14's `ClassStaticBlockDefinitionEvaluation` — a `static { … }` body, run once.
    ///
    /// The same shape as a static field's initialiser and for the same reason: it is compiled as a
    /// body of its own and *called* with the constructor as its receiver, because §15.7.14 binds
    /// `this` to the constructor and a call is the only thing that binds a receiver. What differs is
    /// that a block is a statement list rather than an expression, and defines no property — so its
    /// answer is discarded, and `return` is a Syntax Error the parser has already refused.
    fn static_block(
        &mut self,
        block: &crate::ast::ClassStaticBlock,
        span: Span,
    ) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::Duplicate);
        let parameters = FormalParameters {
            items: Box::new([]),
            rest: None,
            span,
        };
        let body = self.compile_nested(
            &parameters,
            Body::Statements(&block.body),
            Lexical::No,
            span,
        )?;
        self.emit_function(body, span)?;
        // §15.7.14 makes a static block a method of the *constructor*, which is the copy one below —
        // so `super.x` in one reads the parent class rather than its prototype.
        self.chunk.emit(Instruction::MakeMethod(1));
        self.chunk.emit(Instruction::CallMethod(0));
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// §15.7.14's `DefineField` for a `static` one — the half that runs after every element.
    ///
    /// Two things about a static field happen at different times, and that is the whole difficulty.
    /// Its **name** is evaluated during the walk over the elements, in source order and interleaved
    /// with the methods; its **initialiser** runs only once every element has been defined. A first
    /// attempt emitted both here and got `static [k(1)] = order.push(2)` the wrong way round.
    ///
    /// So the key was evaluated in the walk and left in a compiler temporary, and this reads it back.
    /// The initialiser is compiled as a body of its own and **called** with the constructor as its
    /// receiver, because §15.7.14 binds `this` to the constructor and a call is the only thing that
    /// binds a receiver — inline code would take whatever `this` the surrounding scope has, which is a
    /// wrong answer rather than a missing one.
    fn static_field(
        &mut self,
        field: &crate::ast::ClassField,
        slot: u32,
        span: Span,
    ) -> Result<(), CompileError> {
        // One copy is the property's target, which `DefineField` peeks; one is the receiver.
        self.chunk.emit(Instruction::Duplicate);
        match &field.initializer {
            Some(expression) => {
                self.chunk.emit(Instruction::Duplicate);
                let parameters = FormalParameters {
                    items: Box::new([]),
                    rest: None,
                    span,
                };
                // `Body::Expression` is evaluated and returned, which is what an initialiser is;
                // `Lexical::No` gives the body a `this` of its own for the call to bind.
                let body = self.compile_nested(
                    &parameters,
                    Body::Expression(expression),
                    Lexical::No,
                    span,
                )?;
                self.emit_function(body, span)?;
                // A static field's initialiser is a method of the constructor too, for the same
                // reason and with the constructor in the same place.
                self.chunk.emit(Instruction::MakeMethod(1));
                self.chunk.emit(Instruction::CallMethod(0));
            }
            // §15.7.14 — written without one it is `undefined`, and there is nothing to call.
            None => self.constant(crate::value::Value::Undefined)?,
        }
        // The key was evaluated long before this; only its arrangement happens now. `Bury(1)` puts it
        // under the value, in the order `DefineField` reads them.
        self.chunk.emit(Instruction::LoadVariable(0, slot));
        self.chunk.emit(Instruction::Bury(1));
        self.chunk.emit(Instruction::DefineField);
        self.chunk.emit(Instruction::Pop);
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
        // §15.7.14 step 15 — a derived class with no constructor written gets
        // `constructor(...args) { super(...args); }` rather than an empty one, because the parent has
        // to be given what the `new` was given. Synthesised as a rest parameter and a spread call so
        // that it is the *same* code path as a written one: an argument list assembled by hand here
        // would be a second implementation of §13.3.8 to keep in step with the first.
        let forwarding = derived_default(span);
        let (parameters, statements) = match (written.is_none() && class.heritage.is_some(), &forwarding) {
            (true, (parameters, statements)) => (parameters, statements.as_slice()),
            (false, _) => (parameters, statements),
        };
        let mut body = self.compile_nested(
            parameters,
            Body::Constructor {
                fields,
                statements,
                derived: class.heritage.is_some(),
            },
            Lexical::No,
            span,
        )?;
        // §15.7.14 — a class constructor has a `[[Construct]]` and no useful `[[Call]]`: written
        // without `new` it is a TypeError. The body has to carry that, because by the time a call
        // happens the only thing left is the chunk.
        body.class_constructor = true;
        self.emit_class(body, class.heritage.is_some(), span)
    }

    /// Push a constructor's body and emit [`Instruction::MakeClass`] for it.
    ///
    /// The sibling of [`Compiler::emit_function`], and separate for one reason: the instruction
    /// differs. Both have to carry an inner arrow's reach for `arguments` outward, which is what
    /// [`Compiler::file_function`] is doing.
    fn emit_class(
        &mut self,
        body: Chunk,
        derived: bool,
        span: Span,
    ) -> Result<(), CompileError> {
        let index = self.file_function(body, span)?;
        self.chunk.emit(Instruction::MakeClass {
            body: index,
            derived,
        });
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
        self.load_this();
        for (at, field) in fields.iter().enumerate() {
            // Read back out of the slot the walk filled, whatever kind of name it was. Choosing
            // between the slot and a fresh constant here would be a branch with no observable
            // difference for a plain name, since the two hold the same value.
            self.load_name(&field_name_slot(at))?;
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

/// §15.7.14 step 15's implicit constructor for a derived class — `(...args) => super(...args)`.
///
/// Built as a syntax tree rather than as bytecode, so that it goes through the one path a written
/// constructor goes through: the rest parameter is §15.1's, the spread is §13.3.8's, and neither is
/// implemented a second time here to be kept in step with the first. A first attempt that assembled
/// the argument list by hand is exactly how the two would come to disagree about a `Symbol.iterator`
/// somebody had replaced.
///
/// The parameter is named `%args`, which no source can spell, so the forwarding cannot be disturbed
/// by anything the class body declares — and there is no class body to disturb it, since this is only
/// used when no constructor was written.
fn derived_default(span: Span) -> (FormalParameters, Vec<Stmt>) {
    let name = crate::ast::BindingName {
        name: "%args".into(),
        span,
    };
    let parameters = FormalParameters {
        items: Box::new([]),
        rest: Some(Box::new(crate::ast::Binding::Identifier(name))),
        span,
    };
    let forward = crate::ast::Expr::new(
        crate::ast::ExprKind::Call {
            optional: false,
            callee: Box::new(crate::ast::Expr::new(crate::ast::ExprKind::Super, span)),
            arguments: Box::new([crate::ast::Argument::Spread(crate::ast::Expr::new(
                crate::ast::ExprKind::Identifier("%args".to_string()),
                span,
            ))]),
        },
        span,
    );
    (
        parameters,
        vec![Stmt {
            kind: crate::ast::StmtKind::Expression(Box::new(forward)),
            span,
        }],
    )
}

/// The name of the compiler temporary holding the `at`th instance field's evaluated key.
///
/// A `%` in front, which is the house convention for a slot no source can name — see
/// [`Compiler::declare_hidden`]. That helper is not used here for one reason: it builds its name from
/// the number of locals at the moment it is called, so the name cannot be worked out again
/// afterwards. This slot has to be found a second time, from inside the *nested* compiler that
/// builds the constructor, so the name is derived from the field's position instead. That way the
/// walk which fills the slot and the prologue which reads it cannot disagree about which one they
/// mean, without either having to be told.
fn field_name_slot(at: usize) -> String {
    format!("%class field name {at}")
}

/// One `static` element of a class body, in the order it was written.
///
/// §15.7.14 runs the static fields and the static blocks as one list, after every element has been
/// defined, so they cannot be gathered separately without losing the order between them.
enum StaticElement<'a> {
    /// `static x = 1;`, with the slot its evaluated name was left in.
    Field(&'a crate::ast::ClassField, u32),
    /// `static { … }`, which defines nothing and is run for its effects.
    Block(&'a crate::ast::ClassStaticBlock),
}
