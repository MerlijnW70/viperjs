//! §10.4.2 and §23.1 as a script sees them — the one exotic object in the language.

use super::*;

#[test]
fn an_index_at_or_past_the_end_raises_the_length() {
    // §10.4.2.1 step 3. This is the half people know, and it is what makes an array grow by being
    // written to rather than by being told to.
    assert_eq!(run("var a = []; a[0] = 1; a.length"), "1");
    assert_eq!(run("var a = []; a[5] = 1; a.length"), "6");
    assert_eq!(run("var a = [1, 2, 3]; a[1] = 9; a.length"), "3");
    // The indices in between are absent rather than `undefined` — that is what a hole is, and
    // `in` is what tells them apart.
    assert_eq!(
        run("var a = []; a[2] = 1; (0 in a) + '|' + (2 in a)"),
        "false|true"
    );
    // A key that is not the canonical spelling of an index is an ordinary property and moves
    // nothing: `a["01"]` and `a["1.0"]` are names, not indices.
    assert_eq!(run("var a = []; a['01'] = 1; a.length"), "0");
    assert_eq!(run("var a = []; a['1.0'] = 1; a.length"), "0");
    assert_eq!(run("var a = []; a['-1'] = 1; a.length"), "0");
    // `"-0"` is a canonical numeric index string (§7.1.21) and is not an array index (§6.1.7),
    // because `ToUint32` writes it back as `"0"`. Its own row because `-0.0 < 0.0` is false, so
    // a sign test written as a comparison lets it through and nothing else notices.
    assert_eq!(
        run("var a = []; a['-0'] = 1; a.length + '|' + a['-0']"),
        "0|1"
    );
    // §6.1.7 stops one short of `2^32`, so the last value is a name rather than an index.
    assert_eq!(run("var a = []; a['4294967295'] = 1; a.length"), "0");
    assert_eq!(
        run("var a = []; a['4294967294'] = 1; a.length"),
        "4294967295"
    );
}

#[test]
fn writing_the_length_deletes_the_indices_above_it() {
    // §10.4.2.4, the half that is less well known and is why `a.length = 0` is the idiom for
    // emptying an array.
    assert_eq!(
        run("var a = [1, 2, 3]; a.length = 1; a.length + '|' + a[0] + '|' + a[1]"),
        "1|1|undefined"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.length = 0; a.length + '|' + (0 in a)"),
        "0|false"
    );
    // Growing deletes nothing and adds nothing: the indices between are simply absent.
    assert_eq!(
        run("var a = [1]; a.length = 5; a.length + '|' + (3 in a)"),
        "5|false"
    );
}

#[test]
fn a_shortening_that_cannot_finish_stops_where_it_got_to() {
    // §10.4.2.4 step 15 deletes from the top down and stops at the first index that refuses,
    // leaving `length` one past *that* one rather than where it was asked to go. The elements
    // below the immovable one survive and the ones above it are already gone — which is a state
    // no other operation in the language can produce.
    let frozen = "var a = [1, 2, 3, 4, 5]; \
                  Object.defineProperty(a, '2', {value: 3, configurable: false}); \
                  a.length = 0; ";
    assert_eq!(run(&format!("{frozen} a.length")), "3");
    assert_eq!(
        run(&format!("{frozen} (2 in a) + '|' + (3 in a)")),
        "true|false"
    );
    assert_eq!(
        run(&format!("{frozen} (0 in a) + '|' + (1 in a)")),
        "true|true"
    );
}

