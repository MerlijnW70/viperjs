//! Unpaired surrogates — DR-0004 through every operation a String goes through.
//!
//! A String is any sequence of UTF-16 code units (§6.1.4), and a lead or trail surrogate with no
//! partner is one of them like any other. Every row here puts one at the start, in the middle or at
//! the end of a String, joins two halves into a pair, or tells one from U+FFFD — which is what a
//! lone surrogate became wherever the engine passed a String through Rust text.
//!
//! **Every expected value was computed once with V8** (Node 22.23.3) by evaluating the row's own
//! source and spelling the answer with the same `spell` function, and is written in as that
//! answer. `spell` writes printable ASCII as itself and every other unit as `\uXXXX`, so a row
//! says exactly which units came back and cannot be satisfied by a replacement character.

use super::*;

/// A String as its code units: printable ASCII as itself, every other unit as a `\u` escape.
/// Anything that is not a String is spelled by `String(value)`.
const SPELL: &str = "function spell(value) { if (typeof value !== 'string') return String(value); \
    let out = ''; for (let i = 0; i < value.length; i++) { const unit = value.charCodeAt(i); \
    out += unit >= 0x20 && unit < 0x7f && unit !== 0x5c ? value[i] : \
    '\\\\u' + unit.toString(16).padStart(4, '0'); } return out; }";

/// What `source` evaluates to, spelled unit for unit.
fn units(source: &str) -> String {
    run(&format!("{SPELL}\nspell({source})"))
}

#[test]
fn literals_and_escapes_keep_an_unpaired_surrogate_where_it_was_written() {
    // §12.9.4 — a `\u` escape's SV is a code unit with no well-formedness condition, and `\u{…}`
    // admits a surrogate too.
    assert_eq!(units("'\\ud800ab'"), "\\ud800ab");
    assert_eq!(units("'a\\udc00b'"), "a\\udc00b");
    assert_eq!(units("'ab\\udbff'"), "ab\\udbff");
    assert_eq!(units("\"\\u{d800}\""), "\\ud800");
    assert_eq!(units("'\\udc00\\ud800'"), "\\udc00\\ud800");
    assert_eq!(units("'\\ud800\\udc00'"), "\\ud800\\udc00");
    assert_eq!(units("'\\ud800'.length"), "1");
    assert_eq!(units("'\\ud800' === '\\u{d800}'"), "true");
}

#[test]
fn from_char_code_and_the_two_readers_answer_the_unit_itself() {
    // §22.1.2.1, §22.1.3.2 and §22.1.3.4. The first row is the one that read 65533.
    assert_eq!(units("String.fromCharCode(0xd800).charCodeAt(0)"), "55296");
    assert_eq!(units("String.fromCharCode(0xd800)"), "\\ud800");
    assert_eq!(
        units("String.fromCharCode(0x61, 0xdc00, 0x62)"),
        "a\\udc00b"
    );
    assert_eq!(
        units("String.fromCharCode(0xd83d, 0xde00).codePointAt(0)"),
        "128512"
    );
    assert_eq!(units("String.fromCodePoint(0xdfff)"), "\\udfff");
    assert_eq!(units("'\\ud800ab'.codePointAt(0)"), "55296");
    assert_eq!(units("'a\\udc00b'.codePointAt(1)"), "56320");
    assert_eq!(units("'ab\\ud800'.codePointAt(2)"), "55296");
    assert_eq!(units("'\\ud800\\udc00'.codePointAt(1)"), "56320");
    assert_eq!(units("'a\\udc00b'.charCodeAt(1)"), "56320");
    assert_eq!(units("'ab\\udbff'.charCodeAt(2)"), "56319");
    assert_eq!(units("String.fromCharCode(0xd800) === '\\ufffd'"), "false");
}

#[test]
fn length_and_indexing_count_an_unpaired_surrogate_as_one_unit() {
    // §6.1.4 and §10.4.3.5 — one unit is one element, paired or not.
    assert_eq!(units("'\\ud800ab'.length"), "3");
    assert_eq!(units("'a\\udc00b'[1]"), "\\udc00");
    assert_eq!(units("'ab\\ud800'[2]"), "\\ud800");
    assert_eq!(units("new String('a\\udc00')[1]"), "\\udc00");
    assert_eq!(units("'\\ud800ab'.at(0)"), "\\ud800");
    assert_eq!(units("'ab\\udc00'.at(-1)"), "\\udc00");
    assert_eq!(units("'a\\udc00b'.charAt(1)"), "\\udc00");
    assert_eq!(units("Object.keys(new String('\\ud800a')).length"), "2");
}

