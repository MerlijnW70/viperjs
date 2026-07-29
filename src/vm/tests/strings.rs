//! §22.1 and §10.4.3 — `String`, its prototype, and the object that has a property per character.
//!
//! Every row was checked against V8 first. The ones worth reading twice are the boundaries: what a
//! position outside the string answers, which of `charAt` and `s[i]` gives `undefined`, and the
//! three ways `slice` and `substring` disagree about a backwards range.

use super::*;

#[test]
fn a_string_answers_its_own_length_and_characters() {
    // §10.4.3.4 and §10.4.3.5, reached from a *primitive* — §7.3.2 says a read from one reads from
    // the object it stands for, and these are the properties that object has.
    assert_eq!(run("'abc'.length"), "3");
    assert_eq!(run("''.length"), "0");
    assert_eq!(run("'abc'[1]"), "b");
    // Outside the string is `undefined`, because nothing is found rather than because an error is
    // raised — the ordinary end of a property lookup.
    assert_eq!(run("typeof 'abc'[9]"), "undefined");
    assert_eq!(run("typeof 'abc'[-1]"), "undefined");
    // §6.1.4 — a String is code *units*, so an astral character is two of everything.
    assert_eq!(run("'\\ud83d\\ude00'.length"), "2");
    assert_eq!(run("'\\ud83d\\ude00'.charCodeAt(0)"), "55357");
    // A method found on `String.prototype` through the same chain, and it is *the* method there
    // rather than one made for the read: a primitive receiver does not get an object of its own.
    assert_eq!(run("'abc'.charAt(1)"), "b");
    assert_eq!(run("'abc'.charAt === String.prototype.charAt"), "true");
    assert_eq!(
        run("Object.getPrototypeOf(Object('a')) === String.prototype"),
        "true"
    );
}

#[test]
fn the_constructor_converts_when_called_and_wraps_when_constructed() {
    assert_eq!(run("String(42)"), "42");
    assert_eq!(run("String(null)"), "null");
    assert_eq!(run("typeof String(42)"), "string");
    assert_eq!(run("typeof new String('a')"), "object");
    // §22.1.1.1 step 1 — *no argument* is the empty String, and an `undefined` one is not. The
    // only place in the language the two differ, so it is the one worth a row.
    assert_eq!(run("String() === ''"), "true");
    assert_eq!(run("String(undefined)"), "undefined");
    assert_eq!(run("new String('ab') instanceof String"), "true");
    assert_eq!(
        run("Object.prototype.toString.call(new String('a'))"),
        "[object String]"
    );
    // §22.1.3 — the prototype is itself a String object over the empty String, which is why it has
    // a `length` at all. `Number.prototype` is deliberately not like this.
    assert_eq!(run("String.prototype.length"), "0");
    // §22.1.2.1 — `ToUint16` of each argument, which wraps rather than refusing.
    assert_eq!(run("String.fromCharCode(72, 105)"), "Hi");
    assert_eq!(run("String.fromCharCode(65536) === '\\u0000'"), "true");
    assert_eq!(run("String.fromCharCode(-1).charCodeAt(0)"), "65535");
    assert_eq!(run("String.fromCharCode(65.9)"), "A");
    assert_eq!(run("String.fromCharCode() === ''"), "true");
}

