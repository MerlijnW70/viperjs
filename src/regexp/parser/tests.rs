//! §22.2.1's grammar, one production at a time.
//!
//! Beside the parser rather than inside it, for the reason the file is this long: Annex B's
//! §B.1.2 doubles every question — a pattern carrying neither `u` nor `v` reads `/}/` as a brace
//! and `//` with no group as a legacy octal escape — so most constructs are tested twice, once
//! under each grammar.

use super::{Assertion, ClassEscape, ClassItem, Flags, GroupKind, Node};

/// `parse` over source written as a `&str`, which is every test in this file.
///
/// The parser takes code units because §22.2.1 reads a pattern as units without `u` or `v` —
/// see [`super::parse`]. A test writes its pattern as Rust source, so it converts here rather
/// than at forty call sites, and a test that means to write a lone surrogate builds the units
/// itself and calls the real one.
fn parse(source: &str, flags: Flags) -> Result<super::Pattern, super::Error> {
    super::parse(&source.encode_utf16().collect::<Vec<_>>(), flags)
}

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
        Node::alternation(vec![Node::Character(97), Node::Empty])
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
        Node::alternation(vec![
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
fn braces_that_do_not_spell_a_quantifier_are_read_as_characters() {
    // Annex B §B.1.2's `ExtendedPatternCharacter`. A `{` that begins no quantifier is a brace,
    // so `/a{/` matches the two characters it looks like.
    assert_eq!(
        plain("a{"),
        Node::Sequence(vec![Node::Character(97), Node::Character(123)])
    );
    assert_eq!(
        plain("a{,2}"),
        Node::Sequence(vec![
            Node::Character(97),
            Node::Character(123),
            Node::Character(44),
            Node::Character(50),
            Node::Character(125),
        ])
    );
    // …and one that does spell a quantifier is consumed whole, braces and all.
    assert_eq!(
        plain("a{2}"),
        Node::Repeat {
            node: Box::new(Node::Character(97)),
            min: 2,
            max: Some(2),
            greedy: true,
        }
    );
    // The two productions compete only where an atom belongs, and `InvalidBracedQuantifier` is
    // listed first: a brace that *does* spell a quantifier with nothing in front of it is a
    // Syntax Error, which is the half a plain "read it as a character" rule would lose.
    for source in ["{1}", "{1,}", "{1,2}", "a{1}{2}", "(?:a){2,3}{4}"] {
        assert_eq!(refused(source), "this has nothing to repeat", "{source}");
    }
    // …and the whole production is `[~UnicodeMode]`, so `u` refuses every one of them.
    for source in ["a{", "a{,2}", "{1}"] {
        assert_eq!(
            unicode(source),
            Err("a regular expression has an unmatched {"),
            "{source}"
        );
    }
}

#[test]
fn a_lone_bracket_or_brace_is_a_character_without_u_and_an_error_with_it() {
    // §B.1.2 takes `]` and `}` off `PatternCharacter`'s exclusion list. `[` and `)` stay on it:
    // both open something, so reading them as characters would lose the unbalanced-bracket
    // error rather than gain a pattern.
    assert_eq!(plain("]"), Node::Character(93));
    assert_eq!(plain("}"), Node::Character(125));
    assert_eq!(
        plain("]{}"),
        Node::Sequence(vec![
            Node::Character(93),
            Node::Character(123),
            Node::Character(125),
        ])
    );
    assert_eq!(unicode("]"), Err("a regular expression has an unmatched ]"));
    assert_eq!(unicode("}"), Err("a regular expression has an unmatched }"));
    assert_eq!(refused("a)"), "a regular expression has an unmatched )");
    assert_eq!(refused("[a"), "a character class is not closed");
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
    // Out of range is an error under `u` and a different production without it — §B.1.2
    // conditions `AtomEscape :: DecimalEscape` on the number being one a group wears, so a
    // bigger one leaves the text to `CharacterEscape` rather than refusing it.
    assert_eq!(unicode("\\1"), Err("a backreference names no group"));
    assert_eq!(unicode("(a)\\2"), Err("a backreference names no group"));
    assert_eq!(plain("\\1"), Node::Character(0x01));
    assert_eq!(
        plain("(a)\\2"),
        Node::Sequence(vec![
            Node::Group {
                kind: GroupKind::Capturing(1),
                body: Box::new(Node::Character(97)),
            },
            Node::Character(0x02),
        ])
    );
    // …and the group count is what decides, so the same text one group later is a reference.
    assert_eq!(
        plain("(a)(b)\\2"),
        Node::Sequence(vec![
            Node::Group {
                kind: GroupKind::Capturing(1),
                body: Box::new(Node::Character(97)),
            },
            Node::Group {
                kind: GroupKind::Capturing(2),
                body: Box::new(Node::Character(98)),
            },
            Node::Backreference(2),
        ])
    );
    // A named one is not conditioned that way: with a group name in the pattern the production
    // is in the grammar, and naming no group is an error in both modes.
    assert_eq!(
        refused("(?<n>a)\\k<m>"),
        "a named backreference names no group"
    );
    assert_eq!(
        unicode("\\k<n>"),
        Err("a named backreference names no group")
    );
}

#[test]
fn a_k_is_a_named_backreference_only_in_a_pattern_that_has_a_group_name() {
    // §22.2.1's `AtomEscape :: [+N] k GroupName`, and `N` is set by a `GroupSpecifier` anywhere
    // in the pattern. With none, §B.1.2's `SourceCharacterIdentityEscape` takes the `k` and the
    // rest is ordinary characters — so this is a pattern rather than the error it looks like.
    assert_eq!(
        plain("\\k<a>"),
        Node::Sequence(vec![
            Node::Character(107),
            Node::Character(60),
            Node::Character(97),
            Node::Character(62),
        ])
    );
    assert_eq!(plain("\\k"), Node::Character(107));
    // A lookbehind is not a `GroupSpecifier`, which is the one shape that reads as though it
    // might be: `(?<=` and `(?<!` name nothing.
    assert!(parse("\\k<a>(?<=>)a", Flags::default()).is_ok());
    assert!(parse("(?<!a>)\\k<a>", Flags::default()).is_ok());
    // One named group anywhere puts the production back, and the `k` with it.
    assert_eq!(
        refused("\\k<a>(?<b>x)"),
        "a named backreference names no group"
    );
    // …and the same fact takes `k` out of the identity escape, which is the only way the two
    // readings could otherwise have met over one pattern. Inside a class is where that is
    // visible, `ClassEscape` having no named-backreference production of its own.
    assert_eq!(
        plain("[\\k]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Single(107)],
            strings: Vec::new(),
        }
    );
    assert_eq!(
        refused("(?<a>x)[\\k]"),
        "a k may not be escaped in a pattern that has a group name"
    );
    // Under `u` there is no identity escape to fall back to at all.
    assert_eq!(unicode("\\k"), Err("a named backreference has no name"));
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
            strings: Vec::new(),
        }
    );
    assert_eq!(
        plain("[^a]"),
        Node::Class {
            negated: true,
            items: vec![ClassItem::Single(97)],
            strings: Vec::new(),
        }
    );
    // A `-` at the end is an atom, not an unfinished range.
    assert_eq!(
        plain("[a-]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Single(97), ClassItem::Single(45)],
            strings: Vec::new(),
        }
    );
    assert_eq!(refused("[z-a]"), "a character class range runs backwards");
    assert_eq!(refused("[a"), "a character class is not closed");
}