#[test]
fn taking_a_piece_of_a_string_keeps_the_surrogate_or_cuts_a_pair() {
    // §22.1.3's `slice`, `substring` and `substr` count in units, so they may also cut a pair in
    // two.
    assert_eq!(units("'\\ud800ab'.slice(0, 1)"), "\\ud800");
    assert_eq!(units("'a\\udc00b'.slice(1, 2)"), "\\udc00");
    assert_eq!(units("'ab\\ud800'.slice(-1)"), "\\ud800");
    assert_eq!(units("'\\ud83d\\ude00'.slice(0, 1)"), "\\ud83d");
    assert_eq!(units("'a\\ud800\\udc00'.substring(2)"), "\\udc00");
    assert_eq!(units("'a\\ud800b'.substr(1, 1)"), "\\ud800");
    assert_eq!(units("'\\ud800,a,\\udc00'.split(',').length"), "3");
    assert_eq!(units("'\\ud800,a,\\udc00'.split(',')[2]"), "\\udc00");
    assert_eq!(units("'\\ud800\\udc00a'.split('').length"), "3");
    assert_eq!(units("'\\ud800\\udc00a'.split('')[1]"), "\\udc00");
    assert_eq!(units("'a\\ud800b'.split('\\ud800')[1]"), "b");
}

#[test]
fn searching_finds_an_unpaired_surrogate_and_not_the_replacement_character() {
    // §22.1.3's searches compare units, so U+FFFD is a different unit and is not found.
    assert_eq!(units("'a\\ud800b'.indexOf('\\ud800')"), "1");
    assert_eq!(units("'a\\ud800b'.indexOf('\\ufffd')"), "-1");
    assert_eq!(units("'\\udc00a\\udc00'.lastIndexOf('\\udc00')"), "2");
    assert_eq!(units("'x\\udc00'.includes('\\ufffd')"), "false");
    assert_eq!(units("'\\ud800x'.startsWith('\\ud800')"), "true");
    assert_eq!(units("'x\\udc00'.endsWith('\\udc00')"), "true");
    assert_eq!(units("'ab\\udc00'.search('\\udc00')"), "2");
    assert_eq!(units("'\\ud83d\\ude00'.indexOf('\\ude00')"), "1");
}

#[test]
fn the_methods_that_build_a_string_carry_an_unpaired_surrogate_through() {
    // §22.1.3's builders: nothing in them is a reason to replace a unit.
    assert_eq!(units("'\\ud800'.repeat(3)"), "\\ud800\\ud800\\ud800");
    assert_eq!(units("'a'.padStart(3, '\\udc00')"), "\\udc00\\udc00a");
    assert_eq!(units("'a'.padEnd(3, '\\ud800')"), "a\\ud800\\ud800");
    assert_eq!(units("' \\ud800 '.trim()"), "\\ud800");
    assert_eq!(units("'\\ud800'.concat('\\udc00')"), "\\ud800\\udc00");
    assert_eq!(units("'\\ud800'.concat('\\udc00').codePointAt(0)"), "65536");
    assert_eq!(units("'\\ud800a\\udc00'.toUpperCase()"), "\\ud800A\\udc00");
    assert_eq!(units("'\\ud800A\\udc00'.toLowerCase()"), "\\ud800a\\udc00");
    assert_eq!(
        units("'\\udc00e\\u0301\\ud800'.normalize()"),
        "\\udc00\\u00e9\\ud800"
    );
    assert_eq!(
        units("'\\udc00\\u00e9\\ud800'.normalize('NFD')"),
        "\\udc00e\\u0301\\ud800"
    );
    assert_eq!(
        units("'a\\ud800b'.replace('\\ud800', '\\udc00')"),
        "a\\udc00b"
    );
    assert_eq!(units("'ab'.replace('a', '$&\\ud800')"), "a\\ud800b");
    assert_eq!(
        units("'\\ud800a\\ud800'.replaceAll('\\ud800', '\\udc00')"),
        "\\udc00a\\udc00"
    );
    assert_eq!(
        units("'a\\udc00'.replace(/a/, '\\ud800$&')"),
        "\\ud800a\\udc00"
    );
    assert_eq!(
        units("'\\ud800'.anchor('\\udc00')"),
        "<a name=\"\\udc00\">\\ud800</a>"
    );
    assert_eq!(units("'\\ud800'.localeCompare('\\ufffd') === 0"), "false");
}

