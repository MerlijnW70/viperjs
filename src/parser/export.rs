//! `export` declarations, and the two rules that read the finished list (ECMAScript §16.2.3).
//!
//! # Six forms, told apart by one or two tokens
//!
//! ```text
//! ExportDeclaration : export ExportFromClause FromClause WithClause_opt ;
//!                   | export NamedExports ;
//!                   | export VariableStatement
//!                   | export Declaration
//!                   | export default HoistableDeclaration
//!                   | export default ClassDeclaration
//!                   | export default [lookahead ∉ { function, async [nlth] function, class }]
//!                                    AssignmentExpression ;
//! ```
//!
//! A `*` or a `{` after the word settles the first two; `default` settles the last three, and the
//! lookahead there is the only real work — `export default function () {}` is a declaration with
//! no name, which `[+Default]` allows and nothing else in the grammar does.
//!
//! # The two early errors are one about each side
//!
//! Every export has a local name and an exported name, and `export {a as b}` is what shows it:
//! `a` is a binding here and `b` is what another module asks for. §16.2.1.1 has a rule about each
//! — the exported names may not repeat, and the local names must be declared somewhere in the
//! module. Both are asked of the finished `ModuleItemList`, because `export {a}; var a;` is
//! ordinary code and the answer is not known where the export is written.
//!
//! With a `FromClause` there is no local side at all: `export {a} from "b"` re-exports someone
//! else's `a`, so `export {"a"} from "b"` is fine where `export {"a"}` has nothing it could mean.

use super::{ParseError, Parser};
use crate::ast::{ExportDeclaration, ExportDefault, ExportKind, ExportSpecifier, ModuleExportName};
use crate::lexer::{Goal, ReservedWord, TokenKind};
use crate::span::Span;
use crate::static_semantics::{lexically_declared_names, var_declared_names};

