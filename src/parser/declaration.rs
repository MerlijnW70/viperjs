//! `var`, `let` and `const` (ECMAScript §14.3).
//!
//! # Telling a `let` declaration from an identifier called `let`
//!
//! `let` is not a reserved word — §12.7.2 puts it among the names "always allowed as
//! identifiers, but also keywords within certain syntactic productions" — so `let = 1` and
//! `let.a = 2` are ordinary assignments, and `let x = 1` is a declaration. Nothing shorter than
//! looking at the token after it decides which, which is the one place this parser needs two
//! tokens of lookahead.
//!
//! Two details make the test narrower than it first appears. `LetOrConst : let` is a *terminal*,
//! and §5.1.5.1 says terminals match literal source characters — so `let x = 1` is not a
//! declaration at all, however much it looks like one, and the token's `contains_escape` flag is
//! what says so. And the early error about the *name* `let` is about `BoundNames`, which is a
//! StringValue — so `let let = 1` **is** refused, because the escape is in the name rather
//! than in the keyword. The two rules read the same word in opposite ways, and the lexer has
//! carried the flag needed for both since its identifier slice.

use super::expression::AllowIn;
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Binding, Declaration, DeclarationKind, Declarator, Stmt, StmtKind};
use crate::lexer::{Goal, TokenKind, identifier_value};
use crate::span::Span;
use crate::static_semantics::bound_names;
use std::collections::HashSet;

