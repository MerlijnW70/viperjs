//! §22.1.3's five pattern methods — `replace`, `replaceAll`, `match`, `matchAll` and `search`.
//!
//! Each hands the whole operation to a well-known Symbol method on its argument when there is one.
//! That is the seam regular expressions will arrive through, and it is open to anything else that
//! supplies the method — which is why most of what is testable here is testable without `RegExp`.

use super::*;

#[test]
fn replace_changes_the_first_occurrence_and_replace_all_changes_every_one() {
    assert_eq!(run("'abcabc'.replace('b', 'X')"), "aXcabc");
    assert_eq!(run("'abcabc'.replaceAll('b', 'X')"), "aXcaXc");
    // A search value that is not there leaves the string alone rather than erroring.
    assert_eq!(run("'abc'.replace('x', 'X')"), "abc");
    assert_eq!(run("'abc'.replaceAll('x', 'X')"), "abc");
    // Matches do not overlap: after `aa` is taken from `aaa` there is one `a` left, not two.
    assert_eq!(run("'aaa'.replaceAll('aa', 'X')"), "Xa");
    // Both convert their arguments, so a number searches and replaces as its digits.
    assert_eq!(run("'a1b1'.replaceAll(1, 2)"), "a2b2");
}

#[test]
fn an_empty_search_value_matches_between_every_pair_of_units_and_terminates() {
    // The case that separates a correct `replaceAll` from one that loops: an empty needle is found
    // at every position *including* the end, so the advance has to be by one unit rather than by
    // the length of the match.
    assert_eq!(run("'abc'.replaceAll('', '-')"), "-a-b-c-");
    assert_eq!(run("''.replaceAll('', '-')"), "-");
    assert_eq!(run("'abc'.replace('', '-')"), "-abc");
}

#[test]
fn a_replacement_function_is_handed_the_match_its_position_and_the_whole_string() {
    assert_eq!(
        run("'abc'.replace('b', function (m, i, s) { return '[' + m + i + s + ']'; })"),
        "a[b1abc]c"
    );
    // §22.1.3.19 step 14.a — a function's answer is used **as written**, so a `$` in it is not a
    // substitution form. That is the whole difference between the two kinds of replacement.
    assert_eq!(
        run("'abc'.replace('b', function () { return '$&'; })"),
        "a$&c"
    );
    assert_eq!(
        run(
            "var seen = []; 'aXaXa'.replaceAll('X', function (m, i) { seen.push(i); return '-'; }); seen.join()"
        ),
        "1,3"
    );
    // §22.1.3.19 step 14.a passes `undefined` as the receiver, not the string. Visible only in a
    // strict function: a sloppy one has the global substituted for it by §10.2.1.2, so the
    // obvious spelling of this test would pass whatever was passed.
    assert_eq!(
        run("'a'.replace('a', function () { 'use strict'; return typeof this; })"),
        "undefined"
    );
    assert_eq!(
        run("'a'.replace('a', function () { return this === globalThis; })"),
        "true"
    );
}

#[test]
fn the_dollar_forms_in_a_replacement_template_read_around_the_match() {
    assert_eq!(run("'abc'.replace('b', '<$&>')"), "a<b>c");
    assert_eq!(run("'abc'.replace('b', '<$`>')"), "a<a>c");
    assert_eq!(run("'abc'.replace('b', \"<$'>\")"), "a<c>c");
    assert_eq!(run("'abc'.replace('b', '<$$>')"), "a<$>c");
    // Anything else after a `$` is left exactly as written — a rule about not erroring.
    assert_eq!(run("'abc'.replace('b', '$x')"), "a$xc");
    assert_eq!(run("'abc'.replace('b', '$')"), "a$c");
    // With no captures there is no `$1` to read, so it too stays literal.
    assert_eq!(run("'abc'.replace('b', '$1')"), "a$1c");
    assert_eq!(run("'abc'.replace('b', '$<name>')"), "a$<name>c");
}