#[test]
fn a_string_object_has_a_property_per_character_and_will_not_give_it_up() {
    // §10.4.3 — the characters are own properties and behave as such, but nothing may change one.
    assert_eq!(run("new String('ab').length"), "2");
    assert_eq!(run("new String('ab')[0]"), "a");
    assert_eq!(run("new String('ab').hasOwnProperty('1')"), "true");
    assert_eq!(run("new String('ab').hasOwnProperty('2')"), "false");
    assert_eq!(run("Object.keys(new String('ab')).join(',')"), "0,1");
    assert_eq!(
        run("Object.getOwnPropertyNames(new String('ab')).join(',')"),
        "0,1,length"
    );
    // §14.7.5.10 — enumerable, so `for`-`in` walks them; `length` is not, so it does not appear.
    assert_eq!(
        run(
            "(function () { var r = ''; for (var k in new String('ab')) { r += k; } return r; })()"
        ),
        "01"
    );
    // §10.4.3.5 step 8 — not writable, so an assignment is refused and the character stands.
    assert_eq!(
        run("(function () { var s = new String('ab'); s[0] = 'z'; return s[0]; })()"),
        "a"
    );
    // …and not configurable, so it cannot be deleted either.
    assert_eq!(
        run("(function () { var s = new String('ab'); return delete s[0]; })()"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('a'), '0').writable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('a'), '0').enumerable"),
        "true"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('a'), '0').configurable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('a'), 'length').value"),
        "1"
    );
    // §10.4.3.3 — a define is refused unless it describes exactly the property already there, and
    // one that does is allowed and stores nothing.
    assert_eq!(
        run(
            "(function () { try { Object.defineProperty(new String('a'), '0', {value: 'z'}); \
             return 'allowed'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run("(function () { var s = new String('a'); \
             Object.defineProperty(s, '0', {value: 'a'}); return s[0]; })()"),
        "a"
    );
    // §10.4.3.3 refuses every *other* way of describing it, and each refusal is a different step
    // of §10.1.6.3 rather than one rule stated five times.
    assert_eq!(
        run(&refused("{get: function () { return 1; }}")),
        "TypeError"
    );
    assert_eq!(run(&refused("{set: function (v) {}}")), "TypeError");
    assert_eq!(run(&refused("{writable: true}")), "TypeError");
    assert_eq!(run(&refused("{enumerable: false}")), "TypeError");
    assert_eq!(run(&refused("{configurable: true}")), "TypeError");
    // …while a descriptor that asks for nothing, or for exactly what is there, is allowed.
    assert_eq!(run(&refused("{}")), "allowed");
    assert_eq!(
        run(&refused(
            "{enumerable: true, writable: false, configurable: false, value: 'a'}"
        )),
        "allowed"
    );
    // §10.4.3.4 step 5 — `length` is fixed in every way a property can be, so an assignment does
    // nothing and a delete refuses. Its being non-enumerable is why `Object.keys` above stops at
    // the characters.
    assert_eq!(
        run("(function () { var s = new String('ab'); s.length = 9; return s.length; })()"),
        "2"
    );
    assert_eq!(
        run("(function () { var s = new String('ab'); return delete s.length; })()"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('ab'), 'length').writable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('ab'), 'length').configurable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new String('ab'), 'length').enumerable"),
        "false"
    );
    // Only the characters are held back. An ordinary property put on a String object deletes as
    // any other would, and so does one that was never there.
    assert_eq!(
        run("(function () { var s = new String('ab'); s.foo = 1; return delete s.foo; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var s = new String('ab'); return delete s.missing; })()"),
        "true"
    );
    // Past the last character there is nothing exotic left: an ordinary property, stored, and it
    // sorts after the characters because every index it can have is larger than theirs.
    assert_eq!(
        run("(function () { var s = new String('ab'); s[5] = 'q'; return s[5]; })()"),
        "q"
    );
    assert_eq!(
        run(
            "(function () { var s = new String('ab'); s[5] = 'q'; return Object.keys(s).join(','); })()"
        ),
        "0,1,5"
    );
}

#[test]
fn a_position_outside_the_string_has_a_different_answer_for_each_method() {
    // The four disagree, and each is what its own clause asks for. A reader who assumed they were
    // the same operation with different names would have written three of these wrongly.
    assert_eq!(run("'abc'.charAt(9) === ''"), "true");
    assert_eq!(run("'abc'.charAt(-1) === ''"), "true");
    assert_eq!(run("typeof 'abc'[9]"), "undefined");
    assert_eq!(run("'abc'.charCodeAt(9)"), "NaN");
    assert_eq!(run("'abc'.indexOf('z')"), "-1");
    // §7.1.5 `ToIntegerOrInfinity` — NaN is zero, and a fraction truncates towards zero.
    assert_eq!(run("'abc'.charAt(NaN)"), "a");
    assert_eq!(run("'abc'.charAt()"), "a");
    assert_eq!(run("'abc'.charAt(1.9)"), "b");
    assert_eq!(run("'abc'.charAt(Infinity) === ''"), "true");
}

