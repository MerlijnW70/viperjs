//! The syntax tree to bytecode — §13's evaluation semantics, rearranged.
//!
//! # Why bytecode rather than walking the tree
//!
//! A tree-walking interpreter is shorter and would work. Bytecode is here for two reasons that
//! are not speed. The first is that generators and `async` functions have to *suspend*, and a
//! suspended tree walk is a stack of Rust frames that cannot be put down; a suspended bytecode
//! frame is an index. The second is that the flattening happens once per function rather than
//! once per execution, which is where the speed comes from and is not why it is here.
//!
//! Where a compiler's parts live.
//!
//! - `annex_b` — §B.3.3, which of a body's block-level function declarations also get a `var`.
//! - `binding` — §8.6.2's binding patterns, and the declarations that hold one.
//! - `chunk` — the code an embedder holds, and the instruction set.
//! - `class` — §15.7's `ClassDefinitionEvaluation`.
//! - `delegate` — §27.5.3.7 step 7's `yield*`, as the loop the clause describes.
//! - `expression` — §13, and everything that leaves one value on the stack.
//! - `for_await` — §14.7.5 over an async iterator, and the two awaits an ordinary loop has not.
//! - `function` — §15.2, where a body becomes a chunk of its own and a call is emitted.
//! - `pattern` — §13.15.5's destructuring *assignment*, which writes references rather than
//!   making names. Its twin is `binding`, and the two are apart because the syntax trees are.
//! - `statement` — §14, and everything that leaves the stack as it found it.
//! - here — how a name is resolved, and the scopes a resolution walks.
//!
//! # What it refuses
//!
//! A construct the compiler has not been taught is a [`ErrorKind::Unsupported`] carrying the span
//! of the thing it could not do — a refusal with a location, not a panic and not a silent wrong
//! answer. **The refusals are the list**: `grep unsupported(` says what is missing, and it is
//! short. Read the spans as well as the messages — several name a construct in one *position*
//! rather than the construct itself, so "a spread argument" is refused inside a `super()` call
//! and nowhere else.
//!
//! There is no prose inventory here, deliberately. One would be a second copy of that list which
//! nothing checks, and this file held one for long enough to be describing a compiler that could
//! manage literals and the unary operators.

use crate::ast::{Expr, Module, Script, Stmt};
use crate::value::Value;
mod annex_b;
mod binding;
mod chunk;
mod class;
mod delegate;
mod expression;
mod for_await;
mod function;
mod pattern;
mod statement;

pub use self::chunk::{
    Chunk, Deletable, ExportEntry, ExportSource, ImportEntry, Instruction, NamePrefix, Scope,
    ShortCircuit, SpreadCall, Template,
};

use self::chunk::Unpatched;
use crate::heap::{Heap, Mutability};
use crate::span::Span;
use crate::static_semantics::var_declared_names;
use std::rc::Rc;

/// Why a program could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// Where — the span of the construct that could not be compiled.
    pub span: Span,
}

/// The kinds of compilation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A construct the compiler does not handle yet, named as the specification names it.
    ///
    /// Not a syntax error: the parser accepted it and the tree is well-formed. This is the engine
    /// saying what it has not been taught, and it exists so that the answer to "does ViperJS
    /// support X" is a message with a span rather than a wrong value.
    Unsupported(&'static str),
    /// More than `u32::MAX` constants in one chunk.
    TooManyConstants,
    /// More than `u32::MAX` instructions in one chunk, so a jump could not name its target.
    TooLong,
    /// An expression nested deeper than the compiler will walk.
    TooDeep,
    /// §22.2.1.1 — a regular expression literal whose pattern is not one.
    ///
    /// An **early** error, and that is the whole point of it being here. §12.9.5 accepts the
    /// literal's shape without reading it as a pattern, and then says the body must parse as one
    /// *at parse time* — so `if (false) { /(/ }` is a script that does not run at all, and a
    /// version that threw when the literal was evaluated would run it.
    BadPattern(&'static str),
}

impl CompileError {
    /// A sentence describing the failure, without the span.
    pub fn message(&self) -> String {
        match self.kind {
            ErrorKind::Unsupported(what) => format!("{what} is not implemented yet"),
            ErrorKind::BadPattern(why) => why.to_string(),
            ErrorKind::TooManyConstants => "too many constants in one unit of code".to_string(),
            ErrorKind::TooLong => "too many instructions in one unit of code".to_string(),
            ErrorKind::TooDeep => "an expression nested too deeply to compile".to_string(),
        }
    }
}

/// Compile one expression into a chunk whose completion value is the expression's.
///
/// Takes the heap because a String literal is a heap value and the compiler is where it is made.
pub fn compile_expression(expression: &Expr, heap: &mut Heap) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    compiler.expression(expression)?;
    compiler.chunk.emit(Instruction::SetCompletion);
    Ok(compiler.finish())
}

/// Compile a whole script — §16.1.7, whose value is the completion value of its body.
///
/// Every `var` in the script is given a slot before anything runs and starts as `undefined`, which
/// is what hoisting *is*: `x` is readable before its declaration and holds nothing.
pub fn compile_script(script: &Script, heap: &mut Heap) -> Result<Chunk, CompileError> {
    compile_script_bindings(script, heap, Deletable::No)
}

/// The same, for the Script an **indirect** `eval` evaluates — §19.2.1.1.
///
/// One difference from [`compile_script`] and it is not the scope chain: an indirect `eval` has the
/// global one, exactly as a Script does, which is why the two share everything below. What differs
/// is §9.1.1.4.17's `D`. §19.2.1.1 step 8 creates the global bindings **deletable**, so
/// `eval("var x = 1"); delete x` is `true` and leaves nothing behind, where the same two statements
/// written directly in a script cannot delete anything.
///
/// ```
/// use viperjs::compile::{Deletable, Instruction, compile_eval, compile_script};
/// use viperjs::heap::Heap;
/// use viperjs::parser::parse_script;
///
/// let mut heap = Heap::default();
/// let script = parse_script("var x = 1;").expect("this parses");
/// let deletable = |chunk: &viperjs::compile::Chunk| {
///     chunk.code().iter().find_map(|instruction| match instruction {
///         Instruction::DeclareGlobal { deletable, .. } => Some(*deletable),
///         _ => None,
///     })
/// };
/// let evaluated = compile_eval(&script, &mut heap).expect("this compiles");
/// let plain = compile_script(&script, &mut heap).expect("this compiles too");
/// assert_eq!(deletable(&evaluated), Some(Deletable::Yes));
/// assert_eq!(deletable(&plain), Some(Deletable::No), "a script's `var` is permanent");
/// ```
pub fn compile_eval(script: &Script, heap: &mut Heap) -> Result<Chunk, CompileError> {
    compile_script_bindings(script, heap, Deletable::Yes)
}

/// What both of those do, with `deletable` the one thing they disagree about.
fn compile_script_bindings(
    script: &Script,
    heap: &mut Heap,
    deletable: Deletable,
) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    // §11.2.1 — a Script is strict only if its own Directive Prologue says so, and everything nested
    // inherits that. A Module always is, which is M7's to record.
    compiler.chunk.strict = script.is_strict;
    compiler.deletable = deletable;
    // §16.1.7 step 8's `GlobalDeclarationInstantiation`. A script's `var`s belong to the *global
    // object*, not to a scope of its own — which is why `var x = 1` at the top level makes
    // `globalThis.x` and a `let` never will. See `Compiler::instantiate_globals` for the three
    // passes and why their order is observable.
    compiler.instantiate_globals(&script.body)?;
    // §B.3.3.2's `CreateGlobalVarBinding` for each block-level function declaration that earns one,
    // alongside the `var`s and before anything runs — which is what makes `f` a property of the
    // global object holding `undefined` at the top of a script whose `{ function f() {} }` is below.
    if !script.is_strict {
        compiler.declare_block_functions(&script.body, &std::collections::HashSet::new())?;
    }
    // §16.1.7 `GlobalDeclarationInstantiation` step 17 — a script's `let` and `const` go in the
    // global *declarative* record rather than onto the global object, which is why these get slots
    // like any other lexical binding while the `var`s above became properties.
    compiler.declare_lexical_names(&script.body)?;
    compiler.hoist_functions(&script.body)?;
    compiler.statements(&script.body)?;
    Ok(compiler.finish())
}

/// Compile a whole Module — §16.2.1.6's `ExecuteModule`, as far as its own body goes.
///
/// A Module differs from a Script in four ways that all land here, and none of them is the
/// grammar's `import` and `export`:
///
/// - It is **always strict** (§11.2.2), with no directive needed and none able to turn it off.
/// - Its top-level declarations are **its own**, not the global object's. `var x = 1` in a module
///   makes no `globalThis.x`, which is the split §16.1.7 makes for a Script reversed.
/// - A function declared at the top level is *lexically* scoped: §16.2.1.1 asks a
///   `ModuleItemList` for its `LexicallyDeclaredNames` where a `Script` is asked for its
///   `TopLevelLexicallyDeclaredNames`, so `function f() {} function f() {}` is a redeclaration in
///   a module and fine in a script.
/// - Its `this` is `undefined` (§16.2.1.6), which is the one place the two goal symbols disagree
///   about it and is the caller's to set — see [`crate::vm::Vm::run_module`].
///
/// `import` and `export` are refused here by name. They need a module *record* — a resolver, a
/// link step, and bindings that point into another module's scope — and that is the rest of M7;
/// a body that declares nothing across a module boundary needs none of it and runs today.
pub fn compile_module(module: &Module, heap: &mut Heap) -> Result<Chunk, CompileError> {
    let (chunk, saw_await) = compile_module_body(module, heap, false)?;
    // §16.2.1.5.3 — the body contained an `await` at its top level, so the module is asynchronous
    // and its body needs an execution to park into. Compiled again rather than patched, because
    // what an async body needs is not only an epilogue: the handler that turns an escaping throw
    // into a rejection is installed *before* the first instruction, and by the time the first
    // `await` was met there were already instructions in front of it.
    //
    // Only a module that really uses one pays for the second walk, and there is no third: the
    // second pass allows `await`, so it cannot discover anything the first did not.
    match saw_await {
        true => compile_module_body(module, heap, true).map(|(chunk, _)| chunk),
        false => Ok(chunk),
    }
}

