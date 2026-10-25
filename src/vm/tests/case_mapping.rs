//! §22.1.3.28 and §22.1.3.30 as a script sees them: full case mappings, and `Final_Sigma`.
//!
//! The tables and the context rule are tested at the code-point level in
//! `unicode_case_map/tests.rs`; this is the same behaviour reached through `String.prototype`, the
//! four entry points that share it, and a corpus whose expected output was computed once with V8
//! and written in rather than derived — so a change that agrees with itself and not with other
//! engines fails here.

use super::*;

/// A script prelude: `hex(s)` spells every code unit of `s` as a `\uXXXX` escape.
///
/// Code units rather than the string itself, because several answers below hold unpaired
/// surrogates and combining marks, and an assertion failure on those is unreadable otherwise. The
/// backslash is built from its code so that no escaping level between here and the script can eat
/// it.
const HEX: &str = "function hex(s) { var out = ''; for (var i = 0; i < s.length; i++) { \
    out += String.fromCharCode(92) + 'u' + ('000' + s.charCodeAt(i).toString(16).toUpperCase()).slice(-4); \
    } return out; }";

/// `hex` of `method` applied to the string the escapes in `input` spell.
fn mapped(input: &str, method: &str) -> String {
    run(&format!("{HEX} hex('{input}'.{method}())"))
}

