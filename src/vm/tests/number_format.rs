//! §21.1.3's `toFixed`, `toExponential` and `toPrecision` — and the two kinds of exactness they
//! need that an ordinary formatting call does not give.

use super::*;

#[test]
fn a_tie_rounds_up_and_never_to_even() {
    // §21.1.3.3 step 9 asks for the `n` closest to `x × 10^f`, taking the **larger** when two are
    // equally close. Every one of these is an exact tie — a half that really is a half — and a
    // formatter rounding half-to-even answers 0, 2, 2, 4 instead.
    assert_eq!(
        run("[(0.5).toFixed(0), (1.5).toFixed(0), (2.5).toFixed(0), (3.5).toFixed(0)].join(',')"),
        "1,2,3,4"
    );
    // `1.25` and `1.375` are exactly representable, so their halves are real halves too.
    assert_eq!(
        run("[(1.25).toFixed(1), (1.75).toFixed(1), (1.375).toFixed(2)].join(',')"),
        "1.3,1.8,1.38"
    );
    // A negative rounds by its magnitude — §21.1.3.3 step 8 takes the sign off first — so `-1.25`
    // goes *away* from zero to `-1.3` rather than towards it.
    assert_eq!(
        run("[(-1.25).toFixed(1), (-0.5).toFixed(0), (-2.5).toFixed(0)].join(',')"),
        "-1.3,-1,-3"
    );
}

#[test]
fn the_value_rounded_is_the_double_and_not_the_way_it_was_written() {
    // The other exactness, and the one that surprises people. `1.005` is not 1.005: the nearest
    // double is 1.00499999999999989…, so there is no tie and it rounds **down**. An engine that
    // computed `x * 100` and rounded that would answer "1.01", because the multiplication rounds
    // first — which is the bug this whole module is shaped to avoid.
    assert_eq!(
        run("[(1.005).toFixed(2), (1.45).toFixed(1), (8.575).toFixed(2)].join(',')"),
        "1.00,1.4,8.57"
    );
    // …and the expansion is exact however far it is asked to go, which is what says the digits
    // come from the double rather than from a shortest-round-trip spelling.
    assert_eq!(run("(0.1).toFixed(20)"), "0.10000000000000000555");
    assert_eq!(
        run("(1.1).toFixed(20) + '|' + (0.3).toFixed(20)"),
        "1.10000000000000008882|0.29999999999999998890"
    );
}

