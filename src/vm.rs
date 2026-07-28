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
//! # A throw is an answer, not a failure
//!
//! §6.2.4's Completion Records have five types, and a bytecode compiler turns four of them into
//! jumps: `break`, `continue` and `return` are known at compile time and become instructions.
//! Only **throw** has to travel at run time, because where it lands depends on what the stack
//! looks like when it happens. So an [`Outcome`] is a value or a thrown value, and the rest of
//! §6.2.4 lives in [`crate::compile`].

use crate::ast::UnaryOperator;
use crate::compile::{Chunk, Instruction, ShortCircuit};
use crate::heap::Heap;
use crate::realm::{NativeError, Realm};
use crate::value::{Completion, TypeError, Value, apply_binary};

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
    /// A `LoadLocal` or `StoreLocal` named a slot the frame does not have.
    MissingLocal,
    /// A `PopHandler` with no matching `PushHandler`.
    UnmatchedPopHandler,
    /// The chunk finished with something still on the stack.
    ///
    /// Every statement is stack-neutral and every expression consumes its operands, so a chunk
    /// that has run to the end has nothing left over. Anything else is a compiler bug that would
    /// otherwise show up much later as the wrong value.
    UnbalancedStack,
}

/// What running a chunk came to.
///
/// Two of §6.2.4's completion types, and the two that a *script* can end with. `break` and
/// `continue` never escape the code that names them, and `return` needs a function to return
/// from.
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    /// The script finished; this is its completion value.
    Value(Value),
    /// The script threw and nothing caught it.
    ///
    /// The value is whatever was thrown, which need not be an Error: `throw 1` is legal and the
    /// specification never asks what it was given.
    Thrown(Value),
}

/// Where a throw goes, and what the stack should look like when it gets there.
#[derive(Debug, Clone, Copy)]
struct Handler {
    /// The instruction to continue at.
    target: u32,
    /// How deep the operand stack was when the handler was installed.
    ///
    /// A throw in the middle of an expression leaves its half-built operands behind. Without
    /// this, a caught exception would leave rubbish under everything the handler pushed
    /// afterwards, and the imbalance would surface somewhere else entirely.
    depth: usize,
}

/// The interpreter.
///
/// Holds the operand stack and nothing else so far. Call frames, the environment and the job
/// queue join it as the things that need them arrive.
#[derive(Debug)]
pub struct Vm {
    stack: Vec<Value>,
    /// The intrinsics a thrown error is built from.
    realm: Realm,
    /// A throw that nothing caught, kept until the loop can stop.
    ///
    /// The loop cannot return from inside the `match` — an operation that throws has to leave the
    /// program counter somewhere legal so that `while let` ends — so the value waits here.
    escaped: Option<Value>,
    /// The handlers a throw would look at, innermost last.
    handlers: Vec<Handler>,
    /// One slot per local variable, all starting as `undefined`.
    ///
    /// Resolved to indices at compile time, so nothing here searches for a name. That is what
    /// makes hoisting free: the slot exists before the first instruction runs, which is exactly
    /// what a `var` read before its declaration is asking for.
    locals: Vec<Value>,
    /// The script's completion value so far — §14.2.2's `UpdateEmpty`, as a register.
    completion: Value,
}

impl Vm {
    /// A machine with an empty stack, belonging to a realm built into `heap`.
    ///
    /// Takes the heap because a machine cannot run without intrinsics: the first TypeError it
    /// throws needs a prototype to be an instance of.
    pub fn new(heap: &mut Heap) -> Self {
        Self {
            realm: Realm::new(heap),
            escaped: None,
            stack: Vec::new(),
            handlers: Vec::new(),
            locals: Vec::new(),
            completion: Value::Undefined,
        }
    }

