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
    // §23.2.3.32 does not describe a method: it says the value of `toString` **is**
    // `%Array.prototype.toString%`. So the identity is the test, and a copy that behaved the same
    // would still be wrong — which is what a program comparing the two sees, and what this used to
    // answer `false` to.
    assert_eq!(
        run("Object.getPrototypeOf(Int8Array.prototype).toString === Array.prototype.toString"),
        "true"
    );
    // …and being that function means §23.1.3.36 step 2 reads whatever `join` currently *is*, so
    // replacing it changes what `toString` answers.
    assert_eq!(
        run(
            "var a = new Int8Array([1, 2]); a.join = function () { return 'replaced'; }; \
             a.toString()"
        ),
        "replaced"
    );
    // **Step 3 falls back rather than refusing**, which is where the copy differed and where this
    // assertion used to pin the copy's TypeError as though it were the clause. A `join` that is not
    // callable sends `toString` to `%Object.prototype.toString%`, which answers by @@toStringTag.
    assert_eq!(
        run("var a = new Int8Array([1, 2]); a.join = 1; a.toString()"),
        "[object Int8Array]"
    );
    assert_eq!(
        run("var a = new BigInt64Array(1); a.join = null; a.toString()"),
        "[object BigInt64Array]"
    );
    // The alias is defined rather than installed with the other methods, so it has to carry the
    // ordinary built-in attributes itself — writable, not enumerable, configurable. Getting them
    // wrong would leave `toString` unreplaceable, or make it show up in a `for...in`.
    for (attribute, expected) in [
        ("writable", "true"),
        ("enumerable", "false"),
        ("configurable", "true"),
    ] {
        assert_eq!(
            run(&format!(
                "Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Int8Array.prototype), \
                 'toString').{attribute}"
            )),
            expected
        );
    }
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

#[test]
fn a_canonical_index_stops_the_walk_for_every_operation_and_not_only_a_read() {
    // §10.4.5.2 and §10.4.5.5 — an index a TypedArray does not have is *absent*, not inherited.
    // The read has always said so; `in` and assignment have to say the same, and they say it in
    // their own walks now that a proxy on a chain means the heap cannot do the walking.
    assert_eq!(
        run("Int32Array.prototype[9] = 'inherited'; 9 in new Int32Array(2)"),
        "false"
    );
    // A non-canonical key is an ordinary one and does reach the prototype.
    assert_eq!(
        run("Int32Array.prototype.tag = 'inherited'; 'tag' in new Int32Array(2)"),
        "true"
    );
    // An assignment stops there too, so the write lands on the receiver rather than being refused
    // by a non-writable property further along the chain.
    assert_eq!(
        run(
            "Object.defineProperty(Int32Array.prototype, 9, {value: 'fixed', writable: false}); \
             var child = Object.create(new Int32Array(2)); child[9] = 5; child[9]"
        ),
        "5"
    );
}

#[test]
fn a_bigint_typed_array_holds_bigints_and_not_numbers() {
    // §23.2.1's `[[ContentType]]`, from the outside. Two of the eleven kinds hold §6.1.6.2's type,
    // and an element read out of one is a BigInt rather than a Number — which is the whole reason
    // an element is a value and not an `f64`.
    assert_eq!(run("typeof new BigInt64Array(1)[0]"), "bigint");
    assert_eq!(
        run("var a = new BigInt64Array(2); a[0] = -1n; a[1] = 7n; a[0] + ',' + a[1]"),
        "-1,7"
    );
    // Eight bytes each, and §23.2.6.2's `BYTES_PER_ELEMENT` says so on the constructor.
    assert_eq!(
        run("var a = new BigUint64Array(3); \
             a.length + ',' + a.byteLength + ',' + BigUint64Array.BYTES_PER_ELEMENT"),
        "3,24,8"
    );
    // The two are one buffer read two ways, exactly as the `DataView` pair is: the same eight
    // bytes are `-1n` signed and the largest `u64` unsigned.
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = -1n; \
             String(new BigUint64Array(a.buffer)[0])"),
        "18446744073709551615"
    );
    // A value too large for the slot takes its low bits — §7.1.15's `ToBigInt64`, which is what a
    // fixed width means and is not a refusal.
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = 2n ** 63n; String(a[0])"),
        "-9223372036854775808"
    );
    // §23.2.3.32's tag names the kind, so `Object.prototype.toString` tells the two apart.
    assert_eq!(
        run("Object.prototype.toString.call(new BigUint64Array(1))"),
        "[object BigUint64Array]"
    );
}

#[test]
fn the_two_numeric_types_refuse_each_other_at_every_way_into_a_typed_array() {
    // §10.4.5.16 step 1 — a write converts by the array's content type, and §7.1.4 throws for
    // *every* BigInt while §7.1.13 throws for *every* Number. So the refusal is a fact about the
    // pair of types and not about the value, and it is the same refusal by every route in.
    //
    // A plain assignment. Silent success here would be the worst outcome of the lot: §10.4.5.5
    // discards an out-of-range write without complaint, so a missing conversion would look like
    // one of those and write nothing while the program believed it had.
    assert_eq!(
        run("var a = new BigInt64Array(1); try { a[0] = 1 } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var a = new Int8Array(1); try { a[0] = 1n } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and the conversion runs even for an index the array does not have, which §10.4.5.5 step 1.b
    // is explicit about: the write goes nowhere but the `valueOf` still happened.
    assert_eq!(
        run("var seen = 'no'; var a = new Int8Array(1); \
             a[99] = { valueOf: function () { seen = 'yes'; return 1 } }; seen"),
        "yes"
    );
    // Every method that takes a value asks the same question.
    for source in [
        "new BigInt64Array(2).fill(1)",
        "new BigInt64Array(2).set([1])",
        "new BigInt64Array(2).with(0, 1)",
        "BigInt64Array.from([1])",
        "BigInt64Array.of(1)",
        "new BigInt64Array(new Int8Array(2))",
        "new Int8Array(new BigInt64Array(2))",
        "new BigInt64Array(2).set(new Int8Array(2))",
        "new BigInt64Array(2).map(function () { return 1 })",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source} }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source}"
        );
    }
    // And each of them works when handed the type the array does hold, so the refusals above are
    // the content-type check and not the method being broken.
    assert_eq!(run("new BigInt64Array(2).fill(1n).join()"), "1,1");
    assert_eq!(run("BigInt64Array.from([1n]).join()"), "1");
    assert_eq!(run("BigInt64Array.of(1n).join()"), "1");
    assert_eq!(run("new BigInt64Array(2).with(0, 5n).join()"), "5,0");
    assert_eq!(
        run("new BigInt64Array([1n]).map(function (v) { return v * 3n }).join()"),
        "3"
    );
    assert_eq!(
        run("var a = new BigInt64Array(2); a.set([7n], 1); a.join()"),
        "0,7"
    );
    assert_eq!(
        run("new BigUint64Array(new BigInt64Array([3n])).join()"),
        "3"
    );
}

