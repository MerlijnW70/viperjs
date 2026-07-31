//! §22.2.2 — the matcher, which is where a pattern meets a string.
//!
//! # Why backtracking, and why that is not a shortcut
//!
//! The specification's own semantics are a backtracking matcher: §22.2.2.1 turns each node into a
//! *Matcher*, a function taking a state and a continuation. A backreference is not a regular
//! language and lookbehind is not either, so no finite automaton decides this — and the observable
//! answer is not "does it match" but "*which* match is found", which the order of the attempts
//! decides. `/a|ab/` matches `a` because the first alternative is tried first, and an engine
//! answering `ab` would be wrong however fast it was.
//!
//! # The continuation, made concrete
//!
//! §22.2.2's continuation is a closure. A closure here would have to borrow the matcher mutably
//! while the matcher is already borrowed, so it is a **chain of frames** instead: each is built on
//! the stack of the call that needs it and points at the one behind it. That is the same structure
//! a closure would have captured, written where the borrow checker can see it, and it is why
//! [`Cont`] has a variant per thing that can still be owed — the rest of a sequence, the closing of
//! a group, another turn of a quantifier.
//!
//! Recursion is over the pattern rather than the subject, so its depth is the pattern's nesting
//! times the repetition count — which is why the step budget below is the thing that keeps
//! DR-0002's promise, and not the shape of the recursion.
//!
//! # The one thing a program can make expensive
//!
//! `/(a+)+b/` against a long run of `a`s is exponential, and that is a property of the semantics
//! every engine implements rather than a fault in this one. A slow match is not a panic, but a
//! program should not be able to hang its host either, so the steps are counted and the attempt is
//! abandoned. Abandoning reports *no match*, which the specification does not authorise; it is the
//! least bad of three bad answers and it is written down here rather than hidden.

use super::parser::{Assertion, ClassEscape, ClassItem, GroupKind, Node, Pattern};

/// How much work one match attempt may cost before it is abandoned.
///
/// Ten million is far past any honest pattern — a linear match over a megabyte costs a few million
/// — and far short of a wait that reads as a hang. A policy figure rather than a behaviour: what
/// the tests pin is that *a* budget stops an exponential pattern, which
/// [`Matcher::with_budget`] lets them show without spending ten million steps to do it.
const MAX_STEPS: u64 = 10_000_000;

/// Where a capture reached, in code units.
///
/// `None` for a group that has not participated, which is a different thing from one that matched
/// emptily: `/(a)?/` leaves group 1 undefined where `/(a?)/` leaves it `""`, and every consumer of
/// a match has to be able to tell those apart.
pub type Capture = Option<(usize, usize)>;

/// What a successful match found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Where the whole match began and ended, in code units.
    pub span: (usize, usize),
    /// Each capturing group, numbered from one — so index zero is group one.
    pub captures: Vec<Capture>,
}

/// What is still owed when the current node has matched.
///
/// The two lifetimes are the pattern's and the chain's: a frame borrows nodes from the pattern and
/// points at a frame living further up the Rust stack. Nothing here is allocated.
enum Cont<'a, 'c> {
    /// Nothing — the match is complete.
    Done,
    /// The rest of a sequence.
    Terms {
        /// The terms not yet matched.
        terms: &'a [Node],
        /// What follows the sequence.
        next: &'c Cont<'a, 'c>,
    },
    /// A capturing group ends here, and this is where it began.
    Close {
        /// Which group, counted from zero.
        index: usize,
        /// Where it started.
        start: usize,
        /// What follows the group.
        next: &'c Cont<'a, 'c>,
    },
    /// §22.2.2.5's `d` — one more turn of a quantifier, or the end of it.
    Again {
        /// What is repeated.
        body: &'a Node,
        /// How many more turns are required.
        min: u32,
        /// How many more are allowed, or `None` for unbounded.
        max: Option<u32>,
        /// Whether to prefer taking another turn.
        greedy: bool,
        /// Where this turn began, which is what step 2.a compares against.
        start: usize,
        /// What follows the quantifier.
        next: &'c Cont<'a, 'c>,
    },
    /// An assertion's body must finish exactly here — what a lookbehind is asking.
    EndAt(usize),
}

/// A pattern, ready to be run against subjects.
pub struct Matcher<'a> {
    pattern: &'a Pattern,
    /// The subject, as code units. §22.2.2 indexes by code unit even under `u`, where it *reads* by
    /// code point — so the input stays units and only the reading changes.
    input: &'a [u16],
    captures: Vec<Capture>,
    steps: u64,
    budget: u64,
}

/// What one attempt answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// It matched, ending here.
    Matched(usize),
    /// It did not.
    Failed,
}

impl<'a> Matcher<'a> {
    /// A matcher for this pattern over this subject.
    #[must_use]
    pub fn new(pattern: &'a Pattern, input: &'a [u16]) -> Self {
        Self::with_budget(pattern, input, MAX_STEPS)
    }

