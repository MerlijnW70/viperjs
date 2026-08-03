//! §19.2.6 — the four URI functions, and the population of `URIError`.
//!
//! Checked against V8 first. The rows worth reading are the ones where the two encoders disagree
//! and the ones where decoding *refuses*: RFC 3629 says a code point has exactly one UTF-8
//! encoding, and everything that reassembles into the right bits by a longer route is rejected
//! rather than accepted.

use super::*;

/// The name of the error `source` throws, or `"no throw"` if it produced a value instead.
///
/// Named rather than repeated because a refusal test that silently stopped refusing would
/// otherwise read as a passing `try`/`catch` with nothing in the `catch`, and there are thirty of
/// them below.
fn refusal(source: &str) -> String {
    run(&format!(
        "try {{ {source}; 'no throw' }} catch (e) {{ e.name }}"
    ))
}

#[test]
fn the_reserved_set_is_the_only_thing_the_two_encoders_disagree_about() {
    // §19.2.6.3 passes `uriReserved` plus `#` as `extraUnescaped` and §19.2.6.4 passes nothing,
    // which is the entire difference between the two functions.
    assert_eq!(run("encodeURI(';/?:@&=+$,#')"), ";/?:@&=+$,#");
    assert_eq!(
        run("encodeURIComponent(';/?:@&=+$,#')"),
        "%3B%2F%3F%3A%40%26%3D%2B%24%2C%23"
    );
    // …and they agree about everything else, which is what makes the disagreement meaningful.
    assert_eq!(run("encodeURI(' ')"), "%20");
    assert_eq!(run("encodeURIComponent(' ')"), "%20");
    assert_eq!(run("encodeURI('a\"b')"), "a%22b");
    assert_eq!(run("encodeURIComponent('a\"b')"), "a%22b");
    // The practical shape of it: a `&` inside a value would otherwise invent a parameter.
    assert_eq!(run("'q=' + encodeURIComponent('a&b=c')"), "q=a%26b%3Dc");
}

#[test]
fn the_always_unescaped_set_is_the_word_characters_and_nine_marks() {
    // §19.2.6.5 step 3 — the ASCII word characters, so the underscore is in and no other
    // punctuation is except by the mark list.
    assert_eq!(run("encodeURIComponent('abcXYZ019_')"), "abcXYZ019_");
    assert_eq!(run("encodeURIComponent(\"-.!~*'()\")"), "-.!~*'()");
    // Everything one character away from that list is escaped, which is what pins the list.
    assert_eq!(run("encodeURIComponent('%')"), "%25");
    assert_eq!(run("encodeURIComponent('[]{}')"), "%5B%5D%7B%7D");
    assert_eq!(run("encodeURIComponent('\\\\^`')"), "%5C%5E%60");
    assert_eq!(run("encodeURIComponent('\"<>')"), "%22%3C%3E");
}

#[test]
fn an_escape_is_two_uppercase_digits_and_the_leading_zero_is_not_optional() {
    // §19.2.6.5 step 6.c.v.1 formats the octet as *two* uppercase digits. A decoder here takes
    // either case, so only half of this is convention — the width is load-bearing.
    assert_eq!(run("encodeURIComponent('\\x07')"), "%07");
    assert_eq!(run("encodeURIComponent('\\x00')"), "%00");
    assert_eq!(run("encodeURIComponent('\\x7F')"), "%7F");
    // Uppercase: `\xFF` is two octets and both of them have a digit above nine.
    assert_eq!(run("encodeURIComponent('\\xFF')"), "%C3%BF");
}