/// One pass of [`compile_module`], with `asynchronous` deciding whether the body may park.
///
/// Answers the chunk and whether an `await` was met that this pass had no execution for — which is
/// always `false` when `asynchronous`, since then there was one.
fn compile_module_body(
    module: &Module,
    heap: &mut Heap,
    asynchronous: bool,
) -> Result<(Chunk, bool), CompileError> {
    let mut compiler = Compiler::new(heap);
    compiler.chunk.strict = true;
    // §16.2.1.5.3 — an asynchronous module's body is entered like an `async` function's and answers
    // a promise, so it carries the same brand and the same wrapper.
    compiler.chunk.is_async = asynchronous;
    // Script code for the two questions that flag decides — §14.2.2's completion value, and
    // `return` at the top level being a Syntax Error — and *not* for where a `var` goes.
    compiler.global_vars = false;
    // §16.2.1.5.2 step 9 — the import bindings are created **first**, before anything the body
    // declares, and they are immutable: `import {a} from "x"; a = 1` is a TypeError, which is what
    // `Mutability::Const` on the binding says. They are also *uninitialised* until the graph is
    // linked, so a name read before its module is evaluated finds a dead zone rather than
    // `undefined`.
    for item in &module.body {
        let crate::ast::ModuleItem::Import(declaration) = item else {
            continue;
        };
        declare_imports(&mut compiler, declaration);
    }
    let statements = module_statements(module)?;
    // §16.2.1.6 step 4's `InitializeEnvironment`, minus the imports: a module's `var`s are slots
    // of its own scope, hoisted before anything runs exactly as a function body's are.
    for name in var_declared_names(&statements) {
        compiler.declare(name.name);
    }
    compiler.declare_lexical_names(&statements)?;
    // §27.7.5.2 — the same wrapper an `async` function body gets, and for the same reason: a throw
    // that nothing inside catches does not travel to whoever evaluated the module, it **rejects the
    // module's promise**. Installed before the first instruction of the body, which is why knowing
    // that this module is asynchronous cannot wait until the `await` is met.
    let rejecting = asynchronous.then(|| compiler.chunk.emit_jump(Instruction::PushHandler));
    // §16.2.3.7 — `export default` of anything but a `HoistableDeclaration` is a **lexical**
    // declaration of `*default*`, so the name is created here and left uninitialised. A module can
    // reach it: its own namespace object exports `default`, and reading that before the statement
    // has run is §10.4.6.8's dead zone rather than `undefined`. Skipped for a function default,
    // which §16.2.3.7 hoists initialised like any other function declaration.
    let default_slot = match module.body.iter().find_map(default_expression) {
        Some(DefaultKind::Lexical) => {
            let slot = compiler.declare_lexical(DEFAULT_EXPORT, Mutability::Mutable);
            compiler.chunk.emit(Instruction::Uninitialise(slot));
            Some(slot)
        }
        Some(DefaultKind::Hoisted) | None => None,
    };
    compiler.hoist_functions(&statements)?;
    // §16.2.3.7 — an anonymous function default is a `HoistableDeclaration`, so it is made and
    // bound *here* with the named ones rather than where it is written. Apart from
    // `hoist_functions` only because that walk is over statements and this declaration is not one:
    // it has no name, which is the shape no other position in the grammar admits.
    let default_slot = match module.body.iter().find_map(hoisted_default) {
        Some(statement) => {
            let slot = compiler.declare_lexical(DEFAULT_EXPORT, Mutability::Mutable);
            compiler.default_declaration(statement)?;
            compiler.chunk.emit(Instruction::Initialise(slot));
            compiler.chunk.emit(Instruction::Pop);
            Some(slot)
        }
        None => default_slot,
    };
    // The body, in source order and item by item. Not `statements` again: `export default <expr>`
    // is not a statement and has to be compiled where it stands, because what it evaluates runs
    // between the statements around it.
    for item in &module.body {
        match item {
            crate::ast::ModuleItem::Statement(statement) => compiler.statement(statement)?,
            crate::ast::ModuleItem::Import(_) => {}
            crate::ast::ModuleItem::Export(declaration) => match &declaration.kind {
                crate::ast::ExportKind::Declaration(statement) => compiler.statement(statement)?,
                crate::ast::ExportKind::Default(crate::ast::ExportDefault::Declaration(
                    statement,
                )) => match declared_name(statement) {
                    // Named: an ordinary declaration, already hoisted, and the export below points
                    // at the binding it made.
                    Some(_) => compiler.statement(statement)?,
                    // Anonymous, so there is no binding to point at and one was made under the
                    // name §16.2.3.7 gives it. A *function* was hoisted above and has nothing
                    // left to do here; a class was left in its dead zone and is built where it
                    // stands.
                    None => {
                        if !matches!(statement.kind, crate::ast::StmtKind::Function(_)) {
                            let slot = default_slot
                                .unwrap_or_else(|| unreachable_default_slot(&mut compiler));
                            compiler.default_declaration(statement)?;
                            compiler.chunk.emit(Instruction::Initialise(slot));
                            compiler.chunk.emit(Instruction::Pop);
                        }
                    }
                },
                // §16.2.3.7 — the value is bound to a name no source can spell and exported as
                // `default`. `*default*` is the specification's own spelling for it, and the `*`
                // is what makes it unreachable from the module that declares it.
                crate::ast::ExportKind::Default(crate::ast::ExportDefault::Expression(value)) => {
                    let slot =
                        default_slot.unwrap_or_else(|| unreachable_default_slot(&mut compiler));
                    // §8.6.3 — `export default function () {}` is a named position and the name is
                    // `"default"`, which is the one place a function is called that.
                    compiler.named_evaluation("default", value)?;
                    compiler.chunk.emit(Instruction::Initialise(slot));
                    compiler.chunk.emit(Instruction::Pop);
                }
                crate::ast::ExportKind::Named(_) => {}
                crate::ast::ExportKind::All { .. } | crate::ast::ExportKind::NamedFrom { .. } => {}
            },
        }
    }
    // §16.2.1.3's `ExportEntries`, gathered *after* the body so that every name they point at has
    // the slot it will keep. An export is not a declaration and adds nothing: `export var a = 1`
    // declares `a` exactly as the same line without the word would, and the export is a second
    // fact about the name.
    for item in &module.body {
        let crate::ast::ModuleItem::Export(declaration) = item else {
            continue;
        };
        record_exports(&mut compiler, declaration)?;
    }
    // §16.2.1.5.3 — reaching the end of an asynchronous module's body *fulfils* its promise, which
    // is what `Return` does for an `async` function and is why the body ends with one. A module that
    // is not asynchronous simply runs off the end of its chunk, as a script does.
    if let Some(rejecting) = rejecting {
        compiler.constant(Value::Undefined)?;
        compiler.chunk.emit(Instruction::Return);
        // The handler's target, reached only by a throw. `Return` above has already taken the
        // handler down, so the ordinary way out never arrives here.
        compiler.chunk.patch(rejecting)?;
        compiler.chunk.emit(Instruction::AsyncReject);
    }
    let saw_await = compiler.saw_top_level_await;
    Ok((compiler.finish(), saw_await))
}

/// §16.2.1.3 — one `import`'s entries, as slots the link step will bind to another module's.
fn declare_imports(compiler: &mut Compiler<'_>, declaration: &crate::ast::ImportDeclaration) {
    use crate::ast::ImportClause;
    let specifier: Box<str> = String::from_utf16_lossy(&declaration.specifier).into();
    // `import "a";` binds nothing and is written for the other module being evaluated. The edge in
    // the graph is still there, so the entry is recorded with no name and no slot — the link step
    // needs to know the dependency exists even when nothing crosses it.
    let Some(clause) = &declaration.clause else {
        compiler.chunk.imports.push(chunk::ImportEntry {
            specifier,
            import_name: None,
            slot: None,
        });
        return;
    };
    let mut names: Vec<(&str, Option<Box<str>>)> = Vec::new();
    match clause {
        ImportClause::Default(name) => names.push((&name.name, Some("default".into()))),
        ImportClause::Namespace(name) => names.push((&name.name, None)),
        ImportClause::Named(specifiers) => {
            for entry in specifiers {
                names.push((&entry.local.name, Some(export_name(&entry.imported))));
            }
        }
        ImportClause::DefaultAndNamespace(default, namespace) => {
            names.push((&default.name, Some("default".into())));
            names.push((&namespace.name, None));
        }
        ImportClause::DefaultAndNamed(default, specifiers) => {
            names.push((&default.name, Some("default".into())));
            for entry in specifiers {
                names.push((&entry.local.name, Some(export_name(&entry.imported))));
            }
        }
    }
    for (local, imported) in names {
        // No `Uninitialise`, unlike every other lexical binding. §16.2.1.5.2 binds an import to
        // *another module's* slot before any body runs, and this instruction would run after that
        // — so it would reach through the binding and put the **exporting** module's slot back in
        // its dead zone, which is a module clearing a variable it does not own. The dead zone an
        // import has is the exporter's own, which is exactly right: reading an import before its
        // module has evaluated finds the exporter's uninitialised `let`.
        let slot = compiler.declare_lexical(local, Mutability::Const);
        compiler.chunk.imports.push(chunk::ImportEntry {
            specifier: specifier.clone(),
            import_name: imported,
            slot: Some(slot),
        });
    }
}

/// §16.2.1.3 — one `export`'s entries, once the body has given every name its slot.
fn record_exports(
    compiler: &mut Compiler<'_>,
    declaration: &crate::ast::ExportDeclaration,
) -> Result<(), CompileError> {
    use crate::ast::ExportKind;
    let specifiers: Vec<(Box<str>, Box<str>)> = match &declaration.kind {
        // `export {a, b as c}` — of names this module declares, which the body has given slots.
        ExportKind::Named(specifiers) => specifiers
            .iter()
            .map(|entry| (export_name(&entry.local), export_name(&entry.exported)))
            .collect(),
        // `export var a = 1;` and its four siblings — the declaration has run as itself, and every
        // name it bound is exported under its own spelling.
        ExportKind::Declaration(statement) => {
            let one = std::slice::from_ref(statement);
            // Both walks, because the five things `export` may stand in front of are split across
            // them: a `var` is var-declared, a `let`, a `const` and a `class` are lexical, and a
            // function declaration is one or the other depending on where it stands.
            let mut names: Vec<Box<str>> = var_declared_names(one)
                .into_iter()
                .chain(crate::static_semantics::lexically_declared_names(one))
                .map(|name| Box::<str>::from(name.name))
                .collect();
            if let crate::ast::StmtKind::Function(function) = &statement.kind
                && let Some(name) = &function.name
            {
                names.push(Box::<str>::from(&*name.name));
            }
            names.sort();
            names.dedup();
            names.into_iter().map(|name| (name.clone(), name)).collect()
        }
        // §16.2.3.7 — `export default`, whose exported name is `default` whatever the thing is
        // called here. A *declaration* keeps its own name too, so `export default function f() {}`
        // binds `f` and exports `default`; an expression has only the unspellable slot.
        ExportKind::Default(what) => {
            let local: Box<str> = match what {
                crate::ast::ExportDefault::Expression(_) => DEFAULT_EXPORT.into(),
                crate::ast::ExportDefault::Declaration(statement) => {
                    match declared_name(statement) {
                        Some(name) => name,
                        // `export default function () {}` — anonymous, so §16.2.3.7 gives it the
                        // same unspellable binding an expression gets.
                        None => DEFAULT_EXPORT.into(),
                    }
                }
            };
            vec![(local, "default".into())]
        }
        // §16.2.1.3 — an *indirect* export names another module and binds nothing here. The
        // resolution is §16.2.1.6.3's and happens at link time, because what a specifier answers is
        // the host's business and a chain of them may pass through several modules.
        ExportKind::NamedFrom {
            specifiers,
            specifier,
            ..
        } => {
            let from: Box<str> = String::from_utf16_lossy(specifier).into();
            // §16.2.1.4's `[[RequestedModules]]` — the module is a dependency whether or not any
            // name crosses. `export {} from "m"` names nothing and still requires `m`, so a
            // specifier nothing answers is a resolution error rather than a line with no effect.
            // The same shape `import "m";` makes: an edge that binds nothing.
            compiler.chunk.imports.push(chunk::ImportEntry {
                specifier: from.clone(),
                import_name: None,
                slot: None,
            });
            for entry in specifiers {
                compiler.chunk.exports.push(chunk::ExportEntry {
                    export_name: export_name(&entry.exported),
                    from: chunk::ExportSource::Indirect {
                        specifier: from.clone(),
                        import_name: Some(export_name(&entry.local)),
                    },
                });
            }
            return Ok(());
        }
        // `export * as n from "m"` exports one name whose value is the *whole* of `m` — §16.2.1.10's
        // namespace object rather than any name in it. `export * from "m"` exports no name of its
        // own at all: which names it brings is a question about `m` that only the graph can answer.
        ExportKind::All {
            exported,
            specifier,
            ..
        } => {
            let from: Box<str> = String::from_utf16_lossy(specifier).into();
            match exported {
                Some(name) => compiler.chunk.exports.push(chunk::ExportEntry {
                    export_name: export_name(name),
                    from: chunk::ExportSource::Indirect {
                        specifier: from,
                        import_name: None,
                    },
                }),
                None => compiler.chunk.star_exports.push(from),
            }
            return Ok(());
        }
    };
    for (local, exported) in specifiers {
        // §16.2.1.1 makes an `export` of a name nothing declares an early error, so a name with no
        // slot here is one this compiler failed to place rather than one a program can write.
        let Some(slot) = compiler.resolve(&local) else {
            return Err(unsupported(
                "an `export` of a name this module does not declare",
                declaration.span,
            ));
        };
        // §16.2.1.4 step 12 — an `export { a }` whose `a` is an **imported** name is not a local
        // export at all: it becomes an indirect one naming the module the import came from. That is
        // what makes `import {x} from "m"; export {x}` and `export {x} from "m"` resolve to the
        // *same* binding, so a module reached by both is unambiguous rather than a conflict.
        //
        // Found by slot rather than by name, because the import entry records the name the other
        // module exports and not the one this one bound it as.
        let from = match compiler
            .chunk
            .imports
            .iter()
            .find(|entry| entry.slot == Some(slot))
        {
            // A *namespace* import is the exception the same step makes: `import * as n from "m";
            // export {n}` exports a binding this module holds, whose value is an object, and there
            // is no name in `m` to point at.
            Some(entry) if entry.import_name.is_some() => chunk::ExportSource::Indirect {
                specifier: entry.specifier.clone(),
                import_name: entry.import_name.clone(),
            },
            Some(_) | None => chunk::ExportSource::Local(slot),
        };
        compiler.chunk.exports.push(chunk::ExportEntry {
            export_name: exported,
            from,
        });
    }
    Ok(())
}

