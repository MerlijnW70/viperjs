//! §14.7.5 — `for`-`in`, and the enumeration order and shadowing that decide what it visits.
//!
//! Checked against V8 before being written down, like the lexical rows: enumeration is a rule
//! about which names appear and in what order, and an engine that got it slightly wrong would
//! still run most loops correctly.

use super::*;

#[test]
fn a_for_in_visits_the_enumerable_names_of_the_object_and_its_prototypes() {
    assert_eq!(
        run("var r = ''; for (var k in {a: 1, b: 2}) { r = r + k; } r"),
        "ab"
    );
    assert_eq!(
        run("var r = ''; for (var k in [7, 8, 9]) { r = r + k; } r"),
        "012"
    );
    // §10.1.11's order, per object: array indices ascending, then names in creation order.
    assert_eq!(
        run("var r = ''; for (var k in {b: 1, 2: 2, a: 3, 1: 4}) { r = r + k; } r"),
        "12ba"
    );
    // What it yields is always a String, even for an index.
    assert_eq!(
        run("var r = ''; for (var k in [1]) { r = r + typeof k; } r"),
        "string"
    );
    // Own names first, then each prototype's in turn.
    assert_eq!(
        run(
            "var p = {inherited: 1}; var o = Object.create(p); o.own = 2; \
             var r = ''; for (var k in o) { r = r + k + ','; } r"
        ),
        "own,inherited,"
    );
    // An object with nothing to enumerate runs the body no times — and neither does one whose
    // only properties are on a prototype that has none either.
    assert_eq!(
        run("var r = 'no'; for (var k in {}) { r = 'ran'; } r"),
        "no"
    );
}

#[test]
fn a_name_is_visited_once_and_a_hidden_property_still_takes_it() {
    // §14.7.5.10 — the enumeration "must not visit a property more than once", so an own property
    // shadows a prototype's of the same name.
    assert_eq!(
        run(
            "var p = {shared: 1}; var o = Object.create(p); o.shared = 2; \
             var r = ''; for (var k in o) { r = r + k; } r"
        ),
        "shared"
    );
    // …and *shadowed* is about the name, not about visibility: a non-enumerable own property is
    // not visited itself and stops the prototype's from being visited either. Getting this wrong
    // is the difference between skipping a name and yielding it twice.
    assert_eq!(
        run("var p = {hidden: 1, plain: 2}; var o = Object.create(p); \
             Object.defineProperty(o, 'hidden', {value: 3, enumerable: false}); \
             var r = ''; for (var k in o) { r = r + k; } r"),
        "plain"
    );
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'hidden', {value: 1, enumerable: false}); \
             o.seen = 2; var r = ''; for (var k in o) { r = r + k; } r"
        ),
        "seen"
    );
}

#[test]
fn a_property_deleted_during_the_loop_is_not_visited_and_one_added_is_not_either() {
    // The half of §14.7.5.10 that a snapshot alone does not give: the names are taken once, but
    // each is asked about again before it is used, so a `delete` in the body is respected.
    assert_eq!(
        run("var o = {a: 1, b: 2, c: 3}; var r = ''; \
             for (var k in o) { delete o.b; r = r + k; } r"),
        "ac"
    );
    // The other half, which the snapshot *is*: §14.7.5.10 says a property added during the
    // enumeration need not be visited, and this one is not — which is also what stops the loop
    // below from running forever.
    assert_eq!(
        run("var o = {a: 1, b: 2}; var r = ''; for (var k in o) { o.added = 9; r = r + k; } r"),
        "ab"
    );
}

#[test]
fn a_head_that_is_not_a_declaration_assigns_to_whatever_it_names() {
    assert_eq!(
        run("var r = ''; var k; for (k in {p: 1, q: 2}) { r = r + k; } r"),
        "pq"
    );
    // A property target, which is why the name is put in a slot of its own before it is assigned
    // anywhere: the base and the key have to be built *under* it.
    assert_eq!(
        run("var t = {}; var r = ''; for (t.p in {a: 1, b: 2}) { r = r + t.p; } r"),
        "ab"
    );
    assert_eq!(
        run("var t = {}; var n = 'x'; var r = ''; for (t[n] in {a: 1}) { r = r + t.x; } r"),
        "a"
    );
}

#[test]
fn a_var_head_outlives_the_loop_and_a_lexical_one_does_not() {
    // §14.7.5.5 gives a `let` a fresh binding per pass and gives a `var` none at all: the `var`
    // is the function's, hoisted with every other, and it keeps the last name it was given.
    assert_eq!(
        run("var o = {a: 1}; for (var k in o) { } typeof k"),
        "string"
    );
    assert_eq!(run("var o = {a: 1, b: 2}; for (var k in o) { } k"), "b");
    assert_eq!(run("for (let k in {a: 1}) { } typeof k"), "undefined");
    assert_eq!(run("for (const k in {a: 1}) { } typeof k"), "undefined");
    // At the top level of a script a `var` is a property of the global object rather than a slot,
    // and the head has to write to the same place the hoisting made — which is the bug this row
    // caught: scoped to the loop, it died with the loop and read back `undefined`.
    assert_eq!(run("for (var k in {a: 1}) { } k"), "a");
    assert_eq!(
        run("function f() { for (var k in {z: 1}) { } return k; } f()"),
        "z"
    );
    // A lexical head is a binding of its own, so it shadows rather than assigns.
    assert_eq!(run("let k = 'kept'; for (let k in {a: 1}) { } k"), "kept");
    // …and a `const` head is written once per pass by the loop and refuses anything else.
    assert_eq!(
        run(
            "var r = ''; for (const k in {a: 1, b: 2}) { try { k = 'x'; } catch (e) { r = r + e.name; } } r"
        ),
        "TypeErrorTypeError"
    );
}

