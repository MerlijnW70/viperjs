//! §6.1.6.2's BigInt as a *language* value — the literal, the operators, and what refuses to mix.
//!
//! The arithmetic itself is tested in `crate::bigint`. What is here is everything the type does
//! once a program can write it down: the one place a JavaScript value has no width, and the rules
//! that keep it from quietly becoming one that does.
//!
//! # The line the whole type is drawn along
//!
//! **Arithmetic does not mix and comparison does.** `1n + 1` is a TypeError; `1n == 1` is true.
//! That is not an inconsistency: comparing asks whether two values are the same point on the
//! number line, which has an answer at every width, where arithmetic has to *produce* a value and
//! would have to choose a width to produce it in. Choosing loses either precision or magnitude, so
//! the specification stops instead.

use super::*;

#[test]
fn a_literal_is_a_value_of_its_own_type() {
    assert_eq!(run("typeof 1n"), "bigint");
    assert_eq!(run("typeof 1"), "number");
    // The magnitude is the program's to choose, which is the whole point of the type: this is
    // eleven digits past what a Number can hold exactly and comes back unchanged.
    assert_eq!(
        run("String(123456789012345678901234567890n)"),
        "123456789012345678901234567890"
    );
    // Every radix the literal grammar has, and no `n` in the output: the suffix is syntax.
    assert_eq!(
        run("String(0xffn) + ',' + String(0o17n) + ',' + String(0b101n)"),
        "255,15,5"
    );
    assert_eq!(run("String(1_000_000n)"), "1000000");
    // A negative literal is a unary minus applied to a positive one, exactly as for a Number.
    assert_eq!(run("String(-42n)"), "-42");
    assert_eq!(run("String(-0n)"), "0");
}

#[test]
fn arithmetic_between_a_bigint_and_a_number_is_refused() {
    // §13.15.3 step 3, and this is the error people meet first. Every arithmetic operator, both
    // ways round, because a rule applied to one of them is a rule half-written.
    for source in [
        "1n + 1", "1 + 1n", "1n - 1", "1 - 1n", "1n * 2", "2 * 1n", "1n / 2", "2 / 1n", "1n % 2",
        "2 % 1n", "1n ** 2", "2 ** 1n", "1n & 1", "1 & 1n", "1n | 1", "1n ^ 1", "1n << 1",
        "1n >> 1",
    ] {
        assert_eq!(
            run(&format!(
                "var e = 'none'; try {{ {source} }} catch (x) {{ e = x.constructor.name }} e"
            )),
            "TypeError",
            "{source} should not mix"
        );
    }
    // …and unary `+`, which is the one unary operator a BigInt does not have: it *is* `ToNumber`,
    // and §7.1.4 refuses a BigInt. Every other unary operator has a BigInt meaning.
    assert_eq!(
        run("var e = 'none'; try { +1n } catch (x) { e = x.constructor.name } e"),
        "TypeError"
    );
    // A String is not arithmetic, so `+` concatenates rather than refusing — the String test in
    // §13.15.3 step 1 comes before the numeric one.
    assert_eq!(run("1n + 'a'"), "1a");
    assert_eq!(run("'a' + 1n"), "a1");
}

#[test]
fn every_operator_that_does_have_a_bigint_meaning_has_it() {
    assert_eq!(run("String(1n + 2n)"), "3");
    assert_eq!(run("String(1n - 2n)"), "-1");
    assert_eq!(run("String(6n * 7n)"), "42");
    assert_eq!(run("String(2n ** 100n)"), "1267650600228229401496703205376");
    // Truncated towards zero, with the remainder taking the dividend's sign — §6.1.6.2.5 and
    // §6.1.6.2.6, which together make `(a / b) * b + (a % b)` equal `a`.
    assert_eq!(run("String(7n / 2n) + ',' + String(-7n / 2n)"), "3,-3");
    assert_eq!(run("String(7n % 2n) + ',' + String(-7n % 2n)"), "1,-1");
    assert_eq!(
        run("String(-1n) + ',' + String(~0n) + ',' + String(~-1n)"),
        "-1,-1,0"
    );
    assert_eq!(
        run("String(12n & 10n) + ',' + String(12n | 10n) + ',' + String(12n ^ 10n)"),
        "8,14,6"
    );
    // An arithmetic shift, which rounds towards negative infinity where a division truncates.
    assert_eq!(run("String(-1n >> 1n) + ',' + String(-1n / 2n)"), "-1,0");
    assert_eq!(run("String(1n << 64n)"), "18446744073709551616");
}

