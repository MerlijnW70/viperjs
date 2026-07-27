//! Rendering the two pattern grammars and the bindings they create.
//!
//! Two families that look alike and are not: a `Binding` creates names and a `Pattern` assigns to
//! targets that already exist. They render alike on purpose, so that a test comparing the two —
//! and several do — is comparing the trees rather than two notations.

use super::{render, render_key};
use crate::ast::{
    AssignmentTarget, Binding, BindingElement, BindingPattern, Pattern, PatternElement,
};

/// A binding: a name, or the pattern of them a declaration creates.
pub(in crate::parser) fn render_binding(binding: &Binding) -> String {
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
pub(in crate::parser) fn render_binding_element(element: &BindingElement) -> String {
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
pub(in crate::parser) fn render_target(target: &AssignmentTarget) -> String {
    match target {
        AssignmentTarget::Simple(expr) => render(expr),
        AssignmentTarget::Pattern(pattern) => render_pattern(pattern),
    }
}

/// A destructuring pattern, rendered so a hole and a default are both visible.
pub(in crate::parser) fn render_pattern(pattern: &Pattern) -> String {
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
pub(in crate::parser) fn render_element(element: &PatternElement) -> String {
    match &element.default {
        Some(default) => format!("(= {} {})", render_target(&element.target), render(default)),
        None => render_target(&element.target),
    }
}
