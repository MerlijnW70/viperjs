//! §19.2.1 — `eval`, and the half of it that needs no scope of the caller's.
//!
//! # The two evals are two operations
//!
//! §13.3.6.1 makes a call a **direct** eval when its callee is a reference spelled `eval` that
//! turns out to be this very function. Anything else that reaches the same function — `(0, eval)`,
//! `globalThis.eval`, a variable it was assigned to, a callback it was passed as — is an
//! **indirect** eval, and §19.2.1.1 gives the two different scopes:
//!
//! - indirect: a fresh declarative scope over the *global* one, whatever the caller was doing.
//! - direct: a fresh declarative scope over the **caller's**, and the caller's variable scope.
//!
//! Only the first is here. The second is in [`crate::vm`] and not in this file at all, because a
//! native call has no handle on the environment its caller was running in — the interpreter has
//! already moved on to the callee's. [`crate::compile::Instruction::CallDirectEval`] is what keeps
//! the decision at the call site, where that handle still exists.
//!
//! # Why indirect eval needs nothing else
//!
//! Because a script's top-level `var` is *already* a property of the global object and its
//! top-level `let` is already a slot in the script's own environment — §16.1.7's split, which
//! ViperJS implements for ordinary scripts. §19.2.1.1's indirect mode asks for exactly that: `var`
//! into the global scope where it outlives the eval, `let` into a scope that is discarded with it.
//! So compiling the text as a Script and running it with a fresh environment *is* the semantics,
//! and there is no special case anywhere in the compiler.

use crate::heap::{Heap, NativeCall};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §19.2.1 `eval(x)`, reached indirectly.
///
/// A direct call never arrives here — the call site answers it itself — so this is the indirect
/// operation and does not ask which it is.
pub(super) fn eval(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §19.2.1.1 step 2 — anything that is not a String is answered unchanged, and *not* converted.
    // `eval(1)` is 1 and `eval(new String("1+1"))` is that object rather than 2, which surprises
    // people and is what stops `eval` from running whatever an object's `toString` felt like.
    let Value::String(source) = call.argument(0) else {
        return Ok(call.argument(0));
    };
    let text = String::from_utf16_lossy(heap.string(source).unwrap_or(&[]));
    perform(vm, heap, &text)
}

/// Parse, compile and run one piece of source as a Script in the global scope.
///
/// Split from the entry point above because the two errors it can answer with are the interesting
/// part, and both are **SyntaxError**: §19.2.1.1 step 8 says a text that is not a Script throws
/// one, and ViperJS decides some of §22.2.1's early errors in the compiler rather than the parser —
/// so a compile refusal has to arrive as the same error a parse refusal does, or a program could
/// tell where ViperJS happens to have put the check.
fn perform(vm: &mut Vm, heap: &mut Heap, text: &str) -> Completion<Value> {
    let script = match crate::parser::parse_script(text) {
        Ok(script) => script,
        Err(error) => return Err(syntax_error(vm, heap, &error.kind.to_string())),
    };
    // §19.2.1.1 rather than §16.1.7 — the scope chain is the same and the bindings' `D` is not:
    // an `eval`'s globals are deletable, direct and indirect alike.
    let chunk = match crate::compile::compile_eval(&script, heap) {
        Ok(chunk) => chunk,
        Err(error) => return Err(syntax_error(vm, heap, &error.message())),
    };
    // §19.2.1.1 step 12 — a *new* declarative environment, and its parent is the global scope and
    // not the caller's. `None` is how ViperJS spells the global scope for a script, which is what
    // makes a name the eval'd code does not declare resolve to a property of the global object.
    let environment =
        heap.new_named_environment(None, chunk.locals(), std::rc::Rc::clone(chunk.bindings()));
    vm.run_script(&chunk, environment, heap)
}

/// A SyntaxError carrying what the parser or the compiler said.
///
/// Built as an *object* rather than raised by kind and message, because [`Abrupt::Raised`] carries
/// a `&'static str` and these messages are made from the source being evaluated. That is the
/// distinction that type draws, and this is a case on the other side of it.
///
/// Shared with the direct mode — [`crate::vm`] — so that the two report a text that will not parse
/// the same way. §19.2.1.1 step 8 makes no distinction between them and neither should ViperJS.
pub(crate) fn syntax_error(vm: &mut Vm, heap: &mut Heap, message: &str) -> Abrupt {
    Abrupt::Thrown(
        vm.realm()
            .error(heap, crate::realm::NativeError::Syntax, message),
    )
}