#[test]
fn a_range_with_a_class_escape_at_an_end_is_a_union_of_three_without_u_and_an_error_with_it() {
    // §B.1.4.1.1's `CharacterRangeOrUnion`. The hyphen is one of the three, so `[\d-z]` matches
    // a hyphen as well as the digits and the `z` — reading it as "the range is dropped" would
    // lose that and pass most of the tests.
    assert_eq!(
        plain("[\\d-z]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Escape(ClassEscape::Digit(false)),
                ClassItem::Single(45),
                ClassItem::Single(122),
            ],
            strings: Vec::new(),
        }
    );
    // Either end is enough, and so is both.
    assert_eq!(
        plain("[%-\\d]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Single(37),
                ClassItem::Single(45),
                ClassItem::Escape(ClassEscape::Digit(false)),
            ],
            strings: Vec::new(),
        }
    );
    assert_eq!(
        plain("[\\s-\\d]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Escape(ClassEscape::Space(false)),
                ClassItem::Single(45),
                ClassItem::Escape(ClassEscape::Digit(false)),
            ],
            strings: Vec::new(),
        }
    );
    // A leading hyphen is an ordinary atom, so `[--\d]` is a range whose *low* end is one — and
    // the union then holds two of them, which is the case a "drop the hyphen" reading answers
    // the same way by accident and a "drop the range" reading gets wrong.
    assert_eq!(
        plain("[--\\d]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Single(45),
                ClassItem::Single(45),
                ClassItem::Escape(ClassEscape::Digit(false)),
            ],
            strings: Vec::new(),
        }
    );
    // …and what follows the union goes on being read as it was.
    assert_eq!(
        plain("[\\d-az]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Escape(ClassEscape::Digit(false)),
                ClassItem::Single(45),
                ClassItem::Single(97),
                ClassItem::Single(122),
            ],
            strings: Vec::new(),
        }
    );
    // Two single ends are still a range, which is the half this must not take with it.
    assert_eq!(
        plain("[a-c]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Range(97, 99)],
            strings: Vec::new(),
        }
    );
    // Under `u` and `v` the production is §22.2.1.1's and refuses.
    assert_eq!(
        unicode("[\\d-z]"),
        Err("a character class range has a class escape as an end")
    );
    assert_eq!(
        parse(
            "[\\d-z]",
            Flags {
                unicode_sets: true,
                ..Flags::default()
            }
        )
        .err()
        .map(|error| error.message),
        Some("a character class range has a class escape as an end")
    );
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
            strings: Vec::new(),
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
            strings: Vec::new(),
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
    assert_eq!(one("\\01"), 1);
}

