//! Array literals (ECMAScript §13.2.4).
//!
//! # Commas do two different things, and the difference is whether one follows an element
//!
//! `ArrayLiteral` is written as an `ElementList` with `Elision`s scattered through it, and an
//! `Elision` is a run of commas. That makes a comma a separator in one place and a hole in
//! another, and the rule for telling them apart is the whole content of this file: **a comma
//! makes a hole when nothing precedes it in its slot**, and separates otherwise. So a trailing
//! comma is a separator with nothing after it, not a hole:
//!
//! ```text
//! []        0        [1]       1        [1, 2]    2
//! [,]       1        [, 1]     2        [1, ]     1
//! [, , ]    2        [, , 1]   3        [1, , ]   2
//! [1, , 2]  3        [a, b, ]  2        [a, b, ,] 3
//! ```
//!
//! Those lengths are what a program can observe, and every one of them is asserted below against
//! what the specification's grammar produces — because the difference between `[1, ]` and `[1, ,]`
//! is one character and one element, and nothing in the code will remind a reader of it.
//!
//! # What is not here
//!
//! `[a] = b` and `for ([a] of b)`, which parse in ECMAScript and are refused here. The specification
//! reaches them through a cover grammar: an `ArrayLiteral` on the left of an assignment is
//! *refined* into an `ArrayAssignmentPattern` (§13.15.5), and §13.15.1 skips its usual
//! AssignmentTargetType rule for exactly that case. Refining a literal into a pattern is the next
//! slice, and until it lands an array is only ever a value. The tests below pin that.

use super::expression::AllowIn;
use super::{ParseError, Parser};
use crate::ast::{ArrayElement, Expr, ExprKind};
use crate::lexer::{Goal, TokenKind};

