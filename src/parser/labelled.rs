//! Labelled statements (ECMAScript §14.13).
//!
//! `LabelledStatement : LabelIdentifier : LabelledItem`, and a `LabelIdentifier` is an ordinary
//! `Identifier` — so telling one from an expression statement takes two tokens, the second of
//! which is a `:`. That is the second and last place this parser needs to look ahead; `let` was
//! the first, and both are cases where the same first token begins two productions.
//!
//! # What a label may be attached to
//!
//! `LabelledItem : Statement | FunctionDeclaration`. A `Statement`, so `a: var b;` is fine and
//! `a: let b;` is not — the same asymmetry every body position has, for the same reason. The
//! `FunctionDeclaration` alternative is §14.13.1's, and it is a Syntax Error unless the code is
//! non-strict and the host supports Labelled Function Declarations; there are no functions to
//! label yet, and when there are that rule needs strict mode to state.
//!
//! # Whether the label means anything is not decided here
//!
//! Every rule about labels — that one may not repeat inside another, that a `break` must name one
//! that exists, that a `continue` must name one on a loop — is a syntax-directed operation over
//! the finished tree (§8.3), and lives in [`crate::static_semantics`]. This file only reads them.
//!
//! That split is what let the parser stop counting. It used to carry an `iteration_depth` and a
//! `switch_depth` so that an unlabelled `break` could be refused where it stood, and those are
//! gone: §14.8.1 and §14.9.1 are questions about the tree, the tree can be asked, and a parser
//! that keeps its own answer to a question the tree can settle has two things to keep right.

use super::{ParseError, Parser};
use crate::ast::{Label, LabelledStatement, Stmt, StmtKind};
use crate::lexer::{Goal, TokenKind, identifier_value};

