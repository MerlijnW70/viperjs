//! §21.3 — `Math`, and the four places it is not the arithmetic a CPU does.
//!
//! # What is pinned here and what is not
//!
//! §21.3.2 marks most of these **implementation-approximated**: `cos`, `cbrt`, `atan2` and their
//! kind need only be close, and two conforming engines may differ in the last bit. Checked against
//! V8 over 1551 rows, ViperJS differs from it in 47 — every one a last-bit difference in exactly
//! those functions, and none in a function whose result the specification fixes.
//!
//! So the rows below are the ones where the answer is *exact*: the constants, the roundings, the
//! signs of zero, the integer operations, and the comparisons. Pinning `Math.cos(1)` to seventeen
//! digits would be pinning the platform's libm, which is not what the specification says.

use super::*;

/// Whether an expression is `-0` rather than `+0`, which no comparison can tell.
///
/// `1 / -0` is `-Infinity` and `1 / +0` is `+Infinity`, which is the only way to ask from inside
/// the language until `Object.is` exists.
fn signed_zero(source: &str) -> String {
    run(&format!("var v = {source}; v + '|' + (1 / v)"))
}

#[test]
fn the_constants_are_what_they_are_and_cannot_be_moved() {
    assert_eq!(run("Math.PI"), "3.141592653589793");
    assert_eq!(run("Math.E"), "2.718281828459045");
    assert_eq!(run("Math.LN10"), "2.302585092994046");
    assert_eq!(run("Math.LN2"), "0.6931471805599453");
    assert_eq!(run("Math.LOG10E"), "0.4342944819032518");
    assert_eq!(run("Math.LOG2E"), "1.4426950408889634");
    assert_eq!(run("Math.SQRT1_2"), "0.7071067811865476");
    assert_eq!(run("Math.SQRT2"), "1.4142135623730951");
    // §21.3.1 — none of them is writable or configurable, which is the difference between a
    // constant and a variable that happens to start out right.
    assert_eq!(run("Math.PI = 3; Math.PI"), "3.141592653589793");
    assert_eq!(
        run("var d = Object.getOwnPropertyDescriptor(Math, 'PI'); \
             d.writable + ',' + d.enumerable + ',' + d.configurable"),
        "false,false,false"
    );
    // §21.3 — an ordinary object. Not a function and not a constructor.
    assert_eq!(run("typeof Math"), "object");
    assert_eq!(run("try { Math(); } catch (e) { e.name }"), "TypeError");
    assert_eq!(run("try { new Math(); } catch (e) { e.name }"), "TypeError");
}

#[test]
fn a_half_rounds_upwards_and_not_away_from_zero() {
    // §21.3.2.28, and the one every implementation gets wrong first: `round` is `floor(x + 0.5)`,
    // so a negative half goes *up* towards zero. The C library's `round` goes the other way.
    assert_eq!(run("Math.round(0.5)"), "1");
    assert_eq!(run("Math.round(1.5)"), "2");
    assert_eq!(run("Math.round(2.5)"), "3");
    assert_eq!(run("Math.round(-0.5)"), "0");
    assert_eq!(run("Math.round(-1.5)"), "-1");
    assert_eq!(run("Math.round(-2.5)"), "-2");
    assert_eq!(run("Math.round(0.4)"), "0");
    assert_eq!(run("Math.round(-0.6)"), "-1");
    // Steps 3 and 4 — the signed zeros, which `floor(x + 0.5)` alone would lose: everything in
    // `[-0.5, -0)` answers `-0` and not `+0`.
    assert_eq!(signed_zero("Math.round(-0.4)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.round(-0.5)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.round(-0)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.round(0.4)"), "0|Infinity");
    // A value with nothing after the point is returned exactly, which matters where adding a half
    // would round it in binary before the floor could see it.
    assert_eq!(run("Math.round(4503599627370497)"), "4503599627370497");
    assert_eq!(run("Math.round(0.49999999999999994)"), "0");
    assert_eq!(
        run("Math.round(NaN) + ',' + Math.round(Infinity) + ',' + Math.round(-Infinity)"),
        "NaN,Infinity,-Infinity"
    );
    // Both sides of every boundary the clause draws, since each is a comparison that could be
    // written one notch out and still pass everything above.
    assert_eq!(run("Math.round(0.49999999999999994)"), "0");
    assert_eq!(run("Math.round(0.5)"), "1");
    assert_eq!(run("Math.round(-0.5)"), "0");
    assert_eq!(run("Math.round(-0.5000000000000001)"), "-1");
    assert_eq!(run("Math.round(1)"), "1");
    assert_eq!(run("Math.round(-1)"), "-1");
    assert_eq!(signed_zero("Math.round(0)"), "0|Infinity");
}

