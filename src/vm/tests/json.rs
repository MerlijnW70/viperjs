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

#[test]
fn json_that_nests_past_the_cap_is_refused_rather_than_running_out_of_stack() {
    // DR-0002 — a stack overflow is not a failure any `Result` can rescue and it takes the
    // embedder's process with it. §25.5 puts no limit on nesting, so the refusal is ViperJS's and
    // is reported as a RangeError: the text is perfectly good JSON, and what ran out is this
    // engine's willingness to descend.
    //
    // All three of §25.5's walks recurse and all three were unbounded. Each gets a row, because
    // one cap shared between them is bounded by whichever spends the most stack per level.
    let deep = |wrappers: usize| {
        format!("var s = '1'; for (var i = 0; i < {wrappers}; i++) {{ s = '[' + s + ']'; }} s")
    };
    // The reader: at the cap it parses, one past it refuses.
    assert_eq!(
        run(&format!("{} && JSON.parse(s) && 'read'", deep(64))),
        "read"
    );
    assert_eq!(
        run(&format!(
            "{} && (function () {{ try {{ JSON.parse(s); return 'read'; }} \
             catch (e) {{ return e.constructor.name; }} }})()",
            deep(65)
        )),
        "RangeError"
    );
    // The reviver walk, over the same text — one deeper than the reader would accept is never
    // reached, so this is asked at the cap and one past it like the reader.
    assert_eq!(
        run(&format!(
            "{} && JSON.parse(s, function (k, v) {{ return v; }}) && 'revived'",
            deep(64)
        )),
        "revived"
    );
    // The serialiser, over a graph built rather than parsed — `stringify` never sees the text.
    let built = |levels: usize| {
        format!(
            "var o = {{}}; var t = o; for (var i = 0; i < {levels}; i++) {{ t.n = {{}}; t = t.n; }} o"
        )
    };
    assert_eq!(
        run(&format!("{} && JSON.stringify(o).length > 0", built(63))),
        "true"
    );
    assert_eq!(
        run(&format!(
            "{} && (function () {{ try {{ JSON.stringify(o); return 'wrote'; }} \
             catch (e) {{ return e.constructor.name; }} }})()",
            built(64)
        )),
        "RangeError"
    );
    // A **cycle** keeps its own answer — §25.5.2.1 step 4's TypeError — because the cycle check is
    // asked first. Reaching the depth cap instead would rename an error a program relies on.
    assert_eq!(
        run("var o = {}; o.self = o; \
             (function () { try { JSON.stringify(o); return 'wrote'; } \
              catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // …and the counter comes back down, so a *wide* document is not a deep one. Written the
    // obvious way — raised on the way in and lowered only after the loop — this refuses
    // `[[],[],[]…]` for being 2,000 deep when it is 2,000 wide and one deep.
    assert_eq!(
        run("var wide = '[' + new Array(2000).join('[],') + '[]]'; JSON.parse(wide).length"),
        "2000"
    );
    assert_eq!(
        run(
            "var a = []; for (var i = 0; i < 2000; i++) { a.push([]); } \
             JSON.stringify(a).length"
        ),
        "6001"
    );
}

#[test]
fn a_reviver_that_puts_something_deeper_where_the_walk_is_going_is_answered_rather_than_crashed() {
    // The walk `revive` descends is **not** the text that was parsed. §25.5.1.1 hands the reviver
    // the holder as its `this`, so it can replace a sibling the walk has not reached yet — and the
    // graph therefore grows as the walk goes, which is why the reader's own cap cannot stand in
    // for this one.
    //
    // A hundred levels put where element 1 is about to be visited, from a two-element document.
    assert_eq!(
        run(
            "var deep = {}; var t = deep;              for (var i = 0; i < 100; i++) { t.n = {}; t = t.n; }              (function () { try {                  JSON.parse('[0,0]', function (k, v) { this[1] = deep; return v; }); return 'ran';              } catch (e) { return e.constructor.name; } })()"
        ),
        "RangeError"
    );
    // …and the same shape inside the budget runs, so the row above is the cap firing rather than
    // the trick itself being refused.
    assert_eq!(
        run(
            "var shallow = { a: { b: 1 } };              JSON.parse('[0,0]', function (k, v) { this[1] = shallow; return v; }) && 'ran'"
        ),
        "ran"
    );
    // test262's `reviver-array-length-coerce-err.js`, reduced — the program that found the crash.
    // It gets §25.5.1.1's own answer now: step 2.b.ii reads the array's `length`, and this one is
    // answered by a proxy with something whose `valueOf` throws. Before the array branch existed
    // that read never happened, the walk went into the *function object* behind `valueOf`, and
    // ViperJS ran off the end of the stack instead.
    assert_eq!(
        run(
            "var uncoercible = { valueOf: function () { throw 'boom'; } };              var badLength = new Proxy([], { get: function (_, name) {                  if (name === 'length') { return uncoercible; } } });              (function () { try { JSON.parse('[0,0]', function () { this[1] = badLength; });                  return 'ran'; } catch (e) { return typeof e === 'string' ? e : e.constructor.name; } })()"
        ),
        "boom"
    );
}

#[test]
fn the_reviver_walks_an_array_by_index_and_an_object_by_its_enumerable_names() {
    // §25.5.1.1 steps 2.b and 2.c are two different walks, and ViperJS used to do neither: it asked
    // every value for its own keys. Both halves of that are observable, and reaching them needs
    // the value to be *in place* when the walk arrives — a reviver's return value replaces a
    // property and is not descended into, so each row puts its subject where the walk is going.

    // Step 2.b — an array is walked from `0` to `ToLength(Get(val, "length"))`, so **reading
    // `length` is a step of the algorithm**: a proxy answering for it is called.
    assert_eq!(
        run("var reads = 0; \
             var a = new Proxy([7, 8], { get: function (t, k) { \
                 if (k === 'length') { reads++; } return t[k]; } }); \
             JSON.parse('{\"x\":0,\"y\":0}', function (k, v) { \
                 if (k === 'x') { this.y = a; } return v; }); \
             reads"),
        "1"
    );
    // …and an index the array has no property for is still visited, because the walk counts rather
    // than asking which keys exist. A hole arrives as `undefined` and the reviver may fill it.
    assert_eq!(
        run("var a = [1]; a.length = 3; \
             var out = JSON.parse('{\"x\":0,\"y\":0}', function (k, v) { \
                 if (k === 'x') { this.y = a; return v; } \
                 if (this === a) { return v === undefined ? 'filled' : v; } \
                 return v; }); \
             JSON.stringify(out.y)"),
        "[1,\"filled\",\"filled\"]"
    );

    // Step 2.c — everything else by `EnumerableOwnPropertyNames`, which excludes two different
    // things: a Symbol key, which is not a name a document could have had, and a **non-enumerable**
    // property, which is not the walk's to visit.
    assert_eq!(
        run("var o = { seen: 1 }; \
             Object.defineProperty(o, 'hidden', { value: 2, enumerable: false }); \
             o[Symbol('s')] = 3; \
             var names = []; \
             JSON.parse('{\"x\":0,\"y\":0}', function (k, v) { \
                 if (k === 'x') { this.y = o; } \
                 if (this === o) { names.push(k); } return v; }); \
             names.join(',')"),
        "seen"
    );
    // The same rule read the other way, and the reason the crash happened: a **function** has
    // `length` and `name` and neither is enumerable, so the walk does not go into one. Asking for
    // every own key instead is what sent `revive` down through a `valueOf` into its own properties
    // until the stack ran out.
    assert_eq!(
        run("var f = function () {}; var count = 0; \
             JSON.parse('{\"x\":0,\"y\":0}', function (k, v) { \
                 if (k === 'x') { this.y = f; } \
                 if (this === f) { count++; } return v; }); \
             count"),
        "0"
    );
}

#[test]
fn walking_json_at_the_cap_fits_in_the_stack_it_claims_to_need() {
    // What makes `MAX_JSON_DEPTH` a measurement rather than a hope, and the twin of the parser's
    // `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need`. A cap the stack cannot afford is
    // worse than no cap: the walk dies by overflow one level before the check meant to prevent
    // exactly that.
    //
    // One mebibyte is the smallest thread stack in common use, and this is a debug build, whose
    // frames are largest. Measured that way the reader dies between 750 and 800 wrappers, a
    // reviver walk past 400, and the serialiser between 250 and 300 — so the serialiser is what
    // the number has to fit inside, and if a slice adds frames between one level and the next
    // this is where it says so.
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let deep = "var s = '1'; for (var i = 0; i < 64; i++) { s = '[' + s + ']'; } s";
            let built =
                "var o = {}; var t = o; for (var i = 0; i < 63; i++) { t.n = {}; t = t.n; } o";
            [
                run(&format!("{deep} && JSON.parse(s) && 'read'")),
                run(&format!(
                    "{deep} && JSON.parse(s, function (k, v) {{ return v; }}) && 'revived'"
                )),
                run(&format!("{built} && JSON.stringify(o).length > 0")),
                // The three together, which is the shape a round trip actually takes.
                run(&format!(
                    "{deep} && JSON.stringify(JSON.parse(s, function (k, v) {{ return v; }})) === s"
                )),
            ]
        })
        .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
    let answers = worker
        .join()
        .unwrap_or_else(|_| panic!("walking at the cap needs more than the mebibyte it claims")); // a panic in the thread is the failure being reported
    assert_eq!(answers, ["read", "revived", "true", "true"]);
}