/// Inputs and their `toLowerCase` and `toUpperCase`, every string as `\uXXXX` code units.
///
/// **The expected columns were computed once with V8, not by ViperJS**, by evaluating both methods
/// on each input and printing the code units. The run that produced them:
///
/// ```text
/// node 22.23.3, V8 12.4.254.21-node.57, ICU 78.3, Unicode 17.0
/// ```
static CORPUS: &[(&str, &str, &str)] = &[
    (
        r"\u039F\u0394\u039F\u03A3",
        r"\u03BF\u03B4\u03BF\u03C2",
        r"\u039F\u0394\u039F\u03A3",
    ),
    (
        r"\u039F\u03B4\u03CC\u03C2",
        r"\u03BF\u03B4\u03CC\u03C2",
        r"\u039F\u0394\u038C\u03A3",
    ),
    (
        r"\u039F\u0394\u03A5\u03A3\u03A3\u0395\u03A5\u03A3",
        r"\u03BF\u03B4\u03C5\u03C3\u03C3\u03B5\u03C5\u03C2",
        r"\u039F\u0394\u03A5\u03A3\u03A3\u0395\u03A5\u03A3",
    ),
    (
        r"\u03A3\u0399\u03A3\u03A5\u03A6\u039F\u03A3\u0020\u039A\u0391\u0399\u0020\u03A3\u0399\u03A3\u03A5\u03A6\u039F\u03A3\u002E",
        r"\u03C3\u03B9\u03C3\u03C5\u03C6\u03BF\u03C2\u0020\u03BA\u03B1\u03B9\u0020\u03C3\u03B9\u03C3\u03C5\u03C6\u03BF\u03C2\u002E",
        r"\u03A3\u0399\u03A3\u03A5\u03A6\u039F\u03A3\u0020\u039A\u0391\u0399\u0020\u03A3\u0399\u03A3\u03A5\u03A6\u039F\u03A3\u002E",
    ),
    (
        r"\u0391\u03A3\u0020\u03A3\u0391\u0020\u03A3",
        r"\u03B1\u03C2\u0020\u03C3\u03B1\u0020\u03C3",
        r"\u0391\u03A3\u0020\u03A3\u0391\u0020\u03A3",
    ),
    (
        r"\u03A3\u0391\u03A3\u002C\u0020\u03A3\u0391\u03A3\u003B\u0020\u03A3\u0391\u03A3\u0021",
        r"\u03C3\u03B1\u03C2\u002C\u0020\u03C3\u03B1\u03C2\u003B\u0020\u03C3\u03B1\u03C2\u0021",
        r"\u03A3\u0391\u03A3\u002C\u0020\u03A3\u0391\u03A3\u003B\u0020\u03A3\u0391\u03A3\u0021",
    ),
    (
        r"\u0391\u0027\u03A3",
        r"\u03B1\u0027\u03C2",
        r"\u0391\u0027\u03A3",
    ),
    (
        r"\u0391\u03A3\u0027",
        r"\u03B1\u03C2\u0027",
        r"\u0391\u03A3\u0027",
    ),
    (
        r"\u0391\u03A3\u0027\u0391",
        r"\u03B1\u03C3\u0027\u03B1",
        r"\u0391\u03A3\u0027\u0391",
    ),
    (
        r"\u0391\u2019\u03A3",
        r"\u03B1\u2019\u03C2",
        r"\u0391\u2019\u03A3",
    ),
    (
        r"\u0391\u002E\u03A3\u002E",
        r"\u03B1\u002E\u03C2\u002E",
        r"\u0391\u002E\u03A3\u002E",
    ),
    (
        r"\u0391\u00AD\u03A3",
        r"\u03B1\u00AD\u03C2",
        r"\u0391\u00AD\u03A3",
    ),
    (
        r"\u0391\u03A3\u00AD\u0392",
        r"\u03B1\u03C3\u00AD\u03B2",
        r"\u0391\u03A3\u00AD\u0392",
    ),
    (r"\u0345\u03A3", r"\u0345\u03C3", r"\u0399\u03A3"),
    (
        r"\u0391\u0345\u03A3",
        r"\u03B1\u0345\u03C2",
        r"\u0391\u0399\u03A3",
    ),
    (
        r"\u0391\u03A3\u0345",
        r"\u03B1\u03C2\u0345",
        r"\u0391\u03A3\u0399",
    ),
    (
        r"\u0391\u03A3\u0345\u0391",
        r"\u03B1\u03C3\u0345\u03B1",
        r"\u0391\u03A3\u0399\u0391",
    ),
    (
        r"\u0391\u180E\u03A3",
        r"\u03B1\u180E\u03C2",
        r"\u0391\u180E\u03A3",
    ),
    (
        r"\u0391\u03A3\u180E",
        r"\u03B1\u03C2\u180E",
        r"\u0391\u03A3\u180E",
    ),
    (r"\u03A3", r"\u03C3", r"\u03A3"),
    (r"\u03A3\u03A3", r"\u03C3\u03C2", r"\u03A3\u03A3"),
    (
        r"\u0391\u03A3\u03A3",
        r"\u03B1\u03C3\u03C2",
        r"\u0391\u03A3\u03A3",
    ),
    (
        r"\u03A3\u03A3\u0391",
        r"\u03C3\u03C3\u03B1",
        r"\u03A3\u03A3\u0391",
    ),
    (r"\u0031\u03A3", r"\u0031\u03C3", r"\u0031\u03A3"),
    (r"\u03A3\u0031", r"\u03C3\u0031", r"\u03A3\u0031"),
    (
        r"\u0391\u03A3\u0031",
        r"\u03B1\u03C2\u0031",
        r"\u0391\u03A3\u0031",
    ),
    (
        r"\uD835\uDCA2\u03A3",
        r"\uD835\uDCA2\u03C2",
        r"\uD835\uDCA2\u03A3",
    ),
    (
        r"\u0391\u03A3\uD835\uDCA2",
        r"\u03B1\u03C3\uD835\uDCA2",
        r"\u0391\u03A3\uD835\uDCA2",
    ),
    (
        r"\u0391\uD834\uDE42\u03A3",
        r"\u03B1\uD834\uDE42\u03C2",
        r"\u0391\uD834\uDE42\u03A3",
    ),
    (
        r"\u0391\uD800\u03A3",
        r"\u03B1\uD800\u03C3",
        r"\u0391\uD800\u03A3",
    ),
    (
        r"\u0391\u03A3\uDC00",
        r"\u03B1\u03C2\uDC00",
        r"\u0391\u03A3\uDC00",
    ),
    (r"\uDBFF\u03A3", r"\uDBFF\u03C3", r"\uDBFF\u03A3"),
    (
        r"\u0391\u03A3\uDBFF\uDFFF",
        r"\u03B1\u03C2\uDBFF\uDFFF",
        r"\u0391\u03A3\uDBFF\uDFFF",
    ),
    (
        r"\u0053\u0074\u0072\u0061\u00DF\u0065",
        r"\u0073\u0074\u0072\u0061\u00DF\u0065",
        r"\u0053\u0054\u0052\u0041\u0053\u0053\u0045",
    ),
    (
        r"\u0053\u0054\u0052\u0041\u0053\u0053\u0045",
        r"\u0073\u0074\u0072\u0061\u0073\u0073\u0065",
        r"\u0053\u0054\u0052\u0041\u0053\u0053\u0045",
    ),
    (
        r"\u00DF\u0020\u1E9E",
        r"\u00DF\u0020\u00DF",
        r"\u0053\u0053\u0020\u1E9E",
    ),
    (
        r"\u0130\u0073\u0074\u0061\u006E\u0062\u0075\u006C",
        r"\u0069\u0307\u0073\u0074\u0061\u006E\u0062\u0075\u006C",
        r"\u0130\u0053\u0054\u0041\u004E\u0042\u0055\u004C",
    ),
    (
        r"\u0049\u0053\u0054\u0041\u004E\u0042\u0055\u004C\u0020\u0131\u0069",
        r"\u0069\u0073\u0074\u0061\u006E\u0062\u0075\u006C\u0020\u0131\u0069",
        r"\u0049\u0053\u0054\u0041\u004E\u0042\u0055\u004C\u0020\u0049\u0049",
    ),
    (
        r"\u0149\u0020\u01F0\u0020\u0390\u0020\u03B0\u0020\u0587",
        r"\u0149\u0020\u01F0\u0020\u0390\u0020\u03B0\u0020\u0587",
        r"\u02BC\u004E\u0020\u004A\u030C\u0020\u0399\u0308\u0301\u0020\u03A5\u0308\u0301\u0020\u0535\u0552",
    ),
    (
        r"\uFB00\u0020\uFB01\u0020\uFB02\u0020\uFB03\u0020\uFB04\u0020\uFB05\u0020\uFB06\u0020\uFB13\u0020\uFB14\u0020\uFB15\u0020\uFB16\u0020\uFB17",
        r"\uFB00\u0020\uFB01\u0020\uFB02\u0020\uFB03\u0020\uFB04\u0020\uFB05\u0020\uFB06\u0020\uFB13\u0020\uFB14\u0020\uFB15\u0020\uFB16\u0020\uFB17",
        r"\u0046\u0046\u0020\u0046\u0049\u0020\u0046\u004C\u0020\u0046\u0046\u0049\u0020\u0046\u0046\u004C\u0020\u0053\u0054\u0020\u0053\u0054\u0020\u0544\u0546\u0020\u0544\u0535\u0020\u0544\u053B\u0020\u054E\u0546\u0020\u0544\u053D",
    ),
    (
        r"\u1F80\u0020\u1F88\u0020\u1FB3\u0020\u1FBC\u0020\u1FC3\u0020\u1FCC\u0020\u1FF3\u0020\u1FFC\u0020\u1FB2\u0020\u1FB4\u0020\u1FB6\u0020\u1FB7",
        r"\u1F80\u0020\u1F80\u0020\u1FB3\u0020\u1FB3\u0020\u1FC3\u0020\u1FC3\u0020\u1FF3\u0020\u1FF3\u0020\u1FB2\u0020\u1FB4\u0020\u1FB6\u0020\u1FB7",
        r"\u1F08\u0399\u0020\u1F08\u0399\u0020\u0391\u0399\u0020\u0391\u0399\u0020\u0397\u0399\u0020\u0397\u0399\u0020\u03A9\u0399\u0020\u03A9\u0399\u0020\u1FBA\u0399\u0020\u0386\u0399\u0020\u0391\u0342\u0020\u0391\u0342\u0399",
    ),
    (
        r"\u01C5\u0020\u01C8\u0020\u01CB\u0020\u01F2\u0020\u01C4\u0020\u01C6",
        r"\u01C6\u0020\u01C9\u0020\u01CC\u0020\u01F3\u0020\u01C6\u0020\u01C6",
        r"\u01C4\u0020\u01C7\u0020\u01CA\u0020\u01F1\u0020\u01C4\u0020\u01C4",
    ),
    (
        r"\u004B\u0065\u006C\u0076\u0069\u006E\u0020\u004B\u0020\u00C5\u006E\u0067\u0073\u0074\u0072\u00F6\u006D\u0020\u00C5\u0020\u03A9",
        r"\u006B\u0065\u006C\u0076\u0069\u006E\u0020\u006B\u0020\u00E5\u006E\u0067\u0073\u0074\u0072\u00F6\u006D\u0020\u00E5\u0020\u03C9",
        r"\u004B\u0045\u004C\u0056\u0049\u004E\u0020\u004B\u0020\u00C5\u004E\u0047\u0053\u0054\u0052\u00D6\u004D\u0020\u00C5\u0020\u03A9",
    ),
    (
        r"\uAB70\uAB71\uAB72\u0020\u13F8\u13F9\u0020\u13A0\u13A1",
        r"\uAB70\uAB71\uAB72\u0020\u13F8\u13F9\u0020\uAB70\uAB71",
        r"\u13A0\u13A1\u13A2\u0020\u13F0\u13F1\u0020\u13A0\u13A1",
    ),
    (
        r"\uD801\uDC00\uD801\uDC28\u0020\uD801\uDCC0\uD801\uDCD8\u0020\uD83A\uDD00\uD83A\uDD22",
        r"\uD801\uDC28\uD801\uDC28\u0020\uD801\uDCE8\uD801\uDCD8\u0020\uD83A\uDD22\uD83A\uDD22",
        r"\uD801\uDC00\uD801\uDC00\u0020\uD801\uDCC0\uD801\uDCB0\u0020\uD83A\uDD00\uD83A\uDD00",
    ),
    (
        r"\u10D0\u0020\u10D1\u0020\u1C90\u0020\u1C91",
        r"\u10D0\u0020\u10D1\u0020\u10D0\u0020\u10D1",
        r"\u1C90\u0020\u1C91\u0020\u1C90\u0020\u1C91",
    ),
    (
        r"\u24D0\u24E9\u0020\u24B6\u24CF",
        r"\u24D0\u24E9\u0020\u24D0\u24E9",
        r"\u24B6\u24CF\u0020\u24B6\u24CF",
    ),
    (r"\u01F0\u030C", r"\u01F0\u030C", r"\u004A\u030C\u030C"),
    (r"\u0131\u0307", r"\u0131\u0307", r"\u0049\u0307"),
    (r"\u0069\u0307", r"\u0069\u0307", r"\u0049\u0307"),
    (
        r"\uA7B4\uA7B5\u0020\uA7C0\uA7C1",
        r"\uA7B5\uA7B5\u0020\uA7C1\uA7C1",
        r"\uA7B4\uA7B4\u0020\uA7C0\uA7C0",
    ),
    (r"", r"", r""),
];

