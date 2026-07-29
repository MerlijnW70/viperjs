//! The rest of §23.1.3 — folding, quantifying, searching, and moving elements about.

use super::*;

#[test]
fn a_fold_with_no_initial_value_takes_the_first_element_that_is_there() {
    // §23.1.3.24 step 6. A leading hole is not it, and an array with nothing present at all is
    // empty as far as a fold is concerned — step 7's TypeError being the only answer left.
    assert_eq!(
        run("[1, 2, 3].reduce(function (t, x) { return t + x })"),
        "6"
    );
    assert_eq!(
        run("[1, 2, 3].reduce(function (t, x) { return t + x }, 10)"),
        "16"
    );
    assert_eq!(run("[5].reduce(function (t, x) { return t + x })"), "5");
    assert_eq!(run("[].reduce(function (t, x) { return t + x }, 5)"), "5");
    assert_eq!(
        run("try { [].reduce(function (t, x) { return t }) } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { [, ,].reduce(function (t, x) { return t }) } catch (e) { e.name }"),
        "TypeError"
    );
    // …and an *initial value* of `undefined` is an initial value, so the same array folds fine.
    assert_eq!(
        run("typeof [, ,].reduce(function (t, x) { return t }, undefined)"),
        "undefined"
    );
}

#[test]
fn reduce_right_reverses_the_walk_and_not_the_arguments() {
    // The accumulator stays first. Getting this backwards would be invisible for `+` on numbers
    // and wrong for everything else, which is why the row uses strings.
    assert_eq!(
        run("[1, 2, 3].reduceRight(function (t, x) { return '' + t + x })"),
        "321"
    );
    assert_eq!(
        run("['a', 'b'].reduceRight(function (t, x) { return t + x }, 'i')"),
        "iba"
    );
    // §23.1.3.24 step 8.c.ii — no receiver at all. `reduce` is the one callback method with no
    // `thisArg`, so `this` inside it is what a plain call gets.
    assert_eq!(
        run("[1].reduce(function () { return this === globalThis }, 0)"),
        "true"
    );
}

#[test]
fn every_and_some_are_one_walk_with_two_answers_and_they_disagree_about_empty() {
    assert_eq!(
        run("[1, 2, 3].every(function (x) { return x > 0 })"),
        "true"
    );
    assert_eq!(
        run("[1, 2, 3].every(function (x) { return x > 1 })"),
        "false"
    );
    assert_eq!(run("[1, 2, 3].some(function (x) { return x > 2 })"), "true");
    assert_eq!(
        run("[1, 2, 3].some(function (x) { return x > 9 })"),
        "false"
    );
    // Vacuously true and vacuously false, which is the pair a program relying on either had
    // better expect.
    assert_eq!(run("[].every(function () { return false })"), "true");
    assert_eq!(run("[].some(function () { return true })"), "false");
    // Both stop as soon as the answer is decided.
    assert_eq!(
        run("var n = 0; [1, 2, 3].every(function () { n = n + 1; return false }); n"),
        "1"
    );
    assert_eq!(
        run("var n = 0; [1, 2, 3].some(function () { n = n + 1; return true }); n"),
        "1"
    );
    // Both skip a hole, and both take a receiver.
    assert_eq!(
        run("var n = 0; [1, , 3].every(function () { n = n + 1; return true }); n"),
        "2"
    );
    assert_eq!(
        run("[1].every(function () { return this.x }, {x: true})"),
        "true"
    );
}

#[test]
fn the_find_family_visits_a_hole_where_everything_older_skips_one() {
    // The generational difference. `find` and its three relatives were added long after the
    // others and deliberately read every index, so a hole is handed to the callback as
    // `undefined` rather than passed over.
    assert_eq!(
        run("var n = 0; [1, , 3].find(function () { n = n + 1; return false }); n"),
        "3"
    );
    assert_eq!(
        run("[, 1].findIndex(function (x) { return x === undefined })"),
        "0"
    );
    assert_eq!(
        run("[, 1].some(function (x) { return x === undefined })"),
        "false"
    );
    // The element or the index, from either end.
    assert_eq!(run("[1, 2, 3].find(function (x) { return x > 1 })"), "2");
    assert_eq!(
        run("[1, 2, 3].findIndex(function (x) { return x > 1 })"),
        "1"
    );
    assert_eq!(
        run("[1, 2, 3].findLast(function (x) { return x < 3 })"),
        "2"
    );
    assert_eq!(
        run("[1, 2, 3].findLastIndex(function (x) { return x < 3 })"),
        "1"
    );
    // Nothing found says so in two shapes, and only one of them is safe to test: `-1` is an
    // index that cannot exist, and `undefined` is an element that can.
    assert_eq!(
        run("typeof [1].find(function () { return false })"),
        "undefined"
    );
    assert_eq!(run("[1].findIndex(function () { return false })"), "-1");
}

