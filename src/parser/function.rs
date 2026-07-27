//! Function definitions (ECMAScript §15.2), and the `return` they make legal (§14.10).
//!
//! # `[Return]` is a field and `[In]` was a parameter
//!
//! `ReturnStatement` is an alternative of `Statement[Return]` and appears only under `[+Return]`,
//! which a `FunctionBody` is the only thing that sets. Every other statement production passes
//! `[?Return]` straight down, and nothing anywhere turns it back off — so unlike `[In]`, which
//! resets at every bracket and therefore has to be a decision at each one, this is a single fact
//! about where you are. It is [`super::Parser::inside_function`], saved and restored around a
//! body, and the restoring is the part with teeth: `function f() {} return;` must fail.
//!
//! # What the parameters may repeat, and when
//!
//! `FormalParameters` is *not* `UniqueFormalParameters`, so `function f(a, a) {}` is legal — two
//! parameters of one name, the second winning. §15.1.1 takes that away the moment the list stops
//! being simple: `function f(a, a = 1) {}` and `function f(a, [a]) {}` are both Syntax Errors,
//! because a non-simple list is initialised by running code and running code needs to know which
//! `a` it is talking about.
//!
//! # The body is a boundary
//!
//! §15.2.1 asks of every `FunctionStatementList` exactly what §16.1.1 asks of a `Script`: no
//! duplicate lexical names, no lexical name that is also var-declared, and §8.3's three rules
//! about labels. It asks them *again* rather than inheriting, because a function body is where
//! those walks stop — `while (1) { function f() { break; } }` is a Syntax Error, the `break`
//! being unable to see the loop it appears to be in.
//!
//! One rule of §15.2.1 is this file's own: no name bound by the parameters may also be
//! lexically declared by the body. `function f(a) { let a; }` is refused and
//! `function f(a) { var a; }` is not, the second being the same binding twice rather than two.

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Binding, BindingElement, ExprKind, FormalParameters, Function, Stmt, StmtKind};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;
use crate::static_semantics::{bound_names, top_level_lexically_declared_names};
use std::collections::HashSet;

