//! Mark and sweep — the collector DR-0010 was shaped around.
//!
//! # Why not reference counting
//!
//! DR-0010's argument, and it is worth repeating where the alternative would go: `Rc` frees at
//! zero and never frees a cycle, and JavaScript makes cycles before any user code runs.
//! `f.prototype.constructor === f` is one, and every function has it — so a counting collector
//! would leak every function ever made. Marking does not care: a cycle nothing reaches is
//! unreachable, and unreachable is exactly what it looks for.
//!
//! # What a root is
//!
//! Everything a running program can still name. That is not something the heap can work out for
//! itself — the interpreter has the stack, the frames and the environment it is running in — so
//! the roots are handed in. A collector that guessed would be a collector that freed something
//! still in use, and no amount of testing finds that reliably.
//!
//! # The generation counter DR-0010 deferred, and why there is still none
//!
//! DR-0010 left it out and said the sweep would decide with evidence. This is that decision, and
//! the answer is that it is still not needed — because a freed slot is never *reused*. Sweeping
//! empties a slot and leaves the hole; the arena only grows. A stale handle therefore addresses
//! an empty slot and answers `None`, which is the same narrow promise every handle already makes.
//!
//! A free list would change that, and would need a generation the same day: without one, a reused
//! slot turns a stale handle into a use-after-free with the types intact — a wrong answer rather
//! than a crash, which is the worse of the two. Reusing slots is an M8 experiment, and this is the
//! note that says what it costs.
//!
//! # What is not here
//!
//! Any decision about *when* to collect. §9.10's note leaves that to the implementation entirely,
//! and picking a moment needs a measurement of what allocation costs — an M8 experiment. What is
//! here is the operation, and an embedder that calls it.

use crate::heap::{EnvironmentId, Heap, ObjectId, PropertyKind, StringId};
use crate::value::Value;

/// Everything a running program can still reach, handed to the collector by its owner.
///
/// Deliberately explicit. The heap cannot see the interpreter's stack, and an interpreter that
/// forgot to mention it would have its values freed underneath it — so this is one struct with one
/// field per place a value can be, and adding a place is a change the compiler asks about.
#[derive(Debug, Default)]
pub struct Roots {
    /// Values on an operand stack, in a constant table, or held by an embedder.
    pub values: Vec<Value>,
    /// Environments a frame or a closure can still reach.
    pub environments: Vec<EnvironmentId>,
}

/// What a collection freed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collected {
    /// How many objects were unreachable.
    pub objects: usize,
    /// How many environments were unreachable.
    pub environments: usize,
    /// How many Strings were unreachable.
    pub strings: usize,
}

impl Heap {
    /// Free everything `roots` cannot reach, and answer how much that was.
    ///
    /// # What survives
    ///
    /// Whatever is reachable, by any path. From a root value to an object, from an object to its
    /// prototype and to every value in every property, from a function to the environment it was
    /// written in, from an environment to its parent and to every variable in it. A cycle among
    /// them survives if anything outside reaches it and is freed if nothing does, which is the
    /// whole point.
    ///
    /// The intern table is *not* a root. A property name nothing uses any more should go, and
    /// keeping the table strong would pin every name a program ever computed — which is the leak
    /// [`Heap::intern`] warned about.
    pub fn collect(&mut self, roots: &Roots) -> Collected {
        let mut marked = Marked {
            objects: vec![false; self.objects.len()],
            environments: vec![false; self.environments.len()],
            strings: vec![false; self.strings.len()],
        };
        for value in &roots.values {
            self.mark_value(*value, &mut marked);
        }
        for environment in &roots.environments {
            self.mark_environment(*environment, &mut marked);
        }
        self.sweep(&marked)
    }

    /// Mark a value and everything it leads to.
    fn mark_value(&self, value: Value, marked: &mut Marked) {
        match value {
            Value::String(id) => {
                if let Some(seen) = marked.strings.get_mut(id.index()) {
                    *seen = true;
                }
            }
            Value::Object(id) => self.mark_object(id, marked),
            // A primitive that is not a String leads nowhere: it *is* its value.
            Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => {}
        }
    }

