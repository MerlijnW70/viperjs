//! §22.2.1 — the pattern grammar, and §22.2.1.1's early errors.
//!
//! # Why the pattern is read as code points and not as code units
//!
//! §22.2.1's grammar is written over `SourceCharacter`, which is a code point. Without the `u` flag
//! a pattern still *matches* code unit by code unit, but it is *parsed* as text — so a surrogate
//! pair in a pattern is one character to the grammar and two to the matcher. Reading the pattern as
//! `char`s and letting the matcher decide how to compare is what keeps those two facts apart.
//!
//! # Why the group count is known before the body is parsed
//!
//! `\1` may appear before the group it names: `/\1(a)/` is a valid pattern whose backreference is
//! to a group written later. So a first pass counts capturing groups and collects their names, and
//! only then is the body parsed — which is also how `\k<name>` can be an error for naming nothing
//! while `\1` with no groups at all is one too.

use super::syntax::{
    Assertion, ClassEscape, ClassItem, ClassOperation, ClassSet, Error, Flags, GroupKind, Node,
    Pattern,
};
use crate::unicode_id::{is_id_continue, is_id_start};
use crate::unicode_property::Property;

/// §22.2.1 — read `source` under `flags`.
///
/// # Errors
///
/// Every §22.2.1.1 early error this implements, as an [`Error`] whose message is what the
/// `SyntaxError` will say.
pub fn parse(source: &str, flags: Flags) -> Result<Pattern, Error> {
    let text: Vec<char> = source.chars().collect();
    // The counting pass. `\1` may name a group written later, and `\k<name>` may too, so neither
    // can be checked while the body is still being read.
    let (groups, names) = survey(&text)?;
    let mut reader = Reader {
        text: &text,
        at: 0,
        flags,
        groups,
        names: &names,
        next_group: 0,
        negated_classes: 0,
    };
    let node = reader.disjunction()?;
    if reader.at < reader.text.len() {
        // The only way out of `disjunction` short of the end is an unmatched `)`.
        return Err(Error::at("a regular expression has an unmatched )"));
    }
    Ok(Pattern {
        node,
        groups,
        names,
        flags,
    })
}

/// Where a group sits in the pattern's alternations — one entry per enclosing `Disjunction`.
///
/// The `u32` pair is *which* disjunction and *which* of its alternatives, and both halves are
/// needed. Depth alone cannot answer §22.2.1.1's question: in `(?:(?<x>a)|b)(?:c|(?<x>d))` the two
/// groups are each the second entry of a path and each in a different alternative *of a different
/// disjunction*, which is not the same as being in different alternatives of one.
type Path = Vec<(u32, u32)>;

/// §22.2.1.1 `MightBothParticipate` — whether one match could fill in both of these groups.
///
/// The clause asks whether any single `Disjunction` has one of them in one `Alternative` and the
/// other in a different one; if any does, at most one can participate and the pair is legal. Two
/// paths from the same pattern share a prefix and then part, so this is one walk:
///
/// - the same disjunction with **different** alternatives is the clause's answer, `false`;
/// - **different** disjunctions means the two are inside separate groups that sit side by side, so
///   one match reaches both — `true`, and the walk stops because nothing deeper is shared;
/// - running out of path means one group encloses the other's position sequentially, also `true`.
fn might_both_participate(left: &Path, right: &Path) -> bool {
    for (here, there) in left.iter().zip(right.iter()) {
        if here.0 != there.0 {
            return true;
        }
        if here.1 != there.1 {
            return false;
        }
    }
    true
}

/// The first pass — how many capturing groups there are, what they are called, and where each sits.
///
/// Counts `(` that are not `(?`, and `(?<` that is not `(?<=` or `(?<!`. Deliberately crude about
/// everything else: it skips escapes and the insides of classes so that `\(` and `[(]` are not
/// counted, and leaves every other question to the real parse.
///
/// It also follows `(`, `)` and `|` well enough to say which alternative of which disjunction each
/// name was written in, which §22.2.1.1 needs and nothing else here does. Crude in the same way:
/// an unbalanced `)` is left to the real parse to complain about, and popping nothing is not an
/// error *here* because it is one *there*.
fn survey(text: &[char]) -> Result<(u32, Vec<(String, u32)>), Error> {
    let mut groups = 0;
    let mut names: Vec<(String, u32)> = Vec::new();
    let mut paths: Vec<Path> = Vec::new();
    // The pattern is itself a `Disjunction`, so there is always a level to be in an alternative of.
    let mut path: Path = vec![(0, 0)];
    let mut disjunctions = 0;
    let mut at = 0;
    let mut in_class = false;
    while at < text.len() {
        match text[at] {
            '\\' => at += 2,
            '[' if !in_class => {
                in_class = true;
                at += 1;
            }
            ']' if in_class => {
                in_class = false;
                at += 1;
            }
            // Every `(` opens a `Disjunction` — a capturing group, `(?:`, and all four lookarounds
            // alike — so every one of them is a level. `|` inside it belongs to that level and not
            // to the one outside, which is the whole reason the alternatives are counted on a stack
            // rather than as one number.
            '|' if !in_class => {
                if let Some(level) = path.last_mut() {
                    level.1 += 1;
                }
                at += 1;
            }
            ')' if !in_class => {
                // An unbalanced `)` is the real parse's to complain about, so the base level is
                // never popped. Popping it would leave every later name with an empty path, and two
                // empty paths compare as "both could participate" — so `/)(?<x>a)|(?<x>b)/` would be
                // refused here for sharing a name, which is not its fault and not what is wrong
                // with it. This pass is crude on purpose; being crude must not mean answering for
                // a question it was not asked.
                if path.len() > 1 {
                    path.pop();
                }
                at += 1;
            }
            '(' if !in_class => {
                at += 1;
                // §22.2.1.1 reads a `GroupSpecifier` as part of `( GroupSpecifier Disjunction )`,
                // so the name sits in the alternative containing the *whole group* and not inside
                // the group's own disjunction. Taken before the level is pushed, for that reason.
                let outer = path.clone();
                disjunctions += 1;
                path.push((disjunctions, 0));
                if text.get(at) != Some(&'?') {
                    groups += 1;
                    continue;
                }
                at += 1;
                if text.get(at) != Some(&'<') {
                    continue;
                }
                // `(?<=` and `(?<!` are lookbehind and name nothing.
                if matches!(text.get(at + 1), Some('=' | '!')) {
                    continue;
                }
                at += 1;
                let Some(end) = text[at..]
                    .iter()
                    .position(|c| *c == '>')
                    .map(|off| at + off)
                else {
                    return Err(Error::at("a group name is not closed"));
                };
                let name: String = text[at..end].iter().collect();
                if name.is_empty() {
                    return Err(Error::at("a group name is empty"));
                }
                if !is_identifier(&name) {
                    return Err(Error::at("a group name is not an identifier"));
                }
                groups += 1;
                names.push((name, groups));
                paths.push(outer);
                // The `>` is left where it is: the loop's own advance steps over it, and a second
                // one here would be a step no test could see the absence of.
                at = end;
            }
            _ => at += 1,
        }
    }
    // §22.2.1.1 — two groups may share a name **only** when no single match could fill in both.
    // Checked here because the names are all in hand and the body parse would otherwise have to
    // carry them through every recursion.
    //
    // Every name against every *earlier* name of the same spelling, which is each pair once. A set
    // of names already met sat in front of this and skipped the first occurrence of each — a branch
    // no pattern could tell from its absence, because a first occurrence has no earlier twin for
    // the scan below to find and the answer was `false` either way.
    for (at, (name, _)) in names.iter().enumerate() {
        let conflicts = names[..at]
            .iter()
            .zip(paths.iter())
            .filter(|((had, _), _)| had == name)
            .any(|(_, earlier)| might_both_participate(earlier, &paths[at]));
        if conflicts {
            return Err(Error::at("two groups have the same name"));
        }
    }
    Ok((groups, names))
}

