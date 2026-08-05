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
    // A line terminator is escaped for the same reason — but one that is **already** escaped needs
    // the letter and not a second backslash. `\<LF>` is an identity escape of a newline and `\n` is
    // the escape sequence for one, so the two are the same pattern; writing `\\n` instead is a
    // literal backslash followed by an `n`, which is a *different* pattern that reads almost alike.
    // The `/` row above always asked this question and these four productions did not.
    assert_eq!(
        run(
            "var s = new RegExp('\\\\' + String.fromCharCode(10)).source; \
             s.length + ':' + s.charCodeAt(0) + ',' + s.charCodeAt(1)"
        ),
        "2:92,110"
    );
    assert_eq!(
        run(
            "var s = new RegExp('\\\\' + String.fromCharCode(13)).source; \
             s.length + ':' + s.charCodeAt(1)"
        ),
        "2:114"
    );
    // A bare one still gets its own backslash, which is the case that must not regress.
    assert_eq!(
        run("var s = new RegExp(String.fromCharCode(10)).source; \
             s.length + ':' + s.charCodeAt(0) + ',' + s.charCodeAt(1)"),
        "2:92,110"
    );
    // …and an escaped *backslash* clears the flag, so a newline after it is escaped in its own
    // right: the answer is four characters and not three.
    assert_eq!(
        run("new RegExp('\\\\\\\\' + String.fromCharCode(10)).source.length"),
        "4"
    );
    // U+2028 takes the same rule, spelled as a `\u` escape.
    assert_eq!(
        run("new RegExp('\\\\' + String.fromCharCode(0x2028)).source"),
        "\\u2028"
    );
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
        // `\1` naming no group is not one of these: §B.1.2 reads it as a legacy octal escape.
        "new RegExp('(?<n>a)\\\\k<m>')",
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
fn a_class_may_match_strings_and_the_operations_apply_to_them_too() {
    // §22.2.1's `ClassStringDisjunction` — `\q{abc|def}`, the one operand that matches *strings*
    // rather than code points, so a class stops being a predicate on one character.
    assert_eq!(
        run(
            r"[/^[\q{abc|def}]$/v.test('abc'), /^[\q{abc|def}]$/v.test('def'),               /^[\q{abc|def}]$/v.test('abd')].join(',')"
        ),
        "true,true,false"
    );
    // An alternative exactly **one** code point long is an ordinary member of the character set,
    // not a string. That is not a shortcut: it is what makes `[[0-9]--\q{0|2|4}]` remove three
    // digits, and what makes `[^\q{a}]` a legal class where `[^\q{ab}]` is not.
    assert_eq!(
        run(
            r"[/^[\q{a|bc}]$/v.test('a'), /^[\q{a|bc}]$/v.test('bc'), /^[\q{a|bc}]$/v.test('b')].join(',')"
        ),
        "true,true,false"
    );
    assert_eq!(
        run(
            r"var re = /^[[0-9]--\q{0|2|4}]+$/v; [re.test('1357'), re.test('0'), re.test('24')].join(',')"
        ),
        "true,false,false"
    );
    // §22.2.2.7.2 step 1 tries the candidates **longest first** and offers each to the
    // continuation in turn — a backtracking choice and not a longest-match rule. `ab` is tried,
    // fails for want of a following `b`, and `a` is tried after it.
    assert_eq!(
        run(r"[/^[\q{ab|a}]b$/v.test('ab'), /^[\q{ab|a}]b$/v.test('abb')].join(',')"),
        "true,true"
    );
    // The three operations apply to the *strings* as well, and they are computable where the code
    // points are not: a string set is finite and written down. An operand with no strings — a
    // range, a class escape — contributes none, which is why an intersection with one is empty.
    assert_eq!(
        run(
            r"[/^[\q{ab}&&\q{ab|cd}]$/v.test('ab'), /^[\q{ab}&&[a-z]]$/v.test('ab'),               /^[\q{ab|cd}--\q{ab}]$/v.test('cd'), /^[\q{ab|cd}--\q{ab}]$/v.test('ab')].join(',')"
        ),
        "true,false,true,false"
    );
    // `\q{}` is one **empty** alternative rather than none, and it sorts last: after every longer
    // candidate and after the ordinary character read.
    assert_eq!(
        run(r"[/^[\q{}]$/v.test(''), /^a[\q{}]b$/v.test('ab')].join(',')"),
        "true,true"
    );
    // §22.2.1's `ClassSetCharacter` admits `\b` in here, and it is a **backspace** — the one escape
    // whose meaning changes at a class boundary, which is why this reader cannot defer to the one
    // that reads an escape outside a class. The sequence is three code points and not two.
    assert_eq!(
        run("var re = /^[\\q{a\\bc}]$/v; \
             [re.test('a' + String.fromCharCode(8) + 'c'), re.test('ac'), re.test('abc')].join(',')"),
        "true,false,false"
    );
    // …and every *other* escape in there is the ordinary one, which is the half that says `\b` is
    // a special case rather than the rule: reading them all as a backspace passes the row above
    // and turns `\u0041` into a backspace followed by five literal characters.
    assert_eq!(
        run(
            "var re = /^[\\q{a\\u0041c}]$/v; [re.test('aAc'), re.test('a' + String.fromCharCode(8) + 'c')].join(',')"
        ),
        "true,false"
    );
    // The rest of a `v` class's reservation holds in here too: a syntax character has to be
    // written escaped, so `\q{(}` is refused and `\q{\(}` is a parenthesis. Without the check the
    // parenthesis would be taken as an ordinary character and the pattern would quietly mean
    // something the grammar does not allow.
    assert_eq!(
        run(
            r"var why = function (p) { try { new RegExp(p, 'v'); return 'accepted' }                                       catch (e) { return e.constructor.name } };               [why('[\\q{(}]'), why('[\\q{\\(}]')].join(',')"
        ),
        "SyntaxError,accepted"
    );
    // §22.2.2.9 canonicalizes each character of a sequence, so `i` folds a string as it folds a
    // literal.
    assert_eq!(
        run(r"[/[\q{AB}]/vi.test('ab'), /[\q{ab}]/v.test('AB')].join(',')"),
        "true,false"
    );
    // §22.2.1 — `[^…]` is refused when its contents `MayContainStrings`, and that is a *syntactic*
    // question: a difference of two identical string operands is refused although it resolves to
    // nothing, and an intersection with a code-point operand is accepted although its first
    // operand could. Reading it as "is the resolved set non-empty" gets both backwards.
    assert_eq!(
        run(
            r"var why = function (p) { try { new RegExp(p, 'v'); return 'accepted' }                                        catch (e) { return e.constructor.name } };               [why('[^\\q{ab}]'), why('[^\\q{a}]'), why('[^\\q{}]'),                why('[^[\\q{ab}--\\q{ab}]]'), why('[^[\\q{ab}&&[a]]]')].join(',')"
        ),
        "SyntaxError,accepted,SyntaxError,SyntaxError,accepted"
    );
    // `\p{RGI_Emoji}` is the other way a class comes to match more than one code point, and it is
    // still refused **by name** rather than as bad syntax: it is a legal operand, so calling it a
    // syntax error would pass every test asserting a pattern must be rejected — a gap wearing a
    // rule's clothes. It needs the Unicode sequence data, which `\q{}` does not.
    assert_eq!(
        run("try { new RegExp('\\\\p{RGI_Emoji}', 'v'); 'no error' } catch (e) { e.message }"),
        "a property of strings"
    );
    // …and outside a `v` pattern `\q` is an ordinary escape question, which this must not have
    // changed: `u` refuses it as the syntax error it is there.
    assert_eq!(
        run(
            "var p = '[' + String.fromCharCode(92) + 'q{abc}]';              try { new RegExp(p, 'u'); 'no error' } catch (e) { e.message }"
        ),
        "this character may not be escaped in a Unicode pattern"
    );
}

