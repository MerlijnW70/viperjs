//! Helpers shared by the parser's test modules.
//!
//! The renderers are split by what they render — [`expression`], [`binding`], [`statement`] — and
//! re-exported here, so a test module's `use test_support::*` reaches all of them and none of them
//! has to know where the others live. What stays in this file is the driving half: the four ways a
//! test gets a tree or an error out of a source string.

mod binding;
mod expression;
mod statement;

pub(in crate::parser) use self::binding::*;
pub(in crate::parser) use self::expression::*;
pub(in crate::parser) use self::statement::*;

use super::{ParseError, parse_expression};
use crate::ast::{Expr, ExprKind, RegExpLiteral};

/// The parsed expression of `source`.
pub(super) fn parse(source: &str) -> Expr {
    parse_expression(source)
        .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)) // a test about a tree cannot proceed without one
}

/// The statements of `source`, rendered compactly.
///
/// Panics if `source` does not parse: a test about a tree cannot proceed without one, and the
/// message names the source so the failure reads without a debugger.
pub(super) fn statements(source: &str) -> Vec<String> {
    let script = crate::parser::parse_script(source)
        .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // needs the tree
    script.body.iter().map(render_statement).collect()
}

/// The error `source` fails with as a script.
///
/// Panics if it parses, for the mirror of the reason above.
pub(super) fn script_error(source: &str) -> ParseError {
    match crate::parser::parse_script(source) {
        Err(err) => err,
        Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // needs the error
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
