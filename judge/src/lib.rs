//! A judge for delivered JavaScript, built to run as a WebAssembly module that imports nothing.
//!
//! A host hands [`judge`] a delivered module's source and a list of cases — arguments and the
//! answer each should produce — and gets back one verdict per case. Everything that decides a
//! verdict happens here, in Rust, and nothing the delivery runs can reach it:
//!
//! - **The comparison is not JavaScript.** A delivery that replaces `JSON.stringify` or
//!   `Array.prototype.map` changes nothing here, because the answer is read off the engine as a
//!   primitive and compared in Rust. Only `undefined`, `null`, booleans, numbers and strings are
//!   answers; an object is refused rather than converted, because converting one runs its code.
//! - **Every case is a fresh engine.** No state carries from one case to the next, so a delivery
//!   cannot learn the inputs and adapt.
//! - **Every case is reproducible.** DR-0031's step budget instead of a clock, a fixed seed for
//!   `Math.random` and a fixed time for `Date`: the same input gives the same verdicts on any
//!   machine, so anyone can check a judge's work by running it again.
//! - **There is nothing to reach.** Built for `wasm32-unknown-unknown`, the module has no imports
//!   at all — no files, no clock, no environment — and its memory has a ceiling set when it is
//!   linked.
//!
//! # The wire format
//!
//! Little-endian throughout. A *string* is a `u32` byte length and that many bytes of UTF-8. A
//! *value* is a tag byte: `0` undefined, `1` null, `2` false, `3` true, `4` a number followed by
//! its `f64`, `5` a string whose bytes are [WTF-8](https://simonsapin.github.io/wtf-8/).
//!
//! WTF-8 is UTF-8 that can also carry an unpaired surrogate, which a JavaScript String may hold
//! and UTF-8 cannot: the unit is written as the three bytes it would take if it were a code
//! point, `ED A0 80` for U+D800 through `ED BF BF` for U+DFFF. A surrogate *pair* is always the
//! four bytes of the character it encodes and never two such triples; an input that writes one as
//! two is refused, so every String has exactly one spelling. A string value is therefore exactly
//! the code units the delivery's String held: an answer of `"\ufffd"` is `EF BF BD` and an answer
//! of `"\ud800"` is `ED A0 80`, and a host tells them apart by those bytes. Well-formed UTF-8 is
//! WTF-8 as it stands, so a host that never sends a lone surrogate sends exactly what it always
//! has; a host that decodes an answer as UTF-8 meets an unpaired surrogate as bytes UTF-8 refuses,
//! which a replacing decoder turns into U+FFFD.
//!
//! The input is: `u32` version (`1`), `u64` step budget, `u64` heap budget in bytes, `u64` seed,
//! `f64` the time `Date` reads, the module source as a string, a `u32` count of cases, and for each
//! a `u32` count of arguments, the arguments as values, and the expected answer as a value.
//!
//! The output is: `u32` version (`1`), a `u8` that is `0` when the input was read and `1` when it
//! was not, a `u32` count of verdicts, and for each a `u8` [`Verdict`] code followed by its detail:
//! the answer as a value for [`Verdict::Wrong`], a string for [`Verdict::Threw`] and
//! [`Verdict::NoModule`], nothing for the rest. A detail is at most [`DETAIL_BYTES`] long, cut
//! before a character and never inside one, and is the delivery's own text — a host prints it
//! escaped or not at all. A [`Verdict::Threw`] or [`Verdict::NoModule`] detail is a message rather
//! than an answer and is plain UTF-8, with U+FFFD for any unpaired surrogate in it.

#![deny(unsafe_code)]

use viperjs::api::{Engine, Error};
use viperjs::value::Value;

/// The one version of the wire format there is.
pub const VERSION: u32 = 1;

/// How much of a delivery's own text a verdict carries back.
pub const DETAIL_BYTES: usize = 256;

