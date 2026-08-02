//! Tokens to a syntax tree.
//!
//! # How the goal symbol is chosen
//!
//! The lexer refuses to guess whether a `/` is division or a regular expression, and hands the
//! question to whoever knows ([`Goal`]). This is that caller, and the rule it uses is a single
//! invariant, stated here once because every `Parser::advance` call depends on it:
//!
//! > **The goal is chosen when advancing *past* a token, by what may legally follow it.**
//!
//! A token that completes an operand is followed by an operator, so the parser advances past it
//! under [`Goal::Div`]. A token that demands an operand — an operator, an opening parenthesis,
//! the start of a statement — is followed by one, so the parser advances under [`Goal::RegExp`].
//! There is no lookahead buffer to invalidate and no rescanning, because a position is never
//! read twice: by the time the parser knows what a token is, it has already decided what may
//! come after it.
//!
//! # Recursion is bounded here, not by the operating system
//!
//! DR-0002 requires it, and requires it in the same commit as the recursion itself: a
//! recursive-descent parser handed `((((…` recurses once per bracket, and a stack overflow is
//! not a failure any `Result` can rescue — it takes the embedder's process with it. So every
//! recursive entry is counted, and refused past [`MAX_NESTING_DEPTH`]. The cap is a number we
//! chose rather than one the specification has an opinion about, and it is chosen from a
//! measurement of what a level of nesting actually costs — see the constant.

//! # How this module is laid out
//!
//! - `error` — [`ParseError`] and its kinds.
//! - `operator` — precedence, associativity, and the pairs §13 keeps apart.
//! - `expression` — the operator ladder of §13.4 – §13.16.
//! - `member` — `LeftHandSideExpression` (§13.3): member access, calls and `new`.
//! - `primary` — `PrimaryExpression` (§13.2), the operands everything else is built from.
//! - `array_literal` — `[…]` (§13.2.4), and the two different things a comma does inside one.
//! - `object_literal` — `{…}` (§13.2.5), which has no elisions and one rule about `__proto__`.
//! - `pattern` — refining either literal into the assignment pattern it covered (§13.15.5).
//! - `binding` — binding patterns (§14.3.3), which need no cover grammar and say so.
//! - `function` — function definitions (§15.2), and the `return` they make legal (§14.10).
//! - `strict` — where strict mode starts (§11.2.1) and what it takes away (§13.1.1).
//! - `method` — method definitions (§15.4), the last `PropertyDefinition` alternative.
//! - `arrow` — arrow functions (§15.3), and the cover grammar that reaches them.
//! - `template` — template literals (§13.2.8) and the tags that take them (§13.3).
//! - `statement` — the grammar of §14, and automatic semicolon insertion (§12.10).
//! - `declaration` — `var`, `let` and `const` (§14.3), and the early errors on them.
//! - `control` — conditionals, loops, `throw`, `break` and `continue` (§14.6 – §14.14).
//! - `for_statement` — the three-part `for` (§14.7.4), the one header read under `[~In]`.
//! - `for_in_of` — `for`-`in` and `for`-`of` (§14.7.5), which share that header.
//! - `labelled` — labelled statements (§14.13), the second and last place two tokens decide.
//! - `scope` — the early errors a statement list has about the names it declares (§14.2.1).
//! - `try_catch` — `try`, `catch` and `finally` (§14.15), and the early errors on a handler.
//! - `switch` — `switch` (§14.12), whose CaseBlock is one scope across all its clauses.
//! - `body` — what a function body inherits from the production that opened it.
//! - `class` — class definitions (§15.7), and the `super` they make legal (§13.3.7, §13.3.5).
//! - `class_element` — what a class body is made of: methods, fields and static blocks.
//! - `generator` — generators (§15.5), and the `[Yield]` grammar parameter they turn on.
//! - `asynchronous` — everything `async`: functions (§15.8), generators (§15.6), arrows (§15.9).
//! - `module` — the `Module` goal symbol and the `import` declarations only it admits (§16.2).
//! - `export` — §16.2.3's declarations, and the two rules that read the finished list.
//! - here — the `Parser` itself: the token it is looking at, how it advances, and the count
//!   that bounds its recursion.

