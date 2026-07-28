//! §7.1.1 as a script sees it — what an operator does when an operand is an object.

use super::*;

#[test]
fn an_object_is_asked_and_the_hint_decides_which_method_first() {
    // §7.1.1.1. The order is the whole of what a hint does, and it is observable through the side
    // effects of the two methods rather than only through the answer.
    let both = "var log = ''; var o = {valueOf: function () { log += 'v'; return 1 }, \
                toString: function () { log += 't'; return 's' }}; ";
    // An arithmetic operator wants a Number, so `valueOf` is tried first and answers.
    assert_eq!(run(&format!("{both} o * 2")), "2");
    assert_eq!(run(&format!("{both} o * 2; log")), "v");
    // A property key wants a String, so `toString` is tried first (§7.1.19).
    assert_eq!(run(&format!("{both} var t = {{}}; t[o] = 1; log")), "t");
    // `+` uses the default hint, which §7.1.1 step 3 turns into Number — so `valueOf` again,
    // and that is why `({valueOf: () => 1}) + '' ` is `'1'` and not `'s'`.
    assert_eq!(run(&format!("{both} o + ''")), "1");
}

#[test]
fn an_object_answer_is_not_an_answer_and_the_other_method_is_tried() {
    // §7.1.1.1 step 3.b.iii. The commonest object in the language reaches this: `{}` has
    // `Object.prototype.valueOf`, which answers with the object itself — so every plain object
    // converts through `toString` even though `valueOf` was tried first.
    assert_eq!(run("({}) + 1"), "[object Object]1");
    assert_eq!(
        run(
            "var o = {valueOf: function () { return {} }, toString: function () { return 't' }}; o + ''"
        ),
        "t"
    );
    // It does *not* recurse into the object that was answered with: `toString` is tried, not the
    // inner `valueOf`.
    assert_eq!(
        run("var o = {valueOf: function () { return {valueOf: function () { return 1 }} }}; o + 1"),
        "[object Object]1"
    );
    // §7.1.1.1 step 3.b.i asks `IsCallable` and skips what is not, rather than throwing — so a
    // `valueOf` that is a number is passed over in silence.
    assert_eq!(
        run("var o = {valueOf: 1, toString: function () { return 's' }}; o + ''"),
        "s"
    );
}