/// How §16.2.3.7 binds an `export default` that has no name of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultKind {
    /// An expression or a class — a lexical declaration, uninitialised until the statement runs.
    Lexical,
    /// A function — a `HoistableDeclaration`, made and bound before the body starts.
    Hoisted,
}

/// Which of the two this item is, or `None` if it is not an unnamed `export default` at all.
///
/// A *named* declaration keeps its own binding and `*default*` is never made, so it is `None` too:
/// the export entry points at the name the source wrote.
fn default_expression(item: &crate::ast::ModuleItem) -> Option<DefaultKind> {
    let crate::ast::ModuleItem::Export(declaration) = item else {
        return None;
    };
    match &declaration.kind {
        crate::ast::ExportKind::Default(crate::ast::ExportDefault::Expression(_)) => {
            Some(DefaultKind::Lexical)
        }
        crate::ast::ExportKind::Default(crate::ast::ExportDefault::Declaration(statement)) => {
            match (&statement.kind, declared_name(statement)) {
                (_, Some(_)) => None,
                (crate::ast::StmtKind::Function(_), None) => Some(DefaultKind::Hoisted),
                (_, None) => Some(DefaultKind::Lexical),
            }
        }
        _ => None,
    }
}

/// The anonymous function an `export default` hoists, if this item is one.
fn hoisted_default(item: &crate::ast::ModuleItem) -> Option<&Stmt> {
    let crate::ast::ModuleItem::Export(declaration) = item else {
        return None;
    };
    let crate::ast::ExportKind::Default(crate::ast::ExportDefault::Declaration(statement)) =
        &declaration.kind
    else {
        return None;
    };
    match default_expression(item)? {
        DefaultKind::Hoisted => Some(statement),
        DefaultKind::Lexical => None,
    }
}

/// The `*default*` slot, for the two paths that cannot be reached without one having been made.
///
/// [`default_expression`] answers for the same item the body loop is about to compile, so a module
/// with an `export default` always has the slot by the time either reaches this. Making a second
/// one is the honest answer to "this cannot happen": a spare slot nothing reads, where a panic
/// would be an outage.
fn unreachable_default_slot(compiler: &mut Compiler<'_>) -> u32 {
    compiler.declare_lexical(DEFAULT_EXPORT, Mutability::Mutable)
}

/// §16.2.3.7's binding for a default export with no name of its own.
///
/// The specification's own spelling, and the `*` is the point: it is not an `IdentifierName`, so
/// the module that declares it cannot read it back and only the export list reaches it.
const DEFAULT_EXPORT: &str = "*default*";

impl Compiler<'_> {
    /// Build the value of an anonymous `export default` declaration — §16.2.3.7.
    ///
    /// A `function` or a `class` with no name, which is a shape no other position in the grammar
    /// admits: everywhere else a declaration must be named, and the hoisting passes assume it. So
    /// this makes the object the way an *expression* would and leaves it on the stack, and §8.6.3
    /// gives it the name `"default"` — the one place a function is called that.
    fn default_declaration(&mut self, statement: &Stmt) -> Result<(), CompileError> {
        match &statement.kind {
            crate::ast::StmtKind::Function(function) => self.make_function(
                function,
                self::function::Naming::of("default"),
                statement.span,
            ),
            crate::ast::StmtKind::Class(class) => {
                self.class(class, self::function::Naming::of("default"), statement.span)
            }
            // §16.2.3's `HoistableDeclaration` and `ClassDeclaration` are the only two the
            // production admits, and the parser has already refused everything else.
            _ => Err(unsupported(
                "an `export default` of something that is not a declaration",
                statement.span,
            )),
        }
    }
}

/// The single name a declaration binds, when it has one.
///
/// `export default function f() {}` keeps `f` as a binding of the module *and* exports `default`,
/// so the export entry has to point at `f`'s slot rather than at a second copy of the function.
fn declared_name(statement: &Stmt) -> Option<Box<str>> {
    match &statement.kind {
        crate::ast::StmtKind::Function(function) => {
            function.name.as_ref().map(|name| Box::from(&*name.name))
        }
        crate::ast::StmtKind::Class(class) => {
            class.name.as_ref().map(|name| Box::from(&*name.name))
        }
        _ => None,
    }
}

/// A `ModuleExportName` as the text another module asks for.
fn export_name(name: &crate::ast::ModuleExportName) -> Box<str> {
    match name {
        crate::ast::ModuleExportName::Identifier(text) => text.clone(),
        crate::ast::ModuleExportName::String(units) => String::from_utf16_lossy(units).into(),
    }
}

/// The statements of a module, refusing the two item kinds that need a module record.
///
/// Cloned rather than borrowed because a `ModuleItem` is not a `Stmt` and the walks below take a
/// slice of statements. A module is compiled once, so the copy is paid once and buys every
/// existing pass over a body unchanged.
fn module_statements(module: &Module) -> Result<Vec<Stmt>, CompileError> {
    let mut statements = Vec::new();
    for item in &module.body {
        match item {
            crate::ast::ModuleItem::Statement(statement) => statements.push(statement.clone()),
            // Its bindings were declared above; it contributes no statements.
            crate::ast::ModuleItem::Import(_) => {}
            // §16.2.3 — an `export` in front of a declaration takes nothing away: the declaration
            // is hoisted and scoped exactly as it would be without the word, and the export is a
            // second fact about the names it binds.
            crate::ast::ModuleItem::Export(declaration) => match &declaration.kind {
                crate::ast::ExportKind::Declaration(statement) => {
                    statements.push(statement.clone())
                }
                // §16.2.3.7's `[+Default]` is what lets a declaration here have **no name**, which
                // no other position allows. A named one is hoisted like any other; an anonymous one
                // has nothing to hoist and is built where it stands, into the unspellable slot.
                crate::ast::ExportKind::Default(crate::ast::ExportDefault::Declaration(
                    statement,
                )) => {
                    if declared_name(statement).is_some() {
                        statements.push(statement.clone());
                    }
                }
                crate::ast::ExportKind::Named(_)
                | crate::ast::ExportKind::Default(crate::ast::ExportDefault::Expression(_)) => {}
                // The two that reach into another module's *list* rather than at a name of its
                // own. Neither declares anything here — §16.2.1.3 is explicit that an indirect
                // export is not an import — so there is nothing for either hoisting pass to do.
                crate::ast::ExportKind::All { .. } | crate::ast::ExportKind::NamedFrom { .. } => {}
            },
        }
    }
    Ok(statements)
}

/// Where a direct `eval`'s `var` declarations go — §19.2.1.1's `varEnv`.
///
/// The one thing about an eval that its own source cannot decide. §19.2.1.1 step 12 hands the
/// evaluated code the *caller's* variable environment, so which of these applies is a question
/// about where the call was made, and only the interpreter knows the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalVars {
    /// Onto the global object — a sloppy direct eval whose caller's variable scope is the script's.
    ///
    /// §16.1.7's split, reached by exactly the path an ordinary script's `var` takes, which is why
    /// `eval("var x = 1"); globalThis.x` is 1 and a `let` beside it is not.
    Global,
    /// Into the eval's own scope, which is what step 14 does when the code is **strict**.
    ///
    /// A strict eval's declarations are discarded with it — `"use strict"; eval("var x = 1"); x` is
    /// a ReferenceError — so the eval's own environment is a place they can go that is sized when
    /// its chunk is compiled.
    Own,
    /// A sloppy `var` inside a function, which ViperJS refuses by name.
    ///
    /// §19.2.1.1 would add the binding to the *caller's* function scope, whose slot count was fixed
    /// when that function was compiled. No name list makes a `Vec` longer, so this is the one shape
    /// DR-0018 leaves open — refused with a message rather than quietly put somewhere else, because
    /// somewhere else is a wrong answer that runs.
    Caller,
}

