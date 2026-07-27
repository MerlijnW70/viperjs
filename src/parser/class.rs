//! Class definitions (ECMAScript §15.7), and the `super` they make legal (§13.3.7, §13.3.5).
//!
//! # A class body is strict code, and nothing says so
//!
//! §11.2.2: "All parts of a `ClassDeclaration` or a `ClassExpression` are strict mode code."
//! There is no directive to write and no way to opt out — so `class C { m() { with (a) {} } }` is
//! a Syntax Error in a script that is otherwise sloppy, and `class yield {}` is one too. That is
//! the whole of what makes classes different from everything else here, and it is one line.
//!
//! # A `ClassDeclaration` is lexically scoped, unlike a function
//!
//! `function f() {} function f() {}` is fine at the top level and `class C {} class C {}` is not.
//! A class is not a `HoistableDeclaration`, so §8.2.10 puts it on the lexical side of the top
//! level as much as of a block — which is what makes `let C; class C {}` a redeclaration.
//!
//! # Where `super` may stand
//!
//! Two forms and two different rules, and the difference is not decoration:
//!
//! - `SuperProperty` — `super.a`, `super[a]` — is legal in any method, including an object
//!   literal's. It reads from the home object's prototype, which every method has.
//! - `SuperCall` — `super(…)` — is legal only in the constructor of a class that `extends`
//!   something. It calls the parent constructor, and a class with no parent has none.
//!
//! Both are refused everywhere else: in a plain function, at the top level, and in a constructor
//! that has nothing to be derived from. §15.2.1 has said so since the function slice — "It is a
//! Syntax Error if FunctionBody Contains SuperProperty" — and this is the slice that can tell.

use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{Class, ClassElement, Expr, ExprKind, MethodKind, PropertyKey, Stmt, StmtKind};
use crate::lexer::{Goal, ReservedWord, TokenKind};

