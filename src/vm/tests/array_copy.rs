//! §23.1.3's change-copy methods and `copyWithin` — where the copy differs from the original, and
//! the one place a relative index throws instead of clamping.

use super::*;

#[test]
fn a_change_copy_leaves_the_array_it_was_given_alone() {
    // §23.1.3.33, §23.1.3.35 and §23.1.3.39 — each is one of the mutating methods with the
    // mutation taken out, so the row that matters is the original still reading the same
    // afterwards and the answer being a different object.
    assert_eq!(
        run("var a = [1, 2, 3]; var b = a.toReversed(); \
             a.join(',') + '|' + b.join(',') + '|' + (a === b)"),
        "1,2,3|3,2,1|false"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; var b = a.with(1, 'x'); a.join(',') + '|' + b.join(',')"),
        "1,2,3|1,x,3"
    );
    assert_eq!(
        run(
            "var a = [1, 2, 3, 4]; var b = a.toSpliced(1, 2, 'x', 'y', 'z'); \
             a.join(',') + '|' + b.join(',')"
        ),
        "1,2,3,4|1,x,y,z,4"
    );
    // Each answers a real Array however it was called, which is what `ArrayCreate` rather than
    // `ArraySpeciesCreate` means: a subclass does not get its own kind back.
    assert_eq!(
        run(
            "[Array.isArray([1].toReversed()), Array.isArray([1].with(0, 1)), \
             Array.isArray([1].toSpliced(0))].join(',')"
        ),
        "true,true,true"
    );
    // …and they are generic, like the rest of §23.1.3.
    assert_eq!(
        run("var o = {0: 'a', 1: 'b', length: 2}; \
             Array.prototype.toReversed.call(o).join(',') + '|' + \
             Array.prototype.with.call(o, 0, 'z').join(',')"),
        "b,a|z,b"
    );
}

#[test]
fn a_change_copy_has_no_holes_because_it_never_asks() {
    // The whole of §23.1.3.33, §23.1.3.35 and §23.1.3.39 reads with `Get` and writes with
    // `CreateDataPropertyOrThrow` — there is no `HasProperty` in any of them. So a hole reads as
    // `undefined` and is *written*, and the copy is dense where the original was not. `slice` is
    // the contrast: it does ask, and it does keep them.
    assert_eq!(
        run("var b = [, 1, ,].toReversed(); \
             b.length + ',' + (0 in b) + ',' + (2 in b) + '|' + [, 1, ,].slice().length + ',' \
             + (0 in [, 1, ,].slice())"),
        "3,true,true|3,false"
    );
    assert_eq!(
        run("var b = [, ,].with(0, 'x'); b.length + ',' + (1 in b) + ',' + b.join(',')"),
        "2,true,x,"
    );
    assert_eq!(
        run("var b = [, 1, ,].toSpliced(1, 0); b.length + ',' + (0 in b) + ',' + (2 in b)"),
        "3,true,true"
    );
    // A hole with something at the same index on the prototype reads *through* — which is what
    // says these use `Get` and not an element read of their own.
    assert_eq!(
        run(
            "Array.prototype[0] = 'inherited'; var b = [, 'x'].toReversed(); \
             delete Array.prototype[0]; b.join(',')"
        ),
        "x,inherited"
    );
}

