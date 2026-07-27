//! Statements (ECMAScript §14), and automatic semicolon insertion (§12.10).
//!
//! # Where the semicolons come from
//!
//! §12.10 gives three rules, and this file implements the first two directly; the third is the
//! restricted productions, which each enforce themselves where they are parsed — the `++` of
//! [`super::expression`] was the first.
//!
//! 1. A token that no production allows gets a semicolon inserted before it if it is on a new
//!    line, or is a `}`, or would close a `do`-`while`.
//! 2. At the end of input, if the script is not yet complete, a semicolon is inserted.
//! 3. A restricted token on a new line gets one before it.
//!
//! There is an overriding condition that no semicolon is ever inserted where it would become an
//! empty statement, or one of the two in a `for` header. Neither can arise here, because
//! [`Parser::consume_semicolon`] is only ever called where a semicolon *terminates* something,
//! never where one would *be* a statement. That is why `while (a)` at the end of input is an
//! error rather than a loop with an empty body, and why nothing checks for it — see
//! [`super::control`], which is where the first half of that became load-bearing.
//!
//! # What ASI does not do
//!
//! It does not join lines, and it does not break them. `a = b` followed by a line starting with
//! `(` or `[` is one expression, because the `(` *is* allowed by a production and rule 1 never
//! fires. The same is true of a line starting with `/`, which divides — and that case is decided
//! by the goal symbol rather than here: a token that ends an operand is followed by one read
//! under [`Goal::Div`], so the slash is division before ASI is ever consulted.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{DeclarationKind, Script, Stmt, StmtKind};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::span::Span;

/// Parse `source` as a `Script` (§16.1).
///
/// ```
/// use praxis::ast::StmtKind;
/// use praxis::parser::parse_script;
///
/// let script = parse_script("a = 1\nb = 2").expect("this parses");
/// assert_eq!(script.body.len(), 2, "a line break ends a statement");
/// assert!(matches!(script.body[0].kind, StmtKind::Expression(_)));
/// ```
pub fn parse_script(source: &str) -> Result<Script, ParseError> {
    let script = parse_script_before_label_rules(source)?;
    // §16.1.1's other five rules, about labels and the jumps that name them. Apart from the rest
    // because the walk that answers them has its own tests, and those need a tree that broke the
    // rules — which is the one thing this function will not hand back.
    super::scope::check_labels(&script.body)?;
    Ok(script)
}

/// [`parse_script`] up to but not including §16.1.1's label rules.
fn parse_script_before_label_rules(source: &str) -> Result<Script, ParseError> {
    let mut parser = Parser::new(source)?;
    let body = parser.parse_statement_list(TokenKind::Eof)?;
    parser.expect_eof()?;
    // §16.1.1 states the same two rules about a Script that §14.2.1 states about a Block.
    super::scope::check_declared_names(&body, super::scope::Level::Top)?;
    Ok(Script {
        body,
        span: Span::new(0, source.len() as u32),
    })
}

/// A `Script` that may break §16.1.1's label rules, for the tests of the walk that finds them.
#[cfg(test)]
pub(crate) fn parse_script_with_label_rules_unchecked(source: &str) -> Result<Script, ParseError> {
    parse_script_before_label_rules(source)
}

