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
use crate::ast::{Binding, BindingPattern, Stmt, StmtKind};

/// `LexicallyDeclaredNames` of a `StatementList` (§8.2.6).
///
/// The names `let` and `const` bind *at this level*. Nested blocks and statement bodies are not
/// looked at: they are their own scopes, and their names are their own problem.
///
/// ```
/// use viperjs::parser::parse_script;
/// use viperjs::static_semantics::lexically_declared_names;
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
    collect_lexical(body, false)
}

/// `TopLevelLexicallyDeclaredNames` of a `StatementList` (§8.2.10).
///
/// What a `Script` body and a `FunctionStatementList` want instead. It differs from its sibling in
/// one production: a `HoistableDeclaration` — a function — contributes nothing here, being
/// var-scoped at a top level and lexically scoped anywhere else. That single line is the whole of
/// why `function f() {} function f() {}` is fine at the top of a script and
/// `{ function f() {} let f; }` is not.
pub fn top_level_lexically_declared_names(body: &[Stmt]) -> Vec<DeclaredName<'_>> {
    collect_lexical(body, true)
}

/// Both of the above, which differ only in what they do with a function.
fn collect_lexical(body: &[Stmt], top_level: bool) -> Vec<DeclaredName<'_>> {
    let mut names = Vec::new();
    for stmt in body {
        match &stmt.kind {
            StmtKind::Declaration(declaration) if declaration.kind.is_lexical() => {
                push_bound_names(declaration, &mut names);
            }
            StmtKind::Function(function) if !top_level => {
                if let Some(name) = &function.name {
                    names.push(DeclaredName {
                        name: &name.name,
                        span: name.span,
                    });
                }
            }
            // A class is lexically declared at *both* levels, where a function is only at
            // the inner one: §8.2.9 excludes a `HoistableDeclaration` from
            // `TopLevelLexicallyDeclaredNames` and a class is not one. Which is the whole of
            // why `function f() {} function f() {}` is fine at the top of a script and
            // `class C {} class C {}` is not.
            StmtKind::Class(class) => {
                if let Some(name) = &class.name {
                    names.push(DeclaredName {
                        name: &name.name,
                        span: name.span,
                    });
                }
            }
            // §8.2.6 hands `StatementListItem : Statement` on to the statement only when it is a
            // `LabelledStatement`, and from there to the `LabelledItem` — which §B.3.2 lets be a
            // `FunctionDeclaration`. So `{ a: function f() {} let f; }` is a redeclaration, for
            // exactly the reason `{ function f() {} let f; }` is.
            //
            // Not at a top level: §8.2.10 gives `TopLevelLexicallyDeclaredNames` of a
            // `StatementListItem : Statement` an empty list unconditionally, which is what makes a
            // labelled function at the top of a body var-scoped instead — see
            // [`top_level_var_declared_names`], which passes `direct` through a label to say so.
            StmtKind::Labelled(_) if !top_level => {
                names.extend(labelled_function_name(stmt));
            }
            _ => {}
        }
    }
    names
}

/// The names a `FunctionDeclaration` binds at the top level of `body` — §B.3.3.5's carve-out.
///
/// The subset of [`lexically_declared_names`] that a *`FunctionDeclaration`* put there, so that a
/// caller can ask whether a duplicate is "only bound by FunctionDeclarations". A label is followed
/// for the reason the walk above follows one: §B.3.2 makes `a: function f() {}` a lexical binding
/// of `f`.
///
/// A `GeneratorDeclaration`, an `AsyncFunctionDeclaration` and an `AsyncGeneratorDeclaration` are
/// **not** among them. Each is a `HoistableDeclaration` and none is a `FunctionDeclaration`, which
/// is the production the carve-out names — so `{ function f() {} function* f() {} }` is the Syntax
/// Error §14.2.1 makes it, and test262 has a file for all sixteen pairings.
///
/// ```
/// use viperjs::parser::parse_script;
/// use viperjs::static_semantics::function_declared_names;
///
/// let script = parse_script("{ function f() {} a: function g() {} function* h() {} let i; }")
///     .expect("this parses");
/// let viperjs::ast::StmtKind::Block(block) = &script.body[0].kind else { panic!("a block") };
/// let names: Vec<_> = function_declared_names(block)
///     .iter()
///     .map(|declared| declared.name)
///     .collect();
/// assert_eq!(names, ["f", "g"], "a generator is not one, and `let i` is not a function at all");
/// ```
pub fn function_declared_names(body: &[Stmt]) -> Vec<DeclaredName<'_>> {
    body.iter()
        .filter_map(|stmt| {
            let function = hoistable_declaration(stmt)?;
            match function.is_generator || function.is_async {
                true => None,
                false => declared(function),
            }
        })
        .collect()
}

