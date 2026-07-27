//! Everything `async`: functions (§15.8), generators (§15.6), arrows (§15.9), and the
//! `await` they all make legal.
//!
//! # `[Await]` is `[Yield]`'s twin, and the differences are the interesting part
//!
//! Everything [`super::generator`] says about `[Yield]` holds here: a field rather than a
//! parameter, changed at function boundaries, inherited by an arrow's *parameters* and dropped by
//! its body, and inherited by a declaration's name but not an expression's. Read that first; this
//! file is the four places the two differ.
//!
//! **`await` takes a `UnaryExpression`, `yield` takes an `AssignmentExpression`.** So an
//! `AwaitExpression` binds *tighter* than almost everything, where a `YieldExpression` binds
//! looser than everything:
//!
//! ```js
//! await a ? b : c      // (await a) ? b : c   — legal
//! yield a ? b : c      // no derivation at all
//! await a + b          // (await a) + b
//! ```
//!
//! The one thing it does not reach past is `**`, and for the reason `-a ** b` does not: §13.6
//! refuses a `UnaryExpression` on the left of an exponentiation, so `await a ** b` must be
//! written `(await a) ** b`.
//!
//! **The operand is mandatory.** §15.8's Note 2 says so outright — "Unlike YieldExpression, it is
//! a Syntax Error to omit the operand of an AwaitExpression. You must await something." So there
//! is no `begins_an_expression` question here and no line-terminator restriction either:
//! `await\na` is one expression.
//!
//! **`async` is not a reserved word.** `yield` is one, so the lexer hands over a keyword and the
//! parameter decides what it means. `async` is an ordinary `Identifier` that four productions
//! happen to begin with, so every one of them is a lookahead — and `async` stays usable as a name
//! everywhere else, including as a method's name and as its own parameter.
//!
//! **`async [no LineTerminator here] function`.** The restriction is real and observable, and it
//! does not make a bad async function: it makes `async` an expression statement, and then
//! `function f() {}` is an ordinary declaration on the next line. Both parse, which is why the
//! test asserts the shape rather than success.
//!
//! # One record for both `Contains` rules
//!
//! §15.5.1 forbids a `YieldExpression` in a generator's parameters and §15.8.1 forbids an
//! `AwaitExpression` in an async function's, for the same reason: the defaults are evaluated
//! before there is anything to suspend into. An async generator's parameters are `[+Yield, +Await]`
//! and §15.6.1 forbids both.
//!
//! Whichever of the two can appear in a given parameter list is the one that is forbidden there —
//! the parameters of a plain generator are `[~Await]`, so no `AwaitExpression` can arise in them
//! to begin with. So [`super::Parser::forbidden_in_parameters`] is one field holding a
//! ready-made error, rather than two fields and two saves that could never both be set.

use super::arrow::ArrowOrGroup;
use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Argument, Binding, BindingElement, Expr, ExprKind, FormalParameters};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::span::Span;
use crate::static_semantics::bound_names;