#[test]
fn encoding_walks_code_points_so_a_pair_gets_one_sequence_and_a_half_gets_a_uri_error() {
    // §19.2.6.5 step 6.c.i is `CodePointAt`, so a surrogate pair is *one* code point and gets the
    // four-octet form. Reading unit by unit would give the six-octet CESU-8 spelling instead, and
    // `decodeURIComponent` here would then refuse its own output.
    assert_eq!(run("encodeURIComponent('\\u{1F600}')"), "%F0%9F%98%80");
    assert_eq!(
        run("decodeURIComponent(encodeURIComponent('\\u{1F600}')) === '\\u{1F600}'"),
        "true"
    );
    // The three widths below four, so every range boundary is pinned by a row.
    assert_eq!(run("encodeURIComponent('\\u0080')"), "%C2%80");
    assert_eq!(run("encodeURIComponent('\\u07FF')"), "%DF%BF");
    assert_eq!(run("encodeURIComponent('\\u0800')"), "%E0%A0%80");
    assert_eq!(run("encodeURIComponent('\\uFFFF')"), "%EF%BF%BF");
    // Step 6.c.ii — half of a pair is not a code point, and there is no UTF-8 for it. Both halves
    // fail, and a leading one at the end of the string fails for the other of the two reasons the
    // clause gives.
    assert_eq!(refusal("encodeURI('\\uD800')"), "URIError");
    assert_eq!(refusal("encodeURI('\\uDC00')"), "URIError");
    assert_eq!(refusal("encodeURI('a\\uD800b')"), "URIError");
    assert_eq!(refusal("encodeURIComponent('\\uD800')"), "URIError");
    // Two trailing halves in a row, which is the only input that distinguishes step 6's test from
    // no test at all: a lone trailing surrogate anywhere else fails at step 8 instead, because
    // what follows it is not a trailing surrogate either. Without step 6 this pairs a trailing
    // half with a trailing half and encodes `%F4%90%80%80` — a code point above the last plane.
    assert_eq!(refusal("encodeURI('\\uDC00\\uDC00')"), "URIError");
    assert_eq!(refusal("encodeURI('\\uDC00\\uDFFF')"), "URIError");
    // …and a well-formed pair does not fail, which is what says the check is about pairing rather
    // than about the range.
    assert_eq!(run("encodeURI('\\uD800\\uDC00')"), "%F0%90%80%80");
    // The top of the range, which is the only place the four-octet form's leading octet carries
    // any bits at all: below `U+40000` it is `0xF0` however the shift is written.
    assert_eq!(run("encodeURIComponent('\\u{10FFFF}')"), "%F4%8F%BF%BF");
    assert_eq!(run("encodeURIComponent('\\u{40000}')"), "%F1%80%80%80");
}

#[test]
fn decoding_preserves_the_reserved_set_as_escapes_so_the_pair_round_trips() {
    // §19.2.6.6 step 4.c.vi.2 — an escape spelling a reserved character stays *spelled that way*,
    // which is why `decodeURI` undoes exactly what `encodeURI` did and no more.
    assert_eq!(run("decodeURI('%3B')"), "%3B");
    assert_eq!(run("decodeURI('%2F%3F%3A')"), "%2F%3F%3A");
    assert_eq!(run("decodeURI('%23')"), "%23");
    assert_eq!(run("decodeURIComponent('%3B')"), ";");
    assert_eq!(run("decodeURIComponent('%2F')"), "/");
    // The round trip that preserving buys: a literal `;` and an escaped one stay distinguishable.
    assert_eq!(run("decodeURI(encodeURI(';%3B')) === ';%3B'"), "true");
    // A preserved escape is copied from the source rather than re-spelled, so its case survives —
    // without which the round trip above would hold for uppercase input only.
    assert_eq!(run("decodeURI('%3b')"), "%3b");
    // Everything outside the set decodes in both, including a `%` that then sits beside digits
    // and is *not* re-read as another escape.
    assert_eq!(run("decodeURI('%5E')"), "^");
    assert_eq!(run("decodeURI('%2541')"), "%41");
}

#[test]
fn a_truncated_or_non_hexadecimal_escape_is_a_uri_error() {
    // §19.2.6.7 needs two digits and will not read a shorter escape as the characters it looks
    // like, which is what makes each of these a throw rather than a passthrough.
    assert_eq!(refusal("decodeURI('%')"), "URIError");
    assert_eq!(refusal("decodeURI('%A')"), "URIError");
    assert_eq!(refusal("decodeURI('%1')"), "URIError");
    assert_eq!(refusal("decodeURI('%zz')"), "URIError");
    assert_eq!(refusal("decodeURI('%1z')"), "URIError");
    assert_eq!(refusal("decodeURI('%g1')"), "URIError");
    // Both cases of digit are accepted, which is the other half of what §12.9.3's `HexDigit` says.
    assert_eq!(run("decodeURIComponent('%2f') === '/'"), "true");
    assert_eq!(run("decodeURIComponent('%2F') === '/'"), "true");
    assert_eq!(run("decodeURIComponent('%7b') === '{'"), "true");
    assert_eq!(run("decodeURIComponent('%c3%a9') === '\\xE9'"), "true");
}

