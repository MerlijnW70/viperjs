//! The expression grammar (ECMAScript §13), from `Expression` down to `PrimaryExpression`.
//!
//! Every function here is on the path a bracket recurses through, so every one of them costs
//! nesting depth — which is why `parse_assignment` also handles the conditional operator, why
//! the non-recursive `PrimaryExpression` forms live in their own function, and why the operator
//! tables are next door in [`super::operator`] rather than being a cascade of layers. See
//! [`super::MAX_NESTING_DEPTH`] and DR-0006.

use super::operator::{
    OperatorKind, assignment_operator, binary_operator, combine, is_simple_assignment_target,
    unary_operator,
};
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{BinaryOperator, Expr, ExprKind, RegExpLiteral};
use crate::lexer::{
    Goal, ReservedWord, Token, TokenKind, identifier_value, numeric_value, regexp_parts,
    string_value,
};
use crate::span::Span;

impl<'a> Parser<'a> {
    /// `Expression` (§13.16) — one or more `AssignmentExpression`s separated by commas.
    pub(super) fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_assignment()?;
        if self.current.kind != TokenKind::Comma {
            return Ok(first);
        }
        let mut parts = vec![first];
        while self.current.kind == TokenKind::Comma {
            self.advance(Goal::RegExp)?;
            parts.push(self.parse_assignment()?);
        }
        let span = match (parts.first(), parts.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            // `parts` was built from one element and only ever grows, so this cannot happen;
            // an empty span is the answer that costs nothing if it somehow does.
            _ => Span::empty_at(self.current.span.start),
        };
        Ok(Expr {
            kind: ExprKind::Sequence(parts),
            span,
            parenthesized: false,
        })
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
    pub(super) fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_binary(0)?;
        if self.current.kind == TokenKind::Question {
            left = self.parse_conditional_tail(left)?;
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
        let value = self.parse_assignment();
        self.leave();
        let value = value?;
        Ok(Expr {
            span: left.span.to(value.span),
            kind: ExprKind::Assignment {
                operator,
                target: Box::new(left),
                value: Box::new(value),
            },
            parenthesized: false,
        })
    }

