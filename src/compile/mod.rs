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
//! - `chunk` — the code an embedder holds, and the instruction set.
//! - `expression` — §13, and everything that leaves one value on the stack.
//! - `function` — §15.2, where a body becomes a chunk of its own and a call is emitted.
//! - `statement` — §14, and everything that leaves the stack as it found it.
//! - here — the [`Chunk`] an embedder holds, the instruction set, and how a name is resolved.
//!
//! # What it can compile so far
//!
//! Expressions over the values that exist: literals, the unary operators, and the binary
//! operators that need neither an object nor a name to look up. Everything else is a
//! [`ErrorKind::Unsupported`] carrying the span of the thing it could not do — a refusal with
//! a location, not a panic and not a silent wrong answer. That list shrinks with each slice, and
//! the errors are how a reader can tell what is genuinely finished.

use crate::ast::{Expr, Script};
use crate::value::Value;
mod chunk;
mod expression;
mod function;
mod statement;

pub use self::chunk::{Chunk, Instruction, ShortCircuit};

use self::chunk::Unpatched;
use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;

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
    /// saying what it has not been taught, and it exists so that the answer to "does praxis
    /// support X" is a message with a span rather than a wrong value.
    Unsupported(&'static str),
    /// More than `u32::MAX` constants in one chunk.
    TooManyConstants,
    /// More than `u32::MAX` instructions in one chunk, so a jump could not name its target.
    TooLong,
    /// An expression nested deeper than the compiler will walk.
    TooDeep,
}

