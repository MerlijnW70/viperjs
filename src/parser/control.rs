//! Conditionals, loops, and the statements that jump out of them (ECMAScript §14.6 – §14.14).
//!
//! # A body is a `Statement`, not a `StatementListItem`
//!
//! `if (a) var b = 1;` parses and `if (a) let b = 1;` does not, and the difference is in the
//! grammar rather than in any rule about it: `IfStatement` takes a `Statement`, `Statement` has
//! no `Declaration` alternative, and a `VariableStatement` is a `Statement` while a
//! `LexicalDeclaration` is not. Only a `StatementList` admits both. So the two are separate
//! functions here, and body positions call the narrower one.
//!
//! # Two places automatic semicolon insertion earns its keep
//!
//! The `;` at the end of `do … while (…)` may be left out, and §12.10's rule 1(c) is written for
//! exactly that one case: "the previous token is `)` and the inserted semicolon would then be
//! parsed as the terminating semicolon of a do-while statement". Unlike the other conditions it
//! does not ask about line breaks at all, so `do a; while (b) c` is two statements on one line.
//!
//! The other place is the overriding condition — a semicolon is never inserted where it would
//! become an empty statement. `while (a)` at the end of input must be an error rather than a
//! loop with an empty body, and it is, without a check: semicolon insertion is only ever
//! consulted where a semicolon *terminates* something, and a body is parsed by asking for a
//! statement, which end of input is not.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{DoWhileStatement, IfStatement, Stmt, StmtKind, WhileStatement};
use crate::lexer::{Goal, ReservedWord, TokenKind};

