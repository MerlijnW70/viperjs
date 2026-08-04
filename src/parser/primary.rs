//! `PrimaryExpression` (ECMAScript §13.2) — the operands everything else is built from.
//!
//! Kept out of [`super::expression`] and [`super::member`] because most of what is here cannot
//! recurse, and a debug build gives every local its own stack slot whether its arm runs or not —
//! so a literal's locals would be paid for by every level of nesting that passes through. Moving
//! them out roughly halved the stack a deep parse needs, which is not a speed argument: it is how
//! many legitimate programs [`super::MAX_NESTING_DEPTH`] can afford to accept.
//!
//! The two forms that do contain expressions have files of their own —
//! [`super::array_literal`] and [`super::object_literal`] — for the same reason.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{BigIntLiteral, Expr, ExprKind, RegExpLiteral};
use crate::lexer::{
    Goal, ReservedWord, Token, TokenKind, bigint_digits, identifier_value, numeric_value,
    regexp_parts, string_value,
};
use crate::span::Span;

impl Parser<'_> {
    /// `PrimaryExpression` (§13.2), for the forms that need no other production.
    ///
    /// Only the one recursive production lives in this frame. Everything else is next door in
    /// [`Parser::parse_atom`], because a debug build gives every local in a function its own
    /// stack slot and does not reuse them between match arms — so an arm that never recurses
    /// still costs its slots once per level of nesting. Moving them out roughly halved the stack
    /// a deep parse needs, which is not a speed argument: it is how many legitimate programs
    /// [`MAX_NESTING_DEPTH`] can afford to accept.
    pub(super) fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.current;
        if token.kind != TokenKind::LParen {
            return self.parse_atom(token);
        }
        // `( Expression )`. The `(` is advanced past under `Goal::RegExp` because an operand
        // follows it, and the `)` under `Goal::Div` because the bracketed expression is one.
        self.advance(Goal::RegExp)?;
        self.enter()?;
        let inner = self.parse_expression(AllowIn::Yes);
        self.leave();
        // The inner failure is reported before the missing `)` is looked for, and the order
        // matters: whatever went wrong inside the brackets happened first and is what the reader
        // needs to see. Checking for the closing bracket first turns every error inside a
        // bracketed expression into "expected `)`" — including, absurdly, the depth cap.
        let inner = inner?;
        let close = self.eat(TokenKind::RParen, Goal::Div, "`)`")?;
        Ok(inner.in_parentheses(token.span.to(close.span)))
    }

    /// The `PrimaryExpression` forms that contain no other expression.
    fn parse_atom(&mut self, token: Token) -> Result<Expr, ParseError> {
        // …except this one, which does — but which opens a bracket of its own, so it recurses
        // through its own frame rather than adding one to every atom.
        if token.kind == TokenKind::LBracket {
            return self.parse_array_literal();
        }
        // Reached only where an expression may begin, so §14.5's restriction has already taken
        // any `{` that could have been a block — which is what keeps the two apart.
        if token.kind == TokenKind::LBrace {
            return self.parse_object_literal();
        }
        if matches!(token.kind, TokenKind::Template { .. }) {
            return self.parse_template(super::template::Tagged::No);
        }
        if token.kind == TokenKind::Keyword(ReservedWord::Function) {
            return self.parse_function_expression(false);
        }
        if self.at_async_function()? {
            return self.parse_function_expression(true);
        }
        if token.kind == TokenKind::Keyword(ReservedWord::Class) {
            return self.parse_class_expression();
        }
        // §13.3: `super` is not a `PrimaryExpression` — it is the head of a `SuperProperty`
        // or a `SuperCall` and of nothing else, so what may follow it is part of the
        // question of whether it may stand here at all.
        if token.kind == TokenKind::Keyword(ReservedWord::Super) {
            return self.parse_super();
        }
        let literal = |kind| Ok(Expr::new(kind, token.span));
        match token.kind {
            TokenKind::Keyword(ReservedWord::This) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::This)
            }
            TokenKind::Keyword(ReservedWord::Null) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Null)
            }
            TokenKind::Keyword(ReservedWord::True) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(true))
            }
            TokenKind::Keyword(ReservedWord::False) => {
                self.advance(Goal::Div)?;
                literal(ExprKind::Boolean(false))
            }
            // An `Identifier` is an `IdentifierName` that is not a `ReservedWord` — and the lexer
            // has already made that distinction, contextual keywords included.
            _ if self.is_identifier_token(token.kind) => {
                self.advance(Goal::Div)?;
                let name = identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                // §13.1.1's strict-reserved words, but not its `eval`/`arguments` rule: reading
                // either is fine in strict code, and only binding or assigning is refused.
                self.check_strict_name(&name, token.span, false)?;
                // §15.7.9's `ContainsArguments`, recorded rather than walked — see
                // [`Parser::arguments_reference`]. A *property* name never reaches here, so
                // `a.arguments` is not one of these and neither is `({arguments: 1})`.
                self.note_arguments(&name, token.span);
                literal(ExprKind::Identifier(name.into_owned()))
            }
            // Annex B.1.1's two legacy forms, which §12.9.3.1 refuses in strict code. The lexer
            // reads them and flags them, that being the lexical grammar's business.
            TokenKind::Number { legacy: true } if self.strict => Err(ParseError {
                kind: ParseErrorKind::StrictLegacyOctal,
                span: token.span,
            }),
            TokenKind::String {
                legacy_escape: true,
            } if self.strict => Err(ParseError {
                kind: ParseErrorKind::StrictLegacyOctal,
                span: token.span,
            }),
            // Sloppy here and possibly not sloppy by the end of the line. §12.9.4.1 refuses one in
            // strict code, and a **directive prologue** can turn strict on *after* this literal has
            // been read: `function f() { "\1"; "use strict"; }` is a Syntax Error and the escape is
            // two statements before the thing that makes it one. So the span is remembered and
            // [`Parser::parse_body_with_prologue`] judges it once the prologue has spoken.
            TokenKind::String {
                legacy_escape: true,
            } => {
                self.legacy_strings.push(token.span);
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::String(value))
            }
            TokenKind::Number { .. } => {
                self.advance(Goal::Div)?;
                let value = numeric_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::Number(value))
            }
            TokenKind::BigInt => {
                self.advance(Goal::Div)?;
                literal(ExprKind::BigInt(Box::new(self.bigint_literal(token)?)))
            }
            TokenKind::String { .. } => {
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                literal(ExprKind::String(value))
            }
            TokenKind::RegExp => {
                self.advance(Goal::Div)?;
                let parts = regexp_parts(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                let text = |span: Span| span.slice(self.source).unwrap_or_default().to_string();
                literal(ExprKind::RegExp(Box::new(RegExpLiteral {
                    body: text(parts.body),
                    flags: text(parts.flags),
                })))
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    /// The error for a token whose value the lexer produced but this parser cannot read back.
    ///
    /// Unreachable in principle — the value functions accept every span the lexer hands out — but
    /// the types do not say so, and the alternative to an error here is an `unwrap` that DR-0002
    /// forbids. It reports the token as unexpected, which is what it has become.
    /// The [`BigIntLiteral`] a `TokenKind::BigInt` token holds.
    ///
    /// Shared by the two places §12.9.3's `BigIntLiteral` can appear — as a `PrimaryExpression`
    /// and as a `PropertyName` — because they build the same node from the same token and only
    /// the surrounding production differs.
    pub(super) fn bigint_literal(&self, token: Token) -> Result<BigIntLiteral, ParseError> {
        let (radix, digits) =
            bigint_digits(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
        Ok(BigIntLiteral {
            radix,
            digits: digits.into_boxed_str(),
        })
    }

    pub(super) fn value_missing(&self, token: Token) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected: "a literal this parser can read",
                found: token.kind,
            },
            span: token.span,
        }
    }
}

