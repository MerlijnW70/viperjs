//! The pieces the literal forms are made of (ECMAScript §13.2), and the regular-expression node.
//!
//! Split from [`super::expression`] because these are the *parts* rather than the expressions:
//! nothing here is an `ExprKind`, and each is held by exactly one variant of it. Keeping them
//! apart leaves that enum readable as the catalogue it is.

use super::Expr;
use crate::span::Span;

/// One element of an array literal (§13.2.4).
///
/// A hole is an element and not an absence: `[, 1]` has two of them, and `[1, ]` has one. The
/// difference is whether a comma had anything before it in its slot, which is the whole content
/// of `Elision` and the one thing about array literals that is easy to get wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElement {
    /// A comma with nothing before it — `[, 1]`, `[1, , 2]`. Reads as `undefined` and is not the
    /// same as one: the index is absent from the array rather than holding that value.
    Hole,
    /// An ordinary element.
    Value(Expr),
    /// `...a` — a `SpreadElement`, which contributes however many elements it turns out to
    /// have.
    Spread {
        /// What is spread.
        value: Expr,
        /// Whether a comma was written after it.
        ///
        /// A trailing comma leaves no element, so `[...a, ]` and `[...a]` become the same list —
        /// and they are not the same *pattern*, `AssignmentRestElement` and `BindingRestElement`
        /// both being last with nothing after them. The tree has to say so, because by refinement
        /// time the comma is the only thing that distinguished them and it is gone.
        ///
        /// On the element rather than on the parser, which is where it used to live: a record on
        /// the parser cannot say *which* literal it belongs to, so `[x = [...a,], [b]] = c` read
        /// it off the default and blamed the target.
        followed_by_comma: bool,
    },
}

/// One entry of an object literal (§13.2.5).
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyDefinition {
    /// `a: 1` — `PropertyName : AssignmentExpression`, the only production the `__proto__` rule
    /// of §13.2.5.1 counts.
    KeyValue {
        /// What names the property.
        key: PropertyKey,
        /// What it is set to.
        value: Expr,
    },
    /// `{a}` — an `IdentifierReference`, which is narrower than the `IdentifierName` a key may
    /// be: `{if: 1}` is a property and `{if}` has no derivation.
    Shorthand {
        /// The name, which is both the key and the value.
        name: Box<str>,
        /// Where it was written.
        span: Span,
    },
    /// `{a = 1}` — a `CoverInitializedName`, which is **not** a legal object literal.
    ///
    /// §13.2.5.1 says it is always a Syntax Error where an object literal stays one. It is here
    /// because the cover grammar needs it: `({a = 1} = b)` is a pattern, and the `=` that says so
    /// arrives long after this has been parsed. A literal that still holds one when the
    /// expression around it is finished is that Syntax Error — see [`crate::parser`].
    ShorthandWithDefault {
        /// The name, which is both the key and the target.
        name: Box<str>,
        /// What to use when the value is `undefined`.
        default: Box<Expr>,
        /// Where the name was written.
        span: Span,
    },
    /// `a() {}`, `get a() {}`, `set a(v) {}` — a `MethodDefinition` (§15.4).
    Method {
        /// What names the property.
        key: PropertyKey,
        /// Which of the three it is.
        kind: MethodKind,
        /// The function, which is never named: a method's name is the property's.
        function: Box<super::Function>,
    },
    /// `...a` — a spread, whose properties are copied in.
    Spread {
        /// What is spread.
        value: Expr,
        /// Whether a comma was written after it — see [`ArrayElement::Spread`], which
        /// carries it for the same reason. `BindingRestProperty` and
        /// `AssignmentRestProperty` are both last with nothing after them.
        followed_by_comma: bool,
    },
}

/// A `TemplateLiteral` (§13.2.8) — its literal parts, and the expressions between them.
///
/// `quasis` is always one longer than `expressions`: a template begins and ends with a literal
/// part, even when that part is empty. `` `${a}` `` has two empty ones.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateLiteral {
    /// The literal components, in order.
    pub quasis: Box<[TemplateElement]>,
    /// The substitutions, in order. One fewer than the components.
    pub expressions: Box<[Expr]>,
}

/// One literal component of a template (§12.9.6).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateElement {
    /// `TV`, the cooked value — `None` when the component holds a `NotEscapeSequence`, which is
    /// what the specification means by "undefined" there. Only a tagged template may have one,
    /// and §13.2.8.1 is why.
    pub cooked: Option<Vec<u16>>,
    /// `TRV`, the raw value. Always present, escapes left exactly as written.
    pub raw: Vec<u16>,
    /// The component including its delimiters.
    pub span: Span,
}

