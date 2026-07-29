//! §6.1.5 and §20.4 — the Symbol type, and the one thing that makes it unlike every other value.
//!
//! # A Symbol is its handle
//!
//! Every other primitive is its contents: two Strings spelled `"a"` are the same value, and two
//! `1`s are the same value. Two Symbols made by two calls to `Symbol("a")` are **different values**
//! that will never be equal to each other, however identical their descriptions.
//!
//! That is the whole point of the type. A Symbol is a property key nothing else can guess, so a
//! library can store something on an object it was handed without any chance of colliding with a
//! name the object's owner chose. Its description exists only to be read back by a debugger — it
//! takes no part in equality, and [`Symbol::description`] is the only thing that ever looks at it.
//!
//! So this is an arena of descriptions and the handle *is* the value, which is what
//! [`crate::heap::ObjectId`] already does for objects and what DR-0010 argues for.
//!
//! # The two kinds, and why only one of them is here
//!
//! §20.4.2.2's registry — `Symbol.for("a")` — hands back the *same* Symbol for the same key, across
//! realms, for as long as the process runs. That is a table from a String to a Symbol and it lives
//! beside this one; a registered Symbol is an ordinary Symbol that something else happens to be
//! holding a reference to, and nothing about it is different here.
//!
//! # What a Symbol may not do
//!
//! Be turned into a String by accident. §7.1.17 `ToString` **throws** for a Symbol, which is why
//! `"" + Symbol()` is a TypeError and `String(Symbol())` is not — `String` has a step of its own
//! for exactly this, and it is the only way to get the description out as text.

use crate::heap::StringId;

/// A handle to a Symbol — §6.1.5, and the value itself rather than a pointer to it.
///
/// Comparison is on this and nothing else, so two Symbols are equal exactly when they are the
/// same Symbol. Deriving `PartialEq` here is the whole of §7.2.10's rule for the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub(super) usize);

impl SymbolId {
    /// Where this Symbol sits in the arena, for the collector's mark set.
    pub(super) fn index(self) -> usize {
        self.0
    }
}

/// What a Symbol carries — §20.4's `[[Description]]`, and nothing else.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The text a debugger shows, if the Symbol was given one.
    ///
    /// `None` and `Some("")` are different: `Symbol().description` is `undefined` and
    /// `Symbol("").description` is `""`, and §20.4.3.2 makes `toString` spell them `"Symbol()"`
    /// both times. So this cannot be flattened to an empty String.
    pub(super) description: Option<StringId>,
    /// The registry key this Symbol was made under, if it was made by `Symbol.for`.
    ///
    /// §20.4.2.7 `Symbol.keyFor` is the only thing that reads it, and it is what tells a
    /// registered Symbol from an ordinary one with the same description — which is otherwise a
    /// difference nothing could observe.
    pub(super) registered: Option<StringId>,
}