#[test]
fn concatenation_joins_two_halves_into_one_character() {
    // §13.15.3 and §7.1.17 — `+` joins units, and a lead next to a trail is a pair whichever way it
    // got there.
    assert_eq!(units("'\\ud800' + '\\udc00'"), "\\ud800\\udc00");
    assert_eq!(units("('\\ud800' + '\\udc00').length"), "2");
    assert_eq!(units("('\\ud83d' + '\\ude00').codePointAt(0)"), "128512");
    assert_eq!(units("('\\ud83d' + '\\ude00') === '\\u{1f600}'"), "true");
    assert_eq!(units("[...('\\ud83d' + '\\ude00')].length"), "1");
    assert_eq!(units("[...('\\ude00' + '\\ud83d')].length"), "2");
    assert_eq!(units("[...('a\\ud800' + 'b\\udc00')].length"), "4");
    assert_eq!(units("'\\udc00' + '\\ud800'"), "\\udc00\\ud800");
}

#[test]
fn comparison_tells_an_unpaired_surrogate_from_the_replacement_character() {
    // §7.2.15 and §7.2.13 compare code units, and U+D800 is below U+E000 is below U+FFFD.
    assert_eq!(units("'\\ud800' === '\\ufffd'"), "false");
    assert_eq!(units("'\\ud800' == '\\ufffd'"), "false");
    assert_eq!(units("'\\ud800' < '\\ufffd'"), "true");
    assert_eq!(units("'\\ue000' < '\\ud800'"), "false");
    assert_eq!(units("'a\\udc00' === 'a\\udc00'"), "true");
    assert_eq!(
        units("['\\ufffd', '\\ud800', '\\ue000'].sort().join(',')"),
        "\\ud800,\\ue000,\\ufffd"
    );
    assert_eq!(
        units("new Set(['\\ud800', '\\ufffd', '\\ud800']).size"),
        "2"
    );
    assert_eq!(units("['\\udc00'].includes('\\ufffd')"), "false");
    assert_eq!(units("Object.is('\\ud800', '\\ufffd')"), "false");
}

#[test]
fn a_property_key_is_the_string_unit_for_unit() {
    // §7.1.19 — a String key is the String, so two keys differing only in a surrogate are two keys.
    assert_eq!(
        units("(() => { const o = {}; o['\\ud800'] = 1; return Object.keys(o)[0]; })()"),
        "\\ud800"
    );
    assert_eq!(
        units("(() => { const o = {}; o['\\ud800'] = 1; return o['\\ufffd']; })()"),
        "undefined"
    );
    assert_eq!(units("'\\ufffd' in { '\\ud800': 1 }"), "false");
    assert_eq!(
        units("Object.keys({ ['a\\ud800b']: 1, ['a\\ufffdb']: 2 }).length"),
        "2"
    );
    assert_eq!(
        units("Object.getOwnPropertyNames({ '\\udc00': 1, '\\ud800': 2 }).join('')"),
        "\\udc00\\ud800"
    );
    assert_eq!(units("new Map([['\\ud800', 1]]).has('\\ufffd')"), "false");
    assert_eq!(units("Reflect.ownKeys({ '\\udc00': 1 })[0]"), "\\udc00");
    assert_eq!(units("Symbol('\\ud800').description"), "\\ud800");
    assert_eq!(units("({ ['\\ud800']() {} })['\\ud800'].name"), "\\ud800");
    assert_eq!(
        units(
            "Object.getOwnPropertyDescriptor({ get ['\\udc00']() { return 1; } }, '\\udc00').get.name"
        ),
        "get \\udc00"
    );
}