impl Parser<'_> {
    /// `ArrayLiteral` (§13.2.4), with the cursor on the `[`.
    pub(super) fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let open = self.advance(Goal::RegExp)?;
        self.enter()?;
        // Inside a literal nothing is decided: an expression here may still turn out to be part
        // of a pattern. See [`Parser::literal_depth`].
        self.literal_depth += 1;
        let elements = self.parse_array_elements();
        self.literal_depth -= 1;
        self.leave();
        let elements = elements?;
        let close = self.eat(TokenKind::RBracket, Goal::Div, "`]`")?;
        Ok(Expr::new(
            ExprKind::Array(elements),
            open.span.to(close.span),
        ))
    }

    /// The elements between the brackets, holes included.
    ///
    /// Apart from [`Parser::parse_array_literal`] so the growing list is not a local in a frame
    /// the bracket recursion passes through.
    fn parse_array_elements(&mut self) -> Result<Box<[ArrayElement]>, ParseError> {
        let mut elements = Vec::new();
        while self.current.kind != TokenKind::RBracket {
            // A comma in a slot nothing has filled is an `Elision`. Reaching this with a comma
            // in hand means the previous iteration consumed a separator, or that none has been
            // read at all — either way the slot is empty and this comma is a hole.
            if self.current.kind == TokenKind::Comma {
                elements.push(ArrayElement::Hole);
                self.advance(Goal::RegExp)?;
                continue;
            }
            // `SpreadElement : ... AssignmentExpression`, or the plain element. Inline rather
            // than in a function of its own: this loop is on the path a bracket recurses
            // through, and a frame here is nesting depth every array pays for.
            let spread = self.current.kind == TokenKind::DotDotDot;
            if spread {
                self.advance(Goal::RegExp)?;
            }
            // `AssignmentExpression[+In]`, so a comma separates elements rather than sequencing
            // values: `[a, b]` is two and `[(a, b)]` is one. `[+In]` whatever clause encloses the
            // literal, a bracket starting afresh.
            let value = self.parse_assignment(AllowIn::Yes)?;
            elements.push(if spread {
                ArrayElement::Spread(value)
            } else {
                ArrayElement::Value(value)
            });
            if self.current.kind != TokenKind::Comma {
                break;
            }
            // A *trailing* comma after a `...` leaves no trace once parsed — it adds no
            // element, so `[...a, ]` and `[...a]` become the same list. They are not the same
            // pattern, so the difference is recorded while it is still there to see. A comma with
            // an element after it needs no record: that element is visible in the list, and
            // refinement finds it sitting behind a rest.
            if matches!(elements.last(), Some(ArrayElement::Spread(_)))
                && self.peek(Goal::RegExp)?.kind == TokenKind::RBracket
            {
                self.rest_followed_by_comma.get_or_insert(self.current.span);
            }
            // The separator. If a `]` follows it the literal simply ends — a trailing comma adds
            // nothing, which is what makes `[1, ]` one element and `[1, , ]` two.
            self.advance(Goal::RegExp)?;
        }
        Ok(elements.into_boxed_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// How many elements `source` has, holes counted.
    ///
    /// This is `length`, which is what a program sees — so every case below is a claim that can
    /// be checked against any engine, and was.
    fn length(source: &str) -> usize {
        let expr = parse_expression(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // a test about elements needs them
        match expr.kind {
            ExprKind::Array(elements) => elements.len(),
            other => panic!("{source:?} parsed as {other:?}"), // same
        }
    }

    #[test]
    fn a_comma_is_a_hole_when_nothing_precedes_it_and_a_separator_otherwise() {
        // Every one of these is `[…].length` in a running engine.
        assert_eq!(length("[]"), 0);
        assert_eq!(length("[1]"), 1);
        assert_eq!(length("[1, 2]"), 2);
        assert_eq!(
            length("[1, 2, ]"),
            2,
            "a trailing comma is a separator with nothing after it"
        );
        assert_eq!(length("[,]"), 1, "…while a leading one is a hole");
        assert_eq!(length("[, , ]"), 2);
        assert_eq!(length("[1, ]"), 1);
        assert_eq!(length("[1, , ]"), 2);
        assert_eq!(length("[, 1]"), 2);
        assert_eq!(length("[, , 1]"), 3);
        assert_eq!(length("[1, , 2]"), 3);
        assert_eq!(length("[a, b, , ]"), 3);
        // …and the shapes, not just the counts.
        assert_eq!(shape("[]"), "[]");
        assert_eq!(shape("[1]"), "[1]");
        assert_eq!(shape("[1, 2]"), "[1 2]");
        assert_eq!(shape("[, 1]"), "[<hole> 1]");
        assert_eq!(shape("[1, , 2]"), "[1 <hole> 2]");
        assert_eq!(shape("[1, ]"), "[1]");
        assert_eq!(shape("[a, b, , ]"), "[a b <hole>]");
        let script = parse_script("[1, 2];").expect("this parses");
        assert_eq!(script.body[0].span, crate::span::Span::new(0, 7));
    }

    #[test]
    fn an_element_is_an_assignment_expression_so_a_comma_never_sequences() {
        // `ElementList` separates `AssignmentExpression`s, which a comma expression is not.
        assert_eq!(shape("[a, b]"), "[a b]");
        assert_eq!(shape("[(a, b)]"), "[(, a b)]");
        assert_eq!(shape("[a = 1]"), "[(= a 1)]");
        assert_eq!(shape("[a ? b : c]"), "[(? a b c)]");
        assert_eq!(shape("[[1], [2]]"), "[[1] [2]]");
        assert_eq!(
            shape("[a in b]"),
            "[(in a b)]",
            "a bracket is `[+In]` whatever encloses it"
        );
        // …including inside a `for` header, where the clause around it is not.
        assert!(parse_script("for ([a in b];;);").is_ok());
    }

    #[test]
    fn a_spread_element_takes_the_place_of_one_element() {
        assert_eq!(shape("[...a]"), "[(... a)]");
        assert_eq!(shape("[...a, b]"), "[(... a) b]");
        assert_eq!(shape("[a, ...b]"), "[a (... b)]");
        assert_eq!(shape("[, ...a]"), "[<hole> (... a)]");
        assert_eq!(shape("[...a, ]"), "[(... a)]");
        assert_eq!(length("[...a, b]"), 2);
        // `SpreadElement : ... AssignmentExpression` — there is no bare `...`.
        assert!(parse_expression("[...]").is_err());
        assert!(parse_expression("[..., a]").is_err());
        assert_eq!(shape("[...a = b]"), "[(... (= a b))]");
    }

    #[test]
    fn an_array_is_a_value_here_and_a_pattern_only_where_an_equals_says_so() {
        // §13.15.1 skips its AssignmentTargetType rule when the left of an assignment is an
        // `ArrayLiteral`, because §13.15.5 refines it into an `ArrayAssignmentPattern` instead —
        // see [`super::super::pattern`], which is where that happens.
        assert!(parse_script("[a] = b;").is_ok());
        assert!(parse_script("for ([a] of b);").is_ok());
        // Everywhere else the brackets are a value, and this file is what reads them.
        assert!(parse_script("a = [1, 2];").is_ok());
        assert!(parse_script("f([1], [2]);").is_ok());
        assert!(parse_script("[1, 2].length;").is_ok());
        // The refinement is the `=`'s doing, so an array that is merely compared stays a value —
        // and one that cannot be a pattern says so only when something asks it to be.
        assert!(parse_script("[1] == b;").is_ok());
        assert_eq!(
            script_error_kind("[1] = b;"),
            ParseErrorKind::InvalidDestructuringTarget
        );
    }

    /// The kind of error `source` fails with, as a script.
    fn script_error_kind(source: &str) -> ParseErrorKind {
        match parse_script(source) {
            Err(err) => err.kind,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // a test about an error needs one
        }
    }

    #[test]
    fn no_array_however_truncated_can_panic() {
        let cases = [
            "[".to_string(),
            "[1".to_string(),
            "[1,".to_string(),
            "[,".to_string(),
            "[...".to_string(),
            "]".to_string(),
            "[a b]".to_string(),
            "[".repeat(10_000),
            format!("[{}]", ",".repeat(100_000)),
            format!("[{}]", "1,".repeat(100_000)),
        ];
        for source in &cases {
            let _ = parse_expression(source);
        }
        // A long flat list is a loop, so it is bounded by memory; nesting is not.
        assert_eq!(length(&format!("[{}]", "1,".repeat(10_000))), 10_000);
        assert_eq!(length(&format!("[{}]", ",".repeat(10_000))), 10_000);
        assert_eq!(
            parse_expression(&"[".repeat(10_000))
                .map(|_| ())
                .unwrap_err()
                .kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
