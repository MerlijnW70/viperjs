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

use super::body::BodyContext;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Binding, BindingElement, ExprKind, FormalParameters, Function, Stmt, StmtKind};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;
use crate::static_semantics::{bound_names, top_level_lexically_declared_names};
use std::collections::HashSet;

/// Whether a `{ … }` body is a boundary the static semantics stop at.
///
/// An arrow's is not, and that is not a detail of this parser: §8.2.2's `Contains` and §15.7.9's
/// `ContainsArguments` both recurse through an `ArrowFunction` where they stop at every other
/// function. It is what makes `class C { x = () => arguments }` a Syntax Error and
/// `class C { x = function () { return arguments; } }` an ordinary field — an arrow has no
/// `arguments` of its own for the name to mean, so the name still means the enclosing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    /// A `FunctionBody`, which every static semantic stops at.
    Function,
    /// An `ArrowFunction`'s block `ConciseBody`, which they see through.
    Arrow,
}

impl Parser<'_> {
    /// `FunctionDeclaration` (§15.2) or `AsyncFunctionDeclaration` (§15.8), with the cursor on
    /// `function` or on the `async` before it.
    ///
    /// Named, always: the anonymous alternative is `[+Default]`, which only an `export default`
    /// reaches, and there are no modules yet.
    pub(super) fn parse_function_declaration(
        &mut self,
        is_async: bool,
    ) -> Result<Stmt, ParseError> {
        let function = self.parse_function(true, is_async, Goal::RegExp)?;
        Ok(Stmt {
            span: function.span,
            kind: StmtKind::Function(Box::new(function)),
        })
    }

    /// Whether a `FunctionDeclaration` stands here that Annex B lets into a `Statement` position.
    ///
    /// Two positions ask: §B.3.4's `if (x) function f() {}` and §B.3.2's `L: function f() {}`. Both
    /// name the production `FunctionDeclaration`, and the whole of what this decides is whether the
    /// text at the cursor is one.
    ///
    /// Strictness first, because DR-0008's amendment makes it the only condition Annex B is allowed
    /// to have here — a fact the compiler can already read off the directive prologue.
    ///
    /// Then a token past `function`, because a `GeneratorDeclaration` is **not** a
    /// `FunctionDeclaration` and Annex B does not extend to one: `if (x) function* g() {}` has no
    /// derivation with or without B.3. `async function` needs no test at all — `async` is an
    /// ordinary identifier to the lexer, so the cursor is not on `function` and this answers no,
    /// which leaves [`Parser::parse_statement`] to refuse it exactly as it did before.
    pub(super) fn at_annex_b_function(&mut self) -> Result<bool, ParseError> {
        if self.strict
            || self.current.kind != TokenKind::Keyword(crate::lexer::ReservedWord::Function)
        {
            return Ok(false);
        }
        // `Goal::Div` rather than `Goal::RegExp`: nothing legal follows `function` with a `/`, and
        // under the other goal an unterminated regular expression would come back as a lexical
        // error from a token this only wants to *look* at.
        Ok(self.peek(Goal::Div)?.kind != TokenKind::Star)
    }

    /// §16.2.3's `[+Default]` `HoistableDeclaration`, with the cursor on `function` or the
    /// `async` before it.
    ///
    /// The one position where a declaration may be anonymous: `export default function () {}`
    /// binds `*default*` and needs no name of its own.
    pub(super) fn parse_default_function(&mut self, is_async: bool) -> Result<Stmt, ParseError> {
        let function = self.parse_function(false, is_async, Goal::RegExp)?;
        Ok(Stmt {
            span: function.span,
            kind: StmtKind::Function(Box::new(function)),
        })
    }

    /// `FunctionExpression` (§15.2) or `AsyncFunctionExpression` (§15.8), with the cursor on
    /// `function` or on the `async` before it.
    pub(super) fn parse_function_expression(
        &mut self,
        is_async: bool,
    ) -> Result<crate::ast::Expr, ParseError> {
        let function = self.parse_function(false, is_async, Goal::Div)?;
        let span = function.span;
        Ok(crate::ast::Expr::new(
            ExprKind::Function(Box::new(function)),
            span,
        ))
    }

    /// Both forms, which differ only in whether the name may be left out.
    ///
    /// `after` is the goal the token following the closing `}` is read under, and it is the
    /// caller's to give because the same production is both a declaration and an expression.
    /// A declaration ends a statement, so an operand comes next and a `/` there opens a regular
    /// expression: `function f() {}` then `/re/.test(x)` is two statements. An expression is
    /// followed by an operator instead. §12.6 makes this the parser's call, and it is the one
    /// place a `FunctionBody` cannot decide for itself — see [`Goal`].
    fn parse_function(
        &mut self,
        name_required: bool,
        is_async: bool,
        after: Goal,
    ) -> Result<Function, ParseError> {
        // `async [no LineTerminator here] function` — the caller checked both, and the `async` is
        // still the current token when it did not.
        if is_async {
            self.advance(Goal::RegExp)?;
        }
        let keyword = self.advance(Goal::RegExp)?;
        // `function *` — the one bit of syntax between §15.2's productions and §15.5's, and
        // nothing about it is restricted — no `[no LineTerminator here]`, so `function*g`,
        // `function * g` and a `*` on its own line are the same generator.
        let is_generator = self.current.kind == TokenKind::Star;
        if is_generator {
            self.advance(Goal::RegExp)?;
        }
        let name = self.parse_function_name(name_required, is_generator, is_async)?;
        self.enter()?;
        let parts = self.parse_function_parts(is_generator, is_async, after);
        self.leave();
        let FunctionParts {
            parameters,
            body,
            end,
            is_strict,
        } = parts?;
        // §15.2.1 — a function whose **body** is strict may not be called `eval` or `arguments`,
        // and the body says so after the name has been read. `parse_function_name` asks the same
        // question with the *enclosing* strictness, which is the right answer for
        // `"use strict"; function arguments() {}` and the wrong one for
        // `function arguments() { "use strict"; }`.
        if is_strict
            && !self.strict
            && let Some(written) = &name
        {
            check_strict_name(&written.name, written.span)?;
        }
        Ok(Function {
            name,
            parameters,
            body,
            is_strict,
            is_generator,
            is_async,
            span: keyword.span.to(end),
        })
    }

    /// The `BindingIdentifier`, under the `[Yield]` the production gives it.
    ///
    /// A *declaration's* name is `[?Yield]`: it is a binding in the scope around the function, so
    /// it is read under whatever that scope has, and `function* g() { function yield() {} }` is
    /// refused. An *expression's* name belongs to the function and is read under the function's
    /// own — `[~Yield]` for a plain one, `[+Yield]` for a generator — so
    /// `function* g() { (function yield() {}); }` parses and `(function* yield() {})` does not.
    fn parse_function_name(
        &mut self,
        name_required: bool,
        is_generator: bool,
        is_async: bool,
    ) -> Result<Option<crate::ast::BindingName>, ParseError> {
        if self.current.kind == TokenKind::LParen && !name_required {
            return Ok(None);
        }
        let enclosing = (self.yield_allowed, self.await_allowed);
        if !name_required {
            self.yield_allowed = is_generator;
            self.await_allowed = is_async;
        }
        let name = self.parse_binding_name();
        (self.yield_allowed, self.await_allowed) = enclosing;
        Ok(Some(name?))
    }

    /// The parameters and the body, apart so their locals are not carried by every level of
    /// nesting that passes through [`Parser::parse_function`].
    fn parse_function_parts(
        &mut self,
        is_generator: bool,
        is_async: bool,
        after: Goal,
    ) -> Result<FunctionParts, ParseError> {
        // `FormalParameters[+Yield]` for a generator and `[~Yield]` for everything else — which
        // is why a plain function nested in a generator may still take a parameter called
        // `yield`. The refusal of a `YieldExpression` among them lives there too.
        //
        // The parameters are *inside* the function, so what the function grants they are granted
        // — and what it takes away they lose. A plain function is where `super` stops, so
        // `class C extends D { m() { function f(x = super.foo) {} } }` is a Syntax Error even
        // though the same text one level out is fine. Restored around the read rather than set
        // once, because a parameter default may contain a whole function of its own.
        let enclosing_context = self.body_context;
        self.body_context = BodyContext::FUNCTION;
        let parameters = self.parse_parameters_of(is_generator, is_async);
        self.body_context = enclosing_context;
        let parameters = parameters?;
        // A plain function is where `super` stops, however deep inside a method it is written:
        // §15.2.1 makes a `FunctionBody` containing either form a Syntax Error outright.
        let enclosing = (self.yield_allowed, self.await_allowed);
        self.yield_allowed = is_generator;
        self.await_allowed = is_async;
        let parts = self.parse_function_body(BodyContext::FUNCTION, after, Boundary::Function);
        (self.yield_allowed, self.await_allowed) = enclosing;
        let (body, end, declares_strict) = parts?;
        check_parameters_against_body(&parameters, &body)?;
        // The two rules that cannot be applied while the parameters are read, because the body
        // has not said whether it is strict yet.
        if declares_strict && !parameters.is_simple() {
            return Err(ParseError {
                kind: ParseErrorKind::UseStrictWithNonSimpleParameters,
                span: parameters.span,
            });
        }
        // The body's own strictness: its directive, or the enclosing code's, because strictness is
        // inherited and never given back. `self.strict` is the *enclosing* value here — the body's own
        // was restored on the way out of `parse_function_body` — so this is exactly the union.
        let is_strict = declares_strict || self.strict;
        if is_strict {
            check_strict_parameters(&parameters)?;
        }
        Ok(FunctionParts {
            parameters,
            body,
            end,
            is_strict,
        })
    }

    /// `( FormalParameters )` (§15.1).
    pub(super) fn parse_formal_parameters(&mut self) -> Result<FormalParameters, ParseError> {
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
    /// What `super` and `new.target` may mean inside is the caller's to say, because that is
    /// the one thing a function body does not decide for itself: a method's body has `super`
    /// and the very same production written as a plain function does not. See [`super::body`].
    pub(super) fn parse_function_body(
        &mut self,
        body_context: BodyContext,
        after: Goal,
        boundary: Boundary,
    ) -> Result<(Box<[Stmt]>, Span, bool), ParseError> {
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        // `[+Return]`, which only this production sets — and which is restored on the way out
        // even when the body fails, so that a `return` after the function is still refused. The
        // same for strictness, which a body may switch on for itself and never off, and for
        // `super`, whose two permissions both stop at a function boundary.
        let enclosing = self.inside_function;
        let enclosing_strict = self.strict;
        let enclosing_context = self.body_context;
        // `Contains` stops at a function boundary, and this is the boundary — so a `yield`
        // written in here is never a `yield` written in the parameter list that encloses it.
        let enclosing_forbidden_in_parameters = self.forbidden_in_parameters.take();
        // §15.7.9's `ContainsArguments` stops here too, and at nothing smaller — **which is why
        // an arrow's body is not a boundary for it**. This function serves both, and taking the
        // record unconditionally made an arrow one: `class C { x = () => arguments }` was refused
        // and `class C { x = () => { arguments; } }` was not, the same rule answering differently
        // for a concise body and a block. An arrow leaves the record alone in both directions —
        // it neither hides what is outside nor discards what its own body read.
        let enclosing_arguments = match boundary {
            Boundary::Function => self.arguments_reference.take(),
            Boundary::Arrow => None,
        };
        self.inside_function = true;
        self.body_context = body_context;
        let body = self.parse_body_with_prologue(TokenKind::RBrace);
        self.inside_function = enclosing;
        self.strict = enclosing_strict;
        self.body_context = enclosing_context;
        self.forbidden_in_parameters = enclosing_forbidden_in_parameters;
        if boundary == Boundary::Function {
            self.arguments_reference = enclosing_arguments;
        }
        let (body, declares_strict) = body?;
        let close = self.eat(TokenKind::RBrace, after, "`}`")?;
        // §15.2.1 asks of a FunctionStatementList exactly what §16.1.1 asks of a Script, and asks
        // it again rather than inheriting — this is where those walks stop.
        super::scope::check_declared_names(&body, super::scope::Level::Top)?;
        super::scope::check_labels(&body)?;

        Ok((body, close.span, declares_strict))
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
    check_no_duplicate_parameter(parameters)
}

/// The question both of the duplicate-parameter rules ask, once.
///
/// §15.1.1 and `UniqueFormalParameters` differ only in the guard in front — the first asks it of a
/// non-simple list and the second of every list — and yet this walk existed twice, in `function`
/// and in `arrow`. The arrow's copy **stopped at `items`**, so `(x, ...x) => 1` was accepted where
/// `function f(x, ...x) {}` was refused: `BoundNames` of `FormalParameters : FormalParameterList ,
/// FunctionRestParameter` is the names of both halves, and one of the two walks knew it.
pub(super) fn check_no_duplicate_parameter(
    parameters: &FormalParameters,
) -> Result<(), ParseError> {
    let mut seen: HashSet<String> = HashSet::new();
    let targets = parameters
        .items
        .iter()
        .map(|element| &element.target)
        .chain(parameters.rest.as_deref());
    for target in targets {
        for declared in bound_names(target) {
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

/// §15.2.1 — the names a function may not have in strict code.
///
/// `eval` and `arguments`, which are the two §13.1.1 refuses as a `BindingIdentifier` there. Split
/// out because two callers ask it about different text: the name as it is read, with the enclosing
/// strictness, and the same name again once the body's own directive has spoken.
pub(super) fn check_strict_name(name: &str, span: crate::span::Span) -> Result<(), ParseError> {
    match name {
        "eval" | "arguments" => Err(ParseError {
            kind: ParseErrorKind::StrictEvalOrArguments,
            span,
        }),
        _ => Ok(()),
    }
}

/// §15.2.1 and §13.1.1, for a strict function's parameters.
///
/// Two rules the parameters could not be judged by when they were read, the body not yet having
/// said whether it is strict: every name must be one strict code may bind, and no name may repeat
/// — a strict list being unique whether or not it is simple.
pub(super) fn check_strict_parameters(parameters: &FormalParameters) -> Result<(), ParseError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut names: Vec<crate::static_semantics::DeclaredName<'_>> = Vec::new();
    for element in &parameters.items {
        names.extend(bound_names(&element.target));
    }
    if let Some(rest) = &parameters.rest {
        names.extend(bound_names(rest));
    }
    for declared in names {
        if declared.name == "eval" || declared.name == "arguments" {
            return Err(ParseError {
                kind: ParseErrorKind::StrictEvalOrArguments,
                span: declared.span,
            });
        }
        if !seen.insert(declared.name.to_string()) {
            return Err(ParseError {
                kind: ParseErrorKind::DuplicateParameterName,
                span: declared.span,
            });
        }
    }
    Ok(())
}

/// §15.3.1 borrows §15.2.1's rule for an arrow, whose body is a `ConciseBody`.
pub(super) fn check_parameters_against_arrow_body(
    parameters: &FormalParameters,
    body: &crate::ast::ArrowBody,
) -> Result<(), ParseError> {
    match body {
        // An `ExpressionBody` declares nothing, so there is nothing for a parameter to clash
        // with — which is most arrows, and why this is not simply the function's check.
        crate::ast::ArrowBody::Expression(_) => Ok(()),
        crate::ast::ArrowBody::Block(body) => check_parameters_against_body(parameters, body),
    }
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

/// Everything reading a function's parentheses and braces produces.
///
/// A struct because four returns is where a tuple stops being readable — and because the fourth is a
/// `bool` beside a `Span`, which is exactly the pair a caller can silently swap.
struct FunctionParts {
    /// What it takes.
    parameters: FormalParameters,
    /// The `FunctionBody`.
    body: Box<[Stmt]>,
    /// Where the closing brace is, for the function's own span.
    end: Span,
    /// Whether the body is strict code — §11.2.1, its own directive or the enclosing code's.
    is_strict: bool,
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    #[test]
    fn a_bodys_own_use_strict_re_judges_the_text_around_it() {
        // Three early errors that a directive prologue decides *after* the text they are about has
        // been read. Each was accepted, and each is refused when the same strictness comes from
        // outside — so the checks existed and were being asked at the wrong moment.
        //
        // §12.9.4.1 — a legacy octal escape is judged against the strictness of the code it is in,
        // and the escape here is two statements before the thing that makes it strict.
        assert_eq!(
            script_error(r#"function invalid() { "\1"; "use strict"; }"#).kind,
            ParseErrorKind::StrictLegacyOctal
        );
        assert_eq!(
            script_error(r#""\1"; "use strict";"#).kind,
            ParseErrorKind::StrictLegacyOctal
        );
        // §15.2.1 — a function whose *body* is strict may not be called `eval` or `arguments`, and
        // the name was read before the body said anything.
        assert_eq!(
            script_error(r#"function arguments() { "use strict"; }"#).kind,
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            script_error(r#"var f = function eval() { "use strict"; };"#).kind,
            ParseErrorKind::StrictEvalOrArguments
        );
        // §15.5.1 — the same about a method's parameters, of which a setter is the smallest case.
        assert_eq!(
            script_error(r#"var o = { set x(eval) { "use strict"; } };"#).kind,
            ParseErrorKind::StrictEvalOrArguments
        );

        // Every sloppy form is still accepted, which is what keeps this about the *prologue* rather
        // than about strict mode arriving everywhere.
        assert!(parse_script(r#"function ok() { "\1"; }"#).is_ok());
        assert!(parse_script(r#"function ok() { "use strict"; } var s = "\1";"#).is_ok());
        assert!(parse_script("function arguments() {}").is_ok());
        assert!(parse_script("var o = { set x(eval) {} };").is_ok());
        // A directive that is not `"use strict"` decides nothing, so an escape beside one stands.
        assert!(parse_script(r#"function ok() { "\1"; "use asm"; }"#).is_ok());
        // …and one *after* the prologue has ended is judged where it is read, which it already was.
        assert!(parse_script(r#"function ok() { var a = 1; "\1"; }"#).is_ok());
    }

    #[test]
    fn a_slash_after_a_declaration_opens_a_regular_expression() {
        // §12.6: the goal symbol for a token is the parser's to choose, and a `}` that ends a
        // *declaration* ends a statement — so an operand comes next and a `/` there opens a
        // literal. The same `}` ending a function *expression* is followed by an operator.
        // One production serves both, which is why the caller has to say which it asked for.
        assert!(parse_script("function f() {}\n/re/.test(x);").is_ok());
        assert!(parse_script("class C {}\n/re/.test(x);").is_ok());
        assert!(parse_script("async function f() {}\n/re/.test(x);").is_ok());
        assert!(parse_script("function* g() {}\n/re/.test(x);").is_ok());
        // The shape babel's corpus has, where the declaration is inside another function.
        assert!(parse_script("function fn() {\n return\n function foo() {}\n /42/i\n }").is_ok());
        // …and the expression side, where the very same `}` is followed by division.
        assert_eq!(shape("(function () {}) / 2"), "(/ (fn <anon> [] {}) 2)");
        assert_eq!(shape("(class {}) / 2"), "(/ (class <anon> - []) 2)");
        // Without the parentheses the same text is still an expression, a `FunctionExpression`
        // being a `PrimaryExpression` and so an operand of `/` like any other. That it divides
        // rather than opening a literal is the whole of what the expression side asks for.
        assert!(parse_script("x = function () {} / 2;").is_ok());
        assert!(parse_script("x = class {} / 2;").is_ok());
        // An arrow is the exception, and it is the grammar that makes it one: an
        // `ArrowFunction` is an `AssignmentExpression` and nothing tighter, so it can never be
        // the left operand of a `/`. With nothing there to divide, the `/` opens a literal and
        // the arrow has ended the statement.
        assert!(
            parse_script(
                "var f = x => {}
/re/.test(x);"
            )
            .is_ok()
        );
        assert!(
            parse_script(
                "var f = async x => {}
/re/.test(x);"
            )
            .is_ok()
        );
        assert!(
            parse_script(
                "var f = async (x) => {}
/re/.test(x);"
            )
            .is_ok()
        );
        assert!(
            parse_script(
                "var g = () => { var f = x => {}
/re/.test(x) };"
            )
            .is_ok()
        );
        // …so this has no derivation, where the two above it do.
        assert!(parse_script("x = y => {} / 2;").is_err());
        // Parentheses put it back: `(x => {})` is a `PrimaryExpression`, and the `)` decides.
        assert!(parse_script("x = (y => {}) / 2;").is_ok());
        // A *concise* body has no brace to decide about — the body is an expression, so the
        // division simply continues it and no literal is opened.
        assert!(
            parse_script(
                "var f = x => y
/re/.test(x);"
            )
            .is_err()
        );
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
        // …except when both are functions, which is §B.3.3.5's carve-out and is conditioned on
        // sloppiness alone. `annexB/language/function-code/function-redeclaration-block.js` is
        // this program, and its strict sibling is the line below.
        assert!(parse_script("{ function f() {} function f() {} }").is_ok());
        assert!(
            parse_script("switch (x) { case 1: function f() {} case 2: function f() {} }").is_ok()
        );
        assert_eq!(
            script_error("'use strict'; { function f() {} function f() {} }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        // "only bound by FunctionDeclarations" is the whole of the exemption: one function and one
        // `let` of a name is still two lexical bindings of it, and still refused.
        assert_eq!(
            script_error("{ function f() {} function f() {} let f; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("{ class f {} function f() {} }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        // …and a name that repeats beside one that does not is still caught.
        assert_eq!(
            script_error("{ function f() {} function f() {} let g; const g = 1; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        // …and a function in a nested block is not var-declared at the level above it, which is
        // what "directly" means in §8.2.12.
        assert!(parse_script("function f() { { function g() {} } let g; }").is_ok());
        assert!(parse_script("{ function f() {} } let f;").is_ok());
    }

    #[test]
    fn annex_b_takes_a_function_where_a_statement_stands_and_only_where_it_says() {
        // §B.3.4 lets a `FunctionDeclaration` be a clause of an `if`, and §B.3.2 lets one be
        // labelled. DR-0008's amendment conditions both on strictness and on nothing else.
        assert_eq!(
            statements("if (x) function f() {}"),
            ["(if x {(fn f [] {})})"]
        );
        assert_eq!(
            statements("if (x) function f() {} else function g() {}"),
            ["(if x {(fn f [] {})} {(fn g [] {})})"],
            "§B.3.4 evaluates it as if the clause were a block, so that is what the tree holds"
        );
        assert_eq!(
            statements("if (x) ; else function g() {}"),
            ["(if x <empty> {(fn g [] {})})"]
        );
        assert_eq!(
            statements("a: function f() {}"),
            ["(label a (fn f [] {}))"],
            "§B.3.2 does *not* wrap: §8.2.12 hands a label's item to TopLevelVarDeclaredNames"
        );
        assert_eq!(
            statements("a: b: function f() {}"),
            ["(label a (label b (fn f [] {})))"],
            "§14.13's item may be another LabelledStatement, and the permission is inherited"
        );
        // Strict code takes neither, which is the whole condition.
        for source in [
            "'use strict'; if (x) function f() {}",
            "'use strict'; a: function f() {}",
            "function h() { 'use strict'; if (x) function f() {} }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
        // A `GeneratorDeclaration` is not a `FunctionDeclaration`, so Annex B does not reach one
        // in either position — which is what the token past `function` is looked at for.
        for source in [
            "if (x) function* g() {}",
            "a: function* g() {}",
            "if (x) async function g() {}",
            "a: async function g() {}",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
        // An `IterationStatement` body has no exemption anywhere, being a `Statement` in every
        // dialect — and neither does a `with` body or the branch of an `if` inside one.
        for source in [
            "while (x) function f() {}",
            "for (;;) function f() {}",
            "do function f() {} while (x)",
            "with (x) function f() {}",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
        // §14.6.1 and its five siblings: `IsLabelledFunction(Statement)` is a Syntax Error
        // wherever the body is a `Statement` rather than a `StatementList`. So the *position* rule
        // is a second condition and not a restatement of strictness — every one of these is sloppy
        // and every one is refused, while the same text inside braces is taken.
        for source in [
            "if (x) a: function f() {}",
            "if (x) ; else a: function f() {}",
            "if (x) a: b: function f() {}",
            "while (x) a: function f() {}",
            "do a: function f() {} while (x)",
            "for (;;) a: function f() {}",
            "for (a in b) a: function f() {}",
            "for (a of b) a: function f() {}",
            "with (x) a: function f() {}",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
        for source in [
            "{ a: function f() {} }",
            "if (x) { a: function f() {} }",
            "while (x) { a: function f() {} }",
            "switch (x) { case 1: a: function f() {} }",
            "function h() { a: function f() {} }",
            "try { a: function f() {} } finally {}",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
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