/// §19.2.1.1 — compile eval'd source against the scopes its caller is *running* in.
///
/// The difference from [`compile_script`] is the chain. A script is compiled against nothing and
/// resolves every name it does not declare to the global object; this is handed `chain` —
/// **outermost first**, one entry per environment the caller is inside, each holding what that
/// environment called its slots — and resolves into it exactly as a nested function's body is
/// resolved into the scopes it was written in. DR-0018 is why the environments carry those names
/// at all.
///
/// An entry may be **shorter than its environment's slots**, or empty: a scope the engine built for
/// itself names nothing, and a slot past the end of a list belongs to a scope already left. Both
/// mean "nothing resolves here", and the walk carries on outwards — which is the same answer the
/// compiler would give for a scope that declares nothing. What the entry may *not* be is missing:
/// a `LoadVariable`'s depth counts environments, so a level left out would make every name outside
/// it resolve one hop too shallow.
///
/// Strictness is the tree's — [`crate::parser::parse_eval`] was told the caller's and folded §19.2.1.1
/// step 5's two halves together before anything was parsed, because §11.2.1's early errors and every
/// nested function's own strictness are settled there and cannot be set on a finished tree.
pub fn compile_direct_eval(
    script: &Script,
    heap: &mut Heap,
    chain: Vec<Vec<crate::heap::Binding>>,
    vars: EvalVars,
    dynamic: bool,
) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    // §14.11 — the call is inside a `with`, so every free name in this text is a run-time question.
    // The chain above cannot say it: an object environment has no names to list and arrives as an
    // empty level, indistinguishable from one the engine made for a temporary. One flag rather than
    // a mark per level, because that is all [`Compiler::names_are_dynamic`] can use — a single
    // object environment anywhere outside makes every name a lookup, and which level it sits at
    // changes nothing about the answer.
    compiler.with_depth = u32::from(dynamic);
    compiler.chunk.strict = script.is_strict;
    compiler.outer = chain
        .into_iter()
        .map(|level| level.into_iter().map(Local::from).collect())
        .collect();
    compiler.seeded_scopes = compiler.outer.len();
    // §19.2.1.1 step 14 — a strict eval's `var`s are its own, so the eval is not "the script" for
    // the purpose that flag decides: `Bind::Var` puts them in slots rather than on the global.
    compiler.global_vars = vars == EvalVars::Global;
    // §19.2.1.1 step 8 — an `eval`'s global bindings are **deletable**, where §16.1.7 makes a
    // Script's permanent. That is the same clause for a direct and an indirect call, so
    // [`compile_eval`] sets it too.
    compiler.deletable = Deletable::Yes;
    match vars {
        // §19.2.1.1's `EvalDeclarationInstantiation` against a global variable environment, which
        // asks §16.1.7's questions in §16.1.7's order — the same three passes, from the same
        // function, so that the two cannot drift.
        EvalVars::Global => compiler.instantiate_globals(&script.body)?,
        // Slots in the eval's own environment, hoisted before anything runs exactly as a function
        // body's are — which is what step 14 asks for and what makes them go away with it.
        EvalVars::Own => {
            for name in var_declared_names(&script.body) {
                compiler.declare(name.name);
            }
        }
        // The refusal, and it is asked before anything is emitted so that the eval either runs or
        // does nothing at all. A source with no `var` and no function declaration in it needs the
        // caller's variable scope for nothing, and is compiled here like any other — which is most
        // of what a direct eval is written for.
        EvalVars::Caller => {
            if let Some(name) = var_declared_names(&script.body).into_iter().next() {
                return Err(unsupported(
                    "a sloppy `var` inside a direct eval in a function",
                    name.span,
                ));
            }
            // A **function declaration** at the top level is var-scoped too and is not one of those
            // names: `VarDeclaredNames` and `TopLevelVarDeclaredNames` differ on exactly this
            // production, and ViperJS computes the first. Left out, `eval("function h(){}")` inside
            // a function would put `h` in a slot that goes away with the eval — a name the caller
            // asked for and cannot find, which is a wrong answer that runs.
            if let Some(statement) = script
                .body
                .iter()
                .find(|statement| matches!(statement.kind, crate::ast::StmtKind::Function(_)))
            {
                return Err(unsupported(
                    "a sloppy function declaration inside a direct eval in a function",
                    statement.span,
                ));
            }
        }
    }
    // §B.3.3.3's half of the clause, and for [`EvalVars::Caller`] as well as for the other two —
    // which is not the same answer the `var` above gets, and the difference is what the binding is
    // *for*. A `var` in a caller-scoped eval is refused because the name has to outlive the eval
    // and the caller's frame has no room for it. B.3.3's binding does not have to outlive anything
    // this engine can observe: every one of test262's `func-*-eval-func-*` files reads the name
    // from inside the eval, where a slot of the eval's own answers correctly. What is left wrong is
    // the caller reading it *afterwards*, and that was already wrong — the name was bound nowhere
    // at all — so the eval's own scope is strictly closer than nothing and refuses no program.
    if !script.is_strict {
        compiler.declare_block_functions(&script.body, &std::collections::HashSet::new())?;
    }
    compiler.declare_lexical_names(&script.body)?;
    compiler.hoist_functions(&script.body)?;
    compiler.statements(&script.body)?;
    Ok(compiler.finish())
}

/// One name with a slot, and what the compiler knows about it.
#[derive(Debug, Clone)]
struct Local {
    /// What the source calls it.
    name: Box<str>,
    /// Whether it can still be resolved from where the compiler is standing.
    ///
    /// A block's bindings stop being visible when the block ends, but their slots are not given
    /// back — [`Compiler::leave_scope`] says why. So going out of scope is a flag rather than a
    /// truncation, and [`Compiler::resolve`] skips what is no longer in scope.
    live: bool,
    /// What an assignment to it does — §9.1.1.1.5, which has three answers.
    ///
    /// Known here rather than at run time because the compiler resolved the binding: an
    /// environment does not have to be asked a question that was already settled when the name was
    /// looked up.
    mutability: Mutability,
}

impl Local {
    /// Whether this is what `name` refers to from where the compiler is standing.
    fn answers_to(&self, name: &str) -> bool {
        self.live && &*self.name == name
    }
}

impl From<crate::heap::Binding> for Local {
    /// A slot of a **running** environment, read back as the compiler would have written it.
    ///
    /// `live` is unconditionally true because the list is only ever handed over for scopes that are
    /// running: a name that had gone out of scope is not in it — see [`bindings_of`]. Whether the
    /// binding was a `let` is not carried, and does not need to be: §9.1.1.1's dead zone is a slot
    /// holding *nothing*, which the slot itself says at the moment it is read.
    fn from(binding: crate::heap::Binding) -> Self {
        Self {
            name: binding.name,
            live: true,
            mutability: binding.mutability,
        }
    }
}

/// What the compiler knows while it works.
struct Compiler<'a> {
    chunk: Chunk,
    heap: &'a mut Heap,
    /// The variables that have slots, in the order they were given them.
    ///
    /// A `Vec` searched backwards rather than a map: a scope holds a handful of names, and the
    /// backwards search is what makes an inner declaration shadow an outer one when there are
    /// inner scopes to have. It is also what a map would have to be told to do.
    locals: Vec<Local>,
    /// The jumps that leave the innermost loop, waiting for its end.
    ///
    /// One list per enclosing loop. `break` in a loop inside a loop leaves the inner one, which
    /// is what the stack is for.
    breaks: Vec<Vec<Unpatched>>,
    /// The slot this body's `arguments` object would go in, if it has one to fill.
    ///
    /// Given to every non-arrow body, before anything is compiled, so that the name resolves to
    /// it rather than walking out to a global. Whether it is ever *used* is a separate question —
    /// see [`Compiler::uses_arguments`] — because a slot costs nothing and an object costs an
    /// allocation per call.
    arguments_slot: Option<u32>,
    /// Whether anything actually read it.
    uses_arguments: bool,
    /// How many locals existed when each enclosing *scope* began, innermost last.
    ///
    /// What a declaration needs and [`Compiler::resolve`] cannot give it. Resolving a name walks
    /// outwards and finds the nearest binding of that name anywhere in the function, which is the
    /// right answer for a *use* and the wrong one for a *declaration*: `let i = 'kept'; for (let
    /// i = 0; …)` would find the outer `i` and the loop would assign to it, so the loop's variable
    /// would leak and the outer one would be destroyed. A declaration only ever looks inside the
    /// scope it is written in.
    ///
    /// Empty means the function body's own scope, which starts at slot zero — so `last()`
    /// defaulting to zero is the right answer rather than a missing case.
    scope_marks: Vec<usize>,
    /// The `continue` list of each breakable statement, at the **same index** as its break list.
    ///
    /// `None` for a statement that may be broken out of and not continued — §14.12's switch is the
    /// only one. Kept parallel rather than pushed only by loops because an exit's depth indexes the
    /// break lists, and two stacks of different heights make that one number mean two things: a
    /// `continue` inside a switch unwound to the *switch's* depth and left the discriminant on the
    /// stack, and a `continue outer` to a loop written inside a switch indexed a continue list that
    /// was not there and was never patched. Both faulted the interpreter from ordinary source.
    continues: Vec<Option<Vec<Unpatched>>>,
    /// How deep into an expression the compiler currently is.
    depth: u32,
    /// The scopes this one is written inside, outermost first.
    ///
    /// The script's is at the front and the immediately enclosing function's at the back, so
    /// counting *backwards* from the end is counting environments outwards — which is exactly
    /// what a [`Instruction::LoadVariable`] depth means.
    outer: Vec<Vec<Local>>,
    /// Where `this` lives, when it is a binding rather than the register — DR-0015.
    ///
    /// `Some` inside a derived constructor and inside any arrow written in one, and that propagation
    /// rule is §10.2.11's `[[ThisMode]]`: an arrow reaches outward for `this`, so it reaches for the
    /// binding; a non-arrow function is given one of its own by the call, so it must *not*, or a
    /// method written inside a derived constructor would answer the enclosing instance instead of
    /// its own receiver. See [`Compiler::compile_nested`], which is where the rule is applied.
    ///
    /// The depth is carried rather than looked up. Resolving `%this` by name through the scope chain
    /// would answer the same thing, and would also have a failure case — a `None` that could only
    /// mean the compiler had lost its own binding, with no honest thing to emit for it. Counting
    /// instead is exact by construction: an arrow is one environment, so propagating adds one.
    ///
    /// Not a [`Where`], though the two hold the same numbers. A `Where` also carries `immutable`,
    /// which decides whether a *write* is a TypeError — and nothing ever writes this binding through
    /// `store_name`, so the field would be a value no input could distinguish. Mutation coverage said
    /// exactly that, by surviving a flip of it.
    this_binding: Option<ThisSlot>,
    /// The body that initialises this derived constructor's instance fields, if it has any.
    ///
    /// §15.7.14 runs `InitializeInstanceElements` from `super()` rather than on entry, because until
    /// the parent has made the object there is nothing to put a field on. So the field list has to
    /// reach the `super()`, which may be anywhere in the constructor — and a *list* cannot be kept
    /// here, its lifetime being the syntax tree's rather than this compiler's. A compiled body can:
    /// the fields become a function of no arguments, called with the new object as its receiver,
    /// which is the same shape a static field's initialiser already uses and for the same reason.
    derived_fields: Option<u32>,
    /// Whether this is Script code rather than a function body.
    ///
    /// §14.2.2's completion value belongs to the script; what a function's statements evaluate to
    /// is nobody's business but `return`'s. So an expression statement inside a function discards
    /// its value where one at the top level keeps it, and a `return` written at the top level is a
    /// Syntax Error rather than a way out of anything.
    ///
    /// True for a direct `eval` too, however deep inside a function the call was written: §19.2.1.1
    /// evaluates a **Script**, which is what makes `eval("1; ;")` answer 1 and `eval("return")` a
    /// SyntaxError. That is not the same question as [`Compiler::global_vars`], and this field used
    /// to answer both — which made every direct eval in a function evaluate to `undefined`.
    is_script: bool,
    /// Whether a `var` here becomes a property of the global object rather than a slot.
    ///
    /// §16.1.7's split, and the half that is *not* about being a script. A script's `var`s are
    /// global properties and a function's are slots — and §19.2.1.1 makes an eval's depend on
    /// something neither of them can see: the caller's variable environment, and whether the
    /// evaluated code is strict. See [`EvalVars`].
    global_vars: bool,
    /// §9.1.1.4.17's `D` for every global binding this compilation creates.
    ///
    /// A second fact about the same variable scope [`Compiler::global_vars`] names, and not the
    /// same one: *where* a `var` goes is decided by §16.1.7's split, and *whether it can be
    /// deleted again* is decided by which operation asked. A Script's are permanent and an
    /// `eval`'s are not, direct and indirect alike — §19.2.1.1 passes `true` without asking which
    /// kind of call it was.
    deletable: Deletable,
    /// How many `with` scopes are open at the point being compiled — §14.11.
    ///
    /// Not a depth to add to anything: a `with` opens a real compiler level like a block does, so
    /// every slot index and every hop is already right. What this decides is whether a **name** can
    /// be resolved at all. Inside one, what a name means depends on what an object holds at the
    /// moment of the read, so the compiler stops answering and emits the walk instead.
    ///
    /// A count and not a flag, because a nested `with` that is left still leaves an outer one open.
    /// Inherited by every function written inside one, which is what makes the whole of
    /// `with (o) { function f() { return a; } }` dynamic — `f`'s scope chain contains the object.
    with_depth: u32,
    /// How many enclosing scopes this compiler was handed before it compiled anything.
    ///
    /// Zero for a script and for a function body, whose `outer` is built by the caller and then
    /// only grows and shrinks with the scopes this compilation opens. A direct `eval` is seeded
    /// with the chain its caller is running in, so "no scope has been opened here" is a comparison
    /// against that number rather than against zero — see [`Compiler::at_global_scope`], which is
    /// the one question that has to tell the difference.
    seeded_scopes: usize,
    /// The most slots that were in use at once.
    ///
    /// Not `locals.len()` at the end: a catch parameter's slot is given back when its block ends,
    /// so the table shrinks. The frame still has to be big enough for the moment it was widest,
    /// which is what a run-time slot index is an index into.
    high_water: usize,
    /// Whether an `await` was met where the body has no execution to park — §16.2.1.5.3.
    ///
    /// Only a module's top level can produce one: the parser makes `await` an identifier wherever
    /// the production is not available, so `ExprKind::Await` cannot appear in a script or in a
    /// non-async function at all. See [`compile_module`], which reads this and compiles again.
    saw_top_level_await: bool,
    /// Where each function declaration that was hoisted was written.
    ///
    /// [`Compiler::hoist_functions`] runs over the *top level* of a body and nothing else, so a
    /// declaration inside a block was never made — and the statement itself does nothing, because
    /// hoisting is supposed to have done it. Without this the two facts together would make
    /// `{ function g() {} } typeof g` answer `"undefined"` in silence, which is a wrong answer
    /// rather than a missing feature. §14.1 block-scopes such a declaration and Annex B.3.3
    /// hoists it in sloppy code; both need block scoping, so until then it is refused.
    hoisted: Vec<Span>,
    /// Where §B.3.3's extra `var` binding lives, for each declaration that earned one.
    ///
    /// Keyed by the declaration's span rather than by its name, because the extension belongs to
    /// one *declaration* and not to a name: `{ function f() {} { function f() {} } }` has an
    /// eligible outer and an ineligible inner, and both are called `f`. Two spans never coincide
    /// in one source, so this is an identity and not a heuristic.
    ///
    /// The target is decided once, at the variable scope, and read again where the copy is emitted
    /// — see [`BlockFunctionTarget::Global`] for the defect that comes of asking a second time.
    block_functions: Vec<(Span, BlockFunctionTarget)>,
    /// The labels in scope, and how many loops were open when each was met.
    ///
    /// A `break name` joins the break list of the loop that number identifies, which is what
    /// makes it leave *that* statement rather than the innermost one. Innermost last, so a
    /// backwards search finds the nearest of two labels with the same name — which the parser
    /// forbids, and which costs nothing to be right about.
    labels: Vec<(Box<str>, usize)>,

    /// The jumps out of each `OptionalChain` currently being compiled, innermost last.
    ///
    /// §13.3.9's short circuit ends at the **chain**, not at the link: `a?.b.c` gives up on the whole
    /// thing when `a` is nullish, and `(a?.b).c` gives up only on the part inside the parentheses and
    /// then reads `.c` of `undefined`. The syntax tree marks that boundary with a wrapper, and this is
    /// where the jumps to it wait — the same shape `breaks` has, for the same reason: the target is
    /// not compiled yet when the jump is emitted.
    chains: Vec<Vec<Unpatched>>,
    /// What each enclosing statement needs done on the way out of it — innermost last.
    ///
    /// A `break`, a `continue` and a `return` all leave statements that installed something, and
    /// every one of those has to be taken back down in the order it was put up. The specification
    /// has no such list: §14.15.3 says a `finally` runs on *every* way out of its `try`, §7.4.9
    /// says a `for`-`of` closes its iterator, and the handler stack is this implementation's own.
    /// They interleave by nesting and nothing else, so they belong on one stack rather than on
    /// three that have to be reconciled at each jump.
    ///
    /// One stack because two did not work. The iterators were kept in a list parallel to `breaks`
    /// and the `finally`s in a list of loop depths, and neither could say which of the two was
    /// *inner* when both sat at the same depth — which is precisely the question a `continue` out
    /// of a `try` inside a `for`-`of` asks. Position on this stack answers it for free.
    unwinds: Vec<Unwind>,
}

