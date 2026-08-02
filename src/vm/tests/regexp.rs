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
