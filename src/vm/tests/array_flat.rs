//! §23.1.3.13 `flat`, §23.1.3.14 `flatMap` and §23.1.3.32 `toLocaleString`.

use super::*;

#[test]
fn flattening_goes_exactly_as_deep_as_it_was_asked_to() {
    // §23.1.3.13 step 4 — an absent depth is **one**, which is the whole reason `flat()` is useful
    // without an argument. Every row below has more nesting than the depth asks for, so an engine
    // that flattened one level too many or too few disagrees with all of them.
    assert_eq!(run("[1, [2, [3, [4]]]].flat().join('|')"), "1|2|3,4");
    assert_eq!(run("[1, [2, [3, [4]]]].flat(2).join('|')"), "1|2|3|4");
    assert_eq!(
        run("[1, [2, [3, [4]]]].flat(Infinity).join('|')"),
        "1|2|3|4"
    );
    // A depth of nought copies, and a negative one is nought rather than counting backwards or
    // becoming its own magnitude. `NaN` is nought too, by `ToIntegerOrInfinity`.
    //
    // The nested array holds *two* elements, so flattening one level changes the length — with
    // only one inside, `flat(0)` and `flat(1)` both answer two and the row proves nothing.
    assert_eq!(
        run(
            "[1, [2, 3]].flat(0).length + ',' + [1, [2, 3]].flat(-1).length + ',' \
             + [1, [2, 3]].flat(NaN).length + ',' + [1, [2, 3]].flat(-Infinity).length"
        ),
        "2,2,2,2"
    );
    assert_eq!(run("[1, [2]].flat(0)[1].join('')"), "2");
    // A fraction truncates toward zero, so 1.9 is one level and 0.9 is none.
    assert_eq!(
        run("[1, [2, [3]]].flat(1.9).length + ',' + [1, [2]].flat(0.9).length"),
        "3,2"
    );
    // Step 3.c.iv — only a real **Array** is flattened. An array-like with a `length` is not,
    // however many indices it has, and neither is a string. Both of its indices are filled, so an
    // engine that flattened array-likes answers two rather than one — with only index 0 present
    // the two readings agree, because the hole at index 1 would have been skipped anyway.
    assert_eq!(run("[{length: 2, 0: 'a', 1: 'b'}].flat().length"), "1");
    assert_eq!(run("[{length: 2, 0: 'a', 1: 'b'}].flat()[0].length"), "2");
    assert_eq!(run("['ab'].flat().join('')"), "ab");
    // The `length` is what bounds the walk, and it is read rather than inferred: a property past
    // it is not visited, however present it is. An engine reading one index too many finds `b`.
    assert_eq!(
        run("Array.prototype.flat.call({length: 1, 0: 'a', 1: 'b'}).join('')"),
        "a"
    );
    assert_eq!(
        run("Array.prototype.flat.call({length: 0, 0: 'a'}).length"),
        "0"
    );
    // Step 3.b — a hole contributes nothing at all, at any level. It is not flattened into an
    // `undefined`, so the result is shorter than the source rather than the same length.
    assert_eq!(
        run("var f = [1, , 2, [3, , 4]].flat(); f.join('|') + '|' + f.length"),
        "1|2|3|4|4"
    );
    assert_eq!(run("[, , ,].flat().length"), "0");
    // …and an `undefined` is an element, which is what tells the two apart.
    assert_eq!(
        run("var f = [1, undefined, [undefined]].flat(); f.length + ',' + (1 in f)"),
        "3,true"
    );
    assert_eq!(run("[].flat().length + ',' + [[]].flat().length"), "0,0");
    // The answer is always a real Array and never the one it was given.
    assert_eq!(
        run("var a = [1]; var f = a.flat(); Array.isArray(f) + ',' + (f === a)"),
        "true,false"
    );
}

#[test]
fn nesting_deep_enough_to_exhaust_a_stack_flattens_anyway() {
    // §23.1.3.13.1 calls itself once per level, and the *data* decides how many levels there are —
    // so a recursive implementation runs out of Rust stack on an input a program can build in a
    // loop, which DR-0002 does not allow. Twenty thousand levels is far past what any recursion
    // here survives and well within what the heap budget allows.
    assert_eq!(
        run("var deep = []; var cur = deep; \
             for (var i = 0; i < 20000; i++) { var next = []; cur.push(next); cur = next; } \
             cur.push('end'); \
             var f = deep.flat(Infinity); f.length + ',' + f[0]"),
        "1,end"
    );
}

