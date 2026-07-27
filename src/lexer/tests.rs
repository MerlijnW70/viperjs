//! The lexer's cross-cutting tests: the round-trip oracle, the goal symbols, and the promise
//! that no input panics.
//!
//! Apart from `mod.rs` because they are about the lexer as a whole rather than about any one
//! token form — every other file here tests what it parses, and these test what they add up to.

use super::*;
use crate::lexer::test_support::*;
/// Rebuild `source` from the token stream alone, and return how far lexing got.
///
/// For each token this appends the trivia gap that preceded it, then the text the token's
/// own span covers — so the result can only equal the source if the spans are ordered,
/// non-overlapping, and leave nothing out. It also asserts each span covers the *right*
/// bytes by cross-checking against [`TokenKind::as_str`]; tiling alone would be satisfied by
/// spans that are contiguous but shifted.
///
/// Placeholders rather than `unwrap` on a bad span: a panic here would be reported as a
/// crash in the helper, while a placeholder shows up in the diff of the failing assertion.
fn retile(source: &str) -> (String, usize) {
    retile_under(source, Goal::Div)
}

/// The same, reading every `/` as opening a regular expression literal.
///
/// A goal is a per-token choice in a real parser, so lexing a whole source under one is
/// artificial — but the property is not about which reading is right, it is that whichever
/// reading the lexer takes, it accounts for every byte it consumed.
fn retile_under(source: &str, goal: Goal) -> (String, usize) {
    let mut lexer = Lexer::new(source);
    let mut out = String::new();
    let mut at = 0usize;
    loop {
        match lexer.next_token(goal) {
            Ok(token) => {
                let start = token.span.start as usize;
                out.push_str(source.get(at..start).unwrap_or("<GAP OUT OF ORDER>"));
                let text = token.span.slice(source).unwrap_or("<SPAN OFF BOUNDARY>");
                if let Some(fixed) = token.kind.as_str() {
                    assert_eq!(text, fixed, "span and kind disagree in {source:?}");
                }
                out.push_str(text);
                at = token.span.end as usize;
                if token.kind == TokenKind::Eof {
                    return (out, at);
                }
            }
            Err(err) => {
                let stop = err.span.start as usize;
                out.push_str(source.get(at..stop).unwrap_or("<GAP OUT OF ORDER>"));
                return (out, stop);
            }
        }
    }
}

