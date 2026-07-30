//! What the compiler refuses, and where it says so.
//!
//! Two kinds of test. Most run source and check the refusal it earns — the list of what praxis
//! cannot do yet, kept honest by being asserted rather than written in a comment. The rest build
//! a syntax tree *by hand*, because the parser will not produce one: a private name outside a
//! class, an optional chain as a bare member, an expression nested past the limit. A guard for a
//! state the tree can hold and no source can reach is still worth having, and this is the only
//! way to reach it.

use super::*;
use crate::ast::{BinaryOperator, UnaryOperator};
use crate::ast::{ExprKind, Stmt, StmtKind};
use crate::parser::parse_expression;
use crate::parser::parse_script;
use crate::value::Value;
use std::rc::Rc;

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
        ("1n", "a BigInt literal"),
        ("1 ? 2n : 3", "a BigInt literal"),
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
fn compiling_at_the_cap_fits_in_the_stack_it_claims_to_need() {
    // What makes [`MAX_EXPRESSION_DEPTH`] a measurement rather than a hope, and the twin of the
    // parser's `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need`. A cap the stack cannot
    // afford is worse than no cap: the compile dies by overflow — which DR-0002 says no `Result`
    // can rescue and which takes the embedder's process with it — one level before the check that
    // was supposed to prevent exactly that.
    //
    // One mebibyte is the smallest thread stack in common use, and this is a debug build, whose
    // frames are largest. If a slice adds frames between one level of expression and the next,
    // this is where it says so — which is what did not happen when the cap was 128, and CI on
    // another platform found it by aborting instead.
    let deep = MAX_EXPRESSION_DEPTH;
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            let mut heap = Heap::new();
            // At the cap, which must compile, and one past it, which must be refused rather than
            // overflow — both of them descend the whole way down.
            let deepest = compile_expression(&nested(deep - 1), &mut heap).is_ok();
            let refused = compile_expression(&nested(deep), &mut heap).is_err();
            deepest && refused
        })
        .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
    assert!(
        worker.join().unwrap_or(false), // a panic in the thread is the failure being reported
        "compiling at the cap needs more than the mebibyte it claims"
    );
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
            // Refused where the *compiler* can be reached with it: the parser makes `#x` outside a
            // class an early error, so this shape only exists in a tree built by hand. A refusal and
            // not a fault, which is what it was for one commit — `load_name` fell back to a global of
            // the same name and handed `undefined` to `GetPrivate` as a Private Name.
            "a private name outside a class body",
        ),
        (
            ExprKind::Member {
                private: false,
                optional: true,
                object: object(),
                property: "x".into(),
            },
            // The wrapper is what the jump out of a chain lands on, so a link without one has
            // nowhere to go. Refused rather than emitted: an unpatched jump carries `u32::MAX` and
            // would leap off the end of the chunk at run time.
            "`?.` outside an optional chain",
        ),
        (
            ExprKind::ComputedMember {
                optional: true,
                object: object(),
                property: object(),
            },
            // The wrapper is what the jump out of a chain lands on, so a link without one has
            // nowhere to go. Refused rather than emitted: an unpatched jump carries `u32::MAX` and
            // would leap off the end of the chunk at run time.
            "`?.` outside an optional chain",
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
        .property_reference(&not_a_reference, crate::compile::function::Keep::Nothing)
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
        is_strict: false,
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
fn a_name_no_scope_declares_is_a_global_however_deeply_it_is_nested() {
    // Reading outwards stops at the script, and what is past the script is the global object.
    // So a name that is nowhere on the chain is not a mistake the compiler can name — it is a
    // question for run time, because the global it wants may be created a line later. The
    // enclosing function has a local of its own, so the walk has something to look at and still
    // reaches the same answer.
    let mut heap = Heap::new();
    for source in [
        "function outer() { var x; function inner() { return nowhere; } }",
        "function outer() { var x; function inner() { nowhere = 1; } }",
    ] {
        let script = parse_script(source).expect("the row parses"); // a row that does not is the bug
        assert!(
            compile_script(&script, &mut heap).is_ok(),
            "compiling {source:?}"
        );
    }
}

#[test]
fn an_optional_call_the_parser_cannot_build_is_still_refused() {
    // As with `o?.a`, the parser wraps `f?.()` in an `OptionalChain` and the inner flag never
    // arrives on its own. The tree can hold it, and the short circuit has nowhere to jump to
    // without the wrapper — so this is refused rather than left pointing at the placeholder.
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
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported("`?.` outside an optional chain")
    );
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
            is_strict: false,
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
    let error = compile("1 + 2 * (3 - 4n)").expect_err("a BigInt is not implemented yet"); // same
    assert_eq!(error.kind, ErrorKind::Unsupported("a BigInt literal"));
    assert_eq!(error.span, Span::new(13, 15));
}

/// The body of the first function written in `source`.
fn inner(source: &str) -> Rc<Chunk> {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a compiler test needs a tree
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
    Rc::clone(chunk.function(0).expect("the script declares a function")) // likewise
}

#[test]
fn a_function_is_given_an_arguments_object_only_if_it_reaches_for_one() {
    // §10.2.11 step 19 makes an arguments object for every non-arrow function, and a program can
    // only tell by *reading* the name. So this is asked of the chunk rather than of a running
    // program: whether the object is built is the compiler's answer, and the difference between
    // the two answers is an allocation per call and nothing observable at all.
    assert!(
        inner("function f() { return arguments[0]; }")
            .arguments()
            .is_some()
    );
    assert!(
        inner("function f(a) { return arguments; }")
            .arguments()
            .is_some()
    );
    assert!(inner("function f() { return 1; }").arguments().is_none());
    assert!(
        inner("function f(a, b) { return a + b; }")
            .arguments()
            .is_none()
    );
    // A name that merely looks like it: the compiler compares the whole string, not a prefix.
    assert!(
        inner("function f() { var argument = 1; return argument; }")
            .arguments()
            .is_none()
    );
    // §10.2.11 step 18 — a *parameter* of that name takes it, and then the object is not built
    // even though the name is read.
    assert!(
        inner("function f(arguments) { return arguments; }")
            .arguments()
            .is_none()
    );
    // A `var` of that name does not, which is step 19's least obvious half: it names the
    // parameters, the hoisted functions and the lexical declarations, and `var` is none of the
    // three. `function f() { var arguments; return typeof arguments; }` answers `"object"`.
    assert!(
        inner("function f() { var arguments = 1; return arguments; }")
            .arguments()
            .is_some()
    );
    // An arrow has none of its own, so the name it reads belongs to the function around it — and
    // that function is the one that has to build it.
    assert!(
        inner("function f(a) { var g = () => arguments[0]; return g(); }")
            .arguments()
            .is_some()
    );
    // …but a *function* written inside asks for its own, and leaves the outer one alone.
    assert!(
        inner("function f(a) { return (function () { return arguments[0]; })(); }")
            .arguments()
            .is_none()
    );
    // A script is not a call, so an `arguments` at the top level is an ordinary global read.
    let mut heap = Heap::new();
    let script = parse_script("var x = arguments;").expect("parses"); // a compiler test needs a tree
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
    assert!(chunk.arguments().is_none());
}
