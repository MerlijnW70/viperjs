//! The syntax tree.
//!
//! Every node is a value that owns what it means and carries a [`Span`] saying where it came
//! from — see `decisions/DR-0005-ast-owns-its-data-and-carries-spans.md` for why those are two
//! separate decisions and why the span is never allowed to become the second copy of the data.
//!
//! The tree grows one grammar slice at a time, so what is here is what the parser can build
//! today: a `Script` of statements, and expressions down to `PrimaryExpression`.
//!
//! Split by grammar layer, the way §13 and §14 are:
//!
//! - `statement` — §14, plus the declarations that appear only in a `StatementList`.
//! - `expression` — §13, down to the literals.
//! - `operator` — the operator enums of §13.4 – §13.15, and how each is written.
//! - `pattern` — destructuring assignment patterns (§13.15.5), and what a literal covers.
//! - `binding` — destructuring binding patterns (§14.3.3), which are the other kind.
//! - `function` — function definitions (§15.2), and where their names go.
//!
//! Everything is re-exported here, so `crate::ast::Whatever` names it wherever it lives.

mod binding;
mod expression;
mod function;
mod literal;
mod operator;
mod pattern;
mod statement;

pub use self::binding::{
    ArrayBindingPattern, Binding, BindingElement, BindingName, BindingPattern, BindingProperty,
    ObjectBindingPattern,
};
pub use self::expression::{Argument, Expr, ExprKind, YieldExpression};
pub(crate) use self::function::key_is;
pub use self::function::{
    ArrowBody, ArrowFunction, Class, ClassElement, ClassField, ClassMethod, ClassStaticBlock,
    FormalParameters, Function,
};
pub use self::literal::{
    ArrayElement, MethodKind, PropertyDefinition, PropertyKey, RegExpLiteral, TemplateElement,
    TemplateLiteral,
};
pub use self::operator::{
    AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator, UpdateOperator,
};
pub use self::pattern::{
    ArrayPattern, AssignmentTarget, ObjectPattern, Pattern, PatternElement, PatternProperty,
};
pub use self::statement::{
    CatchClause, CatchParameter, Declaration, DeclarationKind, Declarator, DoWhileStatement,
    ForInOfKind, ForInOfStatement, ForInOfTarget, ForInit, ForStatement, IfStatement, Label,
    LabelledStatement, Script, Stmt, StmtKind, SwitchCase, SwitchStatement, TryStatement,
    WhileStatement, WithStatement,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn expr(kind: ExprKind) -> Expr {
        Expr::new(kind, Span::new(0, 1))
    }

    #[test]
    fn parenthesizing_marks_the_node_and_widens_its_span_without_touching_its_meaning() {
        // The flag exists for early errors that distinguish `(a) = 1` from `(a, b) = 1`, and the
        // widened span exists because that is the text a diagnostic should underline. Neither may
        // change what the expression *is*.
        let inner = expr(ExprKind::Identifier("a".to_string()));
        let outer = inner.clone().in_parentheses(Span::new(0, 3));
        assert_eq!(outer.kind, inner.kind);
        assert_eq!(outer.span, Span::new(0, 3));
        assert!(outer.parenthesized);
        assert!(!inner.parenthesized);
        // Doing it twice is idempotent in everything but the span: no rule counts the brackets.
        let twice = outer.clone().in_parentheses(Span::new(0, 5));
        assert!(twice.parenthesized);
        assert_eq!(twice.span, Span::new(0, 5));
        assert_eq!(twice.kind, inner.kind);
    }

    #[test]
    fn no_single_variant_is_allowed_to_set_the_size_of_every_expression() {
        // An enum is as large as its largest variant, and the parser holds `Expr` values on its
        // stack once per level of nesting — so a fat variant is paid for by
        // `MAX_NESTING_DEPTH`, in levels. The regular expression literal is the only one that
        // needed boxing; this is the assertion that says so, and that would fail if a later
        // slice added another.
        assert!(
            size_of::<ExprKind>() <= 32,
            "ExprKind grew to {} bytes — box the variant that did it",
            size_of::<ExprKind>()
        );
        assert!(
            size_of::<Expr>() <= 48,
            "Expr is {} bytes",
            size_of::<Expr>()
        );
        // Statements nest too — `{ { { … } } }` recurses once per brace — so the same rule
        // applies to them, with more room to spare because there are fewer of them.
        assert!(
            size_of::<StmtKind>() <= 24,
            "StmtKind grew to {} bytes — box the variant that did it",
            size_of::<StmtKind>()
        );
        assert!(
            size_of::<Stmt>() <= 32,
            "Stmt is {} bytes",
            size_of::<Stmt>()
        );
    }

    #[test]
    fn two_literals_are_equal_when_they_denote_the_same_value_however_they_were_written() {
        // Equality is over meaning, not spelling — the span is the only record of how a value
        // was written, and DR-0005 forbids reading meaning back out of it.
        assert_eq!(ExprKind::Number(1000.0), ExprKind::Number(1e3));
        assert_ne!(ExprKind::Number(0.0), ExprKind::Number(1.0));
        assert_eq!(ExprKind::String(vec![0x61]), ExprKind::String(vec![0x61]));
        assert_ne!(ExprKind::Boolean(true), ExprKind::Boolean(false));
        assert_ne!(ExprKind::Null, ExprKind::This);
        // A string value may hold an unpaired surrogate, which is the whole reason it is not a
        // `String` (DR-0004).
        assert_eq!(
            ExprKind::String(vec![0xd800]),
            ExprKind::String(vec![0xd800])
        );
    }
}
