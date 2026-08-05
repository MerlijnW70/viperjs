//! §10.5's Proxy exotic object — a target, a handler, and the pair being revocable together.
//!
//! # Why a Proxy is not a wrapper
//!
//! Every other exotic object in ViperJS answers *more* than an ordinary one: a String object has
//! characters, a TypedArray has elements, an Array has a live `length`. A Proxy answers **less** on
//! its own — it has no properties of its own at all — and instead hands each of §6.1.7.2's internal
//! methods to a function the handler supplies, falling back to the target when there is none.
//!
//! That inverts where the work happens. A String object's extra behaviour lives in the heap,
//! because reading a character needs no interpreter. A Proxy's lives in the interpreter, because
//! calling a trap is calling JavaScript — so what is here is only the state, and every one of the
//! thirteen operations is in [`crate::vm`].
//!
//! # Why revoking clears both
//!
//! §10.5.14's revocation function sets the target *and* the handler to null, and §10.5's every
//! internal method then throws a TypeError. Holding one `Option` for the pair rather than two is
//! what makes "half revoked" unrepresentable: there is no state in which a trap could be looked up
//! on a handler that is gone, or run against a target that is.

use crate::heap::ObjectId;

/// §10.5's `[[ProxyTarget]]` and `[[ProxyHandler]]`, or nothing once revoked.
#[derive(Debug, Clone, Copy)]
pub struct Proxy {
    /// The two, together — `None` once §10.5.14's revoker has run.
    pair: Option<(ObjectId, ObjectId)>,
}

impl Proxy {
    /// A live proxy over this target with this handler.
    #[must_use]
    pub fn new(target: ObjectId, handler: ObjectId) -> Self {
        Self {
            pair: Some((target, handler)),
        }
    }

    /// The target and the handler, or `None` if this proxy has been revoked.
    ///
    /// Answers both or neither, because §10.5's internal methods need both and a revoked proxy has
    /// neither. A caller that could get one without the other would have a case to write for a
    /// state that cannot exist.
    #[must_use]
    pub fn parts(&self) -> Option<(ObjectId, ObjectId)> {
        self.pair
    }

    /// The target, whether or not this proxy is still live.
    ///
    /// For the collector, which has to keep the target reachable for as long as the proxy is —
    /// and for `Array.isArray`, which §7.2.2 says walks through a proxy to ask about its target.
    #[must_use]
    pub fn target(&self) -> Option<ObjectId> {
        self.pair.map(|(target, _)| target)
    }

    /// §10.5.14 — take both away, which nothing puts back.
    pub fn revoke(&mut self) {
        self.pair = None;
    }
}
