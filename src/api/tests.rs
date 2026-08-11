//! What an embedder can do through [`super`], said as sentences about behaviour.
//!
//! Beside the module rather than inside it because it is longer than the surface it tests, and
//! a reader who wants to know what `api.rs` offers should not have to scroll past its tests to
//! find out. DR-0021 is what these are about: the two halves of the boundary, and what the heap
//! refuses to hand back.

use super::*;
use crate::heap::NativeCall;
use crate::value::Completion;

#[test]
fn the_heap_budget_is_the_hosts_to_choose_and_defaults_to_something_useful() {
    // DR-0013's number is a policy rather than a fact about the machine, and which policy is
    // right belongs to the embedder. A default of zero would refuse every allocation, which is
    // the mistake `heap::Budget` exists to make unwritable — this row is what would notice.
    let mut engine = Engine::new();
    let answer = engine
        .eval("var a = []; for (var i = 0; i < 5000; i++) a.push({x: i}); a.length")
        .expect("the default budget allows an ordinary program");
    assert_eq!(engine.text(answer).as_deref(), Ok("5000"));

    // Lowered below what a program needs, the program is refused rather than the process dying
    // — which is the whole of what DR-0013 is for.
    let mut small = Engine::new();
    small.set_heap_budget(1 << 16);
    let refused = small.eval("var a = []; for (var i = 0; i < 200000; i++) a.push({x: i}); 1");
    assert!(
        matches!(&refused, Err(Error::Thrown(said)) if said.contains("heap has grown past")),
        "{refused:?}"
    );

    // …and raising it is what lets a program that legitimately wants the memory have it. The
    // pair matters more than either: a budget that only ever refuses is indistinguishable from
    // a broken engine, and one that never refuses is not a budget.
    let mut large = Engine::new();
    large.set_heap_budget(1 << 28);
    let answer = large
        .eval("var a = []; for (var i = 0; i < 200000; i++) a.push({x: i}); a.length")
        .expect("a raised budget allows it");
    assert_eq!(large.text(answer).as_deref(), Ok("200000"));
}

#[test]
fn a_namespace_of_host_functions_is_an_ordinary_object_with_named_methods() {
    fn answer(_: &mut Vm, _: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        Ok(call.argument(0))
    }
    let mut engine = Engine::new();
    engine.bind_namespace("host", &[("echo", 1, answer), ("also", 2, answer)]);
    // An ordinary object with ordinary properties: the program owns it and may take it apart.
    let answer = engine
        .eval("typeof host + ',' + Object.keys(host).join('|')")
        .expect("runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("object,echo|also"));
    let answer = engine.eval("host.echo(7)").expect("runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("7"));
    // §10.3.3's metadata, which is the whole reason this is not a loop over `bind`: a host
    // outside the crate can make the function but cannot name it, and an unnamed method is
    // what every hand-built namespace used to have.
    let answer = engine
        .eval("host.echo.name + ',' + host.echo.length + ',' + host.also.name")
        .expect("runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("host.echo,1,host.also"));
    // It inherits from `Object.prototype`, so the ordinary object protocol works on it.
    let answer = engine
        .eval("host.hasOwnProperty('echo') + ',' + ('toString' in host)")
        .expect("runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("true,true"));
}

#[test]
fn a_script_answers_its_completion_value_and_the_host_reads_it_as_text() {
    let mut engine = Engine::new();
    let answer = engine.eval("1 + 1").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    // §14.2.2's completion value, which is the last statement's: a declaration produces nothing
    // and leaves `undefined` behind rather than the value it bound.
    let answer = engine.eval("1; 2").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    let answer = engine.eval("var x = 1").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("undefined"));
}

#[test]
fn a_syntax_error_says_where_and_not_only_what() {
    // The span is the point of the variant, so it is asserted as a *position* rather than as
    // "some span": a message carrying the wrong offset is worse than one carrying none, because
    // a host will print a caret under the wrong token and be believed.
    let mut engine = Engine::new();
    let source = "var a = 1;\nvar b = ;\nvar c = 3;";
    let Err(Error::Syntax { message, span }) = engine.eval(source) else {
        panic!("that does not parse");
    };
    let at = crate::span::line_col(source, span.start);
    assert_eq!((at.line, at.column), (2, 9), "{message}");
    assert_eq!(&source[span.start as usize..span.end as usize], ";");

    // §16's early errors are the compiler's rather than the parser's — §22.2.1's patterns are
    // read after the literal's shape — so this is the other construction site, and it would be
    // the one to lose its span if only the parser's were carried.
    let source = "var re = /(?<a>x)(?<a>y)/;";
    let Err(Error::Syntax { span, .. }) = engine.eval(source) else {
        panic!("a duplicate group name is an early error");
    };
    let at = crate::span::line_col(source, span.start);
    assert_eq!(at.line, 1);
    assert!(at.column > 1, "the pattern is not at the start of the line");

    // A failure at the very end of input still has a position — the one *past* the last
    // character, which is a legal column and the place a missing `}` is missing from.
    let source = "function f() {";
    let Err(Error::Syntax { span, .. }) = engine.eval(source) else {
        panic!("an unclosed body does not parse");
    };
    assert!(span.start as usize >= source.len() - 1);
}

#[test]
fn the_three_failures_are_told_apart_because_a_host_answers_them_differently() {
    let mut engine = Engine::new();
    // Source that cannot be read is the host's own bug — it gave the engine nonsense.
    assert!(matches!(engine.eval("1 +"), Err(Error::Syntax { .. })));
    // A throw is the script's answer and is often the expected one, so it is a different case
    // and carries what a `catch` would have seen.
    assert_eq!(
        engine.eval("throw new TypeError('no')").unwrap_err(),
        Error::Thrown("TypeError: no".to_string())
    );
    // …and a thrown value that is not an Error at all still says something: §7.1.17 spells it.
    assert_eq!(
        engine.eval("throw 7").unwrap_err(),
        Error::Thrown("7".to_string())
    );
    // An engine-raised error reads the same as a script-thrown one, because it goes through the
    // realm's constructor rather than a spelling of this surface's own.
    assert_eq!(
        engine.eval("null.x").unwrap_err(),
        Error::Thrown(
            "TypeError: cannot read a property of something that is not an object".to_string()
        )
    );
}

/// Answers its first argument's length, so a test can tell it ran *and* that it was handed what
/// the call passed.
fn measure(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let length = heap.string(text).map_or(0, <[u16]>::len);
    Ok(Value::Number(length as f64))
}

/// A native's `Err` is a throw in the language and not a Rust failure.
fn refuse(_: &mut Vm, _: &mut Heap, _: &NativeCall<'_>) -> Completion<Value> {
    Err(crate::value::Abrupt::type_error("the host said no"))
}

#[test]
fn a_host_function_is_reachable_from_script_and_sees_its_arguments() {
    let mut engine = Engine::new();
    engine.bind("measure", 1, measure);
    let answer = engine.eval("measure('abcd')").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("4"));
    // §10.3.3's two properties, which every built-in has and which diagnostics read. A host
    // function without them is not the same kind of thing as one of ours.
    let answer = engine
        .eval("measure.name + '/' + measure.length")
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("measure/1"));
    // It is an ordinary function, so the language reaches it the way it reaches any other.
    let answer = engine
        .eval("[1, 22, 333].map(function (n) { return measure(String(n)) }).join(',')")
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("1,2,3"));
}

