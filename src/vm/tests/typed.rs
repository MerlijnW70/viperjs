//! §23.2 and §10.4.5 — the TypedArrays, and the exotic object that makes them arrays.
//!
//! Almost everything here is about the *exotic* half: an element is answered from the buffer rather
//! than from a property table, so a key that looks like an index is intercepted and one that merely
//! looks numeric is not. That distinction is invisible until someone writes `ta["00"]`.

use super::*;

#[test]
fn an_element_is_the_buffers_bytes_and_not_a_stored_property() {
    // The whole design: two views over one buffer see each other's writes, because neither holds
    // anything. An implementation that stored elements would pass every test about one array.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var x = new Int32Array(b); var y = new Uint8Array(b); \
             x[0] = 1; y[0] + ',' + x.length + ',' + y.length"
        ),
        "1,2,8"
    );
    assert_eq!(run("var a = new Int32Array(4); a[0] = 7; a[0]"), "7");
    // A fresh array is zeroed, because its buffer is.
    assert_eq!(run("var a = new Int32Array(3); a[0] + ',' + a[2]"), "0,0");
    // §23.2.3.19 — `length` is in **elements** where `byteLength` is in bytes, which is where a
    // TypedArray differs from a `DataView`: a `DataView` has no elements and so no `length`.
    assert_eq!(
        run("var a = new Int32Array(4); a.length + ',' + a.byteLength + ',' + a.byteOffset"),
        "4,16,0"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(16); var a = new Int32Array(b, 4, 2); \
             a.length + ',' + a.byteLength + ',' + a.byteOffset + ',' + (a.buffer === b)"
        ),
        "2,8,4,true"
    );
    // …and it cannot change, because the buffer's cannot.
    assert_eq!(
        run("var a = new Int32Array(2); a.length = 9; a.length"),
        "2"
    );
}

#[test]
fn a_key_is_an_element_only_when_it_is_a_canonical_numeric_string() {
    // §7.1.21 — the test is `ToString(ToNumber(key)) === key`, not "does it look like a number".
    // `"0"` is canonical and `"00"` is not, so one is an element and the other is an ordinary
    // property that really is stored. Getting this wrong is invisible until someone writes it.
    assert_eq!(
        run("var a = new Int32Array(2); a['00'] = 5; a['00'] + ',' + Object.keys(a).join(',')"),
        "5,0,1,00"
    );
    assert_eq!(
        run("var a = new Int32Array(2); a['1e1'] = 5; a['1e1'] + ',' + a[10]"),
        "5,undefined"
    );
    // A canonical index the array does **not** have is *absent*: the read answers `undefined`
    // without consulting the prototype, and the write is discarded. This is the one assignment in
    // the language that fails silently by design — there is nowhere for the value to go.
    assert_eq!(
        run("var a = new Int32Array(2); a[9] = 1; a[9] + ',' + Object.keys(a).join(',')"),
        "undefined,0,1"
    );
    assert_eq!(
        run("Int32Array.prototype[9] = 'inherited'; var a = new Int32Array(2); a[9]"),
        "undefined"
    );
    // …and one that is *not* canonical does reach the prototype, because it is an ordinary key.
    assert_eq!(
        run("Int32Array.prototype['x'] = 'inherited'; new Int32Array(2).x"),
        "inherited"
    );
    // `-0` is canonical and is never an index — the one case where the two questions differ, since
    // `ToString(-0)` is `"0"` and the key was not.
    assert_eq!(
        run("var a = new Int32Array(2); a['-0'] = 5; a[0] + ',' + a['-0']"),
        "0,undefined"
    );
    // A fraction and a negative are canonical and absent, not ordinary properties.
    assert_eq!(
        run("var a = new Int32Array(2); a[1.5] = 5; a[-1] = 5; \
             a[1.5] + ',' + a[-1] + ',' + Object.keys(a).length"),
        "undefined,undefined,2"
    );
}

#[test]
fn the_elements_are_listed_and_described_as_ordinary_writable_properties() {
    // §10.4.5.1 — writable, enumerable *and* configurable, all three. Not what a String object's
    // characters get: those cannot be written, and the whole point of a TypedArray is that these
    // can. Configurable was a change in ES2021 and is what lets `defineProperty` reach them.
    assert_eq!(
        run("JSON.stringify(Object.getOwnPropertyDescriptor(new Int32Array(2), '0'))"),
        "{\"value\":0,\"writable\":true,\"enumerable\":true,\"configurable\":true}"
    );
    // §10.4.5.6 — the indices are listed even though nothing stored them, in order and ahead of
    // anything that was stored.
    assert_eq!(run("Object.keys(new Int32Array(3)).join(',')"), "0,1,2");
    assert_eq!(
        run("var a = new Int32Array(2); var out = ''; for (var k in a) { out += k; } out"),
        "01"
    );
    assert_eq!(
        run("var a = new Int32Array(2); a.tail = 1; Object.keys(a).join(',')"),
        "0,1,tail"
    );
    // §10.4.5.3 — a define at an index is the ordinary write, and one that asks for anything an
    // element cannot be is refused: an accessor, or any attribute turned off.
    assert_eq!(
        run("var a = new Int32Array(2); Object.defineProperty(a, '0', { value: 9 }); a[0]"),
        "9"
    );
    assert_eq!(
        run("var a = new Int32Array(2); \
             try { Object.defineProperty(a, '0', { value: 9, writable: false }); } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var a = new Int32Array(2); Reflect.defineProperty(a, '0', { get: function () {} })"),
        "false"
    );
    // …and one at an index the array does not have is **refused** rather than stored, because a
    // property there would be a length that lied.
    assert_eq!(
        run("var a = new Int32Array(2); Reflect.defineProperty(a, '5', { value: 1 }) + ',' + a[5]"),
        "false,undefined"
    );
    // §10.4.5.4 — an index it has cannot be deleted, and one it has not is already gone. The two
    // answers are opposite and come from the same place.
    assert_eq!(
        run("var a = new Int32Array(2); delete a[0] + ',' + delete a[9] + ',' + a[0]"),
        "false,true,0"
    );
}