/// The name of the declaration a chain of labels ends in, if it ends in one.
///
/// Whatever kind it is: §8.2.6 gives a `LabelledStatement` the `BoundNames` of its item, and does
/// not ask which `HoistableDeclaration` the item is. That §B.3.2 only ever produces the plain kind
/// is a fact about the parser, and this operation should not be the place that knows it.
fn labelled_function_name(stmt: &Stmt) -> Option<DeclaredName<'_>> {
    let StmtKind::Labelled(_) = &stmt.kind else {
        return None;
    };
    declared(hoistable_declaration(stmt)?)
}

/// The declaration a statement-list item is, looking through any labels §B.3.2 allows.
///
/// `a: b: function f() {}` is a `LabelledStatement` whose `LabelledItem` is another, so this is a
/// loop rather than a look: §14.13's item may be a `LabelledStatement` and each level hands the
/// question down unchanged.
fn hoistable_declaration(stmt: &Stmt) -> Option<&crate::ast::Function> {
    let mut current = stmt;
    loop {
        match &current.kind {
            StmtKind::Labelled(statement) => current = &statement.body,
            StmtKind::Function(function) => return Some(function),
            _ => return None,
        }
    }
}

/// A declaration's `BoundNames`, which is its own name — `None` for the anonymous `export default`.
fn declared(function: &crate::ast::Function) -> Option<DeclaredName<'_>> {
    let name = function.name.as_ref()?;
    Some(DeclaredName {
        name: &name.name,
        span: name.span,
    })
}

/// `VarDeclaredNames` of a `StatementList` (§8.2.8).
///
/// Every name `var` binds anywhere below, because that is what `var` means: the binding belongs
/// to the enclosing function, not to the block it was written in. So this descends through
/// blocks, both branches of an `if`, and loop bodies — everything that contains a statement.
///
/// ```
/// use viperjs::parser::parse_script;
/// use viperjs::static_semantics::var_declared_names;
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
    collect_var(body, false)
}

/// `TopLevelVarDeclaredNames` of a `StatementList` (§8.2.12).
///
/// The mirror of [`top_level_lexically_declared_names`], and the mirror of its one difference: a
/// function declared *directly* in this list is var-scoped and so belongs here. Only directly —
/// a nested `StatementListItem` is asked the ordinary question, which is why
/// `function f() { { function g() {} } }` declares no `g` at `f`'s top level.
pub fn top_level_var_declared_names(body: &[Stmt]) -> Vec<DeclaredName<'_>> {
    collect_var(body, true)
}