#[test]
fn every_entry_point_answers_the_corpus_as_v8_does() {
    for &(input, lower, upper) in CORPUS {
        assert_eq!(
            mapped(input, "toLowerCase"),
            lower,
            "toLowerCase of {input}"
        );
        assert_eq!(
            mapped(input, "toUpperCase"),
            upper,
            "toUpperCase of {input}"
        );
        // §22.1.3.26 and §22.1.3.27 without a locale, in an engine without locale data, are the
        // same mapping under a second name.
        assert_eq!(
            mapped(input, "toLocaleLowerCase"),
            lower,
            "toLocaleLowerCase of {input}"
        );
        assert_eq!(
            mapped(input, "toLocaleUpperCase"),
            upper,
            "toLocaleUpperCase of {input}"
        );
    }
}

#[test]
fn a_capital_sigma_lowers_to_the_final_form_only_at_the_end_of_a_word() {
    // The case audit round H14 found: V8 answers `οδος`, and an engine without `Final_Sigma`
    // answers `οδοσ`.
    assert_eq!(run("'ΟΔΟΣ'.toLowerCase()"), "οδος");
    assert_eq!(
        run("'ΟΔΟΣ'.toLowerCase() === '\\u03BF\\u03B4\\u03BF\\u03C2'"),
        "true"
    );
    assert_eq!(run("'ΟΔΟΣ'.toLocaleLowerCase()"), "οδος");
    // Word-medial, alone, and at either end of the string.
    assert_eq!(run("'ΑΣΑ'.toLowerCase()"), "ασα");
    assert_eq!(run("'Σ'.toLowerCase()"), "σ");
    assert_eq!(run("'ΣΑ'.toLowerCase()"), "σα");
    assert_eq!(run("'ΑΣ'.toLowerCase()"), "ας");
    // Case-ignorable code points are looked through, on both sides.
    assert_eq!(run("\"Α'Σ\".toLowerCase()"), "α'ς");
    assert_eq!(run("\"ΑΣ'Α\".toLowerCase()"), "ασ'α");
    assert_eq!(run("'Α\\u0345Σ'.toLowerCase() === 'α\\u0345ς'"), "true");
    assert_eq!(run("'ΑΣ\\u0345Α'.toLowerCase() === 'ασ\\u0345α'"), "true");
    // The result is one code unit per sigma either way: `Final_Sigma` never changes a length.
    assert_eq!(run("'ΟΔΟΣ'.toLowerCase().length"), "4");
}

#[test]
fn the_length_changing_mappings_go_both_ways_and_do_not_round_trip() {
    assert_eq!(run("'ß'.toUpperCase()"), "SS");
    assert_eq!(run("'ß'.toUpperCase().toLowerCase()"), "ss");
    assert_eq!(run("'İ'.toLowerCase().length"), "2");
    assert_eq!(run("'İ'.toLowerCase() === 'i\\u0307'"), "true");
    assert_eq!(
        run("'İ'.toLowerCase().toUpperCase() === 'I\\u0307'"),
        "true"
    );
    assert_eq!(run("'ﬃ'.toUpperCase()"), "FFI");
    assert_eq!(run("'ᾳ'.toUpperCase()"), "ΑΙ");
    assert_eq!(run("'ẞ'.toLowerCase()"), "ß");
}
