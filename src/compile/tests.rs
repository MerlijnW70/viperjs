//! What the compiler refuses, and where it says so.
//!
//! Two kinds of test. Most run source and check the refusal it earns — the list of what ViperJS
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
    // "ViperJS cannot do this yet" and a wrong answer nobody notices.
    // The example has to be replaced each time one of them lands — a `class` was here, then a
    // generator, then an `await`, then `import('x')` until §13.3.10 arrived, then a destructuring
    // rest parameter until §15.1 did, then `import.meta` until §13.3.12 did.
    //
    // A `(?i:…)` pattern now, and it should be the last replacement: the RegExp **modifiers**
    // proposal is Stage 3 and building it is on nobody's list. It also puts the row back in a
    // *script*, which the last two could not be — and if it ever does land and there is nothing to
    // replace it with, this test has outlived the thing it was watching.
    let cases = [
        ("var r = /(?i:a)/;", "the RegExp modifiers proposal"),
        ("var r = 1 ? /(?i:a)/ : 3;", "the RegExp modifiers proposal"),
    ];
    for (source, what) in cases {
        let error = compile_source(source).expect_err("not implemented yet"); // the test is about the error
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
    let error = compile_source("1 + 2 * (3 - /(?i:a)/)").expect_err("not implemented yet"); // same
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported("the RegExp modifiers proposal")
    );
    // The literal, not the whole line: 0..22 is what an engine that reported the statement would
    // say.
    assert_eq!(error.span, Span::new(13, 21));
}

/// Compile `source` as a Script, for the rows that are about what the compiler refuses.
fn compile_source(source: &str) -> Result<Chunk, crate::compile::CompileError> {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a row that does not is the bug
    compile_script(&script, &mut heap)
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

/// What a chunk calls its own environment's slots, as plain strings.
fn names(chunk: &Chunk) -> Vec<&str> {
    chunk.bindings().iter().map(|at| &*at.name).collect()
}

/// What the scope at `index` of a chunk calls its slots.
fn scope_names(chunk: &Chunk, index: u32) -> Vec<&str> {
    chunk
        .scope(index)
        .expect("the chunk has that scope") // a compiler test needs the entry
        .names
        .iter()
        .map(|at| &*at.name)
        .collect()
}

#[test]
fn a_scope_names_its_slots_in_slot_order_and_says_which_may_not_be_assigned() {
    // DR-0018 — the list a direct `eval` resolves into. The claim is positional: the name at
    // index *i* is the name of slot *i*, so a compiler seeded from this emits the same
    // `(depth, index)` the compiler that built it would have.
    //
    // `arguments` is in it because §10.2.11 gives every non-arrow function the binding whether or
    // not the body reads it — see `a_function_is_given_an_arguments_object_only_if_it_reaches_for
    // _one`, which is about the *object*. The slot is a slot either way, and a list that skipped
    // it would put every name after it one place too early.
    let chunk = inner("function f(a, b) { var c; const d = 1; return a + b + c + d; }");
    assert_eq!(names(&chunk), ["a", "b", "arguments", "c", "d"]);
    // …and `const` travels with the name, because the compiler that resolves one is the only
    // thing that knows, and a chain has nowhere else to learn it from.
    let bindings = chunk.bindings();
    assert_eq!(
        bindings.iter().map(|at| at.mutability).collect::<Vec<_>>(),
        [
            crate::heap::Mutability::Mutable,
            crate::heap::Mutability::Mutable,
            crate::heap::Mutability::Mutable,
            crate::heap::Mutability::Mutable,
            crate::heap::Mutability::Const,
        ]
    );
}

#[test]
fn a_slot_the_compiler_made_for_itself_keeps_its_place_under_a_name_no_source_can_spell() {
    // Dropping them would shorten the list, and every name after one would then answer for the
    // wrong slot. `%` is in neither `IdentifierStart` nor `IdentifierPart`, so a slot that keeps
    // its place cannot be reached by anything a program can write.
    let chunk = inner("function f(a) { for (var k in a) { a = k; } return a; }");
    assert_eq!(names(&chunk)[0], "a");
    assert!(names(&chunk).contains(&"k"));
    assert!(
        names(&chunk)
            .iter()
            .filter(|at| at.starts_with('%'))
            .count()
            >= 4,
        "the four slots a `for`-`in` needs are in the list: {:?}",
        names(&chunk)
    );
}

#[test]
fn a_block_that_declares_something_names_its_own_slots_and_not_the_functions() {
    // A block's environment is where its `let` lives, so the block's list holds `b` and the
    // function's holds `a` — which is the whole reason the two are separate scopes.
    let chunk = inner("function f() { var a = 1; { let b = 2; return a + b; } }");
    assert!(names(&chunk).contains(&"a"));
    assert!(!names(&chunk).contains(&"b"));
    assert_eq!(scope_names(&chunk, 0), ["b"]);
    assert!(chunk.scope(1).is_none());
}

#[test]
fn a_loops_per_iteration_copy_is_the_same_scope_as_the_one_it_copies() {
    // §14.7.4.7 makes a *sibling* holding the same bindings, so the `CopyScope` names the entry
    // its `PushScope` does. Two entries would be two descriptions of one scope, which is one more
    // than can be kept in step.
    let chunk = inner("function f() { for (let i = 0; i < 3; i++) { } }");
    let opened: Vec<u32> = chunk
        .code()
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::PushScope(index) | Instruction::CopyScope(index) => Some(*index),
            _ => None,
        })
        .collect();
    assert!(opened.len() > 1, "the loop pushes and copies: {opened:?}");
    assert!(opened.iter().all(|index| *index == opened[0]));
    assert_eq!(scope_names(&chunk, opened[0]), ["i"]);
}

