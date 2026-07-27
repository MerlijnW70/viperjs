//! The parser's cross-cutting tests: the nesting cap, what it costs in stack, and the promise
//! that no source panics.
//!
//! Apart from `mod.rs` because they are about the parser as a whole rather than about any one
//! production — every other file here tests the grammar it reads, and these test what they add
//! up to. `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` is the important one: it is
//! the real specification of [`super::MAX_NESTING_DEPTH`], and the only thing that can tell you
//! a slice has made nesting more expensive.

use super::*;
use crate::ast::ExprKind;
use crate::lexer::LexErrorKind;
use crate::parser::test_support::*;
use crate::span::Span;
#[test]
fn nesting_is_bounded_by_the_parser_rather_than_by_the_stack() {
    // DR-0002: a stack overflow is not a failure any `Result` can rescue, and it takes the
    // embedder's process with it. So the cap is the parser's, it is explicit, and it is
    // reported as its own kind — the grammar has no depth limit, this refusal is ours.
    let at_the_cap = format!(
        "{}1{}",
        "(".repeat(MAX_NESTING_DEPTH as usize),
        ")".repeat(MAX_NESTING_DEPTH as usize)
    );
    assert_eq!(parse(&at_the_cap).kind, ExprKind::Number(1.0));

    let past_it = format!(
        "{}1{}",
        "(".repeat(MAX_NESTING_DEPTH as usize + 1),
        ")".repeat(MAX_NESTING_DEPTH as usize + 1)
    );
    assert_eq!(error(&past_it).kind, ParseErrorKind::TooDeeplyNested);

    // Far past it: the answer must still be an error rather than a crash, and must arrive
    // without parsing the other million brackets first.
    let absurd = "(".repeat(1_000_000);
    assert_eq!(error(&absurd).kind, ParseErrorKind::TooDeeplyNested);

    // The count unwinds. Two deep parses in a row must both succeed, which they cannot if a
    // failed one leaks its depth — and a failure inside brackets is the case that leaks,
    // since `enter` and `leave` are paired by hand rather than by a scope.
    assert!(parse_expression("((((1))))").is_ok());
    assert!(parse_expression("((((@))))").is_err());
    assert!(parse_expression("((((1))))").is_ok());
    assert!(parse_expression("((((1)").is_err());
    assert!(parse_expression("((((1))))").is_ok());
}
#[test]
fn parsing_at_the_cap_fits_in_the_stack_it_claims_to_need() {
    // This is what makes MAX_NESTING_DEPTH a measurement rather than a hope. A cap that the
    // stack cannot afford is worse than no cap at all: the parse dies by overflow — which
    // DR-0002 says no `Result` can rescue and which takes the embedder's process with it —
    // one level before the check that was supposed to prevent exactly that.
    //
    // One mebibyte is the smallest thread stack in common use, and this runs in a debug
    // build, which is several times hungrier than a release one. If a future production adds
    // frames between one bracket and the next, this test is where it says so.
    //
    // Every recursive path gets its own full-depth parse, because the count is one budget
    // shared between them and what bounds it is whichever spends the most stack per level.
    // Expressions do today by a wide margin; the rest are here so that the day one of them
    // overtakes, this fails rather than the cap quietly becoming a lie for one grammar form.
    let deep = MAX_NESTING_DEPTH as usize;
    let paths = [
        format!("{}1{}", "(".repeat(deep), ")".repeat(deep)),
        format!("{}{}", "{".repeat(deep), "}".repeat(deep)),
        format!("{}{};", "[".repeat(deep), "]".repeat(deep)),
        // The pattern refinement recurses once per level too, on top of the parse.
        format!("{}a{} = b;", "[".repeat(deep), "]".repeat(deep)),
        format!("{}1{};", "({a: ".repeat(deep / 2), "})".repeat(deep / 2)),
        format!("{}a;", "if (a) ".repeat(deep)),
        format!("{}a;", "if (a) b; else ".repeat(deep)),
        format!("{}a;", "while (a) ".repeat(deep)),
        format!("{}a;", "for (;;) ".repeat(deep)),
        format!("{}1", "a => ".repeat(deep)),
        // Half the levels: a function spends one of the count on itself and one on its body.
        format!(
            "{}{}",
            "function f() { ".repeat(deep / 2),
            "}".repeat(deep / 2)
        ),
        // Distinct labels, since §8.3.1 refuses a repeat before the stack ever gets a say.
        format!(
            "{};",
            (0..deep).map(|i| format!("l{i}: ")).collect::<String>()
        ),
        format!("{};", "with (a) ".repeat(deep - 1)),
        // One shallower: a level holds one of the count for the loop itself, and the
        // innermost header still needs one more to read the target before the `in`.
        format!("{}a;", "for (a in b) ".repeat(deep - 1)),
        format!("{}a;{}", "do ".repeat(deep), " while (b);".repeat(deep)),
        // Half as many levels, because a `try` spends two of the count on each: one for the
        // statement and one for its guarded Block, which is a nested scope in its own right.
        format!(
            "{}{}",
            "try { ".repeat(deep / 2),
            "} catch (e) {}".repeat(deep / 2)
        ),
        // One shallower again: each level holds one of the count for its CaseBlock, and the
        // innermost `case` still needs one more to parse the expression after it.
        format!(
            "{}{}",
            "switch (x) { case 1: ".repeat(deep - 1),
            "}".repeat(deep - 1)
        ),
        // One shallower, because `throw` counts the frame it holds while its value is
        // parsed — so `throw` plus a full-depth expression is one level past the cap, and
        // the deepest that parses has one bracket fewer.
        format!("throw {}1{};", "(".repeat(deep - 1), ")".repeat(deep - 1)),
    ];
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            for source in &paths {
                // A failure would be a bug in the test's sources, not a stack problem — the
                // point of the run is that it returns at all.
                assert!(parse_script(source).is_ok(), "{source:.32?} at full depth");
            }
            parse_expression(&format!("{}1{}", "(".repeat(deep), ")".repeat(deep)))
                .map(|expr| expr.kind)
        })
        .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
    let parsed = worker
        .join()
        .unwrap_or_else(|_| panic!("a full-depth parse did not survive one mebibyte")); // the panic IS the assertion
    assert_eq!(parsed, Ok(ExprKind::Number(1.0)));
}
#[test]
fn a_lexical_failure_arrives_as_a_parse_error_with_its_span_intact() {
    // The parser does not re-word what the lexer said; it carries it. A diagnostic that lost
    // the difference between "unterminated string" and "unexpected token" would be worse
    // than one that never had it.
    assert_eq!(
        error("'abc").kind,
        ParseErrorKind::Lexical(LexErrorKind::UnterminatedStringLiteral)
    );
    assert_eq!(error("'abc").span, Span::new(0, 4));
    assert_eq!(
        error("@").kind,
        ParseErrorKind::Lexical(LexErrorKind::UnexpectedCharacter)
    );
    assert_eq!(
        error("(1 @)").kind,
        ParseErrorKind::Lexical(LexErrorKind::UnexpectedCharacter),
        "a failure mid-parse is still the lexer's, reported where it happened"
    );
    assert_eq!(error("(1 @)").span, Span::new(3, 4));
    assert_eq!(
        error("3in").kind,
        ParseErrorKind::Lexical(LexErrorKind::NumericLiteralFollowedByIdentifierOrDigit)
    );
}
#[test]
fn no_source_however_odd_can_make_the_parser_panic() {
    // DR-0002, at the level above the lexer's. Deep nesting is the one that matters here,
    // and the rest are the shapes that reach the parser's own error paths.
    let cases = [
        String::new(),
        "(".repeat(100_000),
        ")".repeat(100_000),
        "((((".to_string(),
        "'".to_string(),
        "/".to_string(),
        "`".to_string(),
        "0x".to_string(),
        format!("({})", "1 ".repeat(10_000)),
        format!("{}1", "(".repeat(500)),
    ];
    for source in &cases {
        // The verdict does not matter; not unwinding does.
        let _ = parse_expression(source);
    }
    // An empty source wants an expression and says so.
    assert_eq!(
        error("").kind,
        ParseErrorKind::Unexpected {
            expected: "an expression",
            found: TokenKind::Eof,
        }
    );
}