    /// Run `chunk` to the end and answer the single value it leaves behind.
    ///
    /// The stack is cleared first, so a machine that faulted once is usable again: a fault says
    /// the chunk was wrong, not that the interpreter is now untrustworthy.
    pub fn run(&mut self, chunk: &Chunk, heap: &mut Heap) -> Result<Outcome, Fault> {
        self.stack.clear();
        self.handlers.clear();
        self.escaped = None;
        // §14.2.2 — a statement list whose statements all produce nothing has the value
        // `undefined`, which is what `eval("var x")` and `eval(";")` come to.
        self.completion = Value::Undefined;
        self.locals.clear();
        self.locals.resize(chunk.locals(), Value::Undefined);
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
                    let value = apply_unary(operator, operand, heap);
                    match self.settle(value, heap, &mut at, code.len())? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
                }
                Instruction::Binary(operator) => {
                    // Right first: it was pushed second, so it is on top. Getting this backwards
                    // would make every subtraction and comparison silently mirror itself.
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let value = apply_binary(operator, left, right, heap);
                    match self.settle(value, heap, &mut at, code.len())? {
                        Some(value) => self.stack.push(value),
                        None => continue,
                    }
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
                Instruction::JumpIfTrue(target) => {
                    if self.pop()?.to_boolean(heap) {
                        at = jump_to(target, code.len())?;
                    }
                }
                Instruction::Pop => {
                    self.pop()?;
                }
                Instruction::LoadLocal(slot) => {
                    let value = *self.locals.get(slot as usize).ok_or(Fault::MissingLocal)?;
                    self.stack.push(value);
                }
                Instruction::StoreLocal(slot) => {
                    // Peeked, not popped: assignment is an expression, and `a = (b = 1)` needs
                    // the inner one to leave its value behind.
                    let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
                    *self
                        .locals
                        .get_mut(slot as usize)
                        .ok_or(Fault::MissingLocal)? = value;
                }
                Instruction::SetCompletion => {
                    self.completion = self.pop()?;
                }
                Instruction::Throw => {
                    // §6.2.4 — a throw completion travels up until something wants it. Here that
                    // is the innermost handler; with nothing to want it, it leaves the script.
                    let thrown = self.pop()?;
                    self.unwind(thrown, &mut at, code.len())?;
                }
                Instruction::PushHandler(target) => self.handlers.push(Handler {
                    target,
                    depth: self.stack.len(),
                }),
                Instruction::PopHandler => {
                    // A pop with nothing to pop is a chunk that does not make sense: the compiler
                    // emits these in pairs.
                    self.handlers.pop().ok_or(Fault::UnmatchedPopHandler)?;
                }
            }
        }
        // Nothing should be left. Anything else means the chunk and the compiler disagree about
        // what the instructions do, and saying so here is cheaper than finding out later.
        if let Some(thrown) = self.escaped {
            return Ok(Outcome::Thrown(thrown));
        }
        if !self.stack.is_empty() {
            return Err(Fault::UnbalancedStack);
        }
        Ok(Outcome::Value(self.completion))
    }

    /// What to do with an operation that may have thrown.
    ///
    /// `Ok(Some(value))` is the ordinary answer. `Ok(None)` means a handler took the throw and
    /// `at` has been moved to it, so the caller should go round the loop again rather than push
    /// anything. `Err` is a chunk that does not make sense, which is a different thing entirely.
    ///
    /// One place rather than one per instruction, because every operation that converts a value
    /// can now throw and they should all unwind the same way.
    fn settle(
        &mut self,
        outcome: Completion<Value>,
        heap: &mut Heap,
        at: &mut usize,
        length: usize,
    ) -> Result<Option<Value>, Fault> {
        let TypeError(message) = match outcome {
            Ok(value) => return Ok(Some(value)),
            Err(error) => error,
        };
        // The value layer said which error; the realm decides what object stands for it. This is
        // the seam described in [`crate::realm`].
        let thrown = self.realm.error(heap, NativeError::Type, message);
        self.unwind(thrown, at, length)
    }

    /// Hand `thrown` to the innermost handler, or answer that nothing wanted it.
    fn unwind(
        &mut self,
        thrown: Value,
        at: &mut usize,
        length: usize,
    ) -> Result<Option<Value>, Fault> {
        let Some(handler) = self.handlers.pop() else {
            self.escaped = Some(thrown);
            *at = length;
            return Ok(None);
        };
        self.stack.truncate(handler.depth);
        self.stack.push(thrown);
        *at = jump_to(handler.target, length)?;
        Ok(None)
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
fn apply_unary(operator: UnaryOperator, operand: Value, heap: &mut Heap) -> Completion<Value> {
    Ok(match operator {
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
        UnaryOperator::Plus => Value::Number(operand.to_number(heap)?),
        // §13.5.5 — `ToNumber` and then negate. Negation is not subtraction from zero: `-0` is
        // `-0` where `0 - 0` is `+0`.
        UnaryOperator::Minus => Value::Number(-operand.to_number(heap)?),
        // §13.5.6 — `ToInt32` and then complement, so `~x` is `-(x + 1)` for a 32-bit `x`, and
        // `~"abc"` is `-1` because NaN becomes `+0` on the way through.
        UnaryOperator::BitwiseNot => Value::Number(f64::from(!operand.to_int32(heap)?)),
        // §13.5.7 — `ToBoolean` and then negate, which is why `!!x` is the shortest cast.
        UnaryOperator::LogicalNot => Value::Boolean(!operand.to_boolean(heap)),
        // Refused by the compiler, which is where the message with a span comes from. Answering
        // `undefined` here means a mistake shows up as a wrong value rather than a plausible one.
        UnaryOperator::Delete => Value::Undefined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOperator;
    use crate::compile::{compile_expression, compile_script};
    use crate::parser::{parse_expression, parse_script};

    /// Evaluate `source` and describe the result the way `String(x)` would, so that a row of a
    /// test reads as the JavaScript it is about.
    fn eval(source: &str) -> String {
        let mut heap = Heap::new();
        let expression = parse_expression(source).expect("the source parses"); // a VM test needs a chunk
        let chunk = compile_expression(&expression, &mut heap).expect("the source compiles"); // same
        let outcome = Vm::new(&mut heap)
            .run(&chunk, &mut heap)
            .expect("the chunk is well formed"); // same
        describe(outcome, &mut heap)
    }

    /// Run a whole script and describe its completion value the way `String(x)` would.
    fn run(source: &str) -> String {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
        let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
        let outcome = Vm::new(&mut heap)
            .run(&chunk, &mut heap)
            .expect("the chunk is well formed"); // same
        describe(outcome, &mut heap)
    }

    /// The outcome, written the way `String(x)` would write it — with a thrown one marked, so
    /// that a test row saying `"thrown 1"` cannot be confused with one saying `"1"`.
    fn describe(outcome: Outcome, heap: &mut Heap) -> String {
        let (prefix, value) = match outcome {
            Outcome::Value(value) => ("", value),
            Outcome::Thrown(value) => ("thrown ", value),
        };
        // A thrown *object* has no `toString` to call yet, so writing it down would throw again.
        // Naming it by its type is enough for a test row to say which error it was, and it stops
        // one describing failure from failing.
        let Ok(id) = value.to_string(heap) else {
            return format!("{prefix}[{}]", value.type_of());
        };
        format!(
            "{prefix}{}",
            String::from_utf16_lossy(heap.string(id).unwrap_or(&[]))
        )
    }

    #[test]
    fn a_script_evaluates_to_its_last_value_producing_statement() {
        // §14.2.2's `UpdateEmpty`. A declaration produces nothing, so it does not replace what
        // came before — which is why the third row is 1 and not `undefined`.
        assert_eq!(run("1;"), "1");
        assert_eq!(run("1; 2;"), "2");
        assert_eq!(run("1; var x = 2;"), "1");
        assert_eq!(run("var x = 2;"), "undefined");
        assert_eq!(run(""), "undefined");
        assert_eq!(run(";;;"), "undefined");
        assert_eq!(run("1; ;"), "1");
        assert_eq!(run("{ 1; }"), "1");
        assert_eq!(run("{ } 1; { }"), "1");
    }

    #[test]
    fn a_var_is_hoisted_so_it_exists_before_its_declaration_and_holds_nothing() {
        // The whole of what hoisting is: the binding is made before the first statement runs and
        // the initializer is not. `x` is readable and `undefined` on the first line.
        assert_eq!(run("var seen = typeof x; var x = 1; seen;"), "undefined");
        assert_eq!(run("var before = x; var x = 1; before;"), "undefined");
        assert_eq!(run("var x = 1; x;"), "1");
        // …including from inside a block or a loop, because `var` belongs to the script rather
        // than to where it was written. That is the difference `let` was introduced to fix.
        assert_eq!(run("{ var inner = 5; } inner;"), "5");
        assert_eq!(
            run("var i = 0; while (i < 1) { var loop_var = 9; i = i + 1; } loop_var;"),
            "9"
        );
        // A second `var` with no initializer does not wipe the first one's value.
        assert_eq!(run("var x = 1; var x; x;"), "1");
        assert_eq!(run("var x = 1; var x = 2; x;"), "2");
    }

    #[test]
    fn assignment_is_an_expression_whose_value_is_what_was_assigned() {
        assert_eq!(run("var a; a = 5;"), "5");
        assert_eq!(run("var a; var b; a = b = 3; a;"), "3");
        assert_eq!(run("var a = 1; a += 2; a;"), "3");
        assert_eq!(run("var a = 1; (a += 2);"), "3");
        assert_eq!(run("var a = 'x'; a += 1; a;"), "x1");
        assert_eq!(run("var a = 8; a /= 2; a;"), "4");
        assert_eq!(run("var a = 5; a **= 2; a;"), "25");
        assert_eq!(run("var a = 12; a &= 10; a;"), "8");
        assert_eq!(run("var a = 1; a <<= 3; a;"), "8");
    }

    #[test]
    fn an_if_runs_one_branch_and_a_missing_else_runs_none() {
        assert_eq!(run("var r = 'none'; if (1) r = 'then'; r;"), "then");
        assert_eq!(run("var r = 'none'; if (0) r = 'then'; r;"), "none");
        assert_eq!(run("var r; if (0) r = 'then'; else r = 'else'; r;"), "else");
        assert_eq!(run("var r; if (1) r = 'then'; else r = 'else'; r;"), "then");
        // Truthiness rather than equality with `true`, and nesting.
        assert_eq!(run("var r = 0; if ('0') r = 1; r;"), "1");
        assert_eq!(run("var r = 0; if ('') r = 1; r;"), "0");
        assert_eq!(run("var r; if (1) if (0) r = 'a'; else r = 'b'; r;"), "b");
    }

    #[test]
    fn the_three_loops_agree_about_when_they_test() {
        // `while` tests first, `do` tests last — so a false condition runs the body once in one
        // of them and never in the other.
        assert_eq!(run("var n = 0; while (0) n = n + 1; n;"), "0");
        assert_eq!(run("var n = 0; do n = n + 1; while (0) n;"), "1");
        assert_eq!(run("var n = 0; while (n < 5) n = n + 1; n;"), "5");
        assert_eq!(run("var n = 0; do n = n + 1; while (n < 5) n;"), "5");
        assert_eq!(
            run("var n = 0; for (var i = 0; i < 5; i = i + 1) n = n + i; n;"),
            "10"
        );
        // A `for` with parts missing: no init, no update, and no test at all.
        assert_eq!(run("var i = 0; for (; i < 3; ) i = i + 1; i;"), "3");
        assert_eq!(
            run("var i = 0; for (;;) { i = i + 1; if (i > 3) break; } i;"),
            "4"
        );
    }

    #[test]
    fn break_leaves_the_loop_and_continue_goes_round_again() {
        assert_eq!(
            run("var n = 0; while (1) { n = n + 1; if (n > 2) break; } n;"),
            "3"
        );
        assert_eq!(
            run(
                "var n = 0; var i = 0; while (i < 5) { i = i + 1; if (i < 3) continue; n = n + 1; } n;"
            ),
            "3"
        );
        // In a `for` loop, `continue` still runs the update — which is the whole reason the third
        // part exists, and the thing a `while` translation gets wrong.
        assert_eq!(
            run(
                "var n = 0; for (var i = 0; i < 5; i = i + 1) { if (i < 3) continue; n = n + 1; } n;"
            ),
            "2"
        );
        assert_eq!(
            run("var i = 0; for (i = 0; i < 5; i = i + 1) { continue; } i;"),
            "5"
        );
        // In a `do` loop, `continue` goes to the *test*, so a loop whose test then fails stops.
        assert_eq!(
            run("var n = 0; do { n = n + 1; continue; } while (n < 3) n;"),
            "3"
        );
        // The innermost loop is the one that is left, and the outer one carries on.
        assert_eq!(
            run(
                "var n = 0; var i = 0; while (i < 3) { i = i + 1; var j = 0; while (1) { j = j + 1; if (j > 1) break; n = n + 1; } } n;"
            ),
            "3"
        );
    }

    #[test]
    fn a_loop_that_never_runs_leaves_the_stack_and_the_completion_value_alone() {
        // The stack-neutrality every statement promises, checked where it is easiest to break: a
        // loop whose body pushes and pops, taken zero times and many times.
        assert_eq!(run("7; while (0) { 1; 2; 3; }"), "7");
        // …and a body that *does* run replaces the completion value, once per iteration.
        assert_eq!(
            run("7; var i = 0; while (i < 3) { i = i + 1; i * 10; }"),
            "30"
        );
        assert_eq!(run("7; for (var i = 0; i < 2; i = i + 1) i;"), "1");
    }

    #[test]
    fn a_script_that_cannot_be_compiled_yet_says_which_construct_and_where() {
        let cases = [
            ("let x = 1;", "let and const"),
            ("const x = 1;", "let and const"),
            ("function f() {}", "a function declaration"),
            ("try { } catch ([a]) { }", "a destructuring catch parameter"),
            ("switch (1) { }", "switch"),
            ("for (var k in 1) ;", "for-in and for-of"),
            ("var [a] = 1;", "a destructuring binding"),
            ("outer: while (1) break outer;", "a labelled statement"),
            ("x;", "a reference to an undeclared name"),
            ("var a; a.b = 1;", "an assignment to a property"),
            ("undeclared = 1;", "an assignment to an undeclared name"),
            ("var a; a ||= 1;", "a logical assignment"),
        ];
        for (source, what) in cases {
            let mut heap = Heap::new();
            let script = parse_script(source).expect("the source parses"); // the test is about compiling
            let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
            assert_eq!(
                error.kind,
                crate::compile::ErrorKind::Unsupported(what),
                "compiling {source:?}"
            );
        }
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
    fn a_throw_that_nothing_catches_leaves_the_script() {
        // §14.14 — anything at all may be thrown, and nothing asks what it is. An Error object
        // would be the usual thing; there are no objects yet and the language never required one.
        assert_eq!(run("throw 1;"), "thrown 1");
        assert_eq!(run("throw 'a' + 'b';"), "thrown ab");
        assert_eq!(run("throw void 0;"), "thrown undefined");
        // Everything after the throw is skipped, including the statement that would have set the
        // completion value.
        assert_eq!(run("1; throw 2; 3;"), "thrown 2");
        assert_eq!(
            run("var n = 0; while (1) { n = n + 1; if (n > 2) throw n; } n;"),
            "thrown 3"
        );
    }

    #[test]
    fn a_catch_block_receives_the_value_and_the_script_carries_on() {
        assert_eq!(run("try { throw 1; } catch (e) { e; }"), "1");
        assert_eq!(
            run("try { throw 'x'; } catch (e) { 'caught ' + e; }"),
            "caught x"
        );
        // The try block's own value survives when nothing is thrown, and the catch block is not
        // entered at all.
        assert_eq!(run("try { 7; } catch (e) { 8; }"), "7");
        // ES2019's optional binding: the value is simply discarded.
        assert_eq!(run("try { throw 1; } catch { 'caught'; }"), "caught");
        // A throw inside a loop inside a try still finds the handler.
        assert_eq!(
            run(
                "try { var i = 0; while (1) { i = i + 1; if (i > 2) throw i; } } catch (e) { 'caught ' + e; }"
            ),
            "caught 3"
        );
    }

    #[test]
    fn a_throw_in_the_middle_of_an_expression_leaves_no_rubbish_behind() {
        // The handler puts the operand stack back to the depth the protected region began at, so
        // the half-built operands of the interrupted expression are discarded rather than left
        // under everything that follows. No source can reach this yet — nothing throws from
        // inside an expression until an operation can — so the chunk is written by hand, the way
        // a malformed one is.
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);
        let chunk = Chunk::from_parts(
            vec![
                // try {
                Instruction::PushHandler(6),
                // …two operands of an expression that never finishes…
                Instruction::Constant(0),
                Instruction::Constant(0),
                // …and a throw from the middle of it.
                Instruction::Constant(1),
                Instruction::Throw,
                Instruction::PopHandler,
                // catch: the thrown value is here and the two operands are not.
                Instruction::SetCompletion,
            ],
            vec![Value::Number(9.0), Value::Number(1.0)],
        );
        // A leftover operand would be an unbalanced stack rather than a wrong answer, which is
        // exactly what makes the balance check worth having.
        let outcome = vm.run(&chunk, &mut heap).expect("well formed"); // the test is about the outcome
        assert_eq!(describe(outcome, &mut heap), "1");
    }

    #[test]
    fn a_nested_try_is_caught_by_the_innermost_handler_that_is_still_open() {
        assert_eq!(
            run("try { try { throw 1; } catch (e) { 'inner ' + e; } } catch (e) { 'outer'; }"),
            "inner 1"
        );
        // A throw from a *catch* block is not caught by its own try.
        assert_eq!(
            run("try { try { throw 1; } catch (e) { throw 2; } } catch (e) { 'outer ' + e; }"),
            "outer 2"
        );
        // …and one that nothing catches still leaves the script.
        assert_eq!(
            run("try { throw 1; } catch (e) { throw e + 1; }"),
            "thrown 2"
        );
    }

    #[test]
    fn a_finally_block_runs_on_both_ways_out() {
        // The normal way…
        assert_eq!(
            run("var log = ''; try { log = log + 'a'; } finally { log = log + 'b'; } log;"),
            "ab"
        );
        // …and the way that carries a thrown value, which then carries on outwards.
        assert_eq!(
            run("var log = ''; try { throw 1; } finally { log = log + 'f'; }"),
            "thrown 1"
        );
        assert_eq!(
            run(
                "var log = ''; try { try { throw 1; } finally { log = log + 'f'; } } catch (e) { log + e; }"
            ),
            "f1"
        );
        // All three tails together, and a throw from the *catch* block still runs the finally.
        assert_eq!(
            run(
                "var log = ''; try { try { throw 1; } catch (e) { log = log + 'c'; throw 2; } finally { log = log + 'f'; } } catch (e) { log + e; }"
            ),
            "cf2"
        );
        // …and when nothing throws at all, the catch is skipped and the finally is not.
        assert_eq!(
            run(
                "var log = ''; try { log = log + 't'; } catch (e) { log = log + 'c'; } finally { log = log + 'f'; } log;"
            ),
            "tf"
        );
    }

    #[test]
    fn a_catch_parameter_shadows_an_outer_name_only_inside_its_block() {
        // §14.15.3 — the parameter is a binding of its own. Inside the block it is the thrown
        // value; outside it, the outer binding is untouched.
        assert_eq!(
            run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; }"),
            "inner"
        );
        assert_eq!(
            run("var e = 'outer'; try { throw 'inner'; } catch (e) { e; } e;"),
            "outer"
        );
        // Assigning to it inside the block does not reach the outer one either.
        assert_eq!(
            run("var e = 'outer'; try { throw 1; } catch (e) { e = 'changed'; } e;"),
            "outer"
        );
    }

    #[test]
    fn leaving_a_try_that_has_a_finally_is_refused_rather_than_skipping_it() {
        // A `break` past a `finally` is a third way out, and the finally would have to run on the
        // way. Refusing is narrow: a loop written *inside* the try is unaffected, which is the
        // second row.
        let mut heap = Heap::new();
        let script = parse_script("while (1) { try { break; } finally { } }").expect("parses"); // the test is about compiling
        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
        );
        // A loop inside the `try` may still be left, because that jump crosses no finally.
        assert_eq!(
            run("var n = 0; try { while (1) { n = n + 1; break; } } finally { n = n + 10; } n;"),
            "11"
        );
        // …and a `break` inside a `try` that has only a `catch` is fine too.
        assert_eq!(
            run("var n = 0; while (1) { try { break; } catch (e) { } } n;"),
            "0"
        );

        // The guard belongs to the `try` that raised it and is put down when that `try` ends, so
        // a `break` *after* one is crossing nothing.
        assert_eq!(
            run("var n = 0; while (1) { try { } finally { } n = 1; break; } n;"),
            "1"
        );
        // …and an inner `try` with no finally does not put down the outer one's guard, so a
        // `break` inside it is still refused.
        let source = "while (1) { try { try { } catch (e) { } break; } finally { } }";
        let script = parse_script(source).expect("parses"); // the test is about compiling
        let error = compile_script(&script, &mut heap).expect_err("still crosses a finally"); // same
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported("break or continue out of a try with a finally")
        );
    }

    #[test]
    fn a_chunk_that_does_not_make_sense_is_a_fault_and_not_a_panic() {
        // The three ways a chunk can be wrong, each reached by handing the VM one no compiler
        // would produce. A script cannot get here; a compiler bug can, and DR-0002 is a promise
        // about *any* input rather than about correct ones.
        let mut heap = Heap::new();
        let mut vm = Vm::new(&mut heap);

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
        let _ = &unpatched;
        assert!(matches!(
            vm.run(&unpatched, &mut heap),
            Err(Fault::JumpOutOfRange)
        ));
        // …while a jump to exactly the end is how every short circuit finishes, and is fine.
        let to_the_end = Chunk::from_parts(
            vec![
                Instruction::Constant(0),
                Instruction::SetCompletion,
                Instruction::Jump(3),
            ],
            vec![Value::Boolean(true)],
        );
        assert!(matches!(
            vm.run(&to_the_end, &mut heap),
            Ok(Outcome::Value(Value::Boolean(true)))
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
        // An *empty* chunk is not a fault — it is an empty script, whose completion value is
        // `undefined`.
        let empty = Chunk::from_parts(Vec::new(), Vec::new());
        assert!(matches!(
            vm.run(&empty, &mut heap),
            Ok(Outcome::Value(Value::Undefined))
        ));

        // A slot the frame does not have, in both directions.
        let no_such_slot = Chunk::from_parts(vec![Instruction::LoadLocal(3)], Vec::new());
        assert!(matches!(
            vm.run(&no_such_slot, &mut heap),
            Err(Fault::MissingLocal)
        ));
        let nowhere_to_store = Chunk::from_parts(
            vec![Instruction::Constant(0), Instruction::StoreLocal(3)],
            vec![Value::Null],
        );
        assert!(matches!(
            vm.run(&nowhere_to_store, &mut heap),
            Err(Fault::MissingLocal)
        ));
        let nothing_to_store = Chunk::from_parts(vec![Instruction::StoreLocal(0)], Vec::new());
        assert!(matches!(
            vm.run(&nothing_to_store, &mut heap),
            Err(Fault::StackUnderflow)
        ));
        let nothing_to_complete = Chunk::from_parts(vec![Instruction::SetCompletion], Vec::new());
        assert!(matches!(
            vm.run(&nothing_to_complete, &mut heap),
            Err(Fault::StackUnderflow)
        ));

        // …and the machine still works afterwards, which is the other half of the claim: a fault
        // is about the chunk, not about the interpreter.
        let sound = Chunk::from_parts(
            vec![Instruction::Constant(0), Instruction::SetCompletion],
            vec![Value::Null],
        );
        assert!(matches!(
            vm.run(&sound, &mut heap),
            Ok(Outcome::Value(Value::Null))
        ));
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
