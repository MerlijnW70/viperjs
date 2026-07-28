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

use crate::ast::{BinaryOperator, Expr, ExprKind, LogicalOperator, UnaryOperator};
use crate::heap::Heap;
use crate::span::Span;
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
    /// Discard the top value.
    Pop,
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
        Self { code, constants }
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
        let Unpatched(at) = jump;
        let target = u32::try_from(self.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })?;
        if let Some(instruction) = self.code.get_mut(at) {
            *instruction = match *instruction {
                Instruction::Jump(_) => Instruction::Jump(target),
                Instruction::JumpIfFalse(_) => Instruction::JumpIfFalse(target),
                Instruction::JumpKeeping(condition, _) => {
                    Instruction::JumpKeeping(condition, target)
                }
                other => other,
            };
        }
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

/// Compile one expression into a chunk that leaves its value on the stack.
///
/// Takes the heap because a String literal is a heap value and the compiler is where it is made.
pub fn compile_expression(expression: &Expr, heap: &mut Heap) -> Result<Chunk, CompileError> {
    let mut chunk = Chunk::default();
    compile_into(&mut chunk, expression, heap)?;
    Ok(chunk)
}

/// Compile `expression` onto the end of `chunk`.
fn compile_into(chunk: &mut Chunk, expression: &Expr, heap: &mut Heap) -> Result<(), CompileError> {
    let span = expression.span;
    match &expression.kind {
        // The literals. `undefined` is not among them because it is not a literal: it is an
        // identifier that happens to resolve to a property of the global object, which is why
        // `void 0` exists and why minifiers use it.
        ExprKind::Null => constant(chunk, Value::Null),
        ExprKind::Boolean(value) => constant(chunk, Value::Boolean(*value)),
        ExprKind::Number(value) => constant(chunk, Value::Number(*value)),
        ExprKind::String(units) => {
            let id = heap.new_string(units.clone());
            constant(chunk, Value::String(id))
        }
        ExprKind::Unary { operator, argument } => {
            // §13.5.1.2 — `delete` does not evaluate its operand to a *value*; it needs the
            // reference, which is a thing the compiler does not have until names exist.
            if *operator == UnaryOperator::Delete {
                return Err(unsupported("the delete operator", span));
            }
            compile_into(chunk, argument, heap)?;
            chunk.emit(Instruction::Unary(*operator));
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
            compile_into(chunk, left, heap)?;
            compile_into(chunk, right, heap)?;
            chunk.emit(Instruction::Binary(*operator));
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
            compile_into(chunk, left, heap)?;
            let over_the_right =
                chunk.emit_jump(|target| Instruction::JumpKeeping(condition, target));
            compile_into(chunk, right, heap)?;
            chunk.patch(over_the_right)
        }
        // §13.14 — the conditional operator, where the test *is* thrown away and exactly one of
        // the two branches runs.
        ExprKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            compile_into(chunk, test, heap)?;
            let to_alternate = chunk.emit_jump(Instruction::JumpIfFalse);
            compile_into(chunk, consequent, heap)?;
            let past_the_alternate = chunk.emit_jump(Instruction::Jump);
            chunk.patch(to_alternate)?;
            compile_into(chunk, alternate, heap)?;
            chunk.patch(past_the_alternate)
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
                compile_into(chunk, expression, heap)?;
                chunk.emit(Instruction::Pop);
            }
            compile_into(chunk, last, heap)
        }
        // Everything else, named as the specification names it so that the message says which
        // clause is missing rather than which Rust variant.
        ExprKind::Identifier(_) => Err(unsupported("an identifier reference", span)),
        ExprKind::This => Err(unsupported("this", span)),
        ExprKind::BigInt(_) => Err(unsupported("a BigInt literal", span)),
        ExprKind::Member { .. } | ExprKind::ComputedMember { .. } => {
            Err(unsupported("a member expression", span))
        }
        ExprKind::Call { .. } => Err(unsupported("a call expression", span)),
        ExprKind::New { .. } => Err(unsupported("the new operator", span)),
        ExprKind::Update { .. } => Err(unsupported("an update expression", span)),
        ExprKind::Assignment { .. } => Err(unsupported("an assignment", span)),
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

/// Emit a constant and the instruction that pushes it.
fn constant(chunk: &mut Chunk, value: Value) -> Result<(), CompileError> {
    let index = chunk.add_constant(value)?;
    chunk.emit(Instruction::Constant(index));
    Ok(())
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
            ("x", "an identifier reference"),
            ("f()", "a call expression"),
            ("[1]", "an array literal"),
            ("({})", "an object literal"),
            ("1n", "a BigInt literal"),
            ("delete x.y", "the delete operator"),
            ("1 instanceof 2", "the instanceof and in operators"),
            ("`a`", "a template literal"),
            ("1 ? b : c", "an identifier reference"),
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
    fn a_refusal_deep_inside_an_expression_carries_the_inner_span() {
        // The refusal comes from where the trouble is, not from the top: an engine that reported
        // the whole line would be useless on a long one.
        let error = compile("1 + 2 * (3 - x)").expect_err("x is not implemented yet"); // same
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported("an identifier reference")
        );
        assert_eq!(error.span, Span::new(13, 14));
    }
}
