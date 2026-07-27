//! What a function body inherits from the production that opened it.
//!
//! Three grammar facts are decided by *which* function you are in rather than by where you are in
//! it, and none of them is something a `FunctionBody` can work out for itself: the same
//! `{ … }` means different things after `class C { m` and after `function f`. So the caller says,
//! and this is what it says with.
//!
//! # Why an arrow inherits and a function replaces
//!
//! An arrow has no `this`, no home object and no `[[Construct]]` of its own, so the three things
//! here all reach through one:
//!
//! ```js
//! class C extends D { constructor() { () => super(); } }   // legal
//! class C extends D { constructor() { function f() { super(); } } }  // not
//! function f() { () => new.target; }                       // legal
//! () => new.target;                                        // not, at the top of a Script
//! ```
//!
//! §8.4's `Contains` says the same thing from the other side: it descends into an `ArrowFunction`
//! looking for `NewTarget`, `SuperProperty`, `SuperCall`, `this` and `arguments`, and stops at
//! every other function. So an arrow passes [`Parser::body_context`] straight through, and a
//! function passes a fresh one.
//!
//! `[Yield]` is *not* here, and that is the asymmetry worth knowing: an arrow's parameters keep it
//! and its body drops it, so it does not travel with a body — see [`super::generator`].

use super::Parser;

/// What `super` may mean where the parser currently is.
///
/// Two forms and two rules. `SuperProperty` — `super.a` — reads from the home object's prototype,
/// which every method has and no plain function does. `SuperCall` — `super(…)` — calls the parent
/// constructor, which only the constructor of a derived class has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SuperAllowed {
    /// Whether `super.a` may stand here — true inside any method.
    pub property: bool,
    /// Whether `super(…)` may stand here — true only in a derived constructor.
    pub call: bool,
}

impl SuperAllowed {
    /// Neither, which is where a script and a plain function both start.
    pub(super) const NEITHER: Self = Self {
        property: false,
        call: false,
    };

    /// What an object literal's or a class's ordinary method grants (§13.3.7).
    ///
    /// Every method has a home object, so `super.a` is always legal in one; `super(…)` never is,
    /// there being no parent constructor for a method to reach.
    pub(super) const PROPERTY_ONLY: Self = Self {
        property: true,
        call: false,
    };
}

/// The whole of what a `FunctionBody` is told by whoever opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BodyContext {
    /// §13.3.7, §13.3.5.
    pub super_allowed: SuperAllowed,
    /// Whether `new.target` may stand here (§13.3).
    ///
    /// True in every function body and false at the top of a `Script` — §16.1.1 makes a
    /// `ScriptBody` containing a `NewTarget` a Syntax Error, and there is nothing for it to mean
    /// there. Separate from `super_allowed` because a plain function grants this and grants
    /// neither of those.
    pub new_target_allowed: bool,
}

impl BodyContext {
    /// The top of a `Script`: no `super`, no `new.target`.
    pub(super) const SCRIPT: Self = Self {
        super_allowed: SuperAllowed::NEITHER,
        new_target_allowed: false,
    };

    /// A plain `FunctionBody`. §15.2.1 makes one containing either form of `super` a Syntax Error
    /// outright, however deep inside a method it is written.
    pub(super) const FUNCTION: Self = Self {
        super_allowed: SuperAllowed::NEITHER,
        new_target_allowed: true,
    };

    /// A `MethodDefinition`'s body, whose `super` is the caller's to say.
    pub(super) const fn method(super_allowed: SuperAllowed) -> Self {
        Self {
            super_allowed,
            new_target_allowed: true,
        }
    }
}

impl Parser<'_> {
    /// Whether `super.a` may stand at the cursor.
    pub(super) fn super_property_allowed(&self) -> bool {
        self.body_context.super_allowed.property
    }

    /// Whether `super(…)` may stand at the cursor.
    pub(super) fn super_call_allowed(&self) -> bool {
        self.body_context.super_allowed.call
    }
}