#[test]
fn with_refuses_an_index_the_array_does_not_have() {
    // §23.1.3.39 step 5 — the one relative index in §23.1.3 that throws rather than clamping.
    // `slice` and `indexOf` clamp an out-of-range start because there is still a range to search;
    // `with` has to put the value *somewhere*, and past the end there is nowhere.
    assert_eq!(run("[1, 2, 3].with(-1, 'x').join(',')"), "1,2,x");
    assert_eq!(run("[1, 2, 3].with(-3, 'x').join(',')"), "x,2,3");
    assert_eq!(run("[1, 2, 3].with(2, 'x').join(',')"), "1,2,x");
    for index in ["3", "-4", "Infinity", "-Infinity"] {
        assert_eq!(
            run(&format!(
                "try {{ [1, 2, 3].with({index}, 'x'); }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "with({index}) should be out of range"
        );
    }
    // …and every index of an empty array is out of range, including zero.
    assert_eq!(
        run("try { [].with(0, 'x'); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // A fractional index truncates toward zero rather than rounding, and `undefined` is zero.
    assert_eq!(
        run("[1, 2].with(1.9, 'x').join(',') + '|' + [1, 2].with(undefined, 'x').join(',')"),
        "1,x|x,2"
    );
    // Step 8.b uses the replacement *instead of* reading, so the getter at that index never runs.
    assert_eq!(
        run(
            "var seen = ''; var o = {length: 2, get 0() { seen += '0'; return 'a'; }, \
             get 1() { seen += '1'; return 'b'; }}; \
             Array.prototype.with.call(o, 1, 'z').join(',') + '|' + seen"
        ),
        "a,z|0"
    );
}

#[test]
fn to_spliced_counts_its_arguments_rather_than_reading_them() {
    // §23.1.3.35 step 6 — three cases, decided by how many arguments were *given*. This is the
    // one place where an absent argument and an explicit `undefined` differ, and they differ a
    // lot: absent removes the whole tail, `undefined` removes nothing.
    assert_eq!(run("[1, 2, 3].toSpliced().join(',')"), "1,2,3");
    assert_eq!(run("[1, 2, 3].toSpliced(1).join(',')"), "1");
    assert_eq!(run("[1, 2, 3].toSpliced(1, undefined).join(',')"), "1,2,3");
    assert_eq!(run("[1, 2, 3].toSpliced(1, 1).join(',')"), "1,3");
    // The start clamps at both ends the way every other relative index does — this is not `with`.
    assert_eq!(run("[1, 2, 3].toSpliced(-1, 1).join(',')"), "1,2");
    assert_eq!(run("[1, 2, 3].toSpliced(-9, 1).join(',')"), "2,3");
    assert_eq!(run("[1, 2, 3].toSpliced(9, 1, 'x').join(',')"), "1,2,3,x");
    // …and so does the count: more than is there removes what is there, and a negative removes
    // nothing rather than counting backwards.
    assert_eq!(run("[1, 2, 3].toSpliced(1, 99).join(',')"), "1");
    assert_eq!(run("[1, 2, 3].toSpliced(1, -1, 'x').join(',')"), "1,x,2,3");
    // Inserting without removing, removing without inserting, and doing both at once — the three
    // shapes the length arithmetic has to get right.
    assert_eq!(
        run(
            "[1, 2].toSpliced(1, 0, 'x').length + ',' + [1, 2].toSpliced(0, 1).length + ',' \
             + [1, 2].toSpliced(0, 2, 'x').length"
        ),
        "3,1,1"
    );
    assert_eq!(run("[].toSpliced(0, 0, 'x').join(',')"), "x");
}

#[test]
fn a_copy_longer_than_an_array_may_be_is_refused_by_the_rule_that_applies_first() {
    // §23.1.3.35 step 8 is a **TypeError** about 2^53-1, and step 9's `ArrayCreate` is a
    // **RangeError** about 2^32-1. Which one a program meets says the checks are in the specified
    // order — an engine doing `ArrayCreate` first answers RangeError to both of these.
    assert_eq!(
        run(
            "try { Array.prototype.toSpliced.call({length: 9007199254740991}, 0, 0, 'x'); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Array.prototype.toSpliced.call({length: 4294967296}, 0, 0); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // `toReversed` and `with` have only the second of the two.
    assert_eq!(
        run(
            "try { Array.prototype.toReversed.call({length: 4294967296}); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run(
            "try { Array.prototype.with.call({length: 4294967296}, 0, 'x'); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
}

#[test]
fn copy_within_moves_overlapping_elements_without_reading_what_it_wrote() {
    // §23.1.3.4 — the copy is inside one array, so the ranges can overlap. Every row here has an
    // overlap in it, because that is the only thing that tells a correct implementation from one
    // that walks the wrong way: `[1,2,3,4,5].copyWithin(1, 0)` written forwards answers
    // `1,1,1,1,1` as each write feeds the next read.
    assert_eq!(
        run("[1, 2, 3, 4, 5].copyWithin(1, 0).join(',')"),
        "1,1,2,3,4"
    );
    assert_eq!(
        run("[1, 2, 3, 4, 5].copyWithin(0, 1).join(',')"),
        "2,3,4,5,5"
    );
    assert_eq!(
        run("[1, 2, 3, 4, 5].copyWithin(0, 3, 5).join(',')"),
        "4,5,3,4,5"
    );
    assert_eq!(
        run("[1, 2, 3, 4, 5].copyWithin(1, 3, 5).join(',')"),
        "1,4,5,4,5"
    );
    // The three relative indices all count back from the end and all clamp.
    assert_eq!(
        run("[1, 2, 3, 4, 5].copyWithin(-2, -3, -1).join(',')"),
        "1,2,3,3,4"
    );
    assert_eq!(run("[1, 2, 3].copyWithin(9, 0).join(',')"), "1,2,3");
    assert_eq!(run("[1, 2, 3].copyWithin(0, 9).join(',')"), "1,2,3");
    assert_eq!(run("[1, 2, 3].copyWithin(0, 2, 1).join(',')"), "1,2,3");
    // It answers the array it changed, and it changes it in place.
    assert_eq!(
        run("var a = [1, 2, 3]; (a.copyWithin(0, 1) === a) + ',' + a.join(',')"),
        "true,2,3,3"
    );
    // Step 9.c — a hole at the source *deletes* the destination rather than writing `undefined`,
    // which is what makes `copyWithin` a mover rather than a copier.
    assert_eq!(
        run("var a = [1, , 3]; a.copyWithin(0, 1); a.join(',') + '|' + (0 in a) + ',' + (1 in a)"),
        ",3,3|false,true"
    );
    // …and it is generic, like the rest of them.
    assert_eq!(
        run("var o = {0: 'a', 1: 'b', 2: 'c', length: 3}; \
             Array.prototype.copyWithin.call(o, 0, 1); o[0] + o[1] + o[2]"),
        "bcc"
    );
}

#[test]
fn the_direction_a_copy_runs_in_is_visible_at_the_two_edges_of_the_overlap() {
    // §23.1.3.4 step 8 decides the direction and step 9 walks in it, performing a `Get` and a
    // `Set` per index — so the *order* of those is specified, not merely the values that end up
    // in the array. On a plain array the two directions are indistinguishable at the boundaries:
    // when the source and the destination are the same range, or when they merely touch, both
    // orders move the same values to the same places. Accessors are what make the difference
    // visible, and these are the only rows that can tell `from < to` from `from <= to`.
    let watched = "function watched(n) { \
         var o = {length: n, order: ''}; \
         for (var i = 0; i < n; i++) { (function (i) { \
             Object.defineProperty(o, i, { \
                 get: function () { o.order += 'g' + i; return i; }, \
                 set: function (v) { o.order += 's' + i; }, \
                 configurable: true }); })(i); } \
         return o; } ";
    // `copyWithin(0, 0)` copies the array onto itself. Nothing moves, and there is no overlap to
    // walk backwards for — `from < to` is false — so it runs forwards, from index 0.
    assert_eq!(
        run(&format!(
            "{watched} var o = watched(3); Array.prototype.copyWithin.call(o, 0, 0); o.order"
        )),
        "g0s0g1s1g2s2"
    );
    // …and the other edge: a source of `[0, 2)` and a destination of `[2, 4)` *touch* without
    // overlapping, so `to < from + count` is false by exactly one and the walk is forwards again.
    // Reading these as `<=` would run both of them backwards and answer the same array.
    assert_eq!(
        run(&format!(
            "{watched} var o = watched(4); Array.prototype.copyWithin.call(o, 2, 0, 2); o.order"
        )),
        "g0s2g1s3"
    );
    // One index further and they do overlap, which is where the direction earns its place: this
    // is the case that would read what it had already written if it ran forwards.
    assert_eq!(
        run(&format!(
            "{watched} var o = watched(4); Array.prototype.copyWithin.call(o, 1, 0, 3); o.order"
        )),
        "g2s3g1s2g0s1"
    );
}