#[test]
fn a_name_that_belongs_to_a_scope_is_named_by_that_scope_and_not_by_the_level_around_it() {
    // The half of DR-0018 the last five commits were for. A name list has no position to be read
    // against, where `resolve` consults `live` at the position it is compiling — so a scope whose
    // names go out of scope while the level around it carries on has to *be* an environment, or
    // its names would still be in the level's list and an `eval` after it would resolve one.
    //
    // A `catch` parameter, a `switch` case block and a class body are three of the constructs that
    // used to flatten, and none of them leaves a name behind now.
    for (source, gone) in [
        (
            "function f(a) { try { a(); } catch (e) { a = e; } return a; }",
            "e",
        ),
        (
            "function f(a) { switch (a) { case 1: let b = 1; a = b; } return a; }",
            "b",
        ),
        (
            "function f(a) { a = class C { m() { return C; } }; return a; }",
            "C",
        ),
    ] {
        let chunk = inner(source);
        assert!(names(&chunk).contains(&"a"), "compiling {source}");
        assert!(
            !names(&chunk).contains(&gone),
            "{gone} belongs to its own scope in {source}: {:?}",
            names(&chunk)
        );
        assert!(
            (0..)
                .map_while(|index| chunk.scope(index))
                .any(|scope| scope.names.iter().any(|at| &*at.name == gone)),
            "…and some scope of {source} does name it"
        );
    }
}

#[test]
fn a_slot_no_name_reached_is_left_out_of_the_list_rather_than_stood_in_for() {
    // `Chunk::locals` is a high-water mark across every level the body compiled, so a body whose
    // nested block needed more slots than it did gets an environment with slots past its own last
    // name. What a resolver needs is that index *i* be slot *i*, which a prefix gives — so the list
    // stops, and the slots past it belong to a scope that has already been left.
    let chunk = inner("function f(a) { { let b = 1, c = 2, d = 3, e = 4; a = b + c + d + e; } }");
    assert_eq!(names(&chunk), ["a", "arguments"]);
    assert!(
        chunk.bindings().len() < chunk.locals(),
        "the block needed more slots than `f` did: {} of {}",
        chunk.bindings().len(),
        chunk.locals()
    );
}

