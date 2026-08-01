//! §22.2.1's vocabulary — the flags a pattern is read under, and the tree it becomes.
//!
//! # Why the tree and the reader are apart
//!
//! Everything here is what a *parsed* pattern is; [`super::parser`] is the one thing that builds
//! one and [`super::matcher`] the one thing that walks it. Keeping the vocabulary in a file of its
//! own is the same split `crate::ast` and `crate::parser` make, and for the same reason: a reader
//! asking what a `Node` can be should not have to walk past the grammar to find out.
//!
//! # Why `Flags` lives here rather than with the object
//!
//! Several of them change what the *tree* means — `u` makes a surrogate pair one character, `v`
//! makes fewer patterns valid at all — so a pattern that does not carry its flags is not a pattern
//! anyone can use. `RegExp.prototype.flags` reads the same struct, which is why its spelling order
//! is here too.

/// Why a pattern could not be read.
///
/// One type rather than a code and a message, because every one of these becomes the same
/// `SyntaxError` to a program and the text is the only part it sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// What was wrong, in the words a `SyntaxError` will carry.
    pub message: &'static str,
    /// Whether this is a pattern the specification forbids, or one this engine has not built.
    ///
    /// A program cannot tell the two apart and is not meant to: `new RegExp("(?i:x)")` throws a
    /// SyntaxError either way, because a syntax an engine does not implement is a syntax it does
    /// not accept. The difference is for the *engine's own* accounting — see
    /// [`crate::compile::ErrorKind::BadPattern`], which is an early error the conformance harness
    /// judges, against `Unsupported`, which it declines to judge at all.
    ///
    /// Getting this backwards is not a small mistake in one direction. A refusal recorded as an
    /// early error passes every test that asserts "this must be rejected" — and a proposal's
    /// negative tests are exactly that shape, so an engine implementing none of it would be
    /// credited with the half of the suite that says no.
    pub unimplemented: bool,
}

impl Error {
    /// The error carrying this message — a pattern the specification says is not one.
    pub(super) fn at(message: &'static str) -> Self {
        Self {
            message,
            unimplemented: false,
        }
    }

    /// The same, for a pattern this engine has not been taught rather than one that is wrong.
    pub(super) fn unsupported(message: &'static str) -> Self {
        Self {
            message,
            unimplemented: true,
        }
    }
}

/// §22.2.1.5's flags, as the set they are.
///
/// A struct rather than a bitmask so that each is named where it is read. `d`, `v` and the Unicode
/// property forms are accepted and recorded; what depends on them is the matcher's business.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// `d` — `hasIndices`, which adds the `indices` property to a match.
    pub indices: bool,
    /// `g` — `global`, which makes `lastIndex` advance between matches.
    pub global: bool,
    /// `i` — `ignoreCase`.
    pub ignore_case: bool,
    /// `m` — `multiline`, which makes `^` and `$` see line terminators.
    pub multiline: bool,
    /// `s` — `dotAll`, which makes `.` match a line terminator too.
    pub dot_all: bool,
    /// `u` — `unicode`, which matches by code point rather than by code unit.
    pub unicode: bool,
    /// `v` — `unicodeSets`, which implies everything `u` does and adds set notation.
    pub unicode_sets: bool,
    /// `y` — `sticky`, which anchors every attempt at `lastIndex`.
    pub sticky: bool,
}

impl Flags {
    /// §22.2.1.5 — the flags as written, or a `SyntaxError`.
    ///
    /// A repeated flag is an error, not an idempotent no-op, and so is any letter that is not one
    /// of the eight. `u` and `v` are mutually exclusive: they disagree about what a character class
    /// means, so a pattern claiming both has no reading.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut flags = Self::default();
        for letter in text.chars() {
            let seen = match letter {
                'd' => &mut flags.indices,
                'g' => &mut flags.global,
                'i' => &mut flags.ignore_case,
                'm' => &mut flags.multiline,
                's' => &mut flags.dot_all,
                'u' => &mut flags.unicode,
                'v' => &mut flags.unicode_sets,
                'y' => &mut flags.sticky,
                _ => return Err(Error::at("this is not a regular expression flag")),
            };
            if *seen {
                return Err(Error::at("a regular expression flag is repeated"));
            }
            *seen = true;
        }
        if flags.unicode && flags.unicode_sets {
            return Err(Error::at("the u and v flags cannot be used together"));
        }
        Ok(flags)
    }

    /// Whether the pattern is read in one of the two Unicode modes — §22.2.1's `[+U]` parameter.
    ///
    /// `v` implies everything `u` does, so nearly every rule that asks about `u` means this. The
    /// two are told apart only where `v`'s set notation differs.
    #[must_use]
    pub fn unicode_mode(self) -> bool {
        self.unicode || self.unicode_sets
    }

    /// The flags as `RegExp.prototype.flags` spells them — §22.2.6.4's fixed order.
    #[must_use]
    pub fn spelled(self) -> String {
        let mut text = String::new();
        for (present, letter) in [
            (self.indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.unicode_sets, 'v'),
            (self.sticky, 'y'),
        ] {
            if present {
                text.push(letter);
            }
        }
        text
    }
}

