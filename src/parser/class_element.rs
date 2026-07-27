//! The elements a class body is made of (ECMAScript §15.7): methods, fields and static blocks.
//!
//! Split from [`super::class`], which is the shape of a class — its name, its heritage and the
//! `super` it makes legal. This file is what goes between the braces.
//!
//! # One production, and four words that only sometimes mean anything
//!
//! ```text
//! ClassElement : MethodDefinition | static MethodDefinition
//!              | FieldDefinition ; | static FieldDefinition ;
//!              | ClassStaticBlock | ;
//! ```
//!
//! `static`, `get`, `set` and `async` are every one of them a perfectly good `ClassElementName`,
//! so each is a lookahead rather than a token test and each has a shape where the word is simply
//! the name: `static;` and `static = 1;` are fields called `static`, `static() {}` is a method
//! called `static`, and `static m() {}` is none of those. What separates a field from a method in
//! the end is one token — a `(` — and everything before it is shared.
//!
//! # A field initialiser and a static block are the same thing twice
//!
//! Both are evaluated by a synthetic method with the class as its home object, so both get
//! `super.a` and `new.target`, neither gets `super(…)`, and §15.7.1 forbids `arguments` in both
//! for the same reason: it would be that method's `arguments` and never the enclosing function's,
//! which is nobody's idea of what the word means.

use super::body::{BodyContext, SuperAllowed};
use super::{ParseError, ParseErrorKind, Parser};
use crate::ast::{
    ClassElement, ClassField, ClassMethod, ClassStaticBlock, Expr, MethodKind, PropertyKey, key_is,
};
use crate::lexer::{Goal, TokenKind};
use crate::span::Span;