#[test]
fn several_groups_may_wear_one_name_and_the_one_that_matched_answers_for_it() {
    // §22.2.1.1 lets a name be reused across alternatives, and each group keeps a **capture index
    // of its own** — the array still has one entry per `(`, in source order, with `undefined` for
    // the alternative the match did not take.
    assert_eq!(
        run("JSON.stringify(/(?<x>a)|(?<x>b)/.exec('bab'))"),
        "[\"b\",null,\"b\"]"
    );
    assert_eq!(
        run("JSON.stringify(/(?<x>b)|(?<x>a)/.exec('bab'))"),
        "[\"b\",\"b\",null]"
    );
    // §22.2.7.2 step 34's `groups` gets **one** property per distinct name, holding whichever
    // group took part. Defining it once per group would let the alternative that did not match
    // overwrite the one that did, and the answer would depend on which came last in the source.
    assert_eq!(run("/(?<x>a)|(?<x>b)/.exec('bab').groups.x"), "b");
    assert_eq!(run("/(?<x>b)|(?<x>a)/.exec('bab').groups.x"), "b");
    assert_eq!(run("String(/(?<x>a)|(?<x>b)/.exec('cb').groups.x)"), "b");
    // The property is created where the name is **first written**, which is the enumeration order
    // the clause produces and is observable through `Object.keys`.
    assert_eq!(
        run("Object.keys(/(?<b>1)|(?<a>2)|(?<b>3)/.exec('2').groups).join(',')"),
        "b,a"
    );
    // …and a name none of whose groups took part is present and `undefined`, not absent. That is
    // the difference between "this pattern has no such group" and "it has one and it did not run".
    assert_eq!(
        run("var g = /(?<x>a)|(?<x>b)|z/.exec('z').groups; ('x' in g) + '|' + g.x"),
        "true|undefined"
    );
}

