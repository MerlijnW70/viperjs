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