#[test]
fn writing_an_element_converts_the_way_its_type_says() {
    // §7.1.9 — the integer kinds **wrap**, which is what makes a TypedArray behave like memory.
    assert_eq!(run("var a = new Uint8Array(1); a[0] = 300; a[0]"), "44");
    assert_eq!(run("var a = new Int8Array(1); a[0] = 200; a[0]"), "-56");
    assert_eq!(run("var a = new Uint8Array(1); a[0] = -1; a[0]"), "255");
    // `NaN`, the infinities and a fraction all resolve rather than refusing, so a write is total.
    assert_eq!(
        run(
            "var a = new Int32Array(4); a[0] = NaN; a[1] = Infinity; a[2] = 1.9; a[3] = '5'; \
             a[0] + ',' + a[1] + ',' + a[2] + ',' + a[3]"
        ),
        "0,0,1,5"
    );
    // §7.1.11 — `Uint8ClampedArray` is the one kind that differs, and this is the whole of the
    // difference: it **saturates** instead of wrapping, and rounds a half to *even* rather than
    // away from zero. Both are what pixel data wants — 300 is as bright as it gets, and rounding
    // half to even stops a long run of averages drifting upward.
    assert_eq!(
        run(
            "var a = new Uint8ClampedArray(4); a[0] = 300; a[1] = -5; a[2] = 2.5; a[3] = 1.5; \
             a[0] + ',' + a[1] + ',' + a[2] + ',' + a[3]"
        ),
        "255,0,2,2"
    );
    assert_eq!(
        run("var a = new Uint8ClampedArray(2); a[0] = 3.5; a[1] = 0.5; a[0] + ',' + a[1]"),
        "4,0"
    );
    // A float32 rounds to its width, so it does not read back what was written; a float64 does.
    assert_eq!(
        run("var a = new Float32Array(1); a[0] = 0.1; a[0]"),
        "0.10000000149011612"
    );
    assert_eq!(run("var a = new Float64Array(1); a[0] = 0.1; a[0]"), "0.1");
}

