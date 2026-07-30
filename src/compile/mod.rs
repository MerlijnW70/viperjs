//! The syntax tree to bytecode — §13's evaluation semantics, rearranged.
//!
//! # Why bytecode rather than walking the tree
//!
//! A tree-walking interpreter is shorter and would work. Bytecode is here for two reasons that
//! are not speed. The first is that generators and `async` functions have to *suspend*, and a
//! suspended tree walk is a stack of Rust frames that cannot be put down; a suspended bytecode
//! frame is an index. The second is that the flattening happens once per function rather than
//! once per execution, which is where the speed comes from and is not why it is here.
//!
//! Where a compiler's parts live.
//!
//! - `chunk` — the code an embedder holds, and the instruction set.
//! - `expression` — §13, and everything that leaves one value on the stack.
//! - `function` — §15.2, where a body becomes a chunk of its own and a call is emitted.
//! - `statement` — §14, and everything that leaves the stack as it found it.
//! - here — the [`Chunk`] an embedder holds, the instruction set, and how a name is resolved.
//!
//! # What it can compile so far
//!
//! Expressions over the values that exist: literals, the unary operators, and the binary
//! operators that need neither an object nor a name to look up. Everything else is a
//! [`ErrorKind::Unsupported`] carrying the span of the thing it could not do — a refusal with
//! a location, not a panic and not a silent wrong answer. That list shrinks with each slice, and
//! the errors are how a reader can tell what is genuinely finished.

use crate::ast::{Expr, Script};
use crate::value::Value;
mod chunk;
mod class;
mod expression;
mod function;
mod statement;

pub use self::chunk::{Chunk, Instruction, ShortCircuit, SpreadCall, Template};

use self::chunk::Unpatched;
use crate::heap::Heap;
use crate::span::Span;
use crate::static_semantics::var_declared_names;

/// Why a program could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// Where — the span of the construct that could not be compiled.
    pub span: Span,
}

/// The kinds of compilation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A construct the compiler does not handle yet, named as the specification names it.
    ///
    /// Not a syntax error: the parser accepted it and the tree is well-formed. This is the engine
    /// saying what it has not been taught, and it exists so that the answer to "does praxis
    /// support X" is a message with a span rather than a wrong value.
    Unsupported(&'static str),
    /// More than `u32::MAX` constants in one chunk.
    TooManyConstants,
    /// More than `u32::MAX` instructions in one chunk, so a jump could not name its target.
    TooLong,
    /// An expression nested deeper than the compiler will walk.
    TooDeep,
}

impl CompileError {
    /// A sentence describing the failure, without the span.
    pub fn message(&self) -> String {
        match self.kind {
            ErrorKind::Unsupported(what) => format!("{what} is not implemented yet"),
            ErrorKind::TooManyConstants => "too many constants in one unit of code".to_string(),
            ErrorKind::TooLong => "too many instructions in one unit of code".to_string(),
            ErrorKind::TooDeep => "an expression nested too deeply to compile".to_string(),
        }
    }
}

/// Compile one expression into a chunk whose completion value is the expression's.
///
/// Takes the heap because a String literal is a heap value and the compiler is where it is made.
pub fn compile_expression(expression: &Expr, heap: &mut Heap) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    compiler.expression(expression)?;
    compiler.chunk.emit(Instruction::SetCompletion);
    Ok(compiler.finish())
}