#[test]
fn template_literals_keep_a_surrogate_cooked_raw_and_substituted() {
    // §13.2.8 — the cooked value is a code unit like a string literal's, and a substitution is
    // `ToString`.
    assert_eq!(units("`\\ud800`"), "\\ud800");
    assert_eq!(units("`a${'\\udc00'}b`"), "a\\udc00b");
    assert_eq!(units("`${'\\ud83d'}${'\\ude00'}`.length"), "2");
    assert_eq!(units("String.raw`\\ud800`"), "\\u005cud800");
    assert_eq!(units("((s) => s[0])`x\\ud800`"), "x\\ud800");
    assert_eq!(units("((s) => s.raw[0])`\\udc00`"), "\\u005cudc00");
}

#[test]
fn json_reads_and_writes_an_unpaired_surrogate_exactly() {
    // §25.5.1 reads `\uXXXX` as a unit, and §25.5.2.3's well-formed `stringify` escapes a lone one.
    assert_eq!(
        units("JSON.parse('\"\\\\ud800x\\\\udc00\"')"),
        "\\ud800x\\udc00"
    );
    assert_eq!(units("JSON.parse('\"' + '\\ud800' + '\"')"), "\\ud800");
    assert_eq!(
        units("Object.keys(JSON.parse('{\"\\\\udc00\":1}'))[0]"),
        "\\udc00"
    );
    assert_eq!(
        units("JSON.stringify('\\ud800a\\udc00')"),
        "\"\\u005cud800a\\u005cudc00\""
    );
    assert_eq!(
        units("JSON.stringify('\\ud800\\udc00')"),
        "\"\\ud800\\udc00\""
    );
    assert_eq!(
        units("JSON.stringify({ '\\ud800': '\\udc00' })"),
        "{\"\\u005cud800\":\"\\u005cudc00\"}"
    );
    assert_eq!(
        units("JSON.stringify(JSON.parse('[\"\\\\udbff\"]'))"),
        "[\"\\u005cudbff\"]"
    );
    assert_eq!(units("JSON.stringify('\\ufffd')"), "\"\\ufffd\"");
}

#[test]
fn well_formedness_is_asked_and_answered_per_unit() {
    // §22.1.3.10 and §22.1.3.31 — only an unpaired surrogate is ill-formed, and only it is
    // replaced.
    assert_eq!(units("'\\ud800ab'.isWellFormed()"), "false");
    assert_eq!(units("'a\\udc00b'.isWellFormed()"), "false");
    assert_eq!(units("'ab\\ud800'.isWellFormed()"), "false");
    assert_eq!(units("('\\ud800' + '\\udc00').isWellFormed()"), "true");
    assert_eq!(units("'\\ud800a\\udc00'.toWellFormed()"), "\\ufffda\\ufffd");
    assert_eq!(units("'\\ud800\\udc00'.toWellFormed()"), "\\ud800\\udc00");
    assert_eq!(units("'\\ufffd'.isWellFormed()"), "true");
}

#[test]
fn joining_and_printing_a_string_keeps_its_units() {
    // §23.1.3.18, §23.2.3.16, §20.5.3.4 and §20.2.3.2 assemble a String out of Strings, and
    // assembling one is not a reason to replace a unit.
    assert_eq!(
        units("['\\ud800', 'a', '\\udc00'].join('\\ud800')"),
        "\\ud800\\ud800a\\ud800\\udc00"
    );
    assert_eq!(units("String(['\\ud800', '\\udc00'])"), "\\ud800,\\udc00");
    assert_eq!(
        units("['\\ud800', '\\udc00'].toLocaleString()"),
        "\\ud800,\\udc00"
    );
    assert_eq!(units("['\\ud83d', '\\ude00'].join('')"), "\\ud83d\\ude00");
    assert_eq!(units("new Int8Array(2).join('\\udc00')"), "0\\udc000");
    assert_eq!(units("new Error('\\ud800').toString()"), "Error: \\ud800");
    assert_eq!(
        units("(() => { const e = new Error('x'); e.name = '\\udc00'; return String(e); })()"),
        "\\udc00: x"
    );
    assert_eq!(
        units("({ ['\\ud800']: function () {} })['\\ud800'].bind().name"),
        "bound \\ud800"
    );
    assert_eq!(units("[[['\\udc00']]].flat(2).join('')"), "\\udc00");
}

