//! §13 — expressions, each of which leaves exactly one value on the stack.
//!
//! The one invariant every function here keeps: compiling an expression adds one value and
//! consumes whatever it needed. That is what lets a statement be stack-neutral by construction
//! and what [`crate::vm::Fault::UnbalancedStack`] checks at the end of a chunk.

use super::function::{Keep, Naming};
use super::{
    CompileError, Compiler, ErrorKind, Instruction, MAX_EXPRESSION_DEPTH, ShortCircuit, unsupported,
};
use crate::ast::ArrayElement;
use crate::ast::MethodKind;
use crate::ast::PropertyKey as AstPropertyKey;
use crate::ast::{
    Argument, AssignmentOperator, AssignmentTarget, BinaryOperator, Expr, ExprKind,
    LogicalOperator, PropertyDefinition, TemplateElement, TemplateLiteral, UnaryOperator,
    UpdateOperator,
};
use crate::span::Span;
use crate::value::Value;

impl Compiler<'_> {
    /// Compile `expression` so that it leaves exactly one value on the stack.
    pub(super) fn expression(&mut self, expression: &Expr) -> Result<(), CompileError> {
        let span = expression.span;
        // The tree is walked by recursing, so a nested expression is as deep as it is nested and
        // the Rust stack is what runs out. Counting is DR-0006's argument again: a count is
        // portable and a measurement of the remaining stack is not.
        //
        // One increment and one decrement, with the refusal *between* them rather than beside
        // them. Written with an early return it needed a second decrement on the error path — a
        // line that could be anything at all, since a failed compilation is discarded whole and
        // nothing ever reads the depth again.
        self.depth += 1;
        let compiled = if self.depth > MAX_EXPRESSION_DEPTH {
            Err(CompileError {
                kind: ErrorKind::TooDeep,
                span,
            })
        } else {
            self.expression_inner(expression, span)
        };
        self.depth -= 1;
        compiled
    }
    /// Compile `expression`, with the depth already counted.
    fn expression_inner(&mut self, expression: &Expr, span: Span) -> Result<(), CompileError> {
        match &expression.kind {
            // The literals. `undefined` is not among them because it is not a literal: it is an
            // identifier that happens to resolve to a property of the global object, which is why
            // `void 0` exists and why minifiers use it.
            ExprKind::Null => self.constant(Value::Null),
            ExprKind::Boolean(value) => self.constant(Value::Boolean(*value)),
            ExprKind::Number(value) => self.constant(Value::Number(*value)),
            ExprKind::String(units) => {
                let id = self.heap.new_string(units.to_vec());
                self.constant(Value::String(id))
            }
            ExprKind::Unary { operator, argument } => {
                // §13.5.1 — `delete` does not evaluate its operand to a *value*; it wants the
                // reference. For a property that is the base and the key, and for anything else
                // there is nothing to delete.
                if *operator == UnaryOperator::Delete {
                    return self.delete(argument, span);
                }
                // §13.5.1.1 step 2 — `typeof` is the one operator that takes an *unresolvable*
                // reference and answers instead of throwing. `typeof JSON !== "undefined"` is
                // how a program asks whether something exists at all, and it is in test262's own
                // harness; evaluating the operand first would turn the question into the very
                // error it was written to avoid. Only a bare name can be unresolvable, so only a
                // bare name takes this path.
                if *operator == UnaryOperator::Typeof
                    && let ExprKind::Identifier(name) = &argument.kind
                    && self.binding(name).is_none()
                {
                    let index = self.name(name)?;
                    self.chunk.emit(Instruction::TypeofGlobal(index));
                    return Ok(());
                }
                self.expression(argument)?;
                self.chunk.emit(Instruction::Unary(*operator));
                Ok(())
            }
            // §13.15 — a binary operator, and its whole left-leaning chain at once.
            //
            // `a + b + c` is `((a + b) + c)`, so a chain is a tree as deep as it is long, and
            // minified code chains thousands of terms. Recursing once per term would run out of
            // Rust stack long before the source ran out of terms — which is exactly how the
            // parser handles the same shape, with a loop rather than recursion (DR-0006).
            //
            // So the left spine is walked flat: down to the innermost operand, then back out
            // emitting each right operand and its operator. The order is unchanged, which is what
            // matters — §13.15.1 evaluates the left operand first, and that is what the emitted
            // order still says.
            ExprKind::Binary { .. } => {
                let mut spine = Vec::new();
                let mut innermost = expression;
                while let ExprKind::Binary {
                    operator,
                    left,
                    right,
                } = &innermost.kind
                {
                    spine.push((*operator, &**right));
                    innermost = left;
                }
                self.expression(innermost)?;
                for (operator, right) in spine.into_iter().rev() {
                    self.expression(right)?;
                    // Two of them are not value operations: they ask an object a question
                    // rather than converting one, so each is an instruction of its own rather
                    // than a row in `apply_binary`.
                    match operator {
                        BinaryOperator::In => self.chunk.emit(Instruction::HasProperty),
                        BinaryOperator::Instanceof => self.chunk.emit(Instruction::Instanceof),
                        _ => self.chunk.emit(Instruction::Binary(operator)),
                    }
                }
                Ok(())
            }
            // §13.13 and §13.14 — the operators that may not evaluate their right operand at all.
            // The left one is evaluated, looked at, and *kept* if it decides the answer: `a || b` is
            // `a` itself when `a` is truthy, not `true`.
            ExprKind::Logical {
                operator,
                left,
                right,
            } => {
                let condition = match operator {
                    LogicalOperator::And => ShortCircuit::WhenFalsy,
                    LogicalOperator::Or => ShortCircuit::WhenTruthy,
                    LogicalOperator::NullishCoalescing => ShortCircuit::WhenNotNullish,
                };
                self.expression(left)?;
                let over_the_right = self
                    .chunk
                    .emit_jump(|target| Instruction::JumpKeeping(condition, target));
                self.expression(right)?;
                self.chunk.patch(over_the_right)
            }
            // §13.14 — the conditional operator, where the test *is* thrown away and exactly one of
            // the two branches runs.
            ExprKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test)?;
                let to_alternate = self.chunk.emit_jump(Instruction::JumpIfFalse);
                self.expression(consequent)?;
                let past_the_alternate = self.chunk.emit_jump(Instruction::Jump);
                self.chunk.patch(to_alternate)?;
                self.expression(alternate)?;
                self.chunk.patch(past_the_alternate)
            }
            // §13.16 — the comma operator: evaluate each, keep the last. Every earlier value is
            // discarded, which is the only reason it is ever written.
            ExprKind::Sequence(expressions) => {
                let Some((last, earlier)) = expressions.split_last() else {
                    // A comma expression with no operands has no production. The parser cannot build
                    // one; if the tree ever holds one, an empty chunk would leave the VM with an
                    // unbalanced stack, so saying so here is the honest answer.
                    return Err(unsupported("an empty comma expression", span));
                };
                for expression in earlier {
                    self.expression(expression)?;
                    self.chunk.emit(Instruction::Pop);
                }
                self.expression(last)
            }
            // §13.2.5 — an object literal.
            ExprKind::Object(properties) => self.object_literal(properties, span),
            // §13.3.2 and §13.3.3 — `o.x` and `o[k]`. The name is a constant where the key is
            // an expression, which is the whole difference between the two forms.
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                let reference = self.property_reference(expression, Keep::Nothing)?;
                self.chunk.emit(reference.get());
                Ok(())
            }
            // Everything else, named as the specification names it so that the message says which
            // clause is missing rather than which Rust variant.
            // §13.1.3 — a name is a slot when the compiler can place it and a property of the
            // global object when it cannot. Which of the two is decided here; whether the global
            // is *there* is decided at run time, because a script can make one at any moment.
            ExprKind::Identifier(name) => self.load_name(name),
            // §13.2.1 — `this`, which the call decided and the frame is holding, unless DR-0015
            // moved it into a binding because a derived constructor can change it.
            ExprKind::This => {
                self.load_this();
                Ok(())
            }
            // §13.10.1 — `#x in o`, the one way to ask whether an object carries a private field
            // without §7.3.31's TypeError. The name is not an expression and has no production that
            // would let it stand alone, which is why it is part of this node.
            ExprKind::PrivateIn { name, object } => {
                self.expression(object)?;
                self.load_private_name(name)?;
                self.chunk.emit(Instruction::HasPrivate);
                Ok(())
            }
            ExprKind::BigInt(_) => Err(unsupported("a BigInt literal", span)),
            ExprKind::Call {
                optional,
                callee,
                arguments,
            } => {
                if *optional {
                    return Err(unsupported("optional chaining", span));
                }
                self.call(callee, arguments, span)
            }
            // §13.3.5 — `new f(a)`. The callee is an ordinary expression and never a method:
            // `new o.m()` constructs with `o.m`, and the `o` plays no part.
            ExprKind::New { callee, arguments } => {
                self.expression(callee)?;
                // §13.3.5 with a spread: gathered into one array and expanded by the call, exactly as
                // an ordinary call's are. `new` differs only in who makes the receiver.
                if arguments
                    .iter()
                    .any(|argument| matches!(argument, Argument::Spread(_)))
                {
                    self.argument_array(arguments)?;
                    self.chunk.emit(Instruction::CallSpread(
                        crate::compile::chunk::SpreadCall::Construct,
                    ));
                    return Ok(());
                }
                for argument in arguments.iter() {
                    let Argument::Value(value) = argument else {
                        return Err(unsupported("a spread argument", span));
                    };
                    self.expression(value)?;
                }
                let count = u32::try_from(arguments.len()).map_err(|_| CompileError {
                    kind: ErrorKind::TooLong,
                    span,
                })?;
                self.chunk.emit(Instruction::Construct(count));
                Ok(())
            }
            // §13.4 — `++a` and `a++`, which differ in what they *evaluate to* and in nothing
            // else. Both read, coerce, add one and write back.
            ExprKind::Update {
                operator,
                prefix,
                argument,
            } => self.update(*operator, *prefix, argument, span),
            // §13.15 — assignment, whose *value* is the value assigned. That is why the store
            // leaves it on the stack rather than taking it: `a = b = 1` and `f(a = 1)` both
            // need it.
            ExprKind::Assignment {
                operator,
                target,
                value,
            } => {
                let AssignmentTarget::Simple(target) = &**target else {
                    // §13.15.5 — a destructuring assignment. Only `=` has one: `[a] += b` is a
                    // Syntax Error the parser has already refused, so there is no operator to
                    // apply and the value is simply taken apart.
                    let AssignmentTarget::Pattern(pattern) = &**target else {
                        return Err(unsupported("a destructuring assignment", span));
                    };
                    self.expression(value)?;
                    // §13.15.2 — the *value* of the assignment is what was assigned, and a
                    // pattern consumes it. So a copy is kept underneath for whatever wanted the
                    // expression's value.
                    self.chunk.emit(Instruction::Duplicate);
                    return self.assign_pattern(pattern, span);
                };
                if let ExprKind::Member { .. } | ExprKind::ComputedMember { .. } = &target.kind {
                    return self.assign_to_property(*operator, target, value, span);
                }
                let ExprKind::Identifier(name) = &target.kind else {
                    return Err(unsupported(
                        "an assignment to something that is not a name",
                        target.span,
                    ));
                };
                match compound_operator(*operator) {
                    // `a = v`, which is one of §8.6.3's named positions: `f = function () {}` gives
                    // the function the name `f`. Only a plain `=` — `f ||= function () {}` does not
                    // name it, because §13.15.2's compound forms are not in the list.
                    None if *operator == AssignmentOperator::Assign => {
                        self.named_evaluation(name, value)?;
                    }
                    // `a += v` is `a = a + v`, with `a` read once — which matters not at all for
                    // a slot and matters a great deal for `o[f()] += 1`, where the property key
                    // is evaluated once. The shape is the same either way.
                    //
                    // The read is an ordinary one, so `undeclared += 1` is a ReferenceError from
                    // the *read* — §13.15.2 evaluates the target's value before the operator.
                    Some(binary) => {
                        self.load_name(name)?;
                        self.expression(value)?;
                        self.chunk.emit(Instruction::Binary(binary));
                    }
                    // §13.15.2 — `a ||= v` and its two siblings assign *only* when the short circuit
                    // does not fire, so the store is inside the jump rather than after the match. The
                    // value the whole expression answers is the old one when it fires and the new one
                    // when it does not, which is exactly what `JumpKeeping` leaves behind.
                    None if short_circuit_operator(*operator).is_some() => {
                        let condition = match short_circuit_operator(*operator) {
                            Some(condition) => condition,
                            // Unreachable: the guard above just asked. Written out rather than
                            // unwrapped because a panic here would be an engine bug the types cannot
                            // encode, and there is a real answer to give instead.
                            None => return Err(unsupported("a logical assignment", span)),
                        };
                        self.load_name(name)?;
                        let over_the_store = self
                            .chunk
                            .emit_jump(|target| Instruction::JumpKeeping(condition, target));
                        // No `Pop` for the old value: `JumpKeeping` keeps it only on the path that
                        // jumps, and consumes it on the one that falls through — which is the whole
                        // reason `a || b` answers `b` rather than leaving both behind.
                        self.expression(value)?;
                        self.store_name(name)?;
                        return self.chunk.patch(over_the_store);
                    }
                    None => return Err(unsupported("a logical assignment", span)),
                }
                self.store_name(name)
            }
            // §13.2.4 — an array literal. The length is the element count *including holes*, so
            // it is known here and set once; each element that was written is then defined at its
            // own index, and each one that was not is simply never defined. That absence is what
            // a hole is: `[, 1]` has a length of 2 and one property, and `0 in [, 1]` is false
            // where `0 in [undefined, 1]` is true.
            ExprKind::Array(elements) => {
                // A spread makes the length unknowable here — `[...a]` is as long as `a` turns out
                // to be — so the count moves into a slot and the whole literal is built by
                // running rather than by placing. Kept apart from the ordinary path because that
                // one knows its length and should go on saying so.
                if elements
                    .iter()
                    .any(|element| matches!(element, ArrayElement::Spread { .. }))
                {
                    return self.spread_array(elements, span);
                }
                let count = u32::try_from(elements.len()).map_err(|_| CompileError {
                    kind: ErrorKind::TooLong,
                    span,
                })?;
                self.chunk.emit(Instruction::NewArray(count));
                for (at, element) in elements.iter().enumerate() {
                    let value = match element {
                        ArrayElement::Hole => continue,
                        ArrayElement::Spread { .. } => {
                            return Err(unsupported("a spread element", span));
                        }
                        ArrayElement::Value(value) => value,
                    };
                    // The index as a *name*, because that is what a property key is: an array
                    // holds `"0"` and not `0`, which is why `a["0"]` and `a[0]` are one property.
                    let id = self
                        .heap
                        .new_string(at.to_string().encode_utf16().collect());
                    self.constant(Value::String(id))?;
                    self.expression(value)?;
                    self.chunk.emit(Instruction::DefineField);
                }
                Ok(())
            }
            // §15.2.5 — a function expression. The object is made where the expression is
            // *evaluated*, so a `function` keyword inside a loop makes one object per iteration.
            // Unnamed unless something above named it; §8.6.3 reaches only the positions that do,
            // and [`Compiler::named_evaluation`] is where each of those is listed.
            ExprKind::Function(function) => self.make_function(function, Naming::default(), span),
            // §15.3 — an arrow, which is a function expression that keeps the `this` around it.
            ExprKind::Arrow(arrow) => self.make_arrow(arrow, Naming::default(), span),
            // §15.7.12 — an expression leaves the constructor where it was evaluated. Its own name,
            // if it has one, binds only inside the body and is not created yet.
            ExprKind::Class(class) => self.class(class, Naming::default(), span),
            ExprKind::Template(template) => self.template(template),
            ExprKind::TaggedTemplate { tag, quasi } => self.tagged_template(tag, quasi, span),
            ExprKind::RegExp(_) => Err(unsupported("a regular expression literal", span)),
            ExprKind::Await(_) => Err(unsupported("await", span)),
            ExprKind::Yield(_) => Err(unsupported("yield", span)),
            ExprKind::Super => Err(unsupported("super", span)),
            // §13.3.12 — `GetNewTarget()`, which the running call decided and which the parser has
            // already refused anywhere there is no call to have decided it.
            ExprKind::NewTarget => {
                self.chunk.emit(Instruction::LoadNewTarget);
                Ok(())
            }
            ExprKind::ImportMeta => Err(unsupported("import.meta", span)),
            ExprKind::ImportCall { .. } => Err(unsupported("a dynamic import", span)),
            ExprKind::OptionalChain(_) => Err(unsupported("optional chaining", span)),
        }
    }
    /// §13.2.5 — an object literal.
    ///
    /// Every property is *defined* rather than assigned, so nothing inherited can refuse one and
    /// no setter can intercept it. Its own method rather than an arm of [`Compiler::expression`]
    /// because that function recurses once per level of an expression, and every arm's locals are
    /// part of every frame: a long `a + b + c` chain is as deep as it is long, and the frame it
    /// walks with should be no larger than it has to be.
    /// Kept out of line, deliberately.
    ///
    /// [`Compiler::expression_inner`] recurses, so its stack frame is paid once per level of a
    /// nested expression — and DR-0006's depth limit bounds the levels, not the bytes. A helper
    /// inlined into it adds its locals to *every* level, which is how a change that touched
    /// neither the limit nor the recursion overflowed a smaller stack than this machine's.
    #[inline(never)]
    fn object_literal(
        &mut self,
        properties: &[PropertyDefinition],
        span: Span,
    ) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::NewObject);

        for property in properties.iter() {
            // §13.2.5 — `...o` contributes properties rather than *a* property, so it has no key to
            // push and nothing for the match below to describe. Its own enumerable properties are
            // copied onto the literal being built, by the same walk §14.3.3's object rest uses.
            if let PropertyDefinition::Spread { value, .. } = property {
                self.expression(value)?;
                self.chunk.emit(Instruction::SpreadProperties);
                continue;
            }
            // B.3.1 — `__proto__: v` sets the prototype rather than making a property, and this is
            // the one Annex B rule praxis implements (DR-0008 says why). The production is exactly
            // `PropertyName : AssignmentExpression`, so every other way of writing the same spelling
            // is an ordinary property and those exclusions are the whole difficulty:
            //
            //   `{ __proto__: p }`     sets the prototype
            //   `{ '__proto__': p }`   sets it too — a String literal key has the same StringValue
            //   `{ ['__proto__']: p }` does **not**: a computed key is a different production
            //   `{ __proto__ }`        does not: a shorthand is a different production
            //   `{ __proto__() {} }`   does not: a method is a different production
            if let PropertyDefinition::KeyValue { key, value } = property
                && is_proto_key(key)
            {
                self.expression(value)?;
                self.chunk.emit(Instruction::SetLiteralPrototype);
                continue;
            }
            // §13.2.5's four productions that make a property, reduced to a key and something to
            // put under it. A shorthand is `{a: a}` — the name is both — and a method is a
            // function expression with the property's name; only an *accessor* is a different
            // kind of property rather than a different way of writing a value.
            let (key, value, accessor) = match property {
                PropertyDefinition::KeyValue { key, value } => (key, Element::Value(value), None),
                PropertyDefinition::Shorthand { name, span } => (
                    &AstPropertyKey::Identifier(name.clone()),
                    Element::Name(name, *span),
                    None,
                ),
                PropertyDefinition::Method {
                    key,
                    kind,
                    function,
                } => (key, Element::Function(function), Some(*kind)),
                // §13.2.5.1 — `{a = 1}` is a Syntax Error wherever the literal stays a literal,
                // and the parser has already refused every position where it does. Reaching it
                // here would mean a tree no source produces.
                PropertyDefinition::ShorthandWithDefault { .. } => {
                    return Err(unsupported("a shorthand with an initializer", span));
                }
                // Handled above: a spread has no key, so it cannot produce the triple this match is
                // for. Listed rather than swept into a catch-all so that a fourth kind of property
                // cannot arrive here unnoticed.
                PropertyDefinition::Spread { .. } => {
                    return Err(unsupported("a spread in an object literal", span));
                }
            };
            // A key is pushed once and used twice for an accessor pair, so it is emitted here
            // whatever kind of property follows.
            match key {
                AstPropertyKey::Identifier(name) => {
                    let units: Vec<u16> = name.encode_utf16().collect();
                    let id = self.heap.new_string(units);
                    self.constant(Value::String(id))?;
                }
                AstPropertyKey::String(units) => {
                    let id = self.heap.new_string(units.to_vec());
                    self.constant(Value::String(id))?;
                }
                // §13.2.5.5 — a numeric key becomes the String `ToString` writes, at
                // compile time because the number is already known: `{1.0: x}` and
                // `{1: x}` are one property, and `{1e21: x}` is the key `"1e+21"`.
                AstPropertyKey::Number(number) => {
                    let value = Value::Number(*number);
                    let id = value
                        .to_string(self.heap)
                        .map_err(|_| unsupported("a key that cannot be written down", span))?;
                    self.constant(Value::String(id))?;
                }
                AstPropertyKey::Computed(expression) => self.expression(expression)?,
                AstPropertyKey::BigInt(_) | AstPropertyKey::Private(_) => {
                    return Err(unsupported("a BigInt or private key", span));
                }
            }
            match value {
                Element::Value(expression) => match key {
                    // §13.2.5.5 — a property definition is one of §8.6.3's named positions, so
                    // `{ a: function () {} }` and `{ a: () => {} }` are both called `a`.
                    AstPropertyKey::Identifier(name) => {
                        self.named_evaluation(name, expression)?;
                    }
                    _ => self.expression(expression)?,
                },
                Element::Name(name, at) => {
                    let _ = at;
                    self.load_name(name)?;
                }
                // §15.4.5 `MethodDefinitionEvaluation` — a *method* gets `MakeMethod` and a function
                // written as a value does not: `{ m() { return super.x; } }` is legal and
                // `{ m: function () { return super.x; } }` is a Syntax Error, which is the only place
                // the two shapes differ at all. The object is under the key, two down.
                Element::Function(function) => {
                    // §10.2.9 — a literal's method is named by its key, and an accessor carries the
                    // prefix. A computed key is not known at compile time, so it is left unnamed.
                    let prefix = match accessor {
                        Some(MethodKind::Get) => Some("get "),
                        Some(MethodKind::Set) => Some("set "),
                        _ => None,
                    };
                    let named = match key {
                        AstPropertyKey::Identifier(name) => Some(&**name),
                        _ => None,
                    };
                    // §15.4.5 — a literal's method is not a constructor either, which is the one
                    // thing that distinguishes `{ m() {} }` from `{ m: function () {} }` beyond the
                    // name. `new o.m()` is a TypeError for the first and an object for the second.
                    self.make_method_function(
                        function,
                        Naming {
                            name: named,
                            prefix,
                        },
                        true,
                        span,
                    )?;
                    self.chunk.emit(Instruction::MakeMethod(2));
                }
            }
            match accessor {
                // §15.4.5 — a getter and a setter are *halves* of one property, so defining one
                // must not wipe the other. That is `DefineOwnProperty` with only the half that
                // was written, which is not what `CreateDataProperty` does.
                Some(MethodKind::Get) => self.chunk.emit(Instruction::DefineGetter),
                Some(MethodKind::Set) => self.chunk.emit(Instruction::DefineSetter),
                // §15.4.4's ordinary method is a data property like any other. What makes it a
                // method rather than a function *expression* is the `name` and the missing
                // `prototype`, and neither is here yet.
                _ => self.chunk.emit(Instruction::DefineField),
            }
        }
        Ok(())
    }
    /// §13.3.11 — a tagged template, which is a call whose first argument is the template object.
    ///
    /// `f\`a${b}c\`` is `f(templateObject, b)`, and the tag is called as a *method* when it is written
    /// as one: `` o.m`x` `` has `o` for its receiver, exactly as `o.m()` would. So this is the ordinary
    /// call path with an argument list nobody wrote.
    fn tagged_template(
        &mut self,
        tag: &Expr,
        quasi: &TemplateLiteral,
        span: Span,
    ) -> Result<(), CompileError> {
        let method = matches!(
            tag.kind,
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. }
        );
        if method {
            let reference = self.property_reference(tag, Keep::Receiver)?;
            self.chunk.emit(reference.get());
        } else {
            self.expression(tag)?;
        }
        // §13.2.8.3's object first, then the substitutions in order — which is the argument list
        // §13.3.11 builds, and the reason a tag with no substitutions still takes one argument.
        let index = self.template_object(quasi, span)?;
        self.chunk.emit(Instruction::TemplateObject(index));
        for expression in quasi.expressions.iter() {
            self.expression(expression)?;
        }
        let count = u32::try_from(quasi.expressions.len() + 1).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        self.chunk.emit(match method {
            true => Instruction::CallMethod(count),
            false => Instruction::Call(count),
        });
        Ok(())
    }

    /// File this site's template strings and answer their index.
    ///
    /// Interned, because the components of a template are fixed text and the same one may appear in
    /// a hundred sites — and because the key a property is filed under has to be the interned one.
    fn template_object(
        &mut self,
        quasi: &TemplateLiteral,
        span: Span,
    ) -> Result<u32, CompileError> {
        let mut cooked = Vec::with_capacity(quasi.quasis.len());
        let mut raw = Vec::with_capacity(quasi.quasis.len());
        for element in quasi.quasis.iter() {
            cooked.push(element.cooked.as_ref().map(|units| self.heap.intern(units)));
            raw.push(self.heap.intern(&element.raw));
        }
        let index = u32::try_from(self.chunk.templates.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        self.chunk
            .templates
            .push(crate::compile::chunk::Template { cooked, raw });
        Ok(index)
    }

    /// §8.6.3 `NamedEvaluation` — compile `value`, and name it `name` if it is anonymous.
    ///
    /// The positions this applies to are a closed list in the specification, and every caller of this
    /// is one of them: a variable declaration, an assignment to a name, a property definition, a class
    /// field, and a destructuring default. It reaches an **anonymous** function, arrow or class and
    /// nothing else — `var a = function f() {}` is called `f`, and `var a = (0, function () {})` is
    /// called nothing at all, because a parenthesised comma expression is not a function.
    pub(super) fn named_evaluation(
        &mut self,
        name: &str,
        value: &Expr,
    ) -> Result<(), CompileError> {
        let span = value.span;
        match &value.kind {
            ExprKind::Function(function) if function.name.is_none() => {
                self.make_function(function, Naming::of(name), span)
            }
            // An arrow has no name of its own to prefer — there is no production for one.
            ExprKind::Arrow(arrow) => self.make_arrow(arrow, Naming::of(name), span),
            ExprKind::Class(class) if class.name.is_none() => {
                self.class(class, Naming::of(name), span)
            }
            _ => self.expression(value),
        }
    }

    /// The object half of a property reference, and the receiver a call would want with it.
    ///
    /// Two shapes, and `super` is the one that needs a name. An ordinary base *is* the receiver, so a
    /// call copies it. §13.3.7.1's super reference has two different objects: the property is looked
    /// up on the home object's prototype and the receiver stays `this`, so the receiver is pushed
    /// first and the base after it. Everything above this — reading, calling, assigning, deleting —
    /// then works on a stack of `[receiver?, base, key]` either way.
    fn base(&mut self, object: &Expr, keep: Keep) -> Result<Reference, CompileError> {
        if matches!(object.kind, ExprKind::Super) {
            // The receiver §13.3.7.1 keeps, and a copy of it if a call is going to want one under the
            // callee. `keep.receiver` is the same `Duplicate` an ordinary base uses, applied to the
            // receiver rather than to the base — which is precisely the difference.
            self.load_this();
            keep.receiver(self);
            self.chunk.emit(Instruction::LoadSuperBase);
            return Ok(Reference::Super);
        }
        self.expression(object)?;
        keep.receiver(self);
        Ok(Reference::Ordinary)
    }

    /// The base and the key of a property reference, pushed in that order.
    ///
    /// Shared by reading, writing and deleting, which all need the same two values and all refuse
    /// the same two things. Written out three times it was the same guard three times, and two of
    /// them were unreachable: `o?.a = 1` and `#x` outside a class are refused by the parser long
    /// before they reach a compiler.
    pub(super) fn property_reference(
        &mut self,
        target: &Expr,
        keep: Keep,
    ) -> Result<Reference, CompileError> {
        match &target.kind {
            ExprKind::Member {
                private,
                optional,
                object,
                property,
            } => {
                if *optional {
                    return Err(unsupported("optional chaining", target.span));
                }
                let reference = self.base(object, keep)?;
                // §13.3.7 — `a.#b`'s key is the Private Name the enclosing class minted, read out of
                // the class scope by the same walk that reaches the class's own name. The parser has
                // already refused a `#b` no enclosing class binds, so the slot is there.
                if *private {
                    self.load_private_name(property)?;
                    return Ok(Reference::Private);
                }
                let units: Vec<u16> = property.encode_utf16().collect();
                let id = self.heap.new_string(units);
                self.constant(Value::String(id))?;
                Ok(reference)
            }
            ExprKind::ComputedMember {
                optional,
                object,
                property,
            } => {
                if *optional {
                    return Err(unsupported("optional chaining", target.span));
                }
                let reference = self.base(object, keep)?;
                self.expression(property)?;
                Ok(reference)
            }
            _ => Err(unsupported(
                "a reference to something that is not a property",
                target.span,
            )),
        }
    }
    /// §13.5.1 — `delete`.
    ///
    /// Answers whether the property is gone, which is not the same as whether it was there:
    /// `delete o.nothing` is `true`. Deleting anything that is not a property reference is also
    /// `true` and does nothing at all, which is why `delete 1` is legal outside strict mode.
    fn delete(&mut self, argument: &Expr, span: Span) -> Result<(), CompileError> {
        match &argument.kind {
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                let reference = self.property_reference(argument, Keep::Nothing)?;
                // §13.5.1.1 step 3 — `delete super.x` is a **ReferenceError**, always, and there is
                // no super reference it is legal for. A run-time throw rather than an early error
                // because the reference is evaluated first: `delete super[k]` runs `ToPropertyKey(k)`
                // and *then* refuses, so a `toString` on the key has already had its effect.
                self.chunk.emit(match reference {
                    Reference::Super => Instruction::ThrowSuperDelete,
                    // §15.7.1 makes `delete this.#x` an *early* error, so the parser has already
                    // refused it and this arm is unreachable from source. `DeleteProperty` is the
                    // honest thing for a hand-built tree: the name is a Symbol that no property table
                    // holds, so the delete finds nothing and answers `true`.
                    Reference::Ordinary | Reference::Private => Instruction::DeleteProperty,
                });
                Ok(())
            }
            // §13.5.1.2 step 2 — deleting a name is only legal in sloppy code and only for a
            // configurable global, which needs the global object. Deleting anything else at all
            // evaluates it and answers `true`.
            ExprKind::Identifier(_) => Err(unsupported("deleting a name", span)),
            _ => {
                self.expression(argument)?;
                self.chunk.emit(Instruction::Pop);
                self.constant(Value::Boolean(true))
            }
        }
    }
    /// `o.x = v` and `o[k] = v`, and their compound forms — §13.15.2.
    ///
    /// The base and the key are evaluated **once**, which is what makes `o[f()] += 1` call `f`
    /// once rather than twice. For a compound operator that means the two have to be on the stack
    /// twice over — once for the read and once for the write — which is what `DuplicateTwo` is
    /// for.
    /// Kept out of line, deliberately.
    ///
    /// [`Compiler::expression_inner`] recurses, so its stack frame is paid once per level of a
    /// nested expression — and DR-0006's depth limit bounds the levels, not the bytes. A helper
    /// inlined into it adds its locals to *every* level, which is how a change that touched
    /// neither the limit nor the recursion overflowed a smaller stack than this machine's.
    #[inline(never)]
    fn assign_to_property(
        &mut self,
        operator: AssignmentOperator,
        target: &Expr,
        value: &Expr,
        span: Span,
    ) -> Result<(), CompileError> {
        // §13.3.7.1 leaves a reference with *three* values — the receiver as well as the base and the
        // key — and every compound form below reads the old value with `DuplicateTwo`, which copies
        // two. Rather than a three-deep copy for one construct, the plain assignment is compiled here
        // and the rest is refused by name: `super.x += 1` is legal and rare, and a refusal with a span
        // is better than an unbalanced stack.
        // The reference is compiled once, and it decides everything below: how wide it is, how the old
        // value is read back, and how the new one is written. `super.x` is three values and `o.#x` is
        // two with a Private Name where a key would be, and both used to be refused here for exactly
        // that — a compound form that copied two values could not read either of them back.
        let reference = self.property_reference(target, Keep::Nothing)?;
        let width = reference.width();
        match compound_operator(operator) {
            None if operator == AssignmentOperator::Assign => self.expression(value)?,
            Some(binary) => {
                self.chunk.emit(Instruction::DuplicateTop(width));
                self.chunk.emit(reference.get());
                self.expression(value)?;
                self.chunk.emit(Instruction::Binary(binary));
            }
            // The same rule as for a name, with the reference to clear up. On the path where the
            // circuit fires it is still under the old value, so the value is buried beneath it and it
            // is dropped — both paths have to leave the stack one deep or the chunk is unbalanced.
            None if short_circuit_operator(operator).is_some() => {
                let condition = match short_circuit_operator(operator) {
                    Some(condition) => condition,
                    None => return Err(unsupported("a logical assignment", span)),
                };
                self.chunk.emit(Instruction::DuplicateTop(width));
                self.chunk.emit(reference.get());
                let circuit_fired = self
                    .chunk
                    .emit_jump(|target| Instruction::JumpKeeping(condition, target));
                self.expression(value)?;
                self.chunk.emit(reference.set());
                let done = self.chunk.emit_jump(Instruction::Jump);
                self.chunk.patch(circuit_fired)?;
                self.chunk.emit(Instruction::Bury(width));
                for _ in 0..width {
                    self.chunk.emit(Instruction::Pop);
                }
                return self.chunk.patch(done);
            }
            None => return Err(unsupported("a logical assignment", span)),
        }
        self.chunk.emit(reference.set());
        Ok(())
    }
    /// §13.4 — `++a`, `a++`, `--a` and `a--`.
    ///
    /// # Why the operand is coerced before anything is added
    ///
    /// §13.4.4.1 step 3 applies `ToNumeric` to the *old* value and then adds one to the result.
    /// That is not the same as adding one and coercing afterwards: `x = "1"; x++` leaves `x` as
    /// the number 2 and evaluates to the number **1**, not to the string `"1"`. So the coercion
    /// is an instruction of its own — `+x` is exactly `ToNumber` (§13.5.4) — and once it has run
    /// the addition cannot be string concatenation, whatever was there before.
    ///
    /// `ToNumeric` rather than `ToNumber` is what the specification says, and the difference is
    /// BigInt: `1n++` is 2n and never becomes a Number. There are no BigInt values yet, so the
    /// two agree on every value that exists, and this changes when they stop agreeing.
    /// Kept out of line, deliberately.
    ///
    /// [`Compiler::expression_inner`] recurses, so its stack frame is paid once per level of a
    /// nested expression — and DR-0006's depth limit bounds the levels, not the bytes. A helper
    /// inlined into it adds its locals to *every* level, which is how a change that touched
    /// neither the limit nor the recursion overflowed a smaller stack than this machine's.
    #[inline(never)]
    fn update(
        &mut self,
        operator: UpdateOperator,
        prefix: bool,
        argument: &Expr,
        span: Span,
    ) -> Result<(), CompileError> {
        let step = match operator {
            UpdateOperator::Increment => BinaryOperator::Add,
            UpdateOperator::Decrement => BinaryOperator::Subtract,
        };
        match &argument.kind {
            ExprKind::Identifier(name) => {
                self.load_name(name)?;
                self.chunk.emit(Instruction::Unary(UnaryOperator::Plus));
                // The old value has to outlive the store, and only a postfix one needs it.
                if !prefix {
                    self.chunk.emit(Instruction::Duplicate);
                }
                self.constant(Value::Number(1.0))?;
                self.chunk.emit(Instruction::Binary(step));
                self.store_name(name)?;
                // The store leaves the *new* value behind. A postfix one wants the old, which is
                // underneath it, so the new one goes.
                if !prefix {
                    self.chunk.emit(Instruction::Pop);
                }
                Ok(())
            }
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                // §13.4.4.1 step 1 evaluates the reference *once*, so `o[f()]++` calls `f` once —
                // the same guarantee `o[f()] += 1` needs, and the same instructions.
                let reference = self.property_reference(argument, Keep::Nothing)?;
                let width = reference.width();
                self.chunk.emit(Instruction::DuplicateTop(width));
                self.chunk.emit(reference.get());
                self.chunk.emit(Instruction::Unary(UnaryOperator::Plus));
                if !prefix {
                    // Under the base, the key and the copy that is about to become the new value
                    // — the three the store is going to consume. It surfaces again when the new
                    // value the store leaves behind is discarded.
                    self.chunk.emit(Instruction::Duplicate);
                    // Under the whole reference and the copy that is about to become the new value —
                    // everything the store is going to consume. `width + 1` and not a literal, because
                    // a `super` reference is one value wider than the other two.
                    self.chunk.emit(Instruction::Bury(width + 1));
                }
                self.constant(Value::Number(1.0))?;
                self.chunk.emit(Instruction::Binary(step));
                self.chunk.emit(reference.set());
                if !prefix {
                    self.chunk.emit(Instruction::Pop);
                }
                Ok(())
            }
            // §13.4.1 — the parser has already refused anything that is not a simple assignment
            // target, so what is left is a target this compiler has not learned yet.
            _ => Err(unsupported(
                "an update of something that is not a name",
                span,
            )),
        }
    }

    /// Emit a constant and the instruction that pushes it.
    /// §13.2.4.1 — an array literal with a spread in it, built one element at a time.
    ///
    /// The ordinary path knows its length before it starts and defines each element at a written
    /// index. A spread has no such number: `[...a]` is as long as `a` turns out to be, and the
    /// elements after it sit wherever that left off. So the index is a slot the loop moves, and
    /// `length` is written at the end — which is also what makes a trailing hole count.
    ///
    /// §13.2.4.1 spreads by *iterating*, not by reading indices, so `[..."ab"]` is two one-character
    /// strings and `[...someSet]` works. That is the same `@@iterator` a `for`-`of` asks for, and
    /// an object without one is a TypeError here as it is there.
    fn spread_array(&mut self, elements: &[ArrayElement], span: Span) -> Result<(), CompileError> {
        let at = self.declare_hidden("at");
        self.chunk.emit(Instruction::NewArray(0));
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        for element in elements {
            match element {
                // A hole defines nothing and still takes a place, which is what makes `[, 1]` two
                // long with one property.
                ArrayElement::Hole => self.bump(at)?,
                ArrayElement::Value(value) => {
                    // No copy of the array: `DefineField` takes the key and the value and leaves
                    // the object it defined on, which is what lets the whole literal be built
                    // with one of them on the stack throughout.
                    self.chunk.emit(Instruction::LoadVariable(0, at));
                    self.expression(value)?;
                    self.chunk.emit(Instruction::DefineField);
                    self.bump(at)?;
                }
                ArrayElement::Spread { value, .. } => {
                    self.expression(value)?;
                    self.spread_into(at)?;
                }
            }
        }
        // §13.2.4.1 sets the length from the count, which matters when the last element was a
        // hole: nothing was defined there, so nothing grew the array to reach it.
        self.chunk.emit(Instruction::Duplicate);
        let name = self.name_of("length");
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.chunk.emit(Instruction::SetProperty);
        self.chunk.emit(Instruction::Pop);
        let _ = span;
        Ok(())
    }

    /// Move the running index on by one.
    pub(super) fn bump(&mut self, at: u32) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.constant(Value::Number(1.0))?;
        self.chunk.emit(Instruction::Binary(BinaryOperator::Add));
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// Iterate whatever is on top of the stack into the array beneath it.
    ///
    /// Leaves the array where it found it. Nothing is closed on the way out because there is no
    /// way out until the iterator says it is done — §13.2.4.1 spreads to exhaustion, so the only
    /// abrupt end is one the iterator itself raised, and §7.4.9 has nothing to add to that.
    pub(super) fn spread_into(&mut self, at: u32) -> Result<(), CompileError> {
        let iterator = self.declare_hidden("iterator");
        let next = self.declare_hidden("next");
        let done = self.declare_hidden("done");
        self.chunk.emit(Instruction::Duplicate);
        self.chunk
            .emit(Instruction::LoadWellKnown(super::statement::well_known(
                "iterator",
            )));
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

        let top = self.here()?;
        self.emit_step(iterator, next, done)?;
        let current = self.declare_hidden("current");
        self.chunk.emit(Instruction::StoreVariable(0, current));
        self.chunk.emit(Instruction::Pop);
        self.chunk.emit(Instruction::LoadVariable(0, done));
        let out = self.chunk.emit_jump(Instruction::JumpIfTrue);
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.chunk.emit(Instruction::LoadVariable(0, current));
        self.chunk.emit(Instruction::DefineField);
        self.bump(at)?;
        self.chunk.emit(Instruction::Jump(top));
        self.chunk.patch(out)
    }

    /// §13.2.8.6 — a template literal, which is its pieces joined in the order they are written.
    ///
    /// Each substitution is `ToString`ed the moment it is evaluated, not at the end. That ordering
    /// is observable: a `toString` that throws must do so before the *next* substitution is
    /// evaluated, and one with a side effect must see only what came before it.
    ///
    /// `ToString` and not `+ ""`, which is the other thing that looks like it would work. Addition
    /// asks an object with the default hint and reaches `valueOf`; a template asks with the string
    /// hint and reaches `toString`. An object with both answers differently in the two places, and
    /// that is a difference the specification means.
    ///
    /// The cooked strings are joined with `+` rather than through a second `ToString`, because
    /// they are Strings already and adding two Strings is exact concatenation.
    /// Kept out of line, deliberately.
    ///
    /// [`Compiler::expression_inner`] recurses, so its stack frame is paid once per level of a
    /// nested expression — and DR-0006's depth limit bounds the levels, not the bytes. A helper
    /// inlined into it adds its locals to *every* level, which is how a change that touched
    /// neither the limit nor the recursion overflowed a smaller stack than this machine's.
    #[inline(never)]
    fn template(&mut self, template: &TemplateLiteral) -> Result<(), CompileError> {
        let mut quasis = template.quasis.iter();
        // A template always has one more component than it has substitutions, so the first is
        // there even when the template is empty — `` is one empty component and no substitutions.
        self.cooked(quasis.next())?;
        for expression in template.expressions.iter() {
            self.expression(expression)?;
            self.chunk.emit(Instruction::Stringify);
            self.chunk.emit(Instruction::Binary(BinaryOperator::Add));
            self.cooked(quasis.next())?;
            self.chunk.emit(Instruction::Binary(BinaryOperator::Add));
        }
        Ok(())
    }

    /// Push one cooked component of a template, or the empty String if it has none.
    ///
    /// A component with no cooked value holds a `NotEscapeSequence`, which §13.2.8.1 permits only
    /// in a *tagged* template — where the tag may read the raw text instead. An untagged one with
    /// such a component is a SyntaxError the parser raises, so reaching here with `None` would
    /// mean a tree no source can produce; the empty String is what that would have joined to.
    fn cooked(&mut self, element: Option<&TemplateElement>) -> Result<(), CompileError> {
        let units = element
            .and_then(|found| found.cooked.as_deref())
            .unwrap_or(&[]);
        let id = self.heap.intern(units);
        self.constant(Value::String(id))
    }

    pub(super) fn constant(&mut self, value: Value) -> Result<(), CompileError> {
        let index = self.chunk.add_constant(value)?;
        self.chunk.emit(Instruction::Constant(index));
        Ok(())
    }
}