    /// The same, with a ceiling of its own.
    ///
    /// For a host that wants a tighter one, and for the tests: an exponential pattern proves the
    /// budget works whatever the budget is, and proving it against ten million steps would cost
    /// ten million steps on every run of the suite.
    #[must_use]
    pub fn with_budget(pattern: &'a Pattern, input: &'a [u16], budget: u64) -> Self {
        Self {
            pattern,
            input,
            captures: vec![None; pattern.groups as usize],
            steps: 0,
            budget,
        }
    }

    /// §22.2.7.2's loop — try at `start`, and at every later position unless `y` forbids it.
    ///
    /// Answers `None` for no match anywhere. A sticky pattern tries exactly one position, which is
    /// the whole of what `y` means and the reason this is one function and not two.
    pub fn find(&mut self, start: usize) -> Option<Match> {
        // No guard for a start past the end: the range is then empty and the answer is `None` of
        // its own accord.
        for at in start..=self.input.len() {
            for slot in &mut self.captures {
                *slot = None;
            }
            self.steps = 0;
            if let Outcome::Matched(end) = self.node(&self.pattern.node, at, &Cont::Done) {
                return Some(Match {
                    span: (at, end),
                    captures: self.captures.clone(),
                });
            }
            if self.pattern.flags.sticky {
                return None;
            }
        }
        None
    }

