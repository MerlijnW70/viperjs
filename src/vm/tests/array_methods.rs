//! §23.1.3 and §20.2.3 as a script sees them.

use super::*;

#[test]
fn a_method_is_written_against_a_shape_rather_than_against_an_array() {
    // §23.1.3 never asks whether it was given an Array — it reads a `length` and some indices.
    // That is the specified reading and not a trick, and `Function.prototype.call` is what makes
    // it sayable from a script at all.
    let like = "{0: 'a', 1: 'b', length: 2}";
    assert_eq!(
        run(&format!("Array.prototype.join.call({like}, '-')")),
        "a-b"
    );
    assert_eq!(
        run(&format!("Array.prototype.indexOf.call({like}, 'b')")),
        "1"
    );
    assert_eq!(
        run(&format!(
            "Array.prototype.map.call({like}, function (x) {{ return x + x }}).join(',')"
        )),
        "aa,bb"
    );
    // §7.1.20's clamp is what lets a `length` of anything at all be handed over: negative reads
    // as zero rather than failing, and so does one that is not a number.
    assert_eq!(
        run("Array.prototype.join.call({length: -1, 0: 'a'}, '-')"),
        ""
    );
    assert_eq!(run("Array.prototype.join.call({length: 'x'}, '-')"), "");
}

#[test]
fn join_reads_a_hole_and_a_null_and_an_undefined_all_as_nothing() {
    // §23.1.3.18 step 4.b. Three different things in the array and one answer out, which is why
    // `[1, , 3].join('-')` is `1--3` rather than `1-undefined-3`.
    assert_eq!(run("[1, 2, 3].join('-')"), "1-2-3");
    assert_eq!(run("[1, , 3].join('-')"), "1--3");
    assert_eq!(run("[1, undefined, 3].join('-')"), "1--3");
    assert_eq!(run("[1, null, 3].join('-')"), "1--3");
    // The separator is `","` when absent, and coerced when it is not a string.
    assert_eq!(run("[1, 2].join()"), "1,2");
    assert_eq!(run("[1, 2].join(0)"), "102");
    assert_eq!(run("[].join('-')"), "");
    // An element is coerced with `ToString`, so an object reaches its own `toString`.
    assert_eq!(
        run("[{toString: function () { return 'x' }}, 1].join('-')"),
        "x-1"
    );
}

#[test]
fn to_string_calls_whatever_join_the_object_has() {
    // §23.1.3.31 step 3 reaches for the object's *own* `join`, not the intrinsic one — so
    // replacing `Array.prototype.join` changes what every array prints. That is what makes `join`
    // the single place an array's text is decided.
    assert_eq!(run("[1, 2].toString()"), "1,2");
    assert_eq!(run("'' + [1, 2]"), "1,2");
    assert_eq!(
        run("var a = [1, 2]; a.join = function () { return 'mine' }; '' + a"),
        "mine"
    );
    // Step 4 — an object whose `join` is not callable falls back to
    // `Object.prototype.toString`, which is the only way to see an array's own tag.
    assert_eq!(
        run("var a = [1]; a.join = 1; Array.prototype.toString.call(a)"),
        "[object Array]"
    );
}

#[test]
fn push_and_pop_move_the_length_and_answer_different_things() {
    assert_eq!(
        run("var a = [1]; a.push(2, 3); a.length + '|' + a[2]"),
        "3|3"
    );
    // `push` answers the *new length* and `pop` answers the element, which is the asymmetry
    // people write bugs about.
    assert_eq!(run("[1].push(2)"), "2");
    assert_eq!(run("[1, 2].pop()"), "2");
    assert_eq!(run("var a = [1, 2]; a.pop(); a.length"), "1");
    // An empty array pops `undefined` and stays empty.
    assert_eq!(run("[].pop() + '|' + [].length"), "undefined|0");
    // …and on an array-like, both write a `length` that was not a number before.
    assert_eq!(
        run("var o = {length: '0'}; Array.prototype.push.call(o, 'a'); o.length + '|' + o[0]"),
        "1|a"
    );
    assert_eq!(
        run("var o = {length: '0'}; Array.prototype.pop.call(o); o.length"),
        "0"
    );
}

