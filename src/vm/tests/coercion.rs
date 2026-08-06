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
fn a_conversion_may_nest_to_the_cap_and_one_past_it_is_refused() {
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
    assert_eq!(run(&nest(32)), "end");
    assert_eq!(run(&nest(33)), "RangeError");
}

#[test]
fn a_conversion_at_the_cap_fits_in_the_stack_it_claims_to_need() {
    // The twin of the parser's `parsing_at_the_cap_fits_in_the_stack_it_claims_to_need`, for the
    // other place in this engine that spends a Rust frame per level of *input*. A cap the stack
    // cannot afford is worse than no cap: the conversion dies by overflow one level before the
    // check that exists to prevent exactly that, and DR-0002 says nothing can rescue it.
    //
    // One mebibyte is the smallest thread stack in common use, and this is a debug build, whose
    // frames are largest. The cap was 200 and had never been measured against a stack at all;
    // this is what would have said so.
    //
    // It has now said so twice. The second time the number was 64 and the margin was 1.3×, measured
    // on Windows and recorded as thin — and macOS CI, whose frames are larger, aborted on the next
    // push with no panic and the output cut off mid-run. A margin that only one platform can afford
    // is not a margin, which is why 32 is what this asks for now.
    //
    // **And it measured the wrong shape until 2026-08-06.** A `toString` chain is the *cheapest*
    // way to re-enter, and the cap has to hold for the dearest: `lab`'s `reentry-cost` bisects the
    // cliff at 43 levels for `toString`, 38 for `map` and **35 for `sort`**, which carries a `Vec`
    // of elements across the comparator call. So the guard passed with room to spare while the real
    // margin was 1.09×, and a slice that fattened the frame by a tenth would have been found by CI
    // aborting rather than by this.
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            // The cap as a literal rather than through the constant, so that raising the
            // constant without re-measuring makes this fail rather than quietly follow it up.
            let deep = "var d = 0; function f() { d = d + 1; if (d >= 32) return 'end'; \
                        var out = ''; [1, 2].sort(function () { out = f(); return 0; }); \
                        return out; } f()";
            run(deep)
        })
        .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
    assert_eq!(
        worker.join().unwrap_or_default(), // a panic in the thread is the failure being reported
        "end",
        "re-entering at the cap through the fattest native needs more than the mebibyte it claims"
    );
}

#[test]
fn the_cheapest_way_to_re_enter_is_not_what_the_cap_has_to_afford() {
    // The row above measures `sort` because it is the dearest. This one measures a conversion at
    // the same depth, and exists to keep the *pair* honest: if some slice ever makes the
    // conversion path the fattest, the two swap places and the guard above should follow. Both
    // passing says nothing on its own; the point is that the guard tracks whichever is worse, and
    // the only way to notice a change is to have measured both.
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let deep = "var d = 0; function make() { d = d + 1; \
                        return {toString: function () { return d < 32 ? '' + make() : 'end' }}; } \
                        '' + make()";
            run(deep)
        })
        .unwrap_or_else(|err| panic!("could not spawn the measuring thread: {err}")); // without the thread there is no measurement
    assert_eq!(
        worker.join().unwrap_or_default(), // a panic in the thread is the failure being reported
        "end",
        "converting at the cap needs more than the mebibyte it claims"
    );
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

#[test]
fn an_exotic_to_primitive_is_asked_before_either_ordinary_method() {
    // §7.1.1 step 1.a — `@@toPrimitive` comes **first**, so neither `valueOf` nor `toString` is
    // reached at all when there is one. That is what makes it an override rather than a preference.
    assert_eq!(
        run("String({toString(){return 'no'}, valueOf(){return 'no'}, \
             [Symbol.toPrimitive](){return 'yes'}})"),
        "yes"
    );
    assert_eq!(
        run("'' + {toString(){throw new Error('reached')}, [Symbol.toPrimitive](){return 'ok'}}"),
        "ok"
    );
    // Step 1.b.vi — an Object is not an answer, and unlike §7.1.1.1's walk there is **nothing else
    // to try**: the object said how it wished to convert and did not. A fallback to `valueOf`
    // would be a second chance the clause does not give, and this row is the difference.
    // Asserted by **message**: without the guard the object is handed back as the primitive and
    // whatever reads it next throws a TypeError of its own, so `e.name` cannot tell the clause
    // being implemented from the clause being skipped.
    assert_eq!(
        run(
            "try { ({valueOf(){return 1}, [Symbol.toPrimitive](){return {}}}) + ''; 'no throw' } \
             catch (e) { e.message }"
        ),
        "Symbol.toPrimitive did not answer with a primitive value"
    );
    // `GetMethod`, so `undefined` and `null` both mean "there is none" and the ordinary walk runs,
    // while anything else that is not callable is a TypeError rather than something to walk past.
    assert_eq!(
        run("'' + {[Symbol.toPrimitive]: undefined, toString(){return 'ord'}}"),
        "ord"
    );
    assert_eq!(
        run("'' + {[Symbol.toPrimitive]: null, toString(){return 'ord'}}"),
        "ord"
    );
    // By **message**: calling a 1 throws a TypeError of its own, so the name is the same whether
    // `GetMethod`'s callable check is there or not, and only the message tells them apart.
    assert_eq!(
        run("try { ({[Symbol.toPrimitive]: 1}) + ''; 'no throw' } catch (e) { e.message }"),
        "Symbol.toPrimitive is not a function"
    );
}

