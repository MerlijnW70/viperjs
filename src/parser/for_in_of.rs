//! `for`-`in` and `for`-`of` (ECMAScript §14.7.5).
//!
//! The head is shared with the three-part loop and lives in [`super::for_statement`], because
//! nothing distinguishes the three productions until the token *after* the first clause: a `;`
//! makes it a `ForStatement`, and `in` or `of` a `ForInOfStatement`. That is also why the first
//! clause is read under `[~In]` — see [`super::expression::AllowIn`]. This file is what happens
//! once the answer is known.
//!
//! # The two forms differ in more than a word
//!
//! `for (a in b, c)` parses and `for (a of b, c)` does not, because the iterable of a `for`-`in`
//! is an `Expression[+In]` and that of a `for`-`of` is an `AssignmentExpression[+In]` — no comma.
//! Both are `[+In]`, so `for (a in b in c)` is `for (a in (b in c))`.
//!
//! `for`-`of` also carries a lookahead restriction its sibling does not:
//! `[lookahead ∉ { let, async of }]`. `for (let.a of b)` is refused and `for (let.a in b)` is
//! fine, because the restriction is on the *token* `let` rather than on anything it might begin —
//! and `for ((let) of b)` is fine again, a parenthesis being a different token. The `async` half
//! is a two-token restriction, so `for (async of b)` is refused while `for (async.x of b)` and
//! `for (async() of b)` are not.
//!
//! # A binding here is one name and takes no initialiser
//!
//! `ForBinding` is a `BindingIdentifier` or a `BindingPattern`, singular, with no `Initializer` in
//! the grammar at all. So `for (var a, b in c)` and `for (var a = 1 of b)` have no derivation.
//!
//! `for (var a = 1 in b)` is the exception, and it is refused here. Annex B.3.5 restores an
//! initialiser to exactly that one shape — `var`, `in`, a `BindingIdentifier` rather than a
//! pattern, and non-strict code — so refusing it is right everywhere except sloppy code on a host
//! that implements B.3.5.
//!
//! **The reason recorded here used to be that this parser did not track strictness. It does** —
//! `Parser::strict`, set by the Directive Prologue and accurate at this point — so that is no
//! longer what stands in the way. What does is the *runtime* half: §B.3.5 evaluates the
//! initialiser and assigns it **before** `ForIn/OfHeadEvaluation` runs, which is a shape the
//! compiler has nowhere to put, and the whole clause is worth one run of test262. Costed and left
//! deliberately; the condition here is one `&& !self.strict` away whenever the other half is
//! written.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Expr, ForInOfKind, ForInOfStatement, ForInOfTarget, StmtKind};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::span::Span;

