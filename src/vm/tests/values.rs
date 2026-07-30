//! Expressions over primitives — the operators, and what each of them means.
//!
//! Every row runs *source* rather than asserting on a chunk: an instruction sequence is an
//! implementation detail and a value is not.

use super::*;

#[test]
fn a_literal_evaluates_to_itself() {
    // The floor everything else stands on. `false` is here rather than assumed because a
    // compiler that pushed `true` for both would pass every other test in this file.
    assert_eq!(eval("1"), "1");
    assert_eq!(eval("1.5"), "1.5");
    assert_eq!(eval("true"), "true");
    assert_eq!(eval("false"), "false");
    assert_eq!(eval("null"), "null");
    assert_eq!(eval("'text'"), "text");
    assert_eq!(eval("''"), "");
    // …and a Number literal is written back the way §6.1.6.1.20 writes it, not the way the
    // source spelled it: `0x10` is `16` and `1e3` is `1000`.
    assert_eq!(eval("0x10"), "16");
    assert_eq!(eval("1e3"), "1000");
    assert_eq!(eval("1_000"), "1000");
    assert_eq!(eval("1e21"), "1e+21");
}

#[test]
fn arithmetic_comes_out_the_way_the_language_says() {
    // Precedence and associativity are the parser's; that they survive into the bytecode is
    // this test's. `**` is the one right-associative operator, so the sixth row is 512 and
    // not 64.
    assert_eq!(eval("1 + 2"), "3");
    assert_eq!(eval("1 + 2 * 3"), "7");
    assert_eq!(eval("(1 + 2) * 3"), "9");
    assert_eq!(eval("7 % 3"), "1");
    assert_eq!(eval("-7 % 3"), "-1");
    assert_eq!(eval("2 ** 3 ** 2"), "512");
    assert_eq!(eval("1 / 0"), "Infinity");
    assert_eq!(eval("-1 / 0"), "-Infinity");
    assert_eq!(eval("0 / 0"), "NaN");
    // Subtraction and division are not commutative, so an operand order bug in the VM shows
    // up here and almost nowhere else.
    assert_eq!(eval("10 - 3"), "7");
    assert_eq!(eval("10 / 4"), "2.5");
    assert_eq!(eval("2 ** -1"), "0.5");
}

#[test]
fn plus_concatenates_as_soon_as_either_side_is_a_string() {
    assert_eq!(eval("'a' + 'b'"), "ab");
    assert_eq!(eval("1 + '1'"), "11");
    assert_eq!(eval("'1' + 1"), "11");
    // …and grouping decides which: the first is `(1 + 2) + "3"`, the second `"3" + 1` then
    // `+ 2`. Left associativity is the whole difference.
    assert_eq!(eval("1 + 2 + '3'"), "33");
    assert_eq!(eval("'3' + 1 + 2"), "312");
    // Every other operator reads the String as a Number instead.
    assert_eq!(eval("'3' - 1"), "2");
    assert_eq!(eval("'3' * '4'"), "12");
    assert_eq!(eval("'a' - 1"), "NaN");
}

#[test]
fn the_unary_operators_are_each_one_conversion() {
    assert_eq!(eval("-'5'"), "-5");
    assert_eq!(eval("+'5'"), "5");
    assert_eq!(eval("+'a'"), "NaN");
    assert_eq!(eval("!0"), "true");
    assert_eq!(eval("!''"), "true");
    assert_eq!(eval("!'0'"), "false");
    assert_eq!(eval("!!1"), "true");
    assert_eq!(eval("~5"), "-6");
    assert_eq!(eval("~'abc'"), "-1");
    assert_eq!(eval("~~1.7"), "1");
    assert_eq!(eval("void 0"), "undefined");
    assert_eq!(eval("typeof 1"), "number");
    assert_eq!(eval("typeof 'a'"), "string");
    assert_eq!(eval("typeof true"), "boolean");
    assert_eq!(eval("typeof null"), "object");
    assert_eq!(eval("typeof void 0"), "undefined");
    // Negation keeps the sign where subtraction does not, and `String` hides it again — so
    // the difference is only visible by dividing into it.
    assert_eq!(eval("1 / -0"), "-Infinity");
    assert_eq!(eval("1 / (0 - 0)"), "Infinity");
}