/// How many cases one input may hold, so that the count cannot ask for an allocation by itself.
pub const MAX_CASES: u32 = 10_000;

/// How many arguments one case may pass.
pub const MAX_ARGUMENTS: u32 = 64;

/// A primitive, as the wire carries one and as an answer is compared.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    /// `undefined`.
    Undefined,
    /// `null`.
    Null,
    /// `true` or `false`.
    Boolean(bool),
    /// A Number. Two are the same answer when they are equal or both NaN.
    Number(f64),
    /// A String, as the UTF-16 code units it is — an unpaired surrogate included, so that
    /// `"\ud800"` and `"\ufffd"` are different answers here as they are in JavaScript.
    String(Vec<u16>),
}

impl Primitive {
    /// A String primitive spelling `text`.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self::String(text.encode_utf16().collect())
    }

    /// Whether `self` is the answer `expected` asked for.
    ///
    /// Exact for strings, unit for unit, and for booleans. Numbers are equal when `==` says so, which makes `0` and
    /// `-0` the same answer, or when both are NaN, which `==` would never say.
    #[must_use]
    pub fn answers(&self, expected: &Self) -> bool {
        match (self, expected) {
            (Self::Number(got), Self::Number(wanted)) => {
                got == wanted || (got.is_nan() && wanted.is_nan())
            }
            _ => self == expected,
        }
    }
}

/// What one case came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The answer was the expected one. Code `0`.
    Pass,
    /// The answer was a primitive and not the expected one. Code `1`, carrying the answer.
    Wrong(Primitive),
    /// The answer was an object, a Symbol or a BigInt, which is refused unread. Code `2`.
    NotPrimitive,
    /// The call threw. Code `3`, carrying what it threw, cut to [`DETAIL_BYTES`].
    Threw(String),
    /// A budget stopped the module or the call. Code `4`.
    Stopped,
    /// The module did not parse, threw while it ran, or has no default export to call. Code `5`,
    /// carrying why.
    NoModule(String),
    /// The engine reported a fault of its own. Code `6`. Not something a delivery can cause.
    Engine,
}

impl Verdict {
    /// The code this verdict is on the wire.
    #[must_use]
    pub fn code(&self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Wrong(_) => 1,
            Self::NotPrimitive => 2,
            Self::Threw(_) => 3,
            Self::Stopped => 4,
            Self::NoModule(_) => 5,
            Self::Engine => 6,
        }
    }
}

/// One case: what to pass, and what should come back.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// The arguments, in order.
    pub arguments: Vec<Primitive>,
    /// The answer that passes.
    pub expected: Primitive,
}

/// Everything one judgement needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// DR-0031's budget for the module's evaluation and, separately, for each call.
    pub steps: u64,
    /// DR-0013's budget for each case's heap, in bytes.
    pub heap: u64,
    /// What `Math.random` is seeded with, the same for every case.
    pub seed: u64,
    /// The time `Date` reads, in milliseconds since the epoch.
    pub clock: f64,
    /// The delivered module.
    pub source: String,
    /// The cases, judged in order.
    pub cases: Vec<Case>,
}

/// Judge the input on the wire and answer the output on the wire. Never panics on any input.
#[must_use]
pub fn judge(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&VERSION.to_le_bytes());
    let Some(input) = read_input(input) else {
        out.push(1);
        out.extend_from_slice(&0_u32.to_le_bytes());
        return out;
    };
    out.push(0);
    let verdicts = judge_all(&input);
    out.extend_from_slice(&u32::try_from(verdicts.len()).unwrap_or(0).to_le_bytes());
    for verdict in &verdicts {
        out.push(verdict.code());
        match verdict {
            Verdict::Wrong(answer) => write_primitive(&mut out, answer),
            Verdict::Threw(text) | Verdict::NoModule(text) => write_string(&mut out, text),
            Verdict::Pass | Verdict::NotPrimitive | Verdict::Stopped | Verdict::Engine => {}
        }
    }
    out
}

