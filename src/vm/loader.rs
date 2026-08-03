//! §16.2.1.7 `HostLoadImportedModule` — the one fact about a module the language cannot know.
//!
//! A `ModuleSpecifier` is text. Whether it names a file, a URL, a database row or an entry in a map
//! the embedder built is the host's business and nothing the specification will ever settle — the
//! same division DR-0014 makes for the local time zone: praxis implements the clause and the host
//! supplies the fact.
//!
//! # Why a trait, when a graph was enough before
//!
//! A *static* `import` names its modules in the source, so a host can read the whole graph before
//! anything runs and hand it over — which is what [`Graph`](super::Graph) is for, and it remains the
//! simplest way to run a module. A **dynamic** `import()` cannot be answered that way: the specifier
//! is whatever an expression evaluated to, so the host has to be reachable *while the program runs*.
//!
//! A machine with no loader is not an error until a program asks for one. `import("x")` then rejects
//! its promise, which is what §16.2.1.7 says a host that cannot load a module does — so an embedder
//! that wants no modules at all gets that behaviour by doing nothing.

use crate::compile::Chunk;
use crate::heap::Heap;
use std::rc::Rc;

/// How the host answers for a `ModuleSpecifier` — §16.2.1.7.
///
/// Called with the specifier as the source wrote it, after `ToString`. The implementation resolves
/// it however it likes, parses it as a **Module** (§16.2), compiles it, and answers with the chunk.
///
/// The heap is lent because compiling needs one: a chunk's constants — its strings, its regular
/// expressions — live on it, so a module compiled against a different heap would hold handles this
/// one cannot read. See [`crate::compile::compile_module`].
///
/// # Answering twice for one specifier
///
/// The engine memoises: a specifier it has already loaded is never asked for again, so a host need
/// not keep a cache of its own to make a module evaluate once. What a host **must** do is answer
/// consistently, since a specifier that answered differently later would put two records where the
/// specification has one.
pub trait ModuleLoader {
    /// Resolve, read and compile the module `specifier` names, **as written in `referrer`**.
    ///
    /// `referrer` is the key of the module doing the importing, or `None` for an entry point and
    /// for an `import()` written at the top level of a Script. It is the parameter §16.2.1.7's
    /// `HostLoadImportedModule(referrer, specifier, …)` puts first, and without it a relative
    /// specifier cannot be resolved at all: `./index.js` means a different file in every
    /// directory that writes it.
    ///
    /// The `String` answered with is the module's **resolved identity** — an absolute path, a URL,
    /// a package name, whatever this host uses to tell two modules apart. The engine never parses
    /// it and only ever compares them, so a host wanting the old behaviour answers with the
    /// specifier unchanged. DR-0020 has the argument.
    ///
    /// # Errors
    ///
    /// Whatever the host could not do — a specifier that resolves nowhere, a file it cannot read, a
    /// source that is not a Module. The error is a sentence for a person: it reaches a script only
    /// as the message of the error a rejected `import()` carries, so "no such file" is worth more
    /// than a code.
    fn load(
        &mut self,
        referrer: Option<&str>,
        specifier: &str,
        heap: &mut Heap,
    ) -> Result<(String, Rc<Chunk>), String>;
}

/// What the engine remembers about one module — as much of §16.2.1's Cyclic Module Record as it has.
///
/// Kept on the [`Vm`](super::Vm) rather than in a local of the link, because §16.2.1.6's "each body
/// once" is a fact about the whole execution: a dynamic `import()` arriving later has to find what
/// an earlier one evaluated, and find the *same* namespace object for it.
#[derive(Debug)]
pub(super) struct ModuleRecord {
    /// `[[Environment]]` — made at link time, before any body runs, because a cycle means a
    /// module's imports may name one the walk has not reached yet.
    pub(super) environment: crate::heap::EnvironmentId,
    /// §16.2.1.10's memo — the one namespace object this module will ever have.
    pub(super) namespace: Option<crate::heap::ObjectId>,
    /// Whether the body has run — §16.2.1.6's "each body once".
    ///
    /// Set **before** the body runs rather than after, so that a module reached again while it is
    /// still running is not started a second time. That is what §16.2.1.6.2's `[[Status]]` of
    /// `evaluating` is for, and one flag serves both because the only thing either answer decides
    /// here is whether to run the body.
    pub(super) evaluated: bool,
    /// What the body threw, if it did — §16.2.1.6 step 9's `[[EvaluationError]]`.
    ///
    /// Kept because a module that threw must throw the **same** value at every later importer
    /// rather than being run again: `import("m")` twice over a module whose body failed rejects
    /// twice with one error object.
    pub(super) failure: Option<crate::value::Value>,
}
