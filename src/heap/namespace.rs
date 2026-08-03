//! §10.4.6 — Module Namespace Exotic Objects.
//!
//! `import * as n from "m"` binds one of these, and so does the value a dynamic `import("m")`
//! resolves with. It looks like an object with one property per exported name, and it is not one:
//! every property is a **live read of the exporting module's slot**, so `export let n = 0; n++` is
//! seen through it, and every way of changing it is refused.
//!
//! # Where the data lives, and why it is not on the object
//!
//! Beside the objects rather than on them, in a table `Heap` owns and membership in which *is* the
//! marker. The alternative is a field on [`crate::heap::Object`], and an `Object` sits inline in the
//! arena — so a field there is eight bytes charged to every object any program ever makes, to serve
//! one that arrives once per imported module. DR-0013's budget counts exactly that, and the same
//! argument already moved §16.2.1.5.2's import bindings off `Environment` after a `Vec` there cost a
//! conformance file its heap.
//!
//! # What the specification asks for that an ordinary object cannot do
//!
//! - `[[Prototype]]` is **null** and `[[SetPrototypeOf]]` refuses anything else (§10.4.6.1).
//! - `[[IsExtensible]]` is false and `[[PreventExtensions]]` answers true (§10.4.6.2, §10.4.6.3).
//! - An export is a **data** property — `{ writable: true, enumerable: true, configurable: false }`
//!   — and not an accessor, which is what `getOwnPropertyDescriptor` reports and test262 checks.
//!   Writable and yet unwritable: §10.4.6.9's `[[Set]]` always answers false, so the attribute
//!   describes the binding rather than what may be done through the object.
//! - `[[OwnPropertyKeys]]` lists the exports **sorted by code unit** (§10.4.6.10), which is not the
//!   order they were written in and is the one ordering in the language that is not insertion order.
//! - `@@toStringTag` is `"Module"`, and is the only own property that is not an export.

use super::environment::EnvironmentId;
use super::object::ObjectId;
use super::property::{Property, PropertyKey, PropertyKind};
use super::{Heap, StringId};
use crate::value::Value;

/// What a namespace object is, beyond being an object — §10.4.6's `[[Module]]` and `[[Exports]]`.
#[derive(Debug)]
pub(super) struct Namespace {
    /// The module this is the namespace *of*, as its environment.
    ///
    /// `[[Module]]` is a Module Record in the specification and an environment here, because the
    /// only thing §10.4.6 ever asks a module for is `GetBindingValue` on its environment. Kept
    /// beside the per-name bindings rather than instead of them, because a re-exported name is a
    /// slot of a *different* module and this one still has to be traced.
    pub(super) environment: EnvironmentId,
    /// `[[Exports]]` — each exported name and where its value is, **sorted by code unit**.
    ///
    /// Sorted once here rather than at every `[[OwnPropertyKeys]]`, because the list cannot change:
    /// §16.2.1.10 settles a module's exports at link time and nothing afterwards adds one.
    ///
    /// The names are **interned**, so looking one up is comparing two handles rather than two
    /// strings: a `PropertyKey::String` is already interned, which is what makes that comparison
    /// the same question as comparing the text.
    pub(super) exports: Box<[(StringId, Binding)]>,
}

/// Where one exported name's value is — §16.2.1.6.3's two kinds of resolution.
///
/// A re-exported name lives in a module this one may never have heard of, so a namespace cannot be
/// one environment and a list of offsets into it: each name carries its own.
#[derive(Debug, Clone, Copy)]
pub enum Binding {
    /// A slot of some module's environment, read live — the ordinary kind.
    Slot(EnvironmentId, u32),
    /// A fixed value, which `export * as n from "m"` is: the name's value is `m`'s whole namespace
    /// object, and there is no binding anywhere that holds it.
    Value(Value),
}

/// Whether a binding could be read, and what it held — the shape §10.4.6.8 step 9 needs.
///
/// The dead zone is a third answer and not an absent value: a module in a cycle can be asked for an
/// export whose `let` has not run yet, and §10.4.6.8 makes that a **ReferenceError** rather than
/// `undefined`. Only the interpreter can throw, so this says which of the three it is and lets each
/// caller do what it is able to.
#[derive(Debug, Clone, Copy)]
pub enum Export {
    /// The module has this export and its binding holds a value.
    Value(Value),
    /// The module has this export and its binding is still in its dead zone — §10.4.6.8 step 9.
    Uninitialised,
}

