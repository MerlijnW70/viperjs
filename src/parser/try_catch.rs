//! `try`, `catch` and `finally` (ECMAScript §14.15).
//!
//! `throw` is not here — it is a jump, and lives with `break` and `continue` in
//! [`super::control`]. This file is about the statement that catches one.
//!
//! # Three Blocks, and none of them is a statement
//!
//! `TryStatement : try Block Catch Finally`, and every one of those is a `Block` in the grammar
//! rather than a `Statement`. So `try a; catch (e) {}` has no derivation — braces are required,
//! and this is the one construct so far where that is true. All three still get §14.2.1, because
//! all three are Blocks; they share [`Parser::parse_block_body`] for exactly that reason.
//!
//! The grammar has no `try Block` on its own, so at least one of `Catch` and `Finally` must be
//! there. That is not an early error — there is simply no production — and it is the only thing
//! this file refuses that the shape of the tree could not have refused for it.
//!
//! # The three early errors of §14.15.1, and why one and a half of them are here
//!
//! All three are about the `BoundNames` of the `CatchParameter`:
//!
//! 1. They may not repeat. A `BindingIdentifier` is one name, so nothing can repeat until
//!    `catch ([a, a])` is parseable. Not deferred — *not yet reachable*, which is a different
//!    thing, and it becomes reachable the day destructuring lands.
//! 2. None may occur in the `LexicallyDeclaredNames` of the handler's Block. This one is here:
//!    `catch (e) { let e; }` is refused. Note it is `LexicallyDeclaredNames`, which does not
//!    descend — so `catch (e) { { let e; } }` is a different scope and is fine.
//! 3. None may occur in the `VarDeclaredNames` of the Block — *unless* the parameter is a
//!    `BindingIdentifier` and the host supports `VariableStatement`s in catch blocks
//!    (Annex B.3.4). Every `CatchParameter` this parser accepts is a `BindingIdentifier`, so the
//!    exemption covers all of them and the rule cannot fire yet. `catch (e) { var e; }` parses.
//!
//! The third is implemented and its exemption is declined. `catch (e) { var e; }` is refused
//! here and accepted by every browser, and DR-0008 is the argument: Annex B.3's syntactic
//! extensions are not implemented, praxis being no web browser. This was the other way round for
//! three slices, on the narrower ground that test262's main tree cannot be asserting the refusal
//! — which is true, and is not the question. The exemption names a `BindingIdentifier` anyway, so
//! `catch ([e]) { var e; }` would be refused on every host.

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{CatchClause, CatchParameter, Stmt, StmtKind, TryStatement};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::static_semantics::{bound_names, lexically_declared_names, var_declared_names};
use std::collections::HashSet;

