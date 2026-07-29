//! Where a variable lives — §9.1's Declarative Environment Records.
//!
//! # Why a variable is not simply a slot in a frame
//!
//! Because a function outlives the call it was written in:
//!
//! ```text
//! function counter() { var n = 0; return function () { n = n + 1; return n; }; }
//! var next = counter();
//! next(); next();     // 1, then 2
//! ```
//!
//! `counter` has returned by the time `next` runs, so `n` cannot be in `counter`'s frame — the
//! frame is gone. It is in an **environment**, which the inner function holds a reference to and
//! which lives as long as anything can still reach it. That is the whole of what a closure is,
//! and it is why capturing by *value* at creation would be wrong rather than merely slow: the two
//! calls above would both answer 1.
//!
//! # How a name is found
//!
//! Every environment has a parent, and the chain ends at the script's. The compiler knows the
//! shape of that chain — it built it — so a name is resolved to a **depth and an index** rather
//! than searched for at run time: how many parents out, and which slot there. Nothing here
//! compares a string.
//!
//! # What this costs
//!
//! An allocation per call, and a pointer walk per variable that is not the running function's
//! own. Real engines pay neither: they work out which variables are actually captured and leave
//! the rest in the frame. That is an M8 experiment with a benchmark in front of it — the shape
//! here is the one the specification describes, and it is the one that is obviously right.

use crate::heap::Heap;
use crate::value::Value;

/// An environment on the heap.
///
/// Meaningful only to the [`Heap`] that issued it, on the same terms as every other handle here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentId(pub(super) usize);

/// One scope's variables, and the scope it is written inside.
#[derive(Debug)]
pub struct Environment {
    /// The variables, by the index the compiler assigned.
    ///
    /// All `undefined` to begin with, which is what makes a `var` readable before its declaration
    /// and holding nothing — hoisting is this array existing before the first instruction runs.
    ///
    /// A slot may also hold *nothing*, which is not the same as holding `undefined`. §9.1.1.1
    /// gives a declarative binding two states, and the second one is what the temporal dead zone
    /// is made of: `let` creates the binding when its block is entered and leaves it uninitialised
    /// until the declaration runs, and reading it in between is a ReferenceError rather than
    /// `undefined`. `Option<Value>` is the same sixteen bytes as `Value` — the enum has spare
    /// discriminants for the niche — so saying so costs nothing.
    slots: Vec<Option<Value>>,
    /// The environment this one is written inside, or `None` for the script's.
    ///
    /// §9.1.1's `[[OuterEnv]]`. The chain is the *lexical* nesting and not the call stack: a
    /// function called from anywhere still sees the scope it was written in, which is the
    /// difference between closures and dynamic scope.
    parent: Option<EnvironmentId>,
}

impl Environment {
    /// The variables, for the collector to walk.
    pub(super) fn slots(&self) -> &[Option<Value>] {
        &self.slots
    }

    /// The environment this one is written inside.
    pub(super) fn parent(&self) -> Option<EnvironmentId> {
        self.parent
    }
}

impl Heap {
    /// A new environment with `size` slots, written inside `parent`.
    pub fn new_environment(&mut self, parent: Option<EnvironmentId>, size: usize) -> EnvironmentId {
        let id = EnvironmentId(self.environments.len());
        self.environments.push(Some(Environment {
            slots: vec![Some(Value::Undefined); size],
            parent,
        }));
        id
    }

    /// The environment `depth` parents out from `from`.
    ///
    /// `0` is `from` itself. Answers `None` for a chain shorter than asked for, which no compiled
    /// code can ask for — the compiler counted the depth from the scopes it had built — and which
    /// a hand-written chunk can.
    pub fn environment_at(&self, from: EnvironmentId, depth: u32) -> Option<EnvironmentId> {
        let mut at = from;
        for _ in 0..depth {
            at = self.environments.get(at.0)?.as_ref()?.parent?;
        }
        Some(at)
    }

    /// What a slot of an environment holds, if there is such a slot at all.
    ///
    /// Two layers of absence, and they are different failures. The outer `None` is a slot the
    /// environment does not have, which no compiled code can ask for and a hand-written chunk can
    /// — a [`crate::vm::Fault`]. The inner one is §9.1.1.1's uninitialised binding, which a script
    /// reaches every time it reads a `let` above its declaration — a ReferenceError.
    pub fn variable(&self, environment: EnvironmentId, index: u32) -> Option<Option<Value>> {
        self.environments
            .get(environment.0)?
            .as_ref()?
            .slots
            .get(index as usize)
            .copied()
    }

    /// Put a slot back into §9.1.1.1's uninitialised state, answering whether there was one.
    ///
    /// What `let` does to its binding when its block is entered. Needed as an operation of its own
    /// because a slot is not new each time the block is: a loop body's bindings occupy the same
    /// slots on every pass, and without this the second pass would find the first pass's value
    /// sitting where the dead zone should be.
    pub fn uninitialise(&mut self, environment: EnvironmentId, index: u32) -> bool {
        let Some(slot) = self.slot_mut(environment, index) else {
            return false;
        };
        *slot = None;
        true
    }