#[test]
fn includes_matches_a_nan_and_a_hole_where_index_of_matches_neither() {
    // The whole reason it exists rather than `indexOf(x) !== -1`. §7.2.11's `SameValueZero`
    // differs from strict equality in exactly one place — NaN — and from `SameValue` in exactly
    // one other, the signed zeroes.
    assert_eq!(
        run("[NaN].includes(NaN) + '|' + [NaN].indexOf(NaN)"),
        "true|-1"
    );
    assert_eq!(
        run("[0].includes(-0) + '|' + [-0].includes(0)"),
        "true|true"
    );
    // …and it reads a hole as `undefined` rather than skipping it.
    assert_eq!(
        run("[, 1].includes(undefined) + '|' + [, 1].indexOf(undefined)"),
        "true|-1"
    );
    assert_eq!(
        run("[1, 2].includes(2, 1) + '|' + [1, 2].includes(1, 1)"),
        "true|false"
    );
}

#[test]
fn last_index_of_reads_the_same_two_rules_from_the_other_end() {
    assert_eq!(run("[1, 2, 1].lastIndexOf(1)"), "2");
    assert_eq!(run("[1, 2, 1].lastIndexOf(1, 1)"), "0");
    assert_eq!(run("[1, 2, 1].lastIndexOf(1, -2)"), "0");
    assert_eq!(run("[1, 2, 1].lastIndexOf(1, -99)"), "-1");
    assert_eq!(run("[1, 2, 1].lastIndexOf(1, 99)"), "2");
    assert_eq!(run("[1, 2].lastIndexOf(9)"), "-1");
    // Strict equality and a skipped hole, the same as `indexOf`.
    assert_eq!(run("[1].lastIndexOf('1')"), "-1");
    assert_eq!(run("[, 1].lastIndexOf(undefined)"), "-1");
    // §23.1.3.19 step 5 — the *default* start is the last index, not the length, so an absent
    // second argument and one of `length` mean the same thing.
    assert_eq!(
        run("[1, 2].lastIndexOf(2) + '|' + [1, 2].lastIndexOf(2, 2)"),
        "1|1"
    );
}

#[test]
fn shift_and_unshift_move_everything_and_keep_the_holes_where_they_are() {
    assert_eq!(
        run("var a = [1, 2, 3]; a.shift() + '|' + a.join(',')"),
        "1|2,3"
    );
    assert_eq!(
        run("var a = [2, 3]; a.unshift(1) + '|' + a.join(',')"),
        "3|1,2,3"
    );
    assert_eq!(run("[].shift() + '|' + [].length"), "undefined|0");
    assert_eq!(run("var a = []; a.unshift(1, 2); a.join(',')"), "1,2");
    assert_eq!(run("var a = [1]; a.unshift(); a.length"), "1");
    // A hole travels with its position rather than being filled in, which is the difference a
    // `Delete` makes over a `Set` of `undefined`.
    assert_eq!(
        run("var a = [1, , 3]; a.shift(); a.length + '|' + (0 in a) + '|' + (1 in a)"),
        "2|false|true"
    );
    assert_eq!(
        run("var a = [, 1]; a.unshift(9); a.length + '|' + (1 in a) + '|' + (2 in a)"),
        "3|false|true"
    );
}

#[test]
fn reverse_swaps_in_place_and_a_hole_swaps_with_it() {
    assert_eq!(run("[1, 2, 3].reverse().join(',')"), "3,2,1");
    assert_eq!(run("[1, 2].reverse().join(',')"), "2,1");
    assert_eq!(run("[1].reverse().join(',')"), "1");
    // In place, and it answers the array it was given rather than a copy.
    assert_eq!(run("var a = [1, 2]; a.reverse() === a"), "true");
    // §23.1.3.24 steps 6.f to 6.i are four cases rather than a swap, because a hole on one side
    // has to *become* a hole on the other.
    assert_eq!(
        run("var a = [1, , 3]; a.reverse(); (0 in a) + '|' + (1 in a) + '|' + (2 in a)"),
        "true|false|true"
    );
    assert_eq!(
        run("var a = [, 1]; a.reverse(); (0 in a) + '|' + (1 in a) + '|' + a[0]"),
        "true|false|1"
    );
}