impl Parser<'_> {
    /// `ClassDeclaration` (§15.7), with the cursor on `class`.
    pub(super) fn parse_class_declaration(&mut self) -> Result<Stmt, ParseError> {
        let class = self.parse_class(true)?;
        Ok(Stmt {
            span: class.span,
            kind: StmtKind::Class(Box::new(class)),
        })
    }

    /// `ClassExpression` (§15.7), with the cursor on `class`.
    pub(super) fn parse_class_expression(&mut self) -> Result<Expr, ParseError> {
        let class = self.parse_class(false)?;
        let span = class.span;
        Ok(Expr::new(ExprKind::Class(Box::new(class)), span))
    }

    /// Both forms, which differ only in whether the name may be left out.
    fn parse_class(&mut self, name_required: bool) -> Result<Class, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        // One level of the count for the whole class, as a function takes one for itself: both of
        // a class's recursions run through here. The heritage is an expression and a class is one,
        // so `class C extends class … {}` nests; and a method body holds statements, so
        // `class C { m() { class D { … } } }` nests too. Neither is bounded anywhere else — a
        // class body is not a `Block`, and `parse_function_body` does not count.
        //
        // Before the strictness is touched rather than after, so that the early return leaves
        // nothing to restore. The pair below is exact either way.
        self.enter()?;
        // §11.2.2: every part of a class is strict mode code, with no directive to write and no
        // way to opt out — so the name is read under it too, and `class yield {}` is refused.
        let enclosing_strict = self.strict;
        self.strict = true;
        let class = self.parse_class_parts(keyword.span, name_required);
        self.strict = enclosing_strict;
        self.leave();
        class
    }

    /// The name, the heritage and the body, apart so their locals are not carried by every level
    /// of nesting that passes through [`Parser::parse_class`].
    fn parse_class_parts(
        &mut self,
        keyword: crate::span::Span,
        name_required: bool,
    ) -> Result<Class, ParseError> {
        let name = if matches!(
            self.current.kind,
            TokenKind::LBrace | TokenKind::Keyword(ReservedWord::Extends)
        ) && !name_required
        {
            None
        } else {
            Some(self.parse_binding_name()?)
        };
        // `ClassHeritage : extends LeftHandSideExpression` — an expression, not a name, so
        // `class C extends f() {}` is ordinary.
        let heritage = if self.current.kind == TokenKind::Keyword(ReservedWord::Extends) {
            self.advance(Goal::RegExp)?;
            Some(Box::new(self.parse_member(true, None)?))
        } else {
            None
        };
        self.eat(TokenKind::LBrace, Goal::RegExp, "`{`")?;
        // Only the references read inside *this* body are this class's to answer for, so
        // the ones already waiting are marked off first: `class C { m() { this.#a; } }`
        // followed by `class D { #a; }` leaves C's use unresolved, D being a different
        // private space and not an enclosing one.
        let mark = self.private_references.len();
        let mut elements = Vec::new();
        let mut constructors = 0;
        while self.current.kind != TokenKind::RBrace {
            // `ClassElement : ;` — an empty element, which declares nothing and is allowed
            // anywhere among the others.
            if self.current.kind == TokenKind::Semicolon {
                self.advance(Goal::RegExp)?;
                continue;
            }
            let element = self.parse_class_element(heritage.is_some())?;
            if element.is_constructor() {
                constructors += 1;
                // §15.7.1: at most one `constructor` among the prototype methods.
                if constructors > 1 {
                    let ClassElement::Method(method) = &element else {
                        // `is_constructor` answers true of nothing else.
                        return Err(self.unexpected("a method"));
                    };
                    return Err(ParseError {
                        kind: ParseErrorKind::DuplicateConstructor,
                        span: method.key_span,
                    });
                }
            }
            elements.push(element);
        }
        let close = self.eat(TokenKind::RBrace, Goal::Div, "`}`")?;
        self.resolve_private_names(&elements, mark)?;
        Ok(Class {
            name,
            heritage,
            elements: elements.into_boxed_slice(),
            span: keyword.to(close.span),
        })
    }

    /// §15.7.1's duplicate rule and §15.7.7's scope rule, once the body is closed.
    ///
    /// Both wait for the whole body, and for the same reason: a private name may be used before
    /// the element that declares it, so `class C { m() { this.#a; } #a; }` is ordinary code.
    /// References are collected as they are read ([`Parser::private_references`]) and this is
    /// where the ones this class answers for are taken off the list. What is left belongs to the
    /// class around this one — an inner class may use an outer's private names — and what
    /// survives to the end of the script was declared nowhere.
    fn resolve_private_names(
        &mut self,
        elements: &[ClassElement],
        mark: usize,
    ) -> Result<(), ParseError> {
        let mut declared: Vec<(&str, bool, MethodKind)> = Vec::new();
        for element in elements {
            let (name, is_static, kind, span) = match element {
                ClassElement::Method(method) => {
                    (&method.key, method.is_static, method.kind, method.key_span)
                }
                ClassElement::Field(field) => (
                    &field.key,
                    field.is_static,
                    MethodKind::Normal,
                    field.key_span,
                ),
                ClassElement::StaticBlock(_) => continue,
            };
            let PropertyKey::Private(name) = name else {
                continue;
            };
            // §15.7.1: no duplicates among `PrivateBoundIdentifiers`, "unless the name is used
            // once for a getter and once for a setter and in no other entries, and the getter and
            // setter are either both static or both non-static". One private member written as
            // two elements is the only case, which is why the staticness has to match.
            let pairs_with = |(other, other_static, other_kind): &(&str, bool, MethodKind)| {
                *other == &**name
                    && (*other_static != is_static
                        || *other_kind == kind
                        || kind == MethodKind::Normal
                        || *other_kind == MethodKind::Normal)
            };
            if declared.iter().any(pairs_with) {
                return Err(ParseError {
                    kind: ParseErrorKind::DuplicatePrivateName,
                    span,
                });
            }
            declared.push((name, is_static, kind));
        }
        let mut index = mark;
        while index < self.private_references.len() {
            let (name, _) = &self.private_references[index];
            if declared.iter().any(|(declared, _, _)| *declared == &**name) {
                self.private_references.remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }

    /// `SuperProperty` and `SuperCall` (§13.3), with the cursor on `super`.
    pub(super) fn parse_super(&mut self) -> Result<Expr, ParseError> {
        let keyword = self.advance(Goal::RegExp)?;
        match self.current.kind {
            TokenKind::Dot | TokenKind::LBracket => {
                if !self.super_property_allowed() {
                    return Err(ParseError {
                        kind: ParseErrorKind::SuperPropertyOutsideMethod,
                        span: keyword.span,
                    });
                }
                Ok(Expr::new(ExprKind::Super, keyword.span))
            }
            TokenKind::LParen => {
                if !self.super_call_allowed() {
                    return Err(ParseError {
                        kind: ParseErrorKind::SuperCallOutsideDerivedConstructor,
                        span: keyword.span,
                    });
                }
                Ok(Expr::new(ExprKind::Super, keyword.span))
            }
            // `super` on its own is neither production. There is no value it could be.
            _ => Err(ParseError {
                kind: ParseErrorKind::SuperPropertyOutsideMethod,
                span: keyword.span,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_expression, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
    }

    #[test]
    fn a_class_is_a_name_a_heritage_and_a_list_of_methods() {
        assert_eq!(statements("class C {}"), ["(class C - [])"]);
        assert_eq!(statements("class C extends D {}"), ["(class C D [])"]);
        assert_eq!(shape("(class {})"), "(class <anon> - [])");
        assert_eq!(shape("(class C {})"), "(class C - [])");
        assert_eq!(
            statements("class C { m() {} }"),
            ["(class C - [(m (fn <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { get a() {} set a(v) {} }"),
            ["(class C - [(get a (fn <anon> [] {})) (set a (fn <anon> [v] {}))])"]
        );
        assert_eq!(
            statements("class C { static m() {} }"),
            ["(class C - [(static m (fn <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { [a]() {} 1() {} \"s\"() {} }"),
            [
                "(class C - [([a] (fn <anon> [] {})) (n1 (fn <anon> [] {})) (s\"s\" (fn <anon> [] {}))])"
            ]
        );
        // `ClassElement : ;` declares nothing and may stand anywhere among the others, so it
        // leaves no element behind.
        assert_eq!(statements("class C { ; }"), ["(class C - [])"]);
        assert_eq!(
            statements("class C { ;; m() {} ; }"),
            ["(class C - [(m (fn <anon> [] {}))])"]
        );
        // `ClassHeritage : extends LeftHandSideExpression` — an expression and not a name, so
        // everything a `LeftHandSideExpression` is, is allowed and nothing wider.
        for source in [
            "class C extends a.b {}",
            "class C extends f() {}",
            "class C extends new D {}",
            "class C extends (a, b) {}",
            "class C extends {} {}",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        assert!(parse_script("class C extends a => b {}").is_err());
    }

    #[test]
    fn a_class_declaration_needs_a_name_and_a_class_expression_does_not() {
        assert_eq!(shape("(class {})"), "(class <anon> - [])");
        assert!(parse_expression("class extends D {}").is_ok());
        // ...while the declaration form has no such production: `class {` is `class` and then a
        // binding that is not there.
        assert!(parse_script("class {}").is_err());
        assert!(parse_script("class extends D {}").is_err());
    }

    #[test]
    fn a_class_is_a_declaration_so_it_is_lexically_scoped_and_not_a_statement() {
        // §8.2.9 excludes only a `HoistableDeclaration` from `TopLevelLexicallyDeclaredNames`,
        // and a class is not one — so unlike `function f() {} function f() {}`, this is a
        // redeclaration at the top of a script as much as inside a block.
        assert_eq!(
            kind("class C {} class C {}"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            kind("{ class C {} class C {} }"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert_eq!(
            kind("class C {} var C;"),
            ParseErrorKind::ConflictingVarAndLexicalDeclaration
        );
        assert_eq!(
            kind("class C {} function C() {}"),
            ParseErrorKind::ConflictingVarAndLexicalDeclaration,
            "a function is var-scoped at a top level and a class is not, so the two collide"
        );
        assert_eq!(
            kind("let C; class C {}"),
            ParseErrorKind::DuplicateLexicalBinding
        );
        assert!(parse_script("function f() {} function f() {}").is_ok());
        // ...and being a `Declaration` it belongs to a `StatementList` and nowhere else. §14.5's
        // lookahead is what stops the expression path taking these instead.
        for source in ["if (a) class C {}", "for (;;) class C {}", "a: class C {}"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DeclarationInStatementPosition,
                "{source:?}"
            );
        }
        assert!(parse_script("(class C {});").is_ok());
    }

    #[test]
    fn every_part_of_a_class_is_strict_code_with_no_directive_to_write() {
        // §11.2.2, and the reason it is worth a slice of its own: the enclosing script is sloppy
        // and the class is not, with nothing written down to say so.
        assert_eq!(
            kind("class C { m() { with (a) {} } }"),
            ParseErrorKind::StrictWith
        );
        assert_eq!(
            kind("class C { m(a, a) {} }"),
            ParseErrorKind::DuplicateParameterName
        );
        assert_eq!(
            kind("class C { m() { delete a; } }"),
            ParseErrorKind::StrictDeleteOfName
        );
        assert!(parse_script("with (a) {} function f(b, b) {}").is_ok());
        // ...including the name, which is read under it — the class is not entered first.
        assert_eq!(kind("class yield {}"), ParseErrorKind::StrictReservedWord);
        assert_eq!(kind("class eval {}"), ParseErrorKind::StrictEvalOrArguments);
        assert_eq!(
            kind("(class yield {});"),
            ParseErrorKind::StrictReservedWord
        );
        // ...and the heritage, which is an expression and is read under it too.
        assert_eq!(
            kind("class C extends yield {}"),
            ParseErrorKind::StrictReservedWord
        );
        // `await` is only reserved in a module, so it is a name here as it is anywhere else.
        assert!(parse_script("class await {}").is_ok());
        // Strictness stops at the class, being a fact about where you are: after it, the script
        // is as sloppy as it was.
        assert!(parse_script("class C {} with (a) {}").is_ok());
    }

    #[test]
    fn a_class_has_at_most_one_constructor_and_it_is_not_an_accessor() {
        assert!(parse_script("class C { constructor() {} }").is_ok());
        assert_eq!(
            kind("class C { constructor() {} constructor() {} }"),
            ParseErrorKind::DuplicateConstructor
        );
        // §15.7.1 is about `PropName`, so a string key is the same name written differently...
        assert_eq!(
            kind("class C { \"constructor\"() {} constructor() {} }"),
            ParseErrorKind::DuplicateConstructor
        );
        // ...and a computed one is not a name until it runs, which is how a class gets a method
        // called `constructor` at all.
        assert!(parse_script("class C { [\"constructor\"]() {} constructor() {} }").is_ok());
        // Only prototype methods are counted: a static `constructor` names an ordinary property
        // of the constructor object and is not the constructor.
        assert!(parse_script("class C { static constructor() {} constructor() {} }").is_ok());
        assert_eq!(
            kind("class C { get constructor() {} }"),
            ParseErrorKind::ConstructorMayNotBeAnAccessor
        );
        assert_eq!(
            kind("class C { set constructor(a) {} }"),
            ParseErrorKind::ConstructorMayNotBeAnAccessor
        );
        assert!(parse_script("class C { static get constructor() {} }").is_ok());
    }

    #[test]
    fn a_static_method_may_not_be_named_prototype_and_a_prototype_method_may() {
        // `prototype` is the one property a class definition already puts on its constructor,
        // and it is not writable — so a static method by that name could never take effect.
        for source in [
            "class C { static prototype() {} }",
            "class C { static \"prototype\"() {} }",
            "class C { static get prototype() {} }",
            "class C { static set prototype(a) {} }",
        ] {
            assert_eq!(kind(source), ParseErrorKind::StaticPrototype, "{source:?}");
        }
        assert!(parse_script("class C { prototype() {} }").is_ok());
        assert!(parse_script("class C { static [\"prototype\"]() {} }").is_ok());
    }

    #[test]
    fn super_property_is_legal_in_any_method_and_super_call_only_in_a_derived_constructor() {
        for source in [
            "class C { m() { super.a; } }",
            "class C { m() { super[a]; } }",
            "class C { m() { super.a(); } }",
            "class C { m() { super.a = 1; } }",
            "class C { m() { new super.a; } }",
            "class C { static m() { super.a; } }",
            "class C { get a() { super.b; } }",
            "class C extends D { constructor() { super.a; } }",
            "({ m() { super.a; } });",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        assert!(parse_script("class C extends D { constructor() { super(); } }").is_ok());
        // A base class has no parent constructor for `super()` to reach, and no method other
        // than the constructor may call one however the class was written.
        assert_eq!(
            kind("class C { constructor() { super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        assert_eq!(
            kind("class C extends D { m() { super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        assert_eq!(
            kind("({ m() { super(); } });"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        // Outside a method there is no home object, so neither form has anything to mean.
        assert_eq!(kind("super.a;"), ParseErrorKind::SuperPropertyOutsideMethod);
        assert_eq!(
            kind("function f() { super.a; }"),
            ParseErrorKind::SuperPropertyOutsideMethod
        );
        assert_eq!(
            kind("super();"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        // `super` on its own is neither production — there is no value it could be.
        assert_eq!(
            kind("class C { m() { super; } }"),
            ParseErrorKind::SuperPropertyOutsideMethod
        );
        assert_eq!(
            kind("class C { m() { super`x`; } }"),
            ParseErrorKind::SuperPropertyOutsideMethod
        );
    }

    #[test]
    fn a_function_stops_super_and_an_arrow_does_not() {
        // §15.2.1 makes a `FunctionBody` containing either form a Syntax Error outright, so a
        // plain function is a wall however deep inside a method it stands...
        assert_eq!(
            kind("class C { m() { function f() { super.a; } } }"),
            ParseErrorKind::SuperPropertyOutsideMethod
        );
        assert_eq!(
            kind("class C extends D { constructor() { function f() { super(); } } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        // ...while an arrow has no `this` and no home object of its own, so it inherits both.
        assert!(parse_script("class C { m() { () => super.a; } }").is_ok());
        assert!(parse_script("class C extends D { constructor() { () => super(); } }").is_ok());
        assert!(parse_script("({ m() { () => super.a; } });").is_ok());
        // ...and inherits exactly what its method had, not more.
        assert_eq!(
            kind("class C { constructor() { () => super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        // A method inside a method's body starts over: the inner one is a method too.
        assert!(parse_script("class C { m() { ({ n() { super.a; } }); } }").is_ok());
        assert_eq!(
            kind("class C extends D { constructor() { ({ n() { super(); } }); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
    }

    #[test]
    fn no_class_however_truncated_can_panic() {
        let long_body = format!("class C {{ {}}}", "m() {} ".repeat(10_000));
        let deep_heritage = format!(
            "{}D{};",
            "class C extends ".repeat(1000),
            " {}".repeat(1000)
        );
        let deep_bodies = format!("{}{}", "class C { m() { ".repeat(1000), "} }".repeat(1000));
        let cases = [
            "class".to_string(),
            "class C".to_string(),
            "class C {".to_string(),
            "class C extends".to_string(),
            "class C extends D".to_string(),
            "class C { m".to_string(),
            "class C { m(".to_string(),
            "class C { m() {".to_string(),
            "class C { static".to_string(),
            "class C { get".to_string(),
            "class C { a =".to_string(),
            "class C { a = 1".to_string(),
            "class C { static {".to_string(),
            "class C { static { a".to_string(),
            "class C { #".to_string(),
            "class C { #a".to_string(),
            "class C { #a; m() { this.#".to_string(),
            "class C { #a; m() { #a in".to_string(),
            "#a".to_string(),
            "super".to_string(),
            "class C { m() { super".to_string(),
            long_body.clone(),
            deep_heritage.clone(),
            deep_bodies.clone(),
        ];
        for source in &cases {
            let _ = parse_script(source);
        }
        // A long body is a loop, so its length is its own business…
        assert!(parse_script(&long_body).is_ok());
        // …while both of a class's recursions are bounded by the cap. A heritage is an
        // expression and a class is one; a method body holds statements and a class is one of
        // those too. Neither is counted anywhere else — a class body is not a `Block`.
        assert_eq!(kind(&deep_heritage), ParseErrorKind::TooDeeplyNested);
        assert_eq!(kind(&deep_bodies), ParseErrorKind::TooDeeplyNested);
    }
}
