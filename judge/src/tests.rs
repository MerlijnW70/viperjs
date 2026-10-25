//! What a judgement comes to, said as sentences — and every attack on it the design names.

use super::*;

const WORD_COUNT: &str = "export default function count(text) { \
     const words = text.match(/\\S+/g); return words === null ? 0 : words.length; }";

fn input(source: &str, cases: Vec<Case>) -> Input {
    Input {
        steps: 2_000_000,
        heap: 16 << 20,
        seed: 7,
        clock: 1_700_000_000_000.0,
        source: source.to_string(),
        cases,
    }
}

fn text_case(text: &str, expected: f64) -> Case {
    Case {
        arguments: vec![Primitive::text(text)],
        expected: Primitive::Number(expected),
    }
}

fn verdicts(source: &str, cases: Vec<Case>) -> Vec<Verdict> {
    judge_all(&input(source, cases))
}

fn one(source: &str) -> Verdict {
    verdicts(source, vec![text_case("a b", 2.0)])
        .pop()
        .unwrap_or(Verdict::Engine)
}

#[test]
fn a_correct_delivery_passes_and_a_wrong_one_is_told_what_it_answered() {
    let cases = vec![
        text_case("", 0.0),
        text_case("one", 1.0),
        text_case("one\ttwo\u{3000}three", 3.0),
    ];
    assert_eq!(
        verdicts(WORD_COUNT, cases.clone()),
        vec![Verdict::Pass, Verdict::Pass, Verdict::Pass]
    );
    let split = "export default function count(text) { return text.split(' ').length; }";
    assert_eq!(
        verdicts(split, cases),
        vec![
            Verdict::Wrong(Primitive::Number(1.0)),
            Verdict::Pass,
            Verdict::Wrong(Primitive::Number(1.0))
        ]
    );
}

#[test]
fn rewriting_the_built_ins_cannot_change_a_verdict() {
    // The comparison is Rust's, so a delivery that rewrites everything a JavaScript harness would
    // lean on — the serializer, the iteration, equality's helpers — has nothing to rewrite here.
    let tamper = "JSON.stringify = () => '[2]'; Array.prototype.map = function () { return [2]; }; \
         Object.is = () => true; Number.prototype.valueOf = () => 2; \
         export default function count(text) { return 99; }";
    assert_eq!(one(tamper), Verdict::Wrong(Primitive::Number(99.0)));
}

#[test]
fn an_answer_that_is_not_a_primitive_is_refused_unread() {
    // An object with a `valueOf` that answers 2 is the classic: converted, it would pass. Read as
    // an object it is refused, and its code never runs.
    for source in [
        "export default function () { return { valueOf() { return 2; }, toString() { return '2'; } }; }",
        "export default function () { return [2]; }",
        "export default function () { return Symbol('2'); }",
        "export default function () { return 2n; }",
        "export default function () { return new Number(2); }",
    ] {
        assert_eq!(one(source), Verdict::NotPrimitive, "{source}");
    }
}

#[test]
fn every_case_starts_from_nothing_the_previous_one_left() {
    // A counter at module level would reach 2 on the second case if anything carried over.
    let counter = "let calls = 0; export default function () { calls += 1; globalThis.seen = calls; return calls; }";
    let cases = vec![
        Case {
            arguments: vec![],
            expected: Primitive::Number(1.0),
        };
        3
    ];
    assert_eq!(
        verdicts(counter, cases),
        vec![Verdict::Pass, Verdict::Pass, Verdict::Pass]
    );
}

#[test]
fn the_same_input_gives_the_same_verdicts_on_every_run() {
    // What makes a verdict checkable by someone else: the generator, the clock and the stopping
    // point are all fixed by the input, so a delivery that branches on any of them still answers
    // the same twice.
    let reads = "export default function () { let n = 0; while (Math.random() > 1e-3) n++; \
         return String(n) + '|' + Date.now() + '|' + new Date().toISOString(); }";
    let case = vec![Case {
        arguments: vec![],
        expected: Primitive::Null,
    }];
    let first = verdicts(reads, case.clone());
    assert_eq!(first, verdicts(reads, case));
    let Some(Verdict::Wrong(Primitive::String(units))) = first.first() else {
        panic!("{first:?}");
    };
    let text = String::from_utf16_lossy(units);
    assert!(
        text.ends_with("|1700000000000|2023-11-14T22:13:20.000Z"),
        "{text}"
    );
    let spin = "export default function () { let n = 0; while (true) n++; }";
    assert_eq!(one(spin), Verdict::Stopped);
}

