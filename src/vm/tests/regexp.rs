//! §22.2 — the `RegExp` object, `exec`, and the literal that makes one.
//!
//! The pattern grammar and the matcher have their own tests beside them, over strings. What is
//! here is the *object*: the shape a match answers in, the `lastIndex` a `g` pattern keeps, and the
//! accessors — none of which the engine below can be asked about.

use super::*;

#[test]
fn a_literal_makes_a_regular_expression_and_a_new_one_each_time() {
    assert_eq!(run("typeof /a/"), "object");
    assert_eq!(run("/a/ instanceof RegExp"), "true");
    // §20.1.3.6 step 12 — the tag comes from the `[[RegExpMatcher]]` slot, which is why an object
    // given `RegExp.prototype` does not get it.
    assert_eq!(
        run("Object.prototype.toString.call(/a/)"),
        "[object RegExp]"
    );
    // §13.2.7.3 — a **new** object per evaluation, so a pattern in a loop does not carry
    // `lastIndex` from one turn to the next. ES3 shared one object per literal and the change is
    // exactly this observation.
    assert_eq!(run("/a/ === /a/"), "false");
    assert_eq!(
        run(
            "var seen = []; for (var i = 0; i < 2; i++) { var r = /a/g; r.exec('aa'); \
             seen.push(r.lastIndex); } seen.join()"
        ),
        "1,1"
    );
}

#[test]
fn exec_answers_an_array_carrying_where_it_matched_and_what_it_searched() {
    assert_eq!(run("JSON.stringify(/b(c)/.exec('abcd'))"), r#"["bc","c"]"#);
    assert_eq!(
        run("var m = /b(c)/.exec('abcd'); m.index + ',' + m.input + ',' + m.length"),
        "1,abcd,2"
    );
    assert_eq!(run("/z/.exec('abc')"), "null");
    assert_eq!(run("Array.isArray(/a/.exec('a'))"), "true");
    // A group that did not participate is `undefined` in the array, which is a different thing
    // from one that matched emptily.
    assert_eq!(run("String(/(a)?b/.exec('b')[1])"), "undefined");
    assert_eq!(run("'[' + /(a?)b/.exec('b')[1] + ']'"), "[]");
}

#[test]
fn named_groups_arrive_in_an_object_with_no_prototype() {
    assert_eq!(
        run(
            "var m = /(?<year>\\d{4})-(?<month>\\d{2})/.exec('on 2026-07-31'); \
             m.groups.year + '/' + m.groups.month"
        ),
        "2026/07"
    );
    // §22.2.7.2 step 34 — `undefined` when the pattern names nothing, not an empty object, so a
    // program can tell "no named groups" from "named groups that did not participate".
    assert_eq!(run("String(/a/.exec('a').groups)"), "undefined");
    // The holder's prototype is **null**, so a group called `toString` reads as itself rather than
    // as the method it would otherwise shadow.
    assert_eq!(
        run("Object.getPrototypeOf(/(?<x>a)/.exec('a').groups)"),
        "null"
    );
    assert_eq!(
        run("typeof /(?<toString>a)/.exec('a').groups.toString"),
        "string"
    );
    assert_eq!(run("String(/(?<x>a)|b/.exec('b').groups.x)"), "undefined");
}

#[test]
fn last_index_moves_only_for_a_pattern_that_asked_for_it() {
    // §22.2.7.2 steps 4 and 5 — without `g` or `y` the property is neither read nor written, which
    // is why a plain pattern finds the same match however many times it is asked.
    assert_eq!(run("var r = /a/; r.exec('aa'); r.exec('aa').index"), "0");
    assert_eq!(run("var r = /a/; r.exec('aa'); r.lastIndex"), "0");
    assert_eq!(
        run(
            "var r = /a/g; var out = []; out.push(r.exec('aa').index, r.lastIndex); \
             out.push(r.exec('aa').index, r.lastIndex); out.join()"
        ),
        "0,1,1,2"
    );
    // A failed search puts it back to zero, so the next call starts over rather than being stuck.
    assert_eq!(
        run("var r = /a/g; r.exec('aa'); r.exec('aa'); r.exec('aa'); r.lastIndex"),
        "0"
    );
    // It is an ordinary writable property, so a program may move the search itself.
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 1; r.exec('aa').index"),
        "1"
    );
    // …and anything it is set to is read through `ToLength`, so nonsense becomes zero rather than
    // an error.
    assert_eq!(
        run("var r = /a/g; r.lastIndex = -5; r.exec('aa').index"),
        "0"
    );
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 'x'; r.exec('aa').index"),
        "0"
    );
    assert_eq!(run("var r = /a/g; r.lastIndex = 99; r.exec('aa')"), "null");
    assert_eq!(
        run(
            "var r = /a/g; var d = Object.getOwnPropertyDescriptor(r, 'lastIndex'); \
             d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,false"
    );
}