#[test]
fn a_pattern_with_a_symbol_method_is_handed_the_whole_operation() {
    // §22.1.3.19 step 2 — and it is handed the receiver **unconverted**, which is what lets a
    // pattern do something other than work on characters.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.replace] = function (s, r) { return 'took ' + s + ' ' + r; }; \
             'here'.replace(o, 'with')"
        ),
        "took here with"
    );
    assert_eq!(
        run("var o = {}; o[Symbol.search] = function (s) { return s.length; }; 'abcd'.search(o)"),
        "4"
    );
    assert_eq!(
        run("var o = {}; o[Symbol.match] = function () { return 'matched'; }; 'x'.match(o)"),
        "matched"
    );
    // The method is looked for *on* the argument, so it may be inherited.
    assert_eq!(
        run(
            "function P() {} P.prototype[Symbol.replace] = function () { return 'from prototype'; }; \
             'x'.replace(new P(), 'y')"
        ),
        "from prototype"
    );
    // §7.3.11 — `undefined` and null both mean "no method", and the string path is taken instead.
    assert_eq!(
        run(
            "var o = {toString: function () { return 'b'; }}; o[Symbol.replace] = null; 'abc'.replace(o, 'X')"
        ),
        "aXc"
    );
    // …and anything else that is not callable is a TypeError rather than a quiet fall through.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.replace] = 5; try { 'x'.replace(o, 'y') } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn replace_all_and_match_all_demand_a_global_pattern() {
    // §22.1.3.20 step 2.b — replacing *all* of something with a pattern that stops at the first
    // match could not do what was asked, so it is refused before the delegate is even looked for.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.match] = true; try { 'x'.replaceAll(o, 'y') } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var o = {}; o[Symbol.match] = true; try { 'x'.matchAll(o) } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // With a `g` among the flags it goes through.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; o.flags = 'g'; \
             o[Symbol.replace] = function () { return 'ok'; }; 'x'.replaceAll(o, 'y')"),
        "ok"
    );
    // §7.2.8 — a *falsy* `Symbol.match` means it is not a pattern, so no flag is demanded and the
    // ordinary string path runs.
    assert_eq!(
        run(
            "var o = {toString: function () { return 'b'; }}; o[Symbol.match] = false; 'abc'.replaceAll(o, 'X')"
        ),
        "aXc"
    );
    // A pattern whose `flags` are `undefined` is refused before they can be spelled.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; o.flags = undefined; \
             try { 'x'.replaceAll(o, 'y') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // `replace` makes no such demand — it only ever changes one.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; \
             o[Symbol.replace] = function () { return 'ok'; }; 'x'.replace(o, 'y')"),
        "ok"
    );
}