impl Parser<'_> {
    /// `ClassElementName : PropertyName | PrivateIdentifier` (§15.7).
    ///
    /// The private half is what makes a class body a lexical space rather than a list of property
    /// names, so it is read here and nowhere an `ObjectLiteral` can reach.
    fn parse_class_element_name(&mut self) -> Result<PropertyKey, ParseError> {
        let token = self.current;
        if !matches!(token.kind, TokenKind::PrivateIdentifier { .. }) {
            return self.parse_property_key();
        }
        self.advance(Goal::Div)?;
        // A declaring position, so it is not a reference — it is what references resolve against.
        let name = self.private_name_only(token)?;
        // §15.7.1: "It is a Syntax Error if the StringValue of PrivateIdentifier is
        // "#constructor"." The constructor is not a private member and cannot be made one.
        if &*name == "constructor" {
            return Err(ParseError {
                kind: ParseErrorKind::PrivateConstructor,
                span: token.span,
            });
        }
        Ok(PropertyKey::Private(name))
    }

    /// One `ClassElement` that is not an empty `;` (§15.7).
    pub(super) fn parse_class_element(
        &mut self,
        derived: bool,
    ) -> Result<ClassElement, ParseError> {
        // `static` is an ordinary `ClassElementName` until something follows it that an element
        // may begin with — so `static() {}` is a method named `static`, `static;` and
        // `static = 1;` are *fields* named `static`, and `static m() {}` is none of those. No
        // `[no LineTerminator here]`, so `static\na;` is a static field and not two.
        let is_static = self.at_contextual("static") && {
            let next = self.peek(Goal::Div)?;
            !matches!(
                next.kind,
                TokenKind::LParen | TokenKind::Semicolon | TokenKind::Eq | TokenKind::RBrace
            )
        };
        if is_static {
            self.advance(Goal::RegExp)?;
            // `ClassStaticBlock : static { ClassStaticBlockStatementList }`. A `{` can begin
            // nothing else here — a `ClassElementName` is never one — so no lookahead is needed
            // beyond the token itself.
            if self.current.kind == TokenKind::LBrace {
                return self
                    .parse_class_static_block()
                    .map(ClassElement::StaticBlock);
            }
        }
        // `AsyncMethod : async [no LineTerminator here] ClassElementName …`, and
        // `AsyncGeneratorMethod` puts the `*` between the two — so the words come in this order
        // and `static` is outside both, the grammar putting it on the `ClassElement`.
        let is_async = self.at_async_method()?;
        if is_async {
            self.advance(Goal::Div)?;
        }
        let is_generator = self.current.kind == TokenKind::Star;
        if is_generator {
            self.advance(Goal::RegExp)?;
        }
        let first = self.current;
        let escaped = matches!(
            first.kind,
            TokenKind::Identifier {
                contains_escape: true
            }
        );
        let key = self.parse_class_element_name()?;
        // `get`/`set` are ordinary names until a name follows them — see [`super::method`]. When
        // one does, the *second* word is what the early errors below are about.
        // …but only when this is an ordinary method: §15.7's `MethodDefinition` gives the
        // accessor forms no `async` and no `*`, so `async get m() {}` has no derivation and the
        // word `get` is simply this method's name.
        let accessor = (!is_async && !is_generator)
            .then(|| self.at_accessor(&key, escaped))
            .flatten();
        let (key, key_span, kind) = match accessor {
            Some(kind) => {
                let name = self.current.span;
                (self.parse_class_element_name()?, name, kind)
            }
            None => (key, first.span, MethodKind::Normal),
        };
        let is_constructor = ClassMethod::names_the_constructor(&key, kind, is_static);
        // §15.7.1: a `constructor` may not be an accessor, and a static method may not be called
        // `prototype` — the one name a class already puts on its constructor object.
        if !is_static && kind != MethodKind::Normal && key_is(&key, "constructor") {
            return Err(ParseError {
                kind: ParseErrorKind::ConstructorMayNotBeAnAccessor,
                span: key_span,
            });
        }
        if is_static && key_is(&key, "prototype") {
            return Err(ParseError {
                kind: ParseErrorKind::StaticPrototype,
                span: key_span,
            });
        }
        // `super.a` in any method; `super(…)` in the constructor of a derived class and nowhere
        // else — a base class has no parent constructor for it to reach.
        // §15.7.1: `SpecialMethod` of the constructor is a Syntax Error — `new` can neither
        // resume a generator nor await, so there would be nothing for the class to be.
        if is_constructor && (is_generator || is_async) {
            return Err(ParseError {
                kind: if is_generator {
                    ParseErrorKind::ConstructorMayNotBeAGenerator
                } else {
                    ParseErrorKind::ConstructorMayNotBeAsync
                },
                span: key_span,
            });
        }
        // Everything above was shared; a `(` is what says this is a `MethodDefinition` at all.
        // Without one it is a `FieldDefinition`, and the three modifiers a method may carry are
        // not modifiers of anything — `class C { *a = 1; }` has no derivation.
        if self.current.kind != TokenKind::LParen {
            if is_async || is_generator || kind != MethodKind::Normal {
                return Err(self.unexpected("`(`"));
            }
            return self
                .parse_class_field(key, key_span, is_static)
                .map(ClassElement::Field);
        }
        let function = self.parse_method(
            kind,
            SuperAllowed {
                property: true,
                call: is_constructor && derived,
            },
            is_generator,
            is_async,
        )?;
        Ok(ClassElement::Method(ClassMethod {
            key,
            kind,
            function,
            is_static,
            key_span,
        }))
    }

    /// `FieldDefinition ;` (§15.7), with the name already read.
    ///
    /// The `;` is an ordinary one, so automatic insertion supplies it before a `}` and across a
    /// line break: `class C { a }` and `class C { a\nb }` are one field and two.
    fn parse_class_field(
        &mut self,
        key: PropertyKey,
        key_span: Span,
        is_static: bool,
    ) -> Result<ClassField, ParseError> {
        // §15.7.1. A field is a property of each instance, and `constructor` is not one — there
        // is already a constructor and it is the class.
        //
        // The other name rule — a static `prototype` — is not repeated here. It is asked of every
        // element before the `(` decides which this is, because it applies to a static method and
        // a static field alike.
        if key_is(&key, "constructor") {
            return Err(ParseError {
                kind: ParseErrorKind::ConstructorAsFieldName,
                span: key_span,
            });
        }
        let initializer = self.parse_field_initializer()?;
        let end = initializer.as_ref().map_or(key_span, |value| value.span);
        self.consume_semicolon(end)?;
        Ok(ClassField {
            key,
            initializer,
            is_static,
            key_span,
        })
    }

    /// `Initializer[+In, ?Yield, ?Await]opt`, and the two things §15.7.1 forbids inside one.
    ///
    /// # The one place this parser reads the grammar against every engine
    ///
    /// The printed production is `Initializer[+In, ?Yield, ?Await]`, which would make
    /// `function* g() { class C { a = yield; } }` a bare `YieldExpression`. Every engine refuses
    /// it, and semantically they have to: a field initialiser is evaluated by a synthetic method
    /// when an *instance* is constructed, so there is no generator there to suspend and no
    /// promise to await. Nothing in §15.7.1 forbids it either, which makes the printed grammar
    /// hard to take at face value.
    ///
    /// So both parameters are dropped here, and `class C { a = await; }` reads `await` as the
    /// name it is. This is the only place praxis knowingly reads the grammar against its text;
    /// M5's test262 run is the thing that settles it, and the test below is written so that the
    /// day it changes, it changes on purpose.
    fn parse_field_initializer(&mut self) -> Result<Option<Box<Expr>>, ParseError> {
        if self.current.kind != TokenKind::Eq {
            return Ok(None);
        }
        self.advance(Goal::RegExp)?;
        let enclosing = (self.yield_allowed, self.await_allowed);
        let enclosing_arguments = self.arguments_reference.take();
        let enclosing_context = self.body_context;
        self.yield_allowed = false;
        self.await_allowed = false;
        self.body_context = BodyContext::CLASS_INITIALIZER;
        self.enter()?;
        let value = self.parse_assignment(super::expression::AllowIn::Yes);
        self.leave();
        let arguments = self.arguments_reference;
        (self.yield_allowed, self.await_allowed) = enclosing;
        self.arguments_reference = enclosing_arguments;
        self.body_context = enclosing_context;
        // §15.7.1: "It is a Syntax Error if Initializer is present and ContainsArguments of
        // Initializer is true." The initialiser runs as its own method, so `arguments` there
        // would be that method's and never the enclosing function's — which is nobody's idea of
        // what it means. `Contains` stops at a function boundary and not at an arrow, so
        // `a = () => arguments` is refused and `a = function () { arguments; }` is not.
        if let Some(span) = arguments {
            return Err(ParseError {
                kind: ParseErrorKind::ArgumentsInClassInitializer,
                span,
            });
        }
        Ok(Some(Box::new(value?)))
    }

    /// `ClassStaticBlock : static { ClassStaticBlockStatementList }` (§15.7), with the cursor on
    /// the `{`.
    ///
    /// `StatementList[~Yield, +Await, ~Return]`, and the `[+Await]` is there so that §15.7.1 can
    /// forbid the word outright rather than let it be a name: the block runs once while the class
    /// is being defined, so there is nothing to suspend into either way.
    fn parse_class_static_block(&mut self) -> Result<ClassStaticBlock, ParseError> {
        let open = self.current;
        let enclosing_yield = self.yield_allowed;
        let enclosing_await = self.await_allowed;
        let enclosing_return = self.inside_function;
        let enclosing_forbidden = self.forbidden_in_parameters.take();
        let enclosing_arguments = self.arguments_reference.take();
        let enclosing_context = self.body_context;
        self.yield_allowed = false;
        self.await_allowed = true;
        // `~Return`, so a `return` here is refused exactly as one at the top of a script is.
        self.inside_function = false;
        self.body_context = BodyContext::CLASS_INITIALIZER;
        let body = self.parse_block_body();
        let forbidden = self.forbidden_in_parameters;
        let arguments = self.arguments_reference;
        self.yield_allowed = enclosing_yield;
        self.await_allowed = enclosing_await;
        self.inside_function = enclosing_return;
        self.forbidden_in_parameters = enclosing_forbidden;
        self.arguments_reference = enclosing_arguments;
        self.body_context = enclosing_context;
        let (body, close) = body?;
        // §15.7.1 asks the three label operations of a `ClassStaticBlockStatementList` from
        // `« »`, exactly as §15.2.1 asks them of a `FunctionStatementList` — so a `break`
        // here cannot see a loop outside the class, and the walks start over.
        super::scope::check_labels(&body)?;
        // §15.7.1's two `Contains` rules, both stopping at a function boundary and neither at an
        // arrow — which is what the two saves above implement.
        if let Some(span) = arguments {
            return Err(ParseError {
                kind: ParseErrorKind::ArgumentsInClassInitializer,
                span,
            });
        }
        // `[~Yield]` above means a `YieldExpression` can never be what was recorded, so the
        // record is an `AwaitExpression` and the message may say so.
        if let Some(error) = forbidden {
            return Err(ParseError {
                kind: ParseErrorKind::AwaitInStaticBlock,
                span: error.span,
            });
        }
        Ok(ClassStaticBlock {
            body,
            span: open.span.to(close),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::test_support::*;
    use crate::parser::{ParseErrorKind, parse_script};

    /// The kind of error `source` fails with, as a script.
    fn kind(source: &str) -> ParseErrorKind {
        script_error(source).kind
    }

    #[test]
    fn a_bigint_names_a_class_element_and_is_never_one_of_the_reserved_names() {
        // `ClassElementName : PropertyName`, so a `BigIntLiteral` names a method or a field.
        assert_eq!(
            shape("class C { 1n(){} }"),
            "(class C - [(1n (fn <anon> [] {}))])"
        );
        assert_eq!(
            shape("class C { get 1n(){} }"),
            "(class C - [(get 1n (fn <anon> [] {}))])"
        );
        assert_eq!(
            shape("class C { async 1n(){} }"),
            "(class C - [(1n (async-fn <anon> [] {}))])"
        );
        assert_eq!(shape("class C { 1n = 2 }"), "(class C - [(field 1n 2)])");
        // §15.7.1's two name rules are about `PropName`, which for a BigInt is a number written
        // out — so it is neither `constructor` nor `prototype`, and both of these parse. A
        // `key_is` that said otherwise would refuse the first as a duplicate constructor and the
        // second as a static `prototype`.
        assert_eq!(
            shape("class C { constructor(){} 1n(){} }"),
            "(class C - [(constructor (fn <anon> [] {})) (1n (fn <anon> [] {}))])"
        );
        assert_eq!(
            shape("class C { static 1n(){} }"),
            "(class C - [(static 1n (fn <anon> [] {}))])"
        );
    }
    #[test]
    fn static_is_a_method_name_until_something_a_method_may_begin_with_follows_it() {
        assert_eq!(
            statements("class C { static() {} }"),
            ["(class C - [(static (fn <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { static static() {} }"),
            ["(class C - [(static static (fn <anon> [] {}))])"]
        );
        assert_eq!(
            statements("class C { static get static() {} }"),
            ["(class C - [(static get static (fn <anon> [] {}))])"]
        );
        // Every other word is an `IdentifierName` here, keyword or not — a method name is a
        // property name, not a binding.
        assert!(parse_script("class C { if() {} get() {} get get() {} }").is_ok());
    }

    #[test]
    fn a_private_name_may_name_any_element_a_public_one_may() {
        assert_eq!(
            statements("class C { #a; }"),
            ["(class C - [(field #a <none>)])"]
        );
        assert_eq!(
            statements("class C { #a = 1; }"),
            ["(class C - [(field #a 1)])"]
        );
        assert_eq!(
            statements("class C { #m() {} }"),
            ["(class C - [(#m (fn <anon> [] {}))])"]
        );
        for source in [
            "class C { static #a; }",
            "class C { static #m() {} }",
            "class C { get #a() {} }",
            "class C { set #a(v) {} }",
            "class C { *#m() {} }",
            "class C { async #m() {} }",
            "class C { async *#m() {} }",
            "class C { static async *#m() {} }",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // §15.7.1: the constructor is not a private member and cannot be made one, whichever kind
        // of element asks for the name.
        for source in ["class C { #constructor; }", "class C { #constructor() {} }"] {
            assert_eq!(
                kind(source),
                ParseErrorKind::PrivateConstructor,
                "{source:?}"
            );
        }
        // …and the `#` is part of the token, so the lexer refuses these before the parser sees
        // anything.
        for source in ["class C { # a; }", "class C { #0a; }"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn two_elements_may_share_a_private_name_only_as_a_getter_and_a_setter() {
        // §15.7.1: no duplicates among `PrivateBoundIdentifiers`, "unless the name is used once
        // for a getter and once for a setter and in no other entries, and the getter and setter
        // are either both static or both non-static". One member written as two elements.
        assert!(parse_script("class C { get #a() {} set #a(v) {} }").is_ok());
        assert!(parse_script("class C { set #a(v) {} get #a() {} }").is_ok());
        assert!(parse_script("class C { static get #a() {} static set #a(v) {} }").is_ok());
        // …and every other pairing is two members with one name.
        for source in [
            "class C { #a; #a; }",
            "class C { get #a() {} get #a() {} }",
            "class C { get #a() {} set #a(v) {} #a; }",
            "class C { #a(){} #a; }",
            "class C { #a; static #a; }",
            "class C { static get #a() {} set #a(v) {} }",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::DuplicatePrivateName,
                "{source:?}"
            );
        }
        // Different names never collide, and a private name is not a public one.
        assert!(parse_script("class C { #a; #b; }").is_ok());
        assert!(parse_script("class C { #a; a; }").is_ok());
    }

    #[test]
    fn a_private_name_is_in_scope_for_the_whole_class_that_declares_it_and_no_other() {
        assert_eq!(
            shape("(class { #a; m() { return this.#a; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (. this #a))}))])"
        );
        // A use may come before the element that declares it, which is the whole reason
        // §15.7.7 is asked of the finished body rather than where the name is read.
        assert!(parse_script("class C { m() { this.#a; } #a; }").is_ok());
        // Every enclosing class counts, so an inner class may reach an outer's names…
        assert!(
            parse_script("class C { #a; m() { return class { n() { return this.#a; } }; } }")
                .is_ok()
        );
        assert!(parse_script("class C { #a; m() { class D { n() { this.#a; } } } }").is_ok());
        // …and a class that merely finished does not, however it is related.
        assert_eq!(
            kind("class C { #a; } class D { m() { this.#a; } }"),
            ParseErrorKind::UndeclaredPrivateName
        );
        assert_eq!(
            kind("class C { m() { this.#a; } } class D { #a; }"),
            ParseErrorKind::UndeclaredPrivateName
        );
        assert_eq!(
            kind("class C { #a; } class D extends C { m() { this.#a; } }"),
            ParseErrorKind::UndeclaredPrivateName
        );
        // …nor does a name that was never declared at all, inside a class or outside one.
        assert_eq!(kind("this.#a;"), ParseErrorKind::UndeclaredPrivateName);
        assert_eq!(
            kind("class C { m() { this.#b; } #a; }"),
            ParseErrorKind::UndeclaredPrivateName
        );
        // A function boundary is not a private-name boundary: only a class body is.
        assert!(parse_script("class C { #a; m() { function f() { this.#a; } } }").is_ok());
        assert!(parse_script("class C { #a; m() { ({ n() { this.#a; } }); } }").is_ok());
        assert!(parse_script("class C { #a; static { this.#a; } }").is_ok());
        assert!(parse_script("class C { #a; b = this.#a; }").is_ok());
    }

    #[test]
    fn a_private_member_is_read_like_any_other_and_deleted_like_none() {
        assert_eq!(
            shape("(class { #a; m() { return a.#a; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (. a #a))}))])"
        );
        // It chains exactly as a public one does, including through an optional link.
        for source in [
            "a.#a();",
            "a?.#a;",
            "a.#a`x`;",
            "({}).#a;",
            "new C().#a;",
            "this.#a = 1;",
            "this.#a++;",
            "[this.#a] = b;",
            "typeof this.#a;",
        ] {
            assert!(
                parse_script(&format!("class C {{ #a; m() {{ {source} }} }}")).is_ok(),
                "{source:?}"
            );
        }
        // §13.3.7 gives `SuperProperty` an `IdentifierName` and no private form: the name would
        // have to be looked up in the parent's private space, which is not a thing that exists.
        assert_eq!(
            kind("class C { #a; m() { super.#a; } }"),
            ParseErrorKind::PrivateNameAfterSuper
        );
        // §13.5.1: the name is not a property key, so there is no property to remove — and unlike
        // `delete a`, this holds in sloppy code too.
        assert_eq!(
            kind("class C { #a; m() { delete this.#a; } }"),
            ParseErrorKind::DeleteOfPrivateMember
        );
        assert_eq!(
            kind("class C { #a; m() { delete (this.#a); } }"),
            ParseErrorKind::DeleteOfPrivateMember
        );
        assert_eq!(
            kind("class C { #a; m() { delete a?.#a; } }"),
            ParseErrorKind::DeleteOfPrivateMember
        );
        assert!(parse_script("class C { #a; m() { delete a.b; } }").is_ok());
    }

    #[test]
    fn the_only_place_a_private_name_stands_alone_is_the_left_of_an_in() {
        assert_eq!(
            shape("(class { #a; m() { return #a in b; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (#in a b))}))])"
        );
        // `RelationalExpression : PrivateIdentifier in ShiftExpression`, so the right operand
        // stops where an ordinary `in`'s would and everything looser still applies to the whole.
        assert_eq!(
            shape("(class { #a; m() { return #a in b + c; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (#in a (+ b c)))}))])"
        );
        assert_eq!(
            shape("(class { #a; m() { return #a in b in c; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (in (#in a b) c))}))])"
        );
        assert_eq!(
            shape("(class { #a; m() { return #a in b == c; } })"),
            "(class <anon> - [(field #a <none>) (m (fn <anon> [] {(return (== (#in a b) c))}))])"
        );
        assert!(parse_script("class C { #a; m() { x = #a in b; } }").is_ok());
        assert!(parse_script("class C { #a; m() { (#a in b); } }").is_ok());
        // The `in` is required — a private name alone is not anything…
        assert!(parse_script("class C { #a; m() { #a; } }").is_err());
        assert!(parse_script("class C { #a; m() { #a == b; } }").is_err());
        // …and `[~In]` refuses it as it refuses any other `in`.
        assert!(parse_script("class C { #a; m() { for (#a in b;;); } }").is_err());
        // The name still has to be declared, and a `ClassElement` is not an expression.
        assert_eq!(kind("#a in b;"), ParseErrorKind::UndeclaredPrivateName);
        assert!(parse_script("class C { #a in b; }").is_err());
    }

    #[test]
    fn a_field_is_a_name_an_optional_initialiser_and_a_semicolon_that_may_be_inserted() {
        assert_eq!(
            statements("class C { a; }"),
            ["(class C - [(field a <none>)])"]
        );
        assert_eq!(
            statements("class C { a = 1; }"),
            ["(class C - [(field a 1)])"]
        );
        assert_eq!(
            statements("class C { static a = 1; }"),
            ["(class C - [(static field a 1)])"]
        );
        assert_eq!(
            statements("class C { a; b; }"),
            ["(class C - [(field a <none>) (field b <none>)])"]
        );
        // Every `ClassElementName` a method may have, a field may have.
        assert_eq!(
            statements("class C { [a] = 1; 1 = 2; \"s\" = 3; if = 4; }"),
            ["(class C - [(field [a] 1) (field n1 2) (field s\"s\" 3) (field if 4)])"]
        );
        // The `;` is an ordinary one, so §12.10 supplies it before a `}` and across a line break.
        assert_eq!(
            statements("class C { a }"),
            ["(class C - [(field a <none>)])"]
        );
        assert_eq!(
            statements("class C { a\nb }"),
            ["(class C - [(field a <none>) (field b <none>)])"]
        );
        assert_eq!(
            statements("class C { a = 1\nb = 2 }"),
            ["(class C - [(field a 1) (field b 2)])"]
        );
        // …and not where automatic insertion would not.
        for source in ["class C { a = 1 b = 2 }", "class C { a m() {} }"] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
        // `FieldDefinition` is singular: there is no comma list, unlike a `var` statement.
        assert!(parse_script("class C { a = 1, b = 2; }").is_err());
        // A field mixes freely with everything else a class body holds.
        assert!(parse_script("class C { a; static b; static {} m() {} ; }").is_ok());
    }

    #[test]
    fn a_word_that_modifies_a_method_is_a_field_name_when_nothing_follows_it() {
        // `static`, `get`, `set` and `async` are all `ClassElementName`s in their own right, and
        // what follows the word is the whole of what says which it was here.
        for source in [
            "class C { static; }",
            "class C { static = 1; }",
            "class C { static static; }",
            "class C { static static = 1; }",
            "class C { get; }",
            "class C { get = 1; }",
            "class C { async; }",
            "class C { async = 1; }",
            "class C { static async; }",
            "class C { static get; }",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        assert_eq!(
            statements("class C { static static = 1; }"),
            ["(class C - [(static field static 1)])"]
        );
        // `static` carries no `[no LineTerminator here]`, so this is one static field and not two.
        assert_eq!(
            statements("class C { static\na; }"),
            ["(class C - [(static field a <none>)])"]
        );
        // …while `async` does carry one, so this is two fields.
        assert_eq!(
            statements("class C { async\na; }"),
            ["(class C - [(field async <none>) (field a <none>)])"]
        );
        // The three modifiers modify a *method*, so none of them may precede a field.
        for source in [
            "class C { *a = 1; }",
            "class C { async a = 1; }",
            "class C { get a = 1; }",
            "class C { get a; }",
            "class C { async a; }",
            "class C { static *; }",
        ] {
            assert!(parse_script(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn a_field_may_not_be_called_constructor_and_a_static_one_may_not_be_called_prototype() {
        // §15.7.1. A field is a property of each instance, and `constructor` is not one — there is
        // already a constructor and it is the class.
        for source in [
            "class C { constructor; }",
            "class C { constructor = 1; }",
            "class C { static constructor; }",
        ] {
            assert_eq!(
                kind(source),
                ParseErrorKind::ConstructorAsFieldName,
                "{source:?}"
            );
        }
        assert_eq!(
            kind("class C { static prototype; }"),
            ParseErrorKind::StaticPrototype
        );
        // …and a *prototype* field is fine, the rule being about the constructor object.
        assert!(parse_script("class C { prototype; }").is_ok());
        // A computed key has no `PropName` until it runs, so neither rule can see it.
        assert!(parse_script("class C { static [\"prototype\"]; }").is_ok());
        // A method named `constructor` is still the constructor, which is a different rule.
        assert!(parse_script("class C { constructor() {} }").is_ok());
    }

    #[test]
    fn an_initialiser_has_a_home_object_but_no_arguments_and_no_super_call() {
        // §15.7.1's two `Contains` rules. Both run as a synthetic method with the class as its
        // home object, so `super.a` and `new.target` are legal…
        for source in [
            "class C extends D { a = super.b; }",
            "class C { a = super.b(); }",
            "class C { a = new.target; }",
            "class C { a = this; }",
            "class C { a = () => super.b; }",
            "class C { a = class { b = super.c; }; }",
        ] {
            assert!(parse_script(source).is_ok(), "{source:?}");
        }
        // …and `super(…)` is not, an initialiser having no parent constructor to reach.
        assert_eq!(
            kind("class C extends D { a = super(); }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        // `arguments` would be the synthetic method's and never the enclosing function's.
        // `Contains` stops at a function boundary and not at an arrow, which is the whole of the
        // difference between these two.
        assert_eq!(
            kind("class C { a = arguments; }"),
            ParseErrorKind::ArgumentsInClassInitializer
        );
        assert_eq!(
            kind("class C { a = () => arguments; }"),
            ParseErrorKind::ArgumentsInClassInitializer
        );
        assert_eq!(
            kind("class C { a = (arguments) => 1; }"),
            ParseErrorKind::ArgumentsInClassInitializer
        );
        assert!(parse_script("class C { a = function () { arguments; }; }").is_ok());
        assert!(parse_script("class C { a = class { m() { arguments; } }; }").is_ok());
        // …and a *property* named `arguments` is not a reading of the name at all.
        assert!(parse_script("class C { a = b.arguments; }").is_ok());
        assert!(parse_script("class C { a = ({arguments: 1}); }").is_ok());
    }

    #[test]
    fn an_initialiser_drops_both_suspension_parameters_against_the_printed_grammar() {
        // The production is `Initializer[+In, ?Yield, ?Await]`, which would make the first of
        // these a bare `YieldExpression`. It is refused instead — see
        // [`Parser::parse_field_initializer`] for why, and expect this test to be what changes if
        // test262 says otherwise.
        assert!(parse_script("function* g() { class C { a = yield; } }").is_err());
        assert_eq!(
            statements("async function f() { class C { a = await; } }"),
            ["(async-fn f [] {(class C - [(field a await)])})"]
        );
        // The *name* keeps both, which is the asymmetry: `[yield]` here is a `YieldExpression`.
        assert!(parse_script("function* g() { class C { [yield] = 1; } }").is_ok());
        // Outside a generator the class body's strictness is what refuses the word.
        assert_eq!(
            kind("class C { a = yield; }"),
            ParseErrorKind::StrictReservedWord
        );
    }

    #[test]
    fn a_static_block_is_a_statement_list_that_starts_every_walk_over() {
        assert_eq!(
            statements("class C { static {} }"),
            ["(class C - [(static-block {})])"]
        );
        assert_eq!(
            statements("class C { static { a; } }"),
            ["(class C - [(static-block {a})])"]
        );
        assert!(parse_script("class C { static {} static {} }").is_ok());
        // A `{` can begin nothing else after `static`, so no lookahead is needed — including
        // across a line break, `ClassStaticBlock` carrying no restriction.
        assert!(parse_script("class C { static\n{} }").is_ok());
        // `[~Return]`, so a `return` is refused exactly as one at the top of a script is…
        assert_eq!(
            kind("class C { static { return; } }"),
            ParseErrorKind::ReturnOutsideFunction
        );
        // …and a function inside gets its own `[+Return]` back.
        assert!(parse_script("class C { static { function f() { return; } } }").is_ok());
        // §15.7.1 asks the three label operations from `« »`, as §15.2.1 does of a function body,
        // so a jump here cannot see a loop outside the class.
        assert_eq!(
            kind("class C { static { break; } }"),
            ParseErrorKind::BreakOutsideLoop
        );
        assert_eq!(
            kind("class C { static { continue; } }"),
            ParseErrorKind::ContinueOutsideLoop
        );
        assert!(parse_script("class C { static { l: break l; } }").is_ok());
        assert!(parse_script("class C { static { for (;;) break; } }").is_ok());
        // …and §14.2.1's name rules are the block's own.
        assert!(parse_script("class C { static { var a; let a; } }").is_err());
        assert!(parse_script("class C { static { let a; { let a; } } }").is_ok());
        // It is an initialiser, so it has the same home object and the same refusals.
        assert!(parse_script("class C { static { super.a; this; new.target; } }").is_ok());
        assert_eq!(
            kind("class C extends D { static { super(); } }"),
            ParseErrorKind::SuperCallOutsideDerivedConstructor
        );
        assert_eq!(
            kind("class C { static { arguments; } }"),
            ParseErrorKind::ArgumentsInClassInitializer
        );
        assert_eq!(
            kind("class C { static { () => arguments; } }"),
            ParseErrorKind::ArgumentsInClassInitializer
        );
        assert!(parse_script("class C { static { function f() { arguments; } } }").is_ok());
    }

    #[test]
    fn a_static_block_makes_await_a_keyword_only_so_that_it_can_forbid_it() {
        // `[~Yield, +Await]`, and then §15.7.1's "Contains await". The parameter is set so the
        // word is a keyword rather than a name, and the rule then refuses it either way: the
        // block runs once while the class is being defined, so there is nothing to suspend into.
        assert_eq!(
            kind("class C { static { await a; } }"),
            ParseErrorKind::AwaitInStaticBlock
        );
        assert!(parse_script("class C { static { await; } }").is_err());
        assert!(parse_script("async function f() { class C { static { await a; } } }").is_err());
        // `Contains` stops at a function boundary, so a nested async function may await…
        assert!(parse_script("class C { static { async function f() { await a; } } }").is_ok());
        assert!(parse_script("class C { static { ({ async m() { await a; } }); } }").is_ok());
        // …and not at an arrow, which has no `await` of its own to give.
        assert!(parse_script("class C { static { () => await a; } }").is_err());
        // A nested class's field initialiser drops the parameter, so the word is a name again.
        assert!(parse_script("class C { static { class D { a = await; } } }").is_ok());
        // `[~Yield]`, and the class body is strict, so `yield` is refused as a name.
        assert_eq!(
            kind("class C { static { yield; } }"),
            ParseErrorKind::StrictReservedWord
        );
        assert_eq!(
            kind("function* g() { class C { static { yield; } } }"),
            ParseErrorKind::StrictReservedWord,
            "the parameter is dropped, so this is the name being refused and not an operator"
        );
    }
}
