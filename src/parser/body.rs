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

    /// A field initialiser or a static block (§15.7).
    ///
    /// Both are evaluated by a synthetic method with the class as its home object, so both have
    /// `super.a` and `new.target` and neither has `super(…)` — §15.7.1 forbids a `SuperCall` in
    /// either outright, there being no parent constructor to reach from something that is not one.
    pub(super) const CLASS_INITIALIZER: Self = Self {
        super_allowed: SuperAllowed::PROPERTY_ONLY,
        new_target_allowed: true,
    };

    /// The Script a **direct** `eval` evaluates — §19.2.1.1 steps 5.d to 5.f.
    ///
    /// The one body whose context is decided at *run time*. §19.2.1.1 asks three questions about
    /// the execution the call was made from — is there a function at all, does it have a `super`
    /// binding, is it a derived constructor — and grants `new.target`, `super.a` and `super(…)`
    /// one for one. So `eval("super.m()")` written inside a method is legal and the same text at
    /// the top of a script is a Syntax Error, which is not something the text can decide.
    pub(super) const fn eval(context: EvalContext) -> Self {
        Self {
            super_allowed: SuperAllowed {
                property: context.in_method,
                call: context.in_derived_constructor,
            },
            new_target_allowed: context.in_function,
        }
    }

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

/// What §19.2.1.1 steps 3.b.ii to 3.b.iv learn about the execution a direct `eval` was called from.
///
/// A struct rather than three `bool` parameters in a row, which is a call whose arguments can be
/// silently swapped — and these three are exactly alike to the compiler and answer for three
/// different constructs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvalContext {
    /// Step 3.b.ii — whether there is a function around the call at all. Grants `new.target`.
    pub in_function: bool,
    /// Step 3.b.iii's `HasSuperBinding` — whether that function is a method. Grants `super.a`.
    pub in_method: bool,
    /// Step 3.b.iv — whether it is a derived constructor. Grants `super(…)`.
    pub in_derived_constructor: bool,
    /// Whether the call was written in a class field's initialiser — §15.7.1.
    ///
    /// The one question here that is not about what the text may *say*: it forbids `arguments`
    /// outright, wherever in the evaluated text it appears. `arguments` in an initialiser would be
    /// the initialiser's own method's, which is nobody's idea of what it means, so the clause
    /// refuses the word rather than answering it.
    pub in_field_initializer: bool,
}
