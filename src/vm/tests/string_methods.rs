//! §22.1.3 — the rest of `String.prototype`, and the two static methods that build a string.
//!
//! Separate from [`super::strings`], which is about the constructor and the exotic object. Every
//! row here was checked against V8 first, and the four that were *wrong* when first written are
//! commented as such — they are the cases a reader would otherwise assume were arbitrary.

use super::*;

#[test]
fn a_place_in_a_string_can_be_asked_for_from_either_end() {
    // §22.1.3.1 `at` — counts back for a negative index and answers `undefined` rather than `""`,
    // which are the two ways it is not `charAt` under a newer name.
    assert_eq!(run("'abc'.at(0)"), "a");
    assert_eq!(run("'abc'.at(-1)"), "c");
    assert_eq!(run("'abc'.at(1.9)"), "b");
    assert_eq!(run("typeof 'abc'.at(3)"), "undefined");
    assert_eq!(run("'abc'.at(-4) === undefined"), "true");
    // §22.1.3.4 `codePointAt` — the whole code point at a leading surrogate, and the lone unit at
    // the trailing one. Reading the second half answers that half, which is the asymmetry that
    // makes this not `charCodeAt`.
    assert_eq!(run("'\\ud83d\\ude00a'.codePointAt(0)"), "128512");
    assert_eq!(run("'\\ud83d\\ude00a'.codePointAt(1)"), "56832");
    assert_eq!(run("'\\ud83d\\ude00a'.codePointAt(2)"), "97");
    assert_eq!(run("typeof 'a'.codePointAt(5)"), "undefined");
    // A pair is only a pair when *both* halves are right, and each half is its own test. A
    // trailing surrogate after an ordinary character is not a pair, and a leading one before an
    // ordinary character is not either — so both answer the first unit alone.
    assert_eq!(run("'a\\ude00'.codePointAt(0)"), "97");
    assert_eq!(run("'\\ud83da'.codePointAt(0)"), "55357");
    assert_eq!(run("'\\ud83d'.codePointAt(0)"), "55357");
}

#[test]
fn the_three_ways_of_asking_whether_a_string_contains_another() {
    assert_eq!(run("'abcdef'.includes('cd')"), "true");
    assert_eq!(run("'abcdef'.includes('cd', 3)"), "false");
    assert_eq!(run("'abcdef'.includes('')"), "true");
    assert_eq!(run("'abcdef'.includes('a', -5)"), "true");
    assert_eq!(run("'abcdef'.startsWith('abc')"), "true");
    assert_eq!(run("'abcdef'.startsWith('bcd', 1)"), "true");
    assert_eq!(run("'abcdef'.endsWith('def')"), "true");
    // §22.1.3.7's position argument says where the match must *end*, which is why this is true —
    // the other two would read it as where to start.
    assert_eq!(run("'abcdef'.endsWith('abc', 3)"), "true");
    assert_eq!(run("'abcdef'.endsWith('abc', undefined)"), "false");
}