#[test]
fn the_roundings_and_the_sign_keep_a_negative_zero() {
    // §21.3.2.10, §21.3.2.15 and §21.3.2.35 — each answers `-0` for an argument in `(-1, -0]`,
    // and that is a value a program can tell apart.
    assert_eq!(signed_zero("Math.ceil(-0.5)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.trunc(-0.5)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.floor(-0)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.ceil(0.5)"), "1|1");
    // §21.3.2.29 — `sign` answers `NaN`, `+0` and `-0` with *themselves*, which is the whole
    // difference from asking whether the value is less than zero.
    assert_eq!(signed_zero("Math.sign(-0)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.sign(0)"), "0|Infinity");
    assert_eq!(
        run("Math.sign(-3) + ',' + Math.sign(3) + ',' + Math.sign(NaN)"),
        "-1,1,NaN"
    );
    // §21.3.2.1 — `abs` of a negative zero is a *positive* zero.
    assert_eq!(signed_zero("Math.abs(-0)"), "0|Infinity");
}

#[test]
fn min_and_max_propagate_a_nan_and_can_tell_the_zeros_apart() {
    assert_eq!(run("Math.max(1, 2, 3) + ',' + Math.min(1, 2, 3)"), "3,1");
    assert_eq!(run("Math.max(-1, -2) + ',' + Math.min(-1, -2)"), "-1,-2");
    // §21.3.2.24 step 4 — a `NaN` anywhere wins, wherever it is in the list. `f64::max` ignores
    // one instead, which is why these are not that.
    assert_eq!(run("Math.max(1, NaN) + ',' + Math.max(NaN, 1)"), "NaN,NaN");
    assert_eq!(run("Math.min(1, NaN) + ',' + Math.min(NaN, 1)"), "NaN,NaN");
    // …and `+0` is larger than `-0`, which no comparison says.
    assert_eq!(signed_zero("Math.max(0, -0)"), "0|Infinity");
    assert_eq!(signed_zero("Math.max(-0, 0)"), "0|Infinity");
    assert_eq!(signed_zero("Math.min(0, -0)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.min(-0, 0)"), "0|-Infinity");
    // With nothing to compare, the answer is the identity rather than an error — so that adding
    // an argument can only make a maximum larger.
    assert_eq!(run("Math.max()"), "-Infinity");
    assert_eq!(run("Math.min()"), "Infinity");
    // A single argument is the answer whatever its sign, and two equal ones that are not zeros do
    // not go looking for a sign to prefer.
    assert_eq!(run("Math.max(-5) + ',' + Math.min(-5)"), "-5,-5");
    assert_eq!(run("Math.max(2, 2) + ',' + Math.min(2, 2)"), "2,2");
    assert_eq!(signed_zero("Math.max(-0, -0)"), "0|-Infinity");
    assert_eq!(signed_zero("Math.min(0, 0)"), "0|Infinity");
    assert_eq!(run("Math.max(0, 1) + ',' + Math.max(1, 0)"), "1,1");
    assert_eq!(run("Math.min(0, -1) + ',' + Math.min(-1, 0)"), "-1,-1");
}

