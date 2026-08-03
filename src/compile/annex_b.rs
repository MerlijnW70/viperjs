//! Annex B §B.3.3 — which block-level function declarations also get a `var` binding.
//!
//! §14.1 makes a `FunctionDeclaration` inside a block belong to that block, created and initialised
//! when the block is entered. B.3.3 adds a second binding in the enclosing *variable* scope for the
//! web's sake, and the whole of the difficulty is that it does not add one for every such
//! declaration. It adds one where the source would still have been legal had the declaration been
//! written as `var F` instead:
//!
//! > If replacing the FunctionDeclaration f with a VariableStatement that has F as a
//! > BindingIdentifier would not produce any Early Errors for func and F is not an element of
//! > parameterNames, then …
//!
//! That sentence is a hypothetical about a program nobody wrote, and this module answers it by
//! asking what a `var F` written where f stands would collide with. §14.2.1's second rule is the
//! one that fires: a `var` name may not also be lexically declared in any list it passes through on
//! its way out to the variable scope. So the walk carries the lexical names of every enclosing
//! scope, and a declaration whose name is among them is left alone.
//!
//! # What blocks it, and the one thing that looks as though it should and does not
//!
//! A `let`, a `const`, a `class`, a lexical `for` head, another lexical binding of the same name in
//! the declaration's own block — all of those make `var F` a Syntax Error, so all of them skip the
//! extension. A **catch parameter** is the interesting one: §14.15.1 refuses a `var` naming one,
//! *except* when the parameter is a plain `BindingIdentifier`, which is B.3.4's own carve-out. So
//! `try {} catch (f) { { function f() {} } }` takes the extension and `catch ({ f })` does not, and
//! test262 has a test for each — `no-skip-try.js` against `skip-early-err-try.js`.
//!
//! # Two functions of one name in one block, where this follows the letter and browsers do not
//!
//! §B.3.3.5 lets `{ function f() {} function f() {} }` parse in sloppy code, so the question of
//! what `f` means outside the block is a real one. Read as written, the answer is *nothing*:
//! replacing either declaration with `var f` leaves the other lexically declaring `f` in the same
//! list, and §14.2.1's second rule refuses that — B.3.3.5 relaxes the **duplicate** rule and not
//! this one. So neither declaration is eligible and no `var` binding is made.
//!
//! V8, SpiderMonkey and JavaScriptCore all answer with the second function instead. No test262
//! file measures it — `function-redeclaration-block.js` asserts only that the program parses — so
//! there is nothing here to be told which reading is right, and the letter of the clause is what
//! this implements. Recorded rather than left to be re-derived: it is one `saturating_sub` in
//! [`scoped_refs`], and a session with data about real code should change it there.
//!
//! # A function's own parameters, and `arguments`
//!
//! Two more conditions, and they are stated outside the hypothetical rather than inside it: F may
//! not be a parameter name, and F may not be `arguments`. Neither is an early error — a parameter
//! and a `var` of one name are the same binding, and so are `arguments` and a `var` — so neither
//! would be found by asking about `var F`. They are separate clauses because what they protect is
//! separate: the value a parameter was called with, and §10.2.11 step 19's arguments object.

use crate::ast::{ForInOfTarget, ForInit, Stmt, StmtKind};
use crate::span::Span;
use std::collections::HashSet;

/// One declaration §B.3.3 gives a `var` binding in the enclosing variable scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockFunction<'a> {
    /// The span of the whole declaration, which is how the compiler knows this one again.
    ///
    /// The extension is not a fact about the *name* — step 3 replaces the evaluation of one
    /// particular declaration, and a body may hold an eligible `f` in one block and an ineligible
    /// one in another. Two spans never coincide in a source, so the span is the identity.
    pub(super) span: Span,
    /// The name it binds.
    pub(super) name: &'a str,
}

/// Every declaration in `body` that §B.3.3 gives a `var` binding, in source order.
///
/// `parameters` is `parameterNames` — empty for a script and for an `eval`, which have none.
///
/// Source order matters twice over. The bindings are created in it, which nothing can see; and the
/// list is searched by span when each declaration is reached, which is why it is a list at all
/// rather than a set of names.
pub(super) fn block_functions<'a>(
    body: &'a [Stmt],
    parameters: &HashSet<&str>,
) -> Vec<BlockFunction<'a>> {
    let mut found = Vec::new();
    // The names a `var` here would collide with. Seeded with the variable scope's own lexical
    // names, because that list is the last one a `var` passes through — `function f() { let g; {
    // function g() {} } }` is `skip-early-err.js`, and the `let` is at the body's top level.
    //
    // §8.2.10 rather than §8.2.6: at a variable scope a function declaration is var-scoped and not
    // lexical, so a top-level `function g` does not block a nested one. That is the whole of
    // `existing-fn-update.js`, where the two are meant to become one binding.
    let mut blocked: Vec<&str> = crate::static_semantics::top_level_lexically_declared_names(body)
        .into_iter()
        .map(|declared| declared.name)
        .collect();
    walk(body, &mut blocked, parameters, &mut found);
    found
}

