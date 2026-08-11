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
pub fn parse(source: &[u16], flags: Flags) -> Result<Pattern, Error> {
    // §22.2.1 reads a pattern as **code units** unless `[+UnicodeMode]`, and as code points when it
    // is set. That is not a detail of the escapes: a surrogate pair written *literally* in the
    // source is two `SourceCharacter`s without the flag and one with it, so the same text is two
    // different patterns depending on which was asked for.
    //
    // This read `source.chars()` — always code points — which made a literal astral character one
    // atom of `U+1F600`. The matcher then compared that atom against single code units, so it
    // could never match: `new RegExp("<emoji>").test("<emoji>")` was **false**, and a `replace`
    // with one did nothing at all.
    //
    // `u32` and not `char` because a lone surrogate is neither a `char` nor an error: a pattern
    // may name an unpaired half, and taking the source as a `&str` had already replaced one with
    // `U+FFFD` before parsing began.
    let text: Vec<u32> = match flags.unicode_mode() {
        true => code_points(source),
        false => source.iter().map(|unit| u32::from(*unit)).collect(),
    };
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

/// A `GroupSpecifier`'s name, from the units it was written as.
///
/// §22.2.1's `RegExpIdentifierStart` and `RegExpIdentifierPart` admit
/// `UnicodeLeadSurrogate UnicodeTrailSurrogate` **as one identifier character**, and they do so
/// without asking for `u` — so a group named with an astral letter is written as two units in a
/// pattern with no flag and is still one character of the name. Pairing here rather than dropping
/// what is not a `char`: dropping made such a name come out *empty*, which is a different refusal
/// from the one the name deserves and reads as a different bug.
fn identifier_of(units: &[u32]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < units.len() {
        let first = units[at];
        // Under `u` or `v` the elements are already code points, so an astral letter arrives whole;
        // without them it arrives as the two units it was written as, and is paired here. Both
        // spellings have to work: routing everything through `u16` first dropped the code points
        // the flags had already made, which is four tests' worth of a name coming out empty.
        if (0xD800..=0xDBFF).contains(&first)
            && let Some(second) = units.get(at + 1).copied()
            && (0xDC00..=0xDFFF).contains(&second)
        {
            let paired = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
            out.extend(char::from_u32(paired));
            at += 2;
            continue;
        }
        out.extend(char::from_u32(first));
        at += 1;
    }
    out
}

/// The pattern's code points, for `[+UnicodeMode]` — §11.1.5 `StringToCodePoints`.
///
/// An unpaired surrogate is a code point of its own and is kept as one, which is what lets a
/// pattern name the half a program wrote rather than a replacement character.
fn code_points(source: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(source.len());
    let mut at = 0;
    while at < source.len() {
        let first = u32::from(source[at]);
        let second = source.get(at + 1).copied().map(u32::from);
        if (0xD800..=0xDBFF).contains(&first)
            && let Some(second) = second
            && (0xDC00..=0xDFFF).contains(&second)
        {
            out.push(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00));
            at += 2;
            continue;
        }
        out.push(first);
        at += 1;
    }
    out
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
fn survey(text: &[u32]) -> Result<(u32, Vec<(String, u32)>), Error> {
    let mut groups = 0;
    let mut names: Vec<(String, u32)> = Vec::new();
    let mut paths: Vec<Path> = Vec::new();
    // The pattern is itself a `Disjunction`, so there is always a level to be in an alternative of.
    let mut path: Path = vec![(0, 0)];
    let mut disjunctions = 0;
    let mut at = 0;
    let mut in_class = false;
    while at < text.len() {
        match char::from_u32(text[at]) {
            Some('\\') => at += 2,
            Some('[') if !in_class => {
                in_class = true;
                at += 1;
            }
            Some(']') if in_class => {
                in_class = false;
                at += 1;
            }
            // Every `(` opens a `Disjunction` — a capturing group, `(?:`, and all four lookarounds
            // alike — so every one of them is a level. `|` inside it belongs to that level and not
            // to the one outside, which is the whole reason the alternatives are counted on a stack
            // rather than as one number.
            Some('|') if !in_class => {
                if let Some(level) = path.last_mut() {
                    level.1 += 1;
                }
                at += 1;
            }
            Some(')') if !in_class => {
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
            Some('(') if !in_class => {
                at += 1;
                // §22.2.1.1 reads a `GroupSpecifier` as part of `( GroupSpecifier Disjunction )`,
                // so the name sits in the alternative containing the *whole group* and not inside
                // the group's own disjunction. Taken before the level is pushed, for that reason.
                let outer = path.clone();
                disjunctions += 1;
                path.push((disjunctions, 0));
                if text.get(at) != Some(&u32::from(b'?')) {
                    groups += 1;
                    continue;
                }
                at += 1;
                if text.get(at) != Some(&u32::from(b'<')) {
                    continue;
                }
                // `(?<=` and `(?<!` are lookbehind and name nothing.
                if matches!(
                    text.get(at + 1).copied().and_then(char::from_u32),
                    Some('=' | '!')
                ) {
                    continue;
                }
                at += 1;
                let Some(end) = text[at..]
                    .iter()
                    .position(|c| *c == u32::from(b'>'))
                    .map(|off| at + off)
                else {
                    return Err(Error::at("a group name is not closed"));
                };
                let name: String = identifier_of(&text[at..end]);
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
    text: &'a [u32],
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
    ///
    /// `None` at the end **and** for a lone surrogate, which is not a `char` — and which is never a
    /// syntax character either, so every test written against this correctly declines one. The
    /// places that need the value rather than the syntax read [`Reader::unit`] instead.
    fn peek(&self) -> Option<char> {
        self.unit(self.at).and_then(char::from_u32)
    }

    /// The code unit or code point at `at`, whatever it is.
    fn unit(&self, at: usize) -> Option<u32> {
        self.text.get(at).copied()
    }

    /// The character at `at` for the purpose of matching *syntax*, which a surrogate never is.
    fn ch(&self, at: usize) -> Option<char> {
        self.unit(at).and_then(char::from_u32)
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
            _ => Node::alternation(branches),
        })
    }

    /// §22.2.1 `Alternative` — terms until a `|` or a `)` or the end.
    fn alternative(&mut self) -> Result<Node, Error> {
        let mut terms = Vec::new();
        // The loop ends at the *end of the pattern*, which is where there is no unit — not where
        // there is no `char`. A lone surrogate has no `char` and is a term all the same.
        while self.unit(self.at).is_some() {
            if matches!(self.peek(), Some('|' | ')')) {
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
        // The **unit** decides whether there is a term at all, and the *character* decides which
        // one. A lone surrogate is a `PatternCharacter` like any other and is not a `char`, so
        // reading only the character would end the pattern here — which is what happened for one
        // commit, and turned every literal astral character into "a regular expression ends
        // part-way through a term".
        let Some(unit) = self.unit(self.at) else {
            return Err(Error::at(
                "a regular expression ends part-way through a term",
            ));
        };
        let Some(next) = char::from_u32(unit) else {
            // A surrogate is none of the syntax below, so it is the last arm's answer directly.
            self.at += 1;
            return Ok(Node::Character(unit));
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
            // Annex B §B.1.2's `ExtendedPatternCharacter` is `SourceCharacter` but not one of
            // `^ $ \ . * + ? ( ) [ |` — which is `PatternCharacter`'s list with `]`, `{` and `}`
            // taken *off* it. So a lone bracket or brace is an ordinary character in a pattern read
            // without `u` or `v`, and stays a Syntax Error under either flag, the production being
            // `[~UnicodeMode]`.
            ']' | '}' if !self.flags.unicode_mode() => {
                self.at += 1;
                Ok(Node::Character(next as u32))
            }
            ']' => Err(Error::at("a regular expression has an unmatched ]")),
            '}' => Err(Error::at("a regular expression has an unmatched }")),
            // A `{` where an atom belongs is the one place two `ExtendedAtom` productions compete,
            // and their order in the grammar is the whole rule: `InvalidBracedQuantifier` is listed
            // first and §B.1.2.1 makes it a Syntax Error outright, so `/{1}/` and `/a{1}{2}/` are
            // refused while `/a{/` and `/x{o}x/` are literal braces. Reading the brace as a
            // character without asking would accept `/{1}/`, which is a quantifier with nothing in
            // front of it wearing a bracket.
            //
            // `braced` is exactly `InvalidBracedQuantifier`'s three forms and puts the cursor back
            // when it is none of them, so there is nothing to undo on the character path.
            '{' if !self.flags.unicode_mode() => match self.braced()?.is_some() {
                true => Err(Error::at("this has nothing to repeat")),
                false => {
                    self.at += 1;
                    Ok(Node::Character(next as u32))
                }
            },
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
        while let Some(next) = self.ch(at) {
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
                Some('<') if self.ch(self.at + 1) == Some('=') => {
                    self.at += 2;
                    GroupKind::Lookbehind(false)
                }
                Some('<') if self.ch(self.at + 1) == Some('!') => {
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
        // Walked by *unit*: a name may hold a surrogate — §22.2.1's `RegExpIdentifierStart` admits
        // a pair as one identifier character, without asking for `u` — and reading only characters
        // ended the walk at the first half, reporting the name as unclosed.
        while self.unit(self.at).is_some() {
            if self.peek() == Some('>') {
                let name: String = identifier_of(&self.text[start..self.at]);
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
        // §22.2.1's operations resolved over the string operands, **once**, so the matcher can ask
        // whether this class consumes sequences without walking the tree at every attempt.
        //
        // No `if !set.negated` in front of it. A negated class that could match a string has
        // already been refused by `class_set`, and the one negated form that survives — `[^\q{a}]`,
        // whose alternative is a single code point — resolves to nothing here anyway, because a
        // one-code-point alternative is a member of the character set rather than a sequence. So
        // the guard would be a branch no input could flip.
        let strings = resolved_strings(set.operation, &set.items);
        match set.operation {
            ClassOperation::Union => Ok(Node::Class {
                negated: set.negated,
                items: set.items,
                strings,
            }),
            // The negation belongs to the class and the operation to what is inside it, which is
            // the order `[^\d&&[0-4]]` is written in and the order it has to be evaluated in.
            _ => Ok(Node::Class {
                negated: set.negated,
                items: vec![ClassItem::Nested(ClassSet {
                    negated: false,
                    ..set
                })],
                strings,
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
            // Only a *character* can close the class, so `peek` answering `None` for a lone
            // surrogate is right here — a surrogate is an operand and never a `]`. The end of the
            // pattern is deliberately **not** tested: `class_atom` below reports an unterminated
            // class, and a second test of the same thing was a branch no input could distinguish.
            {
                if self.peek() == Some(']') {
                    self.at += 1;
                    // `[a--]` has an operator and one operand short of a use for it.
                    if operation != ClassOperation::Union && items.len() < 2 {
                        return Err(Error::at("a set operation needs an operand on both sides"));
                    }
                    self.negated_classes -= usize::from(negated);
                    // §22.2.1 — `[^…]` is a Syntax Error when its contents `MayContainStrings`.
                    // A *syntactic* question and not "is the resolved set non-empty": the rule is
                    // per-operation, so a difference of two identical string operands is refused
                    // although it resolves to nothing, and an intersection with a code-point
                    // operand is accepted although its first operand could.
                    if negated && may_contain_strings(operation, &items) {
                        return Err(Error::at(
                            "a negated class may not contain anything that matches a string",
                        ));
                    }
                    return Ok(ClassSet {
                        negated,
                        operation,
                        items,
                    });
                }
            }
            // §22.2.1's `ClassSetReservedDoublePunctuator` — `v` reserves a *doubled* punctuator
            // inside a class for set notation it does not have yet, so `/[&&]/v` is a Syntax Error
            // where `/[&&]/u` is a class holding two ampersands. Checked before the atom is read,
            // because either character alone is fine and it is the pair that is reserved.
            if self.flags.unicode_sets
                && let Some(here) = self.ch(self.at)
                && self.ch(self.at + 1) == Some(here)
                && is_reserved_double(here)
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
            if self.peek() == Some('-') && self.ch(self.at + 1) != Some(']') {
                self.at += 1;
                let second = self.class_operand()?;
                let ends = match (&first, &second) {
                    (ClassItem::Single(low), ClassItem::Single(high)) => Some((*low, *high)),
                    _ => None,
                };
                match ends {
                    Some((low, high)) => {
                        if low > high {
                            return Err(Error::at("a character class range runs backwards"));
                        }
                        items.push(ClassItem::Range(low, high));
                    }
                    // §22.2.1.1 — a class escape stands for a set, and a set has no place at the
                    // end of a range.
                    None if self.flags.unicode_mode() => {
                        return Err(Error::at(
                            "a character class range has a class escape as an end",
                        ));
                    }
                    // §B.1.4.1.1's `CharacterRangeOrUnion`, and the name is the rule: without `u`
                    // or `v` the same text is not a malformed range but a **union** of the three
                    // things written, hyphen included. So `[\d-x]` holds the digits, a hyphen and
                    // an `x` — which is why this is decided by the mode rather than by the shape.
                    // Both readings are derivations of the same characters.
                    None => {
                        items.push(first);
                        items.push(ClassItem::Single(u32::from(b'-')));
                        items.push(second);
                    }
                }
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
        match (self.ch(self.at), self.ch(self.at + 1)) {
            (Some('&'), Some('&')) if self.ch(self.at + 2) != Some('&') => {
                self.at += 2;
                Some(ClassOperation::Intersection)
            }
            (Some('-'), Some('-')) => {
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
        if self.flags.unicode_sets
            && self.ch(self.at) == Some('\\')
            && self.ch(self.at + 1) == Some('q')
            && self.ch(self.at + 2) == Some('{')
        {
            return self.class_strings();
        }
        self.class_atom()
    }

    /// §22.2.1's `ClassStringDisjunction` — `\q{abc|def}`, read as its alternatives.
    ///
    /// Every alternative is a run of `ClassSetCharacter`, which is a code point and never a set:
    /// `\d` has no derivation in here, and neither does a range. So this reads characters and
    /// escapes and nothing else, and the two delimiters are the only unescaped punctuation.
    ///
    /// `\q{}` is one **empty** alternative rather than none, which matters twice: it is a legal
    /// operand matching the empty string, and §22.2.1 makes it `MayContainStrings` — so
    /// `[^\q{}]` is a Syntax Error where `[^\q{a}]` is an ordinary class.
    fn class_strings(&mut self) -> Result<ClassItem, Error> {
        // Past the backslash, the `q` and the `{`, all three of which the caller has already seen.
        self.at += 3;
        let mut alternatives = Vec::new();
        let mut current: Vec<u32> = Vec::new();
        loop {
            match self.peek() {
                None => return Err(Error::at("a class of strings is not closed")),
                Some('}') => {
                    self.at += 1;
                    alternatives.push(current);
                    return Ok(ClassItem::Strings(alternatives));
                }
                Some('|') => {
                    self.at += 1;
                    alternatives.push(std::mem::take(&mut current));
                }
                Some('\\') => {
                    self.at += 1;
                    // `\b` is a backspace in here as it is anywhere else inside a class, and it
                    // is the one escape whose meaning changes at that boundary.
                    if self.peek() == Some('b') {
                        self.at += 1;
                        current.push(0x08);
                        continue;
                    }
                    current.push(self.character_escape()?);
                }
                // The same reservation the rest of a `v` class is under: a syntax character has to
                // be written escaped, so `\q{(}` is refused.
                Some(next) if is_class_set_syntax(next) => {
                    return Err(Error::at(
                        "this character must be escaped inside a class in a v pattern",
                    ));
                }
                Some(next) => {
                    self.at += 1;
                    current.push(next as u32);
                }
            }
        }
    }

    /// One entry inside `[…]` — a character, or an escape that may stand for a set.
    fn class_atom(&mut self) -> Result<ClassItem, Error> {
        // The unit says whether there is an atom; the character says which one. A lone surrogate is
        // a `ClassAtom` and has no `char`, so asking only for the character ended the class here.
        let Some(unit) = self.unit(self.at) else {
            return Err(Error::at("a character class is not closed"));
        };
        let Some(next) = char::from_u32(unit) else {
            self.at += 1;
            return Ok(ClassItem::Single(unit));
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
        // §B.1.2 gives a class its own `ClassEscape :: c ClassControlLetter`, which accepts a
        // decimal digit or `_` where `ControlLetter` wants an ASCII letter — and its own
        // `ClassAtomNoDash :: \ [lookahead = c]` for one that still does not match. Answered here
        // rather than passed down as a flag: this is the only caller for which the answer is
        // anything but "outside a class", and a parameter saying so at the other two would be a
        // value no pattern could tell the setting of.
        if letter == 'c' {
            self.at += 1;
            return Ok(ClassItem::Single(self.control_escape(true)?));
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
        // A property name is ASCII, so a surrogate cannot be part of one and is dropped here —
        // whatever is left then fails the property lookup, which is the right refusal.
        let spelled: String = self.text[start..self.at]
            .iter()
            .filter_map(|unit| char::from_u32(*unit))
            .collect();
        self.at += 1;
        if crate::unicode_property::OF_STRINGS.contains(&spelled.as_str()) {
            // §22.2.1's early errors, and all three are the specification refusing rather than
            // ViperJS not having built something — so they are `Error::at`, which is a `BadPattern`
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
            // §22.2.1's `AtomEscape :: [+N] k GroupName`. `N` is set when the pattern contains a
            // `GroupSpecifier` anywhere, which is a question the survey has already answered — and
            // with no named group at all the production is not in the grammar, so §B.1.2's
            // `SourceCharacterIdentityEscape` takes the `k` instead and `/\k<a>/` matches the four
            // characters `k<a>`.
            //
            // The two readings can never compete over one pattern, because that production's `[+N]`
            // form excludes `k` for exactly as long as this one is available.
            'k' if !self.flags.unicode_mode() && self.names.is_empty() => {
                self.at += 1;
                Ok(Node::Character(u32::from(b'k')))
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
                let start = self.at;
                let number = self.digits().unwrap_or(0);
                if number <= self.groups {
                    return Ok(Node::Backreference(number));
                }
                // §22.2.1.1 — a backreference past the last group is an error in Unicode mode.
                // §B.1.2 does not make it one: it conditions `AtomEscape :: DecimalEscape` on the
                // number being within range, so out of range the production simply does not match
                // and `CharacterEscape` reads the same text as a legacy octal or an identity
                // escape. Hence `/\1/` is a `\x01` and `/\8/` is an `8`, while `/(.)\1/` is still a
                // backreference — the group count decides, and it is why this is checked before the
                // digits are given back.
                if self.flags.unicode_mode() {
                    return Err(Error::at("a backreference names no group"));
                }
                self.at = start;
                // The two digit productions of `CharacterEscape` rather than the whole of it: what
                // stands here is a digit, and a digit reaches none of the others.
                match letter {
                    // `8` and `9` appear in none of `LegacyOctalEscapeSequence`'s four productions,
                    // so they fall to the identity escape and stand for themselves.
                    '8' | '9' => {
                        self.at += 1;
                        Ok(Node::Character(letter as u32))
                    }
                    _ => Ok(Node::Character(self.legacy_octal())),
                }
            }
            _ => Ok(Node::Character(self.character_escape()?)),
        }
    }

    /// §22.2.1 `CharacterEscape` — the forms that stand for one code point.
    ///
    /// The one escape a class reads differently never arrives here. `class_atom` takes `\c` first,
    /// and the other caller inside a class — the alternatives of a `\q{…}` — needs `v`, under which
    /// §B.1.2's wider reading does not exist to choose.
    fn character_escape(&mut self) -> Result<u32, Error> {
        let Some(letter) = self.peek() else {
            return Err(Error::at("a regular expression ends after a backslash"));
        };
        self.at += 1;
        let after = self.at;
        match letter {
            't' => Ok(0x09),
            'n' => Ok(0x0A),
            'v' => Ok(0x0B),
            'f' => Ok(0x0C),
            'r' => Ok(0x0D),
            'c' => self.control_escape(false),
            // §22.2.1's `HexEscapeSequence` and `RegExpUnicodeEscapeSequence`. Neither *failing* is
            // an error under Annex B — see `short_escape`, which is where a `\x` with too few
            // digits stops being one.
            'x' => match self.fixed_hex(2) {
                Ok(value) => Ok(value),
                Err(error) => self.short_escape(letter, after, error),
            },
            'u' => match self.unicode_escape() {
                Ok(value) => Ok(value),
                Err(error) => self.short_escape(letter, after, error),
            },
            // §B.1.2's `LegacyOctalEscapeSequence`, `[~UnicodeMode]`, which is what a digit means
            // here once `DecimalEscape` has had its turn: `\101` is an `A`. The arm stops at seven
            // because `8` and `9` are not octal digits in any of its four productions — they reach
            // the identity escape below and stand for themselves.
            '0'..='7' if !self.flags.unicode_mode() => {
                // The digit is given back rather than passed along: reading all of them in one
                // place is what keeps the three-versus-two bound a single rule.
                self.at = after - 1;
                Ok(self.legacy_octal())
            }
            // `\0` is NUL, and §22.2.1 gives it a `[lookahead ∉ DecimalDigit]` — so a digit after it
            // takes the production away, and in Unicode mode nothing replaces it.
            '0' => match self.peek().is_some_and(|next| next.is_ascii_digit()) {
                true => Err(Error::at("a legacy octal escape is not a character escape")),
                false => Ok(0),
            },
            '1'..='7' => Err(Error::at("a legacy octal escape is not a character escape")),
            // §22.2.1's `IdentityEscape`. In Unicode mode only a `SyntaxCharacter` or `/` may be
            // escaped this way, so `\a` is an error there and an `a` outside — one of the few
            // places the two modes disagree about whether a pattern is *valid* at all.
            //
            // Outside it the production is §B.1.2's `SourceCharacterIdentityEscape`, which is every
            // character but `c` — and but `k` as well once the pattern has a group name, that being
            // the one spelling a named backreference has already claimed.
            _ => {
                if self.flags.unicode_mode() && !is_syntax_character(letter) && letter != '/' {
                    return Err(Error::at(
                        "this character may not be escaped in a Unicode pattern",
                    ));
                }
                if letter == 'k' && !self.names.is_empty() {
                    return Err(Error::at(
                        "a k may not be escaped in a pattern that has a group name",
                    ));
                }
                Ok(letter as u32)
            }
        }
    }

    /// §22.2.1's `CharacterEscape :: c ControlLetter`, and the two things Annex B does with it.
    ///
    /// The letter's code modulo 32, which is the control character it names. §B.1.2 widens the set
    /// inside a class — `ClassControlLetter` adds the decimal digits and `_`, so `[\c0]` is a
    /// `\x10` where `\c0` outside one is not a control escape at all.
    ///
    /// And when no letter of the accepted set follows, `ExtendedAtom :: \ [lookahead = c]` and
    /// `ClassAtomNoDash :: \ [lookahead = c]` say the **backslash alone** is the atom. So `/\c/`
    /// matches the two characters `\c`, `[\c]` holds both of them, and the `c` is left for the
    /// caller's next turn round its own loop rather than being consumed here.
    fn control_escape(&mut self, in_class: bool) -> Result<u32, Error> {
        let wider = in_class && !self.flags.unicode_mode();
        let control = self.peek().filter(|next| match wider {
            true => next.is_ascii_alphanumeric() || *next == '_',
            false => next.is_ascii_alphabetic(),
        });
        if let Some(control) = control {
            self.at += 1;
            return Ok(control as u32 % 32);
        }
        if self.flags.unicode_mode() {
            return Err(Error::at("a control escape needs a letter"));
        }
        // Giving the `c` back is the whole of the fallback — the backslash has already been
        // consumed by the caller, and it is the entire atom.
        self.at -= 1;
        Ok(u32::from(b'\\'))
    }

    /// §B.1.2's `LegacyOctalEscapeSequence`, from the first of its digits.
    ///
    /// Its four productions differ in one thing only: how many digits may follow the first. A
    /// leading `0`–`3` takes two more and a leading `4`–`7` one, which is what holds the value
    /// inside a byte — so `\400` is a space followed by a `0` rather than a code point above 255,
    /// and `\0111` is a tab followed by a `1`. Read as "up to three octal digits" it would answer
    /// 0o400 for the first, which is not a character any of the four can name.
    ///
    /// Only ever entered on an octal digit, so the loop always takes at least one.
    fn legacy_octal(&mut self) -> u32 {
        let most = match self.peek() {
            Some('0'..='3') => 3,
            _ => 2,
        };
        let mut value = 0;
        for _ in 0..most {
            let Some(digit) = self.peek().and_then(|next| next.to_digit(8)) else {
                break;
            };
            value = value * 8 + digit;
            self.at += 1;
        }
        value
    }

    /// A `\x` or `\u` whose digits did not arrive, re-read as the identity escape of its letter.
    ///
    /// §22.2.1 has no such reading: `IdentityEscape[~UnicodeMode]` there is "SourceCharacter but not
    /// `UnicodeIDContinue`", and both letters are one — so a short escape is a pattern with no
    /// derivation. §B.1.2 replaces that production with `SourceCharacterIdentityEscape`, which
    /// excludes only `c`, and a production that fails to match then simply hands the text on: the
    /// digits are given back and the letter stands for itself, so `/\xa/` matches `xa` and `/\u/`
    /// matches `u`.
    ///
    /// Under `u` or `v` the original error is what the program sees, because there the shorter
    /// production is the only one there is.
    fn short_escape(&mut self, letter: char, from: usize, error: Error) -> Result<u32, Error> {
        if self.flags.unicode_mode() {
            return Err(error);
        }
        self.at = from;
        Ok(letter as u32)
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
            && self.ch(self.at + 1) == Some('u')
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

/// §22.2.1's `MayContainStrings`, which decides whether a class may be negated.
///
/// Syntactic and deliberately coarser than the resolved set: the three operations answer
/// differently — a union may contain strings if **any** operand may, an intersection only if
/// **every** one may, and a difference if its **first** one may — and none of them asks what the
/// operands actually hold. That is why `[^[\q{ab}--\q{ab}]]` is refused although the difference
/// is empty.
fn may_contain_strings(operation: ClassOperation, items: &[ClassItem]) -> bool {
    match operation {
        ClassOperation::Union => items.iter().any(item_may_contain_strings),
        ClassOperation::Intersection => items.iter().all(item_may_contain_strings),
        ClassOperation::Difference => items.first().is_some_and(item_may_contain_strings),
    }
}

/// The same question of one operand.
///
/// An alternative exactly one code point long is **not** a string — `\q{a}` is an ordinary member
/// of the character set, so `[^\q{a}]` is a class and `[^\q{ab}]` is not. The empty alternative
/// is one, which is what makes `[^\q{}]` a Syntax Error.
fn item_may_contain_strings(item: &ClassItem) -> bool {
    match item {
        ClassItem::Strings(alternatives) => alternatives.iter().any(|written| written.len() != 1),
        ClassItem::Nested(set) => may_contain_strings(set.operation, &set.items),
        _ => false,
    }
}

/// The sequences a class can consume whole, longest first — §22.2.1's operations over its strings.
///
/// Only lengths other than one, because a one-code-point alternative is an ordinary member of the
/// character set and the matcher's predicate already answers for it. Keeping both halves would
/// make every such alternative match twice over, once as a sequence and once as a character.
///
/// The three operations are the three ways a set of *sequences* combines, and they are computable
/// where the code points are not: a string set is finite and written down, so an intersection can
/// be taken by hand where intersecting `\d` with `\p{L}` could only be a predicate.
fn resolved_strings(operation: ClassOperation, items: &[ClassItem]) -> Vec<Vec<u32>> {
    let mut resolved: Vec<Vec<u32>> = match operation {
        ClassOperation::Union => items.iter().flat_map(item_strings).collect(),
        ClassOperation::Intersection => {
            let mut kept = items.first().map(item_strings).unwrap_or_default();
            for other in items.iter().skip(1) {
                let theirs = item_strings(other);
                kept.retain(|written| theirs.contains(written));
            }
            kept
        }
        ClassOperation::Difference => {
            let mut kept = items.first().map(item_strings).unwrap_or_default();
            for other in items.iter().skip(1) {
                let theirs = item_strings(other);
                kept.retain(|written| !theirs.contains(written));
            }
            kept
        }
    };
    // §22.2.2.7.2 tries the longest candidate first, so the order is part of what the pattern
    // *means* rather than an optimisation — and it is settled here so that no attempt pays for it.
    resolved.sort_by_key(|written| std::cmp::Reverse(written.len()));
    resolved.dedup();
    resolved
}

/// The sequences one operand contributes.
fn item_strings(item: &ClassItem) -> Vec<Vec<u32>> {
    match item {
        ClassItem::Strings(alternatives) => alternatives
            .iter()
            .filter(|written| written.len() != 1)
            .cloned()
            .collect(),
        // No `if !set.negated`. A nested `[^…]` whose contents could match a string has already
        // been refused by `class_set`, so asking a negated one for its strings answers the empty
        // list either way — and a guard no input can flip is a branch nothing could test.
        ClassItem::Nested(set) => resolved_strings(set.operation, &set.items),
        _ => Vec::new(),
    }
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
mod tests;