/// Compile a whole script — §16.1.7, whose value is the completion value of its body.
///
/// Every `var` in the script is given a slot before anything runs and starts as `undefined`, which
/// is what hoisting *is*: `x` is readable before its declaration and holds nothing.
pub fn compile_script(script: &Script, heap: &mut Heap) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler::new(heap);
    // §11.2.1 — a Script is strict only if its own Directive Prologue says so, and everything nested
    // inherits that. A Module always is, which is M7's to record.
    compiler.chunk.strict = script.is_strict;
    // §16.1.7 step 8's `GlobalDeclarationInstantiation`. A script's `var`s belong to the *global
    // object*, not to a scope of its own — which is why `var x = 1` at the top level makes
    // `globalThis.x` and a `let` never will. `VarDeclaredNames` of the whole body rather than of
    // the top level, because a `var` inside a block or a loop belongs to the script all the same;
    // that is the difference between `var` and everything that replaced it.
    //
    // `TopLevelVarDeclaredNames` would answer the same thing today. The two differ on exactly one
    // production, a *function declaration* at the top level, which is var-scoped and which
    // `hoist_functions` handles separately; this is the one that stays right for a Script.
    for name in var_declared_names(&script.body) {
        let index = compiler.name(name.name)?;
        compiler.chunk.emit(Instruction::DeclareGlobal(index));
    }
    // §16.1.7 `GlobalDeclarationInstantiation` step 17 — a script's `let` and `const` go in the
    // global *declarative* record rather than onto the global object, which is why these get slots
    // like any other lexical binding while the `var`s above became properties.
    compiler.declare_lexical_names(&script.body)?;
    compiler.hoist_functions(&script.body)?;
    compiler.statements(&script.body)?;
    Ok(compiler.finish())
}

/// One name with a slot, and what the compiler knows about it.
#[derive(Debug, Clone)]
struct Local {
    /// What the source calls it.
    name: Box<str>,
    /// Whether it can still be resolved from where the compiler is standing.
    ///
    /// A block's bindings stop being visible when the block ends, but their slots are not given
    /// back — [`Compiler::leave_scope`] says why. So going out of scope is a flag rather than a
    /// truncation, and [`Compiler::resolve`] skips what is no longer in scope.
    live: bool,
    /// Whether it was declared by `let` or `const` rather than by `var` or as a parameter.
    ///
    /// What it changes is the *dead zone*: a lexical binding starts uninitialised and reading it
    /// before its declaration is a ReferenceError, where a `var` reads as `undefined`.
    lexical: bool,
    /// Whether assigning to it is a TypeError — §9.1.1.1.5, and the whole of what `const` is.
    ///
    /// Known here rather than at run time because the compiler resolved the binding: an
    /// environment does not have to carry a mutability bit per slot to answer a question that was
    /// already settled when the name was looked up.
    immutable: bool,
}

impl Local {
    /// Whether this is what `name` refers to from where the compiler is standing.
    fn answers_to(&self, name: &str) -> bool {
        self.live && &*self.name == name
    }
}