#[test]
fn the_integer_operations_are_exact() {
    // §21.3.2.11 — `clz32` counts the leading zeros of `ToUint32`, so a value with none has 0 and
    // zero itself has all thirty-two.
    assert_eq!(run("Math.clz32(0)"), "32");
    assert_eq!(run("Math.clz32(1)"), "31");
    assert_eq!(run("Math.clz32(4294967295)"), "0");
    assert_eq!(run("Math.clz32(-1)"), "0");
    // §21.3.2.19 — `imul` is a 32-bit multiply that wraps and comes back signed.
    assert_eq!(run("Math.imul(3, 4)"), "12");
    assert_eq!(run("Math.imul(-5, 12)"), "-60");
    assert_eq!(run("Math.imul(4294967295, 5)"), "-5");
    assert_eq!(run("Math.imul(2, 4294967295)"), "-2");
    // §21.3.2.16 — `fround` is a round trip through a 32-bit float, so anything too large for one
    // comes back as an infinity rather than as an error.
    assert_eq!(run("Math.fround(1.1)"), "1.100000023841858");
    assert_eq!(run("Math.fround(1e40)"), "Infinity");
    assert_eq!(run("Math.fround(1) + ',' + Math.fround(NaN)"), "1,NaN");
    // §21.3.2.18 — `hypot` of nothing is zero, and an infinity beats a `NaN` because step 4 comes
    // before step 5.
    assert_eq!(run("Math.hypot()"), "0");
    assert_eq!(run("Math.hypot(3, 4)"), "5");
    assert_eq!(
        run("Math.hypot(NaN, Infinity) + ',' + Math.hypot(Infinity, NaN)"),
        "Infinity,Infinity"
    );
    assert_eq!(run("Math.hypot(NaN, 1)"), "NaN");
    // One argument each, because these are the two shapes where the scaling would answer for the
    // rule: a lone `NaN` has no larger magnitude to be divided by, and all-zeros would divide a
    // zero by a zero.
    assert_eq!(run("Math.hypot(NaN)"), "NaN");
    assert_eq!(run("Math.hypot(0, 0)"), "0");
    assert_eq!(run("Math.hypot(-0)"), "0");
    assert_eq!(run("Math.hypot(-3)"), "3");
    // Scaled before squaring, or this would overflow to `Infinity` where the answer fits easily.
    // §21.3.2.18 is implementation-approximated, so what is pinned is that the answer is a finite
    // number of the right size rather than its last bit — though it does happen to match V8's.
    assert_eq!(
        run("var v = Math.hypot(1e300, 1e300); (v > 1e300) + ',' + (v < Infinity)"),
        "true,true"
    );
    assert_eq!(run("Math.hypot(1e300, 1e300)"), "1.4142135623730952e+300");
}

#[test]
fn pow_is_the_operator_rather_than_a_second_copy_of_it() {
    // §21.3.2.26 defines `Math.pow` as `Number::exponentiate` — the same operation `**` is — so
    // the two cannot disagree about the case they would disagree about. §6.1.6.1.3's note: the
    // first edition of ECMAScript made `1 ** NaN` be `NaN`, IEEE 754 later made `pow` answer 1,
    // and the language kept its own answer.
    assert_eq!(run("Math.pow(1, NaN)"), "NaN");
    assert_eq!(run("1 ** NaN"), "NaN");
    assert_eq!(run("Math.pow(-1, Infinity)"), "NaN");
    assert_eq!(run("(-1) ** Infinity"), "NaN");
    assert_eq!(run("Math.pow(2, 10)"), "1024");
    assert_eq!(run("Math.pow(2, 32) - 1"), "4294967295");
    assert_eq!(run("Math.pow(0, 0)"), "1");
}

