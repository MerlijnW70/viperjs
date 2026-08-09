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
    /// `\p{…}` inside a class, which stands for a set for the same reason and so cannot either.
    Property(crate::unicode_property::Property),
    /// `\q{abc|def}` — §22.2.1's `ClassStringDisjunction`, the one operand that matches **strings**.
    ///
    /// Every alternative as written, including the ones a code point long and the empty one. It is
    /// two things at once and they are kept together because the grammar keeps them together: an
    /// alternative of exactly one code point is an ordinary member of the class's character set,
    /// and every other length is a *string* the class can consume whole. So `[\q{a|bc}]` matches
    /// `a` where a code-point predicate answers, and `bc` where only a sequence can.
    Strings(Vec<Vec<u32>>),
    /// A nested class, or a set operation over several — §22.2.1's `ClassSetExpression`.
    ///
    /// Only a `v` pattern makes one. A `u` pattern's `[` inside a class is an ordinary bracket and
    /// a `v` pattern's opens this, which is the difference the two flags cannot share a pattern
    /// over.
    Nested(ClassSet),
}

/// A class's contents, and how they combine — §22.2.1's `ClassSetExpression`.
///
/// The top level of every class is one of these, and so is every `[…]` written inside one. Which
/// is why the *negation* lives here rather than beside the operation: `[^\d&&[0-4]]` negates the
/// intersection, and a nested `[^…]` negates its own contents before the level above combines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSet {
    /// Whether it was written `[^…]`.
    pub negated: bool,
    /// What joins the items.
    pub operation: ClassOperation,
    /// The operands, in the order written.
    pub items: Vec<ClassItem>,
}

/// The three ways §22.2.1 joins a class's operands.
///
/// They do not mix at one level: `ClassIntersection` and `ClassSubtraction` are separate
/// productions and neither admits the other, so `[\d&&\w--a]` is a Syntax Error and
/// `[[\d&&\w]--a]` is how to write it. That is what makes this one value per level rather than
/// one per separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassOperation {
    /// The ordinary class: anything that matches any operand.
    ///
    /// The only one a `u` pattern has, and the only one that admits a *range* — §22.2.1 puts
    /// `ClassSetRange` in `ClassUnion` and nowhere else, so `[a-z&&b-d]` has no derivation.
    Union,
    /// `&&` — everything that matches **every** operand.
    Intersection,
    /// `--` — everything matching the first operand and none of the rest.
    Difference,
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

/// §22.2.2.9's `Canonicalize`, in the part this engine implements — ASCII case folding.
///
/// Here rather than in the matcher because [`First`] has to agree with it exactly: a prefilter that
/// folded differently from the comparison it is filtering for would skip a branch that matches.
pub(crate) fn fold(code: u32) -> u32 {
    match code {
        0x61..=0x7A => code - 32,
        _ => code,
    }
}

/// What an alternative can *begin* with, as much of it as is worth knowing.
///
/// §22.2.2.3 tries every alternative of a disjunction at every position, and the matcher did
/// exactly that: measured, the cost was `branches × positions × 5.2 ns` with no attention paid to
/// the character actually sitting there. An alternation of two thousand entity names — which is
/// what `he.decode` is, and it is under `htmlparser2` and `cheerio` — spent 138 µs deciding that an
/// eight-character string containing none of them contained none of them. See `lab/NOTES.md`'s
/// `alternation-width`.
///
/// This is a **prefilter and never a decision**: it may say yes to a branch that cannot match, and
/// the branch is then tried and fails as before. What it may never do is say no to one that can, so
/// every case that is not obviously analysable is [`First::Any`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum First {
    /// Try this branch at every position. Either it can match **empty** — so no character it might
    /// consume says anything about whether it matches — or it opens with something that consumes
    /// nothing (an assertion, a lookaround), or with something this does not summarise (a class, a
    /// backreference, a property escape).
    Any,
    /// The branch consumes at least one character, and that character is one of these.
    Only {
        /// Which of the first 128 code points may begin it. A bitmap, because the alternatives
        /// worth filtering are overwhelmingly ASCII — entity names, keywords, operators.
        ascii: u128,
        /// Whether anything at or above 128 may. Not a set: one bit that admits every non-ASCII
        /// code point, which keeps surrogate pairs and the two Unicode modes out of this entirely.
        /// A pattern that is mostly non-ASCII loses the filter and loses nothing else.
        wide: bool,
    },
}

