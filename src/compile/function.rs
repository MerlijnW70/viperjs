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
        self.make_method_function(function, naming, false, span)
    }

    /// The same, for a `MethodDefinition` — §15.4.5, which has no `[[Construct]]`.
    pub(super) fn make_method_function(
        &mut self,
        function: &Function,
        naming: Naming<'_>,
        method: bool,
        span: Span,
    ) -> Result<(), CompileError> {
        // §10.2.9 — a function expression that names itself takes that name, and it wins over the
        // position: `var a = function f() {}` is called `f`. `NamedEvaluation` only reaches an
        // *anonymous* one, which is what §8.6.3 says and what the callers here check.
        let naming = match &function.name {
            Some(written) => Naming::of(&written.name),
            None => naming,
        };
        let body = self.compile_nested_method(
            &function.parameters,
            Body::Statements(&function.body),
            naming,
            Strict::of(function.is_strict),
            Lexical::No,
            method,
            Generator::of(function.is_generator),
            Asynchrony::of(function.is_async),
            span,
        )?;
        self.emit_function(body, span)
    }

    /// §15.2.5 `InstantiateOrdinaryFunctionExpression` — a function expression, and the scope that
    /// holds its own name.
    ///
    /// Steps 3 to 5 and 9: a **named** function expression gets an environment of its own holding
    /// an immutable binding of its name, and the function closes over that environment. It is the
    /// only way such a function can refer to itself — an expression makes no binding outside — and
    /// it is why `var f = function g() { return g; }` answers the function where `g` is otherwise
    /// nowhere. §15.5.5, §15.6.4 and §15.8.4 defer to this for generators and `async`, so all four
    /// kinds arrive here.
    ///
    /// A **declaration** does not come this way and must not: its name is a binding of the scope
    /// around it, which is an ordinary mutable one — `function f() { f = 1; return f; }` called as
    /// a declaration answers 1, and as an expression answers the function.
    ///
    /// The environment is a real one and not merely a level of the compiler's, for DR-0018's
    /// reason: a scope only the compiler can see is one a direct `eval` written in the body cannot
    /// resolve into. That is the same argument §15.7.14's class body makes, and this is the same
    /// clause one production over.
    pub(super) fn make_function_expression(
        &mut self,
        function: &Function,
        naming: Naming<'_>,
        span: Span,
    ) -> Result<(), CompileError> {
        let Some(written) = &function.name else {
            // An anonymous one binds nothing a program can write, so there is no environment to
            // make. §8.6.3 may still have given it a name — that is `naming`, and it is a property
            // of the object rather than a binding anything resolves.
            return self.make_function(function, naming, span);
        };
        let opened = self.enter_environment();
        let mark = self.enter_scope();
        // No `Uninitialise` before it, unlike §15.7.14's class name. A class evaluates its heritage
        // and its computed keys inside its own scope, so its binding has a dead zone something can
        // reach; between this binding being made and step 9 filling it there is only the making of
        // the function object, which runs no code. An instruction nothing can observe is one no
        // test can pin.
        let slot = self.declare_lexical(&written.name, crate::heap::Mutability::OwnName);
        let made = self.make_function(function, naming, span);
        // Step 9's `InitializeBinding`, and it *peeks* rather than popping — the same terms as a
        // store — so the function it wrote is still on the stack as the expression's value.
        //
        // Emitted even when the body was refused, which is not carelessness: a compile that fails
        // anywhere discards the whole chunk, so instructions after the refusal are never run and
        // never read. A guard here would be a branch no input can reach, and mutation coverage
        // said so by surviving its removal.
        self.chunk.emit(Instruction::Initialise(slot));
        // Closed on the failing path too: a refusal deeper in the body leaves this compiler holding
        // a scope it is no longer inside, and the next thing compiled would resolve one hop wrong.
        self.leave_scope(mark);
        self.leave_environment(opened)?;
        made
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
        // §15.3.3's `ConciseBody` has two shapes and one meaning: `a => b` returns `b`, and
        // `a => { … }` is an ordinary body. The first is compiled as the second with the `return`
        // written in, which is what the grammar says rather than a shortcut.
        let shape = match &arrow.body {
            ArrowBody::Expression(expression) => Body::Expression(expression),
            ArrowBody::Block(body) => Body::Statements(body),
        };
        let body = self.compile_nested(
            &arrow.parameters,
            shape,
            naming,
            Strict::of(arrow.is_strict),
            Lexical::Yes,
            Asynchrony::of(arrow.is_async),
            span,
        )?;
        self.emit_function(body, span)
    }

    /// Compile a body written inside this one into a chunk of its own.
    ///
    /// What the nested body may see: the script's names, and — only to refuse against — the names
    /// of every function it is written inside. It is written inside this scope, so its chain is
    /// ours with ours on the end.
    #[allow(clippy::too_many_arguments)] // the body's shape, threaded rather than shared
    pub(super) fn compile_nested(
        &mut self,
        parameters: &FormalParameters,
        body: Body<'_>,
        naming: Naming<'_>,
        strict: Strict,
        lexical: Lexical,
        asynchrony: Asynchrony,
        span: Span,
    ) -> Result<Chunk, CompileError> {
        self.compile_nested_method(
            parameters,
            body,
            naming,
            strict,
            lexical,
            false,
            Generator::No,
            asynchrony,
            span,
        )
    }

    /// The same, saying whether the body is a `MethodDefinition` — §15.4.5.
    #[allow(clippy::too_many_arguments)] // one flag per fact the body carries, threaded rather than shared
    pub(super) fn compile_nested_method(
        &mut self,
        parameters: &FormalParameters,
        body: Body<'_>,
        naming: Naming<'_>,
        strict: Strict,
        lexical: Lexical,
        method: bool,
        generator: Generator,
        asynchrony: Asynchrony,
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
                method,
                generator: generator == Generator::Yes,
                is_async: asynchrony == Asynchrony::Yes,
                strict: match strict {
                    Strict::Yes => true,
                    Strict::No => false,
                    // A body praxis synthesises — a field initialiser, a static block — has no source
                    // of its own and so no directive: it is exactly as strict as what encloses it.
                    Strict::Inherited => self.chunk.strict,
                },
                naming,
                lexical,
                this_binding,
                inside_with: self.names_are_dynamic(),
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
        optional: bool,
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
        // §9.1.1.2.10 — a bare name inside a `with` may resolve to the object, and then the call
        // gets it as `this`. So it is compiled as a *method* call: the receiver is pushed under the
        // callee by one instruction, because the two answers come from one walk and asking twice
        // would ask an object that may have changed in between.
        let with_name = self.names_are_dynamic() && matches!(callee.kind, ExprKind::Identifier(_));
        let method = with_name
            || matches!(
                callee.kind,
                ExprKind::Member { .. } | ExprKind::ComputedMember { .. }
            );
        if let ExprKind::Identifier(name) = &callee.kind
            && with_name
        {
            let index = self.name(name)?;
            self.chunk.emit(Instruction::LoadNameForCall(index));
        } else if method {
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
        // §13.3.9 — `a?.()` gives up when the *callee* is nullish, which is one link later than
        // `a?.b()` gives up. The receiver a method call copied is under it and goes with it, which is
        // the one place a short circuit has more than the value itself to clear away.
        if optional {
            self.optional_call_link(method)?;
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
        // §13.3.6.1 — a call whose callee is written as the bare name `eval` may be a *direct*
        // eval, and only the compiler can see that it was written that way. Whether it really is
        // one depends on what that name turns out to hold, which only the interpreter can see, so
        // the instruction carries the question rather than an answer.
        let direct_eval = matches!(&callee.kind, ExprKind::Identifier(name) if name == "eval");
        // §10.2.11 step 19 makes an arguments object for every non-arrow function, and praxis skips
        // it when the compiler saw nothing read the name. A direct eval can read it — the source
        // does not exist yet, so there is nothing to have seen — and a slot nothing filled would
        // answer `undefined` where the specification has an object. Whether this body has such a
        // slot at all is a separate question, and one this flag does not decide.
        self.uses_arguments |= direct_eval;
        self.chunk.emit(match (method, direct_eval) {
            (true, _) => Instruction::CallMethod(count),
            (false, true) => Instruction::CallDirectEval(count),
            (false, false) => Instruction::Call(count),
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
        self.chunk.emit(Instruction::BindThis {
            depth: at.depth,
            index: at.index,
        });
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
    /// Whether the body is a `MethodDefinition` — §15.4.5, which decides `[[Construct]]`.
    method: bool,
    /// Whether the body is a generator's — §15.5, which decides whether calling it runs it.
    generator: bool,
    /// Whether the body is an `async` function's — §15.8, which decides what the call answers with.
    is_async: bool,
    /// Whether the body is strict code — §11.2.1, decided by the parser and carried here.
    ///
    /// Passed in rather than inherited from the enclosing compiler, because a body may *add*
    /// strictness with a directive of its own and the parser has already worked out the union. The
    /// one exception is a body praxis synthesises, which has no source to have a directive in and
    /// takes the enclosing answer — the callers that pass `Strict::Inherited`.
    strict: bool,
    /// What §10.2.9 calls the function, if the position it was written in says.
    naming: Naming<'a>,
    /// Whether the body binds `this` itself — §15.3's whole difference from §15.2.
    lexical: Lexical,
    /// The enclosing derived constructor's `this`, if this body may reach it — DR-0015.
    this_binding: Option<ThisSlot>,
    /// Whether this body is written inside a `with` — §14.11.
    ///
    /// Inherited rather than recomputed, and it has to be: the body's *own* scopes contain no
    /// `with`, but the chain it closes over does, so every name in it is still a run-time walk.
    /// `with (o) { function f() { return a; } }` is the program — `f` called long afterwards and
    /// from anywhere still reads `o.a`, because the environment it captured is the object's.
    inside_with: bool,
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

/// Whether a body is strict code, or takes the enclosing answer — §11.2.1.
///
/// Three values rather than a `bool`, because a body praxis *synthesises* has no source to carry a
/// directive and must not be read as sloppy: a field initialiser inside a class is strict because the
/// class is, and nothing in the tree says so on its behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Strict {
    /// The parser said so — a directive here or in something enclosing this.
    Yes,
    /// The parser said not.
    No,
    /// There is no source to ask: as strict as whatever this is being compiled inside.
    Inherited,
}

impl Strict {
    /// What the parser recorded, as this enum.
    pub(super) fn of(is_strict: bool) -> Self {
        match is_strict {
            true => Self::Yes,
            false => Self::No,
        }
    }
}

/// Whether the body is a generator's — §15.5's `*`.
///
/// A named flag rather than a `bool` beside the two others, for the reason [`Lexical`] is one: the
/// call sites pass three booleans in a row, and three `true`s in a row is a place to make a mistake
/// that compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Generator {
    /// An ordinary function: calling it runs its body.
    No,
    /// A generator: calling it makes a generator object and runs nothing — §15.5.4.
    Yes,
}

impl Generator {
    /// What the parser recorded, as this enum.
    pub(super) fn of(is_generator: bool) -> Self {
        match is_generator {
            true => Self::Yes,
            false => Self::No,
        }
    }
}

/// Whether the body is an `async` function's — §15.8's `async`.
///
/// A named flag beside [`Generator`] and for the same reason: the call sites pass several booleans
/// in a row, and one `true` in the wrong position compiles and runs a generator as an `async`
/// function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Asynchrony {
    /// An ordinary function: what it returns is what the call answers with.
    No,
    /// An `async` function: the call answers with a promise — §27.7.5.1.
    Yes,
}

impl Asynchrony {
    /// What the parser recorded, as this enum.
    pub(super) fn of(is_async: bool) -> Self {
        match is_async {
            true => Self::Yes,
            false => Self::No,
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
    compiler.chunk.method = nesting.method;
    compiler.chunk.generator = nesting.generator;
    compiler.chunk.is_async = nesting.is_async;
    compiler.chunk.strict = nesting.strict;
    compiler.chunk.name = nesting.naming.spelled().map(|name| {
        compiler
            .heap
            .intern(&name.encode_utf16().collect::<Vec<_>>())
    });
    compiler.is_script = false;
    compiler.global_vars = false;
    // The scopes this body is written *inside*, which every depth it resolves is measured against
    // — see `Compiler::own_depth`.
    compiler.seeded_scopes = outer.len();
    compiler.with_depth = u32::from(nesting.inside_with);
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

    // §15.5.4 and §27.6.2 — the parameters are done, so this is where the generator is made and
    // everything after it becomes the parked body. Above the `async` handler below, because that
    // handler belongs to the *body*: a throw from a parameter default is the caller's to catch,
    // and a throw from the body rejects the promise the resumption answered with.
    if nesting.generator {
        compiler.chunk.emit(Instruction::GeneratorStart);
    }

    // §27.7.5.2 — an `async` function's body is wrapped in a handler the source never wrote. A
    // throw that nothing inside caught does not travel to the caller: it *rejects the promise*, and
    // the caller is handed that promise like any other. Written as a handler because that is what
    // the unwinder already does — the alternative is teaching every throw in the engine which
    // frames are `async`.
    let rejecting = nesting
        .is_async
        .then(|| compiler.chunk.emit_jump(Instruction::PushHandler));

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
                    Strict::Inherited,
                    Lexical::No,
                    Asynchrony::No,
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
    // The handler's target, reached only by a throw. `Return` above has already taken the handler
    // down — it truncates to the frame's mark — so the ordinary way out never arrives here.
    if let Some(rejecting) = rejecting {
        compiler.chunk.patch(rejecting)?;
        compiler.chunk.emit(Instruction::AsyncReject);
    }
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