/// Whether an `AssignmentExpression` may begin with this token.
///
/// One place needs this: `YieldExpression : yield` competes with
/// `yield [no LineTerminator here] AssignmentExpression`, and one token has to settle which. Every
/// other optional-operand form in the grammar is decided by a line terminator or a `;` instead,
/// which is why this predicate has waited until §15.5 to be needed at all.
///
/// The match is exhaustive on purpose — no catch-all arm. This has to agree with
/// [`Parser::parse_primary`] and [`Parser::parse_unary`], and the two are in different files; an
/// arm-less `_ => false` would let a new token kind quietly become "not an expression", which for
/// `yield` means silently reading `yield <thing>` as two statements. A compile error is the only
/// reminder that survives.
pub(super) fn begins_an_expression(kind: TokenKind) -> bool {
    match kind {
        // Operands.
        TokenKind::Identifier { .. }
        | TokenKind::PrivateIdentifier { .. }
        | TokenKind::Number { .. }
        | TokenKind::String { .. }
        | TokenKind::Template { .. }
        | TokenKind::RegExp
        | TokenKind::BigInt
        | TokenKind::LParen
        | TokenKind::LBracket
        | TokenKind::LBrace => true,
        // The prefix operators of §13.4 and §13.5, which `parse_unary` reads before an operand.
        TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Bang
        | TokenKind::Tilde
        | TokenKind::PlusPlus
        | TokenKind::MinusMinus => true,
        // The words that are operands or prefix operators rather than statements. `yield` and
        // `await` are here because §13.1 makes each an `IdentifierReference` where its parameter
        // is unset, and a `YieldExpression` where it is set — either way something begins.
        TokenKind::Keyword(word) => matches!(
            word,
            ReservedWord::This
                | ReservedWord::Null
                | ReservedWord::True
                | ReservedWord::False
                | ReservedWord::Function
                | ReservedWord::Class
                | ReservedWord::New
                | ReservedWord::Super
                | ReservedWord::Import
                | ReservedWord::Typeof
                | ReservedWord::Void
                | ReservedWord::Delete
                | ReservedWord::Yield
                | ReservedWord::Await
        ),
        // Everything else closes something, separates something, or is an infix operator — none
        // of which any expression may start with.
        TokenKind::Eof
        | TokenKind::RBrace
        | TokenKind::RParen
        | TokenKind::RBracket
        | TokenKind::Dot
        | TokenKind::DotDotDot
        | TokenKind::Semicolon
        | TokenKind::Comma
        | TokenKind::Colon
        | TokenKind::Arrow
        | TokenKind::QuestionDot
        | TokenKind::Question
        | TokenKind::Lt
        | TokenKind::Gt
        | TokenKind::LtEq
        | TokenKind::GtEq
        | TokenKind::EqEq
        | TokenKind::BangEq
        | TokenKind::EqEqEq
        | TokenKind::BangEqEq
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::StarStar
        | TokenKind::LtLt
        | TokenKind::GtGt
        | TokenKind::GtGtGt
        | TokenKind::Amp
        | TokenKind::Pipe
        | TokenKind::Caret
        | TokenKind::AmpAmp
        | TokenKind::PipePipe
        | TokenKind::QuestionQuestion
        | TokenKind::Eq
        | TokenKind::PlusEq
        | TokenKind::MinusEq
        | TokenKind::StarEq
        | TokenKind::SlashEq
        | TokenKind::PercentEq
        | TokenKind::StarStarEq
        | TokenKind::LtLtEq
        | TokenKind::GtGtEq
        | TokenKind::GtGtGtEq
        | TokenKind::AmpEq
        | TokenKind::PipeEq
        | TokenKind::CaretEq
        | TokenKind::AmpAmpEq
        | TokenKind::PipePipeEq
        | TokenKind::QuestionQuestionEq => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::test_support::*;
    #[test]
    fn every_primary_expression_the_grammar_reaches_today() {
        assert_eq!(parse("this").kind, ExprKind::This);
        assert_eq!(parse("null").kind, ExprKind::Null);
        assert_eq!(parse("true").kind, ExprKind::Boolean(true));
        assert_eq!(parse("false").kind, ExprKind::Boolean(false));
        assert_eq!(parse("1").kind, ExprKind::Number(1.0));
        assert_eq!(parse("0x10").kind, ExprKind::Number(16.0));
        assert_eq!(parse("1e3").kind, ExprKind::Number(1000.0));
        assert_eq!(parse("'hi'").kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse(r#""hi""#).kind, ExprKind::String(vec![0x68, 0x69]));
        assert_eq!(parse("x").kind, ExprKind::Identifier("x".to_string()));
        // The value is the cooked one, so an escaped name and a plain one give the same node.
        assert_eq!(parse(r"x").kind, ExprKind::Identifier("x".to_string()));
        // Contextual keywords are identifiers, which is the whole reason the lexer refused to
        // decide: `let` and `of` are ordinary names until a grammatical context says otherwise.
        assert_eq!(parse("let").kind, ExprKind::Identifier("let".to_string()));
        assert_eq!(parse("of").kind, ExprKind::Identifier("of".to_string()));
        assert_eq!(
            parse("async").kind,
            ExprKind::Identifier("async".to_string())
        );
        // …while a genuine reserved word is not an expression at all.
        assert_eq!(
            error("var").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::Keyword(ReservedWord::Var),
            }
        );
        // Spans cover exactly the construct.
        assert_eq!(parse("  1  ").span, Span::new(2, 3));
        assert_eq!(parse("this").span, Span::new(0, 4));
    }
    #[test]
    fn a_bigint_literal_keeps_its_digits_rather_than_becoming_a_number() {
        // §12.9.3: `BigIntLiteral :: NumericLiteralBase BigIntLiteralSuffix`. The node holds what
        // was written, radix and all, because the value it denotes has no type here until M7.
        assert_eq!(shape("1n"), "1n");
        assert_eq!(shape("0n"), "0n");
        assert_eq!(shape("0b101n"), "0b101n");
        assert_eq!(shape("0o17n"), "0o17n");
        assert_eq!(shape("0x1Fn"), "0x1Fn");
        assert_eq!(shape("1_000n"), "1000n");
        // The one that says why this is not an `f64`: 2^53+1, which a Number cannot hold and
        // would silently become 9007199254740992.
        assert_eq!(shape("9007199254740993n"), "9007199254740993n");
        assert_eq!(parse("  1n  ").span, Span::new(2, 4));
        // An operand like any other, and specifically not an assignment target — §13.4.2.1's
        // early error, which is about `AssignmentTargetType` and does not care that the operand
        // is a literal of a new kind.
        assert_eq!(shape("-1n"), "(- 1n)");
        assert_eq!(shape("1n + 2n"), "(+ 1n 2n)");
        assert_eq!(shape("1n.toString()"), "(call (. 1n toString) [])");
        assert!(matches!(
            error("1n++").kind,
            ParseErrorKind::InvalidAssignmentTarget
        ));
    }
    #[test]
    fn a_slash_at_the_start_of_an_expression_opens_a_literal() {
        // The goal symbol, from the other side of the handoff. `Parser::new` reads the first
        // token under `Goal::RegExp` because a program begins where an operand may stand — so
        // this is a regular expression and not the start of a division.
        assert_eq!(parse("/ab+/gi").kind, regexp("ab+", "gi"));
        // The escaped slash and the character class stay in the body, since the lexer found the
        // real closing slash.
        assert_eq!(parse(r"/a\/[/]b/").kind, regexp(r"a\/[/]b", ""));
        // Empty flags are an empty string rather than a missing one.
        assert_eq!(parse("/x/").kind, regexp("x", ""));
        // …and inside parentheses, where an operand may also stand.
        assert!(matches!(parse("(/x/)").kind, ExprKind::RegExp(_)));
    }
    #[test]
    fn a_slash_after_an_operand_divides_and_the_next_one_may_still_open_a_literal() {
        // The other half of the goal invariant, now that there is a binary expression to parse
        // the division into. `a /b/ g` is two divisions, and it is the parser's choice of goal
        // that makes it so — a lexer guessing from the previous token would have to get this
        // right by luck.
        assert_eq!(shape("a / b"), "(/ a b)");
        assert_eq!(shape("a /b/ g"), "(/ (/ a b) g)");
        // An operator is followed by an operand, so the goal after one is `RegExp` again: this
        // really is `a` divided by a regular expression, which is legal and rare.
        assert_eq!(shape("a / /b/"), "(/ a /b/)");
        assert_eq!(shape("typeof /b/"), "(typeof /b/)");
        assert_eq!(shape("1 + /b/g"), "(+ 1 /b/g)");
    }
    #[test]
    fn parentheses_are_recorded_without_becoming_a_node() {
        let bracketed = parse("(1)");
        assert_eq!(bracketed.kind, ExprKind::Number(1.0));
        assert!(bracketed.parenthesized);
        assert_eq!(
            bracketed.span,
            Span::new(0, 3),
            "the span covers the brackets"
        );
        // …and the same expression without them is not marked.
        assert!(!parse("1").parenthesized);
        // Nesting them changes only the span: no rule counts brackets.
        let twice = parse("((1))");
        assert!(twice.parenthesized);
        assert_eq!(twice.kind, ExprKind::Number(1.0));
        assert_eq!(twice.span, Span::new(0, 5));
        assert_eq!(parse(" ( 1 ) ").span, Span::new(1, 6));
        // An empty pair is not an expression — `()` is a production of the cover grammar and of
        // nothing else, so it means something only when a `=>` follows it. That is the assignment
        // level's business now; see [`super::arrow`].
        assert_eq!(
            error("()").kind,
            ParseErrorKind::CoverGroupIsNotAnExpression
        );
        assert_eq!(shape("() => 1"), "(=> [] 1)");
        // …while a `(` reached from the operand path is the plain parenthesized production, an
        // arrow being an `AssignmentExpression` and no operand at all.
        assert_eq!(
            error("-()").kind,
            ParseErrorKind::Unexpected {
                expected: "an expression",
                found: TokenKind::RParen,
            }
        );
        assert_eq!(
            error("(1").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Eof,
            }
        );
        assert_eq!(
            error("(1 2)").kind,
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::Number { legacy: false },
            }
        );
    }
}
