//! Rendering an expression as a compact s-expression.
//!
//! The shape is deliberately terse — `(+ a (* b c))` rather than a pretty-printed tree — because
//! what a test asserts is the *structure*, and a structure you can write on one line is one you
//! can compare by eye when it is wrong.

use super::{render_binding, render_binding_element, render_block, render_target};
use crate::ast::{
    Argument, ArrayElement, ArrowBody, Class, Expr, ExprKind, Function, MethodKind,
    PropertyDefinition, PropertyKey, TemplateLiteral,
};

/// An expression rendered as a parenthesized prefix form.
///
/// Precedence and associativity are claims about *shape*, and a shape is far easier to read
/// as `(+ 1 (* 2 3))` than as three nested constructors — which matters, because a test
/// nobody can read is a test nobody checks.
pub(in crate::parser) fn render(expr: &Expr) -> String {
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
        ExprKind::Member {
            optional,
            object,
            property,
        } => format!("({} {} {})", dot(*optional), render(object), property),
        ExprKind::ComputedMember {
            optional,
            object,
            property,
        } => format!(
            "({}[] {} {})",
            if *optional { "?" } else { "" },
            render(object),
            render(property)
        ),
        ExprKind::Call {
            optional,
            callee,
            arguments,
        } => format!(
            "({}call {} [{}])",
            if *optional { "?" } else { "" },
            render(callee),
            render_arguments(arguments)
        ),
        ExprKind::New { callee, arguments } => {
            format!("(new {} [{}])", render(callee), render_arguments(arguments))
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
        ExprKind::Class(class) => render_class(class),
        ExprKind::Super => "super".to_string(),
        ExprKind::NewTarget => "new.target".to_string(),
        ExprKind::Await(argument) => format!("(await {})", render(argument)),
        ExprKind::OptionalChain(chain) => format!("(?chain {})", render(chain)),
        ExprKind::Yield(yielded) => {
            let star = if yielded.delegate { "*" } else { "" };
            match &yielded.argument {
                Some(argument) => format!("(yield{star} {})", render(argument)),
                None => format!("(yield{star})"),
            }
        }
        ExprKind::Template(quasi) => render_template(quasi),
        ExprKind::TaggedTemplate { tag, quasi } => {
            format!("(tag {} {})", render(tag), render_template(quasi))
        }
        ExprKind::Arrow(arrow) => {
            let mut parameters: Vec<String> = arrow
                .parameters
                .items
                .iter()
                .map(render_binding_element)
                .collect();
            if let Some(rest) = &arrow.parameters.rest {
                parameters.push(format!("(... {})", render_binding(rest)));
            }
            let body = match &arrow.body {
                ArrowBody::Expression(value) => render(value),
                ArrowBody::Block(body) => render_block(body),
            };
            format!(
                "({} [{}] {})",
                if arrow.is_async { "async=>" } else { "=>" },
                parameters.join(" "),
                body
            )
        }
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
                    PropertyDefinition::Method {
                        key,
                        kind,
                        function,
                    } => match kind {
                        MethodKind::Normal => {
                            format!("({} {})", render_key(key), render_function(function))
                        }
                        MethodKind::Get => {
                            format!("(get {} {})", render_key(key), render_function(function))
                        }
                        MethodKind::Set => {
                            format!("(set {} {})", render_key(key), render_function(function))
                        }
                    },
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

/// A template, rendered as `(tpl ["a" "c"] [b])`.
pub(in crate::parser) fn render_template(quasi: &TemplateLiteral) -> String {
    let parts: Vec<String> = quasi
        .quasis
        .iter()
        .map(|element| match &element.cooked {
            Some(cooked) => format!("{:?}", String::from_utf16_lossy(cooked)),
            None => "<raw>".to_string(),
        })
        .collect();
    if quasi.expressions.is_empty() {
        return format!("(tpl [{}])", parts.join(" "));
    }
    let expressions: Vec<String> = quasi.expressions.iter().map(render).collect();
    format!("(tpl [{}] [{}])", parts.join(" "), expressions.join(" "))
}

/// A function, rendered as `(fn name [params] {body})`.
pub(in crate::parser) fn render_function(function: &Function) -> String {
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
        "({} {} [{}] {})",
        match (function.is_async, function.is_generator) {
            (false, false) => "fn",
            (false, true) => "fn*",
            (true, false) => "async-fn",
            (true, true) => "async-fn*",
        },
        function.name.as_ref().map_or("<anon>", |name| &name.name),
        parameters.join(" "),
        render_block(&function.body)
    )
}

/// `.` or `?.`, for a member access.
fn dot(optional: bool) -> &'static str {
    if optional { "?." } else { "." }
}

/// An argument list, `...` marking a spread.
fn render_arguments(arguments: &[Argument]) -> String {
    let rendered: Vec<String> = arguments
        .iter()
        .map(|argument| match argument {
            Argument::Value(value) => render(value),
            Argument::Spread(value) => format!("(... {})", render(value)),
        })
        .collect();
    rendered.join(" ")
}

/// A class as `(class <name> <heritage> [element …])`, `-` standing for an absent heritage.
pub(in crate::parser) fn render_class(class: &Class) -> String {
    let elements: Vec<String> = class
        .elements
        .iter()
        .map(|element| {
            let name = render_key(&element.key);
            let body = render_function(&element.function);
            let head = match element.kind {
                MethodKind::Normal => name,
                MethodKind::Get => format!("get {name}"),
                MethodKind::Set => format!("set {name}"),
            };
            if element.is_static {
                format!("(static {head} {body})")
            } else {
                format!("({head} {body})")
            }
        })
        .collect();
    format!(
        "(class {} {} [{}])",
        class.name.as_ref().map_or("<anon>", |name| &name.name),
        class
            .heritage
            .as_ref()
            .map_or_else(|| "-".to_string(), |parent| render(parent)),
        elements.join(" ")
    )
}

/// A property key, rendered so the four forms stay apart.
pub(in crate::parser) fn render_key(key: &PropertyKey) -> String {
    match key {
        PropertyKey::Identifier(name) => name.to_string(),
        PropertyKey::String(units) => format!("s{:?}", String::from_utf16_lossy(units)),
        PropertyKey::Number(value) => format!("n{value}"),
        PropertyKey::Computed(expr) => format!("[{}]", render(expr)),
    }
}
