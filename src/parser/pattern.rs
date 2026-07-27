//! Refining a literal into the pattern it covered (ECMAScript §13.15.5).
//!
//! `[a, b]` is parsed as an `ArrayLiteral` because nothing yet says it is not one; the `=` that
//! follows is what decides. So this takes the finished literal and rebuilds it as the
//! `ArrayAssignmentPattern` "that is covered by" it, refusing where the two grammars part company.
//!
//! # Why a rebuild and not a re-parse
//!
//! The specification's cover grammar is stated as a re-reading of the same source, and re-reading
//! it is what a parser with a rewindable lexer would do. It would not work here, and the reason is
//! worth knowing: `{a = 1}` is a valid *pattern* and not a valid *literal*, so by the time the `=`
//! arrives the literal parse would already have failed. The literal parser therefore accepts it
//! and records the position, and this is what either consumes the record or lets it become the
//! Syntax Error §13.2.5.1 says it is. See [`super::Parser::cover_initialized_name`].
//!
//! # Where the two grammars differ, in both directions
//!
//! | source | literal | pattern |
//! | --- | --- | --- |
//! | `{a = 1}` | no — §13.2.5.1 | yes |
//! | `[...a, ]` | yes | no — nothing follows a rest element |
//! | `[...a, b]` | yes | no — a rest element is last |
//! | `[...[a]]` | yes | yes |
//! | `{...[a]}` | yes | no — §13.15.5.1 |
//! | `[1]` | yes | no — `1` is nothing to assign to |
//! | `[f()]` | yes | no — *not simple*, which is stricter than §13.15.1 |
//!
//! The last is the one to remember. §13.15.1 refuses an assignment target whose
//! `AssignmentTargetType` is `invalid`, and §13.15.5.1 refuses a destructuring target that is not
//! `simple` — so the `web-compat` middle case of §8.6.4 is allowed by one and refused by the
//! other, on every host.

use super::operator::is_simple_assignment_target;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    ArrayElement, ArrayPattern, AssignmentTarget, Expr, ExprKind, ObjectPattern, Pattern,
    PatternElement, PatternProperty, PropertyDefinition,
};