#[test]
fn the_three_that_need_a_pattern_make_one_out_of_what_they_were_given() {
    // §22.1.3.14, .15 and .21 all end "let rx be `RegExpCreate(argument)`, and invoke its Symbol
    // method". So a *string* argument becomes a pattern rather than being searched for as text,
    // which is why `"a.c".match(".")` finds `a` and not the dot.
    assert_eq!(run("'abc'.match('b')[0]"), "b");
    assert_eq!(run("'abc'.match('.')[0]"), "a");
    assert_eq!(run("'abc'.search('c')"), "2");
    assert_eq!(run("'abc'.search('z')"), "-1");
    assert_eq!(run("String('abc'.match('z'))"), "null");
    // An absent argument makes the *empty* pattern, which matches at once.
    assert_eq!(run("'abc'.match().index + ',' + 'abc'.search()"), "0,0");
    // …and the receiver is still checked first, so `null` is refused for being `null`.
    assert_eq!(
        run("try { String.prototype.match.call(null, {}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn each_of_the_five_refuses_a_receiver_that_cannot_be_coerced() {
    // §22.1.3's `RequireObjectCoercible` is **step 1**, before the pattern is asked for anything,
    // so a nullish receiver is refused whatever the argument is. This row used to say the opposite
    // — "a pattern that handles everything never sees the refusal" — and had a case asserting it.
    // What is true of the dispatch is that it comes before the **conversion**, which is a different
    // step and is checked below.
    for source in [
        "String.prototype.replace.call(null, 'a', 'b')",
        "String.prototype.replaceAll.call(undefined, 'a', 'b')",
        "String.prototype.search.call(null, {})",
        // …and with a pattern that would have taken over, which is the case that was wrong.
        "var o = {}; o[Symbol.replace] = function () { return 1; }; \
         String.prototype.replace.call(null, o, 'x')",
        "var o = {}; o[Symbol.match] = function () { return 1; }; \
         String.prototype.match.call(undefined, o)",
        "var o = {}; o[Symbol.split] = function () { return 1; }; \
         String.prototype.split.call(null, o)",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; 'no throw' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source} should refuse its receiver"
        );
    }
    // A pattern that takes over never reaches the **conversion**, which is the half that is true:
    // the receiver has to be coercible and is then handed over *unconverted*, so a `toString` that
    // throws never runs and the pattern sees the object itself.
    assert_eq!(
        run(
            "var seen; var bad = { toString: function () { throw new RangeError('ran'); } }; \
             var o = {}; o[Symbol.replace] = function (s) { seen = s === bad; return 'ok'; }; \
             String.prototype.replace.call(bad, o, 'x') + ',' + seen"
        ),
        "ok,true"
    );
    // …and with no pattern to take over, that same receiver reaches the conversion and throws its
    // own error rather than this clause's TypeError. Those two rows together are what pin the
    // order: one of them alone passes with the conversion on either side of the dispatch.
    assert_eq!(
        run(
            "var bad = { toString: function () { throw new RangeError('ran'); } }; \
             try { String.prototype.replace.call(bad, 'a', 'b') } catch (e) { e.message }"
        ),
        "ran"
    );
}

#[test]
fn the_five_have_the_names_and_arities_the_specification_gives_them() {
    assert_eq!(
        run("['match', 'matchAll', 'replace', 'replaceAll', 'search']\
             .map(function (n) { return n + ':' + String.prototype[n].length; }).join()"),
        "match:1,matchAll:1,replace:2,replaceAll:2,search:1"
    );
    assert_eq!(
        run("String.prototype.replaceAll.name + ',' + typeof String.prototype.matchAll"),
        "replaceAll,function"
    );
}

#[test]
fn a_primitive_is_never_a_pattern_however_its_prototype_is_written() {
    // §7.2.8 step 1 — `IsRegExp` is asked of an *object*, so a `Symbol.match` inherited by a number
    // does not make the number one. Without that step `"a1".replaceAll(1, 2)` would start demanding
    // a `g` flag of a number.
    assert_eq!(
        run("Number.prototype[Symbol.match] = true; 'a1b1'.replaceAll(1, 2)"),
        "a2b2"
    );
    assert_eq!(
        run("String.prototype[Symbol.match] = true; 'abcabc'.replaceAll('b', 'X')"),
        "aXcaXc"
    );
}

#[test]
fn an_absent_search_value_is_searched_for_as_the_word_undefined() {
    // §22.1.3.19 step 3 — `undefined` and null skip the delegation and are then *converted*, so
    // they search for their own spelling. Surprising, and it is what the specification says.
    assert_eq!(run("'aundefinedb'.replace(undefined, 'X')"), "aXb");
    assert_eq!(run("'aundefinedb'.replaceAll(undefined, 'X')"), "aXb");
    assert_eq!(run("'anullb'.replaceAll(null, 'X')"), "aXb");
    assert_eq!(run("'abc'.replaceAll(undefined, 'X')"), "abc");
}