impl Parser<'_> {
    /// `IfStatement` (§14.6).
    ///
    /// The dangling `else` resolves itself: the consequent is parsed by a recursive call, and
    /// that call takes any `else` it finds before this one looks — which is the "nearest `if`"
    /// rule the specification writes as `[lookahead ≠ else]` on the alternative without one.
    pub(super) fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let test = self.parse_parenthesized_test()?;
        self.enter()?;
        let branches = self.parse_if_branches();
        self.leave();
        let (consequent, alternate) = branches?;
        let end = alternate.as_ref().unwrap_or(&consequent).span;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::If(Box::new(IfStatement {
                test,
                consequent,
                alternate,
            })),
        })
    }

    /// The consequent and optional alternate of an `if`, apart so their locals are not carried
    /// by every level of nesting that passes through [`Parser::parse_if`].
    fn parse_if_branches(&mut self) -> Result<(Stmt, Option<Stmt>), ParseError> {
        let consequent = self.parse_statement()?;
        if self.current.kind != TokenKind::Keyword(ReservedWord::Else) {
            return Ok((consequent, None));
        }
        self.advance(Goal::RegExp)?;
        let alternate = self.parse_statement()?;
        Ok((consequent, Some(alternate)))
    }

    /// `WhileStatement : while ( Expression ) Statement` (§14.7.3).
    pub(super) fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let test = self.parse_parenthesized_test()?;
        self.enter()?;
        let body = self.parse_loop_body();
        self.leave();
        let body = body?;
        Ok(Stmt {
            span: keyword.span.to(body.span),
            kind: StmtKind::While(Box::new(WhileStatement { test, body })),
        })
    }

    /// `DoWhileStatement : do Statement while ( Expression ) ;` (§14.7.2).
    pub(super) fn parse_do_while(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        self.enter()?;
        let body = self.parse_loop_body();
        self.leave();
        let body = body?;
        self.eat(
            TokenKind::Keyword(ReservedWord::While),
            Goal::RegExp,
            "`while`",
        )?;
        let test = self.parse_parenthesized_test()?;
        // §12.10 rule 1(c). The semicolon is simply optional here, with no condition attached —
        // not "optional at a line break" as everywhere else — so `do a; while (b) c` puts two
        // statements on one line and is not the error it looks like.
        let end = if self.current.kind == TokenKind::Semicolon {
            self.advance(Goal::RegExp)?.span
        } else {
            test.span
        };
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::DoWhile(Box::new(DoWhileStatement { body, test })),
        })
    }

    /// A loop body, with the surrounding statement counted as an enclosing iteration.
    ///
    /// That count is what §14.8.1 and §14.9.1 ask about: a `continue` must be inside an
    /// `IterationStatement`, and a `break` inside one or a `switch`. Kept as a depth rather than
    /// a flag because loops nest, and restored on the way out even when the body fails to parse.
    pub(super) fn parse_loop_body(&mut self) -> Result<Stmt, ParseError> {
        self.iteration_depth += 1;
        let body = self.parse_statement();
        self.iteration_depth -= 1;
        body
    }

    /// `( Expression )`, the head shared by `if`, `while` and `do`-`while`.
    fn parse_parenthesized_test(&mut self) -> Result<crate::ast::Expr, ParseError> {
        self.eat(TokenKind::LParen, Goal::RegExp, "`(`")?;
        self.enter()?;
        let test = self.parse_expression(AllowIn::Yes);
        self.leave();
        let test = test?;
        self.eat(TokenKind::RParen, Goal::RegExp, "`)`")?;
        Ok(test)
    }

    /// `ThrowStatement : throw [no LineTerminator here] Expression ;` (§14.14).
    ///
    /// The restriction has teeth here that it does not have on `break` and `continue`: `throw`
    /// has no argument-less form, so a line break after it does not leave a shorter statement —
    /// it leaves one with no derivation at all. `throw\na;` is a Syntax Error, and the value on
    /// the next line is not thrown.
    pub(super) fn parse_throw(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        if self.current.newline_before {
            return Err(ParseError {
                kind: ParseErrorKind::NewlineAfterThrow,
                span: self.current.span,
            });
        }
        self.enter()?;
        let value = self.parse_expression(AllowIn::Yes);
        self.leave();
        let value = value?;
        let end = self.consume_semicolon(value.span)?;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::Throw(Box::new(value)),
        })
    }

    /// `BreakStatement` and `ContinueStatement` (§14.9, §14.8), without their label forms.
    ///
    /// The labelled forms are restricted productions — `break [no LineTerminator here]
    /// LabelIdentifier` — and arrive with labelled statements, which is what would give a label
    /// something to name. Until then the restriction needs no code: with no label to take, a
    /// name on the next line is simply the next statement, which is what the restriction says it
    /// should be.
    pub(super) fn parse_break_or_continue(&mut self, is_break: bool) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        // §14.8.1 and §14.9.1, which differ in exactly one word: a `break` may be inside "an
        // IterationStatement or a SwitchStatement", a `continue` only inside the first. Two
        // counts rather than one, because that difference is the whole rule.
        let enclosing = if is_break {
            self.iteration_depth + self.switch_depth
        } else {
            self.iteration_depth
        };
        if enclosing == 0 {
            return Err(ParseError {
                kind: if is_break {
                    ParseErrorKind::BreakOutsideLoop
                } else {
                    ParseErrorKind::ContinueOutsideLoop
                },
                span: keyword.span,
            });
        }
        let end = self.consume_semicolon(keyword.span)?;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: if is_break {
                StmtKind::Break
            } else {
                StmtKind::Continue
            },
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
    fn an_else_belongs_to_the_nearest_if_that_has_none() {
        assert_eq!(statements("if (a) b;"), ["(if a b)"]);
        assert_eq!(statements("if (a) b; else c;"), ["(if a b c)"]);
        assert_eq!(statements("if (a) {} else {}"), ["(if a {} {})"]);
        // The dangling else, resolved the usual way. Both readings are grammatical without the
        // `[lookahead ≠ else]` restriction, and they mean different things — so this is one of
        // the few places where getting it wrong changes what a program does rather than whether
        // it parses.
        assert_eq!(
            statements("if (a) if (b) c; else d;"),
            ["(if a (if b c d))"]
        );
        assert_eq!(
            statements("if (a) { if (b) c; } else d;"),
            ["(if a {(if b c)} d)"]
        );
        assert_eq!(
            statements("if (a) b; else if (c) d; else e;"),
            ["(if a b (if c d e))"]
        );
        assert_eq!(
            parse_script("if (a) b;").map(|s| s.body[0].span).ok(),
            Some(Span::new(0, 9))
        );
        // The head is a whole `Expression`, brackets and all.
        assert_eq!(statements("if (a, b) c;"), ["(if (, a b) c)"]);
        assert_eq!(statements("if (a = 1) c;"), ["(if (= a 1) c)"]);
        for source in ["if a b;", "if (a b;", "if (a);else", "if () a;", "else a;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // An empty statement is a legal body, and is how `if (a);` gets one at all.
        assert_eq!(statements("if (a);"), ["(if a <empty>)"]);
    }

    #[test]
    fn a_body_is_a_statement_so_var_may_stand_there_and_let_may_not() {
        // `IfStatement : if ( Expression ) Statement`, and `Statement` has no `Declaration`
        // alternative — only a `StatementList` admits both. A `VariableStatement` is a
        // `Statement`; a `LexicalDeclaration` is not.
        assert_eq!(statements("if (a) var b = 1;"), ["(if a (var b=1))"]);
        assert_eq!(statements("while (a) var b = 1;"), ["(while a (var b=1))"]);
        assert!(parse_script("if (a) let b = 1;").is_err());
        assert!(parse_script("if (a) const b = 1;").is_err());
        assert!(parse_script("while (a) let b = 1;").is_err());
        // …and in a block, which is a StatementList, both are fine.
        assert_eq!(statements("if (a) { let b = 1; }"), ["(if a {(let b=1)})"]);
        assert_eq!(statements("{ const b = 1; }"), ["{(const b=1)}"]);
        // `let` is still an identifier where it cannot be a declaration, so this is an
        // expression statement rather than an error.
        assert_eq!(statements("if (a) let;"), ["(if a let)"]);
    }

    #[test]
    fn both_loops_parse_and_the_do_while_semicolon_is_simply_optional() {
        assert_eq!(statements("while (a) b;"), ["(while a b)"]);
        assert_eq!(statements("while (a) {}"), ["(while a {})"]);
        assert_eq!(statements("do a; while (b);"), ["(do a b)"]);
        assert_eq!(statements("do { a; } while (b);"), ["(do {a} b)"]);
        // §12.10 rule 1(c): the semicolon after a do-while's `)` is optional with no condition
        // attached — not merely at a line break, as everywhere else. So this is two statements
        // on one line, which no other statement form allows.
        assert_eq!(statements("do a; while (b) c;"), ["(do a b)", "c"]);
        assert_eq!(statements("do a; while (b)"), ["(do a b)"]);
        assert_eq!(statements("do a; while (b)\nc;"), ["(do a b)", "c"]);
        // …and every other statement still needs its terminator.
        assert_eq!(
            script_error("a b").kind,
            ParseErrorKind::Unexpected {
                expected: "`;`",
                found: TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
        // The overriding condition of §12.10: no semicolon is inserted where it would become an
        // empty statement, so a loop with nothing after it is an error rather than a loop with
        // an empty body. Nothing checks for this — a body is parsed by asking for a statement,
        // and the end of input is not one.
        assert!(parse_script("while (a)").is_err());
        assert!(parse_script("if (a)").is_err());
        assert!(parse_script("do while (a);").is_err());
        // …while an explicit empty statement is a perfectly good body.
        assert_eq!(statements("while (a);"), ["(while a <empty>)"]);
    }

    #[test]
    fn break_and_continue_must_be_inside_a_loop() {
        // §14.8.1 and §14.9.1: a Syntax Error if not nested within an IterationStatement.
        assert_eq!(statements("while (a) break;"), ["(while a break)"]);
        assert_eq!(statements("while (a) continue;"), ["(while a continue)"]);
        assert_eq!(statements("do break; while (a);"), ["(do break a)"]);
        assert_eq!(
            statements("while (a) { if (b) break; }"),
            ["(while a {(if b break)})"]
        );
        // Nested loops keep the count right on the way back out.
        assert_eq!(
            statements("while (a) { while (b) break; } "),
            ["(while a {(while b break)})"]
        );
        assert_eq!(
            script_error("while (a) {} break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        assert_eq!(
            script_error("{ break; }").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("if (a) continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        assert_eq!(script_error("break;").span, Span::new(0, 5));
        // The count is restored even when the body fails, so a later legal `break` is not
        // wrongly accepted and a later illegal one is not wrongly allowed.
        assert!(parse_script("while (a) { @ }").is_err());
        assert_eq!(
            script_error("break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        // Semicolon insertion applies to them as to anything else.
        assert_eq!(statements("while (a) { break }"), ["(while a {break})"]);
        assert_eq!(
            statements("while (a) { break\nb; }"),
            ["(while a {break b})"]
        );
    }

    #[test]
    fn a_line_break_after_throw_leaves_a_statement_with_no_derivation() {
        assert_eq!(statements("throw a;"), ["(throw a)"]);
        assert_eq!(statements("throw new Error();"), ["(throw (new Error []))"]);
        assert_eq!(statements("throw a, b;"), ["(throw (, a b))"]);
        assert_eq!(statements("throw a"), ["(throw a)"], "…and ASI ends it");
        // `throw [no LineTerminator here] Expression` — and unlike `break` and `continue`, there
        // is no shorter form to fall back to. So the restriction does not end the statement
        // early; it leaves one that cannot be derived at all.
        assert_eq!(
            script_error("throw\na;").kind,
            ParseErrorKind::NewlineAfterThrow
        );
        assert_eq!(script_error("throw\n a;").span, Span::new(7, 8));
        assert_eq!(
            script_error("throw /* \n */ a;").kind,
            ParseErrorKind::NewlineAfterThrow,
            "§12.4: a comment containing a newline is a line terminator"
        );
        assert_eq!(statements("throw /* no break */ a;"), ["(throw a)"]);
        assert!(parse_script("throw;").is_err());
        assert!(parse_script("throw").is_err());
    }

    #[test]
    fn no_control_flow_however_odd_can_panic() {
        let cases = [
            "if".to_string(),
            "if (".to_string(),
            "while".to_string(),
            "do".to_string(),
            "do a".to_string(),
            "throw".to_string(),
            "break".to_string(),
            "if (a) ".repeat(1000),
            "while (a) ".repeat(1000),
            "do ".repeat(1000),
            "if (a) b; else ".repeat(1000),
            "{ break; }".repeat(1000),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // Nested bodies recurse, so they are bounded by the nesting cap rather than by memory.
        assert_eq!(
            script_error(&"if (a) ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
        assert_eq!(
            script_error(&"while (a) ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