    /// Mark an object, its prototype, its properties and the environment it closed over.
    ///
    /// Iterative rather than recursive. An object graph is as deep as a program makes it — a list
    /// of a million links is a chain of a million objects — and recursing would run out of Rust
    /// stack on data rather than on nesting. DR-0002 again: a collector that crashes on a long
    /// list is not a collector.
    fn mark_object(&self, from: ObjectId, marked: &mut Marked) {
        let mut pending = vec![from];
        while let Some(id) = pending.pop() {
            match marked.objects.get_mut(id.index()) {
                // Already marked, so its edges have been walked. This is also what makes a cycle
                // terminate rather than spin.
                Some(true) | None => continue,
                Some(seen) => *seen = true,
            }
            let Some(object) = self.object(id) else {
                continue;
            };
            if let Some(prototype) = object.prototype() {
                pending.push(prototype);
            }
            if let Some(environment) = object.environment() {
                self.mark_environment(environment, marked);
            }
            // An arrow's captured `this` is reachable *through the arrow*, and nothing else may
            // be holding it: `function F() { return () => this; }` leaves the constructed object
            // alive only because the arrow it returned points at it.
            match object.lexical_this() {
                Some(Value::Object(reached)) => pending.push(reached),
                Some(other) => self.mark_value(other, marked),
                None => {}
            }
            for key in object.own_property_keys(self) {
                // A key is a String and is reachable *because* it is a key: a property nothing
                // else names still has its name.
                if let Some(seen) = marked.strings.get_mut(key.as_string().index()) {
                    *seen = true;
                }
                let Some(property) = object.get_own_property(key) else {
                    continue;
                };
                let values = match property.kind {
                    PropertyKind::Data { value, .. } => [value, Value::Undefined],
                    PropertyKind::Accessor { getter, setter } => [getter, setter],
                };
                for value in values {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
        }
    }

    /// Mark an environment, its parent chain, and every variable along it.
    fn mark_environment(&self, from: EnvironmentId, marked: &mut Marked) {
        let mut next = Some(from);
        while let Some(id) = next {
            match marked.environments.get_mut(id.index()) {
                Some(true) | None => return,
                Some(seen) => *seen = true,
            }
            for value in self.environment_slots(id) {
                self.mark_value(value, marked);
            }
            next = self.environment_parent(id);
        }
    }

    /// Free everything unmarked, leaving a hole where it was.
    ///
    /// A hole rather than a compaction: moving an object would mean finding every handle to it,
    /// and a handle is a plain index that anything may hold — including an embedder. So a slot is
    /// emptied and its generation moves on, which is what makes a stale handle answer `None`
    /// instead of addressing whatever is put there next.
    fn sweep(&mut self, marked: &Marked) -> Collected {
        let mut freed = Collected {
            objects: 0,
            environments: 0,
            strings: 0,
        };
        // Zipped rather than indexed. The marks were sized from the arenas and nothing allocates
        // between, so the two are the same length — and `zip` says that rather than an index with
        // a default for a case that cannot happen.
        for (object, marked) in self.objects.iter_mut().zip(&marked.objects) {
            if *marked || object.is_none() {
                continue;
            }
            *object = None;
            freed.objects += 1;
        }
        for (environment, marked) in self.environments.iter_mut().zip(&marked.environments) {
            if *marked || environment.is_none() {
                continue;
            }
            *environment = None;
            freed.environments += 1;
        }
        for (string, marked) in self.strings.iter_mut().zip(&marked.strings) {
            if *marked || string.is_none() {
                continue;
            }
            *string = None;
            freed.strings += 1;
        }
        // The intern table would otherwise keep pointing at freed Strings, and a later `intern`
        // of the same text would hand back a handle to nothing.
        let strings = &self.strings;
        self.interned
            .retain(|_, id| strings.get(id.index()).is_some_and(Option::is_some));
        freed
    }

    /// The values in an environment's slots, copied out so the walk may borrow the heap.
    fn environment_slots(&self, id: EnvironmentId) -> Vec<Value> {
        self.environments
            .get(id.index())
            .and_then(Option::as_ref)
            .map(|found| found.slots().to_vec())
            .unwrap_or_default()
    }

    /// An environment's parent, if it has one and exists.
    fn environment_parent(&self, id: EnvironmentId) -> Option<EnvironmentId> {
        self.environments
            .get(id.index())
            .and_then(Option::as_ref)
            .and_then(|found| found.parent())
    }
}

/// Which slots the mark phase reached.
///
/// A bit per slot rather than a flag on each object, so that a collection leaves no trace behind
/// it: the marks are gone the moment it returns, and nothing has to be cleared for the next one.
struct Marked {
    objects: Vec<bool>,
    environments: Vec<bool>,
    strings: Vec<bool>,
}

/// The index inside a handle, for the collector's own use.
pub(super) trait Slot {
    /// Which slot of its arena this handle names.
    fn index(&self) -> usize;
}

impl Slot for StringId {
    fn index(&self) -> usize {
        self.0
    }
}

impl Slot for ObjectId {
    fn index(&self) -> usize {
        self.0
    }
}

impl Slot for EnvironmentId {
    fn index(&self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{PropertyDescriptor, PropertyKey};

    fn define(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
        let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, key, &descriptor));
    }

    #[test]
    fn what_nothing_reaches_is_freed_and_what_something_reaches_is_not() {
        let mut heap = Heap::new();
        let kept = heap.new_object(None);
        let dropped = heap.new_object(None);
        let roots = Roots {
            values: vec![Value::Object(kept)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.objects, 1);
        assert!(heap.object(kept).is_some());
        // The handle survives the object; it addresses an empty slot and says so, which is the
        // same narrow promise a handle from another heap already makes.
        assert!(heap.object(dropped).is_none());
        assert_eq!(heap.object_count(), 1);
    }

    #[test]
    fn a_cycle_nothing_reaches_is_freed_which_is_the_whole_reason_for_marking() {
        // The case reference counting cannot do, and it is not a corner: every function in a
        // program is in one, because §10.2.5 gives it a `prototype` whose `constructor` points
        // back. A counting collector would leak all of them.
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(None);
        define(&mut heap, first, "other", Value::Object(second));
        define(&mut heap, second, "other", Value::Object(first));
        let before = heap.object_count();

        let freed = heap.collect(&Roots::default());
        assert_eq!(freed.objects, before);
        assert!(heap.object(first).is_none());
        assert!(heap.object(second).is_none());

        // …and the same cycle survives whole when anything at all reaches into it.
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(None);
        define(&mut heap, first, "other", Value::Object(second));
        define(&mut heap, second, "other", Value::Object(first));
        let roots = Roots {
            values: vec![Value::Object(first)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).objects, 0);
        assert!(heap.object(second).is_some());
    }

    #[test]
    fn everything_an_object_leads_to_survives_with_it() {
        let mut heap = Heap::new();
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        let held = heap.new_object(None);
        let text = heap.new_string("kept".encode_utf16().collect());
        define(&mut heap, object, "child", Value::Object(held));
        define(&mut heap, object, "text", Value::String(text));
        let orphan = heap.new_object(None);
        let forgotten = heap.new_string("gone".encode_utf16().collect());

        let roots = Roots {
            values: vec![Value::Object(object)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.objects, 1);
        assert!(heap.object(prototype).is_some());
        assert!(heap.object(held).is_some());
        assert!(heap.string(text).is_some());
        assert!(heap.object(orphan).is_none());
        assert!(heap.string(forgotten).is_none());
        // A property's *name* is reachable because it is a name, so the keys survive too — a
        // property nobody else mentions still has one.
        assert!(
            heap.object(object)
                .is_some_and(|found| found.property_count() == 2)
        );

        // …and the names themselves, which are Strings like any other and are reachable *because*
        // they are names. Without that, a surviving object would have properties whose keys had
        // been freed underneath it.
        let names: Vec<String> = heap
            .object(object)
            .map_or_else(Vec::new, |found| found.own_property_keys(&heap))
            .into_iter()
            .map(|key| String::from_utf16_lossy(heap.string(key.as_string()).unwrap_or(&[])))
            .collect();
        assert_eq!(names, ["child", "text"]);
    }

    #[test]
    fn an_environment_keeps_its_parents_and_its_variables() {
        // What a closure is, from the collector's side: a function reaches the environment it was
        // written in, that environment reaches its parent, and every variable along the way is
        // kept because something can still name it.
        let mut heap = Heap::new();
        let outer = heap.new_environment(None, 1);
        let inner = heap.new_environment(Some(outer), 1);
        let held = heap.new_object(None);
        assert!(heap.set_variable(outer, 0, Value::Object(held)));
        let unreachable = heap.new_environment(None, 1);

        let roots = Roots {
            environments: vec![inner],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.environments, 1);
        assert!(heap.environment_at(inner, 1).is_some());
        assert!(heap.object(held).is_some());
        assert!(heap.variable(unreachable, 0).is_none());
    }

    #[test]
    fn a_function_keeps_the_environment_it_closed_over() {
        let mut heap = Heap::new();
        let captured = heap.new_environment(None, 1);
        let held = heap.new_object(None);
        assert!(heap.set_variable(captured, 0, Value::Object(held)));
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let function = heap.new_function(prototype, body, captured, None);

        let roots = Roots {
            values: vec![Value::Object(function)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).environments, 0);
        // The variable the closure can still read is still there, which is the property that
        // makes a closure work at all.
        assert!(heap.object(held).is_some());
    }

    #[test]
    fn an_arrow_keeps_the_this_it_closed_over() {
        // §15.3's captured `this` is an edge in the object graph like any other, and it is the
        // *only* one holding the receiver: `function F() { return () => this; }` leaves nothing
        // else pointing at the constructed object. A collector that walked the environment but
        // not this field would free the object the arrow is about to answer with — a
        // use-after-free with the types intact, which is the wrong kind of failure.
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 0);
        let receiver = heap.new_object(None);
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let arrow = heap.new_function(prototype, body, environment, Some(Value::Object(receiver)));

        let roots = Roots {
            values: vec![Value::Object(arrow)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(receiver).is_some());
        // …and a captured String is kept for the same reason, a primitive `this` being reachable
        // exactly as far as the arrow is.
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 0);
        let text = heap.intern(&"held".encode_utf16().collect::<Vec<_>>());
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let arrow = heap.new_function(prototype, body, environment, Some(Value::String(text)));
        let roots = Roots {
            values: vec![Value::Object(arrow)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).strings, 0);
        assert!(heap.string(text).is_some());
    }

    #[test]
    fn the_intern_table_is_not_a_root_and_forgets_a_freed_name() {
        // `Heap::intern` warned that an interned key lives as long as the heap. It does not any
        // more: a name nothing uses is freed, and the table forgets it — so a later `intern` of
        // the same text makes a *new* String rather than handing back a handle to nothing.
        let mut heap = Heap::new();
        let name = heap.intern(&"gone".encode_utf16().collect::<Vec<_>>());
        assert_eq!(heap.collect(&Roots::default()).strings, 1);
        assert!(heap.string(name).is_none());
        let again = heap.intern(&"gone".encode_utf16().collect::<Vec<_>>());
        assert_ne!(again, name);
        assert!(heap.string(again).is_some());
    }

    #[test]
    fn a_long_chain_of_objects_does_not_run_out_of_stack() {
        // The mark phase walks the graph, and a graph is as deep as a program makes it: a list of
        // a hundred thousand links is a chain of a hundred thousand objects. Recursing would run
        // out of Rust stack on *data* rather than on nesting, which DR-0002 does not allow.
        let mut heap = Heap::new();
        let mut previous = heap.new_object(None);
        let head = previous;
        for _ in 0..100_000 {
            let next = heap.new_object(None);
            define(&mut heap, previous, "next", Value::Object(next));
            previous = next;
        }
        let roots = Roots {
            values: vec![Value::Object(head)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).objects, 0);
        assert!(heap.object(previous).is_some());
    }

    #[test]
    fn collecting_twice_frees_nothing_the_second_time() {
        // The marks live for one collection and are gone when it returns, so nothing has to be
        // cleared and a second pass over the same heap is a no-op rather than a double free.
        let mut heap = Heap::new();
        heap.new_object(None);
        heap.new_string("gone".encode_utf16().collect());
        let first = heap.collect(&Roots::default());
        assert_eq!((first.objects, first.strings), (1, 1));
        let second = heap.collect(&Roots::default());
        assert_eq!((second.objects, second.strings), (0, 0));
    }
}