#[test]
fn the_first_argument_decides_which_of_four_things_a_constructor_makes() {
    // §23.2.5.1 — a length, a buffer, another TypedArray, or anything iterable. Four shapes and
    // one constructor, and the difference between the middle two is the one that matters: a buffer
    // is *shared* and another array is *copied*.
    assert_eq!(run("new Int32Array(3).length"), "3");
    assert_eq!(run("new Int32Array().length"), "0");
    assert_eq!(
        run("var a = new Int8Array([1, 2, 3]); a.length + ',' + a[0] + a[1] + a[2]"),
        "3,123"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(8); var a = new Int32Array(b); \
             (a.buffer === b) + ',' + a.length"),
        "true,2"
    );
    // Copied *by value*, converting each one — so a `Int32Array` from a `Float64Array` holds the
    // truncated numbers and shares no buffer with it. Mistaking this for the buffer form is the
    // easiest way to write a program that aliases when it meant to copy.
    assert_eq!(
        run(
            "var f = new Float64Array([1.5, 2.5]); var i = new Int32Array(f); \
             i[0] + ',' + i[1] + ',' + (i.buffer === f.buffer)"
        ),
        "1,2,false"
    );
    // An offset must be a whole number of elements, and so must whatever is left of the buffer:
    // an `Int32Array`'s elements would otherwise straddle the boundaries the format promised.
    assert_eq!(
        run("var b = new ArrayBuffer(8); new Int32Array(b, 4).length"),
        "1"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); try { new Int32Array(b, 2); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(9); try { new Int32Array(b); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); try { new Int32Array(b, 0, 3); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // §23.2.5.1 step 1 — a plain call has no `new.target` to take a prototype from.
    assert_eq!(
        run("try { Int32Array(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_nine_share_one_abstract_constructor_that_cannot_construct() {
    // §23.2.1 — `%TypedArray%` exists to be the prototype of the nine and to hold what they share.
    // It has no name in the language: `Object.getPrototypeOf(Int8Array)` is the only way to reach
    // it, and calling it is a TypeError whatever the arguments.
    assert_eq!(
        run("Object.getPrototypeOf(Int8Array) === Object.getPrototypeOf(Float64Array)"),
        "true"
    );
    assert_eq!(
        run(
            "Object.getPrototypeOf(Int8Array.prototype) === Object.getPrototypeOf(Float64Array.prototype)"
        ),
        "true"
    );
    assert_eq!(
        run("try { new (Object.getPrototypeOf(Int8Array))(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …which is what makes the shared accessors *the same function object* rather than nine copies.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Int8Array.prototype), 'length'); \
             typeof d.get"
        ),
        "function"
    );
    // §23.2.6.2 — `BYTES_PER_ELEMENT`, on the constructor and on its prototype, and on neither is
    // it writable or configurable: it is the one fact about a kind that cannot change.
    assert_eq!(
        run(
            "Int8Array.BYTES_PER_ELEMENT + ',' + Int32Array.BYTES_PER_ELEMENT + ',' \
             + Float64Array.BYTES_PER_ELEMENT + ',' + Uint8ClampedArray.BYTES_PER_ELEMENT"
        ),
        "1,4,8,1"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(Int8Array, 'BYTES_PER_ELEMENT'); \
             d.writable + ',' + d.configurable + ',' + (Int8Array.prototype.BYTES_PER_ELEMENT === 1)"
        ),
        "false,false,true"
    );
    // §23.2.3.32 — the tag is an **accessor** answering the kind's name, so one function serves all
    // nine. It answers `undefined` rather than throwing for anything that is not a TypedArray,
    // because `Object.prototype.toString` reads it off whatever it was given.
    assert_eq!(
        run("Object.prototype.toString.call(new Int8Array(1)) + ',' \
             + Object.prototype.toString.call(new Uint8ClampedArray(1))"),
        "[object Int8Array],[object Uint8ClampedArray]"
    );
    assert_eq!(
        run("var tag = Object.getOwnPropertyDescriptor( \
               Object.getPrototypeOf(Int8Array.prototype), Symbol.toStringTag).get; \
             tag.call({}) + ',' + tag.call(new Float32Array(1))"),
        "undefined,Float32Array"
    );
    // Every other accessor throws for something that is not one, which is where they differ.
    assert_eq!(
        run("try { Object.getOwnPropertyDescriptor( \
               Object.getPrototypeOf(Int8Array.prototype), 'length').get.call({}); } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_detached_buffer_leaves_the_array_empty_rather_than_throwing() {
    // §23.2.3.2 and §23.2.3.19 — a TypedArray over a detached buffer answers **0** for its length
    // and byte length, where a `DataView` throws for the same question. The difference is
    // deliberate: a TypedArray is an array-like and an array-like with no length is unusable, so
    // it becomes an empty one instead.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer(); \
             a.length + ',' + a.byteLength + ',' + a.byteOffset"
        ),
        "0,0,0"
    );
    // Its elements go with it: every index becomes absent, so a read answers `undefined` and a
    // write is discarded, exactly as an out-of-range index does.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int32Array(b); a[0] = 5; b.transfer(); \
             a[0] + ',' + (a[0] = 9) + ',' + a[0] + ',' + Object.keys(a).length"
        ),
        "undefined,9,undefined,0"
    );
    // …and `buffer` still answers, because it is about which buffer and not about its bytes.
    assert_eq!(
        run("var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer(); a.buffer === b"),
        "true"
    );
    // A buffer that is already detached cannot have a view made over it at all.
    assert_eq!(
        run("var b = new ArrayBuffer(8); b.transfer(); \
             try { new Int32Array(b); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn each_kind_wraps_at_its_own_width_and_only_one_of_them_clamps() {
    // The nine differ in exactly two things: how wide an element is, and — for one of them — how a
    // value is converted on the way in. A kind that clamped when it should wrap would answer 255
    // for every one of these.
    assert_eq!(
        run("var a = new Int16Array(1); a[0] = 40000; a[0]"),
        "-25536"
    );
    assert_eq!(
        run("var a = new Uint16Array(1); a[0] = 40000; a[0]"),
        "40000"
    );
    assert_eq!(run("var a = new Uint16Array(1); a[0] = 65536; a[0]"), "0");
    assert_eq!(
        run("var a = new Int32Array(1); a[0] = 2147483648; a[0]"),
        "-2147483648"
    );
    assert_eq!(
        run("var a = new Uint32Array(1); a[0] = -1; a[0]"),
        "4294967295"
    );
    assert_eq!(run("var a = new Int8Array(1); a[0] = 128; a[0]"), "-128");
    // Above 255 and below 0, where wrapping and clamping finally disagree for a *signed* byte:
    // 200 and 128 give the same answer either way, so neither of them says which this kind does.
    assert_eq!(run("var a = new Int8Array(1); a[0] = 300; a[0]"), "44");
    assert_eq!(run("var a = new Int8Array(1); a[0] = -5; a[0]"), "-5");
    // §7.1.11 rounds a fraction *above* a half upward, which is the arm the halves do not reach.
    assert_eq!(
        run(
            "var a = new Uint8ClampedArray(3); a[0] = 2.6; a[1] = 2.4; a[2] = NaN;              a[0] + ',' + a[1] + ',' + a[2]"
        ),
        "3,2,0"
    );
}

#[test]
fn an_index_past_the_end_is_absent_even_when_the_buffer_has_bytes_there() {
    // The bound is the **view's** length and not the buffer's, which is only visible when the two
    // differ: a window over part of a buffer must not read the part it does not cover, even though
    // the bytes are there and readable through another view.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var full = new Int32Array(b); full[1] = 7;              var part = new Int32Array(b, 0, 1); part[1] + ',' + part.length + ',' + full[1]"
        ),
        "undefined,1,7"
    );
    // …and writing there is discarded rather than reaching the neighbour.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var full = new Int32Array(b);              var part = new Int32Array(b, 0, 1); part[1] = 9; full[1]"
        ),
        "0"
    );
}

#[test]
fn writing_an_element_never_throws_however_strict_the_code_is() {
    // §10.4.5.5 answers "unused" rather than a Boolean, so there is nothing for strict mode to
    // turn into a TypeError. A write to an index that is not there is discarded in both modes,
    // which is where a TypedArray differs from every other object: a frozen object's refusal
    // throws in strict code, and this one cannot.
    assert_eq!(
        run("'use strict'; var a = new Int32Array(2); a[9] = 1; a[9] + ',' + a.length"),
        "undefined,2"
    );
    assert_eq!(
        run("'use strict'; var a = new Int32Array(2); a[0] = 5; a[0]"),
        "5"
    );
    // …and the same after the buffer has gone, where every read and write becomes a no-op.
    assert_eq!(
        run(
            "'use strict'; var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer();              a[0] = 1; a[0]"
        ),
        "undefined"
    );
}

#[test]
fn a_define_at_an_element_refuses_every_attribute_an_element_cannot_have() {
    // §10.4.5.3 — an element is writable, enumerable and configurable, and a descriptor asking for
    // any of the three to be otherwise is refused. Three separate checks because they are three
    // separate lies a program could tell about an element.
    for asked in [
        "writable: false",
        "enumerable: false",
        "configurable: false",
        "get: function () {}",
        "set: function () {}",
    ] {
        assert_eq!(
            run(&format!(
                "var a = new Int32Array(2); Reflect.defineProperty(a, '0', {{ {asked} }})"
            )),
            "false",
            "{asked}"
        );
    }
    // …and one that asks for exactly what an element already is, is allowed.
    assert_eq!(
        run(
            "var a = new Int32Array(2); Reflect.defineProperty(a, '0',              { value: 3, writable: true, enumerable: true, configurable: true }) + ',' + a[0]"
        ),
        "true,3"
    );
}