impl Parser<'_> {
    /// Whether `expr` is a literal that could be refined into a pattern.
    ///
    /// Parentheses are what makes this a question about the node rather than about its kind:
    /// §13.15.1's carve-out names an `ObjectLiteral` or an `ArrayLiteral`, and a parenthesized one
    /// is neither — it is a `PrimaryExpression : CoverParenthesizedExpression…`. So `([a]) = b`
    /// has no derivation where `[a] = b` does.
    pub(super) fn covers_a_pattern(expr: &Expr) -> bool {
        !expr.parenthesized && matches!(expr.kind, ExprKind::Array(_) | ExprKind::Object(_))
    }

    /// Refine a literal into the pattern it covered, or say why it covered none.
    ///
    /// Consumes the literal, because what it produces replaces it: keeping both would be keeping
    /// a tree that means two things.
    pub(super) fn refine_to_pattern(&mut self, expr: Expr) -> Result<Pattern, ParseError> {
        // Whatever the literal parser recorded about this literal is now the pattern's business,
        // and a `{a = 1}` inside it has found the `=` that makes it legal.
        self.cover_initialized_name = None;
        let pattern = self.refine_pattern(expr)?;
        // …while a rest element with something after it was fine as a literal and is not a
        // pattern, so that record is only now an error.
        if let Some(span) = self.rest_followed_by_comma.take() {
            return Err(ParseError {
                kind: ParseErrorKind::RestElementMustBeLast,
                span,
            });
        }
        Ok(pattern)
    }

    /// The refinement proper, which recurses once per level of nesting.
    fn refine_pattern(&mut self, expr: Expr) -> Result<Pattern, ParseError> {
        let span = expr.span;
        match expr.kind {
            ExprKind::Array(elements) => {
                self.enter()?;
                let refined = self.refine_array(elements, span);
                self.leave();
                Ok(Pattern::Array(refined?))
            }
            ExprKind::Object(properties) => {
                self.enter()?;
                let refined = self.refine_object(properties, span);
                self.leave();
                Ok(Pattern::Object(refined?))
            }
            // Only the two literals cover a pattern, and `covers_a_pattern` is asked first
            // everywhere this is reached — so this is the caller's mistake rather than the
            // source's, and saying so beats inventing a diagnostic about the source.
            _ => Err(ParseError {
                kind: ParseErrorKind::InvalidDestructuringTarget,
                span,
            }),
        }
    }

    /// `ArrayAssignmentPattern` from the `ArrayLiteral` that covered it.
    fn refine_array(
        &mut self,
        elements: Box<[ArrayElement]>,
        span: crate::span::Span,
    ) -> Result<ArrayPattern, ParseError> {
        let mut refined: Vec<Option<PatternElement>> = Vec::with_capacity(elements.len());
        let mut rest: Option<Box<AssignmentTarget>> = None;
        for element in Vec::from(elements) {
            // A rest element is last, so anything after one has no derivation. The trailing-comma
            // case looks identical here and is caught by the record the literal parser left.
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span,
                });
            }
            match element {
                ArrayElement::Hole => refined.push(None),
                ArrayElement::Value(value) => {
                    refined.push(Some(self.refine_element(value)?));
                }
                ArrayElement::Spread(target) => {
                    // `AssignmentRestElement : ... DestructuringAssignmentTarget` — no
                    // `Initializer`, so `[...a = 1] = b` has no derivation. The default would
                    // have been parsed into the target, which is where it is found.
                    if let ExprKind::Assignment { .. } = target.kind {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestElementWithInitializer,
                            span: target.span,
                        });
                    }
                    rest = Some(Box::new(self.refine_target(target)?));
                }
            }
        }
        Ok(ArrayPattern {
            elements: refined.into_boxed_slice(),
            rest,
            span,
        })
    }

    /// `ObjectAssignmentPattern` from the `ObjectLiteral` that covered it.
    fn refine_object(
        &mut self,
        properties: Box<[PropertyDefinition]>,
        span: crate::span::Span,
    ) -> Result<ObjectPattern, ParseError> {
        let mut refined: Vec<PatternProperty> = Vec::with_capacity(properties.len());
        let mut rest: Option<Box<Expr>> = None;
        for property in Vec::from(properties) {
            if rest.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::RestElementMustBeLast,
                    span,
                });
            }
            match property {
                PropertyDefinition::KeyValue { key, value } => refined.push(PatternProperty {
                    key,
                    value: self.refine_element(value)?,
                }),
                PropertyDefinition::Shorthand { name, span } => refined.push(PatternProperty {
                    key: crate::ast::PropertyKey::Identifier(name.clone()),
                    value: PatternElement {
                        target: AssignmentTarget::Simple(Expr::new(
                            ExprKind::Identifier(name.into_string()),
                            span,
                        )),
                        default: None,
                    },
                }),
                PropertyDefinition::ShorthandWithDefault {
                    name,
                    default,
                    span,
                } => refined.push(PatternProperty {
                    key: crate::ast::PropertyKey::Identifier(name.clone()),
                    value: PatternElement {
                        target: AssignmentTarget::Simple(Expr::new(
                            ExprKind::Identifier(name.into_string()),
                            span,
                        )),
                        default: Some(default),
                    },
                }),
                // A `MethodDefinition` has no `AssignmentProperty` to be refined into: there is
                // nowhere to put a value that is written as a function.
                PropertyDefinition::Method {
                    key: _,
                    kind: _,
                    function,
                } => {
                    return Err(ParseError {
                        kind: ParseErrorKind::InvalidDestructuringTarget,
                        span: function.span,
                    });
                }
                PropertyDefinition::Spread(target) => {
                    // §13.15.5.1: an `AssignmentRestProperty` target may not be an array or object
                    // literal. An array's rest may — `[...[a]] = b` is legal — and the asymmetry
                    // is real: there is no way to spread the remaining properties *into* a
                    // pattern, where the remaining elements of an iterator can be.
                    if Self::covers_a_pattern(&target) {
                        return Err(ParseError {
                            kind: ParseErrorKind::RestTargetMayNotBePattern,
                            span: target.span,
                        });
                    }
                    Self::require_simple_target(&target)?;
                    rest = Some(Box::new(target));
                }
            }
        }
        Ok(ObjectPattern {
            properties: refined.into_boxed_slice(),
            rest,
            span,
        })
    }

    /// `AssignmentElement : DestructuringAssignmentTarget Initializer_opt`.
    ///
    /// The initialiser was parsed as an assignment, `[a = 1]` being a perfectly good literal, so
    /// finding it means taking that assignment apart again.
    fn refine_element(&mut self, expr: Expr) -> Result<PatternElement, ParseError> {
        let ExprKind::Assignment {
            operator,
            target,
            value,
        } = expr.kind
        else {
            return Ok(PatternElement {
                target: self.refine_target(expr)?,
                default: None,
            });
        };
        // Only `=` covers a default. `[a += 1] = b` is an assignment whose target is `a`, and an
        // assignment is not a `DestructuringAssignmentTarget`.
        if operator != crate::ast::AssignmentOperator::Assign {
            return Err(ParseError {
                kind: ParseErrorKind::InvalidDestructuringTarget,
                span: expr.span,
            });
        }
        let target = match *target {
            AssignmentTarget::Simple(target) => self.refine_target(target)?,
            AssignmentTarget::Pattern(pattern) => AssignmentTarget::Pattern(pattern),
        };
        Ok(PatternElement {
            target,
            default: Some(value),
        })
    }

    /// `DestructuringAssignmentTarget : LeftHandSideExpression` (§13.15.5).
    fn refine_target(&mut self, expr: Expr) -> Result<AssignmentTarget, ParseError> {
        if Self::covers_a_pattern(&expr) {
            return Ok(AssignmentTarget::Pattern(self.refine_pattern(expr)?));
        }
        Self::require_simple_target(&expr)?;
        Ok(AssignmentTarget::Simple(expr))
    }

    /// §13.15.5.1: a destructuring target that is not a pattern must be *simple*.
    fn require_simple_target(expr: &Expr) -> Result<(), ParseError> {
        if is_simple_assignment_target(expr) {
            return Ok(());
        }
        Err(ParseError {
            kind: ParseErrorKind::InvalidDestructuringTarget,
            span: expr.span,
        })
    }
}

