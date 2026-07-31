//! §24.2.4's seven set operations, the set-like they accept, and the size that decides the work.

use super::*;

/// A set-like written by hand, so that its `size`, `has` and `keys` can be watched.
///
/// `keys` yields the given values once. Counting `has` calls and `next` calls is how the rows
/// below tell the two branches apart, since both answer the same *set*.
const SETLIKE: &str = "function setLike(values, size) { \
     return {size: size === undefined ? values.length : size, \
             hasCalls: 0, nextCalls: 0, closed: false, \
             has: function (v) { this.hasCalls++; return values.indexOf(v) !== -1; }, \
             keys: function () { var o = this, i = 0; return { \
                 next: function () { o.nextCalls++; \
                     return i < values.length ? {done: false, value: values[i++]} : {done: true}; }, \
                 return: function () { o.closed = true; return {done: true}; } }; }}; } ";

#[test]
fn the_seven_answer_the_sets_their_names_promise() {
    assert_eq!(
        run("[...new Set([1, 2, 3]).union(new Set([2, 3, 4]))].join(',')"),
        "1,2,3,4"
    );
    assert_eq!(
        run("[...new Set([1, 2, 3]).intersection(new Set([2, 3, 4]))].join(',')"),
        "2,3"
    );
    assert_eq!(
        run("[...new Set([1, 2, 3]).difference(new Set([2, 3, 4]))].join(',')"),
        "1"
    );
    assert_eq!(
        run("[...new Set([1, 2, 3]).symmetricDifference(new Set([2, 3, 4]))].join(',')"),
        "1,4"
    );
    assert_eq!(
        run("var a = new Set([1, 2]), b = new Set([1, 2, 3]); \
             a.isSubsetOf(b) + ',' + b.isSubsetOf(a) + ',' + a.isSubsetOf(a)"),
        "true,false,true"
    );
    assert_eq!(
        run("var a = new Set([1, 2]), b = new Set([1, 2, 3]); \
             b.isSupersetOf(a) + ',' + a.isSupersetOf(b) + ',' + a.isSupersetOf(a)"),
        "true,false,true"
    );
    assert_eq!(
        run("new Set([1]).isDisjointFrom(new Set([2])) + ',' \
             + new Set([1]).isDisjointFrom(new Set([1])) + ',' \
             + new Set([]).isDisjointFrom(new Set([1]))"),
        "true,false,true"
    );
    // Six answer a `Set` and never the receiver, and the seventh pair answer booleans.
    assert_eq!(
        run("var a = new Set([1]); var u = a.union(new Set([2])); \
             (u instanceof Set) + ',' + (u === a) + ',' + a.size"),
        "true,false,1"
    );
    // §6.1.6.1 — `-0` is stored as `+0`, so a union of the two holds one element.
    assert_eq!(
        run("new Set([0]).union(new Set([-0])).size + ',' \
             + [...new Set([1]).union(new Set([-0]))].join(',')"),
        "1,1,0"
    );
}