/// Both of the above. `top_level` applies to the direct items of `body` and to nothing below
/// them, exactly as §8.2.12 hands `VarDeclaredNames` to everything it descends into.
fn collect_var(body: &[Stmt], top_level: bool) -> Vec<DeclaredName<'_>> {
    let mut names = Vec::new();
    // Reversed, so popping yields source order — which is the order the specification's
    // list-concatenation produces, and the order that makes a diagnostic point at the first
    // offender rather than an arbitrary one.
    let mut pending: Vec<(&Stmt, bool)> = body.iter().rev().map(|stmt| (stmt, top_level)).collect();
    // Everything a statement contains is asked the ordinary question: §8.2.12 hands
    // `VarDeclaredNames` to what it descends into, and only the direct items of this list get the
    // top-level reading.
    fn nested(stmt: &Stmt) -> (&Stmt, bool) {
        (stmt, false)
    }
    while let Some((stmt, direct)) = pending.pop() {
        match &stmt.kind {
            StmtKind::Declaration(declaration) => {
                // A lexical declaration contributes nothing: §8.2.8 gives
                // `StatementListItem : Declaration` an empty list, and only a `VariableStatement`
                // a non-empty one.
                if !declaration.kind.is_lexical() {
                    push_bound_names(declaration, &mut names);
                }
            }
            // §8.2.12: a `HoistableDeclaration` written *directly* in a top-level list is
            // var-scoped and belongs here. Written anywhere else it is lexical, and §8.2.8 gives
            // a `Declaration` nothing. Either way its body is not descended into — a `var` in
            // there belongs to *that* function.
            StmtKind::Function(function) => {
                if direct && let Some(name) = &function.name {
                    names.push(DeclaredName {
                        name: &name.name,
                        span: name.span,
                    });
                }
            }
            StmtKind::Block(inner) => pending.extend(inner.iter().rev().map(nested)),
            StmtKind::If(statement) => {
                if let Some(alternate) = &statement.alternate {
                    pending.push(nested(alternate));
                }
                pending.push(nested(&statement.consequent));
            }
            // §8.2.12 hands a `LabelledStatement` to `TopLevelVarDeclaredNames` rather than to
            // the ordinary one, so a function under a label at a top level is still var-scoped.
            StmtKind::Labelled(statement) => pending.push((&statement.body, direct)),
            StmtKind::With(statement) => pending.push(nested(&statement.body)),
            StmtKind::While(statement) => pending.push(nested(&statement.body)),
            StmtKind::DoWhile(statement) => pending.push(nested(&statement.body)),
            StmtKind::For(statement) => {
                pending.push(nested(&statement.body));
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
                pending.push(nested(&statement.body));
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
                        .flat_map(|case| case.body.iter().rev())
                        .map(nested),
                );
            }
            StmtKind::Try(statement) => {
                // All three Blocks, because a `var` in any of them belongs to the enclosing
                // function just as much. The catch *parameter* is not among them — it is bound
                // by the handler's own scope and is not a var name at all.
                if let Some(finalizer) = &statement.finalizer {
                    pending.extend(finalizer.iter().rev().map(nested));
                }
                if let Some(handler) = &statement.handler {
                    pending.extend(handler.body.iter().rev().map(nested));
                }
                pending.extend(statement.block.iter().rev().map(nested));
            }
            // §8.2.8 lists these explicitly as contributing nothing, and they contain no
            // statement to look inside: empty, expression, `continue`, `break`, `throw`,
            // `debugger`, `return`.
            // A `ClassDeclaration` is lexical wherever it stands, so it is never a var name
            // — the one asymmetry with a function, which is var-scoped at a top level.
            StmtKind::Empty
            | StmtKind::Expression(_)
            | StmtKind::Debugger
            | StmtKind::Throw(_)
            | StmtKind::Return(_)
            | StmtKind::Break(_)
            | StmtKind::Continue(_)
            | StmtKind::Class(_) => {}
        }
    }
    names
}

/// The `BoundNames` of a declaration (§8.2.1), appended in source order.
fn push_bound_names<'a>(
    declaration: &'a crate::ast::Declaration,
    names: &mut Vec<DeclaredName<'a>>,
) {
    for declarator in &declaration.declarators {
        push_binding_names(&declarator.binding, names);
    }
}

/// The `BoundNames` of one binding (§8.2.1) — every name a pattern creates, in source order.
///
/// Iterative for the reason every walk here is: `Drop` stays the only recursive path over a tree.
/// A pattern nests, so this one genuinely had somewhere to recurse.
pub fn push_binding_names<'a>(binding: &'a Binding, names: &mut Vec<DeclaredName<'a>>) {
    let mut pending = vec![binding];
    while let Some(binding) = pending.pop() {
        match binding {
            Binding::Identifier(name) => names.push(DeclaredName {
                name: &name.name,
                span: name.span,
            }),
            Binding::Pattern(BindingPattern::Array(pattern)) => {
                if let Some(rest) = &pattern.rest {
                    pending.push(rest);
                }
                pending.extend(
                    pattern
                        .elements
                        .iter()
                        .flatten()
                        .map(|element| &element.target),
                );
            }
            Binding::Pattern(BindingPattern::Object(pattern)) => {
                if let Some(rest) = &pattern.rest {
                    names.push(DeclaredName {
                        name: &rest.name,
                        span: rest.span,
                    });
                }
                pending.extend(
                    pattern
                        .properties
                        .iter()
                        .map(|property| &property.value.target),
                );
            }
        }
    }
}

/// The `BoundNames` of one binding, as a list.
pub fn bound_names(binding: &Binding) -> Vec<DeclaredName<'_>> {
    let mut names = Vec::new();
    push_binding_names(binding, &mut names);
    names
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
                            binding: crate::ast::Binding::Identifier(crate::ast::BindingName {
                                name: "deep".into(),
                                span: Span::new(0, 4),
                            }),
                            initializer: None,
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