/// What the compiler knows while it works.
struct Compiler<'a> {
    chunk: Chunk,
    heap: &'a mut Heap,
    /// The variables that have slots, in the order they were given them.
    ///
    /// A `Vec` searched backwards rather than a map: a scope holds a handful of names, and the
    /// backwards search is what makes an inner declaration shadow an outer one when there are
    /// inner scopes to have. It is also what a map would have to be told to do.
    locals: Vec<Local>,
    /// The jumps that leave the innermost loop, waiting for its end.
    ///
    /// One list per enclosing loop. `break` in a loop inside a loop leaves the inner one, which
    /// is what the stack is for.
    breaks: Vec<Vec<Unpatched>>,
    /// The slot this body's `arguments` object would go in, if it has one to fill.
    ///
    /// Given to every non-arrow body, before anything is compiled, so that the name resolves to
    /// it rather than walking out to a global. Whether it is ever *used* is a separate question —
    /// see [`Compiler::uses_arguments`] — because a slot costs nothing and an object costs an
    /// allocation per call.
    arguments_slot: Option<u32>,
    /// Whether anything actually read it.
    uses_arguments: bool,
    /// How many locals existed when each enclosing *scope* began, innermost last.
    ///
    /// What a declaration needs and [`Compiler::resolve`] cannot give it. Resolving a name walks
    /// outwards and finds the nearest binding of that name anywhere in the function, which is the
    /// right answer for a *use* and the wrong one for a *declaration*: `let i = 'kept'; for (let
    /// i = 0; …)` would find the outer `i` and the loop would assign to it, so the loop's variable
    /// would leak and the outer one would be destroyed. A declaration only ever looks inside the
    /// scope it is written in.
    ///
    /// Empty means the function body's own scope, which starts at slot zero — so `last()`
    /// defaulting to zero is the right answer rather than a missing case.
    scope_marks: Vec<usize>,
    /// How many locals existed when each enclosing loop began, innermost last.
    ///
    /// What it is for is §14.7.4.7 `CreatePerIterationEnvironment`. A `let` written inside a loop
    /// is a *fresh binding on every pass*, so a closure made on the third pass and one made on the
    /// fourth must see different variables. praxis gives each lexical declaration one slot for the
    /// whole call, which is right for every binding that is entered once and wrong for one that is
    /// entered again — and the difference is only observable through a closure.
    ///
    /// So this records where each loop started, and making a function is refused while a lexical
    /// binding declared inside the innermost loop is live. Refused rather than compiled, because
    /// the alternative is every closure in the loop sharing one variable and answering the last
    /// value — a wrong answer that looks like a working program.
    loop_marks: Vec<usize>,
    /// Where `continue` goes — the top of the innermost loop's test, or its update.
    continues: Vec<Vec<Unpatched>>,
    /// How deep into an expression the compiler currently is.
    depth: u32,
    /// The scopes this one is written inside, outermost first.
    ///
    /// The script's is at the front and the immediately enclosing function's at the back, so
    /// counting *backwards* from the end is counting environments outwards — which is exactly
    /// what a [`Instruction::LoadVariable`] depth means.
    outer: Vec<Vec<Local>>,
    /// Where `this` lives, when it is a binding rather than the register — DR-0015.
    ///
    /// `Some` inside a derived constructor and inside any arrow written in one, and that propagation
    /// rule is §10.2.11's `[[ThisMode]]`: an arrow reaches outward for `this`, so it reaches for the
    /// binding; a non-arrow function is given one of its own by the call, so it must *not*, or a
    /// method written inside a derived constructor would answer the enclosing instance instead of
    /// its own receiver. See [`Compiler::compile_nested`], which is where the rule is applied.
    ///
    /// The depth is carried rather than looked up. Resolving `%this` by name through the scope chain
    /// would answer the same thing, and would also have a failure case — a `None` that could only
    /// mean the compiler had lost its own binding, with no honest thing to emit for it. Counting
    /// instead is exact by construction: an arrow is one environment, so propagating adds one.
    ///
    /// Not a [`Where`], though the two hold the same numbers. A `Where` also carries `immutable`,
    /// which decides whether a *write* is a TypeError — and nothing ever writes this binding through
    /// `store_name`, so the field would be a value no input could distinguish. Mutation coverage said
    /// exactly that, by surviving a flip of it.
    this_binding: Option<ThisSlot>,
    /// The body that initialises this derived constructor's instance fields, if it has any.
    ///
    /// §15.7.14 runs `InitializeInstanceElements` from `super()` rather than on entry, because until
    /// the parent has made the object there is nothing to put a field on. So the field list has to
    /// reach the `super()`, which may be anywhere in the constructor — and a *list* cannot be kept
    /// here, its lifetime being the syntax tree's rather than this compiler's. A compiled body can:
    /// the fields become a function of no arguments, called with the new object as its receiver,
    /// which is the same shape a static field's initialiser already uses and for the same reason.
    derived_fields: Option<u32>,
    /// Whether this is the script rather than a function body.
    ///
    /// §14.2.2's completion value belongs to the script; what a function's statements evaluate to
    /// is nobody's business but `return`'s. So an expression statement inside a function discards
    /// its value where one at the top level keeps it.
    is_script: bool,
    /// The most slots that were in use at once.
    ///
    /// Not `locals.len()` at the end: a catch parameter's slot is given back when its block ends,
    /// so the table shrinks. The frame still has to be big enough for the moment it was widest,
    /// which is what a run-time slot index is an index into.
    high_water: usize,
    /// Where each function declaration that was hoisted was written.
    ///
    /// [`Compiler::hoist_functions`] runs over the *top level* of a body and nothing else, so a
    /// declaration inside a block was never made — and the statement itself does nothing, because
    /// hoisting is supposed to have done it. Without this the two facts together would make
    /// `{ function g() {} } typeof g` answer `"undefined"` in silence, which is a wrong answer
    /// rather than a missing feature. §14.1 block-scopes such a declaration and Annex B.3.3
    /// hoists it in sloppy code; both need block scoping, so until then it is refused.
    hoisted: Vec<Span>,
    /// The labels in scope, and how many loops were open when each was met.
    ///
    /// A `break name` joins the break list of the loop that number identifies, which is what
    /// makes it leave *that* statement rather than the innermost one. Innermost last, so a
    /// backwards search finds the nearest of two labels with the same name — which the parser
    /// forbids, and which costs nothing to be right about.
    labels: Vec<(Box<str>, usize)>,
    /// For each enclosing `try` that has a `finally`, how many loops were open when it began.
    ///
    /// A `break` may not jump past a `finally`, and this is what tells the two cases apart: a
    /// loop opened inside the `try` has a greater depth than the number recorded here.
    finally_guards: Vec<usize>,
    /// The iterator slot of each `for`-`of` currently being compiled, innermost last.
    ///
    /// §7.4.9 `IteratorClose` has to run on every way out of the loop that is not the iterator
    /// saying it is done — a `break`, a `return`, a labelled break crossing this loop. The
    /// compiler is the only thing that knows where those jumps are, so it emits the closing
    /// before each of them, and this is how it knows which iterators are still open.
    ///
    /// Parallel to `breaks` — **one entry per breakable statement**, and `None` for the ones that
    /// drive no iterator.
    ///
    /// Genuinely parallel, which it was not: it held one entry per open `for`-`of` while `breaks`
    /// held one per breakable, and the two indices coincide only while every enclosing breakable is a
    /// `for`-`of`. A `switch` between a label and its loop was enough to make a labelled break close
    /// the wrong iterators, or none. Now the index is the same index, and the `Option` is what says
    /// there is nothing to close rather than the lengths saying it.
    closes: Vec<Option<u32>>,
}

