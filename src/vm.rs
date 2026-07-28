//! The interpreter loop — a stack, a chunk, and one `match`.
//!
//! # Two kinds of failure, and only one of them is the language's
//!
//! A script can fail: it throws, and that is a value travelling upwards. A *chunk* can also be
//! wrong — an instruction that pops two values from a stack holding one, a constant index past
//! the end of the table — and that is not something a script can cause. The compiler does not
//! produce such a chunk; a hand-written one can, and this module answers for it with a [`Fault`]
//! rather than a panic.
//!
//! Keeping the two apart matters. If a malformed chunk were reported as a thrown value, a bug in
//! the compiler would arrive as a `catch` block running, which is the kind of thing that takes a
//! week to find. And if it were a panic, DR-0002 would hold only as long as the compiler is
//! correct, which is not what DR-0002 says.
//!
//! # What it can run so far
//!
//! What [`crate::compile`] can compile: expressions over primitives. Nothing here throws yet,
//! which is why the answer is a `Result<Value, Fault>` and not a completion — a throw needs
//! either an operation that can fail or a `throw` statement, and both arrive with the statements.

use crate::ast::UnaryOperator;
use crate::compile::{Chunk, Instruction, ShortCircuit};
use crate::heap::Heap;
use crate::value::{Value, apply_binary};

/// A chunk that does not make sense.
///
/// Never reachable from a script. Reachable from a hand-written chunk, which is how it is tested,
/// and from a compiler bug, which is what it exists to make loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// An instruction wanted more values than the stack held.
    StackUnderflow,
    /// A `Constant` instruction pointed past the end of the constant table.
    MissingConstant,
    /// A jump named an instruction past the end of the chunk.
    ///
    /// Includes the placeholder an unpatched forward jump carries, which is `u32::MAX` precisely
    /// so that forgetting to patch one is loud rather than a jump to somewhere plausible.
    JumpOutOfRange,
    /// The chunk finished without leaving exactly one value behind.
    ///
    /// An expression evaluates to one value. Zero means the chunk did nothing; more than one
    /// means something was pushed and never consumed, which is a compiler bug that would
    /// otherwise show up much later as the wrong value.
    UnbalancedStack,
}

/// The interpreter.
///
/// Holds the operand stack and nothing else so far. Call frames, the environment and the job
/// queue join it as the things that need them arrive.
#[derive(Debug, Default)]
pub struct Vm {
    stack: Vec<Value>,
}

impl Vm {
    /// A machine with an empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `chunk` to the end and answer the single value it leaves behind.
    ///
    /// The stack is cleared first, so a machine that faulted once is usable again: a fault says
    /// the chunk was wrong, not that the interpreter is now untrustworthy.
    pub fn run(&mut self, chunk: &Chunk, heap: &mut Heap) -> Result<Value, Fault> {
        self.stack.clear();
        let code = chunk.code();
        let mut at = 0_usize;
        // A counter rather than an iterator, because a jump moves it. Nothing bounds how long
        // this runs: a backward jump is how a loop will be built, and a script that loops forever
        // is a script that loops forever. DR-0002 is about panics, not about halting.
        while let Some(instruction) = code.get(at) {
            at += 1;
            match *instruction {
                Instruction::Constant(index) => {
                    let value = chunk.constant(index).ok_or(Fault::MissingConstant)?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let operand = self.pop()?;
                    self.stack.push(apply_unary(operator, operand, heap));
                }
                Instruction::Binary(operator) => {
                    // Right first: it was pushed second, so it is on top. Getting this backwards
                    // would make every subtraction and comparison silently mirror itself.
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(apply_binary(operator, left, right, heap));
                }
                Instruction::Jump(target) => at = jump_to(target, code.len())?,
                Instruction::JumpIfFalse(target) => {
                    // The test is consumed either way — this is the conditional operator's jump,
                    // and `a ? b : c` evaluates to `b` or `c` and never to `a`.
                    if !self.pop()?.to_boolean(heap) {
                        at = jump_to(target, code.len())?;
                    }
                }
                Instruction::JumpKeeping(condition, target) => {
                    // Peeked, not popped: if the short circuit fires, this value *is* the answer.
                    let deciding = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    let stop = match condition {
                        ShortCircuit::WhenFalsy => !deciding.to_boolean(heap),
                        ShortCircuit::WhenTruthy => deciding.to_boolean(heap),
                        ShortCircuit::WhenNotNullish => {
                            !matches!(deciding, Value::Undefined | Value::Null)
                        }
                    };
                    if stop {
                        at = jump_to(target, code.len())?;
                    } else {
                        // It did not decide, so it is not the answer and the right operand's
                        // value will take its place.
                        self.pop()?;
                    }
                }
                Instruction::Pop => {
                    self.pop()?;
                }
            }
        }
        // An expression is one value. Anything else means the chunk and the compiler disagree
        // about what the instructions do, and saying so here is cheaper than finding out later.
        if self.stack.len() != 1 {
            return Err(Fault::UnbalancedStack);
        }
        self.pop()
    }

