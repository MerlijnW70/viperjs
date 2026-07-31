//! §23.1.3.30 `sort` and §23.1.3.34 `toSorted` — the default order, stability, and where the
//! things a comparator never sees end up.

use super::*;

#[test]
fn the_default_order_compares_the_spellings_rather_than_the_values() {
    // §23.1.3.30.2 steps 5 to 11 — `ToString` on both, then §7.2.12's comparison. This is why the
    // first row surprises everyone who meets it, and it is the specified answer: "10" sorts before
    // "5" because "1" is before "5", and nothing here knows the elements were numbers.
    assert_eq!(run("[1, 5, 10].sort().join(',')"), "1,10,5");
    assert_eq!(run("[10, 9, 1].sort().join(',')"), "1,10,9");
    assert_eq!(run("['b', 'a', 'c'].sort().join(',')"), "a,b,c");
    // Mixed types go through the same conversion, so the order is the order of their spellings.
    assert_eq!(run("[true, 100, 'a'].sort().join(',')"), "100,a,true");
    // An element with a `toString` is converted by calling it, which is the whole of "the
    // spelling" — an engine comparing anything else would order these the other way about.
    assert_eq!(
        run("var low = { toString: function () { return 'a'; } }; \
             var high = { toString: function () { return 'z'; } }; \
             [high, low].sort().map(String).join(',')"),
        "a,z"
    );
    // §7.2.12 step 3 compares **code units**. U+FF3A is one unit, 0xFF3A; U+1D400 is the pair
    // 0xD835 0xDC00, and its first unit is the smaller — so the character with the *larger* code
    // point sorts first, and an engine comparing code points would disagree.
    assert_eq!(
        run("['\\uFF3A', '\\u{1D400}'].sort()[0] === '\\u{1D400}'"),
        "true"
    );
}

