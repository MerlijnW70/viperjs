//! Helpers shared by the parser's test modules.

use super::{ParseError, parse_expression};
use crate::ast::{
    Declaration, Expr, ExprKind, ForInOfKind, ForInOfTarget, ForInit, RegExpLiteral, Stmt, StmtKind,
};

/// The parsed expression of `source`.
pub(super) fn parse(source: &str) -> Expr {
    parse_expression(source)
        .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)) // a test about a tree cannot proceed without one
}

/// An expression rendered as a parenthesized prefix form.
///
/// Precedence and associativity are claims about *shape*, and a shape is far easier to read
/// as `(+ 1 (* 2 3))` than as three nested constructors — which matters, because a test
/// nobody can read is a test nobody checks.
pub(super) fn render(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::This => "this".to_string(),
        ExprKind::Null => "null".to_string(),
        ExprKind::Boolean(value) => value.to_string(),
        ExprKind::Number(value) => value.to_string(),
        ExprKind::Identifier(name) => name.clone(),
        ExprKind::String(units) => format!("{units:?}"),
        ExprKind::RegExp(literal) => format!("/{}/{}", literal.body, literal.flags),
        ExprKind::Unary { operator, argument } => {
            format!("({} {})", operator.as_str(), render(argument))
        }
        ExprKind::Binary {
            operator,
            left,
            right,
        } => format!("({} {} {})", operator.as_str(), render(left), render(right)),
        ExprKind::Logical {
            operator,
            left,
            right,
        } => format!("({} {} {})", operator.as_str(), render(left), render(right)),
        ExprKind::Conditional {
            test,
            consequent,
            alternate,
        } => format!(
            "(? {} {} {})",
            render(test),
            render(consequent),
            render(alternate)
        ),
        ExprKind::Assignment {
            operator,
            target,
            value,
        } => format!(
            "({} {} {})",
            operator.as_str(),
            render(target),
            render(value)
        ),
        ExprKind::Member { object, property } => format!("(. {} {})", render(object), property),
        ExprKind::ComputedMember { object, property } => {
            format!("([] {} {})", render(object), render(property))
        }
        ExprKind::Call { callee, arguments } => {
            let rendered: Vec<String> = arguments.iter().map(render).collect();
            format!("(call {} [{}])", render(callee), rendered.join(" "))
        }
        ExprKind::New { callee, arguments } => {
            let rendered: Vec<String> = arguments.iter().map(render).collect();
            format!("(new {} [{}])", render(callee), rendered.join(" "))
        }
        ExprKind::Update {
            operator,
            prefix,
            argument,
        } => format!(
            "({}{} {})",
            if *prefix { "pre" } else { "post" },
            operator.as_str(),
            render(argument)
        ),
        ExprKind::Sequence(parts) => {
            let rendered: Vec<String> = parts.iter().map(render).collect();
            format!("(, {})", rendered.join(" "))
        }
    }
}

/// The shape of the tree `source` parses to.
pub(super) fn shape(source: &str) -> String {
    render(&parse(source))
}

/// A regular expression node, spelled the way the tests want to read it.
pub(super) fn regexp(body: &str, flags: &str) -> ExprKind {
    ExprKind::RegExp(Box::new(RegExpLiteral {
        body: body.to_string(),
        flags: flags.to_string(),
    }))
}

/// The error `source` fails with.
pub(super) fn error(source: &str) -> ParseError {
    match parse_expression(source) {
        Err(err) => err,
        Ok(expr) => panic!("{source:?} should not parse, got {expr:?}"), // a test about an error cannot proceed without one
    }
}

/// A declaration rendered as `(let a=1 b)`.
pub(super) fn render_declaration(declaration: &Declaration) -> String {
    let names: Vec<String> = declaration
        .declarators
        .iter()
        .map(|declarator| match &declarator.initializer {
            Some(value) => format!("{}={}", declarator.name, render(value)),
            None => declarator.name.to_string(),
        })
        .collect();
    format!("({} {})", declaration.kind.as_str(), names.join(" "))
}

/// A statement list rendered as `{a b}`.
pub(super) fn render_block(body: &[Stmt]) -> String {
    let rendered: Vec<String> = body.iter().map(render_statement).collect();
    format!("{{{}}}", rendered.join(" "))
}

/// A statement rendered compactly, for tests about statement structure.
///
/// Blocks print as `{a b}` and expression statements as their expression, so a script reads as
/// the list of shapes it is — which is what a test about semicolon insertion wants to assert.
pub(super) fn render_statement(stmt: &Stmt) -> String {
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
            match statement.kind {
                ForInOfKind::In => "in",
                ForInOfKind::Of => "of",
            },
            match &statement.left {
                ForInOfTarget::Expression(expr) => render(expr),
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
                        format!("(catch {} {})", parameter.name, render_block(&handler.body))
                    }
                    None => format!("(catch {})", render_block(&handler.body)),
                });
            }
            if let Some(finalizer) = &statement.finalizer {
                parts.push(format!("(finally {})", render_block(finalizer)));
            }
            format!("{})", parts.join(" "))
        }
        StmtKind::Break => "break".to_string(),
        StmtKind::Continue => "continue".to_string(),
    }
}
