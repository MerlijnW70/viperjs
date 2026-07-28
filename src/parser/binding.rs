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
//! # The other way a `Binding` comes to be
//!
//! Arrow parameters are the exception, and they are here rather than in [`super::arrow`] because
//! what they produce is governed by this file's grammar and not by that one's. `([a]) => b` was
//! read as an array *literal* — the `=>` had not arrived yet — so the second half of this module
//! turns a finished `Expr` back into the `Binding` it was covering, refusing exactly what parsing
//! one directly would have refused. `([a.b]) => c` has no derivation for the same reason
//! `let [a.b] = c` has none.
//!
//! The mirror of it is in [`super::pattern`], which refines an `Expr` into a `Pattern` instead —
//! the assignment grammar, which is wider, because `[a.b] = c` assigns to a property that already
//! exists where a binding creates a name.
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
    ArrayBindingPattern, AssignmentTarget, Binding, BindingElement, BindingName, BindingPattern,
    BindingProperty, Expr, ExprKind, ObjectBindingPattern, Pattern, PropertyDefinition,
};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;

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
        if !self.is_identifier_token(token.kind) {
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

    /// One parameter, refined from the expression that covered it.
    pub(super) fn refine_to_binding_element(
        &mut self,
        expr: Expr,
    ) -> Result<BindingElement, ParseError> {
        let mut expr = expr;
        // A `match` and not a `let … else`: [`Expr`] has a `Drop`, so the node cannot be taken
        // apart field by field — and when this turns out not to be an assignment the node is
        // still wanted whole, so the kind goes back where it came from.
        let (operator, target, value) = match expr.take_kind() {
            ExprKind::Assignment {
                operator,
                target,
                value,
            } => (operator, target, value),
            other => {
                expr.kind = other;
                return Ok(BindingElement {
                    target: self.refine_to_binding(expr)?,
                    default: None,
                });
            }
        };
        if operator != crate::ast::AssignmentOperator::Assign {
            return Err(ParseError {
                kind: ParseErrorKind::InvalidArrowParameter,
                span: expr.span,
            });
        }
        let target = self.target_as_binding(*target)?;
        Ok(BindingElement {
            target,
            default: Some(value),
        })
    }

    /// An expression, refined into the name or pattern it covered.
    pub(super) fn refine_to_binding(&mut self, expr: Expr) -> Result<Binding, ParseError> {
        // The mirror of what [`Parser::refine_to_pattern`] does, and for the same reason: a
        // `{a = 1}` in here has found the thing that makes it legal. `CoverInitializedName`
        // refines into `SingleNameBinding : BindingIdentifier Initializer` exactly as it refines
        // into `AssignmentProperty : IdentifierReference Initializer_opt`, so `({a = 1}) => b` is
        // as ordinary as `({a = 1} = b)`. Only a literal that reaches the end of an
        // `AssignmentExpression` unrefined is the Syntax Error §13.2.5.1 describes — which is why
        // `f({a = 1})` and `async({a = 1})` still are one.
        self.cover_initialized_name = None;
        self.duplicate_proto = None;
        let span = expr.span;
        match expr.into_kind() {
            ExprKind::Identifier(name) => Ok(Binding::Identifier(crate::ast::BindingName {
                name: name.into_boxed_str(),
                span,
            })),
            ExprKind::Array(elements) => {
                self.enter()?;
                let refined = self.refine_array_binding(elements, span);
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Array(refined?)))
            }
            ExprKind::Object(properties) => {
                self.enter()?;
                let refined = self.refine_object_binding(properties, span);
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Object(refined?)))
            }
            // A binding creates a name, so `a.b` is refused where an assignment pattern takes it.
            _ => Err(ParseError {
                kind: ParseErrorKind::InvalidArrowParameter,
                span,
            }),
        }
    }

    /// A `DestructuringAssignmentTarget`, as the `Binding` the `=>` says it was.
    ///
    /// The one place two refinements meet. `({a} = {}) => b` reads `{a} = {}` as an
    /// `AssignmentExpression`, so the `=` refines the literal into an *assignment* pattern before
    /// the `=>` has been seen — and then the `=>` says it was a parameter with a default all
    /// along, whose target is a *binding* pattern. The literal is gone by then, so the shapes are
    /// converted rather than the source read a third time.
    ///
    /// The two grammars are not the same, which is the whole reason this is a conversion and not
    /// a cast: `[a.b] = c` is a perfectly good assignment pattern and `([a.b]) => c` has no
    /// derivation. Every target goes through [`Parser::refine_to_binding`], which is what refuses
    /// the ones a binding may not have.
    fn target_as_binding(&mut self, target: AssignmentTarget) -> Result<Binding, ParseError> {
        match target {
            AssignmentTarget::Simple(expr) => self.refine_to_binding(expr),
            AssignmentTarget::Pattern(Pattern::Array(pattern)) => {
                self.enter()?;
                let refined = self.array_pattern_as_binding(pattern);
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Array(refined?)))
            }
            AssignmentTarget::Pattern(Pattern::Object(pattern)) => {
                self.enter()?;
                let refined = self.object_pattern_as_binding(pattern);
                self.leave();
                Ok(Binding::Pattern(BindingPattern::Object(refined?)))
            }
        }
    }

    /// An `ArrayAssignmentPattern`, as the `ArrayBindingPattern` it turns out to have been.
    ///
    /// No rest-element check: the assignment refinement already made one, and a rest that was
    /// last there is last here — the two grammars agree about that much.
    fn array_pattern_as_binding(
        &mut self,
        pattern: crate::ast::ArrayPattern,
    ) -> Result<crate::ast::ArrayBindingPattern, ParseError> {
        let mut elements = Vec::with_capacity(pattern.elements.len());
        for element in Vec::from(pattern.elements) {
            elements.push(match element {
                None => None,
                Some(element) => Some(self.pattern_element_as_binding(element)?),
            });
        }
        let rest = match pattern.rest {
            Some(rest) => Some(Box::new(self.target_as_binding(*rest)?)),
            None => None,
        };
        Ok(crate::ast::ArrayBindingPattern {
            elements: elements.into_boxed_slice(),
            rest,
            span: pattern.span,
        })
    }

    /// An `ObjectAssignmentPattern`, as the `ObjectBindingPattern` it turns out to have been.
    fn object_pattern_as_binding(
        &mut self,
        pattern: crate::ast::ObjectPattern,
    ) -> Result<crate::ast::ObjectBindingPattern, ParseError> {
        let span = pattern.span;
        let mut properties = Vec::with_capacity(pattern.properties.len());
        for property in Vec::from(pattern.properties) {
            properties.push(BindingProperty {
                key: property.key,
                value: self.pattern_element_as_binding(property.value)?,
            });
        }
        // `BindingRestProperty : ... BindingIdentifier`, where an `AssignmentRestProperty` takes
        // any simple target — so `({...a.b} = c)` is legal and `({...a.b}) => c` is not.
        let rest = match pattern.rest {
            Some(rest) => {
                let Binding::Identifier(name) = self.refine_to_binding(*rest)? else {
                    return Err(ParseError {
                        kind: ParseErrorKind::RestTargetMayNotBePattern,
                        span,
                    });
                };
                Some(name)
            }
            None => None,
        };
        Ok(crate::ast::ObjectBindingPattern {
            properties: properties.into_boxed_slice(),
            rest,
            span,
        })
    }

    /// One `AssignmentElement`, as the `BindingElement` it turns out to have been.
    fn pattern_element_as_binding(
        &mut self,
        element: crate::ast::PatternElement,
    ) -> Result<BindingElement, ParseError> {
        Ok(BindingElement {
            target: self.target_as_binding(element.target)?,
            default: element.default,
        })
    }

    /// An array literal, refined into an `ArrayBindingPattern`.
    fn refine_array_binding(
        &mut self,
        elements: Box<[crate::ast::ArrayElement]>,
        span: Span,
    ) -> Result<crate::ast::ArrayBindingPattern, ParseError> {
        let mut refined = Vec::with_capacity(elements.len());
        let mut rest = None;
        for element in Vec::from(elements) {
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span,
                });
            }
            match element {
                crate::ast::ArrayElement::Hole => refined.push(None),
                crate::ast::ArrayElement::Value(value) => {
                    refined.push(Some(self.refine_to_binding_element(value)?));
                }
                crate::ast::ArrayElement::Spread {
                    value: target,
                    followed_by_comma,
                } => {
                    // `BindingRestElement` is last with nothing after it, exactly as an
                    // `AssignmentRestElement` is — so `([...a,]) => b` has no derivation any more
                    // than `[...a,] = b` does.
                    if followed_by_comma {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestElementMustBeLast,
                            span: target.span,
                        });
                    }
                    rest = Some(Box::new(self.refine_to_binding(target)?));
                }
            }
        }
        Ok(crate::ast::ArrayBindingPattern {
            elements: refined.into_boxed_slice(),
            rest,
            span,
        })
    }

    /// An object literal, refined into an `ObjectBindingPattern`.
    fn refine_object_binding(
        &mut self,
        properties: Box<[PropertyDefinition]>,
        span: Span,
    ) -> Result<crate::ast::ObjectBindingPattern, ParseError> {
        let mut refined = Vec::with_capacity(properties.len());
        let mut rest = None;
        for property in Vec::from(properties) {
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span,
                });
            }
            match property {
                PropertyDefinition::KeyValue { key, value } => refined.push(BindingProperty {
                    key,
                    value: self.refine_to_binding_element(value)?,
                }),
                PropertyDefinition::Shorthand { name, span } => refined.push(BindingProperty {
                    key: crate::ast::PropertyKey::Identifier(name.clone()),
                    value: BindingElement {
                        target: Binding::Identifier(crate::ast::BindingName { name, span }),
                        default: None,
                    },
                }),
                PropertyDefinition::ShorthandWithDefault {
                    name,
                    default,
                    span,
                } => refined.push(BindingProperty {
                    key: crate::ast::PropertyKey::Identifier(name.clone()),
                    value: BindingElement {
                        target: Binding::Identifier(crate::ast::BindingName { name, span }),
                        default: Some(default),
                    },
                }),
                // `BindingRestProperty : ... BindingIdentifier`, as everywhere else.
                PropertyDefinition::Spread {
                    value: target,
                    followed_by_comma,
                } => {
                    if followed_by_comma {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestElementMustBeLast,
                            span: target.span,
                        });
                    }
                    let Binding::Identifier(name) = self.refine_to_binding(target)? else {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestTargetMayNotBePattern,
                            span,
                        });
                    };
                    rest = Some(name);
                }
                PropertyDefinition::Method { function, .. } => {
                    return Err(ParseError {
                        kind: ParseErrorKind::InvalidArrowParameter,
                        span: function.span,
                    });
                }
            }
        }
        Ok(crate::ast::ObjectBindingPattern {
            properties: refined.into_boxed_slice(),
            rest,
            span,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
    }

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

    #[test]
    fn a_literal_refined_into_a_binding_has_found_what_makes_its_shorthand_legal() {
        // `CoverInitializedName` refines into `SingleNameBinding : BindingIdentifier Initializer`
        // exactly as it refines into `AssignmentProperty : IdentifierReference Initializer_opt`,
        // so a `{a = 1}` is as ordinary in arrow parameters as it is on the left of an `=`.
        assert_eq!(shape("({a = 1}) => b"), "(=> [{(a (= a 1))}] b)");
        for source in [
            "({a = 1}) => b;",
            "({a = 1, b}) => c;",
            "({a: {b = 1}}) => c;",
            "({a = 1}, b) => c;",
            "([{a = 1}]) => b;",
            "async ({a = 1}) => b;",
            "({a = 1} = {}) => b;",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // …and a literal that reaches the end of an `AssignmentExpression` unrefined is still the
        // Syntax Error §13.2.5.1 describes. The parentheses of a call cannot rescue it, and
        // neither can the ones of a group that turned out not to be parameters.
        for source in [
            "({a = 1});",
            "f({a = 1});",
            "async({a = 1});",
            "[{a = 1}];",
            "({a = 1}) + 1;",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::ShorthandPropertyWithInitializer,
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_parameters_default_may_have_a_pattern_for_its_target() {
        // `({a} = {}) => b` reads `{a} = {}` as an `AssignmentExpression`, so the `=` refines the
        // literal into an *assignment* pattern before the `=>` has been seen — and then the `=>`
        // says it was a parameter with a default all along, whose target is a *binding* pattern.
        // The literal is gone by then, so the shapes are converted.
        assert_eq!(shape("({a} = {}) => b"), "(=> [(= {(a a)} {})] b)");
        assert_eq!(shape("([a] = []) => b"), "(=> [(= [a] [])] b)");
        for source in [
            "(a, {b} = {}) => c;",
            "([a = 1] = []) => b;",
            "({a: {b}} = {}) => c;",
            "([[a]] = []) => b;",
            "({...a} = {}) => b;",
            "([...a] = []) => b;",
            "({} = {}) => a;",
            "([] = []) => a;",
            "({a, ...b} = {}) => c;",
            "({a} = {}, [b] = []) => c;",
            "({a: {b} = {}} = {}) => c;",
            "async ({a} = {}) => b;",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // The two grammars are not the same, which is why this is a conversion and not a cast:
        // every target still goes through the binding rules, and those refuse what a binding may
        // not have.
        for source in [
            "({a: b.c} = {}) => d;",
            "([a.b] = []) => c;",
            "({...a.b}) => c;",
            "([...a.b]) => c;",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // …and the very same shapes are ordinary assignment patterns, which is the comparison
        // that shows there are two grammars here and not one.
        assert!(parse_script("({a: b.c} = {});").is_ok());
        assert!(parse_script("[a.b] = [];").is_ok());
    }

    #[test]
    fn a_comma_after_a_rest_is_carried_by_the_element_that_it_followed() {
        // A trailing comma adds no element, so `[...a, ]` and `[...a]` are the same list and only
        // the element can say which was written. On the element rather than on the parser,
        // because a record on the parser cannot say *which* literal it belongs to.
        for source in [
            "[...a,] = b;",
            "[[...a,]] = b;",
            "({x: [...a,]} = b);",
            "([...a,]) => b;",
            "([[...a,]]) => b;",
            "({p: [...a,]}) => b;",
            "var [[...a,]] = b;",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::RestElementMustBeLast,
                "{source:?}"
            );
        }
        // An object's rest is last too, which nothing used to check at all.
        for source in [
            "({...a,} = b);",
            "({...a,}) => b;",
            "[{...a,}] = b;",
            "({p: {...a,}} = b);",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::RestElementMustBeLast,
                "{source:?}"
            );
        }
        // As a *literal* the comma is ordinary, and the literal beside it stays ordinary too —
        // which is what a parser-wide record could not manage: `[...a,]` here is a value and
        // `[b]` there is a pattern, and they have nothing to do with each other.
        for source in [
            "[...a,];",
            "({...a,});",
            "[...a,]; [b] = c;",
            "[a, ...b,]; [c] = d;",
            "f([...a,]); [b] = c;",
            "[[...a,]]; [b] = c;",
            "x = [...a,]; [b] = c;",
            "(x = [...a,], [b]) => c;",
            "([b], x = [...a,]) => c;",
            "[x = [...a,], [b]] = c;",
            "function f(x = [...a,], [b]) {}",
            "var x = [...a,], [b] = c;",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // A rest with an element after it needs no such record: the element is in the list, and
        // refinement finds it sitting behind the rest.
        assert!(parse_script("[...a, b];").is_ok());
        assert_eq!(
            kind("[...a, b] = c;"),
            ParseErrorKind::RestElementMustBeLast
        );
    }
}