#[test]
fn the_token_spans_and_the_trivia_between_them_reconstruct_the_source_exactly() {
    // The oracle for this slice. Every input here has broken a real lexer at some point.
    let lexes_completely = [
        "",                            // empty file — EOF is still a token
        ";",                           // no trivia at all
        " ; ",                         // trivia on both sides, including trailing
        "\u{feff};",                   // a BOM, which is just white space (§12.2)…
        ";\u{feff};",                  // …anywhere, not only at the start
        "\r",                          // lone CR, old-Mac style
        "\r\n;",                       // CRLF
        "\n\r;",                       // LF then CR — two line breaks, not a pair
        "\u{2028};",                   // LINE SEPARATOR
        "\u{2029};",                   // PARAGRAPH SEPARATOR
        "//x",                         // line comment ended by EOF, not a newline
        "//x\n;",                      // …and one ended by a newline it does not own
        "//x\u{2028};",                // U+2028 ends a line comment too
        "/**/;",                       // the shortest block comment
        "/***/;",                      // an asterisk that is not the terminator
        "/*/*/;",                      // comments do not nest: this one closes
        "/* a\n b */;",                // a block comment spanning lines
        "<!--",                        // Annex B.1.1, deliberately not a comment yet
        ">>>=?.(){}[]...=>",           // longest-match punctuators, back to back
        "{}();,:",                     //
        "/ /=",                        // a slash that is neither comment form
        "\t\u{000b}\u{000c}\u{00a0};", // <TAB> <VT> <FF> and NO-BREAK SPACE
        "\u{1680}\u{2000}\u{200a};",   // exotic <USP> members
        "\u{202f}\u{205f}\u{3000};",   // …and the rest of them
        "a",                           // the shortest name there is
        "a b",                         // …two of them, and the trivia between
        "_$0",                         // both ECMAScript additions plus a digit
        "if else",                     // keywords, whose spans must also line up
        "caf\u{e9} \u{5d0} \u{3042}",  // names that are not ASCII
        "x\u{1d49c}",                  // …including one outside the BMP
        "#priv",                       // a private name, `#` included in the span
        "#!/usr/bin/env node\n;",      // §12.5 hashbang, only at byte 0
        "\\u0061",                     // a name spelled entirely as an escape
        "a\\u{62}c",                   // …and one spelled partly as one
        "\\u{61}\\u{62}",              // two escapes in a row
        "0",                           // the shortest literal there is
        "1_000.5e-3",                  // a decimal wearing everything at once
        ".5",                          // …and one with no integer part
        "?.5",                         // `? .5`, the conditional §12.8's lookahead protects
        "0x1F 0b1_0 0o7 0123 08",      // every radix, Annex B's two included
        "1n 0x2n",                     // BigInt, whose `n` is part of the span
        "1..toString",                 // `1.` then `.` then a name
        "\"\" ''",                     // both empty literals
        "\"a\" 'b'",                   // …and both non-empty
        "\"it's\"",                    // the other quote, unescaped
        "\"a\\\"b\"",                  // an escaped quote, which does not end the literal
        "\"a\\\nb\"",                  // a line continuation, spanning a line inside a token
        "\"\\u{1f680}\\ud800\\x41\"",  // escapes of every form, lone surrogate included
        "\"\\7\\8\"",                  // Annex B legacy escapes
        "\"\u{2028}\"",                // <LS>, legal raw here and nowhere else
    ];
    for source in lexes_completely {
        let (tiled, stopped) = retile(source);
        assert_eq!(tiled, source, "retiling {source:?}");
        assert_eq!(stopped, source.len(), "stopped early on {source:?}");
    }

    // Templates, whose components are delimited at both ends and may span lines. The
    // `}` forms need the goal that lets a brace resume one.
    for source in [
        "`abc`",
        "``",
        "`a${",
        "`a\nb`",
        "`$`",
        r"`\u{1f680}`",
        r"`\unicode`",
        r"`\\`",
    ] {
        let (tiled, stopped) = retile_under(source, Goal::Div);
        assert_eq!(tiled, source, "retiling {source:?}");
        assert_eq!(stopped, source.len(), "stopped early on {source:?}");
    }
    for source in ["}abc`", "}${", "}`", "}a${"] {
        let (tiled, stopped) = retile_under(source, Goal::TemplateTail);
        assert_eq!(tiled, source, "retiling {source:?}");
        assert_eq!(stopped, source.len(), "stopped early on {source:?}");
    }

    // The same property under the other goal, over sources where the two disagree — a
    // literal's span must tile just as exactly as a punctuator's, escaped slashes and
    // character classes included.
    for source in [
        "/a/",
        "/a/gi",
        "/=/",
        r"/ab\/[/]c/gi",
        "/[]/ /(?:)/",
        "// a comment, even here",
        "/* and this */ /x/",
        "x = /a/g;",
    ] {
        let (tiled, stopped) = retile_under(source, Goal::RegExp);
        assert_eq!(tiled, source, "retiling {source:?} as regular expressions");
        assert_eq!(stopped, source.len(), "stopped early on {source:?}");
    }

    // Inputs that stop partway: the reconstruction must still be an exact prefix — the
    // lexer may refuse to continue, but it may not invent or lose a byte before it does.
    for source in [
        "/*",
        "/*/",
        "/* x",
        "@",
        "3in",
        "0x",
        "1__0",
        "\"abc",
        "\"a\nb\"",
        "\"\\x4\"",
        ";\u{200b}",
        "a\\x",
        "#5",
    ] {
        let (tiled, stopped) = retile(source);
        assert_eq!(source.get(..stopped), Some(tiled.as_str()), "on {source:?}");
        assert!(
            stopped < source.len(),
            "{source:?} should not lex completely"
        );
    }
}

#[test]
fn eof_is_a_token_with_an_empty_span_at_the_end_and_repeats_forever() {
    let mut lexer = Lexer::new(" ");
    let eof = lexer.next_token(Goal::Div).expect("whitespace only lexes"); // the assertion under test needs the token
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(eof.span, Span::empty_at(1)); // at the END of the trivia, not the start
    // Asking again must not advance, wrap, or produce a different token: a recovering
    // parser will ask an unbounded number of times.
    for _ in 0..3 {
        assert_eq!(lexer.next_token(Goal::Div), Ok(eof));
    }
    // An empty source is the same story with nothing before it.
    assert_eq!(kinds(""), [TokenKind::Eof]);
    assert_eq!(first("").span, Span::empty_at(0));
}