impl Parser<'_> {
    /// `FunctionDeclaration` (§15.2), with the cursor on `function`.
    ///
    /// Named, always: the anonymous alternative is `[+Default]`, which only an `export default`
    /// reaches, and there are no modules yet.
    pub(super) fn parse_function_declaration(&mut self) -> Result<Stmt, ParseError> {
        let function = self.parse_function(true)?;
        Ok(Stmt {
            span: function.span,
            kind: StmtKind::Function(Box::new(function)),
        })
    }

    /// `FunctionExpression` (§15.2), with the cursor on `function`.
    pub(super) fn parse_function_expression(&mut self) -> Result<crate::ast::Expr, ParseError> {
        let function = self.parse_function(false)?;
        let span = function.span;
        Ok(crate::ast::Expr::new(
            ExprKind::Function(Box::new(function)),
            span,
        ))
    }

    /// Both forms, which differ only in whether the name may be left out.
    fn parse_function(&mut self, name_required: bool) -> Result<Function, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let name = if self.current.kind == TokenKind::LParen && !name_required {
            None
        } else {
            Some(self.parse_binding_name()?)
        };
        self.enter()?;
        let parts = self.parse_function_parts();
        self.leave();
        let (parameters, body, end) = parts?;
        Ok(Function {
            name,
            parameters,
            body,
            span: keyword.span.to(end),
        })
    }

    /// The parameters and the body, apart so their locals are not carried by every level of
    /// nesting that passes through [`Parser::parse_function`].
    fn parse_function_parts(
        &mut self,
    ) -> Result<(FormalParameters, Box<[Stmt]>, Span), ParseError> {
        let parameters = self.parse_formal_parameters()?;
        let (body, end) = self.parse_function_body()?;
        check_parameters_against_body(&parameters, &body)?;
        Ok((parameters, body, end))
    }

    /// `( FormalParameters )` (§15.1).
    fn parse_formal_parameters(&mut self) -> Result<FormalParameters, ParseError> {
        let open = self.eat(TokenKind::LParen, Goal::RegExp, "`(`")?;
        let mut items: Vec<BindingElement> = Vec::new();
        let mut rest: Option<Box<Binding>> = None;
        while self.current.kind != TokenKind::RParen {
            if self.current.kind == TokenKind::DotDotDot {
                self.advance(Goal::RegExp)?;
                rest = Some(Box::new(self.parse_binding()?));
            } else {
                items.push(self.parse_binding_element()?);
            }
            if self.current.kind != TokenKind::Comma {
                break;
            }
            // `FormalParameterList , FunctionRestParameter` puts the rest last and allows no
            // trailing comma after it — so a comma here, with a rest already read, has no
            // derivation whether or not anything follows.
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span: self.current.span,
                });
            }
            self.advance(Goal::RegExp)?;
        }
        let close = self.eat(TokenKind::RParen, Goal::RegExp, "`)`")?;
        let parameters = FormalParameters {
            items: items.into_boxed_slice(),
            rest,
            span: open.span.to(close.span),
        };
        check_parameter_names(&parameters)?;
        Ok(parameters)
    }

    /// `{ FunctionBody }` (§15.2), and everything that being a boundary implies.
    fn parse_function_body(&mut self) -> Result<(Box<[Stmt]>, Span), ParseError> {
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        // `[+Return]`, which only this production sets — and which is restored on the way out
        // even when the body fails, so that a `return` after the function is still refused.
        let enclosing = self.inside_function;
        self.inside_function = true;
        let body = self.parse_statement_list(TokenKind::RBrace);
        self.inside_function = enclosing;
        let body = body?;
        let close = self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        // §15.2.1 asks of a FunctionStatementList exactly what §16.1.1 asks of a Script, and asks
        // it again rather than inheriting — this is where those walks stop.
        super::scope::check_declared_names(&body, super::scope::Level::Top)?;
        super::scope::check_labels(&body)?;
        Ok((body, close.span))
    }

    /// `ReturnStatement : return [no LineTerminator here] Expression_opt ;` (§14.10).
    ///
    /// A restricted production, and the third shape of one: unlike `throw` it has a shorter form
    /// to fall back on, so a value on the next line does not fail — it becomes the next statement
    /// and this becomes a bare `return`.
    pub(super) fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        // `Statement[Return]` — a `ReturnStatement` is an alternative only under `[+Return]`, so
        // outside a function body there is no such statement rather than a bad one.
        if !self.inside_function {
            return Err(ParseError {
                kind: ParseErrorKind::ReturnOutsideFunction,
                span: keyword.span,
            });
        }
        let value = if self.current.newline_before
            || matches!(
                self.current.kind,
                TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
            ) {
            None
        } else {
            self.enter()?;
            let value = self.parse_expression(super::expression::AllowIn::Yes);
            self.leave();
            Some(Box::new(value?))
        };
        let end = self.consume_semicolon(value.as_ref().map_or(keyword.span, |v| v.span))?;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::Return(value),
        })
    }
}

/// §15.1.1: a non-simple parameter list may not repeat a name.
///
/// `FormalParameters` is not `UniqueFormalParameters`, so a simple list may — `function f(a, a)`
/// binds `a` twice and the second wins. The moment a default, a pattern or a rest appears, the
/// parameters are initialised by running code, and that code has to know which `a` it means.
fn check_parameter_names(parameters: &FormalParameters) -> Result<(), ParseError> {
    if parameters.is_simple() {
        return Ok(());
    }
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
    if let Some(rest) = &parameters.rest {
        for declared in bound_names(rest) {
            if !seen.insert(declared.name.to_string()) {
                return Err(ParseError {
                    kind: ParseErrorKind::DuplicateParameterName,
                    span: declared.span,
                });
            }
        }
    }
    Ok(())
}

