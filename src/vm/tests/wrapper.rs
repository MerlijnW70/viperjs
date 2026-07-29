//! §20.3 `Boolean`, §21.1 `Number`, and §7.1.18's `ToObject`.
//!
//! The interesting rows are the ones where a wrapper is *not* the primitive it wraps: it is an
//! object, so it is truthy, it is not `===` to what it holds, and it can be given properties.

use super::*;

#[test]
fn a_wrapper_constructor_converts_when_called_and_wraps_when_constructed() {
    // §20.3.1.1 and §21.1.1.1 — the same function, and the difference is `new`.
    assert_eq!(run("typeof Number(1)"), "number");
    assert_eq!(run("typeof new Number(1)"), "object");
    assert_eq!(run("typeof Boolean(1)"), "boolean");
    assert_eq!(run("typeof new Boolean(1)"), "object");
    assert_eq!(
        run("Number('12') + ',' + Number(null) + ',' + Number(true)"),
        "12,0,1"
    );
    assert_eq!(
        run("Boolean(0) + ',' + Boolean('x') + ',' + Boolean()"),
        "false,true,false"
    );
    // §21.1.1.1 step 1 — no argument at all is `+0`, which is not `ToNumber(undefined)`. So
    // `Number()` and `Number(undefined)` differ, and only one of them is `NaN`.
    assert_eq!(run("Number() + ',' + Number(undefined)"), "0,NaN");
    // The wrapper is an *object*, which is the whole of why this surprises people: every object
    // is truthy, so a wrapper of `false` takes the branch.
    assert_eq!(run("new Boolean(false) ? 'truthy' : 'falsy'"), "truthy");
    assert_eq!(run("new Number(3) == 3"), "true");
    assert_eq!(run("new Number(3) === 3"), "false");
    // …and it is ordinary in every other way: an ordinary prototype, and properties of its own.
    assert_eq!(
        run("Object.getPrototypeOf(new Number(1)) === Number.prototype"),
        "true"
    );
    assert_eq!(run("var n = new Number(1); n.x = 5; n.x"), "5");
    assert_eq!(run("Number.prototype.constructor === Number"), "true");
    assert_eq!(run("Boolean.prototype.constructor === Boolean"), "true");
    // §20.3.2.1 and §21.1.2.15 — a constructor's `prototype` is none of the three, as every
    // constructor's is: an instance already inherits from it before a script could move it.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Number, 'prototype');              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "false,false,false"
    );
    assert_eq!(
        run("Number.prototype = 1; typeof Number.prototype"),
        "object"
    );
    // §20.3.3.2 — and the boolean's own `toString`, which is the one method whose answer is the
    // value it was given rather than a conversion of it.
    assert_eq!(
        run("true.toString() + ',' + false.toString()"),
        "true,false"
    );
    assert_eq!(
        run("new Boolean(true).toString() + ',' + new Boolean(false).toString()"),
        "true,false"
    );
}

#[test]
fn a_method_of_one_kind_will_not_read_the_other_kind() {
    // §20.3.3's `thisBooleanValue` and §21.1.3's `thisNumberValue`. praxis keeps one slot holding
    // the primitive and lets the *value* say which kind it is, so this is the row that proves the
    // two are still told apart.
    assert_eq!(run("new Number(5).valueOf()"), "5");
    assert_eq!(run("new Boolean(true).valueOf()"), "true");
    assert_eq!(
        run("try { Boolean.prototype.valueOf.call(new Number(1)); } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Number.prototype.valueOf.call(new Boolean(1)); } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Number.prototype.valueOf.call({}); } catch (e) { e.name }"),
        "TypeError"
    );
    // §21.1.3 — `Number.prototype` is an ordinary object and not a Number wrapper, so it has no
    // primitive of its own to answer with.
    assert_eq!(
        run("try { Number.prototype.valueOf.call(Number.prototype); } catch (e) { e.name }"),
        "TypeError"
    );
    // A primitive receiver answers itself, which is what a built-in sees: §10.3.1 does no
    // substitution, so the number arrives unwrapped.
    assert_eq!(run("(3.5).valueOf()"), "3.5");
    assert_eq!(run("true.valueOf()"), "true");
}

#[test]
fn a_number_is_written_in_the_radix_it_is_asked_for() {
    // §21.1.3.6 — radix 10 is §6.1.6.1.20's exactly-specified algorithm, and every other radix is
    // implementation-approximated. These are the exact ones.
    assert_eq!(run("(255).toString(16)"), "ff");
    assert_eq!(run("(255).toString(2)"), "11111111");
    assert_eq!(run("(255).toString()"), "255");
    assert_eq!(run("(-255).toString(16)"), "-ff");
    assert_eq!(run("(0).toString(36)"), "0");
    assert_eq!(run("(35).toString(36)"), "z");
    assert_eq!(run("(0.5).toString(2)"), "0.1");
    assert_eq!(run("(1e21).toString()"), "1e+21");
    // Step 3 — outside 2 to 36 is a RangeError, and so is a radix that rounds to outside it.
    assert_eq!(
        run("try { (1).toString(1); } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { (1).toString(37); } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { (1).toString(0); } catch (e) { e.name }"),
        "RangeError"
    );
    // …while `undefined` is not "outside", it is *absent*, and absent means ten.
    assert_eq!(run("(255).toString(undefined)"), "255");
    // The three values that have no digits at all, in a radix that would otherwise try to find
    // some. An infinity is the one that matters most: dividing it by the radix for ever is a loop
    // that never ends rather than a wrong answer.
    assert_eq!(run("(NaN).toString(2)"), "NaN");
    assert_eq!(run("(Infinity).toString(2)"), "Infinity");
    assert_eq!(run("(-Infinity).toString(2)"), "-Infinity");
    assert_eq!(run("(0).toString(2)"), "0");
    // A negative zero has no sign to write, because it is not less than zero.
    assert_eq!(run("(-0).toString(2)"), "0");
    assert_eq!(run("(-0.5).toString(2)"), "-0.1");
}