#[test]
fn comparison_and_equality_agree_with_the_algorithms_they_come_from() {
    assert_eq!(eval("1 < 2"), "true");
    assert_eq!(eval("'10' < '9'"), "true");
    assert_eq!(eval("'10' < 9"), "false");
    // `undefined` is spelled `void 0` here because it is an *identifier*, not a literal —
    // which is exactly why minifiers write it that way, and why the compiler cannot read the
    // other spelling until names resolve.
    assert_eq!(eval("null == void 0"), "true");
    assert_eq!(eval("null === void 0"), "false");
    assert_eq!(eval("'' == 0"), "true");
    assert_eq!(eval("'' === 0"), "false");
    assert_eq!(eval("'1' == true"), "true");
    assert_eq!(eval("'true' == true"), "false");
    assert_eq!(eval("0 / 0 == 0 / 0"), "false");
    assert_eq!(eval("1 <= 1"), "true");
    assert_eq!(eval("(0 / 0) <= 1"), "false");
    assert_eq!(eval("1 << 32"), "1");
    assert_eq!(eval("-1 >>> 0"), "4294967295");
}

#[test]
fn the_three_bitwise_operators_are_three_different_operators() {
    // Chosen so that no two of `&`, `|` and `^` agree: 12 is 1100 and 10 is 1010, so the
    // three answers are 1000, 1110 and 0110. A table of equal-answer rows would let any two
    // of them be swapped without a test noticing.
    assert_eq!(eval("12 & 10"), "8");
    assert_eq!(eval("12 | 10"), "14");
    assert_eq!(eval("12 ^ 10"), "6");
    // Through ToInt32, which is where the 32-bit truncation and the sign come from.
    assert_eq!(eval("2147483648 | 0"), "-2147483648");
    assert_eq!(eval("4294967296 | 0"), "0");
    assert_eq!(eval("-1 & 255"), "255");
    assert_eq!(eval("1.9 | 0"), "1");
    assert_eq!(eval("'abc' | 0"), "0");
    assert_eq!(eval("(0 / 0) | 0"), "0");
}

#[test]
fn each_comparison_is_a_different_comparison() {
    // Every one of the eight, on operands where the answers differ — so that no two of them
    // can be confused for each other and no negation can be dropped.
    assert_eq!(eval("1 < 2"), "true");
    assert_eq!(eval("2 < 1"), "false");
    assert_eq!(eval("1 > 2"), "false");
    assert_eq!(eval("2 > 1"), "true");
    assert_eq!(eval("1 <= 2"), "true");
    assert_eq!(eval("2 <= 1"), "false");
    assert_eq!(eval("1 >= 2"), "false");
    assert_eq!(eval("2 >= 1"), "true");
    // …and the two negations, which a missing `!` would turn into their opposites.
    assert_eq!(eval("1 == 1"), "true");
    assert_eq!(eval("1 != 1"), "false");
    assert_eq!(eval("1 != 2"), "true");
    assert_eq!(eval("1 === 1"), "true");
    assert_eq!(eval("1 !== 1"), "false");
    assert_eq!(eval("1 !== '1'"), "true");
    assert_eq!(eval("1 != '1'"), "false");
}

#[test]
fn an_infinite_exponent_is_nan_only_over_a_base_of_magnitude_one() {
    // §6.1.6.1.3 steps 11 and 12. The guard is a conjunction, and loosening it either way is
    // wrong in a different direction — so both halves need a row that says so.
    assert_eq!(eval("1 ** (1 / 0)"), "NaN");
    assert_eq!(eval("(0 - 1) ** (1 / 0)"), "NaN");
    assert_eq!(eval("2 ** (1 / 0)"), "Infinity");
    assert_eq!(eval("0.5 ** (1 / 0)"), "0");
    assert_eq!(eval("1 ** 2"), "1");
    assert_eq!(eval("(0 - 1) ** 3"), "-1");
}

