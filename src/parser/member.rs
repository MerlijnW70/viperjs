//! `LeftHandSideExpression` (ECMAScript §13.3): member access, calls, and `new`.
//!
//! The layer between the operator ladder in [`super::expression`] and the primary expressions in
//! [`super::primary`], and the one place the grammar is genuinely awkward: `new` and a call both
//! want the same argument list, and which of them gets it is decided by how many `new`s are
//! waiting. `parse_new` has the argument.

use super::expression::AllowIn;
use super::{ParseError, Parser};
use crate::ast::{Expr, ExprKind};
use crate::lexer::{Goal, ReservedWord, TokenKind, identifier_value};
use crate::span::Span;

impl Parser<'_> {
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
    pub(super) fn parse_member(
        &mut self,
        allow_call: bool,
        head: Option<Expr>,
    ) -> Result<Expr, ParseError> {
        // A head that is already read is a parenthesized group the assignment level had to
        // open to see whether a `=>` followed it (§15.3). It is a `PrimaryExpression`, so the
        // suffixes below apply to it exactly as if this had read it.
        let mut expr = match head {
            Some(head) => head,
            None if self.current.kind == TokenKind::Keyword(ReservedWord::New) => {
                self.parse_new()?
            }
            None => self.parse_primary()?,
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
        let span = object.span.to(end);
        Ok(Expr::new(
            ExprKind::Member {
                object: Box::new(object),
                property,
            },
            span,
        ))
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
        let span = object.span.to(close.span);
        Ok(Expr::new(
            ExprKind::ComputedMember {
                object: Box::new(object),
                property: Box::new(property),
            },
            span,
        ))
    }

    /// `CallExpression Arguments`, with the cursor on the `(`.
    fn call_after(&mut self, callee: Expr) -> Result<Expr, ParseError> {
        let (arguments, end) = self.parse_arguments()?;
        let span = callee.span.to(end);
        Ok(Expr::new(
            ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
            span,
        ))
    }

    /// `new MemberExpression Arguments` or `new NewExpression` (§13.3).
    ///
    /// Not on the bracket-recursion path — it is entered only for a literal `new` — so its
    /// locals cost nothing when there is none.
    fn parse_new(&mut self) -> Result<Expr, ParseError> {
        let token = self.advance(Goal::RegExp)?;
        self.enter()?;
        let callee = self.parse_member(false, None);
        self.leave();
        let callee = callee?;
        // `new a()()` is a call on `new a()`, because the first argument list belongs to the
        // `new` and the loop in `parse_member` takes the second.
        let (arguments, end) = if self.current.kind == TokenKind::LParen {
            self.parse_arguments()?
        } else {
            (Vec::new().into_boxed_slice(), callee.span)
        };
        Ok(Expr::new(
            ExprKind::New {
                callee: Box::new(callee),
                arguments,
            },
            token.span.to(end),
        ))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ParseErrorKind;
    use crate::parser::parse_expression;
    use crate::parser::test_support::*;
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
        // `super`, `import()`, spread arguments and object literals fail as unrecognised
        // operands. Array literals used to be on this list and now parse — see
        // [`super::array_literal`].
        for source in ["super.a", "super()", "import('x')", "f(...a)"] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
    }
}
