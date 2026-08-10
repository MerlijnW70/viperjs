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

/// §22.2.2.9 `Canonicalize`, whole.
///
/// Here rather than in the matcher because [`First`] has to agree with it exactly: a prefilter that
/// canonicalised differently from the comparison it is filtering for would skip a branch that
/// matches.
///
/// **The two branches are different functions and the flag chooses between them**, which is the
/// whole of step 1 against steps 3 to 10. A pattern carrying `u` or `v` folds; one carrying neither
/// uppercases, and then refuses its own answer when it is not a single code unit or when it would
/// take a non-ASCII code unit to an ASCII one. They disagree on real characters: `/ſ/iu`
/// matches `s` and `/ſ/i` does not, because long s folds to `s` and uppercases to `S` — which
/// step 9 then refuses.
///
/// This used to be `a`–`z` and nothing else, so `/café/i` did not match `CAFÉ`. The comment saying
/// so called it "wrong in a way that is bounded and visible"; it was bounded and it was not
/// visible, because test262 barely reaches it — a differential sweep is what found it.
#[inline]
pub(crate) fn canonicalize(code: u32, unicode: bool) -> u32 {
    // ASCII is an index and everything else is a search, because this is a **hot path**: `same`
    // runs per character compared, so a binary search there costs every ordinary `i` pattern a
    // dozen comparisons per character where the arithmetic it replaced cost one.
    //
    // **The bound is the array's own length and not a literal**, which is the part worth copying.
    // Written as `if code < 0x80` with the mapping as arithmetic, the comparison is a decision no
    // input can distinguish — U+0080 has no case mapping, so both sides of the boundary answer the
    // same thing — and that is an untestable branch sitting in the middle of the hottest function
    // here. `get` puts the boundary where the data already is.
    let ascii = match unicode {
        true => &crate::unicode_case_table::FOLD_ASCII,
        false => &crate::unicode_case_table::UPPER_ASCII,
    };
    if let Some(found) = ascii.get(code as usize) {
        return *found;
    }
    canonicalize_wide(code, unicode)
}

/// [`canonicalize`] past ASCII, kept out of line so the ASCII arm above inlines into the matcher.
///
/// **This split is worth 3.3× on an ordinary `i` pattern**, and the number is why it is a split
/// rather than one function: `same` runs per character compared, and a body carrying two static
/// references and a binary search is too big to inline into it — so ASCII paid for the table it
/// never reads. Measured on a scan of 8,800 characters, 300 times: 12 ms before the tables, 40 ms
/// with one function, 12 ms again with two.
#[inline(never)]
fn canonicalize_wide(code: u32, unicode: bool) -> u32 {
    let table = match unicode {
        true => crate::unicode_case_table::SIMPLE_FOLD,
        false => crate::unicode_case_table::UPPER_CANON,
    };
    mapped(table, code).unwrap_or(code)
}

/// The value `table` gives `code`, if it gives one.
fn mapped(table: &[(u32, u32)], code: u32) -> Option<u32> {
    table
        .binary_search_by_key(&code, |(from, _)| *from)
        .ok()
        .and_then(|at| table.get(at).map(|(_, to)| *to))
}

/// Every code point that canonicalises to the same value as `code`, `code` itself included.
///
/// §22.2.2.7's `CharacterSetMatcher` asks whether **any** member of a class canonicalises to the
/// same value as the input. For one code point that is a comparison of canonical forms; for a
/// *range* it is a question about every code point between two bounds, which cannot be asked one
/// at a time. So the equivalence class is walked instead, and it is never more than a handful —
/// the generated orbit links each member to the next and closes the ring.
///
/// A code point in no class answers with itself alone, which is the overwhelming majority.
pub(crate) fn case_class(code: u32, unicode: bool) -> impl Iterator<Item = u32> {
    let mut at = Some(code);
    std::iter::from_fn(move || {
        let here = at?;
        // The ring closes on the code point it started from, which is what ends the walk. A code
        // point in no class links to itself and ends immediately.
        at = orbit_link(here, unicode).filter(|next| *next != code);
        Some(here)
    })
}

