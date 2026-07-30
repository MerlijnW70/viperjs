//! §13 — expressions, each of which leaves exactly one value on the stack.
//!
//! The one invariant every function here keeps: compiling an expression adds one value and
//! consumes whatever it needed. That is what lets a statement be stack-neutral by construction
//! and what [`crate::vm::Fault::UnbalancedStack`] checks at the end of a chunk.

use super::function::Keep;
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
                self.property_reference(expression, Keep::Nothing)?;
                self.chunk.emit(Instruction::GetProperty);
                Ok(())
            }
            // Everything else, named as the specification names it so that the message says which
            // clause is missing rather than which Rust variant.
            // §13.1.3 — a name is a slot when the compiler can place it and a property of the
            // global object when it cannot. Which of the two is decided here; whether the global
            // is *there* is decided at run time, because a script can make one at any moment.
            ExprKind::Identifier(name) => self.load_name(name),
            // §13.2.1 — `this`, which the call decided and the frame is holding.
            ExprKind::This => {
                self.chunk.emit(Instruction::LoadThis);
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
                    // `a = v`.
                    None if *operator == AssignmentOperator::Assign => self.expression(value)?,
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
                    // `a &&= v` and its two siblings only assign when the short circuit does not
                    // fire, so the store is *inside* the jump. Left for the slice that has a
                    // reason to build it.
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
            ExprKind::Function(function) => self.make_function(function, span),
            // §15.3 — an arrow, which is a function expression that keeps the `this` around it.
            ExprKind::Arrow(arrow) => self.make_arrow(arrow, span),
            // §15.7.12 — an expression leaves the constructor where it was evaluated. Its own name,
            // if it has one, binds only inside the body and is not created yet.
            ExprKind::Class(class) => self.class(class, span),
            ExprKind::Template(template) => self.template(template),
            ExprKind::TaggedTemplate { .. } => Err(unsupported("a tagged template", span)),
            ExprKind::RegExp(_) => Err(unsupported("a regular expression literal", span)),
            ExprKind::Await(_) => Err(unsupported("await", span)),
            ExprKind::Yield(_) => Err(unsupported("yield", span)),
            ExprKind::Super => Err(unsupported("super", span)),
            ExprKind::NewTarget => Err(unsupported("new.target", span)),
            ExprKind::ImportMeta => Err(unsupported("import.meta", span)),
            ExprKind::ImportCall { .. } => Err(unsupported("a dynamic import", span)),
            ExprKind::OptionalChain(_) => Err(unsupported("optional chaining", span)),
            ExprKind::PrivateIn { .. } => Err(unsupported("a private-name in expression", span)),
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
                Element::Value(expression) => self.expression(expression)?,
                Element::Name(name, at) => {
                    let _ = at;
                    self.load_name(name)?;
                }
                Element::Function(function) => self.make_function(function, span)?,
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
    ) -> Result<(), CompileError> {
        match &target.kind {
            ExprKind::Member {
                private,
                optional,
                object,
                property,
            } => {
                if *private {
                    return Err(unsupported("a private name", target.span));
                }
                if *optional {
                    return Err(unsupported("optional chaining", target.span));
                }
                self.expression(object)?;
                keep.receiver(self);
                let units: Vec<u16> = property.encode_utf16().collect();
                let id = self.heap.new_string(units);
                self.constant(Value::String(id))
            }
            ExprKind::ComputedMember {
                optional,
                object,
                property,
            } => {
                if *optional {
                    return Err(unsupported("optional chaining", target.span));
                }
                self.expression(object)?;
                keep.receiver(self);
                self.expression(property)
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
                self.property_reference(argument, Keep::Nothing)?;
                self.chunk.emit(Instruction::DeleteProperty);
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
        self.property_reference(target, Keep::Nothing)?;
        match compound_operator(operator) {
            None if operator == AssignmentOperator::Assign => self.expression(value)?,
            Some(binary) => {
                self.chunk.emit(Instruction::DuplicateTwo);
                self.chunk.emit(Instruction::GetProperty);
                self.expression(value)?;
                self.chunk.emit(Instruction::Binary(binary));
            }
            None => return Err(unsupported("a logical assignment", span)),
        }
        self.chunk.emit(Instruction::SetProperty);
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
                self.property_reference(argument, Keep::Nothing)?;
                self.chunk.emit(Instruction::DuplicateTwo);
                self.chunk.emit(Instruction::GetProperty);
                self.chunk.emit(Instruction::Unary(UnaryOperator::Plus));
                if !prefix {
                    // Under the base, the key and the copy that is about to become the new value
                    // — the three the store is going to consume. It surfaces again when the new
                    // value the store leaves behind is discarded.
                    self.chunk.emit(Instruction::Duplicate);
                    self.chunk.emit(Instruction::Bury(3));
                }
                self.constant(Value::Number(1.0))?;
                self.chunk.emit(Instruction::Binary(step));
                self.chunk.emit(Instruction::SetProperty);
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
