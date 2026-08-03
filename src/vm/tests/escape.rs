//! §B.2.1 — `escape` and `unescape`, and the two ways they are not `encodeURI`.
//!
//! Checked against V8 first. The rows worth reading are the ones that separate these from §19.2.6:
//! a surrogate pair escapes as two halves rather than as one code point, and nothing here throws
//! for input the other family calls a URIError.

use super::*;

#[test]
fn escape_leaves_the_word_characters_and_six_marks_and_escapes_the_rest() {
    // §B.2.1.1 step 4's set, written out — test262's `unmodified.js` spells it out too, and the
    // `_` in the middle of it is the
    // row that matters: it is an ASCII **word** character, so `is_ascii_alphanumeric` plus the
    // punctuation list would escape it and nothing else here would notice.
    assert_eq!(run("escape('ABCyz019@*_+-./')"), "ABCyz019@*_+-./");
    assert_eq!(run("escape('_')"), "_");
    // The marks §19.2.6 keeps and this does not — `*` is in **both** sets and is deliberately not
    // in this row, because the two sets overlap by coincidence rather than by derivation.
    assert_eq!(run("escape(\"!~'()\")"), "%21%7E%27%28%29");
    // …and the other half: what this keeps and `encodeURIComponent` escapes.
    assert_eq!(
        run("escape('@+/') + ',' + encodeURIComponent('@+/')"),
        "@+/,%40%2B%2F"
    );
    // The three the two sets agree on, so the difference above is a difference and not a shape.
    assert_eq!(
        run("escape('-.*') + ',' + encodeURIComponent('-.*')"),
        "-.*,-.*"
    );
}

#[test]
fn escape_writes_two_digits_below_256_and_four_above_it() {
    // Step 6.c.ii — under 256 is `%XX`, and `StringPad(hex, 2, "0", start)` means the leading zero
    // is written rather than dropped.
    assert_eq!(run("escape('\\x00\\x01\\x02\\x03')"), "%00%01%02%03");
    assert_eq!(run("escape('\\x07')"), "%07");
    assert_eq!(run("escape(' ')"), "%20");
    assert_eq!(run("escape('\\x7F\\x80\\xFF')"), "%7F%80%FF");
    // Step 6.c.iii — 256 and above keeps its `u` and all four digits, the form §19.2.6 has no
    // equivalent of. The boundary is pinned from both sides.
    assert_eq!(run("escape('\\u0100\\u0101')"), "%u0100%u0101");
    assert_eq!(run("escape('\\uFFFD\\uFFFE\\uFFFF')"), "%uFFFD%uFFFE%uFFFF");
    // Uppercase digits, and all four positions carry one above nine so a mis-shifted nibble shows.
    assert_eq!(run("escape('\\uABCD')"), "%uABCD");
    assert_eq!(run("escape('\\uDEAD')"), "%uDEAD");
    assert_eq!(run("escape('')"), "");
}

#[test]
fn escape_walks_code_units_where_encode_uri_component_walks_code_points() {
    // The whole difference between the two families, in one row: a surrogate pair is **two code
    // units** here and gets two escapes, where §19.2.6.5 reads it as one code point and gives it
    // one four-octet UTF-8 sequence.
    assert_eq!(run("escape('\\u{1F600}')"), "%uD83D%uDE00");
    assert_eq!(run("encodeURIComponent('\\u{1F600}')"), "%F0%9F%98%80");
    assert_eq!(run("escape('\\uD834\\uDF06')"), "%uD834%uDF06");
    // …and because a code unit is all this knows about, an **unpaired** surrogate is an ordinary
    // one and escapes rather than throwing. `encodeURI` calls the same string a URIError.
    assert_eq!(run("escape('\\uD800')"), "%uD800");
    assert_eq!(run("escape('\\uDC00')"), "%uDC00");
    assert_eq!(
        run("try { encodeURI('\\uD800'); 'no throw' } catch (e) { e.name }"),
        "URIError"
    );
}