#[test]
fn a_name_is_a_slot_the_compiler_chose_and_only_a_with_makes_it_a_walk() {
    // The second structural claim this file makes, and it is here because nothing else can make
    // it. §14.11 needs a name resolved at run time, and DR-0018's name lists mean that walk finds
    // *exactly* the binding the slot was chosen for — so the two are indistinguishable by any
    // program, and a test that runs source cannot tell which was emitted.
    //
    // What it pins is the engine's premise: a name costs nothing at run time (DR-0010), and the
    // walk is the exception rather than the rule. `lab/`'s `name-resolution` measured what
    // abandoning that costs — **3.0× to 3.7×** on local variable access — so this is a property
    // worth asserting rather than a shape that happens to be true today.
    let reads = |source: &str| {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the source parses"); // a compiler test needs a tree
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
        let inner = Rc::clone(chunk.function(0).expect("the script declares a function")); // likewise
        inner
            .code()
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::LoadVariable(_, _)
                        | Instruction::LoadGlobal(_)
                        | Instruction::LoadName(_)
                        | Instruction::StoreVariable(_, _)
                        | Instruction::StoreGlobal(_)
                        | Instruction::StoreName(_)
                )
            })
            .copied()
            .collect::<Vec<_>>()
    };
    // A local, a name from an enclosing scope, and a global: all three are placed when they are
    // compiled, and none of them is a walk.
    let placed = reads("function f() { var a = 1; a = a + 1; return a + globalThis; }");
    assert!(
        placed.iter().all(|instruction| !matches!(
            instruction,
            Instruction::LoadName(_) | Instruction::StoreName(_)
        )),
        "nothing outside a `with` resolves a name at run time: {placed:?}"
    );
    assert!(
        placed
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadVariable(_, _)))
    );
    assert!(
        placed
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_)))
    );
    // …and inside a `with` every one of them is, because the object may have any of those names
    // and the compiler cannot know which.
    let walked = reads("function f() { var o = {}; var a = 1; with (o) { a = a + 1; return a; } }");
    assert!(
        walked
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadName(_))),
        "a read inside a `with` is a walk: {walked:?}"
    );
    assert!(
        walked
            .iter()
            .any(|instruction| matches!(instruction, Instruction::StoreName(_))),
        "so is a write: {walked:?}"
    );
    // The body of a function *written inside* one too — its chain contains the object, so its
    // names are no more placeable than the ones written beside them.
    let nested =
        reads("function f() { var o = {}; with (o) { return function () { return a; }; } }");
    let _ = nested;
    let inner_names = {
        let mut heap = Heap::new();
        let script = parse_script(
            "function f() { var o = {}; with (o) { return function () { return a; }; } }",
        )
        .expect("parses"); // same
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
        let outer = Rc::clone(chunk.function(0).expect("f")); // same
        let inner = Rc::clone(outer.function(0).expect("the function inside the with")); // same
        inner.code().to_vec()
    };
    assert!(
        inner_names
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadName(_))),
        "a function written inside a `with` keeps the object in its chain: {inner_names:?}"
    );
}

#[test]
fn a_body_holding_a_direct_eval_resolves_its_names_at_run_time() {
    // §19.2.1.1 — a direct `eval` may add a `var` to this body's own variable scope, so a name
    // here cannot be pinned to a slot: the binding it should find may not exist until the eval
    // runs. Every read becomes a run-time lookup, exactly as inside a `with`.
    //
    // **Structural, because it has to be.** Turning the flag on is behaviour-preserving — DR-0018's
    // name lists make the walk find precisely the binding a slot would have named — so no program
    // distinguishes the two and mutation coverage cannot kill it. The same argument the `with` test
    // below makes, reached from the other cause.
    //
    // It is also what proves the *second pass* ran at all: the first compiles `x` before it has
    // met the `eval`, so a chunk with `LoadVariable` in it is one that was never compiled again.
    let reads = |source: &str| {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the source parses"); // a compiler test needs a tree
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
        let body = chunk
            .function(0)
            .expect("the script declares one function")
            .clone();
        body.code()
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::LoadVariable(_, _) | Instruction::LoadName(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    // Without an eval the local is a slot, which is the ordinary path and the fast one.
    let placed = reads("function f() { var x = 1; return x; }");
    assert!(
        placed
            .iter()
            .all(|instruction| matches!(instruction, Instruction::LoadVariable(_, _))),
        "a body with no eval keeps its slots: {placed:?}"
    );
    // With one — and the `eval` is written *after* the read, so only a second pass can have known.
    let dynamic = reads("function f() { var x = 1; var got = x; eval(''); return got; }");
    assert!(
        dynamic
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadName(_))),
        "a body holding a direct eval asks at run time: {dynamic:?}"
    );
    assert!(
        !dynamic
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadVariable(_, _))),
        "no name is left pinned to a slot: {dynamic:?}"
    );
    // The detection stops at a function boundary: a nested body's eval adds to its *own* variable
    // scope, so the outer body's names are not at risk from it and stay placed.
    let nested = reads("function f() { var x = 1; function g() { eval(''); } return x; }");
    assert!(
        nested
            .iter()
            .all(|instruction| matches!(instruction, Instruction::LoadVariable(_, _))),
        "a nested eval does not make the enclosing body dynamic: {nested:?}"
    );
}

