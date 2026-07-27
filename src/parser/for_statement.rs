//! The three-part `for` loop (ECMAScript §14.7.4).
//!
//! # The header is where `in` stops being an operator
//!
//! `for (a in b; ; )` and `for (a in b)` begin identically, and the second is a `for`-`in` loop,
//! so the first clause of a header is read under `[~In]` — see [`AllowIn`]. It is the only place
//! in the language that asks for it, and it is why the parameter exists at all.
//!
//! Its reach is exactly one clause. The other two are `Expression[+In]`, so `for (;a in b;)` is
//! fine, and any bracket inside the first clause starts afresh: `for ((a in b);;)`,
//! `for (a[b in c];;)` and `for (f(a in b);;)` all parse.
//!
//! # The header's semicolons cannot be inserted
//!
//! §12.10's overriding condition names two cases, and this is the second: no semicolon is ever
//! inserted where it would become one of the two in a `for` header. So `for (var a);` is a Syntax
//! Error even though `var a` followed by `)` would end a statement anywhere else, and the header
//! asks for a real `;` with [`Parser::eat`] rather than going through
//! [`Parser::consume_semicolon`]. That is also why a `for` head takes
//! [`Parser::parse_declarator_list`] rather than a whole declaration: a `LexicalDeclaration`
//! carries its own semicolon, and here that semicolon is the header's.
//!
//! # Three scopes, not one
//!
//! `for (let a;;) { let a; }` is fine and `for (let a;;) { var a; }` is not, and both follow from
//! the same fact: a lexical header declaration is its own scope, sitting between the enclosing one
//! and the body's. So the body may shadow it — that is what a nested scope is for — while a `var`
//! in the body belongs to the enclosing function and would land in the header's scope on the way
//! past. §14.7.4.1 states exactly that, and it is the one early error here.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    DeclarationKind, ForInOfKind, ForInOfTarget, ForInit, ForStatement, Stmt, StmtKind,
};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::span::Span;