#[test]
fn the_two_operations_a_bigint_refuses_say_which_and_why() {
    // §6.1.6.2.5 step 1 — a RangeError where a Number would answer `Infinity`, because there is no
    // BigInt infinity for it to be.
    assert_eq!(
        run("var e = 'none'; try { 1n / 0n } catch (x) { e = x.constructor.name } e"),
        "RangeError"
    );
    assert_eq!(
        run("var e = 'none'; try { 1n % 0n } catch (x) { e = x.constructor.name } e"),
        "RangeError"
    );
    // §6.1.6.2.3 step 1 — a negative power is one over something, and a BigInt is an integer.
    assert_eq!(
        run("var e = 'none'; try { 2n ** -1n } catch (x) { e = x.constructor.name } e"),
        "RangeError"
    );
    // §6.1.6.2.11 — `>>>` fills from the left with zeros, which needs a width. A **TypeError**,
    // not a RangeError: it is not that the answer is out of range, it is that there is no answer.
    assert_eq!(
        run("var e = 'none'; try { 1n >>> 1n } catch (x) { e = x.constructor.name } e"),
        "TypeError"
    );
}

#[test]
fn comparison_mixes_where_arithmetic_does_not() {
    // §7.2.13 and §7.2.15 — the other half of the type's contract. These *are* comparable across
    // the two numeric types, and exactly: the question has an answer at every width.
    assert_eq!(run("1n < 2"), "true");
    assert_eq!(run("2n > 1"), "true");
    assert_eq!(run("1n <= 1"), "true");
    assert_eq!(run("1n == 1"), "true");
    assert_eq!(run("1n != 2"), "true");
    // …and `===` still says no, because the *types* differ. That is what makes `==` and `===`
    // worth telling apart here rather than a wart.
    assert_eq!(run("1n === 1"), "false");
    assert_eq!(run("1n === 1n"), "true");
    // A fraction decides when the integer parts agree, in the direction its sign says.
    assert_eq!(run("1n < 1.5"), "true");
    assert_eq!(run("-1n > -1.5"), "true");
    assert_eq!(run("1n == 1.5"), "false");
    // **The comparison is exact on both sides.** Turning the BigInt into a Number would make
    // these two the same value, and they are not: `2 ** 53` is the first integer an `f64` cannot
    // tell from its neighbour.
    assert_eq!(run("(2n ** 53n + 1n) == 2 ** 53"), "false");
    assert_eq!(run("(2n ** 53n) == 2 ** 53"), "true");
    assert_eq!(run("(2n ** 53n + 1n) > 2 ** 53"), "true");
    // A NaN is not comparable to anything, so all three of these are false at once — the same
    // fold every Number comparison against a NaN goes through.
    assert_eq!(
        run("(1n < NaN) + ',' + (1n > NaN) + ',' + (1n == NaN)"),
        "false,false,false"
    );
    // An infinity *is* comparable, and equals no BigInt: every one of them is inside it.
    assert_eq!(run("(1n < Infinity) + ',' + (1n > -Infinity)"), "true,true");
}

#[test]
fn a_string_is_read_as_a_bigint_where_one_is_expected() {
    // §7.1.14 `StringToBigInt` — and text that is not an integer is simply *not equal* rather than
    // an error, which is the difference from `ToNumber` giving a NaN.
    assert_eq!(run("1n == '1'"), "true");
    assert_eq!(run("1n == ' 1 '"), "true");
    // All three radix prefixes, which §7.1.14 accepts — and none of them may carry a sign, so a
    // signed one is read as decimal, fails, and is simply not equal.
    assert_eq!(run("255n == '0xff'"), "true");
    assert_eq!(run("255n == '0o377'"), "true");
    assert_eq!(run("255n == '0b11111111'"), "true");
    assert_eq!(
        run("(-255n == '-0xff') + ',' + (-255n == '-0o377')"),
        "false,false"
    );
    // A prefix with no digits after it is not a number at all — and it is *not* zero, which is
    // what it would be if the digits were simply read as an empty run.
    assert_eq!(
        run("(0n == '0x') + ',' + (0n == '0o') + ',' + (0n == '0b')"),
        "false,false,false"
    );
    assert_eq!(run("-1n == '-1'"), "true");
    assert_eq!(run("0n == ''"), "true");
    assert_eq!(run("1n == '1.5'"), "false");
    assert_eq!(run("1n == 'abc'"), "false");
    // …and one that is not a BigInt makes the *ordering* undefined too, so both directions are
    // false rather than one of them being true.
    assert_eq!(run("(1n < 'abc') + ',' + (1n > 'abc')"), "false,false");
}

