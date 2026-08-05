//! Template literal components (ECMA-262 §12.9.6), and the two values each one carries.
//!
//! # A malformed escape is still a valid token
//!
//! This is the production where that stops being obvious. `TemplateCharacter` has an alternative
//! for `\ NotEscapeSequence` — an escape that is *not* well formed — and §12.9.6 spells out
//! eleven shapes it can take. So `` `\u{110000}` `` lexes cleanly, and `` `\unicode` `` does too.
//! What happens instead is that the component's `TV` is **undefined** (§12.9.6.1) while its `TRV`
//! is unaffected, which is exactly what a tagged template needs: `` tag`\unicode` `` hands the
//! tag `undefined` for the cooked string and the raw text alongside it. Rejecting it is the
//! parser's business and only for *untagged* templates (§13.2.8.1).
//!
//! That is the 2018 revision to the language, and it is the reason nothing in this file returns
//! an error for a bad escape. The only way a template fails to lex is by never being closed.
//!
//! # Nesting needs no stack here
//!
//! `` `a${ `b` }c` `` nests, and this file has no counter for it. A `}` resumes a template only
//! under [`super::Goal::TemplateTail`], and the parser passes that goal exactly when it has
//! finished the substitution expression it was parsing — so the nesting is tracked where it is
//! already being tracked, by the recursion in the parser. A lexer-side depth counter would be a
//! second copy of that state, and the two would eventually disagree.
//!
//! # `TRV` is simpler than it looks
//!
//! Escapes contribute their own source text to the raw value, and ordinary characters contribute
//! themselves — so `TRV` is just the component's text, with one substitution: §12.9.6.2
//! normalizes `<CR>` and `<CR><LF>` to a single `<LF>`, in the raw value as much as the cooked
//! one. An explicit escape is the only way to get a real carriage return into either.

use super::escape::{CodeUnits, hex_value, utf16_encode};
use super::{LexError, LexErrorKind, Lexer, TokenKind};
use crate::span::Span;

/// Which of §12.9.6's four components a template token is.
///
/// The pair of questions they answer is "did this open with a backtick or a `}`?" and "does it
/// close with a backtick or a `${`?" — the parser needs the first to know whether it is starting
/// a template and the second to know whether a substitution follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemplatePart {
    /// `` `…` `` — a whole template with no substitutions in it.
    NoSubstitution,
    /// `` `…${ `` — the start of a template, with a substitution following.
    Head,
    /// `}…${` — between two substitutions.
    Middle,
    /// ``}…` `` — after the last substitution.
    Tail,
}

impl TemplatePart {
    /// Whether a substitution expression follows this component.
    ///
    /// True for the two that end in `${`. This is the question that tells a parser whether to
    /// keep going, and phrasing it once here beats matching two variants at every call site.
    pub fn is_followed_by_substitution(&self) -> bool {
        matches!(self, Self::Head | Self::Middle)
    }
}

/// The two strings §12.9.6 gives a template component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateValue {
    /// `TV` (§12.9.6.1), the cooked value — `None` when the component contains a
    /// `NotEscapeSequence`, which is what the specification means by "undefined" there.
    ///
    /// A tagged template hands this straight to the tag function, `undefined` and all. An
    /// untagged one is a Syntax Error (§13.2.8.1), which the parser raises because only it knows
    /// which of the two it is looking at.
    pub cooked: Option<Vec<u16>>,
    /// `TRV` (§12.9.6.2), the raw value — always present, escapes left exactly as written.
    pub raw: Vec<u16>,
}

/// The `TV` and `TRV` of a template component, or `None` if `span` does not cover one.
///
/// ```
/// use viperjs::lexer::{template_value, Goal, Lexer, TemplatePart, TokenKind};
///
/// // A well-formed escape cooks; the raw value keeps it as written.
/// let source = r"`a\n`";
/// let token = Lexer::new(source).next_token(Goal::Div).expect("this lexes");
/// assert_eq!(
///     token.kind,
///     TokenKind::Template { part: TemplatePart::NoSubstitution, cooked_undefined: false }
/// );
/// let value = template_value(source, token.span).expect("a template component");
/// assert_eq!(value.cooked, Some(vec![0x61, 0x0a]));
/// assert_eq!(value.raw, vec![0x61, 0x5c, 0x6e]);
/// ```
pub fn template_value(source: &str, span: Span) -> Option<TemplateValue> {
    let body = component_body(span.slice(source)?)?;
    Some(TemplateValue {
        cooked: cooked_value(body),
        raw: raw_value(body),
    })
}

