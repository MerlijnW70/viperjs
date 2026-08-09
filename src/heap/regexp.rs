//! §22.2.3's internal slots — what a `RegExp` object *is*, apart from its properties.
//!
//! # Why the source text is kept beside the parsed pattern
//!
//! `RegExp.prototype.source` must answer the pattern as written, not as understood: `/a\/b/.source`
//! is `a\/b` and no tree can say that. §22.2.6.13 also requires a form that can be put back between
//! slashes and read again, which is why an empty pattern reads as `(?:)` — so the text kept here is
//! the *escaped* one, computed once when the object is made.
//!
//! # Why `lastIndex` is not here
//!
//! It is an ordinary property, writable and non-configurable, and a program may set it to anything
//! at all — including a string, or a number past the end of any subject. §22.2.7.2 reads it with
//! `ToLength` every time rather than trusting it, so keeping a copy in a slot would mean two
//! answers that could disagree.

use crate::regexp::{Flags, Pattern};

/// §22.2.3's `[[OriginalSource]]`, `[[OriginalFlags]]` and `[[RegExpMatcher]]`.
#[derive(Debug, Clone)]
pub struct RegExp {
    /// The pattern, parsed and **shared**.
    ///
    /// §22.2.3's `[[RegExpMatcher]]` is built once when the object is made and never changes, so
    /// nothing needs a copy of it — and a copy is expensive in exactly the way a tree is. Every
    /// match operation used to `clone()` this whole structure, which made the cost of *one* call
    /// proportional to the size of the pattern: `he.decode`'s two-thousand-branch alternation cost
    /// 138 µs to decide that an eight-character string held none of them, and the alternation was
    /// never even entered. See `lab/NOTES.md`'s `alternation-width`.
    ///
    /// An `Rc` rather than a borrow because of what the call site is doing: it reads the pattern out
    /// of the heap and then hands the heap to the matcher's caller, so a reference would hold the
    /// heap borrowed across the match. A refcount bump costs nothing and releases the borrow.
    pattern: std::rc::Rc<Pattern>,
    /// The source as `source` must spell it — §22.2.6.13's escaped form.
    escaped: Vec<u16>,
    /// The flags as written, which `flags` re-spells in its own order.
    flags: Flags,
    /// The source **unescaped**, which is what a pattern built from this one must be built from.
    ///
    /// `new RegExp(/a\/b/)` takes §22.2.3.1's `[[OriginalSource]]` and not `source`'s escaped
    /// spelling, or every round trip would add another backslash.
    source: Vec<u16>,
}

impl RegExp {
    /// A compiled regular expression, with the text `source` will answer.
    #[must_use]
    pub fn new(pattern: Pattern, source: Vec<u16>, escaped: Vec<u16>, flags: Flags) -> Self {
        Self {
            pattern: std::rc::Rc::new(pattern),
            escaped,
            flags,
            source,
        }
    }

    /// `[[OriginalSource]]` — the pattern text as it was given, before escaping.
    #[must_use]
    pub fn source(&self) -> &[u16] {
        &self.source
    }

    /// The parsed pattern, for a caller that can hold the heap borrowed while it reads.
    #[must_use]
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    /// The same, as a handle that outlives the borrow.
    ///
    /// What a match operation wants: it has to let go of the heap before running, and this is how
    /// it takes the pattern with it for the price of a refcount. Cloning the `Pattern` is what this
    /// exists instead of — see the field.
    #[must_use]
    pub fn shared(&self) -> std::rc::Rc<Pattern> {
        std::rc::Rc::clone(&self.pattern)
    }

    /// What `RegExp.prototype.source` answers.
    #[must_use]
    pub fn escaped(&self) -> &[u16] {
        &self.escaped
    }

    /// The flags it was built with.
    #[must_use]
    pub fn flags(&self) -> Flags {
        self.flags
    }
}
