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

/// Run `source` in a machine that has a second realm, whose global is reachable as `other`.
///
/// `other` is the second realm's *global object*, so `other.Array` is its `%Array%` and
/// `other.Array.prototype.map` is a method belonging to it. Named rather than passed as an
/// argument because what is under test is which realm a method *runs* in, and a method reached
/// through a global is reached the way a program reaches one.
fn run_with_other_realm(source: &str) -> String {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let mut vm = Vm::new(&mut heap);
    let second = vm.create_realm(&mut heap);
    let global = vm.realm().global();
    crate::builtins::define_value(&mut heap, global, "other", Value::Object(second.global()));
    let outcome = vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    describe(outcome, &mut heap)
}

#[test]
fn another_realms_array_is_not_a_species_and_the_copy_is_made_where_the_method_ran() {
    // §23.1.3.21 step 5.c. A copying method asks the array it was given for its `constructor`, and
    // for an ordinary array that answers the `%Array%` of whichever realm the array came from —
    // which is an accident of provenance and no statement about what to build. So a constructor
    // that is merely *another* realm's `%Array%` is discarded, and step 6 makes a plain Array in
    // the realm the method is running in, which §10.3.1 step 3 makes the method's own.
    //
    // Both directions, because the wrong answer in each is the other realm and an engine doing no
    // demotion at all passes neither. Without step 5.c this reads `other,here`.
    for method in COPYING {
        assert_eq!(
            run_with_other_realm(&format!(
                "var mine = [[1]]; var theirs = other.eval('[[1]]'); \
                 var a = other.Array.prototype.{method_name}.call(mine{args}); \
                 var b = Array.prototype.{method_name}.call(theirs{args}); \
                 (Object.getPrototypeOf(a) === other.Array.prototype ? 'other' : 'here') + ',' + \
                 (Object.getPrototypeOf(b) === Array.prototype ? 'here' : 'other')",
                method_name = method.split('(').next().expect("a method has a name"),
                args = {
                    let inner = method
                        .trim_end_matches(')')
                        .split_once('(')
                        .expect("a method has arguments")
                        .1;
                    if inner.is_empty() {
                        String::new()
                    } else {
                        format!(", {inner}")
                    }
                },
            )),
            "other,here",
            "{method} across a realm boundary"
        );
    }
}

#[test]
fn a_subclass_from_another_realm_survives_the_demotion_that_a_plain_array_does_not() {
    // The other half of step 5.c, and the reason it is a `SameValue` against one intrinsic rather
    // than a realm comparison: a subclass declared in the other realm is *not* that realm's
    // `%Array%`, so it is never discarded and its `@@species` decides as it would at home. An
    // engine that demoted on the realm difference alone would pass the test above and lose every
    // cross-realm subclass silently.
    //
    // Written with an explicit `.call` and not as `a.map(…)`: a subclass of the *other* realm's
    // Array inherits the *other* realm's `map`, so calling it through the instance puts the
    // constructor and the running realm back in the same realm and step 5.c is never reached at
    // all. That is what the first draft of this test did, and it passed against an engine
    // demoting on the realm difference alone.
    assert_eq!(
        run_with_other_realm(
            "var Sub = other.eval('(class Sub extends Array {})'); \
             var a = new Sub(); a.push(1); \
             var answer = Array.prototype.map.call(a, function (x) { return x; }); \
             (answer instanceof Sub) + ',' + (Object.getPrototypeOf(answer) === Sub.prototype)"
        ),
        "true,true"
    );
    // The realm difference is half the condition and this is the other half: `%Array%` in the
    // realm the method is *running* in is not demoted, so a program that overrides
    // `Array[@@species]` still decides what its own arrays copy into. Nothing in this suite said
    // so until step 5.c was written — an engine demoting every `%Array%` passed all 1,715 tests,
    // and answered a plain Array where node answers a `C`.
    assert_eq!(
        run("var C = function () { this.length = 0; this.tag = 'C'; }; \
             Object.defineProperty(Array, Symbol.species, {value: C}); \
             var m = [1, 2].map(function (x) { return x * 2; }); \
             m.tag + ',' + Array.isArray(m) + ',' + m[0] + ',' + [3].slice().tag"),
        "C,false,2,C"
    );
    // …and replacing the other realm's *global* `Array` does not exempt its arrays, because what
    // step 5.c compares against is the intrinsic and not the property.
    assert_eq!(
        run_with_other_realm(
            "other.eval('Array = function Fake() {}'); \
             var theirs = other.eval('[1]'); \
             var answer = Array.prototype.slice.call(theirs); \
             Object.getPrototypeOf(answer) === Array.prototype"
        ),
        "true"
    );
}
