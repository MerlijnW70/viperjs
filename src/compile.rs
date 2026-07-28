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
//! # What it can compile so far
//!
//! Expressions over the values that exist: literals, the unary operators, and the binary
//! operators that need neither an object nor a name to look up. Everything else is a
//! [`ErrorKind::Unsupported`] carrying the span of the thing it could not do — a refusal with
//! a location, not a panic and not a silent wrong answer. That list shrinks with each slice, and
//! the errors are how a reader can tell what is genuinely finished.

use crate::ast::{
    AssignmentOperator, AssignmentTarget, BinaryOperator, Binding, Declaration, DeclarationKind,
    Expr, ExprKind, ForInit, ForStatement, LogicalOperator, Script, Stmt, StmtKind, TryStatement,
    UnaryOperator,
};
use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;
use crate::value::Value;

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
        }
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
}

impl CompileError {
    /// A sentence describing the failure, without the span.
    pub fn message(&self) -> String {
        match self.kind {
            ErrorKind::Unsupported(what) => format!("{what} is not implemented yet"),
            ErrorKind::TooManyConstants => "too many constants in one unit of code".to_string(),
            ErrorKind::TooLong => "too many instructions in one unit of code".to_string(),
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

    fn statements(&mut self, statements: &[Stmt]) -> Result<(), CompileError> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }
}

/// Compile `expression` onto the end of `chunk`.
impl Compiler<'_> {
    /// Compile `expression` so that it leaves exactly one value on the stack.
    fn expression(&mut self, expression: &Expr) -> Result<(), CompileError> {
        let span = expression.span;
        match &expression.kind {
            // The literals. `undefined` is not among them because it is not a literal: it is an
            // identifier that happens to resolve to a property of the global object, which is why
            // `void 0` exists and why minifiers use it.
            ExprKind::Null => self.constant(Value::Null),
            ExprKind::Boolean(value) => self.constant(Value::Boolean(*value)),
            ExprKind::Number(value) => self.constant(Value::Number(*value)),
            ExprKind::String(units) => {
                let id = self.heap.new_string(units.clone());
                self.constant(Value::String(id))
            }
            ExprKind::Unary { operator, argument } => {
                // §13.5.1.2 — `delete` does not evaluate its operand to a *value*; it needs the
                // reference, which is a thing the compiler does not have until names exist.
                if *operator == UnaryOperator::Delete {
                    return Err(unsupported("the delete operator", span));
                }
                self.expression(argument)?;
                self.chunk.emit(Instruction::Unary(*operator));
                Ok(())
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => {
                if matches!(operator, BinaryOperator::Instanceof | BinaryOperator::In) {
                    return Err(unsupported("the instanceof and in operators", span));
                }
                // Left before right, always. §13.15.1 evaluates the left operand first and every
                // operand may have side effects, so the order the instructions are emitted in *is*
                // the order the language guarantees.
                self.expression(left)?;
                self.expression(right)?;
                self.chunk.emit(Instruction::Binary(*operator));
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
            // Everything else, named as the specification names it so that the message says which
            // clause is missing rather than which Rust variant.
            // §13.1.3 — a name is a slot, resolved here rather than looked up at run time.
            // A name with no slot is a *global*, which needs the global object, so refusing is
            // the honest answer until there is one — and it is why `undefined` is still spelled
            // `void 0` in this engine's tests.
            ExprKind::Identifier(name) => match self.resolve(name) {
                Some(slot) => {
                    self.chunk.emit(Instruction::LoadLocal(slot));
                    Ok(())
                }
                None => Err(unsupported("a reference to an undeclared name", span)),
            },
            ExprKind::This => Err(unsupported("this", span)),
            ExprKind::BigInt(_) => Err(unsupported("a BigInt literal", span)),
            ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
                Err(unsupported("a member expression", span))
            }
            ExprKind::Call { .. } => Err(unsupported("a call expression", span)),
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
                let ExprKind::Identifier(name) = &target.kind else {
                    return Err(unsupported("an assignment to a property", target.span));
                };
                let Some(slot) = self.resolve(name) else {
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
                        self.chunk.emit(Instruction::LoadLocal(slot));
                        self.expression(value)?;
                        self.chunk.emit(Instruction::Binary(binary));
                    }
                    // `a &&= v` and its two siblings only assign when the short circuit does not
                    // fire, so the store is *inside* the jump. Left for the slice that has a
                    // reason to build it.
                    None => return Err(unsupported("a logical assignment", span)),
                }
                self.chunk.emit(Instruction::StoreLocal(slot));
                Ok(())
            }
            ExprKind::Array(_) => Err(unsupported("an array literal", span)),
            ExprKind::Object(_) => Err(unsupported("an object literal", span)),
            ExprKind::Function(_) => Err(unsupported("a function expression", span)),
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
}

impl Compiler<'_> {
    /// Emit a constant and the instruction that pushes it.
    fn constant(&mut self, value: Value) -> Result<(), CompileError> {
        let index = self.chunk.add_constant(value)?;
        self.chunk.emit(Instruction::Constant(index));
        Ok(())
    }
}