    /// `? AssignmentExpression : AssignmentExpression` (§13.14), with `test` already parsed.
    ///
    /// Both arms are `AssignmentExpression`s, which §13.14's own Note explains: it lets an
    /// assignment be governed by either arm, and keeps a comma expression out of the middle.
    /// So `a ? b = 1 : c = 2` is two assignments, and `a ? b, c : d` is an error.
    fn parse_conditional_tail(&mut self, test: Expr) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let arms = self.parse_conditional_arms();
        self.leave();
        let (consequent, alternate) = arms?;
        Ok(Expr {
            span: test.span.to(alternate.span),
            kind: ExprKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
            parenthesized: false,
        })
    }

    /// The two arms of a conditional, split out so their locals do not sit in a frame that the
    /// bracket recursion also passes through.
    fn parse_conditional_arms(&mut self) -> Result<(Expr, Expr), ParseError> {
        let consequent = self.parse_assignment()?;
        self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
        let alternate = self.parse_assignment()?;
        Ok((consequent, alternate))
    }

    /// The operator layers of §13.6 – §13.13, by precedence climbing.
    ///
    /// `minimum` is the weakest binding power this call will accept; an operator weaker than it
    /// belongs to the caller. Left-associative operators recurse one level tighter so the next
    /// one of equal precedence is left for the loop, and `**` recurses at its own level so the
    /// next one is taken by the recursion instead — which is the whole of associativity.
    fn parse_binary(&mut self, minimum: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some(operator) = binary_operator(self.current.kind) {
            if operator.precedence < minimum {
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
            let right = self.parse_binary(tighter);
            self.leave();
            let right = right?;
            left = combine(left, operator, right)?;
        }
        Ok(left)
    }

    /// `UnaryExpression` (§13.5), or whatever it falls through to.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let Some(operator) = unary_operator(self.current.kind) else {
            return self.parse_primary();
        };
        let token = self.advance(Goal::RegExp)?;
        self.enter()?;
        // `- UnaryExpression`, so the operators stack: `- - a` is two of them.
        let argument = self.parse_unary();
        self.leave();
        let argument = argument?;
        Ok(Expr {
            span: token.span.to(argument.span),
            kind: ExprKind::Unary {
                operator,
                argument: Box::new(argument),
            },
            parenthesized: false,
        })
    }

    /// `PrimaryExpression` (§13.2), for the forms that need no other production.
    ///
    /// Only the one recursive production lives in this frame. Everything else is next door in
    /// [`Parser::parse_atom`], because a debug build gives every local in a function its own
    /// stack slot and does not reuse them between match arms — so an arm that never recurses
    /// still costs its slots once per level of nesting. Moving them out roughly halved the stack
    /// a deep parse needs, which is not a speed argument: it is how many legitimate programs
    /// [`MAX_NESTING_DEPTH`] can afford to accept.
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.current;
        if token.kind != TokenKind::LParen {
            return self.parse_atom(token);
        }
        // `( Expression )`. The `(` is advanced past under `Goal::RegExp` because an operand
        // follows it, and the `)` under `Goal::Div` because the bracketed expression is one.
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let inner = self.parse_expression();
        self.leave();
        // The inner failure is reported before the missing `)` is looked for, and the order
        // matters: whatever went wrong inside the brackets happened first and is what the reader
        // needs to see. Checking for the closing bracket first turns every error inside a
        // bracketed expression into "expected `)`" — including, absurdly, the depth cap.
        let inner = inner?;
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok(inner.in_parentheses(token.span.to(close.span)))
    }

    /// The `PrimaryExpression` forms that contain no other expression.
    fn parse_atom(&mut self, token: Token) -> Result<Expr, ParseError> {
        let literal = |kind| {
            Ok(Expr {
                kind,
                span: token.span,
                parenthesized: false,
            })
        };
        match token.kind {
            TokenKind::Keyword(ReservedWord::This) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::This)
            }
            TokenKind::Keyword(ReservedWord::Null) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Null)
            }
            TokenKind::Keyword(ReservedWord::True) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(true))
            }
            TokenKind::Keyword(ReservedWord::False) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(false))
            }
            // An `Identifier` is an `IdentifierName` that is not a `ReservedWord` — and the lexer
            // has already made that distinction, contextual keywords included.
            TokenKind::Identifier { .. } => {
                self.advance(Goal::Div)?;
                let name = identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::Identifier(name.into_owned()))
            }
            TokenKind::Number { .. } => {
                self.advance(Goal::Div)?;
                let value = numeric_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::Number(value))
            }
            TokenKind::String { .. } => {
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::String(value))
            }
            TokenKind::RegExp => {
                self.advance(Goal::Div)?;
                let parts = regexp_parts(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                let text = |span: Span| span.slice(self.source).unwrap_or_default().to_string();
                literal(ExprKind::RegExp(Box::new(RegExpLiteral {
                    body: text(parts.body),
                    flags: text(parts.flags),
                })))
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    /// The error for a token whose value the lexer produced but this parser cannot read back.
    ///
    /// Unreachable in principle — the value functions accept every span the lexer hands out — but
    /// the types do not say so, and the alternative to an error here is an `unwrap` that DR-0002
    /// forbids. It reports the token as unexpected, which is what it has become.
    fn value_missing(&self, token: Token) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected: "a literal this parser can read",
                found: token.kind,
            },
            span: token.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AssignmentOperator;
    use crate::parser::parse_expression;
    use crate::parser::test_support::*;
    #[test]
    fn every_primary_expression_the_grammar_reaches_today() {
        assert_eq!(parse("this").kind, ExprKind::This);
        assert_eq!(parse("null").kind, ExprKind::Null);
        assert_eq!(parse("true").kind, ExprKind::Boolean(true));
        assert_eq!(parse("false").kind, ExprKind::Boolean(false));
        assert_eq!(parse("1").kind, ExprKind::Number(1.0));
        assert_eq!(parse("0x10").kind, ExprKind::Number(16.0));
        assert_eq!(parse("1e3").kind, ExprKind::Number(1000.0));
        assert_eq!(parse("'hi'").kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse(r#""hi""#).kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse("x").kind, ExprKind::Identifier("x".to_string()));
        // The value is the cooked one, so an escaped name and a plain one give the same node.
        assert_eq!(parse(r"x").kind, ExprKind::Identifier("x".to_string()));
        // Contextual keywords are identifiers, which is the whole reason the lexer refused to
        // decide: `let` and `of` are ordinary names until a grammatical context says otherwise.
        assert_eq!(parse("let").kind, ExprKind::Identifier("let".to_string()));
        assert_eq!(parse("of").kind, ExprKind::Identifier("of".to_string()));
        assert_eq!(
            parse("async").kind,
            ExprKind::Identifier("async".to_string())
        );
        // …while a genuine reserved word is not an expression at all.
        assert_eq!(
            error("var").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Keyword(ReservedWord::Var),
            }
        );
        // Spans cover exactly the construct.
        assert_eq!(parse("  1  ").span, Span::new(2, 3));
        assert_eq!(parse("this").span, Span::new(0, 4));
    }
    #[test]
    fn a_slash_at_the_start_of_an_expression_opens_a_literal() {
        // The goal symbol, from the other side of the handoff. `Parser::new` reads the first
        // token under `Goal::RegExp` because a program begins where an operand may stand — so
        // this is a regular expression and not the start of a division.
        assert_eq!(parse("/ab+/gi").kind, regexp("ab+", "gi"));
        // The escaped slash and the character class stay in the body, since the lexer found the
        // real closing slash.
        assert_eq!(parse(r"/a\/[/]b/").kind, regexp(r"a\/[/]b", ""));
        // Empty flags are an empty string rather than a missing one.
        assert_eq!(parse("/x/").kind, regexp("x", ""));
        // …and inside parentheses, where an operand may also stand.
        assert!(matches!(parse("(/x/)").kind, ExprKind::RegExp(_)));
    }
    #[test]
    fn a_slash_after_an_operand_divides_and_the_next_one_may_still_open_a_literal() {
        // The other half of the goal invariant, now that there is a binary expression to parse
        // the division into. `a /b/ g` is two divisions, and it is the parser's choice of goal
        // that makes it so — a lexer guessing from the previous token would have to get this
        // right by luck.
        assert_eq!(shape("a / b"), "(/ a b)");
        assert_eq!(shape("a /b/ g"), "(/ (/ a b) g)");
        // An operator is followed by an operand, so the goal after one is `RegExp` again: this
        // really is `a` divided by a regular expression, which is legal and rare.
        assert_eq!(shape("a / /b/"), "(/ a /b/)");
        assert_eq!(shape("typeof /b/"), "(typeof /b/)");
        assert_eq!(shape("1 + /b/g"), "(+ 1 /b/g)");
    }
    #[test]
    fn parentheses_are_recorded_without_becoming_a_node() {
        let bracketed = parse("(1)");
        assert_eq!(bracketed.kind, ExprKind::Number(1.0));
        assert!(bracketed.parenthesized);
        assert_eq!(
            bracketed.span,
            Span::new(0, 3),
            "the span covers the brackets"
        );
        // …and the same expression without them is not marked.
        assert!(!parse("1").parenthesized);
        // Nesting them changes only the span: no rule counts brackets.
        let twice = parse("((1))");
        assert!(twice.parenthesized);
        assert_eq!(twice.kind, ExprKind::Number(1.0));
        assert_eq!(twice.span, Span::new(0, 5));
        assert_eq!(parse(" ( 1 ) ").span, Span::new(1, 6));
        // An empty pair is not an expression — `()` is only meaningful as an arrow parameter
        // list, which is a cover grammar this parser does not reach yet.
        assert_eq!(
            error("()").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::RParen,
            }
        );
        assert_eq!(
            error("(1").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(
            error("(1 2)").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Number { legacy: false },
            }
        );
    }
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