impl Parser<'_> {
    /// `ForStatement` or `ForInOfStatement` (§14.7.4, §14.7.5), with the cursor on `for`.
    pub(super) fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let is_await = self.parse_for_await()?;
        self.eat(TokenKind::LParen, Goal::RegExp, "`(`")?;
        self.enter()?;
        let parts = self.parse_for_parts();
        self.leave();
        let (mut kind, end) = parts?;
        // §14.7.5 gives `for await` three alternatives and every one of them is a `for`-`of`:
        // there is no asynchronous enumeration of property keys, and nothing to await in a
        // three-part loop. Asked of the finished header rather than threaded through the four
        // functions that build one — the question is about which production won, and only the
        // finished header knows.
        if is_await {
            match &mut kind {
                StmtKind::ForInOf(statement) if statement.kind == ForInOfKind::Of => {
                    statement.is_await = true;
                }
                _ => {
                    return Err(ParseError {
                        kind: ParseErrorKind::ForAwaitMustBeForOf,
                        span: keyword.span,
                    });
                }
            }
        }
        Ok(Stmt {
            span: keyword.span.to(end),
            kind,
        })
    }

    /// The `await` of `for await`, if one is written (§14.7.5).
    ///
    /// No `[no LineTerminator here]`, so `for` and `await` may be on separate lines. The word is
    /// a reserved one, so there is nothing else it could be here — which is why an `await` with
    /// the parameter unset is refused by name rather than by the `(` that was expected.
    fn parse_for_await(&mut self) -> Result<bool, ParseError> {
        if self.current.kind != TokenKind::Keyword(ReservedWord::Await) {
            return Ok(false);
        }
        if !self.await_allowed {
            return Err(ParseError {
                kind: ParseErrorKind::ForAwaitOutsideAsync,
                span: self.current.span,
            });
        }
        self.advance(Goal::RegExp)?;
        Ok(true)
    }

    /// The header and body, apart so their locals are not carried by every level of nesting that
    /// passes through [`Parser::parse_for`].
    ///
    /// Three productions begin identically and nothing tells them apart until the token *after*
    /// the first clause: a `;` makes it a `ForStatement`, and `in` or `of` a `ForInOfStatement`.
    /// So the clause is parsed once — under `[~In]`, which is what keeps `for (a in b)` available
    /// to be a loop rather than a comparison — and the answer decides where it goes.
    fn parse_for_parts(&mut self) -> Result<(StmtKind, Span), ParseError> {
        if self.current.kind == TokenKind::Semicolon {
            self.advance(Goal::RegExp)?;
            return self.parse_three_part_tail(None);
        }
        let declared = match self.current.kind {
            TokenKind::Keyword(ReservedWord::Var) => Some(DeclarationKind::Var),
            TokenKind::Keyword(ReservedWord::Const) => Some(DeclarationKind::Const),
            // The `[lookahead ≠ let [ ]` restriction on the expression forms, from the side that
            // knows what the brackets are for — exactly as in a statement list.
            _ if self.at_lexical_let()? => Some(DeclarationKind::Let),
            _ => None,
        };
        match declared {
            Some(kind) => self.parse_declared_head(kind),
            None => self.parse_expression_head(),
        }
    }

    /// A header beginning with `var`, `let` or `const`.
    fn parse_declared_head(
        &mut self,
        kind: DeclarationKind,
    ) -> Result<(StmtKind, Span), ParseError> {
        // `[~In]`, which `Initializer[?In]` carries down — `for (var a = b in c;;)` has no
        // derivation, and that is what leaves the `in` for the `for`-`in` production to take.
        let (declaration, _) = self.parse_declarator_list(kind, AllowIn::No)?;
        let Some(operator) = self.for_in_of_operator() else {
            // Now that it is settled as a `LexicalDeclaration` or a `VariableDeclaration`
            // rather than a `ForDeclaration`, the rules about a missing initialiser apply —
            // `for (const a;;)` has nothing to be constant and `for (let [a];;)` nothing to take
            // apart, where `for (const [a] of b)` takes its value from the iteration.
            Self::check_declaration_initializers(&declaration)?;
            self.eat(TokenKind::Semicolon, Goal::RegExp, "`;`")?;
            return self.parse_three_part_tail(Some(ForInit::Declaration(Box::new(declaration))));
        };
        // §14.7.5: `ForBinding` is singular and the grammar gives it no `Initializer`. Both are
        // missing productions rather than early errors, which is why they are checked here rather
        // than by the declaration itself — where both are perfectly legal.
        let [binding] = &*declaration.declarators else {
            return Err(ParseError {
                kind: ParseErrorKind::ForInOfBindsSeveralNames,
                span: declaration.declarators[0].binding.span(),
            });
        };
        if let Some(initializer) = &binding.initializer {
            return Err(ParseError {
                kind: ParseErrorKind::ForInOfBindingHasInitializer,
                span: initializer.span,
            });
        }
        self.parse_for_in_of_tail(ForInOfTarget::Declaration(Box::new(declaration)), operator)
    }

    /// A header beginning with an expression, which is a `ForStatement` init or a `for`-`in`/`of`
    /// target depending on what follows it.
    fn parse_expression_head(&mut self) -> Result<(StmtKind, Span), ParseError> {
        // Both halves of §14.7.5's `[lookahead ∉ { let, async of }]`, noted before the expression
        // is read because afterwards the tokens are gone. They restrict the `for`-`of` target
        // only, so nothing is refused until the operator turns out to be `of`.
        let begins_with_let = self.at_contextual("let");
        self.enter()?;
        let expr = self.parse_expression(AllowIn::No);
        self.leave();
        let expr = expr?;
        let Some(operator) = self.for_in_of_operator() else {
            // §12.10: `eat`, never `consume_semicolon`. A semicolon that would become one of the
            // header's two is the one kind automatic insertion may not supply.
            self.eat(TokenKind::Semicolon, Goal::RegExp, "`;`")?;
            return self.parse_three_part_tail(Some(ForInit::Expression(Box::new(expr))));
        };
        // §13.15.1's carve-out again: a literal here is a pattern, refined the same way the
        // left of an `=` is. `for ([a] of b)` and `[a] = b` are the same rule twice.
        if Self::covers_a_pattern(&expr) {
            // Neither lookahead restriction can bite here: a pattern begins with `[` or `{`,
            // which is neither the token `let` nor the token `async`.
            let pattern = self.refine_to_pattern(expr)?;
            return self.parse_for_in_of_tail(
                ForInOfTarget::Expression(Box::new(crate::ast::AssignmentTarget::Pattern(pattern))),
                operator,
            );
        }
        // §14.7.5's other lookahead half, `[lookahead != let [`, and the only one still worth
        // code. `async of` used to be checked here too and no longer needs to be: §15.9's arrow
        // head commits on `async` followed by an identifier, so `for (async of b)` fails asking
        // for the `=>` that an arrow head owes — which is what the grammar says happens once the
        // for-of alternative is excluded and the head has to be an `Expression`. It is also why
        // `for (async of => 1;;)` parses: the three-part alternative was never restricted.
        if operator == ForInOfKind::Of && begins_with_let {
            return Err(ParseError {
                kind: ParseErrorKind::ForOfTargetBeginsWithLet,
                span: expr.span,
            });
        }
        self.check_for_in_of_target(&expr)?;
        self.parse_for_in_of_tail(
            ForInOfTarget::Expression(Box::new(crate::ast::AssignmentTarget::Simple(expr))),
            operator,
        )
    }

    /// The two remaining clauses and the body of a three-part `for`, with its init already read.
    fn parse_three_part_tail(
        &mut self,
        init: Option<ForInit>,
    ) -> Result<(StmtKind, Span), ParseError> {
        let test = self.parse_header_clause(TokenKind::Semicolon, "`;`")?;
        let update = self.parse_header_clause(TokenKind::RParen, "`)`")?;
        let body = self.parse_statement()?;
        if let Some(ForInit::Declaration(declaration)) = &init
            && declaration.kind.is_lexical()
        {
            // §14.7.4.1: none of the BoundNames of a header `LexicalDeclaration` may occur in the
            // VarDeclaredNames of the body. A `var` there belongs to the enclosing function, so
            // it passes straight through the header's scope on its way out — where a `let` of the
            // same name is already sitting.
            super::scope::check_header_against_body(declaration, std::slice::from_ref(&body))?;
        }
        let end = body.span;
        Ok((
            StmtKind::For(Box::new(ForStatement {
                init,
                test,
                update,
                body,
            })),
            end,
        ))
    }

    /// One of the two optional `Expression[+In]` clauses, and the token that ends it.
    ///
    /// Both are `[+In]`: the restriction is on the first clause and nowhere else, so
    /// `for (;a in b;)` and `for (;;a in b)` are ordinary relational expressions.
    fn parse_header_clause(
        &mut self,
        terminator: TokenKind,
        expected: &'static str,
    ) -> Result<Option<crate::ast::Expr>, ParseError> {
        if self.current.kind == terminator {
            self.advance(Goal::RegExp)?;
            return Ok(None);
        }
        self.enter()?;
        let expr = self.parse_expression(AllowIn::Yes);
        self.leave();
        let expr = expr?;
        self.eat(terminator, Goal::RegExp, expected)?;
        Ok(Some(expr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ParseErrorKind;
    use crate::parser::parse_script;
    use crate::parser::test_support::*;

    #[test]
    fn every_one_of_the_three_header_clauses_is_optional() {
        assert_eq!(statements("for (;;) ;"), ["(for ; ; ; <empty>)"]);
        assert_eq!(statements("for (a;;);"), ["(for a ; ; <empty>)"]);
        assert_eq!(statements("for (;b;);"), ["(for ; b ; <empty>)"]);
        assert_eq!(statements("for (;;c);"), ["(for ; ; c <empty>)"]);
        assert_eq!(statements("for (a; b; c) d;"), ["(for a b c d)"]);
        assert_eq!(statements("for (;;) { a; }"), ["(for ; ; ; {a})"]);
        // `for (;;)` is the endless loop, and it is endless because there is no test rather than
        // because a test is true.
        assert_eq!(statements("for (;;) break;"), ["(for ; ; ; break)"]);
        // All three are full `Expression`s, so a comma sequences rather than separating clauses.
        assert_eq!(
            statements("for (a, b; c, d; e, f);"),
            ["(for (, a b) (, c d) (, e f) <empty>)"]
        );
        // Exactly two semicolons: no more, no fewer.
        for source in [
            "for () ;",
            "for (;) ;",
            "for (;;;) ;",
            "for (a;b);",
            "for a;b;c;",
            "for (a b c);",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // …and a body, which the overriding condition of §12.10 will not invent.
        assert!(parse_script("for (;;)").is_err());
    }

    #[test]
    fn a_header_may_declare_with_any_of_the_three_keywords() {
        assert_eq!(
            statements("for (var i = 0; i < 9; i++);"),
            ["(for (var i=0) (< i 9) (post++ i) <empty>)"]
        );
        assert_eq!(
            statements("for (let i = 0;;);"),
            ["(for (let i=0) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (const i = 0;;);"),
            ["(for (const i=0) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (var a, b;;);"),
            ["(for (var a b) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (let a, b;;);"),
            ["(for (let a b) ; ; <empty>)"]
        );
        assert_eq!(statements("for (let a;;);"), ["(for (let a) ; ; <empty>)"]);
        // The declaration's early errors are the ones it always has.
        assert_eq!(
            script_error("for (const a;;);").kind,
            ParseErrorKind::ConstWithoutInitializer
        );
        assert_eq!(
            script_error("for (let a, a;;);").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("for (let let;;);").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        // `let` is still an identifier where it cannot begin a declaration, so the expression
        // form takes it — §14.5's `[lookahead ≠ let [ ]`, from the side that knows.
        assert_eq!(statements("for (let;;);"), ["(for let ; ; <empty>)"]);
        assert_eq!(
            statements("for (let = 1;;);"),
            ["(for (= let 1) ; ; <empty>)"]
        );
    }

    #[test]
    fn no_semicolon_is_ever_inserted_where_it_would_become_one_of_the_headers_two() {
        // §12.10's overriding condition, second case. `var a` followed by `)` would end a
        // statement anywhere else; here there is nothing to insert.
        assert_eq!(
            script_error("for (var a);").kind,
            ParseErrorKind::Unexpected {
                expected: "`;`",
                found: TokenKind::RParen,
            }
        );
        assert!(parse_script("for (let a);").is_err());
        assert!(parse_script("for (a);").is_err());
        // …not even across a line break, which is what would let one in anywhere else.
        assert!(parse_script("for (var a\n);").is_err());
        assert!(parse_script("for (a\n);").is_err());
        assert!(
            parse_script("for (var a\n;\n);").is_err(),
            "still only one of the two"
        );
        assert!(
            parse_script("for (var a\n;\n;\n);").is_ok(),
            "both written out is fine"
        );
    }

    #[test]
    fn in_is_not_an_operator_in_the_first_clause_and_is_everywhere_else() {
        // §13.10's `[+In]` gate, and the reason the parameter exists: `for (a in b)` has to stay
        // available to mean a `for`-`in` loop.
        assert!(parse_script("for (a in b;;);").is_err());
        assert!(parse_script("for (var a = b in c;;);").is_err());
        assert!(parse_script("for (a = b in c;;);").is_err());
        // The other two clauses are `Expression[+In]`, so there it is an ordinary operator.
        assert_eq!(
            statements("for (;a in b;);"),
            ["(for ; (in a b) ; <empty>)"]
        );
        assert_eq!(
            statements("for (;;a in b);"),
            ["(for ; ; (in a b) <empty>)"]
        );
        // …and inside the first clause, every bracket starts a fresh `Expression[+In]`. This is
        // the half of the rule that a flag on the parser would quietly get wrong.
        assert_eq!(
            statements("for ((a in b);;);"),
            ["(for (in a b) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (a[b in c];;);"),
            ["(for ([] a (in b c)) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (f(a in b);;);"),
            ["(for (call f [(in a b)]) ; ; <empty>)"]
        );
        assert_eq!(
            statements("for (var a = (b in c);;);"),
            ["(for (var a=(in b c)) ; ; <empty>)"]
        );
        // A conditional resets the parameter for its middle arm and propagates it for the last —
        // so the `in` here is legal, and one after the colon would not be.
        assert_eq!(
            statements("for (a ? b in c : d;;);"),
            ["(for (? a (in b c) d) ; ; <empty>)"]
        );
        assert!(parse_script("for (a ? b : c in d;;);").is_err());
        // `instanceof` is not gated, being the alternative §13.10 leaves alone.
        assert_eq!(
            statements("for (a instanceof b;;);"),
            ["(for (instanceof a b) ; ; <empty>)"]
        );
        // …and outside a header, nothing changed.
        assert_eq!(statements("a in b;"), ["(in a b)"]);
        assert_eq!(statements("while (a in b);"), ["(while (in a b) <empty>)"]);
    }

    #[test]
    fn the_header_is_a_scope_between_the_enclosing_one_and_the_body() {
        // §14.7.4.1, the one early error here: the BoundNames of a header LexicalDeclaration may
        // not occur in the VarDeclaredNames of the body. A `var` belongs to the enclosing
        // function, so it passes through the header's scope on its way out.
        assert_eq!(
            script_error("for (let a;;) { var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        for source in [
            "for (let a;;) var a;",
            "for (const a = 1;;) { var a; }",
            "for (let a;;) { { var a; } }",
            "for (let a;;) { if (x) var a; }",
            "for (let a;;) { try {} finally { var a; } }",
            "for (let a;;) { switch (x) { case 1: var a; } }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                "{source:?}"
            );
        }
        // A `let` in the body shadows the header instead, which is what a nested scope is for.
        assert!(parse_script("for (let a;;) { let a; }").is_ok());
        assert!(parse_script("for (let a;;) { const a = 1; }").is_ok());
        // A `var` header binds in the enclosing scope, so the rule does not apply to it — and
        // duplicate `var`s were never a problem.
        assert!(parse_script("for (var a;;) { var a; }").is_ok());
        // The header's own scope is not the enclosing one, so an outer `let` is not a clash…
        assert!(parse_script("let a; for (let a;;);").is_ok());
        // …while an outer `let` and a header `var` are, the `var` hoisting out to meet it.
        assert_eq!(
            script_error("let a; for (var a;;);").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("let a; for (;;) { var a; }").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
    }

    #[test]
    fn a_for_is_an_iteration_statement_for_break_and_for_continue_alike() {
        assert_eq!(statements("for (;;) break;"), ["(for ; ; ; break)"]);
        assert_eq!(statements("for (;;) continue;"), ["(for ; ; ; continue)"]);
        assert!(parse_script("for (;;) { switch (x) { case 1: continue; } }").is_ok());
        assert!(parse_script("for (;;) { for (;;) break; }").is_ok());
        // …and the count is restored on the way out, including when the body fails.
        assert_eq!(
            script_error("for (;;) {} break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert!(parse_script("for (;;) { @ }").is_err());
        assert_eq!(
            script_error("continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
    }

    #[test]
    fn no_for_however_truncated_can_panic() {
        let cases = [
            "for".to_string(),
            "for (".to_string(),
            "for (;".to_string(),
            "for (;;".to_string(),
            "for (;;)".to_string(),
            "for (var".to_string(),
            "for (let".to_string(),
            "for (var a".to_string(),
            "for (a in".to_string(),
            "for (;;) ".repeat(1000),
            format!("for ({};;);", "(".repeat(10_000)),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        assert_eq!(
            script_error(&"for (;;) ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
