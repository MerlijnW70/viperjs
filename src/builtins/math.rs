//! §21.3 — the `Math` namespace object.
//!
//! Not a constructor and not a function: §21.3 makes `Math` an ordinary object with an ordinary
//! prototype, holding eight constants and thirty-five functions. `new Math` is a TypeError for the
//! dull reason that it has no `[[Construct]]` rather than for a rule of its own.
//!
//! # Where this is not `f64`
//!
//! Most of these are the IEEE operation of the same name and Rust already has it. Four are not,
//! and each is a place the obvious implementation is quietly wrong:
//!
//! - **`round`** rounds a half *upwards*, towards +∞. Rust's `f64::round` rounds a half away from
//!   zero, so it disagrees on every negative half: `Math.round(-1.5)` is `-1` and not `-2`.
//! - **`sign`** answers `-0` for `-0` and `+0` for `+0`. Rust's `signum` answers `-1` and `1`.
//! - **`min`** and **`max`** are not `f64::min` and `f64::max`. Those ignore a `NaN` operand;
//!   §21.3.2.24 propagates it. They also have to tell `-0` from `+0`, which no comparison does.
//! - **`pow`** is `Number::exponentiate` (§21.3.2.26 says so outright), which differs from IEEE
//!   `pow` about `1 ** NaN` — so it is the engine's own, shared with the `**` operator rather
//!   than written a second time here.

use super::{define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Completion, Value};
use crate::vm::Vm;
use std::cell::Cell;
use std::cmp::Ordering;

/// Build `Math` into `heap` as a property of the global object.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let math = heap.new_object(Some(realm.object_prototype()));
    define_value(heap, global, "Math", Value::Object(math));
    seed_random();

    // §21.3.1 — every constant is non-writable, non-enumerable and non-configurable. A program
    // cannot move π, which is the one thing about `Math` everyone eventually tries.
    for (name, value) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        super::define_fixed(heap, math, name, Value::Number(value));
    }

    // §21.3.2, in the order the specification lists them. The `length` beside each is what the
    // clause writes, which is not always how many arguments the function reads: `max` and `min`
    // say 2 and take any number, and `random` says 0.
    for (name, length, native) in [
        ("abs", 1, abs as crate::heap::Native),
        ("acos", 1, acos),
        ("acosh", 1, acosh),
        ("asin", 1, asin),
        ("asinh", 1, asinh),
        ("atan", 1, atan),
        ("atanh", 1, atanh),
        ("atan2", 2, atan2),
        ("cbrt", 1, cbrt),
        ("ceil", 1, ceil),
        ("clz32", 1, clz32),
        ("cos", 1, cos),
        ("cosh", 1, cosh),
        ("exp", 1, exp),
        ("expm1", 1, expm1),
        ("floor", 1, floor),
        ("fround", 1, fround),
        ("hypot", 2, hypot),
        ("imul", 2, imul),
        ("log", 1, log),
        ("log1p", 1, log1p),
        ("log10", 1, log10),
        ("log2", 1, log2),
        ("max", 2, max),
        ("min", 2, min),
        ("pow", 2, pow),
        ("random", 0, random),
        ("round", 1, round),
        ("sign", 1, sign),
        ("sin", 1, sin),
        ("sinh", 1, sinh),
        ("sqrt", 1, sqrt),
        ("tan", 1, tan),
        ("tanh", 1, tanh),
        ("trunc", 1, trunc),
    ] {
        define_method(heap, realm, math, name, length, native);
    }
}

/// Give this thread's generator a starting point, from the one varying thing a host always has.
///
/// A clock that cannot be read leaves [`SEEDLESS`] in place, which is a working generator and not
/// a broken one — a fixed sequence is still inside §21.3.2.27's promise, which is about the
/// interval and not about unpredictability. Anything that needs unpredictability has to be given
/// it by its host.
fn seed_random() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(SEEDLESS, |since| since.as_nanos() as u64);
    set_seed(now);
}

