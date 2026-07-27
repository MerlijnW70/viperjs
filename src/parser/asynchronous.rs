//! `async` functions and the `await` they make legal (ECMAScript §15.8, §15.6).
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

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Expr, ExprKind};
use crate::lexer::{Goal, ReservedWord, TokenKind};

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
    fn the_two_async_forms_this_slice_does_not_reach_fail_rather_than_being_misread() {
        // Pinned so that implementing each is a deliberate change. Both are real JavaScript.
        //
        // §15.9's `AsyncArrowFunction` is the second half of the cover grammar
        // [`super::arrow`] already carries — `async(a, b)` is a call *or* an arrow head, and
        // telling them apart is a slice of its own.
        for source in ["async a => a;", "async (a) => a;", "async () => 1;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // …while an ordinary arrow whose parameter happens to be called `async` is unaffected,
        // which is the thing that would break if the lookahead were a token test.
        assert_eq!(shape("async => 1"), "(=> [async] 1)");
        // §14.7.5's `for await (… of …)`, which needs `[+Await]` and a flag on the loop.
        assert!(parse_script("async function f() { for await (const a of b); }").is_err());
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
            deep.clone(),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // An `await` operand is a `UnaryExpression`, so a chain of them nests and the cap bounds
        // it — one level shallower, the function itself holding one while its body is read.
        assert_eq!(kind(&deep), ParseErrorKind::TooDeeplyNested);
    }
}