mod array_literal;
mod arrow;
mod asynchronous;
mod binding;
mod body;
mod class;
mod class_element;
mod control;
mod declaration;
mod error;
mod export;
mod expression;
mod for_in_of;
mod for_statement;
mod function;
mod generator;
mod labelled;
mod member;
mod method;
mod module;
mod object_literal;
mod operator;
mod pattern;
mod primary;
mod scope;
mod statement;
mod strict;
mod switch;
mod template;
#[cfg(test)]
mod test_support;
mod try_catch;

pub use self::error::{ParseError, ParseErrorKind};
pub use self::module::parse_module;
#[cfg(test)]
pub(crate) use self::statement::parse_script_with_label_rules_unchecked;
pub use self::statement::{parse_eval, parse_script};

use crate::ast::Expr;
use crate::lexer::{Goal, Lexer, ReservedWord, Token, TokenKind};
use crate::span::Span;

/// How deeply the grammar may nest before the parser gives up.
///
/// ECMAScript sets no limit, so this is our refusal rather than the grammar's — which is the
/// point of giving it its own [`ParseErrorKind::TooDeeplyNested`] instead of dressing it up as a
/// syntax error.
///
/// # Where the number comes from
///
/// Measured in a debug build against a one-mebibyte stack — the smallest in common use — and
/// re-measured every time the grammar grows, because it falls every time the grammar grows:
///
/// | after | levels a mebibyte holds | cap |
/// | --- | --- | --- |
/// | primary expressions | 928 | 512 |
/// | prefix and binary operators | 304 | 128 |
/// | conditional, assignment, comma | 168 | 64 |
/// | member access, calls, `new`, update | 112 | 48 |
/// | conditionals and loops | 114 | 48 |
/// | `try`, `catch` and `finally` | 114 | 48 |
/// | `switch` | 114 | 48 |
/// | the `[In]` parameter, and `for` | 113 | 48 |
/// | `for`-`in` and `for`-`of` | 113 | 48 |
/// | labelled statements, and `with` | 113 | 48 |
/// | array literals | 82 | 48 |
/// | object literals | 73 | 48 |
/// | destructuring assignment patterns | 67 | 48 |
/// | functions, and `return` | 67 | 48 |
/// | arrow functions | 61 | 48 |
///
/// Each slice put another function between one bracket and the next. That is the trajectory to
/// expect, and it is why keeping the recursive path narrow counts as correctness work rather
/// than optimisation: every frame removed is nesting a real program is allowed to have. Two
/// slices have now bought depth back by moving locals out of a frame the recursion passes
/// through — the trick works because a debug build reuses no stack slots between match arms, so
/// an arm that cannot recurse is still paid for by every level that does.
///
/// The last row is the first in four slices to cost anything, and it cost one level: threading
/// the `[In]` grammar parameter through the five functions between `Expression` and
/// `RelationalExpression` puts one more local in each of their frames. That is what a grammar
/// parameter costs, it was paid knowingly, and the alternative — holding the flag on the parser —
/// would have saved it by making every place that resets `[+In]` a thing to remember rather than
/// a thing the compiler asks about.
///
/// The three before it cost nothing, and the reason is worth keeping: the count is one budget
/// shared by every kind of nesting, so what bounds it is whichever kind spends the most stack per
/// level. Statements are cheap next to expressions — a level of `if` is three frames where a
/// level of `(` is the whole precedence ladder — and measured alone they afford 339 levels,
/// `with` 508, `while` 504, a label 476, a block 392, a `for` 254, a `try` 221, a `for`-`in`
/// 202, a `switch` 185. None of them came near the expressions.
///
/// The array literal is the first thing that did, and it took the lead: `[[[…]]]` recurses
/// through the whole precedence ladder *and* two frames of its own, so it affords 82 levels where
/// `(((…)))` affords 113. The object literal then took it from the array. Expressions no longer
/// set this number — the narrowest bracket does, and every literal with a bracket in it is a
/// candidate, which is now most of them.
///
/// As of the arrows, the standings are: object literal 61, array literal 77, an arrow 130, a
/// parenthesized expression 138, a function 251. The parentheses got *cheaper*: an assignment
/// level now opens them itself, looking for a `=>`, which is a shallower path than the operand
/// ladder they used to be read through. The refinement is not the binding one and was never likely to be — it
/// recurses over a tree the parse has already finished with, so its frames replace the parse's
/// rather than adding to them.
///
/// Stack is not the only thing a level spends, though, and the two newest forms show it. A `try`
/// takes *two* of the count on each level, one for the statement and one for its guarded `Block`,
/// so `try {` nests 32 deep against a cap of 64 where `{` nests 64. A `switch` takes one for its
/// CaseBlock and borrows a second while it reads the expression after `case`, so it nests 63. In
/// both the count is doing exactly what it should: those really are separate scopes and separate
/// descents, and the cap is about what the machine can afford rather than about tidy numbers.
///
/// A class costs one level for the whole definition, which bounds both of its recursions at
/// once: `class C extends class … {}` through the heritage, and `class C { m() { class D …`
/// through the method bodies. Nothing else was counting the second — a class body is not a
/// `Block` and a function body does not count either — so it was unbounded until this slice.
///
/// `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need` runs a full-depth parse of each
/// recursive path in a thread with exactly one mebibyte. That test is the real specification of
/// this constant: raise the cap, or make a level cost more stack, and it fails.
///
/// # What the measurement says, and why the number is 64
///
/// It was 48, and a sweep of real code is what moved it: a generated protobuf file writes fifty
/// `exports.A = exports.B = … = void 0` assignments in one statement, which right-associate into
/// fifty levels. Every engine takes it and this one refused it. Nothing about 48 was measured — it
/// was a first guess that never had a program argue with it.
///
/// So each path was bisected against one mebibyte in a *debug* build, one process per candidate,
/// because a stack overflow aborts and cannot be caught. Per level:
///
/// | shape | levels in 1 MiB | per level |
/// | --- | --- | --- |
/// | `{` a block | 327 | 3.1 KiB |
/// | `(` a parenthesized expression | 152 | 6.7 KiB |
/// | `[` an array literal | 71 | 14.4 KiB |
/// | `[` refined into a pattern | 70 | 14.6 KiB |
///
/// Seventy is the ceiling, so 64 is the cap: it clears the evidence with thirteen levels to
/// spare and leaves the narrowest path a margin of 1.09×. That margin is thin and is stated
/// rather than hidden — the ratio is comfort and the test below is the guarantee, which is the
/// same position this note took when the margin was 1.3×. A debug build is what is being measured
/// and a release one is several times cheaper, so an embedder's real margin is far larger; the
/// thin number is a warning about *this repository's* CI, not about shipped code.
///
/// The array literal looked like the outlier and is not, which `lab/`'s `nesting-cost` experiment
/// settled: `a[0][0]…` costs the same 14.6 KiB and never touches `parse_array_literal`. What both
/// pay for is the descent `parse_assignment -> parse_binary -> parse_unary -> parse_member ->
/// parse_primary`, about six frames at the 2.5 KiB one frame costs — and `(` is cheap for the
/// complementary reason, being intercepted at the assignment level before the ladder starts.
///
/// So there is nothing here to make cheaper: no frame is fat, and shaving two of them off the
/// path measured *worse* than leaving them. The same experiment found that release is 5.5× cheaper
/// than debug, which is where the headroom actually is — the cap could be around 260 at a
/// comfortable margin if it were allowed to depend on the build, and DR-0006 says it may not. The
/// lab notebook has the numbers and the argument.
///
/// # What real code asks for
///
/// 64 is a number this repository chose; the number the world asks for was measured separately,
/// by sweeping 4,733 minified files — 120 MB of what npm actually ships, plus every built library
/// WordPress and Moodle vendor. Two files went past the cap. They are two copies of the same
/// Emscripten-generated Draco decoder, and bisecting the constant against one of them says it
/// needs **77**: thirteen more than there is.
///
/// That is close enough to be worth stating precisely and still out of reach, because 77 is past
/// the 70 the narrowest path survives in the build being asserted against. Nothing else came
/// near: WordPress's and Moodle's 4,589 built files all parse, and so does every other bundle
/// fetched. The shape is not "minified code nests deeply" — it is one code generator emitting
/// labelled blocks where a person would write anything else.
///
/// # Why a count and not a stack measurement
///
/// Because a stack measurement would make which programs parse depend on how the engine was
/// compiled, and this project's whole premise is a conformance number that does not drift.
/// DR-0006 has the argument, including what it costs — a release build could afford several
/// times this and is not allowed to. The limit becomes an embedder-set value at M3, where
/// somebody knows how much stack there actually is; the default stays conservative.
pub const MAX_NESTING_DEPTH: u32 = 64;

