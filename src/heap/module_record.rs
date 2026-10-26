//! §16.2.1.4's Cyclic Module Record — what the engine remembers about one module, filed under the
//! environment the module runs in. DR-0033.
//!
//! # Why under the environment
//!
//! A module's environment is the one thing about it that everything still able to reach the module
//! already names: its functions close over it, its namespace object reads it, an importer's binding
//! reaches into it and a body parked at a top-level `await` resumes in it. Filed there, the record
//! is kept exactly as long as the environment is, and the objects it holds — the namespace, the
//! `import.meta` object, the value the body threw — are kept with it and freed with it.
//!
//! The key cannot name a second module while the record exists. An [`EnvironmentId`] carries
//! DR-0019's generation, so a slot the collector has freed is issued again under a handle that
//! compares unequal to every earlier one; and the collector drops the record in the same sweep that
//! frees the environment, so nothing is left filed under a handle that names nothing.
//!
//! A table beside the environments rather than a field on [`super::Environment`], for the reason
//! the import bindings and the namespace objects are tables: DR-0013 counts the size of every
//! environment any program makes, and a module is one environment among thousands.

use super::Heap;
use super::environment::{Binding, EnvironmentId};
use super::object::ObjectId;
use crate::value::Value;
use std::rc::Rc;

/// What the engine remembers about one module — as much of §16.2.1.4's Cyclic Module Record as it
/// has.
///
/// `[[Environment]]` is not a field: it is the key the record is filed under, which is what makes
/// the environment and the record live and die together.
#[derive(Debug, Default)]
pub(crate) struct ModuleRecord {
    /// §16.2.1.10's memo — the one namespace object this module will ever have.
    pub(crate) namespace: Option<ObjectId>,
    /// §13.3.12's `[[ImportMeta]]` — the one `import.meta` object this module will ever have.
    ///
    /// Made on first read rather than at link time, which is what step 4's "if importMeta is empty"
    /// says: a module that never writes `import.meta` never costs an object, and a host is never
    /// asked what to put on one nobody looked at.
    pub(crate) import_meta: Option<ObjectId>,
    /// Whether the body has run — §16.2.1.6's "each body once".
    ///
    /// Set **before** the body runs rather than after, so that a module reached again while it is
    /// still running is not started a second time. That is what §16.2.1.6.2's `[[Status]]` of
    /// `evaluating` is for, and one flag serves both because the only thing either answer decides
    /// here is whether to run the body.
    pub(crate) evaluated: bool,
    /// What the body threw, if it did — §16.2.1.6 step 9's `[[EvaluationError]]`.
    ///
    /// Kept because a module that threw must throw the **same** value at every later importer
    /// rather than being run again: `import("m")` twice over a module whose body failed rejects
    /// twice with one error object.
    pub(crate) failure: Option<Value>,
    /// The module's resolved identity, as the host named it — DR-0020's key.
    ///
    /// What a specifier the module wrote is resolved against, and what §16.2.1.9 hands the host to
    /// answer `import.meta` from. `None` for a module a host ran on its own with
    /// [`crate::vm::Vm::run_module`], which no host resolved — until a graph reaches the same code
    /// under a key, which is then its key.
    pub(crate) key: Option<Rc<str>>,
}

impl ModuleRecord {
    /// Every value the record holds, for the collector.
    pub(super) fn reaches(&self) -> impl Iterator<Item = Value> {
        self.namespace
            .map(Value::Object)
            .into_iter()
            .chain(self.import_meta.map(Value::Object))
            .chain(self.failure)
    }
}

impl Heap {
    /// §16.2.1.5.1 step 9's environment for a module, with an empty record filed under it.
    ///
    /// An environment with no parent, because a module's scope is the outermost one its code can
    /// see — which is what lets [`Heap::environment_root`] find the record from anywhere inside it.
    pub(crate) fn new_module_environment(
        &mut self,
        size: usize,
        names: Rc<[Binding]>,
        key: Option<Rc<str>>,
    ) -> EnvironmentId {
        let environment = self.new_named_environment(None, size, names);
        self.modules.insert(
            environment,
            ModuleRecord {
                key,
                ..ModuleRecord::default()
            },
        );
        environment
    }

    /// The record of the module whose environment this is, if it is one and it is still alive.
    pub(crate) fn module_record(&self, environment: EnvironmentId) -> Option<&ModuleRecord> {
        self.modules.get(&environment)
    }

    /// The same, for the writes evaluating and linking make.
    pub(crate) fn module_record_mut(
        &mut self,
        environment: EnvironmentId,
    ) -> Option<&mut ModuleRecord> {
        self.modules.get_mut(&environment)
    }
}
