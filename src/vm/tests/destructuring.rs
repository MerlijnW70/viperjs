//! §14.3.3 — taking an object apart in a declaration.
//!
//! Checked against V8 first. Two things are worth reading twice: a default is compared against
//! `undefined` and not against absence, and it is *evaluated* only when it is needed — so a
//! property that is present and `undefined` takes the default, and one that is present and
//! anything else never runs it.
//!
//! An array pattern is not a shorter object one: it drives an *iterator*, so the source need not
//! be an Array and need not have a `length`. What that buys, and what it costs in closing, is the
//! second half of this file.

use super::*;

#[test]
fn a_pattern_reads_the_properties_it_names() {
    assert_eq!(run("(function () { var {a} = {a: 1}; return a; })()"), "1");
    assert_eq!(
        run("(function () { var {a, b} = {a: 1, b: 2}; return a + b; })()"),
        "3"
    );
    // `{a: x}` reads `a` and binds `x` — the key and the name are two things, and shorthand is
    // the case where they happen to be spelled the same.
    assert_eq!(
        run("(function () { var {a: x} = {a: 5}; return x; })()"),
        "5"
    );
    assert_eq!(run("(function () { let {a} = {a: 3}; return a; })()"), "3");
    assert_eq!(
        run("(function () { const {a} = {a: 4}; return a; })()"),
        "4"
    );
    // A property that is not there binds `undefined` rather than failing.
    assert_eq!(
        run("(function () { var {a} = {}; return typeof a; })()"),
        "undefined"
    );
    // Every kind of key a property may have, since the pattern reads one the same way an ordinary
    // member expression would.
    assert_eq!(
        run("(function () { var {'a b': v} = {'a b': 6}; return v; })()"),
        "6"
    );
    assert_eq!(
        run("(function () { var {0: v} = ['q']; return v; })()"),
        "q"
    );
    assert_eq!(
        run("(function () { var k = 'z'; var {[k]: v} = {z: 8}; return v; })()"),
        "8"
    );
    // A primitive source is *coercible*, so it is read through the object it stands for.
    assert_eq!(
        run("(function () { var {length: n} = 'ab'; return n; })()"),
        "2"
    );
    // The same key twice binds twice, which is legal for a `var` and reads oddly and is right.
    assert_eq!(
        run("(function () { var {a, a: b} = {a: 1}; return a + ',' + b; })()"),
        "1,1"
    );
    // The names are ordinary bindings afterwards — a copy, not a window onto the source.
    assert_eq!(
        run("(function () { var o = {a: 1}; var {a} = o; a = 2; return o.a; })()"),
        "1"
    );
    assert_eq!(run("var {a} = {a: 1}; a"), "1");
    assert_eq!(
        run(
            "(function () { try { const {a} = {a: 1}; a = 2; return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}

#[test]
fn a_default_is_for_undefined_and_is_run_only_when_it_is_wanted() {
    assert_eq!(run("(function () { var {a = 7} = {}; return a; })()"), "7");
    assert_eq!(
        run("(function () { var {a = 7} = {a: 1}; return a; })()"),
        "1"
    );
    // §14.3.3 compares against `undefined`, not against absence — so a property that is *there*
    // and `undefined` takes the default, and `null` does not. The pair is the whole rule.
    assert_eq!(
        run("(function () { var {a = 7} = {a: undefined}; return a; })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var {a = 7} = {a: null}; return a; })()"),
        "null"
    );
    // …and it is *evaluated* only when it is needed, which is observable through a side effect.
    assert_eq!(
        run("(function () { var n = 0; var {a = (n++, 7)} = {a: 1}; return n; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var n = 0; var {a = (n++, 7)} = {}; return n + ',' + a; })()"),
        "1,7"
    );
}

#[test]
fn a_pattern_nests_because_what_it_reads_may_be_taken_apart_too() {
    assert_eq!(
        run("(function () { var {a: {b}} = {a: {b: 9}}; return b; })()"),
        "9"
    );
    // A default on the way down, so the inner pattern has something to read — the idiom for
    // "this whole group is optional".
    assert_eq!(
        run("(function () { var {a: {b = 2} = {}} = {}; return b; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { var {a: {b: {c}}} = {a: {b: {c: 'deep'}}}; return c; })()"),
        "deep"
    );
    assert_eq!(
        run("(function () { function f() { var {a} = {a: 1}; return a; } return f(); })()"),
        "1"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (var i = 0; i < 2; i++) { var {a} = {a: i}; r += a; } \
             return r; })()"
        ),
        "01"
    );
}