/// Judge every case of `input`, each in an engine of its own.
#[must_use]
pub fn judge_all(input: &Input) -> Vec<Verdict> {
    viperjs::set_clock(Some(input.clock));
    input
        .cases
        .iter()
        .map(|case| judge_case(input, case))
        .collect()
}

/// Judge one case in a fresh engine.
fn judge_case(input: &Input, case: &Case) -> Verdict {
    let mut engine = Engine::new();
    // After `new`, because building the realm seeds the generator from the platform.
    engine.set_random_seed(input.seed);
    engine.set_step_budget(Some(input.steps));
    engine.set_heap_budget(usize::try_from(input.heap).unwrap_or(usize::MAX));
    let namespace = match engine.eval_module(&input.source) {
        Ok(namespace) => namespace,
        Err(Error::Interrupted) => return Verdict::Stopped,
        Err(Error::Engine(_) | Error::Collected) => return Verdict::Engine,
        Err(Error::Syntax { message, .. }) => return Verdict::NoModule(cut(&message)),
        Err(Error::Thrown(text)) => return Verdict::NoModule(cut(&text)),
    };
    let callee = match engine.get(namespace, "default") {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(_) => return Verdict::NoModule("the module has no default export to call".into()),
        Err(error) => return failed(error),
    };
    let mut arguments = Vec::with_capacity(case.arguments.len());
    for argument in &case.arguments {
        match to_value(&mut engine, argument) {
            Ok(value) => arguments.push(value),
            Err(error) => return failed(error),
        }
    }
    let answer = match engine.call(callee, Value::Undefined, &arguments) {
        Ok(answer) => answer,
        Err(error) => return failed(error),
    };
    let answer = match answer {
        Value::Undefined => Primitive::Undefined,
        Value::Null => Primitive::Null,
        Value::Boolean(flag) => Primitive::Boolean(flag),
        Value::Number(number) => Primitive::Number(number),
        // A String primitive's units are read, not computed: no code runs for it.
        Value::String(_) => match engine.units(answer) {
            Ok(units) => Primitive::String(units),
            Err(error) => return failed(error),
        },
        Value::Symbol(_) | Value::Object(_) | Value::BigInt(_) => return Verdict::NotPrimitive,
    };
    if answer.answers(&case.expected) {
        Verdict::Pass
    } else {
        Verdict::Wrong(cut_primitive(answer))
    }
}

/// What a failed read or call comes to.
fn failed(error: Error) -> Verdict {
    match error {
        Error::Interrupted => Verdict::Stopped,
        Error::Thrown(text) => Verdict::Threw(cut(&text)),
        Error::Syntax { message, .. } => Verdict::Threw(cut(&message)),
        Error::Engine(_) | Error::Collected => Verdict::Engine,
    }
}

/// A primitive as the engine's value. Only a String can fail, by being too long.
fn to_value(engine: &mut Engine, primitive: &Primitive) -> Result<Value, Error> {
    Ok(match primitive {
        Primitive::Undefined => Value::Undefined,
        Primitive::Null => Value::Null,
        Primitive::Boolean(flag) => Value::Boolean(*flag),
        Primitive::Number(number) => Value::Number(*number),
        Primitive::String(units) => engine.string_from_units(units.clone())?,
    })
}

