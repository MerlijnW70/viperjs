//! §27.1.4's five methods that make an iterator, and §27.1.3.2's `Iterator.from`.

use super::*;

/// An endless source, and a watched one, both inheriting the helpers.
const SOURCES: &str = "function endless() { var n = 0; \
     var o = {next: function () { return {done: false, value: n++}; }}; \
     Object.setPrototypeOf(o, Iterator.prototype); return o; } \
     function watched(values) { \
     var o = {nextCalls: 0, closed: false, at: 0}; \
     o.next = function () { o.nextCalls++; \
         return o.at < values.length ? {done: false, value: values[o.at++]} : {done: true}; }; \
     o.return = function () { o.closed = true; return {done: true}; }; \
     Object.setPrototypeOf(o, Iterator.prototype); return o; } ";

#[test]
fn nothing_happens_until_something_asks_for_a_value() {
    // The whole point of the five. `map` returns having called its mapper *not at all*, and the
    // first `next` draws exactly one value and makes exactly one call. An implementation that
    // collected and transformed would answer the same array and fail every row here.
    assert_eq!(
        run(
            "var calls = 0; var it = [1, 2, 3].values().map(function (x) { calls++; return x; }); \
             calls + ',' + it.next().value + ',' + calls + ',' + it.next().value + ',' + calls"
        ),
        "0,1,1,2,2"
    );
    assert_eq!(
        run(&format!(
            "{SOURCES} var it = watched([1, 2, 3]); var mapped = it.map(function (x) {{ return x; }}); \
             it.nextCalls + ',' + mapped.next().value + ',' + it.nextCalls"
        )),
        "0,1,1"
    );
    // …and laziness is what lets a finite question be asked of an endless source.
    assert_eq!(
        run(&format!("{SOURCES} endless().take(3).toArray().join(',')")),
        "0,1,2"
    );
    assert_eq!(
        run(&format!(
            "{SOURCES} endless().map(function (x) {{ return x * 2; }}).take(3).toArray().join(',')"
        )),
        "0,2,4"
    );
    assert_eq!(
        run(&format!(
            "{SOURCES} endless().filter(function (x) {{ return x % 2 === 0; }}).take(3).toArray().join(',')"
        )),
        "0,2,4"
    );
}

#[test]
fn each_of_the_five_does_what_its_name_says() {
    assert_eq!(
        run("[1, 2, 3].values().map(function (x) { return x * 2; }).toArray().join(',')"),
        "2,4,6"
    );
    assert_eq!(
        run("[1, 2, 3, 4].values().filter(function (x) { return x % 2; }).toArray().join(',')"),
        "1,3"
    );
    assert_eq!(
        run(
            "[1, 2, 3, 4, 5].values().take(2).toArray().join(',') + '|' \
             + [1, 2, 3, 4, 5].values().drop(3).toArray().join(',')"
        ),
        "1,2|4,5"
    );
    assert_eq!(
        run("[[1, 2], [3]].values().flatMap(function (x) { return x; }).toArray().join(',')"),
        "1,2,3"
    );
    // A count past the end is not an error, and nought yields nothing.
    assert_eq!(
        run("[1, 2].values().take(9).toArray().join(',') + '|' \
             + [1, 2].values().drop(9).toArray().length + '|' \
             + [1, 2].values().take(0).toArray().length"),
        "1,2|0|0"
    );
    // Both callbacks are handed the value **and its position**, counted across the whole walk.
    assert_eq!(
        run("[7, 8].values().map(function (x, i) { return x + ':' + i; }).toArray().join(',')"),
        "7:0,8:1"
    );
    // The helpers compose, because a helper is itself an iterator inheriting from
    // `Iterator.prototype`.
    assert_eq!(
        run(
            "[1, 2, 3, 4, 5, 6].values().filter(function (x) { return x % 2; }) \
             .map(function (x) { return x * 10; }).drop(1).toArray().join(',')"
        ),
        "30,50"
    );
    assert_eq!(
        run("Object.prototype.toString.call([1].values().map(function (x) { return x; }))"),
        "[object Iterator Helper]"
    );
}