impl Parser<'_> {
    /// `StatementList` (§14.2), stopping at `terminator` without consuming it.
    pub(super) fn parse_statement_list(
        &mut self,
        terminator: TokenKind,
    ) -> Result<Box<[Stmt]>, ParseError> {
        let mut body = Vec::new();
        while self.current.kind != terminator && self.current.kind != TokenKind::Eof {
            body.push(self.parse_statement_list_item()?);
        }
        Ok(body.into_boxed_slice())
    }

    /// `StatementListItem : Statement | Declaration` (§14.2).
    ///
    /// The wider of the two, and the reason they are separate functions: only a `StatementList`
    /// admits a `Declaration`. A body — of an `if`, of a loop — is a `Statement`, and `Statement`
    /// has no `Declaration` alternative, so `if (a) let b = 1;` has no derivation while
    /// `if (a) var b = 1;` does. That asymmetry is in the grammar rather than in any early error
    /// about it, and this is where it lives.
    pub(super) fn parse_statement_list_item(&mut self) -> Result<Stmt, ParseError> {
        // A `FunctionDeclaration` is a `Declaration`, so it stands here and not among the
        // statements below — `while (x) function f() {}` has no derivation.
        if self.current.kind == TokenKind::Keyword(ReservedWord::Function) {
            return self.parse_function_declaration();
        }
        if self.current.kind == TokenKind::Keyword(ReservedWord::Const) {
            return self.parse_declaration(DeclarationKind::Const);
        }
        // `let` is the only one that needs looking at what follows it — see
        // [`Parser::at_lexical_let`]. It is also §14.5's restriction on `let [`, from the side
        // that knows what the brackets are for.
        if self.at_lexical_let()? {
            return self.parse_declaration(DeclarationKind::Let);
        }
        self.parse_statement()
    }

    /// `Statement` (§14.1), for the forms the parser reaches today.
    ///
    /// The order is the lookahead restriction of §14.5 doing its work: an `ExpressionStatement`
    /// may not begin with `{`, `function`, `async function`, `class`, or `let [`, each because
    /// it would be ambiguous with a statement or declaration form. Taking `{` as a block before
    /// an expression is ever considered is that restriction, spelled as control flow. The
    /// others arrive with the constructs they are ambiguous with; until then they are not
    /// expressions either, so nothing is silently misread.
    pub(super) fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        match self.current.kind {
            TokenKind::LBrace => self.parse_block(),
            TokenKind::Semicolon => {
                let token = self.advance(Goal::RegExp)?;
                Ok(Stmt {
                    kind: StmtKind::Empty,
                    span: token.span,
                })
            }
            TokenKind::Keyword(ReservedWord::Debugger) => {
                let token = self.advance(Goal::RegExp)?;
                let end = self.consume_semicolon(token.span)?;
                Ok(Stmt {
                    kind: StmtKind::Debugger,
                    span: token.span.to(end),
                })
            }
            // A `VariableStatement` is a `Statement`; a `LexicalDeclaration` is not. That is why
            // `var` is here and `let` and `const` are one level up.
            TokenKind::Keyword(ReservedWord::Var) => self.parse_declaration(DeclarationKind::Var),
            TokenKind::Keyword(ReservedWord::If) => self.parse_if(),
            TokenKind::Keyword(ReservedWord::While) => self.parse_while(),
            TokenKind::Keyword(ReservedWord::Do) => self.parse_do_while(),
            TokenKind::Keyword(ReservedWord::For) => self.parse_for(),
            TokenKind::Keyword(ReservedWord::Throw) => self.parse_throw(),
            TokenKind::Keyword(ReservedWord::Return) => self.parse_return(),
            TokenKind::Keyword(ReservedWord::Try) => self.parse_try(),
            TokenKind::Keyword(ReservedWord::Switch) => self.parse_switch(),
            TokenKind::Keyword(ReservedWord::With) => self.parse_with(),
            TokenKind::Keyword(ReservedWord::Break) => self.parse_break_or_continue(true),
            TokenKind::Keyword(ReservedWord::Continue) => self.parse_break_or_continue(false),
            // An identifier and a `:` is a `LabelledStatement`, which is the second and last
            // place this parser needs two tokens — and, like `let`, a case where one token
            // begins two productions.
            // §14.5: an `ExpressionStatement` may not begin with `function`, because that is a
            // declaration — and a `Statement` has no `Declaration` alternative, so there is
            // nowhere here for one to go. `if (x) function f() {}` and `a: function f() {}` are
            // both refused, and Annex B.3.2 and §14.13.1 are what would let a web host take them.
            TokenKind::Keyword(ReservedWord::Function) => Err(ParseError {
                kind: ParseErrorKind::FunctionInStatementPosition,
                span: self.current.span,
            }),
            _ if self.at_labelled_statement()? => self.parse_labelled_statement(),
            _ => self.parse_expression_statement(),
        }
    }

    /// `Block : { StatementList_opt }` (§14.2), as its statement list and its span.
    ///
    /// Separate from [`Parser::parse_block`] because three of the four places a `Block` appears
    /// are not statements: the `try`, `catch` and `finally` of §14.15 each take a `Block`
    /// directly, and wrapping each in a [`StmtKind::Block`] would invent a scope the grammar does
    /// not have. Every one of them is a `Block`, though, so every one gets §14.2.1 — which is the
    /// reason the check lives here and not in the statement form.
    pub(super) fn parse_block_body(&mut self) -> Result<(Box<[Stmt]>, Span), ParseError> {
        let open = self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        self.enter()?;
        let body = self.parse_statement_list(TokenKind::RBrace);
        self.leave();
        let body = body?;
        let close = self.eat(TokenKind::RBrace, Goal::RegExp, "`}`")?;
        // §14.2.1, on the finished list — see `super::scope`.
        super::scope::check_declared_names(&body, super::scope::Level::Block)?;
        Ok((body, open.span.to(close.span)))
    }

    /// `Block` where a `Statement` is wanted (§14.2).
    fn parse_block(&mut self) -> Result<Stmt, ParseError> {
        let (body, span) = self.parse_block_body()?;
        Ok(Stmt {
            kind: StmtKind::Block(body),
            span,
        })
    }

    /// `ExpressionStatement : Expression ;` (§14.5).
    fn parse_expression_statement(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.parse_expression(AllowIn::Yes)?;
        let end = self.consume_semicolon(expr.span)?;
        Ok(Stmt {
            span: expr.span.to(end),
            kind: StmtKind::Expression(Box::new(expr)),
        })
    }

    /// Consume the semicolon that terminates a statement, or find that one was inserted.
    ///
    /// `previous` is the span of what the semicolon terminates, and is what comes back when
    /// there was no semicolon to consume — an inserted one has no source text, so the statement
    /// ends where its content did rather than at some invented position.
    ///
    /// The three conditions below are §12.10's rules 1 and 2, in the one place a statement can
    /// need them. Rule 1's third condition — the previous token being the `)` of a `do`-`while`
    /// — is not here but in [`Parser::parse_do_while`], because it is not a condition on the
    /// *offending* token at all: it makes that one semicolon unconditionally optional, with no
    /// line break required and nothing to be offended by.
    pub(super) fn consume_semicolon(&mut self, previous: Span) -> Result<Span, ParseError> {
        if self.current.kind == TokenKind::Semicolon {
            return Ok(self.advance(Goal::RegExp)?.span);
        }
        // Rule 1: the offending token is on a new line, or is a `}`. Rule 2: it is the end of
        // input. In all three the statement simply ends, and nothing is consumed — the token
        // belongs to whatever comes next.
        if self.current.newline_before
            || self.current.kind == TokenKind::RBrace
            || self.current.kind == TokenKind::Eof
        {
            return Ok(previous);
        }
        Err(ParseError {
            kind: ParseErrorKind::Unexpected {
                expected: "`;`",
                found: self.current.kind,
            },
            span: self.current.span,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::test_support::*;

    /// The statement kinds of `source`, rendered compactly.
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
    fn a_script_is_a_list_of_statements_and_an_empty_source_is_an_empty_one() {
        assert_eq!(statements(""), Vec::<String>::new());
        assert_eq!(statements("   \n  "), Vec::<String>::new());
        assert_eq!(statements("// just a comment"), Vec::<String>::new());
        assert_eq!(statements("a;"), ["a"]);
        assert_eq!(statements("a; b; c;"), ["a", "b", "c"]);
        // `;` on its own is an EmptyStatement, which is a statement — not the same thing as an
        // omitted semicolon, and it is why `if (a);` has a body at all.
        assert_eq!(statements(";"), ["<empty>"]);
        assert_eq!(statements(";;;"), ["<empty>", "<empty>", "<empty>"]);
        assert_eq!(statements("a;;"), ["a", "<empty>"]);
        assert_eq!(statements("debugger;"), ["debugger"]);
        assert_eq!(statements("debugger"), ["debugger"]);
        // The span of a statement covers its semicolon when one was written.
        let script = parse_script("ab;").unwrap_or_else(|err| panic!("{}", err.kind)); // the assertion needs the tree
        assert_eq!(script.body[0].span, Span::new(0, 3));
        let script = parse_script("ab").unwrap_or_else(|err| panic!("{}", err.kind)); // same
        assert_eq!(
            script.body[0].span,
            Span::new(0, 2),
            "an inserted semicolon has no source to include"
        );
    }

    #[test]
    fn a_block_is_a_statement_and_nests() {
        assert_eq!(statements("{}"), ["{}"]);
        assert_eq!(statements("{ a; }"), ["{a}"]);
        assert_eq!(statements("{ a; b; }"), ["{a b}"]);
        assert_eq!(statements("{{}}"), ["{{}}"]);
        assert_eq!(statements("{ a } { b }"), ["{a}", "{b}"]);
        // §14.5's lookahead restriction, as control flow: a `{` at the start of a statement is a
        // block, never an object literal. Without it the two would be ambiguous, which is the
        // reason the restriction exists.
        assert_eq!(statements("{}"), ["{}"], "a block, not an empty object");
        assert_eq!(
            script_error("{").kind,
            ParseErrorKind::Unexpected {
                expected: "`}`",
                found: TokenKind::Eof,
            }
        );
    }

    #[test]
    fn a_semicolon_is_inserted_at_a_line_break_a_closing_brace_and_the_end_of_input() {
        // §12.10 rule 1, first condition: the offending token is on a new line.
        assert_eq!(statements("a\nb"), ["a", "b"]);
        assert_eq!(statements("a\r\nb"), ["a", "b"]);
        assert_eq!(statements("a\u{2028}b"), ["a", "b"]);
        assert_eq!(
            statements("a /* \n */ b"),
            ["a", "b"],
            "§12.4: the comment is a line break"
        );
        // Rule 1, second condition: the offending token is `}`.
        assert_eq!(statements("{ a }"), ["{a}"]);
        assert_eq!(statements("{ a; b }"), ["{a b}"]);
        // Rule 2: the end of input.
        assert_eq!(statements("a"), ["a"]);
        assert_eq!(statements("a; b"), ["a", "b"]);
        // …and nowhere else. Two expressions on one line are not two statements, which is the
        // whole reason ASI is a set of conditions rather than "insert one wherever it helps".
        assert_eq!(
            script_error("a b").kind,
            ParseErrorKind::Unexpected {
                expected: "`;`",
                found: TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
        assert_eq!(script_error("a b").span, Span::new(2, 3));
        assert_eq!(statements("a /* no break */ ; b"), ["a", "b"]);
    }

    #[test]
    fn a_line_break_does_not_end_a_statement_that_can_continue() {
        // The hazard ASI is famous for, and the half of it people forget: a semicolon is only
        // inserted before a token that *no production allows*. A line starting with `(` or `[`
        // continues the expression above it, silently.
        assert_eq!(statements("a = b\n(c)"), ["(= a (call b [c]))"]);
        assert_eq!(statements("a = b\n[c]"), ["(= a ([] b c))"]);
        assert_eq!(statements("a\n.b"), ["(. a b)"]);
        assert_eq!(statements("a\n+ b"), ["(+ a b)"]);
        // A line starting with `/` divides, and that is decided before ASI is consulted: the
        // token after an operand is read under `Goal::Div`, so it is a slash and not a literal.
        assert_eq!(statements("a\n/b/g"), ["(/ (/ a b) g)"]);
        // …while a restricted production does break the line, because §12.10's rule 3 says so
        // even though `++` would otherwise be allowed right there.
        assert_eq!(statements("a\n++b"), ["a", "(pre++ b)"]);
        assert_eq!(statements("a\n--b"), ["a", "(pre-- b)"]);
        assert_eq!(statements("a++\nb"), ["(post++ a)", "b"]);
        // …and on one line there is no break, so nothing is inserted and `a++ b` is the error
        // it looks like. The two cases differ only in the newline, which is the whole point.
        assert_eq!(
            script_error("a ++ b").kind,
            ParseErrorKind::Unexpected {
                expected: "`;`",
                found: TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
    }

    #[test]
    fn the_statement_forms_not_yet_built_fail_where_they_will_one_day_parse() {
        // §14.5 forbids an ExpressionStatement from beginning with `function`, `async function`
        // or `class` as well as `{`, each because it would be ambiguous with a declaration. None
        // of those is an expression here either, so each fails rather than being misread — and
        // will start parsing the day its declaration form lands.
        for source in ["class C {}", "class C extends D {}"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // `function` and `return` used to be on that list. The first is a `Declaration` now, and
        // the second is a `Statement` — but only under `[+Return]`, so at the top of a script it
        // is still refused, for a reason the grammar states rather than for want of an
        // implementation.
        assert_eq!(statements("function f() {}"), ["(fn f [] {})"]);
        assert_eq!(
            script_error("return 1;").kind,
            ParseErrorKind::ReturnOutsideFunction
        );
        // The `let [` pin that stood here through three slices has come all the way round:
        // §14.5 forbids an ExpressionStatement from beginning with `let [` because that is a
        // lexical declaration, and now it is one.
        assert_eq!(statements("let [a] = b"), ["(let [a]=b)"]);
        assert_eq!(statements("var a = 1"), ["(var a=1)"]);
        assert_eq!(statements("let a = 1"), ["(let a=1)"]);
    }

    #[test]
    fn no_source_however_odd_can_make_the_statement_parser_panic() {
        // DR-0002, at the level above expressions.
        let cases = [
            String::new(),
            "{".repeat(10_000),
            "}".repeat(10_000),
            ";".repeat(10_000),
            "a\n".repeat(10_000),
            "{".repeat(100) + &"}".repeat(100),
            "a b c".to_string(),
            "debugger debugger".to_string(),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A long flat list of statements is a loop, so it is bounded by memory rather than by
        // MAX_NESTING_DEPTH — unlike a deeply nested block, which is not.
        assert_eq!(
            parse_script(&"a;".repeat(10_000))
                .map(|s| s.body.len())
                .ok(),
            Some(10_000)
        );
        assert_eq!(
            script_error(&"{".repeat(10_000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