#[test]
fn a_host_function_that_throws_is_caught_by_the_script() {
    let mut engine = Engine::new();
    engine.bind("refuse", 0, refuse);
    let answer = engine
        .eval("try { refuse() } catch (e) { e.constructor.name + ': ' + e.message }")
        .expect("it runs");
    assert_eq!(
        engine.text(answer).as_deref(),
        Ok("TypeError: the host said no")
    );
    // …and uncaught it reaches the host as a throw rather than as a fault.
    assert_eq!(
        engine.eval("refuse()").unwrap_err(),
        Error::Thrown("TypeError: the host said no".to_string())
    );
}

#[test]
fn a_value_crosses_the_boundary_in_both_directions() {
    let mut engine = Engine::new();
    let object = engine
        .eval("({ a: 1, greet: function (who) { return 'hi ' + who } })")
        .expect("it runs");
    // Reading a property walks the whole prototype chain and runs a getter, so it is the
    // script's own read rather than a peek at a table.
    let a = engine.get(object, "a").expect("a is there");
    assert_eq!(engine.text(a).as_deref(), Ok("1"));
    // Calling back in: the receiver is the host's to choose, which is what makes this a method
    // call rather than a bare one.
    let greet = engine.get(object, "greet").expect("greet is there");
    let name = engine.eval("'world'").expect("it runs");
    let said = engine.call(greet, object, &[name]).expect("it calls");
    assert_eq!(engine.text(said).as_deref(), Ok("hi world"));
    // A value the host holds can be handed back to the script by name.
    engine.set_global("held", object).expect("it is live");
    let answer = engine.eval("held.a").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("1"));
}

#[test]
fn calling_something_that_is_not_a_function_throws_rather_than_faulting() {
    let mut engine = Engine::new();
    let not_callable = engine.eval("({})").expect("it runs");
    assert!(matches!(
        engine.call(not_callable, Value::Undefined, &[]),
        Err(Error::Thrown(_))
    ));
    // Reading a property of `undefined` is the same: the host is given the script's error and
    // not a Rust one, because DR-0002 says nothing a caller does may panic.
    assert!(matches!(
        engine.get(Value::Undefined, "anything"),
        Err(Error::Thrown(_))
    ));
}

/// Uppercases its argument through [`Host`] alone — the operations an *out-of-crate* native has.
///
/// The whole of this test's point: fourteen tests passed while a bound function could not
/// convert its own arguments, because a test inside the crate can reach `Vm::to_string` and a
/// real host cannot. `examples/embed.rs` found it; this pins it.
fn shout(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    let said = host.text(call.argument(0))?;
    Ok(host.string(&said.to_uppercase()))
}

/// Calls its first argument with its second, so a callback crosses the boundary both ways.
fn apply_to(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    host.call(call.argument(0), Value::Undefined, &[call.argument(1)])
}

/// Reads `.width` off its argument and refuses what has none.
fn width_of(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut host = Host::new(vm, heap);
    let width = host.get(call.argument(0), "width")?;
    match width {
        Value::Number(_) => Ok(width),
        _ => Err(Host::type_error("that has no numeric width")),
    }
}

#[test]
fn a_native_can_do_its_work_with_only_what_a_host_can_reach() {
    let mut engine = Engine::new();
    engine.bind("shout", 1, shout);
    engine.bind("applyTo", 2, apply_to);
    engine.bind("widthOf", 1, width_of);

    let answer = engine.eval("shout('hi') + shout(42)").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("HI42"));

    // A callback handed to a native and called back into the script — most of what a host API
    // is, and it needs `Host::call` rather than anything the interpreter exposes.
    let answer = engine
        .eval("applyTo(function (n) { return n * 3 }, 7)")
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("21"));

    // Reading a property, and refusing with a message the script catches as its own TypeError.
    let answer = engine.eval("widthOf({ width: 5 })").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("5"));
    let answer = engine
        .eval("try { widthOf({}) } catch (e) { e.constructor.name + ': ' + e.message }")
        .expect("it runs");
    assert_eq!(
        engine.text(answer).as_deref(),
        Ok("TypeError: that has no numeric width")
    );

    // A conversion that throws inside a native travels out as the script's throw, not as a
    // Rust failure — which is what makes `?` the right thing to write in one.
    let answer = engine
        .eval("try { shout(Symbol()) } catch (e) { e.constructor.name }")
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("TypeError"));
}

#[test]
fn a_string_a_native_makes_is_interned_rather_than_copied() {
    // `Host::string` goes through the intern table, so a host loop answering one word does not
    // fill the arena with copies of it. Measured as a footprint that does not grow rather than
    // as an identity, because two equal Strings are equal either way and only the cost differs.
    let mut engine = Engine::new();
    engine.bind("shout", 1, shout);
    engine.eval("shout('warm')").expect("it runs");
    let before = engine.footprint();
    engine
        .eval("for (var i = 0; i < 200; i++) { shout('warm') }")
        .expect("it runs");
    let grew = engine.footprint() - before;
    assert!(
        grew < 200 * std::mem::size_of::<u16>() * 4,
        "200 answers of one word grew the heap by {grew} bytes"
    );
}