#[test]
fn undefined_and_null_are_refused_before_anything_is_read() {
    // §14.3.3.7 step 1 is `RequireObjectCoercible`, and the empty pattern is what makes it a step
    // of its own: with a property in it the first read would throw anyway, and with none there is
    // nothing to read and it throws all the same.
    for source in ["null", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var {{}} = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var {{a}} = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // Everything else is coercible, including a primitive with no properties worth reading.
    assert_eq!(run("(function () { var {} = 5; return 'ok'; })()"), "ok");
    assert_eq!(run("(function () { var {} = 'a'; return 'ok'; })()"), "ok");
    assert_eq!(run("(function () { var {} = true; return 'ok'; })()"), "ok");
}

#[test]
fn an_array_pattern_drives_an_iterator_rather_than_reading_indices() {
    assert_eq!(
        run("(function () { var [a, b] = [1, 2]; return a + ',' + b; })()"),
        "1,2"
    );
    assert_eq!(
        run("(function () { var [a] = [1, 2, 3]; return a; })()"),
        "1"
    );
    // An iterator that runs out leaves the rest `undefined` rather than failing.
    assert_eq!(
        run("(function () { var [a, b, c] = [1]; return a + ',' + typeof b + ',' + typeof c; })()"),
        "1,undefined,undefined"
    );
    // An elision takes a turn and binds nothing — not the same as a name that gets `undefined`.
    assert_eq!(
        run("(function () { var [, b] = [1, 2]; return b; })()"),
        "2"
    );
    assert_eq!(run("(function () { var [a = 9] = []; return a; })()"), "9");
    assert_eq!(
        run("(function () { var [a = 9] = [undefined]; return a; })()"),
        "9"
    );
    assert_eq!(
        run("(function () { var [a = 9] = [null]; return a; })()"),
        "null"
    );
    assert_eq!(run("(function () { let [a] = [3]; return a; })()"), "3");
    assert_eq!(run("(function () { const [a] = [4]; return a; })()"), "4");
    assert_eq!(run("var [a] = [1]; a"), "1");
    // Any iterable, which is the whole difference from reading `0`, `1`, `2`: a String iterates
    // by code point and has no elements at all.
    assert_eq!(
        run("(function () { var [a, b] = 'xy'; return a + b; })()"),
        "xy"
    );
    // A rest element collects what is left, as an ordinary Array.
    assert_eq!(
        run("(function () { var [a, ...r] = [1, 2, 3]; return a + ':' + r.join(','); })()"),
        "1:2,3"
    );
    assert_eq!(
        run("(function () { var [...r] = [1, 2]; return Array.isArray(r) + ',' + r.length; })()"),
        "true,2"
    );
    assert_eq!(
        run("(function () { var [a, ...r] = [1]; return r.length; })()"),
        "0"
    );
    // …and patterns nest through each other in both directions.
    assert_eq!(
        run("(function () { var [[a], [b]] = [[1], [2]]; return a + b; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { var [{a}] = [{a: 5}]; return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var {a: [b]} = {a: [7]}; return b; })()"),
        "7"
    );
}

#[test]
fn an_array_pattern_stops_asking_a_spent_iterator_and_closes_one_it_abandons() {
    // §8.6.2 — the `done` latches, so two names over a one-element iterable call `next` twice and
    // not three times. A counter in `next` is the only thing that can see it.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { n++; return {value: n, done: n > 1}; }}; }; \
             var [a, b] = o; return n + ':' + a + ',' + typeof b; })()"
        ),
        "2:1,undefined"
    );
    // §8.6.2 step 4 — a pattern that finishes while the iterator has not abandons it, and says
    // so. This is the case an object pattern has no equivalent of.
    let endless = "var o = {}; o[Symbol.iterator] = function () { return {\
                   next: function () { return {value: 1, done: false}; }, \
                   return: function () { c = true; return {}; }}; };";
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {endless} var [a] = o; return c; }})()"
        )),
        "true"
    );
    // …and one that ran out on its own is already finished with.
    assert_eq!(
        run(
            "(function () { var c = false; var o = {}; o[Symbol.iterator] = function () { \
             return {next: function () { return {value: 1, done: true}; }, \
             return: function () { c = true; return {}; }}; }; var [a] = o; return c; })()"
        ),
        "false"
    );
    // An error while binding abandons it too — a default that throws is the easiest way in.
    assert_eq!(
        run(&format!(
            "(function () {{ var c = false; {endless} \
             try {{ var [a = (function () {{ throw new Error('x'); }})()] = o; }} catch (e) {{}} \
             return c; }})()"
        )),
        "true"
    );
    // What is not iterable says so, and that includes a plain object — which an object pattern
    // would have taken apart happily.
    for source in ["5", "null", "{}"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ var [a] = {source}; return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
}