impl Parser<'_> {
    /// `AwaitExpression : await UnaryExpression` (§15.8), with the cursor on `await`.
    ///
    /// Only reached when `[+Await]`; where the parameter is unset, `await` is an
    /// `IdentifierReference` and never arrives here.
    pub(super) fn parse_await(&mut self) -> Result<Expr, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        self.forbidden_in_parameters.get_or_insert(ParseError {
            kind: ParseErrorKind::AwaitInParameters,
            span: keyword.span,
        });
        self.enter()?;
        // A `UnaryExpression` and not an `AssignmentExpression`, which is the whole difference in
        // precedence from `yield`. No `[no LineTerminator here]` and no optional operand: §15.8's
        // Note 2 is explicit that you must await something.
        let argument = self.parse_unary(None);
        self.leave();
        let argument = argument?;
        let span = keyword.span.to(argument.span);
        Ok(Expr::new(ExprKind::Await(Box::new(argument)), span))
    }

    /// §15.9's two `AsyncArrowFunction` productions, and the call that the second may turn out
    /// to be instead.
    ///
    /// ```text
    /// AsyncArrowFunction : async [nlth] AsyncArrowBindingIdentifier [nlth] => AsyncConciseBody
    ///                    | CoverCallExpressionAndAsyncArrowHead [nlth] => AsyncConciseBody
    /// CoverCallExpressionAndAsyncArrowHead : MemberExpression Arguments
    /// AsyncArrowHead : async [nlth] ArrowFormalParameters[~Yield, +Await]
    /// ```
    ///
    /// The second is the third cover grammar in this parser and the least like the other two: the
    /// covering production is a *call*, so `async(a, b)` is read as one and the `=>` is what turns
    /// the arguments back into parameters. Which is why `async(a).b` and `async(a) + 1` are
    /// ordinary calls and cost nothing extra — the reading was right the first time.
    ///
    /// The first form needs no cover at all. `async x` has no other derivation, so an identifier
    /// on the same line commits and the `=>` is then required rather than looked for. That is also
    /// why §14.7.5 excludes `async of` from a for-of head: without the exclusion this would
    /// commit, and `for (async of b)` would be an arrow head with no arrow.
    ///
    /// Returns `None` without consuming anything when there is no `async` here to begin with.
    pub(super) fn parse_async_arrow_or_call(
        &mut self,
        allow_in: AllowIn,
    ) -> Result<Option<ArrowOrGroup>, ParseError> {
        if !self.at_contextual("async") {
            return Ok(None);
        }
        let next = self.peek(Goal::RegExp)?;
        // `async [no LineTerminator here] …`, in both productions. With a newline the word is an
        // expression statement of its own and whatever follows is a separate one.
        if next.newline_before {
            return Ok(None);
        }
        if self.is_identifier_token(next.kind) {
            return Ok(Some(ArrowOrGroup::Arrow(
                self.parse_async_arrow_with_one_parameter(allow_in)?,
            )));
        }
        if next.kind != TokenKind::LParen {
            return Ok(None);
        }
        self.parse_async_head_or_call(allow_in).map(Some)
    }

    /// The `CoverCallExpressionAndAsyncArrowHead` half, with the cursor on `async` and a `(`
    /// known to follow it on the same line.
    fn parse_async_head_or_call(&mut self, allow_in: AllowIn) -> Result<ArrowOrGroup, ParseError> {
        let keyword = self.advance(Goal::Div)?;
        self.enter()?;
        // Read as `Arguments`, which is the covering production. A `yield` or an `await` in there
        // is recorded and asked about below, exactly as for a parenthesized group — and so is a
        // `{a = 1}`, these parentheses being able to become parameters like any others.
        let enclosing_forbidden = self.forbidden_in_parameters.take();
        self.open_covers += 1;
        let list = self.parse_arguments();
        self.open_covers -= 1;
        let forbidden = self.forbidden_in_parameters.take();
        self.forbidden_in_parameters = enclosing_forbidden;
        self.leave();
        let list = list?;
        if self.current.kind == TokenKind::Arrow && !self.current.newline_before {
            // §15.9.1: "It is a Syntax Error if CoverCallExpressionAndAsyncArrowHead Contains
            // YieldExpression is true", and the same for `AwaitExpression`. The head is
            // `[~Yield, +Await]` whatever encloses it, so an `await` written here belongs to a
            // parameter default and there is nothing for it to suspend into.
            if let Some(error) = forbidden {
                return Err(error);
            }
            let parameters = self.refine_arguments_to_parameters(list, keyword.span)?;
            return Ok(ArrowOrGroup::Arrow(
                self.parse_arrow_tail(parameters, allow_in, true)?,
            ));
        }
        // No `=>`, so it was a call after all — and a `yield` or `await` inside it was written in
        // the enclosing code, which may itself be a parameter list.
        if let Some(error) = forbidden {
            self.forbidden_in_parameters.get_or_insert(error);
        }
        let name = crate::lexer::identifier_value(self.source, keyword.span)
            .ok_or_else(|| self.value_missing(keyword))?;
        let callee = Expr::new(ExprKind::Identifier(name.into_owned()), keyword.span);
        Ok(ArrowOrGroup::Operand(Expr::new(
            ExprKind::Call {
                optional: false,
                callee: Box::new(callee),
                arguments: list.arguments,
            },
            keyword.span.to(list.end),
        )))
    }

    /// `async [nlth] AsyncArrowBindingIdentifier [nlth] => AsyncConciseBody`, with the cursor on
    /// `async` and an identifier known to follow it on the same line.
    fn parse_async_arrow_with_one_parameter(
        &mut self,
        allow_in: AllowIn,
    ) -> Result<Expr, ParseError> {
        let keyword = self.advance(Goal::Div)?;
        // `AsyncArrowBindingIdentifier[Yield] : BindingIdentifier[?Yield, +Await]` — so `await` is
        // refused here however sloppy the enclosing code, where an ordinary arrow would take it.
        let enclosing = self.await_allowed;
        self.await_allowed = true;
        let name = self.parse_binding_name();
        self.await_allowed = enclosing;
        let name = name?;
        if self.current.kind != TokenKind::Arrow || self.current.newline_before {
            return Err(self.unexpected("`=>`"));
        }
        let span = keyword.span.to(name.span);
        let parameters = FormalParameters {
            items: Box::new([BindingElement {
                target: Binding::Identifier(name),
                default: None,
            }]),
            rest: None,
            span,
        };
        self.parse_arrow_tail(parameters, allow_in, true)
    }

    /// An `Arguments` refined into the `ArrowFormalParameters` the `=>` says it was (§15.9).
    ///
    /// The third refinement in this parser, and it targets the same `Binding` grammar the other
    /// two do — see [`super::binding`]. What is new is the rest element: an `ArgumentList` may
    /// spread anywhere and may end in a comma, and a parameter list may do neither. Both are
    /// "must cover an AsyncArrowHead".
    fn refine_arguments_to_parameters(
        &mut self,
        list: super::member::ArgumentList,
        keyword: Span,
    ) -> Result<FormalParameters, ParseError> {
        let count = list.arguments.len();
        let mut items = Vec::with_capacity(count);
        let mut rest = None;
        for (index, argument) in list.arguments.into_vec().into_iter().enumerate() {
            match argument {
                Argument::Value(value) => items.push(self.refine_to_binding_element(value)?),
                Argument::Spread(value) => {
                    if index + 1 != count || list.trailing_comma {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestParameterMustBeLast,
                            span: value.span,
                        });
                    }
                    rest = Some(Box::new(self.refine_to_binding(value)?));
                }
            }
        }
        let parameters = FormalParameters {
            items: items.into_boxed_slice(),
            rest,
            span: keyword.to(list.end),
        };
        super::arrow::check_unique_parameters(&parameters)?;
        // `AsyncArrowHead : async ArrowFormalParameters[~Yield, +Await]` — `[+Await]` whatever
        // encloses it, and §13.1.1 makes `await` under that parameter a Syntax Error. The
        // arguments were read under the *enclosing* `[Await]`, which is what the cover says, so
        // this is the one place the two readings disagree about a name rather than an expression.
        for element in &parameters.items {
            for declared in bound_names(&element.target) {
                if declared.name == "await" {
                    return Err(ParseError {
                        kind: ParseErrorKind::AwaitAsAsyncArrowParameter,
                        span: declared.span,
                    });
                }
            }
        }
        Ok(parameters)
    }

    /// Whether an `async` written here begins a function rather than being a name.
    ///
    /// `async [no LineTerminator here] function` — and the restriction is what makes
    /// `async\nfunction f() {}` two statements rather than one async function. `async` is not a
    /// reserved word, so this is a lookahead and not a token test; an escaped spelling is not the
    /// terminal the production names, exactly as with `get`, `set` and `static`.
    pub(super) fn at_async_function(&mut self) -> Result<bool, ParseError> {
        if !self.at_contextual("async") {
            return Ok(false);
        }
        let next = self.peek(Goal::RegExp)?;
        Ok(next.kind == TokenKind::Keyword(ReservedWord::Function) && !next.newline_before)
    }

    /// Whether an `async` written here begins a method rather than being one's name.
    ///
    /// `AsyncMethod : async [no LineTerminator here] ClassElementName ( … ) { … }`, and
    /// `AsyncGeneratorMethod` puts a `*` between the two. A `(` after the word makes it the
    /// method's own name — `({ async() {} })` — and so does a newline, and so does anything that
    /// cannot begin a `ClassElementName`.
    pub(super) fn at_async_method(&mut self) -> Result<bool, ParseError> {
        if !self.at_contextual("async") {
            return Ok(false);
        }
        let next = self.peek(Goal::Div)?;
        if next.newline_before {
            return Ok(false);
        }
        Ok(matches!(
            next.kind,
            TokenKind::Star
                | TokenKind::LBracket
                | TokenKind::String { .. }
                | TokenKind::Number { .. }
                | TokenKind::Keyword(_)
                | TokenKind::Identifier { .. }
                | TokenKind::PrivateIdentifier { .. }
        ))
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
    fn async_is_a_word_before_function_and_a_name_everywhere_else() {
        assert_eq!(statements("async function f() {}"), ["(async-fn f [] {})"]);
        assert_eq!(shape("(async function () {})"), "(async-fn <anon> [] {})");
        assert_eq!(shape("(async function f() {})"), "(async-fn f [] {})");
        // All four combinations of the two bits are productions of their own; §15.6's async
        // generator is both.
        assert_eq!(
            statements("async function* f() {}"),
            ["(async-fn* f [] {})"]
        );
        // `async` is not a reserved word, so every other use of it is an ordinary name.
        for source in [
            "async;",
            "async = 1;",
            "async(a);",
            "async.a;",
            "async++;",
            "var async;",
            "let async = 1; async(1);",
            "({ async: 1 });",
            "({ async });",
            "async => 1;",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // `async [no LineTerminator here] function`. The restriction does not make a bad async
        // function — it makes `async` an expression statement, and then the next line is an
        // ordinary declaration. Both parse, so the shape is what says which happened.
        assert_eq!(
            statements("async\nfunction f() {}"),
            ["async", "(fn f [] {})"]
        );
        // A `Declaration`, so §14.5 refuses it where only a `Statement` may stand — the third
        // word on that list, and the only one that needs a lookahead to find.
        for source in ["if (a) async function f() {}", "a: async function f() {}"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
    }

    #[test]
    fn an_async_method_is_the_word_then_the_name_and_has_no_accessor_form() {
        assert_eq!(shape("({async m() {}})"), "{(m (async-fn <anon> [] {}))}");
        assert_eq!(shape("({async *m() {}})"), "{(m (async-fn* <anon> [] {}))}");
        assert_eq!(
            statements("class C { async m() {} }"),
            ["(class C - [(m (async-fn <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { static async *m() {} }"),
            ["(class C - [(static m (async-fn* <anon> [] {}))])"]
        );
        // A `(` after the word makes it the method's own name, and so does anything that cannot
        // begin a `ClassElementName`.
        for source in [
            "({async() {}});",
            "class C { async() {} }",
            "class C { static async() {} }",
            "class C { get async() {} }",
            "({ get async() {} });",
            "({ set async(v) {} });",
            "class C { async async() {} }",
            "({ *async() {} });",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // …and so does a line terminator, `AsyncMethod` carrying the same restriction the
        // declaration does.
        assert!(parse_expression("({async\nm() {}})").is_err());
        // §15.7's `MethodDefinition` gives the accessor forms neither `async` nor `*`, so the
        // word `get` here is this method's name and then `m` is unexpected.
        assert!(parse_script("class C { async get m() {} }").is_err());
        assert!(parse_script("class C { get *m() {} }").is_err());
        // §15.7.1: `new` can neither await nor resume, so the constructor may be neither.
        assert_eq!(
            kind("class C { async constructor() {} }"),
            ParseErrorKind::ConstructorMayNotBeAsync
        );
        assert_eq!(
            kind("class C { async *constructor() {} }"),
            ParseErrorKind::ConstructorMayNotBeAGenerator
        );
        assert_eq!(
            kind("class C { static async prototype() {} }"),
            ParseErrorKind::StaticPrototype
        );
        // An async method is a method, so it has `super.a` where an async function does not.
        assert!(parse_script("class C { async m() { super.a; } }").is_ok());
        assert!(parse_script("async function f() { super.a; }").is_err());
    }

    #[test]
    fn await_takes_a_unary_expression_which_is_where_it_differs_from_yield() {
        assert_eq!(
            shape("(async function () { await a; })"),
            "(async-fn <anon> [] {(await a)})"
        );
        // A `UnaryExpression`, so it binds tighter than nearly everything — and every one of
        // these has no derivation at all when the operator is `yield`.
        assert_eq!(
            shape("(async function () { await a + b })"),
            "(async-fn <anon> [] {(+ (await a) b)})"
        );
        assert_eq!(
            shape("(async function () { await a ? b : c })"),
            "(async-fn <anon> [] {(? (await a) b c)})"
        );
        for source in [
            "await a in b;",
            "await a.b;",
            "await a();",
            "await await a;",
            "typeof await a;",
            "delete await a;",
            "-await a;",
            "await -a;",
            "a = await b;",
            "await a, b;",
            "[await a];",
            "f(await a);",
            "[...await a];",
            "`${await a}`;",
            "await `x`;",
            "await /a/;",
            "await a?.b;",
            "return await a;",
            "throw await a;",
            "if (await a) ;",
            "switch (await a) {}",
        ] {
            assert!(
                parse_script(&format!("async function f() {{ {source} }}")).is_ok(),
                "{source:?}"
            );
        }
        // §13.6 refuses a `UnaryExpression` on the left of `**`, and §15.8 makes this one — so it
        // needs the parentheses `-a ** b` needs.
        assert_eq!(
            kind("async function f() { await a ** b; }"),
            ParseErrorKind::ExponentiationOnUnary
        );
        assert!(parse_script("async function f() { (await a) ** b; }").is_ok());
        // §15.8's Note 2: unlike `yield`, the operand is mandatory. There is no bare form and so
        // no question of what may follow.
        assert!(parse_script("async function f() { await; }").is_err());
        assert!(parse_script("async function f() { await ...a; }").is_err());
        // …and no `[no LineTerminator here]` either, so a newline changes nothing.
        assert!(parse_script("async function f() { await\na; }").is_ok());
        // `new` takes a `MemberExpression`, which this is not.
        assert!(parse_script("async function f() { new await a; }").is_err());
    }

    #[test]
    fn the_await_parameter_turns_on_and_off_exactly_where_the_yield_one_does() {
        // Off outside, so `await` is an ordinary name — and `await a` is two things.
        assert_eq!(statements("await;"), ["await"]);
        assert_eq!(statements("var await;"), ["(var await)"]);
        assert!(parse_script("await a;").is_err());
        assert!(parse_script("function f() { await a; }").is_err());
        // A nested plain function turns it back off, and a nested async one back on.
        assert!(parse_script("async function f() { function g(await) {} }").is_ok());
        assert!(parse_script("async function f() { (function () { await; }); }").is_ok());
        assert!(parse_script("async function f() { ({ m(a = await b) {} }); }").is_err());
        assert!(parse_script("async function f() { function g() { await a; } }").is_err());
        assert!(parse_script("async function f() { ({ async m() { await a; } }); }").is_ok());
        // A generator body is `[~Await]` and an async body is `[~Yield]`, so each turns the
        // other's word back into a name.
        assert!(parse_script("async function f() { yield; }").is_ok());
        assert!(parse_script("async function f() { (function* () { await a; }); }").is_err());
        // …and an async generator has both.
        assert!(parse_script("async function* g() { await a; yield b; }").is_ok());
        // Every binding position refuses the name where the parameter is set.
        for source in ["var await;", "class await {}", "await: 1;", "({await});"] {
            assert!(
                parse_script(&format!("async function f() {{ {source} }}")).is_err(),
                "{source:?}"
            );
        }
        // …while a property name is an `IdentifierName` and does not care.
        assert!(parse_script("async function f() { a.await; ({await: 1}); }").is_ok());
        // A declaration's name inherits the parameter and an expression's does not — the same
        // asymmetry `[Yield]` has, and observable in the same way.
        assert!(parse_script("async function await() {}").is_ok());
        assert!(parse_script("async function f() { (function await() {}); }").is_ok());
        assert!(parse_expression("(async function await() {})").is_err());
        assert!(parse_script("async function f() { var await; }").is_err());
        // An arrow's parameters keep it and its body drops it.
        assert!(parse_script("async function f() { () => await; }").is_ok());
        assert!(parse_script("async function f() { () => await a; }").is_err());
        assert!(parse_script("async function f(a = () => await) {}").is_ok());
    }

    #[test]
    fn one_record_serves_both_contains_rules_because_only_one_can_ever_apply() {
        // §15.8.1, the mirror of §15.5.1: a default is evaluated before the function is
        // suspendable, so there is nothing for the `await` to suspend into.
        for source in [
            "async function f(a = await b) {}",
            "async function f(a = (await b)) {}",
            "async function* g(a = await b) {}",
            "({ async m(a = await b) {} });",
            "async function f() { (async function (a = await b) {}); }",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::AwaitInParameters,
                "{source:?}"
            );
        }
        // …and the name itself is refused for the other reason: a binding under `[+Await]`.
        assert!(parse_script("async function f(await) {}").is_err());
        // An async generator's parameters are `[+Yield, +Await]`, so §15.6.1 forbids both — and
        // which one the record holds is whichever was written.
        assert_eq!(
            kind("async function* g(a = yield b) {}"),
            ParseErrorKind::YieldInParameters
        );
        // `Contains` stops at a function boundary, so a nested function's `await` is its own.
        for source in [
            "async function f(a = function () { await; }) {}",
            "async function f(a = () => await) {}",
            "async function f(a = { m() { await; } }) {}",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
    }

    #[test]
    fn an_ordinary_arrow_whose_parameter_is_called_async_is_still_one() {
        // The thing that would break if either lookahead were a token test rather than a
        // lookahead: `async` is a perfectly ordinary parameter name.
        assert_eq!(shape("async => 1"), "(=> [async] 1)");
        assert_eq!(shape("(async) => 1"), "(=> [async] 1)");
        assert_eq!(shape("(a, async) => 1"), "(=> [a async] 1)");
    }

    #[test]
    fn an_async_arrow_has_two_productions_and_only_one_of_them_needs_a_cover() {
        // `async [nlth] AsyncArrowBindingIdentifier [nlth] => AsyncConciseBody` — one token of
        // parameters, and no cover at all: `async x` has no other derivation.
        assert_eq!(shape("async a => a"), "(async=> [a] a)");
        assert_eq!(
            shape("async a => { return 1; }"),
            "(async=> [a] {(return 1)})"
        );
        // `CoverCallExpressionAndAsyncArrowHead [nlth] => AsyncConciseBody`, where the covering
        // production is a *call* — so the arguments are read as arguments and the `=>` turns them
        // back into parameters.
        assert_eq!(shape("async () => 1"), "(async=> [] 1)");
        assert_eq!(shape("async (a) => 1"), "(async=> [a] 1)");
        assert_eq!(shape("async (a, b) => 1"), "(async=> [a b] 1)");
        assert_eq!(shape("async (a,) => 1"), "(async=> [a] 1)");
        assert_eq!(shape("async (...a) => 1"), "(async=> [(... a)] 1)");
        assert_eq!(shape("async (a, ...b) => 1"), "(async=> [a (... b)] 1)");
        assert_eq!(shape("async (a = 1) => 1"), "(async=> [(= a 1)] 1)");
        assert_eq!(shape("async ([a]) => 1"), "(async=> [[a]] 1)");
        assert_eq!(shape("async ({a}) => 1"), "(async=> [{(a a)}] 1)");
        // No space needed, `async` being a word rather than a punctuator.
        assert_eq!(shape("async(a) => 1"), "(async=> [a] 1)");
        // …and being an `AssignmentExpression`, it stands exactly where one may.
        assert!(parse_script("x = async () => 1;").is_ok());
        assert!(parse_script("f(async () => 1);").is_ok());
        assert!(parse_script("[async () => 1];").is_ok());
        assert!(parse_script("a ? async () => 1 : b;").is_ok());
        assert_eq!(shape("async () => 1, 2"), "(, (async=> [] 1) 2)");
        for source in ["typeof async () => 1;", "new async () => 1;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn the_cover_is_a_call_so_a_call_costs_nothing_to_read_twice_because_it_is_not() {
        // No `=>`, so the reading was right the first time and the node is the call it was.
        assert_eq!(shape("async(a)"), "(call async [a])");
        assert_eq!(shape("async()"), "(call async [])");
        assert_eq!(shape("async(a, b)"), "(call async [a b])");
        assert_eq!(shape("async(...a)"), "(call async [(... a)])");
        // …and the suffixes that follow a call follow this one.
        assert_eq!(shape("async(a).b"), "(. (call async [a]) b)");
        assert_eq!(shape("async(a)(b)"), "(call (call async [a]) [b])");
        assert_eq!(shape("async(a) + 1"), "(+ (call async [a]) 1)");
        assert!(parse_script("new async(a);").is_ok());
        assert!(parse_script("async(a)`x`;").is_ok());
        // An `ArgumentList` has no elision, exactly as anywhere else.
        for source in ["async(,) => 1;", "async(a,,b) => 1;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn the_refinement_is_to_a_binding_and_the_rest_element_is_where_the_two_readings_differ() {
        // A `Binding` and not a `Pattern`, as for an ordinary arrow: parameters create names.
        for source in ["async (a.b) => 1;", "async (1) => 1;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // An `ArgumentList` may spread anywhere and may end in a comma; `FormalParameters` puts
        // the rest last and allows no comma after it. Both are "must cover an AsyncArrowHead".
        for source in ["async (...a, b) => 1;", "async(...a,) => 1;"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::RestParameterMustBeLast,
                "{source:?}"
            );
        }
        // …and as arguments the very same two are ordinary.
        assert!(parse_script("async(...a, b);").is_ok());
        assert!(parse_script("async(...a,);").is_ok());
        // `UniqueFormalParameters`, the same rule the other arrow form gets and from the same
        // check — an arrow's parameters may never repeat.
        assert_eq!(
            kind("async (a, a) => 1;"),
            ParseErrorKind::DuplicateParameterName
        );
        assert_eq!(
            kind("async (a, [a]) => 1;"),
            ParseErrorKind::DuplicateParameterName
        );
        // §15.9.1's two `Contains` rules, which are §15.5.1's and §15.8.1's over again.
        assert_eq!(
            kind("async function f() { async (a = await b) => 1; }"),
            ParseErrorKind::AwaitInParameters
        );
        assert_eq!(
            kind("function* g() { async (a = yield b) => 1; }"),
            ParseErrorKind::YieldInParameters
        );
        // `AsyncArrowHead`'s parameters are `[+Await]` whatever encloses them, while the covering
        // arguments were read under the enclosing one — the single place the two readings
        // disagree about a *name* rather than an expression.
        assert_eq!(
            kind("async (await) => 1;"),
            ParseErrorKind::AwaitAsAsyncArrowParameter
        );
        assert!(parse_script("async(await);").is_ok());
        // The one-parameter form reads its name under `[+Await]` directly, so it fails earlier
        // and for the same reason.
        assert!(parse_script("async await => 1;").is_err());
    }

    #[test]
    fn both_line_terminator_restrictions_make_two_statements_rather_than_a_bad_arrow() {
        // `async [no LineTerminator here] …` — with a newline the word is an expression statement
        // and what follows is a separate one, which is why this parses at all.
        assert_eq!(statements("async\na => a;"), ["async", "(=> [a] a)"]);
        // …but only where automatic insertion would put a `;`. A `(` continues the
        // expression, so this is the call `async(a)` and then a `=>` that nothing wanted —
        // the restriction removed the arrow reading and did not supply a statement break.
        assert!(parse_script("async\n(a) => a;").is_err());
        assert_eq!(shape("async\n(a)"), "(call async [a])");
        // The `=>` restriction is the ordinary arrow's, and it makes a `=>` that nothing wanted.
        assert!(parse_script("async (a)\n=> a;").is_err());
        assert!(parse_script("async a\n=> a;").is_err());
    }

    #[test]
    fn async_stays_an_ordinary_name_and_the_words_after_it_stay_ordinary_names() {
        // The one-parameter form commits on any identifier, so every word that is not reserved
        // may be the parameter — including the ones that mean something elsewhere.
        for source in [
            "async of => 1;",
            "async let => 1;",
            "async yield => 1;",
            "async async => 1;",
            "async (async) => 1;",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // …and `yield` only while `[~Yield]`, the one-parameter form's name inheriting it.
        assert!(parse_script("function* g() { async yield => 1; }").is_err());
        // Anything that is not a `BindingIdentifier` or a `(` is not an arrow head at all, so
        // `async` is left as the name it was and then the next token is unexpected.
        for source in [
            "async if => 1;",
            "async 1 => 1;",
            "async [a] => 1;",
            "async {a} => 1;",
            "async ...a => 1;",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // Committing is what makes these errors rather than misreadings: `async of` has no other
        // derivation, so there is no reading in which the `=>` is optional.
        assert!(parse_script("async of;").is_err());
        assert!(parse_script("async let;").is_err());
    }

    #[test]
    fn an_async_arrows_body_is_the_one_concise_body_that_keeps_the_await_parameter() {
        // `AsyncConciseBody : ExpressionBody[?In, +Await] | { AsyncFunctionBody }`, where an
        // ordinary `ConciseBody` drops both parameters.
        assert!(parse_script("async () => await a;").is_ok());
        assert!(parse_script("async a => await a;").is_ok());
        assert!(parse_script("async () => { await a; };").is_ok());
        // …and the operand is mandatory in there as anywhere else.
        assert!(parse_script("async () => await;").is_err());
        assert!(parse_script("async () => { await; };").is_err());
        // `[Yield]` is dropped either way: there is no async *generator* arrow, an arrow having
        // no `yield` of its own to suspend at.
        assert!(parse_script("function* g() { async () => yield a; }").is_err());
        // Everything else a body is, it still is.
        assert!(parse_script("async () => ({});").is_ok());
        assert_eq!(
            kind("async () => { break; };"),
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            kind("async (a) => { let a; };"),
            ParseErrorKind::ParameterRedeclaredInBody
        );
        assert_eq!(
            kind("async (a = 1) => { \"use strict\"; };"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        assert_eq!(
            kind("\"use strict\"; async (eval) => 1;"),
            ParseErrorKind::StrictEvalOrArguments
        );
        // It is an arrow, so it inherits `super` and `new.target` rather than stopping them.
        assert!(parse_script("class C { m() { async () => super.a; } }").is_ok());
        assert!(parse_script("function f() { async () => new.target; }").is_ok());
        assert_eq!(
            kind("async () => super.a;"),
            ParseErrorKind::SuperPropertyOutsideMethod
        );
        assert_eq!(
            kind("async () => new.target;"),
            ParseErrorKind::NewTargetOutsideFunction
        );
    }

    #[test]
    fn for_await_is_a_for_of_and_needs_the_await_parameter() {
        assert_eq!(
            statements("async function f() { for await (const a of b); }"),
            ["(async-fn f [] {(for-await-of (const a) b <empty>)})"]
        );
        for source in [
            "async function f() { for await (a of b); }",
            "async function f() { for await (var a of b); }",
            "async function f() { for await (let a of b); }",
            "async function f() { for await (const [a] of b); }",
            "async function f() { for await ([a] of b); }",
            "async function f() { for await (const a of b) c; }",
            "async function* g() { for await (const a of b); }",
            "async () => { for await (const a of b); };",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // No `[no LineTerminator here]`, so the two words may be on separate lines.
        assert!(parse_script("async function f() { for\nawait (const a of b); }").is_ok());
        // Every `for await` alternative is `[+Await]`-gated: there is nothing to suspend in a
        // plain function or at the top of a script.
        assert_eq!(
            kind("for await (const a of b);"),
            ParseErrorKind::ForAwaitOutsideAsync
        );
        assert_eq!(
            kind("function f() { for await (const a of b); }"),
            ParseErrorKind::ForAwaitOutsideAsync
        );
        // …and every one of them is a `for`-`of`: there is no asynchronous enumeration of
        // property keys, and nothing to await in a three-part loop.
        for source in [
            "async function f() { for await (a in b); }",
            "async function f() { for await (const a in b); }",
            "async function f() { for await (;;); }",
            "async function f() { for await (a; b; c); }",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::ForAwaitMustBeForOf,
                "{source:?}"
            );
        }
        // The header is the ordinary one in every other respect.
        assert!(parse_script("async function f() { for await (const a of b, c); }").is_err());
        assert!(parse_script("async function f() { for await (let of b); }").is_err());
        assert!(parse_script("async function f() { for await (const a of await b); }").is_ok());
    }

    #[test]
    fn no_async_function_however_truncated_can_panic() {
        let deep = format!("async function f() {{ {}1; }}", "await ".repeat(1000));
        let cases = [
            "async".to_string(),
            "async function".to_string(),
            "async function f".to_string(),
            "async function f(".to_string(),
            "async function f() {".to_string(),
            "async function f() { await".to_string(),
            "async function*".to_string(),
            "({async".to_string(),
            "({async m".to_string(),
            "class C { async".to_string(),
            "class C { async *".to_string(),
            "async (".to_string(),
            "async (a".to_string(),
            "async (a)".to_string(),
            "async (a) =>".to_string(),
            "async a".to_string(),
            "async a =>".to_string(),
            "for await".to_string(),
            "async function f() { for await".to_string(),
            "async function f() { for await (".to_string(),
            "async (a) => ".repeat(1000),
            "async a => ".repeat(1000),
            deep.clone(),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // An `await` operand is a `UnaryExpression`, so a chain of them nests and the cap bounds
        // it — one level shallower, the function itself holding one while its body is read.
        assert_eq!(kind(&deep), ParseErrorKind::TooDeeplyNested);
        // …and an async arrow nests through its body, so a chain of them is bounded too.
        assert_eq!(
            kind(&format!("{}1;", "async a => ".repeat(1000))),
            ParseErrorKind::TooDeeplyNested
        );
        assert_eq!(
            kind(&format!("{}1;", "async (a) => ".repeat(1000))),
            ParseErrorKind::TooDeeplyNested
        );
    }
}