    /// Match `node` at `at`, then whatever `cont` still owes.
    fn node(&mut self, node: &'a Node, at: usize, cont: &Cont<'a, '_>) -> Outcome {
        // The only place work is counted. Every unbounded amount of it is a node matched again —
        // `cont` walks a chain whose length the pattern fixes, and `repeat` reaches one of the two.
        // Counting in all three said the same thing three times, and no test could tell them apart.
        self.steps += 1;
        if self.steps > self.budget {
            return Outcome::Failed;
        }
        match node {
            Node::Empty => self.cont(at, cont),
            Node::Sequence(terms) => self.terms(terms, at, cont),
            // §22.2.2.3 — the first branch that matches *and whose continuation matches* wins.
            // Each is tried with the same continuation, so a branch that matches but leaves the
            // rest unmatchable does not stop the next branch being tried.
            Node::Alternation(branches) => {
                for branch in branches {
                    if let Outcome::Matched(end) = self.node(branch, at, cont) {
                        return Outcome::Matched(end);
                    }
                }
                Outcome::Failed
            }
            Node::Character(code) => match self.read(at) {
                Some((found, next)) if self.same(found, *code) => self.cont(next, cont),
                _ => Outcome::Failed,
            },
            // §22.2.2.7 — `.` is every character *but* a line terminator, and `s` removes even that
            // exception.
            Node::Any => match self.read(at) {
                Some((found, next)) if self.pattern.flags.dot_all || !is_line_terminator(found) => {
                    self.cont(next, cont)
                }
                _ => Outcome::Failed,
            },
            Node::Escape(escape) => match self.read(at) {
                Some((found, next)) if matches_escape(*escape, found) => self.cont(next, cont),
                _ => Outcome::Failed,
            },
            Node::Class { negated, items } => match self.read(at) {
                Some((found, next)) if self.in_class(found, items) != *negated => {
                    self.cont(next, cont)
                }
                _ => Outcome::Failed,
            },
            Node::Assert(assertion) => match self.asserts(*assertion, at) {
                true => self.cont(at, cont),
                false => Outcome::Failed,
            },
            Node::Group { kind, body } => self.group(kind, body, at, cont),
            Node::Backreference(number) => self.backreference(self.capture_of(*number), at, cont),
            Node::NamedBackreference(name) => {
                let held = self
                    .pattern
                    .names
                    .iter()
                    .find(|(had, _)| had == name)
                    .and_then(|(_, number)| self.capture_of(*number));
                self.backreference(held, at, cont)
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.repeat(node, *min, *max, *greedy, at, cont),
        }
    }

    /// The terms of a sequence, one at a time, each continuing into the rest.
    fn terms(&mut self, terms: &'a [Node], at: usize, cont: &Cont<'a, '_>) -> Outcome {
        match terms.split_first() {
            None => self.cont(at, cont),
            Some((first, rest)) => {
                let next = Cont::Terms {
                    terms: rest,
                    next: cont,
                };
                self.node(first, at, &next)
            }
        }
    }

    /// Pay off one frame of what is owed.
    fn cont(&mut self, at: usize, cont: &Cont<'a, '_>) -> Outcome {
        match cont {
            Cont::Done => Outcome::Matched(at),
            Cont::Terms { terms, next } => self.terms(terms, at, next),
            Cont::EndAt(wanted) => match at == *wanted {
                true => Outcome::Matched(at),
                false => Outcome::Failed,
            },
            // The span is recorded *before* the rest runs, because a backreference in the rest must
            // be able to see it: `/(a)\\1/` needs group 1 while `\1` is being matched. And it is put
            // back if the rest fails, or a group would appear to have captured on a path that was
            // abandoned.
            Cont::Close { index, start, next } => {
                let held = self.captures.get(*index).copied().flatten();
                if let Some(slot) = self.captures.get_mut(*index) {
                    *slot = Some((*start, at));
                }
                match self.cont(at, next) {
                    Outcome::Matched(end) => Outcome::Matched(end),
                    Outcome::Failed => {
                        if let Some(slot) = self.captures.get_mut(*index) {
                            *slot = held;
                        }
                        Outcome::Failed
                    }
                }
            }
            // §22.2.2.5's `d`, step 2.
            Cont::Again {
                body,
                min,
                max,
                greedy,
                start,
                next,
            } => {
                // Step 2.a — a body that matched nothing must not be repeated once the minimum is
                // met, or `/(a*)*/` would never stop. The count still rises while the minimum is
                // unmet, so a required turn is never skipped.
                if *min == 0 && at == *start {
                    return Outcome::Failed;
                }
                let fewer = min.saturating_sub(1);
                let less = max.map(|most| most.saturating_sub(1));
                self.repeat(body, fewer, less, *greedy, at, next)
            }
        }
    }

    /// §22.2.2.5's `RepeatMatcher`.
    fn repeat(
        &mut self,
        body: &'a Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        at: usize,
        cont: &Cont<'a, '_>,
    ) -> Outcome {
        // Step 1 — no turns left, so the quantifier is simply over.
        if max == Some(0) {
            return self.cont(at, cont);
        }
        // Steps 3 to 5 — the groups *inside* the body forget what they captured on the previous
        // turn, and that reset belongs to the state another turn is attempted from. Stopping here
        // uses the state as it was: §22.2.2.5 says `m(xr, d)` and `c(x)`, two different states, and
        // an implementation that mutates one list has to put it back to tell them apart.
        //
        // The difference is exactly `/(a*)*/` against `"aaa"`. Leaking the reset into the stop
        // path loses the `aaa` group 1 had captured on the turn that succeeded.
        let held = self.captures.clone();
        let again = Cont::Again {
            body,
            min,
            max,
            greedy,
            start: at,
            next: cont,
        };
        // Steps 6 to 10 — a required turn is taken without asking; an optional one is tried before
        // or after stopping, according to greed. Both orders try both, so greed decides *which*
        // match is found and never whether one is.
        if min != 0 {
            self.forget(body);
            return self.node(body, at, &again);
        }
        if greedy {
            self.forget(body);
            return match self.node(body, at, &again) {
                Outcome::Matched(end) => Outcome::Matched(end),
                Outcome::Failed => {
                    self.captures.clone_from(&held);
                    self.cont(at, cont)
                }
            };
        }
        match self.cont(at, cont) {
            Outcome::Matched(end) => Outcome::Matched(end),
            Outcome::Failed => {
                self.forget(body);
                self.node(body, at, &again)
            }
        }
    }

    /// Set every capturing group inside `node` back to having captured nothing.
    fn forget(&mut self, node: &Node) {
        match node {
            Node::Group { kind, body } => {
                if let GroupKind::Capturing(number) | GroupKind::Named(number, _) = kind
                    && let Some(index) =
                        usize::try_from(*number).ok().and_then(|n| n.checked_sub(1))
                    && let Some(slot) = self.captures.get_mut(index)
                {
                    *slot = None;
                }
                self.forget(body);
            }
            Node::Sequence(terms) | Node::Alternation(terms) => {
                for term in terms {
                    self.forget(term);
                }
            }
            Node::Repeat { node, .. } => self.forget(node),
            _ => {}
        }
    }

    /// §22.2.2.4's five kinds of bracketed term.
    fn group(
        &mut self,
        kind: &'a GroupKind,
        body: &'a Node,
        at: usize,
        cont: &Cont<'a, '_>,
    ) -> Outcome {
        match kind {
            GroupKind::NonCapturing => self.node(body, at, cont),
            GroupKind::Capturing(number) | GroupKind::Named(number, _) => {
                let Some(index) = usize::try_from(*number).ok().and_then(|n| n.checked_sub(1))
                else {
                    // A number the parse should have refused. Matching without recording it keeps
                    // the promise that nothing here panics.
                    return self.node(body, at, cont);
                };
                let close = Cont::Close {
                    index,
                    start: at,
                    next: cont,
                };
                self.node(body, at, &close)
            }
            // §22.2.2.5 — a lookahead matches without consuming. A *positive* one keeps whatever
            // its body captured; a negative one is required to fail, so nothing it did may be kept.
            GroupKind::Lookahead(negative) => {
                let held = self.captures.clone();
                let found = matches!(self.node(body, at, &Cont::Done), Outcome::Matched(_));
                if found == *negative {
                    self.captures = held;
                    return Outcome::Failed;
                }
                // No second reset for the negative form: it succeeds only when its body *failed*,
                // and a body that failed has already had every capture put back by `Cont::Close`.
                match self.cont(at, cont) {
                    Outcome::Matched(end) => Outcome::Matched(end),
                    Outcome::Failed => {
                        self.captures = held;
                        Outcome::Failed
                    }
                }
            }
            // §22.2.2.5 with the direction reversed. This engine matches forwards, so a lookbehind
            // asks "does the body match *ending* here" from each earlier position — which is the
            // same question and is the one place the two directions are told apart.
            GroupKind::Lookbehind(negative) => {
                let held = self.captures.clone();
                let mut found = false;
                for from in (0..=at).rev() {
                    if matches!(self.node(body, from, &Cont::EndAt(at)), Outcome::Matched(_)) {
                        found = true;
                        break;
                    }
                }
                if found == *negative {
                    self.captures = held;
                    return Outcome::Failed;
                }
                match self.cont(at, cont) {
                    Outcome::Matched(end) => Outcome::Matched(end),
                    Outcome::Failed => {
                        self.captures = held;
                        Outcome::Failed
                    }
                }
            }
        }
    }

    /// What group `number` last captured, if it has.
    fn capture_of(&self, number: u32) -> Capture {
        usize::try_from(number)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.captures.get(index).copied())
            .flatten()
    }