#[test]
fn the_hint_has_three_values_and_the_third_is_only_visible_to_such_a_method() {
    // §7.1.1 steps 1.b.i to 1.b.iii — the preference reaches the method as a String, and this is
    // the only place in the language the three are named rather than implied.
    let echo = "var o = {[Symbol.toPrimitive](hint){return hint}};";
    // `+` and `==` ask with **no** preference — §13.15.3 step 1.a and §7.2.15 step 10.
    assert_eq!(run(&format!("{echo} o + ''")), "default");
    assert_eq!(run(&format!("{echo} '' + o")), "default");
    assert_eq!(run(&format!("{echo} o == 'default'")), "true");
    // Everything that wants a number asks for one, which is `ToNumeric` reaching §7.1.1.
    assert_eq!(run(&format!("{echo} o - 0")), "NaN");
    assert_eq!(run(&format!("{echo} String(o * 1)")), "NaN");
    assert_eq!(
        run("var o = {[Symbol.toPrimitive](h){return h === 'number' ? 7 : 0}}; +o"),
        "7"
    );
    // …and everything that wants text asks for a string.
    assert_eq!(run(&format!("{echo} String(o)")), "string");
    assert_eq!(run(&format!("{echo} `${{o}}`")), "string");
    assert_eq!(run(&format!("{echo} ({{}})[o] = 1, String(o)")), "string");
    // The row that makes `Default` a third value rather than a spelling of `Number`: an object
    // with no such method cannot tell them apart, because §7.1.1 step 1.c makes an absent
    // preference number before `OrdinaryToPrimitive` is reached.
    assert_eq!(
        run("var o = {valueOf(){return 1}, toString(){return 'two'}}; (o + '') + ',' + (o - 0)"),
        "1,1"
    );
}

#[test]
fn a_date_reads_the_absent_preference_as_a_string_and_nothing_else_does() {
    // §21.4.4.45, and the whole reason the third hint has to exist. `+` asks with no preference
    // and a Date answers with its text; every other arithmetic operator asks for a number and the
    // same Date answers with its time value.
    assert_eq!(run("typeof (new Date(0) + 1)"), "string");
    assert_eq!(run("typeof (new Date(0) - 0)"), "number");
    assert_eq!(run("typeof (new Date(0) * 1)"), "number");
    assert_eq!(run("new Date(0) + '' === String(new Date(0))"), "true");
    assert_eq!(run("new Date(0) - 0"), "0");
    // …and it is a method a script can reach, move and refuse. Three named hints and no fallback.
    assert_eq!(run("typeof Date.prototype[Symbol.toPrimitive]"), "function");
    assert_eq!(run("Date.prototype[Symbol.toPrimitive].length"), "1");
    assert_eq!(
        run("Date.prototype[Symbol.toPrimitive].call(new Date(0), 'number')"),
        "0"
    );
    assert_eq!(
        run(
            "try { Date.prototype[Symbol.toPrimitive].call(new Date(0), 'nope'); 'no throw' } \
             catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Date.prototype[Symbol.toPrimitive].call(new Date(0)); 'no throw' } \
             catch (e) { e.name }"
        ),
        "TypeError"
    );
    // Step 2 wants an Object and any object: it reads no `[[DateValue]]` of its own, so it works
    // on whatever a script points it at — which is what makes it inheritable rather than branded.
    assert_eq!(
        run("Date.prototype[Symbol.toPrimitive].call({toString(){return 'x'}}, 'default')"),
        "x"
    );
    assert_eq!(
        run(
            "try { Date.prototype[Symbol.toPrimitive].call(1, 'default'); 'no throw' } \
             catch (e) { e.name }"
        ),
        "TypeError"
    );
    // §21.4.4.45 writes its own attributes rather than taking §17's: not writable, not enumerable
    // and **configurable** — so a script may delete it and get the ordinary walk back, which is
    // the only row that shows the method is what decides the answer.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Date.prototype, Symbol.toPrimitive); \
             '' + d.writable + d.enumerable + d.configurable"
        ),
        "falsefalsetrue"
    );
    assert_eq!(
        run("delete Date.prototype[Symbol.toPrimitive]; typeof (new Date(0) + 1)"),
        "number"
    );
}

#[test]
fn a_symbol_wrapper_answers_with_the_symbol_it_wraps() {
    // §20.4.3.5 — the one `@@toPrimitive` that ignores its hint, because a Symbol has no other
    // primitive to become. It is what lets a wrapper compare equal to what it wraps, where
    // §20.4.3.3's `toString` would have refused the coercion outright.
    assert_eq!(run("var s = Symbol('q'); Object(s) == s"), "true");
    assert_eq!(
        run("typeof Symbol.prototype[Symbol.toPrimitive]"),
        "function"
    );
    assert_eq!(
        run("var s = Symbol('q'); typeof Symbol.prototype[Symbol.toPrimitive].call(s)"),
        "symbol"
    );
    // …and the refusal still happens, one step later: the addition gets a Symbol and will not add
    // it, rather than `toString` refusing to produce text.
    assert_eq!(
        run("try { Object(Symbol()) + ''; 'no throw' } catch (e) { e.name }"),
        "TypeError"
    );
    // §20.4.3.5's attributes, which are §21.4.4.45's: not writable, not enumerable, configurable.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Symbol.prototype, Symbol.toPrimitive); \
             '' + d.writable + d.enumerable + d.configurable"
        ),
        "falsefalsetrue"
    );
    // What the method actually decides, and it is **not** the equality above: §20.4.3.4's
    // `valueOf` already answers with the Symbol, so the number path agrees either way. The String
    // path is where they part — without this method a string hint reaches §20.4.3.3's `toString`,
    // which describes the Symbol instead of refusing, and a wrapper silently becomes text.
    assert_eq!(
        run("try { String(Object(Symbol('q'))); 'no throw' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("delete Symbol.prototype[Symbol.toPrimitive]; String(Object(Symbol('q')))"),
        "Symbol(q)"
    );
}