/// Where the parse has got to.
struct Reader<'a> {
    text: &'a [char],
    at: usize,
    flags: Flags,
    groups: u32,
    names: &'a [(String, u32)],
    next_group: u32,
    /// How many negated classes enclose the position being read — §22.2.1's `MayContainStrings`.
    ///
    /// A count and not a flag because `[^[\q{a}]]` nests, and the inner class is inside a negated
    /// one whether or not it is negated itself. Read only by [`Reader::property_escape`], which is
    /// the one place a property that may contain strings can appear.
    negated_classes: usize,
}

impl Reader<'_> {
    /// The character at the cursor, without moving it.
    fn peek(&self) -> Option<char> {
        self.text.get(self.at).copied()
    }

    /// Whether the cursor sits on this character, and step over it if so.
    fn eat(&mut self, wanted: char) -> bool {
        if self.peek() == Some(wanted) {
            self.at += 1;
            return true;
        }
        false
    }

    /// §22.2.1 `Disjunction` — alternatives separated by `|`.
    fn disjunction(&mut self) -> Result<Node, Error> {
        let mut branches = vec![self.alternative()?];
        while self.eat('|') {
            branches.push(self.alternative()?);
        }
        Ok(match branches.len() {
            1 => branches.remove(0),
            _ => Node::Alternation(branches),
        })
    }

    /// §22.2.1 `Alternative` — terms until a `|` or a `)` or the end.
    fn alternative(&mut self) -> Result<Node, Error> {
        let mut terms = Vec::new();
        while let Some(next) = self.peek() {
            if next == '|' || next == ')' {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.remove(0),
            _ => Node::Sequence(terms),
        })
    }

    /// §22.2.1 `Term` — an atom, and the quantifier it may carry.
    fn term(&mut self) -> Result<Node, Error> {
        let atom = self.atom()?;
        let Some((min, max, greedy)) = self.quantifier()? else {
            return Ok(atom);
        };
        // §22.2.1.1 — `{2,1}` has no reading, and it is a SyntaxError rather than a pattern that
        // never matches.
        if max.is_some_and(|most| most < min) {
            return Err(Error::at(
                "a quantifier's lower bound is above its upper bound",
            ));
        }
        // §22.2.1 — an assertion has nothing to repeat, and `Term :: Assertion` carries no
        // `Quantifier` in the main grammar at all. Annex B §B.1.2.1 adds exactly one exception and
        // it is narrower than "an assertion in sloppy mode": `QuantifiableAssertion` is `(?=…)` and
        // `(?!…)` only, and the production is `[~UnicodeMode]`. So `^*`, `$*` and `\b*` are refused
        // whatever the flags, a lookbehind is refused because it was never quantifiable, and a
        // lookahead is refused **only** under `u` or `v`.
        //
        // The doc here used to say DR-0008 refused Annex B's syntactic extensions "in both", which
        // was true when it was written and outlived its reason: B.3 landed on 2026-08-03 and this
        // clause is the one place the *Unicode* flag still decides the grammar.
        let quantifiable = match &atom {
            Node::Assert(_) => false,
            Node::Group { kind, .. } => match kind {
                GroupKind::Lookbehind(_) => false,
                GroupKind::Lookahead(_) => !self.flags.unicode && !self.flags.unicode_sets,
                _ => true,
            },
            _ => true,
        };
        if !quantifiable {
            return Err(Error::at("this has nothing to repeat"));
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    /// §22.2.1 `Quantifier`, or `None` when the cursor is not on one.
    fn quantifier(&mut self) -> Result<Option<(u32, Option<u32>, bool)>, Error> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.at += 1;
                (0, None)
            }
            Some('+') => {
                self.at += 1;
                (1, None)
            }
            Some('?') => {
                self.at += 1;
                (0, Some(1))
            }
            Some('{') => match self.braced()? {
                Some(bounds) => bounds,
                // Not a quantifier at all — `{` that does not spell one is a literal brace, and the
                // cursor has not moved.
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        // A trailing `?` makes it lazy. §22.2.1's `QuantifierPrefix ?`.
        Ok(Some((min, max, !self.eat('?'))))
    }

    /// `{n}`, `{n,}` or `{n,m}` — or `None`, having moved nothing, when the braces spell none.
    fn braced(&mut self) -> Result<Option<(u32, Option<u32>)>, Error> {
        let start = self.at;
        self.at += 1;
        let Some(min) = self.digits() else {
            self.at = start;
            return Ok(None);
        };
        if self.eat('}') {
            return Ok(Some((min, Some(min))));
        }
        if !self.eat(',') {
            self.at = start;
            return Ok(None);
        }
        // No case of its own for `{n,}`: with no digits after the comma `max` is `None`, which is
        // exactly what that form means, so the general path below already answers it.
        let max = self.digits();
        if !self.eat('}') {
            self.at = start;
            return Ok(None);
        }
        Ok(Some((min, max)))
    }

    /// A run of decimal digits as a number, saturating rather than overflowing.
    ///
    /// `{1000000000000}` is a quantifier with an enormous bound, not a syntax error — and
    /// saturating means the matcher meets `u32::MAX` repetitions, which the heap budget stops long
    /// before the count does.
    fn digits(&mut self) -> Option<u32> {
        let start = self.at;
        let mut value: u32 = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(digit);
            self.at += 1;
        }
        (self.at > start).then_some(value)
    }

    /// §22.2.1 `Atom` and `Assertion` — everything a term can be before its quantifier.
    fn atom(&mut self) -> Result<Node, Error> {
        let Some(next) = self.peek() else {
            return Err(Error::at(
                "a regular expression ends part-way through a term",
            ));
        };
        match next {
            '^' => {
                self.at += 1;
                Ok(Node::Assert(Assertion::Start))
            }
            '$' => {
                self.at += 1;
                Ok(Node::Assert(Assertion::End))
            }
            '.' => {
                self.at += 1;
                Ok(Node::Any)
            }
            '(' => self.group(),
            '[' => self.class(),
            '\\' => self.escape(),
            // §22.2.1's `PatternCharacter` excludes every `SyntaxCharacter`. A quantifier with
            // nothing before it lands here, which is what makes `/*/` an error rather than a
            // pattern matching a star.
            '*' | '+' | '?' => Err(Error::at("this has nothing to repeat")),
            ')' | '|' => Err(Error::at("a regular expression has an unmatched )")),
            // A lone `]` or `}` is a `PatternCharacter` only under Annex B, which DR-0008 leaves
            // out — so both are refused, in Unicode mode and out of it alike.
            ']' => Err(Error::at("a regular expression has an unmatched ]")),
            '}' => Err(Error::at("a regular expression has an unmatched }")),
            // A `{` that did not spell a quantifier. Annex B rereads it as a literal brace, so
            // `/a{/` matches `a{` in a browser and is a SyntaxError here — DR-0008 again.
            '{' => Err(Error::at("a regular expression has an unmatched {")),
            _ => {
                self.at += 1;
                Ok(Node::Character(next as u32))
            }
        }
    }

    /// Whether what follows `(?` spells a modifier group — `(?ims-ims:` and its subsets.
    ///
    /// Read without consuming: this decides *which refusal*, and the parser is about to give up
    /// either way. Deliberately loose — any run of the three modifier letters and a `-`, ending in
    /// a `:` — because a *nearly* well-formed modifier group is exactly what the proposal's
    /// negative tests are made of, and calling those a syntax error would be claiming to implement
    /// the thing they check.
    fn at_a_modifier_group(&self) -> bool {
        let mut at = self.at;
        while let Some(&next) = self.text.get(at) {
            match next {
                'i' | 'm' | 's' | '-' => at += 1,
                // No check that anything was read first: `(?:` is a non-capturing group and the
                // arm above this one has already taken it, so a `:` here always follows at least
                // one modifier letter.
                ':' => return true,
                _ => return false,
            }
        }
        false
    }

    /// §22.2.1's five bracketed forms.
    fn group(&mut self) -> Result<Node, Error> {
        self.at += 1;
        let kind = if self.eat('?') {
            match self.peek() {
                Some(':') => {
                    self.at += 1;
                    GroupKind::NonCapturing
                }
                Some('=') => {
                    self.at += 1;
                    GroupKind::Lookahead(false)
                }
                Some('!') => {
                    self.at += 1;
                    GroupKind::Lookahead(true)
                }
                Some('<') if matches!(self.text.get(self.at + 1), Some('=')) => {
                    self.at += 2;
                    GroupKind::Lookbehind(false)
                }
                Some('<') if matches!(self.text.get(self.at + 1), Some('!')) => {
                    self.at += 2;
                    GroupKind::Lookbehind(true)
                }
                Some('<') => {
                    self.at += 1;
                    let name = self.group_name()?;
                    self.next_group += 1;
                    GroupKind::Named(self.next_group, name)
                }
                // §22.2.1's *modifiers* — `(?i:…)` and `(?-i:…)`. Stage 3 and not ES2023, so it
                // is refused as a thing not built rather than as a thing forbidden. The
                // difference matters to the conformance harness and not to a script: a
                // SyntaxError here would *pass* the proposal's own negative tests, which assert
                // that particular bad modifier sequences are rejected — and passing those while
                // rejecting every good one is a wrong answer wearing a right one's clothes.
                _ if self.at_a_modifier_group() => {
                    return Err(Error::unsupported("the RegExp modifiers proposal"));
                }
                _ => return Err(Error::at("this is not a kind of group")),
            }
        } else {
            self.next_group += 1;
            GroupKind::Capturing(self.next_group)
        };
        let body = self.disjunction()?;
        if !self.eat(')') {
            return Err(Error::at("a regular expression has an unclosed ("));
        }
        Ok(Node::Group {
            kind,
            body: Box::new(body),
        })
    }

    /// A `GroupName` — everything up to the `>`, which the survey has already validated.
    fn group_name(&mut self) -> Result<String, Error> {
        let start = self.at;
        while let Some(next) = self.peek() {
            if next == '>' {
                let name: String = self.text[start..self.at].iter().collect();
                self.at += 1;
                return Ok(name);
            }
            self.at += 1;
        }
        Err(Error::at("a group name is not closed"))
    }

    /// §22.2.1 `CharacterClass`, at the top level of a pattern.
    ///
    /// The contents are a [`ClassSet`] whichever flag is in force, and a plain union is unwrapped
    /// back into the node's own item list — which is what keeps `[a-z]` one level deep for the
    /// matcher and for everything else that reads a tree. Only a set *operation* needs a level of
    /// its own, and it gets one nested inside a union of one.
    fn class(&mut self) -> Result<Node, Error> {
        let set = self.class_set()?;
        match set.operation {
            ClassOperation::Union => Ok(Node::Class {
                negated: set.negated,
                items: set.items,
            }),
            // The negation belongs to the class and the operation to what is inside it, which is
            // the order `[^\d&&[0-4]]` is written in and the order it has to be evaluated in.
            _ => Ok(Node::Class {
                negated: set.negated,
                items: vec![ClassItem::Nested(ClassSet {
                    negated: false,
                    ..set
                })],
            }),
        }
    }

    /// §22.2.1's `ClassContents`, from the `[` through the `]`.
    ///
    /// Called for the outermost class and, in a `v` pattern, for every `[…]` written inside one.
    fn class_set(&mut self) -> Result<ClassSet, Error> {
        self.at += 1;
        let negated = self.eat('^');
        // Read before the contents, which is what lets the contents be told. §22.2.1 makes it a
        // Syntax Error for a **negated** class to contain anything that may match a string, and the
        // one thing that can is a property of strings.
        self.negated_classes += usize::from(negated);
        let mut items = Vec::new();
        let mut operation = ClassOperation::Union;
        loop {
            match self.peek() {
                None => return Err(Error::at("a character class is not closed")),
                Some(']') => {
                    self.at += 1;
                    // `[a--]` has an operator and one operand short of a use for it.
                    if operation != ClassOperation::Union && items.len() < 2 {
                        return Err(Error::at("a set operation needs an operand on both sides"));
                    }
                    self.negated_classes -= usize::from(negated);
                    return Ok(ClassSet {
                        negated,
                        operation,
                        items,
                    });
                }
                Some(_) => {}
            }
            // §22.2.1's `ClassSetReservedDoublePunctuator` — `v` reserves a *doubled* punctuator
            // inside a class for set notation it does not have yet, so `/[&&]/v` is a Syntax Error
            // where `/[&&]/u` is a class holding two ampersands. Checked before the atom is read,
            // because either character alone is fine and it is the pair that is reserved.
            if self.flags.unicode_sets
                && matches!(self.text[self.at..], [here, next, ..] if here == next
                    && is_reserved_double(here))
            {
                return Err(Error::at(
                    "this punctuator is doubled, which a v pattern reserves inside a class",
                ));
            }
            let first = self.class_operand()?;
            // A separator decides what this level is, and every one after it has to agree: §22.2.1
            // gives `ClassIntersection` and `ClassSubtraction` separate productions and neither
            // admits the other, so `[\d&&\w--a]` has no derivation and `[[\d&&\w]--a]` is how
            // to write it.
            if let Some(next) = self.class_operator() {
                // Empty means this is the first separator and it decides the level. Otherwise it
                // has to be the one already chosen — which refuses a second operator *and* a union
                // that has already collected operands, since `Union` is never what a separator says.
                if !items.is_empty() && operation != next {
                    return Err(Error::at(
                        "a set operation may not be mixed with a union or with the other operation",
                    ));
                }
                operation = next;
                items.push(first);
                continue;
            }
            // An operand with no separator after it ends the list, so anything before the `]` that
            // is not the last operand is a union appearing where the operator was promised.
            if operation != ClassOperation::Union && self.peek() != Some(']') {
                return Err(Error::at(
                    "every operand of a set operation must be separated by the same operator",
                ));
            }
            // `-` between two atoms makes a range, but a `-` before the closing `]` is itself an
            // atom: `[a-]` is `a` and `-`, not an unfinished range. §22.2.1 puts a range in
            // `ClassUnion` only, which is what the refusal above has established this is.
            if self.peek() == Some('-') && self.text.get(self.at + 1) != Some(&']') {
                self.at += 1;
                let second = self.class_operand()?;
                let (ClassItem::Single(low), ClassItem::Single(high)) = (&first, &second) else {
                    // §22.2.1.1 — a class escape stands for a set and cannot be an end of a range.
                    // Annex B reads `[\d-x]` as three atoms; DR-0008 leaves that out.
                    return Err(Error::at(
                        "a character class range has a class escape as an end",
                    ));
                };
                if low > high {
                    return Err(Error::at("a character class range runs backwards"));
                }
                items.push(ClassItem::Range(*low, *high));
                continue;
            }
            items.push(first);
        }
    }

    /// The separator after an operand, if there is one — §22.2.1's `&&` and `--`.
    ///
    /// Consumes it. `&&` carries a `[lookahead ≠ &]`, so `&&&` is not an intersection with an
    /// ampersand after it — it is the doubled punctuator the refusal above catches.
    fn class_operator(&mut self) -> Option<ClassOperation> {
        if !self.flags.unicode_sets {
            return None;
        }
        match self.text[self.at..] {
            ['&', '&', ..] if self.text.get(self.at + 2) != Some(&'&') => {
                self.at += 2;
                Some(ClassOperation::Intersection)
            }
            ['-', '-', ..] => {
                self.at += 2;
                Some(ClassOperation::Difference)
            }
            _ => None,
        }
    }

    /// §22.2.1's `ClassSetOperand`, or a `u` pattern's `ClassAtom`.
    ///
    /// The one thing a `v` pattern adds is the nested class, which is why `[` is among the
    /// characters it makes an error of when written alone: inside a `v` class a bracket is not a
    /// bracket, it is an operand opening.
    fn class_operand(&mut self) -> Result<ClassItem, Error> {
        if self.flags.unicode_sets && self.peek() == Some('[') {
            return Ok(ClassItem::Nested(self.class_set()?));
        }
        // §22.2.1's `ClassStringDisjunction` — `\q{abc|def}`, an operand that matches *strings*
        // rather than code points. Refused as **unsupported** and not as a syntax error, which is
        // the difference between a gap and a rule: `\q{}` is a legal `v` operand, so reporting it
        // as bad syntax would pass every test asserting a pattern must be rejected and would be a
        // wrong answer in the one direction this engine cannot afford. It is the same refusal
        // `\p{RGI_Emoji}` gets, and for the same reason — a class that can match more than one
        // code point is a matcher change rather than a parser one.
        if self.flags.unicode_sets && matches!(self.text[self.at..], ['\\', 'q', '{', ..]) {
            return Err(Error::unsupported("a class of strings"));
        }
        self.class_atom()
    }

    /// One entry inside `[…]` — a character, or an escape that may stand for a set.
    fn class_atom(&mut self) -> Result<ClassItem, Error> {
        let Some(next) = self.peek() else {
            return Err(Error::at("a character class is not closed"));
        };
        if next != '\\' {
            // §22.2.1's `ClassSetSyntaxCharacter` — `v` reserves these inside a class for its set
            // notation, so `/[(]/v` is a Syntax Error where `/[(]/u` is a class holding a
            // parenthesis. It is the one place `v` is stricter than `u` rather than merely more
            // capable, and the reason the two flags cannot both be set.
            if self.flags.unicode_sets && is_class_set_syntax(next) {
                return Err(Error::at(
                    "this character must be escaped inside a class in a v pattern",
                ));
            }
            self.at += 1;
            return Ok(ClassItem::Single(next as u32));
        }
        self.at += 1;
        let Some(letter) = self.peek() else {
            return Err(Error::at("a regular expression ends after a backslash"));
        };
        if let Some(property) = self.property_escape(letter) {
            return Ok(ClassItem::Property(property?));
        }
        if let Some(escape) = class_escape(letter) {
            self.at += 1;
            return Ok(ClassItem::Escape(escape));
        }
        // `\b` inside a class is a backspace rather than a word boundary — §22.2.1's `ClassEscape`
        // says so outright, and it is the one escape that means something different in here.
        if letter == 'b' {
            self.at += 1;
            return Ok(ClassItem::Single(0x08));
        }
        // `\-` is an identity escape inside a class in every mode, which it is not outside one.
        if letter == '-' {
            self.at += 1;
            return Ok(ClassItem::Single(u32::from(b'-')));
        }
        Ok(ClassItem::Single(self.character_escape()?))
    }

    /// §22.2.1 `CharacterClassEscape :: p{ UnicodePropertyValueExpression }` — the set it names.
    ///
    /// `None` when the escape is not a property one, which is how both callers tell it apart from
    /// everything else a `\` may begin: the letter has *not* been consumed on that path, so the
    /// caller carries on as it did.
    ///
    /// A property escape only exists in Unicode mode. Outside it `\p` is an identity escape and
    /// matches a `p`, which is what falling through to `character_escape` gives — and is why the
    /// mode is checked here rather than being an error.
    fn property_escape(&mut self, letter: char) -> Option<Result<Property, Error>> {
        if !matches!(letter, 'p' | 'P') || !self.flags.unicode_mode() {
            return None;
        }
        let negated = letter == 'P';
        self.at += 1;
        if self.peek() != Some('{') {
            return Some(Err(Error::at("a property escape needs a braced name")));
        }
        self.at += 1;
        let start = self.at;
        while self.peek().is_some_and(|next| next != '}') {
            self.at += 1;
        }
        if self.peek() != Some('}') {
            return Some(Err(Error::at("a property escape's name is not closed")));
        }
        let spelled: String = self.text[start..self.at].iter().collect();
        self.at += 1;
        if crate::unicode_property::OF_STRINGS.contains(&spelled.as_str()) {
            // §22.2.1's early errors, and all three are the specification refusing rather than
            // praxis not having built something — so they are `Error::at`, which is a `BadPattern`
            // and a real answer about the text, and not `unsupported`, which is a gap and is
            // skipped. Getting that backwards passes every one of these tests for the wrong reason.
            //
            // A property of strings names a set that may contain more than one code point, and each
            // of the three positions below is one the specification cannot give that a meaning in.
            if negated {
                // `\P{RGI_Emoji}` — there is no complement of a set of strings to take.
                return Some(Err(Error::at(
                    "a property of strings may not be negated with \\P",
                )));
            }
            if !self.flags.unicode_sets {
                // Only a `v` pattern has set notation, and only set notation can hold a string.
                return Some(Err(Error::at("a property of strings needs the v flag")));
            }
            if self.negated_classes > 0 {
                // `[^\p{RGI_Emoji}]` — the same refusal as `\P`, reached the other way.
                return Some(Err(Error::at(
                    "a negated class may not contain a property of strings",
                )));
            }
            // What is left is the one form that is legal and unbuilt: matching one.
            return Some(Err(Error::unsupported("a property of strings")));
        }
        let Some(property) = crate::unicode_property::lookup(&spelled) else {
            return Some(Err(Error::at("this is not a Unicode property")));
        };
        Some(Ok(match negated {
            true => property.negate(),
            false => property,
        }))
    }

    /// §22.2.1 `AtomEscape` — everything a `\` can begin outside a class.
    fn escape(&mut self) -> Result<Node, Error> {
        self.at += 1;
        let Some(letter) = self.peek() else {
            return Err(Error::at("a regular expression ends after a backslash"));
        };
        if let Some(property) = self.property_escape(letter) {
            return Ok(Node::Property(property?));
        }
        if let Some(escape) = class_escape(letter) {
            self.at += 1;
            return Ok(Node::Escape(escape));
        }
        match letter {
            'b' => {
                self.at += 1;
                Ok(Node::Assert(Assertion::WordBoundary))
            }
            'B' => {
                self.at += 1;
                Ok(Node::Assert(Assertion::NotWordBoundary))
            }
            'k' => {
                self.at += 1;
                if !self.eat('<') {
                    return Err(Error::at("a named backreference has no name"));
                }
                let name = self.group_name()?;
                // §22.2.1.1 — naming a group that does not exist is an error, which is why the
                // survey runs first. `\k` is what makes a *later* group reachable from an earlier
                // reference, so the check cannot wait for the body.
                if !self.names.iter().any(|(had, _)| *had == name) {
                    return Err(Error::at("a named backreference names no group"));
                }
                Ok(Node::NamedBackreference(name))
            }
            '1'..='9' => {
                let number = self.digits().unwrap_or(0);
                // §22.2.1.1 — a backreference past the last group is an error in Unicode mode.
                // Annex B rereads it as a legacy octal escape; DR-0008 leaves that out, so this
                // refuses in both modes rather than reading it two ways.
                if number > self.groups {
                    return Err(Error::at("a backreference names no group"));
                }
                Ok(Node::Backreference(number))
            }
            _ => Ok(Node::Character(self.character_escape()?)),
        }
    }

    /// §22.2.1 `CharacterEscape` — the forms that stand for one code point.
    fn character_escape(&mut self) -> Result<u32, Error> {
        let Some(letter) = self.peek() else {
            return Err(Error::at("a regular expression ends after a backslash"));
        };
        self.at += 1;
        match letter {
            't' => Ok(0x09),
            'n' => Ok(0x0A),
            'v' => Ok(0x0B),
            'f' => Ok(0x0C),
            'r' => Ok(0x0D),
            // `\cX` — the control letter's code modulo 32, and *only* for an ASCII letter. `\c1`
            // is Annex B's reading and is refused here.
            'c' => {
                let Some(control) = self.peek().filter(char::is_ascii_alphabetic) else {
                    return Err(Error::at("a control escape needs a letter"));
                };
                self.at += 1;
                Ok(control as u32 % 32)
            }
            'x' => self.fixed_hex(2),
            'u' => self.unicode_escape(),
            // `\0` is NUL, and only when no digit follows: `\01` is a legacy octal escape, which
            // Annex B has and this does not.
            '0' => match self.peek().is_some_and(|next| next.is_ascii_digit()) {
                true => Err(Error::at("a legacy octal escape is not a character escape")),
                false => Ok(0),
            },
            '1'..='9' => Err(Error::at("a legacy octal escape is not a character escape")),
            // §22.2.1's `IdentityEscape`. In Unicode mode only a `SyntaxCharacter` or `/` may be
            // escaped this way, so `\a` is an error there and an `a` outside — one of the few
            // places the two modes disagree about whether a pattern is *valid* at all.
            _ => {
                if self.flags.unicode_mode() && !is_syntax_character(letter) && letter != '/' {
                    return Err(Error::at(
                        "this character may not be escaped in a Unicode pattern",
                    ));
                }
                Ok(letter as u32)
            }
        }
    }

    /// Exactly `count` hexadecimal digits, as one number.
    fn fixed_hex(&mut self, count: usize) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..count {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) else {
                return Err(Error::at("a hexadecimal escape is too short"));
            };
            value = value * 16 + digit;
            self.at += 1;
        }
        Ok(value)
    }

    /// §22.2.1 `RegExpUnicodeEscapeSequence` — `\uHHHH`, `\u{…}`, and the surrogate pair.
    fn unicode_escape(&mut self) -> Result<u32, Error> {
        if self.eat('{') {
            // `\u{…}` is Unicode-mode only, and the number must be a code point.
            if !self.flags.unicode_mode() {
                return Err(Error::at("a braced Unicode escape needs the u or v flag"));
            }
            let mut value: u32 = 0;
            let mut any = false;
            while let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) {
                value = value.saturating_mul(16).saturating_add(digit);
                self.at += 1;
                any = true;
            }
            if !any || !self.eat('}') {
                return Err(Error::at("a braced Unicode escape is malformed"));
            }
            if value > 0x0010_FFFF {
                return Err(Error::at("a Unicode escape is above the last code point"));
            }
            return Ok(value);
        }
        let high = self.fixed_hex(4)?;
        // A leading surrogate followed by `\uDC00`-style trailing one is *one* code point, but only
        // in Unicode mode: without it the two halves stay two code units and match separately.
        if self.flags.unicode_mode()
            && (0xD800..=0xDBFF).contains(&high)
            && self.peek() == Some('\\')
            && self.text.get(self.at + 1) == Some(&'u')
        {
            let mark = self.at;
            self.at += 2;
            match self.fixed_hex(4) {
                Ok(low) if (0xDC00..=0xDFFF).contains(&low) => {
                    return Ok(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
                // Not a trailing surrogate: put the cursor back and let the two be read apart.
                _ => self.at = mark,
            }
        }
        Ok(high)
    }
}