#[test]
fn splice_removes_and_inserts_at_once_and_answers_what_it_took() {
    assert_eq!(
        run("var a = [1, 2, 3, 4]; a.splice(1, 2).join(',') + '|' + a.join(',')"),
        "2,3|1,4"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, 1, 'x', 'y').join(',') + '|' + a.join(',')"),
        "2|1,x,y,3"
    );
    // One argument means "everything from there", which is not what a second argument of
    // `undefined` means — that is `ToIntegerOrInfinity` of NaN, which is zero.
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1).join(',') + '|' + a.length"),
        "2,3|1"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, undefined).length + '|' + a.length"),
        "0|3"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice().length + '|' + a.length"),
        "0|3"
    );
    // A negative start counts from the end, and a count past the end is clamped.
    assert_eq!(run("var a = [1, 2, 3]; a.splice(-1).join(',')"), "3");
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, 99).join(',') + '|' + a.length"),
        "2,3|1"
    );
    assert_eq!(
        run("var a = [1, 2]; a.splice(9, 1).length + '|' + a.length"),
        "0|2"
    );
    // Inserting more than was removed grows the array, and the result is a real Array.
    assert_eq!(
        run("var a = [1, 2]; a.splice(1, 0, 'x'); a.join(',')"),
        "1,x,2"
    );
    assert_eq!(run("Array.isArray([1].splice(0))"), "true");
}

#[test]
fn concat_spreads_an_array_one_level_and_appends_everything_else_whole() {
    assert_eq!(run("[1, 2].concat(3, [4, 5]).join(',')"), "1,2,3,4,5");
    assert_eq!(run("[].concat([]).length"), "0");
    // One level: an array inside an array stays one, which is why `flat` had to be invented.
    assert_eq!(
        run("[1].concat([[2]]).length + '|' + Array.isArray([1].concat([[2]])[1])"),
        "2|true"
    );
    // An object that is not an Array is appended whole even when it looks like one.
    assert_eq!(run("[1].concat({length: 2, 0: 'a'}).length"), "2");
    // A hole in a spread source stays a hole.
    assert_eq!(
        run("var r = [1].concat([, 2]); r.length + '|' + (1 in r) + '|' + (2 in r)"),
        "3|false|true"
    );
    // …and the answer is always a new Array, whatever it was called on.
    assert_eq!(
        run("Array.isArray(Array.prototype.concat.call({length: 1, 0: 'a'}, 'z'))"),
        "true"
    );
}

#[test]
fn fill_and_at_are_the_two_that_count_from_the_end() {
    assert_eq!(run("[1, 2, 3].fill(0).join(',')"), "0,0,0");
    assert_eq!(run("[1, 2, 3].fill(0, 1).join(',')"), "1,0,0");
    assert_eq!(run("[1, 2, 3].fill(0, 1, 2).join(',')"), "1,0,3");
    assert_eq!(run("[1, 2, 3].fill(0, -1).join(',')"), "1,2,0");
    // `fill` answers the array it changed, and it fills a hole in rather than skipping it.
    assert_eq!(
        run("var a = [1, , 3]; a.fill(9); (1 in a) + '|' + a.join(',')"),
        "true|9,9,9"
    );
    assert_eq!(run("var a = [1]; a.fill(0) === a"), "true");
    // §23.1.3.1 `at` exists because `a[-1]` is a property named `"-1"` and always has been.
    assert_eq!(run("[1, 2, 3].at(-1) + '|' + [1, 2, 3].at(0)"), "3|1");
    assert_eq!(
        run("typeof [1, 2].at(5) + '|' + typeof [1, 2].at(-5)"),
        "undefined|undefined"
    );
    assert_eq!(run("[1, 2].at() + '|' + [1, 2].at(1.7)"), "1|2");
}