impl<'a> Compiler<'a> {
    fn new(heap: &'a mut Heap) -> Self {
        Self {
            chunk: Chunk::default(),
            heap,
            locals: Vec::new(),
            breaks: Vec::new(),
            arguments_slot: None,
            uses_arguments: false,
            scope_marks: Vec::new(),
            loop_marks: Vec::new(),
            continues: Vec::new(),
            hoisted: Vec::new(),
            labels: Vec::new(),
            finally_guards: Vec::new(),
            closes: Vec::new(),
            high_water: 0,
            depth: 0,
            outer: Vec::new(),
            this_binding: None,
            derived_fields: None,
            is_script: true,
        }
    }

    /// Emit a read of `this` — §9.1.1.3's `ResolveThisBinding`.
    ///
    /// Two representations and the compiler picks, which is DR-0015's whole cost. Inside a derived
    /// constructor `this` is a binding that `super()` fills, so reading it before then is the
    /// ReferenceError §10.2.2 asks for; everywhere else it is the register the call set, and there is
    /// no state in which it could be missing.
    pub(super) fn load_this(&mut self) {
        match self.this_binding {
            Some(at) => self.chunk.emit(Instruction::LoadThisBinding {
                depth: at.depth,
                index: at.index,
            }),
            None => self.chunk.emit(Instruction::LoadThis),
        }
    }

    fn finish(self) -> Chunk {
        let mut chunk = self.chunk;
        chunk.locals = self.high_water;
        chunk
    }

    /// Give `name` a slot if it does not have one, and answer which.
    ///
    /// Redeclaring a `var` is legal and is not a second variable: `var x; var x = 1;` declares one
    /// binding. That is why this answers the existing slot rather than making another.
    fn declare(&mut self, name: &str) -> u32 {
        if let Some(slot) = self.resolve(name) {
            return slot;
        }
        self.declare_shadowing(name)
    }