    /// §22.2.2.9 — a backreference matches what its group holds.
    fn backreference(&mut self, held: Capture, at: usize, cont: &Cont<'a, '_>) -> Outcome {
        // A group that has not participated matches the **empty string** rather than failing,
        // which is what makes `/\1(a)/` match `a` and not nothing at all.
        let Some((from, to)) = held else {
            return self.cont(at, cont);
        };
        let width = to.saturating_sub(from);
        if at.saturating_add(width) > self.input.len() {
            return Outcome::Failed;
        }
        for offset in 0..width {
            let wanted = u32::from(self.input[from + offset]);
            let found = u32::from(self.input[at + offset]);
            if !self.same(found, wanted) {
                return Outcome::Failed;
            }
        }
        self.cont(at + width, cont)
    }

    /// One character of the subject and where the next begins.
    ///
    /// Under `u` a surrogate pair is *one* character, so this is where the flag's whole effect on
    /// reading lives: everywhere else the matcher works in code units and does not care.
    fn read(&self, at: usize) -> Option<(u32, usize)> {
        let first = *self.input.get(at)?;
        if self.pattern.flags.unicode_mode()
            && (0xD800..=0xDBFF).contains(&first)
            && let Some(low) = self.input.get(at + 1).copied()
            && (0xDC00..=0xDFFF).contains(&low)
        {
            let code = 0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
            return Some((code, at + 2));
        }
        Some((u32::from(first), at + 1))
    }

    /// §22.2.2.9's `Canonicalize`, as the comparison it exists for.
    ///
    /// ASCII case folding only, which is what `i` can mean without the Unicode case tables. Those
    /// are generated data under DR-0003 and a slice of their own; until then `/ä/i` does not match
    /// `Ä`, which is wrong in a way that is bounded and visible rather than silently approximate.
    fn same(&self, found: u32, wanted: u32) -> bool {
        found == wanted || (self.pattern.flags.ignore_case && fold(found) == fold(wanted))
    }

    /// Whether a character is among a class's items — the class's own `[^…]` is the caller's.
    fn in_class(&self, found: u32, items: &[ClassItem]) -> bool {
        items.iter().any(|item| match item {
            ClassItem::Single(code) => self.same(found, *code),
            ClassItem::Range(low, high) => {
                (*low..=*high).contains(&found)
                    || (self.pattern.flags.ignore_case
                        && ((*low..=*high).contains(&fold(found))
                            || (*low..=*high).contains(&unfold(found))))
            }
            ClassItem::Escape(escape) => matches_escape(*escape, found),
        })
    }

    /// §22.2.2.6's four assertions.
    fn asserts(&self, assertion: Assertion, at: usize) -> bool {
        match assertion {
            // `^` is the start of the input, and with `m` also the position after a line
            // terminator. Note "after", not "at": the terminator itself is not a start.
            Assertion::Start => {
                at == 0
                    || (self.pattern.flags.multiline
                        && at
                            .checked_sub(1)
                            .and_then(|back| self.input.get(back))
                            .is_some_and(|unit| is_line_terminator(u32::from(*unit))))
            }
            Assertion::End => {
                at == self.input.len()
                    || (self.pattern.flags.multiline
                        && self
                            .input
                            .get(at)
                            .is_some_and(|unit| is_line_terminator(u32::from(*unit))))
            }
            // §22.2.2.6's `IsWordChar` on both sides; the boundary is where they disagree.
            Assertion::WordBoundary => self.word_before(at) != self.word_here(at),
            Assertion::NotWordBoundary => self.word_before(at) == self.word_here(at),
        }
    }