#[test]
fn a_set_like_is_anything_with_a_size_a_has_and_a_keys() {
    // §24.2.1.2 checks for three properties and never for a `[[SetData]]`. A `Map` has all three,
    // and its `keys` yields its keys — so it works, which is duck-typing on purpose rather than an
    // accident of the implementation.
    assert_eq!(
        run("[...new Set([1, 2]).union(new Map([[9, 'a'], [1, 'b']]))].join(',')"),
        "1,2,9"
    );
    assert_eq!(
        run(&format!(
            "{SETLIKE} [...new Set([1]).union(setLike([2, 3]))].join(',')"
        )),
        "1,2,3"
    );
    // Steps 2 to 8, each refused in its own way. The size is a **RangeError** when negative and a
    // TypeError when absent, which is the one place these seven do not answer a TypeError.
    assert_eq!(
        run("try { new Set([1]).union({}); } catch (e) { e.constructor.name + ':' + e.message }"),
        "TypeError:a set-like object must have a size"
    );
    assert_eq!(
        run(
            "try { new Set([1]).union({size: -1, has: function () {}, keys: function () {}}); } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run(
            "try { new Set([1]).union({size: 1, has: 1, keys: function () {}}); } \
             catch (e) { e.message }"
        ),
        "a set-like object must have has and keys"
    );
    assert_eq!(
        run(
            "try { new Set([1]).union({size: 1, has: function () {}, keys: 1}); } \
             catch (e) { e.message }"
        ),
        "a set-like object must have has and keys"
    );
    for bad in ["1", "'s'", "null", "undefined"] {
        assert_eq!(
            run(&format!(
                "try {{ new Set([1]).union({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "union({bad})"
        );
    }
    // The **receiver** is checked first and is not duck-typed at all — it needs a real `[[SetData]]`.
    for receiver in ["1", "{}", "new Map()", "new WeakSet()"] {
        assert_eq!(
            run(&format!(
                "try {{ Set.prototype.union.call({receiver}, new Set()); }} \
                 catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "union called on {receiver}"
        );
    }
    // §10.3.3 — one written argument each.
    assert_eq!(
        run(
            "[Set.prototype.union, Set.prototype.intersection, Set.prototype.difference, \
             Set.prototype.symmetricDifference, Set.prototype.isSubsetOf, \
             Set.prototype.isSupersetOf, Set.prototype.isDisjointFrom] \
             .map(function (f) { return f.length; }).join(',')"
        ),
        "1,1,1,1,1,1,1"
    );
}

#[test]
fn which_side_is_walked_is_decided_by_size_and_a_program_can_see_it() {
    // Four of the seven branch on whether the receiver is the smaller. The branch is observable
    // twice over: in the **order** of the result, and in whether `has` or `keys` is used at all.
    // An engine with the comparison backwards answers the same set every time and fails both.
    assert_eq!(
        run("[...new Set([3, 1, 2]).intersection(new Set([1, 2, 3, 4, 5]))].join(',')"),
        "3,1,2"
    );
    assert_eq!(
        run("[...new Set([1, 2, 3, 4, 5]).intersection(new Set([3, 1]))].join(',')"),
        "3,1"
    );
    // The smaller receiver asks `has` once per element and never calls `keys`.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2, 3, 4]); new Set([1, 2]).intersection(s); \
             s.hasCalls + ',' + s.nextCalls"
        )),
        "2,0"
    );
    // …and the larger receiver drives `keys` and never asks `has`.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2]); new Set([1, 2, 3, 4]).intersection(s); \
             s.hasCalls + ',' + s.nextCalls"
        )),
        "0,3"
    );
    // `isSubsetOf` settles the size question without asking anything at all when the receiver is
    // the bigger of the two — step 3, and it is why `has` is never called here.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1], 1); \
             new Set([1, 2]).isSubsetOf(s) + ',' + s.hasCalls + ',' + s.nextCalls"
        )),
        "false,0,0"
    );
    // `symmetricDifference` is the one that never branches: an element in neither set still
    // belongs in the answer, so it must see all of `other` whichever is bigger.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 9]); \
             [...new Set([1, 2, 3, 4, 5]).symmetricDifference(s)].join(',') + '|' + s.hasCalls"
        )),
        "2,3,4,5,9|0"
    );
}