#[test]
fn a_short_circuit_answers_with_the_operand_that_decided() {
    // The thing that makes `&&` and `||` operators rather than `if` in disguise: the value
    // that stopped the evaluation *is* the answer. `0 || 'a'` is `'a'`, and `1 || 'a'` is
    // `1` and not `true`.
    assert_eq!(eval("1 && 2"), "2");
    assert_eq!(eval("0 && 2"), "0");
    assert_eq!(eval("'' && 2"), "");
    assert_eq!(eval("1 || 2"), "1");
    assert_eq!(eval("0 || 2"), "2");
    assert_eq!(eval("'' || 'a'"), "a");
    assert_eq!(eval("null || 'a'"), "a");
    // Chained, and left-associative: `a && b && c`.
    assert_eq!(eval("1 && 2 && 3"), "3");
    assert_eq!(eval("1 && 0 && 3"), "0");
    assert_eq!(eval("0 || '' || 'last'"), "last");
    // Mixed with an operator that is not short-circuiting, to check the stack comes out level.
    assert_eq!(eval("(1 && 2) + 1"), "3");
    assert_eq!(eval("1 + (0 || 5)"), "6");
}

#[test]
fn nullish_coalescing_asks_a_different_question_from_or() {
    // The whole reason `??` was added: `||` tests truthiness and `??` tests only `null` and
    // `undefined`, so every falsy value that is not nullish is where they part company.
    assert_eq!(eval("0 || 'fallback'"), "fallback");
    assert_eq!(eval("0 ?? 'fallback'"), "0");
    assert_eq!(eval("'' ?? 'fallback'"), "");
    assert_eq!(eval("false ?? 'fallback'"), "false");
    assert_eq!(eval("(0 / 0) ?? 'fallback'"), "NaN");
    // …and where they agree.
    assert_eq!(eval("null ?? 'fallback'"), "fallback");
    assert_eq!(eval("void 0 ?? 'fallback'"), "fallback");
    assert_eq!(eval("1 ?? 'fallback'"), "1");
}

#[test]
fn the_conditional_operator_evaluates_one_branch_and_never_the_test() {
    // Unlike a short circuit, the test is thrown away: `a ? b : c` is `b` or `c` and is never
    // `a`, however truthy `a` was.
    assert_eq!(eval("1 ? 'yes' : 'no'"), "yes");
    assert_eq!(eval("0 ? 'yes' : 'no'"), "no");
    assert_eq!(eval("'' ? 'yes' : 'no'"), "no");
    assert_eq!(eval("'0' ? 'yes' : 'no'"), "yes");
    assert_eq!(eval("null ? 'yes' : 'no'"), "no");
    // Right-associative, so this is `1 ? 'a' : (0 ? 'b' : 'c')` and nesting works in both
    // branches — the two jumps have to be patched to different places.
    assert_eq!(eval("1 ? 'a' : 0 ? 'b' : 'c'"), "a");
    assert_eq!(eval("0 ? 'a' : 0 ? 'b' : 'c'"), "c");
    assert_eq!(eval("0 ? 'a' : 1 ? 'b' : 'c'"), "b");
    assert_eq!(eval("(1 ? 2 : 3) + 10"), "12");
}

#[test]
fn the_comma_operator_keeps_the_last_value_and_discards_the_rest() {
    assert_eq!(eval("(1, 2)"), "2");
    assert_eq!(eval("(1, 2, 3)"), "3");
    assert_eq!(eval("(1, 2) + 1"), "3");
    // Each earlier operand is still *evaluated* — the discarding is of the value, not of the
    // work — which is the only reason anyone writes one.
    assert_eq!(eval("('a' + 'b', 'c')"), "c");
}