#[test]
fn a_c_that_names_no_control_letter_is_a_backslash_and_the_c_is_read_again() {
    // Annex B `ExtendedAtom :: \ [lookahead = c]` — the backslash is the whole atom, so `/\c1/`
    // matches the three characters `\c1` rather than being the error §22.2.1 makes it.
    assert_eq!(
        plain("\\c1"),
        Node::Sequence(vec![
            Node::Character(92),
            Node::Character(99),
            Node::Character(49),
        ])
    );
    assert_eq!(
        plain("\\c"),
        Node::Sequence(vec![Node::Character(92), Node::Character(99)])
    );
    // Inside a class the accepted set is wider — `ClassControlLetter` is the decimal digits and
    // `_` on top of the letters — so the same text is a control character in here and a pair of
    // ordinary ones out there. `\c0` is `0x30 % 32`.
    assert_eq!(
        plain("[\\c0]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Single(0x10)],
            strings: Vec::new(),
        }
    );
    assert_eq!(
        plain("[\\c_]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Single(0x1F)],
            strings: Vec::new(),
        }
    );
    // …and a class gets the fallback too, by `ClassAtomNoDash :: \ [lookahead = c]`, so `[\c!]`
    // holds a backslash, a `c` and a `!`.
    assert_eq!(
        plain("[\\c!]"),
        Node::Class {
            negated: false,
            items: vec![
                ClassItem::Single(92),
                ClassItem::Single(99),
                ClassItem::Single(33),
            ],
            strings: Vec::new(),
        }
    );
    // Under `u` neither the wider set nor the fallback exists.
    assert_eq!(unicode("\\c1"), Err("a control escape needs a letter"));
    assert_eq!(unicode("[\\c0]"), Err("a control escape needs a letter"));
}

