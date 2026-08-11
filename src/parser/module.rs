//! The `Module` goal symbol, and the `import` declarations only it admits (ECMAScript §16.2).
//!
//! # What the goal symbol changes
//!
//! A `Module` is not a `Script` with two more statement forms. Five things differ, and every one
//! of them is observable:
//!
//! - **It is strict code**, always, with no directive to write (§11.2.2). So `with`, legacy octal
//!   and `delete a` are all refused, and `yield` is not a name.
//! - **`[+Await]` at the top level**, so `await a;` is an ordinary top-level statement.
//! - **`await` is never an identifier**, anywhere within — §13.1.1 refuses
//!   `IdentifierReference : await` outright "if the goal symbol of the syntactic grammar is
//!   Module". Not the same rule as the parameter: a plain function body inside a module is
//!   `[~Await]`, and `await` is still not a name in it.
//! - **A top-level function is lexically scoped.** §16.2.1.1 asks for the `LexicallyDeclaredNames`
//!   of a `ModuleItemList` where §16.1.1 asks a `Script` for the `TopLevel` variant, and only the
//!   latter moves a `HoistableDeclaration` to the var side. So `function f() {} function f() {}`
//!   is a redeclaration here and is fine in a script — the one difference nothing about the text
//!   gives away.
//! - **Annex B's HTML-like comments are gone** (§B.1.1 is stated over a `Script` only), which the
//!   lexer is told rather than asked.
//!
//! # `import` is a `ModuleItem` and nothing smaller
//!
//! There is no production putting one inside a block, a function or an `if`, so
//! `{ import a from "b"; }` has no derivation — the loop that reads a module body is the only
//! caller. That is also why `import` stays available as the head of a `ImportCall` everywhere
//! else: the word is only a declaration where a `ModuleItem` may stand.
//!
//! # `from`, `as` and `of` are still names
//!
//! None of the three is reserved, so `import from from "b"` is a real import of the default
//! export as `from`, and each is a lookahead rather than a token test.

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    BindingName, ExportDefault, ExportKind, ImportAttribute, ImportClause, ImportDeclaration,
    ImportSpecifier, Module, ModuleExportName, ModuleItem, Stmt,
};
use crate::lexer::{Goal, ReservedWord, TokenKind, string_value};
use crate::span::Span;

/// Parse `source` under the `Module` goal symbol (§16.2).
///
/// The sibling of [`super::parse_script`], and not a mode of it: the two goal symbols disagree
/// about strictness, about `await`, and about whether a top-level function is lexically scoped.
///
/// # Errors
///
/// The first Syntax Error, with the span of the text that caused it.
///
/// ```
/// use viperjs::parser::parse_module;
///
/// assert!(parse_module("import a from \"b\"; await a;").is_ok());
/// assert!(parse_module("await;").is_err(), "`await` is never a name in a module");
/// assert!(parse_script_would_take_it());
/// fn parse_script_would_take_it() -> bool {
///     viperjs::parser::parse_script("await;").is_ok()
/// }
/// ```
pub fn parse_module(source: &str) -> Result<Module, ParseError> {
    let mut parser = Parser::new(source)?;
    // The four things the goal symbol decides, set before the first item is read because every
    // one of them changes what the *first* token may be.
    parser.module = true;
    parser.strict = true;
    parser.await_allowed = true;
    let body = parser.parse_module_items()?;
    parser.expect_eof()?;
    if let Some((_, span)) = parser.private_references.first() {
        return Err(ParseError {
            kind: ParseErrorKind::UndeclaredPrivateName,
            span: *span,
        });
    }
    super::scope::check_module_declared_names(&body)?;
    super::scope::check_exports(&body)?;
    let statements: Vec<_> = body.iter().filter_map(module_statement).collect();
    // §16.2.1.1 borrows §16.1.1's label rules unchanged, so a `break` at the top of a module is
    // refused exactly as one at the top of a script is.
    super::scope::check_labels(&statements)?;
    Ok(Module {
        body: body.into_boxed_slice(),
        span: Span::new(0, source.len() as u32),
        source: source.into(),
    })
}

