//! Object literals (ECMAScript §13.2.5).
//!
//! # No elisions, which is the opposite of the sibling next door
//!
//! `PropertyDefinitionList` is a plain comma-separated list with an optional trailing comma, so
//! `{, }` and `{a: 1, , }` have no derivation. An array literal admits both, because `Elision` is
//! part of `ElementList` and nothing like it is part of this one. The two look alike and are not,
//! and this is the difference worth knowing.
//!
//! # `__proto__` may be written twice, unless both times are the same way
//!
//! §13.2.5.1 refuses a duplicate `__proto__` only when at least two of the entries come from
//! `PropertyDefinition : PropertyName : AssignmentExpression`. So:
//!
//! ```text
//! { __proto__: 1, __proto__: 2 }         refused, two of that production
//! { "__proto__": 1, __proto__: 2 }       refused, a StringLiteral is a PropertyName too
//! { __proto__: 1, ["__proto__"]: 2 }     allowed, a ComputedPropertyName is not one of them
//! { __proto__: 1, __proto__ }            allowed, shorthand is a different production
//! ```
//!
//! The rule is narrow because only that one production sets the prototype; the others define an
//! ordinary property that happens to be spelled `__proto__`, and defining one twice was never an
//! error.
//!
//! # What is not here
//!
//! `MethodDefinition` — `{a() {}}`, `{get a() {}}`, `{set a(v) {}}` — which is a
//! `PropertyDefinition` alternative and needs functions. And `ObjectAssignmentPattern`: `({a} = b)`
//! and `({a = 1} = b)` parse in ECMAScript and are refused here, for the reason an array is not
//! yet a pattern either. `{a = 1}` is worth a word of its own: it is `CoverInitializedName`, a
//! production that exists *only* so the cover grammar can reach a pattern, and §13.2.5.1 says to
//! always throw a Syntax Error if it is matched. Refused here too, then — but for a thinner
//! reason, since what it needs is the refinement rather than the rule.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Expr, ExprKind, PropertyDefinition, PropertyKey};
use crate::lexer::{Goal, TokenKind, identifier_value, numeric_value, string_value};
use crate::span::Span;