#[test]
fn a_piece_of_a_string_is_taken_by_four_rules_that_differ() {
    // B.2.2.1 `substr` — a *length* rather than an end position, and a negative start counting
    // from the end. Neither `slice` nor `substring` does both.
    assert_eq!(run("'abcdef'.substr(1, 2)"), "bc");
    assert_eq!(run("'abcdef'.substr(-2)"), "ef");
    assert_eq!(run("'abcdef'.substr(-2, 1)"), "e");
    assert_eq!(run("'abcdef'.substr(1)"), "bcdef");
    // §22.1.3.20 `repeat` — a **RangeError** for a negative count, which is the one place a String
    // method refuses a number outright rather than clamping it.
    assert_eq!(run("'ab'.repeat(3)"), "ababab");
    assert_eq!(run("'ab'.repeat(0) === ''"), "true");
    // Nothing repeated a hundred quadrillion times is nothing, and answering so must not take a
    // hundred quadrillion turns of a loop. This did not return at all until the repeat was bounded
    // by the length of its answer rather than by the count it was given.
    assert_eq!(run("''.repeat(1e17) === ''"), "true");
    assert_eq!(
        run(
            "(function () { try { return 'a'.repeat(-1); } catch (e) { return e.constructor.name; } })()"
        ),
        "RangeError"
    );
    // §22.1.3.17 — the filler repeats and is then cut to the gap, so it need not divide it.
    assert_eq!(run("'5'.padStart(3, '0')"), "005");
    assert_eq!(run("'5'.padEnd(3, 'ab')"), "5ab");
    assert_eq!(run("'abc'.padStart(2, '0')"), "abc");
    assert_eq!(run("'abc'.padStart(6)"), "   abc");
    // An empty filler has nothing to pad with, and step 5 answers the string rather than looping.
    assert_eq!(run("'a'.padStart(3, '') === 'a'"), "true");
    // …but a length no String could have is a RangeError and not a shrug. This answered `"a"`
    // when first written, because the two ways of having no gap to fill had been made one.
    assert_eq!(
        run(
            "(function () { try { return 'a'.padStart(1e21); } catch (e) { return e.constructor.name; } })()"
        ),
        "RangeError"
    );
}

#[test]
fn splitting_has_four_answers_that_look_like_special_cases_and_are_not() {
    assert_eq!(run("'a,b,c'.split(',').join('|')"), "a|b|c");
    assert_eq!(run("'aXXb'.split('XX').join('|')"), "a|b");
    assert_eq!(run("'a,b,c'.split(',', 2).join('|')"), "a|b");
    // No separator is one piece; an empty separator is one piece per unit; a limit of zero is no
    // pieces at all; and the empty string on an empty separator has nothing to cut between.
    assert_eq!(run("'abc'.split().length"), "1");
    assert_eq!(run("'abc'.split(undefined).join('|')"), "abc");
    assert_eq!(run("'abc'.split('').join('|')"), "a|b|c");
    assert_eq!(run("''.split(',').length"), "1");
    assert_eq!(run("''.split('').length"), "0");
    assert_eq!(run("'a,b,c'.split(',', 0).length"), "0");
    // §22.1.3.22 step 6 is `ToUint32`, so a negative limit wraps and means every piece. This
    // answered zero when first written, from clamping where the specification wraps.
    assert_eq!(run("'abc'.split(',', -1).length"), "1");
    assert_eq!(run("'a,b'.split(',', -1).length"), "2");
    // §7.3.18 `CreateArrayFromList` — the pieces are an ordinary dense array, so its elements are
    // writable, enumerable and configurable like any other array literal's.
    assert_eq!(
        run("(function () { var a = 'a,b'.split(','); a[0] = 'z'; return a[0]; })()"),
        "z"
    );
    assert_eq!(
        run("(function () { var a = 'a,b'.split(','); return Object.keys(a).join('-'); })()"),
        "0-1"
    );
    for attribute in ["writable", "enumerable", "configurable"] {
        assert_eq!(
            run(&format!(
                "Object.getOwnPropertyDescriptor('a,b'.split(','), '0').{attribute}"
            )),
            "true"
        );
    }
}

#[test]
fn case_is_mapped_over_code_points_and_may_change_the_length() {
    assert_eq!(run("'aBc'.toUpperCase()"), "ABC");
    assert_eq!(run("'aBc'.toLowerCase()"), "abc");
    assert_eq!(run("''.toUpperCase() === ''"), "true");
    // §22.1.3.29 — the Unicode Default Case Conversion is not one-to-one, so the result can be
    // longer than what went in. A per-unit table would answer `"ß"` here.
    assert_eq!(run("'\\u00df'.toUpperCase()"), "SS");
    assert_eq!(run("'\\u00df'.toUpperCase().length"), "2");
    assert_eq!(run("'\\u0130'.toLowerCase().length"), "2");
    // …and it is over code *points*, so a surrogate pair is mapped as the one character it is.
    assert_eq!(run("'\\ud83d\\ude00'.toUpperCase().length"), "2");
    // A lone surrogate has no case and no character. It is copied through rather than replaced,
    // which is what keeps the length at one.
    assert_eq!(run("'\\ud800'.toUpperCase().length"), "1");
    assert_eq!(run("'\\ud800'.toUpperCase().charCodeAt(0)"), "55296");
    // §22.1.3.26 lets an implementation without locale data answer the locale-independent
    // mapping, and praxis has none — so these are the same function's answer, deliberately.
    assert_eq!(run("'abc'.toLocaleUpperCase()"), "ABC");
    assert_eq!(run("'ABC'.toLocaleLowerCase()"), "abc");
}

