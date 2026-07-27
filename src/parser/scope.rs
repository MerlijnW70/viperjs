//! The two early errors a statement list has about the names it declares (§14.2.1, §16.1.1).
//!
//! Both rules are stated identically for a `Block` and for a `Script`, and both are about the
//! lists [`crate::static_semantics`] computes:
//!
//! 1. The `LexicallyDeclaredNames` may not contain duplicates — `{ let a; let a; }`.
//! 2. None of them may also occur in the `VarDeclaredNames` — `{ let a; var a; }`.
//!
//! # Why the second rule needs a walk and the first does not
//!
//! Because `var` hoists out of the block it was written in and `let` does not. `{ let a; { let a;
//! } }` is two names in two scopes and is fine; `{ let a; { var a; } }` is two names in one scope
//! and is not, and nothing at the outer level looks like a redeclaration until you go and find
//! the `var`. That is the whole reason the second rule cannot be checked by looking at the list
//! in front of you.
//!
//! # What this does not yet catch
//!
//! §14.2.1's carve-out for `FunctionDeclaration`s under Web Legacy Compatibility Semantics, and
//! the difference between these operations and their `TopLevel` variants (§8.2.10, §8.2.12).
//! Both turn on a `HoistableDeclaration`, and there are none until functions land — at which
//! point a `Script` stops being able to share this function with a `Block`.

use super::{ParseError, ParseErrorKind};
use crate::ast::{Declaration, Stmt, SwitchCase};
use crate::span::Span;
use crate::static_semantics::{DeclaredName, lexically_declared_names, var_declared_names};
use std::collections::HashMap;

/// Apply both rules to a completed statement list, whether a `Block`'s or a `Script`'s.
///
/// Called once per list rather than maintained as the parser goes — DR-0007 — so it runs on a
/// finished tree and can be read straight against §14.2.1. A free function rather than a method
/// on the parser because it needs nothing the parser knows: the tree is the whole input, which is
/// the point of computing early errors this way.
pub(super) fn check_declared_names(body: &[Stmt]) -> Result<(), ParseError> {
    check(lexically_declared_names(body), var_declared_names(body))
}

/// The same two rules over a `CaseBlock` (§14.12.1).
///
/// The clauses are not scopes — the `CaseBlock` is the scope — so §8.2.6 and §8.2.8 both define
/// their lists over it as the concatenation across every clause. Doing exactly that is what makes
/// `case 1: let a; case 2: let a;` a redeclaration, and it is why this is a second caller of the
/// same rules rather than a second rule.
pub(super) fn check_case_block_declared_names(cases: &[SwitchCase]) -> Result<(), ParseError> {
    check(
        cases
            .iter()
            .flat_map(|case| lexically_declared_names(&case.body))
            .collect(),
        cases
            .iter()
            .flat_map(|case| var_declared_names(&case.body))
            .collect(),
    )
}

/// §14.7.4.1: a `for` header's lexical names may not be `var`-declared in its body.
///
/// The header is a scope of its own, between the enclosing one and the body's, so the body may
/// shadow it with a `let` — `for (let a;;) { let a; }` is fine. A `var` is not shadowing: it
/// belongs to the enclosing function and passes through the header's scope on its way out, where
/// the header's name is already sitting.
pub(super) fn check_header_against_body(
    declaration: &Declaration,
    body: &[Stmt],
) -> Result<(), ParseError> {
    let mut header: HashMap<&str, Span> = HashMap::new();
    for declarator in &declaration.declarators {
        header.insert(&declarator.name, declarator.name_span);
    }
    for declared in var_declared_names(body) {
        if let Some(&header_span) = header.get(declared.name) {
            return Err(ParseError {
                kind: ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                span: std::cmp::max_by_key(header_span, declared.span, |span| span.start),
            });
        }
    }
    Ok(())
}