#[test]
fn an_update_coerces_the_old_value_before_it_adds_one() {
    // §13.4.4.1 step 3 — `ToNumeric` runs on the *old* value, and only then is one added. Do it
    // the other way round and `x = "1"; x++` concatenates: the row below would be "11".
    assert_eq!(run("var x = '1'; var r = x++; r + '|' + x"), "1|2");
    assert_eq!(run("var x = '1'; var r = ++x; r + '|' + x"), "2|2");
    // The value a postfix one produces is the coerced old one, not the original — so it is a
    // Number even when what was there was a String.
    assert_eq!(run("var x = '1'; typeof (x++)"), "number");
    assert_eq!(run("var x = 'abc'; var r = x++; r + '|' + x"), "NaN|NaN");
    assert_eq!(run("var x = '0x10'; x++; x"), "17");
    assert_eq!(run("var x = 1; var r = x--; r + '|' + x"), "1|0");
    assert_eq!(run("var x = 1; var r = --x; r + '|' + x"), "0|0");
}

#[test]
fn a_postfix_update_answers_the_old_value_and_a_prefix_one_the_new() {
    // The only difference between the two, and the reason the old value has to be kept rather
    // than computed back: at 2^53 adding one changes nothing, so `new - 1` is not the old value.
    assert_eq!(
        run("var x = 9007199254740992; var r = x++; r + '|' + x"),
        "9007199254740992|9007199254740992"
    );
    assert_eq!(
        run("var o = {p: 9007199254740992}; var r = o.p++; r + '|' + o.p"),
        "9007199254740992|9007199254740992"
    );
    // …and NaN, where nothing at all can be recovered from the new value.
    assert_eq!(
        run("var o = {p: NaN}; var r = o.p--; r + '|' + o.p"),
        "NaN|NaN"
    );
    assert_eq!(run("var x = 1; x++ + x++"), "3");
    assert_eq!(run("var x = 1; ++x + ++x"), "5");
}

#[test]
fn an_update_of_a_property_evaluates_its_key_once() {
    // §13.4.4.1 step 1 evaluates the reference once, and this is where that is observable: a
    // computed key with a side effect must not run twice between the read and the write.
    assert_eq!(
        run(
            "var n = 0; var o = {a: 5}; function f() { n = n + 1; return 'a' } var r = o[f()]++; r + '|' + o.a + '|' + n"
        ),
        "5|6|1"
    );
    assert_eq!(
        run(
            "var n = 0; var o = {a: 5}; function f() { n = n + 1; return 'a' } var r = ++o[f()]; r + '|' + o.a + '|' + n"
        ),
        "6|6|1"
    );
}

#[test]
fn instanceof_walks_the_prototype_chain_and_asks_nothing_else() {
    // §7.3.22 — it looks for the object `C.prototype` *holds* on the chain. Not which
    // constructor ran, which is why it is a question about objects rather than about history.
    let chain = "function A() {} function B() {} B.prototype = new A(); var b = new B();";
    assert_eq!(run(&format!("{chain} b instanceof B")), "true");
    assert_eq!(run(&format!("{chain} b instanceof A")), "true");
    assert_eq!(run(&format!("{chain} new A() instanceof B")), "false");
    // Reassigning `prototype` afterwards changes the answer for objects that already exist —
    // not a bug, and the reason `instanceof` is unreliable across realms.
    assert_eq!(
        run(&format!("{chain} B.prototype = {{}}; b instanceof B")),
        "false"
    );
    // A primitive is an instance of nothing, and that is an answer rather than an error.
    assert_eq!(run("function f() {} 1 instanceof f"), "false");
    assert_eq!(run("function f() {} null instanceof f"), "false");
}

#[test]
fn instanceof_refuses_a_right_operand_it_cannot_ask() {
    // Three different mistakes, so three different sentences. §13.10.2 step 3 — not an object.
    assert_eq!(
        run("try { 1 instanceof 2 } catch (e) { e.name }"),
        "TypeError"
    );
    // Step 5 — an object, but not callable, which is what `1 instanceof {}` reaches.
    assert_eq!(
        run("try { 1 instanceof ({}) } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { 1 instanceof ({}) } catch (e) { e.message }"),
        "the right operand of instanceof is not callable"
    );
    // §7.3.22 step 5 — callable, but its `prototype` is not an object, so there is no chain to
    // look on. Reachable only by assigning one, which is why it needs its own row.
    assert_eq!(
        run("function f() {} f.prototype = 1; try { ({}) instanceof f } catch (e) { e.message }"),
        "the prototype of the right operand of instanceof is not an object"
    );
    // A callable with an object prototype answers rather than throwing, even for a primitive
    // left operand — which is what makes the rows above about the *right* operand.
    assert_eq!(run("function f() {} 1 instanceof f"), "false");
}