#[test]
fn a_multi_octet_sequence_needs_every_one_of_its_octets_escaped() {
    // §19.2.6.6 step 4.c.vii.5 — the octets after the first are `%XX` too, so a sequence that
    // announces three and is handed raw bytes is refused rather than read across them.
    assert_eq!(refusal("decodeURI('%E0%A0')"), "URIError");
    assert_eq!(refusal("decodeURI('%C2')"), "URIError");
    assert_eq!(refusal("decodeURI('%E0A0%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%F0%9F%98')"), "URIError");
    // The shape that pins the `%` test rather than the digits after it: every octet here is two
    // hexadecimal digits in the right place and only the `%` is missing, so a walk that read the
    // digits without checking for the `%` would decode this to `U+0800` and never notice.
    assert_eq!(refusal("decodeURI('%E0-A0-80')"), "URIError");
    assert_eq!(refusal("decodeURI('%C2-80')"), "URIError");
    // Step 4.c.vii.1 — a first octet of `10xxxxxx` continues a sequence that never began, and one
    // of `111110xx` claims a width UTF-8 has not had since RFC 3629 narrowed it to four.
    assert_eq!(refusal("decodeURI('%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%BF')"), "URIError");
    assert_eq!(refusal("decodeURI('%F8%80%80%80%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%FE')"), "URIError");
    assert_eq!(refusal("decodeURI('%FF')"), "URIError");
    // …and this is the five-octet sequence that the *width* check has to catch, because nothing
    // downstream would: its octets reassemble to `U+10000`, which is a real code point in range
    // and not overlong for the four-octet form. Refusing it is the width rule doing work no
    // validity rule can do, and every other five-octet input here would be refused twice over.
    assert_eq!(refusal("decodeURI('%F8%80%90%80%80')"), "URIError");
    // Step 4.c.vii.7 — a continuation octet that is not `10xxxxxx` ends the sequence early rather
    // than contributing its low bits, which is a refusal and not a shorter code point.
    assert_eq!(refusal("decodeURI('%C2%41')"), "URIError");
    assert_eq!(refusal("decodeURI('%E0%A0%41')"), "URIError");
    assert_eq!(refusal("decodeURI('%F0%90%80%41')"), "URIError");
}

#[test]
fn an_overlong_a_surrogate_and_anything_above_the_last_plane_do_not_decode() {
    // Step 4.c.vii.7's "a **valid** UTF-8 encoding" is RFC 3629's definition, and these three are
    // the whole of what it excludes that reassembling the bits would accept. Every one of them
    // has been a way past a filter that inspected the escaped form.
    //
    // Overlong: a code point has exactly one encoding, so a longer spelling is not another one.
    assert_eq!(refusal("decodeURI('%C0%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%C1%BF')"), "URIError");
    assert_eq!(refusal("decodeURI('%E0%80%AF')"), "URIError");
    assert_eq!(refusal("decodeURI('%E0%9F%BF')"), "URIError");
    assert_eq!(refusal("decodeURI('%F0%8F%BF%BF')"), "URIError");
    // …and the shortest form at each boundary decodes, which is what says the test is "overlong"
    // and not "long".
    assert_eq!(run("decodeURI('%C2%80') === '\\u0080'"), "true");
    assert_eq!(run("decodeURI('%E0%A0%80') === '\\u0800'"), "true");
    assert_eq!(run("decodeURI('%F0%90%80%80') === '\\u{10000}'"), "true");
    // A surrogate: those code points exist to encode the other planes in UTF-16 and are not
    // characters, so UTF-8 has no encoding for one.
    assert_eq!(refusal("decodeURI('%ED%A0%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%ED%BF%BF')"), "URIError");
    // …with the code points on either side of the surrogate block decoding normally.
    assert_eq!(run("decodeURI('%ED%9F%BF') === '\\uD7FF'"), "true");
    assert_eq!(run("decodeURI('%EE%80%80') === '\\uE000'"), "true");
    // Above `U+10FFFF`, which four octets have room for and Unicode does not.
    assert_eq!(refusal("decodeURI('%F4%90%80%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%F5%80%80%80')"), "URIError");
    assert_eq!(refusal("decodeURI('%F7%BF%BF%BF')"), "URIError");
    // …and the last code point there is decodes, as the pair of units it becomes.
    assert_eq!(run("decodeURI('%F4%8F%BF%BF') === '\\u{10FFFF}'"), "true");
}