/// The statement a `ModuleItem` holds, if it holds one.
///
/// An exported declaration is exactly the statement it would have been without the word, so every
/// walk that reads a module's statements has to see it — the `export` adds an exported name and
/// changes no scoping at all.
pub(super) fn module_statement(item: &ModuleItem) -> Option<Stmt> {
    match item {
        ModuleItem::Statement(statement) => Some(statement.clone()),
        ModuleItem::Export(declaration) => match &declaration.kind {
            ExportKind::Declaration(statement)
            | ExportKind::Default(ExportDefault::Declaration(statement)) => Some(statement.clone()),
            ExportKind::All { .. }
            | ExportKind::NamedFrom { .. }
            | ExportKind::Named(_)
            | ExportKind::Default(ExportDefault::Expression(_)) => None,
        },
        ModuleItem::Import(_) => None,
    }
}

impl Parser<'_> {
    /// `ModuleItemList` (§16.2), read to the end of input.
    fn parse_module_items(&mut self) -> Result<Vec<ModuleItem>, ParseError> {
        let mut body = Vec::new();
        while self.current.kind != TokenKind::Eof {
            body.push(match self.current.kind {
                // `ImportDeclaration` takes an `ImportClause` or a `ModuleSpecifier` after the
                // word, and neither may begin with `(` or `.`. Those two are the `ImportCall` and
                // `ImportMeta` of §13.3, which are expressions and reach here as statements —
                // `import("a");` at the top of a module is an ordinary one.
                TokenKind::Keyword(ReservedWord::Import)
                    if !matches!(
                        self.peek(Goal::RegExp)?.kind,
                        TokenKind::LParen | TokenKind::Dot
                    ) =>
                {
                    ModuleItem::Import(self.parse_import_declaration()?)
                }
                TokenKind::Keyword(ReservedWord::Export) => {
                    ModuleItem::Export(self.parse_export_declaration()?)
                }
                _ => ModuleItem::Statement(self.parse_statement_list_item()?),
            });
        }
        Ok(body)
    }

    /// `ImportDeclaration` (§16.2.2), with the cursor on `import`.
    ///
    /// ```text
    /// ImportDeclaration : import ImportClause FromClause WithClause_opt ;
    ///                   | import ModuleSpecifier WithClause_opt ;
    /// ```
    ///
    /// The second alternative binds nothing and is written for the side effect of the module
    /// being evaluated, which is why a string straight after the word settles which this is.
    fn parse_import_declaration(&mut self) -> Result<ImportDeclaration, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let clause = if matches!(self.current.kind, TokenKind::String { .. }) {
            None
        } else {
            let clause = self.parse_import_clause()?;
            self.eat_contextual("from", "`from`")?;
            Some(clause)
        };
        let specifier = self.parse_module_specifier()?;
        let attributes = self.parse_with_clause()?;
        let end = self.consume_semicolon(keyword.span)?;
        Ok(ImportDeclaration {
            clause,
            specifier,
            attributes,
            span: keyword.span.to(end),
        })
    }

    /// `ImportClause`'s five alternatives (§16.2.2), told apart by their first token.
    fn parse_import_clause(&mut self) -> Result<ImportClause, ParseError> {
        if self.current.kind == TokenKind::Star {
            return Ok(ImportClause::Namespace(self.parse_namespace_import()?));
        }
        if self.current.kind == TokenKind::LBrace {
            return Ok(ImportClause::Named(self.parse_named_imports()?));
        }
        let default = self.parse_imported_binding()?;
        if self.current.kind != TokenKind::Comma {
            return Ok(ImportClause::Default(default));
        }
        self.advance(Goal::RegExp)?;
        if self.current.kind == TokenKind::Star {
            let namespace = self.parse_namespace_import()?;
            return Ok(ImportClause::DefaultAndNamespace(default, namespace));
        }
        Ok(ImportClause::DefaultAndNamed(
            default,
            self.parse_named_imports()?,
        ))
    }

    /// `NameSpaceImport : * as ImportedBinding` (§16.2.2), with the cursor on the `*`.
    fn parse_namespace_import(&mut self) -> Result<BindingName, ParseError> {
        self.advance(Goal::RegExp)?;
        self.eat_contextual("as", "`as`")?;
        self.parse_imported_binding()
    }

    /// `NamedImports : { ImportsList_opt ,_opt }` (§16.2.2), with the cursor on the `{`.
    fn parse_named_imports(&mut self) -> Result<Box<[ImportSpecifier]>, ParseError> {
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        let mut specifiers = Vec::new();
        while self.current.kind != TokenKind::RBrace {
            specifiers.push(self.parse_import_specifier()?);
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
        }
        self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        Ok(specifiers.into_boxed_slice())
    }

    /// `ImportSpecifier : ImportedBinding | ModuleExportName as ImportedBinding` (§16.2.2).
    ///
    /// The shorthand form is the one place a `ModuleExportName` is *also* a binding, which is why
    /// `import {if} from "a"` is refused and `import {if as a} from "b"` is not: `if` names an
    /// export perfectly well and cannot name a local.
    fn parse_import_specifier(&mut self) -> Result<ImportSpecifier, ParseError> {
        let token = self.current;
        let imported = self.parse_module_export_name()?;
        if self.at_contextual("as") {
            self.advance(Goal::RegExp)?;
            let local = self.parse_imported_binding()?;
            return Ok(ImportSpecifier { imported, local });
        }
        // No `as`, so the export's name has to serve as the binding — and only the identifier
        // alternative can. A string never can, whatever it spells.
        let ModuleExportName::Identifier(name) = &imported else {
            return Err(ParseError {
                kind: ParseErrorKind::StringImportWithoutAlias,
                span: token.span,
            });
        };
        if !self.is_identifier_token(token.kind) {
            return Err(self.unexpected("`as`"));
        }
        self.check_strict_name(name, token.span, true)?;
        let local = BindingName {
            name: name.clone(),
            span: token.span,
        };
        Ok(ImportSpecifier { imported, local })
    }

    /// `ModuleExportName : IdentifierName | StringLiteral` (§16.2.2).
    ///
    /// Neither alternative is a binding, so every reserved word is allowed — the name belongs to
    /// the module being imported from, which spells it however it likes.
    pub(super) fn parse_module_export_name(&mut self) -> Result<ModuleExportName, ParseError> {
        let token = self.current;
        match token.kind {
            TokenKind::String { .. } => {
                self.advance(Goal::Div)?;
                let value = string_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok(ModuleExportName::String(value.into_boxed_slice()))
            }
            TokenKind::Identifier { .. } | TokenKind::Keyword(_) => {
                self.advance(Goal::Div)?;
                let name = crate::lexer::identifier_value(self.source, token.span)
                    .ok_or_else(|| self.value_missing(token))?;
                Ok(ModuleExportName::Identifier(
                    name.into_owned().into_boxed_str(),
                ))
            }
            _ => Err(self.unexpected("an export name")),
        }
    }

    /// `ImportedBinding : BindingIdentifier[~Yield, +Await]` (§16.2.2).
    ///
    /// `[+Await]` regardless of what encloses it — but in a module `await` is not a name anywhere,
    /// so the parameter has nothing left to decide and the shape is the citation's.
    fn parse_imported_binding(&mut self) -> Result<BindingName, ParseError> {
        self.parse_binding_name()
    }

    /// `ModuleSpecifier : StringLiteral` (§16.2.2), and nothing else — not a template, not a
    /// name. Where it points is the host's business.
    pub(super) fn parse_module_specifier(&mut self) -> Result<Box<[u16]>, ParseError> {
        let token = self.current;
        if !matches!(token.kind, TokenKind::String { .. }) {
            return Err(self.unexpected("a module specifier"));
        }
        self.advance(Goal::Div)?;
        let value =
            string_value(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
        Ok(value.into_boxed_slice())
    }

    /// `WithClause : with { WithEntries_opt ,_opt }` (§16.2.2), if one is written.
    ///
    /// No `[no LineTerminator here]` before the `with`, so it may start the next line — which is
    /// safe only because a `WithStatement` cannot follow an `ImportDeclaration` without a `;`
    /// that automatic insertion would have had to supply first.
    pub(super) fn parse_with_clause(&mut self) -> Result<Box<[ImportAttribute]>, ParseError> {
        if self.current.kind != TokenKind::Keyword(ReservedWord::With) {
            return Ok(Box::new([]));
        }
        self.advance(Goal::RegExp)?;
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        let mut attributes = Vec::new();
        while self.current.kind != TokenKind::RBrace {
            let key = self.parse_module_export_name()?;
            self.eat(TokenKind::Colon, Goal::RegExp, "`:`")?;
            let token = self.current;
            if !matches!(token.kind, TokenKind::String { .. }) {
                return Err(self.unexpected("a string"));
            }
            self.advance(Goal::Div)?;
            let value =
                string_value(self.source, token.span).ok_or_else(|| self.value_missing(token))?;
            attributes.push(ImportAttribute {
                key,
                value: value.into_boxed_slice(),
            });
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
        }
        self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        Ok(attributes.into_boxed_slice())
    }

    /// Consume a contextual keyword, or say what was expected instead.
    ///
    /// `from` and `as` are ordinary identifiers everywhere else, so neither can be `eat`en as a
    /// token — and an escaped spelling is not the terminal the production names.
    pub(super) fn eat_contextual(
        &mut self,
        word: &str,
        expected: &'static str,
    ) -> Result<(), ParseError> {
        if !self.at_contextual(word) {
            return Err(self.unexpected(expected));
        }
        self.advance(Goal::RegExp)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_module;
    use crate::parser::{ParseErrorKind, parse_script};

    /// The kind of error `source` fails with, as a module.
    fn kind(source: &str) -> ParseErrorKind {
        match parse_module(source) {
            Err(err) => err.kind,
            Ok(module) => panic!("{source:?} should not parse, got {module:?}"), // needs the error
        }
    }

    /// How many items `source` has, as a module.
    fn items(source: &str) -> usize {
        parse_module(source)
            .unwrap_or_else(|err| panic!("{source:?} should parse, got {}", err.kind)) // needs the tree
            .body
            .len()
    }

    #[test]
    fn the_goal_symbol_is_the_difference_and_every_part_of_it_shows() {
        // Strict, always, with no directive to write.
        assert_eq!(kind("with (a) {}"), ParseErrorKind::StrictWith);
        assert_eq!(kind("var a = 010;"), ParseErrorKind::StrictLegacyOctal);
        assert_eq!(kind("delete a;"), ParseErrorKind::StrictDeleteOfName);
        assert_eq!(kind("var yield;"), ParseErrorKind::StrictReservedWord);
        // …and the same text is ordinary as a script, which is what makes it the goal symbol's
        // doing rather than anything written in the file.
        for source in ["with (a) {}", "var a = 010;", "delete a;", "var yield;"] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // `[+Await]` at the top level, so this is a statement rather than two.
        assert!(parse_module("await a;").is_ok());
        assert!(parse_script("await a;").is_err());
        // …and `await` is not a name anywhere within, which is a different rule: the parameter is
        // `[~Await]` inside a plain function and §13.1.1 refuses the word regardless.
        assert!(parse_module("var await;").is_err());
        assert!(parse_module("function f() { var await; }").is_err());
        assert!(parse_module("function f() { await a; }").is_err());
        assert!(parse_script("function f() { var await; }").is_ok());
        // A top-level function is *lexically* declared here, §16.2.1.1 asking for the
        // `LexicallyDeclaredNames` of a `ModuleItemList` where §16.1.1 asks a `Script` for the
        // `TopLevel` variant. The one difference nothing about the text gives away.
        assert_eq!(
            kind("function f() {} function f() {}"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert!(parse_script("function f() {} function f() {}").is_ok());
        // Everything else a top level does, it still does.
        assert!(parse_module("var a; var a;").is_ok());
        assert_eq!(
            kind("let a; let a;"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(kind("return 1;"), ParseErrorKind::ReturnOutsideFunction);
        assert_eq!(
            kind("new.target;"),
            ParseErrorKind::NewTargetOutsideFunction
        );
        assert_eq!(kind("super.a;"), ParseErrorKind::SuperPropertyOutsideMethod);
        assert_eq!(kind("break;"), ParseErrorKind::BreakOutsideLoop);
        // `this` parses and is `undefined` at run time, which is not this parser's business.
        assert!(parse_module("this;").is_ok());
        // Annex B.1.1's HTML-like comments are stated over a `Script` only, so `<!--` is not a
        // comment here. It is not one in a script either yet — see [`crate::lexer`] — so this
        // passes for the deferral's reason and will need the goal symbol the day that lands.
        assert!(parse_module("<!-- a").is_err());
    }

    #[test]
    fn every_shape_of_import_clause_and_none_of_the_shapes_that_are_not() {
        assert_eq!(items("import \"a\";"), 1);
        for source in [
            "import a from \"b\";",
            "import * as a from \"b\";",
            "import {} from \"b\";",
            "import {a} from \"b\";",
            "import {a, b} from \"c\";",
            "import {a,} from \"b\";",
            "import {a as b} from \"c\";",
            "import {a as b, c} from \"d\";",
            "import a, {b} from \"c\";",
            "import a, * as b from \"c\";",
            "import {default as a} from \"b\";",
            "import {\"a\" as b} from \"c\";",
        ] {
            assert!(parse_module(source).is_ok(), "{source:?}");
        }
        // A `ModuleExportName` names something in the *other* module, so every reserved word is
        // allowed there and none of them is allowed as the binding.
        assert!(parse_module("import {if as a} from \"b\";").is_ok());
        assert!(parse_module("import {if} from \"b\";").is_err());
        // …and a string never binds, whatever it spells.
        assert_eq!(
            kind("import {\"a\"} from \"b\";"),
            ParseErrorKind::StringImportWithoutAlias
        );
        // The clause forms that do not exist.
        for source in [
            "import from \"b\";",
            "import * from \"b\";",
            "import {a} \"b\";",
            "import a \"b\";",
            "import a, b from \"c\";",
            "import;",
            "import {,} from \"b\";",
            "import {a,,b} from \"b\";",
            "import {a as} from \"b\";",
        ] {
            assert!(parse_module(source).is_err(), "{source:?}");
        }
        // `ModuleSpecifier : StringLiteral` and nothing else — not a name, not a number, not a
        // template. Said as the kind, because a specifier that is merely *read* and then found to
        // have no string value fails differently and would hide this.
        for source in [
            "import a from b;",
            "import a from 1;",
            "import a from `b`;",
            "import \"a\" with { type: \"json\" }; import b from c;",
        ] {
            assert!(
                matches!(
                    kind(source),
                    ParseErrorKind::Unexpected {
                        expected: "a module specifier",
                        ..
                    }
                ),
                "{source:?} failed with {:?}",
                kind(source)
            );
        }
        // `from` and `as` are terminals of their productions, so a word that is not the one named
        // is not a word that may be skipped. Without the check the next token would simply be
        // taken as whatever came after, and both of these would quietly parse.
        for (source, expected) in [
            ("import a bogus \"c\";", "`from`"),
            ("import * bogus a from \"b\";", "`as`"),
            ("import {a bogus b} from \"c\";", "`}`"),
        ] {
            assert!(
                matches!(
                    kind(source),
                    ParseErrorKind::Unexpected { expected: found, .. } if found == expected
                ),
                "{source:?} failed with {:?}",
                kind(source)
            );
        }
        // The `;` is an ordinary one, so §12.10 supplies it at the end of input.
        assert_eq!(items("import a from \"b\""), 1);
        assert_eq!(items("import \"a\"\nimport \"b\""), 2);
    }

    #[test]
    fn an_import_is_a_module_item_and_may_stand_nowhere_smaller() {
        // No production puts one inside a block, a function, an `if` or a label, so each of these
        // fails at the word — which is also what keeps `import` free to head an `ImportCall`.
        for source in [
            "{ import a from \"b\"; }",
            "if (a) import b from \"c\";",
            "function f() { import a from \"b\"; }",
            "for (;;) import a from \"b\";",
            "label: import a from \"b\";",
        ] {
            assert!(parse_module(source).is_err(), "{source:?}");
        }
        // …and a script has no `ModuleItem` at all.
        assert!(parse_script("import a from \"b\";").is_err());
    }

    #[test]
    fn an_imported_binding_is_lexical_and_is_read_under_the_module_rules() {
        // §8.2.6 gives `ModuleItem : ImportDeclaration` the declaration's `BoundNames`, and they
        // land on the lexical side — so every collision a `let` would cause, an import causes.
        assert_eq!(
            kind("import a from \"b\"; var a;"),
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        // A top-level function is lexical here, so it collides with an import as a *duplicate*
        // rather than as a var-versus-lexical clash — which is the module reading showing through
        // in the error kind and not only in what is refused.
        for source in [
            "import a from \"b\"; let a;",
            "import a from \"b\"; function a() {}",
            "import a from \"b\"; class a {}",
            "import a from \"b\"; import a from \"c\";",
            "import {a, a} from \"b\";",
            "import a, {a} from \"b\";",
            "import {a as b, c as b} from \"d\";",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DuplicateLexicalBinding,
                "{source:?}"
            );
        }
        // …while a block of its own may shadow one, as it may shadow any lexical name.
        assert!(parse_module("import a from \"b\"; { let a; }").is_ok());
        // Reading and assigning are the parser's business only as far as strictness goes.
        assert!(parse_module("import a from \"b\"; a = 1;").is_ok());
        // `ImportedBinding : BindingIdentifier[~Yield, +Await]`, and this is a module besides, so
        // neither `await` nor the strict-mode names may be bound.
        for source in [
            "import await from \"b\";",
            "import {await} from \"b\";",
            "import {a as await} from \"b\";",
            "import yield from \"b\";",
        ] {
            assert!(parse_module(source).is_err(), "{source:?}");
        }
        // §13.1.1's `eval`/`arguments` rule reaches the shorthand form too, where the export's
        // name is doing double duty as the binding — a separate path from the one above, and the
        // only one where a name is bound without `parse_binding_name` having read it.
        assert_eq!(
            kind("import eval from \"b\";"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("import {eval} from \"b\";"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert_eq!(
            kind("import {arguments} from \"b\";"),
            ParseErrorKind::StrictEvalOrArguments
        );
        assert!(parse_module("import {eval as a} from \"b\";").is_ok());
        // …and `await` as the *export* name is an `IdentifierName` like any other.
        assert!(parse_module("import {await as a} from \"b\";").is_ok());
    }

    #[test]
    fn from_and_as_are_contextual_so_they_are_still_names() {
        // The real test of a lookahead: `import from from "b"` imports the default export and
        // calls it `from`.
        assert_eq!(items("import from from \"b\";"), 1);
        assert!(parse_module("import * as from from \"b\";").is_ok());
        assert!(parse_module("import {from} from \"b\";").is_ok());
        assert!(parse_module("import {as as as} from \"b\";").is_ok());
        assert!(parse_module("var from = 1; from;").is_ok());
    }

    #[test]
    fn a_with_clause_takes_string_values_and_nothing_else() {
        for source in [
            "import a from \"b\" with { type: \"json\" };",
            "import a from \"b\" with {};",
            "import \"a\" with { type: \"json\" };",
            "import a from \"b\" with { \"type\": \"json\" };",
            "import a from \"b\" with { type: \"json\", };",
            "import a from \"b\" with { type: \"json\", other: \"x\" };",
        ] {
            assert!(parse_module(source).is_ok(), "{source:?}");
        }
        // `WithEntries : AttributeKey : StringLiteral` — a value that is not a string has no
        // derivation, however ordinary it would be as a property value.
        for source in [
            "import a from \"b\" with { type: json };",
            "import a from \"b\" with { type: 1 };",
            "import a from \"b\" with { type: `json` };",
        ] {
            assert!(
                matches!(
                    kind(source),
                    ParseErrorKind::Unexpected {
                        expected: "a string",
                        ..
                    }
                ),
                "{source:?} failed with {:?}",
                kind(source)
            );
        }
        // No `[no LineTerminator here]` before the `with`, which is safe only because nothing
        // else could follow an `ImportDeclaration` with that word.
        assert!(parse_module("import a from \"b\"\nwith { type: \"json\" };").is_ok());
    }

    #[test]
    fn import_followed_by_a_paren_or_a_dot_is_an_expression_and_not_a_declaration() {
        // An `ImportDeclaration` takes an `ImportClause` or a `ModuleSpecifier` after the word,
        // and neither may begin with `(` or `.`. So §13.3's two forms reach the module body as
        // ordinary statements, and the lookahead is what lets them.
        for source in [
            "import(\"a\");",
            "import(\"a\", b);",
            "import(\"a\").then(b);",
            "import.meta;",
            "import.meta.url;",
            "export default import.meta;",
            "function f() { return import.meta; }",
        ] {
            assert!(parse_module(source).is_ok(), "{source:?}");
        }
        // …while a declaration is still a declaration, and still only at the top.
        assert!(parse_module("import a from \"b\";").is_ok());
        assert!(parse_module("{ import(\"a\"); }").is_ok());
        assert!(parse_module("{ import a from \"b\"; }").is_err());
        // `import.meta` needs the goal symbol and `import()` does not — the one asymmetry.
        assert!(parse_script("import(\"a\");").is_ok());
        assert!(parse_script("import.meta;").is_err());
        // It is not an assignment target, `MetaProperty` being nowhere in §13.15.1's list.
        assert!(parse_module("import.meta = 1;").is_err());
        assert!(parse_module("import.meta++;").is_err());
        // `meta` is a terminal of the production, so nothing else after the `.` is one —
        // and only a module can tell, a script refusing every spelling for its own reason.
        for source in [
            "import.a;",
            "import.Meta;",
            "import.metaa;",
            "import.if;",
            "import.\\u006deta;",
            "import.\"meta\";",
        ] {
            assert!(parse_module(source).is_err(), "{source:?}");
        }
        assert!(parse_module("import.meta;").is_ok());
    }

    #[test]
    fn no_module_however_truncated_can_panic() {
        let cases = [
            "import",
            "import ",
            "import a",
            "import a from",
            "import a from \"",
            "import {",
            "import {a",
            "import {a as",
            "import *",
            "import * as",
            "import a, ",
            "import \"a\" with",
            "import \"a\" with {",
            "import \"a\" with { a",
            "import \"a\" with { a:",
            "",
        ];
        for source in &cases {
            let _ = parse_module(source);
        }
        // An empty module is a `Module : [empty]` and parses to nothing at all.
        assert_eq!(items(""), 0);
    }
}
