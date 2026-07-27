//! The operator ladder of §13.4 – §13.16, from `Expression` down to `UnaryExpression`.
//!
//! Every function here is on the path a bracket recurses through, so every one of them costs
//! nesting depth — which is why `parse_assignment` also handles the conditional operator, why
//! `parse_unary` also handles update expressions, and why the operator tables are next door in
//! [`super::operator`] rather than being a cascade of one function per grammar layer. See
//! [`super::MAX_NESTING_DEPTH`] and DR-0006.
//!
//! The layers below this one are [`super::member`] (§13.3) and [`super::primary`] (§13.2).

use super::operator::{
    OperatorKind, assignment_operator, binary_operator, combine, is_simple_assignment_target,
    unary_operator, update_operator,
};
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{BinaryOperator, Expr, ExprKind, UpdateOperator};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;

/// Whether `in` is a relational operator here — the `[In]` grammar parameter of §13.
///
/// It exists for one construct. `for (a in b; ; )` and `for (a in b)` begin identically, and the
/// second is a `for`-`in` loop, so the head of a `for` has to be read with `in` unavailable or
/// the two could never be told apart. §13.10 gates both `in` alternatives of
/// `RelationalExpression` on `[+In]`, and every other production either propagates the parameter
/// or resets it.
///
/// Resetting is most of the rule and is the half that is easy to get wrong: `for (a[b in c];;)`,
/// `for (f(a in b);;)` and `for ((a in b);;)` all parse, because a bracket starts a fresh
/// `Expression[+In]`. So does the *middle* arm of a conditional — `for (a ? b in c : d;;)` is
/// legal — while the last arm propagates. Passing this as a parameter rather than keeping it on
/// the parser is what makes every one of those a decision the compiler insists on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AllowIn {
    /// `[+In]` — the ordinary case, everywhere but the head of a `for`.
    Yes,
    /// `[~In]`, which only the head of a `for` asks for.
    No,
}