#[test]
fn a_pattern_binds_in_every_other_place_a_binding_may_be_written() {
    // §14.7.5 — a `for`-`of` or `for`-`in` head, which is the idiom the whole feature exists for.
    assert_eq!(
        run(
            "(function () { var r = ''; for (var [a, b] of [[1, 2], [3, 4]]) { r += a + '' + b; } \
             return r; })()"
        ),
        "1234"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (const [k, v] of [['x', 1]]) { r += k + v; } return r; })()"
        ),
        "x1"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (let {x} of [{x: 'p'}, {x: 'q'}]) { r += x; } return r; })()"
        ),
        "pq"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (const {a: {b}} of [{a: {b: 7}}]) { r += b; } return r; })()"
        ),
        "7"
    );
    assert_eq!(
        run(
            "(function () { var r = ''; for (const [a, ...rest] of [[1, 2, 3]]) { \
             r += a + ':' + rest.join(','); } return r; })()"
        ),
        "1:2,3"
    );
    assert_eq!(
        run(
            "(function () { var r = 0; for (const {x = 5} of [{}, {x: 2}]) { r += x; } return r; })()"
        ),
        "7"
    );
    // …and the names a head declares are the head's kind, which only an assignment to them shows:
    // a `const` pattern binds constants and a `let` one does not.
    assert_eq!(
        run(
            "(function () { for (const [a] of [[1]]) { try { a = 2; return 'ok'; }              catch (e) { return e.constructor.name; } } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { for (const {a} of [{a: 1}]) { try { a = 2; return 'ok'; }              catch (e) { return e.constructor.name; } } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run("(function () { for (let [a] of [[1]]) { a = 2; return a; } })()"),
        "2"
    );
    assert_eq!(
        run("(function () { for (let {a} of [{a: 1}]) { a = 5; return a; } })()"),
        "5"
    );
    // A `for`-`in` head yields Strings, and a pattern takes one apart by its characters.
    assert_eq!(
        run("(function () { var r = ''; for (var [a] in {ab: 1}) { r += a; } return r; })()"),
        "a"
    );
    // §14.15.3 — a catch parameter, whose names are bindings of the catch block and nothing wider.
    assert_eq!(
        run("(function () { try { throw [1, 2]; } catch ([a, b]) { return a + b; } })()"),
        "3"
    );
    assert_eq!(
        run("(function () { try { throw {a: 5}; } catch ({a}) { return a; } })()"),
        "5"
    );
    assert_eq!(
        run("(function () { try { throw {a: 1}; } catch ({a: x}) { return x; } })()"),
        "1"
    );
    assert_eq!(
        run("(function () { try { throw {}; } catch ({a = 9}) { return a; } })()"),
        "9"
    );
    assert_eq!(
        run("(function () { try { throw [[1]]; } catch ([[a]]) { return a; } })()"),
        "1"
    );
    // The catch's names hide an outer one for the block and give it back afterwards, which is
    // what makes them the block's own rather than an assignment to whatever was there.
    assert_eq!(
        run(
            "(function () { var a = 'outer'; try { throw {a: 'inner'}; } catch ({a}) { return a; } \
             finally {} })()"
        ),
        "inner"
    );
    assert_eq!(
        run(
            "(function () { var a = 'outer'; try { throw {a: 'inner'}; } catch ({a}) {} return a; })()"
        ),
        "outer"
    );
}