#[test]
fn a_define_at_an_element_refuses_the_other_numeric_type_by_throwing_and_not_by_answering_false() {
    // §10.4.5.3 step 1.b.v hands the value to §10.4.5.16, which **throws** — where an index the
    // array does not have is a refusal that answers `false`. A program can tell the two apart, and
    // folding the first into the second would turn a TypeError into a quiet `false`.
    assert_eq!(
        run("var a = new BigInt64Array(1); \
             try { Reflect.defineProperty(a, 0, { value: 1 }) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var a = new Int8Array(1); \
             try { Reflect.defineProperty(a, 0, { value: 1n }) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // The refusal that is *not* a throw, for contrast: an index out of range.
    assert_eq!(
        run("Reflect.defineProperty(new BigInt64Array(1), 9, { value: 1n })"),
        "false"
    );
    // …and the define that works, which is what says the throws above are about the type.
    assert_eq!(
        run("var a = new BigInt64Array(1); \
             Reflect.defineProperty(a, 0, { value: 42n }) + ',' + a[0]"),
        "true,42"
    );
    // A value of neither numeric type is left as it was: §7.1.13 would *parse* a String and
    // §7.1.4 would convert one, and a define carries a value with no interpreter to run either
    // conversion with. So the two types refuse each other here and nothing else is judged.
    assert_eq!(
        run("var a = new Int8Array(1); Reflect.defineProperty(a, 0, { value: {} }) + ',' + a[0]"),
        "true,0"
    );
    // §10.4.5.1 — the descriptor read back out carries a BigInt, and all three attributes are
    // true, which is what makes an element writable and configurable like any other.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(new BigInt64Array(1), 0); \
             typeof d.value + ',' + d.writable + ',' + d.enumerable + ',' + d.configurable"
        ),
        "bigint,true,true,true"
    );
}

#[test]
fn a_walk_over_a_bigint_array_sees_bigints_and_orders_them_as_bigints() {
    // §23.2.3.29's default order for the type with no `NaN` and no negative zero: the plain
    // comparison. An engine that sorted these as Numbers would go through `f64`, where
    // `2n ** 63n` and `2n ** 63n + 1n` are the same value and would sort as equal.
    assert_eq!(
        run("new BigInt64Array([5n, -3n, 0n, 12n]).sort().join()"),
        "-3,0,5,12"
    );
    assert_eq!(
        run(
            "var a = new BigUint64Array([0n, 2n ** 64n - 1n, 1n]); a.sort(); \
             String(a[0]) + ',' + String(a[1]) + ',' + String(a[2])"
        ),
        "0,1,18446744073709551615"
    );
    // A comparator is handed the elements as values, so it can do BigInt arithmetic on them.
    assert_eq!(
        run("new BigInt64Array([1n, 3n, 2n]) \
             .sort(function (x, y) { return x < y ? 1 : -1 }).join()"),
        "3,2,1"
    );
    // §7.2.15 — a search compares *types* first, so the other numeric type finds nothing however
    // equal the values look. `indexOf(-3)` on an array holding `-3n` is -1.
    assert_eq!(
        run("var a = new BigInt64Array([5n, -3n]); \
             a.indexOf(-3n) + ',' + a.indexOf(-3) + ',' + a.includes(5n) + ',' + a.includes(5)"),
        "1,-1,true,false"
    );
    // …and neither does anything that is not a numeric at all, by the same step.
    assert_eq!(
        run("var a = new BigInt64Array([5n]); a.indexOf('5') + ',' + a.lastIndexOf(5n)"),
        "-1,0"
    );
    // §23.2.3.16 — `join` renders an element with §7.1.17, which for a BigInt is its digits and
    // not whatever a Number's rendering would make of the bits.
    assert_eq!(
        run("new BigUint64Array([2n ** 64n - 1n]).join()"),
        "18446744073709551615"
    );
    // The callback-driven walks and the folds all carry the element through unchanged.
    assert_eq!(
        run("String(new BigInt64Array([1n, 2n, 3n]).reduce(function (t, v) { return t + v }, 0n))"),
        "6"
    );
    assert_eq!(
        run("new BigInt64Array([1n, 2n, 3n]).filter(function (v) { return v > 1n }).join()"),
        "2,3"
    );
    assert_eq!(
        run("String(new BigInt64Array([1n, 2n]).find(function (v) { return v === 2n }))"),
        "2"
    );
    // …and the copies, which move elements without a program ever seeing one.
    assert_eq!(
        run("new BigInt64Array([1n, 2n, 3n]).slice(1).join()"),
        "2,3"
    );
    assert_eq!(
        run("new BigInt64Array([1n, 2n, 3n]).reverse().join()"),
        "3,2,1"
    );
    assert_eq!(
        run("new BigInt64Array([1n, 2n, 3n]).copyWithin(0, 2).join()"),
        "3,2,3"
    );
    assert_eq!(
        run("new BigInt64Array([1n, 2n]).toReversed().join() + ';' \
             + new BigInt64Array([2n, 1n]).toSorted().join()"),
        "2,1;1,2"
    );
    assert_eq!(run("String(new BigInt64Array([1n, 2n]).at(-1))"), "2");
    assert_eq!(run("[...new BigInt64Array([1n, 2n])].join()"), "1,2");
}