/// Where §B.3.3 puts the copy of a block-level function declaration.
///
/// The variable scope of the code being compiled, in the two forms it comes in — and the choice is
/// made once, at that scope, rather than asked again where the copy is emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockFunctionTarget {
    /// §9.1.1.4.17 `CreateGlobalVarBinding` — the variable scope is the global object, so the
    /// binding is a property of it and the copy is a [`Instruction::StoreGlobal`]. Held as the
    /// constant index of the name, which is what that instruction takes.
    ///
    /// This is where a first attempt at B.3.3 went wrong, and it is worth naming. The natural
    /// thing to ask at the point of the copy is [`Compiler::at_global_scope`] — and that answers
    /// two questions at once, "the var scope is the global object" **and** "no scope has been
    /// opened here". The second half is false inside the block, which is the only place this copy
    /// is ever emitted, so a script's `{ function f() {} }` stored into a slot the script does not
    /// have and `typeof f` answered `"undefined"` while the same program inside a function worked.
    /// Deciding at the variable scope and carrying the answer is what makes the question single.
    Global(u32),
    /// A slot in the variable scope's own environment, reached by [`Compiler::own_depth`] hops.
    ///
    /// Not by resolving the name, which would find the block's own binding — that is the one being
    /// copied *from*, and a store through it would write the value back where it already was.
    Slot(u32),
}

/// One thing an abrupt exit has to do on its way past the statement that installed it.
#[derive(Clone)]
struct Unwind {
    /// How many breakable statements were open when this was installed.
    ///
    /// The jump names the breakable it is leaving by index, so this is what decides whether it
    /// crosses this entry at all: a `try` written *inside* the loop being left has a greater
    /// number than that loop's index, and one written *around* it does not.
    outer: usize,
    /// What to emit.
    what: Crossing,
}

/// Whether closing an iterator has to **wait** for it — §7.4.9 against §7.4.11.
///
/// A `for`-`of` calls `return()` and reads the answer; a `for await` has to await it first, because
/// what an async iterator answers with is a promise and the loop may not leave until it settles.
/// A named pair rather than a `bool` threaded through three signatures, where `true` at the wrong
/// argument position compiles and silently awaits in an ordinary loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Closing {
    /// §7.4.9 — the answer is whatever `return()` gave back.
    Sync,
    /// §7.4.11 — await it, and on the way out of a throw discard anything it raises.
    Awaited,
}

/// The three kinds of thing [`Unwind`] can hold, and what each costs to cross.
#[derive(Clone)]
enum Crossing {
    /// The handlers a `try` armed, which a jump out of it has to take down.
    ///
    /// Nothing in the specification, and a bug without it: §14.15 unwinds by *completion*, where
    /// this VM unwinds by a stack of handlers a `PushHandler` put there. Left armed after a
    /// `break` jumped past it, one of them catches a later throw and lands in a `catch` block the
    /// program has already left — a wrong answer with no exception in sight.
    ///
    /// A count rather than a flag: a `try` with both a `catch` and a `finally` arms two over its
    /// try block, and only one over its catch block, the other having been consumed by the throw
    /// that got there.
    Handlers(u32),
    /// A block environment, which a jump out of the block has to leave.
    ///
    /// The same shape of problem `Handlers` is and the same reason it is not in the specification:
    /// §8.3.2 talks about *restoring* the running environment when a block completes however it
    /// completes, and this VM leaves it by running an instruction. A `break` that jumped past one
    /// would carry on with the block's environment still in force, so every variable read after
    /// the loop would be read one hop too shallow — reading a *different variable*, not failing.
    ///
    /// The throw path needs nothing here: a handler records the environment it was installed in,
    /// and unwinding restores it. That is the difference between an exit that runs instructions on
    /// the way out and one that jumps.
    Scope,
    /// A `finally` block, which §14.15.3 runs on every way out of the `try` it belongs to.
    ///
    /// The statements rather than a jump to one copy of them, because there is nowhere to jump
    /// *back* to: this VM has no return address that is not a call frame. So the block is emitted
    /// again at each exit that crosses it, which is what the throw path has always done — the
    /// normal way out and the unwinding way out are already two copies. Held by `Rc` so that
    /// re-emitting is a pointer copy rather than a clone of the syntax tree.
    ///
    /// Code size is the cost, and it is bounded by the source: a `finally` inside a `finally`
    /// doubles per level, so deeply nested ones grow fast. A single copy with a dispatch on a
    /// pending-completion slot would be linear instead; it is worth doing when a real program is
    /// found that needs it, and not before.
    Finally(Rc<[Stmt]>),
    /// A `for`-`of` iterator to close — §7.4.9 `IteratorClose`, in the slot holding it.
    Iterator(u32, Closing),
    /// A value a statement left on the operand stack, which a jump out of that statement must drop.
    ///
    /// §14.12's switch is the only one: its discriminant is compared against each case in turn, so
    /// it stays on the stack for the whole `CaseBlock` and is dropped where the cases converge. A
    /// `break` lands on that convergence and needs nothing — but a `continue` or a `return` jumps
    /// clean past it, and left there the value is still on the stack when the enclosing loop goes
    /// round again. That is not a wrong answer, it is a `Fault::UnbalancedStack`: an operand stack
    /// that no longer matches what the compiler believes about it.
    Operand,
}

impl<'a> Compiler<'a> {
    fn new(heap: &'a mut Heap) -> Self {
        Self {
            chunk: Chunk::default(),
            heap,
            locals: Vec::new(),
            breaks: Vec::new(),
            arguments_slot: None,
            uses_arguments: false,
            scope_marks: Vec::new(),
            continues: Vec::new(),
            saw_top_level_await: false,
            hoisted: Vec::new(),
            block_functions: Vec::new(),
            labels: Vec::new(),
            unwinds: Vec::new(),
            chains: Vec::new(),
            high_water: 0,
            depth: 0,
            outer: Vec::new(),
            this_binding: None,
            derived_fields: None,
            is_script: true,
            global_vars: true,
            deletable: Deletable::No,
            seeded_scopes: 0,
            with_depth: 0,
        }
    }

    /// Emit a read of `this` — §9.1.1.3's `ResolveThisBinding`.
    ///
    /// Two representations and the compiler picks, which is DR-0015's whole cost. Inside a derived
    /// constructor `this` is a binding that `super()` fills, so reading it before then is the
    /// ReferenceError §10.2.2 asks for; everywhere else it is the register the call set, and there is
    /// no state in which it could be missing.
    pub(super) fn load_this(&mut self) {
        match self.this_binding {
            Some(at) => self.chunk.emit(Instruction::LoadThisBinding {
                depth: at.depth,
                index: at.index,
            }),
            None => self.chunk.emit(Instruction::LoadThis),
        }
    }

