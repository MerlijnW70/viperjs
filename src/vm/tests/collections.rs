//! §24.1 and §24.2 — `Map` and `Set`.
//!
//! Two things carry most of the weight here and neither is about storing values: the equality is
//! `SameValueZero` rather than `===`, and a deletion leaves a *hole* so that an iterator part-way
//! through does not skip. Both are invisible until a program does the thing they are for.

use super::*;

#[test]
fn a_map_keeps_what_it_is_given_and_answers_for_what_it_was_not() {
    assert_eq!(run("var m = new Map(); m.set('a', 1); m.get('a')"), "1");
    assert_eq!(run("new Map([[1, 'a'], [2, 'b']]).size"), "2");
    assert_eq!(run("new Map([[1, 'a']]).get(1)"), "a");
    // §24.1.3.6 step 5 — a key that is not there answers `undefined`, which is deliberately the
    // same answer a key mapped *to* `undefined` gives. `has` is the question that tells them apart.
    assert_eq!(run("new Map().get('nothing')"), "undefined");
    assert_eq!(
        run("var m = new Map(); m.set('k', undefined); m.get('k') + ',' + m.has('k')"),
        "undefined,true"
    );
    // §24.1.3.9 step 8 — `set` answers the *map*, which is what makes it chainable.
    assert_eq!(
        run("var m = new Map(); (m.set('a', 1) === m) + ',' + m.set('a', 1).set('b', 2).size"),
        "true,2"
    );
    // …and `clear` answers `undefined` rather than the map, which is the one that is not.
    assert_eq!(
        run("var m = new Map([['a', 1]]); (m.clear() === undefined) + ',' + m.size"),
        "true,0"
    );
    // §24.1.3.3 — `delete` says whether there was anything to delete.
    assert_eq!(
        run("var m = new Map([['a', 1]]); m.delete('a') + ',' + m.delete('a') + ',' + m.size"),
        "true,false,0"
    );
}

#[test]
fn a_key_is_found_by_same_value_zero_and_not_by_strict_equality() {
    // §24.1.3.9 uses §7.2.11, which differs from `===` in one place and from `Object.is` in
    // another. **`NaN` matches itself**, so a map may be keyed by it — `===` would make that key
    // unreachable the moment it was stored.
    assert_eq!(
        run("var m = new Map(); m.set(NaN, 'found'); m.get(NaN) + ',' + m.has(NaN)"),
        "found,true"
    );
    assert_eq!(run("var s = new Set([NaN, NaN]); s.size"), "1");
    // …and the two zeroes are **one** key, where `Object.is` would make them two.
    assert_eq!(
        run("var m = new Map(); m.set(0, 'z'); m.get(-0) + ',' + m.size"),
        "z,1"
    );
    // §24.1.3.9 step 6 — `-0` is normalised to `+0` on the way in, so what comes back out of an
    // iterator is `+0`. `1 / key` is the only way to ask which zero it is.
    assert_eq!(
        run("var m = new Map(); m.set(-0, 'z'); 1 / Array.from(m.keys())[0]"),
        "Infinity"
    );
    // Everything else is identity, which is what separates a `Map` from an object: two objects
    // that look alike are two keys, and a String key is its *contents*.
    assert_eq!(
        run("var m = new Map(); m.set({}, 1); m.get({})"),
        "undefined"
    );
    assert_eq!(
        run("var k = {}; var m = new Map(); m.set(k, 1); m.get(k)"),
        "1"
    );
    assert_eq!(
        run("var m = new Map(); m.set('abc', 1); m.get('ab' + 'c')"),
        "1"
    );
    assert_eq!(
        run("var m = new Map(); m.set(1, 'n'); m.get('1')"),
        "undefined"
    );
}

#[test]
fn iteration_is_in_first_insertion_order_and_sees_what_changes_beneath_it() {
    // §24.1.3.9 step 4.a.i replaces a value **in place**, so re-setting a key does not move it to
    // the end: the order is *first* insertion order and not most-recently-written order.
    assert_eq!(
        run("var m = new Map([['a', 1], ['b', 2]]); m.set('a', 9); \
             Array.from(m.entries()).map(function (e) { return e.join(':'); }).join('|')"),
        "a:9|b:2"
    );
    // §24.1.5.1 — an iterator remembers a *position*, so an entry added while one is running is
    // visited. This is the row that says the entries are a growing list rather than a snapshot.
    assert_eq!(
        run(
            "var m = new Map([['a', 1]]); var it = m.entries(); m.set('b', 2); \
             it.next().value.join(':') + '|' + it.next().value.join(':') + '|' + it.next().done"
        ),
        "a:1|b:2|true"
    );
    // …and one *deleted* before it is reached is passed over rather than skipping a neighbour,
    // which is what the hole a delete leaves is for. Removing the entry outright would shift `c`
    // into `b`'s place and the iterator would never see it.
    assert_eq!(
        run(
            "var m = new Map([['a', 1], ['b', 2], ['c', 3]]); var it = m.keys(); \
             it.next(); m.delete('b'); it.next().value + ',' + it.next().done"
        ),
        "c,true"
    );
    // An iterator that has run out stays run out, whatever is added afterwards — the same rule an
    // Array Iterator has, and for the same reason.
    assert_eq!(
        run(
            "var m = new Map([['a', 1]]); var it = m.keys(); it.next(); it.next(); \
             m.set('b', 2); it.next().done"
        ),
        "true"
    );
    // `clear` empties it for an iterator too: a cleared collection has nothing more to give even
    // though its list is as long as it was.
    assert_eq!(
        run("var m = new Map([['a', 1], ['b', 2]]); var it = m.keys(); m.clear(); it.next().done"),
        "true"
    );
}

