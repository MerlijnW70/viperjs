//! Tokens to a syntax tree.
//!
//! # How the goal symbol is chosen
//!
//! The lexer refuses to guess whether a `/` is division or a regular expression, and hands the
//! question to whoever knows ([`Goal`]). This is that caller, and the rule it uses is a single
//! invariant, stated here once because every [`Parser::advance`] call depends on it:
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
//! - here — the [`Parser`] itself: the token it is looking at, how it advances, and the count
//!   that bounds its recursion.

mod array_literal;
mod arrow;
mod binding;
mod body;
mod class;
mod control;
mod declaration;
mod error;
mod expression;
mod for_in_of;
mod for_statement;
mod function;
mod generator;
mod labelled;
mod member;
mod method;
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
pub use self::statement::parse_script;
#[cfg(test)]
pub(crate) use self::statement::parse_script_with_label_rules_unchecked;

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
/// so `try {` nests 24 deep against a cap of 48 where `{` nests 48. A `switch` takes one for its
/// CaseBlock and borrows a second while it reads the expression after `case`, so it nests 47. In
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
/// The margin over the narrowest path is 1.3×, and was two for the first several slices. It has
/// not been lowered to restore the ratio, because the ratio is comfort and the test is the
/// guarantee — buying a rounder number would cost real programs a third of the nesting they are
/// entitled to, for no failure that has happened. The trend is the thing to watch rather than the
/// number: each bracketed literal has taken a bite, and the next one may well be what forces the
/// cap down. The day the narrowest path falls below it, that test says so and the number moves.
///
/// # Why a count and not a stack measurement
///
/// Because a stack measurement would make which programs parse depend on how the engine was
/// compiled, and this project's whole premise is a conformance number that does not drift.
/// DR-0006 has the argument, including what it costs — a release build could afford several
/// times this and is not allowed to. The limit becomes an embedder-set value at M3, where
/// somebody knows how much stack there actually is; the default stays conservative.
pub const MAX_NESTING_DEPTH: u32 = 48;

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
    Ok(expr)
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
    /// Where the first `{a = 1}` was written, if one has been parsed and not yet refined away.
    ///
    /// The first half of the cover grammar's bookkeeping (§13.2.5.1, §13.15.5). A
    /// `CoverInitializedName` is a legal *pattern* and never a legal *literal*, so the literal
    /// parser accepts it and leaves this behind; refining the literal into a pattern clears it,
    /// and an expression that reaches the end of an `AssignmentExpression` still carrying one is
    /// the Syntax Error §13.2.5.1 describes.
    pub(super) cover_initialized_name: Option<Span>,
    /// Where a `...` element was followed by a comma, if one has been parsed.
    ///
    /// The other half, and the opposite way round: `[...a, ]` is a legal *literal* and never a
    /// legal *pattern*, an `AssignmentRestElement` being last with nothing after it. The two
    /// look identical once parsed — a trailing comma leaves no element — so the literal parser
    /// records it and refinement is what turns it into an error.
    pub(super) rest_followed_by_comma: Option<Span>,
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
    /// Where a `YieldExpression` was read, since the last function boundary.
    ///
    /// `Contains YieldExpression` (§15.5.1) asked as a record rather than as a walk, because
    /// `Contains` stops at a function boundary and so does this — it is saved and restored by
    /// [`Parser::parse_function_body`] and by the parameter list. The same deferral as
    /// `cover_initialized_name`, for the same reason: the question is asked later than the answer
    /// is known.
    pub(super) yield_expression: Option<Span>,
    /// How many array or object literals are open.
    ///
    /// What makes the record above a *deferred* error rather than an immediate one: inside a
    /// literal, an expression may still turn out to be part of a pattern, so nothing is decided.
    /// At nought, it cannot — which is why `[{a = 1}] = b` parses and `f({a = 1})` does not.
    pub(super) literal_depth: u32,
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
            cover_initialized_name: None,
            rest_followed_by_comma: None,
            literal_depth: 0,
            inside_function: false,
            body_context: self::body::BodyContext::SCRIPT,
            yield_allowed: false,
            yield_expression: None,
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
    /// `await` is still taken unconditionally: `[+Await]` comes from an `AsyncFunctionBody` or the
    /// `Module` goal, and this parser has no production that reaches either. A parameter with the
    /// same value on every path is not a parameter — it is a constant with untestable branches on
    /// it — so it arrives with the construct that varies it, as `[Yield]` just did.
    pub(super) fn is_identifier_token(&self, kind: TokenKind) -> bool {
        match kind {
            TokenKind::Identifier { .. } | TokenKind::Keyword(ReservedWord::Await) => true,
            TokenKind::Keyword(ReservedWord::Yield) => !self.yield_allowed,
            _ => false,
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