#[test]
fn punctuators_take_the_longest_match() {
    // Every family where a shorter punctuator is a prefix of a longer one. Each line is a
    // place a first-match-wins lexer produces two tokens where the source has one.
    let families: &[(&str, &[TokenKind])] = &[
        (">>>=", &[TokenKind::GtGtGtEq]),
        (">>>", &[TokenKind::GtGtGt]),
        (">>=", &[TokenKind::GtGtEq]),
        (">>", &[TokenKind::GtGt]),
        (">=", &[TokenKind::GtEq]),
        (">", &[TokenKind::Gt]),
        ("<<=", &[TokenKind::LtLtEq]),
        ("<<", &[TokenKind::LtLt]),
        ("<=", &[TokenKind::LtEq]),
        ("<", &[TokenKind::Lt]),
        ("...", &[TokenKind::DotDotDot]),
        ("..", &[TokenKind::Dot, TokenKind::Dot]),
        (".", &[TokenKind::Dot]),
        ("===", &[TokenKind::EqEqEq]),
        ("==", &[TokenKind::EqEq]),
        ("=>", &[TokenKind::Arrow]),
        ("=", &[TokenKind::Eq]),
        ("!==", &[TokenKind::BangEqEq]),
        ("!=", &[TokenKind::BangEq]),
        ("!", &[TokenKind::Bang]),
        ("**=", &[TokenKind::StarStarEq]),
        ("**", &[TokenKind::StarStar]),
        ("*=", &[TokenKind::StarEq]),
        ("*", &[TokenKind::Star]),
        ("&&=", &[TokenKind::AmpAmpEq]),
        ("&&", &[TokenKind::AmpAmp]),
        ("&=", &[TokenKind::AmpEq]),
        ("&", &[TokenKind::Amp]),
        ("||=", &[TokenKind::PipePipeEq]),
        ("||", &[TokenKind::PipePipe]),
        ("|=", &[TokenKind::PipeEq]),
        ("|", &[TokenKind::Pipe]),
        ("??=", &[TokenKind::QuestionQuestionEq]),
        ("??", &[TokenKind::QuestionQuestion]),
        ("?.", &[TokenKind::QuestionDot]),
        ("?", &[TokenKind::Question]),
        ("++", &[TokenKind::PlusPlus]),
        ("+=", &[TokenKind::PlusEq]),
        ("+", &[TokenKind::Plus]),
        ("--", &[TokenKind::MinusMinus]),
        ("-=", &[TokenKind::MinusEq]),
        ("-", &[TokenKind::Minus]),
        ("/=", &[TokenKind::SlashEq]),
        ("%=", &[TokenKind::PercentEq]),
        ("^=", &[TokenKind::CaretEq]),
        ("^", &[TokenKind::Caret]),
        ("~", &[TokenKind::Tilde]),
        // `>>>>` is a real hazard: the longest match takes three, leaving one.
        (">>>>", &[TokenKind::GtGtGt, TokenKind::Gt]),
        ("====", &[TokenKind::EqEqEq, TokenKind::Eq]),
    ];
    for (source, expected) in families {
        let mut want = expected.to_vec();
        want.push(TokenKind::Eof);
        assert_eq!(kinds(source), want, "lexing {source:?}");
    }
}

#[test]
fn optional_chaining_yields_to_a_following_decimal_digit() {
    // §12.8: `?. [lookahead ∉ DecimalDigit]`. `a?.5:b` is a conditional expression that has
    // been legal since ES3 — the consequent is the numeric literal `.5` — and lexing `?.`
    // there breaks code older than optional chaining. Now that numbers exist, the whole
    // tokenization is visible: a question mark, then a number, and no `?.` anywhere.
    assert_eq!(kinds("?.5"), [TokenKind::Question, NUMBER, TokenKind::Eof]);
    // Every digit, not just one: a `is_ascii_digit` written as `== b'0'` passes the above.
    for digit in '0'..='9' {
        let source = format!("?.{digit}");
        let mut lexer = Lexer::new(&source);
        assert_eq!(
            lexer.next_token(Goal::Div).map(|t| t.kind),
            Ok(TokenKind::Question),
            "?.{digit} must not be optional chaining"
        );
    }
    // Anything else after `?.` leaves it a single punctuator…
    assert_eq!(
        kinds("?.("),
        [TokenKind::QuestionDot, TokenKind::LParen, TokenKind::Eof]
    );
    assert_eq!(kinds("?."), [TokenKind::QuestionDot, TokenKind::Eof]);
    assert_eq!(
        kinds("?.["),
        [TokenKind::QuestionDot, TokenKind::LBracket, TokenKind::Eof]
    );
    // …including a non-ASCII digit, which `DecimalDigit` (§12.9.3) is not.
    assert_eq!(
        Lexer::new("?.٥").next_token(Goal::Div).map(|t| t.kind),
        Ok(TokenKind::QuestionDot),
        "ARABIC-INDIC DIGIT FIVE is not a DecimalDigit"
    );
    // A space between them is not lookahead: `? .5` was always two tokens.
    assert_eq!(
        kinds("? ."),
        [TokenKind::Question, TokenKind::Dot, TokenKind::Eof]
    );
}

