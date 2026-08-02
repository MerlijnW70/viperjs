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
use std::rc::Rc;

/// An environment on the heap.
///
/// Meaningful only to the [`Heap`] that issued it, on the same terms as every other handle here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentId(pub(super) usize);

/// What a source called one slot, for the one reader that has to ask by name — DR-0018.
///
/// Nothing in ordinary compiled code consults this. A name was resolved to a depth and an index
/// when the code was compiled, and that is the whole of how a variable is found. A **direct**
/// `eval` is the exception the record is about: §19.2.1.1 hands the evaluated source the caller's
/// running lexical environment as its outer scope, and the compiler that handles that source never
/// saw the scopes it has to resolve into. So the scopes carry their names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// What the source calls it.
    ///
    /// A slot the compiler made for its own use is named with a leading `%`, which is in neither
    /// `IdentifierStart` nor `IdentifierPart` — so it takes its place in the list, keeping index
    /// and slot in step, and no source text can ask for it. A binding that has gone **out of
    /// scope** before its environment ended is spelled the same way, and for the same reason: the
    /// slot is still there and its name must no longer resolve.
    pub name: Box<str>,
    /// What an assignment to it does — §9.1.1.1.5.
    ///
    /// Carried because the compiler that resolves a name is the one that decides this, and a
    /// compiler seeded from a running chain has nowhere else to learn it. Without it
    /// `const x = 1; eval("x = 2")` would assign.
    pub mutability: Mutability,
}

/// What an assignment to a binding does — §9.1.1.1.5, which has three answers and not two.
///
/// The two refusals are different, and §9.1.1.1.5 spells the difference as the `S` argument to
/// `CreateImmutableBinding`. Step 2 sets `S` to true when the *binding* is a strict one whatever
/// the assignment said; step 5.b then throws only when `S` is true. So an immutable binding created
/// with `S` false is one that a sloppy assignment silently fails to change — which is not the same
/// as one it changes, and is exactly what makes `function g() { g = 1; return g; }` answer the
/// function rather than 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability {
    /// The assignment writes. Every `var`, `let`, parameter and catch binding.
    Mutable,
    /// §14.3.1's `const` — `CreateImmutableBinding(N, **true**)`, so every assignment is a
    /// TypeError however the code around it is written.
    Const,
    /// §15.2.5's binding of a function expression's own name — `CreateImmutableBinding(N, false)`.
    ///
    /// The only binding in the language created that way, and the reason this is an enum rather
    /// than a flag. The assignment never writes; it *says so* only in strict code.
    OwnName,
}

impl Mutability {
    /// Whether an assignment written in code of this strictness may change the binding.
    ///
    /// Both refusals answer `false` — the difference between them is only whether the refusal is
    /// audible, which is [`Mutability::refusal_throws`].
    pub fn writes(self) -> bool {
        self == Self::Mutable
    }

