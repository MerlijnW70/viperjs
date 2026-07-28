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
//! - `expression` — §13, and everything that leaves one value on the stack.
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

use crate::ast::{BinaryOperator, Expr, Script, UnaryOperator};
mod expression;
mod statement;

use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;
use crate::value::Value;
use std::rc::Rc;

/// One unit of compiled code — the instructions and the values they refer to.
///
/// Constants are held beside the code rather than inside it because a `Value` is 16 bytes and an
/// instruction should not be. It also means a String literal is put on the heap once, at compile
/// time, rather than each time the line runs.
#[derive(Debug, Default)]
pub struct Chunk {
    code: Vec<Instruction>,
    constants: Vec<Value>,
    locals: usize,
    parameters: usize,
    /// The bodies of the functions written inside this one, in the order they were met.
    ///
    /// An `Rc` because a function object has to outlive the code that made it — `var f = g()`
    /// keeps a closure alive after `g` has returned — and because a chunk is immutable once
    /// compiled and holds only chunks *below* it. DR-0010 rejects reference counting for the
    /// *heap*, where cycles are made before user code runs; a tree of code has none, so the
    /// argument does not reach here.
    functions: Vec<Rc<Chunk>>,
}

/// One instruction.
///
/// Deliberately few. An operator is one instruction carrying which operator it is, rather than one
/// instruction per operator: the dispatch inside [`crate::value::apply_binary`] is a `match` on
/// the same value either way, and twenty opcodes would be twenty things to keep in step with the
/// abstract operations rather than one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    /// Push the constant at this index.
    Constant(u32),
    /// Replace the value on top of the stack with the result of a unary operator.
    Unary(UnaryOperator),
    /// Replace the top two values with the result of a binary operator, left below right.
    Binary(BinaryOperator),
    /// Continue at this instruction instead of the next one.
    Jump(u32),
    /// Take the top value; if it is falsy, continue at this instruction instead.
    ///
    /// The value is consumed either way. This is the conditional operator's jump, where the test
    /// is asked about and then thrown away.
    JumpIfFalse(u32),
    /// Look at the top value; if the condition holds, continue at this instruction and **leave it
    /// where it is**. Otherwise take it and carry on.
    ///
    /// The short circuits' jump, and the reason they are not `if` in disguise: `a || b` is not
    /// `a ? true : b`, it is *`a` itself* when `a` is truthy. So the value that decided has to
    /// survive being the answer.
    JumpKeeping(ShortCircuit, u32),
    /// Take the top value; if it is truthy, continue at this instruction instead.
    ///
    /// The mirror of [`Instruction::JumpIfFalse`], and what a `do`/`while` loop jumps back with:
    /// its test is at the *bottom*, so the sense of the jump is reversed.
    JumpIfTrue(u32),
    /// Discard the top value.
    Pop,
    /// Push the value of the local variable in this slot.
    LoadLocal(u32),
    /// Store the top value into this slot, **without** taking it off the stack.
    ///
    /// Assignment is an expression: `a = (b = 1)` works because the inner one leaves its value
    /// behind. A statement that only wants the effect follows this with a [`Instruction::Pop`].
    StoreLocal(u32),
    /// Push a new empty ordinary object, inheriting from `Object.prototype`.
    NewObject,
    /// Push a copy of the top two values, in the same order.
    ///
    /// A compound assignment to a property reads it and then writes it, and both need the base
    /// and the key. Evaluating them twice would call `f` twice in `o[f()] += 1`, so they are
    /// evaluated once and copied — which is what makes the once-only guarantee an instruction
    /// rather than a promise.
    DuplicateTwo,
    /// Take a value and a key and file the value under it, leaving the object where it is.
    ///
    /// `CreateDataPropertyOrThrow` (§7.3.5): an object literal *defines* its properties rather
    /// than assigning them, which is why `{__proto__: 1}` in a literal is special and
    /// `o.__proto__ = 1` is not, and why a literal can shadow a non-writable inherited property.
    DefineField,
    /// Take a key and a base and push the property's value — `[[Get]]`, §10.1.8.
    GetProperty,
    /// Take a value, a key and a base; store the value and leave it on the stack — §10.1.9.
    SetProperty,
    /// Take a key and a base and push whether the property was there to remove — §13.5.1.
    DeleteProperty,
    /// Take a base and a key and push whether the base has it — §13.10.1's `in`.
    HasProperty,
    /// Take the top value and throw it — §14.14.
    ///
    /// Any value, not only an Error: `throw 1` is legal, and the specification never asks what
    /// was thrown. Where it lands is [`Instruction::PushHandler`]'s business.
    Throw,
    /// Remember that a throw from here until the matching pop should continue at this instruction.
    ///
    /// The operand stack's depth is remembered with it. A throw in the middle of an expression
    /// leaves whatever it had pushed behind, and unwinding has to put the stack back where the
    /// handler expects it — otherwise a caught exception would leave rubbish under every
    /// subsequent value.
    PushHandler(u32),
    /// Forget the innermost handler, because its protected region finished normally.
    PopHandler,
    /// Push a new function object over the nested body at this index.
    ///
    /// `InstantiateOrdinaryFunctionExpression` (§15.2.5) in the part that matters here: the
    /// object is made when the expression is *evaluated*, so two visits to the same `function`
    /// keyword make two objects, and `f !== f` across calls is the whole reason closures work.
    MakeFunction(u32),
    /// Take a callee and this many arguments and call it, leaving what it returned — §13.3.6.
    Call(u32),
    /// Leave the current function, taking the top value with it — §14.10.
    Return,
    /// Push the value of a script-level variable.
    ///
    /// Separate from [`Instruction::LoadLocal`] because a function's slots are counted from the
    /// bottom of *its* frame and the script's are counted from the bottom of everything. A
    /// function that reads a name declared at the top level is reaching past its own frame, and
    /// this is the instruction that says so.
    LoadScript(u32),
    /// Store the top value into a script-level variable, leaving it on the stack.
    StoreScript(u32),
    /// Take the top value and make it the script's completion value.
    ///
    /// §14.2.2 — a Script evaluates to the value of its last *value-producing* statement, which
    /// is what makes `eval("1; 2")` be 2 and `eval("var x = 1")` be `undefined`. A register
    /// rather than the stack, because a statement in the middle of a block has to be able to
    /// replace it without anything below being disturbed.
    SetCompletion,
}