#[test]
fn a_prototype_chain_of_any_length_answers_without_running_out_of_stack() {
    // The walk is over data, and data is as deep as a program makes it. Recursing would run out
    // of Rust stack on a long chain rather than on nesting, which DR-0002 does not allow.
    let deep = "function A() {} var o = {}; var i = 0; while (i < 50000) { var n = {}; \
                var k = n; o = n; i = i + 1; } A.prototype = {}; o instanceof A";
    assert_eq!(run(deep), "false");
}

#[test]
fn a_logical_assignment_only_stores_when_the_circuit_does_not_fire() {
    // §13.15.2 — these three are not compound assignments. `a += f()` always calls `f`; `a ||= f()`
    // calls it only when the store is going to happen, and that is the whole difference.
    assert_eq!(
        run("(function () { var a = 0; a ||= 5; return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var a = 1; a ||= 5; return a; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var a = 1; a &&= 5; return a; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var a = 0; a &&= 5; return a; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var a = null; a ??= 5; return a; })()"),
        "5"
    );
    // `??` tests for nullish and not for falsy, which is why it exists: `0 ||= 5` stores and
    // `0 ??= 5` does not.
    assert_eq!(
        run("(function () { var a = 0; a ??= 5; return a; })()"),
        "0"
    );
    // The right-hand side is never evaluated when the circuit fires — asserted by counting calls,
    // because a value alone cannot show whether a function ran.
    assert_eq!(
        run(
            "(function () { var n = 0; var f = function () { n++; return 9; }; \
             var a = 1; a ||= f(); return a + ',' + n; })()"
        ),
        "1,0"
    );
    assert_eq!(
        run(
            "(function () { var n = 0; var f = function () { n++; return 9; }; \
             var a = 0; a ||= f(); return a + ',' + n; })()"
        ),
        "9,1"
    );
    // The expression answers the old value when the circuit fires and the new one when it does not.
    assert_eq!(run("(function () { var a = 1; return (a ||= 5); })()"), "1");
    assert_eq!(run("(function () { var a = 0; return (a ||= 5); })()"), "5");
}

#[test]
fn a_logical_assignment_to_a_property_evaluates_the_reference_once() {
    assert_eq!(
        run("(function () { var o = {p: 0}; o.p ||= 7; return o.p; })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var o = {p: 1}; o.p ||= 7; return o.p; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var o = {p: null}; o.p ??= 3; return o.p; })()"),
        "3"
    );
    assert_eq!(
        run("(function () { var o = {p: 0}; return (o.p ||= 7); })()"),
        "7"
    );
    assert_eq!(
        run("(function () { var o = {p: 1}; return (o.p ||= 7); })()"),
        "1"
    );
    // §13.15.2 evaluates the reference *before* the test, so a computed key runs once even when the
    // circuit fires and no store happens. Twice would be observable and wrong.
    assert_eq!(
        run("(function () { var n = 0; var o = {p: 1}; \
             var k = function () { n++; return 'p'; }; o[k()] ||= 9; return o.p + ',' + n; })()"),
        "1,1"
    );
    assert_eq!(
        run("(function () { var n = 0; var o = {p: 0}; \
             var k = function () { n++; return 'p'; }; o[k()] ||= 9; return o.p + ',' + n; })()"),
        "9,1"
    );
    // And the right-hand side is not evaluated when the circuit fires, here either.
    assert_eq!(
        run(
            "(function () { var n = 0; var f = function () { n++; return 4; }; \
             var o = {p: 0}; o.p ??= f(); return o.p + ',' + n; })()"
        ),
        "0,0"
    );
    // A setter is reached only when the store happens, which is the property half of the same rule.
    assert_eq!(
        run("(function () { var ran = 0; var o = {}; \
             Object.defineProperty(o, 'p', {get: function () { return 1; }, \
                                            set: function () { ran++; }}); \
             o.p ||= 9; return ran; })()"),
        "0"
    );
}
