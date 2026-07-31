//! §22.2.6's four `Symbol` methods, and what `String.prototype` does through them.
//!
//! These are where a regular expression stops being a thing you call `exec` on and becomes the way
//! `replace`, `match`, `search` and `split` work. Every one goes through §22.2.7.1 `RegExpExec`,
//! which reads `exec` off the object — so an overridden `exec` is obeyed by all four, and that is
//! most of what is worth saying about them.

use super::*;

#[test]
fn replace_through_a_pattern_changes_one_or_every_match_by_its_flag() {
    assert_eq!(run("'a1b2'.replace(/\\d/, 'X')"), "aXb2");
    assert_eq!(run("'a1b2'.replace(/\\d/g, 'X')"), "aXbX");
    assert_eq!(run("'abc'.replace(/z/g, 'X')"), "abc");
    // The `$` forms mean the same here as with a string search value, because the substitution is
    // one operation rather than two spellings of one.
    assert_eq!(run("'abc'.replace(/b/, '[$`|$&|$\\'\\]')"), "a[a|b|c]c");
    assert_eq!(run("'a1'.replace(/(\\d)/, '<$1>')"), "a<1>");
    assert_eq!(run("'aaa'.replace(/a/g, '$&$&')"), "aaaaaa");
    // §22.2.6.9 step 14.l — a replacement function is handed the match, then each group, then
    // where it was, then the whole subject.
    assert_eq!(
        run("'a1b'.replace(/(\\d)/, function (m, one, at, whole) { \
             return [m, one, at, whole].join('/'); })"),
        // The tail after the match follows the replacement, so the `b` appears twice: once
        // inside what the function spelled out, and once as what was left of the subject.
        "a1/1/1/a1bb"
    );
    // …and the named groups arrive after the subject, only when the pattern has any.
    assert_eq!(
        run("'a1'.replace(/(?<d>\\d)/, function () { \
             return arguments.length + ':' + arguments[arguments.length - 1].d; })"),
        // Five: the match, one group, the position, the subject, and the named-group object.
        "a5:1"
    );
    assert_eq!(
        run("'a1'.replace(/(\\d)/, function () { return arguments.length; })"),
        "a4"
    );
    assert_eq!(
        run("'2026-07'.replace(/(?<y>\\d{4})-(?<m>\\d{2})/, '$<m>/$<y>')"),
        "07/2026"
    );
}

#[test]
fn a_global_replace_advances_past_a_match_that_consumed_nothing() {
    // Without the step over an empty match, `lastIndex` never moves and the loop does not end.
    assert_eq!(run("'abc'.replace(/(?:)/g, '-')"), "-a-b-c-");
    assert_eq!(run("'abc'.replace(/x*/g, '-')"), "-a-b-c-");
    // A non-global one changes exactly one, so no advance is needed or made.
    assert_eq!(run("'abc'.replace(/(?:)/, '-')"), "-abc");
}

#[test]
fn replace_puts_last_index_back_to_the_start_for_a_global_pattern_and_leaves_it_after() {
    // §22.2.6.9 step 7 — a global pattern begins at zero however `lastIndex` was left, so a second
    // `replace` with the same object answers the same thing.
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 2; 'aXa'.replace(r, 'b')"),
        "bXb"
    );
    assert_eq!(
        run("var r = /a/g; 'aXa'.replace(r, 'b'); 'aXa'.replace(r, 'b')"),
        "bXb"
    );
    // A non-global one is left where the match ended, because `exec` moved it and nothing put it
    // back. That is the observable difference between the two paths.
    assert_eq!(run("var r = /a/g; 'aXa'.replace(r, 'b'); r.lastIndex"), "0");
}

