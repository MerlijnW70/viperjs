//! Labels, and the jumps that name them (ECMAScript §8.3, §14.8.1, §14.9.1).
//!
//! Five rules, and they are all about the same question — what a `break` or `continue` is allowed
//! to be talking about — so they are one walk rather than five:
//!
//! - `ContainsDuplicateLabels` (§8.3.1): `a: a: ;` and `a: { a: ; }` are both refused.
//! - `ContainsUndefinedBreakTarget` (§8.3.2): `break a;` needs an enclosing `a:`.
//! - `ContainsUndefinedContinueTarget` (§8.3.3): `continue a;` needs an enclosing `a:` **on a
//!   loop**.
//! - §14.8.1: every `continue`, labelled or not, must be inside an `IterationStatement`.
//! - §14.9.1: an *unlabelled* `break` must be inside an `IterationStatement` or a `switch`.
//!
//! The specification writes the first three as separate operations because that is how it defines
//! things — piecewise, one production at a time, so each can be read alone. Running them as three
//! walks over the same tree would be three times the traversal to answer one question, so they
//! share one here; each rule is still a single line, next to the section it comes from.
//!
//! # Three sets, and the one that resets
//!
//! §8.3 passes label sets down the tree, and the whole content of the label rules is *which
//! constructs extend them and which reset them*:
//!
//! | set | grown by | reset by |
//! | --- | --- | --- |
//! | `labels` — the `labelSet` of §8.3.1 and §8.3.2 | a labelled statement | nothing |
//! | `pending` — the `labelSet` of §8.3.3 | a labelled statement | *everything else* |
//! | `iteration_labels` — the `iterationSet` of §8.3.3 | a loop, from `pending` | nothing |
//!
//! `labels` never resets, which is why `a: { break a; }` works and why `a: { a: ; }` is a
//! duplicate. `pending` resets at every construct that is not a label, which is why a label only
//! ever reaches `iteration_labels` when a loop is what it labels *directly*:
//!
//! ```text
//! a: while (1) continue a;      a: is directly on the loop, so `a` names an iteration
//! a: b: while (1) continue a;   both are, labels chaining without resetting
//! a: { while (1) continue a; }  the block reset `pending`, so `a` names the block
//! while (1) { a: continue a; }  `a` labels the continue, and labels nothing iterable
//! ```
//!
//! The last two are Syntax Errors, and no rule about them says "block" or "loop body" — the reset
//! is the rule.
//!
//! # What is not here yet
//!
//! Function boundaries. §14.8.1 and §14.9.1 both say "not crossing function or static
//! initialization block boundaries", and §8.3's operations stop at a `FunctionStatementList` — so
//! a `break` inside a function nested in a loop is an error, not a break of the outer loop. There
//! are no functions to cross yet; when there are, this walk stops at them.

use crate::ast::{Stmt, StmtKind};
use crate::span::Span;
use std::rc::Rc;

/// A label rule this tree breaks, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelProblem {
    /// Which rule.
    pub kind: LabelProblemKind,
    /// The label, or the jump that has none.
    pub span: Span,
}

/// Which of the five rules was broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelProblemKind {
    /// §8.3.1: a label repeats one that already encloses it.
    DuplicateLabel,
    /// §8.3.2: `break a;` with no enclosing `a:`.
    UndefinedBreakTarget,
    /// §8.3.3: `continue a;` where `a:` labels something that is not a loop.
    UndefinedContinueTarget,
    /// §14.9.1: an unlabelled `break` outside any loop or `switch`.
    BreakOutsideLoopOrSwitch,
    /// §14.8.1: a `continue` outside any loop, labelled or not.
    ContinueOutsideLoop,
}

