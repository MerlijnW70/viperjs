//! Embedding ViperJS: run a script, bind a host function, read the answer back.
//!
//! ```text
//! cargo run --example embed
//! ```
//!
//! This is the whole of what DR-0021's surface is for, in one screen. Everything a host does is
//! here: give the script a function of its own, run source, read a value out, call back in, and
//! keep a value alive across a collection.

use viperjs::api::{Engine, Error, Host};
use viperjs::heap::{Heap, NativeCall};
use viperjs::value::{Completion, Value};
use viperjs::vm::Vm;

fn main() {
    let mut engine = Engine::new();

    // The host's I/O. GOAL.md §3: we provide the language, the host provides everything else — so
    // there is no `console` until a host binds one.
    engine.bind("print", 1, print);

    // Run some source. Jobs queued by the script have already run when this returns.
    let answer = engine
        .eval(
            "print('hello from the script');
             var totals = [1, 2, 3, 4].map(function (n) { return n * n });
             ({ totals: totals, sum: totals.reduce(function (a, b) { return a + b }, 0) })",
        )
        .expect("it runs");

    // Read values out. `get` is the script's own read: the prototype chain, and a getter runs.
    let sum = engine.get(answer, "sum").expect("sum is there");
    println!("sum = {}", engine.text(sum).unwrap_or_default());

    // Call back in, choosing the receiver.
    let totals = engine.get(answer, "totals").expect("totals is there");
    let join = engine.get(totals, "join").expect("arrays have join");
    let separator = engine.eval("' + '").expect("it runs");
    let joined = engine
        .call(join, totals, &[separator])
        .expect("join is callable");
    println!("totals = {}", engine.text(joined).unwrap_or_default());

    // A script's error is a value, not a crash. Which of the four it is decides what a host does.
    match engine.eval("undefined.x") {
        Err(Error::Thrown(said)) => println!("the script threw: {said}"),
        other => println!("unexpected: {other:?}"),
    }

    // A `Value` the host holds is not a root — DR-0021. Put it where the program can reach it and
    // it survives; hold it in a Rust local across a collection and the next read is refused rather
    // than answering `undefined`.
    engine.set_global("kept", answer).expect("it is live");
    engine.collect();
    let sum = engine
        .get(answer, "sum")
        .expect("it was rooted on the global");
    println!(
        "after a collection, sum = {}",
        engine.text(sum).unwrap_or_default()
    );
}

/// `print(x)` — one line to standard output, the value spelled as `String(x)` spells it.
///
/// A host function is a plain `fn`, so it carries no state of its own; anything it needs comes
/// through the arguments or off the global object.
fn print(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    println!("{}", host.text(call.argument(0))?);
    Ok(Value::Undefined)
}