impl Parser<'_> {
    /// `ExportDeclaration` (§16.2.3), with the cursor on `export`.
    pub(super) fn parse_export_declaration(&mut self) -> Result<ExportDeclaration, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        let kind = match self.current.kind {
            TokenKind::Star => self.parse_export_all()?,
            TokenKind::LBrace => self.parse_export_named()?,
            TokenKind::Keyword(ReservedWord::Default) => {
                ExportKind::Default(self.parse_export_default()?)
            }
            _ => ExportKind::Declaration(self.parse_exported_declaration()?),
        };
        // The declaration forms bring their own terminator — a `VariableStatement` ends in a `;`
        // and a `HoistableDeclaration` in a `}` — so only the three that end in a clause need one
        // supplied here.
        let end = match &kind {
            ExportKind::Declaration(statement) => statement.span,
            ExportKind::Default(ExportDefault::Declaration(statement)) => statement.span,
            ExportKind::Default(ExportDefault::Expression(value)) => {
                self.consume_semicolon(value.span)?
            }
            ExportKind::All { .. } | ExportKind::NamedFrom { .. } | ExportKind::Named(_) => {
                self.consume_semicolon(keyword.span)?
            }
        };
        Ok(ExportDeclaration {
            kind,
            span: keyword.span.to(end),
        })
    }

    /// `export * FromClause` and `export * as ModuleExportName FromClause` (§16.2.3).
    ///
    /// The `FromClause` is not optional in either: a star with nothing to take from is not a
    /// production, which is what makes `export *;` an error rather than an empty re-export.
    fn parse_export_all(&mut self) -> Result<ExportKind, ParseError> {
        self.advance(Goal::RegExp)?;
        let exported = if self.at_contextual("as") {
            self.advance(Goal::RegExp)?;
            Some(self.parse_module_export_name()?)
        } else {
            None
        };
        self.eat_contextual("from", "`from`")?;
        let specifier = self.parse_module_specifier()?;
        let attributes = self.parse_with_clause()?;
        Ok(ExportKind::All {
            exported,
            specifier,
            attributes,
        })
    }

    /// `export NamedExports` and `export NamedExports FromClause` (§16.2.3).
    ///
    /// The same list either way; the `from` after it is what decides whether the left-hand names
    /// are local bindings or someone else's exports, and so which of §16.2.1.1's rules applies.
    fn parse_export_named(&mut self) -> Result<ExportKind, ParseError> {
        let specifiers = self.parse_named_exports()?;
        if !self.at_contextual("from") {
            return Ok(ExportKind::Named(specifiers));
        }
        self.advance(Goal::RegExp)?;
        let specifier = self.parse_module_specifier()?;
        let attributes = self.parse_with_clause()?;
        Ok(ExportKind::NamedFrom {
            specifiers,
            specifier,
            attributes,
        })
    }

    /// `NamedExports : { ExportsList_opt ,_opt }` (§16.2.3), with the cursor on the `{`.
    fn parse_named_exports(&mut self) -> Result<Box<[ExportSpecifier]>, ParseError> {
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        let mut specifiers = Vec::new();
        while self.current.kind != TokenKind::RBrace {
            let local = self.parse_module_export_name()?;
            let exported = if self.at_contextual("as") {
                self.advance(Goal::RegExp)?;
                self.parse_module_export_name()?
            } else {
                local.clone()
            };
            specifiers.push(ExportSpecifier { local, exported });
            if self.current.kind != TokenKind::Comma {
                break;
            }
            self.advance(Goal::RegExp)?;
        }
        self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        Ok(specifiers.into_boxed_slice())
    }

    /// `export default …` (§16.2.3), with the cursor on `default`.
    ///
    /// The lookahead is `∉ { function, async [no LineTerminator here] function, class }` and it is
    /// what keeps the three declaration forms from being read as expressions — all three are also
    /// perfectly good expressions, so without it `export default class {}` would be an
    /// `AssignmentExpression` and would bind nothing.
    fn parse_export_default(&mut self) -> Result<ExportDefault, ParseError> {
        self.advance(Goal::RegExp)?;
        if self.current.kind == TokenKind::Keyword(ReservedWord::Function) {
            return Ok(ExportDefault::Declaration(
                self.parse_default_function(false)?,
            ));
        }
        if self.at_async_function()? {
            return Ok(ExportDefault::Declaration(
                self.parse_default_function(true)?,
            ));
        }
        if self.current.kind == TokenKind::Keyword(ReservedWord::Class) {
            return Ok(ExportDefault::Declaration(
                self.parse_class_declaration(super::class::NameRequired::No)?,
            ));
        }
        // `AssignmentExpression[+In, ~Yield, +Await]`, so a comma ends it: `export default a, b`
        // has no derivation where `export default (a, b)` does.
        self.enter()?;
        let value = self.parse_assignment(super::expression::AllowIn::Yes);
        self.leave();
        Ok(ExportDefault::Expression(Box::new(value?)))
    }

    /// `export VariableStatement` and `export Declaration` (§16.2.3).
    ///
    /// Every one of them is exactly the statement it would have been without the word, which is
    /// why this reads one and adds nothing: the `export` contributes an exported name and changes
    /// no scoping.
    fn parse_exported_declaration(&mut self) -> Result<crate::ast::Stmt, ParseError> {
        // Ahead of the match for the reason §14.5's is: a match guard may not borrow the parser
        // again, and both of these need a second token to answer.
        if self.at_lexical_let()? {
            return self.parse_declaration(crate::ast::DeclarationKind::Let);
        }
        if self.at_async_function()? {
            return self.parse_function_declaration(true);
        }
        match self.current.kind {
            TokenKind::Keyword(ReservedWord::Var) => {
                self.parse_declaration(crate::ast::DeclarationKind::Var)
            }
            TokenKind::Keyword(ReservedWord::Const) => {
                self.parse_declaration(crate::ast::DeclarationKind::Const)
            }
            TokenKind::Keyword(ReservedWord::Function) => self.parse_function_declaration(false),
            TokenKind::Keyword(ReservedWord::Class) => {
                self.parse_class_declaration(super::class::NameRequired::Yes)
            }
            // Nothing else is a `VariableStatement` or a `Declaration`, and a bare expression is
            // not one however much it looks like a thing worth exporting.
            _ => Err(self.unexpected("a declaration")),
        }
    }
}

/// The names an `ExportDeclaration` makes available to other modules (§16.2.3.4).
///
/// As `StringValue`s, because that is what §16.2.1.1 compares and the two `ModuleExportName`
/// alternatives have to meet somewhere: an identifier's is its name and a string's is its value,
/// so `export {a as "b"}` and `export {c as b}` collide exactly as two identifiers would.
///
/// `export * from "a"` contributes none — which names it re-exports is not known until link time,
/// which is also why two of them never collide with each other.
pub(super) fn exported_names(declaration: &ExportDeclaration) -> Vec<(Vec<u16>, Span)> {
    let span = declaration.span;
    match &declaration.kind {
        ExportKind::All { exported, .. } => exported
            .iter()
            .map(|name| (string_value_of(name), span))
            .collect(),
        ExportKind::NamedFrom { specifiers, .. } | ExportKind::Named(specifiers) => specifiers
            .iter()
            .map(|specifier| (string_value_of(&specifier.exported), span))
            .collect(),
        // The declaration is the same one it would have been without the word, so what it exports
        // is what it declares — `export var a, b;` exports both.
        ExportKind::Declaration(statement) => {
            let body = std::slice::from_ref(statement);
            lexically_declared_names(body)
                .into_iter()
                .chain(var_declared_names(body))
                .map(|declared| (declared.name.encode_utf16().collect(), declared.span))
                .collect()
        }
        // Always the one name, whatever it happens to bind locally: `export default function f(){}`
        // declares `f` here and exports `default`.
        ExportKind::Default(_) => vec![("default".encode_utf16().collect(), span)],
    }
}