    fn finish(self) -> Chunk {
        let mut chunk = self.chunk;
        chunk.locals = self.high_water;
        // Every scope this body opened has been closed by now, so `locals` is the body's own level
        // and nothing else — which is exactly the environment the call builds. Any slack between it
        // and `high_water` is slots a *nested* scope needed, which belong to no name at all.
        chunk.bindings = bindings_of(&self.locals);
        chunk
    }

    /// §16.1.7's `GlobalDeclarationInstantiation`, which §19.2.1.1 asks for again.
    ///
    /// Three passes and the order between them is the whole of what a program can see. Steps 5 to
    /// 12 ask whether **every** name may be declared before **any** of them is created, so
    /// `var a; function NaN() {}` leaves no `a` behind: the refusal happens before anything is
    /// made. Written the obvious way round — check each as it is created — the names before the
    /// offending one would stand, and the global object would be half-instantiated by an operation
    /// that threw. `language/eval-code/direct/non-definable-function-with-variable.js` is that
    /// exact program.
    ///
    /// `VarDeclaredNames` of the whole body rather than of the top level, because a `var` inside a
    /// block or a loop belongs to the script all the same; that is the difference between `var` and
    /// everything that replaced it. `TopLevelVarDeclaredNames` would answer the same thing today —
    /// the two differ on exactly one production, a **function declaration** at the top level, which
    /// is var-scoped and which is why the middle pass walks the statements itself. Step 11's
    /// question is the stricter one and is asked of those only.
    ///
    /// Shared with the direct `eval` whose caller's variable scope is the global object, because
    /// §19.2.1.1 step 8 is these same questions and a second copy of them is a second thing to
    /// keep right. It was a second copy for one commit, and the `CheckGlobalFunction` pass was the
    /// half that got left out.
    fn instantiate_globals(&mut self, body: &[Stmt]) -> Result<(), CompileError> {
        for name in var_declared_names(body) {
            let index = self.name(name.name)?;
            self.chunk.emit(Instruction::CheckGlobalVar(index));
        }
        for statement in body {
            let crate::ast::StmtKind::Function(function) = &statement.kind else {
                continue;
            };
            let Some(name) = &function.name else {
                continue;
            };
            let index = self.name(&name.name)?;
            self.chunk.emit(Instruction::CheckGlobalFunction(index));
        }
        for name in var_declared_names(body) {
            let index = self.name(name.name)?;
            let deletable = self.deletable;
            self.chunk.emit(Instruction::DeclareGlobal {
                name: index,
                deletable,
            });
        }
        Ok(())
    }

    /// [`Compiler::declare_block_functions`] for a function body, whose parameters are its
    /// `parameterNames` and whose strictness is its chunk's.
    ///
    /// The two callers below it hoist a body the same way and would each have had to remember the
    /// strictness test; forgetting it in one would make `"use strict"` in a body silently take
    /// Annex B's extension, which is the one thing DR-0008's amendment turns entirely on.
    pub(super) fn declare_block_functions_of(
        &mut self,
        body: &[Stmt],
        parameters: &crate::ast::FormalParameters,
    ) -> Result<(), CompileError> {
        if self.chunk.strict {
            return Ok(());
        }
        // §B.3.3's `parameterNames`, which is `BoundNames of formals` — every name in the list,
        // including the ones a pattern binds and the rest parameter's.
        let names: std::collections::HashSet<&str> = parameters
            .items
            .iter()
            .map(|item| &item.target)
            .chain(parameters.rest.as_deref())
            .flat_map(crate::static_semantics::bound_names)
            .map(|declared| declared.name)
            .collect();
        self.declare_block_functions(body, &names)
    }

    /// §B.3.3's `var` bindings, for every block-level function declaration that earns one.
    ///
    /// Two steps of the clause, and the second is why this is one function rather than a list of
    /// names handed back:
    ///
    /// - **Step 2** creates the binding, initialised to `undefined`, *unless* the variable scope
    ///   already has one of that name — "if instantiatedVarNames does not contain F". Creating a
    ///   second binding for a name that has one is the mistake that makes reads resolve to one and
    ///   the copy below write to the other, which is what the `existing-fn-update` tests see.
    ///
    ///   No list of names is kept to answer that, because both primitives answer it already:
    ///   [`Compiler::declare`] hands back the slot a name already has rather than making another,
    ///   and §9.1.1.4.17's `CreateGlobalVarBinding` leaves a property that is there alone. A guard
    ///   in front of either is a branch no program can distinguish, and mutation coverage said so
    ///   by surviving its removal.
    /// - **Step 3** replaces what the declaration *evaluates to*, and that happens wherever it was
    ///   written. So where the binding is has to be remembered until the compiler gets there, which
    ///   is what [`Compiler::block_functions`] is.
    ///
    /// `parameters` is the clause's `parameterNames`, which is empty for a script and for an eval.
    ///
    /// Called at a variable scope and nowhere else. A module never calls it: §11.2.2 makes module
    /// code strict, and every B.3 rule is conditioned on sloppiness.
    fn declare_block_functions(
        &mut self,
        body: &[Stmt],
        parameters: &std::collections::HashSet<&str>,
    ) -> Result<(), CompileError> {
        for function in annex_b::block_functions(body, parameters) {
            let target = match self.global_vars {
                // §16.1.7 makes a script's `var` a property of the global object, and B.3.3.2 asks
                // for `CreateGlobalVarBinding` — which leaves an existing property exactly as it
                // is, so a name the global already has keeps its attributes and its value.
                true => {
                    let index = self.name(function.name)?;
                    let deletable = self.deletable;
                    self.chunk.emit(Instruction::DeclareGlobal {
                        name: index,
                        deletable,
                    });
                    BlockFunctionTarget::Global(index)
                }
                // A slot, which a frame starts holding `undefined` — so step 2's
                // `InitializeBinding(F, undefined)` is the declaration and nothing more.
                false => BlockFunctionTarget::Slot(self.declare(function.name)),
            };
            self.block_functions.push((function.span, target));
        }
        Ok(())
    }

    /// Give `name` a slot if it does not have one, and answer which.
    ///
    /// Redeclaring a `var` is legal and is not a second variable: `var x; var x = 1;` declares one
    /// binding. That is why this answers the existing slot rather than making another.
    fn declare(&mut self, name: &str) -> u32 {
        if let Some(slot) = self.resolve(name) {
            return slot;
        }
        self.declare_shadowing(name)
    }

    /// Which slot `name` is in, if any.
    ///
    /// Searched from the end, because the last binding with a given name is the innermost one.
    ///
    /// `var` alone never puts a name in twice — [`Compiler::declare`] hands back the existing
    /// slot — so this mattered to nothing until the catch parameter arrived. That one *does*
    /// shadow: `var e = 1; try { throw 2 } catch (e) { e }` is 2 inside the block and 1 after it,
    /// which is what [`Compiler::declare_shadowing`] and the truncation afterwards produce.
    fn resolve(&self, name: &str) -> Option<u32> {
        let at = self
            .locals
            .iter()
            .rposition(|local| local.answers_to(name))?;
        u32::try_from(at).ok()
    }

    /// Which slot `name` has *in the scope being compiled*, if it has one there.
    ///
    /// What a declaration asks, where a use asks [`Compiler::resolve`]. See
    /// [`Compiler::scope_marks`] for why the two are different questions.
    fn resolve_in_scope(&self, name: &str) -> Option<u32> {
        let mark = self.scope_marks.last().copied().unwrap_or(0);
        let at = self
            .locals
            .iter()
            .rposition(|local| local.answers_to(name))
            .filter(|at| *at >= mark)?;
        u32::try_from(at).ok()
    }

    /// Open a scope, answering the mark that closes it.
    fn enter_scope(&mut self) -> usize {
        let mark = self.locals.len();
        self.scope_marks.push(mark);
        mark
    }

    /// Open a real environment, not merely a compiler scope — §8.3.2's running LexicalEnvironment.
    ///
    /// The difference between the two is what a closure sees. A compiler scope hands out fresh
    /// *slots* in the function's one environment, which is enough to keep two blocks' `x` apart
    /// but not enough to keep two *executions* of the same block apart — and an arrow made in a
    /// loop body must capture the binding that iteration had.
    ///
    /// Pushing the current locals onto `outer` is the whole mechanism: [`Compiler::binding`]
    /// already walks that chain and counts the hops, so every name outside the block gains a hop
    /// without a line of resolution code changing, and a nested function's own chain is built from
    /// the same two fields.
    fn enter_environment(&mut self) -> Environment {
        let outer = self.breaks.len();
        self.open_environment(outer)
    }

    /// The same, for an environment that belongs to **one pass of a loop** — §14.7.5.7.
    ///
    /// One number differs and it decides three exits. The loop's own break list is pushed after
    /// this, so an environment recorded at the current count would sit at the loop's own depth —
    /// and a `continue` at that depth does not cross it. A `continue` most certainly leaves the
    /// pass it is in, so the environment is recorded as though it were already inside the body.
    fn enter_iteration_environment(&mut self) -> Environment {
        let outer = self.breaks.len() + 1;
        self.open_environment(outer)
    }

    /// What both of those do once they have decided which exits cross them.
    fn open_environment(&mut self, outer: usize) -> Environment {
        let held = std::mem::take(&mut self.locals);
        self.outer.push(held);
        // DR-0015's `this` is a binding reached by a depth the compiler remembers, and a scope is
        // one more hop to it — exactly as an enclosing arrow is. Only the arrow was counted.
        if let Some(slot) = &mut self.this_binding {
            slot.depth += 1;
        }
        // A scope mark for the new level, and it has to be zero: `resolve_in_scope` asks "was this
        // name declared *here*", and a mark taken in the level that was just set aside would have
        // it looking past the whole of the new one. That is not a subtle failure — a `for (const
        // {a} of …)` head declares `a` and the body then cannot find it.
        self.scope_marks.push(0);
        // Recorded so that a `break`, a `continue` or a `return` written inside the block emits
        // the `PopScope` it would otherwise jump straight past.
        //
        // `outer` is measured in **break lists** and not in loop nesting, which is the scale every
        // other crossing uses and the only one that is right: an [`Exit`]'s depth indexes those
        // lists, and a label on a plain block pushes one without being a loop. Counted the other
        // way, `L: { let x; break L; }` never crosses this entry — the two numbers are both zero —
        // and the code after the block reads its variables one hop too shallow.
        self.unwinds.push(Unwind {
            outer,
            what: Crossing::Scope,
        });
        Environment {
            scope: self.chunk.emit_jump(Instruction::PushScope),
            copies: Vec::new(),
        }
    }

    /// §14.11's scope — the same level a block opens, over an object.
    ///
    /// The bookkeeping is identical and that is the point: a `with` body's temporaries need a level
    /// like any other body's, every hop outwards counts one more like any other scope's, and a
    /// `break` out of it emits the `PopScope` `unwind_across` emits for any other. What differs is
    /// one instruction and [`Compiler::with_depth`], which stops the compiler answering questions
    /// about names it can no longer answer.
    fn enter_with_environment(&mut self) -> Environment {
        let outer = self.breaks.len();
        let held = std::mem::take(&mut self.locals);
        self.outer.push(held);
        // No `this_binding` hop, unlike every other scope. DR-0015's binding exists only inside a
        // *derived constructor* and the arrows written in one, §15.7 makes every class body strict,
        // and §11.2.1 makes a `with` in strict code a Syntax Error the parser refuses. So the two
        // cannot meet, and a line adjusting the depth here would be one no program can reach —
        // which mutation coverage said by surviving its removal.
        self.scope_marks.push(0);
        self.unwinds.push(Unwind {
            outer,
            what: Crossing::Scope,
        });
        self.with_depth += 1;
        Environment {
            scope: self.chunk.emit_jump(Instruction::PushWithScope),
            copies: Vec::new(),
        }
    }

