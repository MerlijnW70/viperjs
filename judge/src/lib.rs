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
//! its `f64`, `5` a string.
//!
//! The input is: `u32` version (`1`), `u64` step budget, `u64` heap budget in bytes, `u64` seed,
//! `f64` the time `Date` reads, the module source as a string, a `u32` count of cases, and for each
//! a `u32` count of arguments, the arguments as values, and the expected answer as a value.
//!
//! The output is: `u32` version (`1`), a `u8` that is `0` when the input was read and `1` when it
//! was not, a `u32` count of verdicts, and for each a `u8` [`Verdict`] code followed by its detail:
//! the answer as a value for [`Verdict::Wrong`], a string for [`Verdict::Threw`] and
//! [`Verdict::NoModule`], nothing for the rest. A detail string is at most [`DETAIL_BYTES`] long
//! and is the delivery's own text — a host prints it escaped or not at all.

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
    /// A String, as UTF-8 — a lone surrogate the delivery produced becomes U+FFFD.
    String(String),
}

impl Primitive {
    /// Whether `self` is the answer `expected` asked for.
    ///
    /// Exact for strings and booleans. Numbers are equal when `==` says so, which makes `0` and
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
        // A String primitive's text is read, not computed: no code runs for it.
        Value::String(_) => match engine.text(answer) {
            Ok(text) => Primitive::String(text),
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
        Primitive::String(text) => engine.string(text)?,
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
fn cut_primitive(answer: Primitive) -> Primitive {
    match answer {
        Primitive::String(text) => Primitive::String(cut(&text)),
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

    fn primitive(&mut self) -> Option<Primitive> {
        Some(match self.u8()? {
            0 => Primitive::Undefined,
            1 => Primitive::Null,
            2 => Primitive::Boolean(false),
            3 => Primitive::Boolean(true),
            4 => Primitive::Number(self.f64()?),
            5 => Primitive::String(self.string()?),
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

fn write_string(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&u32::try_from(text.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
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
        Primitive::String(text) => {
            out.push(5);
            write_string(out, text);
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
