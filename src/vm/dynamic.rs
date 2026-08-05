//! §9.4.2 `ResolveBinding` done at **run time**, which is what `with` costs.
//!
//! # Why a name inside a `with` cannot be a slot
//!
//! praxis resolves a name to a depth and an index when it compiles, and everything about the
//! engine's speed and most of its simplicity comes from that (see [`crate::heap::environment`]).
//! §14.11 makes one construct where it is not possible: the scope a `with` opens holds whatever
//! its object holds *at the moment a name is looked up*, so `with (o) { a }` reads `o.a` before a
//! `delete o.a` and something outside afterwards. There is no index that means "whichever of those
//! is true now".
//!
//! So a name compiled inside a `with` body becomes one of the instructions here, carrying the name
//! itself, and the walk that would have happened at compile time happens on every read. That is
//! the whole cost, and it is confined: only names *inside* a `with` pay it, and the compiler emits
//! the ordinary slot instructions everywhere else.
//!
//! # What the walk asks at each level
//!
//! A declarative scope answers from its name list — DR-0018's, which exists for direct `eval` and
//! is exactly what this needs too. An object scope asks §9.1.1.2.1's `HasBinding`, which is
//! `HasProperty` and then §14.11.2's `@@unscopables`, and either can run a script's own code. The
//! chain ends at the global object, which is where a name that is nowhere becomes a ReferenceError
//! — the same answer, and the same code, an ordinary unresolvable name gets.

use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{EnvironmentId, Heap, Mutability, ObjectId, PropertyKey};
use crate::value::{Abrupt, Completion, Value};
use std::rc::Rc;

/// What a name turned out to mean — §9.4.2's Reference, as far as this engine needs one.
#[derive(Debug, Clone, Copy)]
pub(super) enum Resolved {
    /// A slot of a declarative scope, found by its name rather than by an index the compiler kept.
    Slot {
        /// Which scope.
        environment: EnvironmentId,
        /// Which slot of it.
        index: u32,
        /// What an assignment to it does — §9.1.1.1.5, carried on the binding since DR-0018.
        mutability: Mutability,
    },
    /// A property of a `with` object — §9.1.1.2, whose `WithBaseObject` is also the `this` a call
    /// through this name gets.
    Property(ObjectId),
    /// Nothing in the chain. The global object answers, and a ReferenceError is what it says when
    /// it has nothing either.
    Global,
}

