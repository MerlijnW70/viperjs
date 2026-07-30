//! §15.2 — what a function expression compiles to, and what a call compiles to.
//!
//! A function body is a chunk of its own, compiled by a compiler of its own. That is not
//! tidiness: a body's slots are counted from the bottom of *its* environment, so the two
//! numberings must not share a table — and the separation is what makes a nested body unable to
//! reach a slot it has no environment for.

use super::{
    CompileError, Compiler, ErrorKind, Instruction, Local, THIS_BINDING, ThisSlot, unsupported,
};
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
        naming: Naming<'_>,
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
        // §10.2.9 — a function expression that names itself takes that name, and it wins over the
        // position: `var a = function f() {}` is called `f`. `NamedEvaluation` only reaches an
        // *anonymous* one, which is what §8.6.3 says and what the callers here check.
        let naming = match &function.name {
            Some(written) => Naming::of(&written.name),
            None => naming,
        };
        let body = self.compile_nested(
            &function.parameters,
            Body::Statements(&function.body),
            naming,
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
        naming: Naming<'_>,
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
        let body = self.compile_nested(&arrow.parameters, shape, naming, Lexical::Yes, span)?;
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
        naming: Naming<'_>,
        lexical: Lexical,
        span: Span,
    ) -> Result<Chunk, CompileError> {
        let mut outer = self.outer.clone();
        outer.push(self.locals.clone());
        // DR-0015's propagation rule, and the only place it is applied. An arrow reaches outward for
        // `this`, so it inherits the enclosing derived constructor's binding one environment further
        // out; a non-arrow function is handed a `this` of its own by the call, so it inherits
        // nothing — without that clearing, a method written inside a derived constructor would
        // resolve the binding through the chain and answer the enclosing instance.
        let this_binding = match lexical {
            Lexical::Yes => self.this_binding.map(|at| ThisSlot {
                depth: at.depth + 1,
                index: at.index,
            }),
            Lexical::No => None,
        };
        compile_body(
            self.heap,
            parameters,
            body,
            outer,
            Nesting {
                naming,
                lexical,
                this_binding,
            },
            span,
        )
    }

    /// File a compiled body under this chunk and answer its index, emitting nothing.
    ///
    /// Separate from [`Compiler::emit_function`] for the one body that is filed now and made later:
    /// a derived class's field initialiser is compiled with the constructor and its object is built
    /// by each `super()`, which may be anywhere in the body or not reached at all.
    pub(super) fn file_function(&mut self, body: Chunk, span: Span) -> Result<u32, CompileError> {
        let index = u32::try_from(self.chunk.functions.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span,
        })?;
        // §15.3 again — an arrow's `arguments` is the enclosing function's, so a body that
        // reached outward for it is what tells this one to build the object.
        self.uses_arguments |= body.outer_arguments;
        self.chunk.functions.push(Rc::new(body));
        Ok(index)
    }

    /// File a compiled body under this chunk and emit the instruction that makes an object of it.
    pub(super) fn emit_function(&mut self, body: Chunk, span: Span) -> Result<(), CompileError> {
        let index = self.file_function(body, span)?;
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
        // §13.3.7 — `super(…)` names no callee at all: the parent is the running function's
        // `[[Prototype]]`, so there is nothing to evaluate and push. It is answered first because
        // every branch below assumes a callee is on the stack.
        if matches!(callee.kind, ExprKind::Super) {
            return self.super_call(arguments, span);
        }
        let method = matches!(
            callee.kind,
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. }
        );
        if method {
            // The base is evaluated once and copied *before* the key, so the stack ends as
            // [receiver, method] with nothing between them. Copying after the key would leave the
            // key underneath, and evaluating the base twice would run `f()` twice in `f().m()`.
            let reference = self.property_reference(callee, Keep::Receiver)?;
            // §13.3.7.1 — `super.m()` calls with `this` as the receiver and not with the object the
            // method was found on, which is what makes a parent's method see the instance. The copy
            // above was of `this` for exactly that reason.
            self.chunk.emit(reference.get());
        } else {
            self.expression(callee)?;
        }
        // §13.3.8 — a spread has no argument count until it has been iterated, so the list cannot be
        // pushed one value at a time. It is gathered into an array instead, by the same code an array
        // literal uses, and expanded again by the call.
        if arguments
            .iter()
            .any(|argument| matches!(argument, Argument::Spread(_)))
        {
            self.argument_array(arguments)?;
            let how = match method {
                true => crate::compile::chunk::SpreadCall::Method,
                false => crate::compile::chunk::SpreadCall::Plain,
            };
            self.chunk.emit(Instruction::CallSpread(how));
            return Ok(());
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

    /// §13.3.7.1 `SuperCall` — construct the parent, bind `this` to it, and initialise the fields.
    ///
    /// Three things in that order, and the order is the specification's. The parent makes the object
    /// (step 5); `BindThisValue` makes it this constructor's `this` (step 6), which is also what makes
    /// a second `super()` a ReferenceError rather than a second construction; and only then does
    /// `InitializeInstanceElements` run (step 7), because until now there was nothing to put a field
    /// on. A field initialiser can therefore read `this` and see the parent's work, which is the whole
    /// reason the order is observable.
    ///
    /// The object is left on the stack: §13.3.7.1 step 8 makes it the value of the expression, and an
    /// expression statement will discard it like any other.
    fn super_call(&mut self, arguments: &[Argument], span: Span) -> Result<(), CompileError> {
        // The parser has already refused `super(…)` anywhere but a derived constructor — §15.7.1
        // makes it a Syntax Error even in a base class's — so reaching here without a binding to fill
        // would mean the two disagreed about which bodies those are. Both facts are asked for at
        // once, and by the same guard: a derived constructor has both or the compiler has lost track
        // of one, and a second check for the second one would be a branch nothing could reach.
        let (Some(at), Some(fields)) = (self.this_binding, self.derived_fields) else {
            return Err(unsupported("`super` outside a derived constructor", span));
        };
        if arguments
            .iter()
            .any(|argument| matches!(argument, Argument::Spread(_)))
        {
            self.argument_array(arguments)?;
            self.chunk.emit(Instruction::CallSpread(
                crate::compile::chunk::SpreadCall::Super,
            ));
        } else {
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
            self.chunk.emit(Instruction::SuperCall(count));
        }
        // Peeks, so the object stays for the fields below and for the expression's value.
        self.chunk.emit(Instruction::BindThis(at.index));
        // Called with the object as its receiver, because §15.7.14 evaluates a field initialiser with
        // `this` bound to the instance and a call is the only thing that binds a receiver. The same
        // shape a static field's initialiser uses.
        self.chunk.emit(Instruction::Duplicate);
        self.chunk.emit(Instruction::MakeFunction(fields));
        // §15.7.14 makes a field initialiser a method of the class's prototype, which is the home the
        // *constructor* has — so the synthesised body takes the running function's rather than being
        // told a prototype it has no way to reach from here.
        self.chunk.emit(Instruction::InheritHome);
        self.chunk.emit(Instruction::CallMethod(0));
        self.chunk.emit(Instruction::Pop);
        Ok(())
    }

    /// Gather an argument list that contains a spread into one array, left to right.
    ///
    /// The same shape as an array literal's, and deliberately so: `f(...a)` and `[...a]` iterate by
    /// exactly the rules of §7.4, and writing that out twice is how the two come to disagree about a
    /// custom iterator. The running index is a hidden slot because a spread contributes a count that
    /// is not known until it has run.
    pub(super) fn argument_array(&mut self, arguments: &[Argument]) -> Result<(), CompileError> {
        let at = self.declare_hidden("argument");
        self.chunk.emit(Instruction::NewArray(0));
        self.constant(Value::Number(0.0))?;
        self.chunk.emit(Instruction::StoreVariable(0, at));
        self.chunk.emit(Instruction::Pop);
        for argument in arguments {
            match argument {
                Argument::Value(value) => {
                    self.chunk.emit(Instruction::LoadVariable(0, at));
                    self.expression(value)?;
                    self.chunk.emit(Instruction::DefineField);
                    self.bump(at)?;
                }
                Argument::Spread(value) => {
                    self.expression(value)?;
                    self.spread_into(at)?;
                }
            }
        }
        let name = self.name_of("length");
        self.chunk.emit(Instruction::Duplicate);
        self.constant(Value::String(name))?;
        self.chunk.emit(Instruction::LoadVariable(0, at));
        self.chunk.emit(Instruction::SetProperty);
        self.chunk.emit(Instruction::Pop);
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
    /// A constructor's body, preceded by the instance fields §15.7.14 initialises before it.
    ///
    /// The fields are carried here rather than compiled by the caller because they belong *inside*
    /// this chunk: `this` is only bound within the constructor, and a field initialiser is evaluated
    /// with it. Prepending them to the code afterwards would move every jump target in the body.
    Constructor {
        /// The instance fields, in source order — which is the order they are initialised in.
        fields: &'a [&'a crate::ast::ClassField],
        /// What the author wrote.
        statements: &'a [Stmt],
        /// The private methods and accessors every instance must carry — §15.7.14 steps 1 and 2.
        ///
        /// Owned names rather than a borrow of the tree, because this list has to survive into a
        /// compiler whose lifetime is the heap's. See [`super::class::instance_private_method_names`].
        private_methods: Vec<(Box<str>, super::class::PrivateKind)>,
        /// Whether the class has an `extends` clause — §10.2.2's `[[ConstructorKind]]`.
        ///
        /// Three things follow from it and nothing else does: `this` becomes a binding that starts
        /// out unbound (DR-0015), the fields are initialised by `super()` rather than on entry
        /// (§15.7.14 runs `InitializeInstanceElements` after the parent has made the object), and a
        /// `return` obeys §10.2.2 step 13's stricter rule.
        derived: bool,
    },
}

