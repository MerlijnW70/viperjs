//! §19.2.1.1's **direct** mode — eval'd source resolved against the scopes its caller is running in.
//!
//! # Why this is not the built-in
//!
//! [`crate::builtins::eval`] is the function `eval` refers to, and it is reached only *indirectly*
//! — `(0, eval)(…)`, `globalThis.eval(…)`, a callback it was passed as. §13.3.6.1 makes a call a
//! direct eval when its callee is written as the bare name `eval` and turns out to be that very
//! function, and §19.2.1.1 then gives it a different scope: a fresh declarative environment over
//! the **caller's**, and the caller's variable environment.
//!
//! That difference cannot be expressed as an argument to a built-in. A native call has no handle on
//! the environment the caller is running in — the interpreter has moved on to the callee's — so the
//! decision is made where the call site is, which is here.
//!
//! # What makes it possible
//!
//! DR-0018. praxis resolves a name to a depth and an index when it compiles, and the source of an
//! eval only exists at run time — so the compiler that handles it never saw the scopes it has to
//! resolve into. Environments therefore carry what the source called their slots, and this walks
//! the running chain, hands the compiler the names level by level, and lets it resolve exactly as
//! it would into scopes it had built itself.

use super::Vm;
use crate::compile::{EvalVars, compile_direct_eval};
use crate::heap::{Binding, EnvironmentId, Heap};
use crate::value::{Completion, Value};
use std::collections::HashSet;
use std::rc::Rc;

impl Vm {
    /// §19.2.1.1 `PerformEval(x, strictCaller, direct = true)`.
    ///
    /// `source` is the first argument the call site had, or `None` for `eval()` written with none.
    /// The arguments past the first are evaluated and thrown away, which the call site has already
    /// done by the time this is reached — §19.2.1.1 takes one argument and `eval(a, b)` still runs
    /// `b`.
    pub(super) fn perform_direct_eval(
        &mut self,
        source: Option<Value>,
        strict_caller: bool,
        heap: &mut Heap,
    ) -> Completion<Value> {
        // §19.2.1.1 step 2 — anything that is not a String is answered unchanged and *not*
        // converted, which is what stops `eval(o)` running whatever `o.toString` felt like.
        let Some(Value::String(id)) = source else {
            return Ok(source.unwrap_or(Value::Undefined));
        };
        let text = String::from_utf16_lossy(heap.string(id).unwrap_or(&[]));
        // §19.2.1.1 step 5's strictness, and it is the *parser* that has to be told: it decides
        // §11.2.1's early errors and settles `is_strict` for every function written inside the
        // text before the tree comes back. Set on the finished tree instead, a strict caller's
        // `eval("(function () { return this; })()")` still substituted the global object.
        let private = self.private_names_in_scope(heap);
        let script = match crate::parser::parse_eval(
            &text,
            strict_caller,
            self.eval_context(heap),
            &private,
        ) {
            Ok(script) => script,
            Err(error) => {
                return Err(crate::builtins::eval::syntax_error(
                    self,
                    heap,
                    &error.kind.to_string(),
                ));
            }
        };
        // Either side made it strict, and the parser has already folded the two together — so this
        // is one answer read back rather than a second `||` that could disagree with the first.
        let chain = self.running_chain(heap);
        let vars = self.eval_vars(script.is_strict);
        let chunk = match compile_direct_eval(&script, heap, chain, vars) {
            Ok(chunk) => chunk,
            Err(error) => {
                return Err(crate::builtins::eval::syntax_error(
                    self,
                    heap,
                    &error.message(),
                ));
            }
        };
        // §19.2.1.1 step 12's `NewDeclarativeEnvironment(lexEnv)` — a child of the environment the
        // caller is *running* in, which is the whole difference from the indirect mode's child of
        // the global one. Named, so that an eval written inside this one resolves into it too.
        let environment = heap.new_named_environment(
            Some(self.environment),
            chunk.locals(),
            Rc::clone(chunk.bindings()),
        );
        self.run_script(&chunk, environment, heap)
    }

