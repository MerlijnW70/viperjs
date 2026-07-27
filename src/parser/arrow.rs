//! Arrow functions (ECMAScript §15.3), and the cover grammar that reaches them.
//!
//! # Why this cannot live in `parse_primary`
//!
//! An `ArrowFunction` is an `AssignmentExpression` and nothing tighter, so `x + (a) => b` has no
//! derivation — `+` wants a `MultiplicativeExpression` and an arrow is not one. A `(` is read deep
//! inside the operand path, though, so producing the arrow there would quietly accept it. The
//! decision therefore belongs to [`super::Parser::parse_assignment`], which is the level the
//! grammar puts it at, and the group it may have already read is handed back to the operand path
//! rather than re-read.
//!
//! # The cover grammar has two productions that are not expressions at all
//!
//! ```text
//! CoverParenthesizedExpressionAndArrowParameterList :
//!   ( Expression )        ( Expression , )       ( )
//!   ( ... BindingIdentifier )                    ( ... BindingPattern )
//!   ( Expression , ... BindingIdentifier )       ( Expression , ... BindingPattern )
//! ```
//!
//! `()` and `(...a)` are not expressions in any sense — there is nothing for them to evaluate to —
//! so refining a parsed *expression* into parameters, the way [`super::pattern`] refines a
//! literal, would need an AST node that is not a node of the language. Instead the group is read
//! into a structure that is not part of the tree, and the `=>` decides which of the two things it
//! becomes. That is what a cover grammar is: one reading of the source, committed to late.
//!
//! # A third refinement, which lives next to the grammar it produces
//!
//! `([a]) => b` needs its array literal turned into an `ArrayBindingPattern`, not into an
//! `ArrayAssignmentPattern`: arrow parameters *create* names. That refinement is in
//! [`super::binding`] rather than here, because what it may produce is that file's grammar and
//! not this one's — it must refuse exactly what parsing a `BindingPattern` directly would refuse.
//! [`super::pattern`] is the mirror, refining an `Expr` into a `Pattern` instead. The two differ
//! exactly where the two pattern grammars do: `([a.b]) => c` has no derivation and
//! `[a.b] = c` does.
//!
//! # `[no LineTerminator here]`
//!
//! `ArrowParameters [no LineTerminator here] => ConciseBody`. A newline before the `=>` does not
//! make a bad arrow; it makes the thing before it an ordinary parenthesized expression, and then
//! the `=>` is a token nothing wanted. That is why no rule here mentions it: the check is a
//! condition on *being* an arrow, not an error about one.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    ArrowBody, ArrowFunction, Binding, BindingElement, Expr, ExprKind, FormalParameters,
};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;
use crate::static_semantics::bound_names;
use std::collections::HashSet;

/// A parenthesized group, before anything has decided what it is.
///
/// Not an AST type: two of the cover grammar's productions have no meaning as expressions, and a
/// tree that could hold them would be a tree that can hold what the language cannot say.
pub(super) struct CoverGroup {
    /// The comma-separated `AssignmentExpression`s.
    elements: Vec<Expr>,
    /// A trailing `... BindingIdentifier` or `... BindingPattern`.
    rest: Option<Binding>,
    /// Whether a comma came last. Legal in the cover and in no expression.
    trailing_comma: bool,
    /// The error the parentheses owe if they turn out to be parameters.
    ///
    /// §15.3.1: "It is a Syntax Error if ArrowParameters Contains YieldExpression is true", and
    /// the same for an `AwaitExpression` — the same rules and the same reason as §15.5.1's and
    /// §15.8.1's about a function's own parameters. Which of the two things the group is
    /// decides whether they apply: `function* g() { (yield); }` is an expression and fine,
    /// `function* g() { (a = yield) => 1; }` is a parameter list and is not. So it is recorded
    /// here and asked once the `=>` has settled the question.
    forbidden_in_parameters: Option<ParseError>,
    /// The parentheses and everything between them.
    span: Span,
}

/// What was found where an `AssignmentExpression` may begin.
pub(super) enum ArrowOrGroup {
    /// A whole arrow function, which is the entire `AssignmentExpression`.
    Arrow(Expr),
    /// A parenthesized expression, which is only the beginning of one.
    Operand(Expr),
    /// Neither — nothing was consumed, and the ordinary path should read it.
    Neither,
}