#[test]
fn a_backreference_to_a_shared_name_reads_whichever_group_took_part() {
    // §22.2.2.9 — the reference is to *every* group of that name, and §22.2.1.1 has already made
    // sure at most one can have a capture. Reading the first group wearing the name instead would
    // find the empty capture of the alternative the match did not take, and a backreference to a
    // group that did not participate matches the **empty string** — so `\k<x>` would succeed
    // against anything at all.
    assert_eq!(
        run("JSON.stringify(/(?:(?<x>a)|(?<x>b))\\k<x>/.exec('aa'))"),
        "[\"aa\",\"a\",null]"
    );
    assert_eq!(
        run("JSON.stringify(/(?:(?<x>a)|(?<x>b))\\k<x>/.exec('bb'))"),
        "[\"bb\",null,\"b\"]"
    );
    assert_eq!(
        run("String(/(?:(?<x>a)|(?<x>b))\\k<x>/.exec('abab'))"),
        "null"
    );
    // A repeat re-runs the pair, and the captures the last turn left are what `\k<x>` reads —
    // which is why this matches and answers with the *second* turn's group.
    assert_eq!(
        run("JSON.stringify(/(?:(?:(?<x>a)|(?<x>b))\\k<x>){2}/.exec('aabb'))"),
        "[\"aabb\",null,\"b\"]"
    );
    assert_eq!(
        run("String(/(?:(?:(?<x>a)|(?<x>b))\\k<x>){2}/.exec('abab'))"),
        "null"
    );
    // An alternative that names nothing leaves every group of that name out, and the reference
    // then matches the empty string — so `"z"` matches and `"zz"` does not.
    assert_eq!(
        run("JSON.stringify(/^(?:(?<a>x)|(?<a>y)|z)\\k<a>$/.exec('z'))"),
        "[\"z\",null,null]"
    );
    assert_eq!(
        run("String(/^(?:(?<a>x)|(?<a>y)|z)\\k<a>$/.exec('zz'))"),
        "null"
    );
    // A reference written in a *different* alternative from the only group of that name is
    // reachable and reads nothing, which the clause allows rather than refusing at parse time.
    assert_eq!(
        run("JSON.stringify(/(?<a>x)|(?:zy\\k<a>)/.exec('zy'))"),
        "[\"zy\",null]"
    );
    // `$<name>` in a replacement reads the same property, so it follows from `groups` with no
    // second rule of its own.
    assert_eq!(run("'b'.replace(/(?<x>a)|(?<x>b)/, '[$<x>]')"), "[b]");
}

#[test]
fn the_d_flag_says_where_each_capture_began_and_ended() {
    // §22.2.7.8 `MakeMatchIndicesIndexPairArray` — the same shape as the match array beside it and
    // built from the same spans, but an element is a two-element `[start, end]` rather than the
    // text. Nothing here reads the subject at all.
    assert_eq!(
        run("JSON.stringify(/b(c)/d.exec('abcd').indices)"),
        "[[1,3],[2,3]]"
    );
    // §22.2.7.9 `GetMatchIndexPair` makes an ordinary Array, so a script may treat a pair like any
    // other — which is what its prototype being `Array.prototype` means.
    assert_eq!(
        run("var i = /b(c)/d.exec('abcd').indices; \
             (Object.getPrototypeOf(i[0]) === Array.prototype) + '|' + i[0].length"),
        "true|2"
    );
    // A capture that did not take part is `undefined` and **present**: an empty match and an absent
    // one both have a zero-length span, so only the record can tell them apart.
    assert_eq!(
        run("var i = /a(b)?/d.exec('a').indices; String(i[1]) + '|' + (1 in i)"),
        "undefined|true"
    );
    // Without the flag there is no array at all, which is what `hasIndices` is for.
    assert_eq!(run("String(/a/.exec('a').indices)"), "undefined");
    assert_eq!(run("/a/d.hasIndices + '|' + /a/.hasIndices"), "true|false");
}