#[test]
fn decoding_above_the_basic_plane_produces_the_two_units_a_string_holds() {
    // Step 4.c.vii.9 is `UTF16EncodeCodePoint`, so a four-octet sequence becomes a surrogate pair
    // and the string's `length` is two rather than one.
    assert_eq!(run("decodeURI('%F0%9F%98%80').length"), "2");
    assert_eq!(run("decodeURI('%F0%9F%98%80').charCodeAt(0)"), "55357");
    assert_eq!(run("decodeURI('%F0%9F%98%80').charCodeAt(1)"), "56832");
    // …where anything inside the basic plane is one unit and needs no pair.
    assert_eq!(run("decodeURI('%E4%B8%AD').length"), "1");
    assert_eq!(run("decodeURI('%E4%B8%AD').charCodeAt(0)"), "20013");
    assert_eq!(run("decodeURI('%C2%80').length"), "1");
}

#[test]
fn everything_but_a_percent_passes_through_decoding_untouched() {
    // §19.2.6.6 step 4.b — the walk only ever looks at `%`, so no other character can fail and
    // none is transformed. That includes the characters `encodeURIComponent` would have escaped.
    assert_eq!(run("decodeURI('')"), "");
    assert_eq!(run("decodeURI('abc')"), "abc");
    assert_eq!(run("decodeURI(' \"<>[]')"), " \"<>[]");
    assert_eq!(run("decodeURI('\\u{1F600}').length"), "2");
    assert_eq!(run("encodeURI('')"), "");
}

#[test]
fn the_argument_is_converted_to_a_string_before_anything_is_inspected() {
    // §19.2.6.1 step 1 is `ToString`, so these are about what the argument *becomes* and a
    // `toString` that throws is what escapes — never a URIError about the object.
    assert_eq!(run("encodeURI(undefined)"), "undefined");
    assert_eq!(run("encodeURI(null)"), "null");
    assert_eq!(run("encodeURI(1.5)"), "1.5");
    assert_eq!(run("decodeURI(1)"), "1");
    // `ToString` and not `ToPrimitive` with a number hint: `toString` is asked first, so an object
    // with both methods is encoded from the string one.
    assert_eq!(
        run("encodeURI({toString(){return ' '}, valueOf(){return '_'}})"),
        "%20"
    );
    assert_eq!(run("decodeURI({})"), "[object Object]");
    assert_eq!(
        refusal("encodeURI({toString(){throw new RangeError('x')}})"),
        "RangeError"
    );
    // A Symbol has no `ToString` at all, so it is a TypeError before the walk begins — and it is
    // reached even by an argument that would go on to be a URIError.
    assert_eq!(refusal("encodeURI(Symbol())"), "TypeError");
    assert_eq!(refusal("decodeURI(Symbol())"), "TypeError");
}

#[test]
fn the_four_are_ordinary_built_in_functions_and_none_of_them_constructs() {
    // §10.3.3's two own properties, and §17's attributes on both. `length` is 1 for all four
    // because each takes exactly one argument.
    for name in [
        "decodeURI",
        "decodeURIComponent",
        "encodeURI",
        "encodeURIComponent",
    ] {
        assert_eq!(run(&format!("{name}.length")), "1");
        assert_eq!(run(&format!("{name}.name")), name);
        assert_eq!(
            run(&format!("{name}.propertyIsEnumerable('length')")),
            "false"
        );
        // §17 — the function is a writable, configurable, non-enumerable property of the global
        // object, which is what makes it shadowable and deletable and not enumerated by `for…in`.
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor(this, '{name}'); \
                 '' + d.writable + d.enumerable + d.configurable"
            )),
            "truefalsetrue"
        );
        // None of the four is a constructor, so `new` on one is a TypeError — which having no
        // `prototype` property at all is the visible half of.
        assert_eq!(
            refusal(&format!("new {name}('')")),
            "TypeError",
            "{name} must not be new-able"
        );
        assert_eq!(run(&format!("{name}.prototype")), "undefined");
    }
}