#[test]
fn a_missing_argument_is_not_an_error_and_random_stays_in_its_interval() {
    // Every clause begins "Let n be ? ToNumber(x)", and a missing argument is `undefined`, whose
    // `ToNumber` is `NaN`. So these answer rather than throwing.
    assert_eq!(run("Math.abs()"), "NaN");
    assert_eq!(run("Math.round()"), "NaN");
    assert_eq!(run("Math.pow(2)"), "NaN");
    assert_eq!(run("Math.atan2(1)"), "NaN");
    // §21.3.2.27 — the interval is the whole of what is promised, and it is closed below and open
    // above. Asked many times, because a generator that answered its bound occasionally would
    // pass a single call.
    assert_eq!(
        run("var ok = true; for (var i = 0; i < 500; i = i + 1) { \
             var r = Math.random(); if (!(r >= 0 && r < 1)) { ok = false; } } ok"),
        "true"
    );
    assert_eq!(run("typeof Math.random()"), "number");
    // Two calls answering the same value would be a generator that never advanced.
    assert_eq!(
        run("var same = 0; var first = Math.random(); \
             for (var i = 0; i < 50; i = i + 1) { if (Math.random() === first) { same = same + 1; } } \
             same < 50"),
        "true"
    );
}

#[test]
fn every_math_function_coerces_an_object_through_its_own_value_of() {
    // §7.1.4's `ToNumber` of an object is `ToPrimitive` first, which calls the object's `valueOf`
    // or `toString` — so it needs the interpreter, and a conversion that has only a heap cannot do
    // it. Every function in §21.3.2 used one, and every one of them threw a TypeError for an
    // argument a program had boxed: `Math.floor(new Number(3))` was an error.
    //
    // Written across the three shapes rather than once, because the file reaches `ToNumber` three
    // ways: the shared one-argument helper, the two-argument functions that convert in order, and
    // the two that narrow to an integer afterwards.
    assert_eq!(
        run("Math.floor({ valueOf: function () { return 3.7 } })"),
        "3"
    );
    assert_eq!(run("Math.abs(new Number(-4))"), "4");
    // `toString` when there is no `valueOf` — §7.1.1's `OrdinaryToPrimitive` tries both, in that
    // order, and a `Math` function must reach the second.
    assert_eq!(
        run("Math.sqrt({ toString: function () { return '9' } })"),
        "3"
    );
    // The two-argument ones, and their order, which is observable because either side may run code.
    assert_eq!(
        run(
            "var log = ''; var a = { valueOf: function () { log += 'a'; return 1 } }; \
             var b = { valueOf: function () { log += 'b'; return 2 } }; \
             Math.pow(b, a); log"
        ),
        "ba"
    );
    assert_eq!(
        run("Math.atan2({ valueOf: function () { return 0 } }, 1)"),
        "0"
    );
    assert_eq!(
        run("Math.hypot({ valueOf: function () { return 3 } }, 4)"),
        "5"
    );
    // …the variadic pair, which fold rather than convert a fixed count.
    assert_eq!(
        run("Math.max({ valueOf: function () { return 3 } }, 2)"),
        "3"
    );
    assert_eq!(
        run("Math.min({ valueOf: function () { return 1 } }, 2)"),
        "1"
    );
    // …and the two that narrow to an integer after converting, which is the shape that would be
    // got wrong by narrowing first.
    assert_eq!(
        run("Math.imul({ valueOf: function () { return 3 } }, 4)"),
        "12"
    );
    assert_eq!(
        run("Math.clz32({ valueOf: function () { return 1 } })"),
        "31"
    );
    // A `valueOf` that throws still throws, which is what says the conversion happens at all
    // rather than being skipped for objects.
    assert_eq!(
        run("var said = 'none'; \
             try { Math.floor({ valueOf: function () { throw 'from valueOf' } }) } \
             catch (e) { said = e } said"),
        "from valueOf"
    );
    // …and an object with neither is still a TypeError, which is the row that stops this passing
    // by having made `ToNumber` answer something for everything.
    assert_eq!(
        run("var kind = 'none'; \
             try { Math.floor(Object.create(null)) } catch (e) { kind = e.constructor.name } kind"),
        "TypeError"
    );
}
