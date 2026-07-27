//! Method definitions (ECMAScript §15.4) — the last `PropertyDefinition` alternative.
//!
//! # Telling `get` the accessor from `get` the property
//!
//! `get` and `set` are ordinary identifiers, so all four of these are legal and only two of them
//! are accessors:
//!
//! ```text
//! { get: 1 }        a property named `get`
//! { get() {} }      a method named `get`
//! { get a() {} }    a getter for `a`
//! { get [x]() {} }  a getter for a computed name
//! ```
//!
//! What decides is the token *after* the word: a `(` means the word was the name, and anything
//! that can begin a `PropertyName` means it was the keyword. That is the third and last place this
//! parser looks at two tokens, after `let` and a labelled statement — and, like both of those, it
//! is a case where one word begins two productions.
//!
//! # A getter takes nothing and a setter takes exactly one
//!
//! `get ClassElementName ( ) { FunctionBody }` and
//! `set ClassElementName ( PropertySetParameterList ) { FunctionBody }`, where
//! `PropertySetParameterList : FormalParameter`. Singular, and a `FormalParameter` rather than a
//! `FormalParameters` — so a setter may take a pattern or a default and may not take a rest.
//!
//! # `UniqueFormalParameters` is where the name says the rule
//!
//! A method's parameters are `UniqueFormalParameters`, so `({a(b, b) {}})` is refused where
//! `function f(b, b) {}` is fine. §15.1.1 states it on the production, not on strictness — a
//! method's list may never repeat, sloppy or not.

use super::body::{BodyContext, SuperAllowed};
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{FormalParameters, Function, MethodKind, PropertyKey};
use crate::lexer::TokenKind;
use crate::static_semantics::bound_names;
use std::collections::HashSet;