/// The `TemplateCharacters` between a component's delimiters.
///
/// A component opens with a backtick or a `}` and closes with a backtick or a `${`, in any of the
/// four combinations — so the delimiters are stripped by trying each, and a span that carries
/// neither is not a template component at all.
fn component_body(text: &str) -> Option<&str> {
    let open = text.strip_prefix('`').or_else(|| text.strip_prefix('}'))?;
    open.strip_suffix('`').or_else(|| open.strip_suffix("${"))
}

/// `TRV` (§12.9.6.2): the component's text, with `<CR>` and `<CR><LF>` normalized to `<LF>`.
///
/// No escape knowledge at all, because none is needed — every alternative of `TemplateCharacter`
/// contributes its own source text to the raw value, escapes included. The line terminator rule
/// is the single exception, and it is why `` `a\r\nb`.raw `` has one code unit between the
/// letters rather than two.
fn raw_value(body: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            // `LineTerminatorSequence :: <CR> [lookahead ≠ <LF>]` and `<CR> <LF>` are separate
            // productions with the same TRV, so both collapse to one line feed.
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            out.push(0x000a);
            continue;
        }
        utf16_encode(ch as u32).push_onto(&mut out);
    }
    out
}

/// `TV` (§12.9.6.1): the cooked value, or `None` if any `NotEscapeSequence` appears.
fn cooked_value(body: &str) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(body.len());
    let mut lexer = Lexer::new(body);
    let mut well_formed = true;
    while let Some(ch) = lexer.cursor.peek() {
        if ch == '\\' {
            let escape = lexer.read_template_escape();
            well_formed &= escape.well_formed;
            escape.units.push_onto(&mut out);
            continue;
        }
        let _ = lexer.cursor.bump();
        if ch == '\r' {
            // Normalized in the cooked value exactly as in the raw one.
            if lexer.cursor.peek() == Some('\n') {
                let _ = lexer.cursor.bump();
            }
            out.push(0x000a);
            continue;
        }
        utf16_encode(ch as u32).push_onto(&mut out);
    }
    // The units are still accumulated when an escape is malformed, and then thrown away. It
    // costs nothing measurable and keeps the loop from having two shapes.
    well_formed.then_some(out)
}

/// One `\`-escape inside a template. Unlike a string's, it cannot fail — only be ill-formed.
struct TemplateEscape {
    /// What it contributes to `TV`. Empty for a `LineContinuation`, and for anything ill-formed.
    units: CodeUnits,
    /// `false` for a `NotEscapeSequence` (§12.9.6), which makes the whole component's `TV`
    /// undefined without making it any less of a token.
    well_formed: bool,
}