    /// Which slot `name` is in, if any.
    ///
    /// Searched from the end, because the last binding with a given name is the innermost one.
    ///
    /// `var` alone never puts a name in twice — [`Compiler::declare`] hands back the existing
    /// slot — so this mattered to nothing until the catch parameter arrived. That one *does*
    /// shadow: `var e = 1; try { throw 2 } catch (e) { e }` is 2 inside the block and 1 after it,
    /// which is what [`Compiler::declare_shadowing`] and the truncation afterwards produce.
    fn resolve(&self, name: &str) -> Option<u32> {
        let at = self
            .locals
            .iter()
            .rposition(|local| local.answers_to(name))?;
        u32::try_from(at).ok()
    }

    /// Which slot `name` has *in the scope being compiled*, if it has one there.
    ///
    /// What a declaration asks, where a use asks [`Compiler::resolve`]. See
    /// [`Compiler::scope_marks`] for why the two are different questions.
    fn resolve_in_scope(&self, name: &str) -> Option<u32> {
        let mark = self.scope_marks.last().copied().unwrap_or(0);
        let at = self
            .locals
            .iter()
            .rposition(|local| local.answers_to(name))
            .filter(|at| *at >= mark)?;
        u32::try_from(at).ok()
    }

    /// Open a scope, answering the mark that closes it.
    fn enter_scope(&mut self) -> usize {
        let mark = self.locals.len();
        self.scope_marks.push(mark);
        mark
    }

    /// The local `name` refers to, if it is one.
    fn local(&self, name: &str) -> Option<&Local> {
        self.locals
            .iter()
            .rev()
            .find(|local| local.answers_to(name))
    }

    /// Give `name` a slot that a block will take out of scope again — §14.3.1's `let` and `const`.
    ///
    /// Always a *new* slot, never a reused one, and that is the whole of what makes sibling blocks
    /// safe: `{ let x = 1; f = () => x } { let y = 2 }` would have `f` answering 2 if `y` were
    /// given the slot `x` had finished with. Slots are cheap and closures are not repairable
    /// afterwards.
    fn declare_lexical(&mut self, name: &str, immutable: bool) -> u32 {
        let slot = self.declare_shadowing(name);
        if let Some(local) = self.locals.last_mut() {
            local.lexical = true;
            local.immutable = immutable;
        }
        slot
    }

    /// Whether a function written here would close over a binding that a loop re-creates.
    ///
    /// True when the innermost enclosing loop has a live lexical binding declared inside it. See
    /// [`Compiler::loop_marks`] for why that is refused rather than compiled.
    fn would_capture_a_per_iteration_binding(&self) -> bool {
        let Some(&mark) = self.loop_marks.last() else {
            return false;
        };
        self.locals
            .iter()
            .skip(mark)
            .any(|local| local.live && local.lexical)
    }

    /// Take every local declared since `mark` out of scope, without giving its slot back.
    ///
    /// The slot stays taken for the rest of the function. See [`Compiler::declare_lexical`] — a
    /// closure made inside the block still reads that slot after the block has ended, so handing
    /// it to the next block would make the two share a variable.
    fn leave_scope(&mut self, mark: usize) {
        self.scope_marks.pop();
        for local in self.locals.iter_mut().skip(mark) {
            local.live = false;
        }
    }