#[test]
fn an_assignment_pattern_writes_to_references_and_not_only_to_names() {
    assert_eq!(
        run("(function () { var a, b; [a, b] = [1, 2]; return a + ',' + b; })()"),
        "1,2"
    );
    assert_eq!(
        run("(function () { var a; ({a} = {a: 5}); return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var x; ({a: x} = {a: 6}); return x; })()"),
        "6"
    );
    // The idiom the form exists for, and the one that needs both sides read before either is
    // written.
    assert_eq!(
        run("(function () { var a = 1, b = 2; [a, b] = [b, a]; return a + ',' + b; })()"),
        "2,1"
    );
    // §13.15.5.3 — the target is a *reference*, so a property or a computed one is as ordinary
    // here as a name. This is the whole difference from a binding pattern, which makes names.
    assert_eq!(
        run("(function () { var o = {}; [o.x] = [7]; return o.x; })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var o = {}; ({a: o.y} = {a: 8}); return o.y; })()"),
        "8"
    );
    assert_eq!(
        run("(function () { var a = [0]; [a[0]] = [9]; return a[0]; })()"),
        "9"
    );
    // The reference is evaluated where the element is taken, not before the pattern begins — so
    // a side effect in it happens once, in that order.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {get k() { n++; return {}; }}; \
             var a; [o.k] = [1]; return n; })()"
        ),
        "0"
    );
    assert_eq!(
        run(
            "(function () { var i = 0; var t = [{}, {}]; [t[i++].v] = [5]; \
             return t[0].v + ',' + i; })()"
        ),
        "5,1"
    );
    // Everything a binding pattern does, it does too: defaults, elisions, rest, nesting, and any
    // iterable as the source.
    assert_eq!(
        run("(function () { var a; [a = 3] = []; return a; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { var a; ({a = 4} = {}); return a; })()"),
        "4"
    );
    assert_eq!(
        run("(function () { var a, b; [a, , b] = [1, 2, 3]; return a + ',' + b; })()"),
        "1,3"
    );
    assert_eq!(
        run("(function () { var a, r; [a, ...r] = [1, 2, 3]; return a + ':' + r.join(','); })()"),
        "1:2,3"
    );
    assert_eq!(
        run("(function () { var a, b; [[a], [b]] = [[1], [2]]; return a + b; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { var a; [{a}] = [{a: 5}]; return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var a; ({x: [a]} = {x: [6]}); return a; })()"),
        "6"
    );
    assert_eq!(
        run("(function () { var a, b; [a, b] = 'xy'; return a + b; })()"),
        "xy"
    );
    // §13.15.2 — the *value* of the assignment is what was assigned, not what was bound. A
    // pattern consumes the value, so a copy is kept for whatever wanted the expression.
    assert_eq!(
        run("(function () { var a; return ([a] = [1]).length; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var a; var v = ([a] = [7]); return v[0]; })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var a; var v = ({a} = {a: 2}); return v.a; })()"),
        "2"
    );
    // …and the same refusals a binding pattern makes.
    assert_eq!(
        run("(function () { var a; try { [a] = 5; return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run("(function () { var a; try { ({a} = null); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // §14.7.5.5 — and a `for` head may be a pattern rather than a declaration.
    assert_eq!(
        run(
            "(function () { var r = ''; var a, b; for ([a, b] of [[1, 2], [3, 4]]) { \
             r += a + '' + b; } return r; })()"
        ),
        "1234"
    );
}

#[test]
fn a_rest_property_collects_what_the_pattern_did_not_name() {
    assert_eq!(
        run("(function () { var {a, ...r} = {a: 1, b: 2, c: 3}; \
             return a + ':' + Object.keys(r).join(','); })()"),
        "1:b,c"
    );
    assert_eq!(
        run("(function () { var {...r} = {a: 1}; return Object.keys(r).join(','); })()"),
        "a"
    );
    assert_eq!(
        run("(function () { var {a, ...r} = {a: 1}; return Object.keys(r).length; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var {a, ...r} = {a: 1, b: 2}; return r.a === undefined; })()"),
        "true"
    );
    // …in an assignment pattern and a parameter too, and nested inside another pattern.
    assert_eq!(
        run("(function () { var a, r; ({a, ...r} = {a: 1, b: 2}); \
             return a + ':' + Object.keys(r).join(','); })()"),
        "1:b"
    );
    assert_eq!(
        run(
            "(function () { var o = {}; ({...o.rest} = {a: 1}); return Object.keys(o.rest).join(','); })()"
        ),
        "a"
    );
    assert_eq!(
        run(
            "(function ({a, ...r}) { return a + ':' + Object.keys(r).join(','); })({a: 1, b: 2, c: 3})"
        ),
        "1:b,c"
    );
    assert_eq!(
        run(
            "(function () { var {a: {b, ...inner}} = {a: {b: 1, c: 2}}; \
             return b + ':' + Object.keys(inner).join(','); })()"
        ),
        "1:c"
    );
    // §7.3.25 — own *enumerable* properties, so a hidden one stays hidden…
    assert_eq!(
        run("(function () { var o = {a: 1}; \
             Object.defineProperty(o, 'h', {value: 2, enumerable: false}); \
             var {...r} = o; return Object.keys(r).join(','); })()"),
        "a"
    );
    // …and a **get** per property, so a getter runs here and its answer is what lands. The rest
    // object holds values, never accessors, however the source held them.
    assert_eq!(
        run(
            "(function () { var o = {get g() { return 7; }, a: 1}; var {...r} = o; \
             var d = Object.getOwnPropertyDescriptor(r, 'g'); \
             return r.g + ',' + (typeof d.get); })()"
        ),
        "7,undefined"
    );
    // …and what it lands as is an ordinary property in every way, whatever it was on the source.
    assert_eq!(
        run(
            "(function () { var {...r} = {a: 1};              var d = Object.getOwnPropertyDescriptor(r, 'a');              return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "true,true,true"
    );
    // A Symbol key is copied like any other: §7.3.25 asks about enumerability and not about the
    // kind of key, which is why this carries across where `Object.keys` would not list it.
    assert_eq!(
        run(
            "(function () { var s = Symbol('s'); var o = {a: 1}; o[s] = 2; \
             var {a, ...r} = o; return r[s]; })()"
        ),
        "2"
    );
    // A computed key is evaluated **once**, which is the whole reason the keys are stashed rather
    // than written out a second time for the exclusion list.
    assert_eq!(
        run(
            "(function () { var n = 0; var f = function () { n++; return 'b'; }; \
             var {[f()]: v, ...r} = {a: 1, b: 2}; return n + ':' + v; })()"
        ),
        "1:2"
    );
    assert_eq!(
        run(
            "(function () { var k = 'b'; var {[k]: v, ...r} = {a: 1, b: 2}; \
             return v + ':' + Object.keys(r).join(','); })()"
        ),
        "2:a"
    );
    // The result is an ordinary object, and a primitive source is read through the object it
    // stands for — `undefined` and `null` are refused before any of that.
    assert_eq!(
        run("(function () { var {a, ...r} = {a: 1, b: 2}; \
             return Object.getPrototypeOf(r) === Object.prototype; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var {...r} = 'ab'; return Object.keys(r).join(','); })()"),
        "0,1"
    );
    assert_eq!(
        run("(function () { var {...r} = 5; return Object.keys(r).length; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { try { var {...r} = null; return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn a_spread_element_iterates_rather_than_reading_indices() {
    assert_eq!(run("[...[1, 2]].join(',')"), "1,2");
    assert_eq!(run("[0, ...[1, 2], 3].join(',')"), "0,1,2,3");
    assert_eq!(run("[...[1], ...[2, 3]].join(',')"), "1,2,3");
    assert_eq!(run("[...[]].length"), "0");
    assert_eq!(run("Array.isArray([...[1]])"), "true");
    // A copy, not a window: the two arrays are separate afterwards.
    assert_eq!(
        run("(function () { var a = [1]; var b = [...a]; b.push(2); \
             return a.length + ',' + b.length; })()"),
        "1,2"
    );
    // §13.2.4.1 spreads by *iterating*, so anything iterable works and a String comes apart by
    // code point — which is the whole difference from copying indices.
    assert_eq!(run("[...'ab'].join('|')"), "a|b");
    assert_eq!(run("[...'\\ud83d\\ude00'].length"), "1");
    assert_eq!(run("[...[1, 2].keys()].join(',')"), "0,1");
    assert_eq!(run("[...[1, 2], ...'ab'].join('')"), "12ab");
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.iterator] = function () { var n = 0; \
             return {next: function () { n++; return {value: n, done: n > 3}; }}; }; \
             return [...o].join(','); })()"
        ),
        "1,2,3"
    );
    // …and what is not iterable is refused, a plain object included.
    for source in ["5", "{}", "null"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return [...{source}]; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // Holes still count where they are written, which is why `length` is set from the running
    // index at the end rather than left to whatever the last element happened to grow it to.
    assert_eq!(run("[...[1], , 2].length"), "3");
    assert_eq!(
        run("(function () { var a = [...[1], , 2]; return (1 in a) + ',' + a.length; })()"),
        "false,3"
    );
    assert_eq!(
        run("(function () { var a = [, ...[1]]; return (0 in a) + ',' + a[1]; })()"),
        "false,1"
    );
    // The spread expression is evaluated once, where it is written.
    assert_eq!(
        run(
            "(function () { var n = 0; var a = [...(function () { n++; return [1]; })()]; \
             return n + ':' + a[0]; })()"
        ),
        "1:1"
    );
    // The ordinary path is unchanged — a literal with no spread still knows its own length.
    assert_eq!(run("[1, , 2].length"), "3");
}

#[test]
fn a_step_that_throws_leaves_the_iterator_spent_and_unclosed() {
    // §7.4.8 steps 2.a, 5.a and 9 — `next` throwing, a `done` getter throwing and a `value` getter
    // throwing all set `[[Done]]` before the completion leaves. What that decides is §8.6.2 step 4
    // and §13.15.5.2 step 5: the abandoning caller closes the iterator only while it is *not*
    // done, and an iterator that failed to produce has not been left mid-walk.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { throw 'from next' }, \
                 return: function () { closed += 1; return {} } } }; \
             try { var [a] = able } catch (e) {} closed"),
        "0"
    );
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { get done() { throw 'from done' } } }, \
                 return: function () { closed += 1; return {} } } }; \
             try { var [a] = able } catch (e) {} closed"),
        "0"
    );
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, get value() { throw 'from value' } } }, \
                 return: function () { closed += 1; return {} } } }; \
             try { var [a] = able } catch (e) {} closed"),
        "0"
    );
    // …and a walk abandoned while the iterator is still going **is** closed, which is the half
    // that keeps this about `[[Done]]` rather than about closing less often. The throw here is
    // after a value really arrived, so the flag was cleared and step 4 has something to close.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { closed += 1; return {} } } }; \
             var o = {}; Object.defineProperty(o, 'x', { set: function () { throw 'from target' } }); \
             try { [o.x] = able } catch (e) {} closed"),
        "1"
    );
    // A pattern that finishes first closes too, and that path is not a throw at all.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { closed += 1; return {} } } }; \
             var [a] = able; a + ',' + closed"),
        "1,1"
    );
    // And an iterator that ran out on its own is not closed — it was not abandoned.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: true } }, \
                 return: function () { closed += 1; return {} } } }; \
             var [a] = able; (a === undefined) + ',' + closed"),
        "true,0"
    );
}