#[test]
fn trimming_removes_a_longer_list_of_characters_than_it_looks() {
    assert_eq!(run("'  ab  '.trim()"), "ab");
    assert_eq!(run("'  ab  '.trimStart()"), "ab  ");
    assert_eq!(run("'  ab  '.trimEnd()"), "  ab");
    assert_eq!(run("'   '.trim() === ''"), "true");
    // §22.1.3.31 removes every `White_Space` code point and the line terminators, which includes a
    // no-break space and a byte-order mark — and does *not* include a zero-width space.
    assert_eq!(run("'\\u00a0\\ufeffab'.trim()"), "ab");
    assert_eq!(run("'\\u200bab'.trim().length"), "3");
    // B.2.2.14 and B.2.2.15 — the same function objects under Annex B names, which is stronger
    // than two functions that behave alike and is what the specification actually says.
    assert_eq!(
        run("String.prototype.trimLeft === String.prototype.trimStart"),
        "true"
    );
    assert_eq!(
        run("String.prototype.trimRight === String.prototype.trimEnd"),
        "true"
    );
}

#[test]
fn a_string_can_be_built_from_code_points_or_from_a_raw_template() {
    assert_eq!(run("String.fromCodePoint(128512).length"), "2");
    assert_eq!(run("String.fromCodePoint(65, 66)"), "AB");
    assert_eq!(run("String.fromCodePoint() === ''"), "true");
    assert_eq!(run("String.fromCodePoint(1114111).length"), "2");
    // …and the two units it becomes, which a length alone would not pin: §11.1.1's split of the
    // code point into a leading and a trailing surrogate, in that order.
    assert_eq!(run("String.fromCodePoint(128512).charCodeAt(0)"), "55357");
    assert_eq!(run("String.fromCodePoint(128512).charCodeAt(1)"), "56832");
    assert_eq!(run("String.fromCodePoint(65536).charCodeAt(0)"), "55296");
    assert_eq!(run("String.fromCodePoint(65536).charCodeAt(1)"), "56320");
    // A lone surrogate *is* a code point, and this is the only way to put one in a string
    // deliberately. It threw when first written, from going through a type that cannot hold one.
    assert_eq!(run("String.fromCodePoint(0xd800).length"), "1");
    // §22.1.2.2 steps 5.b to 5.d — a fraction, a negative and anything past the last code point
    // are each a RangeError, which is the whole difference from `fromCharCode`.
    for asked in ["1.5", "-1", "1114112"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return String.fromCodePoint({asked}); }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "RangeError"
        );
    }
    // §22.1.2.4 — reads `raw` as an array-like, so it works on a hand-made object and not only on
    // a tagged template. The substitutions go *between* the pieces, so the last has none after it.
    assert_eq!(run("String.raw({raw: ['a', 'b']}, 1)"), "a1b");
    assert_eq!(run("String.raw({raw: ['a']}, 1, 2)"), "a");
    assert_eq!(run("String.raw({raw: []}) === ''"), "true");
}