impl Parser<'_> {
    /// Whether an `in` or `of` follows the first clause of a `for` header.
    ///
    /// `of` is not a reserved word — it is an ordinary identifier everywhere else — so it is
    /// recognised by its text, and only when written without escapes: §5.1.5.1 makes a terminal
    /// match literal source characters, the same rule that keeps `let` from being a declaration.
    pub(super) fn for_in_of_operator(&self) -> Option<ForInOfKind> {
        if self.current.kind == TokenKind::Keyword(ReservedWord::In) {
            return Some(ForInOfKind::In);
        }
        if self.at_contextual("of") {
            return Some(ForInOfKind::Of);
        }
        None
    }

    /// Everything after the `in` or `of`, with `left` already parsed.
    pub(super) fn parse_for_in_of_tail(
        &mut self,
        left: ForInOfTarget,
        kind: ForInOfKind,
    ) -> Result<(StmtKind, Span), ParseError> {
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // The one real difference between the two productions: `in` takes an `Expression` and
        // `of` an `AssignmentExpression`, so a comma sequences in the first and ends the header
        // in the second. Both are `[+In]` — the restriction was on the clause before this one.
        let right = match kind {
            ForInOfKind::In => self.parse_expression(AllowIn::Yes),
            ForInOfKind::Of => self.parse_assignment(AllowIn::Yes),
        };
        self.leave();
        let right = right?;
        self.eat(TokenKind::RParen, Goal::RegExp, "`)`")?;
        let body = self.parse_statement(super::LabelledFunction::Refused)?;
        if let ForInOfTarget::Declaration(declaration) = &left
            && declaration.kind.is_lexical()
        {
            // §14.7.5.1, the same rule §14.7.4.1 states for the three-part form: a header
            // declaration is its own scope, and a `var` in the body passes through it.
            super::scope::check_header_against_body(declaration, std::slice::from_ref(&body))?;
        }
        let end = body.span;
        Ok((
            StmtKind::ForInOf(Box::new(ForInOfStatement {
                kind,
                // Set by [`Parser::parse_for`] once the header has said which production won.
                is_await: false,
                left,
                right,
                body,
            })),
            end,
        ))
    }

    /// §14.7.5.1: the target of a `for`-`in` or `for`-`of` must be assignable.
    ///
    /// Stated as "a Syntax Error if the AssignmentTargetType is invalid", which is the same test
    /// an assignment applies — so `for (1 in b)` and `for (a + b in c)` are refused for the
    /// reason `1 = b` is, while `for ((a) in b)` is fine because parentheses do not change what
    /// a thing is.
    pub(super) fn check_for_in_of_target(&self, left: &Expr) -> Result<(), ParseError> {
        if super::operator::is_simple_assignment_target(left) {
            // A head like `for (eval in x)` assigns to the name on every turn of the loop, so
            // §13.1.1 reaches it — and this is the only place it can be asked, the target here
            // never passing through the assignment level or through a refinement.
            if let crate::ast::ExprKind::Identifier(name) = &left.kind {
                self.check_target_name(name, left.span)?;
            }
            return Ok(());
        }
        Err(ParseError {
            kind: ParseErrorKind::InvalidAssignmentTarget,
            span: left.span,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    #[test]
    fn both_forms_take_a_target_or_a_binding() {
        assert_eq!(statements("for (a in b);"), ["(for-in a b <empty>)"]);
        assert_eq!(statements("for (a of b);"), ["(for-of a b <empty>)"]);
        assert_eq!(
            statements("for (var a in b);"),
            ["(for-in (var a) b <empty>)"]
        );
        assert_eq!(
            statements("for (var a of b);"),
            ["(for-of (var a) b <empty>)"]
        );
        assert_eq!(
            statements("for (let a in b);"),
            ["(for-in (let a) b <empty>)"]
        );
        assert_eq!(
            statements("for (let a of b);"),
            ["(for-of (let a) b <empty>)"]
        );
        assert_eq!(
            statements("for (const a in b);"),
            ["(for-in (const a) b <empty>)"]
        );
        assert_eq!(
            statements("for (const a of b) c;"),
            ["(for-of (const a) b c)"]
        );
        // A `LeftHandSideExpression` is more than a name.
        assert_eq!(
            statements("for (a.b in c);"),
            ["(for-in (. a b) c <empty>)"]
        );
        assert_eq!(
            statements("for (a[b] of c);"),
            ["(for-of ([] a b) c <empty>)"]
        );
        assert_eq!(statements("for ((a) in b);"), ["(for-in a b <empty>)"]);
        assert_eq!(statements("for (a in b) { c; }"), ["(for-in a b {c})"]);
        for source in [
            "for (a in);",
            "for (a of);",
            "for (in b);",
            "for (a in b",
            "for (a of b",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn the_iterable_of_an_in_is_an_expression_and_that_of_an_of_is_not() {
        // `in` takes `Expression[+In]`, `of` takes `AssignmentExpression[+In]`. So a comma
        // sequences in the first and simply ends the header in the second.
        assert_eq!(
            statements("for (a in b, c);"),
            ["(for-in a (, b c) <empty>)"]
        );
        assert!(parse_script("for (a of b, c);").is_err());
        assert!(parse_script("for (var a of b, c);").is_err());
        assert_eq!(
            statements("for (a of (b, c));"),
            ["(for-of a (, b c) <empty>)"]
        );
        // Both are `[+In]`: the restriction was on the clause before, and does not reach here.
        assert_eq!(
            statements("for (a in b in c);"),
            ["(for-in a (in b c) <empty>)"]
        );
        assert!(parse_script("for (a of b of c);").is_err());
        // An assignment is an `AssignmentExpression`, so both take one.
        assert_eq!(
            statements("for (a of b = c);"),
            ["(for-of a (= b c) <empty>)"]
        );
        assert_eq!(
            statements("for (var a in b = c);"),
            ["(for-in (var a) (= b c) <empty>)"]
        );
    }

    #[test]
    fn a_target_that_cannot_be_assigned_to_is_refused() {
        // §14.7.5.1, which applies the same AssignmentTargetType test an assignment does.
        assert_eq!(
            script_error("for (1 in b);").kind,
            ParseErrorKind::InvalidAssignmentTarget
        );
        for source in [
            "for (a + b in c);",
            "for ((a, b) in c);",
            "for (this of b);",
            "for (1 of b);",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::InvalidAssignmentTarget,
                "{source:?}"
            );
        }
        // …and parentheses do not change what a thing is.
        assert!(parse_script("for ((a) in b);").is_ok());
        assert!(parse_script("for ((a.b) of c);").is_ok());
        // A call is refused here for the same reason `f() = 1` is refused: §8.6.4 gives a
        // `CallExpression` an AssignmentTargetType of `invalid`, and only returns `web-compat`
        // instead under a Normative Optional taken by hosts that are web browsers. ViperJS is not
        // one — GOAL.md is explicit about the hosts it is for — so it takes the default, which is
        // also what every host must do in strict code. V8 accepts `for (f() in b)`; this is the
        // divergence, and it is deliberate.
        assert_eq!(
            script_error("for (f() in b);").kind,
            ParseErrorKind::InvalidAssignmentTarget
        );
        assert_eq!(
            script_error("f() = 1;").kind,
            ParseErrorKind::InvalidAssignmentTarget
        );
    }

    #[test]
    fn a_for_of_target_may_not_begin_with_let_or_be_the_word_async() {
        // `[lookahead ∉ { let, async of }]`, which `for`-`in` does not have. The `let` half is a
        // one-token restriction, so it catches anything beginning with the word…
        assert_eq!(
            script_error("for (let.a of b);").kind,
            ParseErrorKind::ForOfTargetBeginsWithLet
        );
        // …and `for`-`in` is unrestricted, which is the comparison that shows it is the token
        // being refused rather than the expression.
        assert_eq!(
            statements("for (let.a in b);"),
            ["(for-in (. let a) b <empty>)"]
        );
        assert_eq!(statements("for (let in b);"), ["(for-in let b <empty>)"]);
        // A parenthesis is a different token, so it escapes the restriction entirely.
        assert_eq!(statements("for ((let) of b);"), ["(for-of let b <empty>)"]);
        // The `async` half is a *two*-token restriction: it is the sequence `async of` that has
        // no derivation, not anything starting with `async`. It needs no code of its own — §15.9's
        // arrow head commits on `async` followed by an identifier, so what fails is the `=>` that
        // an arrow head owes. Which is what the grammar says: with the for-of alternative excluded
        // by the lookahead, the head has to be an `Expression`, and `async of` begins only one.
        assert_eq!(
            script_error("for (async of b);").kind,
            ParseErrorKind::Unexpected {
                expected: "`=>`",
                found: crate::lexer::TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
        // …and the three-part alternative was never restricted, so this is an ordinary loop whose
        // initialiser happens to be an async arrow.
        assert_eq!(
            statements("for (async of => 1;;);"),
            ["(for (async=> [of] 1) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (async.x of b);"),
            ["(for-of (. async x) b <empty>)"]
        );
        assert_eq!(
            statements("for (async in b);"),
            ["(for-in async b <empty>)"]
        );
        assert_eq!(
            statements("for ((async) of b);"),
            ["(for-of async b <empty>)"]
        );
        // §5.1.5.1 again: a lookahead restriction compares tokens against terminals, and a
        // terminal matches literal source characters — so an escaped spelling is a different
        // token and slips both halves of the restriction. The same rule that keeps
        // `\u006cet x = 1` from being a declaration.
        assert_eq!(
            statements(r"for (\u0061sync of b);"),
            ["(for-of async b <empty>)"]
        );
        assert_eq!(
            statements(r"for (\u006cet.a of b);"),
            ["(for-of (. let a) b <empty>)"]
        );
    }

    #[test]
    fn a_binding_is_one_name_and_the_grammar_gives_it_no_initialiser() {
        // `ForBinding` is singular and has no `Initializer` in the grammar at all.
        assert_eq!(
            script_error("for (var a, b in c);").kind,
            ParseErrorKind::ForInOfBindsSeveralNames
        );
        assert_eq!(
            script_error("for (let a, b of c);").kind,
            ParseErrorKind::ForInOfBindsSeveralNames
        );
        for source in [
            "for (var a = 1 of b);",
            "for (let a = 1 in b);",
            "for (let a = 1 of b);",
            "for (const a = 1 of b);",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ForInOfBindingHasInitializer,
                "{source:?}"
            );
        }
        // Annex B.3.5 restores an initialiser to exactly one shape — `var`, `in`, non-strict —
        // and it is still refused here. Not for want of strictness, which this parser now has:
        // §B.3.5 assigns the initialiser *before* the head is evaluated, and that is the half the
        // compiler cannot yet express. The module doc says which line changes when it can.
        assert_eq!(
            script_error("for (var a = 1 in b);").kind,
            ParseErrorKind::ForInOfBindingHasInitializer
        );
        // A `const` with no initialiser is fine here and nowhere else, the binding coming from
        // the iteration rather than from an `=`.
        assert!(parse_script("for (const a of b);").is_ok());
        assert!(parse_script("for (const a in b);").is_ok());
        // A pattern is the other `ForBinding` alternative, and takes no initialiser either —
        // the value comes from the iteration whatever shape it is taken apart into.
        assert!(parse_script("for (let [a] of b);").is_ok());
        assert!(parse_script("for (var {a} in b);").is_ok());
        assert_eq!(
            script_error("for (let [a] = 1 of b);").kind,
            ParseErrorKind::ForInOfBindingHasInitializer
        );
    }

    #[test]
    fn the_header_is_a_scope_between_the_enclosing_one_and_the_body() {
        // §14.7.5.1, word for word the rule §14.7.4.1 states for the three-part form.
        for source in [
            "for (let a in b) { var a; }",
            "for (let a of b) { var a; }",
            "for (const a in b) { var a; }",
            "for (let a in b) var a;",
            "for (let a of b) { { if (x) var a; } }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                "{source:?}"
            );
        }
        assert!(parse_script("for (let a in b) { let a; }").is_ok());
        assert!(parse_script("for (var a in b) { var a; }").is_ok());
        assert!(parse_script("let a; for (let a in b);").is_ok());
        assert_eq!(
            script_error("let a; for (var a in b);").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        // Both are iteration statements, so both take `break` and `continue`.
        assert!(parse_script("for (a in b) break;").is_ok());
        assert!(parse_script("for (a of b) continue;").is_ok());
        assert!(parse_script("for (a of b) { switch (x) { case 1: continue; } }").is_ok());
        assert_eq!(
            script_error("for (a in b) {} continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
    }

    #[test]
    fn no_header_however_truncated_can_panic() {
        let cases = [
            "for (a in".to_string(),
            "for (a of".to_string(),
            "for (var a in".to_string(),
            "for (let a of".to_string(),
            "for (of".to_string(),
            "for (in".to_string(),
            "for (a in b) ".repeat(1000),
            "for (a of b) ".repeat(1000),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        assert_eq!(
            script_error(&"for (a of b) ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