#[test]
fn the_indices_array_names_its_groups_the_same_way_the_match_does() {
    // Step 5 and step 6 — `groups` is on the indices array whether or not the pattern names
    // anything, and is `undefined` when it names nothing. So `'groups' in indices` is true for every
    // pattern, which is the promise the match array beside it makes too.
    assert_eq!(
        run("JSON.stringify(/(?<x>.)/d.exec('a').indices.groups)"),
        "{\"x\":[0,1]}"
    );
    assert_eq!(run("String(/a/d.exec('a').indices.groups)"), "undefined");
    assert_eq!(run("'groups' in /a/d.exec('a').indices"), "true");
    // §22.2.1.1 lets several groups share a name, and the answer is whichever took part — asked the
    // same way the match array asks it rather than a second time with a second rule.
    assert_eq!(
        run("JSON.stringify(/(?<x>a)|(?<x>b)/d.exec('b').indices.groups)"),
        "{\"x\":[0,1]}"
    );
    // A named group that did not participate is `undefined` here as well.
    assert_eq!(
        run("String(/(?<x>a)|b/d.exec('b').indices.groups.x)"),
        "undefined"
    );
}

#[test]
fn a_match_results_extra_properties_are_the_ones_a_script_could_have_written() {
    // §22.2.7.2 builds these with `CreateDataPropertyOrThrow`, so all three attributes are true —
    // **including enumerable**, which ViperJS had as `false` for every one of them. The difference
    // is not academic: with the installation attributes instead, `Object.keys` of a match answers
    // only its indices and a `for`-`in` over one finds no `index` at all.
    assert_eq!(
        run("Object.keys(/(?<x>a)/d.exec('a')).join(',')"),
        "0,1,index,input,groups,indices"
    );
    assert_eq!(
        run("Object.keys(/a/.exec('a')).join(',')"),
        "0,index,input,groups"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(/(?<x>.)/d.exec('a').indices, 'groups'); \
             d.writable + '|' + d.enumerable + '|' + d.configurable"
        ),
        "true|true|true"
    );
}

#[test]
fn a_property_of_strings_is_refused_by_the_specification_in_three_of_its_four_positions() {
    // §22.2.1's early errors. A property of strings names a set whose members may be longer than
    // one code point, and three positions cannot be given a meaning: negated with `\P`, outside a
    // `v` pattern, and inside a negated class. Each is the **specification** refusing, so each is a
    // real answer about the text.
    //
    // The fourth is legal and unbuilt, and stays a gap. Recording a gap as an early error passes
    // every test asserting the construct must be rejected — which is exactly these three — and
    // recording an early error as a gap loses them. The split is the whole slice.
    //
    // Every row asserts the **message** and not merely the constructor. A first draft asked only
    // for `SyntaxError`, and three of its four rows passed against a pattern whose backslashes the
    // test's own escaping had eaten — a malformed pattern is a SyntaxError too.
    assert_eq!(
        refused_pattern(r"\p{RGI_Emoji}", "u"),
        "a property of strings needs the v flag"
    );
    assert_eq!(
        refused_pattern(r"\P{RGI_Emoji}", "v"),
        "a property of strings may not be negated with \\P"
    );
    assert_eq!(
        refused_pattern(r"[^\p{RGI_Emoji}]", "v"),
        "a negated class may not contain a property of strings"
    );
    // Nesting: the inner class is inside a negated one whether or not it is negated itself, which
    // is why the reader counts them rather than keeping a flag.
    assert_eq!(
        refused_pattern(r"[^[\p{RGI_Emoji}]]", "v"),
        "a negated class may not contain a property of strings"
    );
    // …and a class that is *not* negated leaves the count where it was, so the one legal position
    // is still reached rather than being refused by a stale flag.
    // The gap, worded as the gap. `new RegExp` reports the message alone where a *literal* goes
    // through the compiler and gets `ErrorKind::Unsupported`'s "is not implemented yet" around it —
    // two spellings of one refusal, and this row is the run-time one.
    assert_eq!(
        refused_pattern(r"[[\p{RGI_Emoji}]]", "v"),
        "a property of strings"
    );
    // A negated class that has **closed** no longer encloses anything, so a legal property after
    // one is still legal. That is the half a count gets wrong by never coming back down, and it
    // would refuse a pattern the specification allows rather than accepting one it forbids.
    assert_eq!(
        refused_pattern(r"[^a]\p{RGI_Emoji}", "v"),
        "a property of strings"
    );
    // An ordinary property is unaffected in every one of those positions, which is what keeps this
    // about *properties of strings* rather than about property escapes.
    assert_eq!(
        run(r"/\p{L}/u.test('a') + '|' + /\P{L}/u.test('1')"),
        "true|true"
    );
    assert_eq!(run(r"/[^\p{L}]/v.test('1')"), "true");
    assert_eq!(run(r"/[[\p{L}]]/v.test('a')"), "true");
}