#[test]
fn index_of_compares_strictly_and_never_matches_a_hole() {
    assert_eq!(run("[1, 2, 3].indexOf(2)"), "1");
    assert_eq!(run("[1, 2, 3].indexOf(9)"), "-1");
    // Strict equality, so no coercion and no NaN — which is the whole reason `includes` exists.
    assert_eq!(run("[1, 2].indexOf('1')"), "-1");
    assert_eq!(run("[NaN].indexOf(NaN)"), "-1");
    assert_eq!(run("[0].indexOf(-0)"), "0");
    // §23.1.3.17 step 9.a skips a hole rather than comparing it, so the two shapes differ.
    assert_eq!(run("[, 1].indexOf(undefined)"), "-1");
    assert_eq!(run("[undefined, 1].indexOf(undefined)"), "0");
    // A negative start counts from the end and is clamped rather than wrapping.
    assert_eq!(run("[1, 2, 1].indexOf(1, 1)"), "2");
    assert_eq!(run("[1, 2, 1].indexOf(1, -2)"), "2");
    assert_eq!(run("[1, 2, 1].indexOf(1, -99)"), "0");
    assert_eq!(run("[1, 2, 1].indexOf(1, 99)"), "-1");
}

#[test]
fn a_callback_is_checked_before_anything_runs_and_is_given_three_arguments() {
    // §23.1.3.15 step 3 — an *empty* array with a callback that is not a function still throws,
    // so the check cannot be left until the first element.
    for method in ["forEach", "map", "filter"] {
        assert_eq!(
            run(&format!("try {{ [].{method}(1) }} catch (e) {{ e.name }}")),
            "TypeError",
            "{method} with no callback"
        );
    }
    // The element, its index, and the object itself — in that order.
    assert_eq!(
        run("[7, 8].map(function (x, i, a) { return x + ':' + i + ':' + a.length }).join(',')"),
        "7:0:2,8:1:2"
    );
    // The second argument is the receiver.
    assert_eq!(
        run("[1].map(function () { return this.x }, {x: 'here'})[0]"),
        "here"
    );
}

#[test]
fn a_hole_is_skipped_by_the_callback_methods_and_kept_by_map() {
    // §23.1.3.15 step 6.b — the callback is not run for a hole, so `length` is not the number of
    // times it is called.
    assert_eq!(
        run("var n = 0; [1, , 3].forEach(function () { n = n + 1 }); n"),
        "2"
    );
    // A hole in maps to a hole *out*: the result has the same length and the index is still
    // absent, which is what makes `map` length-preserving where `filter` is not.
    let mapped = "var a = [1, , 3].map(function (x) { return x }); \
                  a.length + '|' + (1 in a) + '|' + a[2]";
    assert_eq!(run(mapped), "3|false|3");
    // `filter` packs what it keeps, so its indices are consecutive whatever they were.
    assert_eq!(
        run("var a = [1, , 3].filter(function () { return true }); a.length + '|' + a[1]"),
        "2|3"
    );
    assert_eq!(
        run("[1, 2, 3].filter(function (x) { return x > 1 }).join(',')"),
        "2,3"
    );
    // `forEach` answers `undefined` whatever the callback returns.
    assert_eq!(
        run("typeof [1].forEach(function () { return 1 })"),
        "undefined"
    );
}

#[test]
fn slice_keeps_a_hole_where_filter_and_map_would_not() {
    assert_eq!(run("[1, 2, 3].slice(1).join(',')"), "2,3");
    assert_eq!(run("[1, 2, 3, 4].slice(1, 3).join(',')"), "2,3");
    assert_eq!(run("[1, 2, 3].slice(-2).join(',')"), "2,3");
    assert_eq!(run("[1, 2, 3].slice().length"), "3");
    // A start past the end, and an end before the start, both answer an empty array rather than
    // counting backwards.
    assert_eq!(run("[1, 2].slice(5).length"), "0");
    assert_eq!(run("[1, 2, 3].slice(2, 1).length"), "0");
    // §23.1.3.25 step 9.b — a hole stays a hole, and the length is still written.
    let holes = "var a = [1, , 3].slice(0); a.length + '|' + (1 in a)";
    assert_eq!(run(holes), "3|false");
    assert_eq!(run("var a = [1, ,].slice(0); a.length"), "2");
    // The result is a real Array whatever it was sliced from.
    assert_eq!(
        run("Array.isArray(Array.prototype.slice.call({0: 'a', length: 1}))"),
        "true"
    );
}

