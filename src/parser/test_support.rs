//! Helpers shared by the parser's test modules.

use super::{ParseError, parse_expression};
use crate::ast::{
    ArrayElement, AssignmentTarget, Binding, BindingElement, BindingPattern, Declaration, Expr,
    ExprKind, ForInOfKind, ForInOfTarget, ForInit, Function, Pattern, PatternElement,
    PropertyDefinition, PropertyKey, RegExpLiteral, Stmt, StmtKind,
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
            render_target(target),
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
        ExprKind::Function(function) => render_function(function),
        ExprKind::Array(elements) => {
            let rendered: Vec<String> = elements
                .iter()
                .map(|element| match element {
                    ArrayElement::Hole => "<hole>".to_string(),
                    ArrayElement::Value(value) => render(value),
                    ArrayElement::Spread(value) => format!("(... {})", render(value)),
                })
                .collect();
            format!("[{}]", rendered.join(" "))
        }
        ExprKind::Object(properties) => {
            let rendered: Vec<String> = properties
                .iter()
                .map(|property| match property {
                    PropertyDefinition::KeyValue { key, value } => {
                        format!("({} {})", render_key(key), render(value))
                    }
                    PropertyDefinition::Shorthand { name, .. } => name.to_string(),
                    PropertyDefinition::ShorthandWithDefault { name, default, .. } => {
                        format!("(= {} {})", name, render(default))
                    }
                    PropertyDefinition::Spread(value) => format!("(... {})", render(value)),
                })
                .collect();
            format!("{{{}}}", rendered.join(" "))
        }
        ExprKind::Sequence(parts) => {
            let rendered: Vec<String> = parts.iter().map(render).collect();
            format!("(, {})", rendered.join(" "))
        }
    }
}

/// A function, rendered as `(fn name [params] {body})`.
pub(super) fn render_function(function: &Function) -> String {
    let mut parameters: Vec<String> = function
        .parameters
        .items
        .iter()
        .map(render_binding_element)
        .collect();
    if let Some(rest) = &function.parameters.rest {
        parameters.push(format!("(... {})", render_binding(rest)));
    }
    format!(
        "(fn {} [{}] {})",
        function.name.as_ref().map_or("<anon>", |name| &name.name),
        parameters.join(" "),
        render_block(&function.body)
    )
}

/// A binding: a name, or the pattern of them a declaration creates.
pub(super) fn render_binding(binding: &Binding) -> String {
    match binding {
        Binding::Identifier(name) => name.name.to_string(),
        Binding::Pattern(BindingPattern::Array(pattern)) => {
            let mut parts: Vec<String> = pattern
                .elements
                .iter()
                .map(|element| match element {
                    Some(element) => render_binding_element(element),
                    None => "<hole>".to_string(),
                })
                .collect();
            if let Some(rest) = &pattern.rest {
                parts.push(format!("(... {})", render_binding(rest)));
            }
            format!("[{}]", parts.join(" "))
        }
        Binding::Pattern(BindingPattern::Object(pattern)) => {
            let mut parts: Vec<String> = pattern
                .properties
                .iter()
                .map(|property| {
                    format!(
                        "({} {})",
                        render_key(&property.key),
                        render_binding_element(&property.value)
                    )
                })
                .collect();
            if let Some(rest) = &pattern.rest {
                parts.push(format!("(... {})", rest.name));
            }
            format!("{{{}}}", parts.join(" "))
        }
    }
}

/// One binding, and its default if it has one.
fn render_binding_element(element: &BindingElement) -> String {
    match &element.default {
        Some(default) => format!(
            "(= {} {})",
            render_binding(&element.target),
            render(default)
        ),
        None => render_binding(&element.target),
    }
}

/// An assignment target: an expression, or the pattern a literal covered.
pub(super) fn render_target(target: &AssignmentTarget) -> String {
    match target {
        AssignmentTarget::Simple(expr) => render(expr),
        AssignmentTarget::Pattern(pattern) => render_pattern(pattern),
    }
}

/// A destructuring pattern, rendered so a hole and a default are both visible.
fn render_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Array(pattern) => {
            let mut parts: Vec<String> = pattern
                .elements
                .iter()
                .map(|element| match element {
                    Some(element) => render_element(element),
                    None => "<hole>".to_string(),
                })
                .collect();
            if let Some(rest) = &pattern.rest {
                parts.push(format!("(... {})", render_target(rest)));
            }
            format!("[{}]", parts.join(" "))
        }
        Pattern::Object(pattern) => {
            let mut parts: Vec<String> = pattern
                .properties
                .iter()
                .map(|property| {
                    format!(
                        "({} {})",
                        render_key(&property.key),
                        render_element(&property.value)
                    )
                })
                .collect();
            if let Some(rest) = &pattern.rest {
                parts.push(format!("(... {})", render(rest)));
            }
            format!("{{{}}}", parts.join(" "))
        }
    }
}

/// One target, and its default if it has one.
fn render_element(element: &PatternElement) -> String {
    match &element.default {
        Some(default) => format!("(= {} {})", render_target(&element.target), render(default)),
        None => render_target(&element.target),
    }
}

/// A property key, rendered so the four forms stay apart.
fn render_key(key: &PropertyKey) -> String {
    match key {
        PropertyKey::Identifier(name) => name.to_string(),
        PropertyKey::String(units) => format!("s{:?}", String::from_utf16_lossy(units)),
        PropertyKey::Number(value) => format!("n{value}"),
        PropertyKey::Computed(expr) => format!("[{}]", render(expr)),
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
            Some(value) => format!("{}={}", render_binding(&declarator.binding), render(value)),
            None => render_binding(&declarator.binding),
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