#[test]
fn unescape_reads_both_forms_and_prefers_the_longer_one() {
    // §B.2.1.2 step 6.b.i is tried before 6.b.ii, so `%u0041` is `"A"` and not the `%u0` that
    // reading two digits first would leave.
    assert_eq!(run("unescape('%u0041')"), "A");
    assert_eq!(run("unescape('%41')"), "A");
    assert_eq!(run("unescape('%u0000').length"), "1");
    assert_eq!(run("unescape('%uFFFF') === '\\uFFFF'"), "true");
    // Both cases of digit, in every position of both forms.
    assert_eq!(run("unescape('%u002a%u002A')"), "**");
    assert_eq!(run("unescape('%2f%2F')"), "//");
    assert_eq!(run("unescape('%uaBcD') === '\\uABCD'"), "true");
    // The round trip, including the one that needs the four-digit form.
    assert_eq!(
        run("unescape(escape('\\u{1F600}')) === '\\u{1F600}'"),
        "true"
    );
    assert_eq!(run("unescape(escape('a b~c')) === 'a b~c'"), "true");
    assert_eq!(run("unescape('')"), "");
}

#[test]
fn unescape_copies_through_every_percent_that_begins_no_escape() {
    // §B.2.1.2 has no failure outcome at all, which is the sharpest way to see that these two
    // families are separate: every one of these is a URIError to `decodeURI`.
    assert_eq!(run("unescape('%')"), "%");
    assert_eq!(run("unescape('%1')"), "%1");
    assert_eq!(run("unescape('%zz')"), "%zz");
    assert_eq!(run("unescape('%u')"), "%u");
    assert_eq!(run("unescape('%u12')"), "%u12");
    assert_eq!(run("unescape('%u123')"), "%u123");
    // Not prefixed with `u`, so the four-digit form does not apply and the two-digit one reads the
    // first two of what is there — `%0041` is `\x00` followed by the characters `41`.
    assert_eq!(run("unescape('%0041') === '\\x0041'"), "true");
    // …and the four that are `u` but not four digits fall through to the two-digit form finding
    // nothing, because `u` is not a hexadecimal digit.
    assert_eq!(run("unescape('%uzzzz')"), "%uzzzz");
    // A `%` that is copied advances by **one**, so what follows it is read afresh — which is why
    // the second `%` here begins an escape that succeeds.
    assert_eq!(run("unescape('%%41')"), "%A");
    assert_eq!(run("unescape('%u%u0041')"), "%uA");
    // The exact boundary of each length condition: one unit short reads nothing, and the shortest
    // string that fits reads. §B.2.1.2 writes these as `k + 5 < len` and `k + 3 <= len`.
    assert_eq!(run("unescape('%u004')"), "%u004");
    assert_eq!(run("unescape('%4')"), "%4");
    assert_eq!(run("unescape('a%u0041b')"), "aAb");
    assert_eq!(run("unescape('a%41b')"), "aAb");
}

#[test]
fn both_convert_the_argument_to_a_string_and_neither_can_fail_after_that() {
    // Step 1 is `ToString` and is the only step either can throw at, so a throw out of one of
    // these is the argument's own.
    assert_eq!(run("escape(undefined)"), "undefined");
    assert_eq!(run("unescape(null)"), "null");
    assert_eq!(run("escape()"), "undefined");
    assert_eq!(run("unescape(true)"), "true");
    assert_eq!(run("escape(-0)"), "0");
    assert_eq!(run("unescape(NaN)"), "NaN");
    assert_eq!(run("escape(Infinity)"), "Infinity");
    assert_eq!(run("escape({})"), "%5Bobject%20Object%5D");
    assert_eq!(
        run("try { escape(Symbol()); 'no throw' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { unescape({toString(){throw new RangeError('x')}}); 'x' } catch (e) { e.name }"),
        "RangeError"
    );
}

#[test]
fn both_are_ordinary_built_in_functions_and_neither_constructs() {
    for name in ["escape", "unescape"] {
        assert_eq!(run(&format!("{name}.length")), "1");
        assert_eq!(run(&format!("{name}.name")), name);
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor(this, '{name}'); \
                 '' + d.writable + d.enumerable + d.configurable"
            )),
            "truefalsetrue"
        );
        assert_eq!(
            run(&format!(
                "try {{ new {name}(''); 'no error' }} catch (e) {{ e.name }}"
            )),
            "TypeError"
        );
        assert_eq!(run(&format!("{name}.prototype")), "undefined");
    }
}
