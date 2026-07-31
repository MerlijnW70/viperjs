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