#[test]
fn a_legacy_octal_escape_takes_three_digits_after_a_low_one_and_two_after_a_high_one() {
    let one = |source: &str| match plain(source) {
        Node::Character(code) => code,
        Node::Sequence(terms) => match terms.first() {
            Some(Node::Character(code)) => *code,
            other => panic!("{source}: {other:?}"),
        },
        other => panic!("{source}: {other:?}"),
    };
    assert_eq!(one("\\1"), 0x01);
    assert_eq!(one("\\7"), 0x07);
    assert_eq!(one("\\00"), 0x00);
    assert_eq!(one("\\07"), 0x07);
    assert_eq!(one("\\377"), 0xFF);
    // The bound is what keeps the value inside a byte, and it is read off the *first* digit:
    // `\400` is `\40` and a `0`, where three digits would have made it 0o400.
    assert_eq!(
        plain("\\400"),
        Node::Sequence(vec![Node::Character(0x20), Node::Character(48)])
    );
    assert_eq!(
        plain("\\770"),
        Node::Sequence(vec![Node::Character(0x3F), Node::Character(48)])
    );
    // …while a leading `0`–`3` takes all three, so a fourth digit is the one left over.
    assert_eq!(
        plain("\\0111"),
        Node::Sequence(vec![Node::Character(0x09), Node::Character(49)])
    );
    assert_eq!(one("\\070"), 0x38);
    // `8` and `9` are in none of the four productions, so they are identity escapes — which is
    // what makes `/\8/` a pattern matching an `8`.
    assert_eq!(one("\\8"), 56);
    assert_eq!(one("\\9"), 57);
    // A digit after `\0` takes §22.2.1's own production away and Unicode mode replaces it with
    // nothing.
    assert_eq!(
        unicode("\\01"),
        Err("a legacy octal escape is not a character escape")
    );
    assert_eq!(unicode("\\0"), Ok(Node::Character(0)));
}

#[test]
fn a_hex_or_unicode_escape_whose_digits_are_missing_is_the_letter_itself() {
    // §B.1.2's `SourceCharacterIdentityEscape` excludes only `c`, so a `HexEscapeSequence` that
    // does not match is a production that did not match rather than an error: the digits go
    // back and the letter stands for itself.
    assert_eq!(plain("\\x"), Node::Character(120));
    assert_eq!(
        plain("\\xa"),
        Node::Sequence(vec![Node::Character(120), Node::Character(97)])
    );
    assert_eq!(plain("\\u"), Node::Character(117));
    assert_eq!(
        plain("\\ua"),
        Node::Sequence(vec![Node::Character(117), Node::Character(97)])
    );
    // `\u{2}` is the same fallback and then a *quantifier*, the braces having no other reading
    // without `u` — so it matches two `u`s rather than one U+0002.
    assert_eq!(
        plain("\\u{2}"),
        Node::Repeat {
            node: Box::new(Node::Character(117)),
            min: 2,
            max: Some(2),
            greedy: true,
        }
    );
    // …and the digits that *do* arrive are still read, which is what stops the fallback from
    // swallowing every escape.
    assert_eq!(plain("\\x41"), Node::Character(0x41));
    assert_eq!(plain("\\u0041"), Node::Character(0x41));
    // Under `u` the shorter production is the only one there is, and its error is what shows.
    assert_eq!(unicode("\\x4"), Err("a hexadecimal escape is too short"));
    assert_eq!(unicode("\\u00"), Err("a hexadecimal escape is too short"));
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
    // `\u{…}` needs the flag, and a surrogate pair is one code point only with it. Without the
    // flag the braces are not part of the escape at all — `\u` is an identity escape and
    // `{41}` a quantifier on it, so this matches forty-one `u`s.
    assert_eq!(
        plain("\\u{41}"),
        Node::Repeat {
            node: Box::new(Node::Character(117)),
            min: 41,
            max: Some(41),
            greedy: true,
        }
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
            strings: Vec::new(),
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
            strings: Vec::new(),
        }
    );
    assert_eq!(
        plain("[\\n]"),
        Node::Class {
            negated: false,
            items: vec![ClassItem::Single(0x0A)],
            strings: Vec::new(),
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
            strings: Vec::new(),
        }
    );
    assert_eq!(
        unicode("[\\-]"),
        Ok(Node::Class {
            negated: false,
            items: vec![ClassItem::Single(45)],
            strings: Vec::new(),
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
            strings: Vec::new(),
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
    // A **property of strings** is a thing ViperJS has not built rather than a name the
    // specification rejects, and the two are refused differently on purpose — see
    // `regexp::Error::unimplemented`.
    // §22.2.1 — a property of strings is legal *only* in a `v` pattern, positive and outside a
    // negated class. The other three positions are the specification refusing, so they are a
    // real answer about the text rather than a gap ViperJS has yet to fill.
    assert_eq!(
        unicode(r"\p{RGI_Emoji}"),
        Err("a property of strings needs the v flag")
    );
}