/// When a [`Instruction::JumpKeeping`] jumps — one per short-circuiting operator (§13.13, §13.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortCircuit {
    /// `&&` — stop at the first falsy operand, which becomes the answer.
    WhenFalsy,
    /// `||` — stop at the first truthy one.
    WhenTruthy,
    /// `??` — stop at the first that is neither `null` nor `undefined`.
    ///
    /// A different test from `||`, and the whole reason `??` exists: `0 || 1` is `1` and
    /// `0 ?? 1` is `0`, because `0` is falsy and is not nullish.
    WhenNotNullish,
}

/// A jump that has been emitted and does not know where it goes yet.
///
/// Exists to make forgetting one impossible rather than unlikely. It is `#[must_use]`, it is not
/// `Copy`, and [`Chunk::patch`] takes it by value — so the only way to obtain one is to emit a
/// jump and the only way to be rid of one is to patch it. A dangling jump becomes a build failure
/// under the gate's denied warnings, which is a stronger claim than any test could make.
#[must_use = "a jump that is never patched jumps nowhere"]
struct Unpatched(usize);

impl Chunk {
    /// The instructions, in order.
    pub fn code(&self) -> &[Instruction] {
        &self.code
    }

    /// The constant at `index`, or `None` if there is none.
    ///
    /// Fallible because a `Chunk` can be built by hand, and a hand-built one may point anywhere.
    /// The compiler never produces such a chunk; the VM still has to answer for one, which is
    /// DR-0002 applied to the engine's own output rather than to a script's input.
    pub fn constant(&self, index: u32) -> Option<Value> {
        self.constants.get(index as usize).copied()
    }