#[test]
fn a_comparator_decides_the_order_and_only_its_sign_is_read() {
    // §23.1.3.30.2 step 4 — negative keeps the pair as it stands, positive swaps it, zero calls
    // them equal. A sort reading it the other way round reverses every one of these.
    assert_eq!(
        run("[1, 5, 10].sort(function (a, b) { return a - b; }).join(',')"),
        "1,5,10"
    );
    assert_eq!(
        run("[1, 5, 10].sort(function (a, b) { return b - a; }).join(',')"),
        "10,5,1"
    );
    // The magnitude says nothing the sign does not, so a comparator answering huge numbers or
    // fractions orders exactly as one answering -1 and 1.
    assert_eq!(
        run("[3, 1, 2].sort(function (a, b) { return (a - b) * 1e300; }).join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run("[3, 1, 2].sort(function (a, b) { return (a - b) * 1e-300; }).join(',')"),
        "1,2,3"
    );
    // Step 4.b — `NaN` is zero, which is "equal". A comparator answering nothing useful therefore
    // still terminates and still answers a permutation, rather than looping or dropping elements.
    assert_eq!(
        run("[3, 1, 2].sort(function () { return NaN; }).length"),
        "3"
    );
    assert_eq!(
        run("[3, 1, 2].sort(function () { return undefined; }).join(',')"),
        "3,1,2"
    );
    // The comparator's answer goes through `ToNumber`, so one answering a string still orders.
    assert_eq!(
        run("[3, 1, 2].sort(function (a, b) { return '' + (a - b); }).join(',')"),
        "1,2,3"
    );
    // Five elements with a comparator that says nothing is out of order: an implementation that
    // moved anything at all fails this, and three elements would not have caught it.
    assert_eq!(
        run("[5, 3, 1, 4, 2].sort(function () { return 0; }).join(',')"),
        "5,3,1,4,2"
    );
    // A comparator answering the same non-zero number to every question contradicts itself — it
    // says `a` is before `b` and also that `b` is before `a` — and §23.1.3.30 leaves the order
    // implementation-defined when it does. What is *not* implementation-defined is that the sort
    // ends and answers a permutation, so that is what these ask. Pinning the arrangement here
    // would be pinning this engine's merge order, and the next sort to be written would fail it
    // while being just as correct.
    assert_eq!(
        run("var a = [1, 2, 3, 4].sort(function () { return 1; }); \
             a.length + ',' + a.slice().sort(function (x, y) { return x - y; }).join(',')"),
        "4,1,2,3,4"
    );
    assert_eq!(
        run("var a = [1, 2, 3, 4].sort(function () { return -1; }); \
             a.length + ',' + a.slice().sort(function (x, y) { return x - y; }).join(',')"),
        "4,1,2,3,4"
    );
}

#[test]
fn equal_elements_come_out_in_the_order_they_went_in() {
    // §23.1.3.30 has required a stable sort since ES2019. Two elements the comparator calls equal
    // must keep the order they had — which is only observable when the elements carry something
    // the comparator does not look at, so every row here compares one field and prints another.
    assert_eq!(
        run(
            "var a = [{k: 1, n: 'a'}, {k: 0, n: 'b'}, {k: 1, n: 'c'}, {k: 0, n: 'd'}, \
                     {k: 1, n: 'e'}, {k: 0, n: 'f'}]; \
             a.sort(function (x, y) { return x.k - y.k; }).map(function (x) { return x.n; }).join('')"
        ),
        "bdface"
    );
    // Long enough to cross more than one merge width — a sort stable at four elements and not at
    // eight is a real mistake and this is where it shows.
    assert_eq!(
        run(
            "var a = []; for (var i = 0; i < 12; i++) { a.push({k: i % 3, n: i}); } \
             a.sort(function (x, y) { return x.k - y.k; }).map(function (x) { return x.n; }).join(',')"
        ),
        "0,3,6,9,1,4,7,10,2,5,8,11"
    );
    // …and every element that went in comes out exactly once, whatever the widths did.
    assert_eq!(
        run(
            "var a = []; for (var i = 0; i < 12; i++) { a.push({k: i % 3, n: i}); } \
             a.sort(function (x, y) { return x.k - y.k; }).length"
        ),
        "12"
    );
}

#[test]
fn what_a_comparator_never_sees_is_put_at_the_end() {
    // §23.1.3.30.2 steps 1 to 3 — `undefined` sorts last whatever the comparator would have said,
    // because it is answered before the comparator is reached. So a comparator sorting descending
    // still finds them at the end rather than the front.
    assert_eq!(
        run("[undefined, 3, undefined, 1].sort(function (a, b) { return b - a; }).join(',')"),
        "3,1,,"
    );
    assert_eq!(
        run("[undefined, 3, undefined, 1].sort().join(',')"),
        "1,3,,"
    );
    // …and it is never *called* with one, which is the reason those steps come first: a comparator
    // that would throw on `undefined` sorts an array full of them without complaint.
    assert_eq!(
        run(
            "var seen = 0; [undefined, undefined, 1].sort(function (a, b) { \
                 seen++; if (a === undefined || b === undefined) { throw new Error('saw one'); } \
                 return 0; }); seen"
        ),
        "0"
    );
    // §23.1.3.30.1's `skip-holes` — a hole is left out of the list entirely, and step 9 deletes the
    // indices past what was written. So holes end up after the `undefined`s, and stay holes.
    assert_eq!(
        run("var a = [, 3, , 1]; a.sort(); a.join(',') + '|' + (0 in a) + ',' + (2 in a)"),
        "1,3,,|true,false"
    );
    assert_eq!(
        run("var a = [, undefined, 2, , 1]; a.sort(); \
             a.join(',') + '|' + (2 in a) + ',' + (3 in a) + ',' + (4 in a)"),
        "1,2,,,|true,false,false"
    );
    // An array that is nothing but holes keeps its length and gains no elements.
    assert_eq!(
        run("var a = [, , ,]; a.sort(); a.length + ',' + (0 in a)"),
        "3,false"
    );
}

#[test]
fn sorting_answers_the_array_it_changed_and_to_sorted_answers_a_new_one() {
    // §23.1.3.30 step 10 against §23.1.3.34 step 9: the same ordering, and the difference is which
    // object comes back and whether the original moved.
    assert_eq!(
        run("var a = [3, 1, 2]; var b = a.sort(); (a === b) + ',' + a.join(',')"),
        "true,1,2,3"
    );
    assert_eq!(
        run("var a = [3, 1, 2]; var b = a.toSorted(); \
             (a === b) + ',' + a.join(',') + ',' + b.join(',')"),
        "false,3,1,2,1,2,3"
    );
    assert_eq!(run("Array.isArray([3, 1].toSorted())"), "true");
    // §23.1.3.34 reads through holes rather than skipping them, so the copy is **dense** where the
    // original was not: every index of it is present, holding the `undefined` a hole reads as.
    assert_eq!(
        run("var b = [, 3, , 1].toSorted(); \
             b.join(',') + '|' + b.length + ',' + (2 in b) + ',' + (3 in b)"),
        "1,3,,|4,true,true"
    );
    // …and the array it was given still has its holes.
    assert_eq!(
        run("var a = [, 3, , 1]; a.toSorted(); (0 in a) + ',' + a.join(',')"),
        "false,,3,,1"
    );
    // Both are generic — §23.1.3 is written against an array-like, and `sort` writes back through
    // ordinary `Set` while `toSorted` answers a real Array either way.
    assert_eq!(
        run("var o = {0: 'c', 1: 'a', 2: 'b', length: 3}; \
             Array.prototype.sort.call(o); o[0] + o[1] + o[2]"),
        "abc"
    );
    assert_eq!(
        run("var o = {0: 'c', 1: 'a', 2: 'b', length: 3}; \
             var b = Array.prototype.toSorted.call(o); \
             Array.isArray(b) + ',' + b.join('') + ',' + o[0]"),
        "true,abc,c"
    );
    // An empty array and a single element are sorted already, and neither calls the comparator.
    assert_eq!(
        run("var seen = 0; [].sort(function () { seen++; return 0; }); \
             [1].sort(function () { seen++; return 0; }); seen"),
        "0"
    );
    assert_eq!(run("[].toSorted().length + ',' + [7].toSorted()[0]"), "0,7");
}

#[test]
fn a_comparator_that_is_not_a_function_is_refused_before_anything_is_read() {
    // §23.1.3.30 step 1 — checked ahead of `this`, so the complaint names the comparator even when
    // `this` is something no Array method could work with. An engine checking them the other way
    // round says the wrong thing here.
    assert_eq!(
        run("try { [1].sort(1); } catch (e) { e.constructor.name + ':' + e.message }"),
        "TypeError:the comparator is not a function"
    );
    assert_eq!(
        run("try { [1].toSorted('x'); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
    assert_eq!(
        run("try { [1].sort(null); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
    assert_eq!(
        run("try { [1].sort({}); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
    assert_eq!(
        run("try { Array.prototype.sort.call(null, 1); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
    // …while *absent* is the default order rather than a mistake, and so is an explicit
    // `undefined` — which is the one value step 1 lets past.
    assert_eq!(run("[2, 1].sort(undefined).join(',')"), "1,2");
    assert_eq!(run("[2, 1].toSorted(undefined).join(',')"), "1,2");
}

#[test]
fn a_comparator_that_throws_stops_the_sort_where_it_stood() {
    // §23.1.3.30.1 step 4 — "stop before performing any further calls". The array is left as it
    // was, because nothing is written back until the whole list is in order.
    assert_eq!(
        run("var a = [3, 1, 2]; \
             try { a.sort(function () { throw new Error('no'); }); } catch (e) { } \
             a.join(',')"),
        "3,1,2"
    );
    assert_eq!(
        run(
            "try { [3, 1, 2].sort(function () { throw new RangeError('no'); }); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // …and no further comparison is made once one has thrown, which is what "stop" adds to
    // "propagate": a sort that caught and continued would keep calling it.
    assert_eq!(
        run("var seen = 0; \
             try { [4, 3, 2, 1].sort(function () { seen++; throw new Error('no'); }); } catch (e) { } \
             seen"),
        "1"
    );
    // A comparator may change the array it is sorting. §23.1.3.30.1 gathered the elements before
    // the first comparison, so what comes back is a permutation of what was there *then*: the
    // three that were gathered are written back over the first three indices in order, and
    // whatever the comparator appended is still past them. How many it appended is how many
    // comparisons this engine made, which is exactly the thing §23.1.3.30.1 step 4 leaves open —
    // so the row asks where the elements are and not how many times it asked.
    assert_eq!(
        run("var a = [3, 1, 2]; \
             a.sort(function (x, y) { a.push(9); return x - y; }); \
             a.slice(0, 3).join(',') + '|' + (a.length > 3)"),
        "1,2,3|true"
    );
}

#[test]
fn an_array_like_longer_than_an_array_may_be_is_refused_by_to_sorted() {
    // §23.1.3.34 step 4 is `ArrayCreate(len)`, and §10.4.2.2 step 1 makes a length past 2^32-1 a
    // **RangeError** — not the clamp `LengthOfArrayLike` applied when the length was read. The
    // check is worth making at a length nothing could walk: reaching it by sorting is not a test,
    // it is a wait, and this is the only way to ask.
    assert_eq!(
        run(
            "try { Array.prototype.toSorted.call({length: 4294967296}); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // One shorter is past that check, and what stops it is DR-0013's budget rather than anything
    // in this module — walking four billion indices interns four billion keys, and `within_budget`
    // is the door that notices. Asking it here would spend four seconds re-proving a boundary that
    // belongs to every method in §23.1.3 rather than to this one.
    //
    // …and a length that fits is not refused, which is what says the bound is a bound rather than
    // a refusal of every array-like that does not happen to be an Array.
    assert_eq!(
        run("Array.prototype.toSorted.call({0: 'b', 1: 'a', length: 2}).join(',')"),
        "a,b"
    );
}
