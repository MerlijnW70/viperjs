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

    /// §9.1.1.4.15 `CanDeclareGlobalVar` — whether `var` may name this.
    ///
    /// Almost always yes. A `var` that names a property the global object already has is not a
    /// redeclaration at all — it leaves the property exactly as it is — so the only way to refuse
    /// one is a name that is *not* there on an object that will take no more properties, which is
    /// `Object.preventExtensions(globalThis)`.
    /// Written as one chain because "the realm has no global object" is not a state that exists —
    /// a branch answering for it is a `false` no input can reach and no test can pin.
    pub(super) fn can_declare_global_var(&self, key: PropertyKey, heap: &Heap) -> bool {
        heap.object(self.realm.global())
            .is_some_and(|object| object.get_own_property(key).is_some() || object.is_extensible())
    }

    /// §9.1.1.4.16 `CanDeclareGlobalFunction` — whether a function declaration may name this.
    ///
    /// The strict one, and the difference from the `var` question is the point: a function
    /// declaration must *put its function in* the property, so a property it could not write is a
    /// property it cannot declare over. §19.1's three — `undefined`, `NaN`, `Infinity` — are
    /// exactly that shape, which is why `function NaN() {}` at the top level of a script is a
    /// TypeError while `var NaN;` beside it is allowed.
    ///
    /// An **accessor** is refused for the same reason by a different route: it is neither
    /// configurable nor a writable data property, so a declaration cannot replace it.
    pub(super) fn can_declare_global_function(&self, key: PropertyKey, heap: &Heap) -> bool {
        heap.object(self.realm.global()).is_some_and(|object| {
            let Some(existing) = object.get_own_property(key) else {
                // Not there yet, so the only question left is whether the object takes new ones.
                return object.is_extensible();
            };
            // Step 5, then step 6: configurable is enough on its own, and a non-configurable
            // property is still enough when it is an ordinary *visible* data property — because
            // §9.1.1.4.16 then redefines it in place rather than replacing it. Both halves of that
            // second test are load-bearing: a writable one that is hidden from enumeration is
            // refused, which is the shape `Object.defineProperty` makes and a `var` never does.
            existing.configurable
                || (matches!(
                    existing.kind,
                    crate::heap::PropertyKind::Data { writable: true, .. }
                ) && existing.enumerable)
        })
    }

    /// §9.1.1.4.17 `CreateGlobalVarBinding`, for a Script.
    ///
    /// Writable and enumerable like an ordinary property, and configurable exactly when the
    /// clause's `D` says. §16.1.7 passes `false` for a Script — which is what makes `var x` at the
    /// top level undeletable where `globalThis.x = 1` is not — and §19.2.1.1 passes `true` for an
    /// `eval`, so that a string evaluated once cannot fix a name on the global object for good. A
    /// name that is already there keeps its value *and its attributes*: `var x` after `x = 1` does
    /// not put `undefined` back, and that is what hoisting means for a global.
    ///
    /// Asks nothing about whether it *may*: [`Vm::can_declare_global_var`] has already been asked,
    /// for every name in the script, before this ran for any of them.
    pub(super) fn declare_global(
        &self,
        key: PropertyKey,
        deletable: crate::compile::Deletable,
        heap: &mut Heap,
    ) {
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
            configurable: Some(deletable == crate::compile::Deletable::Yes),
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
            .describe(heap)
            // A key with no string behind it cannot come from a compiled name; saying so beats
            // an empty message that reads like the name was blank.
            .unwrap_or_else(|| "a name".to_string());
        format!("{name} is not defined")
    }
}