/// §14.2.1, §16.1.1 and §14.12.1, which state the same two rules about different lists.
fn check(
    lexical_names: Vec<DeclaredName<'_>>,
    var_names: Vec<DeclaredName<'_>>,
) -> Result<(), ParseError> {
    let mut lexical: HashMap<&str, Span> = HashMap::new();
    for declared in lexical_names {
        // The list is in source order, so the one that collides is always the later of the
        // two and there is nothing to compare — the caret goes on the redeclaration because
        // that is where it was found, not because a rule picked it.
        if lexical.insert(declared.name, declared.span).is_some() {
            return Err(ParseError {
                kind: ParseErrorKind::DuplicateLexicalBinding,
                span: declared.span,
            });
        }
    }
    // No short-circuit when there are no lexical names, though the loop below can then do
    // nothing: a branch no input can tell from its absence is one mutation testing
    // call untested, and it is not the parser's business to guess where the time goes.
    for declared in var_names {
        if let Some(&lexical_span) = lexical.get(declared.name) {
            return Err(ParseError {
                kind: ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                // Here the two lists are separate walks, so either name may have been written
                // first — `let a; var a;` and `var a; let a;` are both this error. The caret
                // goes on the later, which is the redeclaration. Stated as a maximum rather
                // than as a comparison of my own: the two can never start at the same offset,
                // so a branch for that case would be one no input could reach.
                span: std::cmp::max_by_key(lexical_span, declared.span, |span| span.start),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;

    /// The error `source` fails with.
    fn script_error(source: &str) -> ParseError {
        match parse_script(source) {
            Err(err) => err,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn a_name_may_not_be_declared_lexically_twice_in_one_scope() {
        // §14.2.1 and §16.1.1, rule 1. `let a, a;` is already refused one level down by
        // §14.3.1.1, which is about one declaration; this is about the whole list.
        assert_eq!(
            script_error("let a; let a;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("{ let a; let a; }").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("let a; const a = 1;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("const a = 1; let a;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("let a, b; let b;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        // The caret goes on the redeclaration, not on the declaration it collides with.
        assert_eq!(script_error("let a; let a;").span, Span::new(11, 12));
        assert_eq!(
            script_error("let ab = 1; let ab = 2;").span,
            Span::new(16, 18)
        );
        // A name is its StringValue, so two spellings of one name are one name.
        assert_eq!(
            script_error(r"let a; let a;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        // …and separate scopes are separate. This is the half that makes the rule non-trivial.
        assert!(parse_script("let a; { let a; }").is_ok());
        assert!(parse_script("{ let a; } { let a; }").is_ok());
        assert!(parse_script("{ let a; } let a;").is_ok());
        assert!(parse_script("let a; if (x) { let a; }").is_ok());
        assert!(parse_script("let a; while (x) { let a; }").is_ok());
        // `var` may repeat as much as it likes — it is not a lexical declaration.
        assert!(parse_script("var a; var a;").is_ok());
        assert!(parse_script("var a, a, a;").is_ok());
    }

    #[test]
    fn a_var_may_not_collide_with_a_lexical_name_however_deeply_it_is_buried() {
        // §14.2.1 and §16.1.1, rule 2 — and the reason it needs `VarDeclaredNames` rather than a
        // look at the list in front of you: a `var` belongs to the enclosing function, so it
        // collides from inside any number of blocks and bodies.
        assert_eq!(
            script_error("let a; var a;").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            script_error("var a; let a;").kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        for source in [
            "let a; { var a; }",
            "let a; { { { var a; } } }",
            "let a; if (x) var a;",
            "let a; if (x) b; else var a;",
            "let a; while (x) var a;",
            "let a; do var a; while (x);",
            "let a; while (x) { if (y) { var a; } }",
            "{ let a; { var a; } }",
            "const a = 1; { var a; }",
        ] {
            assert_eq!(
                script_error(source).kind,
                ParseErrorKind::ConflictingVarAndLexicalDeclaration,
                "{source:?}"
            );
        }
        // The caret goes on whichever was written second, in both directions.
        assert_eq!(script_error("let a; var a;").span, Span::new(11, 12));
        assert_eq!(script_error("var a; let a;").span, Span::new(11, 12));
        // …and a `var` in a scope the lexical name does not reach is not a collision. The `let`
        // here is inside the block; the `var` hoists past it to the top level, and the two never
        // share a scope.
        assert!(parse_script("var a; { let a; }").is_ok());
        assert!(parse_script("{ let a; } var a;").is_ok());
        assert!(parse_script("if (x) { let a; } var a;").is_ok());
        // A lexical name inside a nested block does not see an outer `var` either.
        assert!(parse_script("var a; if (x) { let a; }").is_ok());
    }

    #[test]
    fn the_rules_apply_to_a_script_and_to_every_block_it_contains() {
        // Both §14.2.1 (Block) and §16.1.1 (Script) state the same two rules, which is why one
        // function serves both — and why a block nested anywhere is checked, not just the top.
        assert!(parse_script("{ { { let a; let a; } } }").is_err());
        assert!(parse_script("while (x) { let a; var a; }").is_err());
        assert!(parse_script("if (x) { let a; let a; }").is_err());
        assert!(parse_script("do { let a; let a; } while (x);").is_err());
        // A clean script with a great many declarations is not accidentally quadratic or
        // accidentally wrong — the names differ, so nothing collides.
        let many: String = (0..2_000)
            .map(|i| format!("let a{i}; var b{i};\n"))
            .collect();
        assert!(parse_script(&many).is_ok());
        // …and one collision anywhere in that many still lands.
        assert_eq!(
            script_error(&(many + "var a7;")).kind,
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
    }
}