impl Lexer<'_> {
    /// Scan a template component, with the cursor on its opening backtick or `}`.
    ///
    /// `opened_with_backtick` says which, and so decides whether the result is a
    /// `NoSubstitutionTemplate`/`TemplateHead` or a `TemplateMiddle`/`TemplateTail`. The closing
    /// delimiter decides the other half.
    pub(super) fn scan_template(
        &mut self,
        opened_with_backtick: bool,
    ) -> Result<TokenKind, LexError> {
        let start = self.cursor.offset();
        let _ = self.cursor.bump();
        let mut cooked_undefined = false;
        loop {
            let Some(ch) = self.cursor.peek() else {
                return Err(LexError {
                    kind: LexErrorKind::UnterminatedTemplate,
                    span: Span::new(start, self.cursor.offset()),
                });
            };
            match ch {
                '`' => {
                    let _ = self.cursor.bump();
                    return Ok(TokenKind::Template {
                        part: if opened_with_backtick {
                            TemplatePart::NoSubstitution
                        } else {
                            TemplatePart::Tail
                        },
                        cooked_undefined,
                    });
                }
                // `TemplateCharacter :: $ [lookahead ≠ {]` — a `$` is an ordinary character
                // unless a `{` follows, which is why `` `costs $5` `` needs no escaping.
                '$' if self.cursor.peek_byte(1) == Some(b'{') => {
                    self.cursor.advance_ascii(2);
                    return Ok(TokenKind::Template {
                        part: if opened_with_backtick {
                            TemplatePart::Head
                        } else {
                            TemplatePart::Middle
                        },
                        cooked_undefined,
                    });
                }
                '\\' => cooked_undefined |= !self.read_template_escape().well_formed,
                _ => {
                    // Line terminators need no special case here: they are ordinary
                    // `TemplateCharacter`s, which is the whole point of a template. Only the
                    // *values* normalize them.
                    let _ = self.cursor.bump();
                }
            }
        }
    }

    /// Read one `\`-escape, reporting whether it was well formed rather than failing.
    ///
    /// The extents matter as much as the values: §12.9.6's `NotEscapeSequence` productions carry
    /// lookahead restrictions that say precisely how much text a bad escape swallows, because
    /// whatever it leaves behind is ordinary template text. `` `\xg` `` is the two characters
    /// `\x` followed by a `g`, and `` `\x1g` `` is `\x1` followed by a `g`.
    fn read_template_escape(&mut self) -> TemplateEscape {
        let ill_formed = TemplateEscape {
            units: CodeUnits::Nothing,
            well_formed: false,
        };
        let Some(after) = self.cursor.peek_byte(1) else {
            // A backslash against the end of input. The caller finds EOF on its next look and
            // reports the template unterminated, which is the failure that actually matters.
            self.cursor.advance_ascii(1);
            return ill_formed;
        };

        if after == b'u' {
            // `read_unicode_escape` consumes the backslash itself, and stops where
            // `NotEscapeSequence` says to — with one harmless exception: for `\u{110000}` it also
            // takes the closing brace, which `u { NotCodePoint [lookahead ∉ HexDigit]` would
            // leave as an ordinary character. Both readings put the same text in `TRV` and make
            // `TV` undefined, and both resume at the same place, so nothing can tell them apart.
            return match self.read_unicode_escape() {
                Ok(code_point) => TemplateEscape {
                    units: utf16_encode(code_point),
                    well_formed: true,
                },
                Err(_) => ill_formed,
            };
        }

        self.cursor.advance_ascii(1); // the `\`
        match after {
            // `SingleEscapeCharacter`, Table 33 — the same nine a string literal has.
            b'b' | b't' | b'n' | b'v' | b'f' | b'r' | b'"' | b'\'' | b'\\' | b'`' => {
                self.cursor.advance_ascii(1);
                let unit = match after {
                    b'b' => 0x0008,
                    b't' => 0x0009,
                    b'n' => 0x000a,
                    b'v' => 0x000b,
                    b'f' => 0x000c,
                    b'r' => 0x000d,
                    b'"' => 0x0022,
                    b'\'' => 0x0027,
                    b'`' => 0x0060,
                    _ => 0x005c,
                };
                TemplateEscape {
                    units: CodeUnits::One(unit),
                    well_formed: true,
                }
            }
            // `HexEscapeSequence :: x HexDigit HexDigit`, against `NotEscapeSequence :: x` with
            // zero or one digit behind it. Reading what is there and checking the count afterwards
            // gets both the value and the extent right at once.
            b'x' => {
                self.cursor.advance_ascii(1);
                let mut value: u16 = 0;
                let mut digits = 0;
                while digits < 2 {
                    let Some(digit) = self.cursor.peek().and_then(hex_value) else {
                        break;
                    };
                    let _ = self.cursor.bump();
                    digits += 1;
                    // Bounded by construction: two hex digits cannot exceed 0xFF.
                    value = value * 16 + digit as u16;
                }
                if digits < 2 {
                    return ill_formed;
                }
                TemplateEscape {
                    units: CodeUnits::One(value),
                    well_formed: true,
                }
            }
            // `TemplateEscapeSequence :: 0 [lookahead ∉ DecimalDigit]` is a NUL, and every other
            // digit shape is a `NotEscapeSequence`. Note what is missing compared with a string
            // literal: a template has no legacy octal escapes at all, so `` `\7` `` is ill formed
            // where `"\7"` is merely legacy.
            b'0'..=b'9' => {
                self.cursor.advance_ascii(1);
                let next_is_digit = self.cursor.peek().is_some_and(|ch| ch.is_ascii_digit());
                if after == b'0' && !next_is_digit {
                    return TemplateEscape {
                        units: CodeUnits::One(0x0000),
                        well_formed: true,
                    };
                }
                // `NotEscapeSequence :: 0 DecimalDigit` covers two characters where
                // `DecimalDigit but not 0` covers one, and this consumes one either way.
                // Nothing can tell the difference: an ill-formed escape contributes nothing to
                // `TV`, which is undefined for the whole component regardless, and `TRV` is read
                // from the source text rather than from here — so whether the second digit is
                // swallowed by the escape or scanned as the ordinary character it equally is
                // cannot change any output, including the component's span. Spelling the
                // distinction out would mean writing a branch no input could ever pin.
                ill_formed
            }
            _ => {
                // `LineContinuation :: \ LineTerminatorSequence`, whose TV is the empty String —
                // and CRLF counts as one sequence, so the line feed must go with the return.
                let ch = self.cursor.peek();
                if matches!(ch, Some('\n' | '\r' | '\u{2028}' | '\u{2029}')) {
                    let _ = self.cursor.bump();
                    if ch == Some('\r') && self.cursor.peek() == Some('\n') {
                        let _ = self.cursor.bump();
                    }
                    return TemplateEscape {
                        units: CodeUnits::Nothing,
                        well_formed: true,
                    };
                }
                // `CharacterEscapeSequence :: NonEscapeCharacter` — the code point stands for
                // itself, so `` `\q` `` is `q` and is perfectly well formed.
                match self.cursor.bump() {
                    Some(ch) => TemplateEscape {
                        units: utf16_encode(ch as u32),
                        well_formed: true,
                    },
                    None => ill_formed,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Goal;
    use crate::lexer::test_support::*;

    /// The kinds of `source`, reading a `}` as resuming a template.
    fn tail_kinds(source: &str) -> Vec<TokenKind> {
        Lexer::new(source)
            .tokens(Goal::TemplateTail)
            .unwrap_or_else(|err| panic!("{source:?} should lex, got {}", err.kind)) // a test asserting clean lexing has nothing to say if lexing failed
            .iter()
            .map(|t| t.kind)
            .collect()
    }

    /// The cooked and raw values of the one component in `source`.
    fn values(source: &str) -> (Option<String>, String) {
        let token = first(source);
        let value = template_value(source, token.span)
            .unwrap_or_else(|| panic!("{source:?} should have template values")); // same
        let decode = |units: &[u16]| {
            String::from_utf16(units)
                .unwrap_or_else(|_| panic!("{source:?} should be well-formed UTF-16")) // same
        };
        (value.cooked.as_deref().map(decode), decode(&value.raw))
    }

    /// `Template { part, cooked_undefined: false }`.
    fn part(part: TemplatePart) -> TokenKind {
        TokenKind::Template {
            part,
            cooked_undefined: false,
        }
    }

    #[test]
    fn the_four_components_are_told_apart_by_their_two_delimiters() {
        // §12.9.6: a component opens with a backtick or a `}` and closes with a backtick or a
        // `${`, and the four combinations are the four productions. The opening delimiter is the
        // goal's business; the closing one is the scanner's.
        assert_eq!(
            kinds("`abc`"),
            [part(TemplatePart::NoSubstitution), TokenKind::Eof]
        );
        assert_eq!(kinds("`abc${"), [part(TemplatePart::Head), TokenKind::Eof]);
        assert_eq!(
            tail_kinds("}abc${"),
            [part(TemplatePart::Middle), TokenKind::Eof]
        );
        assert_eq!(
            tail_kinds("}abc`"),
            [part(TemplatePart::Tail), TokenKind::Eof]
        );
        // Empty bodies are legal in all four.
        assert_eq!(
            kinds("``"),
            [part(TemplatePart::NoSubstitution), TokenKind::Eof]
        );
        assert_eq!(kinds("`${"), [part(TemplatePart::Head), TokenKind::Eof]);
        assert_eq!(
            tail_kinds("}${"),
            [part(TemplatePart::Middle), TokenKind::Eof]
        );
        assert_eq!(tail_kinds("}`"), [part(TemplatePart::Tail), TokenKind::Eof]);
        // Only the two ending in `${` are followed by a substitution — the question a parser
        // actually asks.
        assert!(TemplatePart::Head.is_followed_by_substitution());
        assert!(TemplatePart::Middle.is_followed_by_substitution());
        assert!(!TemplatePart::NoSubstitution.is_followed_by_substitution());
        assert!(!TemplatePart::Tail.is_followed_by_substitution());
    }

    #[test]
    fn a_closing_brace_resumes_a_template_only_when_the_goal_says_so() {
        // §12.6: `TemplateSubstitutionTail` belongs to the `InputElementTemplateTail` goals
        // alone. Under any other, a `}` is the `RightBracePunctuator` — and it must be, or
        // `if (x) { } ` would swallow the rest of the file looking for a backtick.
        assert_eq!(kinds("}"), [TokenKind::RBrace, TokenKind::Eof]);
        assert_eq!(
            kinds("}`a`"),
            [
                TokenKind::RBrace,
                part(TemplatePart::NoSubstitution),
                TokenKind::Eof
            ]
        );
        assert_eq!(
            tail_kinds("}a`"),
            [part(TemplatePart::Tail), TokenKind::Eof]
        );
        // A backtick opens a template under every goal, since `Template` is a `CommonToken`.
        for goal in [
            Goal::Div,
            Goal::RegExp,
            Goal::TemplateTail,
            Goal::RegExpOrTemplateTail,
        ] {
            let mut lexer = Lexer::new("`a`");
            assert_eq!(
                lexer.next_token(goal).map(|t| t.kind),
                Ok(part(TemplatePart::NoSubstitution)),
                "under {goal:?}"
            );
        }
        // The combined goal answers both questions at once: a `/` opens a literal and a `}`
        // resumes a template.
        let mut lexer = Lexer::new("}a`");
        assert_eq!(
            lexer.next_token(Goal::RegExpOrTemplateTail).map(|t| t.kind),
            Ok(part(TemplatePart::Tail))
        );
        let mut lexer = Lexer::new("/a/");
        assert_eq!(
            lexer.next_token(Goal::RegExpOrTemplateTail).map(|t| t.kind),
            Ok(TokenKind::RegExp)
        );
    }

    #[test]
    fn a_dollar_sign_is_ordinary_unless_a_brace_follows_it() {
        // `TemplateCharacter :: $ [lookahead ≠ {]`, which is why prices need no escaping.
        assert_eq!(values("`costs $5`").0, Some("costs $5".to_string()));
        assert_eq!(values("`$`").0, Some("$".to_string()));
        assert_eq!(values("`$$`").0, Some("$$".to_string()));
        assert_eq!(
            kinds("`a$b`"),
            [part(TemplatePart::NoSubstitution), TokenKind::Eof]
        );
        // …and a `${` ends the component wherever it appears.
        assert_eq!(kinds("`a$${"), [part(TemplatePart::Head), TokenKind::Eof]);
        assert_eq!(values("`a$${").0, Some("a$".to_string()));
    }

    #[test]
    fn a_template_may_span_lines_and_both_values_normalize_the_terminators() {
        // Unlike a string literal, a template takes `LineTerminatorSequence` as an ordinary
        // `TemplateCharacter`. §12.9.6.2's Note: "<CR><LF> and <CR> LineTerminatorSequences are
        // normalized to <LF> for both TV and TRV. An explicit TemplateEscapeSequence is needed
        // to include a <CR>".
        assert_eq!(
            values("`a\nb`"),
            (Some("a\nb".to_string()), "a\nb".to_string())
        );
        assert_eq!(
            values("`a\rb`"),
            (Some("a\nb".to_string()), "a\nb".to_string())
        );
        assert_eq!(
            values("`a\r\nb`"),
            (Some("a\nb".to_string()), "a\nb".to_string())
        );
        // …one line feed, not two: the CRLF is one sequence.
        assert_eq!(values("`a\r\nb`").1.len(), 3);
        // <LS> and <PS> are terminators too, and are *not* normalized.
        assert_eq!(values("`a\u{2028}b`").0, Some("a\u{2028}b".to_string()));
        assert_eq!(values("`a\u{2029}b`").0, Some("a\u{2029}b".to_string()));
        // An escape is the only way to get a real carriage return into either value.
        assert_eq!(
            values("`a\\rb`"),
            (Some("a\rb".to_string()), "a\\rb".to_string())
        );
        // A newline inside the component does not set the flag on the token after it: the
        // template is one token, and the trivia that follows is what ASI looks at.
        let tokens = Lexer::new("`a\nb`;")
            .tokens(Goal::Div)
            .unwrap_or_else(|err| panic!("should lex, got {}", err.kind)); // the assertion needs the tokens
        assert!(!tokens[1].newline_before);
    }

    #[test]
    fn a_well_formed_escape_cooks_while_the_raw_value_keeps_it_as_written() {
        // The difference between TV and TRV, one escape at a time.
        for (source, cooked, raw) in [
            (r"`\n`", "\n", r"\n"),
            (r"`\t`", "\t", r"\t"),
            (r"`\\`", "\\", r"\\"),
            (r"`\``", "`", r"\`"),
            (r"`\x41`", "A", r"\x41"),
            (r"`A`", "A", "A"),
            (r"`\u{1f680}`", "\u{1f680}", r"\u{1f680}"),
            (r"`\u0041`", "A", r"\u0041"),
            (r"`\0`", "\0", r"\0"),
        ] {
            assert_eq!(
                values(source),
                (Some(cooked.to_string()), raw.to_string()),
                "values of {source:?}"
            );
        }
        // Exactly two hex digits, no more: a third is ordinary text, so `\\x414` is `A4`
        // and not U+0414. The raw value cannot see this difference; only the cooked one can.
        assert_eq!(values(r"`\x414`").0, Some("A4".to_string()));
        assert_eq!(values(r"`\x41`").0, Some("A".to_string()));
        // A `NonEscapeCharacter` stands for itself and is perfectly well formed — `\q` is `q`.
        assert_eq!(values(r"`\q`"), (Some("q".to_string()), r"\q".to_string()));
        // A line continuation vanishes from the cooked value and stays in the raw one.
        assert_eq!(
            values("`a\\\nb`"),
            (Some("ab".to_string()), "a\\\nb".to_string())
        );
        assert_eq!(
            values("`a\\\r\nb`").1,
            "a\\\nb",
            "the CRLF normalizes inside it too"
        );
        // …and the cooked value is where a continuation that ate only the carriage return
        // shows up: `LineTerminatorSequence` takes CRLF as ONE, so the line feed goes with
        // it and contributes nothing. Leave the feed behind and it becomes ordinary text.
        assert_eq!(values("`a\\\r\nb`").0, Some("ab".to_string()));
        // The converse: a continuation on a bare line feed must not reach past it, so the
        // second terminator here is text and survives into the cooked value.
        assert_eq!(values("`a\\\n\nb`").0, Some("a\nb".to_string()));
        assert_eq!(values("`a\\\r\rb`").0, Some("a\nb".to_string()));
        assert_eq!(values("`a\\\n\rb`").0, Some("a\nb".to_string()));
    }

    #[test]
    fn a_not_escape_sequence_leaves_the_token_intact_and_the_cooked_value_undefined() {
        // The 2018 revision. Each of these is a `NotEscapeSequence` shape from §12.9.6, and each
        // one lexes: the component is a token, its TRV is what was written, and only its TV is
        // undefined. Refusing them here would break every tagged template that uses a
        // domain-specific escape syntax, which is what the revision existed to allow.
        for source in [
            r"`\u`",
            r"`\u1`",
            r"`\u12`",
            r"`\u123`",
            r"`\unicode`",
            r"`\u{}`",
            r"`\u{110000}`",
            r"`\u{1`",
            r"`\x`",
            r"`\xg`",
            r"`\x1`",
            r"`\x1g`",
            r"`\1`",
            r"`\7`",
            r"`\9`",
            r"`\00`",
            r"`\08`",
        ] {
            let token = first(source);
            assert_eq!(
                token.kind,
                TokenKind::Template {
                    part: TemplatePart::NoSubstitution,
                    cooked_undefined: true,
                },
                "kind of {source:?}"
            );
            let value = template_value(source, token.span)
                .unwrap_or_else(|| panic!("{source:?} should still have a raw value")); // the assertion under test needs the value
            assert_eq!(value.cooked, None, "cooked of {source:?}");
            assert!(!value.raw.is_empty(), "raw of {source:?}");
        }
        // A template has no legacy octal escapes at all — this is where it differs from a string
        // literal, where `"\7"` is merely Annex B and cooks to a code unit.
        assert_eq!(values(r"`\7`").0, None);
        assert_eq!(values(r"`\7`").1, r"\7");
        assert_eq!(kinds(r#""\7""#), [LEGACY_STRING, TokenKind::Eof]);
        // …but `\0` alone is a real escape in both.
        assert_eq!(values(r"`\0`").0, Some("\0".to_string()));
        // One bad escape anywhere is enough, and the rest of the component still scans.
        assert_eq!(values(r"`ok \u bad`").0, None);
        assert_eq!(values(r"`ok \u bad`").1, r"ok \u bad");
        // The extents §12.9.6's lookaheads describe: a bad escape swallows only what could have
        // been one, and the rest is ordinary text.
        assert_eq!(values(r"`\xg`").1, r"\xg");
        assert_eq!(values(r"`\x1g`").1, r"\x1g");
    }

    #[test]
    fn an_unterminated_template_is_the_only_way_one_can_fail() {
        for source in ["`", "`abc", "`abc$", r"`abc\", "`a${", "`\\"] {
            let result = Lexer::new(source).tokens(Goal::Div);
            if source == "`a${" {
                // …except this one, which is a perfectly good TemplateHead.
                assert!(result.is_ok(), "{source:?} is a head, not an error");
                continue;
            }
            assert_eq!(
                result.map(|t| t.len()),
                Err(LexError {
                    kind: LexErrorKind::UnterminatedTemplate,
                    span: Span::new(0, source.len() as u32),
                }),
                "on {source:?}"
            );
        }
        // A `}` component can be unterminated in exactly the same way.
        assert_eq!(
            Lexer::new("}abc")
                .tokens(Goal::TemplateTail)
                .map(|t| t.len()),
            Err(LexError {
                kind: LexErrorKind::UnterminatedTemplate,
                span: Span::new(0, 4),
            })
        );
        // A template runs across lines, so an unterminated one really does reach the end of the
        // source rather than stopping at the newline a string would.
        assert_eq!(
            Lexer::new("`abc\ndef").tokens(Goal::Div).map(|t| t.len()),
            Err(LexError {
                kind: LexErrorKind::UnterminatedTemplate,
                span: Span::new(0, 8),
            })
        );
    }

    #[test]
    fn template_value_answers_rather_than_panicking_on_a_span_it_was_not_given() {
        assert_eq!(template_value("`a`", Span::new(0, 99)), None);
        assert_eq!(template_value("`\u{e9}`", Span::new(0, 2)), None);
        assert_eq!(template_value("abc", Span::new(0, 3)), None);
        assert_eq!(template_value("`", Span::new(0, 1)), None);
        assert_eq!(template_value("}", Span::new(0, 1)), None);
        assert_eq!(template_value("", Span::empty_at(0)), None);
        // Opens like a component but never closes.
        assert_eq!(template_value("`abc", Span::new(0, 4)), None);
        // A valid span that does not start at zero.
        let source = "x = `hi`";
        let value = template_value(source, Span::new(4, 8)).expect("a component"); // the assertion under test needs the value
        assert_eq!(value.cooked, Some(vec![0x68, 0x69]));
    }

    #[test]
    fn no_template_however_odd_can_panic() {
        // DR-0002. Backslashes and `$` against the end of input are what a fuzzer finds first,
        // and a component made entirely of ill-formed escapes exercises the path where the
        // cooked value is discarded.
        let cases = [
            "`".to_string(),
            r"`\".to_string(),
            r"`\u".to_string(),
            r"`\u{".to_string(),
            r"`\x".to_string(),
            "`$".to_string(),
            "`${".to_string(),
            "`\r".to_string(),
            "`\r\n`".to_string(),
            format!("`{}`", r"\u".repeat(2000)),
            format!("`{}`", "$".repeat(5000)),
            format!("`{}`", "\r\n".repeat(2000)),
            format!("`{}`", r"\\".repeat(2000)),
        ];
        for source in &cases {
            if let Ok(tokens) = Lexer::new(source).tokens(Goal::Div) {
                assert!(
                    template_value(source, tokens[0].span).is_some(),
                    "{:?} lexed but has no values",
                    &source[..source.len().min(12)]
                );
            }
        }
        // Two thousand CRLF pairs are two thousand line feeds, in both values.
        let many = format!("`{}`", "\r\n".repeat(2000));
        assert_eq!(values(&many).1.len(), 2000);
        assert_eq!(values(&many).0.map(|c| c.len()), Some(2000));
    }
}
