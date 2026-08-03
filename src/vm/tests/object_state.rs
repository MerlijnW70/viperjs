//! §20.1.2 — sealing, freezing, copying and listing, and the coercion every one of them starts with.
//!
//! Checked against V8 first. The rows worth reading twice are the ones where a *primitive* is not
//! refused: nearly every static here begins with `ToObject`, so `Object.keys("ab")` is a list and
//! not an error, and only `undefined` and `null` have nothing to become.

use super::*;

#[test]
fn freezing_and_sealing_are_two_depths_of_the_same_operation() {
    // Sealing stops the shape changing; freezing stops the values changing as well. In sloppy code
    // both refusals are silent, which is why each is asked by reading the value back.
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.seal(o); o.a = 2; return o.a; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.freeze(o); o.a = 2; return o.a; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.seal(o); return delete o.a; })()"),
        "false"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.freeze(o); o.b = 3; return typeof o.b; })()"),
        "undefined"
    );
    // §7.3.15 asks the properties rather than remembering a promise, so freezing implies sealing
    // and sealing does not imply freezing.
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.freeze(o); return Object.isSealed(o); })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; Object.seal(o); return Object.isFrozen(o); })()"),
        "false"
    );
    // …which is also why an object with no properties at all is frozen as soon as it is
    // non-extensible: there is nothing left that could disagree.
    assert_eq!(run("Object.isFrozen({})"), "false");
    assert_eq!(run("Object.isSealed({})"), "false");
    assert_eq!(run("Object.isFrozen(Object.preventExtensions({}))"), "true");
    assert_eq!(
        run("Object.isFrozen(Object.preventExtensions({a: 1}))"),
        "false"
    );
    // …and a *configurable* property is not sealed either, which is the half of §7.3.15 the frozen
    // rows cannot ask about: they fail on writability before configurability is ever reached.
    assert_eq!(
        run("Object.isSealed(Object.preventExtensions({a: 1}))"),
        "false"
    );
    // §7.3.14 step 3.b.ii — an accessor has no `[[Writable]]` to take away, so freezing takes only
    // its configurability and leaves its getter where it was.
    assert_eq!(
        run("(function () { var o = {}; \
             Object.defineProperty(o, 'a', {get: function () { return 1; }, configurable: true}); \
             Object.freeze(o); var d = Object.getOwnPropertyDescriptor(o, 'a'); \
             return d.configurable + ',' + (typeof d.get); })()"),
        "false,function"
    );
    // A primitive has no properties to shut, so `freeze` hands it back and `isFrozen` says yes.
    // The asymmetry is only apparent: both answer "there is nothing here to do".
    assert_eq!(run("Object.freeze(5)"), "5");
    assert_eq!(run("Object.isFrozen(1)"), "true");
    assert_eq!(run("Object.isSealed('a')"), "true");
}