#[test]
fn to_fixed_answers_the_ordinary_spelling_where_a_fixed_one_would_be_absurd() {
    // §21.1.3.3 step 9's boundary: at 10^21 the fixed spelling *is* the ordinary one, so
    // `(1e21).toFixed(2)` is "1e+21" rather than twenty-two digits and a point.
    assert_eq!(
        run("[(1e21).toFixed(2), (1e20).toFixed(2)].join('|')"),
        "1e+21|100000000000000000000.00"
    );
    // Just below that boundary, `toFixed` writes the double's **exact** value while `String` writes
    // the shortest spelling that names it. The two disagree here by more than thirty thousand, and
    // that disagreement is the module's whole reason for existing rather than a curiosity.
    assert_eq!(
        run("var n = 1e21 - 1e5; n.toFixed(0) + '|' + String(n)"),
        "999999999999999868928|999999999999999900000"
    );
    // Step 6 — the three values with no digits at all keep their own spelling.
    assert_eq!(
        run("[(NaN).toFixed(2), (Infinity).toFixed(2), (-Infinity).toFixed(0)].join('|')"),
        "NaN|Infinity|-Infinity"
    );
    // `-0` has a magnitude of zero and an empty sign — it is not less than zero — so it is "0.00"
    // and never "-0.00". Rust writes negative zero with a minus of its own, which is exactly the
    // sort of thing that leaks through into an answer.
    assert_eq!(
        run(
            "[(-0).toFixed(2), (0).toFixed(2), (-0).toPrecision(3), (-0).toExponential(1)].join('|')"
        ),
        "0.00|0.00|0.00|0.0e+0"
    );
    // Steps 4 and 5 — the count must be an integer between 0 and 100, and an infinity is refused
    // by the same check rather than reaching either end of the range.
    for bad in ["-1", "101", "Infinity", "-Infinity"] {
        assert_eq!(
            run(&format!(
                "try {{ (1).toFixed({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "toFixed({bad})"
        );
    }
    assert_eq!(
        run("[(1).toFixed(), (1).toFixed(undefined), (1).toFixed(100).length].join('|')"),
        "1|1|102"
    );
    // §21.1.3's `ThisNumberValue` — a Number object works and anything else is a TypeError.
    assert_eq!(
        run("Number.prototype.toFixed.call(new Number(1.5), 0)"),
        "2"
    );
    for bad in ["'1'", "{}", "null", "true"] {
        assert_eq!(
            run(&format!(
                "try {{ Number.prototype.toFixed.call({bad}, 0); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "toFixed on {bad}"
        );
    }
}

#[test]
fn to_exponential_keeps_one_digit_before_the_point_and_says_how_far_it_moved() {
    // §21.1.3.2 step 12 — always exactly one significant digit before the point, the requested
    // number after it, and a signed exponent with no padding.
    assert_eq!(
        run("[(77.1234).toExponential(2), (77.1234).toExponential(0), \
             (0.0000001).toExponential(2)].join('|')"),
        "7.71e+1|8e+1|1.00e-7"
    );
    // Step 9 — zero has no significant digit, and is written as the requested number of them.
    assert_eq!(
        run("[(0).toExponential(2), (0).toExponential(0), (0).toExponential()].join('|')"),
        "0.00e+0|0e+0|0e+0"
    );
    // Step 10.b — an absent count means as few digits as still name the number exactly, which is
    // not the same as zero digits.
    assert_eq!(
        run("[(77.1234).toExponential(), (123456).toExponential(), (1).toExponential()].join('|')"),
        "7.71234e+1|1.23456e+5|1e+0"
    );
    // Rounding may carry past the leading digit, and then the exponent moves — `99.9` to one
    // fractional digit is `1.0e+2` and not `10.0e+1`.
    assert_eq!(
        run("[(99.9).toExponential(1), (9.99).toExponential(1), (999).toExponential(0)].join('|')"),
        "1.0e+2|1.0e+1|1e+3"
    );
    // Step 4 comes before step 5, so a value with no digits answers before the count is judged —
    // the one place here where the order of two guards is visible.
    assert_eq!(
        run("[(NaN).toExponential(101), (Infinity).toExponential(-1)].join('|')"),
        "NaN|Infinity"
    );
    assert_eq!(
        run("try { (1).toExponential(101); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
}

#[test]
fn to_precision_chooses_its_spelling_by_where_the_first_digit_falls() {
    // §21.1.3.5 counts *significant* digits rather than places after the point.
    assert_eq!(
        run("[(5.123456).toPrecision(1), (5.123456).toPrecision(2), \
             (5.123456).toPrecision(4)].join('|')"),
        "5|5.1|5.123"
    );
    // Fewer digits than the number has means padding, not truncation.
    assert_eq!(
        run("[(1).toPrecision(5), (0).toPrecision(3), (0).toPrecision(1)].join('|')"),
        "1.0000|0.00|0"
    );
    // Step 12's boundary, and it is exactly where `e < -6` becomes true: one more zero after the
    // point and the spelling changes.
    assert_eq!(
        run("[(0.000001).toPrecision(1), (0.0000001).toPrecision(1)].join('|')"),
        "0.000001|1e-7"
    );
    // …and the other end, where the exponent reaches the precision asked for.
    assert_eq!(
        run("[(123.456).toPrecision(2), (123.456).toPrecision(3), \
             (123.456).toPrecision(4)].join('|')"),
        "1.2e+2|123|123.5"
    );
    // A negative keeps its sign through both spellings, and rounds by magnitude.
    assert_eq!(
        run("[(-1.5).toPrecision(1), (-0.0000001).toPrecision(2)].join('|')"),
        "-2|-1.0e-7"
    );
    // Step 2 — no argument at all is `ToString`, decided before anything else including the range.
    assert_eq!(
        run("[(1.5).toPrecision(), (1e21).toPrecision(), (NaN).toPrecision(101)].join('|')"),
        "1.5|1e+21|NaN"
    );
    // Step 5 — one to a hundred, so nought is refused where `toFixed` allows it.
    for bad in ["0", "101", "-1"] {
        assert_eq!(
            run(&format!(
                "try {{ (1).toPrecision({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "toPrecision({bad})"
        );
    }
}

#[test]
fn the_argument_is_converted_before_the_receiver_is_asked_whether_it_has_digits() {
    // All three convert the count at step 2 or 3, *before* asking whether the receiver has any
    // digits — so a Symbol is a TypeError even when the answer would have been "NaN" and never
    // needed the count at all. Converting after the shortcut lets these three succeed silently.
    for method in ["toFixed", "toExponential", "toPrecision"] {
        assert_eq!(
            run(&format!(
                "try {{ (NaN).{method}(Symbol()); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{method} with a Symbol count"
        );
    }
    // …and the conversion really runs, once, wherever it leads.
    assert_eq!(
        run(
            "var n = 0; var o = {valueOf: function () { n++; return 2; }};              (1.234).toFixed(o) + ',' + (1.234).toExponential(o) + ',' + n"
        ),
        "1.23,1.23e+0,2"
    );
    // The **range** check is the other way about, and the three do not agree. §21.1.3.3 refuses a
    // bad count before answering "NaN"; §21.1.3.2 and §21.1.3.5 answer first and never look. That
    // disagreement is the whole reason these are three orderings rather than one.
    assert_eq!(
        run("try { (NaN).toFixed(101); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run(
            "[(NaN).toExponential(101), (NaN).toPrecision(101),              (Infinity).toExponential(-1), (-Infinity).toPrecision(0)].join('|')"
        ),
        "NaN|NaN|Infinity|-Infinity"
    );
}