impl Parser<'_> {
    /// Whether a `LabelledStatement` begins here.
    ///
    /// An identifier and then a `:`. The identifier may be written with escapes — unlike `let`,
    /// nothing here is a terminal, so `a: ;` labels `a` exactly as `a: ;` does.
    pub(super) fn at_labelled_statement(&self) -> Result<bool, ParseError> {
        if !self.is_identifier_token(self.current.kind) {
            return Ok(false);
        }
        Ok(self.peek(Goal::Div)?.kind == TokenKind::Colon)
    }

    /// `LabelledStatement : LabelIdentifier : LabelledItem` (§14.13).
    pub(super) fn parse_labelled_statement(&mut self) -> Result<Stmt, ParseError> {
        let token = self.advance(Goal::Div)?;
        let name =
            identifier_value(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
        // `LabelIdentifier : Identifier`, so §13.1.1 applies here as it does to a reference.
        self.check_strict_name(&name, token.span, false)?;
        self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
        self.enter()?;
        // `LabelledItem : Statement`, so a declaration may not be labelled — `a: let b;` has no
        // derivation, for the reason `if (x) let b;` does not.
        let body = self.parse_statement();
        self.leave();
        let body = body?;
        Ok(Stmt {
            span: token.span.to(body.span),
            kind: StmtKind::Labelled(Box::new(LabelledStatement {
                label: Label {
                    name: name.into_owned().into_boxed_str(),
                    span: token.span,
                },
                body,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};
    use crate::span::Span;

    #[test]
    fn an_identifier_and_a_colon_begin_a_labelled_statement() {
        assert_eq!(statements("a: ;"), ["(label a <empty>)"]);
        assert_eq!(statements("a: b;"), ["(label a b)"]);
        assert_eq!(statements("a: { b; }"), ["(label a {b})"]);
        assert_eq!(statements("a: b: ;"), ["(label a (label b <empty>))"]);
        assert_eq!(
            statements("a: while (1) ;"),
            ["(label a (while 1 <empty>))"]
        );
        assert_eq!(statements("a: if (x) ;"), ["(label a (if x <empty>))"]);
        // The label is a `LabelIdentifier`, which is an `Identifier` — so the contextual keywords
        // are all available, and only a reserved word is not.
        assert_eq!(statements("let: ;"), ["(label let <empty>)"]);
        assert_eq!(statements("of: ;"), ["(label of <empty>)"]);
        assert_eq!(statements("async: ;"), ["(label async <empty>)"]);
        assert!(parse_script("if: ;").is_err());
        assert!(parse_script("this: ;").is_err());
        // `yield` and `await` belong in the list above and are there now. §13.1 gives
        // `LabelIdentifier` the alternatives `[~Yield] yield` and `[~Await] await`, and both
        // parameters are off in the sloppy script code this parser handles.
        assert_eq!(statements("yield: ;"), ["(label yield <empty>)"]);
        assert_eq!(statements("await: ;"), ["(label await <empty>)"]);
        assert_eq!(statements("var yield = 1;"), ["(var yield=1)"]);
        // Nothing here is a terminal, so an escaped spelling is the same label — unlike `let`,
        // where §5.1.5.1 makes the escape decide whether it is a keyword at all.
        assert_eq!(statements(r"a: ;"), ["(label a <empty>)"]);
        // …and a lone identifier is still an expression statement, the `:` being the whole test.
        assert_eq!(statements("a;"), ["a"]);
        assert_eq!(statements("a ? b : c;"), ["(? a b c)"]);
        let script = parse_script("ab: ;").expect("this parses");
        assert_eq!(script.body[0].span, Span::new(0, 5));
    }

    #[test]
    fn yield_and_await_are_ordinary_names_where_no_production_reserves_them() {
        // §13.1's conditional alternatives. `Identifier : IdentifierName but not ReservedWord`
        // would refuse both — `yield` and `await` are reserved words in §12.7.2 — and all three
        // identifier productions add them back, two of them gated on grammar parameters that
        // nothing here can turn on. See [`super::is_identifier_token`].
        assert_eq!(statements("yield;"), ["yield"]);
        assert_eq!(statements("await;"), ["await"]);
        assert_eq!(statements("yield = 1;"), ["(= yield 1)"]);
        assert_eq!(statements("typeof yield;"), ["(typeof yield)"]);
        assert_eq!(statements("yield in a;"), ["(in yield a)"]);
        assert_eq!(statements("f(yield, await);"), ["(call f [yield await])"]);
        // …as a `BindingIdentifier`, which takes them unconditionally in the grammar and leaves
        // the refusing to §13.1.1's early errors — none of which can fire here.
        assert_eq!(statements("var yield = 1;"), ["(var yield=1)"]);
        assert_eq!(statements("let await = 1;"), ["(let await=1)"]);
        assert_eq!(statements("const yield = 1;"), ["(const yield=1)"]);
        assert_eq!(
            statements("try {} catch (yield) {}"),
            ["(try {} (catch yield {}))"]
        );
        assert_eq!(statements("let [yield] = a;"), ["(let [yield]=a)"]);
        assert_eq!(
            statements("for (var await of a);"),
            ["(for-of (var await) a <empty>)"]
        );
        // …as a `LabelIdentifier`.
        assert_eq!(statements("yield: ;"), ["(label yield <empty>)"]);
        assert_eq!(
            statements("await: while (1) continue await;"),
            ["(label await (while 1 (continue await)))"]
        );
        // …and in the two places a name is not an `Identifier` at all, which never needed them:
        // a property key is an `IdentifierName`, so every reserved word was always allowed.
        assert_eq!(statements("a.yield;"), ["(. a yield)"]);
        assert_eq!(statements("a.await;"), ["(. a await)"]);
        assert_eq!(shape("({yield: 1})"), "{(yield 1)}");
        assert_eq!(shape("({if: 1})"), "{(if 1)}");
        // Shorthand is an `IdentifierReference`, so it takes the two and no other reserved word.
        assert_eq!(shape("({yield})"), "{yield}");
        assert_eq!(shape("({await})"), "{await}");
        assert!(parse_script("({if});").is_err());
        // Every other reserved word is still refused everywhere a name is wanted, which is what
        // makes this about those two rather than about reserved words in general.
        for source in ["var if = 1;", "new;", "if: ;", "class: ;", "typeof var;"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // An escaped spelling was always an ordinary identifier, being no terminal at all — so
        // these were legal before this slice and are unchanged by it.
        assert_eq!(statements(r"var \u0079ield = 1;"), ["(var yield=1)"]);
    }

    #[test]
    fn a_labelled_item_is_a_statement_so_var_may_stand_there_and_let_may_not() {
        // `LabelledItem : Statement | FunctionDeclaration`, and `Statement` has no `Declaration`
        // alternative — the same rule every body position has.
        assert_eq!(statements("a: var b;"), ["(label a (var b))"]);
        assert!(parse_script("a: let b;").is_err());
        assert!(parse_script("a: const b = 1;").is_err());
        // …and inside a block, which is a StatementList, both are fine again.
        assert_eq!(statements("a: { let b; }"), ["(label a {(let b)})"]);
        // `LabelledItem : FunctionDeclaration` is the other alternative, and §14.13.1 makes it a
        // Syntax Error "unless that source text is non-strict code and the host is a web browser
        // or otherwise supports Labelled Function Declarations". It is refused here, and the
        // reason is the one Annex B.3.5 was refused for: the exemption turns on strictness, which
        // this parser cannot yet tell. Accepting unconditionally would be wrong in strict code on
        // every host; refusing is wrong only for sloppy code on a host that implements it. V8
        // accepts it, so this is a divergence — and one that goes away with strict mode.
        assert!(parse_script("a: function f() {}").is_err());
    }

    #[test]
    fn break_and_continue_take_a_label_on_the_same_line_and_not_on_the_next() {
        assert_eq!(statements("a: { break a; }"), ["(label a {(break a)})"]);
        assert_eq!(
            statements("a: while (1) continue a;"),
            ["(label a (while 1 (continue a)))"]
        );
        assert_eq!(statements("while (1) break;"), ["(while 1 break)"]);
        // §12.10 rule 3: `break [no LineTerminator here] LabelIdentifier`. A name on the next line
        // is the next statement, which is what makes the restriction matter — and here it turns a
        // labelled break into an unlabelled one rather than into an error.
        assert_eq!(
            statements("while (1) { break\na; }"),
            ["(while 1 {break a})"],
            "two statements: an unlabelled break, then `a`"
        );
        assert_eq!(
            statements("a: while (1) { continue\na; }"),
            ["(label a (while 1 {continue a}))"]
        );
        // …and on one line it is the label it looks like.
        assert_eq!(
            statements("a: while (1) { continue a; }"),
            ["(label a (while 1 {(continue a)}))"]
        );
        // The label is a StringValue, so an escape does not change which one it names.
        assert_eq!(statements(r"a: { break a; }"), ["(label a {(break a)})"]);
        // A reserved word is no `LabelIdentifier`, so it simply ends the statement.
        assert!(parse_script("a: { break if; }").is_err());
    }

    #[test]
    fn every_rule_about_what_a_label_means_is_reported_from_the_tree() {
        // The parser reads labels; `crate::static_semantics::labels` decides what they mean. All
        // five rules arrive here as one kind of failure each — see that module for the argument.
        assert_eq!(script_error("a: a: ;").kind, ParseErrorKind::DuplicateLabel);
        assert_eq!(
            script_error("a: { a: ; }").kind,
            ParseErrorKind::DuplicateLabel
        );
        assert_eq!(
            script_error("break a;").kind,
            ParseErrorKind::UndefinedBreakTarget
        );
        assert_eq!(
            script_error("a: { continue a; }").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        assert_eq!(
            script_error("while (1) { a: continue a; }").kind,
            ParseErrorKind::UndefinedContinueTarget
        );
        assert_eq!(
            script_error("break;").kind,
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            script_error("continue;").kind,
            ParseErrorKind::ContinueOutsideLoop
        );
        // The caret goes on the label, or on the jump when there is none.
        assert_eq!(script_error("a: ; break zz;").span, Span::new(11, 13));
        assert_eq!(script_error("a: a: ;").span, Span::new(3, 4));
        // …and the legal shapes still parse, which is what makes the above about meaning rather
        // than about syntax.
        assert!(parse_script("a: ; a: ;").is_ok());
        assert!(parse_script("a: while (1) break a;").is_ok());
        assert!(parse_script("a: switch (x) { case 1: break a; }").is_ok());
    }

    #[test]
    fn no_label_however_odd_can_panic() {
        let cases = [
            "a:".to_string(),
            "a: b:".to_string(),
            "break a".to_string(),
            "continue a".to_string(),
            "a: ".repeat(1000),
            "a: b: c: d: e: ;".to_string(),
            format!(
                "{} ;",
                (0..500).map(|i| format!("l{i}: ")).collect::<String>()
            ),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // Labels nest, so they are bounded by the cap rather than by memory.
        assert_eq!(
            script_error(&"a: ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
        // …while five hundred distinct ones in a chain is a chain, and the walk handles it.
        let many: String = (0..500).map(|i| format!("l{i}: ")).collect::<String>() + ";";
        assert!(parse_script(&many).is_err(), "still past the nesting cap");
    }
}