#[test]
fn an_array_method_throws_where_an_assignment_would_be_silent() {
    // §23.1.3 spells every write `Set(O, key, value, true)`, and that `true` is the whole
    // difference. An assignment to a frozen array is refused quietly; a method that would have to
    // make the same write says so instead — and praxis discarded that answer until this row.
    assert_eq!(
        run(
            "(function () { var a = [1, 2]; Object.freeze(a); try { a.push(3); return 'no throw'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { var a = [1, 2]; Object.seal(a); try { a.push(3); return 'no throw'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { var a = [1, 2]; Object.freeze(a); try { a.pop(); return 'no throw'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // …while the assignment itself stays silent, because nothing asked it to throw.
    assert_eq!(
        run("(function () { var o = {}; Object.freeze(o); o.a = 1; return 'no throw'; })()"),
        "no throw"
    );
    assert_eq!(
        run("(function () { var a = [1]; Object.freeze(a); return a.length; })()"),
        "1"
    );
}

#[test]
fn same_value_is_neither_of_the_two_equalities() {
    // §7.2.10 differs from `===` in exactly two places, and they are the reason `Object.is` exists.
    assert_eq!(run("Object.is(NaN, NaN)"), "true");
    assert_eq!(run("Object.is(0, -0)"), "false");
    assert_eq!(run("0 === -0"), "true");
    assert_eq!(run("Object.is(1, 1)"), "true");
    assert_eq!(run("Object.is('a', 'a')"), "true");
}

#[test]
fn assigning_copies_values_and_never_the_accessors_that_produced_them() {
    assert_eq!(run("Object.assign({a: 1}, {b: 2}).b"), "2");
    // Step 4.a — an `undefined` or `null` source is skipped, not refused, which is what makes
    // `Object.assign({}, maybe)` a usable idiom.
    assert_eq!(run("Object.assign({}, null, undefined, {a: 1}).a"), "1");
    assert_eq!(
        run("Object.keys(Object.assign({}, {a: 1, b: 2})).join(',')"),
        "a,b"
    );
    // A getter on the source is **run**, and what it answered is what lands. So the target holds a
    // value where the source held an accessor — this is a `[[Get]]` and a `[[Set]]`, not a
    // descriptor copy, and no other reading of §20.1.2.1 produces this.
    assert_eq!(
        run(
            "(function () { var t = {}; Object.assign(t, {get a() { return 7; }}); \
             var d = Object.getOwnPropertyDescriptor(t, 'a'); return typeof d.get; })()"
        ),
        "undefined"
    );
    assert_eq!(
        run("(function () { try { return Object.assign(null, {}); } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // Step 4.c.ii.1 — `Set(to, key, value, true)`, so a read-only property on the *target* stops
    // the copy with a TypeError. The same assignment written out would be silent, which is the
    // whole point of the `true` and the thing this discarded when it was first written.
    assert_eq!(
        run(
            "(function () { var t = {};              Object.defineProperty(t, 'a', {value: 1, writable: false});              try { Object.assign(t, {a: 2}); return 'ok'; }              catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { var t = Object.freeze({a: 1});              try { Object.assign(t, {a: 2}); return 'ok'; }              catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // Step 4.c.i — own *enumerable* keys, so a hidden property stays hidden. A copy that took
    // every own key would be a different operation wearing the same name.
    assert_eq!(
        run(
            "(function () { var s = {};              Object.defineProperty(s, 'a', {value: 1, enumerable: false});              return typeof Object.assign({}, s).a; })()"
        ),
        "undefined"
    );
    assert_eq!(
        run(
            "(function () { var s = {};              Object.defineProperty(s, 'a', {value: 1, enumerable: true});              return Object.assign({}, s).a; })()"
        ),
        "1"
    );
}

#[test]
fn an_object_can_be_listed_as_keys_values_or_pairs() {
    assert_eq!(run("Object.values({a: 1, b: 2}).join(',')"), "1,2");
    assert_eq!(run("Object.entries({a: 1}).length"), "1");
    assert_eq!(run("Object.entries({a: 1})[0].join(':')"), "a:1");
    assert_eq!(
        run("Object.entries({a: 1, b: 2}).map(function (e) { return e[0] + e[1]; }).join('-')"),
        "a1-b2"
    );
    assert_eq!(run("Object.fromEntries([['a', 1], ['b', 2]]).b"), "2");
    // §20.1.2.9 lists *every* own key and not only the enumerable ones, which is what makes it and
    // `defineProperties` a pair that round-trips an object exactly.
    assert_eq!(
        run("Object.keys(Object.getOwnPropertyDescriptors({a: 1})).join(',')"),
        "a"
    );
    assert_eq!(run("Object.getOwnPropertyDescriptors({a: 1}).a.value"), "1");
    // Both build an ordinary object, so what they put on it is ordinary too — writable,
    // enumerable and configurable, like any property an assignment would have made.
    for built in [
        "Object.fromEntries([['a', 1]])",
        "Object.getOwnPropertyDescriptors({a: 1})",
    ] {
        for attribute in ["writable", "enumerable", "configurable"] {
            assert_eq!(
                run(&format!(
                    "Object.getOwnPropertyDescriptor({built}, 'a').{attribute}"
                )),
                "true"
            );
        }
    }
    // §20.1.2.9 takes every own key and not only the enumerable ones — the difference from
    // `Object.entries` above, and the reason this round-trips through `defineProperties`.
    assert_eq!(
        run("(function () { var o = {}; \
             Object.defineProperty(o, 'h', {value: 1, enumerable: false}); \
             return Object.keys(Object.getOwnPropertyDescriptors(o)).join(','); })()"),
        "h"
    );
}

#[test]
fn a_primitive_is_read_through_the_object_it_stands_for() {
    // §7.1.18 `ToObject`, which is not the same question as "is this an object". These were all a
    // TypeError until the statics stopped refusing what the specification tells them to convert.
    assert_eq!(run("Object.keys('ab').join(',')"), "0,1");
    assert_eq!(run("Object.keys(1).length"), "0");
    assert_eq!(
        run("Object.getOwnPropertyNames('ab').join(',')"),
        "0,1,length"
    );
    assert_eq!(run("Object.entries('ab').length"), "2");
    assert_eq!(run("Object.values('ab').join(',')"), "a,b");
    assert_eq!(run("Object.getOwnPropertyDescriptor('ab', '0').value"), "a");
    assert_eq!(run("Object.getOwnPropertyDescriptors('a')[0].value"), "a");
    // The two that really have no object, and are the only TypeErrors left.
    for asked in ["Object.keys(null)", "Object.values(undefined)"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return {asked}; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
}

#[test]
fn a_prototype_may_be_replaced_unless_that_would_close_a_loop() {
    assert_eq!(
        run(
            "(function () { var p = {x: 5}; var o = Object.setPrototypeOf({}, p); return o.x; })()"
        ),
        "5"
    );
    assert_eq!(
        run(
            "(function () { var o = {}; Object.setPrototypeOf(o, null); \
             return Object.getPrototypeOf(o); })()"
        ),
        "null"
    );
    // §10.1.2's two refusals, which are what every prototype walk in the engine relies on: a
    // non-extensible object's chain is fixed, and a chain may not come back to where it started.
    assert_eq!(
        run("(function () { var o = Object.preventExtensions({}); \
             try { Object.setPrototypeOf(o, {a: 1}); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { var o = {}; try { Object.setPrototypeOf(o, o); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // A primitive target is handed back untouched — it has no prototype slot to be disappointed
    // about — while `undefined` and `null` are refused before anything is looked at.
    assert_eq!(run("Object.setPrototypeOf(5, null)"), "5");
    assert_eq!(
        run("(function () { try { Object.setPrototypeOf(null, null); } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { try { Object.setPrototypeOf({}, 5); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}

#[test]
fn values_and_entries_leave_a_symbol_keyed_property_out() {
    // §7.3.24 step 4 — `EnumerableOwnProperties` lists String keys only, so a Symbol-keyed
    // property is absent from the values as well as from the names. Filtered before the `[[Get]]`,
    // because running a getter for something the answer will not hold would be observable.
    assert_eq!(
        run("var o = {a: 1}; o[Symbol('s')] = 2; Object.values(o).join()"),
        "1"
    );
    assert_eq!(
        run("var o = {a: 1}; o[Symbol('s')] = 2; Object.entries(o).length"),
        "1"
    );
    assert_eq!(
        run("var ran = false; var o = {}; \
             Object.defineProperty(o, Symbol('s'), \
             {get: function () { ran = true; }, enumerable: true}); \
             Object.values(o).length + ',' + ran"),
        "0,false"
    );
}

#[test]
fn from_entries_walks_an_iterable_rather_than_reading_a_length() {
    // §20.1.2.7 step 4 is §7.1.5.1's `AddEntriesFromIterable`, which uses the **iterator**
    // protocol. Reading a `length` and the indices under it looks like a narrower input and is
    // not: it accepts an Array and answers `{}` for a `Map`, which is a wrong value rather than a
    // refusal — and a `Map` is the thing this function is most often pointed at.
    assert_eq!(
        run("JSON.stringify(Object.fromEntries(new Map([['a', 1], ['b', 2]])))"),
        "{\"a\":1,\"b\":2}"
    );
    assert_eq!(
        run("JSON.stringify(Object.fromEntries(new Set([['s', 3]])))"),
        "{\"s\":3}"
    );
    assert_eq!(
        run("JSON.stringify(Object.fromEntries(function* () { yield ['x', 9]; }()))"),
        "{\"x\":9}"
    );
    // …and an Array still works, because an Array is iterable. That is the row that says the
    // change is a correction rather than a swap.
    assert_eq!(
        run("JSON.stringify(Object.fromEntries([['a', 1]]))"),
        "{\"a\":1}"
    );
    // A plain array-like is **not** iterable and is now refused, which is the one input that got
    // narrower. §7.4.2 step 3 is `GetMethod`, so an absent `[@@iterator]` means "not iterable"
    // rather than "walk the object itself".
    // By **message**: without §7.4.2's check the walk goes on to call whatever `[@@iterator]`
    // held, and that throws a TypeError of its own — so the name is the same either way.
    assert_eq!(
        run(
            "try { Object.fromEntries({length: 1, 0: ['a', 1]}); 'no throw' } \
             catch (e) { e.message }"
        ),
        "this value is not iterable"
    );
    // …and a non-callable `[@@iterator]` is the *same* refusal rather than a different one:
    // `GetMethod` separates the two cases and §7.4.2 step 3 throws for both, so one test covers
    // them and asking about nullishness first would be a branch nothing could distinguish.
    assert_eq!(
        run("try { Object.fromEntries({[Symbol.iterator]: 1}); 'x' } catch (e) { e.message }"),
        "this value is not iterable"
    );
    // Step 1's `RequireObjectCoercible` and not `ToObject`: a String is coercible *and* iterable,
    // so it reaches the walk and is refused one character at a time — each is a primitive rather
    // than a pair, which is a different refusal from `undefined`'s.
    assert_eq!(run("JSON.stringify(Object.fromEntries(''))"), "{}");
    assert_eq!(
        run("try { Object.fromEntries('ab'); 'no throw' } catch (e) { e.name }"),
        "TypeError"
    );
    // Step 1 again, by message: without it the `[@@iterator]` read is what fails, and it fails
    // with a TypeError about reading a property — the same name for a different reason.
    for absent in ["undefined", "null"] {
        assert_eq!(
            run(&format!(
                "try {{ Object.fromEntries({absent}); 'no throw' }} catch (e) {{ e.message }}"
            )),
            "undefined and null cannot be converted to an object"
        );
    }
}

#[test]
fn from_entries_closes_the_iterator_it_abandons() {
    // §7.1.5.1 steps 3.c to 3.f — every way one entry can go wrong is an `IfAbruptCloseIterator`,
    // because the walk asked the iterator to start and is now leaving before it is done.
    let watched = "var closed = 0; \
                   function source(entries) { \
                     var at = 0; \
                     return {[Symbol.iterator]() { return { \
                       next() { return at < entries.length \
                         ? {value: entries[at++], done: false} : {value: undefined, done: true} }, \
                       return() { closed = 1; return {}; } }; }}; \
                   } ";
    // An entry that is not an object — step 3.c.
    assert_eq!(
        run(&format!(
            "{watched} try {{ Object.fromEntries(source([1])) }} catch (e) {{}} closed"
        )),
        "1"
    );
    // A `0` or a `1` whose getter throws — steps 3.d and 3.e.
    for index in ["0", "1"] {
        assert_eq!(
            run(&format!(
                "{watched} var bad = {{get {index}() {{ throw new RangeError('x') }}}}; \
                 try {{ Object.fromEntries(source([bad])) }} catch (e) {{}} closed"
            )),
            "1"
        );
    }
    // …and a walk that finishes normally does **not** call `return`, because there is nothing to
    // abandon. Without this row the three above would pass for an implementation that closed
    // unconditionally.
    assert_eq!(
        run(&format!(
            "{watched} Object.fromEntries(source([['a', 1]])); closed"
        )),
        "0"
    );
    // The key is converted **after** the value is read, which a throwing `1` makes observable.
    assert_eq!(
        run("var asked = 0; \
             var entry = {0: {toString() { asked = 1; return 'k' }}, \
                          get 1() { throw new RangeError('x') }}; \
             try { Object.fromEntries([entry]) } catch (e) {} asked"),
        "0"
    );
}
