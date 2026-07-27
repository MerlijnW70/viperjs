//! The names a statement list declares (ECMAScript §8.2).
//!
//! Two syntax-directed operations, and they differ in exactly one way that matters:
//! **`LexicallyDeclaredNames` does not descend and `VarDeclaredNames` does.** A `let` belongs to
//! the block it is written in, so `{ let a; { let a; } }` declares `a` twice in two scopes and is
//! fine. A `var` belongs to the enclosing function however deeply it is nested, so
//! `{ let a; { var a; } }` declares `a` twice in *one* scope and is not.
//!
//! That asymmetry is the whole content of these functions, and it is why they are a pair rather
//! than one function with a flag. See the module documentation for why they are a pass over the
//! tree, and why the walk is iterative.

use super::DeclaredName;
use crate::ast::{Stmt, StmtKind};

/// `LexicallyDeclaredNames` of a `StatementList` (§8.2.6).
///
/// The names `let` and `const` bind *at this level*. Nested blocks and statement bodies are not
/// looked at: they are their own scopes, and their names are their own problem.
///
/// ```
/// use praxis::parser::parse_script;
/// use praxis::static_semantics::lexically_declared_names;
///
/// let script = parse_script("let a; var b; { let c; }").expect("this parses");
/// let names: Vec<_> = lexically_declared_names(&script.body)
///     .iter()
///     .map(|declared| declared.name)
///     .collect();
/// assert_eq!(names, ["a"], "`var` is not lexical and `c` is not at this level");
/// ```
///
/// Today this is also `TopLevelLexicallyDeclaredNames` (§8.2.10), which a `Script` body wants
/// instead. The two differ only on a `HoistableDeclaration` — a function, which at the top level
/// of a script is var-scoped rather than lexical — and there are none yet. The day there are,
/// this needs a sibling and the callers need to choose between them.
pub fn lexically_declared_names(body: &[Stmt]) -> Vec<DeclaredName<'_>> {
    let mut names = Vec::new();
    for stmt in body {
        if let StmtKind::Declaration(declaration) = &stmt.kind
            && declaration.kind.is_lexical()
        {
            push_bound_names(declaration, &mut names);
        }
    }
    names
}