impl Heap {
    /// Make §10.4.6.12's namespace object for a module — `[[Prototype]]` null, not extensible.
    ///
    /// `exports` is every name the module exports with the slot it lives in, in any order; it is
    /// sorted here because §10.4.6.10 lists them by code unit and the list never changes again.
    ///
    /// One per module, which is the caller's business rather than this function's: §16.2.1.10
    /// `GetModuleNamespace` memoises on the module record, and the linker is the only thing here
    /// that holds those.
    pub(crate) fn new_namespace(
        &mut self,
        environment: EnvironmentId,
        to_string_tag: Option<super::SymbolId>,
    ) -> ObjectId {
        let object = self.new_object(None);
        // §10.4.6.12 step 8 — the one own property that is not an export, and the only reason this
        // needs the realm's symbol table at all. Defined *before* the object is sealed, because
        // afterwards nothing may be added to it — including this.
        //
        // `configurable: false`, unlike every other `@@toStringTag` in the language: a namespace's
        // is permanent, so `delete ns[Symbol.toStringTag]` fails and `Object.prototype.toString`
        // answers `"[object Module]"` for ever.
        if let Some(symbol) = to_string_tag {
            let value = Value::String(self.intern(&"Module".encode_utf16().collect::<Vec<_>>()));
            let _ = self.define_own_property(
                object,
                PropertyKey::from_symbol(symbol),
                &super::property::PropertyDescriptor {
                    value: Some(value),
                    writable: Some(false),
                    enumerable: Some(false),
                    configurable: Some(false),
                    ..super::property::PropertyDescriptor::EMPTY
                },
            );
        }
        // §10.4.6.2 — never extensible from the moment it is complete. Nothing can add a property
        // to a namespace, and `Object.isExtensible` on one answers false.
        if let Some(found) = self.objects.get_mut(object.0).and_then(Option::as_mut) {
            found.prevent_extensions();
        }
        self.namespaces.insert(
            object,
            Namespace {
                environment,
                exports: Box::new([]),
            },
        );
        object
    }

    /// Give a namespace object its export list — the second half of `GetModuleNamespace`.
    ///
    /// Apart from making the object because §16.2.1.6.3's resolution can come back around to the
    /// module it started from: `export * as a from "b"` in one module and the same pointing back is
    /// a legal graph, and the walk terminates only because the object already exists to be found.
    /// So the object is made, memoised, and only then filled.
    ///
    /// Calling this twice replaces the list. Nothing does — the linker fills each namespace once —
    /// and the alternative is a second way to fail that says nothing a caller could act on.
    pub(crate) fn fill_namespace(&mut self, object: ObjectId, exports: Vec<(Box<str>, Binding)>) {
        let mut exports: Vec<(StringId, Binding)> = exports
            .into_iter()
            .map(|(name, binding)| {
                let units: Vec<u16> = name.encode_utf16().collect();
                (self.intern(&units), binding)
            })
            .collect();
        // §10.4.6.10 — by code unit, and read out of the heap rather than compared as `str`: the
        // two orders differ above the BMP, where UTF-8 sorts a code point after every surrogate
        // pair and UTF-16 sorts the pair by its lead unit. Exported names are identifiers or string
        // literals, so both really do occur.
        exports.sort_by(|left, right| {
            self.string(left.0)
                .unwrap_or(&[])
                .cmp(self.string(right.0).unwrap_or(&[]))
        });
        // §16.2.1.10 step 4 — a name two `export *`s both reach is listed once. Interned, so this
        // is comparing handles.
        exports.dedup_by(|left, right| left.0 == right.0);
        if let Some(found) = self.namespaces.get_mut(&object) {
            found.exports = exports.into_boxed_slice();
        }
    }

    /// The environment a namespace object reads, if this object is one.
    ///
    /// For the collector, which has to keep the exporting module's environment alive: a namespace
    /// reaches **sideways** into a chain nothing else here points at, exactly as an import binding
    /// does, and a walk that followed only parents would free the slots it reads.
    pub(super) fn namespace_roots(
        &self,
        object: ObjectId,
    ) -> Option<(EnvironmentId, Vec<Binding>)> {
        let found = self.namespaces.get(&object)?;
        Some((
            found.environment,
            found.exports.iter().map(|(_, binding)| *binding).collect(),
        ))
    }

