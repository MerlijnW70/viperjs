//! `switch` (ECMAScript §14.12).
//!
//! # A CaseBlock is one scope, and the clauses inside it are not scopes at all
//!
//! This is the whole reason `switch` needs anything said about it. `{ case 1: let a; case 2:
//! let a; }` looks like two declarations in two places and is a redeclaration, because §8.2.6
//! defines the `LexicallyDeclaredNames` of a `CaseBlock` as the concatenation across every
//! clause. Write the braces — `case 1: { let a; } case 2: { let a; }` — and the two are genuinely
//! separate. The clauses are labels to jump to, not bodies to enter.
//!
//! So §14.12.1 states the same two rules §14.2.1 states about a Block, over a list that is
//! stitched together from every clause. That stitching is the specification's own
//! list-concatenation, and [`super::scope`] does it that way rather than by walking a `Switch`.
//!
//! # `break` belongs here and `continue` does not
//!
//! §14.9.1 lets a `break` be inside "an IterationStatement or a SwitchStatement"; §14.8.1 gives
//! `continue` only the first. That asymmetry is the reason the parser counts two things instead
//! of one: a `switch` is somewhere to break out of, and never somewhere to continue.
//!
//! # At most one `default`, which the tree cannot say
//!
//! `CaseBlock : { CaseClauses_opt DefaultClause CaseClauses_opt }` — the grammar's way of
//! allowing one `default` anywhere among the clauses. The tree here keeps them in one flat list,
//! which is what every consumer wants and what evaluation order actually is, so the constraint
//! stops being structural and becomes a thing to check. It is still a missing production rather
//! than an early error, and it is named that way.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Stmt, StmtKind, SwitchCase, SwitchStatement};
use crate::lexer::{Goal, ReservedWord, TokenKind};