impl Parser<'_> {
    /// Turn an unclaimed `{a = 1}` into the Syntax Error §13.2.5.1 says it is.
    ///
    /// Called where an expression has stopped being a candidate for refinement: no `=` followed
    /// it, or one did and the target was not a literal. A literal still open around it means the
    /// question is not settled — `[{a = 1}] = b` is legal, and the array is what makes it so.
    pub(super) fn report_unrefined_cover_grammar(&mut self) -> Result<(), ParseError> {
        if self.literal_depth > 0 {
            return Ok(());
        }
        match self.cover_initialized_name.take() {
            Some(span) => Err(ParseError {
                kind: ParseErrorKind::ShorthandPropertyWithInitializer,
                span,
            }),
            None => Ok(()),
        }
    }
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
    fn an_array_literal_before_an_equals_is_a_pattern() {
        assert_eq!(shape("[a] = b"), "(= [a] b)");
        assert_eq!(shape("[a, b] = c"), "(= [a b] c)");
        assert_eq!(shape("[] = b"), "(= [] b)");
        assert_eq!(shape("[a.b] = c"), "(= [(. a b)] c)");
        assert_eq!(shape("[a[0]] = c"), "(= [([] a 0)] c)");
        // A hole is a position deliberately skipped, and survives the refinement as one.
        assert_eq!(shape("[, a] = b"), "(= [<hole> a] b)");
        assert_eq!(shape("[a, ] = b"), "(= [a] b)");
        // `AssignmentElement : DestructuringAssignmentTarget Initializer_opt` — the default was
        // parsed as an assignment, `[a = 1]` being a perfectly good literal, so refining it means
        // taking that assignment apart again.
        assert_eq!(shape("[a = 1] = b"), "(= [(= a 1)] b)");
        assert_eq!(shape("[a.b = 1] = c"), "(= [(= (. a b) 1)] c)");
        // …and only `=` covers a default: `[a += 1]` is an assignment, which is no target.
        assert_eq!(
            error_kind("[a += 1] = b"),
            ParseErrorKind::InvalidDestructuringTarget
        );
        // Patterns nest, and a nested one is refined by the same recursion.
        assert_eq!(shape("[[a]] = b"), "(= [[a]] b)");
        assert_eq!(shape("[{a}] = b"), "(= [{(a a)}] b)");
        assert_eq!(shape("[[[a]]] = b"), "(= [[[a]]] b)");
    }

    #[test]
    fn an_object_literal_before_an_equals_is_a_pattern() {
        assert_eq!(shape("({} = b)"), "(= {} b)");
        assert_eq!(shape("({a} = b)"), "(= {(a a)} b)");
        assert_eq!(shape("({a, b} = c)"), "(= {(a a) (b b)} c)");
        assert_eq!(shape("({a: b} = c)"), "(= {(a b)} c)");
        assert_eq!(shape("({a: b.c} = d)"), "(= {(a (. b c))} d)");
        assert_eq!(shape("({[x]: a} = b)"), "(= {([x] a)} b)");
        assert_eq!(shape("({1: a} = b)"), "(= {(n1 a)} b)");
        assert_eq!(shape("({a: b = 1} = c)"), "(= {(a (= b 1))} c)");
        assert_eq!(shape("({a: [b]} = c)"), "(= {(a [b])} c)");
        // The `CoverInitializedName` the literal parser kept, finally meaning something.
        // Shorthand expands to the same name on both sides, which is what `{a}` has always meant.
        assert_eq!(shape("({a = 1} = b)"), "(= {(a (= a 1))} b)");
        // …and the record survives a literal nested around it, which is the whole reason it is a
        // record rather than an error where it is found. `[{a = 1}]` alone is refused; the `=`
        // three tokens later is what makes this one legal.
        assert_eq!(shape("[{a = 1}] = b"), "(= [{(a (= a 1))}] b)");
        assert_eq!(shape("[[{a = 1}]] = b"), "(= [[{(a (= a 1))}]] b)");
        assert_eq!(shape("({a: {b = 1}} = c)"), "(= {(a {(b (= b 1))})} c)");
        assert_eq!(shape("({a = 1, b} = c)"), "(= {(a (= a 1)) (b b)} c)");
        // A value that is not a target is not a pattern, however good a literal it was.
        assert_eq!(
            error_kind("({a: 1} = b)"),
            ParseErrorKind::InvalidDestructuringTarget
        );
    }

    #[test]
    fn a_rest_element_is_last_and_an_objects_may_not_be_a_pattern() {
        assert_eq!(shape("[...a] = b"), "(= [(... a)] b)");
        assert_eq!(shape("[a, ...b] = c"), "(= [a (... b)] c)");
        assert_eq!(shape("[...a.b] = c"), "(= [(... (. a b))] c)");
        assert_eq!(shape("({...a} = b)"), "(= {(... a)} b)");
        assert_eq!(shape("({a, ...b} = c)"), "(= {(a a) (... b)} c)");
        assert_eq!(shape("({...a.b} = c)"), "(= {(... (. a b))} c)");
        // Nothing follows a rest element — not another element, and not even a comma. The comma
        // leaves no trace once parsed, which is why the literal parser has to write it down.
        assert_eq!(
            error_kind("[...a, b] = c"),
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            error_kind("[...a, ] = c"),
            ParseErrorKind::RestElementMustBeLast
        );
        assert_eq!(
            error_kind("({...a, b} = c)"),
            ParseErrorKind::RestElementMustBeLast
        );
        // `AssignmentRestElement` has no `Initializer`.
        assert_eq!(
            error_kind("[...a = 1] = b"),
            ParseErrorKind::RestElementWithInitializer
        );
        // An array rest target may be a pattern and an object rest target may not (§13.15.5.1):
        // the remaining elements of an iterator can be spread into one, and there is no way to
        // spread the remaining properties of an object into one.
        assert_eq!(shape("[...[a]] = b"), "(= [(... [a])] b)");
        assert_eq!(shape("[...{a}] = b"), "(= [(... {(a a)})] b)");
        assert_eq!(
            error_kind("({...[a]} = b)"),
            ParseErrorKind::RestTargetMayNotBePattern
        );
        assert_eq!(
            error_kind("({...{a}} = b)"),
            ParseErrorKind::RestTargetMayNotBePattern
        );
        // …and a literal keeps every one of those shapes, being a literal.
        assert!(parse_expression("[...a, b]").is_ok());
        assert!(parse_expression("[...a, ]").is_ok());
        assert!(parse_expression("[...a = 1]").is_ok());
        assert!(parse_expression("({...[a]})").is_ok());
    }

    #[test]
    fn a_pattern_is_refused_where_only_an_equals_can_refine_one() {
        // The compound operators take a `LeftHandSideExpression` and nothing else, so there is no
        // pattern for them to be.
        assert_eq!(
            error_kind("[a] += b"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        assert_eq!(
            error_kind("[a] ||= b"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        assert_eq!(
            error_kind("({a} += b)"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        // Parentheses make it a `CoverParenthesizedExpression` rather than an `ArrayLiteral`, and
        // §13.15.1 carves out the literal — so the ordinary rule applies and refuses it.
        assert_eq!(
            error_kind("([a]) = b"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        assert_eq!(
            error_kind("({a}) = b"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        // A destructuring target must be *simple*, which is stricter than §13.15.1 refusing only
        // *invalid*: `f() = b` and `[f()] = b` are refused for different reasons, and the second
        // would be refused even by a host that allowed the first.
        assert_eq!(
            error_kind("[1] = b"),
            ParseErrorKind::InvalidDestructuringTarget
        );
        assert_eq!(
            error_kind("[a + b] = c"),
            ParseErrorKind::InvalidDestructuringTarget
        );
        assert_eq!(
            error_kind("[f()] = b"),
            ParseErrorKind::InvalidDestructuringTarget
        );
        assert_eq!(
            error_kind("f() = b"),
            ParseErrorKind::InvalidAssignmentTarget
        );
        // An array that is merely compared is still a value, so nothing refines it and nothing
        // complains about what it holds.
        assert!(parse_expression("[1] == b").is_ok());
    }

    #[test]
    fn a_for_in_or_for_of_target_is_refined_the_same_way() {
        assert_eq!(statements("for ([a] of b);"), ["(for-of [a] b <empty>)"]);
        assert_eq!(
            statements("for ({a} of b);"),
            ["(for-of {(a a)} b <empty>)"]
        );
        assert_eq!(statements("for ([a] in b);"), ["(for-in [a] b <empty>)"]);
        assert_eq!(
            statements("for ([a = 1] of b);"),
            ["(for-of [(= a 1)] b <empty>)"]
        );
        assert_eq!(
            statements("for ([a, ...b] of c);"),
            ["(for-of [a (... b)] c <empty>)"]
        );
        assert!(parse_script("for ([1] of b);").is_err());
        assert!(parse_script("for (([a]) of b);").is_err());
    }

    #[test]
    fn no_pattern_however_odd_can_panic() {
        let cases = [
            "[a] = ".to_string(),
            "[...".to_string(),
            "({...} = b)".to_string(),
            "[[[[a]]]] = b".to_string(),
            format!("{}a{} = b", "[".repeat(10_000), "]".repeat(10_000)),
            format!("[{}] = b", "a, ".repeat(100_000)),
        ];
        for source in &cases {
            let _ = parse_expression(source);
        }
        // Refinement recurses once per level, so it is bounded by the same cap the parse was.
        assert_eq!(
            error_kind(&format!(
                "{}a{} = b",
                "[".repeat(10_000),
                "]".repeat(10_000)
            )),
            ParseErrorKind::TooDeeplyNested
        );
        // A long flat pattern is a loop, so it is bounded by memory.
        assert!(parse_expression(&format!("[{}] = b", "a, ".repeat(10_000))).is_ok());
    }
}