#[test]
fn a_walk_that_knows_the_answer_stops_and_says_so() {
    // §7.4.9 `IteratorClose` — `isSupersetOf` and `isDisjointFrom` stop at the first element that
    // settles the question. Draining the iterator instead would answer the same thing and would
    // call `next` more times than the specification says, which these rows count.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2, 3]); \
             new Set([1, 7, 8, 9]).isSupersetOf(s) + ',' + s.nextCalls + ',' + s.closed"
        )),
        "false,2,true"
    );
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([9, 1, 2]); \
             new Set([1, 2, 3, 4]).isDisjointFrom(s) + ',' + s.nextCalls + ',' + s.closed"
        )),
        "false,2,true"
    );
    // …and a walk that runs to the end does not close, because it was not abandoned.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2]); \
             new Set([1, 2, 3]).isSupersetOf(s) + ',' + s.closed"
        )),
        "true,false"
    );
    // An iterator that never ends is fine as long as the question gets settled — this row would
    // hang on an implementation that collected the keys into a list first.
    //
    // The receiver has to be the *larger* of the two or the walk never starts: an empty set is
    // disjoint from everything and says so from the sizes alone, which is a shorter path and
    // proves nothing about the iterator.
    assert_eq!(
        run(
            "var endless = {size: 1, has: function () { return true; }, \
                 keys: function () { var i = 0; return {next: function () { \
                     return {done: false, value: i++}; }}; }}; \
             new Set([1, 2]).isDisjointFrom(endless)"
        ),
        "false"
    );
    // …and the empty receiver, which is the short path and is worth its own row.
    assert_eq!(
        run(
            "var endless = {size: 1, has: function () { return true; }, \
                 keys: function () { throw new Error('walked'); }}; \
             new Set([]).isDisjointFrom(endless)"
        ),
        "true"
    );
}

#[test]
fn a_set_like_that_lies_about_its_size_shows_which_branch_ran() {
    // Every one of these rows exists because the obvious version of it passes whichever way the
    // code reads. A set-like may claim any `size` it likes, and §24.2.1.2 takes it at its word —
    // which is the only way to make the size branch and the walk disagree, and so the only way to
    // tell that the branch is doing anything.
    //
    // §24.2.4.14 step 3: two elements cannot contain five, so the answer is `false` *from the
    // sizes*. Walking instead would find the single element it actually yields, all of it present,
    // and answer `true`.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1], 5);              new Set([1, 2]).isSupersetOf(s) + ',' + s.nextCalls"
        )),
        "false,0"
    );
    // §24.2.4.12 — the receiver is the smaller, so `has` is asked about each of its elements and
    // the first miss ends it. The size check cannot reach this: it passed.
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2, 3]);              new Set([1, 9]).isSubsetOf(s) + ',' + s.hasCalls"
        )),
        "false,2"
    );
    assert_eq!(
        run(&format!(
            "{SETLIKE} var s = setLike([1, 2, 3]); new Set([1, 2]).isSubsetOf(s)"
        )),
        "true"
    );
    // §24.2.4.9's walking branch keeps only what the receiver also has. With every element of the
    // argument present in the receiver the two readings agree, so one of them must be missing.
    assert_eq!(
        run("[...new Set([1, 2, 3, 4, 5]).intersection(new Set([3, 99]))].join(',')"),
        "3"
    );
    // §24.2.4.5's walking branch *removes* what the argument has. Same reasoning: the removal has
    // to leave something behind for the row to see it happen.
    assert_eq!(
        run("[...new Set([1, 2, 3, 4, 5]).difference(new Set([2, 4]))].join(',')"),
        "1,3,5"
    );
    // §6.1.6.1 canonicalises `-0` and **nothing else**. A negative that is not zero arrives
    // unchanged, which a guard reading "zero or negative" would flatten to `+0`.
    assert_eq!(
        run(&format!(
            "{SETLIKE} [...new Set([]).union(setLike([-5, -0, 7]))].join(',')"
        )),
        "-5,0,7"
    );
    // §24.2.1.2 step 7 refuses a *negative* size and allows nought, which is the boundary between
    // "this set-like is empty" and "this set-like is nonsense".
    assert_eq!(
        run(&format!(
            "{SETLIKE} [...new Set([1]).union(setLike([], 0))].join(',') + '|'              + new Set([1]).isSubsetOf(setLike([], 0))"
        )),
        "1|false"
    );
}