impl Parser<'_> {
    /// `Expression` (§13.16) — one or more `AssignmentExpression`s separated by commas.
    pub(super) fn parse_expression(&mut self, allow_in: AllowIn) -> Result<Expr, ParseError> {
        let first = self.parse_assignment(allow_in)?;
        if self.current.kind != TokenKind::Comma {
            return Ok(first);
        }
        let mut parts = vec![first];
        while self.current.kind == TokenKind::Comma {
            self.advance(Goal::RegExp)?;
            parts.push(self.parse_assignment(allow_in)?);
        }
        let span = match (parts.first(), parts.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            // `parts` was built from one element and only ever grows, so this cannot happen;
            // an empty span is the answer that costs nothing if it somehow does.
            _ => Span::empty_at(self.current.span.start),
        };
        Ok(Expr::new(
            ExprKind::Sequence(parts.into_boxed_slice()),
            span,
        ))
    }

    /// `AssignmentExpression` (§13.15) and `ConditionalExpression` (§13.14), in one frame.
    ///
    /// Merged deliberately. They are separate productions, but a `?` and an `=` cannot both
    /// follow the same operand — a conditional is not a `LeftHandSideExpression`, so it can only
    /// be assigned to in the sense that saying so is an error — and one function fewer on the
    /// recursion path is depth that [`MAX_NESTING_DEPTH`] does not have to give up.
    ///
    /// Falling through to the assignment check rather than returning early after a conditional
    /// is what makes `(a ? b : c) = d` say "this expression cannot be assigned to" instead of
    /// the far less helpful "expected end of input".
    pub(super) fn parse_assignment(&mut self, allow_in: AllowIn) -> Result<Expr, ParseError> {
        let mut left = self.parse_binary(0, allow_in)?;
        if self.current.kind == TokenKind::Question {
            left = self.parse_conditional_tail(left, allow_in)?;
        }
        let Some(operator) = assignment_operator(self.current.kind) else {
            return Ok(left);
        };
        // §13.15.1. The check is here rather than in the AST because it is a *syntax* error:
        // `1 = 2` must be refused before anything runs, not when it runs.
        if !is_simple_assignment_target(&left) {
            return Err(ParseError {
                kind: ParseErrorKind::InvalidAssignmentTarget,
                span: left.span,
            });
        }
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // `LeftHandSideExpression = AssignmentExpression` — the recursion is on the right, so
        // `a = b = c` is `a = (b = c)`.
        let value = self.parse_assignment(allow_in);
        self.leave();
        let value = value?;
        let span = left.span.to(value.span);
        Ok(Expr::new(
            ExprKind::Assignment {
                operator,
                target: Box::new(left),
                value: Box::new(value),
            },
            span,
        ))
    }

    /// `? AssignmentExpression : AssignmentExpression` (§13.14), with `test` already parsed.
    ///
    /// Both arms are `AssignmentExpression`s, which §13.14's own Note explains: it lets an
    /// assignment be governed by either arm, and keeps a comma expression out of the middle.
    /// So `a ? b = 1 : c = 2` is two assignments, and `a ? b, c : d` is an error.
    fn parse_conditional_tail(
        &mut self,
        test: Expr,
        allow_in: AllowIn,
    ) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let arms = self.parse_conditional_arms(allow_in);
        self.leave();
        let (consequent, alternate) = arms?;
        let span = test.span.to(alternate.span);
        Ok(Expr::new(
            ExprKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
            span,
        ))
    }

    /// The two arms of a conditional, split out so their locals do not sit in a frame that the
    /// bracket recursion also passes through.
    fn parse_conditional_arms(&mut self, allow_in: AllowIn) -> Result<(Expr, Expr), ParseError> {
        // `? AssignmentExpression[+In] : AssignmentExpression[?In]` — the middle arm resets
        // the parameter and the last one propagates it, so `for (a ? b in c : d;;)` parses
        // and an `in` in the last arm still would not.
        let consequent = self.parse_assignment(AllowIn::Yes)?;
        self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
        let alternate = self.parse_assignment(allow_in)?;
        Ok((consequent, alternate))
    }

    /// The operator layers of §13.6 – §13.13, by precedence climbing.
    ///
    /// `minimum` is the weakest binding power this call will accept; an operator weaker than it
    /// belongs to the caller. Left-associative operators recurse one level tighter so the next
    /// one of equal precedence is left for the loop, and `**` recurses at its own level so the
    /// next one is taken by the recursion instead — which is the whole of associativity.
    fn parse_binary(&mut self, minimum: u8, allow_in: AllowIn) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some(operator) = binary_operator(self.current.kind) {
            if operator.precedence < minimum {
                break;
            }
            // §13.10 gates the `in` alternatives of `RelationalExpression` on `[+In]`. Under
            // `[~In]` the word is not an operator at all, so the expression simply ends and
            // whatever wanted `in` next is welcome to it — in a `for` head, the `for`-`in`
            // production. Nothing below here takes the parameter because nothing below
            // `RelationalExpression` has one: a `ShiftExpression` cannot contain a bare `in`.
            if allow_in == AllowIn::No && operator.kind == OperatorKind::Binary(BinaryOperator::In)
            {
                break;
            }
            // §13.6: the left operand of `**` is an `UpdateExpression`, and a prefix unary is
            // not one. Checked before the operator is consumed so the error can point at the
            // operand that is wrong rather than at the operator that noticed.
            if operator.kind == OperatorKind::Binary(BinaryOperator::Exponent)
                && matches!(left.kind, ExprKind::Unary { .. })
                && !left.parenthesized
            {
                return Err(ParseError {
                    kind: ParseErrorKind::ExponentiationOnUnary,
                    span: left.span,
                });
            }
            // An operator is followed by an operand, so the goal is `RegExp`: in `a / /b/`, the
            // first slash divides and the second opens a literal.
            self.advance(Goal::RegExp)?;
            let tighter = if operator.right_associative {
                operator.precedence
            } else {
                operator.precedence + 1
            };
            self.enter()?;
            let right = self.parse_binary(tighter, allow_in);
            self.leave();
            let right = right?;
            left = combine(left, operator, right)?;
        }
        Ok(left)
    }

    /// `UnaryExpression` (§13.5) and `UpdateExpression` (§13.4), in one frame.
    ///
    /// Merged for the same reason the conditional lives inside `parse_assignment`: both are on
    /// the path a bracket recurses through, and a frame there costs nesting depth (DR-0006).
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if let Some(operator) = unary_operator(self.current.kind) {
            let token = self.advance(Goal::RegExp)?;
            self.enter()?;
            // `- UnaryExpression`, so the operators stack: `- - a` is two of them.
            let argument = self.parse_unary();
            self.leave();
            let argument = argument?;
            let span = token.span.to(argument.span);
            return Ok(Expr::new(
                ExprKind::Unary {
                    operator,
                    argument: Box::new(argument),
                },
                span,
            ));
        }
        if let Some(operator) = update_operator(self.current.kind) {
            let token = self.advance(Goal::RegExp)?;
            self.enter()?;
            // `++ UnaryExpression` — the operand is a unary expression, not a
            // LeftHandSideExpression, so `++ typeof a` parses and is then rejected below for
            // having nothing to increment.
            let argument = self.parse_unary();
            self.leave();
            let argument = argument?;
            return Self::update(operator, true, token.span, argument);
        }

        // `UpdateExpression : LeftHandSideExpression [no LineTerminator here] ++`
        let operand = self.parse_member(true)?;
        let Some(operator) = update_operator(self.current.kind) else {
            return Ok(operand);
        };
        // The first restricted production this parser meets, and the reason every token has
        // carried a `newline_before` flag since the lexer's very first slice. `a\n++b` is not
        // `a++ b`: the postfix form simply does not match across a line break, which leaves `a`
        // and `++b` as two things — and once statements exist, as two statements.
        if self.current.newline_before {
            return Ok(operand);
        }
        let token = self.advance(Goal::Div)?;
        Self::update(operator, false, token.span, operand)
    }

    /// Build an update node, enforcing §13.4.1.
    ///
    /// `span` is the operator's; the node covers both it and the operand, whichever came first.
    fn update(
        operator: UpdateOperator,
        prefix: bool,
        operator_span: Span,
        argument: Expr,
    ) -> Result<Expr, ParseError> {
        // §13.4.1: a Syntax Error if the AssignmentTargetType is invalid. `1++` is refused for
        // the same reason `1 = 2` is, and by the same test.
        if !is_simple_assignment_target(&argument) {
            return Err(ParseError {
                kind: ParseErrorKind::InvalidAssignmentTarget,
                span: argument.span,
            });
        }
        let span = operator_span.to(argument.span);
        Ok(Expr::new(
            ExprKind::Update {
                operator,
                prefix,
                argument: Box::new(argument),
            },
            span,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AssignmentOperator;
    use crate::parser::parse_expression;
    use crate::parser::test_support::*;
    #[test]
    fn every_prefix_operator_the_grammar_has_today() {
        // §13.5. `await` is absent: it is the `[+Await]` alternative and needs a parameter that
        // arrives with async functions.
        assert_eq!(shape("-a"), "(- a)");
        assert_eq!(shape("+a"), "(+ a)");
        assert_eq!(shape("!a"), "(! a)");
        assert_eq!(shape("~a"), "(~ a)");
        assert_eq!(shape("typeof a"), "(typeof a)");
        assert_eq!(shape("void a"), "(void a)");
        assert_eq!(shape("delete a"), "(delete a)");
        // `- UnaryExpression`, so they stack — and `--` would be one token, which is why the
        // spaced form is the one that means two negations.
        assert_eq!(shape("- - a"), "(- (- a))");
        assert_eq!(shape("!!a"), "(! (! a))");
        assert_eq!(shape("typeof typeof a"), "(typeof (typeof a))");
        // A prefix operator binds tighter than any binary one.
        assert_eq!(shape("-a + b"), "(+ (- a) b)");
        assert_eq!(shape("-a * b"), "(* (- a) b)");
        assert_eq!(shape("typeof a === b"), "(=== (typeof a) b)");
        // The span runs from the operator to the end of its operand.
        assert_eq!(parse("- a").span, Span::new(0, 3));
    }
    #[test]
    fn the_precedence_ladder_is_the_grammars_nesting_read_as_numbers() {
        // Each pair is two adjacent layers of §13.6 – §13.13, checked in both orders so that a
        // table entry cannot be right by accident of which side it was written on.
        // (`??` against `||` is absent on purpose: §13.13 forbids that pair outright, and it
        // has its own test.)
        for (source, shaped) in [
            ("a || b && c", "(|| a (&& b c))"),
            ("a && b || c", "(|| (&& a b) c)"),
            ("a && b | c", "(&& a (| b c))"),
            ("a | b && c", "(&& (| a b) c)"),
            ("a | b ^ c", "(| a (^ b c))"),
            ("a ^ b | c", "(| (^ a b) c)"),
            ("a ^ b & c", "(^ a (& b c))"),
            ("a & b ^ c", "(^ (& a b) c)"),
            ("a & b == c", "(& a (== b c))"),
            ("a == b & c", "(& (== a b) c)"),
            ("a == b < c", "(== a (< b c))"),
            ("a < b == c", "(== (< a b) c)"),
            ("a < b << c", "(< a (<< b c))"),
            ("a << b < c", "(< (<< a b) c)"),
            ("a << b + c", "(<< a (+ b c))"),
            ("a + b << c", "(<< (+ a b) c)"),
            ("a + b * c", "(+ a (* b c))"),
            ("a * b + c", "(+ (* a b) c)"),
            ("a * b ** c", "(* a (** b c))"),
            ("a ** b * c", "(* (** a b) c)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // The relational layer holds the two word-shaped operators as well as the symbols.
        assert_eq!(shape("a instanceof b == c"), "(== (instanceof a b) c)");
        assert_eq!(shape("a in b == c"), "(== (in a b) c)");
        assert_eq!(shape("a + b instanceof c"), "(instanceof (+ a b) c)");
        // Every remaining operator, so no table entry goes unexercised.
        for (source, shaped) in [
            ("a - b", "(- a b)"),
            ("a / b", "(/ a b)"),
            ("a % b", "(% a b)"),
            ("a >> b", "(>> a b)"),
            ("a >>> b", "(>>> a b)"),
            ("a > b", "(> a b)"),
            ("a <= b", "(<= a b)"),
            ("a >= b", "(>= a b)"),
            ("a != b", "(!= a b)"),
            ("a === b", "(=== a b)"),
            ("a !== b", "(!== a b)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // Parentheses override all of it, which is the only reason precedence is bearable.
        assert_eq!(shape("(a + b) * c"), "(* (+ a b) c)");
    }
    #[test]
    fn everything_groups_to_the_left_except_exponentiation() {
        // `AdditiveExpression : AdditiveExpression + MultiplicativeExpression` — the recursion is
        // on the left, so equal precedence groups left.
        assert_eq!(shape("a - b - c"), "(- (- a b) c)");
        assert_eq!(shape("a / b / c"), "(/ (/ a b) c)");
        assert_eq!(shape("a < b < c"), "(< (< a b) c)");
        assert_eq!(shape("a && b && c"), "(&& (&& a b) c)");
        assert_eq!(shape("a ?? b ?? c"), "(?? (?? a b) c)");
        // `ExponentiationExpression : UpdateExpression ** ExponentiationExpression` — the
        // recursion is on the *right*, so `2 ** 3 ** 2` is 512 and not 64.
        assert_eq!(shape("a ** b ** c"), "(** a (** b c))");
        assert_eq!(shape("a ** b ** c ** d"), "(** a (** b (** c d)))");
        // A left-associative chain is a loop rather than a recursion, so its length is bounded by
        // memory rather than by MAX_NESTING_DEPTH.
        let long = vec!["1"; 5000].join(" + ");
        assert!(parse_expression(&long).is_ok());
    }
    #[test]
    fn a_prefix_unary_may_not_be_the_left_operand_of_exponentiation() {
        // §13.6: `ExponentiationExpression : UpdateExpression ** ExponentiationExpression`. A
        // prefix unary is not an `UpdateExpression`, so `-a ** b` has no derivation at all — the
        // rule exists because a reader cannot tell whether it would mean `(-a) ** b` or
        // `-(a ** b)`, and those differ.
        for source in [
            "-a ** b",
            "+a ** b",
            "!a ** b",
            "~a ** b",
            "typeof a ** b",
            "void a ** b",
            "delete a ** b",
        ] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::ExponentiationOnUnary,
                "on {source:?}"
            );
        }
        // The caret goes under the operand that is wrong, not the operator that noticed.
        assert_eq!(error("-a ** b").span, Span::new(0, 2));
        // Both ways of saying what you meant are fine.
        assert_eq!(shape("(-a) ** b"), "(** (- a) b)");
        assert_eq!(shape("-(a ** b)"), "(- (** a b))");
        // The restriction is on the *left* operand only: the right is an
        // `ExponentiationExpression`, which a `UnaryExpression` is.
        assert_eq!(shape("a ** -b"), "(** a (- b))");
        assert_eq!(shape("a ** typeof b"), "(** a (typeof b))");
        // …and only `**` is restricted. Every other operator takes a bare unary on the left.
        assert_eq!(shape("-a * b"), "(* (- a) b)");
        assert_eq!(shape("-a + b"), "(+ (- a) b)");
    }
    #[test]
    fn coalescing_may_not_be_mixed_with_the_boolean_operators_without_parentheses() {
        // §13.13: `CoalesceExpressionHead` admits a `CoalesceExpression` or a
        // `BitwiseORExpression` and nothing else, and `ShortCircuitExpression` keeps the two
        // families apart in the other direction. Both orders are errors, and for the same reason
        // as `**`: nobody would agree on what the unbracketed form meant.
        for source in ["a || b ?? c", "a ?? b || c", "a && b ?? c", "a ?? b && c"] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::MixedCoalesceAndLogical,
                "on {source:?}"
            );
        }
        // The caret goes under the operand from the wrong family.
        assert_eq!(error("a || b ?? c").span, Span::new(0, 6));
        assert_eq!(error("a ?? b || c").span, Span::new(5, 11));
        // Parentheses settle it, in either direction.
        assert_eq!(shape("(a || b) ?? c"), "(?? (|| a b) c)");
        assert_eq!(shape("a ?? (b || c)"), "(?? a (|| b c))");
        assert_eq!(shape("(a ?? b) || c"), "(|| (?? a b) c)");
        assert_eq!(shape("a || (b ?? c)"), "(|| a (?? b c))");
        // `&&` and `||` mix with each other freely — the rule is not symmetric, and a check that
        // rejected `a || b && c` would be rejecting ordinary JavaScript.
        assert_eq!(shape("a || b && c"), "(|| a (&& b c))");
        assert_eq!(shape("a && b || c"), "(|| (&& a b) c)");
        // …and `??` chains with itself, since `CoalesceExpressionHead` may be a
        // `CoalesceExpression`.
        assert_eq!(shape("a ?? b ?? c"), "(?? (?? a b) c)");
        // A `??` whose operand is an ordinary binary expression is fine: the boundary is the
        // boolean operators, not precedence in general.
        assert_eq!(shape("a ?? b + c"), "(?? a (+ b c))");
        assert_eq!(shape("a | b ?? c"), "(?? (| a b) c)");
    }
    #[test]
    fn a_conditional_takes_an_assignment_in_each_arm_and_a_comma_in_neither() {
        // §13.14: `ShortCircuitExpression ? AssignmentExpression : AssignmentExpression`. The
        // Note explains the choice — it lets an assignment be governed by either arm and keeps a
        // comma expression out of the middle, which C and Java do not.
        assert_eq!(shape("a ? b : c"), "(? a b c)");
        assert_eq!(shape("a ? b = 1 : c = 2"), "(? a (= b 1) (= c 2))");
        // The test is a ShortCircuitExpression, so everything below it binds tighter.
        assert_eq!(shape("a || b ? c : d"), "(? (|| a b) c d)");
        assert_eq!(shape("a + b ? c : d"), "(? (+ a b) c d)");
        // Nesting: the alternate is an AssignmentExpression, so a chain groups to the right —
        // which is what makes `a ? b : c ? d : e` mean what everyone assumes it means.
        assert_eq!(shape("a ? b : c ? d : e"), "(? a b (? c d e))");
        assert_eq!(shape("a ? b ? c : d : e"), "(? a (? b c d) e)");
        // A comma is not an AssignmentExpression, so it cannot appear in an arm unbracketed.
        assert_eq!(
            error("a ? b, c : d").kind,
            ParseErrorKind::Unexpected {
                expected: "`:`",
                found: TokenKind::Comma,
            }
        );
        assert_eq!(shape("a ? (b, c) : d"), "(? a (, b c) d)");
        assert_eq!(
            error("a ? b").kind,
            ParseErrorKind::Unexpected {
                expected: "`:`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(parse("a ? b : c").span, Span::new(0, 9));
        // The bracketing flag is part of what a node claims about itself, and nothing else
        // in this slice reads it — so it is asserted here rather than left to a later one
        // that will (arrow functions cover `(a, b)`, and `delete (x, y)` turns on it).
        assert!(!parse("a ? b : c").parenthesized);
        assert!(parse("(a ? b : c)").parenthesized);
    }
    #[test]
    fn assignment_groups_to_the_right_and_only_targets_what_can_be_assigned_to() {
        // §13.15: `LeftHandSideExpression = AssignmentExpression` — the recursion is on the
        // right, so a chain groups that way and `a = b = c` gives both `a` and `b` the value.
        assert_eq!(shape("a = b"), "(= a b)");
        assert_eq!(shape("a = b = c"), "(= a (= b c))");
        // Every operator §13.15 lists, including the three with their own productions.
        for (source, shaped) in [
            ("a += b", "(+= a b)"),
            ("a -= b", "(-= a b)"),
            ("a *= b", "(*= a b)"),
            ("a /= b", "(/= a b)"),
            ("a %= b", "(%= a b)"),
            ("a **= b", "(**= a b)"),
            ("a <<= b", "(<<= a b)"),
            ("a >>= b", "(>>= a b)"),
            ("a >>>= b", "(>>>= a b)"),
            ("a &= b", "(&= a b)"),
            ("a ^= b", "(^= a b)"),
            ("a |= b", "(|= a b)"),
            ("a &&= b", "(&&= a b)"),
            ("a ||= b", "(||= a b)"),
            ("a ??= b", "(??= a b)"),
        ] {
            assert_eq!(shape(source), shaped, "parsing {source:?}");
        }
        // The value is a whole AssignmentExpression, so everything binds tighter than `=`.
        assert_eq!(shape("a = b + c"), "(= a (+ b c))");
        assert_eq!(shape("a += b ? c : d"), "(+= a (? b c d))");
        assert_eq!(shape("a = b || c"), "(= a (|| b c))");

        // §13.15.1: a Syntax Error if the AssignmentTargetType is not simple. Refused here
        // rather than at run time, which is the difference between a program that never starts
        // and one that fails halfway through.
        for source in [
            "1 = 2",
            "'a' = 2",
            "this = 1",
            "null = 1",
            "true = 1",
            "a + b = c",
            "-a = b",
            "(a, b) = 1",
            "(a ? b : c) = d",
            "a ?? b = c",
            "/x/ = 1",
            "1 += 2",
            "1 &&= 2",
        ] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::InvalidAssignmentTarget,
                "on {source:?}"
            );
        }
        // The caret goes under the thing that cannot be assigned to.
        assert_eq!(error("a + b = c").span, Span::new(0, 5));
        // Parentheses do not change the answer, and need no rule of their own: `(a)` *is* the
        // identifier, because bracketing is a flag rather than a node.
        assert_eq!(shape("(a) = 1"), "(= a 1)");
        assert_eq!(shape("((a)) = 1"), "(= a 1)");
        assert!(matches!(
            parse("(a) = 1").kind,
            ExprKind::Assignment { ref target, .. } if target.parenthesized
        ));
        assert_eq!(parse("a = b").span, Span::new(0, 5));
        assert!(!parse("a = b").parenthesized);
        assert!(parse("(a = b)").parenthesized);
        // `&&=`, `||=` and `??=` do not always assign — `a ||= b` leaves `a` alone when it is
        // truthy — which is why the operator says so rather than the compiler guessing.
        assert!(AssignmentOperator::LogicalOr.short_circuits());
        assert!(AssignmentOperator::LogicalAnd.short_circuits());
        assert!(AssignmentOperator::NullishCoalescing.short_circuits());
        assert!(!AssignmentOperator::Assign.short_circuits());
        assert!(!AssignmentOperator::Add.short_circuits());
    }
    #[test]
    fn the_comma_operator_is_the_loosest_thing_there_is_and_is_held_flat() {
        // §13.16: `Expression : Expression , AssignmentExpression`.
        assert_eq!(shape("a, b"), "(, a b)");
        // Flat rather than nested pairs: the grammar's recursion is on the left, so pairs would
        // nest once per comma for no gain — evaluation is left to right either way.
        assert_eq!(shape("a, b, c"), "(, a b c)");
        assert_eq!(shape("a, b, c, d"), "(, a b c d)");
        // …but a bracketed sequence is its own node, so flattening cannot cross parentheses.
        assert_eq!(shape("(a, b), c"), "(, (, a b) c)");
        assert_eq!(shape("a, (b, c)"), "(, a (, b c))");
        // Loosest of all: everything else binds tighter.
        assert_eq!(shape("a = 1, b = 2"), "(, (= a 1) (= b 2))");
        assert_eq!(shape("a ? b : c, d"), "(, (? a b c) d)");
        assert_eq!(shape("a + b, c * d"), "(, (+ a b) (* c d))");
        assert_eq!(parse("a, b").span, Span::new(0, 4));
        assert!(!parse("a, b").parenthesized);
        assert!(parse("(a, b)").parenthesized);
        // A long list is a loop rather than a recursion, so its length is bounded by memory.
        let long = vec!["a"; 5000].join(", ");
        assert!(parse_expression(&long).is_ok());
        // A trailing comma is not part of the operator — that is argument-list syntax.
        assert_eq!(
            error("a, b,").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Eof,
            }
        );
    }
}