#[test]
fn all_of_them_work_on_an_array_like_and_write_its_length_back() {
    let like = "var o = {length: 2, 0: 'a', 1: 'b'}; ";
    assert_eq!(
        run(&format!("{like} Array.prototype.lastIndexOf.call(o, 'b')")),
        "1"
    );
    assert_eq!(
        run(&format!(
            "{like} Array.prototype.unshift.call(o, 'z'); o.length + '|' + o[0] + '|' + o[2]"
        )),
        "3|z|b"
    );
    assert_eq!(
        run(&format!(
            "{like} Array.prototype.shift.call(o) + '|' + o.length + '|' + o[0]"
        )),
        "a|1|b"
    );
    assert_eq!(
        run(&format!(
            "{like} var r = Array.prototype.splice.call(o, 0, 1); r.join(',') + '|' + o.length"
        )),
        "a|1"
    );
    assert_eq!(
        run(&format!(
            "{like} Array.prototype.reverse.call(o); o[0] + '|' + o[1]"
        )),
        "b|a"
    );
    assert_eq!(
        run(&format!(
            "{like} var r = Array.prototype.concat.call(o, 'z'); r.length + '|' + Array.isArray(r)"
        )),
        "2|true"
    );
    // A callback that is not one is refused before anything is read, on every one of them.
    for method in [
        "reduce",
        "reduceRight",
        "every",
        "some",
        "find",
        "findIndex",
    ] {
        assert_eq!(
            run(&format!("try {{ [].{method}(1) }} catch (e) {{ e.name }}")),
            "TypeError",
            "{method} with no callback"
        );
    }
}

#[test]
fn the_arithmetic_of_moving_elements_is_checked_where_the_two_directions_differ() {
    // Every method here moves a run of elements, and a run only shows which direction it was
    // moved in when it is longer than one and the move overlaps. These rows are the shapes that
    // tell a forward walk from a backward one.
    //
    // Inserting into the *front* is the case a forward walk would smear: each element would be
    // copied over the next one before it had been read.
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(0, 0, 'x'); a.join(',')"),
        "x,1,2,3"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.unshift('x'); a.join(',')"),
        "x,1,2,3"
    );
    // …and removing from the front is the case a backward walk would smear.
    assert_eq!(
        run("var a = [1, 2, 3, 4]; a.splice(0, 2); a.join(',')"),
        "3,4"
    );
    assert_eq!(run("var a = [1, 2, 3, 4]; a.shift(); a.join(',')"), "2,3,4");
    // `reverse` only swaps a *pair* that is not the middle when there are four or more, which is
    // where the index arithmetic stops being symmetric.
    assert_eq!(run("[1, 2, 3, 4].reverse().join(',')"), "4,3,2,1");
    assert_eq!(run("[1, 2, 3, 4, 5].reverse().join(',')"), "5,4,3,2,1");
    // On an array-like nothing tidies up afterwards, so a shortening has to delete what it left
    // behind rather than relying on an Array's `length` to do it.
    assert_eq!(
        run("var o = {length: 4, 0: 'a', 1: 'b', 2: 'c', 3: 'd'}; \
             Array.prototype.splice.call(o, 0, 2); o.length + '|' + o[0] + '|' + (2 in o)"),
        "2|c|false"
    );
    assert_eq!(
        run("var o = {length: 2, 0: 'a', 1: 'b'}; \
             Array.prototype.splice.call(o, 0, 0, 'x'); o.length + '|' + o[0] + '|' + o[2]"),
        "3|x|b"
    );
    // A hole inside the *removed* run comes out as a hole, from a start that is not zero.
    assert_eq!(
        run(
            "var a = [1, , 3, 4]; var r = a.splice(1, 2); r.length + '|' + (0 in r) + '|' + (1 in r)"
        ),
        "2|false|true"
    );
}

#[test]
fn a_search_from_an_index_is_checked_at_the_two_places_it_turns() {
    // §23.1.3.19's start is a signed number folded onto an index, and it turns twice: at zero,
    // where a given start stops counting from the end, and at the point where counting from the
    // end falls off the front.
    assert_eq!(run("[1, 2, 1].lastIndexOf(1, 0)"), "0");
    assert_eq!(run("[1, 2, 1].lastIndexOf(2, 0)"), "-1");
    // Exactly `-length` lands on index 0 and still searches it; one further off is nothing.
    assert_eq!(run("[1, 2].lastIndexOf(1, -2)"), "0");
    assert_eq!(run("[1, 2].lastIndexOf(1, -3)"), "-1");
    // `at` needs no upper guard — an index past the end reads a property that is not there — but
    // the negative side does, and the boundary is where counting from the end reaches zero.
    assert_eq!(
        run("[1, 2].at(-2) + '|' + typeof [1, 2].at(-3)"),
        "1|undefined"
    );
    assert_eq!(
        run("typeof [1, 2].at(2) + '|' + [1, 2].at(1)"),
        "undefined|2"
    );
}

