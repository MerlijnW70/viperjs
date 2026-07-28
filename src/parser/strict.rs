//! Strict mode: where it starts, and what it forbids (ECMAScript §11.2.1, §13.1.1).
//!
//! # A Directive Prologue is a run of string literals and stops at the first thing that is not one
//!
//! §11.2.1: "the longest sequence of `ExpressionStatement`s occurring as the initial
//! `StatementListItem`s of a `FunctionBody`, a `ScriptBody`, or a `ModuleBody`… Each
//! `ExpressionStatement` in a Directive Prologue must consist entirely of a `StringLiteral` token
//! followed by a semicolon." So `a; "use strict";` is not strict — the prologue ended at `a`.
//!
//! And: "A Use Strict Directive may not contain an `EscapeSequence` or `LineContinuation`." That
//! is why the test below reads the *source text* rather than the string's value:
//! `"use strict"` denotes the same string and is not the directive.
//!
//! # Strictness is inherited and never given back
//!
//! Code inside strict code is strict. A function with its own directive is strict whatever
//! encloses it; a function inside a strict script is strict without one. What a body cannot do is
//! turn it *off* — so the flag is saved and restored around a function body, and the restoring
//! only ever matters on the way out of the outermost strict thing.
//!
//! # Where the rules are applied, and why not all in one place
//!
//! Most are applied as the offending token is read, because by then the answer is known: a
//! prologue is the first thing in a body, so everything after it knows. Two are not:
//!
//! - A function's parameters are read *before* its body, so a `"use strict"` inside can make them
//!   retroactively illegal. §15.2.1 states those as early errors on the whole function, and they
//!   are checked once the body is in.
//! - `IsSimpleParameterList` and a `"use strict"` body are incompatible for the same reason from
//!   the other side: the parameters of a non-simple list are initialised by running code, and
//!   that code would have to be told a strictness the directive has not announced yet.
//!
//! # What is not here: Annex B
//!
//! Several rules read "…unless the host is a web browser or otherwise supports X". praxis is not
//! a web browser, and DR-0008 has the argument — the short version being that Annex B.1's
//! *lexical* extensions are implemented, because a token is a token, and B.3's *syntactic* ones
//! are not.

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{ExprKind, Stmt, StmtKind};
use crate::span::Span;

/// The names §13.1.1 takes away in strict code.
///
/// `Identifier : IdentifierName but not ReservedWord` — "It is a Syntax Error if this phrase is
/// contained in strict mode code and the StringValue of IdentifierName is: `implements`,
/// `interface`, `let`, `package`, `private`, `protected`, `public`, `static`, or `yield`." They
/// are the words a future edition wanted room for, kept available to the sloppy code that was
/// already using them.
const STRICT_RESERVED: [&str; 9] = [
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
];