#[test]
fn a_runaway_is_stopped_wherever_it_runs() {
    assert_eq!(
        one("while (true) {} export default function () {}"),
        Verdict::Stopped
    );
    assert_eq!(
        one("export default function f() { try { while (true) {} } finally { return 2; } }"),
        Verdict::Stopped
    );
    assert_eq!(
        one(
            "export default function () { const a = []; a.length = 2e8; return a.join('').length; }"
        ),
        Verdict::Stopped
    );
    let bomb = one(
        "export default function () { const a = []; for (;;) a.push(new Array(1e5).fill(1)); }",
    );
    assert!(
        matches!(&bomb, Verdict::Threw(text) if text.contains("heap")) || bomb == Verdict::Stopped,
        "{bomb:?}"
    );
    let deep = one("export default function f(n) { return f(n) + 1; }");
    assert!(
        matches!(&deep, Verdict::Threw(text) if text.starts_with("RangeError")),
        "{deep:?}"
    );
}

#[test]
fn a_module_that_cannot_be_called_says_why() {
    assert!(matches!(
        one("export default function ("),
        Verdict::NoModule(_)
    ));
    assert!(matches!(
        one("throw new Error('at load'); export default function () {}"),
        Verdict::NoModule(text) if text.contains("at load")
    ));
    assert!(matches!(
        one("import fs from 'node:fs'; export default function () {}"),
        Verdict::NoModule(text) if text.starts_with("SyntaxError")
    ));
    assert!(matches!(
        one("export const x = 1;"),
        Verdict::NoModule(text) if text.contains("no default export")
    ));
    assert!(matches!(one("export default 2;"), Verdict::NoModule(_)));
    assert!(matches!(one("export default {};"), Verdict::Threw(_)));
}

#[test]
fn nothing_of_the_host_is_there_to_reach() {
    let probe = "export default function () { return [typeof process, typeof require, typeof fetch, \
         typeof WebAssembly, typeof setTimeout, typeof Deno, typeof console].join(','); }";
    assert_eq!(
        one(probe),
        Verdict::Wrong(Primitive::text(
            "undefined,undefined,undefined,undefined,undefined,undefined,undefined"
        ))
    );
    let escape = "export default function () { return Function('return typeof process')(); }";
    assert_eq!(one(escape), Verdict::Wrong(Primitive::text("undefined")));
    let dynamic = "export default async function () { await import('node:fs'); return 2; }";
    assert_eq!(one(dynamic), Verdict::NotPrimitive);
}

#[test]
fn a_delivery_cannot_make_a_verdict_large() {
    let loud = "export default function () { throw 'x'.repeat(1e6); }";
    assert!(matches!(one(loud), Verdict::Threw(text) if text.len() == DETAIL_BYTES));
    let long = "export default function () { return 'é'.repeat(1e6); }";
    assert!(matches!(
        one(long),
        Verdict::Wrong(Primitive::String(units))
            if to_wtf8(&units).len() <= DETAIL_BYTES && !units.is_empty()
    ));
}

#[test]
fn numbers_compare_as_answers_and_not_as_bits() {
    assert!(Primitive::Number(0.0).answers(&Primitive::Number(-0.0)));
    assert!(Primitive::Number(f64::NAN).answers(&Primitive::Number(f64::NAN)));
    assert!(!Primitive::Number(1.0).answers(&Primitive::Number(f64::NAN)));
    assert!(!Primitive::Number(1.0).answers(&Primitive::text("1")));
    assert!(!Primitive::Null.answers(&Primitive::Undefined));
    assert!(Primitive::text("a").answers(&Primitive::text("a")));
    assert!(!Primitive::String(vec![0xd800]).answers(&Primitive::text("\u{fffd}")));
}

