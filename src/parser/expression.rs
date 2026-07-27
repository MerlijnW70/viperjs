//! The expression grammar (ECMAScript §13), from `Expression` down to `PrimaryExpression`.
//!
//! Every function here is on the path a bracket recurses through, so every one of them costs
//! nesting depth — which is why `parse_assignment` also handles the conditional operator, why
//! the non-recursive `PrimaryExpression` forms live in their own function, and why the operator
//! tables are next door in [`super::operator`] rather than being a cascade of layers. See
//! [`super::MAX_NESTING_DEPTH`] and DR-0006.

use super::operator::{
    OperatorKind, assignment_operator, binary_operator, combine, is_simple_assignment_target,
    unary_operator, update_operator,
};
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{BinaryOperator, Expr, ExprKind, RegExpLiteral, UpdateOperator};
use crate::lexer::{
    Goal, ReservedWord, Token, TokenKind, identifier_value, numeric_value, regexp_parts,
    string_value,
};
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

impl<'a> Parser<'a> {
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
        Ok(Expr {
            kind: ExprKind::Sequence(parts.into_boxed_slice()),
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
            return Ok(Expr {
                span: token.span.to(argument.span),
                kind: ExprKind::Unary {
                    operator,
                    argument: Box::new(argument),
                },
                parenthesized: false,
            });
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
        Ok(Expr {
            span: operator_span.to(argument.span),
            kind: ExprKind::Update {
                operator,
                prefix,
                argument: Box::new(argument),
            },
            parenthesized: false,
        })
    }

    /// `LeftHandSideExpression` (§13.3): a primary expression with member accesses and calls
    /// hung off it, or a `new` expression.
    ///
    /// `allow_call` is what keeps `new a.b()` from giving the arguments to `a.b`. §13.3 reaches
    /// the same place by having `new MemberExpression Arguments` take a `MemberExpression`,
    /// which has no call production in it — so a `new` parses its callee with calls switched
    /// off, and the first argument list it then sees is its own. Callers wanting the whole
    /// production pass `true`; there is no wrapper function saying so, because a wrapper is a
    /// stack frame and this one would sit on the bracket-recursion path.
    ///
    /// Each suffix is built in its own function rather than in an arm here, and that is worth
    /// four hundred lines of nesting depth. A debug build gives every local its own stack slot
    /// and reuses none between match arms, so three arms written inline would all be paid for on
    /// every bracket — including `((((1))))`, which takes no suffix at all. Measured: the inline
    /// form cost a mebibyte 64 levels of nesting for suffixes it never used.
    fn parse_member(&mut self, allow_call: bool) -> Result<Expr, ParseError> {
        let mut expr = if self.current.kind == TokenKind::Keyword(ReservedWord::New) {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        loop {
            expr = match self.current.kind {
                TokenKind::Dot => self.member_after_dot(expr)?,
                TokenKind::LBracket => self.computed_member_after(expr)?,
                TokenKind::LParen if allow_call => self.call_after(expr)?,
                _ => return Ok(expr),
            };
        }
    }

    /// `MemberExpression . IdentifierName`, with the cursor on the `.`.
    fn member_after_dot(&mut self, object: Expr) -> Result<Expr, ParseError> {
        self.advance(Goal::Div)?;
        let (property, end) = self.parse_property_name()?;
        Ok(Expr {
            span: object.span.to(end),
            kind: ExprKind::Member {
                object: Box::new(object),
                property,
            },
            parenthesized: false,
        })
    }

    /// `MemberExpression [ Expression ]`, with the cursor on the `[`.
    fn computed_member_after(&mut self, object: Expr) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // `[ Expression[+In] ]` — a bracket starts afresh, which is why `for (a[b in c];;)`
        // parses.
        let property = self.parse_expression(AllowIn::Yes);
        self.leave();
        let property = property?;
        let close = self.eat(TokenKind::RBracket, Goal::Div, "`]`")?;
        Ok(Expr {
            span: object.span.to(close.span),
            kind: ExprKind::ComputedMember {
                object: Box::new(object),
                property: Box::new(property),
            },
            parenthesized: false,
        })
    }

    /// `CallExpression Arguments`, with the cursor on the `(`.
    fn call_after(&mut self, callee: Expr) -> Result<Expr, ParseError> {
        let (arguments, end) = self.parse_arguments()?;
        Ok(Expr {
            span: callee.span.to(end),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
            parenthesized: false,
        })
    }

    /// `new MemberExpression Arguments` or `new NewExpression` (§13.3).
    ///
    /// Not on the bracket-recursion path — it is entered only for a literal `new` — so its
    /// locals cost nothing when there is none.
    fn parse_new(&mut self) -> Result<Expr, ParseError> {
        let token = self.advance(Goal::RegExp)?;
        self.enter()?;
        let callee = self.parse_member(false);
        self.leave();
        let callee = callee?;
        // `new a()()` is a call on `new a()`, because the first argument list belongs to the
        // `new` and the loop in `parse_member` takes the second.
        let (arguments, end) = if self.current.kind == TokenKind::LParen {
            self.parse_arguments()?
        } else {
            (Vec::new().into_boxed_slice(), callee.span)
        };
        Ok(Expr {
            span: token.span.to(end),
            kind: ExprKind::New {
                callee: Box::new(callee),
                arguments,
            },
            parenthesized: false,
        })
    }

    /// `Arguments` (§13.3), with the cursor on the `(`. Returns them and the closing span.
    ///
    /// A trailing comma is allowed — `Arguments : ( ArgumentList , )` — but an empty list with
    /// one is not, since `ArgumentList` needs at least one argument to trail.
    fn parse_arguments(&mut self) -> Result<(Box<[Expr]>, Span), ParseError> {
        self.advance(Goal::RegExp)?;
        let mut arguments = Vec::new();
        while self.current.kind != TokenKind::RParen {
            self.enter()?;
            let argument = self.parse_assignment(AllowIn::Yes);
            self.leave();
            arguments.push(argument?);
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
        }
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok((arguments.into_boxed_slice(), close.span))
    }

    /// The `IdentifierName` after a `.`, and the span it covers.
    ///
    /// An `IdentifierName`, note, not an `Identifier`: every reserved word is a legal property
    /// name, so `a.if` and `a.new` are ordinary accesses. An escaped spelling works too, and
    /// means what it spells — `a.\u0069f` is `a.if` — which falls out of the lexer having
    /// classified it as an identifier rather than a keyword.
    fn parse_property_name(&mut self) -> Result<(Box<str>, Span), ParseError> {
        let token = self.current;
        match token.kind {
            TokenKind::Identifier { .. } => {
                self.advance(Goal::Div)?;
                let name = identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok((name.into_owned().into_boxed_str(), token.span))
            }
            TokenKind::Keyword(word) => {
                self.advance(Goal::Div)?;
                Ok((word.as_str().into(), token.span))
            }
            _ => Err(self.unexpected("a property name")),
        }
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
        let inner = self.parse_expression(AllowIn::Yes);
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
    pub(super) fn value_missing(&self, token: Token) -> ParseError {
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
    fn member_access_chains_and_takes_any_identifier_name_after_the_dot() {
        assert_eq!(shape("a.b"), "(. a b)");
        assert_eq!(shape("a.b.c"), "(. (. a b) c)");
        assert_eq!(shape("a[b]"), "([] a b)");
        assert_eq!(shape("a[b][c]"), "([] ([] a b) c)");
        assert_eq!(shape("a.b[c].d"), "(. ([] (. a b) c) d)");
        // The computed form takes a whole `Expression`, brackets and all.
        assert_eq!(shape("a[b + c]"), "([] a (+ b c))");
        assert_eq!(shape("a[b, c]"), "([] a (, b c))");
        // §13.3 says `MemberExpression . IdentifierName`, not `Identifier` — so every reserved
        // word is a legal property name. A word is only reserved where a binding could stand.
        assert_eq!(shape("a.if"), "(. a if)");
        assert_eq!(shape("a.new"), "(. a new)");
        assert_eq!(shape("a.class"), "(. a class)");
        assert_eq!(shape("a.default"), "(. a default)");
        // …and an escaped spelling means what it spells, which falls out of the lexer having
        // classified it as an identifier rather than a keyword.
        assert_eq!(shape(r"a.\u0069f"), "(. a if)");
        // Member access binds tighter than any operator.
        assert_eq!(shape("a.b + c.d"), "(+ (. a b) (. c d))");
        assert_eq!(shape("-a.b"), "(- (. a b))");
        assert_eq!(parse("a.b").span, Span::new(0, 3));
        assert_eq!(parse("a[b]").span, Span::new(0, 4));
        // The bracketing flag, which nothing in this slice reads and later ones will.
        assert!(!parse("a.b").parenthesized);
        assert!(parse("(a.b)").parenthesized);
        assert!(!parse("a[b]").parenthesized);
        assert!(parse("(a[b])").parenthesized);
        // A property name has to be there, and has to be a name.
        assert_eq!(
            error("a.").kind,
            ParseErrorKind::Unexpected {
                expected: "a property name",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(
            error("a.+").kind,
            ParseErrorKind::Unexpected {
                expected: "a property name",
                found: TokenKind::Plus,
            }
        );
        // `a.1` is not this error, and the reason is worth knowing: a token is as long as
        // possible, so `.1` is a single `NumericLiteral` and the `.` never reaches the parser as
        // a punctuator at all. The complaint is therefore about a stray number — which is what
        // every other engine says too.
        assert_eq!(
            error("a.1").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::Number { legacy: false },
            }
        );
        assert_eq!(shape("a.b1"), "(. a b1)");
        assert_eq!(
            error("a[b").kind,
            ParseErrorKind::Unexpected {
                expected: "`]`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(
            error("a[]").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::RBracket,
            }
        );
    }

    #[test]
    fn a_call_takes_an_argument_list_and_chains_with_everything_else() {
        assert_eq!(shape("f()"), "(call f [])");
        assert_eq!(shape("f(a)"), "(call f [a])");
        assert_eq!(shape("f(a, b, c)"), "(call f [a b c])");
        assert_eq!(shape("f()()"), "(call (call f []) [])");
        assert_eq!(shape("f().g()"), "(call (. (call f []) g) [])");
        assert_eq!(shape("a.b(c)"), "(call (. a b) [c])");
        assert_eq!(shape("f(a)[b]"), "([] (call f [a]) b)");
        // `Arguments : ( ArgumentList , )` — a trailing comma is allowed, but only after an
        // argument, since `ArgumentList` needs one to trail.
        assert_eq!(shape("f(a,)"), "(call f [a])");
        assert_eq!(shape("f(a, b,)"), "(call f [a b])");
        assert_eq!(
            error("f(,)").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Comma,
            }
        );
        // Each argument is an `AssignmentExpression`, so a comma separates rather than
        // sequences: two arguments here, and one when it is bracketed.
        assert_eq!(shape("f(a, b)"), "(call f [a b])");
        assert_eq!(shape("f((a, b))"), "(call f [(, a b)])");
        assert_eq!(shape("f(a = 1)"), "(call f [(= a 1)])");
        assert_eq!(shape("f(a ? b : c)"), "(call f [(? a b c)])");
        assert_eq!(parse("f(a)").span, Span::new(0, 4));
        assert!(!parse("f(a)").parenthesized);
        assert!(parse("(f(a))").parenthesized);
        assert_eq!(
            error("f(a").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Eof,
            }
        );
    }

    #[test]
    fn new_claims_the_first_argument_list_and_leaves_the_rest_to_the_call() {
        // §13.3 gives `new` a `MemberExpression`, which has no call production in it — so the
        // callee is parsed with calls switched off and the first `(` it then meets is `new`'s.
        assert_eq!(shape("new a"), "(new a [])");
        assert_eq!(
            shape("new a()"),
            "(new a [])",
            "the same node, written two ways"
        );
        assert_eq!(shape("new a(1, 2)"), "(new a [1 2])");
        assert_eq!(
            shape("new a.b()"),
            "(new (. a b) [])",
            "the arguments are `new`'s"
        );
        assert_eq!(shape("new a.b.c()"), "(new (. (. a b) c) [])");
        assert_eq!(shape("new a[b]()"), "(new ([] a b) [])");
        // …and the second list is a call on what `new` produced.
        assert_eq!(shape("new a()()"), "(call (new a []) [])");
        assert_eq!(shape("new a().b"), "(. (new a []) b)");
        assert_eq!(shape("new new a()()"), "(new (new a []) [])");
        assert_eq!(shape("new new a()"), "(new (new a []) [])");
        assert_eq!(parse("new a()").span, Span::new(0, 7));
        assert_eq!(parse("new a").span, Span::new(0, 5));
        assert!(!parse("new a").parenthesized);
        assert!(parse("(new a)").parenthesized);
    }

    #[test]
    fn an_update_expression_will_not_reach_across_a_line_break() {
        assert_eq!(shape("++a"), "(pre++ a)");
        assert_eq!(shape("--a"), "(pre-- a)");
        assert_eq!(shape("a++"), "(post++ a)");
        assert_eq!(shape("a--"), "(post-- a)");
        assert_eq!(shape("a.b++"), "(post++ (. a b))");
        assert_eq!(shape("a[b]++"), "(post++ ([] a b))");
        assert_eq!(shape("a++ + b"), "(+ (post++ a) b)");
        assert_eq!(shape("++a + b"), "(+ (pre++ a) b)");

        // `UpdateExpression : LeftHandSideExpression [no LineTerminator here] ++`, the first
        // restricted production this parser meets — and the reason every token has carried a
        // `newline_before` flag since the lexer's first slice. Across a line break the postfix
        // form does not match at all, which leaves two things where there was nearly one.
        assert_eq!(
            error("a\n++b").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::PlusPlus,
            },
            "`a` and `++b`, not `a++` and `b`"
        );
        // …and on one line it does match, which is what makes the two cases differ.
        assert_eq!(shape("a\t++"), "(post++ a)");
        assert_eq!(
            error("a /* comment */\n++b").kind,
            ParseErrorKind::Unexpected {
                expected: "end of input",
                found: TokenKind::PlusPlus,
            },
            "a comment containing a newline is a line terminator (§12.4)"
        );
        assert_eq!(
            shape("a /* comment */ ++"),
            "(post++ a)",
            "…and one that does not contain a newline is not"
        );

        // §13.4.1: the operand must be something that can be assigned to, prefix or postfix.
        for source in [
            "1++",
            "++1",
            "f()++",
            "++f()",
            "(a + b)++",
            "'x'++",
            "this++",
        ] {
            assert_eq!(
                error(source).kind,
                ParseErrorKind::InvalidAssignmentTarget,
                "on {source:?}"
            );
        }
        // An update expression *is* an `UpdateExpression`, so §13.6 lets it be the left operand
        // of `**` where a prefix unary may not.
        assert_eq!(shape("a++ ** b"), "(** (post++ a) b)");
        assert_eq!(shape("++a ** b"), "(** (pre++ a) b)");
        assert_eq!(error("-a ** b").kind, ParseErrorKind::ExponentiationOnUnary);
        // Member accesses are assignment targets, so they are update targets too.
        assert_eq!(shape("a.b = 1"), "(= (. a b) 1)");
        assert_eq!(shape("a[b] = 1"), "(= ([] a b) 1)");
        assert_eq!(
            error("f() = 1").kind,
            ParseErrorKind::InvalidAssignmentTarget
        );
        assert!(!parse("a++").parenthesized);
        assert!(parse("(a++)").parenthesized);
        assert!(!parse("++a").parenthesized);
    }

    #[test]
    fn the_left_hand_side_forms_not_yet_built_fail_where_they_will_one_day_parse() {
        // Pinned so that implementing each is a deliberate change and not an accident. Every one
        // of these is legal JavaScript that this parser does not reach yet, and every one fails
        // at the token that would have started it.
        for (source, found) in [
            ("a?.b", TokenKind::QuestionDot),   // optional chaining (§13.3)
            ("f?.(x)", TokenKind::QuestionDot), //
            ("new.target", TokenKind::Dot),     // MetaProperty (§13.3)
            (
                "a`x`",
                TokenKind::Template {
                    part: crate::lexer::TemplatePart::NoSubstitution,
                    cooked_undefined: false,
                },
            ), // tagged template (§13.3)
        ] {
            let kind = error(source).kind;
            assert!(
                matches!(kind, ParseErrorKind::Unexpected { found: f, .. } if f == found),
                "{source:?} failed with {kind:?}"
            );
        }
        // `super`, `import()` and spread arguments fail as unrecognised operands.
        for source in [
            "super.a",
            "super()",
            "import('x')",
            "f(...a)",
            "[1]",
            "({})",
        ] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
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
