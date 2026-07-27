//! Template literals, and the tags that take them (ECMAScript §13.2.8, §13.3).
//!
//! # The `}` that is not a `}`
//!
//! `` `a${b}c` `` is four tokens, not six: a head `` `a${ ``, the expression `b`, and a tail
//! `` }c` ``. Whether a `}` closes a block or resumes a template is not something the lexer can
//! know, so it asks — [`Goal::TemplateTail`] — and this is the caller that knows. That is why the
//! lexer has carried four goal symbols since its first slice and has only ever needed two.
//!
//! Nesting comes free with it. `` `${ `${a}` }` `` works because the inner template is read by a
//! recursive call, which is holding the outer one's state on the stack — where a lexer-side depth
//! counter would have been a second copy of what the parser already knows.
//!
//! # An ill-formed escape is legal exactly when somebody else will read it
//!
//! §13.2.8.1: a `NotEscapeSequence` is a Syntax Error unless the `[Tagged]` parameter is set.
//! `` `\u{}` `` is refused and ``f`\u{}` `` is not, because a tag function is handed the *raw*
//! text as well as the cooked value, and `undefined` for the cooked one is a thing it can be told.
//! An untagged template has no such channel: there would be nothing for it to evaluate to.
//!
//! The lexer already knows — it flags `cooked_undefined` and hands back a `TemplateValue` whose
//! `cooked` is `None` — so all this does is decide which of the two it is looking at, which is the
//! one thing the lexer cannot.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Expr, ExprKind, TemplateElement, TemplateLiteral};
use crate::lexer::{Goal, TemplatePart, TokenKind, template_value};

/// Whether a template is being read for a tag function or for itself.
///
/// The `[Tagged]` grammar parameter of §13.2.8, and the only thing it decides is whether an
/// ill-formed escape is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tagged {
    /// `` f`…` `` — the raw text reaches a function, so a `NotEscapeSequence` is legal.
    Yes,
    /// `` `…` `` — there is nothing for a bad escape to become.
    No,
}