#[test]
fn the_number_predicates_do_not_convert_and_that_is_the_point_of_them() {
    // §21.1.2.2 to §21.1.2.5 — the global `isNaN` converts its argument and these do not, which
    // is the entire reason they were added.
    assert_eq!(
        run("Number.isNaN(NaN) + ',' + Number.isNaN('x') + ',' + Number.isNaN(1)"),
        "true,false,false"
    );
    assert_eq!(
        run("Number.isFinite(1) + ',' + Number.isFinite('1') + ',' + Number.isFinite(Infinity)"),
        "true,false,false"
    );
    assert_eq!(
        run("Number.isInteger(1) + ',' + Number.isInteger(1.5) + ',' + Number.isInteger('1')"),
        "true,false,false"
    );
    assert_eq!(
        run(
            "Number.isSafeInteger(9007199254740991) + ',' + Number.isSafeInteger(9007199254740992)"
        ),
        "true,false"
    );
    // A wrapper is not a Number for these either — they ask about the value, not about what it
    // could be converted to.
    assert_eq!(run("Number.isInteger(new Number(1))"), "false");
    // §21.1.2 — the constants.
    assert_eq!(run("Number.MAX_SAFE_INTEGER"), "9007199254740991");
    assert_eq!(run("Number.MIN_SAFE_INTEGER"), "-9007199254740991");
    assert_eq!(run("Number.EPSILON"), "2.220446049250313e-16");
    assert_eq!(run("Number.MAX_VALUE"), "1.7976931348623157e+308");
    assert_eq!(run("Number.MIN_VALUE"), "5e-324");
    assert_eq!(
        run("Number.POSITIVE_INFINITY + ',' + Number.NEGATIVE_INFINITY"),
        "Infinity,-Infinity"
    );
    assert_eq!(run("Number.NaN"), "NaN");
    assert_eq!(
        run("Number.MAX_VALUE = 1; Number.MAX_VALUE"),
        "1.7976931348623157e+308"
    );
}

#[test]
fn reading_a_property_of_a_primitive_finds_its_prototype() {
    // §7.3.2 `GetV` — a primitive is wrapped and the read goes to the wrapper. praxis consults
    // the prototype directly instead, which is the same answer because a Number wrapper and a
    // Boolean wrapper have no own properties at all.
    assert_eq!(run("typeof (1).toString"), "function");
    assert_eq!(run("typeof true.valueOf"), "function");
    assert_eq!(run("(1).missing"), "undefined");
    assert_eq!(run("(1).constructor === Number"), "true");
    // A built-in called as a *callback* is not constructing, whatever it would do if it were:
    // `[1].map(Number)` gives numbers and not wrappers.
    assert_eq!(run("typeof [1].map(Number)[0]"), "number");
    assert_eq!(run("typeof [1].map(Boolean)[0]"), "boolean");
    // …while the two that have no object are still the error §7.3.2 step 2 asks for.
    assert_eq!(run("try { null.x; } catch (e) { e.name }"), "TypeError");
    assert_eq!(
        run("try { undefined.x; } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn to_object_of_a_primitive_is_a_wrapper_and_the_tag_says_which() {
    // §20.1.1.1 step 3 — `Object(x)` of a primitive is `ToObject(x)`.
    assert_eq!(run("typeof Object(1)"), "object");
    assert_eq!(run("Object(1).valueOf()"), "1");
    assert_eq!(run("Object(true).valueOf()"), "true");
    assert_eq!(run("Object(1) === Object(1)"), "false");
    // …and `undefined` and `null` get a fresh ordinary object rather than an error, which is what
    // steps 1 and 2 say and is not what `ToObject` alone would do.
    assert_eq!(run("typeof Object()"), "object");
    assert_eq!(run("typeof Object(null)"), "object");
    // §20.1.3.6 steps 9 and 10 — a wrapper is tagged by what it wraps.
    assert_eq!(run("Object.prototype.toString.call(1)"), "[object Number]");
    assert_eq!(
        run("Object.prototype.toString.call(true)"),
        "[object Boolean]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Number(1))"),
        "[object Number]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Boolean(1))"),
        "[object Boolean]"
    );
    assert_eq!(run("Object.prototype.toString.call({})"), "[object Object]");
    assert_eq!(run("Object.prototype.toString.call([])"), "[object Array]");
}
