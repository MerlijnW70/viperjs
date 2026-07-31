//! §7.3.23 `ArraySpeciesCreate` — what a copying method answers *with*, and who decides.

use super::*;

/// The six §23.1.3 methods that ask the array what kind of thing to make.
const COPYING: [&str; 6] = [
    "map(function (x) { return x; })",
    "filter(function () { return true; })",
    "slice()",
    "concat()",
    "splice(0, 1)",
    "flat()",
];

#[test]
fn a_subclass_of_array_gets_its_own_kind_back() {
    // §7.3.23 step 5 reads `@@species` off the constructor, and §23.1.2.5 makes `Array`'s an
    // accessor answering `this` — so a subclass inherits it and answers *itself*. Without that
    // accessor every one of these is a plain Array and a subclass loses its type on the first
    // method call, silently.
    for method in COPYING {
        assert_eq!(
            run(&format!(
                "class Sub extends Array {{}} var s = new Sub(); s.push([1], [2]); \
                 var answer = s.{method}; \
                 (answer instanceof Sub) + ',' + (answer instanceof Array)"
            )),
            "true,true",
            "{method} should answer a Sub"
        );
    }
    // The accessor itself: an inherited getter, not a data property, and it answers whatever it
    // was read through — which is the whole mechanism.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Array, Symbol.species); \
             (typeof d.get) + ',' + (d.set === undefined) + ',' + d.enumerable + ',' + d.configurable"
        ),
        "function,true,false,true"
    );
    assert_eq!(run("Array[Symbol.species] === Array"), "true");
    assert_eq!(
        run("class Sub extends Array {} Sub[Symbol.species] === Sub"),
        "true"
    );
}

#[test]
fn only_an_array_is_asked_and_the_answer_may_be_overridden() {
    // Step 2 — an array-*like* is never asked, so its `constructor` is not consulted at all. A
    // generic method called on a plain object always answers an ordinary Array, whatever the
    // object claims about itself.
    assert_eq!(
        run(
            "var o = {length: 1, 0: 'a', constructor: function () { throw new Error('asked'); }}; \
             var answer = Array.prototype.map.call(o, function (x) { return x; }); \
             Array.isArray(answer) + ',' + answer.join('')"
        ),
        "true,a"
    );
    // Step 5.b — a species of `null` means "no opinion" and gets an ordinary Array, the same as
    // `undefined`. Two spellings, one answer, and only one of them is the obvious one.
    assert_eq!(
        run(
            "var a = [1]; var C = function () {}; C[Symbol.species] = null; a.constructor = C; \
             Array.isArray(a.map(function (x) { return x; }))"
        ),
        "true"
    );
    assert_eq!(
        run(
            "var a = [1]; var C = function () {}; C[Symbol.species] = undefined; a.constructor = C; \
             Array.isArray(a.map(function (x) { return x; }))"
        ),
        "true"
    );
    // …and a `constructor` of `undefined` never reaches step 5 at all.
    assert_eq!(
        run(
            "var a = [1]; a.constructor = undefined; Array.isArray(a.map(function (x) { return x; }))"
        ),
        "true"
    );
    // An object with no `@@species` of its own is the "no opinion" case too: step 5 reads
    // `undefined` off it and step 6 answers a plain Array. It is *not* a TypeError, which is easy
    // to expect and would be wrong — step 7 is only reached by something that is present and not
    // a constructor.
    assert_eq!(
        run("var a = [1]; a.constructor = {}; Array.isArray(a.map(function (x) { return x; }))"),
        "true"
    );
    // A species that is not a constructor is a TypeError — step 7. `null` as the *constructor*
    // reaches the same step, because step 5 only looks inside an Object and null is not one.
    //
    // The **message** is what these assert, not merely the kind. Handing a number to `Construct`
    // throws a TypeError as well, saying "what was called is not a function" — so step 7's whole
    // observable effect is that the complaint names the species, and a row checking the kind alone
    // cannot tell the check from its absence.
    for bad in ["1", "'C'", "null", "true"] {
        assert_eq!(
            run(&format!(
                "var a = [1]; a.constructor = {bad}; \
                 try {{ a.map(function (x) {{ return x; }}); }} catch (e) {{ e.message }}"
            )),
            "the species of this array is not a constructor",
            "constructor {bad}"
        );
    }
    // …and the same step reached by the other route: an object that *has* an opinion and a
    // useless one.
    assert_eq!(
        run(
            "var a = [1]; a.constructor = {}; a.constructor[Symbol.species] = 1; \
             try { a.map(function (x) { return x; }); } catch (e) { e.message }"
        ),
        "the species of this array is not a constructor"
    );
    // A species that *is* a constructor is used, and it is handed the length — which `map` needs
    // and `filter` does not, so the two are asked for different numbers.
    assert_eq!(
        run(
            "var seen = []; var C = function (n) { seen.push(n); this.length = 0; }; \
             var a = [1, 2, 3]; a.constructor = {}; a.constructor[Symbol.species] = C; \
             a.map(function (x) { return x; }); a.filter(function () { return true; }); \
             seen.join(',')"
        ),
        "3,0"
    );
}

#[test]
fn a_target_that_will_not_take_an_element_is_a_type_error_rather_than_a_silent_loss() {
    // §7.3.5 `CreateDataPropertyOrThrow` — when the target is a fresh Array the define cannot be
    // refused, so this only shows once a species hands back something else. A non-extensible
    // object takes nothing, and the method must say so rather than answering an object with the
    // elements quietly missing.
    let hostile = "var A = function () { this.length = 0; Object.preventExtensions(this); }; ";
    for method in COPYING {
        assert_eq!(
            run(&format!(
                "{hostile} var a = [[1]]; a.constructor = {{}}; \
                 a.constructor[Symbol.species] = A; \
                 try {{ a.{method}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{method} into a non-extensible target"
        );
    }
    // The other way a define is refused: an index that is already there and not configurable.
    assert_eq!(
        run("var A = function () { this.length = 0; \
                 Object.defineProperty(this, '0', {value: 'fixed', configurable: false}); }; \
             var a = [1]; a.constructor = {}; a.constructor[Symbol.species] = A; \
             try { a.map(function (x) { return x; }); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a species that answers something perfectly ordinary still works, which is what says
    // the check is about the refusal rather than about not being an Array.
    assert_eq!(
        run("var C = function () { this.length = 0; }; \
             var a = [1, 2]; a.constructor = {}; a.constructor[Symbol.species] = C; \
             var answer = a.map(function (x) { return x * 10; }); \
             Array.isArray(answer) + ',' + answer[0] + ',' + answer[1] + ',' + answer.length"),
        "false,10,20,0"
    );
}