#[test]
fn an_object_with_nothing_to_ask_is_the_one_that_cannot_be_converted() {
    // §7.1.1.1 step 4. `Object.create(null)` has no prototype, so neither method is anywhere —
    // which is the only ordinary way in the language to have no primitive at all.
    assert_eq!(
        run("try { Object.create(null) + 1 } catch (e) { e.name }"),
        "TypeError"
    );
    // …and so is an object whose two methods both answer with objects.
    assert_eq!(
        run(
            "var o = {valueOf: function () { return {} }, toString: function () { return {} }}; try { o + 1 } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_throw_inside_a_conversion_arrives_as_the_value_that_was_thrown() {
    // The whole reason an abrupt completion carries a value. The `catch` must receive the object
    // the `throw` created — its kind, its message, its identity — and not an error rebuilt from
    // whatever survived the trip back through Rust.
    assert_eq!(
        run(
            "var o = {valueOf: function () { throw new RangeError('boom') }}; try { o + 1 } catch (e) { e.name + ':' + e.message }"
        ),
        "RangeError:boom"
    );
    // Anything at all may be thrown, and a conversion passes it on unchanged.
    assert_eq!(
        run(
            "var o = {valueOf: function () { throw 42 }}; try { o + 1 } catch (e) { typeof e + ':' + e }"
        ),
        "number:42"
    );
    // Identity, which is the part a rebuilt error would lose.
    assert_eq!(
        run(
            "var thrown = new TypeError('x'); var o = {valueOf: function () { throw thrown }}; try { o + 1 } catch (e) { e === thrown }"
        ),
        "true"
    );
}

#[test]
fn the_caller_keeps_its_scope_when_a_conversion_throws() {
    // A `return` restores the environment from the frame it pops. A throw that nothing caught
    // does not pop frames one at a time, so without putting it back by hand the caller carries on
    // running in the *callee's* scope — and the next variable it reads is a slot that is not
    // there. This row is why that is written down rather than assumed.
    assert_eq!(
        run(
            "var kept = 7; var o = {valueOf: function () { var mine = 1; throw 0 }}; try { o + 1 } catch (e) { kept }"
        ),
        "7"
    );
    assert_eq!(
        run(
            "function f() { var local = 3; var o = {valueOf: function () { throw 0 }}; try { o + 1 } catch (e) {} return local } f()"
        ),
        "3"
    );
}

#[test]
fn loose_equality_converts_exactly_what_7_2_15_says_and_nothing_else() {
    // Two objects are compared by identity. Converting both would make every pair of plain
    // objects equal, which is the mistake this row exists to catch.
    assert_eq!(run("({}) == ({})"), "false");
    assert_eq!(run("var o = {}; o == o"), "true");
    // §7.2.15's list of what an object may be converted against is exact, and `null` and
    // `undefined` are not on it. So this does not ask the object anything — which is why
    // `x == null` stays safe even when `x` has a `valueOf` that throws.
    assert_eq!(
        run(
            "var o = {valueOf: function () { throw new Error('never') }}; (o == null) + '|' + (o == undefined)"
        ),
        "false|false"
    );
    // String, Number and Boolean are on it. A Boolean becomes a Number first and the comparison
    // starts again, which is why the object is still asked.
    assert_eq!(
        run(
            "var o = {valueOf: function () { return 1 }}; (o == 1) + '|' + (o == '1') + '|' + (o == true)"
        ),
        "true|true|true"
    );
    // Strict equality converts nothing at all: it compares types, and a conversion would erase
    // the difference it exists to report.
    assert_eq!(
        run("var o = {valueOf: function () { return 1 }}; (o === 1) + '|' + (o !== 1)"),
        "false|true"
    );
}

#[test]
fn each_operand_is_asked_once_and_the_left_one_first() {
    // §13.15.3 evaluates `ToPrimitive(left)` and then `ToPrimitive(right)`, and a `valueOf` with
    // a side effect is how a program can tell. Asking twice, or in the other order, is a bug that
    // only shows up in code that logs.
    assert_eq!(
        run(
            "var log = ''; var a = {valueOf: function () { log += 'a'; return 1 }}; var b = {valueOf: function () { log += 'b'; return 2 }}; a + b; log"
        ),
        "ab"
    );
    assert_eq!(
        run("var n = 0; var o = {valueOf: function () { n = n + 1; return 1 }}; o + o; n"),
        "2"
    );
    // A relational operator converts both as well — §7.2.13, with the Number hint.
    assert_eq!(
        run(
            "var a = {valueOf: function () { return 1 }}; var b = {valueOf: function () { return 2 }}; (a < b) + '|' + (a > b)"
        ),
        "true|false"
    );
}

#[test]
fn a_unary_operator_asks_only_when_it_wants_a_number() {
    assert_eq!(run("-({valueOf: function () { return 2 }})"), "-2");
    assert_eq!(run("+({valueOf: function () { return 2 }})"), "2");
    assert_eq!(run("~({valueOf: function () { return 0 }})"), "-1");
    // `typeof` asks what a value *is* and `!` asks whether it is truthy. Neither converts, and
    // neither can throw — which is why they answer for an object that has no primitive at all.
    assert_eq!(run("typeof Object.create(null)"), "object");
    assert_eq!(run("!Object.create(null)"), "false");
    assert_eq!(
        run("var o = {valueOf: function () { throw new Error('never') }}; typeof o + '|' + !o"),
        "object|false"
    );
}

#[test]
fn a_property_key_is_converted_the_same_way_a_value_is() {
    // §7.1.19 `ToPropertyKey` goes through `ToPrimitive` with the String hint, so an object used
    // as a key is asked for its text — and `o[{}]` really does file under `"[object Object]"`.
    assert_eq!(
        run("var t = {}; var k = {toString: function () { return 'k' }}; t[k] = 5; t.k"),
        "5"
    );
    assert_eq!(run("var t = {}; t[{}] = 5; t['[object Object]']"), "5");
    assert_eq!(
        run("var t = {a: 1}; var k = {toString: function () { return 'a' }}; t[k]"),
        "1"
    );
}

#[test]
fn a_conversion_may_nest_two_hundred_deep_and_the_two_hundred_and_first_is_refused() {
    // Each nesting is a real Rust frame, because the answer is needed in the middle of an
    // instruction — so the limit is far below the one on JavaScript calls, and it is a catchable
    // RangeError rather than a crash. DR-0002 is about *any* input, including this one.
    //
    // Both sides of the boundary, because a limit tested only from above is a limit whose value
    // nobody checked: an off-by-one would pass a test that only asked whether five thousand is
    // too many.
    let nest = |depth: u32| {
        format!(
            "var d = 0; function make() {{ d = d + 1; return {{toString: function () {{              return d < {depth} ? '' + make() : 'end' }}}}; }}              try {{ '' + make() }} catch (e) {{ e.name }}"
        )
    };
    assert_eq!(run(&nest(200)), "end");
    assert_eq!(run(&nest(201)), "RangeError");
}

#[test]
fn a_conversion_that_finished_gives_its_depth_back() {
    // The counter comes down as well as up. If it did not, a script would convert two hundred
    // objects one after another — nesting nothing at all — and then start refusing.
    let many = "var o = {valueOf: function () { return 1 }}; var total = 0;                 for (var i = 0; i < 500; i = i + 1) { total = total + (o + 0); } total";
    assert_eq!(run(many), "500");
    // …and the same for a conversion that ended by throwing, which returns by a different path.
    let throwing = "var bad = {valueOf: function () { throw 0 }}; var good = {valueOf: function () { return 1 }};                     for (var i = 0; i < 500; i = i + 1) { try { bad + 0 } catch (e) {} } good + 1";
    assert_eq!(run(throwing), "2");
}

#[test]
fn an_error_prints_itself_now_that_something_can_be_called() {
    // The seam this slice closed. `Error.prototype.toString` existed and nothing could reach it,
    // so an error was a value a program could catch and not describe.
    assert_eq!(run("'' + new Error('m')"), "Error: m");
    assert_eq!(run("'' + new TypeError('m')"), "TypeError: m");
    assert_eq!(
        run("try { null.x } catch (e) { '' + e }"),
        "TypeError: cannot read a property of something that is not an object"
    );
    assert_eq!(
        run("try { nowhere } catch (e) { '' + e }"),
        "ReferenceError: nowhere is not defined"
    );
}