#[test]
fn comparing_two_strings_is_consistent_even_without_a_locale() {
    // §22.1.3.12 specifies the *sign* and leaves the order to the implementation when there is no
    // locale data — so these rows assert what the specification requires and not what any one
    // engine answers. praxis compares code units, which is the order `<` already uses.
    assert_eq!(run("'abc'.localeCompare('abc')"), "0");
    assert_eq!(run("'abc'.localeCompare('abd') < 0"), "true");
    assert_eq!(run("'abd'.localeCompare('abc') > 0"), "true");
    assert_eq!(run("'a'.localeCompare('ab') < 0"), "true");
    assert_eq!(run("''.localeCompare('')"), "0");
    // Consistent both ways round, which is the one property §22.1.3.12 does insist on.
    assert_eq!(
        run(
            "(function () { var a = 'x'.localeCompare('y'); var b = 'y'.localeCompare('x'); \
             return a < 0 && b > 0; })()"
        ),
        "true"
    );
}

#[test]
fn a_string_can_say_whether_its_surrogates_are_paired_and_mend_them() {
    // §22.1.3.9 — a lone surrogate of **either** kind makes a string ill-formed. A walk that only
    // looked for an unmatched lead would call a run of trailing surrogates well-formed.
    assert_eq!(
        run(
            r"['\u{1F600}'.isWellFormed(), 'ab'.isWellFormed(), '\uD800'.isWellFormed(), '\uDC00'.isWellFormed(), 'a\uD800b'.isWellFormed()].join(',')"
        ),
        "true,true,false,false,false"
    );
    // §22.1.3.29 — one replacement character per lone *code unit*, so the answer is always the
    // same length as the receiver. Two leading surrogates in a row become two, because each is
    // judged where it stands rather than read as a broken pair between them.
    assert_eq!(
        run(
            r"['\uD800'.toWellFormed().charCodeAt(0), '\uD800\uD800'.toWellFormed().length, '\u{1F600}'.toWellFormed().length, 'a\uDC00b'.toWellFormed().charCodeAt(1)].join(',')"
        ),
        "65533,2,2,65533"
    );
    // A pair survives untouched, which is what separates mending from replacing every surrogate.
    assert_eq!(
        run(r"('\u{1F600}'.toWellFormed() === '\u{1F600}') + ',' + 'plain'.toWellFormed()"),
        "true,plain"
    );
    // Both take no arguments and neither coerces the receiver away — §17's ordinary shape.
    assert_eq!(
        run(
            "[String.prototype.isWellFormed.length, String.prototype.toWellFormed.length, \
             String.prototype.isWellFormed.call(1), String.prototype.toWellFormed.call(true)].join(',')"
        ),
        "0,0,true,true"
    );
}

#[test]
fn a_promise_and_its_two_functions_come_out_together() {
    // §27.2.4.8 — the three properties are `CreateDataProperty`'s, so all three are writable,
    // enumerable and configurable. That is the shape for an object a built-in *hands over* and
    // not the shape of a property a built-in *has*: the caller is expected to take it apart.
    assert_eq!(
        run("var r = Promise.withResolvers(); \
             var d = Object.getOwnPropertyDescriptor(r, 'promise'); \
             [typeof r.promise, typeof r.resolve, typeof r.reject, r.promise instanceof Promise, \
              d.writable, d.enumerable, d.configurable, Object.keys(r).join('/')].join(',')"),
        "object,function,function,true,true,true,true,promise/resolve/reject"
    );
    // The two functions really do settle the promise they came with.
    assert_eq!(
        run_settled(
            "var out = 'pending'; var r = Promise.withResolvers(); \
             r.promise.then(function (v) { out = 'resolved:' + v }, function (e) { out = 'rejected:' + e }); \
             r.resolve(7);",
            "out"
        ),
        "resolved:7"
    );
    // `NewPromiseCapability(this)` and not `NewPromiseCapability(%Promise%)`, which is why the
    // receiver decides the kind and a `this` that is not a constructor is a TypeError.
    assert_eq!(
        run("class Sub extends Promise {} \
             var made = Promise.withResolvers.call(Sub).promise; \
             var caught = 'none'; \
             try { Promise.withResolvers.call(1) } catch (e) { caught = e.constructor.name } \
             (made instanceof Sub) + ',' + caught + ',' + Promise.withResolvers.length"),
        "true,TypeError,0"
    );
}
