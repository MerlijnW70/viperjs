//! §13 — expressions, each of which leaves exactly one value on the stack.
//!
//! The one invariant every function here keeps: compiling an expression adds one value and
//! consumes whatever it needed. That is what lets a statement be stack-neutral by construction
//! and what [`crate::vm::Fault::UnbalancedStack`] checks at the end of a chunk.

use super::{
    Chunk, CompileError, Compiler, ErrorKind, Instruction, MAX_EXPRESSION_DEPTH, ShortCircuit,
    unsupported,
};
use crate::ast::PropertyKey as AstPropertyKey;
use crate::ast::{
    Argument, AssignmentOperator, AssignmentTarget, BinaryOperator, Binding, Expr, ExprKind,
    Function, LogicalOperator, PropertyDefinition, UnaryOperator,
};
use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;
use crate::value::Value;
use std::rc::Rc;

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
                    // §13.10.1 — `instanceof` calls a method on its right operand, so it waits
                    // for something that can be called.
                    if *operator == BinaryOperator::Instanceof {
                        return Err(unsupported("the instanceof operator", innermost.span));
                    }
                    spine.push((*operator, &**right));
                    innermost = left;
                }
                self.expression(innermost)?;
                for (operator, right) in spine.into_iter().rev() {
                    self.expression(right)?;
                    // `in` is the one binary operator that is not a value operation: it asks an
                    // object a question rather than converting one, so it is an instruction of
                    // its own rather than a row in `apply_binary`.
                    if operator == BinaryOperator::In {
                        self.chunk.emit(Instruction::HasProperty);
                    } else {
                        self.chunk.emit(Instruction::Binary(operator));
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
            // §13.1.3 — a name is a slot, resolved here rather than looked up at run time.
            // A name with no slot is a *global*, which needs the global object, so refusing is
            // the honest answer until there is one — and it is why `undefined` is still spelled
            // `void 0` in this engine's tests.
            ExprKind::Identifier(name) => match self.binding(name) {
                Some(binding) => {
                    self.load(binding);
                    Ok(())
                }
                None => Err(unsupported("a reference to an undeclared name", span)),
            },
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
            ExprKind::New { .. } => Err(unsupported("the new operator", span)),
            ExprKind::Update { .. } => Err(unsupported("an update expression", span)),
            // §13.15 — assignment, whose *value* is the value assigned. That is why the store
            // leaves it on the stack rather than taking it: `a = b = 1` and `f(a = 1)` both
            // need it.
            ExprKind::Assignment {
                operator,
                target,
                value,
            } => {
                let AssignmentTarget::Simple(target) = &**target else {
                    return Err(unsupported("a destructuring assignment", span));
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
                let Some(target_binding) = self.binding(name) else {
                    return Err(unsupported(
                        "an assignment to an undeclared name",
                        target.span,
                    ));
                };
                match compound_operator(*operator) {
                    // `a = v`.
                    None if *operator == AssignmentOperator::Assign => self.expression(value)?,
                    // `a += v` is `a = a + v`, with `a` read once — which matters not at all for
                    // a slot and matters a great deal for `o[f()] += 1`, where the property key
                    // is evaluated once. The shape is the same either way.
                    Some(binary) => {
                        self.load(target_binding);
                        self.expression(value)?;
                        self.chunk.emit(Instruction::Binary(binary));
                    }
                    // `a &&= v` and its two siblings only assign when the short circuit does not
                    // fire, so the store is *inside* the jump. Left for the slice that has a
                    // reason to build it.
                    None => return Err(unsupported("a logical assignment", span)),
                }
                {
                    self.store(target_binding);
                    Ok(())
                }
            }
            ExprKind::Array(_) => Err(unsupported("an array literal", span)),
            // §15.2.5 — a function expression. The object is made where the expression is
            // *evaluated*, so a `function` keyword inside a loop makes one object per iteration.
            ExprKind::Function(function) => self.make_function(function, span),
            ExprKind::Arrow(_) => Err(unsupported("an arrow function", span)),
            ExprKind::Class(_) => Err(unsupported("a class expression", span)),
            ExprKind::Template(_) => Err(unsupported("a template literal", span)),
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
    fn object_literal(
        &mut self,
        properties: &[PropertyDefinition],
        span: Span,
    ) -> Result<(), CompileError> {
        self.chunk.emit(Instruction::NewObject);

        for property in properties.iter() {
            let PropertyDefinition::KeyValue { key, value } = property else {
                return Err(unsupported(
                    "a shorthand, spread or method in an object literal",
                    span,
                ));
            };
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
            self.expression(value)?;
            self.chunk.emit(Instruction::DefineField);
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
    /// Emit a constant and the instruction that pushes it.
    pub(super) fn constant(&mut self, value: Value) -> Result<(), CompileError> {
        let index = self.chunk.add_constant(value)?;
        self.chunk.emit(Instruction::Constant(index));
        Ok(())
    }
    /// Compile a function's body into a chunk of its own and emit the instruction that makes it.
    ///
    /// The body gets its own [`Compiler`] rather than a scope inside this one. That is not
    /// tidiness: a function's slots are counted from the bottom of *its* frame, so the two
    /// numberings must not share a table, and the separation is what makes a nested body unable
    /// to reach a slot it has no frame for.
    pub(super) fn make_function(
        &mut self,
        function: &Function,
        span: Span,
    ) -> Result<(), CompileError> {
        if function.is_async || function.is_generator {
            return Err(unsupported("an async function or a generator", span));
        }
        // What the body may see: the script's names, and — only to refuse against — the names of
        // every function it is written inside.
        // The body is written inside this scope, so its chain is ours with ours on the end.
        let mut outer = self.outer.clone();
        outer.push(self.locals.clone());
        let body = compile_function_body(self.heap, function, outer, span)?;
        let index = u32::try_from(self.chunk.functions.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        self.chunk.functions.push(Rc::new(body));
        self.chunk.emit(Instruction::MakeFunction(index));
        Ok(())
    }
    /// §13.3.6 — a call: the callee, then the arguments, then the instruction.
    ///
    /// Left to right and callee first, which is observable the moment either has a side effect:
    /// `f()(g())` calls `f` before `g`, and `f(a(), b())` calls `a` before `b`.
    fn call(
        &mut self,
        callee: &Expr,
        arguments: &[Argument],
        span: Span,
    ) -> Result<(), CompileError> {
        // §13.3.6.1 — a method call keeps the object the method was *found on* as the receiver.
        // The base is evaluated once and copied, because `f().m()` must call `f` once.
        let method = matches!(
            callee.kind,
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. }
        );
        if method {
            // The base is evaluated once and copied *before* the key, so the stack ends as
            // [receiver, method] with nothing between them. Copying after the key would leave the
            // key underneath, and evaluating the base twice would run `f()` twice in `f().m()`.
            self.property_reference(callee, Keep::Receiver)?;
            self.chunk.emit(Instruction::GetProperty);
        } else {
            self.expression(callee)?;
        }
        for argument in arguments {
            let Argument::Value(value) = argument else {
                return Err(unsupported("a spread argument", span));
            };
            self.expression(value)?;
        }
        let count = u32::try_from(arguments.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        self.chunk.emit(if method {
            Instruction::CallMethod(count)
        } else {
            Instruction::Call(count)
        });
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

/// Compile one function body into its own chunk.
///
/// A free function rather than a method because it needs the heap while the compiler that asked
/// for it also holds it — and because a body is genuinely a separate unit of code, with its own
/// slots numbered from zero.
fn compile_function_body(
    heap: &mut Heap,
    function: &Function,
    outer: Vec<Vec<Box<str>>>,
    span: Span,
) -> Result<Chunk, CompileError> {
    if function.parameters.rest.is_some() {
        return Err(unsupported("a rest parameter", span));
    }
    let mut compiler = Compiler::new(heap);
    compiler.is_script = false;
    compiler.outer = outer;

    // §10.2.11 — the parameters are the first slots, in order, so an argument can be put in place
    // without the callee being consulted. A default or a pattern would need code to run *inside*
    // the callee before its body, which is a slice of its own.
    for parameter in function.parameters.items.iter() {
        if parameter.default.is_some() {
            return Err(unsupported("a default parameter", span));
        }
        let Binding::Identifier(name) = &parameter.target else {
            return Err(unsupported("a destructuring parameter", span));
        };
        compiler.declare(&name.name);
    }
    compiler.chunk.parameters = compiler.locals.len();

    // A function's own `var`s and inner declarations, on the same terms as a script's.
    for name in var_declared_names(&function.body) {
        compiler.declare(name.name);
    }
    compiler.hoist_functions(&function.body)?;
    compiler.statements(&function.body)?;
    // §10.2.1 step 4 — falling off the end returns `undefined`. The instruction is emitted
    // unconditionally rather than only when the body might reach it: deciding *that* is a
    // reachability analysis, and a `Return` after one that always runs costs a byte.
    compiler.constant(Value::Undefined)?;
    compiler.chunk.emit(Instruction::Return);
    Ok(compiler.finish())
}

/// Whether a property reference should leave its base behind as well.
///
/// A method call wants the object it found the method on — that object becomes the `this` of the
/// call — and every other use of a property wants only the base and the key. One function with a
/// flag rather than two: the guards a property reference has to make are the same either way, and
/// written twice one of the copies is a guard no test can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Just the base and the key, which is what a get, a set and a delete need.
    Nothing,
    /// A copy of the base under them, to be the receiver of a call.
    Receiver,
}

impl Keep {
    /// Emit the copy, if one was asked for.
    fn receiver(self, compiler: &mut Compiler<'_>) {
        if self == Self::Receiver {
            compiler.chunk.emit(Instruction::Duplicate);
        }
    }
}