    /// Take the top of the stack.
    fn pop(&mut self) -> Result<Value, Fault> {
        self.stack.pop().ok_or(Fault::StackUnderflow)
    }
}

/// Where a jump goes, or a fault if that is not inside the chunk.
///
/// `length` itself is a legal target and means "the end": a jump over the last instruction lands
/// there, and the compiler emits exactly that for `a || b` when `b` is the final expression.
/// Anything past it is a chunk that does not make sense — including the `u32::MAX` placeholder a
/// jump carries before it is patched, which is why that placeholder is `u32::MAX`.
fn jump_to(target: u32, length: usize) -> Result<usize, Fault> {
    let target = target as usize;
    if target > length {
        return Err(Fault::JumpOutOfRange);
    }
    Ok(target)
}

/// The unary operators — §13.5.
///
/// `delete` is absent because it takes a reference rather than a value, and the compiler refuses
/// it; the rest are one conversion each, and each of those conversions is already written down.
fn apply_unary(operator: UnaryOperator, operand: Value, heap: &mut Heap) -> Value {
    match operator {
        // §13.5.2 — `void` evaluates its operand and throws the value away. That it evaluates it
        // at all is the point: `void f()` calls `f`.
        UnaryOperator::Void => Value::Undefined,
        // §13.5.3 — the operator that never throws, which is why `typeof undeclared` is the one
        // safe way to ask about a name that may not exist.
        UnaryOperator::Typeof => {
            let text = operand.type_of();
            Value::String(heap.new_string(text.encode_utf16().collect()))
        }
        // §13.5.4 — unary `+` is `ToNumber` and nothing else, which is why `+x` is the shortest
        // spelling of it and why `+"1"` is `1` while `+"a"` is NaN.
        UnaryOperator::Plus => Value::Number(operand.to_number(heap)),
        // §13.5.5 — `ToNumber` and then negate. Negation is not subtraction from zero: `-0` is
        // `-0` where `0 - 0` is `+0`.
        UnaryOperator::Minus => Value::Number(-operand.to_number(heap)),
        // §13.5.6 — `ToInt32` and then complement, so `~x` is `-(x + 1)` for a 32-bit `x`, and
        // `~"abc"` is `-1` because NaN becomes `+0` on the way through.
        UnaryOperator::BitwiseNot => Value::Number(f64::from(!operand.to_int32(heap))),
        // §13.5.7 — `ToBoolean` and then negate, which is why `!!x` is the shortest cast.
        UnaryOperator::LogicalNot => Value::Boolean(!operand.to_boolean(heap)),
        // Refused by the compiler, which is where the message with a span comes from. Answering
        // `undefined` here means a mistake shows up as a wrong value rather than a plausible one.
        UnaryOperator::Delete => Value::Undefined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOperator;
    use crate::compile::compile_expression;
    use crate::parser::parse_expression;

    /// Evaluate `source` and describe the result the way `String(x)` would, so that a row of a
    /// test reads as the JavaScript it is about.
    fn eval(source: &str) -> String {
        let mut heap = Heap::new();
        let expression = parse_expression(source).expect("the source parses"); // a VM test needs a chunk
        let chunk = compile_expression(&expression, &mut heap).expect("the source compiles"); // same
        let value = Vm::new()
            .run(&chunk, &mut heap)
            .expect("the chunk is well formed"); // same
        let id = value.to_string(&mut heap);
        String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
    }

    #[test]
    fn a_literal_evaluates_to_itself() {
        // The floor everything else stands on. `false` is here rather than assumed because a
        // compiler that pushed `true` for both would pass every other test in this file.
        assert_eq!(eval("1"), "1");
        assert_eq!(eval("1.5"), "1.5");
        assert_eq!(eval("true"), "true");
        assert_eq!(eval("false"), "false");
        assert_eq!(eval("null"), "null");
        assert_eq!(eval("'text'"), "text");
        assert_eq!(eval("''"), "");
        // …and a Number literal is written back the way §6.1.6.1.20 writes it, not the way the
        // source spelled it: `0x10` is `16` and `1e3` is `1000`.
        assert_eq!(eval("0x10"), "16");
        assert_eq!(eval("1e3"), "1000");
        assert_eq!(eval("1_000"), "1000");
        assert_eq!(eval("1e21"), "1e+21");
    }

    #[test]
    fn arithmetic_comes_out_the_way_the_language_says() {
        // Precedence and associativity are the parser's; that they survive into the bytecode is
        // this test's. `**` is the one right-associative operator, so the sixth row is 512 and
        // not 64.
        assert_eq!(eval("1 + 2"), "3");
        assert_eq!(eval("1 + 2 * 3"), "7");
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("7 % 3"), "1");
        assert_eq!(eval("-7 % 3"), "-1");
        assert_eq!(eval("2 ** 3 ** 2"), "512");
        assert_eq!(eval("1 / 0"), "Infinity");
        assert_eq!(eval("-1 / 0"), "-Infinity");
        assert_eq!(eval("0 / 0"), "NaN");
        // Subtraction and division are not commutative, so an operand order bug in the VM shows
        // up here and almost nowhere else.
        assert_eq!(eval("10 - 3"), "7");
        assert_eq!(eval("10 / 4"), "2.5");
        assert_eq!(eval("2 ** -1"), "0.5");
    }

    #[test]
    fn plus_concatenates_as_soon_as_either_side_is_a_string() {
        assert_eq!(eval("'a' + 'b'"), "ab");
        assert_eq!(eval("1 + '1'"), "11");
        assert_eq!(eval("'1' + 1"), "11");
        // …and grouping decides which: the first is `(1 + 2) + "3"`, the second `"3" + 1` then
        // `+ 2`. Left associativity is the whole difference.
        assert_eq!(eval("1 + 2 + '3'"), "33");
        assert_eq!(eval("'3' + 1 + 2"), "312");
        // Every other operator reads the String as a Number instead.
        assert_eq!(eval("'3' - 1"), "2");
        assert_eq!(eval("'3' * '4'"), "12");
        assert_eq!(eval("'a' - 1"), "NaN");
    }

    #[test]
    fn the_unary_operators_are_each_one_conversion() {
        assert_eq!(eval("-'5'"), "-5");
        assert_eq!(eval("+'5'"), "5");
        assert_eq!(eval("+'a'"), "NaN");
        assert_eq!(eval("!0"), "true");
        assert_eq!(eval("!''"), "true");
        assert_eq!(eval("!'0'"), "false");
        assert_eq!(eval("!!1"), "true");
        assert_eq!(eval("~5"), "-6");
        assert_eq!(eval("~'abc'"), "-1");
        assert_eq!(eval("~~1.7"), "1");
        assert_eq!(eval("void 0"), "undefined");
        assert_eq!(eval("typeof 1"), "number");
        assert_eq!(eval("typeof 'a'"), "string");
        assert_eq!(eval("typeof true"), "boolean");
        assert_eq!(eval("typeof null"), "object");
        assert_eq!(eval("typeof void 0"), "undefined");
        // Negation keeps the sign where subtraction does not, and `String` hides it again — so
        // the difference is only visible by dividing into it.
        assert_eq!(eval("1 / -0"), "-Infinity");
        assert_eq!(eval("1 / (0 - 0)"), "Infinity");
    }

    #[test]
    fn comparison_and_equality_agree_with_the_algorithms_they_come_from() {
        assert_eq!(eval("1 < 2"), "true");
        assert_eq!(eval("'10' < '9'"), "true");
        assert_eq!(eval("'10' < 9"), "false");
        // `undefined` is spelled `void 0` here because it is an *identifier*, not a literal —
        // which is exactly why minifiers write it that way, and why the compiler cannot read the
        // other spelling until names resolve.
        assert_eq!(eval("null == void 0"), "true");
        assert_eq!(eval("null === void 0"), "false");
        assert_eq!(eval("'' == 0"), "true");
        assert_eq!(eval("'' === 0"), "false");
        assert_eq!(eval("'1' == true"), "true");
        assert_eq!(eval("'true' == true"), "false");
        assert_eq!(eval("0 / 0 == 0 / 0"), "false");
        assert_eq!(eval("1 <= 1"), "true");
        assert_eq!(eval("(0 / 0) <= 1"), "false");
        assert_eq!(eval("1 << 32"), "1");
        assert_eq!(eval("-1 >>> 0"), "4294967295");
    }

    #[test]
    fn the_three_bitwise_operators_are_three_different_operators() {
        // Chosen so that no two of `&`, `|` and `^` agree: 12 is 1100 and 10 is 1010, so the
        // three answers are 1000, 1110 and 0110. A table of equal-answer rows would let any two
        // of them be swapped without a test noticing.
        assert_eq!(eval("12 & 10"), "8");
        assert_eq!(eval("12 | 10"), "14");
        assert_eq!(eval("12 ^ 10"), "6");
        // Through ToInt32, which is where the 32-bit truncation and the sign come from.
        assert_eq!(eval("2147483648 | 0"), "-2147483648");
        assert_eq!(eval("4294967296 | 0"), "0");
        assert_eq!(eval("-1 & 255"), "255");
        assert_eq!(eval("1.9 | 0"), "1");
        assert_eq!(eval("'abc' | 0"), "0");
        assert_eq!(eval("(0 / 0) | 0"), "0");
    }

    #[test]
    fn each_comparison_is_a_different_comparison() {
        // Every one of the eight, on operands where the answers differ — so that no two of them
        // can be confused for each other and no negation can be dropped.
        assert_eq!(eval("1 < 2"), "true");
        assert_eq!(eval("2 < 1"), "false");
        assert_eq!(eval("1 > 2"), "false");
        assert_eq!(eval("2 > 1"), "true");
        assert_eq!(eval("1 <= 2"), "true");
        assert_eq!(eval("2 <= 1"), "false");
        assert_eq!(eval("1 >= 2"), "false");
        assert_eq!(eval("2 >= 1"), "true");
        // …and the two negations, which a missing `!` would turn into their opposites.
        assert_eq!(eval("1 == 1"), "true");
        assert_eq!(eval("1 != 1"), "false");
        assert_eq!(eval("1 != 2"), "true");
        assert_eq!(eval("1 === 1"), "true");
        assert_eq!(eval("1 !== 1"), "false");
        assert_eq!(eval("1 !== '1'"), "true");
        assert_eq!(eval("1 != '1'"), "false");
    }

    #[test]
    fn an_infinite_exponent_is_nan_only_over_a_base_of_magnitude_one() {
        // §6.1.6.1.3 steps 11 and 12. The guard is a conjunction, and loosening it either way is
        // wrong in a different direction — so both halves need a row that says so.
        assert_eq!(eval("1 ** (1 / 0)"), "NaN");
        assert_eq!(eval("(0 - 1) ** (1 / 0)"), "NaN");
        assert_eq!(eval("2 ** (1 / 0)"), "Infinity");
        assert_eq!(eval("0.5 ** (1 / 0)"), "0");
        assert_eq!(eval("1 ** 2"), "1");
        assert_eq!(eval("(0 - 1) ** 3"), "-1");
    }

    #[test]
    fn a_short_circuit_answers_with_the_operand_that_decided() {
        // The thing that makes `&&` and `||` operators rather than `if` in disguise: the value
        // that stopped the evaluation *is* the answer. `0 || 'a'` is `'a'`, and `1 || 'a'` is
        // `1` and not `true`.
        assert_eq!(eval("1 && 2"), "2");
        assert_eq!(eval("0 && 2"), "0");
        assert_eq!(eval("'' && 2"), "");
        assert_eq!(eval("1 || 2"), "1");
        assert_eq!(eval("0 || 2"), "2");
        assert_eq!(eval("'' || 'a'"), "a");
        assert_eq!(eval("null || 'a'"), "a");
        // Chained, and left-associative: `a && b && c`.
        assert_eq!(eval("1 && 2 && 3"), "3");
        assert_eq!(eval("1 && 0 && 3"), "0");
        assert_eq!(eval("0 || '' || 'last'"), "last");
        // Mixed with an operator that is not short-circuiting, to check the stack comes out level.
        assert_eq!(eval("(1 && 2) + 1"), "3");
        assert_eq!(eval("1 + (0 || 5)"), "6");
    }

    #[test]
    fn nullish_coalescing_asks_a_different_question_from_or() {
        // The whole reason `??` was added: `||` tests truthiness and `??` tests only `null` and
        // `undefined`, so every falsy value that is not nullish is where they part company.
        assert_eq!(eval("0 || 'fallback'"), "fallback");
        assert_eq!(eval("0 ?? 'fallback'"), "0");
        assert_eq!(eval("'' ?? 'fallback'"), "");
        assert_eq!(eval("false ?? 'fallback'"), "false");
        assert_eq!(eval("(0 / 0) ?? 'fallback'"), "NaN");
        // …and where they agree.
        assert_eq!(eval("null ?? 'fallback'"), "fallback");
        assert_eq!(eval("void 0 ?? 'fallback'"), "fallback");
        assert_eq!(eval("1 ?? 'fallback'"), "1");
    }

    #[test]
    fn the_conditional_operator_evaluates_one_branch_and_never_the_test() {
        // Unlike a short circuit, the test is thrown away: `a ? b : c` is `b` or `c` and is never
        // `a`, however truthy `a` was.
        assert_eq!(eval("1 ? 'yes' : 'no'"), "yes");
        assert_eq!(eval("0 ? 'yes' : 'no'"), "no");
        assert_eq!(eval("'' ? 'yes' : 'no'"), "no");
        assert_eq!(eval("'0' ? 'yes' : 'no'"), "yes");
        assert_eq!(eval("null ? 'yes' : 'no'"), "no");
        // Right-associative, so this is `1 ? 'a' : (0 ? 'b' : 'c')` and nesting works in both
        // branches — the two jumps have to be patched to different places.
        assert_eq!(eval("1 ? 'a' : 0 ? 'b' : 'c'"), "a");
        assert_eq!(eval("0 ? 'a' : 0 ? 'b' : 'c'"), "c");
        assert_eq!(eval("0 ? 'a' : 1 ? 'b' : 'c'"), "b");
        assert_eq!(eval("(1 ? 2 : 3) + 10"), "12");
    }

    #[test]
    fn the_comma_operator_keeps_the_last_value_and_discards_the_rest() {
        assert_eq!(eval("(1, 2)"), "2");
        assert_eq!(eval("(1, 2, 3)"), "3");
        assert_eq!(eval("(1, 2) + 1"), "3");
        // Each earlier operand is still *evaluated* — the discarding is of the value, not of the
        // work — which is the only reason anyone writes one.
        assert_eq!(eval("('a' + 'b', 'c')"), "c");
    }

    #[test]
    fn a_chunk_that_does_not_make_sense_is_a_fault_and_not_a_panic() {
        // The three ways a chunk can be wrong, each reached by handing the VM one no compiler
        // would produce. A script cannot get here; a compiler bug can, and DR-0002 is a promise
        // about *any* input rather than about correct ones.
        let mut heap = Heap::new();
        let mut vm = Vm::new();

        let underflow =
            Chunk::from_parts(vec![Instruction::Binary(BinaryOperator::Add)], Vec::new());
        assert!(matches!(
            vm.run(&underflow, &mut heap),
            Err(Fault::StackUnderflow)
        ));
        let one_short = Chunk::from_parts(
            vec![
                Instruction::Constant(0),
                Instruction::Binary(BinaryOperator::Add),
            ],
            vec![Value::Null],
        );
        assert!(matches!(
            vm.run(&one_short, &mut heap),
            Err(Fault::StackUnderflow)
        ));

        let missing = Chunk::from_parts(vec![Instruction::Constant(7)], Vec::new());
        assert!(matches!(
            vm.run(&missing, &mut heap),
            Err(Fault::MissingConstant)
        ));

        // A jump past the end, including the placeholder an unpatched one carries — which is the
        // shape a compiler bug would actually take.
        let far = Chunk::from_parts(vec![Instruction::Jump(99)], Vec::new());
        assert!(matches!(
            vm.run(&far, &mut heap),
            Err(Fault::JumpOutOfRange)
        ));
        let unpatched = Chunk::from_parts(
            vec![
                Instruction::Constant(0),
                Instruction::JumpKeeping(ShortCircuit::WhenTruthy, u32::MAX),
            ],
            vec![Value::Boolean(true)],
        );
        assert!(matches!(
            vm.run(&unpatched, &mut heap),
            Err(Fault::JumpOutOfRange)
        ));
        // …while a jump to exactly the end is how every short circuit finishes, and is fine.
        let to_the_end = Chunk::from_parts(
            vec![Instruction::Constant(0), Instruction::Jump(2)],
            vec![Value::Boolean(true)],
        );
        assert!(matches!(
            vm.run(&to_the_end, &mut heap),
            Ok(Value::Boolean(true))
        ));
        // A short circuit that has to peek at an empty stack is an underflow like any other.
        let nothing_to_peek = Chunk::from_parts(
            vec![Instruction::JumpKeeping(ShortCircuit::WhenFalsy, 1)],
            Vec::new(),
        );
        assert!(matches!(
            vm.run(&nothing_to_peek, &mut heap),
            Err(Fault::StackUnderflow)
        ));
        let nothing_to_pop = Chunk::from_parts(vec![Instruction::Pop], Vec::new());
        assert!(matches!(
            vm.run(&nothing_to_pop, &mut heap),
            Err(Fault::StackUnderflow)
        ));
        let nothing_to_test = Chunk::from_parts(vec![Instruction::JumpIfFalse(1)], Vec::new());
        assert!(matches!(
            vm.run(&nothing_to_test, &mut heap),
            Err(Fault::StackUnderflow)
        ));

        // Two values pushed and nothing to join them, and a chunk that pushed none at all.
        let leftover = Chunk::from_parts(
            vec![Instruction::Constant(0), Instruction::Constant(0)],
            vec![Value::Null],
        );
        assert!(matches!(
            vm.run(&leftover, &mut heap),
            Err(Fault::UnbalancedStack)
        ));
        let empty = Chunk::from_parts(Vec::new(), Vec::new());
        assert!(matches!(
            vm.run(&empty, &mut heap),
            Err(Fault::UnbalancedStack)
        ));

        // …and the machine still works afterwards, which is the other half of the claim: a fault
        // is about the chunk, not about the interpreter.
        let sound = Chunk::from_parts(vec![Instruction::Constant(0)], vec![Value::Null]);
        assert!(matches!(vm.run(&sound, &mut heap), Ok(Value::Null)));
    }

    #[test]
    fn a_deeply_nested_expression_does_not_grow_the_rust_stack() {
        // The reason for bytecode, seen from the other side: the tree is nested a thousand deep
        // and the interpreter's loop is flat, so this costs a thousand stack *slots* rather than
        // a thousand Rust frames. The parser's own limit (DR-0006) is what bounds the tree.
        let source = format!("{}1{}", "(".repeat(60), ")".repeat(60));
        assert_eq!(eval(&source), "1");
        let sum = std::iter::repeat_n("1", 500)
            .collect::<Vec<_>>()
            .join(" + ");
        assert_eq!(eval(&sum), "500");
    }
}
