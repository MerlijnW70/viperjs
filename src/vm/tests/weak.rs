//! §24.3's `WeakMap` and §24.4's `WeakSet` as a script sees them.
//!
//! Which is: not weakly at all. Everything weakness does is done by the collector, and a program
//! has no way to ask whether it ran — so these rows are about the four methods, the keys they
//! refuse, and the brand that keeps them apart from `Map` and `Set`. The rows about weakness
//! itself are in `heap::collect`, where the roots can be named by hand.

use super::*;

#[test]
fn a_weak_map_stores_and_finds_a_key_it_does_not_keep_alive() {
    // §24.3.3.3 and §24.3.3.4 — an ordinary insert and lookup, and identity is what decides:
    // a second object that looks the same is a different key.
    assert_eq!(
        run("var k = {}; var m = new WeakMap(); m.set(k, 1); \
             m.get(k) + ',' + m.has(k) + ',' + m.get({}) + ',' + m.has({})"),
        "1,true,undefined,false"
    );
    // §24.3.1.1's iterable is a list of two-element entries, read through `set` — so the same
    // arrangement written by hand and written by the constructor agree.
    assert_eq!(
        run(
            "var k = {}; var j = {}; var m = new WeakMap([[k, 'a'], [j, 'b']]); \
             m.get(k) + m.get(j)"
        ),
        "ab"
    );
    // §24.3.3.4 step 6 — `set` answers the map, so it chains.
    assert_eq!(
        run("var k = {}; var m = new WeakMap(); (m.set(k, 1) === m) + ',' + m.get(k)"),
        "true,1"
    );
    // Re-setting a key replaces the value rather than adding a second entry, which `get` shows
    // and `delete` confirms: one delete is enough to remove it.
    assert_eq!(
        run(
            "var k = {}; var m = new WeakMap(); m.set(k, 1); m.set(k, 2); \
             m.get(k) + ',' + m.delete(k) + ',' + m.has(k)"
        ),
        "2,true,false"
    );
    // §24.3.3.1 — `delete` answers whether there was anything to delete, so a second one is false.
    assert_eq!(
        run("var k = {}; var m = new WeakMap(); m.set(k, 1); \
             m.delete(k) + ',' + m.delete(k) + ',' + m.delete({})"),
        "true,false,false"
    );
    // §24.4.3 — the same three questions with the value as its own key.
    assert_eq!(
        run("var k = {}; var w = new WeakSet([k]); \
             w.has(k) + ',' + w.has({}) + ',' + (w.add(k) === w) + ',' + w.delete(k) + ',' + w.has(k)"),
        "true,false,true,true,false"
    );
    // Neither takes an iterable it must have: `undefined` and `null` both mean "empty".
    assert_eq!(
        run(
            "var k = {}; [new WeakMap(), new WeakMap(undefined), new WeakMap(null)] \
             .map(function (m) { return m.has(k); }).join(',')"
        ),
        "false,false,false"
    );
}