impl Parser<'_> {
    /// Whether a `LexicalDeclaration` starting with `let` begins here.
    ///
    /// True only when what follows could begin a binding: a name, or one of the two destructuring
    /// patterns. That is also §14.5's lookahead restriction from the other side — an
    /// `ExpressionStatement` may not begin with `let [`, and this is what stops one from trying.
    pub(super) fn at_lexical_let(&self) -> Result<bool, ParseError> {
        if !self.at_contextual("let") {
            return Ok(false);
        }
        // Read under `Div`, which is what would follow `let` if it were an identifier. The three
        // tokens this looks for mean the same under either goal, so the choice cannot mislead.
        let next = self.peek(Goal::Div)?;
        // A name or one of the two patterns. `is_identifier_token` rather than a plain
        // `Identifier`, because §13.1 lets `yield` and `await` be bound here too — so
        // `let await = 1;` is a declaration and not two expressions.
        Ok(super::is_identifier_token(next.kind)
            || matches!(next.kind, TokenKind::LBracket | TokenKind::LBrace))
    }

    /// `VariableStatement` or `LexicalDeclaration` (§14.3), with the cursor on the keyword.
    ///
    /// The statement forms, which is to say the ones whose terminating semicolon automatic
    /// semicolon insertion is allowed to supply. A `for` head takes the list without it — see
    /// [`Parser::parse_declarator_list`].
    pub(super) fn parse_declaration(&mut self, kind: DeclarationKind) -> Result<Stmt, ParseError> {
        let (declaration, span) = self.parse_declarator_list(kind, AllowIn::Yes)?;
        Self::check_declaration_initializers(&declaration)?;
        let end = self.consume_semicolon(span)?;
        Ok(Stmt {
            span: span.to(end),
            kind: StmtKind::Declaration(Box::new(declaration)),
        })
    }

    /// The keyword and its `BindingList`, stopping before any semicolon.
    ///
    /// Two things make this worth having apart. A `for` head takes the same list —
    /// `for (var a, b; …)`, `for (let a; …)` — but the semicolon after it belongs to the header,
    /// and §12.10 forbids inserting one there, so the header must ask for a real `;` rather than
    /// accept whatever `consume_semicolon` would allow. And the list is `[~In]` in a header,
    /// because `Initializer[?In]` propagates: `for (var a = b in c;;)` has no derivation.
    pub(super) fn parse_declarator_list(
        &mut self,
        kind: DeclarationKind,
        allow_in: AllowIn,
    ) -> Result<(Declaration, Span), ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let mut declarators: Vec<Declarator> = Vec::new();
        let mut bound: HashSet<String> = HashSet::new();
        // The list is never empty, so the loop always sets this before it is read.
        let mut end;
        loop {
            let declarator = self.parse_declarator(allow_in)?;
            // §14.3.1.1: the BoundNames of a lexical BindingList may not repeat, and may not
            // contain `let`. Both are about `BoundNames`, which a pattern makes a list of — so
            // `let [a, a] = b` is a redeclaration and `var [a, a] = b` is not, exactly as
            // `let a, a;` and `var a, a;` are.
            if kind.is_lexical() {
                for declared in bound_names(&declarator.binding) {
                    if declared.name == "let" {
                        return Err(ParseError {
                            kind: ParseErrorKind::LetAsLexicalBindingName,
                            span: declared.span,
                        });
                    }
                    if !bound.insert(declared.name.to_string()) {
                        return Err(ParseError {
                            kind: ParseErrorKind::DuplicateLexicalBinding,
                            span: declared.span,
                        });
                    }
                }
            }
            end = declarator.span;
            declarators.push(declarator);
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
        }
        Ok((
            Declaration {
                kind,
                declarators: declarators.into_boxed_slice(),
            },
            keyword.span.to(end),
        ))
    }

    /// One `LexicalBinding` or `VariableDeclaration` (§14.3).
    ///
    /// Takes no `DeclarationKind`, and did until patterns arrived. Both rules that needed one —
    /// no `let` among the bound names, and no repeats — are about `BoundNames`, which a pattern
    /// makes a list of; so both moved up to the list, where the list is.
    fn parse_declarator(&mut self, allow_in: AllowIn) -> Result<Declarator, ParseError> {
        let binding = self.parse_binding()?;
        if self.current.kind != TokenKind::Eq {
            return Ok(Declarator {
                span: binding.span(),
                binding,
                initializer: None,
            });
        }
        self.advance(Goal::RegExp)?;
        self.enter()?;
        // `Initializer : = AssignmentExpression` — an assignment expression and not an
        // `Expression`, so a comma separates declarators rather than sequencing values.
        // `Initializer[?In]` propagates the parameter, which is the whole reason
        // `for (var a = b in c;;)` has no derivation.
        let initializer = self.parse_assignment(allow_in);
        self.leave();
        let initializer = initializer?;
        Ok(Declarator {
            span: binding.span().to(initializer.span),
            binding,
            initializer: Some(Box::new(initializer)),
        })
    }

    /// The two rules about a declarator that must have an initialiser.
    ///
    /// - §14.3.1.1: a `const` binding has nothing to be constant without one, and no later
    ///   statement may supply it.
    /// - §14.3.3: `BindingPattern Initializer` — the `_opt` is on the `BindingIdentifier`
    ///   alternative and not on this one, so a pattern has nothing to take apart. Unlike the
    ///   `const` rule this holds for all three keywords: `var [a];` is refused too.
    ///
    /// Applied by the caller rather than while the binding is read, for the reason the `let` rule
    /// is: both belong to a `LexicalDeclaration` or a `VariableDeclaration`, and a `ForDeclaration`
    /// is neither. `for (const [a] of b)` takes its value from the iteration, `ForBinding` having
    /// no `Initializer` in the grammar at all — so applying either there would refuse the
    /// ordinary form.
    pub(super) fn check_declaration_initializers(
        declaration: &Declaration,
    ) -> Result<(), ParseError> {
        for declarator in &declaration.declarators {
            if declarator.initializer.is_some() {
                continue;
            }
            let kind = match declarator.binding {
                Binding::Pattern(_) => ParseErrorKind::PatternWithoutInitializer,
                Binding::Identifier(_) if declaration.kind == DeclarationKind::Const => {
                    ParseErrorKind::ConstWithoutInitializer
                }
                Binding::Identifier(_) => continue,
            };
            return Err(ParseError {
                kind,
                span: declarator.binding.span(),
            });
        }
        Ok(())
    }

    /// `BindingIdentifier` (§13.1), for the names this parser can bind.
    ///
    /// Only an `Identifier` token will do, which rules out every reserved word: `var if` has no
    /// derivation. The contextual keywords are identifiers to the lexer and so are accepted
    /// here, which is right — `var async = 1` and `var of = 2` are ordinary declarations.
    ///
    /// The name comes back as its `StringValue`, escapes resolved. No early error is applied:
    /// every rule about which names may be bound belongs to a particular production — §14.3.1.1
    /// forbids `let` to a lexical declaration and to nothing else — so applying one here would
    /// impose it on every caller, including the ones the specification exempts.
    pub(super) fn parse_binding_identifier(&mut self) -> Result<(Box<str>, Span), ParseError> {
        let token = self.current;
        if !super::is_identifier_token(token.kind) {
            return Err(self.unexpected("a binding name"));
        }
        self.advance(Goal::Div)?;
        let name =
            identifier_value(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
        Ok((name.into_owned().into_boxed_str(), token.span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_script;
    use crate::parser::test_support::*;

    /// The statements of `source`, rendered compactly.
    fn statements(source: &str) -> Vec<String> {
        let script = parse_script(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)); // a test about a tree cannot proceed without one
        script.body.iter().map(render_statement).collect()
    }

    /// The error `source` fails with.
    fn script_error(source: &str) -> ParseError {
        match parse_script(source) {
            Err(err) => err,
            Ok(script) => panic!("{source:?} should not parse, got {script:?}"), // a test about an error cannot proceed without one
        }
    }

    #[test]
    fn all_three_keywords_bind_names_with_or_without_initialisers() {
        assert_eq!(statements("var a;"), ["(var a)"]);
        assert_eq!(statements("var a = 1;"), ["(var a=1)"]);
        assert_eq!(statements("let a;"), ["(let a)"]);
        assert_eq!(statements("let a = 1;"), ["(let a=1)"]);
        assert_eq!(statements("const a = 1;"), ["(const a=1)"]);
        assert_eq!(statements("var a, b, c;"), ["(var a b c)"]);
        assert_eq!(statements("var a = 1, b, c = 3;"), ["(var a=1 b c=3)"]);
        // `Initializer : = AssignmentExpression`, not `Expression` — so a comma separates
        // declarators and does not sequence values. `var a = (1, 2)` is the other reading.
        assert_eq!(statements("var a = 1, b = 2;"), ["(var a=1 b=2)"]);
        assert_eq!(statements("var a = (1, 2);"), ["(var a=(, 1 2))"]);
        // The initialiser is a whole assignment expression.
        assert_eq!(statements("var a = b ? c : d;"), ["(var a=(? b c d))"]);
        assert_eq!(statements("var a = b = c;"), ["(var a=(= b c))"]);
        // Semicolons are inserted here as anywhere else.
        assert_eq!(
            statements("var a = 1\nvar b = 2"),
            ["(var a=1)", "(var b=2)"]
        );
        assert_eq!(statements("{ var a = 1 }"), ["{(var a=1)}"]);
        // Contextual keywords are ordinary names.
        assert_eq!(statements("var async = 1;"), ["(var async=1)"]);
        assert_eq!(statements("var of = 1;"), ["(var of=1)"]);
        assert_eq!(
            statements("var let = 1;"),
            ["(var let=1)"],
            "…including `let`, under `var`"
        );
        // A reserved word is not a name.
        assert_eq!(
            script_error("var if = 1;").kind,
            ParseErrorKind::Unexpected {
                expected: "a binding name",
                found: TokenKind::Keyword(crate::lexer::ReservedWord::If),
            }
        );
        assert!(parse_script("var 1 = 2;").is_err());
        assert!(parse_script("var;").is_err());
    }

    #[test]
    fn the_three_early_errors_of_a_lexical_declaration() {
        // §14.3.1.1, first rule: a `const` binding must be initialised where it is declared.
        // Nothing later may supply it, so `undefined` is not an option and this is a syntax
        // error rather than a runtime one.
        assert_eq!(
            script_error("const a;").kind,
            ParseErrorKind::ConstWithoutInitializer
        );
        assert_eq!(
            script_error("const a, b = 1;").kind,
            ParseErrorKind::ConstWithoutInitializer
        );
        assert_eq!(script_error("const a;").span, Span::new(6, 7));
        assert_eq!(statements("const a = 1, b = 2;"), ["(const a=1 b=2)"]);
        // …and `var` and `let` are content without one.
        assert_eq!(statements("var a;"), ["(var a)"]);
        assert_eq!(statements("let a;"), ["(let a)"]);

        // Second rule: the BoundNames of a lexical BindingList may not contain "let".
        assert_eq!(
            script_error("let let = 1;").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        assert_eq!(
            script_error("const let = 1;").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        assert_eq!(
            script_error("let a, let = 1;").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        // BoundNames is a StringValue, so the rule reads the value and not the spelling: an
        // escaped `let` is still the name `let`.
        assert_eq!(
            script_error(r"let let = 1;").kind,
            ParseErrorKind::LetAsLexicalBindingName
        );
        // The rule is on the lexical forms only.
        assert_eq!(statements("var let = 1;"), ["(var let=1)"]);

        // Third rule: no duplicates within one lexical BindingList.
        assert_eq!(
            script_error("let a, a;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            script_error("const a = 1, a = 2;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(script_error("let a, b, a;").span, Span::new(10, 11));
        // Again lexical only, and again the value rather than the spelling.
        assert_eq!(statements("var a, a;"), ["(var a a)"]);
        assert_eq!(
            script_error(r"let a, a;").kind,
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(statements("let a, b;"), ["(let a b)"]);
    }

    #[test]
    fn let_is_a_declaration_or_a_name_according_to_what_follows_it() {
        // §12.7.2 puts `let` among the names "always allowed as identifiers, but also keywords
        // within certain syntactic productions". Which one it is here depends on the next token
        // and nothing else — the one place this parser looks two tokens ahead.
        assert_eq!(statements("let x = 1;"), ["(let x=1)"]);
        assert_eq!(
            statements("let\nx = 1;"),
            ["(let x=1)"],
            "not a restricted production"
        );
        // Followed by anything that cannot begin a binding, it is an ordinary identifier.
        assert_eq!(statements("let = 1;"), ["(= let 1)"]);
        assert_eq!(statements("let;"), ["let"]);
        assert_eq!(statements("let.a = 1;"), ["(= (. let a) 1)"]);
        assert_eq!(statements("let(1);"), ["(call let [1])"]);
        assert_eq!(statements("let + 1;"), ["(+ let 1)"]);
        // §5.1.5.1: a terminal matches literal source characters, so an escaped spelling is
        // not the keyword — `\u006cet x = 1` is two expressions and not a
        // declaration, and then fails as two expressions on one line always do.
        assert!(parse_script(r"\u006cet x = 1;").is_err());
        assert_eq!(statements(r"\u006cet = 1;"), ["(= let 1)"]);
        // §14.5's restriction, with the thing it was always restricting: `let [` may not begin
        // an ExpressionStatement because it begins a lexical declaration, and now it does.
        assert_eq!(statements("let [a] = b;"), ["(let [a]=b)"]);
        assert_eq!(statements("let {a} = b;"), ["(let {(a a)}=b)"]);
    }

    #[test]
    fn no_declaration_however_odd_can_panic() {
        let cases = [
            "var".to_string(),
            "let".to_string(),
            "const".to_string(),
            "var a,".to_string(),
            "let a =".to_string(),
            "var ".to_string() + &"a, ".repeat(10_000) + "b;",
            "let ".to_string() + &"a".repeat(10_000) + " = 1;",
            "{".repeat(100) + "var a;" + &"}".repeat(100),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A long declarator list is a loop, so it is bounded by memory rather than by the
        // nesting cap — but the duplicate check over it is quadratic, and `var` skips it.
        let many = "var ".to_string()
            + &(0..5000)
                .map(|n| format!("a{n}"))
                .collect::<Vec<_>>()
                .join(", ")
            + ";";
        assert!(parse_script(&many).is_ok());
    }
}