    /// A chunk built by hand, out of instructions and constants that need not agree.
    ///
    /// The compiler does not use this — it emits as it goes — and nothing in the engine does. It
    /// is here so that a chunk the compiler would never produce can be *written*, which is the
    /// only way to reach [`crate::vm::Fault`] and therefore the only way to test that a malformed
    /// chunk is answered rather than crashed on.
    pub fn from_parts(code: Vec<Instruction>, constants: Vec<Value>) -> Self {
        Self {
            code,
            constants,
            locals: 0,
            parameters: 0,
            functions: Vec::new(),
        }
    }

    /// How many named parameters the function this code belongs to declares.
    ///
    /// §10.2.3's `length`: the count *before* the first default or rest parameter, which is why
    /// `function f(a, b = 1, c)` has a length of 1. Neither of those exists yet, so for now it is
    /// simply how many there are.
    pub fn parameters(&self) -> usize {
        self.parameters
    }

    /// The body of the nested function at this index, if there is one.
    pub fn function(&self, index: u32) -> Option<&Rc<Chunk>> {
        self.functions.get(index as usize)
    }

    /// How many local-variable slots the code addresses.
    ///
    /// The VM gives a frame this many slots, each starting as `undefined` — which is what makes a
    /// `var` readable before its declaration and holding nothing.
    pub fn locals(&self) -> usize {
        self.locals
    }

    /// Point a jump at a target that is already known, which a backward jump's is.
    fn patch_to(&mut self, jump: Unpatched, target: u32) {
        let Unpatched(at) = jump;
        if let Some(instruction) = self.code.get_mut(at) {
            *instruction = retarget(*instruction, target);
        }
    }

    /// Add an instruction.
    fn emit(&mut self, instruction: Instruction) {
        self.code.push(instruction);
    }

    /// Emit a jump whose target is not known yet.
    ///
    /// The target of a forward jump is decided by code that has not been compiled — that is what
    /// makes it forward — so a placeholder goes in and [`Chunk::patch`] replaces it once the
    /// destination exists.
    ///
    /// The placeholder is never seen, and not by luck: [`Unpatched`] is `#[must_use]` and is
    /// consumed by `patch`, so a jump left dangling is a warning and the gate denies warnings.
    /// It is `u32::MAX` anyway, so that if that ever stops being true the answer is a loud
    /// [`crate::vm::Fault::JumpOutOfRange`] rather than a quiet jump to the beginning.
    fn emit_jump(&mut self, make: impl FnOnce(u32) -> Instruction) -> Unpatched {
        let at = self.code.len();
        self.emit(make(u32::MAX));
        Unpatched(at)
    }