    /// Close it, and let names resolve again.
    fn leave_with_environment(&mut self, environment: Environment) -> Result<(), CompileError> {
        self.with_depth = self.with_depth.saturating_sub(1);
        self.close_environment(environment, true)
    }

    /// §14.7.4.7 `CreatePerIterationEnvironment` — start the next pass with its own bindings.
    ///
    /// Emitted twice per loop and not once: before the first test, so the body never runs in the
    /// environment the initialiser built, and again after each body before the update, so the
    /// update increments *the next pass's* copy. A closure made during a pass therefore keeps what
    /// that pass had, and the loop still counts, which is the pair of facts that makes
    /// `for (let i = 0; i < 3; i++)` hand three closures three numbers.
    fn copy_environment(&mut self, environment: &mut Environment) {
        environment
            .copies
            .push(self.chunk.emit_jump(Instruction::CopyScope));
    }

    /// Close it again, and say how many slots it turned out to need.
    ///
    /// A `PopScope` is emitted here and on no other path out. Every *jumping* exit — `break`,
    /// `continue`, `return` — goes through `unwind_across`, which emits its own; a throw needs
    /// none at all, because the handler recorded the environment it was installed in.
    fn leave_environment(&mut self, environment: Environment) -> Result<(), CompileError> {
        self.close_environment(environment, true)
    }

    /// The same for an environment whose `PopScope` has already been emitted somewhere better.
    ///
    /// A loop that leaves its per-iteration environment at the bottom of each pass has emitted the
    /// instruction there; emitting a second one after the loop would run it once too often, and it
    /// would be unreachable besides, sitting after an unconditional jump back to the top.
    fn leave_environment_already_popped(
        &mut self,
        environment: Environment,
    ) -> Result<(), CompileError> {
        self.close_environment(environment, false)
    }

    /// What both of those do.
    fn close_environment(
        &mut self,
        environment: Environment,
        pop: bool,
    ) -> Result<(), CompileError> {
        let slots = u32::try_from(self.locals.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })?;
        // DR-0018 — the scope carries what it called its slots, because a direct `eval` resolves
        // against a *running* environment and has no compile-time chain to ask. Recorded here and
        // not when the `PushScope` was emitted: the hidden slots the block's statements needed were
        // made after it, and the names have to be in slot order to be worth anything.
        let names = bindings_of(&self.locals);
        let index = self.chunk.add_scope(chunk::Scope { slots, names })?;
        self.chunk.patch_scope(environment.scope, index);
        for copy in environment.copies {
            self.chunk.patch_scope(copy, index);
        }
        if pop {
            self.chunk.emit(Instruction::PopScope);
        }
        // Down before anything else: the ordinary way out has just been emitted, and leaving the
        // entry in place would make a later `break` emit a second `PopScope` for a block it is no
        // longer inside.
        self.unwinds.pop();
        self.scope_marks.pop();
        if let Some(slot) = &mut self.this_binding {
            slot.depth = slot.depth.saturating_sub(1);
        }
        let Some(held) = self.outer.pop() else {
            // `enter_environment` pushed one and the two are called in pairs, so this is a
            // compiler that has lost track of itself rather than a program that did anything.
            return Err(CompileError {
                kind: ErrorKind::TooDeep,
                span: Span::new(0, 0),
            });
        };
        self.locals = held;
        Ok(())
    }

    /// The local `name` refers to, if it is one.
    fn local(&self, name: &str) -> Option<&Local> {
        self.locals
            .iter()
            .rev()
            .find(|local| local.answers_to(name))
    }

    /// Give `name` a slot that a block will take out of scope again — §14.3.1's `let` and `const`.
    ///
    /// Always a *new* slot, never a reused one, and that is the whole of what makes sibling blocks
    /// safe: `{ let x = 1; f = () => x } { let y = 2 }` would have `f` answering 2 if `y` were
    /// given the slot `x` had finished with. Slots are cheap and closures are not repairable
    /// afterwards.
    fn declare_lexical(&mut self, name: &str, mutability: Mutability) -> u32 {
        let slot = self.declare_shadowing(name);
        if let Some(local) = self.locals.last_mut() {
            local.mutability = mutability;
        }
        slot
    }

    /// Take every local declared since `mark` out of scope, without giving its slot back.
    ///
    /// The slot stays taken for the rest of the function. See [`Compiler::declare_lexical`] — a
    /// closure made inside the block still reads that slot after the block has ended, so handing
    /// it to the next block would make the two share a variable.
    fn leave_scope(&mut self, mark: usize) {
        self.scope_marks.pop();
        for local in self.locals.iter_mut().skip(mark) {
            local.live = false;
        }
    }

    /// Where `name` lives, from where the compiler is standing.
    ///
    /// A depth and an index, because the compiler built the chain of scopes and so knows the
    /// answer: how many environments out, and which slot there. Nothing at run time compares a
    /// string to find a variable — §9.1's records, resolved once.
    fn binding(&self, name: &str) -> Option<Where> {
        if let Some(index) = self.resolve(name) {
            let mutability = self
                .local(name)
                .map_or(Mutability::Mutable, |local| local.mutability);
            return Some(Where {
                depth: 0,
                index,
                mutability,
            });
        }
        // Outwards, one scope at a time. The innermost enclosing scope is the *last* of `outer`,
        // so walking it in reverse is walking the environment chain — and the count is the depth
        // the instruction carries.
        for (back, scope) in self.outer.iter().rev().enumerate() {
            let Some(at) = scope.iter().rposition(|local| local.answers_to(name)) else {
                continue;
            };
            // `back` counts enclosing scopes and `at` indexes one scope's locals, so both are
            // bounded by how much source there is — and `Span` being `u32` already puts a source
            // longer than `u32::MAX` bytes outside what this engine agreed to read. Neither
            // conversion can fail on a source we accepted in the first place.
            let depth = u32::try_from(back + 1).ok()?; // bounded by the u32 source-length contract
            let index = u32::try_from(at).ok()?; // same
            let mutability = scope
                .get(at)
                .map_or(Mutability::Mutable, |local| local.mutability);
            return Some(Where {
                depth,
                index,
                mutability,
            });
        }
        None
    }

    /// Emit the instruction that reads `binding`.
    fn load(&mut self, binding: Where) {
        self.chunk
            .emit(Instruction::LoadVariable(binding.depth, binding.index));
    }

    /// Emit the instruction that writes `binding`, leaving the value on the stack.
    fn store(&mut self, binding: Where) {
        self.chunk
            .emit(Instruction::StoreVariable(binding.depth, binding.index));
    }

    /// Whether a declaration here belongs to the global object rather than to a scope.
    ///
    /// Two conditions and both are needed, which is what one of them alone got wrong in each
    /// direction.
    ///
    /// It used to ask `outer.is_empty()` — "is this compiler the script's own". That answers the
    /// wrong thing for a direct `eval`, which is compiled *against* a chain and so is inside
    /// something by that measure: its top-level `function h() {}` went into a slot discarded with
    /// the eval, where §9.1.1.4.16 asks for a global function binding, and
    /// `eval("function h(){}"); h()` was a ReferenceError.
    ///
    /// Asking [`Compiler::global_vars`] alone is wrong the other way. §14.1 makes a function
    /// declaration inside a **block** that block's, and a script's `global_vars` is true wherever
    /// in it the compiler has got to — so `{ function f() {} } f` would put `f` on the global
    /// object, which is Annex B.3.3 and DR-0008 leaves that out.
    ///
    /// So: the var scope is the global object, **and** the compiler has not opened a scope since it
    /// started. A function declaration is var-scoped, which is why the first half is the same field
    /// a `var` asks rather than a second rule that could disagree with it.
    pub(super) fn at_global_scope(&self) -> bool {
        self.global_vars && self.outer.len() == self.seeded_scopes
    }

    /// The constant index holding `name` as a String, for the instructions that name a global.
    ///
    /// The name is interned, so the hundred references to `assert` in one test share one String —
    /// and so the property key made from it at run time is the key the global object already has.
    pub(super) fn name(&mut self, name: &str) -> Result<u32, CompileError> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let interned = self.heap.intern(&units);
        self.chunk.add_constant(Value::String(interned))
    }

    /// Note that `arguments` was reached for, so that the call will build one.
    ///
    /// Asked on every name because the alternative is asking the parser for a list of the names a
    /// body mentions, which is a second walk over the tree that could disagree with this one.
    /// Comparing a string here costs a length check for every name that is not eight characters
    /// long.
    fn note_arguments(&mut self, name: &str, binding: Option<Where>) {
        if name != "arguments" {
            return;
        }
        match binding {
            // The body's own — the slot given below, or a parameter or `var` that took the name
            // first, in which case §10.2.11 step 19 makes no object and this is that.
            //
            // Compared against the depth of this body's **base level** and not against zero. Every
            // scope opened since — a block with a `let`, a `for` head, a `with` — is one more hop
            // to it, so `function f() { { let z; return arguments[0]; } }` resolved `arguments` at
            // depth 1, took the arm below, and told the *enclosing* function to build an object
            // this one then read from a slot nothing had filled. It threw, from a program with
            // nothing unusual in it.
            Some(found) if found.depth == self.own_depth() => {
                self.uses_arguments = Some(found.index) == self.arguments_slot;
            }
            // An enclosing function's, which only an arrow can reach. The chunk carries it out to
            // whoever is building this body.
            Some(_) => self.chunk.outer_arguments = true,
            // Nowhere at all: an `arguments` at the top level of a script is an ordinary global.
            None => {}
        }
    }

    /// How many scopes deep in **this body** the compiler currently is.
    ///
    /// The chain `binding` walks runs from here out through this body's own blocks and then into
    /// the scopes the body was compiled inside. This is the depth of the boundary between the two
    /// — the body's base level, where its parameters and its `arguments` slot live — so a binding
    /// at exactly this depth is one of the function's own and anything deeper belongs to whoever
    /// wrote it.
    fn own_depth(&self) -> u32 {
        u32::try_from(self.outer.len().saturating_sub(self.seeded_scopes)).unwrap_or(u32::MAX)
    }

    /// Whether a name here has to be resolved at run time — §14.11, and the whole of what it costs.
    pub(super) fn names_are_dynamic(&self) -> bool {
        self.with_depth > 0
    }

    /// Emit a read of `name`, from a scope if it has one and from the global object if not.
    ///
    /// §9.4.2 `ResolveBinding` walks the environment chain outwards and ends at the Global
    /// Environment Record, so a name the compiler cannot place is not an unknown name — it is a
    /// name whose binding, if any, is a property of the global object. Whether it is there is a
    /// question for run time, because a script can create one at any moment.
    pub(super) fn load_name(&mut self, name: &str) -> Result<(), CompileError> {
        let binding = self.binding(name);
        self.note_arguments(name, binding);
        // §14.11 — inside a `with` the compiler's answer may be wrong by the time the read happens,
        // so it does not give one. Asked *after* `note_arguments`, which is about the enclosing
        // function's object and is a question about the source rather than about the scope chain.
        // Forcing this true is **behaviour-preserving**: `LoadName` finds by name the very binding
        // the slot was chosen for, so no program can tell. What it costs is 3.0× to 3.7× on local
        // variable access — `lab/NOTES.md`'s `name-resolution` measured it — which is DR-0010's
        // premise that the common path pays nothing for the rare one. Nothing that *runs* can pin
        // that, so `compile::tests::a_name_is_a_slot_the_compiler_chose_and_only_a_with_makes_it_a
        // _walk` asserts the instructions instead.
        if self.names_are_dynamic() {
            let index = self.name(name)?;
            self.chunk.emit(Instruction::LoadName(index));
            return Ok(());
        }
        match binding {
            Some(binding) => {
                self.load(binding);
                Ok(())
            }
            None => {
                let index = self.name(name)?;
                self.chunk.emit(Instruction::LoadGlobal(index));
                Ok(())
            }
        }
    }

    /// Emit a read of the Private Name for `#name`, or refuse — §9.2's `ResolvePrivateIdentifier`.
    ///
    /// Not [`Compiler::load_name`], and the difference is the whole reason this exists: that one
    /// falls back to a *property of the global object* for a name it cannot place, which is right for
    /// an identifier and silently wrong here. A `#x` with no enclosing class would read a global that
    /// does not exist, hand `undefined` to `GetPrivate` as a name, and reach the interpreter as a
    /// fault — the signal reserved for a chunk that disagrees with the compiler.
    ///
    /// The parser refuses `#x` outside a class (§15.7.1), so no source arrives here. A hand-built
    /// tree can, and this is the honest answer for it: a refusal with a span.
    pub(super) fn load_private_name(&mut self, name: &str) -> Result<(), CompileError> {
        let slot = crate::compile::class::private_name_slot(name);
        let Some(binding) = self.binding(&slot) else {
            return Err(unsupported(
                "a private name outside a class body",
                Span::new(0, 0),
            ));
        };
        self.load(binding);
        Ok(())
    }

    /// Emit a read or a write of one of the compiler's own class-scope slots, or refuse.
    ///
    /// The same argument [`Compiler::load_private_name`] makes, for the slots that hold a private
    /// method's *function* rather than its name. `load_name` and `store_name` fall back to a property
    /// of the global object for a name they cannot place, which for a `%`-prefixed slot is never what
    /// was meant — and it fails *quietly*, because a store and a load of the same missing slot both go
    /// to the same global and agree with each other. Mutation coverage found exactly that: dropping the
    /// static half of the private-method list left every static method working, through a global.
    fn private_slot(&mut self, slot: &str, write: bool) -> Result<(), CompileError> {
        let Some(binding) = self.binding(slot) else {
            return Err(unsupported(
                "a private method outside a class body",
                Span::new(0, 0),
            ));
        };
        match write {
            true => self.store(binding),
            false => self.load(binding),
        }
        Ok(())
    }

    /// Read a class-scope slot the compiler reserved, or refuse — see [`Compiler::private_slot`].
    pub(super) fn load_private_slot(&mut self, slot: &str) -> Result<(), CompileError> {
        self.private_slot(slot, false)
    }

    /// Write one, leaving the value on the stack — see [`Compiler::private_slot`].
    pub(super) fn store_private_slot(&mut self, slot: &str) -> Result<(), CompileError> {
        self.private_slot(slot, true)
    }

    /// Emit a write of `name`, leaving the value on the stack.
    pub(super) fn store_name(&mut self, name: &str) -> Result<(), CompileError> {
        let binding = self.binding(name);
        self.note_arguments(name, binding);
        // The same as the read above — equivalent by behaviour, pinned by the same structural test,
        // and the difference is which of the two costs anything.
        if self.names_are_dynamic() {
            let index = self.name(name)?;
            self.chunk.emit(Instruction::StoreName(index));
            return Ok(());
        }
        match binding {
            // §9.1.1.1.5 step 5 — an immutable binding refuses every assignment, and the compiler
            // already knows which binding this is. What is left for run time is the *refusal*,
            // which happens after the right-hand side has run because §13.15.2 evaluates it first.
            //
            // Step 5.b decides whether the refusal is audible, and it is a question about the code
            // the assignment is written in as well as about the binding. A `const` throws wherever
            // it is written; §15.2.5's function name throws only in strict code, and in sloppy code
            // leaves the value where the store would have left it — an assignment evaluates to its
            // right-hand side whether or not anything kept it.
            Some(binding) if !binding.mutability.writes() => {
                if binding.mutability.refusal_throws(self.chunk.strict) {
                    self.chunk.emit(Instruction::ThrowImmutableAssignment);
                }
                Ok(())
            }
            Some(binding) => {
                self.store(binding);
                Ok(())
            }
            None => {
                let index = self.name(name)?;
                self.chunk.emit(Instruction::StoreGlobal(index));
                Ok(())
            }
        }
    }
}