impl Parser<'_> {
    /// `TemplateLiteral` (§13.2.8), with the cursor on its first component.
    pub(super) fn parse_template(&mut self, tagged: Tagged) -> Result<Expr, ParseError> {
        let start = self.current;
        let mut quasis = Vec::new();
        let mut expressions = Vec::new();
        let mut part = self.read_template_component(tagged, &mut quasis)?;
        let mut end = start.span;
        while part.is_followed_by_substitution() {
            self.enter()?;
            // `TemplateHead Expression[+In] TemplateSpans` — a full `Expression`, so a comma
            // sequences inside the braces rather than ending the substitution.
            let expr = self.parse_expression(AllowIn::Yes);
            self.leave();
            expressions.push(expr?);
            // The `}` that resumes the template. Asking for it under this goal is the whole of
            // what makes a template different from anything else the parser reads.
            // The `}` was read by whatever finished the expression, under a goal that had no
            // way to know a template was waiting for it. Reading it again from the same offset is
            // the one exception to the parser's never-read-twice invariant, and
            // [`Parser::reread_current`] is where that is said. No guard for the end of input:
            // reading it again gives the end of input, which is what the component check below
            // is about to complain about anyway.
            self.reread_current(Goal::TemplateTail)?;
            end = self.current.span;
            part = self.read_template_component(tagged, &mut quasis)?;
        }
        Ok(Expr::new(
            ExprKind::Template(Box::new(TemplateLiteral {
                quasis: quasis.into_boxed_slice(),
                expressions: expressions.into_boxed_slice(),
            })),
            start.span.to(end),
        ))
    }

    /// One `NoSubstitutionTemplate`, `TemplateHead`, `TemplateMiddle` or `TemplateTail`.
    fn read_template_component(
        &mut self,
        tagged: Tagged,
        quasis: &mut Vec<TemplateElement>,
    ) -> Result<TemplatePart, ParseError> {
        let token = self.current;
        let TokenKind::Template {
            part,
            cooked_undefined,
        } = token.kind
        else {
            return Err(self.unexpected("a template"));
        };
        // §13.2.8.1. The lexer admits the ill-formed escape and says so; only this knows whether
        // there is a tag function to be handed the raw text instead.
        if cooked_undefined && tagged == Tagged::No {
            return Err(ParseError {
                kind: ParseErrorKind::BadEscapeInUntaggedTemplate,
                span: token.span,
            });
        }
        let value =
            template_value(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
        quasis.push(TemplateElement {
            cooked: value.cooked,
            raw: value.raw,
            span: token.span,
        });
        // A component that ends in `${` is followed by an expression, so the next token is read
        // where an operand may stand; one that closes the template is followed by an operator.
        self.advance(if part.is_followed_by_substitution() {
            Goal::RegExp
        } else {
            Goal::Div
        })?;
        Ok(part)
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// The kind of error `source` fails with.
    fn kind(source: &str) -> ParseErrorKind {
        match parse_expression(source) {
            Err(err) => err.kind,
            Ok(expr) => panic!("{source:?} should not parse, got {expr:?}"), // needs the error
        }
    }

    #[test]
    fn a_template_is_its_components_and_the_expressions_between_them() {
        assert_eq!(shape("``"), "(tpl [\"\"])");
        assert_eq!(shape("`a`"), "(tpl [\"a\"])");
        assert_eq!(shape("`${a}`"), "(tpl [\"\" \"\"] [a])");
        assert_eq!(shape("`a${b}c`"), "(tpl [\"a\" \"c\"] [b])");
        assert_eq!(shape("`a${b}c${d}e`"), "(tpl [\"a\" \"c\" \"e\"] [b d])");
        assert_eq!(shape("`${a}${b}`"), "(tpl [\"\" \"\" \"\"] [a b])");
        // `TemplateHead Expression[+In] TemplateSpans` — a full `Expression`, so a comma
        // sequences inside the braces rather than ending the substitution.
        assert_eq!(shape("`${a, b}`"), "(tpl [\"\" \"\"] [(, a b)])");
        assert_eq!(shape("`${a ? b : c}`"), "(tpl [\"\" \"\"] [(? a b c)])");
        // Nesting comes free: the inner template is read by a recursive call, which is holding
        // the outer one's state on the stack.
        assert_eq!(
            shape("`${`${a}`}`"),
            "(tpl [\"\" \"\"] [(tpl [\"\" \"\"] [a])])"
        );
        assert_eq!(shape("`${{a: 1}}`"), "(tpl [\"\" \"\"] [{(a 1)}])");
        assert_eq!(shape("`${() => 1}`"), "(tpl [\"\" \"\"] [(=> [] 1)])");
        // …and a template is an operand like any other.
        assert_eq!(shape("`a`.length"), "(. (tpl [\"a\"]) length)");
        assert!(parse_script("x = `a`;").is_ok());
        assert!(parse_script("f(`a`);").is_ok());
    }

    #[test]
    fn a_tag_is_a_member_expression_and_the_template_is_its_argument() {
        assert_eq!(shape("f`a`"), "(tag f (tpl [\"a\"]))");
        assert_eq!(shape("f`a${b}c`"), "(tag f (tpl [\"a\" \"c\"] [b]))");
        assert_eq!(shape("a.b`c`"), "(tag (. a b) (tpl [\"c\"]))");
        assert_eq!(shape("a[0]`c`"), "(tag ([] a 0) (tpl [\"c\"]))");
        // `CallExpression TemplateLiteral` as well as `MemberExpression TemplateLiteral`, so a
        // tag chains with everything else — including another template.
        assert_eq!(shape("f`a`.b"), "(. (tag f (tpl [\"a\"])) b)");
        assert_eq!(
            shape("f`a``b`"),
            "(tag (tag f (tpl [\"a\"])) (tpl [\"b\"]))"
        );
        assert_eq!(shape("f`a`(b)"), "(call (tag f (tpl [\"a\"])) [b])");
        assert_eq!(shape("f(a)`b`"), "(tag (call f [a]) (tpl [\"b\"]))");
        assert!(parse_script("new f`a`;").is_ok());
    }

    #[test]
    fn an_ill_formed_escape_is_legal_exactly_when_a_tag_will_be_handed_the_raw_text() {
        // §13.2.8.1. The lexer admits all three and flags them; only the parser knows whether
        // there is a tag function to hand the raw text to.
        for source in [r"`\u{}`", r"`\x`", r"`\01`", r"`\u`"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::BadEscapeInUntaggedTemplate,
                "{source:?}"
            );
        }
        for source in [r"f`\u{}`", r"f`\x`", r"f`\01`", r"f`\u`"] {
            assert!(parse_expression(source).is_ok(), "{source:?}");
        }
        // …including in a component after a substitution, which is a separate token.
        assert!(parse_expression(r"f`a${b}\x`").is_ok());
        assert_eq!(
            kind(r"`a${b}\x`"),
            ParseErrorKind::BadEscapeInUntaggedTemplate
        );
        // A well-formed escape cooks, tagged or not, and the raw value keeps it as written.
        assert_eq!(shape(r"`\n`"), "(tpl [\"\\n\"])");
        assert_eq!(shape(r"f`\n`"), "(tag f (tpl [\"\\n\"]))");
    }

    #[test]
    fn no_template_however_truncated_can_panic() {
        let cases = [
            "`".to_string(),
            "`${".to_string(),
            "`${a".to_string(),
            "`${a}".to_string(),
            "f`".to_string(),
            "`${`".to_string(),
            "`${".repeat(1000),
            format!("`{}`", "${a}".repeat(10_000)),
        ];
        for source in &cases {
            let _ = parse_expression(source);
        }
        // A long flat template is a loop; a nested one is bounded by the cap.
        assert!(parse_expression(&format!("`{}`", "${a}".repeat(10_000))).is_ok());
        assert_eq!(kind(&"`${".repeat(1000)), ParseErrorKind::TooDeeplyNested);
    }
}