    /// Where `name` lives, from where the compiler is standing.
    ///
    /// A depth and an index, because the compiler built the chain of scopes and so knows the
    /// answer: how many environments out, and which slot there. Nothing at run time compares a
    /// string to find a variable — §9.1's records, resolved once.
    fn binding(&self, name: &str) -> Option<Where> {
        if let Some(index) = self.resolve(name) {
            let immutable = self.local(name).is_some_and(|local| local.immutable);
            return Some(Where {
                depth: 0,
                index,
                immutable,
            });
        }
        // Outwards, one scope at a time. The innermost enclosing scope is the *last* of `outer`,
        // so walking it in reverse is walking the environment chain — and the count is the depth
        // the instruction carries.
        for (back, scope) in self.outer.iter().rev().enumerate() {
            let Some(at) = scope.iter().rposition(|local| local.answers_to(name)) else {
                continue;
            };
            // `back` counts enclosing scopes and `at` indexes one scope's locals, so both are
            // bounded by how much source there is — and `Span` being `u32` already puts a source
            // longer than `u32::MAX` bytes outside what this engine agreed to read. Neither
            // conversion can fail on a source we accepted in the first place.
            let depth = u32::try_from(back + 1).ok()?; // bounded by the u32 source-length contract
            let index = u32::try_from(at).ok()?; // same
            let immutable = scope.get(at).is_some_and(|local| local.immutable);
            return Some(Where {
                depth,
                index,
                immutable,
            });
        }
        None
    }

    /// Emit the instruction that reads `binding`.
    fn load(&mut self, binding: Where) {
        self.chunk
            .emit(Instruction::LoadVariable(binding.depth, binding.index));
    }

    /// Emit the instruction that writes `binding`, leaving the value on the stack.
    fn store(&mut self, binding: Where) {
        self.chunk
            .emit(Instruction::StoreVariable(binding.depth, binding.index));
    }

    /// Whether this compiler is the script's own, rather than some function's body.
    ///
    /// Derived rather than stored, because it is already recorded: a function body is compiled
    /// with the chain of scopes it is written inside, and only the script itself is written
    /// inside none. A separate flag could disagree with that; this cannot.
    pub(super) fn at_global_scope(&self) -> bool {
        self.outer.is_empty()
    }