impl Compiler<'_> {
    /// Give `name` a slot of its own even if the name is already taken.
    ///
    /// A catch parameter shadows anything outside it for the length of its block, which is why
    /// this pushes rather than reusing — and why [`Compiler::resolve`] searches from the end.
    pub(super) fn declare_shadowing(&mut self, name: &str) -> u32 {
        let slot = u32::try_from(self.locals.len()).unwrap_or(u32::MAX);
        self.locals.push(Local {
            name: name.into(),
            live: true,
            mutability: Mutability::Mutable,
        });
        self.high_water = self.high_water.max(self.locals.len());
        slot
    }

    /// A slot for the compiler's own use, under a name no source text can spell.
    ///
    /// `%` is not in `IdentifierStart` or `IdentifierPart`, so a script cannot name this and
    /// cannot reach it. The alternative is a second table of nameless slots, which is the same
    /// thing with more machinery.
    fn declare_hidden(&mut self, what: &str) -> u32 {
        let name = format!("%{what}{}", self.locals.len());
        self.declare_shadowing(&name)
    }

    /// The index the next instruction will have, as a jump target.
    fn here(&self) -> Result<u32, CompileError> {
        u32::try_from(self.chunk.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })
    }
}

/// How deeply an expression may nest before the compiler refuses it.
///
/// Not a limit the language has — an arbitrarily deep expression is legal, and every engine
/// refuses one somewhere. The number matters because two kinds of program come near it from
/// opposite directions: hand-written code nests a handful deep, and minified code chains
/// thousands of terms with `+` and `,`, each of which is one more level of tree.
///
/// Measured rather than guessed, and the measurement is smaller than it looks: on the 1 MiB main
/// thread Windows gives a program, a debug build — whose frames are largest — runs out of stack
/// somewhere between 200 and 400 levels. A test thread has eight times that, which is exactly the
/// trap: a limit checked only under `cargo test` would be four times too large in a real embedder.
///
/// 64 is well inside the smallest of those, and it is the parser's number for a reason: the
/// parser refuses nesting past `MAX_NESTING_DEPTH` before the compiler ever sees it, so a cap
/// above that one is unreachable from source and measured only by a test that builds the tree by
/// hand. The one shape the parser *can* build arbitrarily deep — a left-leaning operator chain —
/// is compiled with a loop rather than by recursing.
///
/// It was 128, and that was a measurement that rotted. A level cost about 5.5 KiB when the number
/// was set; slices since have added frames between one level and the next, and 128 of them came
/// to roughly 700 KiB — inside a mebibyte, but not inside the smaller stacks a debug build gets on
/// other platforms, where CI found it by aborting. `compiling_at_the_cap_fits_in_the_stack_it_
/// claims_to_need` is the guard that was missing: the parser has had one since DR-0006 and the
/// compiler, which recurses just as deeply, did not.
const MAX_EXPRESSION_DEPTH: u32 = 64;

/// The name of the slot a derived constructor's `this` occupies — DR-0015.
///
/// A `%` in front, which is the house convention for a slot no source text can spell: `%` is in
/// neither `IdentifierStart` nor `IdentifierPart`, so nothing a program can write resolves here. It
/// has a name at all only so that the locals table reads honestly when something goes wrong; the
/// slot is reached by the number the compiler kept, never by looking the name up.
const THIS_BINDING: &str = "%this";

struct Environment {
    /// The `PushScope` whose slot count is filled in when the block ends.
    scope: chunk::Unpatched,
    /// Every `CopyScope` in it, which take the same count and learn it at the same moment.
    copies: Vec<chunk::Unpatched>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Where {
    /// How many environments out — `0` is the running function's own.
    depth: u32,
    /// Which slot, in that environment.
    index: u32,
    /// What writing to it does — §9.1.1.1.5.
    ///
    /// Carried here rather than looked up again by whoever writes, because a second lookup is a
    /// second copy of the resolution rule: the two could disagree about *which* `x` a name means,
    /// and only one of them would be right. Resolving once answers both questions at once.
    mutability: Mutability,
}

/// Where a derived constructor's `this` lives, from where the compiler is standing — DR-0015.
///
/// Two numbers and no third, which is the point: see [`Compiler::this_binding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThisSlot {
    /// How many environments out — `0` in the constructor, one more per enclosing arrow.
    depth: u32,
    /// Which slot, in that environment.
    index: u32,
}

/// What a scope calls its slots, in slot order — DR-0018's name list.
///
/// **A slot the compiler made for itself keeps its `%` name** rather than being dropped. Dropping
/// one would shorten the list and every name after it would then answer for the wrong slot; `%` is
/// in neither `IdentifierStart` nor `IdentifierPart`, so leaving them in costs nothing a program
/// can reach.
///
/// **The list may run shorter than the environment's slots, and that needs nothing.**
/// [`Chunk::locals`] is a high-water mark across every level a body compiled, so a body whose
/// nested block needed more slots than it did gets an environment with slots past its own last
/// name. What a resolver needs is that index *i* be slot *i*, which a prefix gives; a slot past the
/// end simply has no name, which is the same answer padding under an unspellable one would produce
/// and one fewer thing to be wrong about.
///
/// **[`Local::live`] is not consulted, and that is the half worth explaining.** DR-0018 asks that
/// every name in a list be in scope for the environment's whole life, because a list has no
/// position to be read against where [`Compiler::resolve`] has one. That holds by construction and
/// not by a check here: a scope whose names go out of scope while the level around it carries on —
/// a `switch`, a `catch`, a class body — now opens an environment of its own, so its names go with
/// it. What is left calling [`Compiler::leave_scope`] without an environment declares nothing a
/// source can spell: a `for`-`in` and a `for`-`of` head take four `%` slots and put a `let` in
/// their per-iteration environment, and a block, a `for` head and a `catch` open an environment
/// exactly when they declare something. A new construct that flattened a scope would break this,
/// which is why the pairing is stated there rather than left to be noticed here.
fn bindings_of(locals: &[Local]) -> Rc<[crate::heap::Binding]> {
    locals
        .iter()
        .map(|local| crate::heap::Binding {
            name: local.name.clone(),
            mutability: local.mutability,
        })
        .collect()
}

/// §14.3.1's two lexical declarations, as what an assignment to one does.
///
/// `let` and `const` are the only productions that reach here, so this answers one of two.
/// [`Mutability::OwnName`] is §15.2.5's and is created where that clause is, which is the only
/// place in the language that makes a binding a program cannot write to and cannot be told so.
pub(super) fn lexical_mutability(immutable: bool) -> Mutability {
    match immutable {
        true => Mutability::Const,
        false => Mutability::Mutable,
    }
}

/// A refusal with a location.
fn unsupported(what: &'static str, span: Span) -> CompileError {
    CompileError {
        kind: ErrorKind::Unsupported(what),
        span,
    }
}

#[cfg(test)]
mod tests;