#[test]
fn the_wire_format_round_trips_and_refuses_anything_else() {
    let cases = vec![
        Case {
            arguments: vec![
                Primitive::Undefined,
                Primitive::Null,
                Primitive::Boolean(true),
                Primitive::Boolean(false),
                Primitive::Number(-1.5),
                Primitive::text("é\u{0}"),
                Primitive::String(vec![0xdbff, 0x61, 0xdc00, 0xd83d, 0xde00]),
            ],
            expected: Primitive::text("x"),
        },
        text_case("a", 1.0),
    ];
    let written = write_input(&input(WORD_COUNT, cases.clone()));
    assert_eq!(read_input(&written), Some(input(WORD_COUNT, cases)));
    assert_eq!(read_input(&[]), None);
    let mut trailing = written.clone();
    trailing.push(0);
    assert_eq!(read_input(&trailing), None);
    let mut version = written.clone();
    version[0] = 2;
    assert_eq!(read_input(&version), None);
    for cut in 0..written.len() {
        assert_eq!(read_input(&written[..cut]), None, "cut at {cut}");
    }
    let mut many = write_input(&input("", vec![]));
    let at = many.len() - 4;
    many[at..].copy_from_slice(&(MAX_CASES + 1).to_le_bytes());
    assert_eq!(read_input(&many), None);
}

#[test]
fn the_output_carries_each_verdict_and_its_detail() {
    let source =
        "export default function (x) { if (x === 'throw') throw new Error('no'); return x; }";
    let case = |argument: &str, expected: &str| Case {
        arguments: vec![Primitive::text(argument)],
        expected: Primitive::text(expected),
    };
    let out = judge(&write_input(&input(
        source,
        vec![case("a", "a"), case("b", "c"), case("throw", "x")],
    )));
    let mut expected = Vec::new();
    expected.extend_from_slice(&VERSION.to_le_bytes());
    expected.push(0);
    expected.extend_from_slice(&3_u32.to_le_bytes());
    expected.push(0);
    expected.push(1);
    expected.push(5);
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.push(b'b');
    expected.push(3);
    expected.extend_from_slice(&9_u32.to_le_bytes());
    expected.extend_from_slice(b"Error: no");
    assert_eq!(out, expected);
    let mut refused = Vec::new();
    refused.extend_from_slice(&VERSION.to_le_bytes());
    refused.push(1);
    refused.extend_from_slice(&0_u32.to_le_bytes());
    assert_eq!(judge(b"nonsense"), refused);
}

// The expected values below that are a JavaScript engine's answers were computed once with V8
// (Node 22.23.3) and are written in as numbers, so that these tests say what ECMAScript says and
// not what ViperJS happens to do.

#[test]
fn an_unpaired_surrogate_is_an_answer_of_its_own_and_not_the_replacement_character() {
    let from_code = "export default (unit) => String.fromCharCode(unit);";
    let lone = |unit: f64, expected: Primitive| Case {
        arguments: vec![Primitive::Number(unit)],
        expected,
    };
    assert_eq!(
        verdicts(
            from_code,
            vec![
                lone(55296.0, Primitive::String(vec![0xd800])),
                lone(55296.0, Primitive::text("\u{fffd}")),
                lone(65533.0, Primitive::text("\u{fffd}")),
                lone(65533.0, Primitive::String(vec![0xd800])),
            ]
        ),
        vec![
            Verdict::Pass,
            Verdict::Wrong(Primitive::String(vec![0xd800])),
            Verdict::Pass,
            Verdict::Wrong(Primitive::text("\u{fffd}")),
        ]
    );
}

#[test]
fn an_argument_reaches_the_delivery_unit_for_unit() {
    // V8: "\udc00".length + ":" + "\udc00".charCodeAt(0) is "1:56320", and the two halves of
    // U+1F600 joined are the character, of length 2 and code point 128512.
    let describe = "export default (a, b) => { const s = a + b; \
         return s.length + ':' + s.charCodeAt(0) + ':' + s.codePointAt(0) + ':' + s.isWellFormed(); }";
    let case = |a: Vec<u16>, b: Vec<u16>, expected: &str| Case {
        arguments: vec![Primitive::String(a), Primitive::String(b)],
        expected: Primitive::text(expected),
    };
    assert_eq!(
        verdicts(
            describe,
            vec![
                case(vec![0xdc00], vec![], "1:56320:56320:false"),
                case(vec![0xd83d], vec![0xde00], "2:55357:128512:true"),
                case(vec![0x61, 0xd800], vec![0x62], "3:97:97:false"),
            ]
        ),
        vec![Verdict::Pass, Verdict::Pass, Verdict::Pass]
    );
    let echo = "export default (s) => s;";
    let back = Case {
        arguments: vec![Primitive::String(vec![0xd800, 0x61, 0xdfff])],
        expected: Primitive::String(vec![0xd800, 0x61, 0xdfff]),
    };
    assert_eq!(verdicts(echo, vec![back]), vec![Verdict::Pass]);
}