/// A budget small enough that a runaway is stopped quickly, and large enough that an ordinary
/// script finishes well inside it. Every row below runs one loop that never ends, so the test
/// costs about this much wall-clock each time it is reached.
const BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

#[test]
fn a_script_that_will_not_stop_is_stopped() {
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    assert_eq!(
        engine.eval("while (true) {}").unwrap_err(),
        Error::Interrupted
    );
    // The machine is usable again, because DR-0022 clears the flag when a run *begins* rather
    // than when one ends — the flag has to survive unwinding every nested execution above it.
    let answer = engine.eval("1 + 1").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    // …and the budget is still in force, so the next runaway is stopped too. Set once, applied
    // per run: a deadline fixed when the host set it would have passed by now.
    assert_eq!(engine.eval("for (;;) {}").unwrap_err(), Error::Interrupted);
}

#[test]
fn a_stopped_run_cannot_be_caught_and_runs_no_finally() {
    // The decision the whole record turns on. A budget a script can catch is not a budget:
    // `catch` would swallow it and the loop would resume, and the check meant to stop a runaway
    // would fire again for ever.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    engine.eval("var reached = 'no'").expect("it runs");
    assert_eq!(
        engine
            .eval("try { while (true) {} } catch (e) { reached = 'catch' }")
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("reached").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    // A `finally` is the same answer for the same reason: it is code, and the machine reads no
    // more instructions. A host that needs cleanup has to do it in Rust, and DR-0022 says so.
    assert_eq!(
        engine
            .eval("try { while (true) {} } finally { reached = 'finally' }")
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("reached").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
}

#[test]
fn a_stop_underneath_a_call_stops_the_caller_too() {
    // Why the flag is read before *every* instruction and not only where the deadline is
    // checked. When a nested execution stops it simply returns, and the call it was serving is
    // left with a frame it never popped and a value it never produced — so the caller carries
    // on as though the call had answered. Without this check the outer loop runs on for another
    // whole check interval, which is a thousand instructions of a script that was supposed to
    // have been stopped.
    //
    // **It is not enough to wrap the call in `try`/`catch`.** A stopped nested execution does
    // not throw — that was the first guess and it left this row passing with the check removed,
    // because the `catch` was never reached in either case. What distinguishes them is an
    // ordinary statement *after* the call.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    engine.eval("var reached = 'no'").expect("it runs");
    assert_eq!(
        engine
            .eval("[1].map(function () { while (true) {} }); reached = 'after the callback'")
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("reached").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    // The same one level down through a coercion, which enters the loop from the middle of an
    // instruction rather than from a native — a different way in and the same answer.
    assert_eq!(
        engine
            .eval(
                "({ valueOf: function () { while (true) {} } }) + 1; reached = 'after the coercion'"
            )
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("reached").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
}

#[test]
fn a_stopped_run_drains_no_jobs() {
    // §9.5's queue is drained at the end of a run, and a job is code like any other — a `then`
    // handler that loops for ever is the same problem wearing a promise. So an interrupted run
    // answers without draining.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    engine.eval("var ran = 'no'").expect("it runs");
    assert_eq!(
        engine
            .eval("Promise.resolve().then(function () { ran = 'yes' }); while (true) {}")
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("ran").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
}

#[test]
fn a_waiters_timeout_never_outlasts_the_runs_budget() {
    // The one place the engine sleeps is the end of a drain with a `waitAsync` still owed an
    // answer, and it is the one place a *program* could otherwise decide how long the host waits
    // for control back. It cannot: the sleep ends at the earlier of the two deadlines, and the
    // waiter stays parked.
    //
    // Written as an elapsed time rather than as a settled value because the value is the same
    // either way — a waiter that has not timed out and one the run gave up on both leave the
    // promise pending. What tells them apart is the clock.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    let started = std::time::Instant::now();
    engine
        .eval(
            "var a = new Int32Array(new SharedArrayBuffer(32)); \
             Atomics.waitAsync(a, 0, 0, 5000);",
        )
        .expect("parking is not itself a runaway");
    // Five seconds asked for, a budget of milliseconds, and a second between them. Two orders of
    // magnitude either side, so the row says which of the two bounded the wait without being a
    // race — and five rather than sixty because a mutant that inverts the choice makes every
    // sandboxed run of this test wait out the whole of it.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the run came back in {:?}, so it waited on the script's timeout",
        started.elapsed()
    );
}

#[test]
fn a_loop_inside_a_coercion_or_a_callback_is_stopped_too() {
    // The reason the flag is read before an instruction rather than checked once per run: a
    // coercion re-enters the interpreter from the *middle* of an instruction, and a callback
    // enters it from inside a native. Both are executions of their own and both must stop.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    assert_eq!(
        engine
            .eval("({ valueOf: function () { while (true) {} } }) + 1")
            .unwrap_err(),
        Error::Interrupted
    );
    assert_eq!(
        engine
            .eval("[1, 2, 3].map(function () { while (true) {} })")
            .unwrap_err(),
        Error::Interrupted
    );
    // A generator resumed from a `for`-`of` is a third way in, and parks and revives an
    // execution rather than nesting one.
    assert_eq!(
        engine
            .eval("function* g() { while (true) { yield 1 } } for (var x of g()) {}")
            .unwrap_err(),
        Error::Interrupted
    );
}

#[test]
fn no_budget_is_the_default_and_removing_one_restores_it() {
    // Off unless a host asks, which is what leaves the conformance suite, the examples and
    // every existing caller exactly as they were.
    let mut engine = Engine::new();
    let answer = engine
        .eval("var n = 0; for (var i = 0; i < 200000; i++) { n += i } n")
        .expect("no budget, so it finishes however long it takes");
    assert_eq!(engine.text(answer).as_deref(), Ok("19999900000"));
    // A budget that is generous does not stop ordinary work either — the check is a deadline
    // and not a step count, so a loop that finishes in time finishes.
    engine.set_time_budget(Some(std::time::Duration::from_secs(30)));
    let answer = engine
        .eval("var n = 0; for (var i = 0; i < 200000; i++) { n += i } n")
        .expect("well inside thirty seconds");
    assert_eq!(engine.text(answer).as_deref(), Ok("19999900000"));
    // …and it can be taken off again.
    engine.set_time_budget(None);
    let answer = engine.eval("1 + 1").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
}

#[test]
fn a_single_enormous_string_build_is_stopped_like_any_other_walk() {
    // DR-0022's "what this does not stop" named this one: a built-in that takes a long time
    // **without** walking a length. `"a".repeat(n)` is the sharpest of them — one fill loop of
    // up to `MAX_STRING_LENGTH` turns, entered directly, with no bounded work in front of it to
    // spend the budget first. The size is already refused past DR-0012's cap; the *time* was
    // not bounded at all.
    //
    // Measured with the check removed by hand: 268,435,455 units takes ~700 ms whatever the
    // budget says. The 50 ms asked for here leaves more than an order of magnitude, which is
    // what keeps this from being a test about how fast the machine is.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(std::time::Duration::from_millis(50)));
    let started = std::time::Instant::now();
    let answer = engine.eval("'a'.repeat(268435455).length");
    let took = started.elapsed();
    assert!(
        matches!(answer, Err(Error::Interrupted)),
        "the build must be stopped rather than finished: {answer:?}"
    );
    assert!(
        took < std::time::Duration::from_millis(400),
        "the stop must arrive near the budget rather than at the end of the build: {took:?}"
    );
    // …and the stop is the machine's rather than the string's, so the engine is usable again.
    let answer = engine
        .eval("1 + 1")
        .expect("the machine is usable after a stop");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
}

#[test]
fn a_sort_with_no_comparator_is_stopped_although_nothing_it_runs_is_a_program() {
    // The other half of DR-0022's paragraph, and the measurement disagreed with it: the record
    // named "a sort's comparator loop" as unbounded and it is not. **This passed before
    // anything was changed**, which is why the sort has no new check — see the amendment.
    //
    // Two doors close it between them. §23.1.3.30.1's gathering walk asks the budget once per
    // index, so a sort big enough to matter spends the budget being read rather than being
    // sorted; and the merge asks once per pass, so what is left is an overshoot of one linear
    // pass, which costs about what the gathering it followed was already allowed.
    //
    // No comparator on purpose: a JavaScript one re-enters the interpreter, which checks the
    // budget between instructions, so the pure-Rust comparison is the only one that could have
    // run away.
    let mut engine = Engine::new();
    engine
        .eval("var a = []; for (var i = 0; i < 400000; i++) a.push((i * 7919) % 100003);")
        .expect("the array is built with no budget set");
    engine.set_time_budget(Some(std::time::Duration::from_millis(10)));
    let answer = engine.eval("a.sort(); a.length");
    assert!(
        matches!(answer, Err(Error::Interrupted)),
        "the sort must be stopped: {answer:?}"
    );
    let answer = engine
        .eval("1 + 1")
        .expect("the machine is usable after a stop");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
}

#[test]
fn the_budget_does_not_reach_the_regular_expression_matcher() {
    // DR-0022 says this in its "what this does not stop", and a limitation stated only in prose
    // is one nobody finds out has changed. §22.2's backtracking is its own loop and does not
    // read the stop flag, so a hostile pattern runs to completion however small the budget is.
    //
    // `/(a+)+b/` against a subject of `a`s that can never match is the classic: every extra `a`
    // doubles the work. Measured here at 52 ms, 210 ms and 689 ms for widths 18, 20 and 22
    // against a 10 ms budget — so 22 leaves a margin of about seventy times over, which is what
    // keeps this from being a test about how fast the machine is.
    //
    // **If this fails, the matcher has gained a check and that is good news** — update
    // DR-0022's list and this row rather than the budget.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(std::time::Duration::from_millis(10)));
    let answer = engine
        .eval("/(a+)+b/.test('aaaaaaaaaaaaaaaaaaaaaa')")
        .expect("the matcher runs to the end, budget or no budget");
    assert_eq!(engine.text(answer).as_deref(), Ok("false"));
    // …and the machine was never stopped, so the very next statement runs in the same breath.
    let answer = engine.eval("1 + 1").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
}

#[test]
fn a_thrown_object_is_described_by_the_fields_it_actually_has() {
    // What a host prints when a script throws, and the three shapes are genuinely different
    // rather than an ordering of one rule. An Error has both fields; an object may have one,
    // the other, or neither, and `throw` accepts all of them.
    let mut engine = Engine::new();
    let said = |engine: &mut Engine, source: &str| match engine.eval(source) {
        Err(Error::Thrown(text)) => text,
        other => panic!("expected a throw, got {other:?}"),
    };
    assert_eq!(
        said(&mut engine, "throw new TypeError('no')"),
        "TypeError: no"
    );
    // §20.5.3.3's `message` defaults to the **empty string**, so an Error made without one is
    // its name alone. Written as `name` and not `"name: "` — the separator belongs to the
    // message, and a trailing colon is what joining them unconditionally produces.
    assert_eq!(said(&mut engine, "throw new TypeError()"), "TypeError");
    // A name and no message at all. `undefined` is *absent* here rather than a value to spell,
    // which is the distinction `[[Get]]` cannot draw and this layer must.
    assert_eq!(said(&mut engine, "throw ({ name: 'Weird' })"), "Weird");
    // …and with no name there is nothing to lead with, so the object speaks for itself through
    // §7.1.17 rather than being announced as `undefined`.
    assert_eq!(
        said(&mut engine, "throw ({ message: 'lonely' })"),
        "[object Object]"
    );
    assert_eq!(said(&mut engine, "throw ({})"), "[object Object]");
    // An empty name is present and useless, which is the same case as absent.
    assert_eq!(
        said(&mut engine, "throw ({ name: '', message: 'm' })"),
        "[object Object]"
    );
    // A thrown primitive never had fields to read.
    assert_eq!(said(&mut engine, "throw 7"), "7");
    assert_eq!(said(&mut engine, "throw 'plain'"), "plain");
    // A subclass keeps its own name, which is the reason to read the field rather than the
    // constructor: `name` is what the object says it is.
    assert_eq!(
        said(
            &mut engine,
            "class Mine extends Error { constructor() { super('detail'); this.name = 'Mine' } } throw new Mine()"
        ),
        "Mine: detail"
    );
}

#[test]
fn a_collected_value_is_refused_and_never_read_as_undefined() {
    // DR-0021's rule, and the reason it is a record rather than a doc line: a `Value` the host
    // holds is not a root, and `collect` keeps only what the *program* can reach.
    //
    // **The refusal is this surface's and not the heap's**, which is what the first draft got
    // wrong. `[[Get]]` on an object that is no longer there degrades to `undefined` — the same
    // answer an absent property gives — so without a check the host cannot tell "no such
    // property" from "the object you are asking about is gone". Measured before it was written:
    // the read answered `Ok(undefined)`.
    let mut engine = Engine::new();
    let held = engine.eval("({ a: 1 })").expect("it runs");
    // Enough other work that nothing the machine happens to be holding — §14.2.2's completion
    // register, the operand stack — still names it. Which of those root a value is not part of
    // any promise, and a test that relied on one would be pinning an accident.
    for _ in 0..50 {
        engine.eval("({ junk: 1 })").expect("it runs");
    }
    assert!(engine.collect().objects > 0, "the collector did something");
    assert_eq!(engine.get(held, "a").unwrap_err(), Error::Collected);
    assert_eq!(engine.text(held), Err(Error::Collected));
    assert_eq!(
        engine.call(held, Value::Undefined, &[]).unwrap_err(),
        Error::Collected
    );
    assert_eq!(engine.set_global("x", held), Err(Error::Collected));
    // An argument is checked too, and not only the callee — a collected one would arrive in the
    // script as `undefined` and read as a value the host chose to pass.
    let live = engine.eval("(function (x) { return x })").expect("it runs");
    assert_eq!(
        engine.call(live, Value::Undefined, &[held]).unwrap_err(),
        Error::Collected
    );
}

#[test]
fn the_global_object_is_how_a_host_roots_a_value() {
    // The escape hatch, and it is the one the language already has rather than a second
    // lifetime discipline: anything the program can reach survives, so anything on the global
    // does.
    let mut engine = Engine::new();
    let held = engine.eval("({ a: 1 })").expect("it runs");
    engine.set_global("held", held).expect("it is live");
    for _ in 0..50 {
        engine.eval("({ junk: 1 })").expect("it runs");
    }
    engine.collect();
    let a = engine.get(held, "a").expect("it survived");
    assert_eq!(engine.text(a).as_deref(), Ok("1"));
    // …and the script sees the same object, which is what makes it a root rather than a copy.
    let answer = engine.eval("held.a = 2; held.a").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("2"));
    let a = engine.get(held, "a").expect("still there");
    assert_eq!(engine.text(a).as_deref(), Ok("2"));
}

#[test]
fn a_value_with_nothing_to_point_at_is_never_collected() {
    // The four that live wholly inside the `Value` have no handle to go stale, so they must not
    // be refused after a collection — a liveness check that asked about them would answer for a
    // slot that does not exist.
    let mut engine = Engine::new();
    let number = engine.eval("42").expect("it runs");
    let boolean = engine.eval("true").expect("it runs");
    let nothing = engine.eval("null").expect("it runs");
    let missing = engine.eval("undefined").expect("it runs");
    for _ in 0..50 {
        engine.eval("({ junk: 1 })").expect("it runs");
    }
    engine.collect();
    assert_eq!(engine.text(number).as_deref(), Ok("42"));
    assert_eq!(engine.text(boolean).as_deref(), Ok("true"));
    assert_eq!(engine.text(nothing).as_deref(), Ok("null"));
    assert_eq!(engine.text(missing).as_deref(), Ok("undefined"));
}

#[test]
fn two_engines_share_nothing() {
    // GOAL.md §3 says isolation comes from running more engines, which is only true if this is.
    let mut first = Engine::new();
    let mut second = Engine::new();
    first.eval("var shared = 1").expect("it runs");
    assert!(matches!(second.eval("shared"), Err(Error::Thrown(_))));
    let answer = first.eval("shared").expect("it runs");
    assert_eq!(first.text(answer).as_deref(), Ok("1"));
}

#[test]
fn two_agents_read_and_write_one_block() {
    // §25.2 exists so that more than one agent can reach the same bytes, and until a host could
    // start a second one that was a claim nothing could test: a `SharedArrayBuffer` whose bytes
    // sat inside a single heap was shared in name only. The two engines here share *this* and
    // nothing else — two heaps, two realms, two of every intrinsic, one allocation.
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(8); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    let agent = std::thread::spawn(move || {
        let mut engine = Engine::new();
        let sab = engine.new_shared_buffer(&block);
        engine.set_global("sab", sab).expect("it is live");
        engine
            .eval("new Int32Array(sab)[1] = 7")
            .expect("it runs in the other agent");
    });
    agent.join().expect("the agent finished");
    let answer = main.eval("i32[1]").expect("it runs");
    assert_eq!(main.text(answer).as_deref(), Ok("7"));
}

#[test]
fn an_ordinary_array_buffer_has_no_block_to_share() {
    // §25.1's bytes belong to the heap that made them. Answering a block for one would let a
    // host hand another agent memory the first agent may `transfer` away underneath it.
    let mut engine = Engine::new();
    let unshared = engine.eval("new ArrayBuffer(8)").expect("it runs");
    assert!(engine.shared_block(unshared).is_none());
    let number = engine.eval("42").expect("it runs");
    assert!(engine.shared_block(number).is_none());
    let shared = engine.eval("new SharedArrayBuffer(8)").expect("it runs");
    assert!(engine.shared_block(shared).is_some());
}

#[test]
fn an_engine_no_host_has_spoken_for_refuses_to_block() {
    // §25.4.3.14 step 12 with `[[CanBlock]]` false, which is the default and is the right
    // answer for an engine running on its own: nothing else could ever notify it.
    let mut engine = Engine::new();
    let answer = engine
        .eval(
            "try { Atomics.wait(new Int32Array(new SharedArrayBuffer(8)), 0, 0, 0) } \
             catch (e) { e.message }",
        )
        .expect("it runs");
    assert_eq!(
        engine.text(answer).as_deref(),
        Ok("this agent cannot be suspended")
    );
}

/// Run `source` and answer what it evaluated to, as text — what every row about waiting wants.
fn answered(engine: &mut Engine, source: &str) -> String {
    let value = engine.eval(source).expect("it runs");
    engine.text(value).expect("a string")
}

/// A second agent: an engine of its own on a thread of its own, sharing `block` and nothing
/// else, which evaluates `source` and answers what it came to as text.
///
/// `sab` is the received `SharedArrayBuffer` — the only thing that crosses between the two.
fn agent(block: crate::heap::Block, source: &'static str) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut engine = Engine::new();
        // §9.7 — an agent something else started may be suspended, because that something else
        // is still running and can notify it.
        engine.set_can_block(true);
        let sab = engine.new_shared_buffer(&block);
        engine.set_global("sab", sab).expect("it is live");
        let value = engine.eval(source).expect("it runs in the other agent");
        engine.text(value).expect("a string")
    })
}

