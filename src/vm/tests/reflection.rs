//! §28.1 — `Reflect`, which is every internal method with a name.
//!
//! Two things separate it from `Object`, and both are why it exists: these answer the **Boolean**
//! the internal method returned instead of throwing, and they require an object rather than
//! converting one. The third is `get` and `set`, which take a receiver no other clause offers.

use super::*;

#[test]
fn reflect_is_an_ordinary_object_and_not_a_constructor() {
    // §28.1 — like `Math` and `JSON`: an object with functions on it. `new Reflect` is a TypeError
    // for the dull reason that it has no `[[Construct]]`.
    assert_eq!(run("typeof Reflect"), "object");
    assert_eq!(
        run("Object.prototype.toString.call(Reflect)"),
        "[object Reflect]"
    );
    assert_eq!(
        run("try { new Reflect(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run(
            "Reflect.get.length + ',' + Reflect.set.length + ',' + Reflect.apply.length \
             + ',' + Reflect.construct.length + ',' + Reflect.ownKeys.length"
        ),
        "2,3,3,2,1"
    );
}

#[test]
fn every_one_of_them_requires_an_object_where_object_would_convert_one() {
    // The difference from §20.1.2: these *are* the internal methods, and an internal method is
    // something an object has. `Object.keys("ab")` converts and answers `["0", "1"]`; the same
    // question asked of `Reflect` is refused, because a String has no `[[OwnPropertyKeys]]`.
    assert_eq!(run("Object.keys('ab').join(',')"), "0,1");
    for source in [
        "Reflect.ownKeys('ab')",
        "Reflect.get('ab', 0)",
        "Reflect.has(1, 'x')",
        "Reflect.getPrototypeOf('ab')",
        "Reflect.isExtensible(null)",
        "Reflect.preventExtensions(undefined)",
        "Reflect.deleteProperty(1, 'x')",
        "Reflect.setPrototypeOf('ab', null)",
        "Reflect.defineProperty(1, 'x', {})",
        "Reflect.getOwnPropertyDescriptor('ab', 0)",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source}"
        );
    }
    // `apply` and `construct` ask a different question — callable and constructor — and refuse
    // for that reason rather than for not being an object.
    // The *message* is what this row is about: without the check the call below fails anyway, and
    // says "what was called is not a function" — true, and unhelpful when the thing handed over
    // was a function often enough that the useful sentence names which argument was wrong.
    assert_eq!(
        run(
            "try { Reflect.apply({}, null, []); } catch (e) { e.constructor.name + ': ' + e.message }"
        ),
        "TypeError: Reflect.apply needs a function"
    );
    assert_eq!(
        run("try { Reflect.construct(function () {}.bind, []); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn five_of_them_answer_a_boolean_where_object_throws_or_answers_the_object() {
    // The reason `Reflect` is usable in a program that has to cope, and the reason a `Proxy`
    // handler can be written by hand: the handlers are specified to answer Booleans, so the
    // operations they wrap have to as well.
    assert_eq!(
        run("var o = {}; Reflect.set(o, 'a', 1) + ',' + o.a"),
        "true,1"
    );
    assert_eq!(
        run("var frozen = Object.freeze({ a: 1 }); Reflect.set(frozen, 'a', 2) + ',' + frozen.a"),
        "false,1"
    );
    assert_eq!(
        run("Reflect.defineProperty(Object.preventExtensions({}), 'x', { value: 1 })"),
        "false"
    );
    assert_eq!(
        run("var o = {}; Reflect.defineProperty(o, 'x', { value: 1 }) + ',' + o.x"),
        "true,1"
    );
    // …where `Object.defineProperty` throws for the same refusal, which is what a caller would
    // otherwise have to write a `try` around to find out.
    assert_eq!(
        run(
            "try { Object.defineProperty(Object.preventExtensions({}), 'x', { value: 1 }); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run("var o = { a: 1 }; Reflect.deleteProperty(o, 'a') + ',' \
             + Reflect.deleteProperty(Object.freeze({ b: 1 }), 'b')"),
        "true,false"
    );
    assert_eq!(
        run("var o = {}; Reflect.setPrototypeOf(o, null) + ',' \
             + Reflect.setPrototypeOf(Object.preventExtensions({}), {})"),
        "true,false"
    );
    // `preventExtensions` answers `true` rather than the object, which is the other half of the
    // same idea — nothing here answers a value a caller has to interpret.
    assert_eq!(
        run("var o = {}; Reflect.preventExtensions(o) + ',' + Reflect.isExtensible(o)"),
        "true,false"
    );
}

#[test]
fn get_and_set_take_a_receiver_that_no_other_clause_offers() {
    // §10.1.8.1 hands a getter the object the read went *through*, and until now that was always
    // the object being read. `Reflect.get` lets a program name it separately, which is what makes
    // a `Proxy`'s trap able to forward to its target without telling the getter that the target is
    // what the program asked about.
    assert_eq!(
        run("var p = { get x() { return this.tag; } }; Reflect.get(p, 'x', { tag: 'given' })"),
        "given"
    );
    // Absent, it is the target — which is what an ordinary read does.
    assert_eq!(
        run("var p = { tag: 'own', get x() { return this.tag; } }; Reflect.get(p, 'x')"),
        "own"
    );
    // For `set` the receiver does two things: a setter is called with it, and a property that
    // shadows an inherited one is created **on it**. The second is what makes a forwarded write
    // land where the program asked rather than where the property was looked up.
    assert_eq!(
        run("var seen; var p = { set x(v) { seen = this.tag; } }; \
             Reflect.set(p, 'x', 1, { tag: 'given' }); seen"),
        "given"
    );
    assert_eq!(
        run(
            "var target = {}; var landing = {}; Reflect.set(target, 'a', 1, landing); \
             landing.a + ',' + (target.a === undefined)"
        ),
        "1,true"
    );
    // §10.1.9.2 step 2.c — what the *receiver* already has decides. An accessor there, or a
    // property that is not writable, refuses the write outright: the value came looking for a home
    // and that one is taken. Neither is about the target, which in both rows is perfectly writable.
    assert_eq!(
        run(
            "var landing = Object.freeze({ a: 0 });              Reflect.set({ a: 1 }, 'a', 2, landing) + ',' + landing.a"
        ),
        "false,0"
    );
    assert_eq!(
        run(
            "var landing = { get a() { return 9; } };              Reflect.set({ a: 1 }, 'a', 2, landing) + ',' + landing.a"
        ),
        "false,9"
    );
    // …and one the receiver already has and *is* writable is updated, keeping its attributes. So a
    // write through a receiver never makes a property enumerable that was not, which is the same
    // promise an ordinary assignment makes.
    assert_eq!(
        run("var landing = { a: 0 }; Reflect.set({ a: 1 }, 'a', 2, landing) + ',' + landing.a"),
        "true,2"
    );
    assert_eq!(
        run(
            "var landing = {}; Object.defineProperty(landing, 'a',              { value: 0, writable: true, enumerable: false, configurable: true });              Reflect.set({ a: 1 }, 'a', 2, landing) + ',' + landing.a + ','              + Object.keys(landing).length"
        ),
        "true,2,0"
    );
    // Step 2.b — a receiver that is not an object at all has nowhere to put anything, so the write
    // is refused rather than thrown away silently.
    assert_eq!(run("Reflect.set({}, 'a', 1, 1)"), "false");
    assert_eq!(run("Reflect.set({}, 'a', 1, null)"), "false");
    // …and an inherited *writable* property is shadowed on the receiver rather than changed,
    // which is the ordinary rule seen through the same lens.
    assert_eq!(
        run(
            "var base = { a: 1 }; var landing = {}; Reflect.set(base, 'a', 2, landing); \
             base.a + ',' + landing.a"
        ),
        "1,2"
    );
}

#[test]
fn construct_may_name_a_new_target_that_is_not_the_constructor() {
    // §28.1.2 — the only way in the language to say it. `new X()` always passes `X`, so what this
    // buys is an object built by one constructor and inheriting from another's `prototype`: the
    // shape a subclass factory needs, and what `Reflect.construct` exists for.
    assert_eq!(
        run(
            "function F() {} function G() {} G.prototype = { tag: 'G' }; \
             Reflect.construct(F, [], G).tag"
        ),
        "G"
    );
    assert_eq!(
        run("class A { constructor() { this.a = 1; } } class B {} \
             var o = Reflect.construct(A, [], B); \
             o.a + ',' + (o instanceof B) + ',' + (o instanceof A)"),
        "1,true,false"
    );
    // A built-in reads it too, so an Array really can be made to inherit from a subclass.
    assert_eq!(
        run(
            "class D extends Array {} var made = Reflect.construct(Array, [3], D); \
             made.length + ',' + (made instanceof D)"
        ),
        "3,true"
    );
    // Absent, it is the target — the ordinary `new`.
    assert_eq!(
        run(
            "function F(a, b) { this.s = a + b; } var o = Reflect.construct(F, [1, 2]); \
             o.s + ',' + (o instanceof F)"
        ),
        "3,true"
    );
    // A `newTarget` that is not a constructor is refused, because the object could not be made.
    assert_eq!(
        run("try { Reflect.construct(function () {}, [], {}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §28.1.1 and §28.1.2 both use `CreateListFromArrayLike`, which requires an object: this is
    // where they differ from `f.apply(x)`, whose missing list means "no arguments".
    assert_eq!(
        run("try { Reflect.apply(function () {}, null); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Reflect.apply(function () { return this.v + arguments[0]; }, { v: 1 }, [2])"),
        "3"
    );
}

#[test]
fn own_keys_is_the_one_listing_that_hides_nothing() {
    // §20.1.2.17 gives enumerable String keys, `getOwnPropertyNames` gives every String key, and
    // this gives every key there is — Symbols included. It is what a `Proxy`'s `ownKeys` trap has
    // to answer, so it has to be able to say everything.
    assert_eq!(
        run("var s = Symbol('s'); var o = { a: 1 }; o[s] = 2; \
             Object.defineProperty(o, 'hidden', { value: 3 }); \
             Object.keys(o).join(',') + '|' + Object.getOwnPropertyNames(o).join(',') \
             + '|' + Reflect.ownKeys(o).length"),
        "a|a,hidden|3"
    );
    // §10.1.11's order survives: integer indices in ascending order, then Strings in creation
    // order, then Symbols — which is the order every listing uses and the only one that is
    // specified at all.
    assert_eq!(
        run(
            "var o = { b: 1, 2: 1, a: 1, 0: 1 }; o[Symbol.iterator] = 1; \
             Reflect.ownKeys(o).map(String).join(',')"
        ),
        "0,2,b,a,Symbol(Symbol.iterator)"
    );
    assert_eq!(
        run("var found = Reflect.ownKeys({ [Symbol.iterator]: 1 }); \
             found.length + ',' + (typeof found[0])"),
        "1,symbol"
    );
    // §28.1.6 — a descriptor that is not there is `undefined` and not an empty object, which is
    // how a caller tells "absent" from "present and holding undefined".
    assert_eq!(
        run("JSON.stringify(Reflect.getOwnPropertyDescriptor({ a: 1 }, 'a'))"),
        "{\"value\":1,\"writable\":true,\"enumerable\":true,\"configurable\":true}"
    );
    assert_eq!(
        run("Reflect.getOwnPropertyDescriptor({}, 'a')"),
        "undefined"
    );
    // §28.1.7 and §28.1.13 — the prototype, both ways, and `null` is a prototype where
    // `undefined` is not.
    assert_eq!(
        run("Reflect.getPrototypeOf([]) === Array.prototype"),
        "true"
    );
    assert_eq!(run("Reflect.getPrototypeOf(Object.create(null))"), "null");
    assert_eq!(
        run("try { Reflect.setPrototypeOf({}, undefined); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §28.1.8 — `has`, which is `in` with the operands the way round a reader expects.
    assert_eq!(
        run(
            "Reflect.has({ a: 1 }, 'a') + ',' + Reflect.has({}, 'toString') + ',' \
             + Reflect.has({}, 'nothing')"
        ),
        "true,true,false"
    );
}