/// `text` cut to at most [`DETAIL_BYTES`] bytes, on a character boundary.
fn cut(text: &str) -> String {
    let mut end = text.len().min(DETAIL_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// An answer cut the way a detail is, so a wrong answer cannot be a megabyte of output.
///
/// Cut by its WTF-8 length, before a character or an unpaired surrogate and never between the two
/// halves of a pair, so the bytes written are a prefix of the whole answer's.
fn cut_primitive(answer: Primitive) -> Primitive {
    match answer {
        Primitive::String(units) => {
            let mut bytes = 0;
            let mut end = 0;
            for decoded in char::decode_utf16(units.iter().copied()) {
                let (width, length) = match decoded {
                    Ok(ch) => (ch.len_utf8(), ch.len_utf16()),
                    Err(_) => (3, 1),
                };
                if bytes + width > DETAIL_BYTES {
                    break;
                }
                bytes += width;
                end += length;
            }
            Primitive::String(units.get(..end).unwrap_or_default().to_vec())
        }
        other => other,
    }
}

/// A cursor over the input that answers `None` rather than reading past the end.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let (head, rest) = self.bytes.split_at_checked(count)?;
        self.bytes = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let length = usize::try_from(self.u32()?).ok()?;
        String::from_utf8(self.take(length)?.to_vec()).ok()
    }

    fn units(&mut self) -> Option<Vec<u16>> {
        let length = usize::try_from(self.u32()?).ok()?;
        from_wtf8(self.take(length)?)
    }

    fn primitive(&mut self) -> Option<Primitive> {
        Some(match self.u8()? {
            0 => Primitive::Undefined,
            1 => Primitive::Null,
            2 => Primitive::Boolean(false),
            3 => Primitive::Boolean(true),
            4 => Primitive::Number(self.f64()?),
            5 => Primitive::String(self.units()?),
            _ => return None,
        })
    }
}

/// Read the wire format, or `None` for anything that is not exactly one well-formed input.
#[must_use]
pub fn read_input(bytes: &[u8]) -> Option<Input> {
    let mut reader = Reader { bytes };
    if reader.u32()? != VERSION {
        return None;
    }
    let steps = reader.u64()?;
    let heap = reader.u64()?;
    let seed = reader.u64()?;
    let clock = reader.f64()?;
    let source = reader.string()?;
    let count = reader.u32()?;
    if count > MAX_CASES {
        return None;
    }
    let mut cases = Vec::new();
    for _ in 0..count {
        let arity = reader.u32()?;
        if arity > MAX_ARGUMENTS {
            return None;
        }
        let mut arguments = Vec::new();
        for _ in 0..arity {
            arguments.push(reader.primitive()?);
        }
        let expected = reader.primitive()?;
        cases.push(Case {
            arguments,
            expected,
        });
    }
    reader.bytes.is_empty().then_some(Input {
        steps,
        heap,
        seed,
        clock,
        source,
        cases,
    })
}

/// The code units well-formed WTF-8 spells, or `None` for bytes that are not.
///
/// UTF-8's own rules — the shortest form, nothing past U+10FFFF, no stray continuation byte — with
/// the one change WTF-8 makes: `ED` may be followed by `A0` to `BF`, which spells a surrogate. A
/// lead surrogate spelled that way and followed at once by a trail surrogate spelled that way is
/// refused, because that pair has a four-byte spelling and WTF-8 allows only that one.
#[must_use]
pub fn from_wtf8(bytes: &[u8]) -> Option<Vec<u16>> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut at = 0;
    let mut after_lead = false;
    while let Some(&first) = bytes.get(at) {
        let (width, low, high, initial) = match first {
            0x00..=0x7f => (1, 0x80, 0xbf, u32::from(first)),
            0xc2..=0xdf => (2, 0x80, 0xbf, u32::from(first & 0x1f)),
            0xe0 => (3, 0xa0, 0xbf, 0),
            0xe1..=0xef => (3, 0x80, 0xbf, u32::from(first & 0x0f)),
            0xf0 => (4, 0x90, 0xbf, 0),
            0xf1..=0xf3 => (4, 0x80, 0xbf, u32::from(first & 0x07)),
            0xf4 => (4, 0x80, 0x8f, 4),
            _ => return None,
        };
        let mut point = initial;
        for (index, &byte) in bytes.get(at + 1..at + width)?.iter().enumerate() {
            let (floor, ceiling) = if index == 0 {
                (low, high)
            } else {
                (0x80, 0xbf)
            };
            if !(floor..=ceiling).contains(&byte) {
                return None;
            }
            point = (point << 6) | u32::from(byte & 0x3f);
        }
        at += width;
        match point {
            0xdc00..=0xdfff if after_lead => return None,
            0xd800..=0xdfff => {
                after_lead = point < 0xdc00;
                units.push(u16::try_from(point).ok()?);
            }
            _ => {
                after_lead = false;
                let ch = char::from_u32(point)?;
                let mut pair = [0; 2];
                units.extend_from_slice(ch.encode_utf16(&mut pair));
            }
        }
    }
    Some(units)
}