#[test]
fn searching_agrees_with_the_two_rules_that_look_alike_and_are_not() {
    assert_eq!(run("'abcabc'.indexOf('b')"), "1");
    assert_eq!(run("'abcabc'.lastIndexOf('b')"), "4");
    // An empty needle is found at once, at the clamped position — which is why the answer changes
    // with a position past the end rather than staying at zero.
    assert_eq!(run("'abc'.indexOf('')"), "0");
    assert_eq!(run("'abc'.indexOf('', 10)"), "3");
    assert_eq!(run("''.indexOf('')"), "0");
    // §22.1.3.10 step 5 — `lastIndexOf` tests its position for NaN *before* truncating, and NaN
    // there means the end of the string. `indexOf` has no such step, so the same argument means
    // zero to it. One row each, because this is the only place the two differ in kind.
    assert_eq!(run("'aa'.lastIndexOf('a', undefined)"), "1");
    assert_eq!(run("'aa'.indexOf('a', undefined)"), "0");
    assert_eq!(run("'aXbXc'.lastIndexOf('X', 2)"), "1");
    assert_eq!(run("'abc'.indexOf('c', -5)"), "2");
    assert_eq!(run("'abc'.indexOf('c', Infinity)"), "-1");
    assert_eq!(run("'abc'.lastIndexOf('a', -1)"), "0");
    assert_eq!(run("'abc'.lastIndexOf('c', Infinity)"), "2");
}

#[test]
fn slice_counts_from_the_end_and_substring_swaps_instead() {
    assert_eq!(run("'abcd'.slice(1, 3)"), "bc");
    assert_eq!(run("'abcd'.slice(-2)"), "cd");
    assert_eq!(run("'abcd'.slice(-10, 10)"), "abcd");
    assert_eq!(run("'abcd'.slice(NaN, Infinity)"), "abcd");
    assert_eq!(run("'abc'.slice()"), "abc");
    // The one difference worth its own pair of rows: a backwards range is empty to `slice` and
    // reversed by `substring`, which swaps its two arguments outright (§22.1.3.24 step 7).
    assert_eq!(run("'abcd'.slice(3, 1) === ''"), "true");
    assert_eq!(run("'abcd'.substring(3, 1)"), "bc");
    // …and `substring` clamps a negative to zero rather than counting back from the end.
    assert_eq!(run("'abcd'.substring(-1, 99)"), "abcd");
    assert_eq!(run("'abcd'.substring(NaN)"), "abcd");
    assert_eq!(run("''.slice(0, 5) === ''"), "true");
    assert_eq!(run("'ab'.concat('cd', 1)"), "abcd1");
    assert_eq!(run("'ab'.concat()"), "ab");
}

#[test]
fn a_receiver_is_converted_except_by_the_two_methods_that_report_what_it_is() {
    // §22.1.3 — `ToString` of whatever it was given, which is why a Number receiver works and
    // reads as a mistake. It is the specification; see the module comment.
    assert_eq!(run("String.prototype.charAt.call(42, 0)"), "4");
    // `RequireObjectCoercible` first, so these two are refused before anything is converted.
    assert_eq!(
        run(
            "(function () { try { return 'abc'.indexOf.call(null, 'a'); } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // `thisStringValue` instead, for the two whose whole job is to say what the receiver is: a
    // Number is refused rather than converted, or there would be no way left to ask.
    assert_eq!(run("'abc'.valueOf()"), "abc");
    assert_eq!(run("new String('q').valueOf()"), "q");
    assert_eq!(run("typeof String.prototype.toString.call('a')"), "string");
    assert_eq!(
        run(
            "(function () { try { return String.prototype.valueOf.call(42); } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}

/// A define of `new String("a")`'s only character, and what came of it.
///
/// The five refusals differ in one token, so they are written as one program with a hole in it —
/// five near-identical blocks would hide which token each row is actually about.
fn refused(descriptor: &str) -> String {
    format!(
        "(function () {{ var s = new String('a');          try {{ Object.defineProperty(s, '0', {descriptor}); return 'allowed'; }}          catch (e) {{ return e.constructor.name; }} }})()"
    )
}