#[test]
fn a_set_is_a_map_whose_value_is_its_key() {
    assert_eq!(run("new Set([1, 2, 2, 3]).size"), "3");
    assert_eq!(run("Array.from(new Set([3, 1, 3, 2])).join(',')"), "3,1,2");
    assert_eq!(
        run("var s = new Set(); (s.add(1) === s) + ',' + s.has(1) + ',' + s.has(2)"),
        "true,true,false"
    );
    // §24.2.3.8 — `keys` and `values` are the **same function object**, which a program can see.
    // It follows from a Set's key being its value, and the specification says so outright.
    assert_eq!(run("Set.prototype.keys === Set.prototype.values"), "true");
    // …and `entries` answers `[v, v]`, a shape that looks like a mistake and is what lets a `Set`
    // be walked by anything written for a `Map`.
    assert_eq!(
        run("Array.from(new Set(['x']).entries())[0].join(':')"),
        "x:x"
    );
    // §24.2.3.6 hands the callback the value **twice** and then the set, for the same reason.
    assert_eq!(
        run(
            "var out = ''; new Set([1, 2]).forEach(function (v, k, s) { \
             out += v + '/' + k + '/' + (s instanceof Set) + ';'; }); out"
        ),
        "1/1/true;2/2/true;"
    );
    // A String is iterable, so `new Set('hello')` is its distinct characters rather than one entry.
    assert_eq!(run("Array.from(new Set('hello')).join('')"), "helo");
}

#[test]
fn for_of_walks_a_map_by_entries_and_a_set_by_values() {
    // §24.1.3.12 and §24.2.3.11 — `[@@iterator]` is the *same function object* as `entries` for a
    // Map and as `values` for a Set, which is what decides what `for`-`of` sees.
    assert_eq!(
        run("Map.prototype[Symbol.iterator] === Map.prototype.entries"),
        "true"
    );
    assert_eq!(
        run("Set.prototype[Symbol.iterator] === Set.prototype.values"),
        "true"
    );
    assert_eq!(
        run(
            "var out = ''; for (var e of new Map([['a', 1], ['b', 2]])) { out += e[0] + e[1]; } out"
        ),
        "a1b2"
    );
    assert_eq!(
        run("var out = ''; for (var v of new Set([1, 2, 3])) { out += v; } out"),
        "123"
    );
    // …and destructuring a Map's entries in the head, which is the shape most code uses.
    assert_eq!(
        run("var out = ''; for (var [k, v] of new Map([['a', 1]])) { out += k + '=' + v; } out"),
        "a=1"
    );
}