/// Ask `source` of `engine` until it answers `wanted`, and fail rather than hang if it will not.
///
/// What a test that has started another agent uses instead of sleeping: there is no moment at
/// which this thread can be *told* the other has reached its wait, so an answer that only the
/// other agent could have produced is the evidence, and asking again is how it is waited for.
fn until(engine: &mut Engine, source: &str, wanted: &str, why: &str) {
    for _ in 0..5_000 {
        if answered(engine, source) == wanted {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("{why}");
}

#[test]
fn a_native_can_convert_what_it_was_given_to_a_number() {
    // §7.1.4 across the boundary, which is what a host function taking a duration or a count
    // wants. The object row is the one that matters: `valueOf` runs, so this is the language's
    // own conversion and not a parse of whatever `String(value)` happened to produce.
    fn doubled(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
        let mut host = Host::new(vm, heap);
        let given = host.number(call.argument(0))?;
        Ok(Value::Number(given * 2.0))
    }
    let mut engine = Engine::new();
    engine.bind("doubled", 1, doubled);
    assert_eq!(answered(&mut engine, "doubled('21')"), "42");
    assert_eq!(
        answered(
            &mut engine,
            "doubled({ valueOf: function () { return 4 } })"
        ),
        "8"
    );
    // Absent is `NaN` and not zero, which is the difference every caller has to decide what to
    // do about — this one doubles it and gets `NaN` back, which is §6.1.6.1 working.
    assert_eq!(answered(&mut engine, "doubled()"), "NaN");
    // A Symbol has no reading at all, and the refusal arrives as the script's to catch.
    assert_eq!(
        answered(
            &mut engine,
            "try { doubled(Symbol()) } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_wait_ends_at_its_timeout_and_a_stale_expectation_does_not_wait_at_all() {
    // Both of §25.4.3.14's answers that need no second agent, which is why they are together:
    // one engine can measure them and a hang is what a wrong answer looks like.
    let mut engine = Engine::new();
    engine.set_can_block(true);
    engine
        .eval("var i32 = new Int32Array(new SharedArrayBuffer(8))")
        .expect("it runs");
    // A finite timeout that nobody notifies. DR-0024's gap was exactly this and it was the
    // *asynchronous* half that could not be closed — a parked thread has a clock to wake it.
    let started = std::time::Instant::now();
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 0, 0, 30)"),
        "timed-out"
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(20));
    // And the comparison, on an **infinite** timeout: the slot no longer holds what the caller
    // expected, so there is nothing to wait for. This row hangs rather than fails if that
    // comparison is dropped, which is the shape of the bug it exists to catch.
    engine.eval("i32[0] = 1").expect("it runs");
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 0, 0)"),
        "not-equal"
    );
}

