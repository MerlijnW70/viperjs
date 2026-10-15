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
        arguments: vec![Primitive::String(text.to_string())],
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
    let Some(Verdict::Wrong(Primitive::String(text))) = first.first() else {
        panic!("{first:?}");
    };
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
        Verdict::Wrong(Primitive::String(
            "undefined,undefined,undefined,undefined,undefined,undefined,undefined".into()
        ))
    );
    let escape = "export default function () { return Function('return typeof process')(); }";
    assert_eq!(
        one(escape),
        Verdict::Wrong(Primitive::String("undefined".into()))
    );
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
        Verdict::Wrong(Primitive::String(text)) if text.len() <= DETAIL_BYTES && !text.is_empty()
    ));
}

#[test]
fn numbers_compare_as_answers_and_not_as_bits() {
    assert!(Primitive::Number(0.0).answers(&Primitive::Number(-0.0)));
    assert!(Primitive::Number(f64::NAN).answers(&Primitive::Number(f64::NAN)));
    assert!(!Primitive::Number(1.0).answers(&Primitive::Number(f64::NAN)));
    assert!(!Primitive::Number(1.0).answers(&Primitive::String("1".into())));
    assert!(!Primitive::Null.answers(&Primitive::Undefined));
    assert!(Primitive::String("a".into()).answers(&Primitive::String("a".into())));
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
                Primitive::String("é\u{0}".into()),
            ],
            expected: Primitive::String("x".into()),
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
        arguments: vec![Primitive::String(argument.into())],
        expected: Primitive::String(expected.into()),
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