impl Parser<'_> {
    /// Whether an accessor begins here, given that `word` was just read as a key.
    ///
    /// True when the word was `get` or `set` written plainly and something that can begin a
    /// `PropertyName` follows it. A `(` does not: that makes the word the method's own name.
    pub(super) fn at_accessor(&self, key: &PropertyKey, escaped: bool) -> Option<MethodKind> {
        let PropertyKey::Identifier(name) = key else {
            return None;
        };
        if escaped {
            return None;
        }
        let kind = match &**name {
            "get" => MethodKind::Get,
            "set" => MethodKind::Set,
            _ => return None,
        };
        // A `ClassElementName`, so a `PrivateIdentifier` counts — `get #a() {}` is an accessor
        // for a private member, and `get` is the accessor rather than the name.
        let starts_a_name = matches!(
            self.current.kind,
            TokenKind::LBracket
                | TokenKind::String { .. }
                | TokenKind::Number { .. }
                | TokenKind::BigInt
                | TokenKind::Keyword(_)
                | TokenKind::Identifier { .. }
                | TokenKind::PrivateIdentifier { .. }
        );
        starts_a_name.then_some(kind)
    }

    /// The parameters and body of a `MethodDefinition` whose name has been read, with the
    /// cursor on the `(`.
    ///
    /// The function alone, rather than a finished node: an object literal's methods and a
    /// class's are the same production and land in different trees, so the node is the
    /// caller's to build. `super_allowed` is the caller's for the same reason — every method
    /// may read `super.a`, and only a derived class's constructor may call `super(…)`.
    pub(super) fn parse_method(
        &mut self,
        kind: MethodKind,
        super_allowed: SuperAllowed,
        is_generator: bool,
        is_async: bool,
    ) -> Result<Box<Function>, ParseError> {
        // `GeneratorMethod : * ClassElementName ( UniqueFormalParameters[+Yield] ) { GeneratorBody }`
        // — so the `*` reaches the parameters as much as the body, exactly as a generator
        // function's does.
        let enclosing = (self.yield_allowed, self.await_allowed);
        self.yield_allowed = is_generator;
        self.await_allowed = is_async;
        let parts = self
            .parse_method_parameters(kind, is_generator, is_async)
            .and_then(|parameters| {
                let body = self.parse_function_body(BodyContext::method(super_allowed))?;
                Ok((parameters, body))
            });
        (self.yield_allowed, self.await_allowed) = enclosing;
        let (parameters, (body, end, declares_strict)) = parts?;
        self.check_method_body(&parameters, &body, declares_strict)?;
        Ok(Box::new(Function {
            // A method's name is its key's, and the key is not a binding — nothing inside the
            // body can see it, where a named function expression's name is visible within.
            name: None,
            parameters,
            body,
            is_generator,
            is_async,
            span: end,
        }))
    }

    /// The parameter list a method of this kind may have (§15.4).
    fn parse_method_parameters(
        &mut self,
        kind: MethodKind,
        is_generator: bool,
        is_async: bool,
    ) -> Result<FormalParameters, ParseError> {
        let parameters = self.parse_parameters_of(is_generator, is_async)?;
        let count = parameters.items.len();
        match kind {
            // `get ClassElementName ( )` — the parentheses are empty in the grammar, so a getter
            // with a parameter is not a getter that ignores it.
            MethodKind::Get if count != 0 || parameters.rest.is_some() => Err(ParseError {
                kind: ParseErrorKind::AccessorParameterCount,
                span: parameters.span,
            }),
            // `PropertySetParameterList : FormalParameter` — singular, and a `FormalParameter`
            // rather than a `FormalParameters`, so a pattern and a default are both fine and a
            // rest is not.
            MethodKind::Set if count != 1 || parameters.rest.is_some() => Err(ParseError {
                kind: ParseErrorKind::AccessorParameterCount,
                span: parameters.span,
            }),
            _ => Ok(parameters),
        }
    }

    /// §15.4.1 and §15.1.1, for a method.
    ///
    /// The same two rules §15.2.1 gives a function, and one more that a function does not have:
    /// a method's parameters are `UniqueFormalParameters`, so they may never repeat — sloppy or
    /// not, simple or not.
    fn check_method_body(
        &self,
        parameters: &FormalParameters,
        body: &[crate::ast::Stmt],
        declares_strict: bool,
    ) -> Result<(), ParseError> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut names = Vec::new();
        for element in &parameters.items {
            names.extend(bound_names(&element.target));
        }
        if let Some(rest) = &parameters.rest {
            names.extend(bound_names(rest));
        }
        for declared in &names {
            if !seen.insert(declared.name.to_string()) {
                return Err(ParseError {
                    kind: ParseErrorKind::DuplicateParameterName,
                    span: declared.span,
                });
            }
        }
        if declares_strict && !parameters.is_simple() {
            return Err(ParseError {
                kind: ParseErrorKind::UseStrictWithNonSimpleParameters,
                span: parameters.span,
            });
        }
        for declared in crate::static_semantics::top_level_lexically_declared_names(body) {
            if names.iter().any(|bound| bound.name == declared.name) {
                return Err(ParseError {
                    kind: ParseErrorKind::ParameterRedeclaredInBody,
                    span: declared.span,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression};

    /// The kind of error `source` fails with.
    fn kind(source: &str) -> ParseErrorKind {
        match parse_expression(source) {
            Err(err) => err.kind,
            Ok(expr) => panic!("{source:?} should not parse, got {expr:?}"), // needs the error
        }
    }

    #[test]
    fn a_method_is_a_property_whose_value_is_written_as_a_function() {
        assert_eq!(shape("({a() {}})"), "{(a (fn <anon> [] {}))}");
        assert_eq!(shape("({a(b) {}})"), "{(a (fn <anon> [b] {}))}");
        assert_eq!(shape("({a(b, c) {}})"), "{(a (fn <anon> [b c] {}))}");
        assert_eq!(shape("({a(...b) {}})"), "{(a (fn <anon> [(... b)] {}))}");
        assert_eq!(shape("({a(b = 1) {}})"), "{(a (fn <anon> [(= b 1)] {}))}");
        assert_eq!(shape("({a([b]) {}})"), "{(a (fn <anon> [[b]] {}))}");
        assert_eq!(
            shape("({a() { return 1; }})"),
            "{(a (fn <anon> [] {(return 1)}))}"
        );
        // A `ClassElementName` is a `PropertyName`, so every key form works.
        assert_eq!(shape("({if() {}})"), "{(if (fn <anon> [] {}))}");
        assert_eq!(shape("({1() {}})"), "{(n1 (fn <anon> [] {}))}");
        assert_eq!(shape("({[x]() {}})"), "{([x] (fn <anon> [] {}))}");
        // …and a method is a `PropertyDefinition`, so it sits among the others.
        assert_eq!(shape("({a: 1, b() {}})"), "{(a 1) (b (fn <anon> [] {}))}");
        assert_eq!(shape("({a() {}, b})"), "{(a (fn <anon> [] {})) b}");
    }

    #[test]
    fn get_and_set_are_keywords_only_when_a_name_follows_them() {
        assert_eq!(shape("({get a() {}})"), "{(get a (fn <anon> [] {}))}");
        assert_eq!(shape("({set a(v) {}})"), "{(set a (fn <anon> [v] {}))}");
        assert_eq!(
            shape("({get a() {}, set a(v) {}})"),
            "{(get a (fn <anon> [] {})) (set a (fn <anon> [v] {}))}"
        );
        assert_eq!(shape("({get [x]() {}})"), "{(get [x] (fn <anon> [] {}))}");
        assert_eq!(shape("({get if() {}})"), "{(get if (fn <anon> [] {}))}");
        // A `(` means the word was the name, not the keyword — so these are ordinary methods.
        assert_eq!(shape("({get() {}})"), "{(get (fn <anon> [] {}))}");
        assert_eq!(shape("({set() {}})"), "{(set (fn <anon> [] {}))}");
        // …and a `:` means it was an ordinary key, as it always was.
        assert_eq!(shape("({get: 1, set: 2})"), "{(get 1) (set 2)}");
        assert_eq!(shape("({get, set})"), "{get set}");
        // An escaped spelling is no keyword, §5.1.5.1 again.
        assert!(parse_expression(r"({\u0067et a() {}})").is_err());
    }

    #[test]
    fn a_getter_takes_nothing_and_a_setter_takes_exactly_one() {
        // `get ClassElementName ( )` — empty in the grammar, so a getter with a parameter is not
        // a getter that ignores it.
        assert_eq!(
            kind("({get a(b) {}})"),
            ParseErrorKind::AccessorParameterCount
        );
        assert_eq!(
            kind("({get a(...b) {}})"),
            ParseErrorKind::AccessorParameterCount
        );
        // `PropertySetParameterList : FormalParameter` — singular, and a `FormalParameter`, so a
        // pattern and a default are fine and a rest is not.
        assert_eq!(
            kind("({set a() {}})"),
            ParseErrorKind::AccessorParameterCount
        );
        assert_eq!(
            kind("({set a(b, c) {}})"),
            ParseErrorKind::AccessorParameterCount
        );
        assert_eq!(
            kind("({set a(...b) {}})"),
            ParseErrorKind::AccessorParameterCount
        );
        assert!(parse_expression("({set a([b]) {}})").is_ok());
        assert!(parse_expression("({set a(b = 1) {}})").is_ok());
    }

    #[test]
    fn a_methods_parameters_are_unique_where_a_functions_are_not() {
        // `UniqueFormalParameters`, which says the rule in its name: a method's list may never
        // repeat, where a plain function's simple list may.
        assert_eq!(
            kind("({a(b, b) {}})"),
            ParseErrorKind::DuplicateParameterName
        );
        assert!(crate::parser::parse_script("function f(b, b) {}").is_ok());
        // The two rules a method shares with a function.
        assert_eq!(
            kind("({a(b) { let b; }})"),
            ParseErrorKind::ParameterRedeclaredInBody
        );
        assert_eq!(
            kind("({a(b = 1) { \"use strict\"; }})"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        assert!(parse_expression("({a() { \"use strict\"; }})").is_ok());
        assert!(parse_expression("({a(b) { var b; }})").is_ok());
        assert!(parse_expression("({a(b) { let c; }})").is_ok());
        // A body is still a body, so `return` is legal and the walks still stop.
        assert!(parse_expression("({a() { return; }})").is_ok());
        assert!(parse_expression("({a() { break; }})").is_err());
    }

    #[test]
    fn no_method_however_truncated_can_panic() {
        for source in [
            "({a(",
            "({a()",
            "({a() {",
            "({get",
            "({get a",
            "({get a(",
            "({set a(",
            "({get a() {",
            "({*a(){}})",
            "({async a(){}})",
        ] {
            let _ = parse_expression(source);
        }
        // All four `MethodDefinition` alternatives now — see [`super::generator`] and
        // [`super::asynchronous`].
        for source in ["({*a() {}})", "({async a() {}})", "({async *a() {}})"] {
            assert!(parse_expression(source).is_ok(), "{source:?}");
        }
    }
}