#[test]
fn a_bigint_converts_where_the_specification_says_it_may() {
    // §7.1.2 — zero is false and everything else true, which is the Number rule without the NaN.
    assert_eq!(
        run("(1n ? 'y' : 'n') + (0n ? 'y' : 'n') + (-1n ? 'y' : 'n')"),
        "yny"
    );
    assert_eq!(run("(!0n) + ',' + (!1n)"), "true,false");
    // §7.1.17 — it does become text, unlike a Symbol, and without the `n`.
    assert_eq!(run("String(255n)"), "255");
    assert_eq!(run("`${255n}`"), "255");
    assert_eq!(run("['a', 1n].join('-')"), "a-1");
    // §7.1.4 — and it does *not* become a Number, which is the conversion everything above rests
    // on. `Number(1n)` is how a program asks for it on purpose; the implicit one is refused.
    assert_eq!(
        run("var e = 'none'; try { Math.abs(1n) } catch (x) { e = x.constructor.name } e"),
        "TypeError"
    );
    // §13.2.5.1 — a BigInt property key is its digits, so it and the Number name the same slot.
    assert_eq!(run("var o = { 1n: 'a' }; o[1] + o['1']"), "aa");
}

#[test]
fn json_refuses_a_bigint_rather_than_writing_a_number() {
    // §25.5.2.2 step 10 — the only value `JSON.stringify` throws for. JSON has no integer syntax
    // that survives a round trip past 2^53, so writing `1n` as `1` would produce text that parses
    // back as a different value — and silently, which is the worst way to be wrong.
    assert_eq!(
        run("var e = 'none'; try { JSON.stringify(1n) } catch (x) { e = x.constructor.name } e"),
        "TypeError"
    );
    assert_eq!(
        run(
            "var e = 'none'; try { JSON.stringify({ a: 1n }) } catch (x) { e = x.constructor.name } e"
        ),
        "TypeError"
    );
}

#[test]
fn the_constructor_converts_on_purpose_what_the_operators_refuse_by_accident() {
    // §21.2.1 — the explicit conversion. Every implicit one is refused, so this is how a program
    // crosses between the two numeric types when it means to.
    assert_eq!(run("String(BigInt(42))"), "42");
    assert_eq!(
        run("String(BigInt('0xff')) + ',' + String(BigInt('-7'))"),
        "255,-7"
    );
    assert_eq!(run("String(BigInt(true)) + String(BigInt(false))"), "10");
    assert_eq!(run("typeof BigInt(1)"), "bigint");
    // §7.1.13 — and it refuses what it cannot do *exactly*. A Number that is not an integer has no
    // BigInt, so this rounds nothing: `Number('1.5')` succeeds where this cannot.
    for (source, error) in [
        ("BigInt(1.5)", "RangeError"),
        ("BigInt(NaN)", "RangeError"),
        ("BigInt(Infinity)", "RangeError"),
        // A String that is not an integer is a **SyntaxError**, which is the only conversion in
        // the language that throws one: there is no BigInt for a NaN to be.
        ("BigInt('1.5')", "SyntaxError"),
        ("BigInt('abc')", "SyntaxError"),
        ("BigInt(undefined)", "TypeError"),
        ("BigInt(null)", "TypeError"),
        ("BigInt(Symbol())", "TypeError"),
        // §21.2.1 step 1 — not a constructor, like `Symbol`. A wrapper is what a method call makes
        // for itself; there is no reason to ask for one.
        ("new BigInt(1)", "TypeError"),
    ] {
        assert_eq!(
            run(&format!(
                "var e = 'none'; try {{ {source} }} catch (x) {{ e = x.constructor.name }} e"
            )),
            error,
            "{source}"
        );
    }
}

#[test]
fn the_prototype_answers_through_a_wrapper_and_directly() {
    // §21.2.3.1's `ThisBigIntValue` takes both, because a method reached on a primitive has a
    // wrapper as its receiver and one reached on `Object(1n)` was already wrapped.
    assert_eq!(run("(255n).toString()"), "255");
    assert_eq!(
        run("(255n).toString(16) + ',' + (255n).toString(2)"),
        "ff,11111111"
    );
    assert_eq!(run("(-255n).toString(16)"), "-ff");
    assert_eq!(run("Object(1n).toString()"), "1");
    assert_eq!(run("typeof (1n).valueOf()"), "bigint");
    assert_eq!(run("Object.prototype.toString.call(1n)"), "[object BigInt]");
    // §21.2.3.5's attributes, which are what makes that tag *replaceable*: a script may delete it
    // and get `[object Object]` back, and may not reach it by enumeration.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(BigInt.prototype, Symbol.toStringTag);              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,true"
    );
    assert_eq!(
        run("delete BigInt.prototype[Symbol.toStringTag]; Object.prototype.toString.call(1n)"),
        "[object Object]"
    );
    // A radix outside 2 to 36 is a RangeError, the same range `Number.prototype.toString` uses.
    assert_eq!(
        run("var e = 'none'; try { (1n).toString(1) } catch (x) { e = x.constructor.name } e"),
        "RangeError"
    );
    // …and a method of this prototype on something that is not a BigInt refuses rather than
    // guessing, which is what makes the wrapper's `[[BigIntData]]` a brand.
    assert_eq!(
        run(
            "var e = 'none'; try { BigInt.prototype.toString.call(1) } catch (x) { e = x.constructor.name } e"
        ),
        "TypeError"
    );
}