impl Parser<'_> {
    /// `SwitchStatement : switch ( Expression ) CaseBlock` (§14.12).
    pub(super) fn parse_switch(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        self.eat(TokenKind::LParen, Goal::RegExp, "`(`")?;
        self.enter()?;
        let discriminant = self.parse_expression(AllowIn::Yes);
        self.leave();
        let discriminant = discriminant?;
        self.eat(TokenKind::RParen, Goal::RegExp, "`)`")?;
        self.enter()?;
        // A `switch` is somewhere a `break` may leave from, and never somewhere a `continue` may
        // go — §14.9.1 against §14.8.1. Restored on the way out even when the block fails.
        self.switch_depth += 1;
        let cases = self.parse_case_block();
        self.switch_depth -= 1;
        self.leave();
        let (cases, end) = cases?;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::Switch(Box::new(SwitchStatement {
                discriminant,
                cases,
            })),
        })
    }

    /// `CaseBlock` (§14.12), returning its clauses and the span of the closing brace.
    fn parse_case_block(&mut self) -> Result<(Box<[SwitchCase]>, crate::span::Span), ParseError> {
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        let mut cases: Vec<SwitchCase> = Vec::new();
        let mut seen_default = false;
        while self.current.kind != TokenKind::RBrace {
            let case = self.parse_case_clause()?;
            if case.test.is_none() {
                // The grammar admits one `DefaultClause` between two runs of `CaseClauses`, so a
                // second one has no derivation. Reported against the clause itself: the first is
                // not the mistake, however far back it was.
                if seen_default {
                    return Err(ParseError {
                        kind: ParseErrorKind::MultipleDefaultClauses,
                        span: case.span,
                    });
                }
                seen_default = true;
            }
            cases.push(case);
        }
        let close = self.eat(TokenKind::RBrace, Goal::RegExp, "`}`")?;
        // §14.12.1, over the clauses stitched together — see `super::scope`.
        super::scope::check_case_block_declared_names(&cases)?;
        Ok((cases.into_boxed_slice(), close.span))
    }

    /// One `CaseClause` or `DefaultClause` (§14.12).
    fn parse_case_clause(&mut self) -> Result<SwitchCase, ParseError> {
        let (keyword, test) = match self.current.kind {
            TokenKind::Keyword(ReservedWord::Case) => {
                let keyword = self.advance(Goal::RegExp)?;
                self.enter()?;
                // `case Expression :` — a full `Expression`, so `case a, b:` is a sequence and
                // the comma does not separate two clauses.
                let test = self.parse_expression(AllowIn::Yes);
                self.leave();
                (keyword, Some(test?))
            }
            TokenKind::Keyword(ReservedWord::Default) => (self.advance(Goal::RegExp)?, None),
            _ => return Err(self.unexpected("`case` or `default`")),
        };
        self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
        let mut body = Vec::new();
        let mut end = keyword.span;
        // `StatementList_opt`, which ends where the next clause begins. A clause body is a
        // StatementList and not a Statement, so `case 1: let a;` is a declaration — one that
        // belongs to the whole CaseBlock rather than to this clause.
        while !matches!(
            self.current.kind,
            TokenKind::RBrace
                | TokenKind::Eof
                | TokenKind::Keyword(ReservedWord::Case)
                | TokenKind::Keyword(ReservedWord::Default)
        ) {
            let stmt = self.parse_statement_list_item()?;
            end = stmt.span;
            body.push(stmt);
        }
        Ok(SwitchCase {
            test,
            body: body.into_boxed_slice(),
            span: keyword.span.to(end),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;
    use crate::parser::test_support::*;
    use crate::span::Span;

    /// The statements of `source`, rendered compactly.
    fn statements(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // a test about a tree cannot proceed without one
        script.body.iter().map(render_statement).collect()
    }

    /// The error `source` fails with.
    fn script_error(source: &str) -> ParseError {
        match parse_script(source) {
            Err(err) => err,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn a_case_block_holds_clauses_and_nothing_else() {
        assert_eq!(statements("switch (x) {}"), ["(switch x)"]);
        assert_eq!(
            statements("switch (x) { case 1: }"),
            ["(switch x (case 1 {}))"]
        );
        assert_eq!(
            statements("switch (x) { case 1: a; }"),
            ["(switch x (case 1 {a}))"]
        );
        assert_eq!(
            statements("switch (x) { default: }"),
            ["(switch x (default {}))"]
        );
        assert_eq!(
            statements("switch (x) { case 1: a; default: b; case 2: c; }"),
            ["(switch x (case 1 {a}) (default {b}) (case 2 {c}))"]
        );
        // A clause with no statements falls through to the next, which is why the body is
        // optional rather than merely allowed to be empty.
        assert_eq!(
            statements("switch (x) { case 1: case 2: a; }"),
            ["(switch x (case 1 {}) (case 2 {a}))"]
        );
        // The head is a full `Expression`, and so is a `case` — `case a, b:` is a sequence, not
        // two clauses.
        assert_eq!(
            statements("switch (x, y) { case a, b: c; }"),
            ["(switch (, x y) (case (, a b) {c}))"]
        );
        // A statement outside a clause has nowhere to go.
        assert_eq!(
            script_error("switch (x) { a; }").kind,
            ParseErrorKind::Unexpected {
                expected: "`case` or `default`",
                found: TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
        for source in [
            "switch (x) case 1: a;",
            "switch x {}",
            "switch (x) { case: a; }",
            "switch (x) { case 1 a; }",
            "switch () {}",
            "switch (x) { default a; }",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        let script = parse_script("switch (x) {}").expect("this parses");
        assert_eq!(script.body[0].span, Span::new(0, 13));
    }

    #[test]
    fn a_switch_may_have_one_default_and_it_may_stand_anywhere() {
        assert!(parse_script("switch (x) { default: a; case 1: b; }").is_ok());
        assert!(parse_script("switch (x) { case 1: b; default: a; }").is_ok());
        assert!(parse_script("switch (x) { case 1: b; default: a; case 2: c; }").is_ok());
        // `CaseBlock : { CaseClauses_opt DefaultClause CaseClauses_opt }` admits exactly one, so
        // a second has no derivation.
        assert_eq!(
            script_error("switch (x) { default: a; default: b; }").kind,
            ParseErrorKind::MultipleDefaultClauses
        );
        assert_eq!(
            script_error("switch (x) { default: default: }").kind,
            ParseErrorKind::MultipleDefaultClauses
        );
        assert_eq!(
            script_error("switch (x) { default: a; case 1: b; default: c; }").kind,
            ParseErrorKind::MultipleDefaultClauses
        );
        // Reported against the second clause, not the first — the first was fine.
        let source = "switch (x) { default: default: }";
        assert_eq!(script_error(source).span, Span::new(22, 29));
        assert_eq!(
            script_error(source).span.slice(source),
            Some("default"),
            "the second one, and it has no body to extend over"
        );
        // A nested switch has a default of its own, and it is not the outer one's second.
        assert!(parse_script("switch (x) { default: switch (y) { default: } }").is_ok());
    }

    #[test]
    fn a_switch_is_somewhere_to_break_out_of_and_never_somewhere_to_continue() {
        // §14.9.1 admits "an IterationStatement or a SwitchStatement"; §14.8.1 admits only the
        // first. Two counts, because the two rules genuinely differ.
        assert_eq!(
            statements("switch (x) { case 1: break; }"),
            ["(switch x (case 1 {break}))"]
        );
        assert_eq!(
            script_error("switch (x) { case 1: continue; }").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        // …and a loop outside the switch supplies what the switch cannot.
        assert!(parse_script("while (y) { switch (x) { case 1: continue; } }").is_ok());
        assert!(parse_script("while (y) { switch (x) { case 1: break; } }").is_ok());
        assert!(parse_script("do { switch (x) { case 1: continue; } } while (y);").is_ok());
        // A switch does not make a `continue` legal after it, and neither count leaks.
        assert_eq!(
            script_error("switch (x) {} break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("switch (x) { case 1: } continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        assert_eq!(
            script_error("while (y) { switch (x) {} } break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        // …including when the block fails to parse, which is the case a stray `?` would break.
        assert!(parse_script("switch (x) { case 1: @ }").is_err());
        assert_eq!(
            script_error("break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
    }

    #[test]
    fn the_whole_case_block_is_one_scope_and_a_clause_is_not_a_scope_at_all() {
        // §14.12.1, over the `LexicallyDeclaredNames` of the CaseBlock — which §8.2.6 defines as
        // the concatenation across every clause. Two clauses are two labels, not two scopes.
        assert_eq!(
            script_error("switch (x) { case 1: let a; case 2: let a; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("switch (x) { case 1: let a; default: const a = 1; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("switch (x) { case 1: let a; case 2: var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("switch (x) { case 1: var a; case 2: let a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        // Write the braces and they really are separate — which is the half that shows the rule
        // is about scopes rather than about clauses.
        assert!(parse_script("switch (x) { case 1: { let a; } case 2: { let a; } }").is_ok());
        assert!(parse_script("switch (x) { case 1: let a; case 2: let b; }").is_ok());
        assert!(parse_script("switch (x) { case 1: var a; case 2: var a; }").is_ok());
        assert!(parse_script("switch (x) { case 1: let a; break; }").is_ok());
        // A `var` in any clause hoists out of the switch entirely.
        for source in [
            "let a; switch (x) { case 1: var a; }",
            "let a; switch (x) { case 1: { var a; } }",
            "let a; switch (x) { default: if (y) var a; }",
            "let a; switch (x) { case 1: while (y) var a; }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                "{source:?}"
            );
        }
        // …while a lexical name inside the block stays inside it.
        assert!(parse_script("let a; switch (x) { case 1: let b; }").is_ok());
        assert!(parse_script("var a; switch (x) { case 1: let a; }").is_ok());
    }

    #[test]
    fn no_switch_however_truncated_can_panic() {
        let cases = [
            "switch".to_string(),
            "switch (".to_string(),
            "switch (x".to_string(),
            "switch (x)".to_string(),
            "switch (x) {".to_string(),
            "switch (x) { case".to_string(),
            "switch (x) { case 1".to_string(),
            "switch (x) { case 1:".to_string(),
            "switch (x) { default".to_string(),
            "case 1:".to_string(),
            "default:".to_string(),
            "switch (x) { case 1: ".repeat(1000),
            format!("switch (x) {{ {} }}", "case 1: ".repeat(10_000)),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A long flat list of clauses is a loop, so it is bounded by memory; nesting is not.
        assert_eq!(
            parse_script(&format!("switch (x) {{ {} }}", "case 1: ".repeat(10_000)))
                .map(|script| script.body.len())
                .ok(),
            Some(1)
        );
        assert_eq!(
            script_error(&"switch (x) { case 1: ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