impl Parser<'_> {
    /// `ObjectLiteral` (§13.2.5), with the cursor on the `{`.
    pub(super) fn parse_object_literal(&mut self) -> Result<Expr, ParseError> {
        let open = self.advance(Goal::RegExp)?;
        self.enter()?;
        self.literal_depth += 1;
        let properties = self.parse_property_definitions();
        self.literal_depth -= 1;
        self.leave();
        let properties = properties?;
        let close = self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        check_single_proto(&properties)?;
        Ok(Expr::new(
            ExprKind::Object(properties),
            open.span.to(close.span),
        ))
    }

    /// `PropertyDefinitionList` (§13.2.5), and the optional trailing comma.
    fn parse_property_definitions(&mut self) -> Result<Box<[PropertyDefinition]>, ParseError> {
        let mut properties = Vec::new();
        while self.current.kind != TokenKind::RBrace {
            // `... AssignmentExpression` — a `PropertyDefinition` alternative, so it stands where
            // any other property would rather than only at one end.
            if self.current.kind == TokenKind::DotDotDot {
                self.advance(Goal::RegExp)?;
                properties.push(PropertyDefinition::Spread(
                    self.parse_assignment(AllowIn::Yes)?,
                ));
            } else {
                properties.push(self.parse_property_definition()?);
            }
            if self.current.kind != TokenKind::Comma {
                break;
            }
            // A separator. Unlike an array's, it may not stand for a missing entry — the next
            // iteration will find either a property or the closing brace, and a second comma is
            // neither.
            self.advance(Goal::RegExp)?;
        }
        Ok(properties.into_boxed_slice())
    }

    /// One `PropertyDefinition` that is not a spread (§13.2.5).
    fn parse_property_definition(&mut self) -> Result<PropertyDefinition, ParseError> {
        let token = self.current;
        let key = self.parse_property_key()?;
        if self.current.kind == TokenKind::Colon {
            self.advance(Goal::RegExp)?;
            let value = self.parse_assignment(AllowIn::Yes)?;
            return Ok(PropertyDefinition::KeyValue { key, value });
        }
        // Everything else is either shorthand or a production that needs functions. Shorthand is
        // `IdentifierReference`, which is narrower than the `IdentifierName` a key may be: `{if}`
        // has no derivation where `{if: 1}` does, because a reserved word is a name and not a
        // reference.
        let PropertyKey::Identifier(name) = key else {
            return Err(self.unexpected("`:`"));
        };
        if !super::is_identifier_token(token.kind) {
            return Err(ParseError {
                kind: ParseErrorKind::Unexpected {
                    expected: "`:`",
                    found: self.current.kind,
                },
                span: token.span,
            });
        }
        // `CoverInitializedName`. Kept rather than refused, because the cover grammar needs it:
        // `({a = 1} = b)` is a pattern, and the `=` that says so is several tokens away. §13.2.5.1
        // makes it a Syntax Error where the literal stays a literal, and the record left here is
        // what becomes that error when no refinement claims it.
        if self.current.kind == TokenKind::Eq {
            self.advance(Goal::RegExp)?;
            let default = self.parse_assignment(AllowIn::Yes)?;
            self.cover_initialized_name.get_or_insert(token.span);
            return Ok(PropertyDefinition::ShorthandWithDefault {
                name,
                default: Box::new(default),
                span: token.span,
            });
        }
        Ok(PropertyDefinition::Shorthand {
            name,
            span: token.span,
        })
    }

    /// `PropertyName : LiteralPropertyName | ComputedPropertyName` (§13.2.5).
    pub(super) fn parse_property_key(&mut self) -> Result<PropertyKey, ParseError> {
        let token = self.current;
        match token.kind {
            // `[ AssignmentExpression ]`, whose value is not known until it runs — so this is the
            // one key the `__proto__` rule cannot be about.
            TokenKind::LBracket => {
                self.advance(Goal::RegExp)?;
                self.enter()?;
                let key = self.parse_assignment(AllowIn::Yes);
                self.leave();
                let key = key?;
                self.eat(TokenKind::RBracket, Goal::Div, "`]`")?;
                Ok(PropertyKey::Computed(Box::new(key)))
            }
            TokenKind::String { .. } => {
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok(PropertyKey::String(value.into_boxed_slice()))
            }
            TokenKind::Number { .. } => {
                self.advance(Goal::Div)?;
                let value = numeric_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok(PropertyKey::Number(value))
            }
            // `LiteralPropertyName : IdentifierName`, which is every name including the reserved
            // words: `{if: 1}` and `{class: 1}` are ordinary properties.
            TokenKind::Identifier { .. } | TokenKind::Keyword(_) => {
                self.advance(Goal::Div)?;
                let name = identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok(PropertyKey::Identifier(name.into_owned().into_boxed_str()))
            }
            _ => Err(self.unexpected("a property name")),
        }
    }
}