#[test]
fn a_key_that_could_never_be_collected_is_refused_where_it_is_stored() {
    // §7.2.10 `CanBeHeldWeakly` — an Object, or a Symbol that is not in §20.4.2.2's registry.
    // A primitive has no identity to hold and a registered Symbol is held by the registry for the
    // life of the realm, so an entry keyed by one could never go away: it would be a leak wearing
    // a weak map's name.
    for bad in ["1", "'s'", "true", "null", "undefined", "Symbol.for('r')"] {
        assert_eq!(
            run(&format!(
                "try {{ new WeakMap().set({bad}, 1); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{bad} cannot be held weakly"
        );
        assert_eq!(
            run(&format!(
                "try {{ new WeakSet().add({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{bad} cannot be held weakly"
        );
    }
    // An *unregistered* Symbol can, which is the whole reason the rule is about the registry
    // rather than about Symbols.
    assert_eq!(
        run("var s = Symbol('s'); var m = new WeakMap(); m.set(s, 1); \
             m.get(s) + ',' + m.has(s) + ',' + new WeakSet([s]).has(s)"),
        "1,true,true"
    );
    // …and the same Symbol asked for twice through the registry is one Symbol, which is exactly
    // why it is refused: `Symbol.for('r') === Symbol.for('r')`.
    assert_eq!(run("Symbol.for('r') === Symbol.for('r')"), "true");
    // Looking one up is *not* an error, though storing one is. §24.3.3.3 has no check at all: a
    // key that could never have been stored simply is not there, and that is an answer rather
    // than a mistake.
    assert_eq!(
        run(
            "var m = new WeakMap(); m.get(1) + ',' + m.has(1) + ',' + m.delete(1) \
             + ',' + new WeakSet().has('s') + ',' + new WeakSet().delete(null)"
        ),
        "undefined,false,false,false,false"
    );
}

#[test]
fn a_weak_collection_offers_nothing_that_would_show_when_the_collector_ran() {
    // §24.3.3 lists four methods and §24.4.3 lists three, and what is *absent* is the design:
    // `size`, `clear`, `forEach` and an iterator would each let a program watch entries vanish,
    // and a program that can see that can tell two runs of the same code apart.
    assert_eq!(
        run(
            "[WeakMap.prototype.size, WeakMap.prototype.clear, WeakMap.prototype.forEach, \
             WeakMap.prototype.keys, WeakMap.prototype[Symbol.iterator], \
             WeakSet.prototype.size, WeakSet.prototype.forEach] \
             .map(function (m) { return typeof m; }).join(',')"
        ),
        "undefined,undefined,undefined,undefined,undefined,undefined,undefined"
    );
    assert_eq!(
        run("try { for (var e of new WeakSet()) { } } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §17's `[@@toStringTag]`, which is what `Object.prototype.toString` reads.
    assert_eq!(
        run("Object.prototype.toString.call(new WeakMap()) + ' ' \
             + Object.prototype.toString.call(new WeakSet())"),
        "[object WeakMap] [object WeakSet]"
    );
    // §10.3.3's `name` and `length` — `set` writes two arguments, `add` writes one, and a
    // constructor's `length` is 0 because its iterable is optional.
    assert_eq!(
        run(
            "WeakMap.length + ',' + WeakSet.length + ',' + WeakMap.prototype.set.length \
             + ',' + WeakSet.prototype.add.length + ',' + WeakMap.name + ',' + WeakSet.name"
        ),
        "0,0,2,1,WeakMap,WeakSet"
    );
}

#[test]
fn a_weak_collection_is_a_different_brand_from_the_strong_one_with_the_same_shape() {
    // §24.3.3.3 requires a `[[WeakMapData]]` where §24.1.3.6 requires a `[[MapData]]`, so neither
    // family's methods answer questions about the other — however alike the two read underneath.
    // An engine that checked only "is it some collection" would let each answer for the other.
    for borrowed in [
        "Map.prototype.get.call(new WeakMap())",
        "Map.prototype.has.call(new WeakMap())",
        "WeakMap.prototype.get.call(new Map())",
        "WeakMap.prototype.set.call(new Map(), {}, 1)",
        "Set.prototype.has.call(new WeakSet())",
        "WeakSet.prototype.add.call(new Set(), {})",
        "WeakSet.prototype.has.call(new WeakMap())",
        "WeakMap.prototype.get.call(new WeakSet())",
        "WeakMap.prototype.get.call({})",
        "WeakMap.prototype.get.call(1)",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {borrowed}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{borrowed}"
        );
    }
    // §24.3.1.1 step 1 — a plain call has no `new.target` to take a prototype from.
    for constructor in ["WeakMap", "WeakSet"] {
        assert_eq!(
            run(&format!(
                "try {{ {constructor}(); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{constructor} without new"
        );
    }
    // …and a subclass gets its own prototype, because §24.3.1.1 reads it from `new.target`.
    assert_eq!(
        run(
            "class W extends WeakMap {} var w = new W(); var k = {}; w.set(k, 1); \
             (w instanceof W) + ',' + (w instanceof WeakMap) + ',' + w.get(k)"
        ),
        "true,true,1"
    );
    // §24.3.1.1 step 7 reads the adder *through the property*, so a subclass that overrode `set`
    // has its own called for each entry of the iterable.
    assert_eq!(
        run("var seen = 0; \
             class W extends WeakMap { set(k, v) { seen++; return super.set(k, v); } } \
             var k = {}; var w = new W([[k, 1]]); seen + ',' + w.get(k)"),
        "1,1"
    );
    // §24.3.1.1 — an entry of a WeakMap's iterable that is not an object is a TypeError rather
    // than an entry holding `undefined`.
    assert_eq!(
        run("try { new WeakMap([1]); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and because the adder is read through the property, a subclass can make it something that
    // is not callable at all. Step 7 checks that *before* the iterable is walked, so this is a
    // TypeError about the adder rather than whatever calling a number would have said.
    assert_eq!(
        run("class W extends WeakMap {} W.prototype.set = 1; \
             try { new W([[{}, 1]]); } catch (e) { e.constructor.name + ':' + e.message }"),
        "TypeError:the collection's adder is not a function"
    );
    assert_eq!(
        run("class W extends WeakSet {} W.prototype.add = undefined; \
             try { new W([{}]); } catch (e) { e.message }"),
        "the collection's adder is not a function"
    );
    // The check runs even when the iterable is empty, which is what says it happens before the
    // walk rather than at the first element.
    assert_eq!(
        run("class W extends WeakMap {} W.prototype.set = 1; \
             try { new W([]); } catch (e) { e.message }"),
        "the collection's adder is not a function"
    );
}
