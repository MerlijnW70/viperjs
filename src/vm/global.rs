//! §9.1.1.4's Global Environment Record — the scope a name falls into when it falls out of
//! every other one.
//!
//! # Why a global is a property and a local is a slot
//!
//! Every other scope is closed: the compiler saw every declaration in it and gave each a number,
//! so nothing at run time compares a string to find a variable. The global scope is not closed.
//! One script can declare what the next one reads, a property can be added while the program
//! runs, and the object is reachable as `globalThis` and can be written to like any other. So a
//! global reference carries its *name* into the bytecode and asks the object each time, and that
//! difference is the whole reason `missing` is a ReferenceError while `o.missing` is `undefined`
//! — one asks a scope whether a binding exists and the other asks an object for a property.
//!
//! # The part that is missing
//!
//! §9.1.1.4 is two records, not one: an object record whose binding object is the global object,
//! and a declarative record beside it holding `let`, `const` and `class`. Only the first is here,
//! because only `var` exists. When the second arrives it goes *in front* — a `let` at the top
//! level shadows a property of the same name rather than replacing it, which is why
//! `globalThis.x` and `let x` can disagree.

use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::{Heap, PropertyDescriptor, PropertyKey};
use crate::value::Value;

impl Vm {
    /// The property key an instruction names, out of the running chunk's constants.
    ///
    /// A constant that is not a String is a chunk that does not make sense, which is a
    /// [`Fault`] rather than a thrown error: no source produces one, and a hand-written chunk
    /// that does is a bug in whoever wrote it rather than in the program being run.
    pub(super) fn global_name(
        &self,
        running: &Chunk,
        index: u32,
        heap: &mut Heap,
    ) -> Result<PropertyKey, Fault> {
        let Some(Value::String(id)) = running.constant(index) else {
            return Err(Fault::MissingConstant);
        };
        Ok(PropertyKey::from_string(heap, id))
    }

    /// §9.1.1.4.1 `HasBinding` and §9.1.1.4.6 `GetBindingValue` together: the value, or nothing.
    ///
    /// Nothing means *no such binding*, which is the answer a ReferenceError is made of. It is
    /// deliberately not the same as `undefined`: `var x;` gives a binding whose value is
    /// `undefined`, and reading it is fine.
    ///
    /// The lookup walks the prototype chain, because §9.1.1.4.1 asks `HasProperty` rather than
    /// `HasOwnProperty`. That is not an oversight in the specification: the global object
    /// inherits from `Object.prototype`, so `toString` really does resolve as a bare name at the
    /// top level, and a program can rely on it.
    pub(super) fn global_binding(
        &mut self,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Option<crate::value::Completion<Value>> {
        let global = self.realm.global();
        heap.find_own(global, key)?;
        // Found, so this is a read of a property that is there — including an accessor, whose
        // getter is called by the same code that calls one for `o.x`.
        Some(self.get_property_key(Value::Object(global), key, heap))
    }

    /// §9.1.1.4.17 `CreateGlobalVarBinding`, for a Script.
    ///
    /// Writable and enumerable like an ordinary property, and **not configurable** — which is
    /// what makes `var x` at the top level undeletable where `globalThis.x = 1` is not. A name
    /// that is already there keeps its value: `var x` after `x = 1` does not put `undefined`
    /// back, and that is what hoisting means for a global.
    pub(super) fn declare_global(&self, key: PropertyKey, heap: &mut Heap) {
        let global = self.realm.global();
        if heap
            .object(global)
            .is_some_and(|object| object.get_own_property(key).is_some())
        {
            return;
        }
        let descriptor = PropertyDescriptor {
            value: Some(Value::Undefined),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        };
        // The global object is extensible and this property is not there, so nothing can refuse
        // it. Ignoring the answer rather than asserting keeps the instruction total.
        let _ = heap.define_own_property(global, key, &descriptor);
    }

    /// The sentence a ReferenceError for this name carries.
    ///
    /// The name is the whole diagnosis — there is no span at run time and no other context — so
    /// it is worth the allocation to say which one, in the words every other engine uses.
    pub(super) fn missing_global(&self, key: PropertyKey, heap: &Heap) -> String {
        let name = key
            .as_string()
            .and_then(|id| heap.string(id))
            .map(String::from_utf16_lossy)
            // A key with no string behind it cannot come from a compiled name; saying so beats
            // an empty message that reads like the name was blank.
            .unwrap_or_else(|| "a name".to_string());
        format!("{name} is not defined")
    }
}