impl Parser<'_> {
    /// Whether `stmt` is a Directive, and the text it was written with.
    ///
    /// The source text, not the value: §11.2.1 refuses an `EscapeSequence`, so the directive is
    /// the eleven characters and not the string they denote.
    fn directive_text(&self, stmt: &Stmt) -> Option<&str> {
        let StmtKind::Expression(expr) = &stmt.kind else {
            return None;
        };
        if expr.parenthesized || !matches!(expr.kind, ExprKind::String(_)) {
            return None;
        }
        expr.span.slice(self.source)
    }

    /// A `ScriptBody` or `FunctionBody`, with its Directive Prologue read first.
    ///
    /// The prologue has to be read as it goes rather than looked at afterwards, because a
    /// `"use strict"` changes how everything after it parses — and nothing before it, the
    /// prologue being string literals all the way down.
    pub(super) fn parse_body_with_prologue(
        &mut self,
        terminator: crate::lexer::TokenKind,
    ) -> Result<(Box<[Stmt]>, bool), ParseError> {
        let mut body = Vec::new();
        let mut in_prologue = true;
        let mut declares_strict = false;
        while self.current.kind != terminator && self.current.kind != crate::lexer::TokenKind::Eof {
            let stmt = self.parse_statement_list_item()?;
            if in_prologue {
                match self.directive_text(&stmt) {
                    Some(text) => {
                        if text == "\"use strict\"" || text == "'use strict'" {
                            self.strict = true;
                            declares_strict = true;
                        }
                    }
                    None => in_prologue = false,
                }
            }
            body.push(stmt);
        }
        Ok((body.into_boxed_slice(), declares_strict))
    }

    /// §13.1.1 and §13.15.1, asked of a name that a *pattern* binds or assigns to.
    ///
    /// The plain sites ask for themselves: `var a` goes through `parse_binding_identifier` and
    /// `a = 1` through the assignment level. A name reached through a pattern passes through
    /// neither — it is read as an ordinary reference first, and only a refinement several
    /// tokens later says it was a target. So every place that turns a name into one asks here.
    ///
    /// Reading is not binding, which is why an object literal's shorthand asks with `false`
    /// where it is read and this asks again once it is known to be a target. `"use strict";
    /// ({eval});` is legal and `"use strict"; ({eval} = x);` is not, from the same four
    /// characters.
    pub(super) fn check_target_name(&self, name: &str, span: Span) -> Result<(), ParseError> {
        self.check_strict_name(name, span, true)
    }

    /// §13.1.1, asked of every `Identifier`, `BindingIdentifier` and `LabelIdentifier`.
    ///
    /// Three rules, and the first applies to sloppy code too:
    ///
    /// - **`Identifier : IdentifierName but not ReservedWord`**, asked of the `StringValue`.
    ///   Only an escape can fail it, §12.7.2 Note 1 having already kept `br\u0065ak` from
    ///   lexing as the token `break` — but the value is still `"break"` and the value is what
    ///   the rule asks about. `yield` and `await` are exempt here because each has a rule of its
    ///   own below; every other reserved word is refused outright.
    /// - **`yield`** is refused where `[Yield]` is set, and in strict code, which the list below
    ///   already covers.
    /// - **`await`** is refused where `[Await]` is set, and everywhere in a module: §13.1.1
    ///   states that one without a parameter, so a non-async function inside a module cannot
    ///   have a variable called `await` either.
    ///
    /// `eval` and `arguments` are refused only where something is *bound* — `eval` may be read
    /// and called in strict code, and only assigning to it or declaring it is refused — so the
    /// caller says which question it is asking.
    ///
    /// A property name never asks: `x.br\u0065ak` and `({br\u0065ak: 1})` are an
    /// `IdentifierName`, which this rule is the *exception* to rather than a case of.
    pub(super) fn check_strict_name(
        &self,
        name: &str,
        span: Span,
        binding: bool,
    ) -> Result<(), ParseError> {
        if let Some(word) = crate::lexer::ReservedWord::from_text(name) {
            let refused = match word {
                crate::lexer::ReservedWord::Yield => self.yield_allowed,
                crate::lexer::ReservedWord::Await => self.await_allowed || self.module,
                _ => true,
            };
            if refused {
                return Err(ParseError {
                    kind: ParseErrorKind::EscapedReservedWord,
                    span,
                });
            }
        }
        if !self.strict {
            return Ok(());
        }
        if STRICT_RESERVED.contains(&name) {
            return Err(ParseError {
                kind: ParseErrorKind::StrictReservedWord,
                span,
            });
        }
        if binding && (name == "eval" || name == "arguments") {
            return Err(ParseError {
                kind: ParseErrorKind::StrictEvalOrArguments,
                span,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    /// The kind of error `source` fails with.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
    }

    /// The kind of error `source` fails with, read under the Module goal.
    fn module_kind(source: &str) -> ParseErrorKind {
        match crate::parser::parse_module(source) {
            Err(error) => error.kind,
            Ok(module) => panic!("{source:?} should not parse, got {module:?}"), // a test about an error needs one
        }
    }

    #[test]
    fn a_label_and_a_shorthand_read_a_name_rather_than_binding_one() {
        // §13.1.1 refuses `eval` and `arguments` in strict code only where one is *bound* or
        // assigned to. A `LabelIdentifier` is neither — it names a statement, not a variable —
        // and a shorthand property is an `IdentifierReference`, which is a read. So all of
        // these are legal in strict code, and each one is a different place that had to ask
        // the question the right way round.
        assert!(parse_script(r#""use strict"; eval: ;"#).is_ok());
        assert!(parse_script(r#""use strict"; arguments: ;"#).is_ok());
        assert!(parse_script(r#""use strict"; eval: while (x) { break eval; }"#).is_ok());
        assert!(parse_script(r#""use strict"; ({eval});"#).is_ok());
        assert!(parse_script(r#""use strict"; ({arguments});"#).is_ok());
        // …while binding one is refused, which is the rule those three must not have copied.
        assert_eq!(
            kind(r#""use strict"; var eval;"#),
            ParseErrorKind::StrictEvalOrArguments
        );
    }

    #[test]
    fn a_reserved_word_spelled_with_an_escape_is_still_a_reserved_word() {
        // §13.1.1: `Identifier : IdentifierName but not ReservedWord`, asked of the *StringValue*.
        // §12.7.2 Note 1 already keeps `br\u0065ak` from lexing as the token `break` — a keyword
        // matches literal characters — so this is the only thing standing between that spelling
        // and a program using `break` as a variable.
        assert_eq!(
            kind(r"var br\u0065ak;"),
            ParseErrorKind::EscapedReservedWord
        );
        assert_eq!(kind(r"br\u0065ak;"), ParseErrorKind::EscapedReservedWord);
        assert_eq!(kind(r"th\u0069s;"), ParseErrorKind::EscapedReservedWord);
        assert_eq!(kind(r"tr\u0075e;"), ParseErrorKind::EscapedReservedWord);
        assert_eq!(kind(r"var n\u0075ll;"), ParseErrorKind::EscapedReservedWord);
        assert_eq!(
            kind(r"function f(){ r\u0065turn; }"),
            ParseErrorKind::EscapedReservedWord
        );
        // A `LabelIdentifier` is an `Identifier`, both where one is declared and where one is
        // named. Neither asked before.
        assert_eq!(kind(r"n\u0065w: ;"), ParseErrorKind::EscapedReservedWord);
        // The label a `break` names is read somewhere else again, and without the rule
        // this one would be reported as an undeclared label rather than as the name it
        // may not be.
        assert_eq!(
            kind(r"while (x) { break n\u0065w; }"),
            ParseErrorKind::EscapedReservedWord
        );
        // Shorthand is the one place a name is both things: as a `PropertyName` any
        // `IdentifierName` will do, and written as shorthand it has to be a name a program
        // could read.
        assert_eq!(
            kind(r"var x = { bre\u0061k } = { break: 42 };"),
            ParseErrorKind::EscapedReservedWord
        );
        // …which is why these two stay legal. A property name is what the rule is the
        // *exception* to, not a case of it.
        assert!(parse_script(r"x.br\u0065ak;").is_ok());
        assert!(parse_script(r"var o = { br\u0065ak: 1 };").is_ok());
        assert!(parse_script(r"class C { br\u0065ak() {} }").is_ok());
        // `let` is not a `ReservedWord` at all — it is contextual, and sloppy code may use it.
        assert!(parse_script(r"var l\u0065t = 1;").is_ok());
    }

    #[test]
    fn an_escaped_yield_or_await_is_refused_exactly_where_the_plain_spelling_is() {
        // Both are exempt from the blanket rule because each has one of its own, parameterised:
        // §13.1.1 refuses `yield` under `[Yield]` and `await` under `[Await]`. Writing either
        // with an escape changes nothing, which is the whole point of the rule being about the
        // `StringValue`.
        assert!(parse_script(r"var \u0079ield;").is_ok());
        assert!(parse_script(r"function f() { var \u0079ield; }").is_ok());
        assert_eq!(
            kind(r"function* g() { var \u0079ield; }"),
            ParseErrorKind::EscapedReservedWord
        );
        assert_eq!(
            kind(r"function* g() { \u0079ield: ; }"),
            ParseErrorKind::EscapedReservedWord
        );
        // Strict code reserves `yield` however it is spelled, and that rule was already here.
        assert_eq!(
            kind(r#""use strict"; var \u0079ield;"#),
            ParseErrorKind::StrictReservedWord
        );

        assert!(parse_script(r"var \u0061wait;").is_ok());
        assert!(parse_script(r"function f() { var \u0061wait; }").is_ok());
        assert_eq!(
            kind(r"async function f() { var \u0061wait; }"),
            ParseErrorKind::EscapedReservedWord
        );
        // A module refuses it with no parameter at all, so even a plain function inside one may
        // not have a variable called `await`.
        assert_eq!(
            module_kind(r"function f() { var \u0061wait; }"),
            ParseErrorKind::EscapedReservedWord
        );
        assert_eq!(
            module_kind(r#"import { x as \u0061wait } from "m";"#),
            ParseErrorKind::EscapedReservedWord
        );
        // `ModuleExportName` is an `IdentifierName`, so the rule does not reach it.
        assert!(crate::parser::parse_module(r#"var x; export { x as br\u0065ak };"#).is_ok());
    }

    #[test]
    fn a_directive_prologue_is_the_leading_run_of_string_literals_and_nothing_else() {
        // §11.2.1. The prologue is the *initial* sequence, so anything that is not a string
        // literal ends it — and everything after that is sloppy however it is spelled.
        assert!(parse_script("\"use strict\"; with (a) {}").is_err());
        assert!(parse_script("'use strict'; with (a) {}").is_err());
        assert!(parse_script("\"other\"; \"use strict\"; with (a) {}").is_err());
        assert!(parse_script("a; \"use strict\"; with (a) {}").is_ok());
        assert!(parse_script("; \"use strict\"; with (a) {}").is_ok());
        assert!(parse_script("\"use strict\" + 1; with (a) {}").is_ok());
        assert!(parse_script("(\"use strict\"); with (a) {}").is_ok());
        assert!(parse_script("with (a) {}").is_ok());
        // "A Use Strict Directive may not contain an EscapeSequence or LineContinuation", which
        // is why the directive is recognised by its *source text* and not by its value — the two
        // denote the same string and only one of them is the directive.
        assert!(parse_script(r#""use\u0020strict"; with (a) {}"#).is_ok());
        assert!(parse_script("\"use\\\nstrict\"; with (a) {}").is_ok());
    }

    #[test]
    fn strictness_is_inherited_and_a_body_may_only_switch_it_on() {
        // A function inside strict code is strict without a directive of its own.
        assert!(parse_script("\"use strict\"; function f() { with (a) {} }").is_err());
        assert!(
            parse_script("\"use strict\"; function f() { function g() { with (a) {} } }").is_err()
        );
        // …and one with its own directive is strict whatever encloses it.
        assert!(parse_script("function f() { \"use strict\"; with (a) {} }").is_err());
        assert!(
            parse_script("function f() { function g() { \"use strict\"; with (a) {} } }").is_err()
        );
        // What a body cannot do is give it back, so the restoring only ever matters on the way
        // out of the outermost strict thing.
        assert!(parse_script("function f() { \"use strict\"; } with (a) {}").is_ok());
        assert!(parse_script("function f() { \"use strict\"; } var let = 1;").is_ok());
        // …and it is restored even when the body fails.
        assert!(parse_script("function f() { \"use strict\"; @ }").is_err());
        assert!(parse_script("with (a) {}").is_ok());
    }

    #[test]
    fn strict_code_keeps_nine_names_for_itself_and_refuses_two_more_as_bindings() {
        // §13.1.1's list — the words a future edition wanted room for, left available to the
        // sloppy code that was already using them.
        for name in [
            "implements",
            "interface",
            "let",
            "package",
            "private",
            "protected",
            "public",
            "static",
            "yield",
        ] {
            assert_eq!(
                kind(&format!("\"use strict\"; var {name} = 1;")),
                ParseErrorKind::StrictReservedWord,
                "{name} as a binding"
            );
            assert_eq!(
                kind(&format!("\"use strict\"; {name};")),
                ParseErrorKind::StrictReservedWord,
                "{name} as a reference"
            );
            assert!(
                parse_script(&format!("var {name} = 1;")).is_ok(),
                "{name} is sloppy code's to use"
            );
        }
        // `eval` and `arguments` are different: reading one is fine, and only binding or
        // assigning to it is refused. That is §8.6.4 rather than §13.1.1 — their
        // `AssignmentTargetType` is *invalid* in strict code, where their value is not.
        assert!(parse_script("\"use strict\"; eval;").is_ok());
        assert!(parse_script("\"use strict\"; eval(1);").is_ok());
        assert!(parse_script("\"use strict\"; a.eval;").is_ok());
        for name in ["eval", "arguments"] {
            assert_eq!(
                kind(&format!("\"use strict\"; var {name} = 1;")),
                ParseErrorKind::StrictEvalOrArguments
            );
            assert_eq!(
                kind(&format!("\"use strict\"; {name} = 1;")),
                ParseErrorKind::StrictEvalOrArguments
            );
            assert_eq!(
                kind(&format!("\"use strict\"; function f() {{ var {name}; }}")),
                ParseErrorKind::StrictEvalOrArguments
            );
            assert!(parse_script(&format!("var {name} = 1;")).is_ok());
        }
    }

    #[test]
    fn strict_code_loses_with_the_bare_delete_and_the_two_legacy_literals() {
        // §14.11.1 — the one statement strict mode removes outright, `with` being what makes the
        // scope of a name undecidable until run time.
        assert_eq!(
            kind("\"use strict\"; with (a) {}"),
            ParseErrorKind::StrictWith
        );
        assert!(parse_script("with (a) {}").is_ok());
        // §13.5.1: `delete a.b` removes a property and `delete a` asks to remove a binding.
        // Parentheses do not help — `(a)` is the same identifier, bracketing being a flag.
        assert_eq!(
            kind("\"use strict\"; delete a;"),
            ParseErrorKind::StrictDeleteOfName
        );
        assert_eq!(
            kind("\"use strict\"; delete (a);"),
            ParseErrorKind::StrictDeleteOfName
        );
        assert!(parse_script("\"use strict\"; delete a.b;").is_ok());
        assert!(parse_script("\"use strict\"; delete a[0];").is_ok());
        assert!(parse_script("delete a;").is_ok());
        // Annex B.1.1's two legacy numeric forms, which §12.9.3.1 refuses in strict code. The
        // lexer reads them and flags them, that being the lexical grammar's business.
        assert_eq!(
            kind("\"use strict\"; 010;"),
            ParseErrorKind::StrictLegacyOctal
        );
        assert_eq!(
            kind("\"use strict\"; 08;"),
            ParseErrorKind::StrictLegacyOctal
        );
        assert_eq!(
            kind("function f() { \"use strict\"; 010; }"),
            ParseErrorKind::StrictLegacyOctal
        );
        assert!(parse_script("010;").is_ok());
        assert!(parse_script("\"use strict\"; 10;").is_ok());
        assert!(parse_script("\"use strict\"; 0o10;").is_ok());
    }

    #[test]
    fn the_two_rules_a_functions_parameters_cannot_be_judged_by_until_its_body_is_read() {
        // §15.2.1. The parameters are read first, so a `"use strict"` inside makes them
        // retroactively illegal — which is why these are early errors on the whole function.
        assert_eq!(
            kind("function f(a, a) { \"use strict\"; }"),
            ParseErrorKind::DuplicateParameterName,
            "a strict list is unique whether or not it is simple"
        );
        assert_eq!(
            kind("\"use strict\"; function f(a, a) {}"),
            ParseErrorKind::DuplicateParameterName
        );
        assert_eq!(
            kind("function f(eval) { \"use strict\"; }"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("\"use strict\"; function f(arguments) {}"),
            ParseErrorKind::StrictEvalOrArguments
        );
        // …and the same thing from the other side: a non-simple list is initialised by running
        // code, which would have to be told a strictness the directive has not announced yet.
        assert_eq!(
            kind("function f(a = 1) { \"use strict\"; }"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        assert_eq!(
            kind("function f([a]) { \"use strict\"; }"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        assert_eq!(
            kind("function f(...a) { \"use strict\"; }"),
            ParseErrorKind::UseStrictWithNonSimpleParameters
        );
        // …while a non-simple list in code that was already strict is perfectly fine, the
        // strictness having been announced before the parameters were read.
        assert!(parse_script("\"use strict\"; function f(a = 1) {}").is_ok());
        assert!(parse_script("\"use strict\"; function f(a, [b]) {}").is_ok());
        // …and a simple list may still repeat in sloppy code, which is the baseline.
        assert!(parse_script("function f(a, a) {}").is_ok());
        assert!(parse_script("function f(a, a) { \"other\"; }").is_ok());
    }

    #[test]
    fn no_strict_source_however_odd_can_panic() {
        let cases = [
            "\"use strict\"".to_string(),
            "\"use strict\";".to_string(),
            "\"use strict\"; function f(".to_string(),
            "\"use strict\"; ".repeat(1000),
            "function f() { \"use strict\"; ".repeat(200),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A directive repeated is still a directive, and the prologue is still a flat list.
        assert!(parse_script(&"\"use strict\"; ".repeat(1000)).is_ok());
    }
}
