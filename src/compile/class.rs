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
//! # What `extends` adds, and where
//!
//! Three things, and none of them is in this file's happy path. The heritage is evaluated *inside*
//! the class scope and left on the stack for [`Instruction::MakeClass`], which reads it three ways
//! (§15.7.14 steps 9 to 11). A derived class with no constructor written gets a forwarding one,
//! synthesised here as a syntax tree so that its rest parameter and its spread are §15.1's and
//! §13.3.8's rather than a second implementation of either. And a derived constructor's `this`
//! becomes a *binding* — DR-0015 — which is [`super::function`]'s doing, because the body compiler is
//! what declares it.
//!
//! # What is not here yet
//!
//! Private names. Refused by name rather than mis-compiled, because a class that silently dropped a
//! `#x` would be worse than one that would not compile.

use super::CompileError;
use super::function::{Asynchrony, Body, Lexical, Naming, Strict};
use crate::ast::{Class, ClassElement, FormalParameters, Stmt};
use crate::compile::Compiler;
use crate::compile::chunk::{Chunk, Instruction};
use crate::span::Span;

impl Compiler<'_> {
    /// Evaluate a class and leave its constructor on the stack.
    ///
    /// The elements are walked once, in source order, because that order is observable: a computed
    /// key runs when it is reached, and two methods with the same key leave the later one in place.
    pub(super) fn class(
        &mut self,
        class: &Class,
        naming: Naming<'_>,
        span: Span,
    ) -> Result<(), CompileError> {
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
        //
        // An environment of its own, and not merely a level of the one around it: the binding is a
        // scope's, and a scope that only the compiler can see is one a direct `eval` cannot — see
        // DR-0018. Opened only for a *named* class, because an anonymous one binds nothing a
        // program can write and the temporaries below are spelled with a `%` no source can.
        let opened = class.name.is_some().then(|| self.enter_environment());
        let mark = self.enter_scope();
        let mut inner = None;
        if let Some(name) = &class.name {
            let slot = self.declare_lexical(&name.name, crate::heap::Mutability::Const);
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

        // §9.2 — a `PrivateEnvironment` holding one Private Name per `#x` the body binds, created
        // **per evaluation** of the class. Here that is a slot per name in this scope, filled with a
        // fresh Symbol: the scope is a fresh environment each time the definition runs, so a class
        // evaluated twice has two sets of names and an instance of one is not a brand of the other.
        // Every `multiple-evaluations-of-class` test in the suite is about exactly that, and a
        // constant in the chunk would make them all pass by accident and be wrong.
        //
        // A method reaches these the way it reaches the class's own name — the scope chain — so
        // nothing has to be threaded into the nested bodies. And the slot name is derived from the
        // private name, so the definition and every `this.#x` in the body agree without being told.
        for name in private_names(class) {
            let slot = self.declare_shadowing(&private_name_slot(&name));
            let index = self.name(&name)?;
            self.chunk.emit(Instruction::NewPrivateName(index));
            self.chunk.emit(Instruction::StoreVariable(0, slot));
            self.chunk.emit(Instruction::Pop);
        }

        // A private method is **one** function object shared by every instance, made here once, with
        // each instance carrying an entry that points at it. So the function goes in a slot of this
        // scope too, and the constructor's prologue adds an entry per construction — reading the slot
        // through the scope chain, exactly as it reads the Private Name beside it.
        //
        // A getter and a setter are two class elements and **two** functions, so they take a slot each
        // — one private *element* is built from them at the definition, which is a separate matter
        // from where the functions live. A slot per written half, therefore, and this is the one place
        // that walks the elements themselves rather than the per-name list.
        for method in class.elements.iter().filter_map(|element| match element {
            ClassElement::Method(method) => Some(method),
            _ => None,
        }) {
            if let crate::ast::PropertyKey::Private(name) = &method.key {
                self.declare_shadowing(&private_function_slot(name, method.kind));
            }
        }

        // §15.7.14 step 8 — the heritage is evaluated **inside** the class scope and after the inner
        // name binding has been made but before it holds anything, so `class C extends C {}` is a
        // ReferenceError from the dead zone rather than a reference to an outer `C`. Its value is
        // left on the stack for `MakeClass`, which is where steps 9 to 11 read it three ways.
        if let Some(heritage) = &class.heritage {
            self.expression(heritage)?;
        }

        self.constructor(class, &fields, naming, span)?;

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
                // A private field has no `PropertyName` to evaluate — §15.7's `ClassElementName` is
                // one *or* a `PrivateIdentifier`, not both — and its Private Name was minted above.
                // So there is nothing to do here, and its `%class field name` slot stays unused.
                if matches!(field.key, crate::ast::PropertyKey::Private(_)) {
                    continue;
                }
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
                // A static *private* field has no `PropertyName` to evaluate and its Private Name
                // was minted above, so there is nothing to keep for later: the initialiser reads the
                // name slot by its own name. `u32::MAX` stands for "no key slot", which is honest
                // because the only thing that reads the slot is the branch that knows the difference.
                if matches!(field.key, crate::ast::PropertyKey::Private(_)) {
                    statics.push(StaticElement::Field(field, u32::MAX));
                    continue;
                }
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
            // A private method is not defined *on* anything: it goes in a slot, and the entry that
            // makes it reachable is added per instance by the constructor — or, for a static one, to
            // the constructor here and now. Either way there is no property to define, which is why
            // this leaves the walk before a target is pushed.
            if let crate::ast::PropertyKey::Private(name) = &method.key {
                self.private_method(method, name, span)?;
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
            // §10.2.9's name is set at run time from the key sitting under the function, for
            // *every* public method rather than only the ones the compiler cannot read. Two ways of
            // arriving at one name is two things that can disagree, and a guard choosing between
            // them was a branch no program could distinguish — the key on the stack and the key in
            // the source are the same key. `method_naming` still answers for a **private** method,
            // which leaves this walk above and keeps its `#`.
            self.make_method_function(&method.function, Naming::default(), true, span)?;
            // §15.7.14's `MethodDefinitionEvaluation` calls `MakeMethod` with the object the method
            // is being put on — the prototype for an instance method and the constructor for a
            // static one, which is exactly what is under the key here. That is what makes `super.x`
            // in a static method read the *parent class* rather than its prototype.
            self.chunk.emit(Instruction::MakeMethod(2));
            self.chunk
                .emit(Instruction::NameFunction(run_time_prefix(method.kind)));
            self.chunk.emit(Instruction::DefineClassMethod(method.kind));
        }
        // §15.7.14 — a static private element belongs to the constructor and to nothing else, and it
        // is added once the walk has made every function. Before the static *fields* run, because one
        // of those may call a static private method.
        let statics_private = private_elements(class, true);
        self.add_private_elements(&statics_private)?;

        for element in &statics {
            match element {
                StaticElement::Field(field, slot) => self.static_field(field, *slot, span)?,
                StaticElement::Block(block) => self.static_block(block, span)?,
            }
        }
        self.leave_scope(mark);
        if let Some(opened) = opened {
            self.leave_environment(opened)?;
        }
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
            Naming::default(),
            // §15.7.1 — every part of a class definition is strict code, and a static block has no
            // directive of its own to say so. Inherited, and the class scope is where it comes from.
            Strict::Inherited,
            Lexical::No,
            Asynchrony::No,
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
                //
                // The naming goes to the *expression* inside, not to this wrapper: the wrapper is
                // ViperJS's own and no program can see it, while §8.6.3 names whatever the initialiser
                // evaluates to. So the wrapper stays anonymous and `named_evaluation` runs inside it.
                let body = self.compile_nested(
                    &parameters,
                    Body::Expression(expression),
                    Naming::default(),
                    Strict::Inherited,
                    Lexical::No,
                    Asynchrony::No,
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
        match &field.key {
            // A private one's name is the Private Name in the class scope, minted at the definition
            // rather than evaluated during the walk — so there is no temporary to read.
            crate::ast::PropertyKey::Private(name) => self.load_private_name(name)?,
            _ => self.chunk.emit(Instruction::LoadVariable(0, slot)),
        }
        self.chunk.emit(Instruction::Bury(1));
        self.chunk.emit(match field.key {
            crate::ast::PropertyKey::Private(_) => Instruction::DefinePrivateField,
            _ => Instruction::DefineField,
        });
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
        naming: Naming<'_>,
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
        let (parameters, statements) =
            match (written.is_none() && class.heritage.is_some(), &forwarding) {
                (true, (parameters, statements)) => (parameters, statements.as_slice()),
                (false, _) => (parameters, statements),
            };
        // §10.2.9 — a class's constructor *is* the class, so its name is the class's. An anonymous
        // class expression in a named position takes that name, which is why it arrives from outside.
        let mut body = self.compile_nested(
            parameters,
            Body::Constructor {
                fields,
                statements,
                derived: class.heritage.is_some(),
                private_methods: instance_private_method_names(class),
            },
            match &class.name {
                Some(written) => Naming::of(&written.name),
                None => naming,
            },
            // §15.7.1 — a class definition is strict whatever encloses it, so a constructor is too,
            // written directive or not. The parser has already set it on the body it parsed.
            Strict::Yes,
            Lexical::No,
            Asynchrony::No,
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
    fn emit_class(&mut self, body: Chunk, derived: bool, span: Span) -> Result<(), CompileError> {
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
            // A private field's name is the Private Name in the class scope, not a property key —
            // the element walk never evaluated one for it, because §15.7 gives it no `PropertyName`
            // to evaluate. Everything after this is the same shape either way.
            match &field.key {
                crate::ast::PropertyKey::Private(name) => {
                    self.load_name(&private_name_slot(name))?;
                }
                _ => self.load_name(&field_name_slot(at))?,
            }
            match &field.initializer {
                // §8.6.3 — `FieldDefinition : ClassElementName Initializer` is a named position, so
                // `class C { x = () => {} }` calls the arrow `x`. A private field's name keeps its `#`.
                Some(expression) => match field_naming(field) {
                    Some(name) => self.named_evaluation(&name, expression)?,
                    // §15.7.10 step 2.g carries the evaluated `ClassElementName` to the initialiser
                    // as `[[ClassFieldInitializerName]]`, so a computed key names an anonymous
                    // definition here exactly as a written one does. The key is already on the stack
                    // — it was loaded a few lines up — which is the shape `NameFunction` reads.
                    None => {
                        self.expression(expression)?;
                        if super::expression::is_anonymous_definition(expression) {
                            self.chunk
                                .emit(Instruction::NameFunction(crate::compile::NamePrefix::Plain));
                        }
                    }
                },
                // §15.7.14 — a field written without one is `undefined`, which is not the same as
                // the field being absent: `x;` makes an own property and `for...in` finds it.
                None => self.constant(crate::value::Value::Undefined)?,
            }
            // §7.3.29 rather than `CreateDataPropertyOrThrow`: a private element is not a property,
            // so nothing that walks properties will find it.
            self.chunk.emit(match field.key {
                crate::ast::PropertyKey::Private(_) => Instruction::DefinePrivateField,
                _ => Instruction::DefineField,
            });
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

impl Compiler<'_> {
    /// Make a private method or accessor and put it where whoever needs it will look.
    ///
    /// Two destinations, and the keyword decides. A **static** one belongs to the constructor and
    /// nothing else ever carries it, so its entry is added here, where the constructor is on the
    /// stack. An **instance** one is added per construction, so the function is only stored, and
    /// [`Compiler::instance_private_methods`] is what emits those adds.
    fn private_method(
        &mut self,
        method: &crate::ast::ClassMethod,
        name: &str,
        span: Span,
    ) -> Result<(), CompileError> {
        // §15.7.14 gives a private method a `[[HomeObject]]` like any other, and it is the object the
        // method is *conceptually* defined on rather than one it is reachable through: the **prototype**
        // for an instance method, the constructor for a static one. That is where `super` starts
        // looking, which is the only thing a home decides — so "a private method is on neither object"
        // is true and is not an argument for either answer. Getting it wrong sent `super.m()` inside a
        // private method to the parent *class* instead of its prototype, and the conformance run said so.
        self.chunk.emit(Instruction::Duplicate);
        if !method.is_static {
            self.chunk.emit(Instruction::ClassPrototype);
        }
        // §10.2.9 — a private method is named with its `#`, which is part of the name.
        self.make_method_function(&method.function, method_naming(method), true, span)?;
        self.chunk.emit(Instruction::MakeMethod(1));
        self.store_private_slot(&private_function_slot(name, method.kind))?;
        // The function and the home, both of which have served their purpose.
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// §15.7.14 — add one private element per *name* to whatever is on top of the stack.
    ///
    /// Per name and not per element, which is the whole correction: a getter and a setter are two class
    /// elements and **one** private element, built here at the definition. Adding each half separately
    /// and merging them at run time let the same accessor be added to one object twice, where §7.3.30
    /// step 2 refuses a name that is already there — and a re-entered constructor reaches exactly that.
    fn add_private_elements(
        &mut self,
        elements: &[(Box<str>, PrivateKind)],
    ) -> Result<(), CompileError> {
        for (name, kind) in elements {
            self.load_private_name(name)?;
            match kind {
                PrivateKind::Method => {
                    self.load_private_slot(&private_function_slot(
                        name,
                        crate::ast::MethodKind::Normal,
                    ))?;
                    self.chunk.emit(Instruction::AddPrivateMethod);
                }
                // A half nobody wrote is `undefined`, and then that direction is a TypeError — which
                // is where a private accessor differs from a public one.
                PrivateKind::Accessor { getter, setter } => {
                    for (present, half) in [
                        (*getter, crate::ast::MethodKind::Get),
                        (*setter, crate::ast::MethodKind::Set),
                    ] {
                        match present {
                            true => self.load_private_slot(&private_function_slot(name, half))?,
                            false => self.constant(crate::value::Value::Undefined)?,
                        }
                    }
                    self.chunk.emit(Instruction::AddPrivateAccessor);
                }
            }
        }
        Ok(())
    }

    /// §15.7.14's `InitializeInstanceElements` steps 1 and 2 — the methods, before the fields.
    ///
    /// Before, and the order is observable: a field initialiser may call a private method, and
    /// §15.7.14 adds every method to the instance before evaluating any field.
    pub(super) fn instance_private_methods(
        &mut self,
        methods: &[(Box<str>, PrivateKind)],
    ) -> Result<(), CompileError> {
        // No early return for a class with none, on exactly the terms [`Compiler::instance_fields`]
        // states below: `LoadThis` followed by `Pop` leaves the stack as it was, so no input can tell
        // the shortcut from its absence — and a branch nothing can pin is worse than one that was
        // never written. Mutation coverage reported it, as it did for the fields.
        self.load_this();
        self.add_private_elements(methods)?;
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }
}

/// The private methods and accessors a class body binds, in source order, instance ones only.
///
/// Owned names rather than references into the tree, because they have to reach the *constructor's*
/// compiler and a borrow of the syntax tree cannot — the compiler's lifetime is the heap's. The same
/// reason the field initialisers became a compiled body rather than a stored list.
pub(super) fn instance_private_method_names(class: &Class) -> Vec<(Box<str>, PrivateKind)> {
    private_elements(class, false)
}

/// The private methods and accessors whose `static` matches, one entry per **name**.
///
/// One per name because that is what §15.7.14 builds: `get #a` and `set #a` are two class elements and
/// one private element, and which halves were written is decided here rather than by merging two adds
/// at run time.
fn private_elements(class: &Class, is_static: bool) -> Vec<(Box<str>, PrivateKind)> {
    let mut elements: Vec<(Box<str>, PrivateKind)> = Vec::new();
    for method in class.elements.iter().filter_map(|element| match element {
        ClassElement::Method(method) if method.is_static == is_static => Some(method),
        _ => None,
    }) {
        let crate::ast::PropertyKey::Private(name) = &method.key else {
            continue;
        };
        let (getter, setter) = match method.kind {
            crate::ast::MethodKind::Normal => {
                elements.push((name.clone(), PrivateKind::Method));
                continue;
            }
            crate::ast::MethodKind::Get => (true, false),
            crate::ast::MethodKind::Set => (false, true),
        };
        // The other half, if it was written first. §15.7.1 has already refused any duplicate that is
        // not a getter/setter pair, so an existing entry under this name is that pair.
        match elements.iter_mut().find(|(held, _)| held == name) {
            Some((
                _,
                PrivateKind::Accessor {
                    getter: held_getter,
                    setter: held_setter,
                },
            )) => {
                *held_getter |= getter;
                *held_setter |= setter;
            }
            _ => elements.push((name.clone(), PrivateKind::Accessor { getter, setter })),
        }
    }
    elements
}

/// Which halves a private element has — §7.3.30's kind, decided at the class definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrivateKind {
    /// `#m() {}`, which is one function and refuses assignment.
    Method,
    /// `get #a` and `set #a`, which are one element however many of the two were written.
    Accessor {
        /// Whether a getter was written; reading without one is a TypeError.
        getter: bool,
        /// Whether a setter was written; writing without one is a TypeError.
        setter: bool,
    },
}

/// The slot holding the function for `#name`, told apart by which half of an accessor it is.
///
/// A getter and a setter are one private element and two functions, so they cannot share a slot. The
/// name is derived from the private name and the kind, so the definition and the prologue agree
/// without either being told.
fn private_function_slot(name: &str, kind: crate::ast::MethodKind) -> String {
    let half = match kind {
        crate::ast::MethodKind::Normal => "method",
        crate::ast::MethodKind::Get => "getter",
        crate::ast::MethodKind::Set => "setter",
    };
    format!("%private {half} #{name}")
}

/// §8.6.3's name for a class field's initialiser, or `None` when there is not one to give.
///
/// Owned, because a private field's is its name with a `#` put back in front — the AST holds the name
/// without it, `#` being punctuation of the production.
fn field_naming(field: &crate::ast::ClassField) -> Option<String> {
    match &field.key {
        crate::ast::PropertyKey::Identifier(name) => Some(name.to_string()),
        crate::ast::PropertyKey::Private(name) => Some(format!("#{name}")),
        _ => None,
    }
}

/// §10.2.9's prefix for a class element whose key is computed.
///
/// Only the three public kinds: a *private* name is never computed — §15.7's `ClassElementName` is a
/// `PropertyName` **or** a `PrivateIdentifier` and the two are different productions — so the `#`
/// forms `method_naming` spells cannot arrive here.
fn run_time_prefix(kind: crate::ast::MethodKind) -> crate::compile::NamePrefix {
    match kind {
        crate::ast::MethodKind::Normal => crate::compile::NamePrefix::Plain,
        crate::ast::MethodKind::Get => crate::compile::NamePrefix::Get,
        crate::ast::MethodKind::Set => crate::compile::NamePrefix::Set,
    }
}

/// §10.2.9's name for a class method, with the accessor prefix that is part of it.
///
/// `None` for a **computed** key, whose name is not known while compiling — it is set at run time
/// from the key on the stack instead, which is what `Instruction::NameFunction` is for.
fn method_naming(method: &crate::ast::ClassMethod) -> Naming<'_> {
    // §10.2.9 — a private method's `#` is part of its name: `#m` and `get #a`. The AST holds the name
    // without it, the `#` being punctuation of the production, so it comes back here.
    let private = matches!(method.key, crate::ast::PropertyKey::Private(_));
    let prefix = match (method.kind, private) {
        (crate::ast::MethodKind::Normal, false) => None,
        (crate::ast::MethodKind::Normal, true) => Some("#"),
        (crate::ast::MethodKind::Get, false) => Some("get "),
        (crate::ast::MethodKind::Get, true) => Some("get #"),
        (crate::ast::MethodKind::Set, false) => Some("set "),
        (crate::ast::MethodKind::Set, true) => Some("set #"),
    };
    let name = match &method.key {
        crate::ast::PropertyKey::Identifier(name) | crate::ast::PropertyKey::Private(name) => {
            Some(&**name)
        }
        // A computed key's name is whatever the expression came to at run time, so it is not known
        // here and falls to §10.2.9's empty string.
        //
        // **That is a divergence and not a reading.** §15.4.5 runs `SetFunctionName(closure,
        // propKey)` with the *evaluated* key, so `class A { ["id"]() {} }` should name the method
        // `"id"` and a Symbol key `"[description]"`; ViperJS answers `""` for both. 36 runs measure
        // it, most of them `language/expressions/object` rather than classes, and the accessors
        // want their `get `/`set ` prefix with it. Fixing it means naming at run time from the key
        // already on the stack, not finding a better answer here.
        _ => None,
    };
    Naming { name, prefix }
}

/// Every private name the class body binds, in source order and without duplicates.
///
/// §15.7.1 has already refused a duplicate that is not a getter/setter pair, so a name may appear
/// twice legitimately and needs one slot either way.
fn private_names(class: &Class) -> Vec<Box<str>> {
    let mut names: Vec<Box<str>> = Vec::new();
    for element in &class.elements {
        let key = match element {
            ClassElement::Field(field) => &field.key,
            ClassElement::Method(method) => &method.key,
            ClassElement::StaticBlock(_) => continue,
        };
        if let crate::ast::PropertyKey::Private(name) = key
            && !names.iter().any(|seen| seen == name)
        {
            names.push(name.clone());
        }
    }
    names
}

/// The name of the compiler temporary holding the Private Name for `#name` — §9.2's environment.
///
/// A `%` in front, which is the house convention for a slot no source can spell. Derived from the
/// private name rather than from a position, because both sides have the name and neither has the
/// other's count: the class definition fills the slot and a `this.#x` anywhere in the body reads it,
/// including from inside a nested method whose compiler resolves it through the scope chain.
pub(super) fn private_name_slot(name: &str) -> String {
    format!("%private #{name}")
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