impl Parser<'_> {
    /// `TryStatement` (§14.15), with the cursor on `try`.
    pub(super) fn parse_try(&mut self) -> Result<Stmt, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        self.enter()?;
        let parts = self.parse_try_parts();
        self.leave();
        let (statement, end) = parts?;
        Ok(Stmt {
            span: keyword.span.to(end),
            kind: StmtKind::Try(Box::new(statement)),
        })
    }

    /// The guarded block and whichever of `Catch` and `Finally` follow it.
    ///
    /// Apart from [`Parser::parse_try`] so that its locals — three blocks' worth — are not
    /// carried by every level of nesting that passes through, which is what the nesting cap is
    /// counted in.
    fn parse_try_parts(&mut self) -> Result<(TryStatement, crate::span::Span), ParseError> {
        let (block, mut end) = self.parse_block_body(super::scope::Level::Block)?;
        let handler = if self.current.kind == TokenKind::Keyword(ReservedWord::Catch) {
            let clause = self.parse_catch()?;
            end = clause.span;
            Some(clause)
        } else {
            None
        };
        let finalizer = if self.current.kind == TokenKind::Keyword(ReservedWord::Finally) {
            self.advance(Goal::RegExp)?;
            let (body, span) = self.parse_block_body(super::scope::Level::Block)?;
            end = span;
            Some(body)
        } else {
            None
        };
        // There is no `TryStatement : try Block`. Reported against the `try` itself rather than
        // against whatever came next, because the missing part is what the reader has to add and
        // the next token is innocent.
        if handler.is_none() && finalizer.is_none() {
            return Err(ParseError {
                kind: ParseErrorKind::TryWithoutHandler,
                span: end,
            });
        }
        Ok((
            TryStatement {
                block,
                handler,
                finalizer,
            },
            end,
        ))
    }

    /// `Catch : catch ( CatchParameter ) Block | catch Block` (§14.15).
    fn parse_catch(&mut self) -> Result<CatchClause, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        // The second alternative is the optional catch binding of ES2019. Not the same as binding
        // a name nobody reads: no binding is created at all, so there is nothing to shadow and
        // nothing for the early errors below to be about.
        let parameter = if self.current.kind == TokenKind::LParen {
            self.advance(Goal::RegExp)?;
            let binding = self.parse_binding()?;
            self.eat(TokenKind::RParen, Goal::RegExp, "`)`")?;
            Some(CatchParameter { binding })
        } else {
            None
        };
        let (body, block_span) = self.parse_block_body(super::scope::Level::Block)?;
        if let Some(parameter) = &parameter {
            let names = bound_names(&parameter.binding);
            // §14.15.1, rule 1: the BoundNames of a CatchParameter may not repeat. Unreachable
            // while a parameter was one name, and reachable now — `catch ([a, a])` is the shape
            // it was always about.
            let mut seen: HashSet<&str> = HashSet::new();
            for declared in &names {
                if !seen.insert(declared.name) {
                    return Err(ParseError {
                        kind: ParseErrorKind::DuplicateCatchParameterName,
                        span: declared.span,
                    });
                }
            }
            // Rule 2. `LexicallyDeclaredNames`, so it is the handler's own level and not what any
            // block inside it declares — `catch (e) { { let e; } }` is two scopes.
            let lexical = lexically_declared_names(&body);
            if let Some(shadow) = lexical
                .iter()
                .find(|declared| names.iter().any(|bound| bound.name == declared.name))
            {
                return Err(ParseError {
                    kind: ParseErrorKind::CatchParameterRedeclared,
                    span: shadow.span,
                });
            }
            // Rule 3, which reads `VarDeclaredNames` and so descends. Annex B.3.4 exempts a
            // `BindingIdentifier` parameter on a web browser, and DR-0008 declines it: praxis is
            // not one, and B.3's syntactic extensions are not implemented.
            if let Some(shadow) = var_declared_names(&body)
                .iter()
                .find(|declared| names.iter().any(|bound| bound.name == declared.name))
            {
                return Err(ParseError {
                    kind: ParseErrorKind::CatchParameterRedeclared,
                    span: shadow.span,
                });
            }
        }
        Ok(CatchClause {
            parameter,
            body,
            span: keyword.span.to(block_span),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;
    use crate::parser::test_support::*;
    use crate::span::Span;

    #[test]
    fn all_three_shapes_parse_and_try_alone_is_not_one_of_them() {
        assert_eq!(statements("try {} catch (e) {}"), ["(try {} (catch e {}))"]);
        assert_eq!(statements("try {} finally {}"), ["(try {} (finally {}))"]);
        assert_eq!(
            statements("try {} catch (e) {} finally {}"),
            ["(try {} (catch e {}) (finally {}))"]
        );
        assert_eq!(
            statements("try { a; } catch (e) { b; } finally { c; }"),
            ["(try {a} (catch e {b}) (finally {c}))"]
        );
        // `TryStatement : try Block` is not a production, so this is not a `try` that catches
        // nothing — it is not a `try` at all.
        assert_eq!(
            script_error("try {}").kind,
            ParseErrorKind::TryWithoutHandler
        );
        assert_eq!(
            script_error("try { a; }").kind,
            ParseErrorKind::TryWithoutHandler
        );
        // …and the order is fixed: a `finally` does not precede a `catch`.
        assert!(parse_script("try {} finally {} catch (e) {}").is_err());
        // Neither clause is a statement on its own.
        assert!(parse_script("catch (e) {}").is_err());
        assert!(parse_script("finally {}").is_err());
        let script = parse_script("try {} catch (e) {}").expect("this parses");
        assert_eq!(script.body[0].span, Span::new(0, 19));
    }

    #[test]
    fn every_one_of_the_three_parts_must_be_a_block() {
        // All three are `Block` in the grammar, not `Statement` — the only construct so far
        // where the braces are the grammar's requirement rather than the author's habit.
        assert_eq!(
            script_error("try a; catch (e) {}").kind,
            ParseErrorKind::Unexpected {
                expected: "`{`",
                found: TokenKind::Identifier {
                    contains_escape: false
                },
            }
        );
        assert!(parse_script("try {} catch (e) a;").is_err());
        assert!(parse_script("try {} finally a;").is_err());
        assert!(parse_script("try {} catch (e) if (x) {}").is_err());
        // …and they nest, being ordinary blocks otherwise.
        assert_eq!(
            statements("try { try {} finally {} } catch (e) {}"),
            ["(try {(try {} (finally {}))} (catch e {}))"]
        );
    }

    #[test]
    fn the_catch_binding_is_optional_and_is_exactly_one_name() {
        // ES2019's optional catch binding — `Catch : catch Block`.
        assert_eq!(statements("try {} catch {}"), ["(try {} (catch {}))"]);
        assert_eq!(
            statements("try {} catch { a; } finally {}"),
            ["(try {} (catch {a}) (finally {}))"]
        );
        // One name, and it is not a list.
        assert!(parse_script("try {} catch (e, f) {}").is_err());
        assert!(parse_script("try {} catch () {}").is_err());
        assert!(parse_script("try {} catch (e {}").is_err());
        // A `BindingPattern` is the other CatchParameter alternative, and is what makes
        // §14.15.1's first early error reachable at last — one name cannot repeat, and two can.
        assert!(parse_script("try {} catch ([a, b]) {}").is_ok());
        assert!(parse_script("try {} catch ({a}) {}").is_ok());
        assert_eq!(
            script_error("try {} catch ([a, a]) {}").kind,
            ParseErrorKind::DuplicateCatchParameterName
        );
        assert_eq!(
            script_error("try {} catch ({a, b: a}) {}").kind,
            ParseErrorKind::DuplicateCatchParameterName
        );
        // …and rule 2 reads the whole list of them, not one name.
        assert_eq!(
            script_error("try {} catch ([a]) { let a; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert_eq!(
            script_error("try {} catch ({a: b}) { let b; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert!(parse_script("try {} catch ([a]) { let b; }").is_ok());
        // §14.3.1.1's ban on `let` as a bound name is stated about a lexical declaration and
        // about nothing else, so a catch parameter may be called `let`.
        assert_eq!(
            statements("try {} catch (let) {}"),
            ["(try {} (catch let {}))"]
        );
        // A reserved word may not, being no `BindingIdentifier` at all.
        assert!(parse_script("try {} catch (if) {}").is_err());
    }

    #[test]
    fn the_catch_parameter_may_not_be_declared_again_at_the_handlers_own_level() {
        // §14.15.1, rule 2 — and it is `LexicallyDeclaredNames`, so "own level" is the whole
        // content of the rule.
        assert_eq!(
            script_error("try {} catch (e) { let e; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert_eq!(
            script_error("try {} catch (e) { const e = 1; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert_eq!(
            script_error("try {} catch (e) { let a, e; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert_eq!(
            script_error("try {} catch (e) { let e; }").span,
            Span::new(23, 24),
            "the caret goes on the redeclaration"
        );
        // A nested block is a different scope, which is what `LexicallyDeclaredNames` not
        // descending means.
        assert!(parse_script("try {} catch (e) { { let e; } }").is_ok());
        assert!(parse_script("try {} catch (e) { if (x) { let e; } }").is_ok());
        // A different name is not a collision, and neither is the same name outside.
        assert!(parse_script("try {} catch (e) { let f; }").is_ok());
        assert!(parse_script("let e; try {} catch (e) {}").is_ok());
        assert!(parse_script("try { let e; } catch (e) {}").is_ok());
        assert!(parse_script("try {} catch (e) {} finally { let e; }").is_ok());
        // With no parameter there is nothing to collide with — the binding-less form creates no
        // binding at all, rather than one nobody reads.
        assert!(parse_script("try {} catch { let e; }").is_ok());
    }

    #[test]
    fn a_var_may_not_reuse_the_catch_parameters_name_and_nor_may_a_lexical_one() {
        // §14.15.1's third rule, whose Annex B.3.4 exemption DR-0008 declines — praxis being no
        // web browser. Every browser accepts all three of these; this is what that costs.
        assert_eq!(
            script_error("try {} catch (e) { var e; }").kind,
            ParseErrorKind::CatchParameterRedeclared
        );
        assert_eq!(
            script_error("try {} catch (e) { { var e; } }").kind,
            ParseErrorKind::CatchParameterRedeclared,
            "the rule reads VarDeclaredNames, so it descends"
        );
        assert_eq!(
            script_error("try {} catch ([e]) { var e; }").kind,
            ParseErrorKind::CatchParameterRedeclared,
            "…and the exemption named a BindingIdentifier, so this one no host takes"
        );
        // A different name is still fine, which is what keeps this a rule about names.
        assert!(parse_script("try {} catch (e) { var f; }").is_ok());
        // §14.2.1 still applies to all three blocks, though, since all three are Blocks.
        assert!(parse_script("try { let a; let a; } catch (e) {}").is_err());
        assert!(parse_script("try {} catch (e) { let a; var a; }").is_err());
        assert!(parse_script("try {} finally { let a; let a; }").is_err());
        // …and a `var` anywhere inside any of the three hoists out to collide with a lexical
        // name in the enclosing scope, which is `VarDeclaredNames` descending into all three.
        for source in [
            "let a; try { var a; } catch (e) {}",
            "let a; try {} catch (e) { var a; }",
            "let a; try {} finally { var a; }",
            "let a; try {} catch (e) { { if (x) var a; } }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                "{source:?}"
            );
        }
        // The catch parameter itself is not a var name and does not hoist.
        assert!(parse_script("let e; try {} catch (e) { a; }").is_ok());
    }

    #[test]
    fn no_try_however_truncated_can_panic() {
        let cases = [
            "try".to_string(),
            "try {".to_string(),
            "try {} catch".to_string(),
            "try {} catch (".to_string(),
            "try {} catch (e".to_string(),
            "try {} catch (e)".to_string(),
            "try {} finally".to_string(),
            "catch".to_string(),
            "finally".to_string(),
            "try { ".repeat(1000),
            "try {} catch (e) { ".repeat(1000),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // Nesting recurses through the guarded block, so it is bounded by the cap.
        assert_eq!(
            script_error(&"try { ".repeat(1000)).kind,
            ParseErrorKind::TooDeeplyNested
        );
    }
}
