//! Generators, and the `[Yield]` grammar parameter they finally turn on (ECMAScript §15.5).
//!
//! # The parameter that was a constant until now
//!
//! `[Yield]` has been in every production this parser reads since the first expression slice, and
//! until now nothing could set it — so [`super::is_identifier_token`] took §13.1's `[~Yield] yield`
//! alternative directly and there was no branch to test. A generator sets it, and every one of
//! those productions starts meaning two different things.
//!
//! It is a field rather than a parameter, for the reason `[Return]` and strictness are: it is set
//! by one production and is a fact about where you are, not a decision to make at each step. What
//! makes it unlike those two is that it is also *turned off*, by an ordinary function nested
//! inside a generator — and every place it changes is a place this parser already saves and
//! restores state. So it costs one field and four assignments.
//!
//! # Where it changes, which is not simply "inside a generator"
//!
//! | Production | Name | Parameters | Body |
//! | --- | --- | --- | --- |
//! | `FunctionDeclaration` | `[?Yield]` | `[~Yield]` | `[~Yield]` |
//! | `FunctionExpression` | `[~Yield]` | `[~Yield]` | `[~Yield]` |
//! | `GeneratorDeclaration` | `[?Yield]` | `[+Yield]` | `[+Yield]` |
//! | `GeneratorExpression` | `[+Yield]` | `[+Yield]` | `[+Yield]` |
//! | `ArrowFunction` | — | `[?Yield]` | `[~Yield]` |
//!
//! Three of those rows are surprising and each is observable:
//!
//! - A *declaration's* name inherits, so `function* g() { function yield() {} }` is refused and
//!   `function* g() { (function yield() {}); }` is not. The name of an expression belongs to the
//!   function; the name of a declaration belongs to the scope around it.
//! - An arrow's parameters inherit and its body does not, so `function* g() { (a = yield) => 1; }`
//!   is refused — the parameters are `[+Yield]`, so that is a `YieldExpression`, and §15.3.1
//!   forbids one there — while `function* g() { () => yield; }` parses, `yield` in the body being
//!   an ordinary identifier.
//! - A method of an object literal or a class resets it unless the method is itself a generator,
//!   so `function* g() { ({ m() { yield; } }); }` reads `yield` as a name.
//!
//! # `Contains YieldExpression`, recorded rather than walked
//!
//! §15.5.1 makes `function* g(a = yield) {}` a Syntax Error, and it has to: a parameter's default
//! is evaluated before the generator is resumable, so there would be nothing to yield to. The
//! parameters are `[+Yield]`, though, so that `yield` *parses* as a `YieldExpression` and the
//! refusal is an early error rather than a parse failure.
//!
//! `Contains` stops at every function boundary — `function* g(a = function*() { yield; }) {}` is
//! fine — which is exactly the shape of a field saved and restored at those boundaries. So the
//! parser records where it read a `YieldExpression` ([`super::Parser::forbidden_in_parameters`]) and the
//! parameter list asks afterwards, the same deferral as `unrefined_covers`. Walking the
//! finished parameter tree would have to re-derive the boundary rule that the save already knows.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Expr, ExprKind, YieldExpression};
use crate::lexer::{Goal, TokenKind};