/// §22.2.1's `Assertion`, in the four forms that consume nothing and look nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assertion {
    /// `^`.
    Start,
    /// `$`.
    End,
    /// `\b`.
    WordBoundary,
    /// `\B`.
    NotWordBoundary,
}

/// The six `CharacterClassEscape`s — §22.2.1's `d`, `D`, `s`, `S`, `w`, `W`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassEscape {
    /// `\d` and `\D`, where the flag is whether it is the negated spelling.
    Digit(bool),
    /// `\s` and `\S` — §22.2.2.9's `WhiteSpace` and `LineTerminator` together.
    Space(bool),
    /// `\w` and `\W` — the ASCII word characters, and nothing else without `i` and `u`.
    Word(bool),
}

/// One entry inside `[…]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassItem {
    /// A single code point.
    Single(u32),
    /// `a-z`, inclusive at both ends.
    Range(u32, u32),
    /// A class escape, which stands for a set and so cannot be an end of a range.
    Escape(ClassEscape),
}

/// What a `(` opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKind {
    /// `(…)` — capturing, numbered from one in the order the `(` appear.
    Capturing(u32),
    /// `(?:…)`.
    NonCapturing,
    /// `(?<name>…)` — capturing *and* named; both spellings reach the same group.
    Named(u32, String),
    /// `(?=…)` and `(?!…)`, where the flag is whether it is the negated form.
    Lookahead(bool),
    /// `(?<=…)` and `(?<!…)`.
    Lookbehind(bool),
}

/// A parsed pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// An `Alternative` with no `Term`s — what `//` and the halves of `(a|)` are.
    Empty,
    /// `a|b|c` — tried left to right, and the **first** that matches wins even if a later one
    /// would match more. §22.2.2.3 is explicit about that, and it is why `/a|ab/` matches `a`.
    Alternation(Vec<Node>),
    /// `abc` — every term in order.
    Sequence(Vec<Node>),
    /// One code point, matched as itself.
    Character(u32),
    /// `.` — every code point but a line terminator, unless `s` says otherwise.
    Any,
    /// `[…]` and `[^…]`.
    Class {
        /// Whether the class was written `[^…]`.
        negated: bool,
        /// What is in it, in the order written.
        items: Vec<ClassItem>,
    },
    /// One of the six single-letter class escapes, outside a class.
    Escape(ClassEscape),
    /// `(…)` in any of its five forms.
    Group {
        /// Which form.
        kind: GroupKind,
        /// What is inside it.
        body: Box<Node>,
    },
    /// `\1` — what a numbered group captured, or nothing if it has not captured yet.
    Backreference(u32),
    /// `\k<name>` — the same, by name.
    NamedBackreference(String),
    /// `^`, `$`, `\b`, `\B`.
    Assert(Assertion),
    /// A quantified term.
    Repeat {
        /// What is repeated.
        node: Box<Node>,
        /// The least number of times, which may be zero.
        min: u32,
        /// The most, or `None` for unbounded.
        max: Option<u32>,
        /// Whether the quantifier was written without a trailing `?`. A greedy quantifier tries
        /// the longest count first; a lazy one the shortest. Both can match the same strings, so
        /// this decides only *which* match is found — and that is observable through the captures.
        greedy: bool,
    },
}

/// A pattern and everything about it a matcher needs that is not the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The tree.
    pub node: Node,
    /// How many capturing groups there are, which fixes the length of a match's capture list.
    pub groups: u32,
    /// Every group name, paired with the group it names, in the order written.
    pub names: Vec<(String, u32)>,
    /// The flags it was parsed under, since several of them change what the tree means.
    pub flags: Flags,
}

#[cfg(test)]
mod tests {
    use super::Flags;

    #[test]
    fn flags_are_a_set_and_a_repeat_is_an_error() {
        assert_eq!(
            Flags::parse("gimsuy").map(|f| f.spelled()).as_deref(),
            Ok("gimsuy")
        );
        // Spelled in §22.2.6.4's fixed order, whatever order they were written in.
        assert_eq!(
            Flags::parse("yus").map(|f| f.spelled()).as_deref(),
            Ok("suy")
        );
        assert_eq!(Flags::parse("").map(|f| f.spelled()).as_deref(), Ok(""));
        assert_eq!(
            Flags::parse("gg").err().map(|e| e.message),
            Some("a regular expression flag is repeated")
        );
        assert_eq!(
            Flags::parse("x").err().map(|e| e.message),
            Some("this is not a regular expression flag")
        );
        // `u` and `v` disagree about what a class means, so a pattern claiming both has no reading.
        assert_eq!(
            Flags::parse("uv").err().map(|e| e.message),
            Some("the u and v flags cannot be used together")
        );
        assert!(Flags::parse("v").is_ok_and(|f| f.unicode_mode() && !f.unicode));
    }
}
