//! A unit of compiled code, and the instruction set it is made of.
//!
//! Everything here is what the *interpreter* sees. The compiler's own working — how a name is
//! resolved, what it refuses — is next door; this is the surface between the two, and it is
//! deliberately small: an embedder holds a [`Chunk`], and a chunk holds instructions, constants,
//! a count of slots, and the bodies of the functions written inside it.

use crate::ast::{BinaryOperator, UnaryOperator};
use crate::compile::{CompileError, ErrorKind};
use crate::span::Span;
use crate::value::Value;
use std::rc::Rc;

/// One unit of compiled code — the instructions and the values they refer to.
///
/// Constants are held beside the code rather than inside it because a `Value` is 16 bytes and an
/// instruction should not be. It also means a String literal is put on the heap once, at compile
/// time, rather than each time the line runs.
#[derive(Debug, Default)]
pub struct Chunk {
    pub(super) code: Vec<Instruction>,
    pub(super) constants: Vec<Value>,
    pub(super) locals: usize,
    pub(super) parameters: usize,
    /// The bodies of the functions written inside this one, in the order they were met.
    ///
    /// An `Rc` because a function object has to outlive the code that made it — `var f = g()`
    /// keeps a closure alive after `g` has returned — and because a chunk is immutable once
    /// compiled and holds only chunks *below* it. DR-0010 rejects reference counting for the
    /// *heap*, where cycles are made before user code runs; a tree of code has none, so the
    /// argument does not reach here.
    pub(super) functions: Vec<Rc<Chunk>>,
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
    /// Push the value of a variable: this many environments out, at this index.
    ///
    /// One instruction rather than one per scope kind. The compiler built the chain of scopes, so
    /// it knows how far out a name is and nothing here searches for it — §9.1's environment
    /// records, resolved at compile time.
    LoadVariable(u32, u32),
    /// Store the top value into a variable, **without** taking it off the stack.
    ///
    /// Assignment is an expression: `a = (b = 1)` works because the inner one leaves its value
    /// behind. A statement that only wants the effect follows this with a [`Instruction::Pop`].
    StoreVariable(u32, u32),
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
    ///
    /// The callee gets no receiver, so its `this` is §10.2.1.2's substitution: the global object.
    Call(u32),
    /// Take a receiver, a callee and this many arguments, and call the callee *on* the receiver.
    ///
    /// A method call is not a plain call of a property's value. `o.m()` and `var f = o.m; f()`
    /// call the same function with different `this`, which is the whole reason the receiver
    /// travels with the call rather than with the function.
    CallMethod(u32),
    /// Push the running function's `this`.
    LoadThis,
    /// Push a copy of the top value.
    ///
    /// A method call needs the base twice — once to find the method on and once to call it with —
    /// and evaluating it twice would run its side effects twice.
    Duplicate,
    /// Leave the current function, taking the top value with it — §14.10.
    Return,
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
pub(super) struct Unpatched(usize);

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
    pub(super) fn patch_to(&mut self, jump: Unpatched, target: u32) {
        let Unpatched(at) = jump;
        if let Some(instruction) = self.code.get_mut(at) {
            *instruction = retarget(*instruction, target);
        }
    }

    /// Add an instruction.
    pub(super) fn emit(&mut self, instruction: Instruction) {
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
    pub(super) fn emit_jump(&mut self, make: impl FnOnce(u32) -> Instruction) -> Unpatched {
        let at = self.code.len();
        self.emit(make(u32::MAX));
        Unpatched(at)
    }

    /// Point a jump at wherever the next instruction will go.
    pub(super) fn patch(&mut self, jump: Unpatched) -> Result<(), CompileError> {
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
    pub(super) fn add_constant(&mut self, value: Value) -> Result<u32, CompileError> {
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

/// The same jump, pointed somewhere else.
///
/// Exhaustive on purpose. Written with a catch-all arm it silently did nothing to a `PushHandler`,
/// whose target then stayed at the unpatched placeholder — so every `try` jumped off the end of
/// its chunk. Listing every instruction means the next one that carries a target cannot be
/// forgotten here: leaving it out is a compile error.
pub(super) fn retarget(instruction: Instruction, target: u32) -> Instruction {
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
        | Instruction::LoadVariable(_, _)
        | Instruction::StoreVariable(_, _)
        | Instruction::SetCompletion
        | Instruction::Throw
        | Instruction::PopHandler
        | Instruction::MakeFunction(_)
        | Instruction::Call(_)
        | Instruction::CallMethod(_)
        | Instruction::LoadThis
        | Instruction::Duplicate
        | Instruction::Return
        | Instruction::NewObject
        | Instruction::DuplicateTwo
        | Instruction::DefineField
        | Instruction::GetProperty
        | Instruction::SetProperty
        | Instruction::DeleteProperty
        | Instruction::HasProperty => instruction,
    }
}