#[test]
fn a_species_of_the_other_content_type_is_refused_before_anything_is_copied_into_it() {
    // §23.2.4.2 step 4 — `TypedArraySpeciesCreate` checks the content type, which is what lets
    // `slice`, `map` and `filter` copy elements across without each asking again. Without it the
    // copy would reach a mismatched buffer and be discarded element by element, and the caller
    // would receive a zeroed array of the right length and no complaint at all.
    let species = "var a = new BigInt64Array([1n, 2n]); \
                   a.constructor = function () {}; \
                   a.constructor[Symbol.species] = Int8Array; ";
    for method in ["a.slice(0)", "a.filter(function () { return true })"] {
        assert_eq!(
            run(&format!(
                "{species} try {{ {method} }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{method}"
        );
    }
    // A species of the *same* content type is fine, which is what says the check is about the
    // content type and not about species being consulted at all.
    assert_eq!(
        run(
            "var a = new BigInt64Array([1n, 2n]); a.constructor = function () {}; \
             a.constructor[Symbol.species] = BigUint64Array; \
             var b = a.slice(0); b.constructor.name + ',' + b.join()"
        ),
        "BigUint64Array,1,2"
    );
}

#[test]
fn slice_notices_a_buffer_the_species_lookup_detached_but_only_when_it_would_copy() {
    // §23.2.3.27 step 10 — the detached check sits *after* `TypedArraySpeciesCreate` and *inside*
    // `if count > 0`, and both halves are observable from one script.
    //
    // §7.3.22 reads `constructor` off the receiver, so a getter there runs in the middle of
    // `slice` and can detach the very buffer the copy is about to read. Checked before the species
    // instead, this would answer a copy of what a detached buffer used to hold.
    assert_eq!(
        run("var a = new Int8Array(4); \
             Object.defineProperty(a, 'constructor', { get: function () { a.buffer.transfer(); } }); \
             try { a.slice(); 'no throw' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a slice of *nothing* must not throw, because step 10 never runs. The species is still
    // made and still detaches the buffer on the way, so the only difference between this row and
    // the one above is the count — which is exactly what the guard is.
    assert_eq!(
        run("var a = new Int8Array(0); \
             Object.defineProperty(a, 'constructor', { get: function () { a.buffer.transfer(); } }); \
             var out = a.slice(); out.length + ',' + out.constructor.name"),
        "0,Int8Array"
    );
    // The same boundary reached by asking for an empty range of a non-empty array: the receiver's
    // buffer is detached and there is still nothing to copy, so there is still nothing to refuse.
    assert_eq!(
        run("var a = new Int8Array(4); \
             Object.defineProperty(a, 'constructor', { get: function () { a.buffer.transfer(); } }); \
             a.slice(2, 2).length"),
        "0"
    );
}

#[test]
fn a_copy_of_the_same_kind_is_the_intrinsic_and_not_whatever_the_global_now_names() {
    // §23.2.4.3 `TypedArrayCreateSameType` says *intrinsic*, and a script may write
    // `globalThis.Int8Array = …` at any time. Looked up by name, `toSorted` would build whatever
    // the name had come to mean; the realm took the nine before a script ran.
    assert_eq!(
        run(
            "var a = new Int8Array([2, 1]); Int8Array = function () { return {}; }; \
             a.toSorted().constructor.name"
        ),
        "Int8Array"
    );
    // §23.2.4.2's default is the same intrinsic, and `map` reaches it through `@@species` — so a
    // `constructor` that is not a constructor at all is harmless when the species says nothing.
    assert_eq!(
        run("var a = new Int8Array([1, 2]); a.constructor = {}; \
             a.map(function (v) { return v; }).constructor.name"),
        "Int8Array"
    );
}

#[test]
fn a_tracking_view_is_out_of_bounds_when_its_offset_is_past_the_buffer() {
    // §10.4.5.2 step 8 has two disjuncts and a **tracking** view is subject to the first. Step 6
    // gives it a `byteOffsetEnd` of the buffer's own length, so its end can never hang off — but
    // its *start* can, and `new Uint8Array(rab, 4)` after `rab.resize(2)` begins past everything
    // there is. No length it could follow makes that a window.
    let out_of_bounds = |method: &str| {
        run(&format!(
            "var rab = new ArrayBuffer(8, {{maxByteLength: 16}}); \
             var t = new Uint8Array(rab, 4); rab.resize(2); \
             try {{ t.{method}; 'no error' }} catch (e) {{ e.constructor.name }}"
        ))
    };
    // Every method that begins with `ValidateTypedArray` refuses it — which is most of §23.2.3,
    // including the three that read nothing at all.
    for method in [
        "keys()",
        "values()",
        "entries()",
        "at(0)",
        "fill(1)",
        "copyWithin(0, 1)",
        "slice(0)",
        "indexOf(1)",
        "includes(1)",
        "join(',')",
        "reverse()",
        "sort()",
        "map(function (x) { return x })",
        "filter(function () { return true })",
        "forEach(function () {})",
        "set([1])",
        "toLocaleString()",
    ] {
        assert_eq!(out_of_bounds(method), "TypeError", "{method}");
    }
    // `subarray` is deliberately not on that list: §23.2.3.30 step 6 gives an out-of-bounds source
    // a length of **zero** rather than calling `ValidateTypedArray`, so it goes on to build a view
    // — and the view it would build starts past the buffer, which §23.2.5.1 refuses with a
    // **RangeError**. A different error from a different clause, and test262 asserts it.
    assert_eq!(out_of_bounds("subarray(0)"), "RangeError");
    // …and `length` and `byteOffset` are *not* among them: their getters answer rather than
    // throwing, so a program can still ask what became of it. **Both answer zero**, which this row
    // used to deny: §23.2.3.3 step 4 returns `+0` for an out-of-bounds view rather than the stored
    // offset, exactly as §23.2.3.2 and §23.2.3.18 do for the two lengths. The old row asserted `4`
    // — what ViperJS did, worded as though the clause only promised *an* answer — and the
    // conformance suite is what caught it.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab, 4); rab.resize(2); t.length + ',' + t.byteOffset"),
        "0,0"
    );
    // In bounds it is the offset it was given, which is what keeps the row above about being out.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab, 4); t.length + ',' + t.byteOffset"),
        "4,4"
    );
    // **The boundary is `>` and not `>=`.** An offset landing exactly at the end is a window on the
    // empty remainder: in bounds, and with no elements. Those are different answers, and this is
    // the row that tells them apart.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab, 4); rab.resize(4); \
             Array.from(t.keys()).length + ',' + t.length"),
        "0,0"
    );
    // A tracking view with **no** offset is never out of bounds however small the buffer gets,
    // which is what the old reading got right and is why it survived.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab); rab.resize(2); \
             Array.from(t.keys()).join(',') + '|' + t.length"),
        "0,1|2"
    );
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab); rab.resize(0); \
             Array.from(t.keys()).length + ',' + t.length"),
        "0,0"
    );
    // Growing back puts it in bounds again — the question is asked of the buffer as it is now, and
    // nothing is remembered about its having been out.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab, 4); rab.resize(2); rab.resize(8); \
             Array.from(t.keys()).length"),
        "4"
    );
    // A view over a **fixed** buffer cannot be out of bounds at all, there being nothing to move.
    assert_eq!(
        run("var t = new Uint8Array(new ArrayBuffer(8), 4); Array.from(t.keys()).length"),
        "4"
    );
    // §25.3's `DataView` is the same rule over the same window, and ViperJS shares the code.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var d = new DataView(rab, 4); rab.resize(2); \
             try { d.getUint8(0); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a *fixed-length* view is out of bounds by the second disjunct, which this must not have
    // broken: its end hangs off even though its start does not.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, {maxByteLength: 16}); \
             var t = new Uint8Array(rab, 0, 4); rab.resize(2); \
             try { Array.from(t.keys()); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_define_and_a_delete_read_the_length_the_buffer_has_now() {
    // §10.4.5.9 `IsValidIntegerIndex` step 2 — an index of a view that is **out of bounds** is not
    // a valid one, whatever the view was created with. Every *read* path already resolved the
    // length before asking; `[[DefineOwnProperty]]` and `[[Delete]]` asked the stored number, which
    // a resize makes stale, and so answered about elements that are no longer there.
    //
    // A define is where this becomes observable without a method to refuse first: §7.1.19 converts
    // the key by running the program's own `toString`, which is free to shrink the buffer between
    // the view being handed over and the index being tested.
    assert_eq!(
        run("var rab = new ArrayBuffer(4, {maxByteLength: 8}); \
             var t = new Uint8Array(rab, 0, 4); \
             var evil = {toString: function () { rab.resize(2); return '0' }}; \
             try { Object.defineProperty(t, evil, {value: 8}); 'no error' } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a *tracking* view is refused for the ordinary reason instead: it follows the shorter
    // buffer, so the index is simply past its end.
    assert_eq!(
        run("var rab = new ArrayBuffer(4, {maxByteLength: 8}); \
             var t = new Uint8Array(rab, 0); \
             var evil = {toString: function () { rab.resize(2); return '3' }}; \
             try { Object.defineProperty(t, evil, {value: 8}); 'no error' } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §10.4.5.4 — deleting an index the view does not have **succeeds**, vacuously. Read from the
    // stale length this said `false`, which is the claim that a property is there and refuses to go.
    assert_eq!(
        run("var rab = new ArrayBuffer(4, {maxByteLength: 8}); \
             var t = new Uint8Array(rab, 0); rab.resize(2); delete t[3]"),
        "true"
    );
    assert_eq!(run("var t = new Uint8Array(4); delete t[1]"), "false");
    // A buffer that has *grown* gives a tracking view the indices it grew into, so this is not
    // "refuse more" — it is the same question asked of today's window in both directions.
    assert_eq!(
        run("var rab = new ArrayBuffer(4, {maxByteLength: 8}); \
             var t = new Uint8Array(rab, 0); rab.resize(8); \
             Reflect.defineProperty(t, '7', {value: 5}) + '|' + t[7]"),
        "true|5"
    );
    assert_eq!(
        run("var t = new Uint8Array(4); Object.defineProperty(t, '1', {value: 9}); t[1]"),
        "9"
    );
    assert_eq!(
        run("Reflect.defineProperty(new Uint8Array(4), '9', {value: 1})"),
        "false"
    );
}

#[test]
fn a_coercion_that_detaches_the_buffer_is_answered_per_method() {
    // §23.2.3 converts each argument and then works from a length read *before* it, and what the
    // clause does about that **differs per method** — which is the whole cost of this area and the
    // reason it cannot be one shared check.
    //
    // `copyWithin` runs `ValidateTypedArray` again (step 14.b) and throws.
    assert_eq!(
        run(
            "var t = new Uint8Array(8);              var e = { valueOf: function () { t.buffer.transfer(); return 0 } };              try { t.copyWithin(0, e, 4); 'no error' } catch (x) { x.constructor.name }"
        ),
        "TypeError"
    );
    // `fill` and `slice` throw as well, and already did.
    assert_eq!(
        run(
            "var t = new Uint8Array(8);              var e = { valueOf: function () { t.buffer.transfer(); return 0 } };              try { t.fill(1, e); 'no error' } catch (x) { x.constructor.name }"
        ),
        "TypeError"
    );
    // …and the searches do **not**. §23.2.3.15 step 11 reads each element live, a detached buffer
    // answers `undefined` for every index, and `undefined` matches no numeric — so the answer is
    // `-1`, not an error and not the elements that were there a moment ago.
    assert_eq!(
        run(
            "var t = new Uint8Array(8);              var e = { valueOf: function () { t.buffer.transfer(); return 0 } };              t.indexOf(0, e)"
        ),
        "-1"
    );
    assert_eq!(
        run(
            "var t = new Uint8Array(8);              var e = { valueOf: function () { t.buffer.transfer(); return 0 } };              t.includes(0, e)"
        ),
        "false"
    );
    assert_eq!(
        run(
            "var t = new Uint8Array(8);              var e = { valueOf: function () { t.buffer.transfer(); return 0 } };              t.lastIndexOf(0, e)"
        ),
        "-1"
    );
    // An ordinary search is unchanged, which is what keeps the above about the detach.
    assert_eq!(
        run(
            "var t = new Uint8Array([1, 2, 3, 2]);              t.indexOf(2) + '|' + t.lastIndexOf(2) + '|' + t.includes(3) + '|' + t.indexOf(9)"
        ),
        "1|3|true|-1"
    );
    // `includes` finds a NaN where `indexOf` cannot — §23.2.3.14 uses `SameValueZero` and the other
    // two use strict equality. Reading the elements later must not have lost that.
    assert_eq!(
        run("var t = new Float64Array([NaN]); t.includes(NaN) + '|' + t.indexOf(NaN)"),
        "true|-1"
    );
    // The length the negative-index arithmetic uses is still step 3's, taken before the coercion —
    // and the elements are read after it, so the two may disagree. A read past what is left finds
    // nothing rather than reaching out of bounds, which DR-0002 makes a refusal and not a panic.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8, { maxByteLength: 16 }); var t = new Uint8Array(b);              var e = { valueOf: function () { b.resize(2); return 0 } };              t.indexOf(0, e)"
        ),
        "0"
    );
}

#[test]
fn a_detached_buffer_leaves_a_view_with_no_indices_at_all() {
    // §10.4.5.1 `IsValidIntegerIndex` step 1 — detachment makes every index invalid, so the three
    // internal methods that ask it answer as they do for a short array. `view_out_of_bounds`
    // deliberately says `false` for a detached buffer so its callers can raise their own error, and
    // reading *that* as the whole question left a resolved view holding its stored length.
    //
    // §10.4.5.5 step 1.a.i — a define at an index the array does not have is refused.
    assert_eq!(
        run("var t = new Int8Array(4); t.buffer.transfer(); \
             Reflect.defineProperty(t, '2', { configurable: true, enumerable: true, writable: true, value: 1 })"),
        "false"
    );
    // §10.4.5.3 step 2.a — and deleting one *succeeds*, because what is not there cannot be in the
    // way. The two answers are opposite and both come from the same test, which is why a stale
    // length got each of them exactly wrong.
    assert_eq!(
        run("var t = new Int8Array(4); t.buffer.transfer(); \
             (delete t[0]) + '|' + (delete t[9]) + '|' + (delete t['-0'])"),
        "true|true|true"
    );
    // §10.4.5.6 step 2 — and it names none of them. The list used to be `0,1,2,3` with every one of
    // their descriptors `undefined`: keys the object did not have.
    assert_eq!(
        run("var t = new Int8Array(4); t.buffer.transfer(); \
             Object.getOwnPropertyNames(t).length + '|' + (Object.getOwnPropertyDescriptor(t, '0') === undefined)"),
        "0|true"
    );
    // An attached array still has all three, which is what keeps this about detachment rather than
    // about the indices disappearing.
    assert_eq!(
        run("var t = new Int8Array(4); \
             Reflect.defineProperty(t, '2', { configurable: true, enumerable: true, writable: true, value: 1 }) \
             + '|' + (delete t[0]) + '|' + Object.getOwnPropertyNames(t).join(',')"),
        "true|false|0,1,2,3"
    );
}

#[test]
fn a_write_through_another_receiver_never_reaches_the_buffer() {
    // §10.4.5.4 step 2.b.i — the element is written only when `SameValue(O, Receiver)`. ViperJS took
    // this path before consulting the receiver at all, on a comment that said "the element belongs
    // to the buffer and no receiver can move it elsewhere" — which is the *define* clause's rule,
    // not this one.
    assert_eq!(
        run("var t = new Int8Array(4); var r = {}; \
             Reflect.set(t, '0', 42, r) + '|' + t[0] + '|' + r[0]"),
        "true|0|42"
    );
    // Step 2.b.ii — an index the array does not have goes nowhere and is reported as accepted, and
    // the value is **not converted**: the receiver never hears about it either.
    assert_eq!(
        run("var t = new Int8Array(4); var r = {}; var seen = 0; \
             var v = { valueOf: function () { seen = 1; return 1 } }; \
             Reflect.set(t, '9', v, r) + '|' + r.hasOwnProperty('9') + '|' + seen"),
        "true|false|0"
    );
    // §10.1.9.2 steps 3.d and 3.f go through the *receiver's* own internal methods, so a TypedArray
    // receiver takes the value as an element — converted for its own kind, not the target's.
    assert_eq!(
        run("var t = new Int8Array(4); var r = new Int8Array(2); \
             Reflect.set(t, '0', new Number(2.3), r) + '|' + t[0] + '|' + r[0]"),
        "true|0|2"
    );
    // And a receiver too short for the index refuses the write outright — §10.4.5.5 step 1.a.i
    // reached through `CreateDataProperty`, which is a `false` rather than a silent discard.
    assert_eq!(
        run("var t = new Int8Array(4); var r = new Int8Array(1); \
             Reflect.set(t, '1', 5, r) + '|' + r.hasOwnProperty('1')"),
        "false|false"
    );
    // An ordinary assignment has the two the same object, which is the path every program takes.
    assert_eq!(run("var t = new Int8Array(2); t[0] = 5; t[0]"), "5");
}

#[test]
fn a_define_at_an_element_converts_its_value_and_does_so_last() {
    // §10.4.5.5 step 1.f hands the value to §10.4.5.16, which converts it. The heap cannot run a
    // conversion — DR-0011's seam — and its doc said a define "carries a value that is already a
    // Value, so there is no conversion to run", so everything that was neither Number nor BigInt
    // was stored as `NaN`.
    assert_eq!(
        run("var t = new Int8Array(4); Object.defineProperty(t, '0', { value: '7' }); t[0]"),
        "7"
    );
    assert_eq!(
        run(
            "var t = new Int8Array(4); var seen = 0; var caught = 'not thrown'; \
             try { Object.defineProperty(t, '0', { value: { valueOf: function () { seen = 1; throw 'boom' } } }) } \
             catch (e) { caught = e } \
             seen + '|' + caught"
        ),
        "1|boom"
    );
    // Steps 1.a to 1.e come **first**, and each of them refuses without converting anything. That
    // is the opposite order from `[[Set]]`, where §10.4.5.16 runs before the index is judged — so
    // counting the `valueOf` calls is what tells the two clauses apart.
    assert_eq!(
        run("var t = new Int8Array(4); var seen = 0; \
             var v = { valueOf: function () { seen++; return 1 } }; \
             Reflect.defineProperty(t, '0', { value: v, configurable: false }) + '|' \
             + Reflect.defineProperty(t, '9', { value: v }) + '|' + seen"),
        "false|false|0"
    );
    // A descriptor with **no** `[[Value]]` field skips step 1.f altogether and step 1.g answers
    // `true` having changed nothing. That is the one define that still reaches the heap — the
    // conversion is what `Vm::define_through` intercepts, and there is none to run — so it is also
    // the only thing keeping the heap's half of the clause from being a refusal and nothing else.
    assert_eq!(
        run("var t = new Int8Array(4); t[0] = 5; \
             Reflect.defineProperty(t, '0', {}) + '|' + t[0] + '|' \
             + Reflect.defineProperty(t, '0', { writable: true }) + '|' \
             + Reflect.defineProperty(t, '9', {})"),
        "true|5|true|false"
    );
    // The same conversion may detach the buffer, and then there is nowhere to write: the define
    // still answers `true` — §10.4.5.16 step 3 leaves an invalid index alone — and the element is
    // gone rather than holding what was converted.
    assert_eq!(
        run("var t = new Int8Array(4); \
             var v = { valueOf: function () { t.buffer.transfer(); return 17 } }; \
             Reflect.defineProperty(t, '0', { value: v }) + '|' + t[0]"),
        "true|undefined"
    );
}

#[test]
fn a_tracking_view_takes_a_different_branch_from_a_fixed_one() {
    // §23.2.5.1 step 7 and step 8 are alternatives, not one branch with a flag. A view over a
    // **resizable** buffer with no explicit length tracks it, and step 7 has no modulo rule — so a
    // ten-byte resizable buffer is an `Int32Array` of two, where the same ten bytes fixed are a
    // RangeError. ViperJS ran step 8's checks over both and refused the first outright.
    assert_eq!(
        run(
            "var rab = new ArrayBuffer(10, { maxByteLength: 20 }); var ta = new Int32Array(rab); \
             ta.length + ',' + ta.byteLength"
        ),
        "2,8"
    );
    assert_eq!(
        run("var caught = 'none'; \
             try { new Int32Array(new ArrayBuffer(10)) } catch (e) { caught = e.constructor.name } caught"),
        "RangeError"
    );
    // …because a tracking view's length is recomputed from the buffer at every read and rounded
    // down to whole elements there. A remainder that is not a whole element is simply not
    // reported, which is why there is nothing for step 7 to refuse at the start.
    assert_eq!(
        run(
            "var rab = new ArrayBuffer(10, { maxByteLength: 20 }); var ta = new Int32Array(rab); \
             var before = ta.length; rab.resize(16); var grown = ta.length; rab.resize(6); \
             [before, grown, ta.length].join(',')"
        ),
        "2,4,1"
    );
    // An **explicit** length pins the window whatever the buffer does, so it takes step 8 even on
    // a resizable buffer — which is the half that keeps this about `length is undefined` rather
    // than about resizability.
    assert_eq!(
        run("var rab = new ArrayBuffer(10, { maxByteLength: 20 }); \
             new Int32Array(rab, 0, 2).length"),
        "2"
    );
    // Step 7.a — the one thing that can be wrong about a tracking view is beginning past the end,
    // and it is `>` rather than `>=`: an offset exactly at the end is a window on the empty
    // remainder, which has no elements and is not an error.
    assert_eq!(
        run("var rab = new ArrayBuffer(8, { maxByteLength: 20 }); \
             var caught = 'none'; \
             try { new Int32Array(rab, 12) } catch (e) { caught = e.constructor.name } \
             new Int32Array(rab, 8).length + ',' + caught"),
        "0,RangeError"
    );
}

#[test]
fn a_walk_caches_its_length_and_re_reads_every_element() {
    // §23.2.3.7 step 3 caches the **length** and step 6.b re-reads each *element* with
    // `Get(O, Pk)`. Two decisions and not one, which `fold` already spelled out and `walk` did
    // not: it snapshotted the elements, above a comment saying the clause "carries on with what it
    // had rather than turning the rest of the walk into `undefined`s" — which is what a snapshot
    // does and not what the clause says.
    //
    // So a callback that shrinks a resizable buffer still gets the number of turns the array had
    // when the walk began, and the turns past the new end are handed `undefined`.
    // The callback's answer has to be the one that does **not** short-circuit the method under
    // test, or the row measures the early exit instead of the walk: `every` runs on while it is
    // told `true`, and `some`, `find` and `findIndex` run on while they are told `false`.
    let shrinking = |method: &str, answer: &str| {
        run(&format!(
            "var rab = new ArrayBuffer(8, {{maxByteLength: 8}}); \
             var t = new Int8Array(rab, 0, 4); t[0] = 0; t[1] = 2; t[2] = 4; t[3] = 6; \
             var seen = []; \
             t.{method}(function (v, i) {{ seen.push(String(v)); if (i === 1) {{ rab.resize(3) }} return {answer} }}); \
             seen.join(',')"
        ))
    };
    for (method, answer) in [
        ("every", "true"),
        ("forEach", "true"),
        ("map", "true"),
        ("filter", "true"),
        ("some", "false"),
        ("find", "false"),
        ("findIndex", "false"),
    ] {
        assert_eq!(
            shrinking(method, answer),
            "0,2,undefined,undefined",
            "{method}"
        );
    }
    // A **tracking** view is the case that tells the two decisions apart from each other: the
    // count is still the one it started with, and the elements the shrunk buffer still holds are
    // read as themselves. Index 2 survives a resize to three bytes where index 3 does not.
    assert_eq!(
        run(
            "var rab = new ArrayBuffer(8, {maxByteLength: 8}); var t = new Int8Array(rab); \
             t[0] = 0; t[1] = 2; t[2] = 4; t[3] = 6; var seen = []; \
             t.forEach(function (v, i) { seen.push(String(v)); if (i === 1) { rab.resize(3) } }); \
             seen.join(',')"
        ),
        "0,2,4,undefined,undefined,undefined,undefined,undefined"
    );
    // And an array nothing resizes walks exactly as it did, which is what keeps this about the
    // resize rather than about the read.
    assert_eq!(
        run("var t = new Int8Array([1, 2, 3, 4]); var seen = []; \
             t.forEach(function (v) { seen.push(v) }); \
             seen.join(',') + '|' + Array.from(t.map(function (v) { return v * 2 })).join(',') \
             + '|' + Array.from(t.filter(function (v) { return v > 2 })).join(',')"),
        "1,2,3,4|2,4,6,8|3,4"
    );
}

#[test]
fn reading_an_element_asks_the_window_and_not_only_the_buffer() {
    // A view that is out of bounds resolves to a count of zero while its bytes are still there, so
    // a read that checks only the slice reads what the window no longer covers. The property path
    // asks `index_of` and gets the count; the direct read never did, and the two disagreed —
    // `t[2]` answered `undefined` while a walk handed the callback `4`.
    assert_eq!(
        run(
            "var rab = new ArrayBuffer(8, {maxByteLength: 8}); var t = new Int8Array(rab, 0, 4); \
             t[2] = 4; rab.resize(3); \
             var seen = 'unset'; \
             try { t.forEach(function () {}) } catch (e) { seen = e.constructor.name } \
             seen + ',' + t[2] + ',' + t.length"
        ),
        "TypeError,undefined,0"
    );
    // Still in bounds, the same read answers as it always did.
    assert_eq!(
        run(
            "var rab = new ArrayBuffer(8, {maxByteLength: 8}); var t = new Int8Array(rab, 0, 4); \
             t[2] = 4; rab.resize(4); t[2] + ',' + t.length"
        ),
        "4,4"
    );
}

#[test]
fn an_array_iterator_over_a_typed_array_refuses_a_buffer_that_went_away() {
    // §23.1.5.1 splits on what is being walked. A TypedArray goes through `ValidateTypedArray`,
    // which **throws** for a detached buffer — where reading `length` as a property answers `0`
    // and the walk simply ends. Those are the same answer for an array that ran out and a
    // different one for a buffer that went away underneath, and only the throw tells them apart.
    assert_eq!(
        run(
            "(function () { var b = new ArrayBuffer(8); var ta = new Int8Array(b);              var it = ta[Symbol.iterator](); it.next(); b.transfer();              try { it.next(); return 'no throw' } catch (e) { return e.constructor.name } })()"
        ),
        "TypeError"
    );
    // Before a single step, too: an empty walk over a detached buffer refuses rather than quietly
    // doing nothing, which is the same rule every §23.2 method begins with.
    for method in ["values", "keys", "entries"] {
        assert_eq!(
            run(&format!(
                "(function () {{ var b = new ArrayBuffer(8); var ta = new Int8Array(b);                  var it = ta.{method}(); b.transfer();                  try {{ it.next(); return 'no throw' }} catch (e) {{ return e.constructor.name }} }})()"
            )),
            "TypeError",
            "for `{method}`"
        );
    }
    // A window that no longer fits its buffer is refused on the same terms as a detached one —
    // §10.4.5.2, and the case a check for `detached` alone would miss.
    assert_eq!(
        run(
            "(function () { var b = new ArrayBuffer(8, {maxByteLength: 8});              var ta = new Int8Array(b, 4); var it = ta.keys(); it.next(); b.resize(2);              try { it.next(); return 'no throw' } catch (e) { return e.constructor.name } })()"
        ),
        "TypeError"
    );
    // …and a *tracking* view simply gets shorter, because it is never out of bounds. This is the
    // row that stops the refusal from being "any resize throws".
    assert_eq!(
        run(
            "(function () { var b = new ArrayBuffer(8, {maxByteLength: 8});              var ta = new Int8Array(b); var it = ta.keys(); it.next(); b.resize(2);              var out = []; var step; while (!(step = it.next()).done) out.push(step.value);              return out.join(','); })()"
        ),
        "1"
    );
}

#[test]
fn an_ordinary_array_is_still_walked_by_its_length_property_each_step() {
    // The other side of the split, and the reason it is a split rather than a replacement: an
    // Array's length is read with `Get` on **every** step, so one that shrinks mid-walk stops
    // early and one that grows keeps going.
    assert_eq!(
        run(
            "(function () { var a = [1, 2, 3, 4]; var out = [];              for (var x of a) { out.push(x); if (out.length === 2) a.length = 3; }              return out.join(','); })()"
        ),
        "1,2,3"
    );
    // An array-like walked through the same iterator answers to its own `length` too, which is
    // what would break if the TypedArray branch had been written as "take the internal length".
    assert_eq!(
        run(
            "(function () { var o = {length: 2, 0: 'a', 1: 'b'};              return Array.prototype.values.call(o).next().value; })()"
        ),
        "a"
    );
}

#[test]
fn set_reads_its_buffer_again_after_converting_the_offset() {
    // §23.2.3.26 steps 6 to 9, and the order is the whole of it: `ToIntegerOrInfinity(offset)` runs
    // a program, and steps 8 and 9 read the buffer **after** it. Everything the brand check learned
    // at step 5 describes a buffer that may no longer exist by then.
    //
    // The conversion must still have happened — a `set` that refused before running `valueOf` would
    // pass an assertion about the TypeError and fail this one about the count.
    assert_eq!(
        run(
            "var target = new Int8Array(2), source = new Int8Array(1), ran = 0;\
             var offset = { valueOf: function () { ran++; target.buffer.transfer(); return 0 } };\
             var name = ''; try { target.set(source, offset) } catch (e) { name = e.name }\
             name + ':' + ran"
        ),
        "TypeError:1"
    );
    // …and the *source* is asked the same question at §23.2.3.26.1 step 4, because the same
    // `valueOf` could as easily have detached its buffer instead.
    assert_eq!(
        run("var target = new Int8Array(2), source = new Int8Array(1);\
             var offset = { valueOf: function () { source.buffer.transfer(); return 0 } };\
             try { target.set(source, offset) } catch (e) { e.name }"),
        "TypeError"
    );
    // Step 7 comes first and does not care what happened to the buffer: a negative offset is a
    // RangeError even when the conversion detached everything on its way past.
    assert_eq!(
        run("var target = new Int8Array(2), source = new Int8Array(1);\
             var offset = { valueOf: function () { target.buffer.transfer(); return -1 } };\
             try { target.set(source, offset) } catch (e) { e.name }"),
        "RangeError"
    );
    // An offset of infinity saturates rather than wrapping, so the length check refuses it.
    assert_eq!(
        run("var target = new Int8Array(2);\
             try { target.set(new Int8Array(1), Infinity) } catch (e) { e.name }"),
        "RangeError"
    );
    // And the ordinary copy still works, which is what makes the four rows above about ordering
    // rather than about `set` having stopped functioning.
    assert_eq!(
        run(
            "var target = new Int8Array(3); target.set(new Int8Array([7, 8]), 1); target.join(',')"
        ),
        "0,7,8"
    );
}

#[test]
fn a_typed_array_constructor_branches_on_having_an_iterator_and_not_on_finding_one_that_works() {
    // §23.2.5.1 step 6.b — `GetMethod(object, @@iterator)`, and step 6.c runs only when there *is*
    // one. This was written as "try the iterable reading, and fall back to the array-like one if it
    // fails", which is a fallback on **failure** where the clause has one on **absence** — so every
    // error the walk could raise was caught and answered with a different construction.
    //
    // The message and not the constructor, in every row that throws. A silent fallback also ends in
    // a TypeError often enough that `assert.throws(TypeError, …)` passes against the bug, which is
    // the weakest assertion there is and the one this file keeps meeting.
    assert_eq!(
        run(
            "var o = function () {}; Object.defineProperty(o, Symbol.iterator, { get: function () { throw new Error('the getter ran') } }); try { new Float64Array(o); 'no error' } catch (e) { e.message }"
        ),
        "the getter ran"
    );
    // …and that row is not artificial: a **function** has a `length` of 0, so the discarded error
    // was replaced by an empty array rather than by anything that looked wrong.
    assert_eq!(
        run(
            "var o = function () {}; Object.defineProperty(o, Symbol.iterator, { get: function () { return undefined } }); new Float64Array(o).length"
        ),
        "0"
    );
    // §7.3.10 step 4 — present, not callable, and not `undefined` or `null`, is a TypeError. The
    // old reading swallowed it and built from `length` instead.
    assert_eq!(
        run(
            "var o = { length: 2, 0: 7, 1: 8 }; o[Symbol.iterator] = 5; try { new Float64Array(o).length + '' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // A throw from inside the walk is the program's, and reaches it.
    assert_eq!(
        run(
            "var o = {}; o[Symbol.iterator] = function () { throw new Error('called it') }; try { new Float64Array(o); 'no error' } catch (e) { e.message }"
        ),
        "called it"
    );
    assert_eq!(
        run(
            "var o = {}; o[Symbol.iterator] = function () { return { next: function () { throw new Error('stepped') } } }; try { new Float64Array(o); 'no error' } catch (e) { e.message }"
        ),
        "stepped"
    );
    // The *absence* branch, which is the one the fallback was standing in for. §7.3.10 reads
    // `undefined` and `null` as "there is none", so both take the array-like reading rather than
    // throwing — and an object with neither an iterator nor a `length` is an empty array, not an
    // error.
    assert_eq!(
        run(
            "var o = { length: 2, 0: 7, 1: 8 }; o[Symbol.iterator] = undefined; Array.prototype.join.call(new Float64Array(o), ',')"
        ),
        "7,8"
    );
    assert_eq!(
        run(
            "var o = { length: 2, 0: 7, 1: 8 }; o[Symbol.iterator] = null; Array.prototype.join.call(new Float64Array(o), ',')"
        ),
        "7,8"
    );
    assert_eq!(run("new Float64Array({}).length"), "0");
    // And where both are there the iterator wins, which is the branch order rather than a
    // preference: step 6.c is reached whenever there is a method at all.
    assert_eq!(
        run(
            "var o = { length: 2, 0: 'no', 1: 'no' }; o[Symbol.iterator] = function () { var n = 0; return { next: function () { return n < 3 ? { value: 90 + n++, done: false } : { done: true } } } }; Array.prototype.join.call(new Float64Array(o), ',')"
        ),
        "90,91,92"
    );
}

#[test]
fn the_iterator_method_a_typed_array_constructor_uses_is_read_exactly_once() {
    // §23.2.5.1 names `usingIterator` in step 6.b and uses it again in step 6.c, which is one read.
    // Reading it to decide the branch and again to walk with is invisible to every test above —
    // both reads answer the same method and the array comes out right — and a getter counts them.
    // That is why `iterable_to_list_with` takes the method rather than the iterable.
    assert_eq!(
        run(
            "var reads = 0; var o = {}; Object.defineProperty(o, Symbol.iterator, { get: function () { reads = reads + 1; return function () { var n = 0; return { next: function () { return n < 2 ? { value: n++, done: false } : { done: true } } } } } }); new Float64Array(o); reads"
        ),
        "1"
    );
}

#[test]
fn from_branches_the_same_way_the_constructor_does_and_set_does_not_branch_at_all() {
    // §23.2.2.1 steps 4 and 5 are the constructor's clause again, so `from` had the same fallback
    // on failure and the same consequence: the program's own error discarded and an array built
    // out of `length` instead.
    assert_eq!(
        run(
            "var o = function () {}; Object.defineProperty(o, Symbol.iterator, { get: function () { throw new Error('from asked') } }); try { Float64Array.from(o); 'no error' } catch (e) { e.message }"
        ),
        "from asked"
    );
    assert_eq!(
        run(
            "var o = { length: 2, 0: 7, 1: 8 }; Array.prototype.join.call(Float64Array.from(o), ',')"
        ),
        "7,8"
    );
    // §23.2.3.26.2 `SetTypedArrayFromArrayLike` is the one that is **not** the same clause: steps 3
    // to 5 are `ToObject`, `LengthOfArrayLike` and a loop of `Get`, and there is no `@@iterator`
    // anywhere in it. `set` read the iterator first and fell back, so an object carrying both wrote
    // what the iterator said — a wrong value, which no error could have warned about.
    assert_eq!(
        run(
            "var ta = new Float64Array(3); var asked = false; var src = { length: 2, 0: 11, 1: 22 }; src[Symbol.iterator] = function () { asked = true; var v = [90, 91], n = 0; return { next: function () { return n < 2 ? { value: v[n++], done: false } : { done: true } } } }; ta.set(src); Array.prototype.join.call(ta, ',') + '|' + asked"
        ),
        "11,22,0|false"
    );
    // …and an iterable with no `length` therefore writes nothing, which is what
    // `LengthOfArrayLike` of `undefined` comes to. An Array has both readings and they agree, which
    // is why every ordinary use of this looked right.
    assert_eq!(
        run(
            "var ta = new Float64Array(2); var src = {}; src[Symbol.iterator] = function () { var n = 0; return { next: function () { return n < 2 ? { value: 5, done: false } : { done: true } } } }; ta.set(src); Array.prototype.join.call(ta, ',')"
        ),
        "0,0"
    );
    assert_eq!(
        run("var ta = new Float64Array(3); ta.set([4, 5]); Array.prototype.join.call(ta, ',')"),
        "4,5,0"
    );
}

#[test]
fn join_takes_its_length_before_the_separator_and_its_elements_after() {
    // §23.2.3.16 steps 3 and 5, and the gap between them. Converting the separator may run a
    // `toString` that resizes the buffer; the count was already read, and the elements are read
    // afterwards — so the answer is that many separators with nothing between them.
    assert_eq!(
        run("var rab = new ArrayBuffer(3, { maxByteLength: 5 }); \
             var ta = new Int8Array(rab); var calls = 0; \
             var sep = { toString: function () { calls++; rab.resize(0); return '-' } }; \
             ta.join(sep) + '|' + calls"),
        "--|1"
    );
    // …and the ordinary case is unchanged, which is what says the rewrite did not make every
    // element empty.
    assert_eq!(run("new Int8Array([1, 2, 3]).join('-')"), "1-2-3");
    // A separator that *detaches* rather than shrinking reaches the same place by the other route:
    // §23.2.3.16 validates once, at step 2, so a buffer detached afterwards is not a TypeError —
    // it is three elements that are no longer there.
    assert_eq!(
        run("var ab = new ArrayBuffer(3); var ta = new Int8Array(ab); \
             var sep = { toString: function () { ab.transfer(); return '+' } }; \
             ta.join(sep)"),
        "++"
    );
    // …and a buffer already detached *before* the call still refuses, which is the step 2 half and
    // the row that stops the one above reading as "detaching is always fine".
    assert_eq!(
        run(
            "var ab = new ArrayBuffer(3); var ta = new Int8Array(ab); ab.transfer(); \
             var kind = 'none'; try { ta.join('-') } catch (e) { kind = e.constructor.name } kind"
        ),
        "TypeError"
    );
}
