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

/// An `Arguments` as read, before anything has decided whether it was a call's or an arrow's.
///
/// Not an AST type: the trailing comma is syntax that leaves nothing behind in either reading, and
/// is here only because §15.9's cover grammar has to ask about it once the `=>` has arrived.
pub(super) struct ArgumentList {
    /// The arguments, in order.
    pub arguments: Box<[Argument]>,
    /// Whether a comma came last — `Arguments : ( ArgumentList , )`.
    pub trailing_comma: bool,
    /// The closing parenthesis.
    pub end: Span,
}

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
            // `CallExpression : ImportCall` and `MemberExpression : MetaProperty` — two
            // productions that begin with the same reserved word and share nothing else, so the
            // token after it is what decides which.
            None if self.current.kind == TokenKind::Keyword(ReservedWord::Import) => {
                self.parse_import_head(allow_call)?
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
                        _ => self.member_after_dot_without_the_dot(expr, true)?,
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
                    let ExprKind::Template(quasi) = quasi.into_kind() else {
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

    /// The name after a `.` or a `?.`, with the punctuation already consumed.
    ///
    /// `MemberExpression . IdentifierName` and `MemberExpression . PrivateIdentifier` differ only
    /// in which token they take, so they are read together and told apart by a flag.
    fn member_after_dot_without_the_dot(
        &mut self,
        object: Expr,
        optional: bool,
    ) -> Result<Expr, ParseError> {
        let token = self.current;
        let private = matches!(token.kind, TokenKind::PrivateIdentifier { .. });
        let (property, end) = if private {
            // §13.3.7 gives `SuperProperty` an `IdentifierName` and no private form: `super.#a`
            // would have to look the name up in the *parent's* private space, which is not a
            // thing that exists.
            if matches!(object.kind, ExprKind::Super) {
                return Err(ParseError {
                    kind: ParseErrorKind::PrivateNameAfterSuper,
                    span: token.span,
                });
            }
            self.advance(Goal::Div)?;
            let name = self.private_name(token)?;
            (name, token.span)
        } else {
            self.parse_property_name()?
        };
        let span = object.span.to(end);
        Ok(Expr::new(
            ExprKind::Member {
                private,
                optional,
                object: Box::new(object),
                property,
            },
            span,
        ))
    }

    /// `MemberExpression . IdentifierName`, with the cursor on the `.`.
    fn member_after_dot(&mut self, object: Expr, optional: bool) -> Result<Expr, ParseError> {
        self.advance(Goal::Div)?;
        self.member_after_dot_without_the_dot(object, optional)
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
        let list = self.parse_arguments()?;
        let span = callee.span.to(list.end);
        Ok(Expr::new(
            ExprKind::Call {
                optional,
                callee: Box::new(callee),
                arguments: list.arguments,
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
        // `new` takes a `MemberExpression` or a `NewExpression`, and the bare word `super` is
        // neither — the two forms it has are the `SuperCall` and the `SuperProperty`, and only
        // the property is a `MemberExpression`. So `new super.x()` derives where `new super()`
        // does not, and `(super())` is a `PrimaryExpression` again, which is why the flag is
        // asked about rather than the kind.
        if matches!(callee.kind, ExprKind::Super) && !callee.parenthesized {
            return Err(ParseError {
                kind: ParseErrorKind::NewOnSuper,
                span: callee.span,
            });
        }
        // `new a()()` is a call on `new a()`, because the first argument list belongs to the
        // `new` and the loop in `parse_member` takes the second.
        let (arguments, end) = if self.current.kind == TokenKind::LParen {
            let list = self.parse_arguments()?;
            (list.arguments, list.end)
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

    /// `ImportCall` and `ImportMeta` (§13.3), with the cursor on `import`.
    ///
    /// `allow_call` is false under a `new`, and that is exactly the rule: an `ImportCall` is a
    /// `CallExpression` where `new MemberExpression Arguments` takes the narrower one, so
    /// `new import(a)` has no derivation. `new import.meta` does — a `MetaProperty` *is* a
    /// `MemberExpression` — which is why this is one function and not two entry points.
    fn parse_import_head(&mut self, allow_call: bool) -> Result<Expr, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        if self.current.kind == TokenKind::Dot {
            return self.parse_import_meta(keyword.span);
        }
        if self.current.kind != TokenKind::LParen {
            return Err(self.unexpected("`(` or `.`"));
        }
        if !allow_call {
            return Err(ParseError {
                kind: ParseErrorKind::NewOnImportCall,
                span: keyword.span,
            });
        }
        self.parse_import_call(keyword.span)
    }

    /// `ImportCall : import ( AssignmentExpression ,_opt )` and its two-argument form (§13.3).
    ///
    /// Not `Arguments`: the grammar spells the list out, so there is no spread and no third
    /// argument, and the first is required — `import()` has nothing to import.
    fn parse_import_call(&mut self, keyword: Span) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let arguments = self.parse_import_arguments();
        self.leave();
        let (specifier, options) = arguments?;
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok(Expr::new(
            ExprKind::ImportCall {
                specifier: Box::new(specifier),
                options: options.map(Box::new),
            },
            keyword.to(close.span),
        ))
    }

    /// The one or two `AssignmentExpression`s of an `ImportCall`, and the comma between them.
    fn parse_import_arguments(&mut self) -> Result<(Expr, Option<Expr>), ParseError> {
        let specifier = self.parse_assignment(AllowIn::Yes)?;
        if self.current.kind != TokenKind::Comma {
            return Ok((specifier, None));
        }
        self.advance(Goal::RegExp)?;
        // `import(a,)` — the trailing comma of the one-argument form, which the grammar gives
        // both forms and which leaves nothing behind.
        if self.current.kind == TokenKind::RParen {
            return Ok((specifier, None));
        }
        let options = self.parse_assignment(AllowIn::Yes)?;
        if self.current.kind == TokenKind::Comma {
            self.advance(Goal::RegExp)?;
        }
        Ok((specifier, Some(options)))
    }

    /// `ImportMeta : import . meta` (§13.3), with the cursor on the `.`.
    ///
    /// Read like `new.target` and refused like it, except that what decides is the goal symbol
    /// rather than the enclosing function: §13.3.12 makes one a Syntax Error "if the syntactic
    /// goal symbol is not Module", there being no module to describe in a script.
    fn parse_import_meta(&mut self, keyword: Span) -> Result<Expr, ParseError> {
        self.advance(Goal::Div)?;
        let word = self.current;
        let is_meta = matches!(
            word.kind,
            TokenKind::Identifier {
                contains_escape: false
            }
        ) && word.span.slice(self.source) == Some("meta");
        if !is_meta {
            return Err(self.unexpected("`meta`"));
        }
        self.advance(Goal::Div)?;
        if !self.module {
            return Err(ParseError {
                kind: ParseErrorKind::ImportMetaOutsideModule,
                span: keyword.to(word.span),
            });
        }
        Ok(Expr::new(ExprKind::ImportMeta, keyword.to(word.span)))
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

    /// `Arguments` (§13.3), with the cursor on the `(`.
    ///
    /// A trailing comma is allowed — `Arguments : ( ArgumentList , )` — but an empty list with
    /// one is not, since `ArgumentList` needs at least one argument to trail. Whether one was
    /// written is reported because §15.9's cover grammar needs it: an argument list may end in a
    /// comma after a spread and a parameter list may not, and only the `=>` says which this was.
    pub(super) fn parse_arguments(&mut self) -> Result<ArgumentList, ParseError> {
        self.advance(Goal::RegExp)?;
        let mut arguments = Vec::new();
        let mut trailing_comma = false;
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
            trailing_comma = self.current.kind == TokenKind::RParen;
        }
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok(ArgumentList {
            arguments: arguments.into_boxed_slice(),
            trailing_comma,
            end: close.span,
        })
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
    fn a_new_expression_refuses_the_bare_word_super() {
        // §13.3: `new` takes a `MemberExpression` or a `NewExpression`, and the bare word
        // `super` is neither — `super(...)` is a `SuperCall` and `super.x` a `SuperProperty`,
        // and only the property is a `MemberExpression`. So `new super.x()` derives where
        // `new super()` does not, and `(super())` is a `PrimaryExpression` again.
        assert_eq!(
            kind("class D extends B { constructor() { new super(); } }"),
            ParseErrorKind::NewOnSuper
        );
        assert_eq!(
            kind("class D extends B { constructor() { new super(1, 2); } }"),
            ParseErrorKind::NewOnSuper
        );
        assert!(parse_script("class D extends B { constructor() { new (super()); } }").is_ok());
        assert!(parse_script("class D extends B { constructor() { new super.x(); } }").is_ok());
        assert!(parse_script("class D extends B { constructor() { super(); } }").is_ok());
        // Elsewhere the word was refused before it ever reached a `new` — for the older
        // reason, which is the one §13.3 gives there.
        assert_eq!(
            kind("class C { constructor() { new super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
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
    fn an_import_call_takes_one_or_two_arguments_and_is_not_a_member_expression() {
        assert_eq!(shape("import(a)"), "(import a)");
        assert_eq!(shape("import(a, b)"), "(import a b)");
        // The grammar spells the list out rather than borrowing `Arguments`, so both forms take a
        // trailing comma, neither takes a spread, and there is no third.
        assert_eq!(shape("import(a,)"), "(import a)");
        assert_eq!(shape("import(a, b,)"), "(import a b)");
        for source in [
            "import()",
            "import(...a)",
            "import(a, b, c)",
            "import(",
            "import(a,",
        ] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
        // A `CallExpression`, so it chains with everything one does\u2026
        assert_eq!(shape("import(a).b"), "(. (import a) b)");
        assert_eq!(shape("import(a)(b)"), "(call (import a) [b])");
        assert!(parse_script("import(a) + 1;").is_ok());
        assert!(parse_script("typeof import(a);").is_ok());
        assert!(parse_script("f(import(a));").is_ok());
        assert!(parse_script("import(import(a));").is_ok());
        // \u2026and `new MemberExpression Arguments` takes the narrower one, so this has no
        // derivation and nothing to construct if it had.
        assert_eq!(
            script_error("new import(a);").kind,
            ParseErrorKind::NewOnImportCall
        );
        // A script may write one: only `import.meta` needs the goal symbol.
        assert!(parse_script("import(a);").is_ok());
        // The word alone is neither production, and the error names both — said as the
        // kind because everything after this point would refuse it anyway, for reasons
        // that have nothing to do with there being no `(` or `.`.
        for source in ["import;", "import a;", "import 1;", "import"] {
            assert!(
                matches!(
                    script_error(source).kind,
                    ParseErrorKind::Unexpected {
                        expected: "`(` or `.`",
                        ..
                    }
                ),
                "{source:?} failed with {:?}",
                script_error(source).kind
            );
        }
    }

    #[test]
    fn import_meta_is_the_one_thing_here_that_needs_the_goal_symbol() {
        // \u00a713.3.12: "It is a Syntax Error if the syntactic goal symbol is not Module." Refused
        // everywhere in a script, including where nothing else about the position is wrong.
        for source in [
            "import.meta;",
            "import.meta.a;",
            "function f() { import.meta; }",
            "class C { m() { import.meta; } }",
            "[import.meta];",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ImportMetaOutsideModule,
                "{source:?}"
            );
        }
        // `meta` is a terminal of the production, so only that spelling works \u2014 and no
        // `[no LineTerminator here]` on either side of the `.`.
        assert!(parse_script("import.Meta;").is_err());
        assert!(parse_script("import.a;").is_err());
        assert_eq!(
            script_error("import . meta;").kind,
            ParseErrorKind::ImportMetaOutsideModule
        );
        assert_eq!(
            script_error("import\n.\nmeta;").kind,
            ParseErrorKind::ImportMetaOutsideModule
        );
    }
}