impl Parser<'_> {
    /// Read an arrow function, or the parenthesized expression that was not one.
    ///
    /// Called at the head of an `AssignmentExpression`, which is the only place an arrow may
    /// stand. Consumes nothing unless it returns something.
    pub(super) fn parse_arrow_or_group(
        &mut self,
        allow_in: AllowIn,
    ) -> Result<ArrowOrGroup, ParseError> {
        // `ArrowParameters : BindingIdentifier` — one token of parameters, and one of lookahead
        // to know it. The `=>` must be on the same line, or this is not an arrow at all.
        if self.is_identifier_token(self.current.kind) {
            let next = self.peek(Goal::Div)?;
            if next.kind == TokenKind::Arrow && !next.newline_before {
                let name = self.parse_binding_name()?;
                let span = name.span;
                let parameters = FormalParameters {
                    items: Box::new([BindingElement {
                        target: Binding::Identifier(name),
                        default: None,
                    }]),
                    rest: None,
                    span,
                };
                return Ok(ArrowOrGroup::Arrow(
                    self.parse_arrow_tail(parameters, allow_in)?,
                ));
            }
            return Ok(ArrowOrGroup::Neither);
        }
        if self.current.kind != TokenKind::LParen {
            return Ok(ArrowOrGroup::Neither);
        }
        self.enter()?;
        let group = self.parse_cover_group();
        self.leave();
        let group = group?;
        if self.current.kind == TokenKind::Arrow && !self.current.newline_before {
            let parameters = self.refine_to_parameters(group)?;
            return Ok(ArrowOrGroup::Arrow(
                self.parse_arrow_tail(parameters, allow_in)?,
            ));
        }
        Ok(ArrowOrGroup::Operand(self.group_as_expression(group)?))
    }

    /// The cover grammar, read as far as its closing parenthesis.
    fn parse_cover_group(&mut self) -> Result<CoverGroup, ParseError> {
        let open = self.advance(Goal::RegExp)?;
        let enclosing_forbidden_in_parameters = self.forbidden_in_parameters.take();
        let mut elements = Vec::new();
        let mut rest = None;
        let mut trailing_comma = false;
        while self.current.kind != TokenKind::RParen {
            if self.current.kind == TokenKind::DotDotDot {
                self.advance(Goal::RegExp)?;
                rest = Some(self.parse_binding()?);
                break;
            }
            elements.push(self.parse_assignment(AllowIn::Yes)?);
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
            if self.current.kind == TokenKind::RParen {
                trailing_comma = true;
            }
        }
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        let forbidden_in_parameters = self.forbidden_in_parameters;
        self.forbidden_in_parameters = enclosing_forbidden_in_parameters;
        Ok(CoverGroup {
            elements,
            rest,
            trailing_comma,
            forbidden_in_parameters,
            span: open.span.to(close.span),
        })
    }

    /// The group as the `( Expression )` it must be, no `=>` having followed.
    ///
    /// Three of the cover's productions are left with nowhere to go: `()`, `( Expression , )` and
    /// anything with a `...`. None of them is an expression, and the error says which.
    fn group_as_expression(&mut self, group: CoverGroup) -> Result<Expr, ParseError> {
        // It was an expression after all, so a `yield` inside it was a `yield` written in the
        // enclosing code — and still counts against whatever parameter list encloses *that*.
        // `function* g(a = (yield)) {}` is refused for the same reason `function* g(a = yield) {}`
        // is, the parentheses having changed nothing.
        if let Some(error) = group.forbidden_in_parameters {
            self.forbidden_in_parameters.get_or_insert(error);
        }
        if group.rest.is_some() || group.trailing_comma || group.elements.is_empty() {
            return Err(ParseError {
                kind: ParseErrorKind::CoverGroupIsNotAnExpression,
                span: group.span,
            });
        }
        let span = group.span;
        let mut elements = group.elements;
        // `Expression` is a comma list, so more than one is a sequence — and it is parenthesized,
        // which is what keeps `(a, b) = c` an error and `(a) = c` legal.
        let inner = if elements.len() == 1 {
            elements.remove(0)
        } else {
            let inner_span = match (elements.first(), elements.last()) {
                (Some(first), Some(last)) => first.span.to(last.span),
                // `elements` is non-empty here, checked above.
                _ => span,
            };
            Expr::new(ExprKind::Sequence(elements.into_boxed_slice()), inner_span)
        };
        Ok(inner.in_parentheses(span))
    }

    /// `ArrowFormalParameters : ( UniqueFormalParameters )` — the group, refined.
    fn refine_to_parameters(&mut self, group: CoverGroup) -> Result<FormalParameters, ParseError> {
        // §15.3.1, now that the `=>` has said these are parameters. An arrow's are `[?Yield]`,
        // so inside a generator a `yield` here parsed as a `YieldExpression` — and a parameter's
        // default is evaluated before there is anything to yield to, exactly as §15.5.1 says of a
        // generator's own list.
        if let Some(error) = group.forbidden_in_parameters {
            return Err(error);
        }
        // No check for a comma after the `...`: reading the group stops at one, so `(...a,)`
        // never reaches here — the closing parenthesis it was looking for is missing, and saying
        // so is both true and more useful than a rule about rest elements.
        let mut items = Vec::with_capacity(group.elements.len());
        for element in group.elements {
            items.push(self.refine_to_binding_element(element)?);
        }
        let parameters = FormalParameters {
            items: items.into_boxed_slice(),
            rest: group.rest.map(Box::new),
            span: group.span,
        };
        // `UniqueFormalParameters`, which says the rule in its name: an arrow's parameters may
        // never repeat, sloppy or not and simple or not — as a method's may not, and a plain
        // function's simple list may.
        let mut seen: HashSet<String> = HashSet::new();
        for element in &parameters.items {
            for declared in bound_names(&element.target) {
                if !seen.insert(declared.name.to_string()) {
                    return Err(ParseError {
                        kind: ParseErrorKind::DuplicateParameterName,
                        span: declared.span,
                    });
                }
            }
        }
        Ok(parameters)
    }

    /// `=> ConciseBody`, with the parameters already refined.
    fn parse_arrow_tail(
        &mut self,
        parameters: FormalParameters,
        allow_in: AllowIn,
    ) -> Result<Expr, ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let body = self.parse_concise_body(allow_in, &parameters);
        self.leave();
        let (body, end, declares_strict) = body?;
        super::function::check_parameters_against_arrow_body(&parameters, &body)?;
        // The parameters were refined from *references*, which §13.1.1 judges by looser rules
        // than bindings — `eval` may be read in strict code and may not be bound. So the binding
        // rules are applied here, where the references have become bindings, and only once the
        // body has had its say about strictness.
        if self.strict || declares_strict {
            super::function::check_strict_parameters(&parameters)?;
        }
        let span = parameters.span.to(end);
        Ok(Expr::new(
            ExprKind::Arrow(Box::new(ArrowFunction {
                parameters,
                body,
                span,
            })),
            span,
        ))
    }

    /// `ConciseBody : [lookahead ≠ {] ExpressionBody | { FunctionBody }` (§15.3).
    ///
    /// The lookahead restriction is why `a => ({})` needs its parentheses: a `{` here opens a
    /// body, so an object literal has to be told apart by the author rather than by the parser.
    fn parse_concise_body(
        &mut self,
        allow_in: AllowIn,
        parameters: &FormalParameters,
    ) -> Result<(ArrowBody, Span, bool), ParseError> {
        // `ConciseBody[In] : ExpressionBody[?In, ~Await] | { FunctionBody[~Yield, ~Await] }` —
        // both alternatives drop *both* parameters, where `ArrowParameters[?Yield, ?Await]`
        // keeps both. So
        // `function* g() { () => yield; }` reads `yield` as a name and
        // `function* g() { (a = yield) => 1; }` is refused, the parameters having been `[+Yield]`.
        // The one place in the grammar where a head and its body disagree about a parameter.
        let enclosing = (self.yield_allowed, self.await_allowed);
        self.yield_allowed = false;
        self.await_allowed = false;
        let body = self.parse_concise_body_inner(allow_in, parameters);
        (self.yield_allowed, self.await_allowed) = enclosing;
        body
    }

    /// The body itself, once `[Yield]` has been dropped for it.
    fn parse_concise_body_inner(
        &mut self,
        allow_in: AllowIn,
        parameters: &FormalParameters,
    ) -> Result<(ArrowBody, Span, bool), ParseError> {
        if self.current.kind == TokenKind::LBrace {
            // An arrow has no `this` and no home object of its own, so it does not stop
            // `super` either — which is what makes `constructor() { () => super(); }` legal.
            let (body, end, declares_strict) = self.parse_function_body(self.body_context)?;
            // §15.3.1 borrows §15.2.1's: the parameters were read before the body could say it
            // was strict, so this is the one rule that has to wait for the body.
            if declares_strict && !parameters.is_simple() {
                return Err(ParseError {
                    kind: ParseErrorKind::UseStrictWithNonSimpleParameters,
                    span: parameters.span,
                });
            }
            return Ok((ArrowBody::Block(body), end, declares_strict));
        }
        // `ExpressionBody : AssignmentExpression[?In]` — so `a => b, c` is `(a => b), c`, the
        // comma being outside the body.
        let value = self.parse_assignment(allow_in)?;
        let end = value.span;
        Ok((ArrowBody::Expression(Box::new(value)), end, false))
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        match parse_script(source) {
            Err(err) => err.kind,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // needs the error
        }
    }

    #[test]
    fn both_shapes_of_arrow_parameters_and_both_shapes_of_body() {
        assert_eq!(shape("a => b"), "(=> [a] b)");
        assert_eq!(shape("(a) => b"), "(=> [a] b)");
        assert_eq!(shape("() => b"), "(=> [] b)");
        assert_eq!(shape("(a, b) => c"), "(=> [a b] c)");
        assert_eq!(shape("(a,) => b"), "(=> [a] b)");
        assert_eq!(shape("(...a) => b"), "(=> [(... a)] b)");
        assert_eq!(shape("(a, ...b) => c"), "(=> [a (... b)] c)");
        assert_eq!(shape("(a = 1) => b"), "(=> [(= a 1)] b)");
        assert_eq!(shape("([a]) => b"), "(=> [[a]] b)");
        assert_eq!(shape("({a}) => b"), "(=> [{(a a)}] b)");
        assert_eq!(shape("({a: b}) => c"), "(=> [{(a b)}] c)");
        assert_eq!(shape("([a, ...b]) => c"), "(=> [[a (... b)]] c)");
        // `ConciseBody : [lookahead ≠ {] ExpressionBody | { FunctionBody }` — the lookahead is
        // why an object literal body needs parentheses.
        assert_eq!(shape("() => {}"), "(=> [] {})");
        assert_eq!(shape("() => ({})"), "(=> [] {})");
        assert_eq!(shape("a => { return 1; }"), "(=> [a] {(return 1)})");
        // `ExpressionBody : AssignmentExpression`, so the body takes everything an assignment
        // does and stops where one does: `a => b, c` is `(a => b), c`.
        assert_eq!(shape("a => b + c"), "(=> [a] (+ b c))");
        assert_eq!(shape("a => b ? c : d"), "(=> [a] (? b c d))");
        assert_eq!(shape("a => b = c"), "(=> [a] (= b c))");
        assert_eq!(shape("a => b, c"), "(, (=> [a] b) c)");
        assert_eq!(shape("a => b => c"), "(=> [a] (=> [b] c))");
        assert_eq!(shape("a => b.c"), "(=> [a] (. b c))");
    }

    #[test]
    fn an_arrow_may_stand_only_where_an_assignment_expression_may() {
        // §15.3: an `ArrowFunction` is an `AssignmentExpression` and nothing tighter, so every
        // operator that wants a narrower operand refuses one — which is the whole reason this is
        // decided at the assignment level rather than where the `(` is read.
        for source in [
            "x + (a) => b;",
            "x + a => b;",
            "x * (a) => b;",
            "typeof (a) => b;",
            "-(a) => b;",
            "!(a) => b;",
            "new (a) => b;",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // …and every place one may: a comma operand, an argument, an element, an arm.
        assert!(parse_script("x, (a) => b;").is_ok());
        assert!(parse_script("f(a => b);").is_ok());
        assert!(parse_script("[a => b];").is_ok());
        assert!(parse_script("a ? (b) => c : d;").is_ok());
        assert!(parse_script("x = a => b;").is_ok());
        assert!(parse_script("({a: b => c});").is_ok());
        // Parenthesized, it is an operand like any other.
        assert_eq!(shape("((a) => b).c"), "(. (=> [a] b) c)");
        assert_eq!(shape("((a) => b) ** c"), "(** (=> [a] b) c)");
    }

    #[test]
    fn a_newline_before_the_arrow_means_there_was_no_arrow() {
        // `ArrowParameters [no LineTerminator here] =>`. The restriction does not make a bad
        // arrow — it makes what came before an ordinary expression, and then nothing wanted the
        // `=>`. Which is why the error names the token rather than the arrow.
        assert_eq!(
            kind("a\n=> b;"),
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: crate::lexer::TokenKind::Arrow,
            }
        );
        assert!(parse_script("(a)\n=> b;").is_err());
        // …and the restriction is only before the `=>`, so the body may start on the next line.
        assert!(parse_script("(a) =>\nb;").is_ok());
        assert!(parse_script("a =>\n{ return; };").is_ok());
    }

    #[test]
    fn three_of_the_covers_productions_are_not_expressions_at_all() {
        // `()`, `( Expression , )` and anything with a `...` are productions of
        // `CoverParenthesizedExpressionAndArrowParameterList` and of nothing else. Without a `=>`
        // there is nothing for them to be.
        for source in ["();", "(a,);", "(...a);", "(a, ...b);", "(a, b,);"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::CoverGroupIsNotAnExpression,
                "{source:?}"
            );
        }
        // …while `( Expression )` is one, comma and all.
        assert_eq!(shape("(a)"), "a");
        assert_eq!(shape("(a, b)"), "(, a b)");
        assert!(parse_expression("(a)").expect("this parses").parenthesized);
        // The group is read once. Whatever it turns out to be, it is not read again — which is
        // what these two prove, the second having suffixes that only apply to an operand.
        assert_eq!(shape("(a, b) => c"), "(=> [a b] c)");
        assert_eq!(shape("(a, b).c"), "(. (, a b) c)");
        assert_eq!(shape("(a)++"), "(post++ a)");
        assert_eq!(shape("(a)(b)"), "(call a [b])");
    }

    #[test]
    fn a_parameter_is_a_binding_so_it_is_narrower_than_an_assignment_target() {
        // The refinement is to a `Binding`, not to a `Pattern`: arrow parameters create names.
        for source in [
            "(a.b) => c;",
            "(1) => b;",
            "([a.b]) => c;",
            "({a: b.c}) => d;",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::InvalidArrowParameter,
                "{source:?}"
            );
        }
        // …where the very same shapes are fine as assignment targets, which is the comparison
        // that shows these are two grammars and not one.
        assert!(parse_script("[a.b] = c;").is_ok());
        assert!(parse_script("({a: b.c} = d);").is_ok());
        // `UniqueFormalParameters`, so a repeat is refused however simple the list — as a
        // method's is, and unlike a plain function's.
        assert_eq!(kind("(a, a) => b;"), ParseErrorKind::DuplicateParameterName);
        assert_eq!(
            kind("([a], a) => b;"),
            ParseErrorKind::DuplicateParameterName
        );
        // Only `=` covers a default, so a compound assignment is no parameter at all.
        assert_eq!(
            kind("(a += 1) => b;"),
            ParseErrorKind::InvalidArrowParameter
        );
        // …and nothing follows a `...` inside a parameter's own pattern either.
        assert_eq!(
            kind("([...a, b]) => c;"),
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            kind("({...a, b}) => c;"),
            ParseErrorKind::RestElementMustBeLast
        );
        assert!(parse_script("([...a]) => b;").is_ok());
        assert!(parse_script("({...a}) => b;").is_ok());
        // A comma after the list's own `...` is caught by the parenthesis that never came.
        assert!(parse_script("(...a,) => b;").is_err());
        assert!(parse_script("function f(a, a) {}").is_ok());
        // The body's rules are a function body's, this being one.
        assert_eq!(
            kind("(a) => { let a; };"),
            ParseErrorKind::ParameterRedeclaredInBody
        );
        assert!(parse_script("() => { return; };").is_ok());
        assert_eq!(
            kind("() => { break; };"),
            ParseErrorKind::BreakOutsideLoop,
            "an arrow body is where the walks stop, like any other function body"
        );
    }

    #[test]
    fn the_parameters_were_references_and_the_strict_rules_are_about_bindings() {
        // §13.1.1 judges a reference by looser rules than a binding — `eval` may be read in
        // strict code and may not be bound — so the binding rules are applied where the
        // references become bindings, and not before.
        assert_eq!(
            kind("\"use strict\"; (eval) => 1;"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("\"use strict\"; ([eval]) => 1;"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("\"use strict\"; (let) => 1;"),
            ParseErrorKind::StrictReservedWord
        );
        assert!(parse_script("(eval) => 1;").is_ok());
        // …and a body that declares itself strict reaches back to them, as a function's does.
        assert_eq!(
            kind("(eval) => { \"use strict\"; };"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("(a = 1) => { \"use strict\"; };"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        assert!(parse_script("(a) => { \"use strict\"; };").is_ok());
    }

    #[test]
    fn no_arrow_however_truncated_can_panic() {
        let cases = [
            "(".to_string(),
            "(a".to_string(),
            "(a,".to_string(),
            "(...".to_string(),
            "a =>".to_string(),
            "() =>".to_string(),
            "() => {".to_string(),
            "a => ".repeat(1000),
            "(".repeat(10_000),
            format!(
                "({}) => 1",
                (0..10_000).map(|i| format!("a{i}, ")).collect::<String>()
            ),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // Arrows nest through their bodies, so they are bounded by the cap.
        assert_eq!(
            kind(&format!("{}1;", "a => ".repeat(1000))),
            ParseErrorKind::TooDeeplyNested
        );
        // …while a long parameter list is a loop.
        let many: String = (0..2_000).map(|i| format!("a{i}, ")).collect();
        assert!(parse_expression(&format!("({many}b) => 1")).is_ok());
    }
}