impl Vm {
    /// Walk the running scopes outwards looking for `key` — §9.4.2 `ResolveBinding`.
    ///
    /// Fallible because an object scope's `HasBinding` runs a script's code: a proxy's `has` trap,
    /// a getter for `@@unscopables`. A declarative scope cannot throw here and does not.
    pub(super) fn resolve_dynamic(
        &mut self,
        name: &str,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Resolved> {
        let mut at = Some(self.environment);
        while let Some(environment) = at {
            // Read what this level is before asking anything, because asking may run code that
            // makes more environments — and a borrow of the heap cannot be held across it.
            let object = heap.environment_binding_object(environment);
            let slot = object
                .is_none()
                .then(|| slot_of(heap, environment, name))
                .flatten();
            if let Some((index, mutability)) = slot {
                return Ok(Resolved::Slot {
                    environment,
                    index,
                    mutability,
                });
            }
            if let Some(object) = object
                && self.has_with_binding(object, key, heap)?
            {
                return Ok(Resolved::Property(object));
            }
            at = heap.environment_at(environment, 1);
        }
        Ok(Resolved::Global)
    }

    /// §9.1.1.2.1 `HasBinding` on an object environment record.
    ///
    /// Two questions and the second is the one people forget. `HasProperty` walks the prototype
    /// chain, so `with ({}) { toString }` finds `Object.prototype`'s — and then §14.11.2's
    /// `@@unscopables` can take a name back out again, which is what lets `Array.prototype`
    /// grow methods like `includes` without `with (array) { includes }` in old code meaning
    /// something new. A name the list blocks is not bound here and the walk carries on outwards.
    fn has_with_binding(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<bool> {
        if !self.has_property_key(Value::Object(object), key, heap)? {
            return Ok(false);
        }
        // Step 5 — `Get(bindingObject, @@unscopables)`. A realm always has the well-known symbols,
        // so the `None` is not a case a program can produce; it is folded into "nothing blocks
        // anything" rather than given a `return` of its own, because a branch no input can take is
        // a branch no test can hold.
        let unscopables = match self
            .realm
            .well_known(crate::builtins::well_known_at("unscopables"))
        {
            Some(symbol) => self.get_property_key(
                Value::Object(object),
                PropertyKey::from_symbol(symbol),
                heap,
            )?,
            None => Value::Undefined,
        };
        // Step 6 — only an *object* is consulted. A `@@unscopables` of `true` blocks nothing,
        // which is the difference between "there is a list" and "the list says yes".
        let Value::Object(list) = unscopables else {
            return Ok(true);
        };
        let blocked = self.get_property_key(Value::Object(list), key, heap)?;
        Ok(!blocked.to_boolean(heap))
    }

    /// Read what `name` resolves to — §6.2.5.5 `GetValue`.
    ///
    /// `strict` is §9.1.1.2.6's `S`, and only an object scope reads it: step 3 turns a binding that
    /// has gone since it was resolved into a ReferenceError for strict code and `undefined` for
    /// sloppy. A declarative scope has its own dead-zone rule and does not ask.
    pub(super) fn load_dynamic(
        &mut self,
        found: Resolved,
        key: PropertyKey,
        strict: bool,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        match found {
            Resolved::Slot {
                environment, index, ..
            } => match heap.variable(environment, index) {
                // §9.1.1.1.6 — the binding is there and holds nothing, which is the dead zone and
                // is a ReferenceError rather than `undefined`.
                Some(None) => Err(Abrupt::Raised(
                    crate::value::ErrorKind::Reference,
                    "a binding read before it was initialised",
                )),
                Some(Some(value)) => Ok(Some(value)),
                // The name list said this slot is here, so a heap that disagrees is a fault rather
                // than a program's problem. Answered as unresolvable, which is the safe direction.
                None => Ok(None),
            },
            // §9.1.1.2.6 `GetBindingValue` — and like its `SetMutableBinding` twin below it is four
            // steps rather than one.
            Resolved::Property(object) => {
                let base = Value::Object(object);
                // Step 2 — the binding is looked for **again**. Resolving it ran `HasBinding`,
                // which reads `@@unscopables`, and that is a getter a script may write: one that
                // deletes the property leaves a reference to a binding that is no longer there.
                //
                // `HasProperty` and not `HasBinding`, so the list is *not* consulted a second time
                // — which is what makes the getter run exactly once for one read, and is asserted
                // as such by the tests either side of this clause.
                if !self.has_property_key(base, key, heap)? {
                    // Step 3. Not a fall-through to the global: the name was bound *here* when it
                    // was resolved, so the walk is over. Answering `None` would send
                    // `read_resolved` on to the global object and read something else entirely.
                    if strict {
                        return Err(Abrupt::Raised(
                            crate::value::ErrorKind::Reference,
                            "a binding that is no longer there",
                        ));
                    }
                    return Ok(Some(Value::Undefined));
                }
                self.get_property_key(base, key, heap).map(Some)
            }
            Resolved::Global => Ok(None),
        }
    }

    /// The value a resolved name has, asking the global object last — §6.2.5.5 `GetValue`.
    ///
    /// `None` is *unresolvable*: nothing in the chain and nothing on the global object either, which
    /// is the one state a ReferenceError is made of and the one `typeof` answers `"undefined"` for.
    pub(super) fn read_resolved(
        &mut self,
        found: Resolved,
        key: PropertyKey,
        strict: bool,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        match self.load_dynamic(found, key, strict, heap)? {
            Some(value) => Ok(Some(value)),
            None => match self.global_binding(key, heap) {
                Some(read) => read.map(Some),
                None => Ok(None),
            },
        }
    }

    /// Write it — §6.2.5.6 `PutValue`, answering whether the write happened.
    ///
    /// `false` means the name is nowhere in the chain and the global object is the one to ask,
    /// which is the caller's to do because a sloppy assignment *creates* the property there and a
    /// strict one throws.
    pub(super) fn store_dynamic(
        &mut self,
        found: Resolved,
        key: PropertyKey,
        value: Value,
        strict: bool,
        heap: &mut Heap,
    ) -> Completion<bool> {
        match found {
            Resolved::Slot {
                environment,
                index,
                mutability,
            } => {
                // §9.1.1.1.5, exactly as a compiled store decides it — the difference is only that
                // the mutability was read from the binding now rather than when it was resolved.
                if !mutability.writes() {
                    if mutability.refusal_throws(strict) {
                        return Err(Abrupt::type_error(
                            "an assignment to a binding that may not be changed",
                        ));
                    }
                    return Ok(true);
                }
                if matches!(heap.variable(environment, index), Some(None)) {
                    return Err(Abrupt::Raised(
                        crate::value::ErrorKind::Reference,
                        "a binding written before it was initialised",
                    ));
                }
                heap.set_variable(environment, index, value);
                Ok(true)
            }
            // §9.1.1.2.5 `SetMutableBinding` — and it is four steps, not one.
            Resolved::Property(object) => {
                let base = Value::Object(object);
                // Step 2 — the binding is looked for **again**, because everything between
                // resolving the reference and writing through it is a program.
                // `with (o) { x += 1 }` where `o`'s `x` getter deletes `x` is the whole of the
                // case: the read succeeds, the property goes, and step 3 tells strict code so
                // rather than quietly making it again.
                //
                // `HasProperty` and not §9.1.1.2.1's `HasBinding`: `@@unscopables` decides what a
                // `with` can *see*, and by here the reference has already been resolved.
                //
                // **Asked whatever the strictness, and only the throw is conditioned on it.** Step
                // 3 reads "if stillExists is false and S is true", which puts the `S` on the
                // refusal and not on the question — and the question is observable, being a
                // `HasProperty` that runs a proxy's `has` trap. Written as `strict && …` it is
                // skipped entirely in sloppy code, so `with (proxy) { p = 1 }` called one trap
                // fewer than every other engine.
                let still_exists = self.has_property_key(base, key, heap)?;
                if strict && !still_exists {
                    return Err(Abrupt::Raised(
                        crate::value::ErrorKind::Reference,
                        "an assignment to a binding that is no longer there",
                    ));
                }
                // Step 4's `S` — a write the object refuses is a TypeError in strict code, which
                // is the rule §6.2.5.6 applies to every other reference. This doc used to say
                // praxis "does not yet carry a store's strictness as far as `[[Set]]`"; the
                // strictness is the argument three lines up.
                let accepted = self.set_property_key(base, key, value, heap)?;
                if strict && matches!(accepted, Value::Boolean(false)) {
                    return Err(Abrupt::type_error(
                        "an assignment to a property that would not take it",
                    ));
                }
                Ok(true)
            }
            Resolved::Global => Ok(false),
        }
    }

    /// The name a name-carrying instruction names, as text.
    ///
    /// The chain is walked by comparing strings, which is the whole of what these instructions cost
    /// over the indexed ones. A constant that is not a String is a chunk that does not make sense —
    /// the same [`Fault`] `Vm::global_name` answers with, and for the same reason.
    pub(super) fn name_text(
        &self,
        running: &Chunk,
        index: u32,
        heap: &Heap,
    ) -> Result<String, Fault> {
        let Some(Value::String(id)) = running.constant(index) else {
            return Err(Fault::MissingConstant);
        };
        Ok(String::from_utf16_lossy(heap.string(id).unwrap_or(&[])))
    }

    /// Resolve, handing a throw from an object scope's `HasBinding` to the innermost handler.
    ///
    /// `None` means a handler took it and the loop should go round again — the same protocol
    /// `Vm::settle` uses, written separately because that one answers with a [`Value`] and this
    /// answers with a [`Resolved`].
    pub(super) fn settle_resolution(
        &mut self,
        name: &str,
        key: PropertyKey,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Option<Resolved>, Fault> {
        match self.resolve_dynamic(name, key, heap) {
            Ok(found) => Ok(Some(found)),
            Err(error) => {
                let thrown = self.thrown_value(error, heap);
                self.unwind(thrown, root, current, at)?;
                Ok(None)
            }
        }
    }

    /// The `this` a call through this name gets — §9.1.1.2.10 `WithBaseObject`.
    ///
    /// Only an object scope has one. Every other resolution calls with `undefined`, which §10.2.1.2
    /// then turns into the global object for a sloppy callee — so `with (o) { m() }` is the one
    /// place a bare call has a receiver, and that is the whole of why `with` is not sugar for a
    /// block of property reads.
    pub(super) fn with_base(found: Resolved) -> Value {
        match found {
            Resolved::Property(object) => Value::Object(object),
            _ => Value::Undefined,
        }
    }
}

/// Which slot of a declarative scope `name` is, if that scope names it — DR-0018's list.
///
/// A linear search over the names of one level. That is what a compiler does too; the difference
/// is only that it did it once, when it had the source in front of it.
fn slot_of(heap: &Heap, environment: EnvironmentId, name: &str) -> Option<(u32, Mutability)> {
    let names = heap.environment_names(environment)?;
    let at = names.iter().position(|binding| &*binding.name == name)?;
    // A scope with more than four billion names is not a program, and `Chunk::locals` is a `u32`
    // besides — so the position always fits. `ok()?` rather than a cast: a wrong slot index is a
    // wrong *variable*, and answering "not here" sends the walk outwards, which is the safe way to
    // be wrong about a name.
    Some((u32::try_from(at).ok()?, names[at].mutability)) // a narrowing that cannot lose, and answers None if it ever did
}