/// `VarDeclaredNames` of a `StatementList` (§8.2.8).
///
/// Every name `var` binds anywhere below, because that is what `var` means: the binding belongs
/// to the enclosing function, not to the block it was written in. So this descends through
/// blocks, both branches of an `if`, and loop bodies — everything that contains a statement.
///
/// ```
/// use praxis::parser::parse_script;
/// use praxis::static_semantics::var_declared_names;
///
/// let script = parse_script("var a; { var b; } if (x) var c; else var d; let e;")
///     .expect("this parses");
/// let names: Vec<_> = var_declared_names(&script.body)
///     .iter()
///     .map(|declared| declared.name)
///     .collect();
/// assert_eq!(names, ["a", "b", "c", "d"], "in source order, and `let` is not one of them");
/// ```
///
/// It will stop at a function body when there are functions — that is the boundary a `var` does
/// not cross, and the point at which this and `TopLevelVarDeclaredNames` (§8.2.12) stop agreeing.
pub fn var_declared_names(body: &[Stmt]) -> Vec<DeclaredName<'_>> {
    let mut names = Vec::new();
    // Reversed, so popping yields source order — which is the order the specification's
    // list-concatenation produces, and the order that makes a diagnostic point at the first
    // offender rather than an arbitrary one.
    let mut pending: Vec<&Stmt> = body.iter().rev().collect();
    while let Some(stmt) = pending.pop() {
        match &stmt.kind {
            StmtKind::Declaration(declaration) => {
                // A lexical declaration contributes nothing: §8.2.8 gives
                // `StatementListItem : Declaration` an empty list, and only a `VariableStatement`
                // a non-empty one.
                if !declaration.kind.is_lexical() {
                    push_bound_names(declaration, &mut names);
                }
            }
            StmtKind::Block(inner) => pending.extend(inner.iter().rev()),
            StmtKind::If(statement) => {
                if let Some(alternate) = &statement.alternate {
                    pending.push(alternate);
                }
                pending.push(&statement.consequent);
            }
            StmtKind::Labelled(statement) => pending.push(&statement.body),
            StmtKind::With(statement) => pending.push(&statement.body),
            StmtKind::While(statement) => pending.push(&statement.body),
            StmtKind::DoWhile(statement) => pending.push(&statement.body),
            StmtKind::For(statement) => {
                pending.push(&statement.body);
                // §8.2.8 gives the `var` header form the BoundNames of its list and gives the
                // lexical form nothing — the same split as any other declaration, in the one
                // place a declaration is not a statement.
                if let Some(crate::ast::ForInit::Declaration(declaration)) = &statement.init
                    && !declaration.kind.is_lexical()
                {
                    push_bound_names(declaration, &mut names);
                }
            }
            StmtKind::ForInOf(statement) => {
                pending.push(&statement.body);
                // The same split as the three-part form: a `var` header binds a var name and a
                // lexical one does not. The target-expression form binds nothing at all.
                if let crate::ast::ForInOfTarget::Declaration(declaration) = &statement.left
                    && !declaration.kind.is_lexical()
                {
                    push_bound_names(declaration, &mut names);
                }
            }
            StmtKind::Switch(statement) => {
                // §8.2.8 defines this over the CaseBlock, which is the concatenation across every
                // clause — a `var` in any of them belongs to the enclosing function just as much.
                // The discriminant is an expression and declares nothing.
                pending.extend(
                    statement
                        .cases
                        .iter()
                        .rev()
                        .flat_map(|case| case.body.iter().rev()),
                );
            }
            StmtKind::Try(statement) => {
                // All three Blocks, because a `var` in any of them belongs to the enclosing
                // function just as much. The catch *parameter* is not among them — it is bound
                // by the handler's own scope and is not a var name at all.
                if let Some(finalizer) = &statement.finalizer {
                    pending.extend(finalizer.iter().rev());
                }
                if let Some(handler) = &statement.handler {
                    pending.extend(handler.body.iter().rev());
                }
                pending.extend(statement.block.iter().rev());
            }
            // §8.2.8 lists these explicitly as contributing nothing, and they contain no
            // statement to look inside: empty, expression, `continue`, `break`, `throw`,
            // `debugger`. `return` joins them when functions arrive.
            StmtKind::Empty
            | StmtKind::Expression(_)
            | StmtKind::Debugger
            | StmtKind::Throw(_)
            | StmtKind::Break(_)
            | StmtKind::Continue(_) => {}
        }
    }
    names
}