#[test]
fn the_shared_accessors_and_the_tag_have_the_attributes_their_clauses_give_them() {
    // §17's convention for an accessor: not enumerable, configurable. `configurable` is what makes
    // each replaceable, which is the only reason a specification ever says so.
    let abstract_prototype = "Object.getPrototypeOf(Int8Array.prototype)";
    for name in ["'buffer'", "'byteLength'", "'byteOffset'", "'length'"] {
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor({abstract_prototype}, {name});                  (typeof d.get) + ',' + d.enumerable + ',' + d.configurable"
            )),
            "function,false,true",
            "{name}"
        );
    }
    assert_eq!(
        run(&format!(
            "var d = Object.getOwnPropertyDescriptor({abstract_prototype}, Symbol.toStringTag);              (typeof d.get) + ',' + d.enumerable + ',' + d.configurable"
        )),
        "function,false,true"
    );
    // Configurable, so a program may take the tag off — and then a TypedArray describes as an
    // ordinary object, which is what makes the attribute worth saying.
    assert_eq!(
        run(&format!(
            "delete {abstract_prototype}[Symbol.toStringTag];              Object.prototype.toString.call(new Int8Array(1))"
        )),
        "[object Object]"
    );
}

#[test]
fn a_typed_array_is_refused_before_it_is_allocated_when_it_is_too_large() {
    // DR-0013 — the same rule an `ArrayBuffer` gets, and it has to be here too: the length is in
    // *elements*, so `new Float64Array(n)` asks for eight times what `new ArrayBuffer(n)` does and
    // a program that computed one from the other would slip past a check that only knew about
    // bytes.
    assert_eq!(
        run("try { new Float64Array(2 ** 40); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { new Int8Array(-1); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
}

#[test]
fn the_methods_answer_a_typed_array_where_an_arrays_would_answer_an_array() {
    // §23.2.3.21 and §23.2.3.10 — `map` and `filter` and `slice` make one of the *same kind*
    // through `@@species`, which is the most visible difference from §23.1.3's generic methods.
    assert_eq!(
        run(
            "var m = new Int8Array([1, 2, 3]).map(function (v) { return v * 2; }); \
             m.join(',') + '|' + (m instanceof Int8Array) + ',' + Array.isArray(m)"
        ),
        "2,4,6|true,false"
    );
    assert_eq!(
        run("new Int8Array([1, 2, 3]).filter(function (v) { return v > 1; }).join(',')"),
        "2,3"
    );
    assert_eq!(
        run("new Int8Array([1, 2, 3, 4]).slice(1, 3).join(',')"),
        "2,3"
    );
    // …and the answer is converted by the *kind*, so a mapper returning a fraction into an
    // `Int8Array` truncates where an Array would have kept it.
    assert_eq!(
        run("new Int8Array([1]).map(function () { return 1.9; }).join(',')"),
        "1"
    );
    // §23.2.3.30 — `subarray` is the one that does **not** copy: another window onto the same
    // buffer, so writing through it is visible through the original. That is the whole reason both
    // it and `slice` exist.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int32Array(b); var s = a.subarray(1); \
             s[0] = 9; a[1] + ',' + s.length + ',' + (s.buffer === b)"
        ),
        "9,1,true"
    );
    assert_eq!(
        run("var a = new Int32Array([1, 2]); var s = a.slice(1); s[0] = 9; a[1] + ',' + s[0]"),
        "2,9"
    );
}

#[test]
fn sort_orders_numbers_where_an_arrays_sort_orders_their_spellings() {
    // §23.2.3.29 — the default comparison is **numeric**. `Array.prototype.sort` renders each
    // element as a String first, which puts 10 before 9; these elements *are* numbers and there is
    // nothing to render.
    assert_eq!(
        run("new Float64Array([10, 9, 2]).sort().join(',')"),
        "2,9,10"
    );
    // Where sorting the same three as an Array gives 10, 2, 9, because `Array.prototype.sort`
    // compares `"10" < "2"`. Stated rather than run: §23.1.3.30 is not implemented here yet.

    // `NaN` sorts last and `-0` before `+0`, neither of which an ordinary comparison can say.
    assert_eq!(
        run("var a = new Float64Array([NaN, 1, NaN, 0]); a.sort(); \
             a[0] + ',' + a[1] + ',' + a[2] + ',' + a[3]"),
        "0,1,NaN,NaN"
    );
    assert_eq!(
        run("var a = new Float64Array([0, -0]); a.sort(); (1 / a[0]) + ',' + (1 / a[1])"),
        "-Infinity,Infinity"
    );
    // A comparator is used when there is one, and it sees numbers rather than strings.
    assert_eq!(
        run("new Int8Array([1, 2, 3]).sort(function (a, b) { return b - a; }).join(',')"),
        "3,2,1"
    );
    // …and one that answers nonsense still terminates with *some* permutation, which is all
    // §23.2.3.29 requires of an inconsistent comparator.
    assert_eq!(
        run("new Int8Array([3, 1, 2]).sort(function () { return NaN; }).length"),
        "3"
    );
    assert_eq!(
        run("try { new Int8Array([1]).sort(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // In place, answering the same array rather than a copy.
    assert_eq!(
        run("var a = new Int8Array([2, 1]); (a.sort() === a) + ',' + a.join(',')"),
        "true,1,2"
    );
}

#[test]
fn a_walk_reads_the_length_from_the_slot_and_not_from_a_property() {
    // §23.2.3's methods take the length from `[[ArrayLength]]` where §23.1.3's read a `length`
    // *property*. A program that assigns one changes nothing, and a generic algorithm would then
    // iterate zero elements.
    assert_eq!(
        run("var a = new Int8Array([1, 2, 3]); a.length = 0; \
             a.join(',') + '|' + a.map(function (v) { return v; }).length"),
        "1,2,3|3"
    );
    // The callback is handed the element, the index and the array — three arguments, in that order.
    assert_eq!(
        run(
            "var seen = ''; new Int8Array([7, 8]).forEach(function (v, i, a) { \
               seen += v + '@' + i + (a instanceof Int8Array) + ';'; }); seen"
        ),
        "7@0true;8@1true;"
    );
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).every(function (v) { return v > 0; }) + ',' \
             + new Int8Array([1, 2, 3]).some(function (v) { return v > 2; }) + ',' \
             + new Int8Array([]).every(function () { return false; })"
        ),
        "true,true,true"
    );
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).find(function (v) { return v > 1; }) + ',' \
             + new Int8Array([1, 2, 3]).findIndex(function (v) { return v > 1; }) + ',' \
             + new Int8Array([1, 2, 3]).findLast(function (v) { return v > 1; }) + ',' \
             + new Int8Array([1, 2, 3]).findLastIndex(function (v) { return v > 1; })"
        ),
        "2,1,3,2"
    );
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).reduce(function (a, b) { return a + b; }) + ',' \
             + new Int8Array([1, 2, 3]).reduce(function (a, b) { return a + b; }, 10) + ',' \
             + new Int8Array([1, 2, 3]).reduceRight(function (a, b) { return a + '' + b; })"
        ),
        "6,16,321"
    );
    // §23.2.3.22 step 5 — an empty array with no initial value is a TypeError, because there is no
    // answer to give. With one, the initial value is the answer.
    assert_eq!(
        run("try { new Int8Array([]).reduce(function () {}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("new Int8Array([]).reduce(function () {}, 'start')"),
        "start"
    );
    // Every one of them refuses a callback that is not a function, before it walks.
    for name in [
        "forEach", "map", "filter", "every", "some", "find", "reduce",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ new Int8Array([1]).{name}(1); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{name}"
        );
    }
}