/// What a nested body inherits from the one it is written inside.
///
/// Two facts that travel together and are decided at the same moment, so a struct rather than two
/// parameters: the second is *computed from* the first, and passing them separately would let a
/// caller pair an arrow's reach with a function's boundary.
pub(super) struct Nesting<'a> {
    /// What §10.2.9 calls the function, if the position it was written in says.
    naming: Naming<'a>,
    /// Whether the body binds `this` itself — §15.3's whole difference from §15.2.
    lexical: Lexical,
    /// The enclosing derived constructor's `this`, if this body may reach it — DR-0015.
    this_binding: Option<ThisSlot>,
}

/// What a function is called, and where the compiler learned it — §10.2.9 and §8.6.3.
///
/// Three sources and they do not compete: a `Function` may carry its own name, a method takes its
/// key's, and an anonymous expression in a named position takes the binding's. Which one applies is
/// decided by the caller, because only the caller knows the position.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Naming<'a> {
    /// The name itself, if there is one.
    pub(super) name: Option<&'a str>,
    /// `get ` or `set ` — §10.2.9's prefix, which is part of the name and not decoration.
    ///
    /// `Object.getOwnPropertyDescriptor(o, 'a').get.name` is `"get a"`, and test262 checks it.
    pub(super) prefix: Option<&'static str>,
}