#[test]
fn a_destructuring_target_is_resolved_before_the_value_is_fetched() {
    // §13.15.5.5 step 1 — the target's *reference* is evaluated before step 2 steps the iterator.
    // So a target that throws leaves the iterator untouched: `next` is never called, and
    // §13.15.5.2 step 5 closes an iterator that was asked for nothing. ViperJS stepped first, which
    // is one `next` too many and one `return` too few.
    assert_eq!(
        run("var nexts = 0, closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { nexts += 1; return { done: true } }, \
                 return: function () { closed += 1; return {} } } }; \
             var thrower = function () { throw 'from target' }; \
             try { 0, [{}[thrower()]] = able } catch (e) {} \
             nexts + ',' + closed"),
        "0,1"
    );
    // §13.15.5.6 — three things in a fixed order for an object pattern, and ViperJS had the last
    // two the other way round: the property **name**, then the target's reference, then the read.
    assert_eq!(
        run("var log = []; \
             var src = { get a() { log.push('read'); return 1 } }; \
             var o = {}; \
             ({ a: o[(function () { log.push('key'); return 't' })()] } = src); \
             log.join(',') + '|' + o.t"),
        "key,read|1"
    );
    // The base of a member target is evaluated then too, and only once.
    assert_eq!(
        run("var log = []; var made = 0; \
             var src = { get a() { log.push('read'); return 1 } }; \
             var base = function () { made += 1; log.push('base'); return {} }; \
             ({ a: base().x } = src); log.join(',') + '|' + made"),
        "base,read|1"
    );
    // …and none of that changes what the assignment *does*, which is the half a reordering can
    // quietly break.
    assert_eq!(
        run("var a = {}, b = {}; [a.x, b.y] = [1, 2]; \
             var o = {}; [...o.rest] = [3, 4]; \
             var d = {}; [d.a = 5] = []; \
             var p = {}, q = {}; ({ m: p.x, ...q.rest } = { m: 6, n: 7 }); \
             [a.x, b.y, o.rest.join('/'), d.a, p.x, q.rest.n].join(',')"),
        "1,2,3/4,5,6,7"
    );
}