#[test]
fn the_two_ways_a_pattern_fails_the_global_demand_are_reported_apart() {
    // §22.1.3.20 step 2.b.ii is `RequireObjectCoercible(flags)` and step 2.b.iii is the search for
    // `g`. Both end in a TypeError, so only the message says which — and a pattern whose `flags`
    // are missing is a different mistake from one that simply is not global.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; \
             try { 'x'.replaceAll(o, 'y') } catch (e) { e.message }"),
        "a pattern given to replaceAll must have flags"
    );
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; o.flags = 'i'; \
             try { 'x'.replaceAll(o, 'y') } catch (e) { e.message }"),
        "a pattern given to replaceAll must be global"
    );
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; try { 'x'.matchAll(o) } catch (e) { e.message }"),
        "a pattern given to matchAll must have flags"
    );
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; o.flags = 'i'; \
             try { 'x'.matchAll(o) } catch (e) { e.message }"),
        "a pattern given to matchAll must be global"
    );
}

#[test]
fn match_all_delegates_on_the_same_terms_replace_all_does() {
    // The two clauses are written alike and were implemented alike, so each of `matchAll`'s branches
    // needs saying as well: `replaceAll` passing is no evidence about this one.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = true; o.flags = 'g'; \
             o[Symbol.matchAll] = function (s) { return 'saw ' + s; }; 'here'.matchAll(o)"),
        "saw here"
    );
    // A falsy `Symbol.match` is not a pattern, so no flag is demanded — and `matchAll` then has
    // nothing to make a RegExp out of and says so.
    assert_eq!(
        run("var o = {}; o[Symbol.match] = false; \
             o[Symbol.matchAll] = function () { return 'delegated'; }; 'x'.matchAll(o)"),
        "delegated"
    );
    // §22.1.3.15 step 3.c — the pattern this makes is **global** whatever it was handed, so an
    // absent argument becomes `/(?:)/g` and matches between every pair of characters.
    assert_eq!(
        run("Array.from('ab'.matchAll(undefined), function (m) { return m.index; }).join()"),
        "0,1,2"
    );
    // `null` is *not* `undefined` here: it skips the delegation and is then converted, so the
    // pattern is `/null/g` and matches the word. Only `undefined` means "no pattern".
    assert_eq!(run("Array.from('ab'.matchAll(null)).length"), "0");
    assert_eq!(
        run("Array.from('xnully'.matchAll(null), function (m) { return m.index; }).join()"),
        "1"
    );
}

#[test]
fn a_symbol_method_that_is_not_callable_is_reported_as_that_and_not_as_a_failed_call() {
    // §7.3.11 `GetMethod` throws for a present-but-uncallable method rather than calling it and
    // letting the call fail. Both are TypeErrors, so the message is the only thing that says which
    // — and calling it would run whatever a getter on the way there did.
    for (method, name) in [
        ("Symbol.replace", "replace(o, 'y')"),
        ("Symbol.search", "search(o)"),
        ("Symbol.match", "match(o)"),
    ] {
        assert_eq!(
            run(&format!(
                "var o = {{}}; o[{method}] = 5; try {{ 'x'.{name} }} catch (e) {{ e.message }}"
            )),
            "this pattern's method is not a function",
            "{method} that is not callable should be reported as such"
        );
    }
}