/// The walk, with `blocked` holding the lexical names of every scope entered so far.
///
/// Recursive, and bounded by the parser's nesting cap rather than by anything here — the same
/// bound [`super::Compiler::statement`] relies on, over the same tree.
fn walk<'a>(
    body: &'a [Stmt],
    blocked: &mut Vec<&'a str>,
    parameters: &HashSet<&str>,
    found: &mut Vec<BlockFunction<'a>>,
) {
    for statement in body {
        match &statement.kind {
            // The blocks proper. §B.3.3 names three lists a declaration may be "directly contained
            // in" — a `Block`, a `CaseClause` and a `DefaultClause` — and the last two are one
            // scope, so the switch below concatenates them exactly as §8.2.6 does.
            StmtKind::Block(inner) => scoped(inner, blocked, parameters, found, |_| {}),
            StmtKind::If(inner) => {
                walk_body(&inner.consequent, blocked, parameters, found);
                if let Some(alternate) = &inner.alternate {
                    walk_body(alternate, blocked, parameters, found);
                }
            }
            // A label is not a scope, so its body is walked with the names this level already has.
            // §B.3.2's `a: function f() {}` inside a block is therefore reached by the block above
            // it, which is where the declaration is lexically declared.
            StmtKind::Labelled(inner) => walk_body(&inner.body, blocked, parameters, found),
            StmtKind::With(inner) => walk_body(&inner.body, blocked, parameters, found),
            StmtKind::While(inner) => walk_body(&inner.body, blocked, parameters, found),
            StmtKind::DoWhile(inner) => walk_body(&inner.body, blocked, parameters, found),
            // §14.7.4.1 — a `for` head's lexical names may not be `var`-declared in its body, so
            // `for (let f; ; ) { { function f() {} } }` skips the extension. That is a rule about
            // the head and the body together, which is why the head's names are pushed for the
            // body and popped after it rather than being part of any block's list.
            StmtKind::For(inner) => {
                let head = match &inner.init {
                    Some(ForInit::Declaration(declaration)) if declaration.kind.is_lexical() => {
                        declared_names(declaration)
                    }
                    _ => Vec::new(),
                };
                with_names(head, blocked, |blocked| {
                    walk_body(&inner.body, blocked, parameters, found);
                });
            }
            // §14.7.5.1 states the same rule for both `for`-`in` and `for`-`of`.
            StmtKind::ForInOf(inner) => {
                let head = match &inner.left {
                    ForInOfTarget::Declaration(declaration) if declaration.kind.is_lexical() => {
                        declared_names(declaration)
                    }
                    _ => Vec::new(),
                };
                with_names(head, blocked, |blocked| {
                    walk_body(&inner.body, blocked, parameters, found);
                });
            }
            StmtKind::Switch(inner) => {
                // §8.2.6 defines a `CaseBlock`'s names over the concatenation of its clauses, so
                // the whole switch is one scope: a `let f` in one clause blocks a declaration in
                // another, which is `skip-early-err-switch.js`.
                let cases: Vec<&Stmt> = inner
                    .cases
                    .iter()
                    .flat_map(|case| case.body.iter())
                    .collect();
                scoped_refs(&cases, blocked, parameters, found);
            }
            StmtKind::Try(inner) => {
                scoped(&inner.block, blocked, parameters, found, |_| {});
                if let Some(handler) = &inner.handler {
                    // §14.15.1's rule against a `var` naming a catch parameter, with B.3.4's
                    // carve-out for the one shape that is a plain name. A pattern's names block;
                    // a `BindingIdentifier` does not, and `no-skip-try.js` is that program.
                    let parameter: Vec<&str> =
                        match handler.parameter.as_ref().map(|it| &it.binding) {
                            Some(binding @ crate::ast::Binding::Pattern(_)) => {
                                crate::static_semantics::bound_names(binding)
                                    .into_iter()
                                    .map(|declared| declared.name)
                                    .collect()
                            }
                            _ => Vec::new(),
                        };
                    scoped(&handler.body, blocked, parameters, found, |blocked| {
                        blocked.extend(parameter.iter().copied());
                    });
                }
                if let Some(finalizer) = &inner.finalizer {
                    scoped(finalizer, blocked, parameters, found, |_| {});
                }
            }
            // A function's body is a variable scope of its own and asks these questions again for
            // itself; a `var` inside it never reaches this one. Everything else declares nothing
            // and contains no statement.
            _ => {}
        }
    }
}

/// A statement in a body position, which is a `StatementList` of one when it is a block.
///
/// The declaration itself cannot stand here — §B.3.4 wraps an `if` clause's in a `Block` at the
/// parser, precisely so that this walk and the compiler meet one shape rather than two.
fn walk_body<'a>(
    statement: &'a Stmt,
    blocked: &mut Vec<&'a str>,
    parameters: &HashSet<&str>,
    found: &mut Vec<BlockFunction<'a>>,
) {
    walk(std::slice::from_ref(statement), blocked, parameters, found);
}