#[test]
fn a_count_is_refused_where_every_other_count_in_the_library_would_clamp() {
    // §27.1.4.12 steps 3 to 7 — `NaN` is a **RangeError** here, where `ToIntegerOrInfinity` makes
    // it zero everywhere else in the library. A negative is refused too, and the iterator is closed
    // on the way out because the method was already holding it.
    for bad in ["-1", "NaN", "'x'", "undefined"] {
        for method in ["take", "drop"] {
            assert_eq!(
                run(&format!(
                    "{SOURCES} var it = watched([1]); \
                     try {{ it.{method}({bad}); }} catch (e) {{ e.constructor.name + ',' + it.closed }}"
                )),
                "RangeError,true",
                "{method}({bad})"
            );
        }
    }
    // `+∞` is "everything", which needs no case of its own.
    assert_eq!(
        run(
            "[1, 2, 3].values().take(Infinity).toArray().join(',') + '|' \
             + [1, 2, 3].values().drop(Infinity).toArray().length"
        ),
        "1,2,3|0"
    );
    // A fraction truncates rather than being refused.
    assert_eq!(
        run("[1, 2, 3].values().take(2.9).toArray().join(',')"),
        "1,2"
    );
    // A mapper that is not callable throws and closes, the same way the consumers do.
    for method in ["map", "filter", "flatMap"] {
        assert_eq!(
            run(&format!(
                "{SOURCES} var it = watched([1]); \
                 try {{ it.{method}(1); }} catch (e) {{ e.constructor.name + ',' + it.closed }}"
            )),
            "TypeError,true",
            "{method} with a callback that is not callable"
        );
    }
}

#[test]
fn a_helper_that_has_finished_stays_finished_and_closes_what_is_under_it() {
    // §27.1.4.12 step 5.a — a `take` that has had its fill closes the source rather than leaving
    // it open, and it does so without drawing another value.
    assert_eq!(
        run(&format!(
            "{SOURCES} var it = watched([1, 2, 3]); var taken = it.take(2); \
             taken.toArray().join(',') + '|' + it.nextCalls + ',' + it.closed"
        )),
        "1,2|2,true"
    );
    // §27.1.5.1's completed generator — once done, done. The source may start answering again and
    // the helper does not.
    assert_eq!(
        run("var values = [1]; \
             var o = {next: function () { return values.length ? {done: false, value: values.pop()} \
                 : {done: true}; }}; \
             Object.setPrototypeOf(o, Iterator.prototype); \
             var m = o.map(function (x) { return x; }); \
             var first = m.next(); var second = m.next(); values.push(9); var third = m.next(); \
             first.value + ',' + second.done + ',' + third.done + ',' + (third.value === undefined)"),
        "1,true,true,true"
    );
    // `return` finishes the helper and closes the source, and doing it twice closes once.
    assert_eq!(
        run(&format!(
            "{SOURCES} var it = watched([1, 2, 3]); var m = it.map(function (x) {{ return x; }}); \
             var r = m.return(); \
             r.done + ',' + (r.value === undefined) + ',' + it.closed + ',' + m.next().done"
        )),
        "true,true,true,true"
    );
    // Closing twice closes the source **once** — the second `return` finds a helper that has
    // already finished and has nothing left to pass on. Counting is the only way to see that: both
    // calls answer the same `{value: undefined, done: true}` either way.
    assert_eq!(
        run(&format!(
            "{SOURCES} var closes = 0; var it = watched([1, 2]);              it.return = function () {{ closes++; return {{done: true}}; }};              var m = it.map(function (x) {{ return x; }});              m.return(); m.return(); m.return(); closes"
        )),
        "1"
    );
    // §7.3.5 — the `{value, done}` a helper answers is made with `CreateDataProperty`, so both of
    // its properties are writable, enumerable and configurable. A result whose fields could not be
    // written would read back the same and behave differently the moment anything touched it.
    assert_eq!(
        run(
            "var r = [1].values().map(function (x) { return x; }).next();              var d = Object.getOwnPropertyDescriptor(r, 'value');              var e = Object.getOwnPropertyDescriptor(r, 'done');              d.writable + ',' + d.enumerable + ',' + d.configurable + '|'              + e.writable + ',' + e.enumerable + ',' + e.configurable + '|'              + Object.keys(r).join('/')"
        ),
        "true,true,true|true,true,true|value/done"
    );
    // A callback that throws finishes the helper and closes the source on the way out.
    assert_eq!(
        run(&format!(
            "{SOURCES} var it = watched([1, 2]); \
             var m = it.map(function () {{ throw new RangeError('no'); }}); \
             try {{ m.next(); }} catch (e) {{ e.constructor.name + ',' + it.closed + ',' + m.next().done }}"
        )),
        "RangeError,true,true"
    );
}