#[test]
fn flat_map_maps_the_top_level_and_flattens_once() {
    // §23.1.3.14 — a depth of exactly one, and the mapper is not handed down. So a mapper that
    // answers a nested array leaves the nesting in place: `flatMap` is not `map` then
    // `flat(Infinity)`, and this is the row that says so.
    assert_eq!(
        run("[1, 2].flatMap(function (x) { return [x, x * 2]; }).join('|')"),
        "1|2|2|4"
    );
    assert_eq!(
        run("var f = [1, 2].flatMap(function (x) { return [[x]]; }); \
             f.length + ',' + Array.isArray(f[0])"),
        "2,true"
    );
    // A mapper answering something that is not an array contributes one element, and one answering
    // an empty array contributes none — which is what makes `flatMap` a filter as well as a map.
    assert_eq!(
        run("[1, 2, 3].flatMap(function (x) { return x % 2 ? [x] : []; }).join('|')"),
        "1|3"
    );
    assert_eq!(
        run("[1, 2].flatMap(function (x) { return x; }).join('|')"),
        "1|2"
    );
    // The callback is given the element, the index and the object, and `thisArg` is the second
    // argument — the same four as `map`.
    assert_eq!(
        run("var seen = []; var a = ['a', 'b']; \
             a.flatMap(function (v, i, o) { seen.push(v + i + (o === a)); return []; }); \
             seen.join('|')"),
        "a0true|b1true"
    );
    assert_eq!(
        run("['x'].flatMap(function () { return [this.tag]; }, {tag: 'here'}).join('')"),
        "here"
    );
    // Step 3 — a mapper that is not callable is refused before anything is read, so an empty
    // array still throws.
    assert_eq!(
        run("try { [].flatMap(1); } catch (e) { e.constructor.name + ':' + e.message }"),
        "TypeError:the callback is not a function"
    );
    // A hole is skipped, so the mapper is not called for one.
    assert_eq!(
        run("var seen = 0; [1, , 2].flatMap(function (x) { seen++; return [x]; }); seen"),
        "2"
    );
}

#[test]
fn to_locale_string_calls_each_element_rather_than_converting_it() {
    // §23.1.3.32 step 6.c — each element is converted by **calling its `toLocaleString`**, not by
    // `ToString`. An engine that used `join` instead agrees about numbers and disagrees the moment
    // an element has one of its own.
    assert_eq!(run("[1, 2, 3].toLocaleString()"), "1,2,3");
    assert_eq!(
        run("[{toLocaleString: function () { return 'X'; }}, 5].toLocaleString()"),
        "X,5"
    );
    // Step 6.c — `undefined` and `null` contribute nothing, and still take their separator. So the
    // number of commas is the number of elements less one, whatever they were.
    assert_eq!(run("[1, null, undefined, 2].toLocaleString()"), "1,,,2");
    assert_eq!(run("[, ,].toLocaleString()"), ",");
    assert_eq!(run("[].toLocaleString()"), "");
    // An element with no `toLocaleString` at all is a TypeError rather than being skipped — which
    // is what "calls it" means, and is why §20.1.3.4 has to exist on `Object.prototype`.
    assert_eq!(
        run("try { [Object.create(null)].toLocaleString(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §20.1.3.4 is `Invoke(O, "toString")`, so an object with its own `toString` answers through
    // it — and one without reaches `Object.prototype.toString`.
    assert_eq!(
        run("[{toString: function () { return 'own'; }}, {}].toLocaleString()"),
        "own,[object Object]"
    );
    // §21.1.3.4 without ECMA-402 is `toString` with no radix, and it is a *different function
    // object* from `Number.prototype.toString` — which a program can see.
    assert_eq!(
        run("(255).toLocaleString() + ',' \
             + (Number.prototype.toLocaleString === Number.prototype.toString)"),
        "255,false"
    );
    // A nested array reaches `Array.prototype.toLocaleString` again, because that is what its
    // elements have — so the commas run together exactly as `toString` would.
    assert_eq!(run("[[1, 2], [3]].toLocaleString()"), "1,2,3");
}

#[test]
fn a_typed_array_has_its_own_locale_string_because_the_generic_one_would_answer_for_anything() {
    // §23.2.3.29 is §23.1.3.32's body with `ValidateTypedArray` in front. Without one of its own,
    // `%TypedArray%.prototype` inherits `Object.prototype.toLocaleString` — which begins with
    // `ToObject` and answers happily about a number or a plain object. Four test262 rows caught
    // exactly that the moment §20.1.3.4 arrived, and these are what keep it caught.
    assert_eq!(run("new Int8Array([1, 2, 3]).toLocaleString()"), "1,2,3");
    for borrowed in ["42", "{}", "[]", "'ab'"] {
        assert_eq!(
            run(&format!(
                "try {{ Int8Array.prototype.toLocaleString.call({borrowed}); }}                  catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "toLocaleString on {borrowed}"
        );
    }
    // …and it is the TypedArray's own function rather than the one it would otherwise inherit.
    assert_eq!(
        run(
            "var p = Object.getPrototypeOf(Int8Array.prototype);              (p.toLocaleString === Object.prototype.toLocaleString) + ',' + typeof p.toLocaleString"
        ),
        "false,function"
    );
    // A detached buffer is the other half of `ValidateTypedArray`, and it is a TypeError rather
    // than an empty string.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int8Array(b); b.transfer();              try { a.toLocaleString(); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}