    /// Whether the character before `at` is a word character — false at the start of the input.
    fn word_before(&self, at: usize) -> bool {
        at.checked_sub(1)
            .and_then(|back| self.input.get(back))
            .is_some_and(|unit| is_word(u32::from(*unit)))
    }

    /// Whether the character at `at` is one — false at the end.
    fn word_here(&self, at: usize) -> bool {
        self.input
            .get(at)
            .is_some_and(|unit| is_word(u32::from(*unit)))
    }
}

/// ASCII case folding — §22.2.2.9's `Canonicalize` for what this engine can decide.
fn fold(code: u32) -> u32 {
    match code {
        0x61..=0x7A => code - 32,
        _ => code,
    }
}

/// The other direction, for testing a range against both cases of a character.
fn unfold(code: u32) -> u32 {
    match code {
        0x41..=0x5A => code + 32,
        _ => code,
    }
}

/// §11.3's `LineTerminator`, which `.` excludes and `m` makes `^` and `$` see.
fn is_line_terminator(code: u32) -> bool {
    matches!(code, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// §22.2.2.9's `IsWordChar` — ASCII letters, digits and underscore, and nothing else.
fn is_word(code: u32) -> bool {
    matches!(code, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x5F)
}

/// §11.2's `WhiteSpace` together with `LineTerminator`, which is what `\s` means.
fn is_space(code: u32) -> bool {
    matches!(
        code,
        0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x20 | 0xA0 | 0x1680 | 0x2000
            ..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF
    )
}

/// Whether one of the six class escapes covers this character.
fn matches_escape(escape: ClassEscape, code: u32) -> bool {
    match escape {
        ClassEscape::Digit(negated) => (0x30..=0x39).contains(&code) != negated,
        ClassEscape::Space(negated) => is_space(code) != negated,
        ClassEscape::Word(negated) => is_word(code) != negated,
    }
}

#[cfg(test)]
mod tests {
    use super::{Match, Matcher};
    use crate::regexp::parser::{Flags, parse};

    /// What `source` under `flags` finds in `subject`, from position `start`.
    fn find(source: &str, flags: &str, subject: &str, start: usize) -> Option<Match> {
        let flags = Flags::parse(flags).expect("flags should parse");
        let pattern = parse(source, flags).expect("pattern should parse");
        let units: Vec<u16> = subject.encode_utf16().collect();
        Matcher::new(&pattern, &units).find(start)
    }

    /// The matched text, or `None`.
    fn matched(source: &str, subject: &str) -> Option<String> {
        let units: Vec<u16> = subject.encode_utf16().collect();
        find(source, "", subject, 0)
            .map(|found| String::from_utf16_lossy(&units[found.span.0..found.span.1]))
    }

    /// The matched text and every capture, with `-` for one that did not participate.
    fn all(source: &str, flags: &str, subject: &str) -> Option<String> {
        let units: Vec<u16> = subject.encode_utf16().collect();
        let found = find(source, flags, subject, 0)?;
        let text = |span: (usize, usize)| String::from_utf16_lossy(&units[span.0..span.1]);
        let mut parts = vec![text(found.span)];
        for capture in found.captures {
            parts.push(capture.map_or_else(|| "-".to_string(), text));
        }
        Some(parts.join("|"))
    }

    #[test]
    fn a_pattern_finds_its_first_match_and_reports_where_it_was() {
        assert_eq!(matched("b", "abc").as_deref(), Some("b"));
        assert_eq!(matched("bc", "abc").as_deref(), Some("bc"));
        assert_eq!(matched("z", "abc"), None);
        assert_eq!(find("b", "", "abc", 0).map(|f| f.span), Some((1, 2)));
        // The search moves along the subject, so a pattern that cannot match at 0 is still found.
        assert_eq!(find("c", "", "abc", 0).map(|f| f.span), Some((2, 3)));
        // An empty pattern matches at the very first position and consumes nothing.
        assert_eq!(find("", "", "abc", 0).map(|f| f.span), Some((0, 0)));
    }

    #[test]
    fn the_first_alternative_that_works_wins_even_when_a_later_one_would_match_more() {
        // §22.2.2.3 — the rule that separates a specification-following engine from a "best match"
        // one, and the single most visible thing an engine can get wrong.
        assert_eq!(matched("a|ab", "ab").as_deref(), Some("a"));
        assert_eq!(matched("ab|a", "ab").as_deref(), Some("ab"));
        // …but a branch whose continuation cannot match does not win, so the next is tried.
        assert_eq!(matched("(?:a|ab)c", "abc").as_deref(), Some("abc"));
    }

    #[test]
    fn a_greedy_quantifier_takes_as_much_as_it_can_and_gives_back_only_as_needed() {
        assert_eq!(matched("a*", "aaa").as_deref(), Some("aaa"));
        assert_eq!(matched("a*?", "aaa").as_deref(), Some(""));
        assert_eq!(matched("a+?b", "aaab").as_deref(), Some("aaab"));
        // The classic: `.*` takes everything and then hands characters back until `b` fits.
        assert_eq!(matched(".*b", "abcb").as_deref(), Some("abcb"));
        assert_eq!(matched(".*?b", "abcb").as_deref(), Some("ab"));
        assert_eq!(matched("a{2}", "aaa").as_deref(), Some("aa"));
        assert_eq!(matched("a{2,}", "aaa").as_deref(), Some("aaa"));
        assert_eq!(matched("a{2,3}", "aaaa").as_deref(), Some("aaa"));
        assert_eq!(matched("a{4}", "aaa"), None);
    }

    #[test]
    fn a_body_that_matched_nothing_stops_a_quantifier_rather_than_looping_forever() {
        // §22.2.2.5 step 2.a. Without it `/(a*)*/` never terminates, which is the first thing a
        // hand-written backtracker gets wrong.
        assert_eq!(all("(a*)*", "", "aaa").as_deref(), Some("aaa|aaa"));
        assert_eq!(all("(a*)*", "", "b").as_deref(), Some("|-"));
        assert_eq!(matched("(?:)*", "abc").as_deref(), Some(""));
    }

    #[test]
    fn captures_report_where_each_group_reached_and_which_never_participated() {
        assert_eq!(all("(a)(b)", "", "ab").as_deref(), Some("ab|a|b"));
        // The difference an engine must keep: a group that did not participate is *undefined*, and
        // one that matched emptily is the empty string.
        assert_eq!(all("(a)?", "", "b").as_deref(), Some("|-"));
        assert_eq!(all("(a?)", "", "b").as_deref(), Some("|"));
        assert_eq!(all("(a)|(b)", "", "b").as_deref(), Some("b|-|b"));
        // A group inside a quantifier holds what its *last* turn captured.
        assert_eq!(all("(?:(a)|(b))+", "", "ab").as_deref(), Some("ab|-|b"));
    }

    #[test]
    fn a_capture_taken_on_a_path_that_was_abandoned_is_not_kept() {
        // The group matches `a` on the way to a failure, and the successful path never enters it.
        // An engine that writes captures as it goes and does not undo them reports `a` here.
        assert_eq!(all("(?:(a)x)?ab", "", "ab").as_deref(), Some("ab|-"));
    }

    #[test]
    fn a_backreference_matches_what_its_group_holds_and_nothing_when_it_holds_nothing() {
        assert_eq!(matched("(a)\\1", "aa").as_deref(), Some("aa"));
        assert_eq!(matched("(a)\\1", "ab"), None);
        assert_eq!(matched("(ab)c\\1", "abcab").as_deref(), Some("abcab"));
        // A reference to a group written later has nothing yet, and matches the empty string —
        // which is why this finds `a` rather than failing.
        assert_eq!(matched("\\1(a)", "a").as_deref(), Some("a"));
        assert_eq!(all("(?<n>a)\\k<n>", "", "aa").as_deref(), Some("aa|a"));
    }

    #[test]
    fn the_four_assertions_consume_nothing_and_look_where_they_should() {
        assert_eq!(find("^a", "", "ab", 0).map(|f| f.span), Some((0, 1)));
        assert_eq!(matched("^b", "ab"), None);
        assert_eq!(find("b$", "", "ab", 0).map(|f| f.span), Some((1, 2)));
        assert_eq!(matched("a$", "ab"), None);
        // With `m` an anchor sees a line terminator, and without it does not.
        assert_eq!(find("^b", "m", "a\nb", 0).map(|f| f.span), Some((2, 3)));
        assert_eq!(find("^b", "", "a\nb", 0), None);
        assert_eq!(find("a$", "m", "a\nb", 0).map(|f| f.span), Some((0, 1)));
        assert_eq!(find("a$", "", "a\nb", 0), None);
        // `\b` is where a word character and a non-word one meet, at either end included.
        assert_eq!(find("\\bb", "", "a b", 0).map(|f| f.span), Some((2, 3)));
        assert_eq!(matched("\\bb", "ab"), None);
        assert_eq!(find("\\Bb", "", "ab", 0).map(|f| f.span), Some((1, 2)));
    }

    #[test]
    fn a_dot_stops_at_a_line_terminator_unless_the_s_flag_says_otherwise() {
        assert_eq!(matched("a.b", "a\nb"), None);
        assert_eq!(find("a.b", "s", "a\nb", 0).map(|f| f.span), Some((0, 3)));
        assert_eq!(matched("a.b", "axb").as_deref(), Some("axb"));
    }

    #[test]
    fn a_class_matches_its_members_and_a_negated_one_matches_everything_else() {
        assert_eq!(matched("[abc]", "xbz").as_deref(), Some("b"));
        assert_eq!(matched("[a-c]+", "abcd").as_deref(), Some("abc"));
        assert_eq!(matched("[^a-c]", "abcd").as_deref(), Some("d"));
        assert_eq!(matched("[\\d]+", "ab12").as_deref(), Some("12"));
        assert_eq!(matched("\\w+", " ab_1 ").as_deref(), Some("ab_1"));
        assert_eq!(matched("\\s+", "a  b").as_deref(), Some("  "));
        assert_eq!(matched("\\D+", "12ab").as_deref(), Some("ab"));
        // A negated class still has to consume a character, so it fails at the end of the subject.
        assert_eq!(matched("[^x]", ""), None);
    }

    #[test]
    fn the_i_flag_folds_ascii_case_for_characters_ranges_and_backreferences() {
        assert_eq!(find("abc", "i", "ABC", 0).map(|f| f.span), Some((0, 3)));
        assert_eq!(find("[a-z]+", "i", "ABC", 0).map(|f| f.span), Some((0, 3)));
        assert_eq!(find("[A-Z]+", "i", "abc", 0).map(|f| f.span), Some((0, 3)));
        assert_eq!(find("(a)\\1", "i", "aA", 0).map(|f| f.span), Some((0, 2)));
        assert_eq!(find("abc", "", "ABC", 0), None);
    }

    #[test]
    fn a_lookahead_asserts_without_consuming_and_a_negative_one_asserts_the_opposite() {
        assert_eq!(find("a(?=b)", "", "ab", 0).map(|f| f.span), Some((0, 1)));
        assert_eq!(matched("a(?=b)", "ac"), None);
        assert_eq!(find("a(?!b)", "", "ac", 0).map(|f| f.span), Some((0, 1)));
        assert_eq!(matched("a(?!b)", "ab"), None);
        // A positive lookahead keeps what it captured; a negative one cannot, because it only
        // succeeds when its body failed.
        assert_eq!(all("(?=(a))a", "", "a").as_deref(), Some("a|a"));
        assert_eq!(all("(?!(b))a", "", "a").as_deref(), Some("a|-"));
    }

    #[test]
    fn a_lookbehind_asks_about_what_came_before_and_consumes_none_of_it() {
        assert_eq!(find("(?<=a)b", "", "ab", 0).map(|f| f.span), Some((1, 2)));
        assert_eq!(matched("(?<=a)b", "cb"), None);
        assert_eq!(find("(?<!a)b", "", "cb", 0).map(|f| f.span), Some((1, 2)));
        assert_eq!(matched("(?<!a)b", "ab"), None);
        // A lookbehind of more than one character, which is where "try every earlier start" earns
        // its keep.
        assert_eq!(find("(?<=ab)c", "", "abc", 0).map(|f| f.span), Some((2, 3)));
        assert_eq!(
            find("(?<=a+)b", "", "aaab", 0).map(|f| f.span),
            Some((3, 4))
        );
        // At the very start there is nothing behind, so a positive lookbehind fails and a negative
        // one succeeds.
        assert_eq!(matched("(?<=a)a", "a"), None);
        assert_eq!(find("(?<!a)a", "", "a", 0).map(|f| f.span), Some((0, 1)));
    }

    #[test]
    fn the_sticky_flag_tries_one_position_and_the_search_otherwise_moves_along() {
        assert_eq!(find("b", "y", "ab", 0), None);
        assert_eq!(find("b", "y", "ab", 1).map(|f| f.span), Some((1, 2)));
        assert_eq!(find("b", "", "ab", 0).map(|f| f.span), Some((1, 2)));
        // A start past the end of the subject finds nothing rather than panicking.
        assert_eq!(find("", "", "ab", 3), None);
        assert_eq!(find("", "", "ab", 2).map(|f| f.span), Some((2, 2)));
    }

    #[test]
    fn the_u_flag_reads_a_surrogate_pair_as_one_character() {
        // Without it `.` matches one code unit and so takes half of an astral character; with it,
        // one `.` takes the whole thing.
        assert_eq!(find(".", "u", "😀", 0).map(|f| f.span), Some((0, 2)));
        assert_eq!(find(".", "", "😀", 0).map(|f| f.span), Some((0, 1)));
        assert_eq!(
            find("\\u{1F600}", "u", "x😀", 0).map(|f| f.span),
            Some((1, 3))
        );
    }

    #[test]
    fn a_pattern_that_would_take_forever_gives_up_rather_than_hanging() {
        // `/(a+)+$/` against a run of `a`s and one `b` is the standard exponential case. What is
        // reported is "no match", which the specification does not authorise — but the alternative
        // is a host that stops responding, and this way the choice is visible.
        //
        // Run against a small budget, because what is being shown is that *a* ceiling stops it.
        // Showing it against the real one would cost ten million steps on every run of the suite,
        // and would say nothing more.
        let subject = "a".repeat(40) + "b";
        let flags = Flags::parse("").expect("flags should parse");
        let pattern = parse("(a+)+$", flags).expect("pattern should parse");
        let units: Vec<u16> = subject.encode_utf16().collect();
        assert_eq!(Matcher::with_budget(&pattern, &units, 50_000).find(0), None);
        // …and the same small budget does not disturb an honest pattern over the same subject: a
        // ceiling that stopped ordinary matches would pass the test above for the wrong reason.
        let honest = parse("a+b$", flags).expect("pattern should parse");
        assert_eq!(
            Matcher::with_budget(&honest, &units, 50_000)
                .find(0)
                .map(|found| found.span.1),
            Some(41)
        );
    }

    #[test]
    fn the_budget_stops_a_match_at_the_step_it_says_and_not_a_step_later() {
        // The exponential case above shows *a* ceiling works. This shows it is the ceiling asked
        // for: an ordinary pattern is stopped by a budget one step short of what it needs, and
        // finishes with exactly enough. Without both halves a budget that never fires, or one
        // off by one, passes.
        let flags = Flags::parse("").expect("flags should parse");
        let pattern = parse("aaaa", flags).expect("pattern should parse");
        let units: Vec<u16> = "aaaa".encode_utf16().collect();
        // Said absolutely as well: the derivation below moves with the comparison and so cannot
        // see an off-by-one in it. One node is matched, so one step is exactly enough.
        let one = parse("a", flags).expect("pattern should parse");
        let single: Vec<u16> = "a".encode_utf16().collect();
        assert!(Matcher::with_budget(&one, &single, 0).find(0).is_none());
        assert!(Matcher::with_budget(&one, &single, 1).find(0).is_some());
        let enough = (1..200)
            .find(|budget| {
                Matcher::with_budget(&pattern, &units, *budget)
                    .find(0)
                    .is_some()
            })
            .expect("some budget should be enough");
        assert!(
            Matcher::with_budget(&pattern, &units, enough - 1)
                .find(0)
                .is_none(),
            "one step short of {enough} should not match"
        );
        assert!(
            Matcher::with_budget(&pattern, &units, enough)
                .find(0)
                .is_some(),
            "{enough} steps should be enough"
        );
    }

    #[test]
    fn a_backreference_wider_than_what_is_left_fails_rather_than_reading_past_the_end() {
        // DR-0002. Without the width check this indexes past the subject, and the answer is a
        // panic rather than a non-match — which is the one outcome no input may produce.
        assert_eq!(matched("(aa)\\1", "aaa"), None);
        assert_eq!(matched("(aa)\\1", "aaaa").as_deref(), Some("aaaa"));
        // …and a reference that fits exactly at the end still matches, so the check is a bound and
        // not a refusal to reach the last character.
        assert_eq!(matched("(a)\\1", "aa").as_deref(), Some("aa"));
        assert_eq!(find("(a)\\1", "", "aab", 0).map(|f| f.span), Some((0, 2)));
    }

    #[test]
    fn case_folding_happens_only_when_the_i_flag_asked_for_it() {
        // Both the single-character comparison and the range test fold, and both must stop folding
        // without the flag — one of them quietly ignoring it makes `/[a-z]/` match `A`.
        assert_eq!(find("abc", "", "ABC", 0), None);
        assert_eq!(find("[a-z]+", "", "ABC", 0), None);
        assert_eq!(find("[A-Z]+", "", "abc", 0), None);
        assert_eq!(find("(a)\\1", "", "aA", 0), None);
    }

    #[test]
    fn a_capture_made_inside_a_quantifier_that_then_failed_is_not_kept() {
        // The repeat matched `a` and recorded it, then the `x` after it failed and the whole
        // quantifier was abandoned for the second alternative. Group 1 must look untouched.
        assert_eq!(all("(?:(a)+x|a)", "", "a").as_deref(), Some("a|-"));
        // …and one that failed inside a lookahead is not kept either.
        assert_eq!(all("(?!(a)b)a", "", "a").as_deref(), Some("a|-"));
    }

    #[test]
    fn nothing_a_pattern_can_say_makes_the_matcher_panic() {
        // DR-0002 over the pair rather than over either alone: the parser accepts these and the
        // matcher has to survive them.
        let awkward = [
            ("(){0}", ""),
            ("(a|){2,}", "aa"),
            ("(?:a?)*", "aaa"),
            ("[^]*", "abc"),
            ("(?<=(a))\\1", "aa"),
            ("\\1(a)\\1", "aa"),
            ("(?:(a)|b)*\\1", "abab"),
            ("a{0,0}", "a"),
            ("(?=)", "a"),
            ("(?<=)", "a"),
        ];
        for (source, subject) in awkward {
            let flags = Flags::parse("").expect("flags should parse");
            let Ok(pattern) = parse(source, flags) else {
                continue;
            };
            let units: Vec<u16> = subject.encode_utf16().collect();
            let _ = Matcher::new(&pattern, &units).find(0);
        }
    }
}