    /// The slot itself, for the two operations that write one.
    fn slot_mut(&mut self, environment: EnvironmentId, index: u32) -> Option<&mut Option<Value>> {
        self.environments
            .get_mut(environment.0)
            .and_then(Option::as_mut)
            .and_then(|found| found.slots.get_mut(index as usize))
    }

    /// Put a value in a slot, answering whether there was a slot to put it in.
    pub fn set_variable(&mut self, environment: EnvironmentId, index: u32, value: Value) -> bool {
        let Some(slot) = self.slot_mut(environment, index) else {
            return false;
        };
        *slot = Some(value);
        true
    }

    /// How many environments this heap holds.
    pub fn environment_count(&self) -> usize {
        self.environments
            .iter()
            .filter(|slot| slot.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_environment_holds_undefined_and_nothing_else() {
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 3);
        for index in 0..3 {
            assert!(matches!(
                heap.variable(environment, index),
                Some(Some(Value::Undefined))
            ));
        }
        // Hoisting is this: the slots exist before anything runs, so a name is readable above the
        // line that declares it and holds nothing.
        assert!(heap.variable(environment, 3).is_none());
        assert!(!heap.set_variable(environment, 3, Value::Null));
        assert!(heap.set_variable(environment, 0, Value::Null));
        assert!(matches!(
            heap.variable(environment, 0),
            Some(Some(Value::Null))
        ));
        // …and a binding put back into §9.1.1.1's uninitialised state is still a slot — the
        // outer `Some` says there is one, and the inner `None` is the dead zone.
        assert!(heap.uninitialise(environment, 0));
        assert!(matches!(heap.variable(environment, 0), Some(None)));
        assert!(!heap.uninitialise(environment, 3));
        // Initialising it again fills it, which is what the declaration finally running does.
        assert!(heap.set_variable(environment, 0, Value::Null));
        assert!(matches!(
            heap.variable(environment, 0),
            Some(Some(Value::Null))
        ));
    }

    #[test]
    fn the_chain_is_walked_by_counting_rather_than_by_searching() {
        let mut heap = Heap::new();
        let script = heap.new_environment(None, 1);
        let outer = heap.new_environment(Some(script), 1);
        let inner = heap.new_environment(Some(outer), 1);
        assert_eq!(heap.environment_at(inner, 0), Some(inner));
        assert_eq!(heap.environment_at(inner, 1), Some(outer));
        assert_eq!(heap.environment_at(inner, 2), Some(script));
        // Past the end of the chain there is nothing, which is what a hand-written chunk asking
        // for a depth the compiler never counted would get.
        assert_eq!(heap.environment_at(inner, 3), None);
        assert_eq!(heap.environment_at(script, 1), None);
    }

    #[test]
    fn two_environments_with_the_same_parent_do_not_share_their_slots() {
        // One call, one environment. That is what makes each call's `var` its own and why a
        // recursive function does not overwrite its caller's variables.
        let mut heap = Heap::new();
        let parent = heap.new_environment(None, 1);
        let first = heap.new_environment(Some(parent), 1);
        let second = heap.new_environment(Some(parent), 1);
        assert!(heap.set_variable(first, 0, Value::Number(1.0)));
        assert!(matches!(
            heap.variable(second, 0),
            Some(Some(Value::Undefined))
        ));
        // …while the parent they share is one environment, which is what closing over it means.
        assert!(heap.set_variable(parent, 0, Value::Number(9.0)));
        let from_first = heap
            .environment_at(first, 1)
            .and_then(|at| heap.variable(at, 0));
        let from_second = heap
            .environment_at(second, 1)
            .and_then(|at| heap.variable(at, 0));
        assert!(matches!(from_first, Some(Some(Value::Number(value))) if value == 9.0));
        assert!(matches!(from_second, Some(Some(Value::Number(value))) if value == 9.0));
    }

    #[test]
    fn a_handle_this_heap_does_not_know_answers_rather_than_panicking() {
        // The same narrow promise every handle here makes (DR-0010): no panic and no
        // out-of-range read, and no detection.
        let mut heap = Heap::new();
        let mut other = Heap::new();
        let stranger = other.new_environment(None, 4);
        other.new_environment(None, 4);
        let past_the_end = other.new_environment(None, 4);
        assert!(heap.variable(past_the_end, 0).is_none());
        assert!(!heap.set_variable(past_the_end, 0, Value::Null));
        assert!(heap.environment_at(past_the_end, 0).is_some());
        assert!(heap.environment_at(past_the_end, 1).is_none());
        let _ = stranger;
    }
}