/// `units` as WTF-8: UTF-8 for every character, and three bytes for each unpaired surrogate.
#[must_use]
pub fn to_wtf8(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len());
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(ch) => {
                let mut buffer = [0; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
            Err(unpaired) => {
                let unit = unpaired.unpaired_surrogate();
                out.extend_from_slice(&[
                    0xe0 | (unit >> 12) as u8,
                    0x80 | ((unit >> 6) & 0x3f) as u8,
                    0x80 | (unit & 0x3f) as u8,
                ]);
            }
        }
    }
    out
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    write_bytes(out, text.as_bytes());
}

fn write_primitive(out: &mut Vec<u8>, primitive: &Primitive) {
    match primitive {
        Primitive::Undefined => out.push(0),
        Primitive::Null => out.push(1),
        Primitive::Boolean(false) => out.push(2),
        Primitive::Boolean(true) => out.push(3),
        Primitive::Number(number) => {
            out.push(4);
            out.extend_from_slice(&number.to_le_bytes());
        }
        Primitive::String(units) => {
            out.push(5);
            write_bytes(out, &to_wtf8(units));
        }
    }
}

/// Write `input` in the wire format — what a host does, and what the tests do.
#[must_use]
pub fn write_input(input: &Input) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&input.steps.to_le_bytes());
    out.extend_from_slice(&input.heap.to_le_bytes());
    out.extend_from_slice(&input.seed.to_le_bytes());
    out.extend_from_slice(&input.clock.to_le_bytes());
    write_string(&mut out, &input.source);
    out.extend_from_slice(&u32::try_from(input.cases.len()).unwrap_or(0).to_le_bytes());
    for case in &input.cases {
        out.extend_from_slice(
            &u32::try_from(case.arguments.len())
                .unwrap_or(0)
                .to_le_bytes(),
        );
        for argument in &case.arguments {
            write_primitive(&mut out, argument);
        }
        write_primitive(&mut out, &case.expected);
    }
    out
}

/// The three exports a host calls, and the only code here a WebAssembly host sees.
///
/// `input(length)` makes room for an input and answers where to write it; `run()` judges what was
/// written and answers the output's length; `output()` answers where the output is. No `unsafe`
/// block anywhere: the host writes into memory this crate owns between calls, and an export only
/// hands out an address, which is safe to make.
#[cfg(target_family = "wasm")]
mod exports {
    use std::cell::RefCell;

    thread_local! {
        static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    #[allow(unsafe_code)] // `no_mangle` is the only way to name an export; nothing here is unsafe
    #[unsafe(no_mangle)]
    pub extern "C" fn input(length: u32) -> *mut u8 {
        INPUT.with_borrow_mut(|input| {
            input.clear();
            input.resize(usize::try_from(length).unwrap_or(0), 0);
            input.as_mut_ptr()
        })
    }

    #[allow(unsafe_code)] // as above
    #[unsafe(no_mangle)]
    pub extern "C" fn run() -> u32 {
        let judged = INPUT.with_borrow(|input| super::judge(input));
        let length = u32::try_from(judged.len()).unwrap_or(0);
        OUTPUT.with_borrow_mut(|output| *output = judged);
        length
    }

    #[allow(unsafe_code)] // as above
    #[unsafe(no_mangle)]
    pub extern "C" fn output() -> *const u8 {
        OUTPUT.with_borrow(Vec::as_ptr)
    }
}

#[cfg(test)]
mod tests;