impl Compiler<'_> {
    /// Compile one statement, leaving the stack as it found it.
    ///
    /// Every statement is stack-neutral: whatever it pushes it consumes, and the one thing it may
    /// leave behind is a new completion value, which lives in a register rather than on the
    /// stack. That invariant is what [`crate::vm::Fault::UnbalancedStack`] checks at the end.
    fn statement(&mut self, statement: &Stmt) -> Result<(), CompileError> {
        let span = statement.span;
        match &statement.kind {
            // §14.4 and §14.16 — neither does anything, and neither is value-producing, so the
            // completion value of `1; ;` is 1 rather than `undefined`.
            StmtKind::Empty | StmtKind::Debugger => Ok(()),
            // §14.5.1 — the only statement whose value is its own.
            StmtKind::Expression(expression) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::SetCompletion);
                Ok(())
            }
            // §14.2 — a block is its statements. No scope of its own yet: a block only *has* one
            // when something lexical is declared inside it, and `let` and `const` are refused
            // below until they can throw on a use before their declaration.
            StmtKind::Block(body) => self.statements(body),
            StmtKind::Declaration(declaration) => self.declaration(declaration, span),
            // §14.6 — the test is thrown away and exactly one branch runs. An `if` with no `else`
            // still jumps over nothing, which is one wasted instruction and no special case.
            StmtKind::If(statement) => {
                self.expression(&statement.test)?;
                let to_alternate = self.chunk.emit_jump(Instruction::JumpIfFalse);
                self.statement(&statement.consequent)?;
                let past_the_alternate = self.chunk.emit_jump(Instruction::Jump);
                self.chunk.patch(to_alternate)?;
                if let Some(alternate) = &statement.alternate {
                    self.statement(alternate)?;
                }
                self.chunk.patch(past_the_alternate)
            }
            // §14.7.2 — the test is at the top, so `while (0) x` never runs its body, and
            // `continue` goes back to that test.
            StmtKind::While(statement) => {
                let top = self.here()?;
                self.expression(&statement.test)?;
                let out = self.chunk.emit_jump(Instruction::JumpIfFalse);
                self.loop_body(&statement.body, |compiler| {
                    compiler.chunk.emit(Instruction::Jump(top));
                    Ok(top)
                })?;
                self.chunk.patch(out)
            }
            // §14.7.1 — the test is at the *bottom*, so the body always runs once, the jump back
            // is the opposite sense, and `continue` goes to the test rather than to the top.
            StmtKind::DoWhile(statement) => {
                let top = self.here()?;
                self.loop_body(&statement.body, |compiler| {
                    let test = compiler.here()?;
                    compiler.expression(&statement.test)?;
                    compiler.chunk.emit(Instruction::JumpIfTrue(top));
                    Ok(test)
                })?;
                Ok(())
            }
            // §14.7.4 — four parts, any of which may be missing. A missing test is `true`, which
            // is what makes `for (;;)` the shortest infinite loop.
            StmtKind::For(statement) => self.for_statement(statement, span),
            // §14.9 and §14.8 — a jump out of, or back to, the innermost loop.
            StmtKind::Break(None) => self.leave_loop(true, span),
            StmtKind::Continue(None) => self.leave_loop(false, span),
            StmtKind::Break(Some(_)) | StmtKind::Continue(Some(_)) => {
                Err(unsupported("a labelled break or continue", span))
            }
            // §14.14 — throw. The value travels up until a handler wants it, or out of the
            // script; nothing looks at what it is.
            StmtKind::Throw(expression) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Throw);
                Ok(())
            }
            // §14.15 — try, in its three shapes.
            StmtKind::Try(statement) => self.try_statement(statement, span),
            StmtKind::Switch(_) => Err(unsupported("switch", span)),
            StmtKind::ForInOf(_) => Err(unsupported("for-in and for-of", span)),
            StmtKind::Labelled(_) => Err(unsupported("a labelled statement", span)),
            StmtKind::With(_) => Err(unsupported("with", span)),
            StmtKind::Function(_) => Err(unsupported("a function declaration", span)),
            StmtKind::Class(_) => Err(unsupported("a class declaration", span)),
            StmtKind::Return(_) => Err(unsupported("return", span)),
        }
    }

    /// §14.15 — `try`, with or without each of its two tails.
    ///
    /// # Why the `finally` block is compiled twice
    ///
    /// It has to run on the way out however the `try` was left, and there are two ways: normally,
    /// and carrying a thrown value. The alternative to two copies is a subroutine that both paths
    /// call and that returns to a variable address — the shape that gave the JVM `jsr`/`ret` and
    /// then a decade of verifier bugs. Two copies of a block are larger and are obviously right.
    ///
    /// # The shape
    ///
    /// ```text
    ///   PushHandler(UNWIND)    ; only when there is a finally
    ///   PushHandler(CATCH)     ; only when there is a catch
    ///   <the try block>
    ///   PopHandler             ; the inner one
    ///   Jump(AFTER)
    /// CATCH:                   ; the thrown value is on the stack
    ///   StoreLocal(parameter); Pop
    ///   <the catch block>
    /// AFTER:
    ///   PopHandler             ; the outer one
    ///   <the finally block>
    ///   Jump(END)
    /// UNWIND:                  ; the thrown value is on the stack
    ///   StoreLocal(saved); Pop
    ///   <the finally block>
    ///   LoadLocal(saved)
    ///   Throw                  ; on to the next handler out
    /// END:
    /// ```
    ///
    /// The outer handler is what makes a throw *inside the catch block* still run the finally,
    /// which is the case a single handler gets wrong.
    fn try_statement(&mut self, statement: &TryStatement, span: Span) -> Result<(), CompileError> {
        let has_finally = statement.finalizer.is_some();
        // `break` and `continue` out of a `try` would have to run the finally on the way past,
        // which is a third exit path and a larger design. Refusing the ones that leave the `try`
        // is narrow and honest; a loop written *inside* the try is unaffected, which is why this
        // records the loop depth rather than refusing outright.
        if has_finally {
            self.finally_guards.push(self.breaks.len());
        }
        let unwind = has_finally.then(|| self.chunk.emit_jump(Instruction::PushHandler));
        let to_catch = statement
            .handler
            .as_ref()
            .map(|_| self.chunk.emit_jump(Instruction::PushHandler));

        let compiled = self.try_body(statement, to_catch);
        if has_finally {
            self.finally_guards.pop();
        }
        compiled?;

        if let Some(unwind) = unwind {
            // The normal way out: forget the handler, then run the finally.
            self.chunk.emit(Instruction::PopHandler);
            if let Some(finalizer) = &statement.finalizer {
                self.statements(finalizer)?;
            }
            let end = self.chunk.emit_jump(Instruction::Jump);
            // …and the other way out, carrying whatever was thrown. The value is parked in a
            // slot no source text can name, because the finally block may use the stack.
            self.chunk.patch(unwind)?;
            let saved = self.declare_hidden("thrown");
            self.chunk.emit(Instruction::StoreLocal(saved));
            self.chunk.emit(Instruction::Pop);
            if let Some(finalizer) = &statement.finalizer {
                self.statements(finalizer)?;
            }
            self.chunk.emit(Instruction::LoadLocal(saved));
            self.chunk.emit(Instruction::Throw);
            self.chunk.patch(end)?;
        }
        let _ = span;
        Ok(())
    }

    /// The try block and its catch clause, up to the point where the finally would begin.
    fn try_body(
        &mut self,
        statement: &TryStatement,
        to_catch: Option<Unpatched>,
    ) -> Result<(), CompileError> {
        self.statements(&statement.block)?;
        let Some(to_catch) = to_catch else {
            // No catch clause, so the try block's protection ends here and a throw inside it has
            // already gone to the finally's handler.
            return Ok(());
        };
        // The try block finished without throwing, so its handler is no longer wanted and the
        // catch block must be jumped over.
        self.chunk.emit(Instruction::PopHandler);
        let past_the_catch = self.chunk.emit_jump(Instruction::Jump);
        self.chunk.patch(to_catch)?;

        let Some(handler) = &statement.handler else {
            // Unreachable: `to_catch` exists only when the handler does. Saying so with a jump
            // that lands here rather than an assertion keeps this total.
            return self.chunk.patch(past_the_catch);
        };
        // §14.15.3 — the catch parameter is a *block-scoped* binding of its own, so it is given a
        // slot for the duration of the catch block and taken away again. `catch { }` with no
        // parameter is ES2019's optional binding: the value is simply discarded.
        let outer_locals = self.locals.len();
        match &handler.parameter {
            Some(parameter) => {
                let Binding::Identifier(name) = &parameter.binding else {
                    return Err(unsupported("a destructuring catch parameter", handler.span));
                };
                let slot = self.declare_shadowing(&name.name);
                self.chunk.emit(Instruction::StoreLocal(slot));
                self.chunk.emit(Instruction::Pop);
            }
            None => self.chunk.emit(Instruction::Pop),
        }
        self.statements(&handler.body)?;
        self.locals.truncate(outer_locals);
        self.chunk.patch(past_the_catch)
    }

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

    /// `break` or `continue` — a jump whose target is not compiled yet.
    ///
    /// The parser refuses either outside a loop, so an empty stack here is unreachable from
    /// source; refusing rather than emitting a jump to nowhere is what keeps it unreachable.
    fn leave_loop(&mut self, leaving: bool, span: Span) -> Result<(), CompileError> {
        let pending = if leaving {
            self.breaks.last_mut()
        } else {
            self.continues.last_mut()
        };
        if pending.is_none() {
            return Err(unsupported("break or continue outside a loop", span));
        }
        // Leaving a `try` that has a `finally` is a third way out of it, and the finally has to
        // run on the way past. A loop written inside the `try` is unaffected, which is what the
        // depth comparison is for: the jump only crosses the finally when its loop began before
        // the `try` did.
        if self
            .finally_guards
            .last()
            .is_some_and(|depth| self.breaks.len() <= *depth)
        {
            return Err(unsupported(
                "break or continue out of a try with a finally",
                span,
            ));
        }
        let jump = self.chunk.emit_jump(Instruction::Jump);
        let pending = if leaving {
            self.breaks.last_mut()
        } else {
            self.continues.last_mut()
        };
        if let Some(pending) = pending {
            pending.push(jump);
        }
        Ok(())
    }

    /// §14.3 — `var`, `let` and `const`.
    ///
    /// Only `var` so far. `let` and `const` are refused rather than treated as `var`, because the
    /// difference between them is the temporal dead zone: reading one before its declaration is a
    /// **ReferenceError**, and nothing can throw one yet. Quietly making them behave like `var`
    /// would be a wrong answer no test of this engine would catch.
    fn declaration(&mut self, declaration: &Declaration, span: Span) -> Result<(), CompileError> {
        if declaration.kind != DeclarationKind::Var {
            return Err(unsupported("let and const", span));
        }
        for declarator in &declaration.declarators {
            let Binding::Identifier(name) = &declarator.binding else {
                return Err(unsupported("a destructuring binding", declarator.span));
            };
            // A `var` with no initializer does nothing at all. The slot already holds `undefined`
            // from hoisting, and assigning it again would overwrite what an earlier `var x = 1`
            // put there: `var x = 1; var x;` leaves `x` as 1, which surprises people once.
            let Some(initializer) = &declarator.initializer else {
                continue;
            };
            let slot = self.declare(&name.name);
            self.expression(initializer)?;
            self.chunk.emit(Instruction::StoreLocal(slot));
            self.chunk.emit(Instruction::Pop);
        }
        Ok(())
    }

    /// §14.7.4's four parts.
    fn for_statement(&mut self, statement: &ForStatement, span: Span) -> Result<(), CompileError> {
        match &statement.init {
            Some(ForInit::Expression(expression)) => {
                self.expression(expression)?;
                self.chunk.emit(Instruction::Pop);
            }
            Some(ForInit::Declaration(declaration)) => self.declaration(declaration, span)?,
            None => {}
        }
        let top = self.here()?;
        // A missing test is `true` — `for (;;)` — so there is simply no jump out of the top.
        let out = match &statement.test {
            Some(test) => {
                self.expression(test)?;
                Some(self.chunk.emit_jump(Instruction::JumpIfFalse))
            }
            None => None,
        };
        let update = statement.update.as_ref();
        self.loop_body(&statement.body, |compiler| {
            // `continue` goes to the *update*, not to the test: `for (i = 0; i < 3; i = i + 1) {
            // continue; }` still increments, which is the whole reason the third part exists.
            let target = compiler.here()?;
            if let Some(update) = update {
                compiler.expression(update)?;
                compiler.chunk.emit(Instruction::Pop);
            }
            compiler.chunk.emit(Instruction::Jump(top));
            Ok(target)
        })?;
        match out {
            Some(out) => self.chunk.patch(out),
            None => Ok(()),
        }
    }

    /// Compile a loop body with somewhere for `break` and `continue` to go.
    ///
    /// `after` compiles whatever follows the body — the jump back, and a `for` loop's update —
    /// and answers where `continue` should land, which is not the same place in all three loops.
    /// Every `break` collected while the body was compiled is patched to just past everything.
    fn loop_body(
        &mut self,
        body: &Stmt,
        after: impl FnOnce(&mut Self) -> Result<u32, CompileError>,
    ) -> Result<(), CompileError> {
        self.breaks.push(Vec::new());
        self.continues.push(Vec::new());
        let compiled = self.statement(body).and_then(|()| after(self));
        // The two stacks come back down even when compilation failed, so that a later loop does
        // not inherit this one's pending jumps and patch them into its own end.
        let continues = self.continues.pop().unwrap_or_default();
        let breaks = self.breaks.pop().unwrap_or_default();
        let continue_target = compiled?;
        for jump in continues {
            self.chunk.patch_to(jump, continue_target);
        }
        for jump in breaks {
            self.chunk.patch(jump)?;
        }
        Ok(())
    }

    /// The index the next instruction will have, as a jump target.
    fn here(&self) -> Result<u32, CompileError> {
        u32::try_from(self.chunk.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })
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
        | Instruction::PopHandler => instruction,
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
mod tests {
    use super::*;
    use crate::parser::parse_expression;

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
            ("f()", "a call expression"),
            ("[1]", "an array literal"),
            ("({})", "an object literal"),
            ("1n", "a BigInt literal"),
            ("delete x.y", "the delete operator"),
            ("1 instanceof 2", "the instanceof and in operators"),
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