/// Which kind of `MethodDefinition` (§15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MethodKind {
    /// `a() {}` — an ordinary method.
    Normal,
    /// `get a() {}`, which takes no parameters.
    Get,
    /// `set a(v) {}`, which takes exactly one.
    Set,
}

/// What names a property (§13.2.5).
///
/// The source forms are kept apart rather than reduced to one string, because reducing them needs
/// `PropName`, and `PropName` of a `NumericLiteral` is `ToString` of its value — an abstract
/// operation this engine does not have yet. Inventing an approximation would be a bug that only
/// ever showed up in a property name.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKey {
    /// An `IdentifierName`, escapes resolved. Includes every reserved word.
    Identifier(Box<str>),
    /// A `StringLiteral`, as UTF-16 code units (DR-0004) — which may include a lone surrogate,
    /// and so is not a `str`.
    String(Box<[u16]>),
    /// A `NumericLiteral`, as its value.
    Number(f64),
    /// A `BigIntLiteral`, which §12.9.3 makes one of the `NumericLiteral` alternatives — so
    /// `({1n: 2})` is an ordinary property and `class C { 1n() {} }` an ordinary method.
    BigInt(Box<BigIntLiteral>),
    /// `[ AssignmentExpression ]`, whose name is not known until it runs.
    Computed(Box<Expr>),
    /// `#a` — a `PrivateIdentifier`, without its `#`.
    ///
    /// The second alternative of §15.7's `ClassElementName` and of nothing else, so only a
    /// class element ever carries one — an object literal's key is a `PropertyName` and has
    /// no private form. A private name is not a property name at all: it lives in a lexical
    /// space of its own, which is why §15.7.7 has to check that every use of one is in scope.
    Private(Box<str>),
}

impl PropertyKey {
    /// Whether this names `__proto__`, for §13.2.5.1.
    ///
    /// A computed key is not asked, the rule being about the other productions; and a numeric key
    /// of either kind cannot spell it, `PropName` of a number being the number written out.
    pub fn is_proto(&self) -> bool {
        match self {
            Self::Identifier(name) => &**name == "__proto__",
            Self::String(units) => units.iter().copied().eq("__proto__".encode_utf16()),
            // §13.2.5.1 is about an `ObjectLiteral`, which has no private keys at all.
            Self::Number(_) | Self::BigInt(_) | Self::Computed(_) | Self::Private(_) => false,
        }
    }
}

/// A `BigIntLiteral` (§12.9.3), as the digits it is made of.
///
/// Not as a value, because the value is a BigInt — a mathematical integer of no fixed width — and
/// this engine has no such type until M7. Approximating one with an `f64` would be a bug that
/// showed up only in the numbers BigInt exists for: `9007199254740993n` is the first integer an
/// `f64` cannot hold, and webpack's test suite writes it down precisely because of that.
///
/// So the parser does the part it can do without arithmetic — strip the separators, the prefix
/// and the suffix, and record which radix the prefix asked for — and leaves `StringToBigInt`
/// (§7.1.14) to whoever owns the value representation. Nothing has to be lexed twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigIntLiteral {
    /// 2, 8, 10 or 16 — what the `0b` / `0o` / `0x` prefix asked for, or ten when there was none.
    ///
    /// Never eight from Annex B: a `LegacyOctalIntegerLiteral` has no `BigIntLiteralSuffix`
    /// alternative at all, so `0123n` has no derivation and never reaches here.
    pub radix: u32,
    /// The digits, with every `NumericLiteralSeparator`, the radix prefix and the `n` removed.
    ///
    /// Never empty, and never signed: `-1n` is a unary minus applied to `1n`, exactly as it is for
    /// every other numeric literal.
    pub digits: Box<str>,
}

/// The two halves of a regular expression literal, as written.
///
/// Neither is parsed here: §12.9.5 says both "are subsequently parsed again using the more
/// stringent ECMAScript Regular Expression grammar", which is the RegExp engine's work at M4. So
/// an unparsable pattern is a perfectly good node, and stops being one later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegExpLiteral {
    /// `BodyText` (§12.9.5.1) — everything between the slashes.
    pub body: String,
    /// `FlagText` (§12.9.5.2) — everything after the closing slash. Often empty.
    pub flags: String,
}