#[test]
fn a_wait_reads_the_byte_its_index_names_and_the_value_as_the_slot_stores_it() {
    let mut engine = Engine::new();
    engine.set_can_block(true);
    engine
        .eval("var i32 = new Int32Array(new SharedArrayBuffer(16)); i32[0] = 5")
        .expect("it runs");
    // §25.4.1 keys a position by **byte**, so index 1 of an `Int32Array` is byte 4. A wait that
    // read byte 0 instead would find the 5 that was put there to notice it, and would answer
    // the other way round on both of these rows.
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 1, 5, 0)"),
        "not-equal"
    );
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 0, 5, 0)"),
        "timed-out"
    );
    // Step 17 compares against the value **as the slot stores it**, and storing into an
    // `Int32Array` wraps rather than clamps: 2**31 lands there as -2147483648, so a wait
    // expecting 2**31 of a slot holding that matches and gets as far as its timeout. §7.1.11's
    // clamping would encode the same expectation as 255 and it would never match anything.
    engine.eval("i32[2] = -Math.pow(2, 31)").expect("it runs");
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 2, Math.pow(2, 31), 0)"),
        "timed-out"
    );
}

#[test]
fn a_wait_that_timed_out_has_taken_itself_off_the_list() {
    // Nobody else can take it off — that is what timing out means — so it does so on its way
    // past, and a notify afterwards has to find nothing. A list that kept it would report
    // having woken a thread that stopped waiting long ago, and the count is a number a program
    // reads and asserts on.
    let mut engine = Engine::new();
    engine.set_can_block(true);
    engine
        .eval("var i32 = new Int32Array(new SharedArrayBuffer(16))")
        .expect("it runs");
    assert_eq!(
        answered(&mut engine, "Atomics.wait(i32, 1, 0, 5)"),
        "timed-out"
    );
    assert_eq!(answered(&mut engine, "Atomics.notify(i32, 1)"), "0");
}