/// The local names an `ExportDeclaration` requires to be declared somewhere in the module.
///
/// Only the `from`-less `NamedExports` form has any. Every other form either declares its own
/// names or names something in another module, which is why `export {a}` needs an `a` and
/// `export {a} from "b"` does not.
pub(super) fn exported_bindings(
    declaration: &ExportDeclaration,
) -> impl Iterator<Item = &ModuleExportName> {
    let specifiers: &[ExportSpecifier] = match &declaration.kind {
        ExportKind::Named(specifiers) => specifiers,
        ExportKind::All { .. }
        | ExportKind::NamedFrom { .. }
        | ExportKind::Declaration(_)
        | ExportKind::Default(_) => &[],
    };
    specifiers.iter().map(|specifier| &specifier.local)
}

/// A `ModuleExportName`'s `StringValue` (§16.2.3.x).
fn string_value_of(name: &ModuleExportName) -> Vec<u16> {
    match name {
        ModuleExportName::Identifier(text) => text.encode_utf16().collect(),
        ModuleExportName::String(units) => units.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::{ParseErrorKind, parse_module};

    /// The kind of error `source` fails with, as a module.
    fn kind(source: &str) -> ParseErrorKind {
        match parse_module(source) {
            Err(err) => err.kind,
            Ok(module) => panic!("{source:?} should not parse, got {module:?}"), // needs the error
        }
    }

    /// Whether `source` parses as a module.
    fn ok(source: &str) -> bool {
        parse_module(source).is_ok()
    }

    #[test]
    fn a_re_export_names_the_other_modules_names_and_binds_nothing_here() {
        for source in [
            "export * from \"a\";",
            "export * as a from \"b\";",
            "export * as \"a\" from \"b\";",
            "export * as default from \"b\";",
            "export {} from \"a\";",
            "export {a} from \"b\";",
            "export {a as b} from \"c\";",
            "export {a as \"b\"} from \"c\";",
            "export {\"a\" as b} from \"c\";",
            "export {\"a\"} from \"b\";",
            "export {a,} from \"b\";",
            "export {default} from \"a\";",
            "export {a as default} from \"b\";",
            "export * from \"a\" with { type: \"json\" };",
        ] {
            assert!(ok(source), "{source:?}");
        }
        // Nothing here is a local name, so nothing here has to be declared — which is the whole
        // difference from the `from`-less form below.
        assert!(ok("export {a} from \"b\";"));
        assert_eq!(kind("export {a};"), ParseErrorKind::UndeclaredExportedName);
        // A star with nothing to take from is not a production.
        for source in ["export *;", "export * as a;", "export from \"a\";"] {
            assert!(!ok(source), "{source:?}");
        }
    }

    #[test]
    fn a_named_export_without_from_names_something_this_module_declares() {
        for source in [
            "var a; export {a};",
            "let a; export {a};",
            "function a() {} export {a};",
            "class a {} export {a};",
            "import a from \"b\"; export {a};",
            "var a; export {a as b};",
            "var a; export {a as \"b\"};",
            "var a; export {a,};",
            "export {};",
        ] {
            assert!(ok(source), "{source:?}");
        }
        // The declaration may come after the export: both rules read the finished list.
        assert!(ok("export {a}; var a;"));
        // A string is never a local name, whatever it spells…
        assert_eq!(
            kind("var a; export {\"a\"};"),
            ParseErrorKind::UndeclaredExportedName
        );
        // …and neither is a reserved word, which as an *export* name would be ordinary.
        assert!(!ok("export {if};"));
        assert!(ok("export {if as a} from \"b\";"));
    }

    #[test]
    fn every_declaration_form_is_the_declaration_it_would_have_been() {
        for source in [
            "export var a;",
            "export var a = 1;",
            "export var a, b;",
            "export let a;",
            "export let a = 1;",
            "export const a = 1;",
            "export function f() {}",
            "export function* f() {}",
            "export async function f() {}",
            "export async function* f() {}",
            "export class C {}",
            "export class C extends D {}",
        ] {
            assert!(ok(source), "{source:?}");
        }
        // …so its own rules still apply: a `const` needs an initialiser and a declaration needs a
        // name, `export` having added an exported name and taken nothing away.
        assert_eq!(
            kind("export const a;"),
            ParseErrorKind::ConstWithoutInitializer
        );
        assert!(!ok("export function() {}"));
        assert!(!ok("export class {}"));
        // …and it declares that name in the module, so a later `let` of it collides.
        assert_eq!(
            kind("export var a; let a;"),
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            kind("export function f() {} let f;"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        // Nothing that is not a `VariableStatement` or a `Declaration`.
        for source in ["export a;", "export 1;", "export;", "export"] {
            assert!(!ok(source), "{source:?}");
        }
    }

    #[test]
    fn export_default_is_the_one_place_a_declaration_may_be_anonymous() {
        for source in [
            "export default function () {}",
            "export default class {}",
            "export default function* () {}",
            "export default async function () {}",
            "export default async function* () {}",
        ] {
            assert!(ok(source), "{source:?}");
        }
        // …and a named one still declares its name here, which is what makes `f` usable after it.
        assert!(ok("export default function f() {} f;"));
        // Being a *declaration* rather than an expression is the whole of what the lookahead
        // buys, and the only way to see it is that the name is bound: all three of these parse
        // either way, and only these three collide.
        for source in [
            "export default function f() {} let f;",
            "export default function* f() {} let f;",
            "export default async function f() {} let f;",
            "export default async function* f() {} let f;",
            "export default class C {} let C;",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DuplicateLexicalBinding,
                "{source:?}"
            );
        }
        // The lookahead is what keeps the three declaration forms from being read as expressions,
        // all three being perfectly good ones.
        assert!(ok("export default class C {}"));
        assert!(ok("export default 1;"));
        assert!(ok("export default a = 1;"));
        assert!(ok("export default () => 1;"));
        assert!(ok("export default (1, 2);"));
        // `AssignmentExpression` and not `Expression`, so a comma ends it.
        assert!(!ok("export default a, b;"));
        // `[+Await]`, this being a module.
        assert!(ok("export default await 1;"));
        // Neither a statement nor a declaration that is not one of the three.
        for source in [
            "export default;",
            "export default var a;",
            "export default let a = 1;",
        ] {
            assert!(!ok(source), "{source:?}");
        }
    }

    #[test]
    fn an_exported_name_may_be_written_once_and_default_is_one_of_them() {
        for source in [
            "var a; export {a}; export {a};",
            "var a, b; export {a as c, b as c};",
            "export var a; export var a;",
            "export default 1; export default 2;",
            "export default class C {} export default 1;",
            "export default 1; export {default} from \"a\";",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DuplicateExportedName,
                "{source:?}"
            );
        }
        // Two of the same shape collide as *bindings* before they collide as exported names — a
        // module's top-level `let` and `function` are both lexical — so §16.2.1.1's first rule is
        // what catches them and the export rule never gets asked. `export var a` twice is the
        // one that reaches it, `var a; var a;` being ordinary.
        for source in [
            "export let a = 1; export let a = 2;",
            "export function f() {} export function f() {}",
            "export class C {} export class C {}",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DuplicateLexicalBinding,
                "{source:?}"
            );
        }
        // Different exported names never collide, however the locals are spelled…
        assert!(ok("var a; export {a as b}; export {a};"));
        assert!(ok("var a; export {a}; export default 1;"));
        // …and the two `ModuleExportName` alternatives meet as `StringValue`s, so an identifier
        // and a string spelling the same name are the same name.
        assert_eq!(
            kind("var a, b; export {a as c}; export {b as \"c\"};"),
            ParseErrorKind::DuplicateExportedName
        );
        // `export * from "a"` contributes no name at all: which ones it re-exports is not known
        // until link time, so two of them never collide.
        assert!(ok("export * from \"a\"; export * from \"b\";"));
    }

    #[test]
    fn an_export_is_a_module_item_and_the_contextual_words_stay_names() {
        for source in [
            "{ export var a; }",
            "if (a) export var b;",
            "function f() { export var a; }",
            "label: export var a;",
        ] {
            assert!(!ok(source), "{source:?}");
        }
        // `as` and `from` are not reserved, so each may be exported and each may be a name.
        assert!(ok("var a; export {a as as};"));
        assert!(ok("export * as as from \"b\";"));
        assert!(ok("var from; export {from};"));
        assert!(ok("var a; export {a as default};"));
        assert!(ok("export {default as a} from \"b\";"));
    }

    #[test]
    fn no_export_however_truncated_can_panic() {
        let cases = [
            "export",
            "export ",
            "export *",
            "export * as",
            "export * from",
            "export {",
            "export {a",
            "export {a as",
            "export {a} from",
            "export default",
            "export default function",
            "export default class",
            "export var",
            "export let",
        ];
        for source in &cases {
            let _ = parse_module(source);
        }
    }
}