#[test]
fn as_int_n_is_where_a_bigint_is_given_a_width() {
    // §21.2.2 — the two functions that exist because a BigInt has no width and the world it talks
    // to does: a database identifier, a hash, a protocol field. Everything is modulo 2^bits; the
    // pair differ only in whether the top bit is read as a sign.
    assert_eq!(run("String(BigInt.asUintN(8, 255n))"), "255");
    assert_eq!(run("String(BigInt.asIntN(8, 255n))"), "-1");
    assert_eq!(run("String(BigInt.asUintN(8, 256n))"), "0");
    assert_eq!(run("String(BigInt.asIntN(8, 128n))"), "-128");
    assert_eq!(run("String(BigInt.asIntN(8, 127n))"), "127");
    // A negative wraps *upwards* into the range, which is a modulo and not §6.1.6.2.6's remainder:
    // `-1n % 256n` is `-1n` and this is `255n`.
    assert_eq!(run("String(BigInt.asUintN(8, -1n))"), "255");
    assert_eq!(
        run("String(BigInt.asUintN(64, -1n))"),
        "18446744073709551615"
    );
    assert_eq!(
        run("String(BigInt.asIntN(64, 18446744073709551615n))"),
        "-1"
    );
    // Zero bits is the empty width, and everything modulo one is zero.
    assert_eq!(
        run("String(BigInt.asUintN(0, 7n)) + String(BigInt.asIntN(0, 7n))"),
        "00"
    );
}

#[test]
fn a_data_view_reads_and_writes_sixty_four_bits_as_a_bigint() {
    // §25.3.4's `BigInt64` pair — eight bytes, and the *only* difference between the two is
    // whether the top bit is read as a sign. Written once and read both ways says that in one
    // line, which two separate assertions would not.
    assert_eq!(
        run(
            "var d = new DataView(new ArrayBuffer(8)); d.setBigInt64(0, -1n);              String(d.getBigInt64(0)) + ',' + String(d.getBigUint64(0))"
        ),
        "-1,18446744073709551615"
    );
    // A value too large for the slot takes its low bits rather than being refused, which is what a
    // fixed width *is* — the same arithmetic `BigInt.asUintN(64, …)` does.
    assert_eq!(
        run(
            "var d = new DataView(new ArrayBuffer(8));              d.setBigUint64(0, 2n ** 64n + 7n); String(d.getBigUint64(0))"
        ),
        "7"
    );
    // Both endiannesses, since §25.3.4 defaults to *big* and the machine underneath does not.
    assert_eq!(
        run(
            "var d = new DataView(new ArrayBuffer(8)); d.setBigInt64(0, 1n, true);              String(d.getBigInt64(0, true)) + ',' + String(d.getBigInt64(0))"
        ),
        "1,72057594037927936"
    );
    // §25.3.1.2 step 4 is `ToBigInt`, which **refuses a Number** — so a `DataView` will not mix
    // the two numeric types either, and for the same reason the operators will not.
    assert_eq!(
        run(
            "var d = new DataView(new ArrayBuffer(8)); var e = 'none';              try { d.setBigInt64(0, 1) } catch (x) { e = x.constructor.name } e"
        ),
        "TypeError"
    );
    // …which is exactly where `BigInt(1)` differs: §21.2.1 converts a Number *before* reaching
    // §7.1.13's table, because an explicit call is a program saying it means to cross over.
    assert_eq!(run("String(BigInt(1))"), "1");
    // Past the end of the view is a RangeError, as it is for every other accessor.
    assert_eq!(
        run(
            "var d = new DataView(new ArrayBuffer(8)); var e = 'none';              try { d.getBigInt64(1) } catch (x) { e = x.constructor.name } e"
        ),
        "RangeError"
    );
}
