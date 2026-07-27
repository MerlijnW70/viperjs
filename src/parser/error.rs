//! Why parsing stopped, and where.
//!
//! A plain data module, like the lexer's: the parser decides what went wrong, and this decides
//! only how to say it. Messages are built without the source, so a host that has kept nothing
//! but the error can still render something a person can act on.

use crate::lexer::{LexError, LexErrorKind, TokenKind};
use crate::span::Span;
use std::fmt;

/// Why parsing stopped, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// What went wrong.
    pub kind: ParseErrorKind,
    /// The source it went wrong at. For an unexpected token this is that token, not the
    /// construct it interrupted — a caret under the surprise beats one under its context.
    pub span: Span,
}

/// Every failure the parser can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The lexer could not produce a token at all.
    Lexical(LexErrorKind),
    /// A token appeared where the grammar does not allow it.
    Unexpected {
        /// What the grammar wanted, phrased for a reader: `` "`)`" ``, `"an expression"`.
        expected: &'static str,
        /// What was actually there, so a message can be built without re-reading the source.
        found: TokenKind,
    },
    /// Nesting exceeded [`MAX_NESTING_DEPTH`].
    TooDeeplyNested,
    /// §13.6: `ExponentiationExpression : UpdateExpression ** ExponentiationExpression`.
    ///
    /// The left operand is an `UpdateExpression`, which a prefix unary is not — so `-a ** b` has
    /// no derivation and `(-a) ** b` does. The rule exists because the alternative reading is
    /// genuinely ambiguous to a reader: `-a ** b` could plausibly mean either `(-a) ** b` or
    /// `-(a ** b)`, and those differ.
    ExponentiationOnUnary,
    /// §13.15.1: the left of an assignment must be something that can be assigned to.
    ///
    /// The specification calls the test `AssignmentTargetType`, and for everything this parser
    /// can build so far the answer is "an identifier, however many parentheses are around it".
    /// `1 = 2` and `(a, b) = 3` are the shapes this rejects.
    InvalidAssignmentTarget,
    /// §13.1.1: a name strict code keeps for itself.
    ///
    /// `implements`, `interface`, `let`, `package`, `private`, `protected`, `public`, `static`
    /// and `yield` — the words a future edition wanted room for, left available to the sloppy
    /// code that was already using them.
    StrictReservedWord,
    /// §13.1.1: `eval` or `arguments` bound or assigned to in strict code.
    ///
    /// Reading them is fine; it is binding one or assigning to one that is refused, which is why
    /// `eval("x")` works in strict code and `eval = 1` does not.
    StrictEvalOrArguments,
    /// §14.11.1: a `with` statement in strict code.
    StrictWith,
    /// §13.5.1: `delete` applied to a bare name in strict code.
    ///
    /// `delete a.b` is fine and `delete a` is not — the second asks to remove a binding rather
    /// than a property, which strict code has no way to express.
    StrictDeleteOfName,
    /// Annex B.1.1 in strict code: a legacy octal literal, or a decimal with a leading zero.
    ///
    /// The lexer reads both and flags them, being the lexical grammar's business; refusing them
    /// where §12.9.3.1 says to is this parser's.
    StrictLegacyOctal,
    /// §15.2.1: a `"use strict"` directive in a body whose parameter list is not simple.
    ///
    /// The parameters of a non-simple list are initialised by running code, and that code would
    /// have to be told a strictness the directive has not announced yet.
    UseStrictWithNonSimpleParameters,
    /// §14.5: a `function` or a `class` where only a `Statement` may stand.
    ///
    /// Both are `Declaration`s, so both belong to a `StatementList` — and §14.5's
    /// `[lookahead ∉ { {, function, async function, class, let [ }]` keeps an
    /// `ExpressionStatement` from beginning with either word, so neither can slip through as an
    /// *expression* either. `if (x) function f() {}`, `a: class C {}` and `for (;;) class C {}`
    /// are the shapes. Annex B.3.2 and §14.13.1 exempt some of the `function` ones for a web
    /// host, and both exemptions turn on strictness; neither ever covers a class.
    DeclarationInStatementPosition,
    /// §14.10: a `return` outside any function body.
    ///
    /// `ReturnStatement` is an alternative of `Statement[Return]`, and only a `FunctionBody` sets
    /// that parameter — so outside one there is no such statement rather than a bad one.
    ReturnOutsideFunction,
    /// §15.1.1: a non-simple parameter list repeats a name.
    ///
    /// `function f(a, a) {}` is legal and `function f(a, a = 1) {}` is not. A non-simple list is
    /// initialised by running code, and that code has to know which `a` it means.
    DuplicateParameterName,
    /// §15.2.1: a parameter name is also lexically declared by the body.
    ///
    /// `function f(a) { let a; }` is refused and `function f(a) { var a; }` is not — the second
    /// is one binding written twice.
    ParameterRedeclaredInBody,
    /// §14.3.3: a binding pattern with no initialiser — `var [a];`.
    ///
    /// `VariableDeclaration : BindingIdentifier Initializer_opt | BindingPattern Initializer` —
    /// the `_opt` is on the first alternative only, so a pattern always needs something to take
    /// apart. Unlike the `const` rule, this holds for all three keywords.
    PatternWithoutInitializer,
    /// §14.15.1: the `BoundNames` of a catch parameter repeat — `catch ([a, a])`.
    DuplicateCatchParameterName,
    /// §14.3.1.1: a `const` binding with no initialiser.
    ///
    /// `const a;` has nothing to be constant, and no later statement may supply it — which is
    /// why this is a Syntax Error rather than a value of `undefined`.
    ConstWithoutInitializer,
    /// §14.3.1.1: the `BoundNames` of a lexical declaration may not contain `let`.
    ///
    /// `let let = 1` and `const let = 1` are both refused. `var let = 1` is not: the rule is on
    /// the lexical forms only.
    LetAsLexicalBindingName,
    /// §14.3.1.1: the `BoundNames` of a lexical declaration may not repeat.
    ///
    /// `let a, a;` is refused where `var a, a;` is not, for the same reason.
    DuplicateLexicalBinding,
    /// §14.2.1 and §16.1.1: a name declared by `var` and also by `let` or `const`.
    ///
    /// The two rules are the same one seen from either side, because a `var` belongs to the
    /// enclosing function however deeply it is nested — so `{ let a; { var a; } }` puts two
    /// bindings of `a` in one scope even though nothing at either level looks like a
    /// redeclaration.
    ConflictingVarAndLexicalDeclaration,
    /// §13.15.5.1: something in a destructuring pattern that cannot be assigned to.
    ///
    /// Stricter than [`ParseErrorKind::InvalidAssignmentTarget`]: that one refuses a target whose
    /// `AssignmentTargetType` is *invalid*, and this one refuses anything not *simple* — so the
    /// `web-compat` case of §8.6.4 is refused here on every host, `[f()] = b` being a Syntax
    /// Error where `f() = b` is a run-time one.
    InvalidDestructuringTarget,
    /// §13.15.5: a `...` element with something after it.
    ///
    /// Including a comma: `[...a, ] = b` has no derivation, an `AssignmentRestElement` being last
    /// with nothing following. As a literal the same text is fine, which is why this is found
    /// during refinement rather than while reading.
    RestElementMustBeLast,
    /// §13.15.5: a `...` element with an initialiser — `[...a = 1] = b`.
    RestElementWithInitializer,
    /// §13.15.5.1: an `AssignmentRestProperty` target that is an array or object literal.
    ///
    /// `({...[a]} = b)` has no derivation where `[...[a]] = b` does. The asymmetry is real: the
    /// remaining elements of an iterator can be spread into a pattern, and there is no way to
    /// spread the remaining properties of an object into one.
    RestTargetMayNotBePattern,
    /// §13.2.8.1: an ill-formed escape in a template that is not tagged.
    ///
    /// A tag function is handed the raw text as well as the cooked value, so `undefined` for the
    /// cooked one is something it can be told. An untagged template has no such channel.
    BadEscapeInUntaggedTemplate,
    /// §15.3: something in an arrow's parameter list that cannot be a binding.
    ///
    /// `(a.b) => c` has no derivation where `[a.b] = c` does, for the reason `let [a.b] = c` has
    /// none: an arrow's parameters *create* names, and `a.b` is not a name.
    InvalidArrowParameter,
    /// §13.2: a parenthesized group that only arrow parameters could have been.
    ///
    /// `()`, `(a,)` and `(...a)` are productions of
    /// `CoverParenthesizedExpressionAndArrowParameterList` and of nothing else — there is nothing
    /// for them to evaluate to, so without a `=>` after them they are not anything.
    CoverGroupIsNotAnExpression,
    /// §15.4: a getter with parameters, or a setter without exactly one.
    ///
    /// `get a()` is written with empty parentheses in the grammar and `set a(v)` with a single
    /// `FormalParameter` — singular, and a `FormalParameter` rather than a `FormalParameters`, so
    /// a setter may take a pattern or a default and may not take a rest.
    AccessorParameterCount,
    /// §15.5.1: a `YieldExpression` in a generator's parameter list.
    ///
    /// `function* g(a = yield) {}`. A default is evaluated before the generator is in a resumable
    /// state, so there would be nothing for it to yield to — the refusal is about the runtime
    /// having no answer, not about the syntax being ambiguous. `Contains` stops at a function
    /// boundary, so `function* g(a = function*() { yield; }) {}` is fine.
    YieldInParameters,
    /// §15.7.1: a `constructor` written as a generator.
    ///
    /// `class C { *constructor() {} }`. A constructor is the function the class is, and `new`
    /// cannot resume a generator — so `SpecialMethod` being true of it is a Syntax Error.
    ConstructorMayNotBeAGenerator,
    /// §15.7.1: two methods named `constructor` in one class body.
    ///
    /// Prototype methods only, and never an accessor or a static one — a class may have a static
    /// `constructor` and a `get constructor` is refused for a different reason below.
    DuplicateConstructor,
    /// §15.7.1: `get constructor() {}` or `set constructor(a) {}`.
    ///
    /// A class's constructor is the function the class *is*, so there is nothing for an accessor
    /// to be. A static one is fine — that names an ordinary property of the constructor object.
    ConstructorMayNotBeAnAccessor,
    /// §15.7.1: a static method named `prototype`.
    ///
    /// `prototype` is the one property a class definition already puts on its constructor, and it
    /// is not writable — so a static method by that name could never take effect.
    StaticPrototype,
    /// §15.2.1 and §16.1.1: `super.a` outside any method.
    ///
    /// Legal in every `MethodDefinition`, including an object literal's, because every method has
    /// a home object. A plain function has none, and neither does the top level.
    SuperPropertyOutsideMethod,
    /// §15.7.1: `super(…)` outside the constructor of a derived class.
    ///
    /// It calls the parent constructor, so it needs a parent: `class C { constructor() { super(); } }`
    /// is refused where the same with `extends D` is not.
    SuperCallOutsideDerivedConstructor,
    /// §13.2.5.1: two `__proto__` properties written as `PropertyName : AssignmentExpression`.
    ///
    /// Only that production counts: a computed key and a shorthand are invisible to the rule,
    /// because only that one sets the prototype rather than defining an ordinary property.
    DuplicateProto,
    /// §13.2.5.1: `{a = 1}` outside a destructuring pattern.
    ///
    /// `CoverInitializedName` exists only so the cover grammar can reach `({a = 1} = b)`, and the
    /// specification says to always throw a Syntax Error where it is matched as a literal.
    ShorthandPropertyWithInitializer,
    /// §14.7.5: a `for`-`of` whose target begins with the token `let`.
    ///
    /// `[lookahead ∉ { let, async of }]`, a one-token restriction — so `for (let.a of b)` is
    /// refused while `for (let.a in b)` and `for ((let) of b)` are not.
    ForOfTargetBeginsWithLet,
    /// §14.7.5: a `for`-`of` whose target is exactly the identifier `async`.
    ///
    /// The other half of the same restriction, and a two-token one: it is the sequence
    /// `async of` that has no derivation, so `for (async.x of b)` is fine.
    AsyncAsForOfTarget,
    /// §14.7.5: a `for`-`in` or `for`-`of` header binding more than one name.
    ///
    /// `ForBinding` is singular — `for (var a, b in c)` has no derivation.
    ForInOfBindsSeveralNames,
    /// §14.7.5: a `for`-`in` or `for`-`of` binding with an initialiser.
    ///
    /// `ForBinding` has no `Initializer` in the grammar. Annex B.3.5 restores one to `var` with
    /// `in` in non-strict code, which this parser refuses until it can tell strict code apart.
    ForInOfBindingHasInitializer,
    /// §14.12: a `switch` with more than one `default` clause.
    ///
    /// `CaseBlock : { CaseClauses_opt DefaultClause CaseClauses_opt }` admits exactly one, so a
    /// second is a missing production rather than an early error.
    MultipleDefaultClauses,
    /// §14.15: a `try` with neither a `catch` nor a `finally`.
    ///
    /// There is no `TryStatement : try Block`, so this is a missing production rather than an
    /// early error — the statement was never grammatical, not merely pointless.
    TryWithoutHandler,
    /// §14.15.1: the catch parameter is declared again at the handler's own level.
    ///
    /// `catch (e) { let e; }`. `LexicallyDeclaredNames`, so a nested block is a different scope
    /// and `catch (e) { { let e; } }` is fine.
    CatchParameterRedeclared,
    /// §14.14: a line terminator between `throw` and its value.
    ///
    /// The one restricted production with no shorter form to fall back on. Where `a\n++b` simply
    /// becomes two statements, `throw\na` becomes a `throw` with nothing to throw, and there is
    /// no such statement — so this is an error rather than a quietly different program.
    NewlineAfterThrow,
    /// §8.3.1: a label repeats one that already encloses it.
    ///
    /// `a: a: ;` and `a: { a: ; }` alike — the label set of §8.3.1 passes through every
    /// construct, so no amount of nesting between the two makes them different labels.
    DuplicateLabel,
    /// §8.3.2: `break a;` with no enclosing `a:`.
    UndefinedBreakTarget,
    /// §8.3.3: `continue a;` where `a:` labels something that is not a loop.
    ///
    /// Not the same as no such label: `a: { while (1) continue a; }` has one, and it names the
    /// block. Only a label written directly on a loop can be continued.
    UndefinedContinueTarget,
    /// §14.9.1: a `break` that is not inside a loop or a `switch`.
    ///
    /// Stated about `break ;` alone, so a labelled `break` needs no such thing — it needs only a
    /// label that exists.
    BreakOutsideLoop,
    /// §14.8.1: a `continue` that is not inside a loop.
    ///
    /// Stated about both forms, unlike §14.9.1 — a `continue` is inside a loop whatever it
    /// names, or it is an error.
    ContinueOutsideLoop,
    /// §13.13: `??` may not be mixed with `&&` or `||` without parentheses.
    ///
    /// `CoalesceExpressionHead` admits a `CoalesceExpression` or a `BitwiseORExpression` and
    /// nothing else, and `ShortCircuitExpression` keeps the two families apart in the other
    /// direction — so `a || b ?? c` and `a ?? b || c` are both errors, for the same reason as
    /// above: no reader would agree on what they meant.
    MixedCoalesceAndLogical,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexical(kind) => write!(f, "{kind}"),
            Self::Unexpected { expected, found } => {
                write!(f, "expected {expected}, found ")?;
                // A token with one spelling is quoted; one whose text varies is named by its
                // category, because "found `x`" is no help when the complaint is that an
                // identifier cannot stand there at all.
                match found {
                    TokenKind::Eof => f.write_str("end of input"),
                    TokenKind::Identifier { .. } => f.write_str("an identifier"),
                    TokenKind::PrivateIdentifier { .. } => f.write_str("a private name"),
                    TokenKind::Number { .. } => f.write_str("a number"),
                    TokenKind::BigInt => f.write_str("a bigint literal"),
                    TokenKind::String { .. } => f.write_str("a string"),
                    TokenKind::RegExp => f.write_str("a regular expression"),
                    TokenKind::Template { .. } => f.write_str("a template"),
                    // Everything left is a punctuator or a keyword, and every one of those has
                    // exactly one spelling — `as_str` cannot be `None` here, and asking for a
                    // default rather than testing for it keeps a branch out of the message path.
                    fixed => write!(f, "`{}`", fixed.as_str().unwrap_or_default()),
                }
            }
            Self::TooDeeplyNested => write!(f, "expression nests too deeply"),
            Self::StrictReservedWord => {
                write!(f, "this name is reserved in strict mode code")
            }
            Self::StrictEvalOrArguments => write!(
                f,
                "strict mode code may not bind or assign to `eval` or `arguments`"
            ),
            Self::StrictWith => {
                write!(f, "strict mode code may not include a `with` statement")
            }
            Self::StrictDeleteOfName => write!(
                f,
                "strict mode code may not `delete` a name, only a property"
            ),
            Self::StrictLegacyOctal => write!(
                f,
                "strict mode code may not use a legacy octal literal or a leading zero"
            ),
            Self::UseStrictWithNonSimpleParameters => write!(
                f,
                "a function with defaults, patterns or a rest may not declare `\"use strict\"`"
            ),
            Self::DeclarationInStatementPosition => {
                write!(f, "a declaration may not stand where only a statement may")
            }
            Self::ReturnOutsideFunction => {
                write!(f, "`return` is only allowed inside a function")
            }
            Self::DuplicateParameterName => write!(
                f,
                "this parameter name is bound twice, which a list with defaults or patterns may not do"
            ),
            Self::ParameterRedeclaredInBody => {
                write!(f, "this name is already a parameter of the function")
            }
            Self::PatternWithoutInitializer => {
                write!(f, "a destructuring declaration must have an initializer")
            }
            Self::DuplicateCatchParameterName => {
                write!(f, "this name is bound twice by the same catch parameter")
            }
            Self::ConstWithoutInitializer => {
                write!(f, "a `const` binding must have an initializer")
            }
            Self::LetAsLexicalBindingName => {
                write!(f, "`let` may not be the name of a `let` or `const` binding")
            }
            Self::DuplicateLexicalBinding => {
                write!(f, "this name is bound twice in the same declaration")
            }
            Self::ConflictingVarAndLexicalDeclaration => write!(
                f,
                "this name is declared by `var` and by `let` or `const` in the same scope"
            ),
            Self::InvalidDestructuringTarget => {
                write!(f, "this expression cannot be destructured into")
            }
            Self::RestElementMustBeLast => {
                write!(f, "a `...` element must be the last thing in a pattern")
            }
            Self::RestElementWithInitializer => {
                write!(f, "a `...` element may not have a default")
            }
            Self::RestTargetMayNotBePattern => write!(
                f,
                "a `...` property must name somewhere to put an object, not a pattern"
            ),
            Self::BadEscapeInUntaggedTemplate => write!(
                f,
                "this escape is only allowed in a template that is passed to a tag"
            ),
            Self::InvalidArrowParameter => {
                write!(f, "this cannot be an arrow function parameter")
            }
            Self::CoverGroupIsNotAnExpression => write!(
                f,
                "these parentheses are only an expression when `=>` follows them"
            ),
            Self::YieldInParameters => write!(
                f,
                "`yield` may not appear in a generator's own parameter list"
            ),
            Self::ConstructorMayNotBeAGenerator => {
                write!(f, "`constructor` may not be a generator")
            }
            Self::DuplicateConstructor => {
                write!(f, "a class may have only one `constructor`")
            }
            Self::ConstructorMayNotBeAnAccessor => {
                write!(f, "`constructor` may not be a getter or a setter")
            }
            Self::StaticPrototype => {
                write!(f, "a static method may not be named `prototype`")
            }
            Self::SuperPropertyOutsideMethod => {
                write!(f, "`super` is only allowed inside a method")
            }
            Self::SuperCallOutsideDerivedConstructor => write!(
                f,
                "`super()` is only allowed in the constructor of a class with `extends`"
            ),
            Self::AccessorParameterCount => write!(
                f,
                "a getter takes no parameters and a setter takes exactly one"
            ),
            Self::DuplicateProto => {
                write!(f, "an object literal may set `__proto__` only once")
            }
            Self::ShorthandPropertyWithInitializer => write!(
                f,
                "a shorthand property may not have an initializer outside a pattern"
            ),
            Self::ForOfTargetBeginsWithLet => {
                write!(f, "the target of a `for`-`of` may not begin with `let`")
            }
            Self::AsyncAsForOfTarget => {
                write!(f, "the target of a `for`-`of` may not be `async`")
            }
            Self::ForInOfBindsSeveralNames => {
                write!(
                    f,
                    "a `for`-`in` or `for`-`of` header binds exactly one name"
                )
            }
            Self::ForInOfBindingHasInitializer => write!(
                f,
                "a `for`-`in` or `for`-`of` binding may not have an initializer"
            ),
            Self::MultipleDefaultClauses => {
                write!(f, "a `switch` may have only one `default` clause")
            }
            Self::TryWithoutHandler => {
                write!(f, "a `try` needs a `catch` or a `finally`")
            }
            Self::CatchParameterRedeclared => write!(
                f,
                "the catch parameter is already declared in the catch block"
            ),
            Self::NewlineAfterThrow => {
                write!(f, "the value thrown must be on the same line as `throw`")
            }
            Self::DuplicateLabel => {
                write!(f, "this label already encloses this statement")
            }
            Self::UndefinedBreakTarget => write!(f, "no enclosing statement has this label"),
            Self::UndefinedContinueTarget => {
                write!(
                    f,
                    "this label is not on a loop, so `continue` cannot name it"
                )
            }
            Self::BreakOutsideLoop => write!(f, "`break` is not inside a loop or a `switch`"),
            Self::ContinueOutsideLoop => write!(f, "`continue` is not inside a loop"),
            Self::InvalidAssignmentTarget => {
                write!(f, "this expression cannot be assigned to")
            }
            Self::ExponentiationOnUnary => write!(
                f,
                "the left operand of `**` may not be an unparenthesized unary expression"
            ),
            Self::MixedCoalesceAndLogical => write!(
                f,
                "`??` may not be mixed with `&&` or `||` without parentheses"
            ),
        }
    }
}