/// Start this thread's generator from `chosen` — the host's answer instead of the clock's.
///
/// §21.3.2.27 asks for "a Number value with positive sign, greater than or equal to +0𝔽 but
/// strictly less than 1𝔽, chosen randomly or pseudo randomly with approximately uniform
/// distribution over that range, using an implementation-defined algorithm or strategy". It asks
/// for **nothing** about unpredictability, and says the choice may depend on any implementation
/// state — so a host that fixes the sequence is inside the clause rather than bending it.
///
/// What wants it is a tool that has to run the same program twice and compare: the fuzzer's seed
/// fixed its inputs and could not fix what the engine did with one, because this was the clock's to
/// decide. A finding that appears once and not again is a finding nobody can act on.
///
/// **Not for anything that needs unpredictability.** A seeded generator is a *predicted* one, and
/// GOAL.md §3 leaves anything cryptographic to the host, which has to bring its own.
///
/// Zero is mapped away because it is the one state a xorshift cannot leave — and **only** zero.
/// This was `seed | 1`, which maps zero away and takes every even seed with it, so 42 and 43 named
/// one sequence: half of all seeds, silently. Harmless while the only caller was the clock, whose
/// nanoseconds nobody compares; not harmless the moment a host may pass a small number it chose.
/// The same spelling was found in the fuzzer's own generator by a test asking whether two seeds
/// disagree, and the test here that pinned `seeded & 1 == 1` was pinning the fault.
pub(crate) fn set_seed(chosen: u64) {
    let state = match chosen {
        0 => SEEDLESS,
        _ => chosen,
    };
    SEED.with(|seed| seed.set(state));
}

/// The one argument these all begin with — `ToNumber` of the first, or `NaN` if there is none.
///
/// Every function in §21.3.2 that takes one argument starts with "Let n be ? ToNumber(x)", and a
/// missing argument is `undefined`, whose `ToNumber` is `NaN`. So `Math.abs()` is `NaN` rather
/// than an error, and that falls out of this rather than being a case anywhere.
///
/// **Through the machine, and that is the whole of what this doc is for.** §7.1.4's `ToNumber` of an
/// **object** is `ToPrimitive` first, which calls the object's `valueOf` or `toString` — and calling
/// a JavaScript function needs the interpreter. `Value::to_number` takes only a heap, so it cannot;
/// it answered a TypeError for every object, and every function in this file used it. `Math.floor`
/// of a `new Number(3)` threw, and so did `Math.max` of anything a program had boxed. Found
/// 2026-08-09 by probing the clause rather than by reading the code, which said the right thing:
/// [`atan2`]'s comment already named "an object with a `valueOf` that runs code" as the reason its
/// two conversions are ordered, above two conversions that could not run one.
fn number(vm: &mut Vm, call: &NativeCall<'_>, heap: &mut Heap) -> Completion<f64> {
    vm.to_number(call.argument(0), heap)
}

/// One `f64 -> f64` function of §21.3.2, given the operation Rust already has.
macro_rules! unary {
    ($name:ident, $operation:expr) => {
        fn $name(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
            let operation: fn(f64) -> f64 = $operation;
            Ok(Value::Number(operation(number(vm, call, heap)?)))
        }
    };
}

unary!(abs, f64::abs);
unary!(acos, f64::acos);
unary!(acosh, f64::acosh);
unary!(asin, f64::asin);
unary!(asinh, f64::asinh);
unary!(atan, f64::atan);
unary!(atanh, f64::atanh);
unary!(cbrt, f64::cbrt);
unary!(ceil, f64::ceil);
unary!(cos, f64::cos);
unary!(cosh, f64::cosh);
unary!(exp, f64::exp);
unary!(expm1, f64::exp_m1);
unary!(floor, f64::floor);
unary!(log, f64::ln);
unary!(log1p, f64::ln_1p);
unary!(log10, f64::log10);
unary!(log2, f64::log2);
unary!(sin, f64::sin);
unary!(sinh, f64::sinh);
unary!(sqrt, f64::sqrt);
unary!(tan, f64::tan);
unary!(tanh, f64::tanh);
unary!(trunc, f64::trunc);