    /// The constant index holding `name` as a String, for the instructions that name a global.
    ///
    /// The name is interned, so the hundred references to `assert` in one test share one String —
    /// and so the property key made from it at run time is the key the global object already has.
    pub(super) fn name(&mut self, name: &str) -> Result<u32, CompileError> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let interned = self.heap.intern(&units);
        self.chunk.add_constant(Value::String(interned))
    }

    /// Note that `arguments` was reached for, so that the call will build one.
    ///
    /// Asked on every name because the alternative is asking the parser for a list of the names a
    /// body mentions, which is a second walk over the tree that could disagree with this one.
    /// Comparing a string here costs a length check for every name that is not eight characters
    /// long.
    fn note_arguments(&mut self, name: &str, binding: Option<Where>) {
        if name != "arguments" {
            return;
        }
        match binding {
            // The body's own — the slot given below, or a parameter or `var` that took the name
            // first, in which case §10.2.11 step 19 makes no object and this is that.
            Some(found) if found.depth == 0 => {
                self.uses_arguments = Some(found.index) == self.arguments_slot;
            }
            // An enclosing function's, which only an arrow can reach. The chunk carries it out to
            // whoever is building this body.
            Some(_) => self.chunk.outer_arguments = true,
            // Nowhere at all: an `arguments` at the top level of a script is an ordinary global.
            None => {}
        }
    }

    /// Emit a read of `name`, from a scope if it has one and from the global object if not.
    ///
    /// §9.4.2 `ResolveBinding` walks the environment chain outwards and ends at the Global
    /// Environment Record, so a name the compiler cannot place is not an unknown name — it is a
    /// name whose binding, if any, is a property of the global object. Whether it is there is a
    /// question for run time, because a script can create one at any moment.
    pub(super) fn load_name(&mut self, name: &str) -> Result<(), CompileError> {
        let binding = self.binding(name);
        self.note_arguments(name, binding);
        match binding {
            Some(binding) => {
                self.load(binding);
                Ok(())
            }
            None => {
                let index = self.name(name)?;
                self.chunk.emit(Instruction::LoadGlobal(index));
                Ok(())
            }
        }
    }

    /// Emit a read of the Private Name for `#name`, or refuse — §9.2's `ResolvePrivateIdentifier`.
    ///
    /// Not [`Compiler::load_name`], and the difference is the whole reason this exists: that one
    /// falls back to a *property of the global object* for a name it cannot place, which is right for
    /// an identifier and silently wrong here. A `#x` with no enclosing class would read a global that
    /// does not exist, hand `undefined` to `GetPrivate` as a name, and reach the interpreter as a
    /// fault — the signal reserved for a chunk that disagrees with the compiler.
    ///
    /// The parser refuses `#x` outside a class (§15.7.1), so no source arrives here. A hand-built
    /// tree can, and this is the honest answer for it: a refusal with a span.
    pub(super) fn load_private_name(&mut self, name: &str) -> Result<(), CompileError> {
        let slot = crate::compile::class::private_name_slot(name);
        let Some(binding) = self.binding(&slot) else {
            return Err(unsupported(
                "a private name outside a class body",
                Span::new(0, 0),
            ));
        };
        self.load(binding);
        Ok(())
    }

    /// Emit a read or a write of one of the compiler's own class-scope slots, or refuse.
    ///
    /// The same argument [`Compiler::load_private_name`] makes, for the slots that hold a private
    /// method's *function* rather than its name. `load_name` and `store_name` fall back to a property
    /// of the global object for a name they cannot place, which for a `%`-prefixed slot is never what
    /// was meant — and it fails *quietly*, because a store and a load of the same missing slot both go
    /// to the same global and agree with each other. Mutation coverage found exactly that: dropping the
    /// static half of the private-method list left every static method working, through a global.
    fn private_slot(&mut self, slot: &str, write: bool) -> Result<(), CompileError> {
        let Some(binding) = self.binding(slot) else {
            return Err(unsupported(
                "a private method outside a class body",
                Span::new(0, 0),
            ));
        };
        match write {
            true => self.store(binding),
            false => self.load(binding),
        }
        Ok(())
    }

    /// Read a class-scope slot the compiler reserved, or refuse — see [`Compiler::private_slot`].
    pub(super) fn load_private_slot(&mut self, slot: &str) -> Result<(), CompileError> {
        self.private_slot(slot, false)
    }

    /// Write one, leaving the value on the stack — see [`Compiler::private_slot`].
    pub(super) fn store_private_slot(&mut self, slot: &str) -> Result<(), CompileError> {
        self.private_slot(slot, true)
    }

    /// Emit a write of `name`, leaving the value on the stack.
    pub(super) fn store_name(&mut self, name: &str) -> Result<(), CompileError> {
        let binding = self.binding(name);
        self.note_arguments(name, binding);
        match binding {
            // §9.1.1.1.5 step 3 — a `const` refuses every assignment, and the compiler already
            // knows which binding this is. What is left for run time is the throw, which happens
            // *after* the right-hand side has run because §13.15.2 evaluates it first.
            Some(binding) if binding.immutable => {
                self.chunk.emit(Instruction::ThrowImmutableAssignment);
                Ok(())
            }
            Some(binding) => {
                self.store(binding);
                Ok(())
            }
            None => {
                let index = self.name(name)?;
                self.chunk.emit(Instruction::StoreGlobal(index));
                Ok(())
            }
        }
    }
}

/// Compile `expression` onto the end of `chunk`.
impl Compiler<'_> {}

impl Compiler<'_> {}

impl Compiler<'_> {
    /// Give `name` a slot of its own even if the name is already taken.
    ///
    /// A catch parameter shadows anything outside it for the length of its block, which is why
    /// this pushes rather than reusing — and why [`Compiler::resolve`] searches from the end.
    pub(super) fn declare_shadowing(&mut self, name: &str) -> u32 {
        let slot = u32::try_from(self.locals.len()).unwrap_or(u32::MAX);
        self.locals.push(Local {
            name: name.into(),
            live: true,
            lexical: false,
            immutable: false,
        });
        self.high_water = self.high_water.max(self.locals.len());
        slot
    }

    /// A slot for the compiler's own use, under a name no source text can spell.
    ///
    /// `%` is not in `IdentifierStart` or `IdentifierPart`, so a script cannot name this and
    /// cannot reach it. The alternative is a second table of nameless slots, which is the same
    /// thing with more machinery.
    fn declare_hidden(&mut self, what: &str) -> u32 {
        let name = format!("%{what}{}", self.locals.len());
        self.declare_shadowing(&name)
    }

    /// The index the next instruction will have, as a jump target.
    fn here(&self) -> Result<u32, CompileError> {
        u32::try_from(self.chunk.code.len()).map_err(|_| CompileError {
            kind: ErrorKind::TooLong,
            span: Span::new(0, 0),
        })
    }
}