/// §13.2.5.1: at most one `PropertyName : AssignmentExpression` may be named `__proto__`.
///
/// The rule counts entries from that production alone, so a computed key and a shorthand are both
/// invisible to it — and a numeric key cannot spell the name at all, `PropName` of a
/// `NumericLiteral` being the number written out.
fn check_single_proto(properties: &[PropertyDefinition]) -> Result<(), ParseError> {
    let mut seen: Option<Span> = None;
    for property in properties {
        let PropertyDefinition::KeyValue { key, value } = property else {
            continue;
        };
        if !key.is_proto() {
            continue;
        }
        if seen.is_some() {
            return Err(ParseError {
                kind: ParseErrorKind::DuplicateProto,
                span: value.span,
            });
        }
        seen = Some(value.span);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// The kind of error `source` fails with, as an expression.
    fn error_kind(source: &str) -> ParseErrorKind {
        match parse_expression(source) {
            Err(err) => err.kind,
            Ok(expr) => panic!("{source:?} should not parse, got {expr:?}"), // a test about an error needs one
        }
    }

    #[test]
    fn a_property_list_takes_a_trailing_comma_and_never_an_empty_slot() {
        assert_eq!(shape("({})"), "{}");
        assert_eq!(shape("({a: 1})"), "{(a 1)}");
        assert_eq!(shape("({a: 1, b: 2})"), "{(a 1) (b 2)}");
        assert_eq!(shape("({a: 1, })"), "{(a 1)}");
        assert_eq!(shape("({a: 1, b: 2, })"), "{(a 1) (b 2)}");
        // The contrast with an array literal, which is the thing to remember: `ElementList` has
        // `Elision` in it and `PropertyDefinitionList` has nothing like it.
        assert!(parse_expression("({, })").is_err());
        assert!(parse_expression("({a: 1, , })").is_err());
        assert!(parse_expression("({a: 1 b: 2})").is_err());
        assert!(parse_expression("({a:: 1})").is_err());
        assert_eq!(shape("[, ]"), "[<hole>]", "…where an array counts one");
    }

    #[test]
    fn a_key_may_be_any_identifier_name_and_shorthand_may_not() {
        assert_eq!(shape("({a: 1})"), "{(a 1)}");
        assert_eq!(shape("({'a': 1})"), "{(s\"a\" 1)}");
        assert_eq!(shape("({1: 2})"), "{(n1 2)}");
        assert_eq!(shape("({[x]: 1})"), "{([x] 1)}");
        assert_eq!(shape("({[x + y]: 1})"), "{([(+ x y)] 1)}");
        // `LiteralPropertyName : IdentifierName`, and an IdentifierName includes every reserved
        // word — so these are ordinary properties.
        assert_eq!(shape("({if: 1})"), "{(if 1)}");
        assert_eq!(shape("({class: 1, new: 2})"), "{(class 1) (new 2)}");
        assert_eq!(
            shape("({get: 1, set: 2, async: 3})"),
            "{(get 1) (set 2) (async 3)}"
        );
        // Shorthand is an `IdentifierReference`, which is narrower: a reserved word is a name but
        // not a reference, so it can be a key and not a shorthand.
        assert_eq!(shape("({a})"), "{a}");
        assert_eq!(shape("({a, b})"), "{a b}");
        assert_eq!(shape("({a, })"), "{a}");
        assert_eq!(shape("({a, b: 2})"), "{a (b 2)}");
        assert!(parse_expression("({if})").is_err());
        assert!(parse_expression("({1})").is_err());
        assert!(parse_expression("({'a'})").is_err());
        // A numeric key keeps its value rather than a spelling: `PropName` of a `NumericLiteral`
        // is `ToString` of the number, which is an abstract operation this engine does not have —
        // so the number is what is stored, and `{1e3: 0}` will be the property `"1000"`.
        assert_eq!(shape("({1e3: 0})"), "{(n1000 0)}");
        assert_eq!(shape("({1.0: 0})"), "{(n1 0)}");
    }

    #[test]
    fn a_spread_stands_where_any_other_property_would() {
        assert_eq!(shape("({...a})"), "{(... a)}");
        assert_eq!(shape("({...a, b: 1})"), "{(... a) (b 1)}");
        assert_eq!(shape("({a: 1, ...b})"), "{(a 1) (... b)}");
        assert_eq!(shape("({...a, ...b})"), "{(... a) (... b)}");
        assert_eq!(shape("({...a, })"), "{(... a)}");
        assert!(parse_expression("({...})").is_err());
    }

    #[test]
    fn proto_may_be_written_twice_unless_both_are_the_same_production() {
        // §13.2.5.1 counts entries from `PropertyName : AssignmentExpression` and nothing else.
        assert_eq!(
            error_kind("({__proto__: 1, __proto__: 2})"),
            ParseErrorKind::DuplicateProto
        );
        assert_eq!(
            error_kind("({'__proto__': 1, __proto__: 2})"),
            ParseErrorKind::DuplicateProto,
            "a StringLiteral is a PropertyName too"
        );
        assert_eq!(
            error_kind("({__proto__: 1, b: 2, __proto__: 3})"),
            ParseErrorKind::DuplicateProto
        );
        // …and the other productions are invisible to it, which is most of the rule.
        assert!(parse_expression("({__proto__: 1})").is_ok());
        assert!(parse_expression("({__proto__: 1, ['__proto__']: 2})").is_ok());
        assert!(parse_expression("({__proto__: 1, __proto__})").is_ok());
        assert!(parse_expression("({__proto__, __proto__})").is_ok());
        assert!(parse_expression("({__proto__: 1, ...__proto__})").is_ok());
        // A name that merely looks similar is a different name.
        assert!(parse_expression("({_proto_: 1, _proto_: 2})").is_ok());
        assert!(
            parse_expression("({a: 1, a: 2})").is_ok(),
            "only `__proto__` is special"
        );
        // The caret goes on the second value, which is the one that would have been ignored.
        let source = "({__proto__: 1, __proto__: 22})";
        match parse_expression(source) {
            Err(err) => assert_eq!(err.span.slice(source), Some("22")),
            Ok(expr) => panic!("{expr:?}"), // the test is about the error
        }
    }

    #[test]
    fn an_object_is_a_value_here_and_a_home_for_no_methods() {
        // `CoverInitializedName` is always a Syntax Error in an object literal (§13.2.5.1). It
        // exists so the cover grammar can reach `({a = 1} = b)` — which is a pattern, and is
        // refused here for want of the refinement rather than for want of this rule.
        assert_eq!(
            error_kind("({a = 1})"),
            ParseErrorKind::ShorthandPropertyWithInitializer
        );
        assert_eq!(
            error_kind("[{a = 1}]"),
            ParseErrorKind::ShorthandPropertyWithInitializer,
            "an open literal defers the question; a closed one settles it"
        );
        // …and every one of those is a pattern the moment an `=` follows it, including the
        // `{a = 1}` above — which is the whole reason the literal parser keeps it.
        for source in ["({a} = b);", "({a: b} = c);", "({} = b);", "({a = 1} = b);"] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // `MethodDefinition` is the remaining `PropertyDefinition` alternative and needs
        // functions.
        for source in [
            "({a() {}})",
            "({get a() {}})",
            "({set a(v) {}})",
            "({*a() {}})",
        ] {
            assert!(parse_expression(source).is_err(), "{source:?}");
        }
        // …while an object as a value is unaffected, which is what this slice adds.
        assert!(parse_script("a = {b: 1};").is_ok());
        assert!(parse_script("f({a: 1}, {b: 2});").is_ok());
        assert!(parse_script("({a: 1}).a;").is_ok());
        // §14.5's restriction still holds: a `{` where a statement may begin is a block.
        assert_eq!(statements_of("{}"), ["{}"]);
        assert_eq!(statements_of("({})"), ["{}"]);
    }

    /// The rendered statements of `source`.
    fn statements_of(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // needs the tree
        script.body.iter().map(render_statement).collect()
    }

    #[test]
    fn no_object_however_truncated_can_panic() {
        let cases = [
            "({".to_string(),
            "({a".to_string(),
            "({a:".to_string(),
            "({a: 1,".to_string(),
            "({[".to_string(),
            "({...".to_string(),
            "({a: ".repeat(10_000),
            "({".repeat(10_000),
            format!("({{{}}})", "a: 1,".repeat(100_000)),
            format!("({{{}}})", "__proto__: 1,".repeat(1_000)),
        ];
        for source in &cases {
            let _ = parse_expression(source);
        }
        // `({({(` is not nesting — the inner `(` stands where a property name should, and that
        // is what it is told. Nesting needs a key to nest under.
        assert_eq!(
            error_kind(&"({".repeat(10_000)),
            ParseErrorKind::Unexpected {
                expected: "a property name",
                found: crate::lexer::TokenKind::LParen,
            }
        );
        assert_eq!(
            error_kind(&"({a: ".repeat(10_000)),
            ParseErrorKind::TooDeeplyNested
        );
        assert_eq!(
            error_kind(&format!("({{{}}})", "__proto__: 1,".repeat(1_000))),
            ParseErrorKind::DuplicateProto
        );
    }
}