/// §21.3.2.8 `Math.atan2`.
fn atan2(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Both are converted, and in this order — §21.3.2.8 steps 1 and 2, which matters because
    // either may be an object with a `valueOf` that runs code.
    let y = vm.to_number(call.argument(0), heap)?;
    let x = vm.to_number(call.argument(1), heap)?;
    Ok(Value::Number(y.atan2(x)))
}

/// §21.3.2.11 `Math.clz32` — how many leading zero bits `ToUint32(x)` has.
fn clz32(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Coerced through the machine first, then narrowed: after `ToNumber` the value is a
    // primitive, so the heap-only `ToUint32` is exactly right for it. See [`number`].
    let value = Value::Number(vm.to_number(call.argument(0), heap)?).to_uint32(heap)?;
    Ok(Value::Number(f64::from(value.leading_zeros())))
}

/// §21.3.2.16 `Math.fround` — the nearest value a 32-bit float can hold.
fn fround(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = number(vm, call, heap)?;
    // Round-trip through `f32`, which is the operation the clause describes. `NaN` and the
    // infinities survive it, and a value too large for `f32` becomes an infinity, which is what
    // step 5 asks for rather than an error.
    Ok(Value::Number(f64::from(value as f32)))
}

/// §21.3.2.18 `Math.hypot` — the square root of the sum of the squares, over any number of them.
fn hypot(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Every argument is converted first, in order, before any is looked at — step 2. An infinity
    // then wins over a `NaN` (step 4 before step 5), which is why this cannot short-circuit on the
    // first `NaN` it meets.
    let mut coerced = Vec::with_capacity(call.arguments.len());
    for argument in call.arguments {
        coerced.push(vm.to_number(*argument, heap)?);
    }
    if coerced.iter().any(|value| value.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if coerced.iter().any(|value| value.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    // Summing the squares overflows where the answer would not, so the largest magnitude is
    // divided out first — the same scaling `f64::hypot` does for two, done for any number.
    let largest = coerced
        .iter()
        .fold(0.0f64, |most, value| most.max(value.abs()));
    if largest == 0.0 {
        return Ok(Value::Number(0.0));
    }
    let sum: f64 = coerced.iter().map(|value| (value / largest).powi(2)).sum();
    Ok(Value::Number(largest * sum.sqrt()))
}

/// §21.3.2.19 `Math.imul` — a 32-bit integer multiply that wraps.
fn imul(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Both coerced through the machine, in order, then narrowed — see [`number`] and
    // [`clz32`]. The order is observable: either may run a `valueOf`.
    let left = Value::Number(vm.to_number(call.argument(0), heap)?).to_int32(heap)?;
    let right = Value::Number(vm.to_number(call.argument(1), heap)?).to_int32(heap)?;
    Ok(Value::Number(f64::from(left.wrapping_mul(right))))
}

/// §21.3.2.24 `Math.max` and §21.3.2.25 `Math.min`, which differ only in direction.
///
/// Neither is `f64::max`. Two rules the IEEE operation does not have: a single `NaN` anywhere
/// makes the answer `NaN`, and `+0` is *larger* than `-0` — so `Math.max(0, -0)` is `+0` and
/// `Math.min(0, -0)` is `-0`, which no `<` can tell apart.
fn extremum(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    want_largest: bool,
) -> Completion<Value> {
    let mut coerced = Vec::with_capacity(call.arguments.len());
    for argument in call.arguments {
        coerced.push(vm.to_number(*argument, heap)?);
    }
    if coerced.iter().any(|value| value.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    // With no arguments at all the answer is the identity: -∞ for a maximum and +∞ for a minimum,
    // so that `Math.max()` is smaller than everything rather than an error.
    let mut best = if want_largest {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    for value in coerced {
        // `total_cmp` and not `<`, because the ordering §21.3.2.24 describes is IEEE 754's
        // *total* order: it puts `-0` below `+0`, which is the one thing an ordinary comparison
        // cannot say and the whole of why `Math.max(0, -0)` is `+0`. Written as a comparison of
        // its own it would be a hand-rolled tie-break with a case no test could reach — the two
        // zeros are `==`, so preferring one over the other is invisible unless the sign is asked
        // about directly.
        let ordering = value.total_cmp(&best);
        let better = match want_largest {
            true => ordering == Ordering::Greater,
            false => ordering == Ordering::Less,
        };
        if better {
            best = value;
        }
    }
    Ok(Value::Number(best))
}

/// §21.3.2.24 `Math.max`.
fn max(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    extremum(vm, heap, call, true)
}

/// §21.3.2.25 `Math.min`.
fn min(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    extremum(vm, heap, call, false)
}

/// §21.3.2.26 `Math.pow` — which the clause defines as `Number::exponentiate`.
///
/// So it is the engine's own, shared with the `**` operator rather than written again. The two
/// cannot drift apart about `1 ** NaN`, which is the case they would drift apart about.
fn pow(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let base = vm.to_number(call.argument(0), heap)?;
    let exponent = vm.to_number(call.argument(1), heap)?;
    Ok(Value::Number(crate::value::exponentiate(base, exponent)))
}

/// §21.3.2.28 `Math.round` — half rounds *upwards*, not away from zero.
fn round(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = number(vm, call, heap)?;
    // Steps 3 to 6 written out. The two zero-signed cases are not decoration: `Math.round(-0.4)`
    // is `-0` and not `+0`, and `floor(x + 0.5)` alone would answer `+0` for it.
    // Only the zeros are named. A `NaN` and the infinities reach the last arm and come back
    // unchanged from it — `fract` of either is `NaN`, and `(x + 0.5).floor()` of an infinity is
    // that infinity — so naming them here as well would be naming a case nothing could reach.
    let rounded = if value == 0.0 {
        // Steps 3 and 4 — each zero answers *itself*, which is what keeps `Math.round(-0)` from
        // becoming `+0` in the range test below.
        value
    } else if (0.0..0.5).contains(&value) {
        // Step 5 — everything below a half is a zero, and a *positive* one.
        0.0
    } else if (-0.5..0.0).contains(&value) {
        // Step 6, and the one `floor(x + 0.5)` alone would lose: this zero is negative.
        -0.0
    } else if value.fract() == 0.0 {
        // Adding a half to a large value can round it in binary before the floor sees it, so a
        // value with nothing after the point is left exactly alone.
        value
    } else {
        (value + 0.5).floor()
    };
    Ok(Value::Number(rounded))
}

/// §21.3.2.29 `Math.sign`.
fn sign(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let value = number(vm, call, heap)?;
    // `NaN`, `+0` and `-0` are answered with *themselves* — steps 3 and 4 — which is the whole
    // difference from `f64::signum`. Past those three, `signum` is right and is used rather than a
    // comparison of its own: `value > 0.0` and `value.is_sign_positive()` agree on everything that
    // reaches here, so writing either would be writing a test no input can fail.
    let signed = if value.is_nan() || value == 0.0 {
        value
    } else {
        value.signum()
    };
    Ok(Value::Number(signed))
}

thread_local! {
    /// The state [`random`] walks.
    ///
    /// Set when the realm is built rather than on first use, so that taking a step is only a step
    /// — a lazily-seeded generator has a branch on every call that no behaviour can tell from its
    /// absence, and an untestable branch is worse than an eager clock read.
    static SEED: Cell<u64> = const { Cell::new(SEEDLESS) };
}

/// What the generator's state is before a realm seeds it.
///
/// Non-zero, because a xorshift started at zero stays at zero — so even a host whose clock reads
/// the same value every time gets a sequence rather than a constant.
const SEEDLESS: u64 = 0x2545_f491_4f6c_dd1d;

/// One step of the xorshift [`random`] walks — §21.3.2.27's "implementation-defined algorithm".
///
/// A free function so that the *algorithm* can be tested. Nothing a program can observe about
/// `Math.random` distinguishes one pseudo-random sequence from another — the clause asks only for
/// the interval — so a test written in JavaScript could not tell this from three shifts in the
/// wrong direction. This one can.
const fn next_state(state: u64) -> u64 {
    let mut state = state;
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

/// §21.3.2.27 `Math.random` — a Number in `[0, 1)`, chosen "using an implementation-defined
/// algorithm or strategy".
///
/// A xorshift, seeded once per thread from the clock. The clause asks for no quality beyond the
/// interval and for values "chosen randomly or pseudo randomly with approximately uniform
/// distribution over that range", and this is the smallest thing that is honestly that. It is not
/// a source of randomness anything may rely on for anything else — a program that needs one has
/// to be given it by its host.
///
/// The mantissa is filled from the top 53 bits, because those are the ones a `f64` can hold
/// exactly; taking the low bits instead would make the low-order digits of every value the least
/// random part of it.
fn random(_vm: &mut Vm, _heap: &mut Heap, _call: &NativeCall<'_>) -> Completion<Value> {
    let bits = SEED.with(|seed| {
        let state = next_state(seed.get());
        seed.set(state);
        state
    });
    // 53 bits over 2^53 is every representable value in `[0, 1)` and nothing outside it.
    let value = (bits >> 11) as f64 / (1u64 << 53) as f64;
    Ok(Value::Number(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_walks_the_sequence_it_was_written_to_walk() {
        // §21.3.2.27 asks for a Number in `[0, 1)` and nothing else, so *which* pseudo-random
        // sequence this is cannot be observed from JavaScript at all: three shifts in the wrong
        // direction would still answer values in the interval, and every test written in the
        // language would still pass. These rows are the algorithm itself.
        //
        // The first three states a xorshift64 with shifts of 13, 7 and 17 walks through from 1,
        // computed rather than remembered.
        assert_eq!(next_state(1), 1_082_269_761);
        assert_eq!(next_state(1_082_269_761), 1_152_992_998_833_853_505);
        assert_eq!(
            next_state(1_152_992_998_833_853_505),
            11_177_516_664_432_764_457
        );
        // Zero is the one state it cannot leave, which is why a realm never seeds it with one.
        assert_eq!(next_state(0), 0);
        assert_ne!(SEEDLESS, 0);
        // A step goes somewhere else — a generator that stood still would answer one value for
        // ever and still be inside the interval.
        assert_ne!(next_state(SEEDLESS), SEEDLESS);
    }

    #[test]
    fn seeding_moves_the_generator_off_the_value_it_starts_at() {
        // The state a thread begins with is not the one it uses, and is never zero — the two
        // things `seed_random` is for.
        SEED.with(|seed| seed.set(SEEDLESS));
        seed_random();
        assert_ne!(SEED.with(Cell::get), 0);
    }

    #[test]
    fn only_zero_is_mapped_away_and_two_seeds_keep_their_difference() {
        // **This row used to assert `seeded & 1 == 1`**, which pinned the consequence of writing
        // the zero-guard as `seed | 1` — and that spelling maps every *even* seed onto its odd
        // neighbour, so 42 and 43 named one sequence. Half of all seeds, silently, and a test that
        // called it correct.
        //
        // The property that matters is the one the guard exists for: zero is the single state a
        // xorshift cannot leave, and nothing else may be touched.
        for (chosen, expected) in [
            (0, SEEDLESS),
            (1, 1),
            (42, 42),
            (43, 43),
            (u64::MAX, u64::MAX),
        ] {
            set_seed(chosen);
            assert_eq!(SEED.with(Cell::get), expected, "seed {chosen}");
        }
        // …said the other way round, which is what a caller actually depends on: two seeds that
        // differ produce sequences that differ.
        set_seed(42);
        let first = next_state(SEED.with(Cell::get));
        set_seed(43);
        assert_ne!(next_state(SEED.with(Cell::get)), first);
    }
}