/// The next member of `code`'s equivalence class, or `None` when it is alone in it.
///
/// **ASCII is an array index and everything else is a search**, because this runs per character
/// tested against a range — the hottest path the matcher has. Measured before the arrays existed:
/// a scan of a mixed subject took 29% longer than the ASCII-only fold it replaced. A link may
/// still leave ASCII, which is how `s` reaches long s under folding, and the walk then carries on
/// through the searched table.
#[inline]
fn orbit_link(code: u32, unicode: bool) -> Option<u32> {
    let (ascii, table) = match unicode {
        true => (
            &crate::unicode_case_table::FOLD_ORBIT_ASCII,
            crate::unicode_case_table::FOLD_ORBIT,
        ),
        false => (
            &crate::unicode_case_table::UPPER_ORBIT_ASCII,
            crate::unicode_case_table::UPPER_ORBIT,
        ),
    };
    if let Some(next) = ascii.get(code as usize) {
        return (*next != code).then_some(*next);
    }
    mapped(table, code)
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
    /// **One bit test**, because the set already holds every input that could begin the branch.
    ///
    /// The set built for a literal stores its whole case-equivalence class, so an input that
    /// canonicalises to that literal is in the set as itself — there is nothing to canonicalise
    /// here. That is what lets this run at parse time without being told the flags, and it is why
    /// this is now cheaper than the ASCII fold it replaced rather than dearer: an orbit walk per
    /// *literal* is paid once, where one per *input position* would be paid on the hot path this
    /// prefilter exists to keep cheap.
    #[must_use]
    pub fn may_start(self, code: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Only { ascii, wide } => Self::holds(ascii, wide, code),
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

    /// The set holding `code` and everything that could match it under `i`, which is what one
    /// literal character contributes.
    ///
    /// **The folding class alone, because it contains the other one.** The flags are not known
    /// here, so the set has to admit everything either canonicalisation could send this way — and
    /// folding is the coarser of the two: checked over every code unit, no uppercase class has a
    /// member its folding class lacks. Chaining both was therefore a second walk that could not
    /// add anything, which is what mutation coverage said by surviving the flag's inversion.
    fn only(code: u32) -> Self {
        let mut ascii = 0u128;
        let mut wide = false;
        for member in std::iter::once(code).chain(case_class(code, true)) {
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
    use super::{First, Flags, GroupKind, Node, canonicalize, case_class};
    use crate::unicode_case_table::{FOLD_ORBIT, SIMPLE_FOLD, UPPER_CANON, UPPER_ORBIT};

    /// The checked-in shape of Unicode 17.0.0. These four numbers are the tables' checksum.
    ///
    /// They are not computed from the tables they check — they come from the UCD as counted at
    /// generation time. A regeneration that moves them is a Unicode version bump, and it should
    /// arrive as a commit that says so rather than as a diff nobody read. DR-0003's rule, applied
    /// to the third generated table.
    #[test]
    fn the_case_tables_match_the_unicode_version_they_claim() {
        assert_eq!(SIMPLE_FOLD.len(), 1_486);
        // The four ASCII arrays are a fixed 128 by type, so what is worth asserting about them is
        // that they agree with the searched tables about the boundary rather than overlapping it.
        assert!(SIMPLE_FOLD.iter().all(|(from, _)| *from >= 0x80));
        assert!(UPPER_CANON.iter().all(|(from, _)| *from >= 0x80));
        assert!(FOLD_ORBIT.iter().all(|(from, _)| *from >= 0x80));
        assert!(UPPER_ORBIT.iter().all(|(from, _)| *from >= 0x80));
        assert_eq!(FOLD_ORBIT.len(), 2_942);
        assert_eq!(UPPER_CANON.len(), 1_143);
        assert_eq!(UPPER_ORBIT.len(), 2_261);
    }

    /// Every table is sorted, because every lookup is a binary search.
    ///
    /// A table that arrived unsorted would answer `None` for entries that are in it — a *miss*
    /// rather than a crash, so every affected character would quietly stop folding and nothing
    /// would fail loudly. The generator sorts; this is the assertion that it did.
    #[test]
    fn every_case_table_is_sorted_by_the_code_point_it_is_searched_by() {
        for (name, table) in [
            ("SIMPLE_FOLD", SIMPLE_FOLD),
            ("FOLD_ORBIT", FOLD_ORBIT),
            ("UPPER_CANON", UPPER_CANON),
            ("UPPER_ORBIT", UPPER_ORBIT),
        ] {
            assert!(
                table.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "{name} is not sorted and strictly increasing"
            );
        }
    }

    /// Every orbit closes, and none of them is long.
    ///
    /// `case_class` walks the ring until it returns to where it started, so a link that pointed
    /// out of its own class — or at itself — would loop for ever or stop early. Both are silent:
    /// the first hangs the matcher on one character, the second drops a case from a range test.
    #[test]
    fn every_case_class_closes_on_itself_within_a_handful_of_members() {
        for unicode in [true, false] {
            let table = match unicode {
                true => FOLD_ORBIT,
                false => UPPER_ORBIT,
            };
            for &(code, _) in table {
                let members: Vec<u32> = case_class(code, unicode).take(16).collect();
                assert!(
                    members.len() < 16,
                    "the class of {code:#x} did not close within sixteen members"
                );
                assert!(members.contains(&code), "{code:#x} is not in its own class");
                // Every member agrees on the canonical form, which is what makes the class one.
                for member in &members {
                    assert_eq!(
                        canonicalize(*member, unicode),
                        canonicalize(code, unicode),
                        "{member:#x} is linked to {code:#x} but canonicalises elsewhere"
                    );
                }
            }
        }
    }

    /// Every uppercase class is inside the folding class of the same code point.
    ///
    /// [`First::only`] takes the folding class alone and depends on this: a code unit that shared
    /// an uppercase class with a literal but not a folding class would be skipped by the prefilter
    /// and never reach the matcher. Folding is the coarser equivalence, and this is the assertion
    /// that it stays so — a Unicode version bump could in principle break it.
    #[test]
    fn an_uppercase_class_never_escapes_the_folding_class_of_the_same_code_point() {
        for code in 0..0x10000u32 {
            let folded: Vec<u32> = case_class(code, true).collect();
            for member in case_class(code, false) {
                assert!(
                    folded.contains(&member),
                    "{member:#x} shares an uppercase class with {code:#x} and not a folding one"
                );
            }
        }
    }

    /// §22.2.2.9's two branches, on the characters where they disagree.
    ///
    /// Asserted here as well as through the matcher because these are the rows a *table* mistake
    /// shows up in first: reading one table for both branches passes every ASCII test there is.
    #[test]
    fn the_two_canonicalisations_are_different_functions() {
        // Step 1 folds; steps 3 to 10 uppercase. For a plain letter they agree on the answer by
        // different routes, which is why the interesting rows are below.
        assert_eq!(canonicalize(0xE9, true), 0xE9, "é folds to itself");
        assert_eq!(canonicalize(0xC9, true), 0xE9, "É folds to é");
        assert_eq!(canonicalize(0xE9, false), 0xC9, "é uppercases to É");
        // Step 9 — a non-ASCII code unit whose uppercase is ASCII keeps itself, which is what
        // separates the long s and the Kelvin sign from the letters they fold to.
        assert_eq!(canonicalize(0x17F, true), 0x73, "long s folds to s");
        assert_eq!(
            canonicalize(0x17F, false),
            0x17F,
            "…and uppercases to itself"
        );
        assert_eq!(
            canonicalize(0x212A, true),
            0x6B,
            "the Kelvin sign folds to k"
        );
        assert_eq!(canonicalize(0x212A, false), 0x212A);
        // Step 7 — an uppercase form of more than one code unit is refused whole.
        assert_eq!(
            canonicalize(0xDF, false),
            0xDF,
            "ß uppercases to SS, so it stands"
        );
        assert_eq!(canonicalize(0xDF, true), 0xDF);
        // A code point in no table is its own answer, which is nearly all of them.
        assert_eq!(canonicalize(0x41, true), 0x61);
        assert_eq!(canonicalize(0x4E00, true), 0x4E00);
        assert_eq!(canonicalize(0x4E00, false), 0x4E00);
    }

    /// The bitmap holding exactly these code points.
    fn bits(codes: &[u32]) -> u128 {
        codes.iter().fold(0u128, |set, code| set | (1u128 << code))
    }

    #[test]
    fn a_first_set_holds_each_literal_and_everything_that_could_match_it() {
        // Asserted as the **exact** set rather than through `may_start`, and that is the point: a
        // prefilter that is too *wide* answers every question correctly and only runs slower, so
        // nothing behavioural can tell it from a right one. This is the structural row that can.
        assert_eq!(
            First::of(&Node::Character(97)),
            First::Only {
                ascii: bits(&[65, 97]),
                wide: false
            },
            "a lower-case letter brings its upper-case form with it"
        );
        // …and **the other direction too**, which the ASCII fold did not give. `may_start` used to
        // canonicalise the input to make up for it; now the set is complete and the input is tested
        // as itself, so a literal that did not bring its lower-case form would skip a branch that
        // matches.
        assert_eq!(
            First::of(&Node::Character(65)),
            First::Only {
                ascii: bits(&[65, 97]),
                wide: false
            },
            "an upper-case letter brings its lower-case form with it"
        );
        // §22.2.2.9's two branches disagree about `s`, and the set is the union because the flags
        // are not known here: `ſ` folds to `s` under `u` and stands alone without it, so the
        // literal has to admit a non-ASCII input — which the bitmap cannot hold and `wide` does.
        assert_eq!(
            First::of(&Node::Character(0x73)),
            First::Only {
                ascii: bits(&[0x53, 0x73]),
                wide: true
            },
            "a literal `s` can be begun by the long s under `u`"
        );
        // `k` is the same shape through the Kelvin sign, and `q` is the control: no non-ASCII code
        // point canonicalises to it, so `wide` stays false and the filter keeps its teeth.
        assert_eq!(
            First::of(&Node::Character(0x6B)),
            First::Only {
                ascii: bits(&[0x4B, 0x6B]),
                wide: true
            }
        );
        assert_eq!(
            First::of(&Node::Character(0x71)),
            First::Only {
                ascii: bits(&[0x51, 0x71]),
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