/// Parse `source` as a single expression, which must be all of it.
///
/// A convenience beside [`parse_script`], which is the entry point a program goes through.
/// Useful where an expression is all there is — and, more often, in tests about one.
///
/// ```
/// use praxis::ast::ExprKind;
/// use praxis::parser::parse_expression;
///
/// let expr = parse_expression("(1)").expect("this parses");
/// assert_eq!(expr.kind, ExprKind::Number(1.0));
/// assert!(expr.parenthesized);
/// ```
pub fn parse_expression(source: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(source)?;
    let expr = parser.parse_expression(self::expression::AllowIn::Yes)?;
    parser.expect_eof()?;
    // The same §15.7.7 sweep [`parse_script`] does, for the same reason.
    if let Some((_, span)) = parser.private_references.first() {
        return Err(ParseError {
            kind: ParseErrorKind::UndeclaredPrivateName,
            span: *span,
        });
    }
    Ok(expr)
}

/// A rule an `ObjectLiteral` owes, held until something says whether it is still a literal.
///
/// See [`Parser::unrefined_covers`] for what is recorded and [`Parser::discard_refined_covers`]
/// for how one settles.
pub(super) struct CoverRecord {
    /// The error this becomes if it is never refined away.
    pub(super) error: ParseError,
    /// The literal that owes the rule, which is what a refinement asks about.
    ///
    /// Not the error's span: that points at the offending property, because that is what a
    /// reader needs to see. A refinement needs to know whether *the literal* is inside what it
    /// refined, and asking with the error's span would make the two boundaries unreachable — a
    /// property always lies strictly within its own braces, so neither end could ever be tested.
    pub(super) literal: Span,
    /// The start of the innermost sub-expression that would survive a refinement enclosing this
    /// record — an assignment's right operand, a `CoverInitializedName`'s default, a computed
    /// key — or `None` when there is no such sub-expression.
    ///
    /// This is what makes the difference between `({a: {b = 1}} = x)`, where the inner literal
    /// *is* a target and the rule goes away with it, and `({a = {b = 1}} = x)`, where the inner
    /// literal is a default: it stays an expression, so it keeps the rule and the whole thing is
    /// a Syntax Error. Both look identical to a refinement that only asks "was this inside the
    /// literal I refined".
    ///
    /// A start offset and not a span, because a region's extent is not known until it has been
    /// parsed and the records inside it are made before then. Spans nest, so a region that
    /// *starts* inside the refined literal is contained in it, which is the only question asked.
    pub(super) protected_from: Option<u32>,
}