/// §22.2.1's `ClassSetReservedDoublePunctuator` — the twenty pairs `v` keeps for itself.
fn is_reserved_double(letter: char) -> bool {
    matches!(
        letter,
        '&' | '!'
            | '#'
            | '$'
            | '%'
            | '*'
            | '+'
            | ','
            | '.'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '^'
            | '`'
            | '~'
    )
}

/// §22.2.1's `ClassSetSyntaxCharacter` — what `v` reserves inside a class.
fn is_class_set_syntax(letter: char) -> bool {
    matches!(letter, '(' | ')' | '[' | '{' | '}' | '/' | '-' | '|')
}

/// The six class escapes, by their letter.
fn class_escape(letter: char) -> Option<ClassEscape> {
    match letter {
        'd' => Some(ClassEscape::Digit(false)),
        'D' => Some(ClassEscape::Digit(true)),
        's' => Some(ClassEscape::Space(false)),
        'S' => Some(ClassEscape::Space(true)),
        'w' => Some(ClassEscape::Word(false)),
        'W' => Some(ClassEscape::Word(true)),
        _ => None,
    }
}

/// §22.2.1's `RegExpIdentifierName` — whether a group may be called this.
///
/// An `IdentifierName` by §12.7's rules, so `(?<1a>…)` and `(?<a-b>…)` are Syntax Errors rather
/// than groups with surprising names. `$` and `_` are allowed anywhere in one, and the zero-width
/// joiners are allowed after the first character — §12.7.1's two exceptions, which exist for
/// scripts whose words need them.
fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    // `is_id_start` and `is_id_continue` *are* §12.7's `IdentifierStartChar` and
    // `IdentifierPartChar`, `$` and `_` and the zero-width joiners included — see their own docs,
    // one of which says outright not to add the joiners back. Repeating any of it here would be a
    // second answer that could disagree with the first.
    characters
        .next()
        .is_some_and(|first| is_id_start(first as u32))
        && characters.all(|next| is_id_continue(next as u32))
}

