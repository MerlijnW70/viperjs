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
    /// The pattern, parsed. Boxed with the rest because a `Pattern` owns a tree.
    pattern: Pattern,
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
            pattern,
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

    /// The parsed pattern, for the matcher.
    #[must_use]
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
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