/// A recursive-descent parser over one source text.
struct Parser<'a> {
    pub(super) source: &'a str,
    lexer: Lexer<'a>,
    /// The token under consideration. Always already lexed — see the module documentation on how
    /// its goal was chosen.
    pub(super) current: Token,
    /// How many recursive entries are open. See [`Parser::enter`].
    depth: u32,
    /// Rules an `ObjectLiteral` owes that a *pattern* does not, and that nothing has yet
    /// refined away.
    ///
    /// The cover grammar's bookkeeping (§13.2.5.1, §13.15.5). Two rules are recorded here and
    /// both for the same reason: they belong to `ObjectLiteral`, and a literal that turns out to
    /// be an `ObjectAssignmentPattern` never matched that production, so neither rule ever
    /// reached it.
    ///
    /// - A `CoverInitializedName` — `{a = 1}` — is a legal *pattern* and never a legal literal.
    /// - A duplicate `__proto__` is refused in a literal and ordinary in a pattern, which sets
    ///   the same target twice.
    ///
    /// A list rather than one slot, because refinement has to be able to drop *some* of them:
    /// see [`Parser::discard_refined_covers`] for the rule and for what went wrong when it was
    /// a single record cleared wholesale.
    pub(super) unrefined_covers: Vec<CoverRecord>,
    /// Where the innermost sub-expression that survives refinement begins, if one is open.
    ///
    /// Stamped onto every record made while it is set — see [`CoverRecord::protected_from`].
    pub(super) protecting_from: Option<u32>,
    /// Whether this is strict mode code (§11.2.1).
    ///
    /// Not a grammar parameter — the specification threads strictness through `IsStrict`, which
    /// asks whether a node is *contained in* strict code, and that is a fact about where you are
    /// rather than a decision at each step. Set by a Directive Prologue, inherited by everything
    /// within, and never turned off: a function body may make itself strict and may not make
    /// itself sloppy, so the saving and restoring around one only matters on the way out.
    pub(super) strict: bool,
    /// Whether a `FunctionBody` encloses this — the `[Return]` grammar parameter of §14.10.
    ///
    /// A field rather than a parameter, where `[In]` is a parameter, and the difference is which
    /// kind of fact each is. `[In]` resets at every bracket, so each bracket is a decision worth
    /// making the compiler ask about. `[Return]` is set by one production and never turned off
    /// within it, so it is not a decision anywhere: it is where you are.
    pub(super) inside_function: bool,
    /// What the enclosing function grants — `super` and `new.target` (§13.3).
    ///
    /// A field for the same reason as `inside_function`, and saved and restored at the same
    /// place. See [`self::body`] for why an arrow passes it through and a function replaces
    /// it.
    pub(super) body_context: self::body::BodyContext,
    /// The `[Yield]` grammar parameter (§15.5) — whether `yield` is an operator here.
    ///
    /// A field for the reason `inside_function` is one, and unlike it in that a nested ordinary
    /// function turns it back off. Every place it changes is a place this parser already saves
    /// state, so it costs one field; [`self::generator`] has the table of where and why.
    pub(super) yield_allowed: bool,
    /// The `[Await]` grammar parameter (§15.8) — whether `await` is an operator here.
    ///
    /// `[Yield]`'s twin in every structural respect; [`self::asynchronous`] has the four places
    /// they differ.
    pub(super) await_allowed: bool,
    /// The error a parameter list owes, if one was read since the last function boundary.
    ///
    /// `Contains YieldExpression` (§15.5.1) and `Contains AwaitExpression` (§15.8.1) asked as a
    /// record rather than as a walk, because `Contains` stops at a function boundary and so does
    /// this — it is saved and restored by [`Parser::parse_function_body`] and by the parameter
    /// list. The same deferral as `unrefined_covers`, for the same reason: the question is
    /// asked later than the answer is known.
    ///
    /// One field and not two: whichever of the two expressions a given parameter list can contain
    /// is the one forbidden there, a generator's parameters being `[~Await]` and an async
    /// function's `[~Yield]`. So it holds the finished error rather than a span and a kind.
    pub(super) forbidden_in_parameters: Option<ParseError>,
    /// Where the name `arguments` was read, since the last function boundary.
    ///
    /// §15.7.9's `ContainsArguments`, asked as a record for the reason the two above are: it
    /// stops at a function boundary and so does this. It does *not* stop at an arrow, which is
    /// what makes `class C { a = () => arguments; }` a Syntax Error and
    /// `class C { a = function () { arguments; }; }` an ordinary field.
    pub(super) arguments_reference: Option<Span>,
    /// Every `#a` read whose declaration has not been found yet.
    ///
    /// §15.7.7's `AllPrivateIdentifiersValid`, which cannot be answered where the name is read:
    /// `class C { m() { this.#a; } #a; }` is legal, so the answer is not known until the class
    /// body closes. Each class body removes the names it declares and leaves the rest for the
    /// class around it; whatever survives to the end of the script was never declared anywhere.
    pub(super) private_references: Vec<(Box<str>, Span)>,
    /// Whether the goal symbol is `Module` rather than `Script` (§16.2).
    ///
    /// Not the same question as `await_allowed`: the parameter is `[~Await]` inside a plain
    /// function however the file was parsed, and §13.1.1 refuses `await` as an identifier in a
    /// module regardless. See [`self::module`] for the five things the goal decides.
    pub(super) module: bool,
    /// How many brackets are open that could still turn what is inside them into a pattern or a
    /// binding.
    ///
    /// What makes the record above a *deferred* error rather than an immediate one. Three
    /// constructs count: an array literal and an object literal, either of which may become a
    /// pattern; and a parenthesized group, which may become arrow parameters. Inside any of them
    /// nothing is decided, so `[{a = 1}] = b` and `({a = 1}) => b` both parse.
    ///
    /// At nought the question is settled, which is why `f({a = 1})` and `({a = 1})` are the
    /// Syntax Error §13.2.5.1 describes — the outermost `AssignmentExpression` always asks, and
    /// by then no bracket is left to make it legal.
    pub(super) open_covers: u32,
}