#[test]
fn a_close_on_the_way_out_of_a_throw_keeps_the_original_error() {
    // §7.4.9 step 4 — the completion being carried out wins, so everything the close itself can
    // fail at is discarded: the `return` throwing when called, and the getter step 2 reads it
    // with. ViperJS swallowed those only for an **awaited** close, above a comment that already
    // said "every failure of the close is discarded".
    assert_eq!(
        run(
            "var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { throw 'from return' } } }; \
             var o = {}; Object.defineProperty(o, 'x', { set: function () { throw 'from target' } }); \
             var caught = 'none'; try { [o.x] = able } catch (e) { caught = e } caught"
        ),
        "from target"
    );
    assert_eq!(
        run(
            "var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 get return() { throw 'from getter' } } }; \
             var o = {}; Object.defineProperty(o, 'x', { set: function () { throw 'from target' } }); \
             var caught = 'none'; try { [o.x] = able } catch (e) { caught = e } caught"
        ),
        "from target"
    );
    // §15.5.5 step 7.b.iii.4 is the exception, and it is why this is a `Check` and not a rule:
    // `yield*` closes a source with no `throw` method carrying a **normal** completion, so step 4
    // does not fire and the close's own error is what the program sees — the TypeError below it
    // never gets raised.
    assert_eq!(
        run(
            "var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { throw 'from return' } } }; \
             function* g() { yield* able } \
             var it = g(); it.next(); \
             var caught = 'none'; try { it.throw('sent') } catch (e) { caught = e } caught"
        ),
        "from return"
    );
    // …and with a `return` that answers cleanly, that same path raises §15.5.5's own TypeError.
    assert_eq!(
        run(
            "var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { return {} } } }; \
             function* g() { yield* able } \
             var it = g(); it.next(); \
             var caught = 'none'; try { it.throw('sent') } catch (e) { caught = e.constructor.name } caught"
        ),
        "TypeError"
    );
}