#[test]
fn the_element_wise_methods_do_what_their_array_counterparts_do() {
    assert_eq!(run("new Int8Array([1, 2, 3]).join('-')"), "1-2-3");
    assert_eq!(run("new Int8Array([1, 2]).join()"), "1,2");
    assert_eq!(run("String(new Int8Array([1, 2]))"), "1,2");
    assert_eq!(run("new Int8Array([]).join(',')"), "");
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).at(-1) + ',' + new Int8Array([1, 2, 3]).at(0) + ',' \
             + new Int8Array([1, 2, 3]).at(9)"
        ),
        "3,1,undefined"
    );
    assert_eq!(run("new Int32Array(4).fill(7).join(',')"), "7,7,7,7");
    assert_eq!(run("new Int32Array(4).fill(7, 1, 3).join(',')"), "0,7,7,0");
    assert_eq!(run("new Int8Array([1, 2, 3]).reverse().join(',')"), "3,2,1");
    assert_eq!(
        run("new Int8Array([1, 2, 3, 4, 5]).copyWithin(0, 3).join(',')"),
        "4,5,3,4,5"
    );
    // §23.2.3.13 finds `NaN` where §23.2.3.14 cannot — `includes` uses `SameValueZero` and
    // `indexOf` uses strict equality, which is the one difference between them.
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).indexOf(2) + ',' + new Int8Array([1, 2, 1]).lastIndexOf(1) \
             + ',' + new Int8Array([1]).indexOf(9)"
        ),
        "1,2,-1"
    );
    assert_eq!(
        run("new Float64Array([NaN]).includes(NaN) + ',' + new Float64Array([NaN]).indexOf(NaN)"),
        "true,-1"
    );
    // §23.2.3.24 — `set` copies a source over this array at an offset, and refuses one that would
    // not fit rather than writing the part of it that would.
    assert_eq!(
        run("var a = new Int8Array(4); a.set([1, 2], 1); a.join(',')"),
        "0,1,2,0"
    );
    assert_eq!(
        run("var a = new Int8Array(4); a.set(new Int8Array([9, 8])); a.join(',')"),
        "9,8,0,0"
    );
    assert_eq!(
        run("var a = new Int8Array(2); try { a.set([1, 2, 3]); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("var a = new Int8Array(2); try { a.set([1], -1); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
}

#[test]
fn a_typed_array_iterates_its_elements_and_says_so_three_ways() {
    // §23.2.3.36 — `[@@iterator]` is the *same function object* as `values`, which a program can
    // see and which follows from a TypedArray's iteration being over its elements and nothing else.
    assert_eq!(
        run("var p = Object.getPrototypeOf(Int8Array.prototype); \
             p[Symbol.iterator] === p.values"),
        "true"
    );
    assert_eq!(
        run("var out = ''; for (var v of new Int8Array([1, 2, 3])) { out += v; } out"),
        "123"
    );
    assert_eq!(
        run("Array.from(new Int8Array([5, 6])).join(',') + '|' \
             + Array.from(new Int8Array([5, 6]).keys()).join(',') + '|' \
             + Array.from(new Int8Array([5, 6]).entries()) \
                 .map(function (e) { return e.join(':'); }).join(' ')"),
        "5,6|0,1|0:5 1:6"
    );
    // §23.2.2.1 and §23.2.2.2 — `from` takes an iterable or an array-like and an optional mapper,
    // and `of` takes the elements it was handed.
    assert_eq!(
        run("Int8Array.from([1, 2, 3], function (v) { return v * 2; }).join(',')"),
        "2,4,6"
    );
    assert_eq!(
        run("Int8Array.from({ length: 2, 0: 7, 1: 8 }).join(',')"),
        "7,8"
    );
    assert_eq!(run("Int8Array.of(9, 8).join(',')"), "9,8");
    assert_eq!(
        run("(Int8Array.of(1) instanceof Int8Array) + ',' \
             + (Int8Array.from([1]) instanceof Int8Array)"),
        "true,true"
    );
}

#[test]
fn every_method_refuses_something_that_is_not_one_or_whose_buffer_has_gone() {
    // §23.2.4.1 `ValidateTypedArray` opens every one of them and asks two questions: is this a
    // TypedArray, and are its bytes still there. The second is asked *first* rather than at the
    // first element, so an empty walk over a detached buffer throws rather than quietly doing
    // nothing — which is what tells a program its data went away.
    for source in [
        "join()",
        "at(0)",
        "map(function () {})",
        "sort()",
        "fill(1)",
        "slice(0)",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ Object.getPrototypeOf(Int8Array.prototype).{source}; }} \
                 catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source}"
        );
        assert_eq!(
            run(&format!(
                "var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer(); \
                 try {{ a.{source}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "detached {source}"
        );
    }
    // §23.2.3.30 asks a *different* question and arrives at the same place: `subarray` makes no
    // element access, so it does not validate — but it builds another view over the same buffer,
    // and a view over a detached one cannot be made. The refusal comes from the constructor rather
    // than from the method, which is why the two are worth telling apart.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer();              try { a.subarray(0); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_conversion_may_detach_the_buffer_and_a_species_may_answer_anything() {
    // §23.2.3.8 step 10 — `fill` converts three arguments and then asks about the buffer *again*,
    // because each of those conversions runs a `valueOf` and a `valueOf` is a program. Without the
    // second check the writes are simply discarded and nothing tells the program its data went.
    for argument in ["a.fill(n)", "a.fill(1, n)", "a.fill(1, 0, n)"] {
        assert_eq!(
            run(&format!(
                "var b = new ArrayBuffer(8); var a = new Int32Array(b);                  var n = {{ valueOf: function () {{ b.transfer(); return 1; }} }};                  try {{ {argument}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{argument}"
        );
    }
    // §23.2.4.3 and §23.2.4.4 — a species, or a constructor `from` was called on, may answer
    // anything at all. What it answered has to be a TypedArray, or the writes below it would go
    // nowhere in silence and the caller would receive something it did not ask for.
    assert_eq!(
        run(
            "var a = new Int8Array(4);              a.constructor = { [Symbol.species]: function () { return {}; } };              try { a.subarray(0); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var a = new Int8Array(4);              a.constructor = { [Symbol.species]: function () { return {}; } };              try { a.map(function (v) { return v; }); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Int8Array.from.call(function () { return {}; }, [1]); }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Int8Array.of.call(function () { return 1; }, 1); }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and so is a constructor that answers a TypedArray that is too *short*: the elements would
    // be written into indices it does not have, which §10.4.5.5 discards in silence, and the
    // caller would receive a short array and no complaint.
    assert_eq!(
        run(
            "try { Int8Array.from.call(function () { return new Int8Array(1); }, [1, 2]); }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Int8Array.of.call(function () { return new Int8Array(1); }, 1, 2); }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and a species that answers a TypedArray that is too short is refused too, because the
    // elements would not fit in what it made.
    assert_eq!(
        run(
            "var a = new Int8Array(4);              a.constructor = { [Symbol.species]: function () { return new Int8Array(1); } };              try { a.map(function (v) { return v; }); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_search_takes_its_direction_and_its_starting_point_from_the_arguments() {
    // §23.2.3.14 and §23.2.3.18 — a `fromIndex` counts back from the end when it is negative, and
    // the two directions start at opposite ends. Every one of these numbers is a boundary: an
    // implementation off by one in either direction agrees with the others.
    assert_eq!(
        run("var a = new Int8Array([1, 2, 1, 2]); \
             a.indexOf(2) + ',' + a.indexOf(2, 2) + ',' + a.indexOf(2, -1) + ',' + a.indexOf(2, -9)"),
        "1,3,3,1"
    );
    assert_eq!(
        run("var a = new Int8Array([1, 2, 1, 2]); \
             a.lastIndexOf(1) + ',' + a.lastIndexOf(1, 1) + ',' + a.lastIndexOf(1, -3) \
             + ',' + a.lastIndexOf(1, -9)"),
        "2,0,0,-1"
    );
    // A `fromIndex` past the end finds nothing forwards and everything backwards, which is what
    // the two clamps are for.
    assert_eq!(
        run("var a = new Int8Array([1, 2]); a.indexOf(1, 9) + ',' + a.lastIndexOf(2, 9)"),
        "-1,1"
    );
    // The first and last elements themselves, which the bounds have to include.
    assert_eq!(
        run("var a = new Int8Array([7, 8, 7]); \
             a.indexOf(7) + ',' + a.lastIndexOf(7) + ',' + a.indexOf(8, 1) + ',' + a.lastIndexOf(8, 1)"),
        "0,2,1,1"
    );
    // §23.2.3.13's `SameValueZero` and §23.2.3.14's strict equality agree about everything except
    // `NaN`, and neither finds a value that is not a number at all — a TypedArray holds numbers,
    // so a string can never be one of them however it would compare.
    assert_eq!(
        run("var a = new Int8Array([1]); \
             a.indexOf('1') + ',' + a.includes('1') + ',' + a.includes(undefined) \
             + ',' + a.indexOf(1) + ',' + a.includes(1)"),
        "-1,false,false,0,true"
    );
    assert_eq!(
        run("var a = new Float64Array([1, NaN]); \
             a.includes(NaN) + ',' + a.includes(1) + ',' + a.includes(9) + ',' + a.indexOf(NaN)"),
        "true,true,false,-1"
    );
    // An empty array finds nothing, whichever way it is asked.
    assert_eq!(
        run("var a = new Int8Array([]); \
             a.indexOf(1) + ',' + a.lastIndexOf(1) + ',' + a.includes(1)"),
        "-1,-1,false"
    );
}

#[test]
fn a_range_that_is_relative_clamps_at_both_ends() {
    // §7.1.5 with a relative index, which `fill`, `slice`, `subarray` and `copyWithin` all use: a
    // negative counts back from the end, anything past the end is the end, and a backwards range
    // is empty rather than reversed.
    assert_eq!(
        run("new Int8Array([1, 2, 3, 4]).slice(-2).join(',') + '|' \
             + new Int8Array([1, 2, 3, 4]).slice(0, -2).join(',') + '|' \
             + new Int8Array([1, 2, 3, 4]).slice(-9).join(',') + '|' \
             + new Int8Array([1, 2, 3, 4]).slice(9).length + '|' \
             + new Int8Array([1, 2, 3, 4]).slice(3, 1).length"),
        "3,4|1,2|1,2,3,4|0|0"
    );
    assert_eq!(
        run(
            "new Int32Array(4).fill(7, -2).join(',') + '|' + new Int32Array(4).fill(7, 9).join(',')"
        ),
        "0,0,7,7|0,0,0,0"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(16); var a = new Int32Array(b); \
             a.subarray(-2).length + ',' + a.subarray(2, 1).length + ',' + a.subarray(1, 3).length"),
        "2,0,2"
    );
    // §23.2.3.24 — `set` fits exactly at the end and refuses one element more, which is the
    // boundary the length check is about.
    assert_eq!(
        run("var a = new Int8Array(3); a.set([1, 2], 1); a.join(',')"),
        "0,1,2"
    );
    assert_eq!(
        run("var a = new Int8Array(3); try { a.set([1, 2], 2); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("var a = new Int8Array(3); a.set([1, 2, 3], 0); a.join(',')"),
        "1,2,3"
    );
}

#[test]
fn a_comparator_decides_the_order_and_may_be_asked_many_times() {
    // §23.2.3.29 — the comparator's *sign* is what is read: negative or zero keeps the order it
    // was given, positive swaps. A sort that read it the other way round reverses everything, and
    // one that never advanced would loop or place every element at the front.
    assert_eq!(
        run("new Int8Array([3, 1, 2]).sort(function (a, b) { return a - b; }).join(',')"),
        "1,2,3"
    );
    assert_eq!(
        run("new Int8Array([3, 1, 2]).sort(function (a, b) { return b - a; }).join(',')"),
        "3,2,1"
    );
    // A comparator that says everything is equal keeps the order it was given, which is what "zero
    // means keep" has to mean — and is only visible with more than two elements.
    assert_eq!(
        run("new Int8Array([3, 1, 2]).sort(function () { return 0; }).join(',')"),
        "3,1,2"
    );
    // …and one that says everything is greater reverses it, which is the other end of the same
    // reading.
    assert_eq!(
        run("new Int8Array([1, 2, 3]).sort(function () { return 1; }).join(',')"),
        "3,2,1"
    );
    assert_eq!(
        run("new Int8Array([1, 2, 3]).sort(function () { return -1; }).join(',')"),
        "1,2,3"
    );
    // The comparator sees the elements as numbers, in the order the sort chose to compare them,
    // and it is called at all — a sort that ignored it would still answer something plausible.
    assert_eq!(
        run(
            "var seen = 0; new Int8Array([3, 1, 2]).sort(function () { seen++; return 0; }); seen > 0"
        ),
        "true"
    );
    // Five elements, so the answer cannot come out right by accident.
    assert_eq!(
        run("new Int8Array([5, 3, 1, 4, 2]).sort(function (a, b) { return a - b; }).join(',')"),
        "1,2,3,4,5"
    );
}

#[test]
fn to_string_goes_through_join_and_a_walk_reads_its_callbacks_answer() {
    // §23.2.3.31 is `Array.prototype.toString`, which calls whatever `join` currently *is* — so
    // replacing it changes what `toString` answers, and a `join` that is not callable is refused
    // rather than falling back to something else.
    assert_eq!(
        run(
            "var a = new Int8Array([1, 2]); a.join = function () { return 'replaced'; }; \
             a.toString()"
        ),
        "replaced"
    );
    assert_eq!(
        run("var a = new Int8Array([1, 2]); a.join = 1; \
             try { a.toString(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §23.2.3.7 and §23.2.3.28 — the two that stop early, and the two answers each gives.
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).every(function (v) { return v < 3; }) + ',' \
             + new Int8Array([1, 2, 3]).every(function (v) { return v < 9; }) + ',' \
             + new Int8Array([1, 2, 3]).some(function (v) { return v > 9; }) + ',' \
             + new Int8Array([]).some(function () { return true; })"
        ),
        "false,true,false,false"
    );
    // …and they stop as soon as they know, which is what "at the first" means.
    assert_eq!(
        run(
            "var seen = 0; new Int8Array([1, 2, 3]).every(function () { seen++; return false; }); seen"
        ),
        "1"
    );
    assert_eq!(
        run(
            "var seen = 0; new Int8Array([1, 2, 3]).some(function () { seen++; return true; }); seen"
        ),
        "1"
    );
    // §23.2.3.22 and §23.2.3.23 differ in direction and in nothing else, which only shows when the
    // callback is not commutative.
    assert_eq!(
        run(
            "new Int8Array([1, 2, 3]).reduce(function (a, b) { return a + '' + b; }) + ',' \
             + new Int8Array([1, 2, 3]).reduceRight(function (a, b) { return a + '' + b; })"
        ),
        "123,321"
    );
    // §23.2.2.1 — a mapper that is not callable is refused before anything is read.
    assert_eq!(
        run("try { Int8Array.from([1], 1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(run("Int8Array.from([1, 2]).join(',')"), "1,2");
}

#[test]
fn what_must_be_callable_says_which_argument_was_wrong() {
    // Without these checks the call below fails anyway and says "what was called is not a
    // function" — true, and unhelpful when the thing handed over was a function often enough that
    // the useful sentence names the argument. Each is a different mistake and reads as one.
    assert_eq!(
        run("var a = new Int8Array([1, 2]); a.join = 1; \
             try { a.toString(); } catch (e) { e.message }"),
        "join is not a function"
    );
    assert_eq!(
        run("try { new Int8Array([1]).map(1); } catch (e) { e.message }"),
        "the callback is not a function"
    );
    assert_eq!(
        run("try { new Int8Array([1]).reduce(1); } catch (e) { e.message }"),
        "the callback is not a function"
    );
    assert_eq!(
        run("try { Int8Array.from([1], 1); } catch (e) { e.message }"),
        "the mapper is not a function"
    );
    assert_eq!(
        run("try { new Int8Array([1]).sort(1); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
}

#[test]
fn at_counts_back_from_the_end_and_answers_nothing_outside_it() {
    // §23.2.3.1 — a negative index counts back, and one that lands before the start is *absent*
    // rather than the first element. The distinction matters because the arithmetic that produces
    // it is unsigned underneath: an index of -6 into a three-element array is not index 0.
    assert_eq!(
        run("var a = new Int8Array([7, 8, 9]); \
             a.at(0) + ',' + a.at(2) + ',' + a.at(-1) + ',' + a.at(-3)"),
        "7,9,9,7"
    );
    assert_eq!(
        run("var a = new Int8Array([7, 8, 9]); \
             a.at(3) + ',' + a.at(-4) + ',' + a.at(-9) + ',' + a.at(9)"),
        "undefined,undefined,undefined,undefined"
    );
    assert_eq!(run("new Int8Array([]).at(0)"), "undefined");
    // A fraction truncates toward zero rather than rounding, and `undefined` is 0.
    assert_eq!(
        run("var a = new Int8Array([7, 8]); a.at(1.9) + ',' + a.at() + ',' + a.at(-1.9)"),
        "8,7,8"
    );
}

#[test]
fn the_iterator_property_has_the_attributes_a_built_in_method_gets() {
    // §17's convention: writable, not enumerable, configurable — which is what makes it
    // replaceable, and it is the *same function object* as `values` rather than a copy.
    assert_eq!(
        run("var p = Object.getPrototypeOf(Int8Array.prototype); \
             var d = Object.getOwnPropertyDescriptor(p, Symbol.iterator); \
             d.writable + ',' + d.enumerable + ',' + d.configurable + ',' + (d.value === p.values)"),
        "true,false,true,true"
    );
    // Replaceable, which is the only reason to say so: taking it off stops `for`-`of` working.
    assert_eq!(
        run("var p = Object.getPrototypeOf(Int8Array.prototype); \
             delete p[Symbol.iterator]; \
             try { for (var v of new Int8Array([1])) { } } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_change_copies_answer_the_intrinsic_kind_where_the_others_answer_the_species() {
    // §23.2.3.32, §23.2.3.33 and §23.2.3.36 make their copy with `TypedArrayCreateSameType`, which
    // uses the **intrinsic** constructor for the element kind. `map`, `filter` and `slice` sitting
    // beside them consult `@@species` instead. So one receiver gives two different answers, and
    // that is the row that says the two operations are not the same one written twice.
    assert_eq!(
        run(
            "class Sub extends Uint8Array {} var s = new Sub(2);              (s.toReversed() instanceof Sub) + ',' + (s.toSorted() instanceof Sub)              + ',' + (s.with(0, 1) instanceof Sub) + '|'              + (s.map(function (x) { return x; }) instanceof Sub)              + ',' + (s.slice() instanceof Sub)"
        ),
        "false,false,false|true,true"
    );
    // …and what they answer is still the right kind, just not the subclass.
    assert_eq!(
        run(
            "class Sub extends Uint8Array {}              Object.prototype.toString.call(new Sub(2).toReversed()) + ','              + (new Sub(2).toReversed() instanceof Uint8Array)"
        ),
        "[object Uint8Array],true"
    );
    // Each leaves the array it was given alone — the whole point of the three.
    assert_eq!(
        run(
            "var a = new Int8Array([3, 1, 2]);              a.toReversed().join(',') + '|' + a.toSorted().join(',') + '|'              + a.with(0, 9).join(',') + '|' + a.join(',')"
        ),
        "2,1,3|1,2,3|9,1,2|3,1,2"
    );
    // §23.2.3.33's default order is **numeric**, where §23.1.3.34's is the spelling — so a
    // TypedArray sorts 10 after 9 and an Array does not.
    assert_eq!(
        run(
            "new Int8Array([10, 9, 1]).toSorted().join(',') + '|'              + [10, 9, 1].toSorted().join(',')"
        ),
        "1,9,10|1,10,9"
    );
    assert_eq!(
        run("new Int8Array([3, 1, 2]).toSorted(function (a, b) { return b - a; }).join(',')"),
        "3,2,1"
    );
    // §23.2.3.36 step 7 — an index outside the array is a RangeError, and a negative counts back.
    assert_eq!(
        run("new Int8Array([1, 2, 3]).with(-1, 9).join(',')"),
        "1,2,9"
    );
    for bad in ["3", "-4", "Infinity", "-Infinity"] {
        assert_eq!(
            run(&format!(
                "try {{ new Int8Array([1, 2, 3]).with({bad}, 0); }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "with({bad})"
        );
    }
    // The value is stored as the element kind stores it, so 300 into a `Uint8Array` is 44.
    assert_eq!(run("new Uint8Array([1, 2]).with(0, 300).join(',')"), "44,2");
    // §23.2.3.33 step 1 comes **before** `ValidateTypedArray`, so a bad comparator is reported as
    // one even when `this` is not a TypedArray at all — which is the only way to tell the order.
    assert_eq!(
        run("try { Int8Array.prototype.toSorted.call([1, 2], 1); } catch (e) { e.message }"),
        "the comparator is not a function"
    );
    assert_eq!(
        run("try { Int8Array.prototype.toSorted.call([1, 2]); } catch (e) { e.message }"),
        "this is not a TypedArray"
    );
    // A detached buffer is refused by all three.
    for method in ["toReversed()", "toSorted()", "with(0, 1)"] {
        assert_eq!(
            run(&format!(
                "var b = new ArrayBuffer(8); var a = new Int8Array(b); b.transfer();                  try {{ a.{method}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{method} on a detached buffer"
        );
    }
}
