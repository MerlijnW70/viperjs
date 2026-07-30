//! §15.2 — what a function expression compiles to, and what a call compiles to.
//!
//! A function body is a chunk of its own, compiled by a compiler of its own. That is not
//! tidiness: a body's slots are counted from the bottom of *its* environment, so the two
//! numberings must not share a table — and the separation is what makes a nested body unable to
//! reach a slot it has no environment for.

use super::{CompileError, Compiler, ErrorKind, Instruction, Local, unsupported};
use crate::ast::{
    Argument, ArrowBody, ArrowFunction, Binding, Expr, ExprKind, FormalParameters, Function, Stmt,
};
use crate::compile::Chunk;
use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;
use crate::value::Value;
use std::rc::Rc;

impl Compiler<'_> {
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
        if self.would_capture_a_per_iteration_binding() {
            return Err(unsupported(
                "a function that closes over a `let` or `const` declared in a loop",
                span,
            ));
        }
        let body = self.compile_nested(
            &function.parameters,
            Body::Statements(&function.body),
            Lexical::No,
            span,
        )?;
        self.emit_function(body, span)
    }

    /// §15.3 — an arrow function.
    ///
    /// The same as a function expression in every way but three, and all three are the same fact:
    /// an arrow is written *over* the scope around it rather than opening one of its own. So it
    /// has no `this`, no `prototype` and no `[[Construct]]` — `this` inside it is whatever it was
    /// one line above, which is the reason arrows replaced `var self = this`.
    pub(super) fn make_arrow(
        &mut self,
        arrow: &ArrowFunction,
        span: Span,
    ) -> Result<(), CompileError> {
        if arrow.is_async {
            return Err(unsupported("an async arrow function", span));
        }
        if self.would_capture_a_per_iteration_binding() {
            return Err(unsupported(
                "a function that closes over a `let` or `const` declared in a loop",
                span,
            ));
        }
        // §15.3.3's `ConciseBody` has two shapes and one meaning: `a => b` returns `b`, and
        // `a => { … }` is an ordinary body. The first is compiled as the second with the `return`
        // written in, which is what the grammar says rather than a shortcut.
        let shape = match &arrow.body {
            ArrowBody::Expression(expression) => Body::Expression(expression),
            ArrowBody::Block(body) => Body::Statements(body),
        };
        let body = self.compile_nested(&arrow.parameters, shape, Lexical::Yes, span)?;
        self.emit_function(body, span)
    }

    /// Compile a body written inside this one into a chunk of its own.
    ///
    /// What the nested body may see: the script's names, and — only to refuse against — the names
    /// of every function it is written inside. It is written inside this scope, so its chain is
    /// ours with ours on the end.
    pub(super) fn compile_nested(
        &mut self,
        parameters: &FormalParameters,
        body: Body<'_>,
        lexical: Lexical,
        span: Span,
    ) -> Result<Chunk, CompileError> {
        let mut outer = self.outer.clone();
        outer.push(self.locals.clone());
        compile_body(self.heap, parameters, body, outer, lexical, span)
    }

    /// File a compiled body under this chunk and emit the instruction that makes an object of it.
    fn emit_function(&mut self, body: Chunk, span: Span) -> Result<(), CompileError> {
        let index = u32::try_from(self.chunk.functions.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        // §15.3 again — an arrow's `arguments` is the enclosing function's, so a body that
        // reached outward for it is what tells this one to build the object.
        self.uses_arguments |= body.outer_arguments;
        self.chunk.functions.push(Rc::new(body));
        self.chunk.emit(Instruction::MakeFunction(index));
        Ok(())
    }

    /// §13.3.6 — a call: the callee, then the arguments, then the instruction.
    ///
    /// Left to right and callee first, which is observable the moment either has a side effect:
    /// `f()(g())` calls `f` before `g`, and `f(a(), b())` calls `a` before `b`.
    pub(super) fn call(
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

/// What a callable body is made of — §15.2's `FunctionBody` or §15.3's `ExpressionBody`.
///
/// Two shapes rather than two compilers, because everything before the body is the same for both
/// and the parameter rules written twice would be a refusal no test could reach.
pub(super) enum Body<'a> {
    /// A statement list: a function's body, or an arrow's `a => { … }`.
    Statements(&'a [Stmt]),
    /// An arrow's `a => b`, whose value is what the call answers.
    Expression(&'a Expr),
}

/// Whether the body binds `this` itself, or takes the one around it.
///
/// One flag rather than two near-identical compilers. The whole of §15.3's difference from §15.2
/// is carried here, and it reaches run time as [`Chunk::is_arrow`] because all three things an
/// arrow lacks — `this`, `prototype`, `[[Construct]]` — are decided there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lexical {
    /// An ordinary function: the call binds `this` from the receiver.
    No,
    /// An arrow: `this` is whatever it was where the arrow was written.
    Yes,
}

/// Compile one callable body into its own chunk.
///
/// A free function rather than a method because it needs the heap while the compiler that asked
/// for it also holds it — and because a body is genuinely a separate unit of code, with its own
/// slots numbered from zero.
fn compile_body(
    heap: &mut Heap,
    parameters: &FormalParameters,
    body: Body<'_>,
    outer: Vec<Vec<Local>>,
    lexical: Lexical,
    span: Span,
) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    compiler.is_script = false;
    compiler.outer = outer;
    compiler.chunk.arrow = lexical == Lexical::Yes;
    compiler.chunk.simple_parameters = parameters.is_simple();

    // §10.2.11 — the parameters are the first slots, in order, so an argument can be put in place
    // without the callee being consulted.
    //
    // A *pattern* takes a slot too, and an unnamed one: the argument has to land somewhere before
    // it can be taken apart, and the names inside the pattern are separate bindings that the body
    // shares with its `var`s. So the slot is hidden and the pattern reads from it below.
    for parameter in parameters.items.iter() {
        match &parameter.target {
            Binding::Identifier(name) => compiler.declare(&name.name),
            Binding::Pattern(_) => compiler.declare_hidden("argument"),
        };
    }
    compiler.chunk.parameters = compiler.locals.len();
    // §10.2.3 — `length` stops at the first parameter that has a default, and a rest parameter is
    // not counted at all. It is what the function says it needs, which is not how many places it
    // has to put things.
    compiler.chunk.length = parameters
        .items
        .iter()
        .position(|item| item.default.is_some())
        .unwrap_or(parameters.items.len());
    // §15.1 — the rest parameter is last and takes a slot of its own, which the *call* fills: the
    // arguments past the named parameters are on the stack at entry and reachable from nowhere
    // else, so building the array is the call's job and not the body's.
    if let Some(rest) = &parameters.rest {
        let Binding::Identifier(name) = rest.as_ref() else {
            return Err(unsupported("a destructuring rest parameter", span));
        };
        compiler.chunk.rest = Some(compiler.declare(&name.name));
    }
    // §10.2.11 steps 19 to 22 — the binding is made after the parameters and before the body, and
    // *not* when a parameter already took the name: `function f(arguments) { … }` has a parameter
    // called that and no arguments object at all.
    //
    // An arrow gets none either, for the same reason it gets no `this` (§15.3): the name reaches
    // outward to the function it is written inside, and `Chunk::outer_arguments` is how that
    // function hears about it.
    //
    // Step 19's other two halves — a hoisted `function arguments` and a `let arguments` — are not
    // consulted. They would suppress the object; here it is built and then written over by the
    // declaration that shares its slot, which no program can tell apart from its never having
    // existed. What it costs is one allocation in a function that has just said it will not look,
    // and reading the body twice to save it is the more expensive of the two.
    if lexical == Lexical::No && compiler.resolve("arguments").is_none() {
        compiler.arguments_slot = Some(compiler.declare_shadowing("arguments"));
    }

    // §10.2.11 step 24 — the defaults run *inside* the callee, before the body and after the
    // arguments object is made. So `arguments` holds what the call actually passed and a default
    // that filled in for a missing one is nowhere in it, which is right and is only visible
    // because the object is built first.
    //
    // Each is guarded by a comparison against `undefined` and not by a count of arguments:
    // §10.2.11 applies the default when the parameter *is* `undefined`, so passing one explicitly
    // takes it too. `f(undefined)` and `f()` agree, and that is the rule rather than a shortcut.
    for (at, parameter) in parameters.items.iter().enumerate() {
        let slot = u32::try_from(at).unwrap_or(u32::MAX);
        if let Some(default) = &parameter.default {
            compiler.chunk.emit(Instruction::LoadVariable(0, slot));
            compiler.constant(Value::Undefined)?;
            compiler
                .chunk
                .emit(Instruction::Binary(crate::ast::BinaryOperator::StrictEqual));
            let given = compiler.chunk.emit_jump(Instruction::JumpIfFalse);
            compiler.expression(default)?;
            compiler.chunk.emit(Instruction::StoreVariable(0, slot));
            // The store leaves its value behind, because an assignment is an expression. Here
            // nothing wants it.
            compiler.chunk.emit(Instruction::Pop);
            compiler.chunk.patch(given)?;
        }
        // …and *then* the pattern is taken apart, if it is one. The order is §10.2.11 step 24's:
        // the default stands in for a missing argument first, and what the pattern reads is
        // whichever of the two arrived. `function f({a} = {a: 1})` binds `a` to 1 when called
        // with nothing.
        match &parameter.target {
            // A name is already in its slot; there is nothing to take apart, and asking anyway
            // would store the slot back into itself.
            Binding::Identifier(_) => {}
            target => {
                compiler.chunk.emit(Instruction::LoadVariable(0, slot));
                compiler.destructure_parameter(target, span)?;
            }
        }
    }

    match body {
        Body::Statements(statements) => {
            // A function's own `var`s and inner declarations, on the same terms as a script's.
            for name in var_declared_names(statements) {
                compiler.declare(name.name);
            }
            // §10.2.11 step 34 — a function body's `let` and `const` are created with the call
            // and left uninitialised, exactly as a block's are.
            compiler.declare_lexical_names(statements)?;
            compiler.hoist_functions(statements)?;
            compiler.statements(statements)?;
            // §10.2.1 step 4 — falling off the end returns `undefined`. The instruction is emitted
            // unconditionally rather than only when the body might reach it: deciding *that* is a
            // reachability analysis, and a `Return` after one that always runs costs a byte.
            compiler.constant(Value::Undefined)?;
        }
        // §15.3.3 — `ExpressionBody : AssignmentExpression` is evaluated and *returned*, so there
        // is no `undefined` to fall through to and no hoisting to do: an expression declares
        // nothing.
        Body::Expression(expression) => compiler.expression(expression)?,
    }
    compiler.chunk.emit(Instruction::Return);
    // Only now is it known whether anything read the name — including an arrow written inside,
    // which says so on the chunk it hands back.
    compiler.chunk.arguments = match compiler.uses_arguments {
        true => compiler.arguments_slot,
        false => None,
    };
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
    pub(super) fn receiver(self, compiler: &mut Compiler<'_>) {
        if self == Self::Receiver {
            compiler.chunk.emit(Instruction::Duplicate);
        }
    }
}