#[test]
fn an_agent_that_may_block_waits_until_another_notifies_it() {
    // The pair that only two agents can show, and the reason the waiter list had to move out of
    // the machine and into the block: this waiter is a parked *thread*, and the notify that
    // ends it is made by an engine that has never heard of it.
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(8); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    let other = agent(block, "Atomics.wait(new Int32Array(sab), 0, 0)");
    until(
        &mut main,
        "Atomics.notify(i32, 0)",
        "1",
        "the other agent never reached its wait",
    );
    assert_eq!(other.join().expect("the agent finished"), "ok");
}

#[test]
fn a_notify_wakes_only_the_position_it_names_and_only_as_many_as_it_was_asked_for() {
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(16); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    let other = agent(block, "Atomics.wait(new Int32Array(sab), 1, 0, 30000)");
    // Asked over and over for about a tenth of a second, and that is the point rather than
    // impatience: both of these answer `0` whether or not the other agent has parked yet, so a
    // single call proves nothing and a call repeated across the moment it parks proves both. A
    // notify at another position must wake nobody, and a count of zero must wake nobody however
    // many are there to be woken.
    for _ in 0..100 {
        assert_eq!(
            answered(&mut main, "Atomics.notify(i32, 0)"),
            "0",
            "a notify at another position"
        );
        assert_eq!(
            answered(&mut main, "Atomics.notify(i32, 1, 0)"),
            "0",
            "a notify of nobody at this one"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    until(
        &mut main,
        "Atomics.notify(i32, 1, 1)",
        "1",
        "the other agent never reached its wait",
    );
    assert_eq!(other.join().expect("the agent finished"), "ok");
}

#[test]
fn two_agents_adding_to_one_slot_lose_nothing() {
    // §25.4.1.2's read-modify-write is **one** step, and with a single agent nothing can tell
    // that from a read followed by a write. With two, everything can: this row counted 39,997
    // of 40,000 the first time it was run against a version that read and wrote separately.
    //
    // It is not an academic number. `atomicsHelper.js` waits for agents to start by spinning
    // until `Atomics.add(i32a, RUNNING, 1)` has reached the number of them — so a lost update
    // there is not a wrong answer but a test that never finishes, which is how this was found.
    const EACH: usize = 20_000;
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(8); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    let other = agent(
        block,
        "var a = new Int32Array(sab); \
         for (var i = 0; i < 20000; i++) { Atomics.add(a, 0, 1); } 'done'",
    );
    main.eval("for (var i = 0; i < 20000; i++) { Atomics.add(i32, 0, 1); }")
        .expect("it runs");
    assert_eq!(other.join().expect("the agent finished"), "done");
    assert_eq!(
        answered(&mut main, "Atomics.load(i32, 0)"),
        (EACH * 2).to_string()
    );
}

#[test]
fn two_agents_incrementing_by_compare_exchange_lose_nothing() {
    // The same claim about §25.4.3.3, and it took two attempts to state it in a way that could
    // fail. The first built a mutual-exclusion lock out of `compareExchange` and counted inside
    // it; that passed against a version where the compare and the write were separate steps,
    // because both agents entering needed a *second* collision on the unprotected counter and
    // two thousand rounds never produced one. It was a test that asserted what the code did.
    //
    // This one puts the collision where the bug is. A compare-exchange increment loop retries
    // until the slot still holds what it read, so if two agents can both find the same `old`
    // and both write `old + 1`, one increment is gone — and the window for that is the exchange
    // itself, the same window the `Atomics.add` row above loses four hundred updates in.
    const EACH: usize = 10_000;
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(8); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    let other = agent(
        block,
        "var a = new Int32Array(sab); \
         for (var i = 0; i < 10000; i++) { \
             var old; \
             do { old = Atomics.load(a, 0); } \
             while (Atomics.compareExchange(a, 0, old, old + 1) !== old); \
         } 'done'",
    );
    main.eval(
        "for (var i = 0; i < 10000; i++) { \
             var old; \
             do { old = Atomics.load(i32, 0); } \
             while (Atomics.compareExchange(i32, 0, old, old + 1) !== old); \
         }",
    )
    .expect("it runs");
    assert_eq!(other.join().expect("the agent finished"), "done");
    assert_eq!(
        answered(&mut main, "Atomics.load(i32, 0)"),
        (EACH * 2).to_string()
    );
}

#[test]
fn a_notify_spends_its_count_on_parked_threads_before_its_own_promises() {
    // The seam DR-0024's amendment names. A blocking waiter is another agent's thread and an
    // asynchronous one is this agent's promise, and this agent settles its own promises at its
    // own convenience — so a count spent on the promise while the thread stayed parked is a
    // right number and a program that never finishes.
    let mut main = Engine::new();
    main.eval("var sab = new SharedArrayBuffer(16); var i32 = new Int32Array(sab)")
        .expect("it runs");
    let sab = main.eval("sab").expect("it runs");
    let block = main.shared_block(sab).expect("a shared buffer has one");
    // Slot 1 says "about to wait", which is `atomicsHelper.js`'s own idiom and carries its
    // caveat: it is the last statement *before* the wait and not the wait itself, so what reads
    // it waits a little afterwards rather than trusting it.
    let other = agent(
        block,
        "var a = new Int32Array(sab); Atomics.store(a, 1, 1); Atomics.wait(a, 0, 0, 30000)",
    );
    until(
        &mut main,
        "Atomics.load(i32, 1)",
        "1",
        "the other agent never started",
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Parked *after* the thread is, so that a notify of one waiter has two to choose between.
    main.eval("var parked = Atomics.waitAsync(i32, 0, 0)")
        .expect("it runs");
    assert_eq!(answered(&mut main, "Atomics.notify(i32, 0, 1)"), "1");
    assert_eq!(other.join().expect("the agent finished"), "ok");
    // …and the promise is still parked, which is the other half of the same claim: the one wake
    // went to the thread and none of it went here.
    assert_eq!(answered(&mut main, "Atomics.notify(i32, 0)"), "1");
}

#[test]
fn the_job_queue_has_run_by_the_time_eval_returns() {
    // DR-0016 — jobs run inside `run`, so a promise settled by the script has had its reactions
    // delivered when the host is handed the answer. Without that a host would have to be told
    // to drain a queue it cannot see.
    let mut engine = Engine::new();
    let answer = engine
        .eval("var seen = 'pending'; Promise.resolve(7).then(function (v) { seen = v }); 0")
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("0"));
    let seen = engine.eval("seen").expect("it runs");
    assert_eq!(engine.text(seen).as_deref(), Ok("7"));
}
#[test]
fn probe_completion_register_rooting() {
    let mut e = Engine::new();
    let a = e.eval("({ n: 1 })").expect("runs");
    for _ in 0..50 {
        e.eval("({ junk: 1 })").expect("runs");
    }
    let freed = e.collect();
    println!("freed {:?}", freed.objects);
    match e.get(a, "n") {
        Ok(v) => println!("read gave Ok: {}", e.text(v).unwrap_or_default()),
        Err(err) => println!("read gave Err: {err:?}"),
    }
}

#[test]
fn the_time_budget_reaches_a_built_ins_own_loop() {
    // DR-0022 is checked between *instructions*, and a built-in walking an array-like's `length`
    // never reaches one — `Array.prototype.join.call({length: 2 ** 32 - 1})` is four billion turns
    // inside Rust. So a host that set a budget waited out all of them, which is the promise of the
    // budget failing rather than a slow engine.
    //
    // Measured on the machine this was written on: the walk below takes about thirty seconds
    // unbounded and finishes in well under one with a budget. The row asserts the *outcome* rather
    // than a duration, because a duration is a property of the machine — but the guard after it
    // fails if the budget stopped nothing at all.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    let started = std::time::Instant::now();
    assert_eq!(
        engine
            .eval("var a = []; a.length = 200000000; a.join('')")
            .unwrap_err(),
        Error::Interrupted
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the walk ran for {:?}, so the budget did not reach it",
        started.elapsed()
    );
    // Every method that walks a length goes through the same check, which is what makes one row
    // stand for the family — but the family is what a host is exposed to, so three more of it.
    for source in [
        "var a = []; a.length = 200000000; a.indexOf(1)",
        "var a = []; a.length = 200000000; a.lastIndexOf(1)",
        "Array.prototype.forEach.call({ length: 200000000 }, function () {})",
    ] {
        let mut engine = Engine::new();
        engine.set_time_budget(Some(BUDGET));
        assert_eq!(
            engine.eval(source).unwrap_err(),
            Error::Interrupted,
            "{source}"
        );
    }
    // …and it cannot be caught, which is the whole of DR-0022. A `try` around the walk sees
    // nothing: the machine's flag is already set, so the handler the throw unwinds to never runs.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    engine.eval("var caught = 'no'").expect("it runs");
    assert_eq!(
        engine
            .eval(
                "try { var a = []; a.length = 200000000; a.join('') } catch (e) { caught = 'yes' }"
            )
            .unwrap_err(),
        Error::Interrupted
    );
    let answer = engine.eval("caught").expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("no"));
    // A walk that finishes inside the budget still answers, which is the row that stops this
    // passing by interrupting everything.
    let mut engine = Engine::new();
    engine.set_time_budget(Some(BUDGET));
    let answer = engine
        .eval("[1, 2, 3].join('-')")
        .expect("a short walk finishes");
    assert_eq!(engine.text(answer).as_deref(), Ok("1-2-3"));
}

#[test]
fn a_host_may_fix_what_math_random_answers() {
    // §21.3.2.27 asks for an approximately uniform distribution over `[0, 1)` and **nothing** about
    // unpredictability, so fixing the sequence is inside the clause. What wants it is a tool that
    // runs the same program twice and compares — the fuzzer, whose seed fixed its inputs and could
    // not fix what the engine did with one, which is how a finding appeared once and never again.
    let taken = |seed: u64| {
        let mut engine = Engine::new();
        engine.set_random_seed(seed);
        let answer = engine
            .eval(
                "var out = []; for (var i = 0; i < 8; i++) out.push(Math.random()); out.join(',')",
            )
            .expect("it runs");
        engine.text(answer).expect("a String")
    };
    // The same seed answers the same sequence, which is the whole point.
    assert_eq!(taken(12345), taken(12345));
    // …and two seeds do not, which is what stops the row above passing on a generator that had
    // stopped moving.
    assert_ne!(taken(12345), taken(12346));
    // Zero is the one state a xorshift cannot leave and is mapped away rather than refused — and
    // **only** zero: `seed | 1` would map every even seed onto its odd neighbour, so these two
    // would agree. That exact bug was found in the fuzzer's own generator.
    assert_ne!(taken(0), taken(1));
    assert_ne!(taken(42), taken(43));
    // Whatever the seed, the answers are still §21.3.2.27's range.
    let mut engine = Engine::new();
    engine.set_random_seed(7);
    let answer = engine
        .eval(
            "var ok = true; \
             for (var i = 0; i < 500; i++) { var r = Math.random(); if (!(r >= 0 && r < 1)) ok = false } \
             ok",
        )
        .expect("it runs");
    assert_eq!(engine.text(answer).as_deref(), Ok("true"));
}

#[test]
fn reverse_reads_each_end_before_the_other() {
    // §23.1.3.24 steps 6.c to 6.f interleave the reads — `HasProperty` then `Get` for the
    // lower end, then the same for the upper — and a Proxy observes them as the order of its
    // traps. Reading both ends before either `Get` answers a different order.
    let mut engine = Engine::new();
    assert_eq!(
        answered(
            &mut engine,
            "var order = []; \
             var p = new Proxy({ length: 3, 0: 'a', 2: 'c' }, \
             { has(t, k) { order.push('has:' + k); return Reflect.has(t, k); }, \
               get(t, k) { order.push('get:' + k); return Reflect.get(t, k); } }); \
             Array.prototype.reverse.call(p); order.join(',')",
        ),
        // The leading `get:length` is §23.1.3.24 step 3 reading the length, which goes through
        // the same traps.
        "get:length,has:0,get:0,has:2,get:2",
    );
}