    /// Whether refusing the assignment throws, rather than being silently ignored.
    ///
    /// §9.1.1.1.5 step 2 and step 5.b, read together: a `const` is a strict binding and forces the
    /// throw itself, and everything else throws exactly when the assignment is in strict code.
    pub fn refusal_throws(self, strict: bool) -> bool {
        self == Self::Const || strict
    }
}

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
    /// What the source called each slot, when a source named them at all.
    ///
    /// **The name at index *i* is the name of slot *i***, which is what lets a compiler seeded from
    /// this chain emit the same `(depth, index)` the original compiler did, with no second
    /// resolution rule to keep in step with the first. The list may be *shorter* than the slots —
    /// a compiled body's slot count is a high-water mark across the scopes inside it — and a slot
    /// past its end has no name and cannot be resolved to. DR-0018 is the long version.
    ///
    /// `None` is deliberate rather than a gap. The engine makes environments for its own purposes
    /// — a bound function's, a job's, a script run by the host — whose slots no source named, and a
    /// name list for them would be a list of names no program can write. An `eval` that reaches one
    /// resolves nothing there and carries on outwards, which is the same answer it would get for a
    /// scope that declares nothing.
    ///
    /// Shared with an [`Rc`] and not owned, because a loop that makes an environment per pass makes
    /// the *same* list a million times. Refcounting is what DR-0010 refuses for heap **values**,
    /// where cycles are built before user code runs; a list of names holds no value and can point
    /// at nothing, so the argument does not reach here.
    names: Option<Rc<[Binding]>>,
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
    /// A new environment with `size` slots, written inside `parent`, whose slots no source named.
    ///
    /// What the engine builds for itself. Compiled code goes through
    /// [`Heap::new_named_environment`], because a scope a program wrote is a scope a direct `eval`
    /// may have to resolve into.
    pub fn new_environment(&mut self, parent: Option<EnvironmentId>, size: usize) -> EnvironmentId {
        self.push_environment(parent, size, None)
    }

    /// The same, for a scope whose slots the source named — DR-0018.
    ///
    /// `names` is trusted to be as long as `size`: the two come from one compiled scope, which is
    /// where the invariant is established. A shorter list is not a fault, only a scope whose last
    /// slots cannot be reached by name.
    pub fn new_named_environment(
        &mut self,
        parent: Option<EnvironmentId>,
        size: usize,
        names: Rc<[Binding]>,
    ) -> EnvironmentId {
        self.push_environment(parent, size, Some(names))
    }

    /// What both of those do.
    fn push_environment(
        &mut self,
        parent: Option<EnvironmentId>,
        size: usize,
        names: Option<Rc<[Binding]>>,
    ) -> EnvironmentId {
        let id = EnvironmentId(self.environments.len());
        self.environments.push(Some(Environment {
            slots: vec![Some(Value::Undefined); size],
            parent,
            names,
        }));
        id
    }

    /// What the source called this environment's slots, if it named them.
    ///
    /// The chain is walked outwards from a direct `eval` and each level's answer handed to the
    /// compiler, which resolves into them exactly as it would into scopes it had built itself.
    pub fn environment_names(&self, environment: EnvironmentId) -> Option<&[Binding]> {
        self.environments
            .get(environment.0)?
            .as_ref()?
            .names
            .as_deref()
    }

    /// How many slots an environment has, or `None` for a handle this heap did not issue.
    ///
    /// Asked by the same walk, because a level's *depth* is counted in environments and its names
    /// may run short of its slots — so the two numbers are not interchangeable.
    pub fn environment_size(&self, environment: EnvironmentId) -> Option<usize> {
        Some(self.environments.get(environment.0)?.as_ref()?.slots.len())
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

    /// A name list, for the tests below.
    fn named(names: &[(&str, Mutability)]) -> Rc<[Binding]> {
        names
            .iter()
            .map(|(name, mutability)| Binding {
                name: (*name).into(),
                mutability: *mutability,
            })
            .collect()
    }

    #[test]
    fn a_scope_a_source_wrote_knows_what_it_called_its_slots() {
        let mut heap = Heap::new();
        let names = named(&[("x", Mutability::Mutable), ("k", Mutability::Const)]);
        let scope = heap.new_named_environment(None, 2, Rc::clone(&names));
        assert_eq!(heap.environment_names(scope), Some(&*names));
        assert_eq!(heap.environment_size(scope), Some(2));
        // The list and the slots are the same length and in the same order, which is the whole
        // invariant: index 1 of the names is slot 1, and `k` is the `const`.
        assert_eq!(heap.environment_names(scope).map(<[_]>::len), Some(2));
        assert!(heap.environment_names(scope).is_some_and(|names| {
            names[1].name.as_ref() == "k"
                && !names[1].mutability.writes()
                && names[0].mutability.writes()
        }));
        // The two refusals are different, and only one of them is audible in sloppy code — which
        // is the whole reason this is three answers rather than a flag.
        assert!(Mutability::Const.refusal_throws(false));
        assert!(Mutability::OwnName.refusal_throws(true));
        assert!(!Mutability::OwnName.refusal_throws(false));
        assert!(!Mutability::OwnName.writes());
    }

    #[test]
    fn a_scope_the_engine_made_for_itself_has_no_names_to_offer() {
        // Not the same as a scope that names nothing: a bound function's environment and a job's
        // hold slots no source wrote, so an `eval` reaching one must resolve nothing *here* and
        // carry on outwards rather than stopping.
        let mut heap = Heap::new();
        let engine = heap.new_environment(None, 3);
        assert_eq!(heap.environment_names(engine), None);
        assert_eq!(heap.environment_size(engine), Some(3));
        let empty = heap.new_named_environment(Some(engine), 0, named(&[]));
        assert_eq!(heap.environment_names(empty), Some(&[][..]));
        assert_eq!(heap.environment_size(empty), Some(0));
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
        assert_eq!(heap.environment_names(past_the_end), None);
        assert_eq!(heap.environment_size(past_the_end), None);
        let _ = stranger;
    }
}