#[test]
fn what_may_be_flattened_and_what_from_will_accept_are_deliberately_different() {
    // §27.1.4.3 calls `GetIteratorFlattenable` with **reject-primitives**, so a mapper answering a
    // string is a TypeError rather than a sequence of letters. That is the one case where being
    // helpful would be wrong: `flatMap` over words would silently become `flatMap` over letters.
    assert_eq!(
        run(
            "try { [1].values().flatMap(function () { return 'ab'; }).toArray(); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    for bad in ["1", "null", "undefined", "true"] {
        assert_eq!(
            run(&format!(
                "try {{ [1].values().flatMap(function () {{ return {bad}; }}).toArray(); }} \
                 catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "flatMap answering {bad}"
        );
    }
    // …while §27.1.3.2 calls it with **iterate-string-primitives**, so `Iterator.from` does take a
    // string. Two callers, two readings, and the difference is the whole of it.
    assert_eq!(run("Iterator.from('ab').toArray().join(',')"), "a,b");
    assert_eq!(run("Iterator.from([1, 2]).toArray().join(',')"), "1,2");
    // An object with only a `next` is flattenable, which is what lets one helper feed another.
    assert_eq!(
        run("[1].values().flatMap(function () { \
                 var n = 0; \
                 return {next: function () { return n < 2 ? {done: false, value: n++} \
                     : {done: true}; }}; }).toArray().join(',')"),
        "0,1"
    );
    // Steps 2 and 3 — something that is already an Iterator is handed straight back rather than
    // wrapped, so the identity holds.
    assert_eq!(run("var a = [1].values(); Iterator.from(a) === a"), "true");
    assert_eq!(
        run("var m = [1].values().map(function (x) { return x; }); Iterator.from(m) === m"),
        "true"
    );
    // …and something that is not is wrapped, and the wrapper works.
    assert_eq!(
        run("var n = 0; \
             var bare = {next: function () { return n < 2 ? {done: false, value: n++} \
                 : {done: true}; }}; \
             var w = Iterator.from(bare); (w === bare) + ',' + w.toArray().join(',')"),
        "false,0,1"
    );
    for bad in ["1", "null", "undefined"] {
        assert_eq!(
            run(&format!(
                "try {{ Iterator.from({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "Iterator.from({bad})"
        );
    }
}

#[test]
fn a_bad_argument_closes_the_iterator_without_ever_reading_next() {
    // §27.1.4's methods judge their argument at step 3 and reach `GetIteratorDirect` at step 4, so
    // a bad one closes the iterator through a record whose `[[NextMethod]]` is still undefined —
    // `next` is never touched. Reading it first answers the same error and is observable to any
    // object that watches the read, which six test262 rows do.
    let watcher = "var log = []; var o = {get next() { log.push('get next'); return function () { return {done: true}; }; }, return: function () { log.push('return'); return {}; }}; Object.setPrototypeOf(o, Iterator.prototype); ";
    for (method, argument, kind) in [
        ("map", "1", "TypeError"),
        ("filter", "1", "TypeError"),
        ("flatMap", "1", "TypeError"),
        ("forEach", "1", "TypeError"),
        ("some", "null", "TypeError"),
        ("every", "{}", "TypeError"),
        ("find", "'x'", "TypeError"),
        ("reduce", "1", "TypeError"),
        ("take", "NaN", "RangeError"),
        ("drop", "-1", "RangeError"),
    ] {
        assert_eq!(
            run(&format!(
                "{watcher} try {{ o.{method}({argument}); }}                  catch (e) {{ e.constructor.name + '|' + log.join(',') }}"
            )),
            format!("{kind}|return"),
            "{method}({argument}) should close without reading next"
        );
    }
}

#[test]
fn a_helpers_return_reports_what_closing_the_source_found() {
    // §27.1.4's Iterator Helper `return` closes the underlying iterator with a **normal**
    // completion, so §7.4.9 step 4 has nothing to keep and steps 2, 5 and 6 are what the program
    // sees. Every other close in this engine is abandoning a walk because something already went
    // wrong, and the clause discards the close's own trouble there — which is why one helper does
    // the opposite of the other five callers rather than sharing their answer.
    //
    // Step 5, the ordinary way: a `return` that throws when called.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } \
               return() { throw new EvalError('r') } } \
             var t = new T().map(function (x) { return 0 }); t.next(); \
             try { t.return(); 'no error' } catch (e) { e.constructor.name }"
        ),
        "EvalError"
    );
    // Step 2 — the `return` **getter** throws, so the close never gets as far as calling anything.
    // A close that swallowed would not tell these two apart, and neither would one that only
    // guarded the call.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } \
               get return() { throw new EvalError('g') } } \
             var t = new T().map(function (x) { return 0 }); t.next(); \
             try { t.return(); 'no error' } catch (e) { e.constructor.name }"
        ),
        "EvalError"
    );
    // Step 6 — a `return` answering a primitive is a **TypeError of the clause's own**, which is
    // the one error §7.4.9 raises rather than passes on.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } \
               return() { return 1 } } \
             var t = new T().filter(function () { return true }); \
             try { t.return(); 'no error' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and an object answer is accepted without being looked at.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } \
               return() { return {} } } \
             var t = new T().map(function (x) { return x }); var r = t.return(); \
             r.value + ',' + r.done"
        ),
        "undefined,true"
    );
    // §7.3.11's other half: a `return` that is *there* and is not callable is a TypeError, and it
    // is raised before the call rather than by it — which is the whole of what the check is worth,
    // since calling a number would be a TypeError too. The **message** is the assertion, because
    // that is the only thing the two orders differ in.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } }              var o = new T(); o.return = 1;              var t = Iterator.prototype.map.call(o, function (x) { return x }); t.next();              try { t.return(); 'no error' } catch (e) { e.constructor.name + ': ' + e.message }"
        ),
        "TypeError: this iterator's return is not a function"
    );
    // §7.3.11 reads an absent `return` as nothing to tell, which is not a failure to close.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: false, value: 1} } } \
             var t = new T().take(5); var r = t.return(); r.value + ',' + r.done"
        ),
        "undefined,true"
    );
    // A helper that has not been started yet still closes its source — §27.1.4's
    // `suspended-start` arm — so this throws before `next` has ever been called.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: true, value: undefined} } \
               return() { throw new EvalError('r') } } \
             var t = new T().map(function (x) { return 0 }); \
             try { t.return(); 'no error' } catch (e) { e.constructor.name }"
        ),
        "EvalError"
    );
}