/// The binary operator a compound assignment applies, if it is one.
///
/// `+=` is `+`, and so on for the eleven that pair up. The three logical ones — `&&=`, `||=`,
/// `??=` — do not: they are short circuits, so they may not assign at all, and there is no
/// binary operator that describes them.
/// The short circuit `&&=`, `||=` and `??=` test before they assign, if this is one of them.
///
/// Separate from [`compound_operator`] because these three are not compound assignments at all:
/// §13.15.2 gives them their own evaluation, in which the value is never computed and the store never
/// happens unless the test fails. `a ||= f()` does not call `f` when `a` is truthy, where `a += f()`
/// always calls it.
fn short_circuit_operator(operator: AssignmentOperator) -> Option<ShortCircuit> {
    Some(match operator {
        AssignmentOperator::LogicalAnd => ShortCircuit::WhenFalsy,
        AssignmentOperator::LogicalOr => ShortCircuit::WhenTruthy,
        AssignmentOperator::NullishCoalescing => ShortCircuit::WhenNotNullish,
        _ => return None,
    })
}

fn compound_operator(operator: AssignmentOperator) -> Option<BinaryOperator> {
    Some(match operator {
        AssignmentOperator::Add => BinaryOperator::Add,
        AssignmentOperator::Subtract => BinaryOperator::Subtract,
        AssignmentOperator::Multiply => BinaryOperator::Multiply,
        AssignmentOperator::Divide => BinaryOperator::Divide,
        AssignmentOperator::Remainder => BinaryOperator::Remainder,
        AssignmentOperator::Exponent => BinaryOperator::Exponent,
        AssignmentOperator::ShiftLeft => BinaryOperator::ShiftLeft,
        AssignmentOperator::ShiftRight => BinaryOperator::ShiftRight,
        AssignmentOperator::ShiftRightUnsigned => BinaryOperator::ShiftRightUnsigned,
        AssignmentOperator::BitwiseAnd => BinaryOperator::BitwiseAnd,
        AssignmentOperator::BitwiseXor => BinaryOperator::BitwiseXor,
        AssignmentOperator::BitwiseOr => BinaryOperator::BitwiseOr,
        AssignmentOperator::Assign
        | AssignmentOperator::LogicalAnd
        | AssignmentOperator::LogicalOr
        | AssignmentOperator::NullishCoalescing => return None,
    })
}