#[test]
fn a_collection_method_asks_about_the_internal_slot_and_not_about_a_shape() {
    // Every one of §24's methods begins by checking `[[MapData]]` or `[[SetData]]`, so borrowing a
    // method onto an object that merely looks like a Map is a TypeError rather than an answer. An
    // implementation that checked the prototype instead would let a plain object be a Map.
    assert_eq!(
        run("try { Map.prototype.get.call({}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Set.prototype.add.call(new Map(), 1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a Map's methods do work on a Map, however it was reached.
    assert_eq!(
        run("var m = new Map([['a', 1]]); Map.prototype.get.call(m, 'a')"),
        "1"
    );
    // §24.1.1.1 step 1 — a plain call has no `new.target` to take a prototype from.
    assert_eq!(
        run("try { Map(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Set(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §24.1.1.2 — each element of a Map's iterable must be an object, because it is read by `0`
    // and `1`. A primitive there is a TypeError rather than an entry full of `undefined`.
    assert_eq!(
        run("try { new Map([1, 2]); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // `undefined` and `null` both mean "no iterable" and make an empty one; anything else is
    // iterated, so a number is a TypeError.
    assert_eq!(
        run("new Map(undefined).size + ',' + new Set(null).size"),
        "0,0"
    );
    assert_eq!(
        run("try { new Set(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_shape_of_the_two_constructors_is_what_the_specification_names() {
    assert_eq!(run("typeof Map + ',' + typeof Set"), "function,function");
    // §24.1.2 — `length` is **0** for both, because the iterable is optional.
    assert_eq!(run("Map.length + ',' + Set.length"), "0,0");
    assert_eq!(run("Map.name + ',' + Set.name"), "Map,Set");
    assert_eq!(
        run("Map.prototype.constructor === Map && Set.prototype.constructor === Set"),
        "true"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Map()) + ',' \
             + Object.prototype.toString.call(new Set())"),
        "[object Map],[object Set]"
    );
    // §24.1.3.10 — `size` is an **accessor**, so it cannot be assigned and always reads what the
    // collection currently holds. A data property would let a program lie about it.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Map.prototype, 'size'); \
             (typeof d.get) + ',' + (d.value === undefined) + ',' + d.enumerable"
        ),
        "function,true,false"
    );
    assert_eq!(run("var m = new Map(); m.size = 99; m.size"), "0");
    // §24.1.5 — a Map Iterator inherits from %IteratorPrototype%, which is what makes the iterator
    // itself iterable and so usable in a second `for`-`of`.
    assert_eq!(
        run("var it = new Map().entries(); \
             (it[Symbol.iterator]() === it) + ',' + Object.prototype.toString.call(it)"),
        "true,[object Map Iterator]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Set().values())"),
        "[object Set Iterator]"
    );
    // A subclass gets its own prototype, because the constructor takes it from `new.target`.
    assert_eq!(
        run("class M extends Map {} var m = new M([['a', 1]]); \
             (m instanceof M) + ',' + (m instanceof Map) + ',' + m.get('a')"),
        "true,true,1"
    );
    // …and the constructor calls the *adder off the object*, so a subclass that overrode `set`
    // sees every entry of the iterable go through its own method.
    assert_eq!(
        run(
            "var seen = 0; class M extends Map { set(k, v) { seen++; return super.set(k, v); } } \
             new M([['a', 1], ['b', 2]]); seen"
        ),
        "2"
    );
}

#[test]
fn the_symbol_properties_and_the_size_accessor_have_the_attributes_the_clause_gives_them() {
    // §17's convention for a built-in method is writable, not enumerable, configurable — and two of
    // these are not that. A tag is **not writable** (§24.1.3.13) where `[@@iterator]` is, and the
    // accessor is neither. Each is a thing a program can detect and a polyfill reads.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Map.prototype, Symbol.iterator);              d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,true"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Map.prototype, Symbol.toStringTag);              d.value + ',' + d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "Map,false,false,true"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Set.prototype, 'size');              (d.set === undefined) + ',' + d.enumerable + ',' + d.configurable"
        ),
        "true,false,true"
    );
    // Configurable is what makes each of them replaceable, which is the only reason a specification
    // ever says so: taking the tag off changes what `Object.prototype.toString` answers.
    assert_eq!(
        run("delete Set.prototype[Symbol.toStringTag]; Object.prototype.toString.call(new Set())"),
        "[object Object]"
    );
}

#[test]
fn what_must_be_callable_is_checked_before_it_is_used() {
    // §24.1.1.1 step 7 — the adder is read off the object and must be a function *before* the
    // iterable is walked, so a subclass that broke `set` is refused rather than half-filled.
    assert_eq!(
        run(
            "class M extends Map {} M.prototype.set = 1;              try { new M([['a', 1]]); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "class S extends Set {} S.prototype.add = null;              try { new S([1]); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and an empty iterable never reaches the adder, so the check is about the *method* and not
    // about whether anything would have been added.
    assert_eq!(
        run(
            "class M extends Map {} M.prototype.set = 1;              try { new M([]); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // §24.1.3.5 step 3 — `forEach` refuses a callback that is not a function, before it walks.
    assert_eq!(
        run("try { new Map([['a', 1]]).forEach(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { new Set([1]).forEach(undefined); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and on an **empty** collection, where nothing would have been called and so nothing else
    // would have complained. That is the row the check exists for: `forEach` refuses a bad callback
    // whether or not it would have reached it, so a program finds out on the pass where the
    // collection happens to be empty rather than on some later one where it is not.
    assert_eq!(
        run("try { new Map().forEach(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { new Set().forEach('not a function'); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn only_the_negative_zero_is_normalised_and_no_other_negative_number_is() {
    // §24.1.3.9 step 6 rewrites `-0` to `+0` and nothing else. A guard that read "negative" rather
    // than "negative *zero*" would fold every negative key onto `+0`, so a map keyed by `-5` would
    // answer for `0` and lose `-5` — which is the kind of wrong answer that looks like a working
    // program until someone stores two negative numbers.
    assert_eq!(
        run(
            "var m = new Map(); m.set(-5, 'five'); m.set(-3, 'three');              m.get(-5) + ',' + m.get(-3) + ',' + m.size + ',' + m.get(0)"
        ),
        "five,three,2,undefined"
    );
    // …and `-0` really is folded, which is the other half of the same line.
    assert_eq!(
        run(
            "var m = new Map(); m.set(-0, 'zero'); m.set(0, 'again');              m.size + ',' + m.get(-0) + ',' + (1 / Array.from(m.keys())[0])"
        ),
        "1,again,Infinity"
    );
}