#[test]
fn a_length_that_is_not_an_integer_index_throws_where_every_other_refusal_is_silent() {
    // §10.4.2.4 step 2, and the one place an *assignment* in this language throws because of its
    // value rather than because of what it was assigned to. Every other refused write in sloppy
    // code is dropped on the floor, which is why this needs saying twice: once for the
    // assignment and once for `defineProperty`.
    assert_eq!(
        run("var a = []; try { a.length = -1 } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        run("var a = []; try { a.length = 1.5 } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { Object.defineProperty([], 'length', {value: -1}) } catch (e) { e.name }"),
        "RangeError"
    );
    // …while an ordinary refusal stays silent, which is the contrast that gives the rows above
    // their meaning.
    assert_eq!(
        run(
            "var a = [1]; Object.defineProperty(a, 'length', {writable: false}); a.length = 5; a.length"
        ),
        "1"
    );
}

#[test]
fn a_length_that_may_not_move_stops_the_array_growing() {
    // §10.4.2.1 step 3.b — an index past the end needs `length` to move, so a `length` that is
    // not writable is what makes an array fixed.
    let fixed = "var a = [1]; Object.defineProperty(a, 'length', {writable: false}); ";
    assert_eq!(
        run(&format!("{fixed} a[5] = 1; a.length + '|' + (5 in a)")),
        "1|false"
    );
    // …and writing *inside* the array still works, because that needs no length change at all.
    assert_eq!(run(&format!("{fixed} a[0] = 9; a[0]")), "9");
}

#[test]
fn a_literal_counts_its_holes_and_does_not_fill_them() {
    // §13.2.4.1 — the length is the element count including elisions, so a trailing hole counts
    // and an absent element is absent rather than `undefined`.
    assert_eq!(run("[1, 2, 3].length"), "3");
    assert_eq!(run("[].length"), "0");
    assert_eq!(run("[, 1].length + '|' + (0 in [, 1])"), "2|false");
    assert_eq!(run("[1, , 2].length"), "3");
    // The difference a hole makes, said the only way it can be said without iteration.
    assert_eq!(run("(0 in [undefined]) + '|' + (0 in [,])"), "true|false");
    assert_eq!(run("[,].length"), "1");
}

#[test]
fn the_constructors_single_number_is_a_length_and_anything_else_is_an_element() {
    // §23.1.1.1 steps 2 and 3 — the reason `Array(3)` and `[3]` differ, and the reason nobody
    // uses the constructor to make a literal.
    assert_eq!(
        run("new Array(3).length + '|' + (0 in new Array(3))"),
        "3|false"
    );
    assert_eq!(
        run("new Array('3').length + '|' + new Array('3')[0]"),
        "1|3"
    );
    assert_eq!(
        run("new Array(1, 2).length + '|' + new Array(1, 2)[1]"),
        "2|2"
    );
    assert_eq!(run("new Array().length"), "0");
    // Called without `new` it does the same thing, like `Error` and unlike most constructors.
    assert_eq!(run("Array(3).length"), "3");
    // A length that is not an integer index throws rather than rounding.
    assert_eq!(
        run("try { new Array(1.5) } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { new Array(-1) } catch (e) { e.name }"),
        "RangeError"
    );
}

#[test]
fn is_array_asks_what_an_object_is_where_instanceof_asks_what_it_inherits() {
    // §23.1.2.2. `instanceof` walks a prototype chain, so it is false for an array from another
    // realm and true for anything simply *given* `Array.prototype`; `typeof` says `"object"` for
    // both. This is the only way to ask the real question.
    assert_eq!(run("Array.isArray([])"), "true");
    assert_eq!(run("Array.isArray(new Array(2))"), "true");
    assert_eq!(run("Array.isArray({})"), "false");
    assert_eq!(
        run("Array.isArray(1) + '|' + Array.isArray(null)"),
        "false|false"
    );
    // An ordinary object with `Array.prototype` behind it is *not* an array, and `instanceof`
    // cannot tell.
    let borrowed = "var fake = Object.create(Array.prototype); ";
    assert_eq!(run(&format!("{borrowed} fake instanceof Array")), "true");
    assert_eq!(run(&format!("{borrowed} Array.isArray(fake)")), "false");
    // §23.1.3 — `Array.prototype` is itself an Array, which is why its `length` is 0 rather than
    // absent.
    assert_eq!(
        run("Array.isArray(Array.prototype) + '|' + Array.prototype.length"),
        "true|0"
    );
}

#[test]
fn an_arrays_length_is_a_property_and_not_a_count_beside_the_elements() {
    // §10.4.2.2 step 6 gives it writable, not enumerable and not configurable — so it may be
    // assigned to, never shows in an enumeration, and cannot be deleted.
    assert_eq!(run("delete [1].length"), "false");
    assert_eq!(run("Object.keys([1, 2]).length"), "2");
    let attributes = "var d = Object.getOwnPropertyDescriptor([1], 'length'); \
                      d.writable + '|' + d.enumerable + '|' + d.configurable";
    assert_eq!(run(attributes), "true|false|false");
    // …and now that arrays exist, the list `Object.keys` answers with is a real one.
    assert_eq!(run("Array.isArray(Object.keys({a: 1}))"), "true");
    assert_eq!(run("Object.keys({a: 1, b: 2})[1]"), "b");
}

#[test]
fn every_way_of_writing_a_length_agrees_about_what_a_length_is() {
    // Three callers can throw §10.4.2.4 step 2's RangeError, and they must agree — a bad length
    // that is a RangeError through an assignment and a TypeError through `defineProperties`
    // would be the same mistake reported as two different ones.
    for write in [
        "a.length = -1",
        "a.length = 1.5",
        "a.length = '1.5'",
        "a.length = 4294967296",
        "Object.defineProperty(a, 'length', {value: -1})",
        "Object.defineProperties(a, {length: {value: -1}})",
    ] {
        let source = format!("var a = [1]; try {{ {write} }} catch (e) {{ e.name }}");
        assert_eq!(run(&source), "RangeError", "writing {write}");
    }
    // …and a length that *is* an integer index goes through every one of them.
    for write in [
        "a.length = 0",
        "a.length = '0'",
        "Object.defineProperty(a, 'length', {value: 0})",
        "Object.defineProperties(a, {length: {value: 0}})",
    ] {
        let source = format!("var a = [1]; {write}; a.length");
        assert_eq!(run(&source), "0", "writing {write}");
    }
}

#[test]
fn a_length_that_may_not_move_may_still_be_written_with_the_value_it_has() {
    // §10.4.2.4 step 12 — the comparison is with the *current* length, not a blanket refusal. So
    // `a.length = a.length` is allowed on a fixed-length array and `a.length = 0` is not, which
    // is the difference between "frozen" and "unwritable".
    let fixed = "var a = [1, 2]; Object.defineProperty(a, 'length', {writable: false}); ";
    assert_eq!(run(&format!("{fixed} a.length = 2; a.length")), "2");
    assert_eq!(
        run(&format!("{fixed} a.length = 0; a.length + '|' + (1 in a)")),
        "2|true"
    );
    assert_eq!(run(&format!("{fixed} a.length = 5; a.length")), "2");
}

#[test]
fn growing_and_shrinking_are_told_apart_at_exactly_the_current_length() {
    // Shrinking deletes and growing does not, so the boundary between them is where a
    // half-implemented `<` shows up: writing the length it already has must delete nothing.
    assert_eq!(
        run("var a = [1, 2, 3]; a.length = 3; (2 in a) + '|' + a.length"),
        "true|3"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.length = 2; (2 in a) + '|' + a.length"),
        "false|2"
    );
    assert_eq!(
        run("var a = [1, 2, 3]; a.length = 4; (2 in a) + '|' + a.length"),
        "true|4"
    );
    // A shortening that finished answers `true` to the assignment; one that could not answers
    // `false` — and in sloppy code the only trace of that is where `length` ended up.
    let stuck =
        "var a = [1, 2, 3]; Object.defineProperty(a, '1', {value: 2, configurable: false}); ";
    assert_eq!(run(&format!("{stuck} a.length = 0; a.length")), "2");
    assert_eq!(run(&format!("{stuck} a.length = 3; a.length")), "3");
}

#[test]
fn a_fixed_length_may_be_redefined_with_the_value_it_already_has() {
    // §10.4.2.4 step 12. An *assignment* never gets here — §10.1.9.2 refuses a write to a
    // non-writable data property before the array's rules are consulted at all — so this is
    // reachable only through `defineProperty`, which is why it needs its own rows.
    let fixed = "var a = [1, 2]; Object.defineProperty(a, 'length', {writable: false}); ";
    assert_eq!(
        run(&format!(
            "{fixed} Object.defineProperty(a, 'length', {{value: 2}}); a.length"
        )),
        "2"
    );
    assert_eq!(
        run(&format!(
            "{fixed} try {{ Object.defineProperty(a, 'length', {{value: 0}}) }} catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
    assert_eq!(
        run(&format!(
            "{fixed} try {{ Object.defineProperty(a, 'length', {{value: 5}}) }} catch (e) {{ e.name }}"
        )),
        "TypeError"
    );
    // The refusal has to come *before* anything is deleted. §10.1.6.3 would refuse the length
    // write on its own, so the error is the same either way — but a shortening that ran first
    // would already have thrown the elements away, and the throw would be the only trace left.
    assert_eq!(
        run(&format!(
            "{fixed} try {{ Object.defineProperty(a, 'length', {{value: 0}}) }} catch (e) {{}} (0 in a) + '|' + (1 in a)"
        )),
        "true|true"
    );
}

#[test]
fn a_length_define_that_the_ordinary_rules_refuse_is_refused_whole() {
    // §10.4.2.4 step 16 stores the length through §10.1.6.3, and step 17 answers with *both*
    // halves: the store having worked and the shortening having finished. `length` is not
    // configurable, so asking to make it so is refused even though the length itself is fine —
    // and a refusal that only looked at the shortening would report success.
    assert_eq!(
        run(
            "var a = [1]; try { Object.defineProperty(a, 'length', {value: 1, configurable: true}) } catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var a = [1]; try { Object.defineProperty(a, 'length', {value: 1, enumerable: true}) } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn an_index_exactly_at_the_length_is_the_one_that_needs_the_length_to_move() {
    // §10.4.2.1 step 3.b compares with `>=`, and the boundary is the whole of what it means: an
    // index *below* the length needs no room made, and one *at* it does.
    let fixed = "var a = [1]; Object.defineProperty(a, 'length', {writable: false}); ";
    assert_eq!(run(&format!("{fixed} a[0] = 9; a[0]")), "9");
    assert_eq!(
        run(&format!("{fixed} a[1] = 9; (1 in a) + '|' + a.length")),
        "false|1"
    );
    // …and with a writable length the same write is ordinary and moves it by one.
    assert_eq!(run("var a = [1]; a[1] = 9; a.length"), "2");
}

#[test]
fn the_constructors_elements_are_ordinary_properties_and_its_prototype_is_not() {
    // §23.1.1.1 step 4 uses `CreateDataPropertyOrThrow`, which is §6.1.7.1's three defaults —
    // so an element made this way is indistinguishable from one written by assignment.
    let element = "var d = Object.getOwnPropertyDescriptor(new Array(7, 8), '0'); \
                   d.value + '|' + d.writable + '|' + d.enumerable + '|' + d.configurable";
    assert_eq!(run(element), "7|true|true|true");
    // §23.1.4 — `Array.prototype` on the constructor is none of the three, for the same reason
    // `Object.prototype` is not: every array in the realm inherits from it.
    let prototype = "var d = Object.getOwnPropertyDescriptor(Array, 'prototype'); \
                     d.writable + '|' + d.enumerable + '|' + d.configurable";
    assert_eq!(run(prototype), "false|false|false");
    assert_eq!(run("delete Array.prototype"), "false");
    assert_eq!(
        run("var p = Array.prototype; Array.prototype = {}; Array.prototype === p"),
        "true"
    );
}