/// §22.2.1's `SyntaxCharacter` — the twelve a Unicode pattern may escape for their own sake.
fn is_syntax_character(letter: char) -> bool {
    matches!(
        letter,
        '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
    )
}

#[cfg(test)]
mod tests {
    use super::{Assertion, ClassEscape, ClassItem, Flags, GroupKind, Node, parse};

    fn plain(source: &str) -> Node {
        parse(source, Flags::default())
            .unwrap_or_else(|error| panic!("{source} should parse: {}", error.message))
            .node
    }

    fn unicode(source: &str) -> Result<Node, &'static str> {
        parse(
            source,
            Flags {
                unicode: true,
                ..Flags::default()
            },
        )
        .map(|pattern| pattern.node)
        .map_err(|error| error.message)
    }

    fn refused(source: &str) -> &'static str {
        parse(source, Flags::default())
            .err()
            .unwrap_or_else(|| panic!("{source} should be refused"))
            .message
    }

    #[test]
    fn an_empty_pattern_is_an_empty_alternative_and_not_an_error() {
        // `new RegExp("")` is a pattern matching everywhere, so `Empty` has to be a node rather
        // than the absence of one.
        assert_eq!(plain(""), Node::Empty);
        assert_eq!(
            plain("a|"),
            Node::Alternation(vec![Node::Character(97), Node::Empty])
        );
    }

    #[test]
    fn a_single_term_is_not_wrapped_in_a_sequence_or_an_alternation() {
        // Not cosmetic: the matcher walks this tree per position, and a needless layer per atom is
        // a needless continuation per atom.
        assert_eq!(plain("a"), Node::Character(97));
        assert_eq!(
            plain("ab"),
            Node::Sequence(vec![Node::Character(97), Node::Character(98)])
        );
    }

    #[test]
    fn alternation_binds_looser_than_sequence() {
        assert_eq!(
            plain("ab|c"),
            Node::Alternation(vec![
                Node::Sequence(vec![Node::Character(97), Node::Character(98)]),
                Node::Character(99),
            ])
        );
    }

    #[test]
    fn the_four_quantifiers_and_their_lazy_spellings() {
        let repeat = |source: &str| match plain(source) {
            Node::Repeat {
                min, max, greedy, ..
            } => (min, max, greedy),
            other => panic!("{source} should be a repeat, not {other:?}"),
        };
        assert_eq!(repeat("a*"), (0, None, true));
        assert_eq!(repeat("a+"), (1, None, true));
        assert_eq!(repeat("a?"), (0, Some(1), true));
        assert_eq!(repeat("a*?"), (0, None, false));
        assert_eq!(repeat("a{2}"), (2, Some(2), true));
        assert_eq!(repeat("a{2,}"), (2, None, true));
        assert_eq!(repeat("a{2,4}"), (2, Some(4), true));
        assert_eq!(repeat("a{2,4}?"), (2, Some(4), false));
    }

    #[test]
    fn braces_that_do_not_spell_a_quantifier_are_refused_rather_than_read_as_characters() {
        // Annex B reads `a{` and `a{,2}` as literal braces; DR-0008 leaves Annex B's syntactic
        // extensions out, so a `{` that begins nothing is the unmatched brace it looks like.
        assert_eq!(refused("a{"), "a regular expression has an unmatched {");
        assert_eq!(refused("a{,2}"), "a regular expression has an unmatched {");
        assert_eq!(refused("a{2"), "a regular expression has an unmatched {");
        // …and one that does spell a quantifier is consumed whole, braces and all.
        assert!(parse("a{2}", Flags::default()).is_ok());
    }

    #[test]
    fn a_backwards_quantifier_bound_is_an_error_and_not_a_pattern_that_never_matches() {
        assert_eq!(
            refused("a{2,1}"),
            "a quantifier's lower bound is above its upper bound"
        );
        // Equal bounds are fine, and so is a lower bound of zero.
        assert!(parse("a{2,2}", Flags::default()).is_ok());
        assert!(parse("a{0,0}", Flags::default()).is_ok());
    }

    #[test]
    fn a_quantifier_with_nothing_before_it_is_refused() {
        for source in ["*", "+", "?", "|*", "(*)"] {
            assert_eq!(refused(source), "this has nothing to repeat", "{source}");
        }
        // …and so is one on an assertion, which has nothing to repeat *of*.
        assert_eq!(refused("^*"), "this has nothing to repeat");
        assert_eq!(refused("\\b+"), "this has nothing to repeat");
        // A lookahead may be quantified — Annex B §B.1.2.1's `QuantifiableAssertion`, which is that
        // form and the negated one and nothing else. A lookbehind may never be.
        assert!(parse("(?=a)*", Flags::default()).is_ok());
        assert!(parse("(?!a)+", Flags::default()).is_ok());
        assert_eq!(refused("(?<=a)*"), "this has nothing to repeat");
        // …and the production is `[~UnicodeMode]`, so `u` and `v` take the exception away again.
        // Every quantifier form, because the rule is about the `Term` and not about the repetition:
        // it is the one place left where a flag decides the *grammar* rather than the matching.
        for source in [
            "(?=a)*",
            "(?!a)*",
            "(?=a)+",
            "(?=a)?",
            "(?=a){2}",
            "(?=a){2,}",
        ] {
            for unicode in [
                Flags {
                    unicode: true,
                    ..Flags::default()
                },
                Flags {
                    unicode_sets: true,
                    ..Flags::default()
                },
            ] {
                assert_eq!(
                    parse(source, unicode)
                        .err()
                        .unwrap_or_else(|| panic!("{source} should be refused under u and v"))
                        .message,
                    "this has nothing to repeat",
                    "{source}"
                );
            }
            // The same pattern without either flag is Annex B's, and is accepted.
            assert!(parse(source, Flags::default()).is_ok(), "{source}");
        }
        // A capturing or non-capturing group is quantifiable under `u` like anything else — the
        // rule is about assertions, and reading it as "a group" would refuse every `(a)*`.
        assert!(
            parse(
                "(a)*(?:b)+",
                Flags {
                    unicode: true,
                    ..Flags::default()
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn groups_are_numbered_by_their_opening_parenthesis() {
        let Node::Sequence(terms) = plain("(a)(?:b)(c)") else {
            panic!("should be a sequence");
        };
        let kinds: Vec<_> = terms
            .iter()
            .map(|term| match term {
                Node::Group { kind, .. } => kind.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                GroupKind::Capturing(1),
                GroupKind::NonCapturing,
                GroupKind::Capturing(2),
            ]
        );
        // A *named* group is capturing too and takes the next number, which is what makes both
        // spellings reach the same group.
        let pattern = parse("(a)(?<tail>b)", Flags::default()).expect("should parse");
        assert_eq!(pattern.groups, 2);
        assert_eq!(pattern.names, vec![("tail".to_string(), 2)]);
    }

    #[test]
    fn the_five_bracketed_forms_are_told_apart_by_what_follows_the_question_mark() {
        let kind = |source: &str| match plain(source) {
            Node::Group { kind, .. } => kind,
            other => panic!("{source}: {other:?}"),
        };
        assert_eq!(kind("(a)"), GroupKind::Capturing(1));
        assert_eq!(kind("(?:a)"), GroupKind::NonCapturing);
        assert_eq!(kind("(?=a)"), GroupKind::Lookahead(false));
        assert_eq!(kind("(?!a)"), GroupKind::Lookahead(true));
        assert_eq!(kind("(?<=a)"), GroupKind::Lookbehind(false));
        assert_eq!(kind("(?<!a)"), GroupKind::Lookbehind(true));
        assert_eq!(kind("(?<n>a)"), GroupKind::Named(1, "n".to_string()));
        // `(?<=` and `(?<!` must not be counted as named groups by the survey, or every lookbehind
        // would shift the numbering of every group after it.
        let pattern = parse("(?<=x)(a)", Flags::default()).expect("should parse");
        assert_eq!((pattern.groups, pattern.names.len()), (1, 0));
    }

    #[test]
    fn a_backreference_may_name_a_group_written_after_it() {
        // Which is the whole reason the group count is taken in a pass of its own.
        assert!(parse("\\1(a)", Flags::default()).is_ok());
        assert!(parse("\\k<n>(?<n>a)", Flags::default()).is_ok());
        assert_eq!(refused("\\1"), "a backreference names no group");
        assert_eq!(refused("(a)\\2"), "a backreference names no group");
        assert_eq!(refused("\\k<n>"), "a named backreference names no group");
        assert_eq!(
            refused("(?<n>a)\\k<m>"),
            "a named backreference names no group"
        );
    }

    #[test]
    fn two_groups_may_not_share_a_name() {
        assert_eq!(refused("(?<n>a)(?<n>b)"), "two groups have the same name");
        assert_eq!(refused("(?<>a)"), "a group name is empty");
        assert_eq!(refused("(?<n"), "a group name is not closed");
    }

    #[test]
    fn two_groups_may_share_a_name_when_no_match_could_fill_in_both() {
        // §22.2.1.1's rule is `MightBothParticipate` and not "the name is unused": two groups may
        // wear one name as long as some `Disjunction` has them in different `Alternative`s, because
        // then at most one of them can take part in any single match.
        assert!(parse("(?<x>a)|(?<x>b)", Flags::default()).is_ok());
        assert!(parse("(?:(?<x>a)|(?<x>b))", Flags::default()).is_ok());
        assert!(parse("^(?:(?<a>x)|(?<a>y)|z)$", Flags::default()).is_ok());
        // A group *around* one of them changes nothing: what matters is the alternative each is
        // written in, however deep it sits.
        assert!(parse("(?:(?<x>a))|(?<x>b)", Flags::default()).is_ok());
        assert!(parse("(?:(?:(?<x>a)))|(?:(?<x>b))", Flags::default()).is_ok());
        // A lookaround is a `Disjunction` too, so its alternatives count the same way.
        assert!(parse("(?=(?<x>a)|(?<x>b))", Flags::default()).is_ok());

        // Sequential is the plain case: both participate whenever the pattern matches.
        assert_eq!(
            refused("(?:(?<x>a))(?:(?<x>b))"),
            "two groups have the same name"
        );
        // **Different alternatives of *different* disjunctions is not the rule**, and this is the
        // pair a depth-only reading gets wrong: each group is the second alternative it is written
        // in, and the two disjunctions sit side by side, so `/(?:(?<x>a)|b)(?:c|(?<x>d))/` can fill
        // in both from `"ad"`.
        assert_eq!(
            refused("(?:(?<x>a)|b)(?:c|(?<x>d))"),
            "two groups have the same name"
        );
        assert_eq!(
            refused("(?:a|(?<x>b))(?:c|(?<x>d))"),
            "two groups have the same name"
        );
        // Nesting one inside the other's own disjunction is sequential in the same sense: the outer
        // group participates whenever the inner one does.
        assert_eq!(refused("(?<x>a|(?<x>b))"), "two groups have the same name");
        // A third name in between must not let a real conflict past — the check is per pair and
        // not "was the last one different".
        assert_eq!(
            refused("(?<x>a)(?<y>b)(?<x>c)"),
            "two groups have the same name"
        );
        // …and a name that is legal against one earlier group must still be checked against the
        // rest. Here `x` in the third alternative is fine against the first, and the *second* is in
        // the same alternative as it.
        assert_eq!(
            refused("(?<x>a)|(?:(?<x>b)(?<x>c))"),
            "two groups have the same name"
        );

        // A pattern that is broken *elsewhere* must be refused for that and not for this. An
        // unbalanced `)` is the real parse's complaint, and this pass must keep its base level so
        // the names either side of it still know which alternative they are in — popping past it
        // leaves both paths empty, which compares as "both could participate" and answers with the
        // wrong sentence about a pattern whose names are fine.
        assert_eq!(
            refused(")(?<x>a)|(?<x>b)"),
            "a regular expression has an unmatched )"
        );
        assert_eq!(
            refused("(?<x>a))|(?<x>b)"),
            "a regular expression has an unmatched )"
        );
    }

    #[test]
    fn a_class_reads_ranges_but_only_between_single_characters() {
        assert_eq!(
            plain("[a-c]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Range(97, 99)],
            }
        );
        assert_eq!(
            plain("[^a]"),
            Node::Class {
                negated: true,
                items: vec![ClassItem::Single(97)],
            }
        );
        // A `-` at the end is an atom, not an unfinished range.
        assert_eq!(
            plain("[a-]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Single(97), ClassItem::Single(45)],
            }
        );
        assert_eq!(refused("[z-a]"), "a character class range runs backwards");
        assert_eq!(
            refused("[\\d-z]"),
            "a character class range has a class escape as an end"
        );
        assert_eq!(refused("[a"), "a character class is not closed");
    }

    #[test]
    fn a_backslash_b_is_a_backspace_inside_a_class_and_a_boundary_outside_one() {
        // The one escape that means something different in the two places, and the kind of detail
        // a pattern parser is wrong about quietly.
        assert_eq!(plain("\\b"), Node::Assert(Assertion::WordBoundary));
        assert_eq!(
            plain("[\\b]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Single(0x08)],
            }
        );
    }

    #[test]
    fn the_six_class_escapes_read_the_same_in_a_class_and_out_of_one() {
        assert_eq!(plain("\\d"), Node::Escape(ClassEscape::Digit(false)));
        assert_eq!(plain("\\D"), Node::Escape(ClassEscape::Digit(true)));
        assert_eq!(plain("\\s"), Node::Escape(ClassEscape::Space(false)));
        assert_eq!(plain("\\W"), Node::Escape(ClassEscape::Word(true)));
        assert_eq!(
            plain("[\\w]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Escape(ClassEscape::Word(false))],
            }
        );
    }

    #[test]
    fn the_character_escapes_each_stand_for_their_own_code_point() {
        let one = |source: &str| match plain(source) {
            Node::Character(code) => code,
            other => panic!("{source}: {other:?}"),
        };
        assert_eq!(one("\\t"), 0x09);
        assert_eq!(one("\\n"), 0x0A);
        assert_eq!(one("\\v"), 0x0B);
        assert_eq!(one("\\f"), 0x0C);
        assert_eq!(one("\\r"), 0x0D);
        assert_eq!(one("\\0"), 0);
        assert_eq!(one("\\x41"), 0x41);
        assert_eq!(one("\\u0041"), 0x41);
        // `\cA` is the control character, which is the letter's code modulo 32.
        assert_eq!(one("\\cA"), 1);
        assert_eq!(one("\\cj"), 10);
        assert_eq!(refused("\\c1"), "a control escape needs a letter");
        assert_eq!(refused("\\x4"), "a hexadecimal escape is too short");
        // A legacy octal escape is Annex B's, and refusing it is DR-0008's rule rather than an
        // oversight — reading it as a character would be the *other* error.
        assert_eq!(
            refused("\\01"),
            "a legacy octal escape is not a character escape"
        );
    }

    #[test]
    fn the_unicode_flag_changes_which_patterns_are_valid_at_all() {
        // `\a` is an identity escape outside Unicode mode and an error inside it — one of the few
        // places the flags decide whether a pattern *parses*, not merely what it matches.
        assert_eq!(plain("\\a"), Node::Character(97));
        assert_eq!(
            unicode("\\a"),
            Err("this character may not be escaped in a Unicode pattern")
        );
        assert_eq!(unicode("\\$"), Ok(Node::Character(36)));
        assert_eq!(unicode("\\/"), Ok(Node::Character(47)));
        // `\u{…}` needs the flag, and a surrogate pair is one code point only with it.
        assert_eq!(
            refused("\\u{41}"),
            "a braced Unicode escape needs the u or v flag"
        );
        assert_eq!(unicode("\\u{1F600}"), Ok(Node::Character(0x1_F600)));
        assert_eq!(
            unicode("\\u{110000}"),
            Err("a Unicode escape is above the last code point")
        );
        assert_eq!(unicode("\\uD83D\\uDE00"), Ok(Node::Character(0x1_F600)));
        // Without the flag those stay two code units, and so two nodes.
        assert_eq!(
            plain("\\uD83D\\uDE00"),
            Node::Sequence(vec![Node::Character(0xD83D), Node::Character(0xDE00)])
        );
        // A leading surrogate not followed by a trailing one is left as itself.
        assert_eq!(
            unicode("\\uD83Dx"),
            Ok(Node::Sequence(vec![
                Node::Character(0xD83D),
                Node::Character(120),
            ]))
        );
    }

    #[test]
    fn a_modifier_group_is_refused_as_unbuilt_and_a_malformed_one_as_wrong() {
        // §22.2.1's *modifiers* are Stage 3 and not ES2023, so `(?i:…)` is a gap rather than a
        // forbidden pattern. A script sees the same SyntaxError for both — a syntax an engine does
        // not implement is a syntax it does not accept — but the engine has to know which it said,
        // because the proposal's own tests are largely negative ones asserting that *particular*
        // malformed modifier groups are rejected. Calling this a syntax error passes all of those
        // while failing every pattern the proposal actually adds.
        for source in ["(?i:a)", "(?-i:a)", "(?im-s:a)"] {
            let error = parse(source, Flags::default())
                .err()
                .unwrap_or_else(|| panic!("{source} should be refused")); // a refusal is the test
            assert!(error.unimplemented, "{source}");
            assert_eq!(error.message, "the RegExp modifiers proposal");
        }
        // …and anything that only *looks* like one is an ordinary syntax error, which is what the
        // split has to get right in the other direction. None of these reaches a `:` through the
        // modifier letters alone.
        for source in ["(?%a)", "(?i", "(?ix:a)", "(?:a)(?", "(?im"] {
            let error = parse(source, Flags::default())
                .err()
                .unwrap_or_else(|| panic!("{source} should be refused")); // same
            assert!(!error.unimplemented, "{source} — {}", error.message);
        }
    }

    #[test]
    fn unbalanced_brackets_are_refused_from_both_sides() {
        assert_eq!(refused("(a"), "a regular expression has an unclosed (");
        assert_eq!(refused("a)"), "a regular expression has an unmatched )");
        assert_eq!(refused("a]"), "a regular expression has an unmatched ]");
        assert_eq!(refused("(?%a)"), "this is not a kind of group");
        assert_eq!(refused("\\"), "a regular expression ends after a backslash");
    }

    #[test]
    fn a_range_whose_ends_are_the_same_character_is_a_range_of_one() {
        // `[a-a]` is well formed: §22.2.1.1 refuses a range only when the low end is *above* the
        // high one, and equal ends are the boundary case that says which comparison it is.
        assert_eq!(
            plain("[a-a]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Range(97, 97)],
            }
        );
        assert_eq!(refused("[b-a]"), "a character class range runs backwards");
    }

    #[test]
    fn only_backslash_b_changes_meaning_inside_a_class() {
        // The neighbouring escapes must not be dragged along with it: `\t` is a tab in both places
        // and it is the escape most likely to be caught by a guard written one character too wide.
        assert_eq!(
            plain("[\\t]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Single(0x09)],
            }
        );
        assert_eq!(
            plain("[\\n]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Single(0x0A)],
            }
        );
    }

    #[test]
    fn a_hyphen_may_be_escaped_inside_a_class_even_in_unicode_mode() {
        // §22.2.1's `ClassEscape` lists `-` outright, and it is the only reason the case exists:
        // outside a class `\-` is an identity escape, which Unicode mode refuses. So this is
        // testable *only* under the flag — without it both readings agree.
        assert_eq!(
            plain("[\\-]"),
            Node::Class {
                negated: false,
                items: vec![ClassItem::Single(45)],
            }
        );
        assert_eq!(
            unicode("[\\-]"),
            Ok(Node::Class {
                negated: false,
                items: vec![ClassItem::Single(45)],
            })
        );
        assert_eq!(
            unicode("\\-"),
            Err("this character may not be escaped in a Unicode pattern")
        );
        // …and the escape consumes exactly the hyphen, so what follows is still read.
        assert_eq!(
            unicode("[\\-a]"),
            Ok(Node::Class {
                negated: false,
                items: vec![ClassItem::Single(45), ClassItem::Single(97)],
            })
        );
    }

    #[test]
    fn a_braced_unicode_escape_needs_digits_and_stops_at_the_last_code_point() {
        assert_eq!(
            unicode("\\u{}"),
            Err("a braced Unicode escape is malformed")
        );
        assert_eq!(
            unicode("\\u{41"),
            Err("a braced Unicode escape is malformed")
        );
        // The boundary is inclusive: `10FFFF` is the last code point and is allowed, `110000` is
        // the first that is not.
        assert_eq!(unicode("\\u{10FFFF}"), Ok(Node::Character(0x0010_FFFF)));
        assert_eq!(
            unicode("\\u{110000}"),
            Err("a Unicode escape is above the last code point")
        );
    }

    #[test]
    fn each_class_escape_is_told_from_its_negated_spelling() {
        // Six escapes and three letters: getting the case wrong swaps a set for its complement,
        // which every pattern using it then matches backwards.
        assert_eq!(plain("\\d"), Node::Escape(ClassEscape::Digit(false)));
        assert_eq!(plain("\\D"), Node::Escape(ClassEscape::Digit(true)));
        assert_eq!(plain("\\s"), Node::Escape(ClassEscape::Space(false)));
        assert_eq!(plain("\\S"), Node::Escape(ClassEscape::Space(true)));
        assert_eq!(plain("\\w"), Node::Escape(ClassEscape::Word(false)));
        assert_eq!(plain("\\W"), Node::Escape(ClassEscape::Word(true)));
    }

    #[test]
    fn a_named_group_s_body_begins_after_the_closing_angle_bracket() {
        // The name and the body are read by two different passes, and if the second starts one
        // character out the name's own letters become part of the pattern — which still parses,
        // still numbers the group right, and matches something else entirely.
        assert_eq!(
            plain("(?<n>a)"),
            Node::Group {
                kind: GroupKind::Named(1, "n".to_string()),
                body: Box::new(Node::Character(97)),
            }
        );
        assert_eq!(
            plain("(?<long>ab)"),
            Node::Group {
                kind: GroupKind::Named(1, "long".to_string()),
                body: Box::new(Node::Sequence(vec![
                    Node::Character(97),
                    Node::Character(98)
                ])),
            }
        );
        // An unterminated name is reported as that rather than as whatever the body parse makes of
        // the rest.
        assert_eq!(refused("(?<n"), "a group name is not closed");
    }

    #[test]
    fn the_v_flag_reserves_inside_a_class_what_the_u_flag_allows() {
        // §22.2.1's `ClassSetSyntaxCharacter` and `ClassSetReservedDoublePunctuator`. `v` is the
        // one flag that makes *fewer* patterns valid rather than more, which is why the two cannot
        // be set together — and it is the difference the suite calls a breaking change.
        let sets = |source: &str| {
            parse(
                source,
                Flags {
                    unicode_sets: true,
                    ..Flags::default()
                },
            )
            .map(|pattern| pattern.node)
            .map_err(|error| error.message)
        };
        assert!(unicode("[(]").is_ok());
        assert_eq!(
            sets("[(]"),
            Err("this character must be escaped inside a class in a v pattern")
        );
        assert!(unicode("[&&]").is_ok());
        assert_eq!(
            sets("[&&]"),
            Err("this punctuator is doubled, which a v pattern reserves inside a class")
        );
        // One of a reserved pair on its own is an ordinary member, and so is a doubled character
        // that is not one of the twenty.
        assert!(sets("[&x]").is_ok());
        assert!(sets("[aa]").is_ok());
        // …and escaping it is how a `v` pattern says it meant the character.
        assert!(sets(r"[\(]").is_ok());
    }

    #[test]
    fn a_group_name_has_to_be_an_identifier() {
        // §22.2.1's `RegExpIdentifierName`. Without this every run of characters up to the `>` is
        // a name, and `(?<1a>x)` becomes a group nothing can refer to.
        assert!(parse("(?<a>x)", Flags::default()).is_ok());
        assert!(parse("(?<$a>x)", Flags::default()).is_ok());
        assert!(parse("(?<_a>x)", Flags::default()).is_ok());
        assert!(parse("(?<a1>x)", Flags::default()).is_ok());
        for bad in ["(?<1a>x)", "(?<a-b>x)", "(?<a b>x)", "(?<a.b>x)"] {
            assert_eq!(
                parse(bad, Flags::default()).err().map(|e| e.message),
                Some("a group name is not an identifier"),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_dot_and_an_anchor_are_nodes_of_their_own() {
        assert_eq!(plain("."), Node::Any);
        assert_eq!(plain("^"), Node::Assert(Assertion::Start));
        assert_eq!(plain("$"), Node::Assert(Assertion::End));
        assert_eq!(plain("\\B"), Node::Assert(Assertion::NotWordBoundary));
    }

    #[test]
    fn a_group_or_class_inside_a_pattern_does_not_confuse_the_counting_pass() {
        // The survey skips escapes and class insides, so neither `\(` nor `[(]` is a group.
        let pattern = parse("\\((a)[(]", Flags::default()).expect("should parse");
        assert_eq!(pattern.groups, 1);
        // …and a `]` that was escaped does not close the class early.
        let pattern = parse("[\\]](a)", Flags::default()).expect("should parse");
        assert_eq!(pattern.groups, 1);
    }

    #[test]
    fn a_property_escape_is_read_as_a_set_and_only_in_unicode_mode() {
        // §22.2.1 `CharacterClassEscape :: p{ … }` — the braces are part of the syntax and the
        // name inside is looked up when the pattern is *compiled*, so every way of writing it
        // wrongly is a Syntax Error before anything runs.
        let Ok(Node::Property(upper)) = unicode(r"\p{Lu}") else {
            panic!("a lone-name property escape should parse"); // the shape is the test
        };
        assert!(upper.contains('A' as u32) && !upper.contains('a' as u32));
        // `\P` is the same set the other way round, and it is the *escape* that negates rather
        // than anything about the name.
        let Ok(Node::Property(not_upper)) = unicode(r"\P{Lu}") else {
            panic!("a negated property escape should parse"); // same
        };
        assert!(!not_upper.contains('A' as u32) && not_upper.contains('a' as u32));
        assert_eq!(upper.negate(), not_upper);
        // Inside a class it is one item among others, and it stands for a set — so it may not be
        // an end of a range, which is what makes it a `ClassItem` of its own.
        let Ok(Node::Class { items, .. }) = unicode(r"[\p{Nd}a]") else {
            panic!("a class with a property should parse"); // same
        };
        assert!(matches!(items.first(), Some(ClassItem::Property(_))));
        // Without `u` there is no property escape at all: `\p` is an identity escape and matches
        // a `p`, which is why the mode is checked before the braces rather than after. Written
        // without braces because a `{` that spells no quantifier is its own Syntax Error here —
        // DR-0008 leaves out the Annex B reading that makes it a literal.
        assert_eq!(
            plain(r"\pa"),
            Node::Sequence(vec![
                Node::Character(u32::from(b'p')),
                Node::Character(u32::from(b'a')),
            ])
        );
    }

    #[test]
    fn a_property_escape_written_wrongly_says_which_way() {
        // Each of these stops at a different point of the read, which is what the four messages
        // are for — and each is an early error, so none of them is a pattern that merely fails to
        // match.
        assert_eq!(
            unicode(r"\pLu"),
            Err("a property escape needs a braced name")
        );
        assert_eq!(unicode(r"\p"), Err("a property escape needs a braced name"));
        assert_eq!(
            unicode(r"\p{Lu"),
            Err("a property escape's name is not closed")
        );
        assert_eq!(
            unicode(r"\p{"),
            Err("a property escape's name is not closed")
        );
        assert_eq!(unicode(r"\p{Nope}"), Err("this is not a Unicode property"));
        assert_eq!(unicode(r"\p{}"), Err("this is not a Unicode property"));
        // A **property of strings** is a thing praxis has not built rather than a name the
        // specification rejects, and the two are refused differently on purpose — see
        // `regexp::Error::unimplemented`.
        // §22.2.1 — a property of strings is legal *only* in a `v` pattern, positive and outside a
        // negated class. The other three positions are the specification refusing, so they are a
        // real answer about the text rather than a gap praxis has yet to fill.
        assert_eq!(
            unicode(r"\p{RGI_Emoji}"),
            Err("a property of strings needs the v flag")
        );
    }
}