/// The first of §8.3's rules this statement list breaks, in source order.
///
/// `None` is the answer for a tree that breaks none of them, which is what §16.1.1 asks about a
/// `Script`. Source order is not something the specification requires — it says only whether a
/// tree contains such a thing — but a diagnostic that pointed at an arbitrary one of several
/// would be a diagnostic nobody could act on twice in a row.
///
/// ```
/// use viperjs::parser::parse_script;
/// use viperjs::static_semantics::{first_label_problem, LabelProblemKind};
///
/// let script = parse_script("a: while (1) continue a;").expect("this parses");
/// assert_eq!(first_label_problem(&script.body), None);
/// ```
pub fn first_label_problem(body: &[Stmt]) -> Option<LabelProblem> {
    let root = Frame {
        labels: None,
        pending: None,
        iteration_labels: None,
        inside_iteration: false,
        inside_switch: false,
    };
    // Reversed, so popping yields source order.
    let mut work: Vec<(&Stmt, Frame<'_>)> =
        body.iter().rev().map(|stmt| (stmt, root.clone())).collect();
    while let Some((stmt, frame)) = work.pop() {
        if let Some(problem) = visit(stmt, frame, &mut work) {
            return Some(problem);
        }
    }
    None
}

/// One link of a label set.
///
/// A set per node held as a shared cons list rather than as a `Vec` cloned down every branch: the
/// walk is iterative (see the module documentation), so every pending node carries its own three
/// sets, and a chain that is only ever extended can be shared instead of copied. Chains are as
/// long as labels nest, which is to say very short.
///
/// Shared rather than indexed into an arena, because an index would need a lookup that can fail
/// and cannot — a branch no input could reach, which is a branch that should not be written.
struct LabelLink<'a> {
    name: &'a str,
    parent: Option<Rc<LabelLink<'a>>>,
}

/// The three label sets of §8.3 and the two nesting flags of §14.8.1 and §14.9.1.
#[derive(Clone)]
struct Frame<'a> {
    /// `labelSet` of §8.3.1 and §8.3.2. Passes through every construct unchanged.
    labels: Option<Rc<LabelLink<'a>>>,
    /// `labelSet` of §8.3.3, which every construct but a labelled statement resets.
    pending: Option<Rc<LabelLink<'a>>>,
    /// `iterationSet` of §8.3.3 — the labels that name a loop.
    iteration_labels: Option<Rc<LabelLink<'a>>>,
    /// Whether an `IterationStatement` encloses this (§14.8.1, §14.9.1).
    inside_iteration: bool,
    /// Whether a `SwitchStatement` encloses this (§14.9.1).
    inside_switch: bool,
}

impl<'a> Frame<'a> {
    /// The frame a construct's children get: everything the same, except that `pending` resets.
    ///
    /// This is the default in §8.3.3 and the reason the operation says anything at all — every
    /// production but `LabelledStatement` and the loops passes `« »` down as its label set.
    fn inside(&self) -> Self {
        Self {
            pending: None,
            ..self.clone()
        }
    }
}

/// Whether `chain` holds `name`.
fn contains(chain: &Option<Rc<LabelLink<'_>>>, name: &str) -> bool {
    let mut link = chain.as_ref();
    while let Some(entry) = link {
        if entry.name == name {
            return true;
        }
        link = entry.parent.as_ref();
    }
    false
}

/// Add `name` to a chain, returning the new head.
fn push_label<'a>(parent: &Option<Rc<LabelLink<'a>>>, name: &'a str) -> Option<Rc<LabelLink<'a>>> {
    Some(Rc::new(LabelLink {
        name,
        parent: parent.clone(),
    }))
}