#[test]
fn an_eval_resolves_names_at_run_time_only_when_the_call_was_made_inside_a_with() {
    // The third structural claim, and it is here for the reason the second one is: forcing
    // `with_depth` on is **behaviour-preserving**, because DR-0018's name lists make the run-time
    // walk find exactly the binding a slot would have named. So no program can tell the two apart,
    // mutation coverage cannot kill the flag, and the only honest assertion is about the
    // instructions. `lab/`'s `name-resolution` measured what abandoning the slot costs — 3.0× to
    // 3.7× on local variable access — which is why this is a property and not an accident.
    //
    // The flag is also *load-bearing for correctness* in the other direction, which the rows below
    // do not show and `vm::tests::with` does: an object environment has no names to list, so the
    // chain hands the eval compiler an empty level and every name past it would resolve to a
    // global. Both halves have to be true at once — dynamic when there is a `with`, placed when
    // there is not.
    let reads = |dynamic: bool| {
        let mut heap = Heap::new();
        let script = parse_script("a = a + 1;").expect("the source parses"); // a compiler test needs a tree
        let outer = vec![(
            vec![crate::heap::Binding {
                name: "a".into(),
                mutability: crate::heap::Mutability::Mutable,
                declared: crate::heap::Declared::Var,
            }],
            1,
        )];
        let chunk = compile_direct_eval(&script, &mut heap, outer, EvalVars::Own, dynamic)
            .expect("compiles"); // likewise
        chunk
            .code()
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::LoadVariable(_, _)
                        | Instruction::LoadGlobal(_)
                        | Instruction::LoadName(_)
                        | Instruction::StoreVariable(_, _)
                        | Instruction::StoreGlobal(_)
                        | Instruction::StoreName(_)
                )
            })
            .copied()
            .collect::<Vec<_>>()
    };
    let placed = reads(false);
    assert!(
        placed.iter().all(|instruction| !matches!(
            instruction,
            Instruction::LoadName(_) | Instruction::StoreName(_)
        )),
        "an eval outside a `with` places its names: {placed:?}"
    );
    assert!(
        placed
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadVariable(_, _))),
        "…and finds the caller's binding in the chain it was handed: {placed:?}"
    );
    let walked = reads(true);
    assert!(
        walked
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadName(_))),
        "an eval called inside a `with` walks for every name: {walked:?}"
    );
    assert!(
        walked
            .iter()
            .any(|instruction| matches!(instruction, Instruction::StoreName(_))),
        "…for writes as well as reads: {walked:?}"
    );
    assert!(
        walked.iter().all(|instruction| !matches!(
            instruction,
            Instruction::LoadVariable(_, _) | Instruction::LoadGlobal(_)
        )),
        "…and places nothing, because the object may hold any of them: {walked:?}"
    );
}

#[test]
fn an_update_expression_resolves_a_name_at_run_time_only_where_it_has_to() {
    // §13.4.4.1's "evaluate the reference once" is implemented two ways: a run-time resolution when
    // a `with` is open, and a slot when the compiler already knows which binding is meant. The
    // second is the fast path and *nothing a program can observe distinguishes them* — the run-time
    // walk finds exactly the binding the slot names — so mutation coverage reports the choice as
    // untested and is right to. This is the structural claim instead, which is the same remedy
    // DR-0018's `any_binding_object` needed for the same reason.
    let mut heap = Heap::new();
    let script = parse_script("function f() { var x = 1; x++; }").expect("parses"); // the test is the output
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
    let body = chunk.function(0).expect("the function body is nested here"); // same
    assert!(
        !body
            .code()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::ResolveName(_))),
        "a name the compiler resolved must not be walked again at run time: {:?}",
        body.code()
    );

    // …and inside a `with`, where it must. The same source shape, one scope different.
    let script = parse_script("function f() { var o = {}; with (o) { x++; } }").expect("parses"); // same
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
    let body = chunk.function(0).expect("the function body is nested here"); // same
    assert!(
        body.code()
            .iter()
            .any(|instruction| matches!(instruction, Instruction::ResolveName(_))),
        "a name inside a `with` has to be resolved at run time: {:?}",
        body.code()
    );
    // Resolved **once** — the whole point of the clause. Two would be the bug this replaced.
    assert_eq!(
        body.code()
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::ResolveName(_)))
            .count(),
        1
    );
}