impl First {
    /// Whether a branch summarised by this may begin with `code`.
    ///
    /// Folded on **both** sides: the set holds each literal and its fold, and the input is tested as
    /// itself and as its fold. That makes one summary right for `i` and for its absence, which is
    /// what lets [`First::of`] run at parse time without being told the flags — and it is sound in
    /// the only direction that matters, since a wider set skips fewer branches.
    #[must_use]
    pub fn may_start(self, code: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Only { ascii, wide } => {
                Self::holds(ascii, wide, code) || Self::holds(ascii, wide, fold(code))
            }
        }
    }

    /// One membership test, of a code point that has already been folded or not.
    fn holds(ascii: u128, wide: bool, code: u32) -> bool {
        match code < 128 {
            true => ascii & (1u128 << code) != 0,
            false => wide,
        }
    }

    /// The empty set — the identity [`First::union`] folds from, and never stored: an alternative
    /// that can begin with nothing at all cannot match, and there is no such alternative.
    fn none() -> Self {
        Self::Only {
            ascii: 0,
            wide: false,
        }
    }

    /// The set holding `code` and its fold, which is what one literal character contributes.
    fn only(code: u32) -> Self {
        let mut ascii = 0u128;
        let mut wide = false;
        for member in [code, fold(code)] {
            match member < 128 {
                true => ascii |= 1u128 << member,
                false => wide = true,
            }
        }
        Self::Only { ascii, wide }
    }

    /// Both, for a disjunction whose branches are themselves alternatives.
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Any, _) | (_, Self::Any) => Self::Any,
            (
                Self::Only { ascii, wide },
                Self::Only {
                    ascii: other,
                    wide: other_wide,
                },
            ) => Self::Only {
                ascii: ascii | other,
                wide: wide || other_wide,
            },
        }
    }

    /// What `node` can begin with.
    ///
    /// Descends only the **leftmost** spine — the first term of a sequence, the body of a group,
    /// the repeated node of a quantifier that must run at least once — so it cannot recurse deeper
    /// than the tree it is handed. That tree was built by a parser recursing at least as deep with
    /// fatter frames, so a pattern this could overflow on is one that never parsed (DR-0002).
    #[must_use]
    pub fn of(node: &Node) -> Self {
        match node {
            Node::Character(code) => Self::only(*code),
            // The first term decides, and an `Alternative` with no terms matches empty.
            Node::Sequence(terms) => terms.first().map_or(Self::Any, Self::of),
            // Already computed, one level down, when that alternation was built.
            Node::Alternation { firsts, .. } => {
                firsts.iter().copied().fold(Self::none(), Self::union)
            }
            Node::Group { kind, body } => match kind {
                // A lookaround consumes nothing, so what it can begin with says nothing about what
                // the alternative consumes — and a *negative* one says the opposite of it.
                GroupKind::Lookahead(_) | GroupKind::Lookbehind(_) => Self::Any,
                GroupKind::Capturing(_) | GroupKind::NonCapturing | GroupKind::Named(_, _) => {
                    Self::of(body)
                }
            },
            // A quantifier that may run zero times matches empty; one that must run does not, and
            // then the first character it consumes is the first character of what it repeats.
            Node::Repeat { node, min, .. } if *min >= 1 => Self::of(node),
            _ => Self::Any,
        }
    }
}