impl<'a> Naming<'a> {
    /// The name a function in this position is given, spelled out.
    fn spelled(self) -> Option<String> {
        let name = self.name?;
        Some(match self.prefix {
            Some(prefix) => format!("{prefix}{name}"),
            None => name.to_string(),
        })
    }

    /// A plain name with no prefix.
    pub(super) fn of(name: &'a str) -> Self {
        Self {
            name: Some(name),
            prefix: None,
        }
    }
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
    nesting: Nesting<'_>,
    span: Span,
) -> Result<Chunk, CompileError> {
    let lexical = nesting.lexical;
    let mut compiler = Compiler::new(heap);
    // §10.2.9 — interned, so that the hundred `function f` in a program share one String and so that
    // the key made from it is the one the object already has.
    compiler.chunk.name = nesting.naming.spelled().map(|name| {
        compiler
            .heap
            .intern(&name.encode_utf16().collect::<Vec<_>>())
    });
    compiler.is_script = false;
    compiler.outer = outer;
    compiler.this_binding = nesting.this_binding;
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

    // DR-0015 — a derived constructor's `this` is a binding, and it starts out unbound. Declared
    // here, above the parameter defaults, because a default may read `this`:
    // `constructor(x = this.y) {}` in a derived class is §10.2.2's ReferenceError, and it can only
    // be one if the binding already exists to be found in the dead zone.
    //
    // The slot is put back into that state explicitly. A fresh environment gives every slot
    // `undefined` rather than nothing, so without this the read would answer `undefined` — a silent
    // wrong answer where the specification asks for a throw.
    if matches!(body, Body::Constructor { derived: true, .. }) {
        let index = compiler.declare_shadowing(THIS_BINDING);
        compiler.chunk.emit(Instruction::Uninitialise(index));
        compiler.this_binding = Some(ThisSlot { depth: 0, index });
        compiler.chunk.derived_this = Some(index);
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
            // §8.6.3 again — `SingleNameBinding : BindingIdentifier Initializer` is a named position,
            // so `function f(a = () => {})` calls the arrow `a`. A pattern's is not, for the reason a
            // destructuring target's is not: it binds several names and none of them is *the* name.
            match &parameter.target {
                Binding::Identifier(name) => compiler.named_evaluation(&name.name, default)?,
                Binding::Pattern(_) => compiler.expression(default)?,
            }
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
        // §15.7.14 — the fields first, then the body. `InitializeInstanceElements` runs before the
        // constructor's first statement, so a field is already there when the body looks.
        Body::Constructor {
            fields,
            statements,
            derived,
            private_methods,
        } => {
            // §15.7.14 — a base class initialises its fields before the body's first statement,
            // because `this` is already there to put them on. A derived class cannot: there is no
            // object until `super()` has made one, so `InitializeInstanceElements` moves to the
            // `super()` itself and the fields are carried there instead.
            if derived {
                // Compiled once, here, and *called* by every `super()` — see
                // [`Compiler::derived_fields`] for why it is a body rather than a stored list.
                //
                // Built even when the class has no fields. Skipping it would save a function object
                // and a call per construction, which is a real cost and would be worth having — but
                // initialising nothing is indistinguishable from not initialising, so the guard is a
                // branch no input can pin, and mutation coverage duly survived forcing it. An
                // optimisation gets a benchmark in front of it before it gets a branch, and there is
                // no benchmark here yet; `lab/` is where that argument would be made.
                let initialiser = compiler.compile_nested(
                    &FormalParameters {
                        items: Box::new([]),
                        rest: None,
                        span,
                    },
                    Body::Constructor {
                        fields,
                        statements: &[],
                        derived: false,
                        private_methods: private_methods.clone(),
                    },
                    Naming::default(),
                    Lexical::No,
                    span,
                )?;
                compiler.derived_fields = Some(compiler.file_function(initialiser, span)?);
            } else {
                // §15.7.14 steps 1 to 4 — the methods first, then the fields. The order is
                // observable: a field initialiser may call a private method.
                compiler.instance_private_methods(&private_methods)?;
                compiler.instance_fields(fields)?;
            }
            for name in var_declared_names(statements) {
                compiler.declare(name.name);
            }
            compiler.declare_lexical_names(statements)?;
            compiler.hoist_functions(statements)?;
            compiler.statements(statements)?;
            compiler.constant(Value::Undefined)?;
        }
    }
    // §10.2.2 step 13 — falling off the end of a derived constructor is a `return undefined`, and
    // that is answered with the bound `this`. Which is also how a constructor that never called
    // `super()` becomes the ReferenceError the specification asks for, rather than answering with
    // nothing: the binding is still unbound, and reading it throws.
    if let Some(slot) = compiler.chunk.derived_this {
        compiler
            .chunk
            .emit(Instruction::CompleteDerivedReturn(slot));
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