#[test]
fn a_jump_out_of_a_pattern_closes_its_iterator_as_a_throw_does() {
    // §13.15.5.2 step 5 and §8.6.2 step 4 close on **any** abrupt completion, and ViperJS armed a
    // handler — which catches a throw and nothing else. A `return` jumping out of a pattern is not
    // a throw, and there is a way to write one: a default inside the pattern may `yield`, so
    // resuming that suspension with `it.return()` unwinds straight through the half-run pattern.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: undefined } }, \
                 return: function () { closed += 1; return {} } } }; \
             function* g() { var r; r = [ {} = yield ] = able } \
             var it = g(); it.next(); it.return(1); closed"),
        "1"
    );
    // The binding form has the same hole and the same fix — `var [a = yield] = able`.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { closed += 1; return {} } } }; \
             function* g() { var [a = yield] = able } \
             var it = g(); it.next(); it.return(1); closed"),
        "1"
    );
    // The entry goes in **after** the handler is armed, because closing takes the handler down as
    // part of the same jump. Reversed, the jump would pop nothing and leave a handler armed over
    // code the program has already left — so this asserts an ordinary throw afterwards is caught
    // where it was written and not by the pattern's.
    assert_eq!(
        run("var closed = 0; \
             var able = {}; able[Symbol.iterator] = function () { return { \
                 next: function () { return { done: false, value: 1 } }, \
                 return: function () { closed += 1; return {} } } }; \
             function* g() { var [a = yield] = able } \
             var it = g(); it.next(); it.return(1); \
             var caught = 'none'; try { throw 'after' } catch (e) { caught = e } \
             caught + ',' + closed"),
        "after,1"
    );
    // And an ordinary destructuring still works, which is what keeps this about the jump.
    assert_eq!(
        run("var a = {}; [a.x] = [7]; var [b] = [8]; a.x + ',' + b"),
        "7,8"
    );
}