#[test]
fn match_answers_one_result_or_a_list_of_strings_by_the_global_flag() {
    // §22.2.6.6 step 5 — the two answers differ in *kind*, not in count: without `g` it is `exec`'s
    // array, with `g` it is the matched strings and no captures at all.
    assert_eq!(run("JSON.stringify('a1b'.match(/(\\d)/))"), r#"["1","1"]"#);
    assert_eq!(run("'a1b'.match(/(\\d)/).index"), "1");
    assert_eq!(
        run("JSON.stringify('a1b2'.match(/(\\d)/g))"),
        r#"["1","2"]"#
    );
    assert_eq!(run("String('a1b2'.match(/(\\d)/g).index)"), "undefined");
    // Step 8.a — nothing found is **null**, not an empty array.
    assert_eq!(run("String('abc'.match(/z/g))"), "null");
    assert_eq!(run("String('abc'.match(/z/))"), "null");
    // An empty match advances, so a global search over one still ends.
    assert_eq!(run("'abc'.match(/(?:)/g).length"), "4");
}

#[test]
fn search_answers_where_and_does_not_move_last_index() {
    assert_eq!(run("'abc'.search(/c/)"), "2");
    assert_eq!(run("'abc'.search(/z/)"), "-1");
    // §22.2.6.11 steps 4 and 8 — it is saved and put back, which is the whole difference from
    // `exec`. A search that moved it would make the next one start somewhere else.
    assert_eq!(
        run("var r = /b/g; r.lastIndex = 7; 'abc'.search(r); r.lastIndex"),
        "7"
    );
    // …and it searches from the beginning whatever `lastIndex` said.
    assert_eq!(run("var r = /a/g; r.lastIndex = 5; 'abc'.search(r)"), "0");
}

#[test]
fn split_cuts_at_every_match_and_keeps_the_captures() {
    assert_eq!(
        run("JSON.stringify('a,b,,c'.split(/,/))"),
        r#"["a","b","","c"]"#
    );
    // The captures go into the answer, which is why this is five pieces and not three.
    assert_eq!(
        run("JSON.stringify('a1b2c'.split(/(\\d)/))"),
        r#"["a","1","b","2","c"]"#
    );
    assert_eq!(run("JSON.stringify('abc'.split(/x/))"), r#"["abc"]"#);
    // §22.2.6.14 step 6 — a limit of zero answers nothing at all, and any other limit cuts the
    // answer short *including* the captures.
    assert_eq!(run("JSON.stringify('a,b,c'.split(/,/, 0))"), "[]");
    assert_eq!(run("JSON.stringify('a,b,c'.split(/,/, 2))"), r#"["a","b"]"#);
    assert_eq!(
        run("JSON.stringify('a1b2c'.split(/(\\d)/, 2))"),
        r#"["a","1"]"#
    );
    // Step 15 — an empty subject answers one empty piece, unless the pattern matches it, in which
    // case it answers none.
    assert_eq!(run("JSON.stringify(''.split(/x/))"), r#"[""]"#);
    assert_eq!(run("JSON.stringify(''.split(/(?:)/))"), "[]");
    // An empty match at a position that already ended a piece is stepped over rather than acted
    // on, or the split would never finish.
    assert_eq!(
        run("JSON.stringify('abc'.split(/(?:)/))"),
        r#"["a","b","c"]"#
    );
    // The splitter is made **sticky** whatever the receiver's flags are, so a non-sticky pattern
    // still cuts at each match rather than searching forward from the last piece.
    assert_eq!(run("JSON.stringify('a1b'.split(/\\d/))"), r#"["a","b"]"#);
    assert_eq!(run("JSON.stringify('a1b'.split(/\\d/y))"), r#"["a","b"]"#);
}

#[test]
fn all_four_go_through_the_objects_own_exec_when_it_has_a_callable_one() {
    // §22.2.7.1 — `exec` is read *off the object*, so overriding it changes what every one of these
    // sees. That is the reason they are written in terms of the operation and not the built-in.
    assert_eq!(
        run("var r = /a/; r.exec = function () { return ['Z']; }; 'qqq'.replace(r, '<$&>')"),
        // The made-up match still consumes its own length from the subject — one character at
        // position zero, `index` being absent and so reading as zero.
        "<Z>qq"
    );
    assert_eq!(
        run("var r = /a/; r.exec = function () { return null; }; String('aaa'.match(r))"),
        "null"
    );
    assert_eq!(
        run(
            "var r = /a/; r.exec = function () { var m = ['x']; m.index = 2; return m; }; \
             'abc'.search(r)"
        ),
        "2"
    );
    // A non-callable `exec` is *ignored* and the built-in used, which is what makes assigning a
    // number to it harmless rather than fatal.
    assert_eq!(run("var r = /b/; r.exec = 5; 'abc'.search(r)"), "1");
    // Step 3.c — an `exec` that answers something else is a TypeError rather than a value the
    // caller would have to guess about.
    assert_eq!(
        run("var r = /a/; r.exec = function () { return 5; }; \
             try { 'abc'.search(r) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_four_are_ordinary_writable_methods_and_not_enumerable() {
    for name in ["match", "replace", "search", "split"] {
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor(RegExp.prototype, Symbol.{name}); \
                 typeof d.value + ',' + d.writable + ',' + d.enumerable + ',' + d.configurable"
            )),
            "function,true,false,true",
            "Symbol.{name}"
        );
    }
    assert_eq!(
        run("RegExp.prototype[Symbol.replace].name"),
        "[Symbol.replace]"
    );
    // …and so is every flag accessor, which is a getter rather than a value.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(RegExp.prototype, 'global').enumerable"),
        "false"
    );
    assert_eq!(run("Object.keys(RegExp.prototype).length"), "0");
}

#[test]
fn a_string_method_makes_a_pattern_out_of_whatever_it_was_given() {
    // §22.1.3.14 and .21 — `RegExpCreate` of the argument, so a string becomes a *pattern*.
    assert_eq!(run("'a.c'.match('.')[0]"), "a");
    assert_eq!(run("'a.c'.search('\\\\.')"), "1");
    assert_eq!(run("'abc'.match().index"), "0");
    // …and `matchAll` makes a **global** one whatever it was handed, because iterating every match
    // is what it is for.
    assert_eq!(
        run("try { 'x'.matchAll(/a/); 'no throw' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_non_global_pattern_does_not_move_its_own_last_index_even_when_it_matches() {
    // §22.2.7.2 steps 4 and 5 read *and write* only for `g` or `y`, so a plain pattern leaves
    // whatever a program put there — including a number it never used.
    assert_eq!(
        run("var r = /a/; r.lastIndex = 5; r.exec('aaa'); r.lastIndex"),
        "5"
    );
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 5; r.exec('aaa'); r.lastIndex"),
        "0"
    );
    // A `y` pattern writes it on success and resets it on failure.
    assert_eq!(run("var r = /a/y; r.exec('aaa'); r.lastIndex"), "1");
    assert_eq!(
        run("var r = /b/y; r.lastIndex = 2; r.exec('aaa'); r.lastIndex"),
        "0"
    );
}

#[test]
fn the_source_escape_starts_unescaped_so_a_leading_slash_is_escaped_too() {
    // The escaping walk tracks whether the character before was a backslash. Starting that tracker
    // as *true* would leave a leading `/` alone, and `toString` would then answer text that no
    // longer parses.
    assert_eq!(run("new RegExp('/a').source"), "\\/a");
    assert_eq!(run("String(new RegExp('/a'))"), "/\\/a/");
    assert_eq!(run("new RegExp('a/b').source"), "a\\/b");
    assert_eq!(run("new RegExp('\\\\/a').source"), "\\/a");
}

#[test]
fn a_group_name_may_hold_a_dollar_after_its_first_character() {
    // §12.7.1's `IdentifierPart` includes `$`, so the continue test is a union and not an
    // intersection — written as one it would refuse every name but a single letter.
    assert_eq!(run("/(?<a$b>x)/.exec('x').groups.a$b"), "x");
    assert_eq!(run("/(?<a1>x)/.exec('x').groups.a1"), "x");
    assert_eq!(run("/(?<_a>x)/.exec('x').groups._a"), "x");
}

#[test]
fn a_failed_search_resets_last_index_only_for_a_pattern_that_keeps_one() {
    // §22.2.7.2 step 6.a.ii — the reset belongs to the same two flags the read did. A plain
    // pattern never touches the property, so whatever a program left there stays there even when
    // the search finds nothing.
    assert_eq!(
        run("var r = /a/; r.lastIndex = 5; r.exec('zzz'); r.lastIndex"),
        "5"
    );
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 5; r.exec('zzz'); r.lastIndex"),
        "0"
    );
    assert_eq!(
        run("var r = /a/y; r.lastIndex = 5; r.exec('zzz'); r.lastIndex"),
        "0"
    );
}

#[test]
fn a_global_walk_advances_only_past_a_match_that_consumed_nothing() {
    // The advance is what stops an empty match looping. Making it unconditional skips a character
    // after *every* match, so adjacent ones are missed — which no test of an empty pattern sees.
    assert_eq!(run("'aa'.match(/a/g).length"), "2");
    assert_eq!(run("JSON.stringify('aa'.match(/a/g))"), r#"["a","a"]"#);
    assert_eq!(run("'aa'.replace(/a/g, 'X')"), "XX");
    assert_eq!(run("'aaa'.replace(/a/g, '')"), "");
}

#[test]
fn only_a_global_replace_winds_last_index_back_before_it_starts() {
    // §22.2.6.9 step 7 is guarded by the flag. A non-global pattern's `lastIndex` is not the
    // replacement's to touch — it is not read either, so writing it would be a change nothing
    // asked for and a program can see it.
    assert_eq!(
        run("var r = /a/; r.lastIndex = 5; 'aa'.replace(r, 'b'); r.lastIndex"),
        "5"
    );
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 5; 'aa'.replace(r, 'b'); r.lastIndex"),
        "0"
    );
}

#[test]
fn a_match_reported_behind_what_is_already_copied_is_skipped() {
    // §22.2.6.9 step 14.n. Only an overriding `exec` can report one — the built-in never moves
    // backwards — and without the guard the pieces overlap and the answer repeats itself.
    assert_eq!(
        run("var r = /a/g; var turn = 0; \
             r.exec = function () { turn++; if (turn > 2) { return null; } \
             var m = ['a']; m.index = 0; return m; }; \
             'abc'.replace(r, '<$&>')"),
        "<a>bc"
    );
    // …and one reported *forwards* of it is used, so the guard is a guard and not a refusal.
    assert_eq!(
        run("var r = /a/g; var turn = 0; \
             r.exec = function () { turn++; if (turn > 2) { return null; } \
             var m = ['b']; m.index = turn; return m; }; \
             'abc'.replace(r, '<$&>')"),
        "a<b><b>"
    );
}

#[test]
fn match_all_walks_every_match_through_an_iterator_over_a_copy() {
    assert_eq!(
        run("JSON.stringify(Array.from('a1b2'.matchAll(/\\d/g), function (m) { return m[0]; }))"),
        r#"["1","2"]"#
    );
    // Each step is `exec`'s whole shape, not just the text — captures and `index` included, which
    // is what tells `matchAll` from a global `match`.
    assert_eq!(
        run("var m = 'a1b2'.matchAll(/(\\d)/g).next().value; m[0] + ',' + m[1] + ',' + m.index"),
        "1,1,1"
    );
    // §22.2.9.1 steps 4 and 5 — the walk uses a **copy**, so the original is left exactly where it
    // was and a program may keep using it.
    assert_eq!(
        run("var r = /a/g; var it = 'aa'.matchAll(r); it.next(); it.next(); r.lastIndex"),
        "0"
    );
    // …and the copy starts where the original had got to, so a pattern part-way through a subject
    // continues rather than starting over.
    assert_eq!(
        run("var r = /a/g; r.lastIndex = 1; \
             JSON.stringify(Array.from('aa'.matchAll(r), function (m) { return m.index; }))"),
        "[1]"
    );
    // An empty match steps forward, or the walk would not end.
    assert_eq!(
        run(
            "JSON.stringify(Array.from('abc'.matchAll(/(?:)/g), function (m) { return m.index; }))"
        ),
        "[0,1,2,3]"
    );
}

#[test]
fn a_non_global_pattern_yields_one_match_and_is_then_finished() {
    // §22.2.9.2.1 step 7 — the flag is read once, when the iterator is made, and decides whether
    // there can be a second step at all.
    assert_eq!(
        run(
            "var it = RegExp.prototype[Symbol.matchAll].call(/a/, 'aaa'); \
             it.next().value[0] + ',' + it.next().done"
        ),
        "a,true"
    );
    // §22.1.3.15 demands a global pattern before it ever gets here, so the one-shot form is only
    // reachable through the Symbol method itself.
    assert_eq!(
        run("try { 'aaa'.matchAll(/a/) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_iterator_is_an_ordinary_iterator_and_says_what_it_is() {
    assert_eq!(
        run("Object.prototype.toString.call('a'.matchAll(/a/g))"),
        "[object RegExp String Iterator]"
    );
    // §22.2.9.3 — it inherits `[Symbol.iterator]` from `%IteratorPrototype%`, so it is iterable
    // without carrying one of its own.
    assert_eq!(
        run(
            "var it = 'a'.matchAll(/a/g); (it[Symbol.iterator]() === it) + ',' + \
             Object.getOwnPropertyNames(Object.getPrototypeOf(it)).join()"
        ),
        "true,next"
    );
    assert_eq!(
        run("var out = []; for (var m of 'a1b2'.matchAll(/\\d/g)) { out.push(m[0]); } out.join()"),
        "1,2"
    );
    // A finished walk stays finished and asks the regular expression nothing further.
    assert_eq!(
        run("var it = 'a'.matchAll(/a/g); it.next(); it.next(); \
             var d = it.next(); d.done + ',' + String(d.value)"),
        "true,undefined"
    );
    assert_eq!(
        run(
            "try { RegExp.prototype[Symbol.matchAll].call(/a/, 'a').next.call({}) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn an_empty_match_steps_a_whole_code_point_under_the_unicode_flags() {
    // §22.2.7.3 `AdvanceStringIndex`. Without it the walk stops *inside* a surrogate pair, which
    // is a position no code point begins at — so the same astral character is reported twice and
    // the indices no longer line up with anything a program can slice.
    assert_eq!(
        run("Array.from('\u{1F600}\u{1F600}'.matchAll(/(?:)/gu), \
             function (m) { return m.index; }).join()"),
        "0,2,4"
    );
    // …and without the flag the step is one code unit, so the halves are visited separately.
    assert_eq!(
        run("Array.from('\u{1F600}\u{1F600}'.matchAll(/(?:)/g), \
             function (m) { return m.index; }).join()"),
        "0,1,2,3,4"
    );
    // `v` asks for the same reading as `u`, so it steps the same way.
    assert_eq!(
        run("Array.from('\u{1F600}'.matchAll(/(?:)/gv), \
             function (m) { return m.index; }).join()"),
        "0,2"
    );
    // A **lone** leading surrogate is one code unit and not half of anything, so the step is one:
    // both halves of the pair test have to hold, not either.
    assert_eq!(
        run("Array.from('\\uD83Dx'.matchAll(/(?:)/gu), function (m) { return m.index; }).join()"),
        "0,1,2"
    );
    // …and a lone *trailing* one likewise.
    assert_eq!(
        run("Array.from('\\uDE00x'.matchAll(/(?:)/gu), function (m) { return m.index; }).join()"),
        "0,1,2"
    );
}

#[test]
fn the_iterator_advances_only_past_a_match_that_consumed_nothing() {
    // The same trap the global `match` had: making the step unconditional passes every test of an
    // *empty* pattern while silently skipping a character after every real match.
    assert_eq!(
        run("Array.from('aa'.matchAll(/a/g), function (m) { return m.index; }).join()"),
        "0,1"
    );
    assert_eq!(run("Array.from('aaa'.matchAll(/a/g)).length"), "3");
}