    /// Whether this object is §10.4.6's exotic kind at all.
    ///
    /// Asked before anything else on the paths that have to refuse a write: a namespace's
    /// `[[Set]]`, `[[Delete]]` and `[[DefineOwnProperty]]` answer false whatever the key.
    pub(crate) fn is_namespace(&self, object: ObjectId) -> bool {
        self.namespaces.contains_key(&object)
    }

    /// §10.4.6.8 `[[Get]]`, as far as the heap can take it — the binding behind an exported name.
    ///
    /// `None` when this is not a namespace, or the key is not one of its exports, or the key is a
    /// Symbol — §10.4.6.8 step 2 sends a Symbol to the ordinary object, which is where
    /// `@@toStringTag` is found. The [`Export::Uninitialised`] answer is the caller's to turn into
    /// a ReferenceError; nothing here can throw.
    pub(crate) fn namespace_export(&self, object: ObjectId, key: PropertyKey) -> Option<Export> {
        let found = self.namespaces.get(&object)?;
        // §10.4.6.8 step 2 — a Symbol is not an export and goes to the ordinary object, which is
        // where `@@toStringTag` is.
        let name = key.as_string()?;
        let binding = found
            .exports
            .iter()
            .find(|(export, _)| *export == name)
            .map(|(_, binding)| *binding)?;
        Some(match binding {
            // `export * as n from "m"` — the value is another module's namespace object and there
            // is no binding anywhere that could be in a dead zone.
            Binding::Value(value) => Export::Value(value),
            Binding::Slot(environment, slot) => match self.variable(environment, slot) {
                Some(Some(value)) => Export::Value(value),
                // A slot the environment does not have cannot happen — the linker built the list
                // from the same chunk that made the slots — but "cannot happen" is not a thing to
                // encode as a panic, and a module in a cycle really can be asked before its `let`
                // has run. Both are the same answer to the program: it is not readable yet.
                Some(None) | None => Export::Uninitialised,
            },
        })
    }

    /// §10.4.6.5 `[[GetOwnProperty]]` for an export, when it can be answered without throwing.
    ///
    /// The attributes are §10.4.6.5 step 5's, and `writable: true` beside a `[[Set]]` that always
    /// refuses is not a contradiction — see this module's header.
    ///
    /// A binding in its dead zone is reported **present, holding `undefined`**, where §10.4.6.5
    /// step 4 would throw — because this is the descriptor path and a descriptor is not a
    /// completion. Present and not absent, so that the three questions about a name agree: it is
    /// listed by `[[OwnPropertyKeys]]`, `in` answers true for it, and a descriptor exists. What it
    /// costs is exactly one wrong answer, and a narrow one: inside a cycle, before the exporting
    /// module's `let` has run, `Object.getOwnPropertyDescriptor(ns, "x").value` is `undefined`
    /// instead of a ReferenceError. A *read* of the same name — `ns.x`, which is what a program
    /// writes and what the tests exercise — goes through [`Heap::namespace_export`] in the
    /// interpreter, which can throw and does.
    pub(super) fn namespace_property(
        &self,
        object: ObjectId,
        key: PropertyKey,
    ) -> Option<Property> {
        match self.namespace_export(object, key)? {
            Export::Value(value) => Some(Property {
                kind: PropertyKind::Data {
                    value,
                    writable: true,
                },
                enumerable: true,
                configurable: false,
            }),
            Export::Uninitialised => Some(Property {
                kind: PropertyKind::Data {
                    value: Value::Undefined,
                    writable: true,
                },
                enumerable: true,
                configurable: false,
            }),
        }
    }

    /// §10.4.6.10 `[[OwnPropertyKeys]]` — the sorted export names, then what the object itself has.
    ///
    /// The exports first and the Symbols after, which is the order §10.4.6.10 builds: the sorted
    /// names concatenated with `OrdinaryOwnPropertyKeys`, whose only entry here is `@@toStringTag`.
    pub(super) fn namespace_keys(&mut self, object: ObjectId) -> Option<Vec<PropertyKey>> {
        let mut keys: Vec<PropertyKey> = self
            .namespaces
            .get(&object)?
            .exports
            .iter()
            .map(|(name, _)| PropertyKey::String(*name))
            .collect();
        keys.extend(
            self.object(object)
                .map_or_else(Vec::new, |found| found.own_property_keys(self)),
        );
        Some(keys)
    }
}
