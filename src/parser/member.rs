//! `LeftHandSideExpression` (ECMAScript §13.3): member access, calls, and `new`.
//!
//! The layer between the operator ladder in [`super::expression`] and the primary expressions in
//! [`super::primary`], and the one place the grammar is genuinely awkward: `new` and a call both
//! want the same argument list, and which of them gets it is decided by how many `new`s are
//! waiting. `parse_new` has the argument.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Argument, Expr, ExprKind};
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
        // Whether a `?.` has been seen in *this* chain. Once one has, the whole thing is an
        // `OptionalExpression` — which is what §13.3's `OptionalChain` productions say by
        // continuing with plain `.`, `[` and `(` after the first `?.`.
        let mut optional_chain = false;
        loop {
            expr = match self.current.kind {
                TokenKind::Dot => self.member_after_dot(expr, false)?,
                TokenKind::LBracket => self.computed_member_after(expr, false)?,
                TokenKind::LParen if allow_call => self.call_after(expr, false)?,
                // `OptionalChain : ?. Arguments | ?. [ Expression ] | ?. IdentifierName`. The
                // lexer has already declined to make a `?.` when a digit follows, so `a?.5:b` is
                // the conditional it has been since ES5 and never arrives here.
                TokenKind::QuestionDot => {
                    optional_chain = true;
                    self.advance(Goal::RegExp)?;
                    match self.current.kind {
                        TokenKind::LBracket => self.computed_member_after(expr, true)?,
                        TokenKind::LParen if allow_call => self.call_after(expr, true)?,
                        // `OptionalChain : ?. TemplateLiteral` is a production, so this is not a
                        // missing property name — it is the shape §13.3.1 names, and the loop
                        // below would never see it because a `?.` was in the way.
                        TokenKind::Template { .. } => {
                            return Err(ParseError {
                                kind: ParseErrorKind::TaggedTemplateOnOptionalChain,
                                span: self.current.span,
                            });
                        }
                        _ => self.member_after_dot_without_the_dot(expr)?,
                    }
                }
                // `MemberExpression TemplateLiteral` and `CallExpression TemplateLiteral` — a
                // call written without parentheses, so it chains like one.
                TokenKind::Template { .. } => {
                    // §13.3.1: "It is a Syntax Error if any source text is matched by this
                    // production: OptionalChain TemplateLiteral". A tag function is handed the
                    // raw strings *and* is called, and a chain that short-circuits has nothing to
                    // call — so the whole chain is poisoned, not just the link with the `?.` on
                    // it. `(a?.b)` closes the chain and may be tagged.
                    if optional_chain {
                        return Err(ParseError {
                            kind: ParseErrorKind::TaggedTemplateOnOptionalChain,
                            span: self.current.span,
                        });
                    }
                    let quasi = self.parse_template(super::template::Tagged::Yes)?;
                    let ExprKind::Template(quasi) = quasi.kind else {
                        // `parse_template` returns nothing else.
                        return Ok(expr);
                    };
                    let span = expr.span.to(self.current.span);
                    Expr::new(
                        ExprKind::TaggedTemplate {
                            tag: Box::new(expr),
                            quasi,
                        },
                        span,
                    )
                }
                _ => break,
            };
        }
        if optional_chain {
            let span = expr.span;
            expr = Expr::new(ExprKind::OptionalChain(Box::new(expr)), span);
        }
        Ok(expr)
    }

    /// `?. IdentifierName`, with the `?.` already consumed.
    ///
    /// The property name is read the same way as after a plain `.`; only the flag differs, and
    /// there is no `.` left to skip.
    fn member_after_dot_without_the_dot(&mut self, object: Expr) -> Result<Expr, ParseError> {
        let (property, end) = self.parse_property_name()?;
        let span = object.span.to(end);
        Ok(Expr::new(
            ExprKind::Member {
                optional: true,
                object: Box::new(object),
                property,
            },
            span,
        ))
    }

    /// `MemberExpression . IdentifierName`, with the cursor on the `.`.
    fn member_after_dot(&mut self, object: Expr, optional: bool) -> Result<Expr, ParseError> {
        self.advance(Goal::Div)?;
        let (property, end) = self.parse_property_name()?;
        let span = object.span.to(end);
        Ok(Expr::new(
            ExprKind::Member {
                optional,
                object: Box::new(object),
                property,
            },
            span,
        ))
    }

    /// `MemberExpression [ Expression ]`, with the cursor on the `[`.
    fn computed_member_after(&mut self, object: Expr, optional: bool) -> Result<Expr, ParseError> {
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
                optional,
                object: Box::new(object),
                property: Box::new(property),
            },
            span,
        ))
    }

    /// `CallExpression Arguments`, with the cursor on the `(`.
    fn call_after(&mut self, callee: Expr, optional: bool) -> Result<Expr, ParseError> {
        let (arguments, end) = self.parse_arguments()?;
        let span = callee.span.to(end);
        Ok(Expr::new(
            ExprKind::Call {
                optional,
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
        // `MetaProperty : NewTarget`, and `NewTarget : new . target` — three tokens that are one
        // production, so nothing here is a member access of anything. No `[no LineTerminator
        // here]` on either side of the `.`.
        if self.current.kind == TokenKind::Dot {
            return self.parse_new_target(token.span);
        }
        self.enter()?;
        let callee = self.parse_member(false, None);
        self.leave();
        let callee = callee?;
        // §13.3: `new MemberExpression Arguments`, and an `OptionalExpression` is not a
        // `MemberExpression`. There is nothing to construct when the chain gives up.
        if matches!(callee.kind, ExprKind::OptionalChain(_)) && !callee.parenthesized {
            return Err(ParseError {
                kind: ParseErrorKind::NewOnOptionalChain,
                span: callee.span,
            });
        }
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

    /// `NewTarget : new . target` (§13.3), with the cursor on the `.`.
    fn parse_new_target(&mut self, keyword: Span) -> Result<Expr, ParseError> {
        self.advance(Goal::Div)?;
        let word = self.current;
        // Not `parse_property_name`: `target` is a literal terminal of the production and not an
        // `IdentifierName`, so `new.Target` and `new.target` are both refused. The spec
        // spells the word out, and so does this.
        let is_target = matches!(
            word.kind,
            TokenKind::Identifier {
                contains_escape: false
            }
        ) && word.span.slice(self.source) == Some("target");
        if !is_target {
            return Err(self.unexpected("`target`"));
        }
        self.advance(Goal::Div)?;
        // §16.1.1: "It is a Syntax Error if StatementList Contains NewTarget" for a `ScriptBody`.
        // There is no function being constructed at the top of a script, so there is nothing for
        // it to mean. An arrow is transparent to this — see [`super::body`].
        if !self.body_context.new_target_allowed {
            return Err(ParseError {
                kind: ParseErrorKind::NewTargetOutsideFunction,
                span: keyword.to(word.span),
            });
        }
        Ok(Expr::new(ExprKind::NewTarget, keyword.to(word.span)))
    }

    /// `Arguments` (§13.3), with the cursor on the `(`. Returns them and the closing span.
    ///
    /// A trailing comma is allowed — `Arguments : ( ArgumentList , )` — but an empty list with
    /// one is not, since `ArgumentList` needs at least one argument to trail.
    fn parse_arguments(&mut self) -> Result<(Box<[Argument]>, Span), ParseError> {
        self.advance(Goal::RegExp)?;
        let mut arguments = Vec::new();
        while self.current.kind != TokenKind::RParen {
            // `ArgumentList : ... AssignmentExpression`, and it may stand anywhere in the list
            // rather than only last — unlike a `BindingRestElement`, which is a name being bound
            // and so has to be the one that takes what is left.
            let spread = self.current.kind == TokenKind::DotDotDot;
            if spread {
                self.advance(Goal::RegExp)?;
            }
            self.enter()?;
            let argument = self.parse_assignment(AllowIn::Yes);
            self.leave();
            let argument = argument?;
            arguments.push(if spread {
                Argument::Spread(argument)
            } else {
                Argument::Value(argument)
            });
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
    use crate::parser::test_support::*;
    use crate::parser::{parse_expression, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
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
    fn an_argument_list_takes_a_spread_anywhere_in_it_and_no_elision_at_all() {
        assert_eq!(shape("f(...a)"), "(call f [(... a)])");
        assert_eq!(shape("f(a, ...b)"), "(call f [a (... b)])");
        // Anywhere, unlike a `BindingRestElement`: an argument spread is not a name being bound,
        // so nothing needs it to be the one that takes what is left.
        assert_eq!(shape("f(...a, b)"), "(call f [(... a) b])");
        assert_eq!(shape("f(...a, ...b)"), "(call f [(... a) (... b)])");
        assert_eq!(shape("f(a, ...b, c)"), "(call f [a (... b) c])");
        // `Arguments : ( ArgumentList , )` — a trailing comma, which leaves nothing behind.
        assert_eq!(shape("f(...a,)"), "(call f [(... a)])");
        assert_eq!(shape("f(a,)"), "(call f [a])");
        // …and `ArgumentList` has no elision, so an empty slot has no derivation where an array
        // literal's `[,]` has one.
        for source in ["f(,)", "f(a,,b)", "f(,...a)", "f(...)"] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
        // A spread takes an `AssignmentExpression`, so it takes everything one does.
        assert_eq!(shape("f(...a.b)"), "(call f [(... (. a b))])");
        assert_eq!(shape("f(...(a, b))"), "(call f [(... (, a b))])");
        assert_eq!(shape("f(...a = b)"), "(call f [(... (= a b))])");
        // `new` takes the same `Arguments`, and a spread chains like anything else.
        assert_eq!(shape("new f(...a)"), "(new f [(... a)])");
        assert_eq!(
            shape("f(...a)(...b)"),
            "(call (call f [(... a)]) [(... b)])"
        );
        assert!(parse_expression("f(...a)`x`").is_ok());
    }

    #[test]
    fn new_target_is_three_tokens_that_are_one_production() {
        assert_eq!(
            statements("function f() { new.target; }"),
            ["(fn f [] {new.target})"]
        );
        // A `MetaProperty` and not a member access, so `target` is a terminal of the production
        // rather than an `IdentifierName` — which is why only that spelling works.
        assert!(parse_script("function f() { new.Target; }").is_err());
        assert!(parse_script("function f() { new.taget; }").is_err());
        // …and no `[no LineTerminator here]` on either side of the `.`.
        assert!(parse_script("function f() { new . target; }").is_ok());
        assert!(parse_script("function f() { new\n.\ntarget; }").is_ok());
        // §16.1.1 refuses one in a `ScriptBody`: there is no function being constructed at the
        // top of a script, so there is nothing for it to mean.
        assert_eq!(
            kind("new.target;"),
            ParseErrorKind::NewTargetOutsideFunction
        );
        // An arrow is transparent to it — §8.4's `Contains` descends into one looking for exactly
        // this, along with `super`, `this` and `arguments`.
        assert_eq!(
            kind("() => new.target;"),
            ParseErrorKind::NewTargetOutsideFunction
        );
        assert!(parse_script("function f() { () => new.target; }").is_ok());
        // Every other kind of function grants it, `super` being the thing that does not.
        for source in [
            "function* g() { new.target; }",
            "({ m() { new.target; } });",
            "class C { m() { new.target; } }",
            "class C extends D { constructor() { new.target; } }",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // It is a `MemberExpression`, so it chains and is an operand…
        assert_eq!(
            shape("(function () { return new.target.a; })"),
            "(fn <anon> [] {(return (. new.target a))})"
        );
        for source in [
            "new.target();",
            "typeof new.target;",
            "new new.target;",
            "new.target`x`;",
            "[new.target];",
            "new.target?.a;",
        ] {
            assert!(
                parse_script(&format!("function f() {{ {source} }}")).is_ok(),
                "{source:?}"
            );
        }
        // …and its `AssignmentTargetType` is invalid, `MetaProperty` being nowhere in §13.15.1's
        // list of the productions that answer simple.
        assert!(parse_script("function f() { new.target = 1; }").is_err());
        assert!(parse_script("function f() { new.target++; }").is_err());
    }

    #[test]
    fn an_optional_chain_continues_with_plain_links_once_it_has_started() {
        assert_eq!(shape("a?.b"), "(?chain (?. a b))");
        assert_eq!(shape("a?.[b]"), "(?chain (?[] a b))");
        assert_eq!(shape("a?.(b)"), "(?chain (?call a [b]))");
        // `OptionalChain : OptionalChain . IdentifierName | OptionalChain [ … ] | …` — so after
        // the first `?.` the ordinary links keep the chain going rather than ending it. Which is
        // what makes `a?.b.c` give up on the whole thing when `a` is nullish.
        assert_eq!(shape("a?.b.c"), "(?chain (. (?. a b) c))");
        assert_eq!(shape("a?.[b][c]"), "(?chain ([] (?[] a b) c))");
        assert_eq!(shape("a?.(b)(c)"), "(?chain (call (?call a [b]) [c]))");
        assert_eq!(shape("a?.b(c)"), "(?chain (call (?. a b) [c]))");
        assert_eq!(shape("a?.b?.c"), "(?chain (?. (?. a b) c))");
        // A chain may start after any number of plain links.
        assert_eq!(shape("a.b?.c"), "(?chain (?. (. a b) c))");
        assert_eq!(shape("f()?.a"), "(?chain (?. (call f []) a))");
        // …and the wrapper is where the short-circuiting stops, which is what parentheses move.
        assert_eq!(shape("(a?.b).c"), "(. (?chain (?. a b)) c)");
        // The property after `?.` is an `IdentifierName`, as after a `.`.
        assert!(parse_expression("a?.if").is_ok());
        assert!(parse_expression("a?.class").is_ok());
        // A line terminator is allowed on either side: `?.` carries no restriction.
        assert!(parse_expression("a\n?.b").is_ok());
        assert!(parse_expression("a?.\nb").is_ok());
        // The lexer declines to make a `?.` when a decimal digit follows, so this is the
        // conditional it has been since ES5 and never reaches the chain code at all.
        assert_eq!(shape("a?.5:b"), "(? a 0.5 b)");
    }

    #[test]
    fn an_optional_chain_is_neither_an_assignment_target_nor_something_new_can_take() {
        // `OptionalExpression` is nowhere in §13.15.1's list, so its `AssignmentTargetType` is
        // invalid — and the wrapper is what makes that one question rather than a walk.
        for source in [
            "a?.b = 1;",
            "a?.b.c = d;",
            "a?.[b] = c;",
            "a?.(b) = c;",
            "a?.b += 1;",
            "a?.b++;",
            "(a?.b) = c;",
            "(a?.b)++;",
            "[a?.b] = c;",
            "({a: a?.b} = c);",
            "for (a?.b of c);",
            "for (a?.b in c);",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // …while `delete` takes a `UnaryExpression` and so takes one happily.
        assert!(parse_script("delete a?.b;").is_ok());
        assert!(parse_script("delete a?.b.c;").is_ok());
        // §13.3: `new MemberExpression Arguments`, and an `OptionalExpression` is not one.
        assert_eq!(kind("new a?.b;"), ParseErrorKind::NewOnOptionalChain);
        assert_eq!(kind("new a?.b();"), ParseErrorKind::NewOnOptionalChain);
        // …and parentheses end the chain, so these construct what is inside them.
        assert!(parse_script("new (a?.b);").is_ok());
        assert!(parse_script("new (a?.b)();").is_ok());
        // §13.3.1: a tag function is called, and a chain that gives up has nothing to call — so
        // the rule poisons the whole chain rather than the link the `?.` is on.
        for source in [
            "a?.`x`;",
            "a?.b`x`;",
            "a.b?.`x`;",
            "a?.[b]`x`;",
            "a?.(b)`x`;",
            "a.b.c?.d`x`;",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::TaggedTemplateOnOptionalChain,
                "{source:?}"
            );
        }
        // …and once again the parentheses close it.
        assert!(parse_script("(a?.b)`x`;").is_ok());
        // Everything that takes an ordinary expression takes one of these.
        for source in [
            "a?.b ?? c;",
            "a?.b || c;",
            "a?.b === c;",
            "a?.b instanceof c;",
            "!a?.b;",
            "a?.b ? c : d;",
            "f(a?.b);",
            "[a?.b];",
            "typeof a?.b;",
            "a?.b(...c);",
            "class C extends a?.b {}",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // `super` is the head of a `SuperProperty` or a `SuperCall` and of nothing else, so it
        // has no `?.` form.
        assert!(parse_script("({ m() { super?.a; } });").is_err());
    }

    #[test]
    fn the_left_hand_side_forms_not_yet_built_fail_where_they_will_one_day_parse() {
        // Pinned so that implementing each is a deliberate change and not an accident. What is
        // left of this list is `ImportCall` and `ImportMeta`, both of which need the host and the
        // `Module` goal that modules will bring. Optional chaining, `new.target`, spread
        // arguments and `super` used to be here and now parse — see the tests above.
        for source in ["import('x')", "import.meta"] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
    }
}