#[test]
fn call_and_apply_differ_only_in_how_the_arguments_arrive() {
    let f = "function f(a, b) { return this.x + ':' + a + ':' + b } ";
    assert_eq!(run(&format!("{f} f.call({{x: 1}}, 2, 3)")), "1:2:3");
    assert_eq!(run(&format!("{f} f.apply({{x: 1}}, [2, 3])")), "1:2:3");
    // §10.2.1.2 belongs to the *function*, not to the shape of the call: a non-strict one is
    // given the global object whenever the receiver is `undefined` or `null`, so `f()`,
    // `f.call()` and `f.call(null)` all agree.
    let same = "function f() { return this === globalThis } ";
    assert_eq!(
        run(&format!("{same} f() + '|' + f.call() + '|' + f.call(null)")),
        "true|true|true"
    );
    // A *built-in* is the one that keeps the `undefined`, because §10.3.1 substitutes nothing —
    // which is the difference the two call paths exist to keep.
    assert_eq!(
        run("var t = Error.prototype.toString; try { t.call(undefined) } catch (e) { e.name }"),
        "TypeError"
    );
    // §20.2.3.1 step 3 — `null` and `undefined` mean *no* arguments rather than one.
    assert_eq!(
        run("function f(a) { return typeof a } f.apply(null) + '|' + f.apply(null, null)"),
        "undefined|undefined"
    );
    // A hole in the argument list is an ordinary `Get`, so it arrives as `undefined` — unlike in
    // most of §23.1.3, where it is skipped.
    assert_eq!(
        run("function f(a, b) { return typeof a + ':' + b } f.apply(null, [, 1])"),
        "undefined:1"
    );
    // Anything that is not an object is refused, because there is nothing to read a length from.
    assert_eq!(
        run("function f() {} try { f.apply(null, 1) } catch (e) { e.name }"),
        "TypeError"
    );
    // …and so is a receiver that is not callable. `Function` is not a global yet, so the method
    // is reached through a function that has it rather than by name.
    assert_eq!(
        run("var f = function () {}; try { f.call.call(1) } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn object_prototype_to_string_can_tell_an_array_from_an_object_that_borrowed_its_prototype() {
    // §20.1.3.6 step 4's `IsArray` is a question about the object, not about its chain — the same
    // difference `Array.isArray` exists for, showing up in the other place it can be seen.
    assert_eq!(run("Object.prototype.toString.call([])"), "[object Array]");
    assert_eq!(run("Object.prototype.toString.call({})"), "[object Object]");
    assert_eq!(
        run("Object.prototype.toString.call(Object.create(Array.prototype))"),
        "[object Object]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(function () {})"),
        "[object Function]"
    );
    assert_eq!(run("Object.prototype.toString.call(null)"), "[object Null]");
}

#[test]
fn what_a_method_builds_is_made_of_ordinary_properties() {
    // §7.3.5 `CreateDataPropertyOrThrow` is §6.1.7.1's three defaults, so an element a method
    // made is indistinguishable from one written by assignment. Nothing in the language can see
    // that except `getOwnPropertyDescriptor`, which is why it is asked here.
    let of = |source: &str| {
        format!(
            "var d = Object.getOwnPropertyDescriptor({source}, '0'); \
                 d.writable + '|' + d.enumerable + '|' + d.configurable"
        )
    };
    assert_eq!(
        run(&of("[1].map(function (x) { return x })")),
        "true|true|true"
    );
    assert_eq!(
        run(&of("[1].filter(function () { return true })")),
        "true|true|true"
    );
    assert_eq!(run(&of("[1, 2].slice(0)")), "true|true|true");
    // …and the `length` they leave behind is the array's own exotic one, not a plain property.
    let length = "var d = Object.getOwnPropertyDescriptor([1].slice(0), 'length'); \
                  d.writable + '|' + d.enumerable + '|' + d.configurable";
    assert_eq!(run(length), "true|false|false");
}

#[test]
fn a_length_that_is_not_a_number_reads_as_none_at_all() {
    // §7.1.20 steps 2 and 3 — NaN and every negative are zero, and nothing throws. That is what
    // lets these methods be handed any object at all without asking first, and it is the same
    // clamp `apply` uses to decide how many arguments there are.
    for length in ["undefined", "NaN", "-1", "-0", "'x'", "null", "{}"] {
        let source = format!("Array.prototype.join.call({{length: {length}, 0: 'a'}}, '-')");
        assert_eq!(run(&source), "", "a length of {length}");
    }
    // …and a fractional one is truncated rather than rounded up.
    assert_eq!(
        run("Array.prototype.join.call({length: 1.9, 0: 'a', 1: 'b'}, '-')"),
        "a"
    );
    assert_eq!(
        run(
            "function f(a, b) { return typeof a + ':' + typeof b } f.apply(null, {length: 1.9, 0: 1, 1: 2})"
        ),
        "number:undefined"
    );
    assert_eq!(
        run("function f(a) { return typeof a } f.apply(null, {length: -1, 0: 1})"),
        "undefined"
    );
}

#[test]
fn a_primitive_receiver_is_wrapped_rather_than_refused() {
    // Every method in §23.1.3 opens `Let O be ? ToObject(this value)`, and §7.1.18 *wraps* a
    // primitive rather than rejecting it. So a String is an array-like — §10.4.3 gives its object
    // an own property per index and a `length` — and the generic methods read it as one.
    assert_eq!(run("Array.prototype.join.call('abc')"), "a,b,c");
    assert_eq!(run("Array.prototype.slice.call('abc').join('|')"), "a|b|c");
    assert_eq!(
        run(
            "Array.prototype.indexOf.call('abc', 'b') + ',' + Array.prototype.indexOf.call('abc', 'z')"
        ),
        "1,-1"
    );
    assert_eq!(
        run("Array.prototype.map.call('ab', function (c) { return c + c; }).join(',')"),
        "aa,bb"
    );
    // A Boolean, a Number and a Symbol wrap too, and their wrappers have no indices and no
    // `length` — so the methods see an empty array-like rather than refusing the call.
    assert_eq!(
        run(
            "[Array.prototype.join.call(true), Array.prototype.join.call(5),              Array.prototype.join.call(Symbol('s'))].join('|')"
        ),
        "||"
    );
    assert_eq!(run("Array.prototype.toSorted.call(true).length"), "0");
    // …and what the wrapper *inherits* is what it reads, which is the whole of "an array-like is
    // whatever has a length and some indices". Nothing here is an Array or pretending to be one.
    assert_eq!(
        run(
            "Boolean.prototype.length = 2; Boolean.prototype[0] = 'x';              var joined = Array.prototype.join.call(true);              delete Boolean.prototype.length; delete Boolean.prototype[0]; joined"
        ),
        "x,"
    );
    // A method that *changes* its receiver writes through to the wrapper, which is thrown away the
    // moment the call ends — so this is a call that does nothing and is not an error, exactly as
    // `[].push.call(5, 1)` has always been.
    assert_eq!(run("typeof Array.prototype.sort.call(true)"), "object");
    // §7.1.18 steps 1 and 2 — the two values that genuinely have no object, and they are still a
    // TypeError. That is the line between "wraps rather than refuses" and "never refuses".
    for receiver in ["null", "undefined"] {
        assert_eq!(
            run(&format!(
                "try {{ Array.prototype.join.call({receiver}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{receiver} has no object to be"
        );
    }
    assert_eq!(
        run("try { Array.prototype.sort.call(null); } catch (e) { e.message }"),
        "undefined and null cannot be converted to an object"
    );
}

#[test]
fn array_species_create_ignores_the_constructor_of_something_that_is_not_an_array() {
    // §7.3.23 step 3 — `IsArray(originalArray)` is the *first* question, and a false answer means
    // a plain Array is made without `constructor` being read at all. So an array-like borrowing an
    // Array method cannot redirect where the result goes, however its constructor is written.
    assert_eq!(
        run(
            "var o = {length: 1, 0: 7}; o.constructor = function () {}; \
             o.constructor[Symbol.species] = function () { return {tagged: 1}; }; \
             var r = Array.prototype.map.call(o, function (x) { return x; }); \
             Array.isArray(r) + ',' + r[0]"
        ),
        "true,7"
    );
    // …and a real array with the same constructor does redirect, which is what says the check is
    // about `IsArray` and not about the species being unreadable.
    assert_eq!(
        run("var a = [7]; a.constructor = function () {}; \
             a.constructor[Symbol.species] = function () { return {tagged: 1}; }; \
             var r = Array.prototype.map.call(a, function (x) { return x; }); \
             Array.isArray(r) + ',' + r.tagged"),
        "false,1"
    );
}

#[test]
fn the_unscopables_list_is_a_fixed_set_of_names_and_not_the_methods_added_since_es5() {
    // §23.1.3.35. The order is the clause's, and it is observable — `OrdinaryOwnPropertyKeys` lists
    // string keys in insertion order, none of these being an array index.
    assert_eq!(
        run("Object.keys(Array.prototype[Symbol.unscopables]).join(',')"),
        "at,copyWithin,entries,fill,find,findIndex,findLast,findLastIndex,flat,flatMap,\
         includes,keys,toReversed,toSorted,toSpliced,values"
    );
    // Step 1's `OrdinaryObjectCreate(null)`. With `Object.prototype` under it a `with` over an
    // array would find `toString` and `valueOf` in here and read them as blocked names.
    assert_eq!(
        run("Object.getPrototypeOf(Array.prototype[Symbol.unscopables])"),
        "null"
    );
    // **`with` is not in the list**, though it is a change-array-by-copy method exactly as
    // `toReversed` and `toSorted` are. Membership is a decision the committee took per method and
    // not a rule about vintage: `with` is a reserved word, so no code ever named a binding that and
    // there is nothing for it to shadow. Read as "the methods newer than ES5" this row is wrong.
    assert_eq!(
        run("Object.prototype.hasOwnProperty.call(Array.prototype[Symbol.unscopables], 'with')"),
        "false"
    );
    // …and neither is anything ES5 already had, however array-ish it looks.
    assert_eq!(
        run(
            "['join', 'slice', 'indexOf', 'forEach', 'map', 'length'].some(function (n) { \
             return n in Array.prototype[Symbol.unscopables] })"
        ),
        "false"
    );
}

#[test]
fn the_unscopables_property_and_its_entries_carry_different_attributes() {
    // The property is not writable, not enumerable and **configurable**; each entry is
    // `CreateDataPropertyOrThrow`, so all three are true. One helper for both gets exactly one of
    // them wrong, and `propertyHelper.js` checks both — which is why they are asserted apart.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Array.prototype, Symbol.unscopables); \
             [d.writable, d.enumerable, d.configurable].join()"
        ),
        "false,false,true"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Array.prototype[Symbol.unscopables], 'keys'); \
             [d.value, d.writable, d.enumerable, d.configurable].join()"
        ),
        "true,true,true,true"
    );
}