/// How deeply an expression may nest before the compiler refuses it.
///
/// Not a limit the language has — an arbitrarily deep expression is legal, and every engine
/// refuses one somewhere. The number matters because two kinds of program come near it from
/// opposite directions: hand-written code nests a handful deep, and minified code chains
/// thousands of terms with `+` and `,`, each of which is one more level of tree.
///
/// Measured rather than guessed, and the measurement is smaller than it looks: on the 1 MiB main
/// thread Windows gives a program, a debug build — whose frames are largest — runs out of stack
/// somewhere between 200 and 400 levels. A test thread has eight times that, which is exactly the
/// trap: a limit checked only under `cargo test` would be four times too large in a real embedder.
///
/// 64 is well inside the smallest of those, and it is the parser's number for a reason: the
/// parser refuses nesting past `MAX_NESTING_DEPTH` before the compiler ever sees it, so a cap
/// above that one is unreachable from source and measured only by a test that builds the tree by
/// hand. The one shape the parser *can* build arbitrarily deep — a left-leaning operator chain —
/// is compiled with a loop rather than by recursing.
///
/// It was 128, and that was a measurement that rotted. A level cost about 5.5 KiB when the number
/// was set; slices since have added frames between one level and the next, and 128 of them came
/// to roughly 700 KiB — inside a mebibyte, but not inside the smaller stacks a debug build gets on
/// other platforms, where CI found it by aborting. `compiling_at_the_cap_fits_in_the_stack_it_
/// claims_to_need` is the guard that was missing: the parser has had one since DR-0006 and the
/// compiler, which recurses just as deeply, did not.
const MAX_EXPRESSION_DEPTH: u32 = 64;

/// The name of the slot a derived constructor's `this` occupies — DR-0015.
///
/// A `%` in front, which is the house convention for a slot no source text can spell: `%` is in
/// neither `IdentifierStart` nor `IdentifierPart`, so nothing a program can write resolves here. It
/// has a name at all only so that the locals table reads honestly when something goes wrong; the
/// slot is reached by the number the compiler kept, never by looking the name up.
const THIS_BINDING: &str = "%this";

/// Where a name lives — §9.1's environment records, resolved at compile time.
///
/// Named for the question rather than for the thing, because the syntax tree already has a
/// `Binding` and it means something else entirely: the *form* a declaration takes.
///
/// Two numbers rather than a name, because the compiler built the chain of scopes and so knows
/// the answer. Nothing at run time compares a string to find a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Where {
    /// How many environments out — `0` is the running function's own.
    depth: u32,
    /// Which slot, in that environment.
    index: u32,
    /// Whether writing to it is a TypeError — §9.1.1.1.5, and the whole of what `const` is.
    ///
    /// Carried here rather than looked up again by whoever writes, because a second lookup is a
    /// second copy of the resolution rule: the two could disagree about *which* `x` a name means,
    /// and only one of them would be right. Resolving once answers both questions at once.
    immutable: bool,
}

/// Where a derived constructor's `this` lives, from where the compiler is standing — DR-0015.
///
/// Two numbers and no third, which is the point: see [`Compiler::this_binding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThisSlot {
    /// How many environments out — `0` in the constructor, one more per enclosing arrow.
    depth: u32,
    /// Which slot, in that environment.
    index: u32,
}

/// A refusal with a location.
fn unsupported(what: &'static str, span: Span) -> CompileError {
    CompileError {
        kind: ErrorKind::Unsupported(what),
        span,
    }
}

#[cfg(test)]
mod tests;