/// The `BoundNames` of a declaration (§8.2.1), appended in source order.
fn push_bound_names<'a>(
    declaration: &'a crate::ast::Declaration,
    names: &mut Vec<DeclaredName<'a>>,
) {
    names.extend(
        declaration
            .declarators
            .iter()
            .map(|declarator| DeclaredName {
                name: &declarator.name,
                span: declarator.name_span,
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;
    use crate::span::Span;

    /// The lexically declared names of `source`, as a list of names.
    fn lexical(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // a test about names needs a tree
        lexically_declared_names(&script.body)
            .iter()
            .map(|declared| declared.name.to_string())
            .collect()
    }

    /// The var-declared names of `source`, as a list of names.
    fn vars(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // same
        var_declared_names(&script.body)
            .iter()
            .map(|declared| declared.name.to_string())
            .collect()
    }

    #[test]
    fn lexically_declared_names_are_the_ones_at_this_level_only() {
        assert_eq!(lexical(""), Vec::<String>::new());
        assert_eq!(lexical("let a;"), ["a"]);
        assert_eq!(lexical("const a = 1;"), ["a"]);
        assert_eq!(lexical("let a, b; const c = 1;"), ["a", "b", "c"]);
        // `var` is not a lexical declaration — that is the whole point of the two lists.
        assert_eq!(lexical("var a;"), Vec::<String>::new());
        // Nested scopes are not looked at. A `let` belongs to the block it is written in, so a
        // name inside one is invisible here and may repeat a name out here without conflict.
        assert_eq!(lexical("let a; { let b; }"), ["a"]);
        assert_eq!(lexical("let a; if (x) { let a; }"), ["a"]);
        assert_eq!(lexical("{ let a; }"), Vec::<String>::new());
        assert_eq!(lexical("while (x) { let a; }"), Vec::<String>::new());
        // Statements that declare nothing contribute nothing.
        assert_eq!(lexical("a; ; debugger; throw a;"), Vec::<String>::new());
        // A name is its StringValue, so an escaped spelling is the same name.
        assert_eq!(lexical(r"let a;"), ["a"]);
    }

    #[test]
    fn var_declared_names_descend_through_everything_that_holds_a_statement() {
        assert_eq!(vars(""), Vec::<String>::new());
        assert_eq!(vars("var a;"), ["a"]);
        assert_eq!(vars("var a, b;"), ["a", "b"]);
        assert_eq!(vars("let a; const b = 1;"), Vec::<String>::new());
        // The asymmetry with the list above: a `var` belongs to the enclosing function however
        // deeply it is nested, so every one of these is visible from out here.
        assert_eq!(vars("{ var a; }"), ["a"]);
        assert_eq!(vars("{ { { var a; } } }"), ["a"]);
        assert_eq!(vars("if (x) var a;"), ["a"]);
        assert_eq!(vars("if (x) var a; else var b;"), ["a", "b"]);
        assert_eq!(vars("while (x) var a;"), ["a"]);
        assert_eq!(vars("do var a; while (x);"), ["a"]);
        assert_eq!(vars("while (x) { if (y) { var a; } }"), ["a"]);
        // …and in source order, which is what the specification's list-concatenation gives and
        // what makes a diagnostic point at the first offender rather than an arbitrary one.
        assert_eq!(
            vars("var a; { var b; if (x) var c; else var d; } var e;"),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(
            vars("if (x) { var a; var b; } else { var c; }"),
            ["a", "b", "c"]
        );
        // Statements with no statement inside them are not descended into, because there is
        // nothing there — not because they are skipped.
        assert_eq!(
            vars("a; ; debugger; throw a; while (x) break;"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_names_carry_the_span_of_the_name_and_not_of_the_initialiser() {
        let script = parse_script("let abc = 1;").expect("this parses");
        let names = lexically_declared_names(&script.body);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].span, Span::new(4, 7), "`abc`, not `abc = 1`");
        assert_eq!(names[0].span.slice("let abc = 1;"), Some("abc"));
        // …and for a binding with no initialiser, where the two would coincide anyway.
        let script = parse_script("var xy;").expect("this parses");
        assert_eq!(var_declared_names(&script.body)[0].span, Span::new(4, 6));
    }

    #[test]
    fn a_tree_far_deeper_than_the_parser_can_build_costs_the_walk_no_stack() {
        // A thousand levels is twenty times the parser's nesting cap, and about a third of what
        // the tree's own destructor survives in this much stack — which is the real ceiling, and
        // the reason this number is a thousand rather than a million. See the module docs.
        //
        // Built by hand, because the parser cannot produce anything like this depth: that is the
        // case the walk has to be right about, since these are public functions over a public
        // tree and nothing makes an embedder go through the parser.
        let deep = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let mut stmt = Stmt {
                    kind: StmtKind::Declaration(Box::new(crate::ast::Declaration {
                        kind: crate::ast::DeclarationKind::Var,
                        declarators: Box::new([crate::ast::Declarator {
                            name: "deep".into(),
                            initializer: None,
                            name_span: Span::new(0, 4),
                            span: Span::new(0, 4),
                        }]),
                    })),
                    span: Span::new(0, 4),
                };
                for _ in 0..1_000 {
                    stmt = Stmt {
                        kind: StmtKind::Block(Box::new([stmt])),
                        span: Span::new(0, 4),
                    };
                }
                let body = [stmt];
                (
                    var_declared_names(&body).len(),
                    lexically_declared_names(&body).len(),
                )
            })
            .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
        let (vars, lexical) = deep
            .join()
            .unwrap_or_else(|_| panic!("the walk did not survive a thousand nested blocks")); // the panic IS the assertion
        assert_eq!(vars, 1, "the `var` was found through a thousand blocks");
        assert_eq!(lexical, 0, "and none of them was lexical");
    }
}