    /// §19.2.1.1 steps 3.b.ii to 3.b.iv — what the evaluated text is allowed to *say*.
    ///
    /// Three questions about the execution the call was made from, and every one of them is a
    /// question only the interpreter can answer: `eval("super.m()")` is legal inside a method and a
    /// Syntax Error at the top of a script, from identical text.
    ///
    /// `super.a` is granted by the running function having a **home object**, which is
    /// `HasSuperBinding` said in praxis's terms — and it is right for an arrow too, because
    /// `Instruction::InheritHome` copies the enclosing method's onto one when it is made.
    ///
    /// `new.target` is granted by there being a **non-arrow** function running. §19.2.1.1 step 3.a
    /// asks `GetThisEnvironment()`, which walks *past* arrows — an arrow has no `this` and no
    /// `new.target` of its own — so an arrow at the top level of a script reaches the script's
    /// environment and grants nothing. Counting frames alone said otherwise, and
    /// `language/eval-code/direct/new.target-arrow.js` is that program.
    ///
    /// An arrow written *inside* a function is refused too, and that is narrower than the clause.
    /// Whether it was is a **lexical** fact — the parser knows it, and refuses `new.target` in a
    /// top-level arrow at compile time — and a running arrow's chunk does not record it, so there
    /// is nothing here to ask. Carrying the parser's answer through to the chunk is the fix and is
    /// its own slice; until then this refuses rather than answering, which is the direction that
    /// costs a Syntax Error instead of a wrong `new.target`.
    ///
    /// `super(…)` is granted by the running *chunk* being a derived constructor, and is narrower
    /// for the same kind of reason: an arrow written inside one may contain a `SuperCall` and its
    /// chunk is not the constructor's. Narrower rather than wider on purpose throughout — the
    /// other way round accepts a `super()` with no constructor to reach.
    fn eval_context(&self, heap: &Heap) -> crate::parser::EvalContext {
        let frame = self
            .frames
            .last()
            .filter(|_| self.frames.len() > self.floor.frames);
        let arrow = frame
            .and_then(|frame| frame.function)
            .and_then(|function| heap.object(function))
            .and_then(crate::heap::Object::lexical)
            .is_some();
        crate::parser::EvalContext {
            in_function: frame.is_some() && !arrow,
            in_method: frame
                .and_then(|frame| frame.function)
                .and_then(|function| heap.object(function))
                .and_then(crate::heap::Object::home_object)
                .is_some(),
            in_derived_constructor: frame
                .and_then(|frame| frame.function)
                .and_then(|function| heap.object(function))
                .and_then(crate::heap::Object::call)
                // A `matches!` with a guard rather than a `match` with a catch-all: a frame is
                // pushed only for a compiled body — a native returns before one exists, and a bound
                // function's frame belongs to its target — so an arm for the other kinds is one no
                // input can reach, and an unreachable arm is a decision no test can hold. What is
                // left is the question that does decide something: whether that body is a derived
                // constructor.
                .is_some_and(|callable| {
                    matches!(
                        callable,
                        crate::heap::Callable::Bytecode(body) if body.derived_this().is_some()
                    )
                }),
        }
    }

    /// §19.2.1.1 step 12's `varEnv`, as far as praxis can follow it — see [`EvalVars`].
    ///
    /// The question is only ever "is there a function between here and the script", because a
    /// script's variable scope is the global object and a function's is an environment whose size
    /// was fixed when it was compiled. Asked of the **frames above the floor** rather than of all
    /// of them: a nested execution — a coercion, an indirect eval, a job — records where the
    /// caller's frames ended, and the code inside it is at the top level of its own script however
    /// deep the Rust stack is. Counted the other way, `(0, eval)("eval('var x = 1')")` would decide
    /// it was inside whatever function happened to be running underneath.
    fn eval_vars(&self, strict: bool) -> EvalVars {
        match (strict, self.frames.len() > self.floor.frames) {
            // §19.2.1.1 step 14 — strict code's declarations are its own and go away with it.
            (true, _) => EvalVars::Own,
            (false, true) => EvalVars::Caller,
            (false, false) => EvalVars::Global,
        }
    }

    /// The environments the caller is inside, **outermost first**, each with what it called its
    /// slots.
    ///
    /// One entry per environment and never a gap, because a `LoadVariable`'s depth counts
    /// environments: a level left out would make every name outside it resolve one hop too shallow,
    /// which is a wrong value rather than a missing one. A level the engine built for itself has no
    /// names and contributes an empty entry, which resolves nothing and is walked past.
    ///
    /// The walk terminates because a parent is always an environment that already existed when its
    /// child was made — `Heap::new_environment` takes the parent as an argument — so a chain
    /// strictly decreases and cannot close on itself.
    /// §15.7.7's private names in scope where the call was made — the one rule the parser cannot
    /// answer for evaluated text.
    ///
    /// §15.7.1 makes a `#a` with no enclosing class a Syntax Error, and the parser enforces it by
    /// keeping every reference it reads and refusing whatever no class body claimed. Evaluated
    /// text has no class body of its own and §19.2.1.1 nonetheless runs it *inside* the caller's,
    /// so `class C { #m = 44; get() { return eval("this.#m") } }` is legal and the parse cannot
    /// see why.
    ///
    /// Nothing new is stored to answer it. A private name is a slot like any other — see
    /// `compile::class::private_name_slot` — and DR-0018 already made every running scope name its
    /// slots, so the classes this call is inside are written down in the environment chain the
    /// compiler is about to be handed. This reads the same chain for a different question.
    ///
    /// The `#` is punctuation rather than part of the name here, matching what the parser records.
    fn private_names_in_scope(&self, heap: &Heap) -> HashSet<Box<str>> {
        let mut found = HashSet::new();
        let mut at: Option<EnvironmentId> = Some(self.environment);
        while let Some(environment) = at {
            for binding in heap.environment_names(environment).unwrap_or(&[]) {
                if let Some(name) = binding.name.strip_prefix("%private #") {
                    found.insert(Box::from(name));
                }
            }
            at = heap.environment_at(environment, 1);
        }
        found
    }

    fn running_chain(&self, heap: &Heap) -> Vec<Vec<Binding>> {
        let mut chain = Vec::new();
        let mut at: Option<EnvironmentId> = Some(self.environment);
        while let Some(environment) = at {
            chain.push(heap.environment_names(environment).unwrap_or(&[]).to_vec());
            at = heap.environment_at(environment, 1);
        }
        chain.reverse();
        chain
    }
}