#[test]
fn a_sticky_pattern_only_matches_where_last_index_points() {
    assert_eq!(run("var r = /b/y; r.exec('ab')"), "null");
    assert_eq!(
        run("var r = /b/y; r.lastIndex = 1; r.exec('ab').index"),
        "1"
    );
    assert_eq!(run("/b/.exec('ab').index"), "1");
}

#[test]
fn test_answers_the_boolean_exec_would_have_implied() {
    assert_eq!(run("/abc/.test('xabcx')"), "true");
    assert_eq!(run("/abc/.test('xyz')"), "false");
    // It shares `exec`'s state, so a `g` pattern walks between calls.
    assert_eq!(
        run(
            "var r = /a/g; r.test('aa') + ',' + r.lastIndex + ',' + r.test('aa') + ',' + r.lastIndex"
        ),
        "true,1,true,2"
    );
    // A non-string argument is converted, so `test(1)` searches `\"1\"`.
    assert_eq!(run("/1/.test(1)"), "true");
}

#[test]
fn source_answers_the_pattern_in_a_form_that_can_be_read_back() {
    assert_eq!(run("/a/.source"), "a");
    // §22.2.6.13 — an empty pattern reads as `(?:)`, because `//` between slashes is a comment.
    assert_eq!(run("new RegExp('').source"), "(?:)");
    assert_eq!(run("String(new RegExp(''))"), "/(?:)/");
    // …and a `/` is escaped for the same reason: `toString` puts this between slashes.
    assert_eq!(run("String(/a\\/b/)"), "/a\\/b/");
    assert_eq!(run("new RegExp('a/b').source"), "a\\/b");
    // One already escaped is not escaped twice.
    assert_eq!(run("new RegExp('a\\\\/b').source"), "a\\/b");
}

#[test]
fn the_flags_read_back_in_the_order_the_clause_fixes() {
    assert_eq!(run("/a/gimsy.flags"), "gimsy");
    // §22.2.6.4's order, whatever order they were written in.
    assert_eq!(run("/a/yig.flags"), "giy");
    assert_eq!(run("/a/.flags"), "");
    assert_eq!(run("String(/a/gi)"), "/a/gi");
    assert_eq!(
        run(
            "[/a/d.hasIndices, /a/g.global, /a/i.ignoreCase, /a/m.multiline, /a/s.dotAll, \
             /a/u.unicode, /a/v.unicodeSets, /a/y.sticky].join()"
        ),
        "true,true,true,true,true,true,true,true"
    );
    assert_eq!(
        run("[/a/.hasIndices, /a/.global, /a/.ignoreCase, /a/.sticky].join()"),
        "false,false,false,false"
    );
}