#[test]
fn a_source_that_ran_out_is_not_told_a_second_time() {
    // The other half, and the reason the reporting close sits behind a test of the helper's own
    // state: once the source has said `done` the helper is finished, and §7.4.9 is not owed a
    // telling. So this `return` throws nothing even though the source's would.
    assert_eq!(
        run(
            "class T extends Iterator { next() { return {done: true, value: undefined} } \
               return() { throw new EvalError('r') } } \
             var t = new T().map(function (x) { return 0 }); t.next(); \
             try { t.return(); 'not forwarded' } catch (e) { 'threw ' + e.constructor.name }"
        ),
        "not forwarded"
    );
    // …and neither does a second `return`, the first having finished it. The helper is marked
    // finished *before* the close, which is what makes a throwing source asked exactly once.
    assert_eq!(
        run("var count = 0; \
             class T extends Iterator { next() { return {done: false, value: 1} } \
               return() { count++; return {} } } \
             var t = new T().map(function (x) { return x }); \
             t.return(); t.return(); t.return(); count"),
        "1"
    );
    assert_eq!(
        run("var count = 0; \
             class T extends Iterator { next() { return {done: false, value: 1} } \
               return() { count++; throw new EvalError('r') } } \
             var t = new T().map(function (x) { return x }); \
             try { t.return() } catch (e) {} \
             try { t.return() } catch (e) {} count"),
        "1"
    );
    // Every helper closes the same way, which is what says this is one code path and not six.
    for helper in [
        "map(function (x) { return x })",
        "filter(function () { return true })",
        "take(5)",
        "drop(0)",
        "flatMap(function (x) { return [x] })",
    ] {
        assert_eq!(
            run(&format!(
                "class T extends Iterator {{ next() {{ return {{done: false, value: 1}} }} \
                   return() {{ throw new EvalError('r') }} }} \
                 var t = new T().{helper}; t.next(); \
                 try {{ t.return(); 'no error' }} catch (e) {{ e.constructor.name }}"
            )),
            "EvalError",
            "{helper}"
        );
    }
}