#[test]
fn the_output_writes_an_unpaired_surrogate_as_its_three_wtf8_bytes() {
    let source = "export default () => 'a' + String.fromCharCode(0xdc00) + '\\ufffd';";
    let out = judge(&write_input(&input(
        source,
        vec![Case {
            arguments: vec![],
            expected: Primitive::Null,
        }],
    )));
    let mut expected = Vec::new();
    expected.extend_from_slice(&VERSION.to_le_bytes());
    expected.push(0);
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.push(1);
    expected.push(5);
    expected.extend_from_slice(&7_u32.to_le_bytes());
    expected.extend_from_slice(&[b'a', 0xed, 0xb0, 0x80, 0xef, 0xbf, 0xbd]);
    assert_eq!(out, expected);
}

#[test]
fn wtf8_is_utf8_for_every_character_and_three_bytes_for_every_unpaired_surrogate() {
    for point in 0..=0x10_ffff_u32 {
        let Some(ch) = char::from_u32(point) else {
            continue;
        };
        let units: Vec<u16> = ch.to_string().encode_utf16().collect();
        assert_eq!(to_wtf8(&units), ch.to_string().into_bytes(), "{point:#x}");
        assert_eq!(
            from_wtf8(ch.to_string().as_bytes()),
            Some(units),
            "{point:#x}"
        );
    }
    for unit in 0xd800..=0xdfff_u16 {
        let bytes = to_wtf8(&[unit]);
        assert_eq!(bytes.len(), 3);
        assert_eq!(bytes[0], 0xed);
        assert_eq!(from_wtf8(&bytes), Some(vec![unit]));
    }
    assert_eq!(to_wtf8(&[0xd800]), [0xed, 0xa0, 0x80]);
    assert_eq!(to_wtf8(&[0xdfff]), [0xed, 0xbf, 0xbf]);
    assert_eq!(to_wtf8(&[0xd83d, 0xde00]), "\u{1f600}".as_bytes());
    // A trail before a lead is two unpaired surrogates, and stays two.
    assert_eq!(
        from_wtf8(&[0xed, 0xb0, 0x80, 0xed, 0xa0, 0x80]),
        Some(vec![0xdc00, 0xd800])
    );
}

#[test]
fn wtf8_refuses_a_pair_written_as_two_surrogates_and_everything_utf8_refuses() {
    for bytes in [
        &[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80][..],
        &[0xc0, 0x80],
        &[0xc1, 0xbf],
        &[0xe0, 0x9f, 0xbf],
        &[0xf0, 0x8f, 0xbf, 0xbf],
        &[0xf4, 0x90, 0x80, 0x80],
        &[0xf5, 0x80, 0x80, 0x80],
        &[0x80],
        &[0xed, 0xa0],
        &[0xe2, 0x82],
        &[0xe2, 0x28, 0xa1],
        &[0xff],
    ] {
        assert_eq!(from_wtf8(bytes), None, "{bytes:x?}");
    }
    let written = write_input(&input("", vec![text_case("abcdef", 1.0)]));
    let at = written
        .windows(6)
        .position(|window| window == b"abcdef")
        .unwrap_or_default();
    let mut lone = written.clone();
    lone[at..at + 6].copy_from_slice(&[0xed, 0xa0, 0xbd, b'd', b'e', b'f']);
    let read = read_input(&lone).map(|read| read.cases[0].arguments.clone());
    assert_eq!(
        read,
        Some(vec![Primitive::String(vec![0xd83d, 0x64, 0x65, 0x66])])
    );
    let mut split = written;
    split[at..at + 6].copy_from_slice(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]);
    assert_eq!(read_input(&split), None);
}

#[test]
fn a_long_answer_is_cut_before_a_unit_and_never_inside_a_pair() {
    let lone = "export default () => String.fromCharCode(0xd800).repeat(1000);";
    let Verdict::Wrong(Primitive::String(units)) = one(lone) else {
        panic!("a string answer");
    };
    assert_eq!(units, vec![0xd800; DETAIL_BYTES / 3]);
    let astral = "export default () => 'a' + '\\u{1f600}'.repeat(1000);";
    let Verdict::Wrong(Primitive::String(units)) = one(astral) else {
        panic!("a string answer");
    };
    assert_eq!(units.len(), 1 + 2 * ((DETAIL_BYTES - 1) / 4));
    assert_eq!(units.last(), Some(&0xde00));
}