#[test]
fn every_flag_is_an_accessor_and_not_a_property_that_can_be_assigned() {
    // §22.2.6 makes each a getter with no setter, so an assignment is ignored in sloppy code and a
    // TypeError in strict — a different thing from a data property holding a Boolean.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(RegExp.prototype, 'global'); \
             typeof d.get + ',' + String(d.set) + ',' + d.configurable"
        ),
        "function,undefined,true"
    );
    assert_eq!(run("var r = /a/; r.global = true; r.global"), "false");
    assert_eq!(
        run("'use strict'; var r = /a/; try { r.global = true } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // The accessors are on the prototype, so an ordinary object cannot answer them.
    assert_eq!(
        run(
            "try { Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').get.call({}) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn the_constructor_takes_a_pattern_and_flags_or_another_regular_expression() {
    assert_eq!(run("new RegExp('a', 'g').flags"), "g");
    assert_eq!(run("new RegExp('a').test('xax')"), "true");
    // §22.2.3.1 — a regular expression argument gives its *source and flags*, so the `g` is kept
    // and a second argument replaces it. Reading its string form instead would give `/a/g`.
    assert_eq!(
        run("new RegExp(/a/g).flags + ',' + new RegExp(/a/g).source"),
        "g,a"
    );
    assert_eq!(run("new RegExp(/a/g, 'i').flags"), "i");
    assert_eq!(run("new RegExp(/a\\/b/).source"), "a\\/b");
    // §22.2.4.1 step 1 — a plain call on one that is already a regular expression, with no new
    // flags, hands back the *same object*. The only constructor in the language that does that.
    assert_eq!(run("var a = /a/; RegExp(a) === a"), "true");
    assert_eq!(run("var a = /a/; RegExp(a, 'g') === a"), "false");
    assert_eq!(run("var a = /a/; (new RegExp(a)) === a"), "false");
    assert_eq!(run("RegExp.name + ',' + RegExp.length"), "RegExp,2");
    // §22.2.5 — the prototype is an ordinary object and not a regular expression itself.
    assert_eq!(run("typeof RegExp.prototype.exec"), "function");
    assert_eq!(
        run("try { RegExp.prototype.exec('a') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_pattern_that_does_not_parse_is_a_syntax_error_when_it_is_made() {
    for source in [
        "new RegExp('(')",
        "new RegExp('a{2,1}')",
        "new RegExp('[z-a]')",
        "new RegExp('\\\\1')",
        "new RegExp('(?<n>a)(?<n>b)')",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source} }} catch (e) {{ e.constructor.name }}"
            )),
            "SyntaxError",
            "{source} should not parse"
        );
    }
    // Bad flags are the same error, and a repeated one counts.
    assert_eq!(
        run("try { new RegExp('a', 'q') } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    assert_eq!(
        run("try { new RegExp('a', 'gg') } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    // A literal's pattern is an **early** error — §22.2.1.1 — so it is not caught by `try` and a
    // bad one inside a branch that never runs still stops the whole script. The *phase* is what
    // test262 asserts, with `negative: {phase: parse}`, and a version that threw when the literal
    // was evaluated failed nearly two thousand tests while looking correct from the inside.
    assert!(
        compiles("var reached = true; if (false) { /a/ } reached"),
        "a good pattern behind a false branch should compile"
    );
    for source in [
        "/(/",
        "var x = /a{2,1}/;",
        "if (false) { /[z-a]/ }",
        "/a/gg",
    ] {
        assert!(!compiles(source), "{source} should not compile at all");
    }
}

#[test]
fn to_string_reads_source_and_flags_as_properties_so_a_subclass_is_obeyed() {
    // §22.2.6.14 goes through `Get`, which is why this answers what the overrides say rather than
    // what the object holds.
    assert_eq!(
        run("var o = {source: 'x', flags: 'g'}; RegExp.prototype.toString.call(o)"),
        "/x/g"
    );
    assert_eq!(run("String(/a/g)"), "/a/g");
}

#[test]
fn compile_replaces_what_a_regular_expression_is_in_place() {
    // §B.2.4.1 — the one thing in the language that changes a regular expression after it is made.
    // Annex B, but a *built-in* rather than a way of writing a program, so DR-0008's exclusion of
    // Annex B's syntactic extensions does not reach it.
    assert_eq!(
        run("var r = /a/; r.compile('b', 'g'); r.source + ',' + r.flags + ',' + r.test('xbx')"),
        "b,g,true"
    );
    // It answers the object it changed, so it chains.
    assert_eq!(run("var r = /a/; r.compile('b') === r"), "true");
    // Re-initialising puts `lastIndex` back to zero and drops the old flags entirely — the new
    // pattern is not a modification of the old one.
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 3; r.compile('b'); r.lastIndex + ',[' + r.flags + ']'"),
        "0,[]"
    );
    // Step 2 — a regular expression argument brings its own flags, so a second argument beside it
    // would be two answers to one question.
    assert_eq!(
        run("var r = /a/; r.compile(/b/i); r.source + ',' + r.flags"),
        "b,i"
    );
    assert_eq!(
        run("try { /a/.compile(/b/i, 'g') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { RegExp.prototype.compile.call({}, 'a') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A pattern that does not parse is refused and the old one is left alone, because
    // `RegExpInitialize` throws before it writes.
    assert_eq!(
        run("var r = /a/; try { r.compile('(') } catch (e) {} r.source"),
        "a"
    );
}

#[test]
fn the_regexp_prototype_is_not_a_regular_expression_and_the_accessors_say_so() {
    // §22.2.6 makes `RegExp.prototype` an ordinary object with no `[[OriginalSource]]` — unlike
    // §21.1.3's `Number.prototype`, which *is* an instance. So every accessor needs step 3's
    // carve-out, and without it reading `RegExp.prototype.source` throws.
    assert_eq!(run("RegExp.prototype.source"), "(?:)");
    assert_eq!(run("RegExp.prototype.flags"), "");
    assert_eq!(run("RegExp.prototype.toString()"), "/(?:)/");
    // `(?:)` is not decoration: it is the source of a pattern matching the empty string, so what
    // `toString` builds out of it parses back to an equivalent regular expression.
    assert_eq!(run("new RegExp(RegExp.prototype.source).test('')"), "true");
    // **`undefined`, not `false`** — which is what makes `flags` above the empty string rather
    // than eight letters, since `undefined` is falsy and each letter is left out.
    assert_eq!(
        run(
            "['hasIndices', 'global', 'ignoreCase', 'multiline', 'dotAll', 'unicode', \
              'unicodeSets', 'sticky'] \
             .map(function (n) { return String(RegExp.prototype[n]) }).join(',')"
        ),
        "undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined"
    );
    // The carve-out is for the prototype and for nothing else: any other object without the slot
    // is still the TypeError §22.2.6 step 3 asks for, and so is a primitive.
    for source in [
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get.call({})",
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').get.call({})",
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get.call(1)",
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').get.call(undefined)",
        "Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get.call(Object.create(RegExp.prototype))",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; 'no error' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source}"
        );
    }
    // …and a real regular expression is unaffected by any of it.
    assert_eq!(run("/ab+c/giy.source"), "ab+c");
    assert_eq!(run("/ab+c/giy.flags"), "giy");
    assert_eq!(run("/a/.global + ',' + /a/g.global"), "false,true");
}

#[test]
fn the_flags_getter_reads_the_eight_properties_and_not_the_slots() {
    // §22.2.6.4's only receiver check is step 2, "is an Object" — there is no `[[OriginalFlags]]`
    // requirement at all. So this works on any object, and a subclass overriding one of the eight
    // is obeyed. Reading the receiver's own flag bits instead would answer `""` for both of these.
    assert_eq!(
        run(
            "Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get \
             .call({global: true, sticky: true})"
        ),
        "gy"
    );
    assert_eq!(
        run("class R extends RegExp { get global() { return true } } new R('a', 'i').flags"),
        "gi"
    );
    // Every letter, in the order §22.2.6.4 lists them — which is not alphabetical and is not the
    // order the accessors are installed in.
    assert_eq!(
        run(
            "Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get.call({ \
                hasIndices: 1, global: 1, ignoreCase: 1, multiline: 1, \
                dotAll: 1, unicode: 1, unicodeSets: 1, sticky: 1 })"
        ),
        "dgimsuvy"
    );
    // Each is `ToBoolean`, so any truthy value counts and any falsy one does not.
    assert_eq!(
        run(
            "Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get \
             .call({global: 'yes', sticky: 0, multiline: {}})"
        ),
        "gm"
    );
    // The reads happen in that order and are observable, because each may run a getter. This is
    // `flags/get-order.js`, which no implementation reading slots can pass.
    assert_eq!(
        run("var seen = []; var o = {}; \
             ['hasIndices', 'global', 'ignoreCase', 'multiline', 'dotAll', 'unicode', \
              'unicodeSets', 'sticky'].forEach(function (n) { \
                Object.defineProperty(o, n, {get: function () { seen.push(n); return false }}) \
             }); \
             Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get.call(o); \
             seen.join(',')"),
        "hasIndices,global,ignoreCase,multiline,dotAll,unicode,unicodeSets,sticky"
    );
    // …and a getter that throws stops the whole thing, rather than being counted as false.
    assert_eq!(
        run("var o = {get global() { throw new EvalError('x') }}; \
             try { Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get.call(o) } \
             catch (e) { e.constructor.name }"),
        "EvalError"
    );
    // Step 2 is still a real check: a primitive receiver is a TypeError.
    for receiver in ["1", "'a'", "undefined", "null", "true"] {
        assert_eq!(
            run(&format!(
                "try {{ Object.getOwnPropertyDescriptor(RegExp.prototype, 'flags').get.call({receiver}); \
                 'no error' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{receiver}"
        );
    }
}

#[test]
fn a_v_pattern_takes_the_three_set_operations_over_a_class() {
    // §22.2.1's `ClassSetExpression`. `--` is difference and `&&` is intersection, and a class
    // written inside a class is an operand — which is the whole of what the `v` flag reserved its
    // extra punctuation for.
    assert_eq!(run("/^[[0-9]--_]+$/v.test('019')"), "true");
    assert_eq!(run("/^[[0-9]--_]+$/v.test('_')"), "false");
    assert_eq!(run("/[[a-z]--[aeiou]]/v.test('b')"), "true");
    assert_eq!(run("/[[a-z]--[aeiou]]/v.test('a')"), "false");
    assert_eq!(run("/^[\\d&&[0-4]]+$/v.test('034')"), "true");
    assert_eq!(run("/^[\\d&&[0-4]]+$/v.test('5')"), "false");
    assert_eq!(
        run("/[\\w&&\\d]/v.test('5') + ',' + /[\\w&&\\d]/v.test('a')"),
        "true,false"
    );
    // A union needs no operator, and a nested class in one is just more operands.
    assert_eq!(
        run(
            "/[[a-c][x-z]]/v.test('b') + ',' + /[[a-c][x-z]]/v.test('y') + ',' \
             + /[[a-c][x-z]]/v.test('m')"
        ),
        "true,true,false"
    );
    // More than two operands, which each operation takes.
    assert_eq!(
        run("/[\\w&&[a-z]&&[a-c]]/v.test('b') + ',' + /[\\w&&[a-z]&&[a-c]]/v.test('d')"),
        "true,false"
    );
    assert_eq!(
        run("/[[a-z]--[aeiou]--[b-d]]/v.test('f') + ',' + /[[a-z]--[aeiou]--[b-d]]/v.test('c')"),
        "true,false"
    );
    // The **negation belongs to the class and the operation to what is inside it**, which is the
    // order it is written in: `[0-9]--[0-4]` is `{5..9}`, and the `^` takes everything else.
    assert_eq!(
        run("/^[^[0-9]--[0-4]]$/v.test('7') + ',' + /^[^[0-9]--[0-4]]$/v.test('2')"),
        "false,true"
    );
    // …and a nested `[^…]` is negated before the level above combines it, which is a different
    // question and a different answer.
    assert_eq!(
        run("/^[\\d&&[^0-4]]$/v.test('7') + ',' + /^[\\d&&[^0-4]]$/v.test('2')"),
        "true,false"
    );
    // Nesting goes as deep as it is written.
    assert_eq!(
        run(
            "/^[[[a-z]--[aeiou]]&&[a-f]]$/v.test('b') + ',' + /^[[[a-z]--[aeiou]]&&[a-f]]$/v.test('e')"
        ),
        "true,false"
    );
    // A property escape is an operand like any other, and `i` still does not reach one — §22.2.2.9
    // folds the pattern's literals and ranges and not its sets.
    assert_eq!(
        run("/[\\p{ASCII}&&[a-c]]/v.test('b') + ',' + /[\\p{ASCII}&&[a-c]]/v.test('z')"),
        "true,false"
    );
}

#[test]
fn a_u_pattern_reads_the_same_brackets_as_ordinary_characters() {
    // The one place `v` is not merely more capable than `u` but *different*, which is why §22.2.1
    // makes the two flags refuse each other. Every one of these is a class of characters under
    // `u` and a set expression under `v`.
    assert_eq!(run("/^[[]$/u.test('[')"), "true");
    assert_eq!(run("/^[a[b]+$/u.test('a[b')"), "true");
    assert_eq!(run("/^[&&]+$/u.test('&&')"), "true");
    // …and where `u` reads brackets as characters it also reads `]` as a closer, so the shapes a
    // `v` pattern nests are not merely different under `u` — several of them have no derivation
    // at all. `[[a-c]]` closes at the first `]` and leaves a second with nothing to match, and
    // `[a--b]` is the range `a` to `-`, which runs backwards.
    assert_eq!(
        run("try { new RegExp('[[a-c]]', 'u'); 'no error' } catch (e) { e.message }"),
        "a regular expression has an unmatched ]"
    );
    assert_eq!(
        run("try { new RegExp('[a--b]', 'u'); 'no error' } catch (e) { e.message }"),
        "a character class range runs backwards"
    );
    // The same text under `v` is a difference, and an empty one: `a` minus `b` still holds `a`,
    // and nothing there matches `-`.
    assert_eq!(
        run("/^[a--b]+$/v.test('a') + ',' + /^[a--b]+$/v.test('-')"),
        "true,false"
    );
    // …and `v` refuses the doubled punctuators it has not given a meaning to, which is what the
    // reservation was for.
    assert_eq!(
        run("try { new RegExp('[!!]', 'v'); 'no error' } catch (e) { e.message }"),
        "this punctuator is doubled, which a v pattern reserves inside a class"
    );
    assert_eq!(run("/^[!!]+$/u.test('!!')"), "true");
    // A plain range is still a range in a `v` pattern — §22.2.1 puts `ClassSetRange` in
    // `ClassUnion`, so it is the operation this class already is.
    assert_eq!(run("/^[a-z]+$/v.test('qed')"), "true");
    assert_eq!(run("/^[a-z0-9]+$/v.test('q3d')"), "true");
}

#[test]
fn the_two_set_operations_do_not_mix_and_neither_mixes_with_a_union() {
    // §22.2.1 gives `ClassIntersection` and `ClassSubtraction` separate productions and neither
    // admits the other, so one level is one operation and nesting is how to write both.
    for source in [
        "new RegExp('[\\\\d&&\\\\w--a]', 'v')",
        "new RegExp('[a--b&&c]', 'v')",
        "new RegExp('[a&&b--c]', 'v')",
        "new RegExp('[ab&&c]', 'v')",
        "new RegExp('[a&&bc]', 'v')",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; 'no error' }} catch (e) {{ e.constructor.name }}"
            )),
            "SyntaxError",
            "{source}"
        );
    }
    // …and nesting is how both are written together, which is the row that says the refusal above
    // is about the *level* rather than about the operations.
    assert_eq!(
        run("/^[[\\d&&[0-8]]--[0-4]]$/v.test('7') + ',' + /^[[\\d&&[0-8]]--[0-4]]$/v.test('9')"),
        "true,false"
    );
    // An operator with nothing on one side of it.
    assert_eq!(
        run("try { new RegExp('[a--]', 'v'); 'no error' } catch (e) { e.message }"),
        "a set operation needs an operand on both sides"
    );
    assert_eq!(
        run("try { new RegExp('[--a]', 'v'); 'no error' } catch (e) { e.constructor.name }"),
        "SyntaxError"
    );
    // §22.2.1's `[lookahead ≠ &]` on `&&`: a third ampersand is not an intersection with one after
    // it. **The message is the assertion**, because both readings refuse this and they refuse it
    // for different reasons — taking the first two as an operator leaves `&b` as two operands with
    // no separator, where declining to gives the doubled punctuator the reservation is for.
    assert_eq!(
        run("try { new RegExp('[a&&&b]', 'v'); 'no error' } catch (e) { e.message }"),
        "this punctuator is doubled, which a v pattern reserves inside a class"
    );
}