/// One statement: check what it breaks, and queue what it contains.
fn visit<'a>(
    stmt: &'a Stmt,
    frame: Frame<'a>,
    work: &mut Vec<(&'a Stmt, Frame<'a>)>,
) -> Option<LabelProblem> {
    match &stmt.kind {
        StmtKind::Labelled(statement) => {
            // §8.3.1. `labels` never resets, so this catches a repeat however many blocks and
            // loops stand between the two.
            if contains(&frame.labels, &statement.label.name) {
                return Some(LabelProblem {
                    kind: LabelProblemKind::DuplicateLabel,
                    span: statement.label.span,
                });
            }
            let labels = push_label(&frame.labels, &statement.label.name);
            let pending = push_label(&frame.pending, &statement.label.name);
            work.push((
                &statement.body,
                Frame {
                    labels,
                    pending,
                    ..frame
                },
            ));
        }
        StmtKind::Break(label) => match label {
            // §8.3.2. A labelled `break` needs no enclosing loop or switch — §14.9.1 is stated
            // about `break ;` alone — so a defined label is the whole of the rule.
            Some(label) => {
                if !contains(&frame.labels, &label.name) {
                    return Some(LabelProblem {
                        kind: LabelProblemKind::UndefinedBreakTarget,
                        span: label.span,
                    });
                }
            }
            // §14.9.1.
            None => {
                if !frame.inside_iteration && !frame.inside_switch {
                    return Some(LabelProblem {
                        kind: LabelProblemKind::BreakOutsideLoopOrSwitch,
                        span: stmt.span,
                    });
                }
            }
        },
        StmtKind::Continue(label) => {
            // §14.8.1, which unlike §14.9.1 is stated about *both* forms: a `continue` is always
            // inside a loop, whatever it names.
            if !frame.inside_iteration {
                return Some(LabelProblem {
                    kind: LabelProblemKind::ContinueOutsideLoop,
                    span: stmt.span,
                });
            }
            // §8.3.3, against `iterationSet` and not `labelSet` — the difference between a label
            // that names a loop and one that merely encloses this jump.
            if let Some(label) = label
                && !contains(&frame.iteration_labels, &label.name)
            {
                return Some(LabelProblem {
                    kind: LabelProblemKind::UndefinedContinueTarget,
                    span: label.span,
                });
            }
        }
        // §8.3.3 for an `IterationStatement`: the labels waiting in `pending` become part of
        // `iterationSet`, and the body starts with `« »`. That banking is what makes
        // `a: while (1) continue a;` legal and `a: { while (1) continue a; }` not.
        StmtKind::While(statement) => {
            work.push((&statement.body, frame.entering_loop()));
        }
        StmtKind::DoWhile(statement) => {
            work.push((&statement.body, frame.entering_loop()));
        }
        StmtKind::For(statement) => {
            work.push((&statement.body, frame.entering_loop()));
        }
        StmtKind::ForInOf(statement) => {
            work.push((&statement.body, frame.entering_loop()));
        }
        StmtKind::Switch(statement) => {
            let inner = Frame {
                inside_switch: true,
                ..frame.inside()
            };
            work.extend(
                statement
                    .cases
                    .iter()
                    .rev()
                    .flat_map(|case| case.body.iter().rev())
                    .map(|stmt| (stmt, inner.clone())),
            );
        }
        StmtKind::Block(body) => {
            work.extend(body.iter().rev().map(|stmt| (stmt, frame.inside())));
        }
        StmtKind::If(statement) => {
            let inner = frame.inside();
            if let Some(alternate) = &statement.alternate {
                work.push((alternate, inner.clone()));
            }
            work.push((&statement.consequent, inner));
        }
        StmtKind::With(statement) => work.push((&statement.body, frame.inside())),
        StmtKind::Try(statement) => {
            let inner = frame.inside();
            if let Some(finalizer) = &statement.finalizer {
                work.extend(finalizer.iter().rev().map(|stmt| (stmt, inner.clone())));
            }
            if let Some(handler) = &statement.handler {
                work.extend(handler.body.iter().rev().map(|stmt| (stmt, inner.clone())));
            }
            work.extend(
                statement
                    .block
                    .iter()
                    .rev()
                    .map(|stmt| (stmt, inner.clone())),
            );
        }
        // A function body is where every one of these operations stops (§8.3 defines them over
        // a `FunctionStatementList` separately, from `« »`), so a `break` inside a function
        // cannot see a loop outside it — `while (1) { function f() { break; } }` is a Syntax
        // Error, and the body is asked all five rules again on its own.
        StmtKind::Function(_) | StmtKind::Class(_) => {}
        // §8.3 gives all of these `false`, and none of them holds a statement to look inside.
        StmtKind::Empty
        | StmtKind::Expression(_)
        | StmtKind::Debugger
        | StmtKind::Declaration(_)
        | StmtKind::Return(_)
        | StmtKind::Throw(_) => {}
    }
    None
}