impl From<LexError> for ParseError {
    fn from(error: LexError) -> Self {
        Self {
            kind: ParseErrorKind::Lexical(error.kind),
            span: error.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::test_support::*;
    #[test]
    fn every_parse_error_says_what_it_wanted_and_what_it_found() {
        // "Errors carry spans and read like a good compiler's" (AGENTS.md). The message is built
        // without the source, so a host that has only the error can still render something a
        // person can act on.
        assert_eq!(
            error("(1").kind.to_string(),
            "expected `)`, found end of input"
        );
        assert_eq!(
            error("(1 2)").kind.to_string(),
            "expected `)`, found a number"
        );
        assert_eq!(
            error("var").kind.to_string(),
            "expected an expression, found `var`"
        );
        assert_eq!(
            error("1 2").kind.to_string(),
            "expected end of input, found a number"
        );
        assert_eq!(
            error("1 x").kind.to_string(),
            "expected end of input, found an identifier"
        );
        assert_eq!(
            error("1 )").kind.to_string(),
            "expected end of input, found `)`"
        );
        assert_eq!(
            error("1 'a'").kind.to_string(),
            "expected end of input, found a string"
        );
        // A template after an operand is a *tag*, `MemberExpression TemplateLiteral` — so
        // `1 \`a\`` parses and throws at run time rather than failing here. Something that can
        // never begin one stands in for it.
        assert_eq!(
            error("1 ]").kind.to_string(),
            "expected end of input, found `]`"
        );
        assert_eq!(
            error("1 #a").kind.to_string(),
            "expected end of input, found a private name"
        );
        assert_eq!(
            error("1 2n").kind.to_string(),
            "expected end of input, found a bigint literal"
        );
        assert_eq!(
            error("1 ]").kind.to_string(),
            "expected end of input, found `]`"
        );
        // A regular expression can only stand where an operand may, and an operand may stand
        // wherever this grammar reaches — so there is no source that puts one somewhere
        // unexpected, and the message for it is checked by building the error directly.
        assert_eq!(
            ParseErrorKind::Unexpected {
                expected: "`)`",
                found: TokenKind::RegExp,
            }
            .to_string(),
            "expected `)`, found a regular expression"
        );
        assert_eq!(
            error("'abc").kind.to_string(),
            "unterminated string literal",
            "a lexical failure keeps its own words"
        );
        assert_eq!(
            ParseErrorKind::TooDeeplyNested.to_string(),
            "expression nests too deeply"
        );
    }
}