#[test]
fn nothing_is_enumerated_over_undefined_or_null() {
    // §14.7.5.6 step 2 — not an error, and not an empty object either: the loop simply does not
    // run, which is the one place `for`-`in` differs from every other operation on `null`.
    assert_eq!(
        run("var r = 'no'; for (var k in null) { r = 'ran'; } r"),
        "no"
    );
    assert_eq!(
        run("var r = 'no'; for (var k in undefined) { r = 'ran'; } r"),
        "no"
    );
    // A primitive that is not nullish is **wrapped** by step 6's `ToObject` and then walked like
    // any other object. A Number's wrapper and a Boolean's have no enumerable own properties, so
    // these two enumerate nothing — which is the answer either way and is why they held while the
    // wrapping was missing entirely.
    assert_eq!(run("var r = 'no'; for (var k in 1) { r = 'ran'; } r"), "no");
    assert_eq!(
        run("var r = 'no'; for (var k in true) { r = 'ran'; } r"),
        "no"
    );
    // **A String's wrapper has one per index** (§10.4.3), and that is the row the two above cannot
    // stand in for: without the wrapping a primitive string enumerated nothing at all, and the
    // comment here said a wrapper never has any.
    assert_eq!(
        run("var r = ''; for (var k in 'ab') { r = r + k; } r"),
        "01"
    );
    assert_eq!(run("var r = ''; for (var k in '') { r = r + k; } r"), "");
    // …and by code *unit*, so an astral character is two indices exactly as `.length` says.
    assert_eq!(
        run("var r = ''; for (var k in String.fromCharCode(0xD83D, 0xDE00)) { r = r + k; } r"),
        "01"
    );
    // The **prototype** is walked too, which is the half a wrapper with no own properties still
    // shows: an enumerable property put on a built-in's prototype is what a `for`-`in` over a
    // primitive of that type is for, and it was invisible for every one of the five.
    for (spelling, primitive) in [
        ("Number", "1"),
        ("Boolean", "true"),
        ("String", "'ab'"),
        ("Symbol", "Symbol()"),
        ("BigInt", "1n"),
    ] {
        assert_eq!(
            run(&format!(
                "{spelling}.prototype.zz = 1; var r = '';                  for (var k in {primitive}) {{ if (k === 'zz') r = 'seen'; }}                  delete {spelling}.prototype.zz; r"
            )),
            "seen",
            "{spelling}"
        );
    }
}

#[test]
fn break_and_continue_and_nesting_work_as_they_do_in_any_loop() {
    assert_eq!(
        run(
            "var r = ''; for (var k in {a: 1, b: 2, c: 3}) { if (k === 'b') { continue; } r = r + k; } r"
        ),
        "ac"
    );
    assert_eq!(
        run(
            "var r = ''; for (var k in {a: 1, b: 2, c: 3}) { if (k === 'b') { break; } r = r + k; } r"
        ),
        "a"
    );
    assert_eq!(
        run("var r = ''; for (var k in {a: 1}) { for (var j in {b: 2}) { r = r + k + j; } } r"),
        "ab"
    );
    // A body that is not a block, which is the shape the grammar allows and the compiler has to
    // take as one statement.
    assert_eq!(
        run("var r = ''; for (var k in {a: 1, b: 2}) r = r + k; r"),
        "ab"
    );
    // A throw out of the body leaves the loop like any other.
    assert_eq!(
        run(
            "var r = ''; try { for (var k in {a: 1, b: 2}) { r = r + k; throw 1; } } catch (e) { r } "
        ),
        "a"
    );
}

#[test]
fn a_lexical_for_head_puts_its_own_names_in_a_dead_zone_and_a_var_head_does_not() {
    // §14.7.5.6 `ForIn/OfHeadEvaluation` step 2 — a `let` or `const` head binds its names *around
    // the expression it iterates*, uninitialised. So the inner name shadows the outer one before
    // the object is built, and reading it there is the temporal dead zone like any other.
    assert_eq!(
        run("let x = 'outer'; try { for (let x in { [x]: 1 }) {} } catch (e) { e.name }"),
        "ReferenceError"
    );
    assert_eq!(
        run("let x = 'outer'; try { for (const x of [x]) {} } catch (e) { e.name }"),
        "ReferenceError"
    );

    // Step 2's condition is `TDZnames is not an empty List`, and a `var` head binds none of them —
    // its name belongs to the enclosing variable scope and was created long before. So the head
    // expression reads the *outer* binding, and there is nothing uninitialised to trip over.
    assert_eq!(
        run("var x = 'outer'; var seen = ''; for (var x in { [x]: 1 }) { seen = x; } seen"),
        "outer"
    );
    // …and neither does a head that declares nothing at all.
    assert_eq!(
        run("var x = 'outer'; var seen = ''; for (x in { [x]: 1 }) { seen = x; } seen"),
        "outer"
    );

    // The environment is gone before the loop starts: §14.7.5.7's per-pass binding is made from the
    // scope *outside* it, so the body still sees its own name and a closure still keeps its pass.
    assert_eq!(
        run("let f = []; for (let x of [1, 2]) { f.push(() => x); } f[0]() + ',' + f[1]()"),
        "1,2"
    );
}
