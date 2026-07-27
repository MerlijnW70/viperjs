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

    /// §13.1.1: a name that strict code may not bind or reference.
    ///
    /// `eval` and `arguments` are refused only where something is *bound* — `eval` may be read
    /// and called in strict code, and only assigning to it or declaring it is refused — so the
    /// caller says which question it is asking.
    pub(super) fn check_strict_name(
        &self,
        name: &str,
        span: Span,
        binding: bool,
    ) -> Result<(), ParseError> {
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