impl<'a> Parser<'a> {
    /// A parser positioned on the first token of `source`.
    ///
    /// That token is read under [`Goal::RegExp`], because a program begins where an operand may
    /// stand: a leading `/` opens a regular expression and never divides.
    fn new(source: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token(Goal::RegExp)?;
        Ok(Self {
            source,
            lexer,
            current,
            depth: 0,
            unrefined_covers: Vec::new(),
            protecting_from: None,
            open_covers: 0,
            inside_function: false,
            body_context: self::body::BodyContext::SCRIPT,
            yield_allowed: false,
            await_allowed: false,
            forbidden_in_parameters: None,
            arguments_reference: None,
            private_references: Vec::new(),
            module: false,
            strict: false,
        })
    }

    /// Whether this token can stand where §13.1 wants an `Identifier`.
    ///
    /// `Identifier : IdentifierName but not ReservedWord`, and `yield` and `await` are both
    /// reserved words — so on the face of it neither could ever be a name. §13.1 gives all three
    /// identifier productions extra alternatives that say otherwise:
    ///
    /// ```text
    /// IdentifierReference[Yield, Await] : Identifier | [~Yield] yield | [~Await] await
    /// BindingIdentifier[Yield, Await]   : Identifier |          yield |          await
    /// LabelIdentifier[Yield, Await]     : Identifier | [~Yield] yield | [~Await] await
    /// ```
    ///
    /// The `BindingIdentifier` row takes `yield` unconditionally and leaves the refusing to
    /// §13.1.1's early error "It is a Syntax Error if this production has a `[Yield]` parameter";
    /// the other two rows are gated in the grammar itself. The two routes reach the same place, so
    /// one question is asked here for all three and the answer is [`Parser::yield_allowed`].
    ///
    /// Both parameters are asked the same way and for the same reason. The `Module` goal is the
    /// other thing that sets `[+Await]`, and it arrives with modules.
    pub(super) fn is_identifier_token(&self, kind: TokenKind) -> bool {
        match kind {
            TokenKind::Identifier { .. } => true,
            TokenKind::Keyword(ReservedWord::Yield) => !self.yield_allowed,
            // §13.1.1 refuses `await` as any of the three under the `Module` goal, whatever
            // the parameter says — so a plain function body inside a module has no `await`
            // as an operator *and* no `await` as a name.
            TokenKind::Keyword(ReservedWord::Await) => !self.await_allowed && !self.module,
            _ => false,
        }
    }

