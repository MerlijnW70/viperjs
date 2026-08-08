//! §27.1's `Iterator` and the §27.1.4 methods that consume one.

use super::*;

/// An iterator written by hand, so that its `next` and `return` can be counted.
const WATCHED: &str = "function watched(values) { \
     var o = {nextCalls: 0, closed: false, at: 0}; \
     o.next = function () { o.nextCalls++; \
         return o.at < values.length ? {done: false, value: values[o.at++]} : {done: true}; }; \
     o.return = function () { o.closed = true; return {done: true}; }; \
     Object.setPrototypeOf(o, Iterator.prototype); \
     return o; } ";

#[test]
fn the_consumers_walk_an_iterator_and_each_says_what_running_out_means() {
    assert_eq!(run("[1, 2, 3].values().toArray().join(',')"), "1,2,3");
    assert_eq!(
        run(
            "[1, 2, 3].values().reduce(function (a, b) { return a + b; }) + ',' \
             + [1, 2, 3].values().reduce(function (a, b) { return a + b; }, 10)"
        ),
        "6,16"
    );
    // §27.1.4.9 step 5 — with no initial value the first value becomes one, so an empty iterator
    // has nothing to answer and says so. With one given, it answers that.
    assert_eq!(
        run(
            "try { [].values().reduce(function (a, b) { return a; }); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run("[].values().reduce(function (a, b) { return a; }, 'empty')"),
        "empty"
    );
    // Vacuous truth, the same way §23.1.3's `every` and `some` have it.
    assert_eq!(
        run("[].values().every(function () { return false; }) + ',' \
             + [].values().some(function () { return true; }) + ',' \
             + ([].values().find(function () { return true; }) === undefined) + ',' \
             + [].values().toArray().length"),
        "true,false,true,0"
    );
    // The callback gets the value **and its position**, and nothing else — an iterator is not a
    // collection that could be handed back as a third argument.
    assert_eq!(
        run("var seen = []; \
             ['a', 'b'].values().forEach(function () { seen.push(arguments.length + ':' + \
                 Array.prototype.join.call(arguments, '/')); }); seen.join(',')"),
        "2:a/0,2:b/1"
    );
    // …and `reduce` gets three, because it has an accumulator to carry.
    assert_eq!(
        run(
            "var seen = 0; [1, 2, 3].values().reduce(function () { seen = arguments.length; \
                 return 0; }, 0); seen"
        ),
        "3"
    );
}

#[test]
fn a_consumer_stops_as_soon_as_it_knows_and_tells_the_iterator() {
    // §27.1.4.10 and §27.1.4.6 — `some` stops at the first value it likes and `every` at the first
    // it does not, and both close what they abandoned. Counting `next` is what says they stopped
    // rather than walked to the end and remembered.
    assert_eq!(
        run(&format!(
            "{WATCHED} var it = watched([1, 2, 3]); \
             it.some(function (v) {{ return v === 2; }}) + ',' + it.nextCalls + ',' + it.closed"
        )),
        "true,2,true"
    );
    assert_eq!(
        run(&format!(
            "{WATCHED} var it = watched([1, 2, 3]); \
             it.every(function (v) {{ return v === 1; }}) + ',' + it.nextCalls + ',' + it.closed"
        )),
        "false,2,true"
    );
    assert_eq!(
        run(&format!(
            "{WATCHED} var it = watched([5, 6, 7]); \
             it.find(function (v) {{ return v > 5; }}) + ',' + it.nextCalls + ',' + it.closed"
        )),
        "6,2,true"
    );
    // A walk that runs to the end is not *abandoned*, so it is not closed — the iterator said it
    // was done and there is nothing to tell it.
    assert_eq!(
        run(&format!(
            "{WATCHED} var it = watched([1]); \
             it.forEach(function () {{}}); it.closed + ',' + it.nextCalls"
        )),
        "false,2"
    );
    // A callback that throws carries its own completion out and still closes the iterator.
    assert_eq!(
        run(&format!(
            "{WATCHED} var it = watched([1, 2]); \
             try {{ it.forEach(function () {{ throw new RangeError('no'); }}); }} \
             catch (e) {{ e.constructor.name + ',' + it.closed + ',' + it.nextCalls }}"
        )),
        "RangeError,true,1"
    );
    // §27.1.4's step 4 — a callback that is not callable throws **and closes**, because the method
    // took possession of the iterator when it was called. Plain "throw" would leave it open.
    for method in ["forEach", "some", "every", "find", "reduce"] {
        assert_eq!(
            run(&format!(
                "{WATCHED} var it = watched([1]); \
                 try {{ it.{method}(1); }} catch (e) {{ e.constructor.name + ',' + it.closed }}"
            )),
            "TypeError,true",
            "{method} with a callback that is not callable"
        );
    }
}

#[test]
fn a_consumer_reads_next_and_never_asks_whether_the_thing_is_iterable() {
    // §7.4.10 `GetIteratorDirect` — the receiver *is* the iterator and only its `next` is read. So
    // a plain object with a `next` works, and an object that is iterable but has no `next` of its
    // own does not. That is the opposite of what `for`-`of` does with the same two objects.
    assert_eq!(
        run("Iterator.prototype.toArray.call({next: function () { return {done: true}; }}).length"),
        "0"
    );
    assert_eq!(
        run("var n = 0; \
             Iterator.prototype.toArray.call({next: function () { \
                 return n < 2 ? {done: false, value: n++} : {done: true}; }}).join(',')"),
        "0,1"
    );
    assert_eq!(
        run("try { Iterator.prototype.toArray.call({}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A primitive receiver is refused before anything is read — these are not generic the way
    // §23.1.3's methods are.
    for bad in ["1", "'ab'", "null", "undefined"] {
        assert_eq!(
            run(&format!(
                "try {{ Iterator.prototype.toArray.call({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "toArray on {bad}"
        );
    }
}

#[test]
fn the_iterator_constructor_exists_to_be_extended_and_not_to_be_called() {
    // §27.1.3.1 — both refusals are one rule read twice: it may not be called, and it may not be
    // the thing being constructed.
    assert_eq!(
        run("try { new Iterator(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Iterator(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a subclass constructs fine, because there the `new.target` is the subclass.
    assert_eq!(
        run(
            "class My extends Iterator { next() { return {done: true}; } } \
             var m = new My(); (m instanceof Iterator) + ',' + m.toArray().length"
        ),
        "true,0"
    );
    // `Iterator.prototype` **is** the prototype every built-in iterator already inherited from,
    // which is what makes the helpers reach an Array's iterator without it being changed.
    assert_eq!(
        run("Iterator.prototype === Object.getPrototypeOf(Object.getPrototypeOf([].values()))"),
        "true"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Iterator, 'prototype'); \
             d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,false"
    );
}

#[test]
fn the_two_accessors_write_to_the_receiver_and_refuse_their_own_home() {
    // §27.1.4.1 and §27.1.4.2 are accessor properties rather than data ones, and the setter is the
    // reason: `SetterThatIgnoresPrototypeProperties`.
    for name in ["'constructor'", "Symbol.toStringTag"] {
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor(Iterator.prototype, {name}); \
                 (typeof d.get) + ',' + (typeof d.set) + ',' + d.enumerable + ',' + d.configurable \
                 + ',' + (d.value === undefined) + ',' + (d.writable === undefined)"
            )),
            "function,function,false,true,true,true",
            "descriptor of {name}"
        );
    }
    assert_eq!(
        run("Iterator.prototype[Symbol.toStringTag] + ',' \
             + (Iterator.prototype.constructor === Iterator) + ',' \
             + Object.prototype.toString.call(Iterator.prototype)"),
        "Iterator,true,[object Iterator]"
    );
    // Writing to the home object itself is a **TypeError** — which is what stops one generator
    // giving itself a tag and changing what every other iterator reports.
    for name in ["Symbol.toStringTag", "'constructor'"] {
        assert_eq!(
            run(&format!(
                "try {{ Iterator.prototype[{name}] = 'x'; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "assigning {name} on Iterator.prototype"
        );
    }
    // …while an object that merely *inherits* it gets an own data property, and the home object is
    // left alone. That is the whole point of the odd setter.
    assert_eq!(
        run(
            "var o = Object.create(Iterator.prototype); o[Symbol.toStringTag] = 'mine'; \
             var d = Object.getOwnPropertyDescriptor(o, Symbol.toStringTag); \
             o[Symbol.toStringTag] + ',' + Iterator.prototype[Symbol.toStringTag] \
             + ',' + d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        // All three attributes, because `CreateDataPropertyOrThrow` sets all three — an assignment
        // that produced a property one could not enumerate or delete would still read back the
        // same value, so checking only the value proves nothing about how it was made.
        "mine,Iterator,true,true,true"
    );
    // An own property that is already there is *written through* rather than redefined, so an
    // accessor of the receiver's own runs.
    assert_eq!(
        run("var seen; var o = {set constructor(v) { seen = v; }}; \
             Object.setPrototypeOf(o, Iterator.prototype); o.constructor = 'through'; \
             seen + ',' + (o.constructor === undefined)"),
        "through,true"
    );
}

#[test]
fn a_helper_that_stops_early_reports_what_closing_found_and_one_that_throws_does_not() {
    // §7.4.9 step 4 is the whole distinction, and this asserts the pair rather than either half.
    //
    // `every`, `find` and `some` stop with `IteratorClose(iterated, NormalCompletion(x))`. There is
    // no original completion to keep, so steps 5 and 6 are what the program sees: a `return` that
    // throws is *reported*.
    let source = "var made = function () { var i = 0; return {\
        [Symbol.iterator]: function () { return this },\
        next: function () { return { value: i++, done: false } },\
        return: function () { throw new RangeError('closing') } } };";
    assert_eq!(
        run(&format!(
            "{source} try {{ Iterator.from(made()).every(function () {{ return false }}) }}\
             catch (e) {{ e.name }}"
        )),
        "RangeError"
    );
    assert_eq!(
        run(&format!(
            "{source} try {{ Iterator.from(made()).some(function () {{ return true }}) }}\
             catch (e) {{ e.name }}"
        )),
        "RangeError"
    );
    assert_eq!(
        run(&format!(
            "{source} try {{ Iterator.from(made()).find(function () {{ return true }}) }}\
             catch (e) {{ e.name }}"
        )),
        "RangeError"
    );
    // §7.4.9 step 2 is inside the close as well, so a `return` **getter** that throws is reported
    // by the same steps — which is why the clause says `Completion(...)` around the lookup.
    let getter = "var made = function () { var i = 0; return {\
        [Symbol.iterator]: function () { return this },\
        next: function () { return { value: i++, done: false } },\
        get return() { throw new RangeError('reading') } } };";
    assert_eq!(
        run(&format!(
            "{getter} try {{ Iterator.from(made()).every(function () {{ return false }}) }}\
             catch (e) {{ e.name }}"
        )),
        "RangeError"
    );

    // …and the other side of step 4: when the *callback* throws, the walk is being abandoned for a
    // reason it already has, so the close's own trouble is discarded and the callback's error is
    // what comes out. Same iterator, same `return`, opposite answer.
    assert_eq!(
        run(&format!(
            "{source} try {{ Iterator.from(made()).every(function () {{ throw new TypeError() }}) }}\
             catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
}

#[test]
fn take_reports_what_closing_the_source_found_when_it_has_had_its_fill() {
    // §27.1.4.9 step 8.b.i.1 — `take` that has had its fill closes with a **NormalCompletion**, so
    // a `return` that throws is reported rather than being papered over with `{ done: true }`.
    let source = "var made = function () { return {\
        [Symbol.iterator]: function () { return this },\
        next: function () { return { value: 1, done: false } },\
        return: function () { throw new RangeError('closing') } } };";
    assert_eq!(
        run(&format!(
            "{source} try {{ Iterator.from(made()).take(0).next() }} catch (e) {{ e.name }}"
        )),
        "RangeError"
    );
    // The close happens *before* the next value is drawn, so a source whose `done` getter throws is
    // never asked at all — `take(0)` reads nothing.
    assert_eq!(
        run(
            "var read = 0; var made = { [Symbol.iterator]: function () { return this },\
             next: function () { read++; return { value: 1, done: false } },\
             return: function () { return {} } };\
             Iterator.from(made).take(0).next(); read"
        ),
        "0"
    );
    // And a `take` that has *not* had its fill closes nothing, so the same source walks normally.
    assert_eq!(
        run(
            "var made = { [Symbol.iterator]: function () { return this },\
             next: function () { return { value: 7, done: false } },\
             return: function () { throw new RangeError() } };\
             Iterator.from(made).take(2).next().value"
        ),
        "7"
    );
}