/// A parsed pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// An `Alternative` with no `Term`s — what `//` and the halves of `(a|)` are.
    Empty,
    /// `a|b|c` — tried left to right, and the **first** that matches wins even if a later one
    /// would match more. §22.2.2.3 is explicit about that, and it is why `/a|ab/` matches `a`.
    ///
    /// Built with [`Node::alternation`], which is what computes `firsts`; the two fields have to
    /// agree, and a constructor is the only place that can be guaranteed.
    Alternation {
        /// The alternatives, in the order written.
        branches: Vec<Node>,
        /// What each of them can begin with — one entry per branch, same order. See [`First`].
        firsts: Vec<First>,
    },
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
        /// The sequences this class can consume whole, **longest first**.
        ///
        /// §22.2.1's set operations resolved over the `\q{…}` operands, and empty for every class
        /// in a pattern without the `v` flag — which is what keeps this off the hot path: the
        /// matcher asks `is_empty()` once and reads a code point as it always did.
        ///
        /// Only lengths other than one, because a one-code-point alternative is an ordinary member
        /// of the character set and the predicate already answers for it. Sorted here rather than
        /// at each attempt because §22.2.2.7.2 tries the longest candidate first and the order is
        /// therefore part of what the pattern *means*, not an optimisation.
        strings: Vec<Vec<u32>>,
    },
    /// One of the six single-letter class escapes, outside a class.
    Escape(ClassEscape),
    /// `\p{…}` and `\P{…}` — §22.2.1's Unicode property escapes, outside a class.
    Property(crate::unicode_property::Property),
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

impl Node {
    /// §22.2.1's `Disjunction`, with each alternative's [`First`] worked out as it is built.
    ///
    /// The only way to make a [`Node::Alternation`], so the summary cannot drift from the branches
    /// it summarises — the failure that would produce is a branch silently never tried, which is a
    /// wrong answer rather than a slow one.
    #[must_use]
    pub fn alternation(branches: Vec<Node>) -> Self {
        let firsts = branches.iter().map(First::of).collect();
        Self::Alternation { branches, firsts }
    }
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
    use super::{First, Flags, GroupKind, Node};

    /// The bitmap holding exactly these code points.
    fn bits(codes: &[u32]) -> u128 {
        codes.iter().fold(0u128, |set, code| set | (1u128 << code))
    }

    #[test]
    fn a_first_set_holds_each_literal_and_its_fold() {
        // Asserted as the **exact** set rather than through `may_start`, and that is the point: a
        // prefilter that is too *wide* answers every question correctly and only runs slower, so
        // nothing behavioural can tell it from a right one. This is the structural row that can.
        assert_eq!(
            First::of(&Node::Character(97)),
            First::Only {
                ascii: bits(&[65, 97]),
                wide: false
            },
            "a lower-case letter brings its upper-case fold with it"
        );
        // §22.2.2.9's `Canonicalize` here folds *towards* upper case and only over a-z, so an
        // upper-case literal is alone in its set — and `may_start` is what makes that enough.
        assert_eq!(
            First::of(&Node::Character(65)),
            First::Only {
                ascii: bits(&[65]),
                wide: false
            }
        );
        // Exactly 128 is the boundary of the bitmap, and the one code point that tells `<` from
        // `<=`. A set that tried to index it would be shifting a `u128` by its own width.
        assert_eq!(
            First::of(&Node::Character(128)),
            First::Only {
                ascii: 0,
                wide: true
            }
        );
        assert_eq!(
            First::of(&Node::Character(0x1F600)),
            First::Only {
                ascii: 0,
                wide: true
            }
        );
    }