#[test]
fn text_handed_to_eval_or_function_keeps_its_unpaired_surrogates() {
    // §19.2.1 and §20.2.1.1 take a String, so a literal in it may hold a surrogate no Rust text
    // can.
    assert_eq!(units("eval(\"'\" + '\\ud800' + \"'\")"), "\\ud800");
    assert_eq!(units("eval(\"'a\" + '\\udc00' + \"b'\").length"), "3");
    assert_eq!(units("eval(\"'\\\\\" + '\\ud800' + \"'\")"), "\\ud800");
    assert_eq!(
        units("eval(\"'\\\\\\\\\" + '\\ud800' + \"'\")"),
        "\\u005c\\ud800"
    );
    assert_eq!(units("eval('`' + '\\udc00' + '`')"), "\\udc00");
    assert_eq!(
        units("eval('String.raw`a\\\\' + '\\udc00' + '`')"),
        "a\\u005c\\udc00"
    );
    assert_eq!(
        units("eval(\"'\" + '\\ud83d' + \"' + '\" + '\\ude00' + \"'\").codePointAt(0)"),
        "128512"
    );
    assert_eq!(
        units("eval(\"'\\ufffd\" + '\\ud800' + \"\\ufffd'\")"),
        "\\ufffd\\ud800\\ufffd"
    );
    assert_eq!(
        units("Function(\"return '\" + '\\udc00' + \"'\")()"),
        "\\udc00"
    );
    assert_eq!(
        units("Function('a', \"return '\" + '\\ud800' + \"'\").toString().length"),
        "37"
    );
    assert_eq!(
        units("Function(\"return '\" + '\\ud800' + \"'\").toString().indexOf('\\ud800') > 0"),
        "true"
    );
    assert_eq!(
        units("(0, eval)(\"({ '\" + '\\udc00' + \"': 1 })\")['\\udc00']"),
        "1"
    );
    assert_eq!(units("eval('/' + '\\ud800' + '/').source"), "\\ud800");
    assert_eq!(
        units("eval('/\\\\' + '\\udc00' + '/').source"),
        "\\u005c\\udc00"
    );
    assert_eq!(
        units("eval('/a' + '\\udc00' + '/').test('a\\udc00')"),
        "true"
    );
    assert_eq!(
        units("new RegExp('\\\\' + '\\ud800').source"),
        "\\u005c\\ud800"
    );
    assert_eq!(
        units("(() => { try { eval('\\ud800'); } catch (e) { return e.name; } })()"),
        "SyntaxError"
    );
    assert_eq!(
        units(
            "(() => { try { new RegExp('\\\\' + '\\ud800', 'u'); } catch (e) { return e.name; } })()"
        ),
        "SyntaxError"
    );
    assert_eq!(units("eval('1 /* ' + '\\ud800' + ' */ + 1')"), "2");
}

#[test]
fn an_unpaired_surrogate_costs_what_any_unit_costs_against_the_heap_budget() {
    // DR-0013 — a String is charged by its units and a surrogate is one unit, so the budget refuses
    // a lone surrogate's repetition exactly where it refuses a letter's. And a String assembled
    // out of Strings by `join` is asked about the budget as it grows: it used to be built in Rust
    // text outside the budget and only charged once it was whole.
    //
    // The one test in this file whose answers are ViperJS's own: a budget is not something V8 has.
    let nearly = crate::heap::MAX_HEAP_BYTES - (1 << 20);
    for unit in ["a", "\\ud800"] {
        let source = format!("try {{ '{unit}'.repeat(1 << 20).length }} catch (e) {{ e.name }}");
        assert_eq!(
            run_with_heap_filled_to(nearly, &source),
            "RangeError",
            "{unit}"
        );
        let source = format!("'{unit}'.repeat(1 << 16).length");
        assert_eq!(run_with_heap_filled_to(nearly, &source), "65536", "{unit}");
    }
    let joined = "const piece = String.fromCharCode(0xd800).repeat(1 << 12); \
         try { new Array(1 << 10).fill(piece).join('').length } catch (e) { e.name }";
    assert_eq!(run_with_heap_filled_to(nearly, joined), "RangeError");
    let fits = "const piece = String.fromCharCode(0xd800).repeat(1 << 6); \
         new Array(1 << 6).fill(piece).join('').length";
    assert_eq!(run_with_heap_filled_to(nearly, fits), "4096");
}
