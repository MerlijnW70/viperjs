//! Rendering statements, declarations and the blocks that hold them.

use super::{render, render_binding, render_class, render_function, render_target};
use crate::ast::{Declaration, Expr, ForInOfKind, ForInOfTarget, ForInit, Stmt, StmtKind};

/// A statement rendered compactly, for tests about statement structure.
///
/// Blocks print as `{a b}` and expression statements as their expression, so a script reads as
/// the list of shapes it is — which is what a test about semicolon insertion wants to assert.
pub(in crate::parser) fn render_statement(stmt: &Stmt) -> String {
    match &stmt.kind {
        StmtKind::Empty => "<empty>".to_string(),
        StmtKind::Debugger => "debugger".to_string(),
        StmtKind::Expression(expr) => render(expr),
        StmtKind::Declaration(declaration) => render_declaration(declaration),
        StmtKind::Block(body) => render_block(body),
        StmtKind::If(statement) => match &statement.alternate {
            Some(alternate) => format!(
                "(if {} {} {})",
                render(&statement.test),
                render_statement(&statement.consequent),
                render_statement(alternate)
            ),
            None => format!(
                "(if {} {})",
                render(&statement.test),
                render_statement(&statement.consequent)
            ),
        },
        StmtKind::While(statement) => format!(
            "(while {} {})",
            render(&statement.test),
            render_statement(&statement.body)
        ),
        StmtKind::DoWhile(statement) => format!(
            "(do {} {})",
            render_statement(&statement.body),
            render(&statement.test)
        ),
        StmtKind::Throw(value) => format!("(throw {})", render(value)),
        StmtKind::For(statement) => {
            let init = match &statement.init {
                Some(ForInit::Expression(expr)) => render(expr),
                Some(ForInit::Declaration(declaration)) => render_declaration(declaration),
                None => ";".to_string(),
            };
            let clause = |expr: &Option<Expr>| match expr {
                Some(expr) => render(expr),
                None => ";".to_string(),
            };
            format!(
                "(for {} {} {} {})",
                init,
                clause(&statement.test),
                clause(&statement.update),
                render_statement(&statement.body)
            )
        }
        StmtKind::ForInOf(statement) => format!(
            "(for-{} {} {} {})",
            match (statement.kind, statement.is_await) {
                (ForInOfKind::In, _) => "in",
                (ForInOfKind::Of, false) => "of",
                (ForInOfKind::Of, true) => "await-of",
            },
            match &statement.left {
                ForInOfTarget::Expression(target) => render_target(target),
                ForInOfTarget::Declaration(declaration) => render_declaration(declaration),
            },
            render(&statement.right),
            render_statement(&statement.body)
        ),
        StmtKind::Switch(statement) => {
            let mut parts = vec![format!("(switch {}", render(&statement.discriminant))];
            for case in &statement.cases {
                parts.push(match &case.test {
                    Some(test) => format!("(case {} {})", render(test), render_block(&case.body)),
                    None => format!("(default {})", render_block(&case.body)),
                });
            }
            format!("{})", parts.join(" "))
        }
        StmtKind::Try(statement) => {
            let mut parts = vec![format!("(try {}", render_block(&statement.block))];
            if let Some(handler) = &statement.handler {
                parts.push(match &handler.parameter {
                    Some(parameter) => {
                        format!(
                            "(catch {} {})",
                            render_binding(&parameter.binding),
                            render_block(&handler.body)
                        )
                    }
                    None => format!("(catch {})", render_block(&handler.body)),
                });
            }
            if let Some(finalizer) = &statement.finalizer {
                parts.push(format!("(finally {})", render_block(finalizer)));
            }
            format!("{})", parts.join(" "))
        }
        StmtKind::Labelled(statement) => format!(
            "(label {} {})",
            statement.label.name,
            render_statement(&statement.body)
        ),
        StmtKind::With(statement) => format!(
            "(with {} {})",
            render(&statement.object),
            render_statement(&statement.body)
        ),
        StmtKind::Function(function) => render_function(function),
        StmtKind::Class(class) => render_class(class),
        StmtKind::Return(value) => match value {
            Some(value) => format!("(return {})", render(value)),
            None => "return".to_string(),
        },
        StmtKind::Break(label) => match label {
            Some(label) => format!("(break {})", label.name),
            None => "break".to_string(),
        },
        StmtKind::Continue(label) => match label {
            Some(label) => format!("(continue {})", label.name),
            None => "continue".to_string(),
        },
    }
}

/// A statement list rendered as `{a b}`.
pub(in crate::parser) fn render_block(body: &[Stmt]) -> String {
    let rendered: Vec<String> = body.iter().map(render_statement).collect();
    format!("{{{}}}", rendered.join(" "))
}

/// A declaration rendered as `(let a=1 b)`.
pub(in crate::parser) fn render_declaration(declaration: &Declaration) -> String {
    let names: Vec<String> = declaration
        .declarators
        .iter()
        .map(|declarator| match &declarator.initializer {
            Some(value) => format!("{}={}", render_binding(&declarator.binding), render(value)),
            None => render_binding(&declarator.binding),
        })
        .collect();
    format!("({} {})", declaration.kind.as_str(), names.join(" "))
}