#[test]
fn a_primitive_pattern_never_has_its_symbol_method_looked_up() {
    // §22.1.3's six pattern-taking methods reach for `%Symbol.match%` and its siblings only when
    // the pattern **is an Object** — a 2025 normative change from "neither undefined nor null".
    // The difference is observable because `GetMethod` on a primitive goes through `ToObject`, so
    // the lookup would land on a wrapper prototype a script can install a getter on.
    //
    // One row per method, each poisoning the key on the prototype the primitive would convert to,
    // and each asserting the method still did its ordinary work.
    let poison = |kind: &str, symbol: &str| {
        format!(
            "Object.defineProperty({kind}.prototype, Symbol.{symbol}, \
             {{get: function () {{ throw new Test262Error('should not be called') }}, \
               configurable: true}}); "
        )
    };
    assert_eq!(
        run(&format!(
            "{}var m = 'a1b1c'.match(1); m.index + ',' + m.input + ',' + m[0]",
            poison("Number", "match")
        )),
        "1,a1b1c,1"
    );
    assert_eq!(
        run(&format!(
            "{}'a-b-c'.split('-').join('|')",
            poison("String", "split")
        )),
        "a|b|c"
    );
    assert_eq!(
        run(&format!(
            "{}'a1b'.replace(1, 'X')",
            poison("Number", "replace")
        )),
        "aXb"
    );
    assert_eq!(
        run(&format!(
            "{}'a1b1'.replaceAll(1, 'X')",
            poison("Number", "replace")
        )),
        "aXbX"
    );
    assert_eq!(
        run(&format!("{}'a1b'.search(1)", poison("Number", "search"))),
        "1"
    );
    assert_eq!(
        run(&format!(
            "{}Array.from('a1b1'.matchAll(1)).length",
            poison("Number", "matchAll")
        )),
        "2"
    );
    // A boolean and a BigInt convert to their own prototypes, which is the same rule and two more
    // objects a script could have written to.
    assert_eq!(
        run(&format!(
            "{}'atrueb'.replace(true, 'X')",
            poison("Boolean", "replace")
        )),
        "aXb"
    );
    assert_eq!(
        run(&format!(
            "{}'a1b'.replace(1n, 'X')",
            poison("BigInt", "replace")
        )),
        "aXb"
    );
    // …and `undefined` and null were never asked either, which the old condition already had
    // right and this must not have broken.
    assert_eq!(run("'aundefinedb'.split(undefined).length"), "1");
    assert_eq!(run("'a'.replace(undefined, 'X')"), "a");
    assert_eq!(run("String('anullb'.replace(null, 'X'))"), "aXb");
}

#[test]
fn an_object_pattern_is_still_asked_and_is_still_obeyed() {
    // The other side of the rule, which is the whole reason the methods delegate at all: an
    // **Object** with the symbol takes over the operation entirely, and sees an unconverted `this`.
    assert_eq!(
        run("'ignored'.match({[Symbol.match]: function (s) { return 'took ' + s }})"),
        "took ignored"
    );
    assert_eq!(
        run("'ignored'.replace({[Symbol.replace]: function (s, w) { return s + '/' + w }}, 'w')"),
        "ignored/w"
    );
    assert_eq!(
        run("'ignored'.search({[Symbol.search]: function () { return 42 }})"),
        "42"
    );
    assert_eq!(
        run("'ignored'.split({[Symbol.split]: function (s) { return ['a', s] }}).join('|')"),
        "a|ignored"
    );
    // A regular expression is an Object, so every one of these still goes through the pattern.
    assert_eq!(run("'a1b'.match(/\\d/)[0]"), "1");
    assert_eq!(run("'a-b'.split(/-/).join('|')"), "a|b");
    assert_eq!(run("'aXb'.replace(/X/, 'Y')"), "aYb");
    assert_eq!(run("'aXbX'.replaceAll(/X/g, 'Y')"), "aYbY");
    assert_eq!(run("'abc'.search(/b/)"), "1");
    assert_eq!(run("Array.from('a1b1'.matchAll(/\\d/g)).length"), "2");
    // §7.2.8's `IsRegExp` is inside the same guard, so `replaceAll`'s demand for a global flag is
    // asked of an Object and of nothing else — a non-global one is refused…
    assert_eq!(
        run("try { 'a'.replaceAll(/a/, 'b'); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …including one that merely *claims* to be a pattern, which is what makes `IsRegExp` a
    // question about behaviour rather than about how the object was made.
    assert_eq!(
        run(
            "try { 'a'.replaceAll({[Symbol.match]: true, flags: ''}, 'b'); 'no error' } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and a primitive is not asked, so it is not refused either: `1` has no flags and is simply
    // searched for as text.
    assert_eq!(run("'a1b1'.replaceAll(1, 'X')"), "aXbX");
}