    /// Point a jump at wherever the next instruction will go.
    fn patch(&mut self, jump: Unpatched) -> Result<(), CompileError> {
        let target = u32::try_from(self.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })?;
        self.patch_to(jump, target);
        Ok(())
    }

    /// Add a constant and answer where it went.
    ///
    /// Does not look for an existing equal constant. Deduplicating would need `SameValue`, which
    /// needs the heap, and would save a few words per chunk — an M8 experiment with a measurement,
    /// not a guess.
    fn add_constant(&mut self, value: Value) -> Result<u32, CompileError> {
        let index = u32::try_from(self.constants.len());
        self.constants.push(value);
        // A chunk with more than four billion constants is not a program anyone wrote, and the
        // index has to fit somewhere. Refusing is the only answer that is neither a panic nor a
        // wrong constant.
        index.map_err(|_| CompileError {
            kind: ErrorKind::TooManyConstants,
            span: Span::new(0, 0),
        })
    }
}

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
    // §16.1.7 step 8's `GlobalDeclarationInstantiation`, in the part that is not about the global
    // object. `VarDeclaredNames` of the whole body rather than of the top level, because a `var`
    // inside a block or a loop belongs to the script all the same — that is the difference
    // between `var` and everything that replaced it.
    //
    // `TopLevelVarDeclaredNames` would answer the same thing today. The two differ on exactly one
    // production, a *function declaration* at the top level, which is var-scoped and which this
    // compiler refuses; when it stops refusing, they part company and this is the one that stays
    // right for a Script.
    for name in var_declared_names(&script.body) {
        compiler.declare(name.name);
    }
    compiler.hoist_functions(&script.body)?;
    compiler.statements(&script.body)?;
    Ok(compiler.finish())
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
    locals: Vec<Box<str>>,
    /// The jumps that leave the innermost loop, waiting for its end.
    ///
    /// One list per enclosing loop. `break` in a loop inside a loop leaves the inner one, which
    /// is what the stack is for.
    breaks: Vec<Vec<Unpatched>>,
    /// Where `continue` goes — the top of the innermost loop's test, or its update.
    continues: Vec<Vec<Unpatched>>,
    /// How deep into an expression the compiler currently is.
    depth: u32,
    /// The script's slots, by name — reachable from inside any function.
    ///
    /// Empty while the script itself is being compiled, because then [`Compiler::locals`] *is*
    /// that table and a name in it is an ordinary local.
    script_names: Vec<Box<str>>,
    /// The slots of every enclosing *function*, by name.
    ///
    /// Not to resolve against — to refuse against. A name found here needs a closure, and saying
    /// "that is a closure and there are none yet" is a far better answer than resolving it to the
    /// wrong slot or to nothing at all.
    enclosing_names: Vec<Box<str>>,
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
            continues: Vec::new(),
            finally_guards: Vec::new(),
            high_water: 0,
            depth: 0,
            script_names: Vec::new(),
            enclosing_names: Vec::new(),
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
        let at = self.locals.iter().rposition(|local| &**local == name)?;
        u32::try_from(at).ok()
    }

    /// Where `name` lives, from where the compiler is standing.
    ///
    /// Three answers and not two. A name is this function's own, or the script's, or an enclosing
    /// function's — and the third is refused rather than resolved, because reaching it needs a
    /// closure and there are none yet. Refusing is what keeps the difference visible: without it,
    /// `function outer() { var x; function inner() { return x; } }` would either find nothing or
    /// find the wrong `x`, and both are worse than a message.
    fn binding(&self, name: &str) -> Option<Where> {
        if let Some(slot) = self.resolve(name) {
            return Some(Where::Local(slot));
        }
        if let Some(at) = self.script_names.iter().rposition(|local| &**local == name) {
            return u32::try_from(at).ok().map(Where::Script);
        }
        if self.enclosing_names.iter().any(|local| &**local == name) {
            return Some(Where::Captured);
        }
        None
    }

    /// Emit the instruction that reads `binding`.
    fn load(&mut self, binding: Where, span: Span) -> Result<(), CompileError> {
        match binding {
            Where::Local(slot) => self.chunk.emit(Instruction::LoadLocal(slot)),
            Where::Script(slot) => self.chunk.emit(Instruction::LoadScript(slot)),
            Where::Captured => return Err(unsupported("a closure over an outer variable", span)),
        }
        Ok(())
    }

    /// Emit the instruction that writes `binding`, leaving the value on the stack.
    fn store(&mut self, binding: Where, span: Span) -> Result<(), CompileError> {
        match binding {
            Where::Local(slot) => self.chunk.emit(Instruction::StoreLocal(slot)),
            Where::Script(slot) => self.chunk.emit(Instruction::StoreScript(slot)),
            Where::Captured => return Err(unsupported("a closure over an outer variable", span)),
        }
        Ok(())
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
        self.locals.push(name.into());
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

/// The same jump, pointed somewhere else.
///
/// Exhaustive on purpose. Written with a catch-all arm it silently did nothing to a `PushHandler`,
/// whose target then stayed at the unpatched placeholder — so every `try` jumped off the end of
/// its chunk. Listing every instruction means the next one that carries a target cannot be
/// forgotten here: leaving it out is a compile error.
fn retarget(instruction: Instruction, target: u32) -> Instruction {
    match instruction {
        Instruction::Jump(_) => Instruction::Jump(target),
        Instruction::JumpIfFalse(_) => Instruction::JumpIfFalse(target),
        Instruction::JumpIfTrue(_) => Instruction::JumpIfTrue(target),
        Instruction::JumpKeeping(condition, _) => Instruction::JumpKeeping(condition, target),
        Instruction::PushHandler(_) => Instruction::PushHandler(target),
        // Not a jump. An `Unpatched` can only ever name one, since `emit_jump` is the only way
        // to make one — so these are unreachable, and are listed rather than swept into a
        // catch-all so that a new jump cannot hide among them.
        Instruction::Constant(_)
        | Instruction::Unary(_)
        | Instruction::Binary(_)
        | Instruction::Pop
        | Instruction::LoadLocal(_)
        | Instruction::StoreLocal(_)
        | Instruction::SetCompletion
        | Instruction::Throw
        | Instruction::PopHandler
        | Instruction::MakeFunction(_)
        | Instruction::Call(_)
        | Instruction::Return
        | Instruction::LoadScript(_)
        | Instruction::StoreScript(_)
        | Instruction::NewObject
        | Instruction::DuplicateTwo
        | Instruction::DefineField
        | Instruction::GetProperty
        | Instruction::SetProperty
        | Instruction::DeleteProperty
        | Instruction::HasProperty => instruction,
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

/// Where a name lives — §9.1's environment records, flattened to what this engine has.
///
/// Named for the question rather than for the thing, because the syntax tree already has a
/// `Binding` and it means something else entirely: the *form* a declaration takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// A slot in the running function's own frame: a parameter, or a `var` it declares.
    Local(u32),
    /// A slot in the script's frame, which every function can reach past its own.
    Script(u32),
    /// A slot in an enclosing *function*, which needs a closure and has none yet.
    Captured,
}

/// A refusal with a location.
fn unsupported(what: &'static str, span: Span) -> CompileError {
    CompileError {
        kind: ErrorKind::Unsupported(what),
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ExprKind, Stmt, StmtKind};
    use crate::parser::parse_expression;
    use crate::parser::parse_script;

    fn compile(source: &str) -> Result<Chunk, CompileError> {
        let mut heap = Heap::new();
        let expression = parse_expression(source).expect("the source parses"); // a compiler test needs a tree
        compile_expression(&expression, &mut heap)
    }

    #[test]
    fn an_operator_is_emitted_after_both_of_its_operands() {
        // The one structural claim worth making about the output: the order is the order §13.15.1
        // guarantees, and it is what makes `f() + g()` call `f` first. Everything else about the
        // compiler is checked by running it — see the VM's tests.
        let chunk = compile("1 + 2").expect("compiles"); // the test is about the output
        assert_eq!(
            chunk.code(),
            [
                Instruction::Constant(0),
                Instruction::Constant(1),
                Instruction::Binary(BinaryOperator::Add),
                // …and then the value becomes the chunk's completion value, which is what makes
                // an expression and a script the same kind of thing to run.
                Instruction::SetCompletion,
            ]
        );
        assert!(matches!(chunk.constant(0), Some(Value::Number(value)) if value == 1.0));
        assert!(matches!(chunk.constant(1), Some(Value::Number(value)) if value == 2.0));
        assert!(chunk.constant(2).is_none());
    }

    #[test]
    fn a_construct_that_is_not_implemented_yet_says_so_and_says_where() {
        // The parser accepted every one of these. Refusing with a span is the difference between
        // "praxis cannot do this yet" and a wrong answer nobody notices.
        let cases = [
            ("x", "a reference to an undeclared name"),
            ("f?.()", "optional chaining"),
            ("[1]", "an array literal"),
            (
                "({a})",
                "a shorthand, spread or method in an object literal",
            ),
            ("1n", "a BigInt literal"),
            ("delete x", "deleting a name"),
            ("1 instanceof 2", "the instanceof operator"),
            ("`a`", "a template literal"),
            ("1 ? b : c", "a reference to an undeclared name"),
            ("/re/", "a regular expression literal"),
        ];
        for (source, what) in cases {
            let error = compile(source).expect_err("not implemented yet"); // the test is about the error
            assert_eq!(
                error.kind,
                ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
            assert!(error.message().contains(what));
            // The span points at the construct rather than at the whole program.
            assert!(
                error.span.end <= source.len() as u32,
                "the span of {source:?} is inside it"
            );
        }
    }

    /// An expression nested `depth` levels deep, built rather than parsed.
    ///
    /// `!!!…1`, which is the cheapest shape that costs one level of tree per character. The
    /// parser refuses one this deep long before the compiler would see it (DR-0006), which is
    /// exactly why it is built here: a guard nothing can reach is a guard nothing can check.
    fn nested(depth: u32) -> Expr {
        let mut expression = Expr::new(ExprKind::Number(1.0), Span::new(0, 1));
        for _ in 0..depth {
            expression = Expr::new(
                ExprKind::Unary {
                    operator: UnaryOperator::LogicalNot,
                    argument: Box::new(expression),
                },
                Span::new(0, 1),
            );
        }
        expression
    }

    #[test]
    fn an_expression_is_refused_one_level_past_the_limit_and_not_one_before() {
        let mut heap = Heap::new();
        // `nested(n)` is `n` operators over one literal, so it is `n + 1` levels deep. The limit
        // is on the levels, so `MAX - 1` operators is the deepest that compiles.
        let deepest = nested(MAX_EXPRESSION_DEPTH - 1);
        assert!(compile_expression(&deepest, &mut heap).is_ok());

        let one_too_deep = nested(MAX_EXPRESSION_DEPTH);
        let error = compile_expression(&one_too_deep, &mut heap).expect_err("one level too deep"); // the test is about the error
        assert_eq!(error.kind, ErrorKind::TooDeep);
        assert!(error.message().contains("nested too deeply"));

        // …and the counter comes back down, so a compiler that refused once can compile again.
        // Written with an early return this leaked a level per refusal, which nothing observes
        // today and would observe the moment one compiler compiled two things.
        let mut compiler = Compiler::new(&mut heap);
        assert!(compiler.expression(&one_too_deep).is_err());
        assert_eq!(compiler.depth, 0);
        assert!(compiler.expression(&nested(1)).is_ok());
    }

    #[test]
    fn a_property_reference_the_parser_cannot_build_is_still_refused() {
        // The parser wraps an optional chain in `OptionalChain` and refuses `#x` outside a class,
        // so neither flag reaches the compiler from source. The *tree* can hold them, and a
        // compiler that ignored them would read `o?.a` as `o.a` — a wrong answer rather than a
        // refusal — the day the wrapper is handled. So the guards are checked where they can be
        // reached, which is here.
        let mut heap = Heap::new();
        let object = || Box::new(Expr::new(ExprKind::Number(1.0), Span::new(0, 1)));
        let cases = [
            (
                ExprKind::Member {
                    private: true,
                    optional: false,
                    object: object(),
                    property: "x".into(),
                },
                "a private name",
            ),
            (
                ExprKind::Member {
                    private: false,
                    optional: true,
                    object: object(),
                    property: "x".into(),
                },
                "optional chaining",
            ),
            (
                ExprKind::ComputedMember {
                    optional: true,
                    object: object(),
                    property: object(),
                },
                "optional chaining",
            ),
        ];
        for (kind, what) in cases {
            let expression = Expr::new(kind, Span::new(0, 4));
            let error = compile_expression(&expression, &mut heap).expect_err("refused"); // the test is about the error
            assert_eq!(error.kind, ErrorKind::Unsupported(what));
        }

        // A reference that is neither kind of member is refused too — the arm that catches a
        // tree nobody should have built.
        let not_a_reference = Expr::new(ExprKind::Number(1.0), Span::new(0, 1));
        let mut compiler = Compiler::new(&mut heap);
        let error = compiler
            .property_reference(&not_a_reference)
            .expect_err("not a property reference"); // same
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported("a reference to something that is not a property")
        );
    }

    #[test]
    fn a_return_at_the_top_level_is_refused_by_the_compiler_too() {
        // The parser refuses it (§14.10's early error), so no source reaches this. A `Return` in
        // the script's own chunk would be a `ReturnWithNoCall` at run time — a fault, which is to
        // say a bug in this compiler — so the guard is worth having and is checked against a tree
        // built by hand.
        let mut heap = Heap::new();
        let script = Script {
            body: Box::new([Stmt {
                kind: StmtKind::Return(None),
                span: Span::new(0, 7),
            }]),
            span: Span::new(0, 7),
        };
        let error = compile_script(&script, &mut heap).expect_err("no function to return from"); // the test is about the error
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported("return outside a function")
        );
        assert!(crate::parser::parse_script("return 1;").is_err());
    }

    #[test]
    fn an_undeclared_name_inside_a_function_is_not_reported_as_a_closure() {
        // Two refusals that would be easy to confuse, and the difference matters to whoever reads
        // the message: one says "this engine cannot do that yet" and the other says "you have a
        // typo". The test needs an enclosing function with a local, so that the closure check has
        // something to look at and still answers correctly.
        let mut heap = Heap::new();
        let cases = [
            (
                "function outer() { var x; function inner() { return x; } }",
                "a closure over an outer variable",
            ),
            (
                "function outer() { var x; function inner() { return nowhere; } }",
                "a reference to an undeclared name",
            ),
            (
                "function outer() { var x; function inner() { nowhere = 1; } }",
                "an assignment to an undeclared name",
            ),
        ];
        for (source, what) in cases {
            let script = parse_script(source).expect("the row parses"); // a row that does not is the bug
            let error = compile_script(&script, &mut heap).expect_err("refused"); // same
            assert_eq!(
                error.kind,
                ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
        }
    }

    #[test]
    fn an_optional_call_the_parser_cannot_build_is_still_refused() {
        // As with `o?.a`, the parser wraps `f?.()` in an `OptionalChain` and the inner flag never
        // arrives on its own. The tree can hold it, and a compiler that ignored it would call
        // through a `null` callee the day the wrapper is handled.
        let mut heap = Heap::new();
        let callee = Box::new(Expr::new(ExprKind::Number(1.0), Span::new(0, 1)));
        let expression = Expr::new(
            ExprKind::Call {
                optional: true,
                callee,
                arguments: Box::new([]),
            },
            Span::new(0, 5),
        );
        let error = compile_expression(&expression, &mut heap).expect_err("refused"); // the test is about the error
        assert_eq!(error.kind, ErrorKind::Unsupported("optional chaining"));
    }

    #[test]
    fn a_break_with_no_loop_around_it_is_refused_rather_than_left_dangling() {
        // The parser refuses this, so no source reaches it — but the syntax tree can be *built*,
        // the same way a malformed chunk can be built for the VM. Without the check, the jump
        // would be emitted and never patched, and a script would leap somewhere at run time
        // because of a bug in the compiler rather than anything in the source.
        let mut heap = Heap::new();
        for kind in [StmtKind::Break(None), StmtKind::Continue(None)] {
            let script = Script {
                body: Box::new([Stmt {
                    kind,
                    span: Span::new(0, 5),
                }]),
                span: Span::new(0, 5),
            };
            let error = compile_script(&script, &mut heap).expect_err("no loop to leave"); // the test is about the error
            assert_eq!(
                error.kind,
                ErrorKind::Unsupported("break or continue outside a loop")
            );
        }
        // …and the parser does refuse it, which is why nothing but a hand-built tree gets here.
        assert!(crate::parser::parse_script("break;").is_err());
        assert!(crate::parser::parse_script("continue;").is_err());
    }

    #[test]
    fn a_refusal_deep_inside_an_expression_carries_the_inner_span() {
        // The refusal comes from where the trouble is, not from the top: an engine that reported
        // the whole line would be useless on a long one.
        let error = compile("1 + 2 * (3 - x)").expect_err("x is not implemented yet"); // same
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported("a reference to an undeclared name")
        );
        assert_eq!(error.span, Span::new(13, 14));
    }
}
