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
fn the_three_that_need_a_pattern_say_so_rather_than_answering_wrongly() {
    // §22.1.3.14, .15 and .21 all end "make a RegExp out of the argument". There is nothing to make
    // one with yet, so they refuse — and the refusal names the reason rather than reporting some
    // unrelated failure further along.
    for source in [
        "'abc'.match('b')",
        "'abc'.matchAll('b')",
        "'abc'.search('b')",
    ] {
        assert_eq!(
            run(&format!("try {{ {source} }} catch (e) {{ e.message }}")),
            "this needs a regular expression, and RegExp is not implemented yet",
            "{source} should say what it wanted"
        );
    }
    // …and the receiver is still checked first, so `null` is refused for being `null`.
    assert_eq!(
        run("try { String.prototype.match.call(null, {}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn each_of_the_five_refuses_a_receiver_that_cannot_be_coerced() {
    // §22.1.3's `RequireObjectCoercible`, and the ordering with it: `replace` looks for the Symbol
    // method *before* converting the receiver, so a pattern that handles everything never sees the
    // refusal — and a string search value hits it.
    for source in [
        "String.prototype.replace.call(null, 'a', 'b')",
        "String.prototype.replaceAll.call(undefined, 'a', 'b')",
        "String.prototype.search.call(null, {})",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; 'no throw' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source} should refuse its receiver"
        );
    }
    // A pattern that takes over never reaches the conversion, so even `null` works through it.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.replace] = function (s) { return typeof s; }; \
             String.prototype.replace.call(null, o, 'x')"
        ),
        "object"
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
    assert_eq!(
        run("try { 'x'.matchAll(undefined) } catch (e) { e.message }"),
        "this needs a regular expression, and RegExp is not implemented yet"
    );
    // …and `null` skips the delegation exactly as `undefined` does.
    assert_eq!(
        run("try { 'x'.matchAll(null) } catch (e) { e.message }"),
        "this needs a regular expression, and RegExp is not implemented yet"
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
