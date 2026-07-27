//! Helpers shared by the parser's test modules.

use super::{ParseError, parse_expression};
use crate::ast::{Expr, ExprKind, RegExpLiteral};

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