#[test]
fn a_fold_skips_a_hole_in_the_middle_as_well_as_at_the_start() {
    // §23.1.3.24 step 8.b. The rows above check that a *leading* hole is not the initial value;
    // this one checks that a later one is not folded in as `undefined` either.
    assert_eq!(
        run("[1, , 3].reduce(function (t, x) { return '' + t + x })"),
        "13"
    );
    assert_eq!(
        run("[1, , 3].reduceRight(function (t, x) { return '' + t + x })"),
        "31"
    );
    assert_eq!(
        run("var n = 0; [1, , 3].reduce(function (t) { n = n + 1; return t }, 0); n"),
        "2"
    );
}

#[test]
fn a_length_nothing_could_index_is_refused_rather_than_walked() {
    // §23.1.3.34 step 4.a and §23.1.3.28 step 8 — the one length rule in §23.1 that throws
    // instead of clamping. `ToLength` clamps what is *read*; this is about what would be
    // *written*, and past 2^53-1 there is no index to write to.
    let huge = "var a = {length: 9007199254740991}; ";
    assert_eq!(
        run(&format!(
            "{huge} try {{ Array.prototype.unshift.call(a, null); }} catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
    assert_eq!(
        run(&format!(
            "{huge} try {{ Array.prototype.splice.call(a, 0, 0, 1); }} catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
    // …while with nothing to insert there is no step 4 at all, so this does not walk 2^53 indices
    // moving each onto itself. It used to, on the grounds that a no-op is unobservable — which it
    // is, except in the time it takes.
    assert_eq!(
        run(&format!("{huge} Array.prototype.unshift.call(a); a.length")),
        "9007199254740991"
    );
    // …and a length past the maximum is *clamped* when read, which is the other rule.
    assert_eq!(
        run("var a = {length: 9007199254740992}; Array.prototype.unshift.call(a); a.length"),
        "9007199254740991"
    );
    // A walk that really does have that far to go meets DR-0013's budget instead of running for
    // ever. The interpreter asks between instructions and a built-in never gets back to it, so
    // the question is asked once per index, where the keys are being interned.
    assert_eq!(
        run(&format!(
            "{huge} try {{ Array.prototype.reduceRight.call(a, function () {{}}); }}              catch (e) {{ e.name }}"
        )),
        "RangeError"
    );
    // The boundary itself is asked in Rust rather than here — see `array_methods`'s
    // `length_tests`. Every interesting case is a length too large to walk, so a test written in
    // JavaScript would be a wait rather than a test.
    //
    // Inserting one without removing one is a length past the end, and that is the rule.
    assert_eq!(
        run(&format!(
            "{huge} try {{ Array.prototype.splice.call(a, 0, 0, 'x'); }} catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
    // §23.1.3.28 steps 14 and 15 are an `if` and an `else if`: when a splice puts back exactly as
    // many as it takes out, *neither* runs and no element moves. Doing it anyway changes no value
    // and takes 2^53 steps to change none.
    assert_eq!(
        run("var a = {length: 9007199254740992}; Array.prototype.splice.call(a); a.length"),
        "9007199254740991"
    );
    // …and a splice that really does move the tail still moves it, in both directions.
    assert_eq!(run("var a = [1, 2, 3]; a.splice(1, 1); a.join(',')"), "1,3");
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, 0, 'x'); a.join(',')"),
        "1,x,2,3"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, 2, 'x', 'y'); a.join(',')"),
        "1,x,y"
    );
    // The ordinary cases are untouched by any of it.
    assert_eq!(
        run("var a = [1, 2, 3]; a.unshift(0); a.join(',')"),
        "0,1,2,3"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.splice(1, 1, 'x'); a.join(',')"),
        "1,x,3"
    );
    assert_eq!(
        run("['a', 'b', 'c'].reduceRight(function (l, r) { return l + r; })"),
        "cba"
    );
}