/// §15.2.1: no parameter name may also be lexically declared by the body.
///
/// `function f(a) { let a; }` is refused and `function f(a) { var a; }` is not — the second is one
/// binding written twice, and the first is two bindings of one name in overlapping scopes.
/// `TopLevel` names, because a function body is a top level: a `function a() {}` in there is
/// var-scoped and so does not clash either.
fn check_parameters_against_body(
    parameters: &FormalParameters,
    body: &[Stmt],
) -> Result<(), ParseError> {
    let lexical = top_level_lexically_declared_names(body);
    let mut names: Vec<crate::static_semantics::DeclaredName<'_>> = Vec::new();
    for element in &parameters.items {
        names.extend(bound_names(&element.target));
    }
    if let Some(rest) = &parameters.rest {
        names.extend(bound_names(rest));
    }
    for declared in lexical {
        if names.iter().any(|bound| bound.name == declared.name) {
            return Err(ParseError {
                kind: ParseErrorKind::ParameterRedeclaredInBody,
                span: declared.span,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseError, ParseErrorKind, parse_script};

    /// The statements of `source`, rendered compactly.
    fn statements(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // needs the tree
        script.body.iter().map(render_statement).collect()
    }

    /// The error `source` fails with.
    fn script_error(source: &str) -> ParseError {
        match parse_script(source) {
            Err(err) => err,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // needs the error
        }
    }

    #[test]
    fn a_declaration_must_be_named_and_an_expression_need_not_be() {
        assert_eq!(statements("function f() {}"), ["(fn f [] {})"]);
        assert_eq!(statements("function f() { a; }"), ["(fn f [] {a})"]);
        assert_eq!(statements("(function () {});"), ["(fn <anon> [] {})"]);
        assert_eq!(statements("(function f() {});"), ["(fn f [] {})"]);
        assert_eq!(
            statements("var x = function () {};"),
            ["(var x=(fn <anon> [] {}))"]
        );
        assert_eq!(
            statements("typeof function () {};"),
            ["(typeof (fn <anon> [] {}))"]
        );
        // The anonymous *declaration* is the `[+Default]` alternative, which only an
        // `export default` reaches — and there are no modules.
        assert!(parse_script("function () {}").is_err());
        // §14.5 keeps an `ExpressionStatement` from beginning with `function`, so an expression
        // needs something in front of it to be reached at all.
        assert_eq!(
            script_error("function () {}").kind,
            ParseErrorKind::Unexpected {
                expected: "a binding name",
                found: crate::lexer::TokenKind::LParen,
            }
        );
    }

    #[test]
    fn parameters_are_binding_elements_and_the_rest_is_last() {
        assert_eq!(statements("function f(a) {}"), ["(fn f [a] {})"]);
        assert_eq!(statements("function f(a, b) {}"), ["(fn f [a b] {})"]);
        assert_eq!(statements("function f(a,) {}"), ["(fn f [a] {})"]);
        assert_eq!(statements("function f() {}"), ["(fn f [] {})"]);
        // The same `BindingElement` a declaration takes, so patterns and defaults come free.
        assert_eq!(statements("function f(a = 1) {}"), ["(fn f [(= a 1)] {})"]);
        assert_eq!(statements("function f([a]) {}"), ["(fn f [[a]] {})"]);
        assert_eq!(statements("function f({a}) {}"), ["(fn f [{(a a)}] {})"]);
        assert_eq!(
            statements("function f([a] = b) {}"),
            ["(fn f [(= [a] b)] {})"]
        );
        assert_eq!(statements("function f(...a) {}"), ["(fn f [(... a)] {})"]);
        assert_eq!(
            statements("function f(a, ...b) {}"),
            ["(fn f [a (... b)] {})"]
        );
        assert_eq!(
            statements("function f(...[a]) {}"),
            ["(fn f [(... [a])] {})"]
        );
        // `FormalParameterList , FunctionRestParameter` puts the rest last and allows nothing
        // after it, not even a comma.
        assert_eq!(
            script_error("function f(...a, b) {}").kind,
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            script_error("function f(...a,) {}").kind,
            ParseErrorKind::RestElementMustBeLast
        );
    }

    #[test]
    fn a_simple_parameter_list_may_repeat_a_name_and_no_other_kind_may() {
        // `FormalParameters` is not `UniqueFormalParameters`, so this binds `a` twice and the
        // second wins. It is the only place in the language where a duplicate binding is fine.
        assert_eq!(statements("function f(a, a) {}"), ["(fn f [a a] {})"]);
        assert_eq!(statements("function f(a, a, a) {}"), ["(fn f [a a a] {})"]);
        // §15.1.1 takes that away the moment the list stops being simple — a default, a pattern
        // or a rest — because then the parameters are initialised by running code.
        for source in [
            "function f(a, a = 1) {}",
            "function f(a = 1, a) {}",
            "function f([a, a]) {}",
            "function f(a, [a]) {}",
            "function f(a, ...a) {}",
            "function f({b: a}, [a]) {}",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::DuplicateParameterName,
                "{source:?}"
            );
        }
        // …and a list that is not simple but has no repeat is fine.
        assert!(parse_script("function f(a, b = 1) {}").is_ok());
        assert!(parse_script("function f([a], {b}) {}").is_ok());
    }

    #[test]
    fn return_is_a_statement_only_where_a_function_body_says_so() {
        assert_eq!(
            statements("function f() { return; }"),
            ["(fn f [] {return})"]
        );
        assert_eq!(
            statements("function f() { return 1; }"),
            ["(fn f [] {(return 1)})"]
        );
        assert_eq!(
            statements("function f() { return a, b; }"),
            ["(fn f [] {(return (, a b))})"]
        );
        assert_eq!(
            statements("function f() { if (x) return; }"),
            ["(fn f [] {(if x return)})"]
        );
        // `Statement[Return]`, and only a `FunctionBody` sets it — so outside one there is no
        // such statement rather than a bad one.
        assert_eq!(
            script_error("return;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        assert_eq!(
            script_error("{ return; }").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        assert_eq!(
            script_error("for (;;) return;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        // The restoring is the half with teeth: after the body, it is off again.
        assert_eq!(
            script_error("function f() {} return;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        assert_eq!(
            script_error("function f() { return; } return;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        // …and it is restored even when the body fails, so a later `return` is still refused.
        assert!(parse_script("function f() { @ }").is_err());
        assert_eq!(
            script_error("return;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        // A nested function sets it again, which changes nothing while it is already set.
        assert!(parse_script("function f() { function g() { return; } return; }").is_ok());
        // The restricted production, and the third shape of one: unlike `throw` there is a
        // shorter form to fall back on, so a value on the next line is the next statement.
        assert_eq!(
            statements("function f() { return\n1; }"),
            ["(fn f [] {return 1})"],
            "a bare return, then the expression statement `1`"
        );
        assert_eq!(
            statements("function f() { return }"),
            ["(fn f [] {return})"],
            "…and a closing brace ends it, by §12.10 rule 1"
        );
    }

    #[test]
    fn a_function_body_is_where_the_walks_stop_and_start_again() {
        // §15.2.1 asks of a FunctionStatementList what §16.1.1 asks of a Script.
        assert_eq!(
            script_error("function f() { let a; let a; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("function f() { let a; var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("function f() { a: a: ; }").kind,
            ParseErrorKind::DuplicateLabel
        );
        assert_eq!(
            script_error("function f() { break a; }").kind,
            ParseErrorKind::UndefinedBreakTarget
        );
        // …and asks them from scratch, because this is where those walks stop. The `break` here
        // cannot see the loop it appears to be inside.
        assert_eq!(
            script_error("while (1) { function f() { break; } }").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("a: while (1) { function f() { continue a; } }").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        assert_eq!(
            script_error("a: { function f() { break a; } }").kind,
            ParseErrorKind::UndefinedBreakTarget
        );
        // …so a label inside may repeat one outside, there being no enclosure across the wall.
        assert!(parse_script("a: { function f() { a: ; } }").is_ok());
        // A `var` in a function belongs to that function and not to the one outside it.
        assert!(parse_script("let a; function f() { var a; }").is_ok());
        assert!(parse_script("function f() { let a; function g() { var a; } }").is_ok());
        // §15.2.1's own rule: a parameter name may not also be lexically declared by the body.
        assert_eq!(
            script_error("function f(a) { let a; }").kind,
            ParseErrorKind::ParameterRedeclaredInBody
        );
        assert_eq!(
            script_error("function f([a]) { const a = 1; }").kind,
            ParseErrorKind::ParameterRedeclaredInBody
        );
        // `var` is the same binding written twice, not two — and a nested block is its own scope.
        assert!(parse_script("function f(a) { var a; }").is_ok());
        assert!(parse_script("function f(a) { { let a; } }").is_ok());
    }

    #[test]
    fn a_function_is_var_scoped_at_a_top_level_and_lexical_anywhere_else() {
        // §8.2.10 and §8.2.12, which have differed from their plain siblings in nothing until
        // now: a `HoistableDeclaration` is on the opposite side of each.
        assert!(parse_script("function f() {} function f() {}").is_ok());
        assert!(parse_script("var f; function f() {}").is_ok());
        assert!(parse_script("function f() { function g() {} function g() {} }").is_ok());
        // …so it collides with a lexical name at that level, being var-scoped.
        assert_eq!(
            script_error("let f; function f() {}").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("function f() {} let f;").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("function f() { let g; function g() {} }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        // In a block it is lexical instead, so the collision is a duplicate rather than a clash.
        assert_eq!(
            script_error("{ let f; function f() {} }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("{ function f() {} function f() {} }").kind,
            ParseErrorKind::DuplicateLexicalBinding,
            "refused; §14.2.1's web-compat carve-out for this needs strict mode to state"
        );
        // …and a function in a nested block is not var-declared at the level above it, which is
        // what "directly" means in §8.2.12.
        assert!(parse_script("function f() { { function g() {} } let g; }").is_ok());
        assert!(parse_script("{ function f() {} } let f;").is_ok());
    }

    #[test]
    fn the_three_shapes_a_web_host_would_take_are_refused_together() {
        // Annex B.3.2 lets a `FunctionDeclaration` be the body of an `if` in non-strict code, and
        // §14.13.1 lets one be labelled. Both exemptions turn on strictness, which this parser
        // cannot yet tell — so both are refused, for the reason Annex B.3.5 was: accepting would
        // be wrong in strict code on every host, and refusing is wrong only for sloppy code on a
        // host that implements them. V8 takes all three; these are the divergences, and they go
        // away together with strict mode.
        for source in ["if (x) function f() {}", "a: function f() {}"] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::FunctionInStatementPosition,
                "{source:?}"
            );
        }
        // …and this one has no exemption anywhere, an `IterationStatement` body being a
        // `Statement` in every dialect.
        assert_eq!(
            script_error("while (x) function f() {}").kind,
            ParseErrorKind::FunctionInStatementPosition
        );
    }

    #[test]
    fn no_function_however_truncated_can_panic() {
        let cases = [
            "function".to_string(),
            "function f".to_string(),
            "function f(".to_string(),
            "function f()".to_string(),
            "function f() {".to_string(),
            "function f(a".to_string(),
            "function f(...".to_string(),
            "(function".to_string(),
            "function f() { return".to_string(),
            "function f() { ".repeat(1000),
            format!("function f({}) {{}}", "a, ".repeat(100_000)),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // Functions nest, so they are bounded by the cap rather than by memory.
        assert_eq!(
            script_error(&"function f() { ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
        // …while a long parameter list is a loop. Every name differs, so nothing collides.
        let many: String = (0..5_000).map(|i| format!("a{i}, ")).collect();
        assert!(parse_script(&format!("function f({many}b) {{}}")).is_ok());
    }
}