/// Whether the first function in `source` makes its `return`'s call a §15.10 tail call.
///
/// Asked of the emitted instructions rather than of a program, because every one of DR-0027's six
/// conditions is a decision the compiler makes and only depth makes it visible: a behavioural test
/// for any single row would have to recurse past `MAX_CALL_DEPTH`, which is ten thousand, and would
/// then be measuring the whole feature rather than the row.
fn tail_called(source: &str) -> bool {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a compiler test needs a tree
    let chunk = compile_script(&script, &mut heap).expect("it compiles"); // as above
    let body = chunk.function(0).expect("the script declares one function");
    body.code().iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::CallTail(_) | Instruction::CallTailMethod(_)
        )
    })
}

#[test]
fn a_return_is_a_tail_call_exactly_where_the_source_says_it_is_safe() {
    // DR-0027's four compile-time conditions, one row each, and the rows are paired: every `false`
    // here differs from a `true` above it in one thing. §15.10.2's `HasCallInTailPosition` is
    // thirty productions of static semantics, and this is the whole of what they decide.
    for source in [
        // The plain shape, and the same call reached as a method.
        "function f(n) { 'use strict'; return f(n - 1); }",
        "var o = { m: function (n) { 'use strict'; return o.m(n - 1); } };",
        // §14.6.1 — a `catch` block is a tail position, and so is a `finally` block. Both are
        // places where nothing of this function is left to run.
        "function f(n) { 'use strict'; try { throw 0 } catch (e) { return f(n - 1) } }",
        "function f(n) { 'use strict'; try { } finally { return f(n - 1) } }",
        // A block, a loop body and a switch case are all just places, and the crossings they push
        // — a scope, an operand — go with the frame.
        "function f(n) { 'use strict'; { let x = 1; return f(n - x); } }",
        "function f(n) { 'use strict'; while (n) { return f(n - 1); } }",
        "function f(n) { 'use strict'; switch (0) { case 0: return f(n - 1); } }",
    ] {
        assert!(tail_called(source), "should be a tail call: {source}");
    }

    for source in [
        // §15.10.3 — sloppy code, where an `arguments` object may be joined to the parameters and
        // a `caller` may be walked. Identical to the first row above but for the directive.
        "function f(n) { return f(n - 1); }",
        // §14.6.1 — inside a `try` block, however the block ends: the `catch` beside it must still
        // be able to catch, and a `finally` beside it must still run.
        "function f(n) { 'use strict'; try { return f(n - 1) } catch (e) { } }",
        "function f(n) { 'use strict'; try { return f(n - 1) } finally { } }",
        // …and inside a `catch` that still has a `finally` under it, which is the row that shows
        // the question is about what is armed rather than about what is written.
        "function f(n) { 'use strict'; try { throw 0 } catch (e) { return f(n - 1) } finally { } }",
        // §7.4.9 — a `for`-`of` closes its iterator on the way out, which is work after the call.
        "function f(n) { 'use strict'; for (var x of [1]) { return f(n - 1); } }",
        // Not a call at all, and a call that is not the whole of the argument: `return f(g())`
        // must mark `f` and never `g`, and neither is marked when the argument is arithmetic.
        "function f(n) { 'use strict'; return n - 1; }",
        "function f(n) { 'use strict'; return 1 + f(n - 1); }",
        // §13.3.6.1 — written as the bare name `eval`. The compiler cannot know whether it holds
        // `%eval%`, and if it does the text runs in *this* frame's scopes.
        "function f(n) { 'use strict'; return eval(n - 1); }",
    ] {
        assert!(!tail_called(source), "should not be a tail call: {source}");
    }
}