impl<'a> Frame<'a> {
    /// The frame a loop body gets: `pending` banked into `iterationSet`, and emptied.
    ///
    /// §8.3.3's `newIterationSet` — the labels waiting on this loop become the ones a `continue`
    /// may name, and the body starts over with none pending.
    fn entering_loop(&self) -> Self {
        let mut iteration_labels = self.iteration_labels.clone();
        let mut link = self.pending.as_ref();
        while let Some(entry) = link {
            iteration_labels = push_label(&iteration_labels, entry.name);
            link = entry.parent.as_ref();
        }
        Self {
            pending: None,
            iteration_labels,
            inside_iteration: true,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script_with_label_rules_unchecked as parse_script;

    /// The first label problem in `source`, if any.
    fn problem(source: &str) -> Option<LabelProblemKind> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // a test about the walk needs a tree
        first_label_problem(&script.body).map(|problem| problem.kind)
    }

    #[test]
    fn a_label_may_not_repeat_one_that_already_encloses_it() {
        // §8.3.1, whose `labelSet` passes through every construct — so distance does not help.
        assert_eq!(problem("a: a: ;"), Some(LabelProblemKind::DuplicateLabel));
        assert_eq!(
            problem("a: { a: ; }"),
            Some(LabelProblemKind::DuplicateLabel)
        );
        assert_eq!(
            problem("a: while (1) { a: ; }"),
            Some(LabelProblemKind::DuplicateLabel)
        );
        assert_eq!(
            problem("a: { b: { a: ; } }"),
            Some(LabelProblemKind::DuplicateLabel)
        );
        assert_eq!(
            problem("a: try { a: ; } finally {}"),
            Some(LabelProblemKind::DuplicateLabel)
        );
        assert_eq!(
            problem("a: switch (x) { case 1: a: ; }"),
            Some(LabelProblemKind::DuplicateLabel)
        );
        // …and siblings are not nested, which is the half that makes it about enclosure.
        assert_eq!(problem("a: ; a: ;"), None);
        assert_eq!(problem("{ a: ; } { a: ; }"), None);
        assert_eq!(problem("a: b: c: ;"), None);
        assert_eq!(problem("a: ; b: ;"), None);
    }

    #[test]
    fn a_break_needs_its_label_defined_and_an_unlabelled_one_needs_a_loop_or_switch() {
        // §8.3.2: `labelSet` never resets, so any enclosing label will do — a `break` does not
        // need a loop when it names something.
        assert_eq!(problem("a: { break a; }"), None);
        assert_eq!(problem("a: while (1) break a;"), None);
        assert_eq!(problem("a: switch (x) { case 1: break a; }"), None);
        assert_eq!(problem("a: with (b) { break a; }"), None);
        assert_eq!(problem("a: b: { break a; }"), None);
        assert_eq!(
            problem("break a;"),
            Some(LabelProblemKind::UndefinedBreakTarget)
        );
        assert_eq!(
            problem("a: ; { break a; }"),
            Some(LabelProblemKind::UndefinedBreakTarget)
        );
        assert_eq!(
            problem("while (1) { a: ; break a; }"),
            Some(LabelProblemKind::UndefinedBreakTarget)
        );
        // §14.9.1, which is stated about `break ;` alone.
        assert_eq!(
            problem("break;"),
            Some(LabelProblemKind::BreakOutsideLoopOrSwitch)
        );
        assert_eq!(
            problem("a: { break; }"),
            Some(LabelProblemKind::BreakOutsideLoopOrSwitch)
        );
        assert_eq!(problem("while (1) break;"), None);
        assert_eq!(problem("switch (x) { case 1: break; }"), None);
        assert_eq!(problem("for (;;) { if (x) break; }"), None);
        assert_eq!(
            problem("while (1) {} break;"),
            Some(LabelProblemKind::BreakOutsideLoopOrSwitch)
        );
    }

    #[test]
    fn a_continue_needs_a_loop_and_its_label_must_name_one() {
        // §14.8.1, stated about *both* forms — so this fires before the label is even considered.
        assert_eq!(
            problem("continue;"),
            Some(LabelProblemKind::ContinueOutsideLoop)
        );
        assert_eq!(
            problem("a: { continue a; }"),
            Some(LabelProblemKind::ContinueOutsideLoop)
        );
        assert_eq!(
            problem("switch (x) { case 1: continue; }"),
            Some(LabelProblemKind::ContinueOutsideLoop)
        );
        // §8.3.3: the label must be in `iterationSet`, which only a label directly on a loop
        // reaches. Every one of these is inside a loop, so §14.8.1 is satisfied and the label is
        // the whole complaint.
        assert_eq!(
            problem("while (1) { a: continue a; }"),
            Some(LabelProblemKind::UndefinedContinueTarget),
            "`a` labels the continue, which is not an iteration statement"
        );
        assert_eq!(
            problem("a: { while (1) continue a; }"),
            Some(LabelProblemKind::UndefinedContinueTarget),
            "the block reset `pending`, so `a` never named the loop"
        );
        assert_eq!(
            problem("a: if (x) while (1) continue a;"),
            Some(LabelProblemKind::UndefinedContinueTarget)
        );
        assert_eq!(
            problem("a: switch (x) { case 1: while (1) continue a; }"),
            Some(LabelProblemKind::UndefinedContinueTarget)
        );
        // …and a label directly on a loop does reach it, however many of them chain.
        assert_eq!(problem("a: while (1) continue a;"), None);
        assert_eq!(problem("a: b: while (1) continue a;"), None);
        assert_eq!(problem("a: b: while (1) continue b;"), None);
        assert_eq!(problem("a: for (;;) continue a;"), None);
        assert_eq!(problem("a: do continue a; while (1);"), None);
        assert_eq!(problem("a: for (x in y) continue a;"), None);
        assert_eq!(problem("a: while (1) { b: continue a; }"), None);
        assert_eq!(problem("a: while (1) { while (2) continue a; }"), None);
        assert_eq!(problem("while (1) continue;"), None);
        assert_eq!(
            problem("a: while (1) { while (2) { b: ; continue a; } }"),
            None
        );
    }

    #[test]
    fn the_walk_reports_the_first_problem_in_source_order() {
        // Not something the specification asks for — it says only whether a tree contains one —
        // but a diagnostic that pointed at an arbitrary one of several would be useless twice.
        assert_eq!(
            problem("break x; break y;"),
            Some(LabelProblemKind::UndefinedBreakTarget)
        );
        let script = parse_script("a: ; break zz;").expect("this parses");
        let found = first_label_problem(&script.body).expect("there is one"); // the test is about which
        assert_eq!(found.span, Span::new(11, 13));
        assert_eq!(found.span.slice("a: ; break zz;"), Some("zz"));
        // An unlabelled jump points at the whole statement, there being no label to point at.
        let script = parse_script("{ break; }").expect("this parses");
        let found = first_label_problem(&script.body).expect("there is one"); // same
        assert_eq!(found.span.slice("{ break; }"), Some("break;"));
    }

    #[test]
    fn a_tree_far_deeper_than_the_parser_can_build_costs_the_walk_no_stack() {
        // The walk carries a frame per pending node, so it is the one here with something to get
        // wrong about depth — see the module documentation for why that is a worklist and an
        // arena rather than recursion and a cloned set.
        let deep = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let mut stmt = Stmt {
                    kind: StmtKind::Break(None),
                    span: Span::new(0, 5),
                };
                for _ in 0..1_000 {
                    stmt = Stmt {
                        kind: StmtKind::Block(Box::new([stmt])),
                        span: Span::new(0, 5),
                    };
                }
                first_label_problem(&[stmt]).map(|problem| problem.kind)
            })
            .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
        assert_eq!(
            deep.join()
                .unwrap_or_else(|_| panic!("the walk did not survive a thousand nested blocks")), // the panic IS the assertion
            Some(LabelProblemKind::BreakOutsideLoopOrSwitch)
        );
    }
}