#[test]
fn a_class_that_would_match_strings_is_refused_by_name_and_not_as_bad_syntax() {
    // §22.2.1's `ClassStringDisjunction` — `\q{abc|def}`, an operand matching *strings* rather
    // than code points, and a matcher change rather than a parser one.
    //
    // The refusal is **unsupported** and not a syntax error, and the difference is the whole
    // reason this row exists: `\q{}` is a legal `v` operand, so calling it bad syntax would pass
    // every test asserting that a pattern must be rejected — a gap wearing a rule's clothes. Same
    // for `\p{RGI_Emoji}`, which is the other way a class comes to match more than one code point.
    assert_eq!(
        run("var p = '[' + String.fromCharCode(92) + 'q{abc}]'; \
             try { new RegExp(p, 'v'); 'no error' } catch (e) { e.message }"),
        "a class of strings"
    );
    assert_eq!(
        run("try { new RegExp('\\\\p{RGI_Emoji}', 'v'); 'no error' } catch (e) { e.message }"),
        "a property of strings"
    );
    // …and outside a `v` pattern `\q` is an ordinary escape question, which this must not have
    // changed: `u` refuses it as the syntax error it is there.
    assert_eq!(
        run("var p = '[' + String.fromCharCode(92) + 'q{abc}]'; \
             try { new RegExp(p, 'u'); 'no error' } catch (e) { e.message }"),
        "this character may not be escaped in a Unicode pattern"
    );
}
