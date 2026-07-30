//! §13.2.8 — a template literal, and the two things about it that look like `+` and are not.
//!
//! Checked against V8 first. A template is its components and its substitutions joined in written
//! order, and the only subtle parts are *which* conversion each substitution gets and *when*.

use super::*;

#[test]
fn a_template_is_its_pieces_joined_in_the_order_they_are_written() {
    assert_eq!(run("`hello`"), "hello");
    assert_eq!(run("`a${1}b`"), "a1b");
    assert_eq!(run("`${1}`"), "1");
    assert_eq!(run("`a${1}b${2}c`"), "a1b2c");
    assert_eq!(run("`${1}${2}`"), "12");
    assert_eq!(run("typeof `x`"), "string");
    assert_eq!(run("`${'a'}` === 'a'"), "true");
    // A template with nothing in it is one empty component and no substitutions — the count is
    // always one more than the other, which is what makes the loop that joins them terminate.
    assert_eq!(run("``.length"), "0");
    assert_eq!(run("`a${1}b`.length"), "3");
    // The substitution is a full expression, not just a name.
    assert_eq!(run("`${1 + 2}`"), "3");
    assert_eq!(
        run("(function () { var x = 5; return `x is ${x}`; })()"),
        "x is 5"
    );
    // Escapes are the lexer's and are cooked before the compiler ever sees them; a `$` that is
    // not followed by a brace is an ordinary character, and a brace inside a substitution closes
    // nothing.
    assert_eq!(run("`a\\nb`.length"), "3");
    assert_eq!(run("`a\\tb`.charCodeAt(1)"), "9");
    assert_eq!(run("`\\u0041`"), "A");
    assert_eq!(run("`$`"), "$");
    assert_eq!(run("`${'}'}`"), "}");
}

#[test]
fn a_substitution_is_to_string_and_not_addition() {
    // The row this is all about. §13.2.8.6 specifies `ToString`, which asks an object with the
    // **string** hint and reaches `toString`; `+` asks with the default hint and reaches
    // `valueOf`. An object with both answers differently in the two places, and writing a
    // template as `"" + x` would quietly get this wrong for every such object.
    let both = "{toString: function () { return 'a'; }, valueOf: function () { return 'b'; }}";
    assert_eq!(run(&format!("`${{{both}}}`")), "a");
    assert_eq!(run(&format!("'' + {both}")), "b");
    // The ordinary conversions, which agree either way and are worth pinning so the row above is
    // read as the exception it is.
    assert_eq!(run("`${null}`"), "null");
    assert_eq!(run("`${undefined}`"), "undefined");
    assert_eq!(run("`${true}`"), "true");
    assert_eq!(run("`${[1, 2]}`"), "1,2");
    assert_eq!(run("`${{}}`"), "[object Object]");
    // §7.1.17 throws for a Symbol, and a template is one of the places that reaches it — so a
    // Symbol in a template is a TypeError rather than something unhelpful in the middle of a
    // string. That is the whole reason the conversion throws.
    assert_eq!(
        run("(function () { try { return `${Symbol()}`; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn each_substitution_is_converted_before_the_next_one_is_evaluated() {
    // Not at the end, over a list of collected values. The difference shows twice: a side effect
    // in one `toString` must be visible to the expression after it, and a `toString` that throws
    // must stop the ones that would have followed.
    assert_eq!(
        run(
            "(function () { var r = []; var s = `${r.push(1)}${r.push(2)}`; return r.join(','); })()"
        ),
        "1,2"
    );
    assert_eq!(
        run("(function () { var r = ''; \
             try { `${{toString: function () { r += 'a'; throw new Error('x'); }}}\
             ${(function () { r += 'b'; return 1; })()}`; } catch (e) {} return r; })()"),
        "a"
    );
    // …and each substitution is converted once, not once per place it is joined.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {toString: function () { n++; return 'z'; }}; \
             `${o}${o}`; return n; })()"
        ),
        "2"
    );
}

#[test]
fn a_tagged_template_calls_its_tag_with_the_template_object_and_the_substitutions() {
    // §13.3.11 — `` f`a${b}c` `` is `f(templateObject, b)`, so a tag with no substitutions still
    // takes one argument and the cooked strings are always one more than the substitutions.
    assert_eq!(
        run(
            "(function () { function tag(s, x, y) { return s.join(',') + '|' + x + '|' + y; } \
             return tag`a${1}b${2}c`; })()"
        ),
        "a,b,c|1|2"
    );
    assert_eq!(
        run(
            "(function () { function tag(s) { return s.length + ',' + arguments.length; } \
             return tag`only`; })()"
        ),
        "1,1"
    );
    // §13.2.8.3 — the cooked strings and a `raw` beside them, which is the whole reason a tag is
    // handed an object rather than a string: `raw` has the escapes exactly as written.
    assert_eq!(
        run(
            "(function () { function tag(s) { return (s[0] === 'a\\nb') + ',' + s.raw[0]; } \
             return tag`a\\nb`; })()"
        ),
        "true,a\\nb"
    );
    // §12.9.6 leaves `TV` **undefined** for an escape that is not one, which only a *tagged* template
    // may contain — an untagged one is a Syntax Error. So `cooked` has a hole and `raw` does not.
    assert_eq!(
        run(
            "(function () { function tag(s) { return String(s[0]) + ',' + s.raw[0]; } \
             return tag`\\unicode`; })()"
        ),
        "undefined,\\unicode"
    );
    // The tag is called as a *method* when it is written as one, exactly as `o.m()` would be.
    assert_eq!(
        run("(function () { var o = { m(s) { return this === o; } }; return o.m`x`; })()"),
        "true"
    );
}

#[test]
fn the_template_object_is_the_same_one_every_time_that_site_is_evaluated() {
    // §13.2.8.3 caches per *Parse Node*, and that identity is the only thing about the object a
    // program can detect which its contents do not already say — it is what lets a tag use it as a key
    // into a table of its own. Building a fresh object each time passes every test about the contents
    // and fails every test about this.
    assert_eq!(
        run("(function () { function tag(s) { return s; } \
             function twice() { return tag`x`; } return twice() === twice(); })()"),
        "true"
    );
    // Two *sites* that happen to spell the same thing are two objects, which is the other half: the
    // key is the site and not the text.
    assert_eq!(
        run("(function () { function tag(s) { return s; } \
             function a() { return tag`x`; } function b() { return tag`x`; } \
             return a() === b(); })()"),
        "false"
    );
    // §13.2.8.3 steps 10 and 11 — both arrays are frozen, so a tag cannot change what a later
    // evaluation of the same site will see. That is what makes handing the same object out safe.
    assert_eq!(
        run(
            "(function () { function tag(s) { return Object.isFrozen(s) + ',' + Object.isFrozen(s.raw); } \
             return tag`x`; })()"
        ),
        "true,true"
    );
    assert_eq!(
        run(
            "(function () { function tag(s) { return s; } function once() { return tag`x`; } \
             var first = once(); first[0] = 'changed'; \
             return once()[0]; })()"
        ),
        "x"
    );
    // …and `raw` is **not enumerable**, which is the one of its three attributes the freeze does not
    // set for it: `Object.keys` of the object is its indices and nothing else.
    assert_eq!(
        run(
            "(function () { function tag(s) { return Object.keys(s).join(',') \
             + '|' + s.propertyIsEnumerable('raw'); } return tag`a${1}b`; })()"
        ),
        "0,1|false"
    );
}