#[test]
fn only_the_outermost_call_of_a_return_is_in_tail_position() {
    // The reason `call_at` takes the position as an argument rather than reading a flag off the
    // compiler: `return f(g())` has two calls in it, `g` is compiled first, and a flag set around
    // the argument would mark `g` — which answers into a frame that is still needed.
    let mut heap = Heap::new();
    let script =
        parse_script("function f(n) { 'use strict'; return f(g(n)); }").expect("the source parses"); // a compiler test needs a tree
    let chunk = compile_script(&script, &mut heap).expect("it compiles"); // as above
    let body = chunk.function(0).expect("the script declares one function");
    let calls: Vec<&Instruction> = body
        .code()
        .iter()
        .filter(|instruction| {
            matches!(instruction, Instruction::Call(_) | Instruction::CallTail(_))
        })
        .collect();
    // `g` first and ordinary, `f` second and tail — in that order, because the argument is
    // evaluated before the call that takes it.
    assert!(matches!(
        calls.as_slice(),
        [Instruction::Call(1), Instruction::CallTail(1)]
    ));
}

#[test]
fn a_direct_eval_carries_where_it_was_written_and_the_default_is_the_body() {
    // §10.2.11 step 20's question, and it is a **structural** claim for the reason the `with` test
    // above states one: by run time a parameter default and a body statement are instructions in
    // one chunk against one environment, so the answer travels on the instruction and no program
    // can read it directly. What a program *can* see is the refusal it produces, which
    // `vm::tests::eval` asserts; what nothing else pins is the value a compiler starts with.
    //
    // `Compiler::in_parameters` begins false and only `compile::function` ever sets it, so every
    // chunk that is not a function body — a script, an eval, a module — emits `Body` by never
    // having said otherwise. That default is invisible to conformance: a script's own direct eval
    // is answered by `EvalVars::Global` before the site is consulted, and an eval nested in an eval
    // by the same rule. So it is asserted here or it is asserted nowhere.
    let mut heap = Heap::new();
    let script = parse_script("eval('1')").expect("the source parses"); // a compiler test needs a tree
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
    let sites = chunk
        .code()
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::CallDirectEval { site, .. }
            | Instruction::CallDirectEvalMethod { site, .. } => Some(*site),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sites,
        vec![EvalSite::Body],
        "a direct eval outside any parameter list is written in a body"
    );
}

#[test]
fn only_code_that_keeps_a_completion_value_pays_for_a_finally() {
    // §14.15.3 step 3 needs the value the `try` produced parked across the finalizer and put back
    // on the way out — but only where a completion value is kept at all, which is a Script and a
    // direct `eval` and nothing else. A function's statements discard their values anyway, so the
    // parking would be a hidden slot and six instructions that no program can observe.
    //
    // **Structural, because that is the only kind of test this can have.** Emitting the machinery
    // inside a function is invisible: the register it saves and restores is never read there, so
    // every behavioural row answers the same either way and mutation coverage is right to call the
    // guard untested. This is the row that fails when it goes.
    let parked = |source: &str| {
        let mut heap = Heap::new();
        let script = parse_script(source).expect("the source parses"); // a compiler test needs a tree
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // likewise
        // **Nested chunks too**, which is the whole point: a function body is a chunk of its own,
        // so counting only the script's code answers zero for a function whether the guard is
        // there or not — and the row below would pass against the mutation it exists to catch.
        fn parked_in(chunk: &Chunk) -> usize {
            let here = chunk
                .code()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::LoadCompletion))
                .count();
            let mut index = 0;
            let mut nested = 0;
            while let Some(inner) = chunk.function(index) {
                nested += parked_in(inner);
                index += 1;
            }
            here + nested
        }
        parked_in(&chunk)
    };
    assert_eq!(
        parked("try { 1 } finally { 2 }"),
        2,
        "a script's finalizer is emitted twice — the ordinary way out and the unwinding one — and \
         both have to park the value"
    );
    assert_eq!(
        parked("function f() { try { 1 } finally { 2 } }"),
        0,
        "a function keeps no completion value, so there is nothing to park"
    );
    // A `break` **in the try block** jumps past the finalizer, so the finalizer is emitted a third
    // time for the crossing — and that copy parks the value as the other two do, or what the label
    // carries out depends on which copy ran. A `break` written *inside* the finalizer is not this
    // shape: it crosses nothing, because the crossing is taken down before the finalizer is
    // compiled.
    assert_eq!(
        parked("L: { try { break L } finally { 2 } }"),
        3,
        "the crossing's copy parks the value as the other two do"
    );
    assert_eq!(
        parked("L: { try { 1 } finally { 2; break L } }"),
        2,
        "a break inside the finalizer crosses nothing, so there is no third copy"
    );
}