/// What `new RegExp(source, flags)` refused this pattern with.
///
/// The message and not the constructor, because every one of these is a `SyntaxError` and so is a
/// pattern the test itself mangled — see the row above that found exactly that.
fn refused_pattern(source: &str, flags: &str) -> String {
    let escaped = source.replace('\\', "\\\\");
    run(&format!(
        "try {{ new RegExp('{escaped}', '{flags}'); 'accepted' }} catch (e) {{ e.message }}"
    ))
}

#[test]
fn a_pattern_can_be_escaped_so_that_it_matches_itself() {
    // §22.2.5.2 — the four kinds of escape, in the order `EncodeForRegExpEscape` decides them. A
    // tab is Table 64's `\t` and not `\x09`, which is what makes the order load-bearing rather
    // than tidy.
    assert_eq!(
        run(
            r"[RegExp.escape('.'), RegExp.escape('\\'), RegExp.escape('/'), RegExp.escape('\t'), RegExp.escape(',')].join(' ')"
        ),
        r"\. \\ \/ \t \x2c"
    );
    // Steps 3 to 5 — whitespace and line terminators, two hex digits while the code point fits in
    // a byte and four when it does not. Reading the boundary the other way writes ` `, which
    // matches the same character and is not what the clause says.
    assert_eq!(
        run(
            r"[RegExp.escape(' '), RegExp.escape(' '), RegExp.escape(' '), RegExp.escape('﻿'), RegExp.escape(' ')].join(' ')"
        ),
        "\\x20 \\xa0 \\u202f \\ufeff \\u2028"
    );
    // Step 4.a is about **position** and not about the character: an ASCII letter or a digit is
    // escaped only where it would begin the answer, so the second `B` here is written as itself.
    // A rule read as "escape every letter" gives `\x42\*\x42` and passes no test at all.
    assert_eq!(
        run(r"[RegExp.escape('B*B'), RegExp.escape('0'), RegExp.escape('.a1b2')].join(' ')"),
        r"\x42\*B \x30 \.a1b2"
    );
    // A **lone** surrogate is escaped; a pair is one code point and passes through whole. That is
    // the difference between walking code units and walking code points, and it is the only place
    // in this function where it shows.
    assert_eq!(
        run(
            r"[RegExp.escape('\uD800'), RegExp.escape('\uDC20'), RegExp.escape('\u{1F600}'), RegExp.escape('퟿')].join(' ')"
        ),
        "\\ud800 \\udc20 \u{1F600} \u{D7FF}"
    );
    // Nearly all of Unicode is written as itself — this is not an ASCII-safe encoder.
    assert_eq!(
        run("RegExp.escape('\u{4F60}\u{597D}!')"),
        "\u{4F60}\u{597D}\\x21"
    );
    // Step 1 refuses a non-String **without coercing it**, which is unusual and is the point: an
    // answer that is safe to concatenate is worth nothing if the input was silently stringified.
    assert_eq!(
        run(
            "var out = []; [123, {}, [], null, undefined].forEach(function (v) { \
             try { RegExp.escape(v); out.push('accepted') } catch (e) { out.push(e.constructor.name) } }); \
             out.join(',')"
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError"
    );
    // And what it answers really does match what went in, which no assertion above establishes.
    assert_eq!(
        run(
            "var raw = 'a.b*c[d]'; new RegExp(RegExp.escape(raw)).test(raw) + ',' + new RegExp(RegExp.escape(raw)).test('axbxcxdx')"
        ),
        "true,false"
    );
}