    /// The name of a `PrivateIdentifier` token, without its `#`, recorded as a reference.
    ///
    /// §12.7's `StringValue` of one *includes* the `#`; praxis keeps the name alone, the `#`
    /// being punctuation of the production rather than part of the name. The two spellings are
    /// never mixed, so nothing has to strip it back off.
    pub(super) fn private_name(&mut self, token: Token) -> Result<Box<str>, ParseError> {
        let name = self.private_name_only(token)?;
        self.private_references.push((name.clone(), token.span));
        Ok(name)
    }

    /// The same name, without recording a reference — for the declaring positions, which are what
    /// the references are resolved *against*.
    pub(super) fn private_name_only(&mut self, token: Token) -> Result<Box<str>, ParseError> {
        // The `#` is one byte and ASCII, so the name is the rest of the span.
        let inner = Span::new(token.span.start + 1, token.span.end);
        let name = crate::lexer::identifier_value(self.source, inner)
            .ok_or_else(|| self.value_missing(token))?;
        Ok(name.into_owned().into_boxed_str())
    }

    /// Record a reading of the name `arguments`, for §15.7.9's `ContainsArguments`.
    ///
    /// Called from the two places a name is read as a name — a reference and a binding — and from
    /// nowhere that reads a *property* name, which is why `a.arguments` is not one.
    pub(super) fn note_arguments(&mut self, name: &str, span: Span) {
        if name == "arguments" {
            self.arguments_reference.get_or_insert(span);
        }
    }