/// What an object literal puts under a key.
///
/// Three shapes and one destination: a shorthand reads the name it also files under, and a method
/// is a function made where the literal is evaluated — so `{m() {}}` inside a loop makes one
/// function object per turn, exactly as `{m: function () {}}` does.
enum Element<'a> {
    /// `a: 1` — an ordinary expression.
    Value(&'a Expr),
    /// `{a}` — the name is both the key and the value.
    Name(&'a str, Span),
    /// `a() {}`, `get a() {}`, `set a(v) {}`.
    Function(&'a crate::ast::Function),
}

/// Whether this key is B.3.1's `__proto__`, in one of the two spellings the rule covers.
///
/// An `IdentifierName` or a `StringLiteral`, because B.3.1 asks about the key's **StringValue** — so
/// `{ '__proto__': p }` is in and `{ ['__proto__']: p }` is out, the latter being a computed key and a
/// different production. A numeric or BigInt key cannot spell it at all.
fn is_proto_key(key: &AstPropertyKey) -> bool {
    let wanted = "__proto__";
    match key {
        AstPropertyKey::Identifier(name) => &**name == wanted,
        // Compared as code units, because that is what the key is (DR-0004) and because a lone
        // surrogate in a key is representable where a `str` would not be.
        AstPropertyKey::String(units) => units.iter().copied().eq(wanted.encode_utf16()),
        _ => false,
    }
}

/// Which of §13.3's two property references was compiled, and so which instruction reads it.
///
/// Answered by the code that pushed the values rather than asked about the tree afterwards. A second
/// walk over the tree is a second copy of the rule, and the copy nothing reads is the one that would
/// disagree — the first attempt at this was such a walk, with a catch-all arm for a shape it could
/// never be handed, which mutation coverage duly reported as unpinnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reference {
    /// `o.x` — one object, which is both where the property is found and the receiver.
    Ordinary,
    /// `super.x` — §13.3.7.1's two, with the receiver pushed under the base.
    Super,
    /// `o.#x` — one object and a **Private Name**, which is not a property key at all.
    Private,
}

impl Reference {
    /// The instruction that reads this reference.
    pub(super) fn get(self) -> Instruction {
        match self {
            Self::Ordinary => Instruction::GetProperty,
            Self::Super => Instruction::GetSuperProperty,
            Self::Private => Instruction::GetPrivate,
        }
    }

    /// The instruction that writes this reference, with the value pushed on top of it.
    ///
    /// All three consume the whole reference and leave the value, so every caller balances without
    /// knowing which it got — which is the point. Writing `SetProperty` for all of them was a silent
    /// wrong answer for two of the three: a Private Name *is* a valid property key, so a private
    /// write through this path quietly made a Symbol-keyed property instead of throwing, and a
    /// `super` write left its receiver stranded on the stack.
    pub(super) fn set(self) -> Instruction {
        match self {
            Self::Ordinary => Instruction::SetProperty,
            Self::Super => Instruction::SetSuperProperty,
            Self::Private => Instruction::SetPrivate,
        }
    }

    /// How many values this reference occupies on the stack.
    ///
    /// What a compound assignment and an update need: both read the old value and then write, and
    /// §13.15.2 evaluates the reference once — so the whole of it has to be copied, and how much that
    /// is depends on which reference it is. Two for `o.x` and for `o.#x`, three for `super.x`, whose
    /// receiver sits under the base.
    pub(super) fn width(self) -> u32 {
        match self {
            Self::Ordinary | Self::Private => 2,
            Self::Super => 3,
        }
    }
}