#[test]
fn an_html_open_comment_lexes_as_three_punctuators_until_annex_b_arrives() {
    // Annex B.1.1 gives `<!--` and `-->` alternative comment definitions for web
    // compatibility. They are deliberately NOT implemented in this slice: `-->` needs
    // "only trivia before it on this line" state and a Script-vs-Module goal flag. This
    // test exists so that implementing Annex B changes it on purpose rather than by
    // accident — if it starts failing, that is the day, not a regression.
    assert_eq!(
        kinds("<!--"),
        [
            TokenKind::Lt,
            TokenKind::Bang,
            TokenKind::MinusMinus,
            TokenKind::Eof
        ]
    );
    assert_eq!(
        kinds("-->"),
        [TokenKind::MinusMinus, TokenKind::Gt, TokenKind::Eof]
    );
}

#[test]
fn a_character_with_no_token_form_yet_is_an_error_that_covers_the_whole_character() {
    // The error span must cover the character a human sees. Reporting one byte of a
    // multi-byte code point produces a caret pointing into the middle of an emoji — and,
    // worse, would leave the cursor off a boundary.
    let cases = [
        ("@", 1),        // never a token in any edition
        ("\u{0000}", 1), // NUL is legal source text, just not a token start
        // Multi-byte code points that are not identifier characters — `é` and `א` would be
        // names now, so these are drawn from categories Unicode leaves out of ID_Start.
        ("\u{00a7}", 2), // SECTION SIGN, two bytes
        ("€", 3),        // three
        ("🚀", 4),       // four
    ];
    for (source, len) in cases {
        assert_eq!(
            Lexer::new(source).tokens(Goal::Div),
            Err(LexError {
                kind: LexErrorKind::UnexpectedCharacter,
                span: Span::new(0, len),
            }),
            "on {source:?}"
        );
    }
    // The offending character is reported where it is, not where the token stream started.
    assert_eq!(
        Lexer::new("; @").tokens(Goal::Div),
        Err(LexError {
            kind: LexErrorKind::UnexpectedCharacter,
            span: Span::new(2, 3),
        })
    );
}

#[test]
fn no_single_code_point_can_make_the_lexer_panic() {
    // DR-0002: no input may panic, and "that input is absurd" is not a defence. A sweep
    // rather than a fuzzer because the interesting boundaries are all reachable by hand:
    // every ASCII byte, both ends of every white-space and line-terminator range, and one
    // character from each UTF-8 length class.
    let mut probes: Vec<String> = (0u8..=0x7f).map(|b| (b as char).to_string()).collect();
    for ch in [
        '\u{0085}',
        '\u{00a0}',
        '\u{167f}',
        '\u{1680}',
        '\u{1681}',
        '\u{1fff}',
        '\u{2000}',
        '\u{200a}',
        '\u{200b}',
        '\u{2027}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202f}',
        '\u{205f}',
        '\u{3000}',
        '\u{feff}',
        '\u{ffff}',
        '\u{10000}',
        '\u{10ffff}',
    ] {
        probes.push(ch.to_string());
    }
    for probe in &probes {
        // Alone, after a slash (the trivia fork), and inside each comment form — the four
        // places a byte-oriented lexer can step off a character boundary.
        for source in [
            probe.clone(),
            format!("/{probe}"),
            format!("//{probe}"),
            format!("/*{probe}*/;"),
            format!("/*{probe}"),
        ] {
            // The result does not matter; not unwinding does. Retiling additionally proves
            // no byte was invented or lost on the way.
            let (tiled, stopped) = retile(&source);
            assert_eq!(source.get(..stopped), Some(tiled.as_str()), "on {source:?}");
        }
    }
}

#[test]
fn tokens_collects_the_whole_stream_and_stops_at_the_first_error() {
    let tokens = Lexer::new(" ;\n; ")
        .tokens(Goal::Div)
        .expect("this source lexes"); // the assertion under test needs the tokens
    assert_eq!(tokens.len(), 3, "two semicolons and EOF");
    assert_eq!(tokens[0].span, Span::new(1, 2));
    assert!(!tokens[0].newline_before);
    assert_eq!(tokens[1].span, Span::new(3, 4));
    assert!(tokens[1].newline_before);
    assert_eq!(tokens[2].kind, TokenKind::Eof);
    assert_eq!(
        tokens[2].span,
        Span::empty_at(5),
        "EOF sits past the trailing space"
    );
    // The first error wins, and the tokens before it are discarded — a caller that wants
    // them can drive `next_token` itself.
    assert_eq!(
        Lexer::new(";@;").tokens(Goal::Div).map(|t| t.len()),
        Err(LexError {
            kind: LexErrorKind::UnexpectedCharacter,
            span: Span::new(1, 2),
        })
    );
}
