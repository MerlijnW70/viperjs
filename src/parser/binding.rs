//! Binding patterns (ECMAScript §14.3.3).
//!
//! No cover grammar here, and that is the whole difference from [`super::pattern`]. A binding
//! position expects a `BindingPattern`, so a `[` there can be nothing else and is parsed as one
//! directly — where `[a] = b` had to be read as a literal first, because until the `=` arrives
//! nothing says it is not one.
//!
//! What that buys is that the errors are immediate and say what they mean: `let [a.b] = c` fails
//! at the `.`, on the grounds that a binding is a name being created and `a.b` is not a name.
//!
//! # What the two rest positions may take
//!
//! `BindingRestElement : ... BindingIdentifier | ... BindingPattern` and
//! `BindingRestProperty : ... BindingIdentifier`. So `let [...[a]] = b` binds and
//! `let {...[a]} = b` does not — the same asymmetry the assignment patterns have, because the
//! remaining properties of an object are an object and there is nothing there to take apart.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    ArrayBindingPattern, Binding, BindingElement, BindingName, BindingPattern, BindingProperty,
    ObjectBindingPattern,
};
use crate::lexer::{Goal, TokenKind};

impl Parser<'_> {
    /// Whether a `BindingPattern` begins here.
    pub(super) fn at_binding_pattern(&self) -> bool {
        matches!(self.current.kind, TokenKind::LBracket | TokenKind::LBrace)
    }

    /// `BindingIdentifier | BindingPattern` (§14.3.3) — anything that can be bound.
    pub(super) fn parse_binding(&mut self) -> Result<Binding, ParseError> {
        match self.current.kind {
            TokenKind::LBracket => {
                self.enter()?;
                let pattern = self.parse_array_binding_pattern();
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Array(pattern?)))
            }
            TokenKind::LBrace => {
                self.enter()?;
                let pattern = self.parse_object_binding_pattern();
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Object(pattern?)))
            }
            _ => Ok(Binding::Identifier(self.parse_binding_name()?)),
        }
    }

    /// `BindingIdentifier` (§13.1), as a name and where it was written.
    pub(super) fn parse_binding_name(&mut self) -> Result<BindingName, ParseError> {
        let (name, span) = self.parse_binding_identifier()?;
        Ok(BindingName { name, span })
    }

    /// `ArrayBindingPattern` (§14.3.3), with the cursor on the `[`.
    fn parse_array_binding_pattern(&mut self) -> Result<ArrayBindingPattern, ParseError> {
        let open = self.advance(Goal::RegExp)?;
        let mut elements: Vec<Option<BindingElement>> = Vec::new();
        let mut rest: Option<Box<Binding>> = None;
        while self.current.kind != TokenKind::RBracket {
            // An `Elision` — a comma with nothing before it in its slot, exactly as in a literal.
            if self.current.kind == TokenKind::Comma {
                elements.push(None);
                self.advance(Goal::RegExp)?;
                continue;
            }
            if self.current.kind == TokenKind::DotDotDot {
                self.advance(Goal::RegExp)?;
                rest = Some(Box::new(self.parse_binding()?));
            } else {
                elements.push(Some(self.parse_binding_element()?));
            }
            if self.current.kind != TokenKind::Comma {
                break;
            }
            // Nothing follows a rest element — not another element, and not even a comma. Asking
            // at the comma catches both at once, and needs no record: a binding pattern is read
            // as itself, so the comma is still in hand when the question comes up. The literal
            // parser has to write this down instead, and [`super::pattern`] says why.
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span: self.current.span,
                });
            }
            self.advance(Goal::RegExp)?;
        }
        let close = self.eat(TokenKind::RBracket, Goal::Div, "`]`")?;
        Ok(ArrayBindingPattern {
            elements: elements.into_boxed_slice(),
            rest,
            span: open.span.to(close.span),
        })
    }

    /// `ObjectBindingPattern` (§14.3.3), with the cursor on the `{`.
    fn parse_object_binding_pattern(&mut self) -> Result<ObjectBindingPattern, ParseError> {
        let open = self.advance(Goal::RegExp)?;
        let mut properties: Vec<BindingProperty> = Vec::new();
        let mut rest: Option<BindingName> = None;
        while self.current.kind != TokenKind::RBrace {
            if self.current.kind == TokenKind::DotDotDot {
                self.advance(Goal::RegExp)?;
                // `BindingRestProperty : ... BindingIdentifier`. A pattern here would be asking to
                // take apart the properties that are left over, which are an object and not a
                // list — so `let {...[a]} = b` has no derivation.
                if self.at_binding_pattern() {
                    return Err(ParseError {
                        kind: ParseErrorKind::RestTargetMayNotBePattern,
                        span: self.current.span,
                    });
                }
                rest = Some(self.parse_binding_name()?);
            } else {
                properties.push(self.parse_binding_property()?);
            }
            if self.current.kind != TokenKind::Comma {
                break;
            }
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span: self.current.span,
                });
            }
            self.advance(Goal::RegExp)?;
        }
        let close = self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        Ok(ObjectBindingPattern {
            properties: properties.into_boxed_slice(),
            rest,
            span: open.span.to(close.span),
        })
    }

    /// `BindingProperty : SingleNameBinding | PropertyName : BindingElement` (§14.3.3).
    fn parse_binding_property(&mut self) -> Result<BindingProperty, ParseError> {
        let token = self.current;
        let key = self.parse_property_key()?;
        if self.current.kind == TokenKind::Colon {
            self.advance(Goal::RegExp)?;
            return Ok(BindingProperty {
                key,
                value: self.parse_binding_element()?,
            });
        }
        // Shorthand. `SingleNameBinding` is a `BindingIdentifier`, narrower than the
        // `IdentifierName` the key form takes — so `{if: a}` binds and `{if}` does not.
        let crate::ast::PropertyKey::Identifier(name) = &key else {
            return Err(self.unexpected("`:`"));
        };
        if !super::is_identifier_token(token.kind) {
            return Err(ParseError {
                kind: ParseErrorKind::Unexpected {
                    expected: "`:`",
                    found: token.kind,
                },
                span: token.span,
            });
        }
        let target = Binding::Identifier(BindingName {
            name: name.clone(),
            span: token.span,
        });
        Ok(BindingProperty {
            key,
            value: BindingElement {
                default: self.parse_binding_default()?,
                target,
            },
        })
    }

    /// `BindingElement : SingleNameBinding | BindingPattern Initializer_opt` (§14.3.3).
    pub(super) fn parse_binding_element(&mut self) -> Result<BindingElement, ParseError> {
        let target = self.parse_binding()?;
        Ok(BindingElement {
            default: self.parse_binding_default()?,
            target,
        })
    }

    /// The `Initializer_opt` of a binding element — the value to use when one is `undefined`.
    fn parse_binding_default(&mut self) -> Result<Option<Box<crate::ast::Expr>>, ParseError> {
        if self.current.kind != TokenKind::Eq {
            return Ok(None);
        }
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // `Initializer[+In]` — a bracket starts afresh, so a `for` head does not reach in here.
        let default = self.parse_assignment(AllowIn::Yes);
        self.leave();
        Ok(Some(Box::new(default?)))
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    #[test]
    fn all_three_keywords_take_a_pattern_where_they_take_a_name() {
        assert_eq!(statements("let [a] = b;"), ["(let [a]=b)"]);
        assert_eq!(statements("var [a] = b;"), ["(var [a]=b)"]);
        assert_eq!(statements("const [a] = b;"), ["(const [a]=b)"]);
        assert_eq!(statements("let {a} = b;"), ["(let {(a a)}=b)"]);
        assert_eq!(statements("let [a, b] = c;"), ["(let [a b]=c)"]);
        assert_eq!(statements("let [] = b;"), ["(let []=b)"]);
        assert_eq!(statements("let {} = b;"), ["(let {}=b)"]);
        // A binding list mixes them freely, each declarator being its own binding.
        assert_eq!(statements("let a, [b] = c;"), ["(let a [b]=c)"]);
        assert_eq!(statements("let [a] = b, [c] = d;"), ["(let [a]=b [c]=d)"]);
        // Elisions, exactly as in a literal — a position deliberately skipped, binding nothing.
        assert_eq!(statements("let [, a] = b;"), ["(let [<hole> a]=b)"]);
        assert_eq!(statements("let [a, ] = b;"), ["(let [a]=b)"]);
        // …and patterns nest.
        assert_eq!(statements("let {a: [b]} = c;"), ["(let {(a [b])}=c)"]);
        assert_eq!(statements("let [[a]] = b;"), ["(let [[a]]=b)"]);
    }

    #[test]
    fn a_pattern_always_takes_an_initialiser_and_a_name_does_not_always() {
        // `VariableDeclaration : BindingIdentifier Initializer_opt | BindingPattern Initializer`
        // — the `_opt` is on the first alternative only, so a pattern with nothing to take apart
        // has no derivation. Unlike the `const` rule, this holds for all three keywords.
        assert_eq!(
            script_error("let [a];").kind,
            ParseErrorKind::PatternWithoutInitializer
        );
        assert_eq!(
            script_error("var [a];").kind,
            ParseErrorKind::PatternWithoutInitializer
        );
        assert_eq!(
            script_error("var {a};").kind,
            ParseErrorKind::PatternWithoutInitializer
        );
        assert_eq!(
            script_error("const [a];").kind,
            ParseErrorKind::PatternWithoutInitializer
        );
        // …while a name needs one only from `const`.
        assert!(parse_script("var a;").is_ok());
        assert!(parse_script("let a;").is_ok());
        assert_eq!(
            script_error("const a;").kind,
            ParseErrorKind::ConstWithoutInitializer
        );
        // A `ForBinding` takes no initialiser at all, and is a pattern just as happily.
        assert!(parse_script("for (let [a] of b);").is_ok());
        assert!(parse_script("for (var {a} in b);").is_ok());
        assert!(parse_script("for (const [a] of b);").is_ok());
        assert_eq!(
            script_error("for (let [a] = 1 of b);").kind,
            ParseErrorKind::ForInOfBindingHasInitializer
        );
        // …and in the three-part form it is a declaration again, so it does need one.
        assert!(parse_script("for (let [a] = b;;);").is_ok());
        assert_eq!(
            script_error("for (let [a];;);").kind,
            ParseErrorKind::PatternWithoutInitializer
        );
    }

    #[test]
    fn a_binding_target_is_a_name_being_created_and_never_a_place_to_put_one() {
        // The difference from an assignment pattern, and the whole reason these are two types:
        // `[a.b] = c` puts a value somewhere, and there is no such thing as declaring `a.b`.
        assert!(parse_script("[a.b] = c;").is_ok());
        assert!(parse_script("let [a.b] = c;").is_err());
        assert!(parse_script("let {a: b.c} = d;").is_err());
        assert!(parse_script("let [1] = b;").is_err());
        assert!(parse_script("let {a: 1} = b;").is_err());
        assert!(parse_script("let [f()] = b;").is_err());
        // A key is an `IdentifierName` and shorthand is a `BindingIdentifier`, the same asymmetry
        // an object literal has.
        assert_eq!(statements("let {if: a} = b;"), ["(let {(if a)}=b)"]);
        assert!(parse_script("let {if} = b;").is_err());
        assert_eq!(statements("let {[x]: a} = b;"), ["(let {([x] a)}=b)"]);
        assert_eq!(statements("let {1: a} = b;"), ["(let {(n1 a)}=b)"]);
    }

    #[test]
    fn defaults_and_rest_elements_are_where_the_two_rests_part_company() {
        assert_eq!(statements("let [a = 1] = b;"), ["(let [(= a 1)]=b)"]);
        assert_eq!(statements("let {a = 1} = b;"), ["(let {(a (= a 1))}=b)"]);
        assert_eq!(statements("let {a: b = 1} = c;"), ["(let {(a (= b 1))}=c)"]);
        assert_eq!(statements("let [...a] = b;"), ["(let [(... a)]=b)"]);
        assert_eq!(statements("let [a, ...b] = c;"), ["(let [a (... b)]=c)"]);
        assert_eq!(statements("let {...a} = b;"), ["(let {(... a)}=b)"]);
        assert_eq!(
            statements("let {a, ...b} = c;"),
            ["(let {(a a) (... b)}=c)"]
        );
        // `BindingRestElement : ... BindingIdentifier | ... BindingPattern`, and
        // `BindingRestProperty : ... BindingIdentifier`. The remaining properties of an object are
        // an object, and there is nothing there to take apart.
        assert_eq!(statements("let [...[a]] = b;"), ["(let [(... [a])]=b)"]);
        assert_eq!(
            script_error("let {...[a]} = b;").kind,
            ParseErrorKind::RestTargetMayNotBePattern
        );
        assert!(parse_script("let {...a.b} = c;").is_err());
        // Nothing follows a rest element — and here the comma is still in hand when the question
        // is asked, so no record is needed and none is kept.
        assert_eq!(
            script_error("let [...a, b] = c;").kind,
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            script_error("let [...a, ] = b;").kind,
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            script_error("let {a, ...b, c} = d;").kind,
            ParseErrorKind::RestElementMustBeLast
        );
    }

    #[test]
    fn every_early_error_about_bound_names_now_reads_the_whole_pattern() {
        // §14.3.1.1: the BoundNames of a lexical BindingList may not repeat, nor contain `let`.
        // Both were about one name per declarator and are about a list of them now.
        assert_eq!(
            script_error("let [a, a] = b;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("let {a, a} = b;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("let [a] = b, [a] = c;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("let [let] = b;").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        // …and `var` is exempt from both, exactly as it always was.
        assert!(parse_script("var [a, a] = b;").is_ok());
        assert!(parse_script("var [let] = b;").is_ok());
        // §14.2.1 and §16.1.1 read them too, so a pattern name collides with a `var` the same way
        // a plain one does.
        assert_eq!(
            script_error("let [a] = b; var a;").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("let {x: a} = b; { var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        // …including §14.7.4.1, about a `for` header against its body.
        assert_eq!(
            script_error("for (let [a] = b;;) { var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("for (let {a} of b) { var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
    }

    #[test]
    fn no_binding_pattern_however_odd_can_panic() {
        let cases = [
            "let [".to_string(),
            "let [a".to_string(),
            "let {".to_string(),
            "let {a".to_string(),
            "let {a:".to_string(),
            "let [...".to_string(),
            "let {...".to_string(),
            format!("let {}a{} = b;", "[".repeat(10_000), "]".repeat(10_000)),
            format!("let [{}] = b;", "a, ".repeat(100_000)),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A pattern nests, so it is bounded by the cap; a long flat one is a loop.
        assert_eq!(
            script_error(&format!(
                "let {}a{} = b;",
                "[".repeat(10_000),
                "]".repeat(10_000)
            ))
            .kind,
            ParseErrorKind::TooDeeplyNested
        );
        assert!(parse_script(&format!("let [{}] = b;", "a, ".repeat(10_000))).is_err());
    }
}