#[test]
fn array_of_and_from_collect_into_their_this_when_it_is_a_constructor() {
    // §23.1.2.3 step 4 and §23.1.2.1 step 5 — `this` is the constructor when it is one, which is
    // the whole reason these two are `Array`'s *static* methods rather than free functions. A
    // subclass inherits both and gets itself.
    assert_eq!(
        run("class C extends Array {} var made = C.of(1, 2); made instanceof C"),
        "true"
    );
    assert_eq!(
        run("class C extends Array {} C.from([7, 8]).join(',')"),
        "7,8"
    );
    // It need not be an Array at all: the clause says `Construct(C, «len»)` and then writes the
    // indices, so anything constructible collects.
    assert_eq!(
        run(
            "function C(n) { this.told = n; } var made = Array.of.call(C, 'a', 'b');\
             made.told + ':' + made[0] + made[1] + ':' + made.length"
        ),
        "2:ab:2"
    );

    // …and a `this` that is **not** a constructor falls back to `ArrayCreate`, which is every
    // ordinary call: `Array.of(1)` is a plain array and nothing about the common case changes.
    assert_eq!(
        run("var a = Array.of(1, 2); Array.isArray(a) + ':' + a.length"),
        "true:2"
    );
    assert_eq!(
        run("Array.from([1, 2, 3], function (x) { return x * 2 }).join(',')"),
        "2,4,6"
    );
    assert_eq!(run("var of = Array.of; Array.isArray(of(9))"), "true");

    // Step 7.c is `CreateDataPropertyOrThrow`: a target that refuses the index stops the whole
    // thing rather than answering something half-filled.
    assert_eq!(
        run("function C() { Object.preventExtensions(this); }\
             try { Array.of.call(C, 'x') } catch (e) { e.name }"),
        "TypeError"
    );
    // …and step 8's `Set(A, 'length', len, true)` is a throwing write, so a setter that refuses is
    // reported rather than discarded.
    assert_eq!(
        run(
            "function C() { Object.defineProperty(this, 'length', { set: function () { throw new RangeError() } }); }\
             try { Array.of.call(C, 'x') } catch (e) { e.name }"
        ),
        "RangeError"
    );
}

#[test]
fn array_from_closes_the_iterator_when_a_step_of_its_own_throws() {
    // §23.1.2.1 steps 6.e.vii and 6.e.ix both close the iterator on an abrupt completion, which is
    // only possible because the write happens **inside** the walk. Draining to `done` first and
    // writing afterwards leaves nothing to close — the iterator is already spent — and that is the
    // shape this asserts against.
    let closes = "var closed = 0;\
        var iterable = { [Symbol.iterator]: function () { return {\
            next: function () { return { value: 1, done: false } },\
            return: function () { closed++; return {} } } } };";
    // A mapper that throws.
    assert_eq!(
        run(&format!(
            "{closes} try {{ Array.from(iterable, function () {{ throw new RangeError() }}) }}\
             catch (e) {{}} closed"
        )),
        "1"
    );
    // …and a target that refuses the index, which is the other of the two.
    assert_eq!(
        run(&format!(
            "{closes} function C() {{ Object.preventExtensions(this); }}\
             try {{ Array.from.call(C, iterable) }} catch (e) {{}} closed"
        )),
        "1"
    );
}