    /// Consume the current token and read the next one under `goal`.
    ///
    /// The returned token is the one just consumed, which is almost always the one the caller
    /// wanted to look at — so `let token = self.advance(…)?` reads as "take this and move on".
    pub(super) fn advance(&mut self, goal: Goal) -> Result<Token, ParseError> {
        let consumed = self.current;
        self.current = self.lexer.next_token(goal)?;
        Ok(consumed)
    }

    /// Read the current token again, under a different goal symbol.
    ///
    /// The parser's invariant is that a position is never read twice — the goal is chosen when
    /// advancing *past* a token, by what may legally follow it. A template substitution is the one
    /// place that cannot work: the `}` that ends it is read by whatever finished the expression,
    /// which has no way to know a template is waiting. So it is read again from the same offset,
    /// and the invariant is stated as having exactly this exception rather than quietly not
    /// holding. See [`super::template`].
    pub(super) fn reread_current(&mut self, goal: Goal) -> Result<(), ParseError> {
        let mut lexer = Lexer::resume_at(self.source, self.current.span.start);
        self.current = lexer.next_token(goal)?;
        self.lexer = lexer;
        Ok(())
    }

    /// The token after the current one, read under `goal`.
    ///
    /// A copy of the lexer reads it, so nothing is buffered and nothing is invalidated: the
    /// lexer is two string slices, and lexing from a copy leaves the real one exactly where it
    /// was. The goal is a parameter for the same reason it is everywhere else — the caller is
    /// the one who knows what could legally stand there.
    ///
    /// Used sparingly, and only where the grammar genuinely needs two tokens to decide: `let`
    /// is a declaration or an identifier depending on what follows it, and nothing shorter than
    /// looking answers that.
    pub(super) fn peek(&self, goal: Goal) -> Result<Token, ParseError> {
        let mut lookahead = self.lexer;
        Ok(lookahead.next_token(goal)?)
    }

    /// Whether the current token is the contextual keyword `word`.
    ///
    /// `let`, `of` and `async` are ordinary identifiers to the lexer, and keywords only where a
    /// production says so — so recognising one means comparing its text. Written without escapes
    /// is part of the test: §5.1.5.1 makes a terminal match literal source characters, so an
    /// escaped spelling is a name and never the keyword.
    pub(super) fn at_contextual(&self, word: &str) -> bool {
        matches!(
            self.current.kind,
            TokenKind::Identifier {
                contains_escape: false
            }
        ) && self.current.span.slice(self.source) == Some(word)
    }

    /// Open one level of nesting, refusing rather than recursing past [`MAX_NESTING_DEPTH`].
    ///
    /// Paired with [`Parser::leave`] rather than wrapping a closure, because a closure costs two
    /// stack frames per level and the whole point of the count is to spend as few as possible.
    /// The pairing is checked by a test that a *failed* nested parse still leaves the count
    /// where it found it, since that is the case a stray `?` would break.
    pub(super) fn enter(&mut self) -> Result<(), ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(ParseError {
                kind: ParseErrorKind::TooDeeplyNested,
                span: self.current.span,
            });
        }
        self.depth += 1;
        Ok(())
    }

    /// Close one level of nesting.
    pub(super) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// The error for "the grammar wanted `expected` here".
    pub(super) fn unexpected(&self, expected: &'static str) -> ParseError {
        ParseError {
            kind: ParseErrorKind::Unexpected {
                expected,
                found: self.current.kind,
            },
            span: self.current.span,
        }
    }

    /// Consume the current token if it is `kind`, reading the next under `goal`.
    pub(super) fn eat(
        &mut self,
        kind: TokenKind,
        goal: Goal,
        expected: &'static str,
    ) -> Result<Token, ParseError> {
        if self.current.kind != kind {
            return Err(self.unexpected(expected));
        }
        self.advance(goal)
    }

    /// Require that nothing follows.
    pub(super) fn expect_eof(&self) -> Result<(), ParseError> {
        if self.current.kind != TokenKind::Eof {
            return Err(self.unexpected("end of input"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
