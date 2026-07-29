//! §25.5 — `JSON.parse` and `JSON.stringify`.
//!
//! Checked against V8 first. Two themes run through it. `stringify` promises its output parses
//! back, which is what decides every awkward case — a lone surrogate, a NaN, a cycle. And JSON's
//! grammar is *narrower* than JavaScript's in both directions, so the parser has its own rows for
//! everything a JavaScript literal would allow and JSON does not.

use super::*;

#[test]
fn what_has_json_and_what_has_none() {
    assert_eq!(run("JSON.stringify(1)"), "1");
    assert_eq!(run("JSON.stringify('a')"), "\"a\"");
    assert_eq!(run("JSON.stringify(true)"), "true");
    assert_eq!(run("JSON.stringify(null)"), "null");
    assert_eq!(run("JSON.stringify([1, 2])"), "[1,2]");
    assert_eq!(
        run("JSON.stringify({a: 1, b: 'x'})"),
        "{\"a\":1,\"b\":\"x\"}"
    );
    assert_eq!(run("JSON.stringify({a: {b: [1]}})"), "{\"a\":{\"b\":[1]}}");
    // `undefined`, a function and a Symbol have no JSON at all, and at the top level that is the
    // answer rather than an error or an empty string.
    assert_eq!(run("typeof JSON.stringify(undefined)"), "undefined");
    assert_eq!(run("typeof JSON.stringify(function () {})"), "undefined");
    // In an object such a property is *omitted*; in an array it becomes `null`. The two are
    // opposite because an array's shape is its indices and dropping one would move the rest.
    assert_eq!(run("JSON.stringify({a: undefined, b: 1})"), "{\"b\":1}");
    assert_eq!(run("JSON.stringify([1, undefined, 2])"), "[1,null,2]");
    assert_eq!(run("JSON.stringify([function () {}])"), "[null]");
    // §25.5.2.1 step 9 — JSON has no NaN and no infinities, and text that did not parse back
    // would be worse than a number that is not the one you had.
    assert_eq!(run("JSON.stringify(NaN)"), "null");
    assert_eq!(run("JSON.stringify(Infinity)"), "null");
    // A wrapper is unwrapped before anything is decided about it.
    assert_eq!(run("JSON.stringify(new Number(5))"), "5");
    assert_eq!(run("JSON.stringify(new String('s'))"), "\"s\"");
    assert_eq!(run("Object.prototype.toString.call(JSON)"), "[object JSON]");
    // §25.5.3's tag is a property like any other, and its attributes are §17's for a value.
    assert_eq!(
        run(
            "(function () { var d = Object.getOwnPropertyDescriptor(JSON, Symbol.toStringTag);              return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "false,false,true"
    );
    assert_eq!(run("JSON.stringify.length"), "3");
    assert_eq!(run("JSON.parse.length"), "2");
}

#[test]
fn a_string_is_written_so_that_it_parses_back() {
    assert_eq!(run("JSON.stringify('a\"b')"), "\"a\\\"b\"");
    assert_eq!(run("JSON.stringify('a\\nb')"), "\"a\\nb\"");
    assert_eq!(run("JSON.stringify('\\u0001')"), "\"\\u0001\"");
    // §25.5.2.2's well-formed rule — an *unpaired* surrogate is escaped and a pair is written
    // through. This is the promise the whole method exists to keep: a lone surrogate does not
    // survive a trip through UTF-8, and an escape does.
    assert_eq!(run("JSON.stringify('\\ud800')"), "\"\\ud800\"");
    assert_eq!(run("JSON.stringify('\\ud83d\\ude00').length"), "4");
    // …and the round trip, which is the property all of the above are for.
    assert_eq!(
        run("JSON.parse(JSON.stringify({a: [1, 'x', null]})).a.join(',')"),
        "1,x,"
    );
}

#[test]
fn the_replacer_and_the_indent_are_two_separate_arguments() {
    // A function replacer sees every key and value and answers what should be written.
    assert_eq!(
        run(
            "JSON.stringify({a: 1}, function (k, v) { return typeof v === 'number' ? v * 2 : v; })"
        ),
        "{\"a\":2}"
    );
    // An array replacer is a list of names, and everything else is left out. It is a *list*, so
    // it decides the order too, and a repeat in it writes the property once.
    assert_eq!(run("JSON.stringify({a: 1, b: 2}, ['a'])"), "{\"a\":1}");
    assert_eq!(
        run("JSON.stringify({a: 1, b: 2}, ['b', 'a'])"),
        "{\"b\":2,\"a\":1}"
    );
    assert_eq!(run("JSON.stringify({a: 1}, ['a', 'a'])"), "{\"a\":1}");
    // §25.5.2.1 step 2 — `toJSON` is asked *before* the replacer, so an object that knows how to
    // describe itself gets the first word.
    assert_eq!(
        run("JSON.stringify({toJSON: function () { return 'custom'; }})"),
        "\"custom\""
    );
    // …and a `toJSON` that is not callable is an ordinary property, written like any other.
    assert_eq!(run("JSON.stringify({toJSON: 5})"), "{\"toJSON\":5}");
    assert_eq!(run("JSON.stringify({toJSON: {}})"), "{\"toJSON\":{}}");
    assert_eq!(
        run("JSON.stringify({a: {toJSON: function () { return 1; }}})"),
        "{\"a\":1}"
    );
    // The indent, written on one line here so the newlines are visible as `|`.
    let flat = |source: &str| run(&format!("{source}.split('\\n').join('|')"));
    assert_eq!(flat("JSON.stringify({a: 1}, null, 2)"), "{|  \"a\": 1|}");
    assert_eq!(flat("JSON.stringify([1, 2], null, 2)"), "[|  1,|  2|]");
    assert_eq!(
        flat("JSON.stringify({a: {b: 1}}, null, 2)"),
        "{|  \"a\": {|    \"b\": 1|  }|}"
    );
    assert_eq!(flat("JSON.stringify({a: 1}, null, '__')"), "{|__\"a\": 1|}");
    // An empty structure has no inside to indent, so it stays on one line.
    assert_eq!(flat("JSON.stringify({}, null, 2)"), "{}");
    assert_eq!(flat("JSON.stringify([], null, 2)"), "[]");
    // §25.5.2 steps 6 and 7 — a number is clamped to ten and truncated, and a negative one
    // indents nothing.
    assert_eq!(flat("JSON.stringify({a: 1}, null, 2.9)"), "{|  \"a\": 1|}");
    assert_eq!(flat("JSON.stringify({a: 1}, null, -1)"), "{\"a\":1}");
    // Ten spaces and no more, however many were asked for — `"a": 1` is six characters.
    assert_eq!(
        run("JSON.stringify({a: 1}, null, 100).split('\\n')[1].length"),
        "16"
    );
    // §25.5.2 step 5 — a wrapper indents as the primitive it holds…
    assert_eq!(
        flat("JSON.stringify({a: 1}, null, new Number(2))"),
        "{|  \"a\": 1|}"
    );
    assert_eq!(
        flat("JSON.stringify({a: 1}, null, new String('__'))"),
        "{|__\"a\": 1|}"
    );
    // …but the slot only chooses *which* conversion runs, and the conversion is applied to the
    // object. So an overridden `valueOf` answers for a Number and an overridden `toString` for a
    // String — reading the slot directly would ignore both and silently indent by the wrong thing.
    assert_eq!(
        flat(
            "(function () { var n = new Number(1); n.valueOf = function () { return 3; }; \
             return JSON.stringify({a: 1}, null, n); })()"
        ),
        "{|   \"a\": 1|}"
    );
    assert_eq!(
        flat(
            "(function () { var s = new String('xx'); s.toString = function () { return '--'; }; \
             return JSON.stringify({a: 1}, null, s); })()"
        ),
        "{|--\"a\": 1|}"
    );
    // Each conversion takes the hint its slot implies, so the *other* method is never reached —
    // asserted by making it throw, which is the only way to observe that it was not called.
    assert_eq!(
        flat(
            "(function () { var n = new Number(1); n.valueOf = function () { return 2; }; \
             n.toString = function () { throw new Error('reached'); }; \
             return JSON.stringify({a: 1}, null, n); })()"
        ),
        "{|  \"a\": 1|}"
    );
    assert_eq!(
        flat(
            "(function () { var s = new String('x'); s.toString = function () { return '__'; }; \
             s.valueOf = function () { throw new Error('reached'); }; \
             return JSON.stringify({a: 1}, null, s); })()"
        ),
        "{|__\"a\": 1|}"
    );
    // …and a conversion that throws is not swallowed on the way out.
    assert_eq!(
        run("(function () { var n = new Number(1); \
             n.valueOf = function () { throw new TypeError('no'); }; \
             try { JSON.stringify({a: 1}, null, n); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn only_the_enumerable_string_keys_of_an_object_are_written() {
    // §25.5.2.5 step 5 — `EnumerableOwnProperties(value, key)`, which is two separate conditions
    // and they exclude different properties. A non-enumerable one is skipped…
    assert_eq!(
        run(
            "(function () { var o = {}; Object.defineProperty(o, 'a', {value: 1, \
             enumerable: false}); o.b = 2; return JSON.stringify(o); })()"
        ),
        "{\"b\":2}"
    );
    // …and a Symbol-keyed one is skipped even when it *is* enumerable, because JSON has no name
    // to write for it. Skipping it is not the same as writing an empty one.
    assert_eq!(
        run(
            "(function () { var o = {}; Object.defineProperty(o, Symbol('s'), {value: 1, \
             enumerable: true}); return JSON.stringify(o); })()"
        ),
        "{}"
    );
    assert_eq!(
        run(
            "(function () { var o = {}; Object.defineProperty(o, Symbol('s'), {value: 1, \
             enumerable: true}); o.a = 1; return JSON.stringify(o); })()"
        ),
        "{\"a\":1}"
    );
    // An accessor is read like anything else — it is the *value* that is written, so a getter is
    // called rather than described.
    assert_eq!(
        run(
            "(function () { var o = {a: 1}; Object.defineProperty(o, 'b', {get: function () { \
             return 2; }, enumerable: true}); return JSON.stringify(o); })()"
        ),
        "{\"a\":1,\"b\":2}"
    );
}

#[test]
fn a_cycle_is_refused_and_a_shape_seen_twice_is_not() {
    // §25.5.2.1 step 4 — a TypeError rather than a stack that runs out.
    assert_eq!(
        run(
            "(function () { var o = {}; o.self = o; try { JSON.stringify(o); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // …and the check is the *path* from the root and not everything seen, so the same object in
    // two different branches is written twice and is not a cycle.
    assert_eq!(
        run("(function () { var i = {}; var o = {a: i, b: i}; return JSON.stringify(o); })()"),
        "{\"a\":{},\"b\":{}}"
    );
}

#[test]
fn json_is_a_narrower_grammar_than_javascript_in_both_directions() {
    assert_eq!(run("JSON.parse('1')"), "1");
    assert_eq!(run("JSON.parse('\"a\"')"), "a");
    assert_eq!(run("JSON.parse('true')"), "true");
    assert_eq!(run("JSON.parse('null')"), "null");
    assert_eq!(run("JSON.parse('[1,2]').join(',')"), "1,2");
    assert_eq!(run("JSON.parse('{\"a\":1}').a"), "1");
    assert_eq!(run("Array.isArray(JSON.parse('[]'))"), "true");
    assert_eq!(
        run("JSON.parse(' { \"a\" : [1, {\"b\": 2}] } ').a[1].b"),
        "2"
    );
    assert_eq!(run("JSON.parse('false')"), "false");
    assert_eq!(run("Object.keys(JSON.parse('{}')).length"), "0");
    assert_eq!(run("JSON.parse('[]').length"), "0");
    assert_eq!(
        run("JSON.parse('{\"a\":{}}').a.constructor === Object"),
        "true"
    );
    assert_eq!(run("JSON.parse('-1.5e2')"), "-150");
    // An exponent may carry a sign, and must carry a digit after it.
    assert_eq!(run("JSON.parse('1e+2')"), "100");
    assert_eq!(run("JSON.parse('1e-2')"), "0.01");
    assert_eq!(run("JSON.parse('\"\\\\u0041\"')"), "A");
    // Everything a JavaScript literal allows and JSON does not — each its own row, because each
    // is a place a parser written by reaching for the lexer would quietly accept.
    for text in [
        "01",         // a leading zero
        "{a:1}",      // an unquoted key
        "[1,]",       // a trailing comma
        "\\\'a\\\'",  // single quotes, escaped so the row survives the JavaScript around it
        "1 2",        // two values where one was asked for
        "",           // nothing at all
        "1.",         // a point with no digits after it
        "+1",         // a leading plus
        ".5",         // no integer part
        "0x10",       // hexadecimal
        "[1 2]",      // a missing comma
        "{\"a\"}",    // a key with no value
        "{x\"a\":1}", // rubbish where the key's quote should be
        // …and the one shape that would *parse* if the opening quote were merely assumed rather
        // than checked: the `x` is swallowed in the quote's place, the next `"` closes an empty
        // key, and the colon then lands exactly where it belongs — `{"":1}`, a key nobody wrote.
        // One quote and not two, because a second would sit where the colon should be and be
        // refused for that instead, which tests the wrong thing.
        "{x\":1}",
        "{\"a\"x1}", // rubbish where the colon should be
        "1e",        // an exponent with no digits
        "1e+",       // …nor after its sign
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ JSON.parse('{text}'); return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "SyntaxError",
            "parsing {text:?}"
        );
    }
}

#[test]
fn the_reviver_walks_what_was_parsed_and_may_remove_from_it() {
    assert_eq!(
        run("JSON.parse('{\"a\":1,\"b\":2}', function (k, v) { \
             return typeof v === 'number' ? v + 1 : v; }).a"),
        "2"
    );
    // §25.5.1.1 step 2.b.ii.2 — a reviver answering `undefined` *deletes* the property, which is
    // the only way it can remove one.
    assert_eq!(
        run(
            "(function () { var o = JSON.parse('{\"a\":1,\"b\":2}', function (k, v) { \
             return k === 'a' ? undefined : v; }); return ('a' in o) + ',' + o.b; })()"
        ),
        "false,2"
    );
    // It is given the root under the empty key, which is what lets it replace the whole thing.
    assert_eq!(
        run("JSON.parse('1', function (k, v) { return k === '' ? 'root' : v; })"),
        "root"
    );
}