impl CompileError {
    /// A sentence describing the failure, without the span.
    pub fn message(&self) -> String {
        match self.kind {
            ErrorKind::Unsupported(what) => format!("{what} is not implemented yet"),
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
    let mut compiler = Compiler::new(heap);
    // §16.1.7 step 8's `GlobalDeclarationInstantiation`. A script's `var`s belong to the *global
    // object*, not to a scope of its own — which is why `var x = 1` at the top level makes
    // `globalThis.x` and a `let` never will. `VarDeclaredNames` of the whole body rather than of
    // the top level, because a `var` inside a block or a loop belongs to the script all the same;
    // that is the difference between `var` and everything that replaced it.
    //
    // `TopLevelVarDeclaredNames` would answer the same thing today. The two differ on exactly one
    // production, a *function declaration* at the top level, which is var-scoped and which
    // `hoist_functions` handles separately; this is the one that stays right for a Script.
    for name in var_declared_names(&script.body) {
        let index = compiler.name(name.name)?;
        compiler.chunk.emit(Instruction::DeclareGlobal(index));
    }
    // §16.1.7 `GlobalDeclarationInstantiation` step 17 — a script's `let` and `const` go in the
    // global *declarative* record rather than onto the global object, which is why these get slots
    // like any other lexical binding while the `var`s above became properties.
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
    /// Whether it was declared by `let` or `const` rather than by `var` or as a parameter.
    ///
    /// What it changes is the *dead zone*: a lexical binding starts uninitialised and reading it
    /// before its declaration is a ReferenceError, where a `var` reads as `undefined`.
    lexical: bool,
    /// Whether assigning to it is a TypeError — §9.1.1.1.5, and the whole of what `const` is.
    ///
    /// Known here rather than at run time because the compiler resolved the binding: an
    /// environment does not have to carry a mutability bit per slot to answer a question that was
    /// already settled when the name was looked up.
    immutable: bool,
}

impl Local {
    /// Whether this is what `name` refers to from where the compiler is standing.
    fn answers_to(&self, name: &str) -> bool {
        self.live && &*self.name == name
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
    /// How many locals existed when each enclosing loop began, innermost last.
    ///
    /// What it is for is §14.7.4.7 `CreatePerIterationEnvironment`. A `let` written inside a loop
    /// is a *fresh binding on every pass*, so a closure made on the third pass and one made on the
    /// fourth must see different variables. praxis gives each lexical declaration one slot for the
    /// whole call, which is right for every binding that is entered once and wrong for one that is
    /// entered again — and the difference is only observable through a closure.
    ///
    /// So this records where each loop started, and making a function is refused while a lexical
    /// binding declared inside the innermost loop is live. Refused rather than compiled, because
    /// the alternative is every closure in the loop sharing one variable and answering the last
    /// value — a wrong answer that looks like a working program.
    loop_marks: Vec<usize>,
    /// Where `continue` goes — the top of the innermost loop's test, or its update.
    continues: Vec<Vec<Unpatched>>,
    /// How deep into an expression the compiler currently is.
    depth: u32,
    /// The scopes this one is written inside, outermost first.
    ///
    /// The script's is at the front and the immediately enclosing function's at the back, so
    /// counting *backwards* from the end is counting environments outwards — which is exactly
    /// what a [`Instruction::LoadVariable`] depth means.
    outer: Vec<Vec<Local>>,
    /// Whether this is the script rather than a function body.
    ///
    /// §14.2.2's completion value belongs to the script; what a function's statements evaluate to
    /// is nobody's business but `return`'s. So an expression statement inside a function discards
    /// its value where one at the top level keeps it.
    is_script: bool,
    /// The most slots that were in use at once.
    ///
    /// Not `locals.len()` at the end: a catch parameter's slot is given back when its block ends,
    /// so the table shrinks. The frame still has to be big enough for the moment it was widest,
    /// which is what a run-time slot index is an index into.
    high_water: usize,
    /// Where each function declaration that was hoisted was written.
    ///
    /// [`Compiler::hoist_functions`] runs over the *top level* of a body and nothing else, so a
    /// declaration inside a block was never made — and the statement itself does nothing, because
    /// hoisting is supposed to have done it. Without this the two facts together would make
    /// `{ function g() {} } typeof g` answer `"undefined"` in silence, which is a wrong answer
    /// rather than a missing feature. §14.1 block-scopes such a declaration and Annex B.3.3
    /// hoists it in sloppy code; both need block scoping, so until then it is refused.
    hoisted: Vec<Span>,
    /// The labels in scope, and how many loops were open when each was met.
    ///
    /// A `break name` joins the break list of the loop that number identifies, which is what
    /// makes it leave *that* statement rather than the innermost one. Innermost last, so a
    /// backwards search finds the nearest of two labels with the same name — which the parser
    /// forbids, and which costs nothing to be right about.
    labels: Vec<(Box<str>, usize)>,
    /// For each enclosing `try` that has a `finally`, how many loops were open when it began.
    ///
    /// A `break` may not jump past a `finally`, and this is what tells the two cases apart: a
    /// loop opened inside the `try` has a greater depth than the number recorded here.
    finally_guards: Vec<usize>,
}

impl<'a> Compiler<'a> {
    fn new(heap: &'a mut Heap) -> Self {
        Self {
            chunk: Chunk::default(),
            heap,
            locals: Vec::new(),
            breaks: Vec::new(),
            scope_marks: Vec::new(),
            loop_marks: Vec::new(),
            continues: Vec::new(),
            hoisted: Vec::new(),
            labels: Vec::new(),
            finally_guards: Vec::new(),
            high_water: 0,
            depth: 0,
            outer: Vec::new(),
            is_script: true,
        }
    }

    fn finish(self) -> Chunk {
        let mut chunk = self.chunk;
        chunk.locals = self.high_water;
        chunk
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
    fn declare_lexical(&mut self, name: &str, immutable: bool) -> u32 {
        let slot = self.declare_shadowing(name);
        if let Some(local) = self.locals.last_mut() {
            local.lexical = true;
            local.immutable = immutable;
        }
        slot
    }

    /// Whether a function written here would close over a binding that a loop re-creates.
    ///
    /// True when the innermost enclosing loop has a live lexical binding declared inside it. See
    /// [`Compiler::loop_marks`] for why that is refused rather than compiled.
    fn would_capture_a_per_iteration_binding(&self) -> bool {
        let Some(&mark) = self.loop_marks.last() else {
            return false;
        };
        self.locals
            .iter()
            .skip(mark)
            .any(|local| local.live && local.lexical)
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
            let immutable = self.local(name).is_some_and(|local| local.immutable);
            return Some(Where {
                depth: 0,
                index,
                immutable,
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
            let immutable = scope.get(at).is_some_and(|local| local.immutable);
            return Some(Where {
                depth,
                index,
                immutable,
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

    /// Whether this compiler is the script's own, rather than some function's body.
    ///
    /// Derived rather than stored, because it is already recorded: a function body is compiled
    /// with the chain of scopes it is written inside, and only the script itself is written
    /// inside none. A separate flag could disagree with that; this cannot.
    pub(super) fn at_global_scope(&self) -> bool {
        self.outer.is_empty()
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

    /// Emit a read of `name`, from a scope if it has one and from the global object if not.
    ///
    /// §9.4.2 `ResolveBinding` walks the environment chain outwards and ends at the Global
    /// Environment Record, so a name the compiler cannot place is not an unknown name — it is a
    /// name whose binding, if any, is a property of the global object. Whether it is there is a
    /// question for run time, because a script can create one at any moment.
    pub(super) fn load_name(&mut self, name: &str) -> Result<(), CompileError> {
        match self.binding(name) {
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

    /// Emit a write of `name`, leaving the value on the stack.
    pub(super) fn store_name(&mut self, name: &str) -> Result<(), CompileError> {
        match self.binding(name) {
            // §9.1.1.1.5 step 3 — a `const` refuses every assignment, and the compiler already
            // knows which binding this is. What is left for run time is the throw, which happens
            // *after* the right-hand side has run because §13.15.2 evaluates it first.
            Some(binding) if binding.immutable => {
                self.chunk.emit(Instruction::ThrowImmutableAssignment);
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

/// Compile `expression` onto the end of `chunk`.
impl Compiler<'_> {}

impl Compiler<'_> {}

impl Compiler<'_> {
    /// Give `name` a slot of its own even if the name is already taken.
    ///
    /// A catch parameter shadows anything outside it for the length of its block, which is why
    /// this pushes rather than reusing — and why [`Compiler::resolve`] searches from the end.
    fn declare_shadowing(&mut self, name: &str) -> u32 {
        let slot = u32::try_from(self.locals.len()).unwrap_or(u32::MAX);
        self.locals.push(Local {
            name: name.into(),
            live: true,
            lexical: false,
            immutable: false,
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
/// 128 is well inside the smallest of those. It can be that small because the deep shapes are not
/// nesting: the parser refuses nesting past its own limit (DR-0006), and the one thing it *can*
/// build arbitrarily deep — a left-leaning operator chain — is compiled with a loop rather than
/// by recursing. What is left is bounded by the parser before it reaches here.
const MAX_EXPRESSION_DEPTH: u32 = 128;

/// Where a name lives — §9.1's environment records, resolved at compile time.
///
/// Named for the question rather than for the thing, because the syntax tree already has a
/// `Binding` and it means something else entirely: the *form* a declaration takes.
///
/// Two numbers rather than a name, because the compiler built the chain of scopes and so knows
/// the answer. Nothing at run time compares a string to find a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Where {
    /// How many environments out — `0` is the running function's own.
    depth: u32,
    /// Which slot, in that environment.
    index: u32,
    /// Whether writing to it is a TypeError — §9.1.1.1.5, and the whole of what `const` is.
    ///
    /// Carried here rather than looked up again by whoever writes, because a second lookup is a
    /// second copy of the resolution rule: the two could disagree about *which* `x` a name means,
    /// and only one of them would be right. Resolving once answers both questions at once.
    immutable: bool,
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