impl Parser<'_> {
    /// `YieldExpression` (§15.5), with the cursor on `yield`.
    ///
    /// Only reached when `[+Yield]`; where the parameter is unset, `yield` is an
    /// `IdentifierReference` and never arrives here.
    pub(super) fn parse_yield(&mut self, allow_in: AllowIn) -> Result<Expr, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        // §15.5's Note 1: the context after `yield` uses the RegExp goal, so `yield /a/g` yields a
        // regular expression rather than dividing something. `advance` above asked for it.
        self.forbidden_in_parameters.get_or_insert(ParseError {
            kind: ParseErrorKind::YieldInParameters,
            span: keyword.span,
        });
        // `yield [no LineTerminator here] * AssignmentExpression`. The restriction is before the
        // `*` and not after it, so `yield *\n a` is one expression and `yield \n * a` is not one
        // at all — the first is a bare `yield` and then a `*` that nothing wanted.
        let delegate = self.current.kind == TokenKind::Star && !self.current.newline_before;
        if delegate {
            self.advance(Goal::RegExp)?;
        }
        // `YieldExpression : yield` — the bare form, which wins whenever no `AssignmentExpression`
        // could begin here. A line terminator is the other way it wins, and it wins there even
        // when what follows would have parsed: `yield\n1` is two statements.
        //
        // Not the case for `yield*`: the delegating form has no bare alternative, so a `*` commits
        // to an operand and the error names what was missing.
        let argument = if delegate
            || (!self.current.newline_before
                && super::primary::begins_an_expression(self.current.kind))
        {
            self.enter()?;
            // `AssignmentExpression[?In, +Yield, ?Await]` — so `yield a, b` is `(yield a), b`, and
            // `[In]` is passed along because a `yield` in a `for` head is still inside that head.
            let argument = self.parse_assignment(allow_in);
            self.leave();
            Some(Box::new(argument?))
        } else {
            None
        };
        let end = argument.as_ref().map_or(keyword.span, |value| value.span);
        Ok(Expr::new(
            ExprKind::Yield(Box::new(YieldExpression { argument, delegate })),
            keyword.span.to(end),
        ))
    }

    /// Read `parameters` under the `[Yield]` and `[Await]` the function kind gives them, and
    /// refuse a `YieldExpression` or an `AwaitExpression` among them (§15.5.1, §15.8.1).
    ///
    /// The saving is what implements `Contains` stopping at a function boundary: whatever the
    /// enclosing code had recorded is put back, so a `yield` written here is attributed here and
    /// a `yield` in a nested generator's body is attributed to that one.
    pub(super) fn parse_parameters_of(
        &mut self,
        is_generator: bool,
        is_async: bool,
    ) -> Result<crate::ast::FormalParameters, ParseError> {
        let enclosing_yield = self.yield_allowed;
        let enclosing_await = self.await_allowed;
        let enclosing_seen = self.forbidden_in_parameters.take();
        let enclosing_await_named = self.await_named.take();
        self.yield_allowed = is_generator;
        self.await_allowed = is_async;
        let parameters = self.parse_formal_parameters();
        let seen = self.forbidden_in_parameters;
        self.yield_allowed = enclosing_yield;
        self.await_allowed = enclosing_await;
        self.forbidden_in_parameters = enclosing_seen;
        self.await_named = enclosing_await_named;
        let parameters = parameters?;
        // §15.5.1 and §15.8.1: a `YieldExpression` in a generator's own parameters, or an
        // `AwaitExpression` in an async function's. A default is evaluated before the function is
        // suspendable, so there is nothing for either to suspend into — the rule is about the
        // runtime having no answer, not about the syntax being ambiguous.
        if let Some(error) = seen {
            return Err(error);
        }
        Ok(parameters)
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
    }

    #[test]
    fn a_star_after_the_word_function_is_the_whole_of_the_syntax_difference() {
        assert_eq!(statements("function* g() {}"), ["(fn* g [] {})"]);
        assert_eq!(statements("function g() {}"), ["(fn g [] {})"]);
        assert_eq!(shape("(function* () {})"), "(fn* <anon> [] {})");
        assert_eq!(shape("(function* g() {})"), "(fn* g [] {})");
        // No `[no LineTerminator here]` anywhere around the `*`, so every spacing is the same
        // generator.
        for source in ["function*g() {}", "function * g() {}", "function*\ng() {}"] {
            assert_eq!(statements(source), ["(fn* g [] {})"], "{source:?}");
        }
        assert_eq!(statements("function\n* g() {}"), ["(fn* g [] {})"]);
        // A `GeneratorDeclaration` is a `HoistableDeclaration` exactly as a function is, so the
        // same places take it and the same places refuse it.
        assert!(parse_script("function* () {}").is_err());
        assert_eq!(
            kind("if (a) function* g() {}"),
            ParseErrorKind::DeclarationInStatementPosition
        );
        assert_eq!(
            kind("a: function* g() {}"),
            ParseErrorKind::DeclarationInStatementPosition
        );
    }

    #[test]
    fn a_generator_method_is_written_with_the_star_before_the_name() {
        assert_eq!(shape("({*m() {}})"), "{(m (fn* <anon> [] {}))}");
        assert_eq!(shape("({* m() {}})"), "{(m (fn* <anon> [] {}))}");
        assert_eq!(shape("({*[a]() {}})"), "{([a] (fn* <anon> [] {}))}");
        assert_eq!(
            statements("class C { *m() {} }"),
            ["(class C - [(m (fn* <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { static *m() {} }"),
            ["(class C - [(static m (fn* <anon> [] {}))])"]
        );
        // Every word is an `IdentifierName` here, so a generator may be called anything a method
        // may — including the words that mean something in the position before it.
        for source in [
            "class C { *static() {} }",
            "class C { static *static() {} }",
            "class C { *get() {} }",
            "class C { *yield() {} }",
            "({*yield() {}});",
            "({*if() {}});",
            "class C { *1() {} }",
            "({*\"s\"() {}});",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // …but a generator is not an accessor and an accessor is not a generator: `get` is read
        // as the name, and then a `*` is not the `(` a method needs.
        for source in [
            "({get *a() {}});",
            "({set *a(v) {}});",
            "class C { get *m() {} }",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // §15.7.1: `new` cannot resume a generator, so there would be nothing for the class to be.
        assert_eq!(
            kind("class C { *constructor() {} }"),
            ParseErrorKind::ConstructorMayNotBeAGenerator
        );
        assert!(parse_script("class C { static *constructor() {} }").is_ok());
        // …and §15.7.1's other name rule does not care how the method was written either.
        assert_eq!(
            kind("class C { static *prototype() {} }"),
            ParseErrorKind::StaticPrototype
        );
        // A generator method is a method: `UniqueFormalParameters`, and `super.a` but no
        // `super()` unless it is a derived constructor, which it never is.
        assert_eq!(
            kind("class C { *m(a, a) {} }"),
            ParseErrorKind::DuplicateParameterName
        );
        assert!(parse_script("class C { *m() { super.a; } }").is_ok());
        assert!(parse_script("({*m() { super.a; }});").is_ok());
        assert_eq!(
            kind("class C { *m() { super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
    }

    #[test]
    fn yield_is_an_operator_where_the_parameter_is_set_and_a_name_where_it_is_not() {
        assert_eq!(
            shape("(function* () { yield; })"),
            "(fn* <anon> [] {(yield)})"
        );
        assert_eq!(
            shape("(function* () { yield 1; })"),
            "(fn* <anon> [] {(yield 1)})"
        );
        assert_eq!(
            shape("(function* () { yield* a; })"),
            "(fn* <anon> [] {(yield* a)})"
        );
        // …and where the parameter is unset it is an ordinary name, which is what §13.1's
        // `[~Yield] yield` alternative is for. Sloppy code only: strict mode reserves it.
        assert_eq!(statements("yield;"), ["yield"]);
        assert_eq!(statements("var yield;"), ["(var yield)"]);
        assert_eq!(statements("function f() { yield; }"), ["(fn f [] {yield})"]);
        assert_eq!(
            kind("\"use strict\"; yield;"),
            ParseErrorKind::StrictReservedWord
        );
        // A name is not an operator, so this is `yield` and then a number that nothing wanted.
        assert!(parse_script("yield 1;").is_err());
        assert!(parse_script("function f() { yield 1; }").is_err());
    }

    #[test]
    fn a_forbidden_in_parameters_is_an_assignment_expression_and_nothing_tighter() {
        // The reason it is produced at the assignment level, as an arrow is: every operator that
        // wants a narrower operand refuses one.
        for source in [
            "1 + yield;",
            "yield ** 2;",
            "yield ? a : b;",
            "yield in a;",
            "yield.a;",
            "yield = 1;",
            "new yield;",
            "typeof yield;",
            "++yield;",
        ] {
            assert!(
                parse_script(&format!("function* g() {{ {source} }}")).is_err(),
                "{source:?}"
            );
        }
        // …and parenthesizing it makes it an operand like anything else.
        for source in ["(yield) ** 2;", "(yield);", "[yield];", "f(yield);"] {
            assert!(
                parse_script(&format!("function* g() {{ {source} }}")).is_ok(),
                "{source:?}"
            );
        }
        // Where an `AssignmentExpression` may stand, so may this.
        for source in [
            "x = yield;",
            "x += yield;",
            "yield, 1;",
            "a ? b : yield;",
            "[a] = yield;",
            "return yield;",
            "throw yield;",
            "if (yield) ;",
            "while (yield) ;",
            "switch (yield) {}",
            "for (a of yield);",
            "[...yield];",
            "`${yield}`;",
        ] {
            assert!(
                parse_script(&format!("function* g() {{ {source} }}")).is_ok(),
                "{source:?}"
            );
        }
    }

    #[test]
    fn the_bare_form_wins_when_no_assignment_expression_could_begin() {
        // `YieldExpression : yield` against `yield [nlth] AssignmentExpression`, settled by one
        // token — see [`super::primary::begins_an_expression`].
        assert_eq!(
            shape("(function* () { (yield, 1) })"),
            "(fn* <anon> [] {(, (yield) 1)})"
        );
        assert_eq!(
            shape("(function* () { yield yield 1 })"),
            "(fn* <anon> [] {(yield (yield 1))})"
        );
        // `(` and a template can begin one, so these take an operand rather than being bare.
        assert_eq!(
            shape("(function* () { yield (1) })"),
            "(fn* <anon> [] {(yield 1)})"
        );
        assert!(parse_script("function* g() { yield`x`; }").is_ok());
        // So can each of the prefix operators, which is why the predicate has to span
        // `parse_unary` as well as `parse_primary`. Read as bare, every one of these would be a
        // `yield` and then an operator with nothing to its left, which is nothing at all.
        for operand in ["-a", "+a", "!a", "~a", "++a", "--a"] {
            let source = format!("function* g() {{ yield {operand}; }}");
            assert!(parse_script(&source).is_ok(), "{source:?}");
            assert!(
                shape(&format!("(function* () {{ yield {operand} }})"))
                    .starts_with("(fn* <anon> [] {(yield ("),
                "{operand:?} is the operand and not a statement of its own"
            );
        }
        // …and §15.5's Note 1: the goal after `yield` is the RegExp one, so this yields a regular
        // expression rather than dividing a name by `a` and then by `g`.
        assert!(parse_script("function* g() { yield /a/g; }").is_ok());
        // A line terminator wins even when what follows would have parsed — `yield` and then a
        // separate statement.
        assert_eq!(
            statements("function* g() { yield\n1; }"),
            ["(fn* g [] {(yield) 1})"]
        );
        // `yield*` has no bare alternative, so the `*` commits to an operand…
        assert!(parse_script("function* g() { yield*; }").is_err());
        // …and the restriction is before the `*`, not after it.
        assert!(parse_script("function* g() { yield*\na; }").is_ok());
        assert!(parse_script("function* g() { yield\n* a; }").is_err());
        // `...` is not an `AssignmentExpression`, so it is not an operand either.
        assert!(parse_script("function* g() { yield ...a; }").is_err());
    }

    #[test]
    fn the_parameter_turns_off_at_a_function_and_not_at_anything_smaller() {
        // A generator body sets it; a plain function nested inside turns it back off, which is
        // what makes `yield` a usable name in there again.
        assert!(parse_script("function* g() { function h(yield) {} }").is_ok());
        assert!(parse_script("function* g() { function h() { yield; } }").is_ok());
        assert!(parse_script("function* g() { (function() { yield; }); }").is_ok());
        assert!(parse_script("function* g() { ({ m() { yield; } }); }").is_ok());
        // …and a nested generator turns it on again.
        assert!(parse_script("function* g() { function* h() { yield 1; } }").is_ok());
        assert!(parse_script("function* g() { ({ *m() { yield 1; } }); }").is_ok());
        // Blocks, loops and labels are not function boundaries, so it survives them.
        assert!(parse_script("function* g() { { yield 1; } }").is_ok());
        assert!(parse_script("function* g() { while (a) yield 1; }").is_ok());
        assert!(parse_script("function* g() { label: yield 1; }").is_ok());
        // A class body is strict rather than `[~Yield]`, so `yield` is refused there for the
        // other reason — and the method body has turned the parameter off regardless.
        assert!(parse_script("function* g() { class C { m() { yield 1; } } }").is_err());
    }

    #[test]
    fn an_arrows_parameters_keep_the_parameter_and_its_body_drops_it() {
        // `ArrowParameters[?Yield]` against `ConciseBody[In]`, whose two alternatives are both
        // `[~Yield]`. The one place in the grammar where a head and its body disagree.
        assert!(parse_script("function* g() { () => yield; }").is_ok());
        assert!(parse_script("function* g() { (a) => yield; }").is_ok());
        assert!(parse_script("function* g() { ((a) => { yield; }); }").is_ok());
        // …so `yield` there is a *name*, and a name takes no operand.
        assert!(parse_script("function* g() { () => yield 1; }").is_err());
        assert!(parse_script("function* g() { () => { yield 1; } }").is_err());
        // …and being a name, strict mode is what refuses it rather than the parameter.
        assert_eq!(
            kind("\"use strict\"; function* g() { () => yield; }"),
            ParseErrorKind::StrictReservedWord
        );
        // The parameters kept it, so `yield` there is an operator and §15.3.1 refuses it.
        assert_eq!(
            kind("function* g() { (a = yield) => 1; }"),
            ParseErrorKind::YieldInParameters
        );
        // …and as an operator it is not a name, so it cannot be a parameter at all.
        assert!(parse_script("function* g() { (yield) => 1; }").is_err());
        assert!(parse_script("function* g() { yield => 1; }").is_err());
        // An arrow *inside* a parameter default has dropped it, so its body may use the name.
        assert!(parse_script("function* g(a = () => yield) {}").is_ok());
    }

    #[test]
    fn a_declarations_name_inherits_the_parameter_and_an_expressions_does_not() {
        // `FunctionDeclaration : function BindingIdentifier[?Yield]` against
        // `FunctionExpression : function BindingIdentifier[~Yield]opt`. The name of a declaration
        // is a binding in the scope around it; the name of an expression belongs to the function.
        assert!(parse_script("function* g() { function yield() {} }").is_err());
        assert!(parse_script("function* g() { function* yield() {} }").is_err());
        assert!(parse_script("function* g() { (function yield() {}); }").is_ok());
        // `GeneratorExpression : function * BindingIdentifier[+Yield]opt` — the one row where an
        // expression's name is `[+Yield]`, so this is refused where the plain form above is not.
        assert!(parse_expression("(function* yield() {})").is_err());
        // At the top of a script nothing has set it, so both declarations take the name.
        assert!(parse_script("function yield() {}").is_ok());
        assert!(parse_script("function* yield() {}").is_ok());
    }

    #[test]
    fn every_binding_position_refuses_the_name_where_the_parameter_is_set() {
        // `BindingIdentifier` takes `yield` in the grammar and §13.1.1 refuses it under `[+Yield]`
        // — so every place that binds a name is one place, asked once.
        for source in [
            "var yield;",
            "let yield;",
            "const yield = 1;",
            "class yield {}",
            "try {} catch (yield) {}",
            "for (var yield of a);",
            "({yield} = a);",
        ] {
            assert!(
                parse_script(&format!("function* g() {{ {source} }}")).is_err(),
                "{source:?}"
            );
        }
        // `LabelIdentifier` is `[~Yield] yield` too, in both of its positions.
        assert!(parse_script("function* g() { yield: 1; }").is_err());
        assert!(parse_script("function* g() { break yield; }").is_err());
        assert!(parse_script("function f() { yield: 1; }").is_ok());
        // A *property* name is an `IdentifierName` and not a binding, so every one of these is
        // the word used as a name and none of them cares about the parameter.
        for source in [
            "a.yield;",
            "({yield: 1});",
            "({yield: a} = b);",
            "({ get yield() {} });",
            "({ [yield]: 1 });",
        ] {
            assert!(
                parse_script(&format!("function* g() {{ {source} }}")).is_ok(),
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_generators_own_parameters_may_not_contain_one_and_a_nested_functions_may() {
        // §15.5.1. A default is evaluated before the generator is resumable, so there would be
        // nothing to yield to.
        for source in [
            "function* g(a = yield) {}",
            "function* g(a = yield 1) {}",
            "function* g(a = yield * b) {}",
            "function* g(a = [yield]) {}",
            "function* g({a = yield}) {}",
            "({*m(a = yield) {}});",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::YieldInParameters,
                "{source:?}"
            );
        }
        // …and the name itself is refused for the other reason: it is a binding under `[+Yield]`.
        assert!(parse_script("function* g(yield) {}").is_err());
        // `Contains` stops at a function boundary, so a `yield` belonging to a nested function is
        // not one written here. Each of these is a different kind of boundary.
        for source in [
            "function* g(a = function*() { yield; }) {}",
            "function* g(a = function() { yield; }) {}",
            "function* g(a = () => yield) {}",
            "function* g(a = { *m() { yield; } }) {}",
            "function* g(a = class { *m() { yield; } }) {}",
            "({*m(a = function*() { yield; }) {}});",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // A plain function nested in a generator has `[~Yield]` parameters, so `yield` is a name…
        assert!(parse_script("function* g() { (function(a = yield) {}); }").is_ok());
        assert!(parse_script("function* g() { ({ m(a = yield) {} }); }").is_ok());
        // …and a nested generator's are its own, so the rule applies to it afresh.
        assert_eq!(
            kind("function* g() { (function*(a = yield) {}); }"),
            ParseErrorKind::YieldInParameters
        );
        // Parentheses change nothing: it was an expression, so it counts where it was written.
        assert_eq!(
            kind("function* g(a = (yield)) {}"),
            ParseErrorKind::YieldInParameters
        );
        // …and outside a parameter list the very same parentheses are ordinary.
        assert!(parse_script("function* g() { (yield); }").is_ok());
    }

    #[test]
    fn a_generator_body_is_a_function_body_in_every_other_respect() {
        assert!(parse_script("function* g() { super.a; }").is_err());
        assert!(parse_script("function* g() { return; }").is_ok());
        assert!(parse_script("function* g() { arguments; eval; }").is_ok());
        assert!(parse_script("function* g(a, a) {}").is_ok());
        assert_eq!(
            kind("\"use strict\"; function* g(a, a) {}"),
            ParseErrorKind::DuplicateParameterName
        );
        assert_eq!(
            kind("function* g(a = 1) { \"use strict\"; }"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
    }

    #[test]
    fn no_generator_however_truncated_can_panic() {
        let deep = format!("function* g() {{ {}1; }}", "yield ".repeat(1000));
        let cases = [
            "function*".to_string(),
            "function* g".to_string(),
            "function* g(".to_string(),
            "function* g() {".to_string(),
            "function* g() { yield".to_string(),
            "function* g() { yield*".to_string(),
            "function* g() { yield* ".to_string(),
            "({*".to_string(),
            "({*m".to_string(),
            "class C { *".to_string(),
            "*m() {}".to_string(),
            deep.clone(),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A `yield` operand is a `yield` operand, so a chain of them nests and the cap bounds it.
        assert_eq!(kind(&deep), ParseErrorKind::TooDeeplyNested);
    }
}