/// Enter one `StatementList` that is a scope: take its candidates, then walk what it contains.
///
/// `extra` adds names the list itself does not declare — a catch parameter is the only one.
fn scoped<'a>(
    body: &'a [Stmt],
    blocked: &mut Vec<&'a str>,
    parameters: &HashSet<&str>,
    found: &mut Vec<BlockFunction<'a>>,
    extra: impl FnOnce(&mut Vec<&'a str>),
) {
    let refs: Vec<&Stmt> = body.iter().collect();
    let mark = blocked.len();
    extra(blocked);
    scoped_refs(&refs, blocked, parameters, found);
    blocked.truncate(mark);
}

/// [`scoped`] over a list already gathered as references — what a `CaseBlock` needs.
fn scoped_refs<'a>(
    body: &[&'a Stmt],
    blocked: &mut Vec<&'a str>,
    parameters: &HashSet<&str>,
    found: &mut Vec<BlockFunction<'a>>,
) {
    let mark = blocked.len();
    // Every name this list declares lexically, which is what a `var` leaving it would collide
    // with. A function declaration is among them (§8.2.6 gives a `Block` the non-`TopLevel`
    // reading), and that is what makes the *inner* declaration of
    // `{ function f() {} { function f() {} } }` ineligible — `nested-blocks-with-fun-decl.js`.
    let lexical: Vec<&str> = body
        .iter()
        .flat_map(|statement| lexical_names_of(statement))
        .collect();
    for statement in body {
        let Some((span, name)) = declaration_of(statement) else {
            continue;
        };
        // The declaration's own binding is not what a `var` of the same name would collide with —
        // it is the one being replaced. Anything *else* lexical here of that name is, which is why
        // this counts rather than asks whether the name is in the list at all. `saturating_sub`
        // because the name is always its own entry and a walk that disagreed with
        // [`lexical_names_of`] would otherwise be a panic rather than a wrong answer.
        //
        // The count can only exceed one where §B.3.3.5 let two functions of a name into one block,
        // which is the case the module doc records as diverging from every browser.
        let others = lexical
            .iter()
            .filter(|other| **other == name)
            .count()
            .saturating_sub(1);
        if others > 0 || blocked.contains(&name) || parameters.contains(name) {
            continue;
        }
        // §10.2.11 step 19's arguments object is not a binding a `var` could collide with, so this
        // is a clause of its own rather than something the hypothetical would find.
        if name == "arguments" {
            continue;
        }
        found.push(BlockFunction { span, name });
    }
    blocked.extend(lexical);
    for statement in body {
        walk(std::slice::from_ref(*statement), blocked, parameters, found);
    }
    blocked.truncate(mark);
}

/// Push `names` for the duration of `inside`, then take them back off.
fn with_names<'a>(
    names: Vec<&'a str>,
    blocked: &mut Vec<&'a str>,
    inside: impl FnOnce(&mut Vec<&'a str>),
) {
    let mark = blocked.len();
    blocked.extend(names);
    inside(blocked);
    blocked.truncate(mark);
}

/// The `BoundNames` of a declaration, as plain names.
fn declared_names(declaration: &crate::ast::Declaration) -> Vec<&str> {
    declaration
        .declarators
        .iter()
        .flat_map(|declarator| crate::static_semantics::bound_names(&declarator.binding))
        .map(|declared| declared.name)
        .collect()
}

/// The names one item of a `StatementList` declares lexically — §8.2.6, as names alone.
fn lexical_names_of(statement: &Stmt) -> Vec<&str> {
    crate::static_semantics::lexically_declared_names(std::slice::from_ref(statement))
        .into_iter()
        .map(|declared| declared.name)
        .collect()
}

/// The declaration a statement-list item is, if §B.3.3 could reach it.
///
/// A `GeneratorDeclaration`, an `AsyncFunctionDeclaration` and an `AsyncGeneratorDeclaration` are
/// each a `HoistableDeclaration` and none of them is a `FunctionDeclaration`, which is the
/// production §B.3.3 names — so `{ function* g() {} }` is block-scoped and gets no `var` binding.
fn declaration_of(statement: &Stmt) -> Option<(Span, &str)> {
    let (function, span) = declared_function(statement)?;
    if function.is_generator || function.is_async {
        return None;
    }
    Some((span, &function.name.as_ref()?.name))
}

/// The `HoistableDeclaration` a statement-list item is, looking through any labels.
///
/// The looking-through is §B.3.2's doing and is why this lives here: `a: function f() {}` is a
/// `LabelledStatement` whose item is a declaration, and §8.2.6 gives the label the item's
/// `BoundNames` — so it is declared by the list exactly as an unlabelled one is, and everything
/// that walks a statement list for its declarations has to see it. `a: b: function f() {}` is a
/// label on a label, which is why this is a loop.
pub(super) fn declared_function(statement: &Stmt) -> Option<(&crate::ast::Function, Span)> {
    let mut current = statement;
    loop {
        match &current.kind {
            StmtKind::Labelled(inner) => current = &inner.body,
            StmtKind::Function(function) => return Some((function, current.span)),
            _ => return None,
        }
    }
}