    #[test]
    fn only_the_shapes_that_must_consume_get_a_set() {
        let a = First::Only {
            ascii: bits(&[65, 97]),
            wide: false,
        };
        // A sequence is decided by its first term, and a group by its body.
        assert_eq!(
            First::of(&Node::Sequence(vec![
                Node::Character(97),
                Node::Character(98)
            ])),
            a
        );
        assert_eq!(
            First::of(&Node::Group {
                kind: GroupKind::NonCapturing,
                body: Box::new(Node::Character(97)),
            }),
            a
        );
        // A lookaround consumes nothing, so what is inside it says nothing about what the
        // alternative consumes — and a negated one says the opposite.
        assert_eq!(
            First::of(&Node::Group {
                kind: GroupKind::Lookahead(false),
                body: Box::new(Node::Character(97)),
            }),
            First::Any
        );
        // A quantifier that must run at least once consumes what it repeats; one that may run zero
        // times matches empty, and then no character says anything about whether it matches. The
        // boundary is exactly `min >= 1`.
        assert_eq!(
            First::of(&Node::Repeat {
                node: Box::new(Node::Character(97)),
                min: 1,
                max: None,
                greedy: true,
            }),
            a
        );
        assert_eq!(
            First::of(&Node::Repeat {
                node: Box::new(Node::Character(97)),
                min: 0,
                max: None,
                greedy: true,
            }),
            First::Any
        );
        // Everything not summarised is `Any`, which is the safe answer and never a wrong one.
        assert_eq!(First::of(&Node::Empty), First::Any);
        assert_eq!(First::of(&Node::Any), First::Any);
        assert_eq!(First::of(&Node::Backreference(1)), First::Any);
        assert_eq!(First::of(&Node::Sequence(Vec::new())), First::Any);
    }

    #[test]
    fn a_nested_alternation_contributes_every_branch() {
        // The union, and it has to keep `wide` from *either* side: a disjunction one of whose
        // branches begins with a non-ASCII character can begin with one.
        let inner = Node::alternation(vec![Node::Character(97), Node::Character(0xE9)]);
        assert_eq!(
            First::of(&inner),
            First::Only {
                ascii: bits(&[65, 97]),
                wide: true
            }
        );
        // And a union of ASCII branches stays ASCII — which is a claim about the *identity* the
        // fold starts from as much as about the branches. An empty set whose `wide` were set would
        // make every alternation in the engine admit every non-ASCII character, and nothing
        // behavioural could see it: the filter would still never skip a branch that could match.
        let ascii_only = Node::alternation(vec![Node::Character(97), Node::Character(98)]);
        assert_eq!(
            First::of(&ascii_only),
            First::Only {
                ascii: bits(&[65, 97, 66, 98]),
                wide: false
            }
        );
        // And a branch that cannot be summarised makes the whole union unsummarisable.
        let mixed = Node::alternation(vec![Node::Character(97), Node::Empty]);
        assert_eq!(First::of(&mixed), First::Any);
    }

    #[test]
    fn may_start_admits_a_character_and_its_fold_in_both_directions() {
        let a = First::of(&Node::Character(97));
        assert!(a.may_start(97), "the literal itself");
        assert!(a.may_start(65), "its fold, for a pattern carrying `i`");
        assert!(!a.may_start(98));
        assert!(!a.may_start(0xE9));
        // An upper-case literal is alone in its set, so admitting the lower case is what the *input*
        // being folded does — which is why the test is on both sides and not on one.
        let upper = First::of(&Node::Character(65));
        assert!(upper.may_start(65));
        assert!(upper.may_start(97), "folded on the way in");
        // 128 is above the bitmap, so it is admitted only by `wide` — and a set with no wide member
        // must refuse it rather than indexing past the end.
        assert!(!a.may_start(128));
        let wide = First::of(&Node::Character(200));
        assert!(wide.may_start(200));
        assert!(
            wide.may_start(128),
            "any non-ASCII, which is all `wide` claims"
        );
        assert!(!wide.may_start(97));
        // And `Any` admits everything, including the end-of-input case the matcher asks about
        // separately.
        assert!(First::Any.may_start(0));
        assert!(First::Any.may_start(0x10FFFF));
    }

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
