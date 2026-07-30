//! §25.1 and §25.3 — `ArrayBuffer` and `DataView`.
//!
//! A buffer is bytes and nothing else; everything that can read them is a view laid over it. The
//! rows that matter are the ones where that separation shows: two views sharing a buffer, an
//! endianness the format chose rather than the machine, and an integer conversion that wraps.

use super::*;

#[test]
fn a_buffer_is_a_block_of_zeroed_bytes_and_nothing_else() {
    assert_eq!(run("new ArrayBuffer(8).byteLength"), "8");
    // §25.1.3.1 step 2 — `undefined` is 0, which is what makes the argument read as optional.
    assert_eq!(run("new ArrayBuffer().byteLength"), "0");
    // Zeroed, and that is not an implementation detail: a program may read every byte of a fresh
    // buffer and the answer has to be 0 rather than whatever was in that memory before.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.getUint8(0) + ',' + v.getUint32(0)"),
        "0,0"
    );
    // §7.1.22 `ToIndex` — a fraction is truncated and only the *range* is refused, so `1.9` is one
    // byte and `-1` is a RangeError rather than being clamped to zero.
    assert_eq!(run("new ArrayBuffer(1.9).byteLength"), "1");
    assert_eq!(
        run("try { new ArrayBuffer(-1); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { new ArrayBuffer(); ArrayBuffer(8); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new ArrayBuffer(1))"),
        "[object ArrayBuffer]"
    );
    // §25.1.5.1 — an accessor, so it cannot be assigned and always reads the buffer.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
             (typeof d.get) + ',' + (d.value === undefined)"
        ),
        "function,true"
    );
    // §25.1.4.1 — `isView` is about *views*, so a buffer is not one and neither is anything else.
    // It answers rather than throwing, because it is the question "may I pass this where a view is
    // wanted" and a wrong shape is an answer.
    assert_eq!(
        run(
            "ArrayBuffer.isView(new DataView(new ArrayBuffer(1))) + ',' \
             + ArrayBuffer.isView(new ArrayBuffer(1)) + ',' + ArrayBuffer.isView(1) + ',' \
             + ArrayBuffer.isView(undefined)"
        ),
        "true,false,false,false"
    );
}

#[test]
fn a_view_is_a_window_and_two_of_them_see_each_others_writes() {
    // The whole design in one row: a view holds no bytes, so writing through one is visible
    // through the other. An implementation that copied on construction would pass every test about
    // a single view and fail this.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var one = new DataView(b); var two = new DataView(b); \
             one.setUint8(3, 42); two.getUint8(3)"
        ),
        "42"
    );
    // …and an offset window sees the same bytes at a different index.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var whole = new DataView(b); var part = new DataView(b, 4); \
             whole.setUint8(4, 7); part.getUint8(0) + ',' + part.byteOffset + ',' + part.byteLength"
        ),
        "7,4,4"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(8); new DataView(b).buffer === b"),
        "true"
    );
    // §25.3.2.1 step 8 — an absent length means "to the end", which is not the same as 0.
    assert_eq!(
        run("new DataView(new ArrayBuffer(8), 2).byteLength + ',' \
             + new DataView(new ArrayBuffer(8), 2, 3).byteLength"),
        "6,3"
    );
    // Steps 6 and 10 — a window that would not fit is a RangeError rather than a shorter one.
    assert_eq!(
        run("try { new DataView(new ArrayBuffer(4), 8); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { new DataView(new ArrayBuffer(4), 2, 4); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // Step 2 — the first argument must be an actual buffer.
    assert_eq!(
        run("try { new DataView({}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new DataView(new ArrayBuffer(1)))"),
        "[object DataView]"
    );
}

#[test]
fn the_endianness_is_the_formats_and_not_the_machines() {
    // §25.3.4's default is **big**-endian, which is the opposite of every machine praxis runs on.
    // Deliberately: a `DataView` exists for data that came from a file or a socket, and the byte
    // order such data has is the one the *format* chose. A default that matched the machine would
    // make the same program correct on one and wrong on another.
    assert_eq!(
        run(
            "var v = new DataView(new ArrayBuffer(4)); v.setUint16(0, 0x1234); \
             v.getUint8(0) + ',' + v.getUint8(1)"
        ),
        "18,52"
    );
    // …and reading the same bytes the other way round gives the other number, which is the whole
    // point of asking each time.
    assert_eq!(
        run(
            "var v = new DataView(new ArrayBuffer(4)); v.setUint16(0, 0x1234); \
             v.getUint16(0) + ',' + v.getUint16(0, true)"
        ),
        "4660,13330"
    );
    assert_eq!(
        run(
            "var v = new DataView(new ArrayBuffer(8)); v.setFloat64(0, 1.5, true); \
             v.getFloat64(0, true) + ',' + (v.getFloat64(0) === 1.5)"
        ),
        "1.5,false"
    );
    // A one-byte type has no order to get wrong, so the argument changes nothing.
    assert_eq!(
        run(
            "var v = new DataView(new ArrayBuffer(2)); v.setInt8(0, -1); \
             v.getInt8(0) + ',' + v.getInt8(0, true)"
        ),
        "-1,-1"
    );
}

#[test]
fn writing_an_integer_wraps_where_writing_a_float_rounds() {
    // §7.1.7 `ToIntN` is modular arithmetic, not saturation and not an error: a byte holds 256
    // values and a number outside them comes back as its residue. That is what makes a buffer
    // behave like memory rather than like a checked container.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.setUint8(0, 256); v.getUint8(0)"),
        "0"
    );
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.setUint8(0, -1); v.getUint8(0)"),
        "255"
    );
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.setInt8(0, 200); v.getInt8(0)"),
        "-56"
    );
    assert_eq!(
        run(
            "var v = new DataView(new ArrayBuffer(4)); v.setInt32(0, 4294967296 + 5); v.getInt32(0)"
        ),
        "5"
    );
    // §7.1.5 first — `NaN` and the infinities become 0, and a fraction is truncated toward zero,
    // so writing to a buffer is total. Nothing a program can pass makes `set` refuse.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); \
             v.setUint8(0, NaN); v.setUint8(1, Infinity); v.setUint8(2, 1.9); v.setUint8(3, -1.9); \
             v.getUint8(0) + ',' + v.getUint8(1) + ',' + v.getUint8(2) + ',' + v.getUint8(3)"),
        "0,0,1,255"
    );
    // A float is *rounded* to its width instead, which is why a float32 does not read back what
    // was written and a float64 does. Nothing is lost turning a float32 into a Number — every one
    // is exactly representable — but the value stored was already the rounded one.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(8)); v.setFloat64(0, 0.1); v.getFloat64(0)"),
        "0.1"
    );
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(8)); v.setFloat32(0, 0.1); v.getFloat32(0)"),
        "0.10000000149011612"
    );
    // …and `set` answers `undefined` rather than what it wrote.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.setUint8(0, 1) === undefined"),
        "true"
    );
}

#[test]
fn a_read_or_write_past_the_window_is_refused_by_its_own_length() {
    // §25.3.1.1 step 8 — the *view's* length, not the buffer's, which is what makes a window a
    // promise about a region rather than a pointer into one.
    assert_eq!(
        run(
            "try { new DataView(new ArrayBuffer(4)).getInt32(1); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(8); \
             try { new DataView(b, 0, 4).getUint8(4); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(8); \
             try { new DataView(b, 0, 4).setUint8(4, 1); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // The last byte a window has is readable, which is the other side of the same bound.
    assert_eq!(
        run("var v = new DataView(new ArrayBuffer(4)); v.setInt32(0, 7); v.getInt32(0)"),
        "7"
    );
    // §25.3.4 borrows badly: these methods are about the internal slots, so a plain object cannot
    // pretend to be a view.
    assert_eq!(
        run("try { DataView.prototype.getUint8.call({}, 0); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { ArrayBuffer.prototype.slice.call({}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn slice_copies_the_bytes_into_a_buffer_of_its_own() {
    // §25.1.5.3 — a *copy*, so writing through the original afterwards does not change it. A slice
    // that shared bytes would look identical until something wrote.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var v = new DataView(b); v.setUint8(2, 9); \
             var c = b.slice(1, 4); v.setUint8(2, 0); \
             c.byteLength + ',' + new DataView(c).getUint8(1)"
        ),
        "3,9"
    );
    // Both ends are relative and both clamp, exactly as `Array.prototype.slice`'s do.
    assert_eq!(
        run("var b = new ArrayBuffer(8); \
             b.slice(-2).byteLength + ',' + b.slice(0, -6).byteLength + ',' \
             + b.slice(99).byteLength + ',' + b.slice(0, 99).byteLength + ',' + b.slice(4, 2).byteLength"),
        "2,2,0,8,0"
    );
    assert_eq!(run("var b = new ArrayBuffer(8); b.slice(0) === b"), "false");
    // §25.1.4.3 — the species accessor answers the receiver, so a subclass gets its own kind back
    // from `slice` and the constructor really is called.
    assert_eq!(run("ArrayBuffer[Symbol.species] === ArrayBuffer"), "true");
    assert_eq!(
        run("class B extends ArrayBuffer {} new B(8).slice(0, 4) instanceof B"),
        "true"
    );
    // …and a species that hands back the *same* buffer is refused, because `slice` would then be
    // copying a buffer onto itself.
    assert_eq!(
        run("var b = new ArrayBuffer(8); \
             b.constructor = { [Symbol.species]: function () { return b; } }; \
             try { b.slice(0, 4); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn transfer_moves_the_bytes_and_leaves_the_old_buffer_detached() {
    // §25.1.5.5 — the only operation in the language that detaches a buffer, and therefore the only
    // way a program can reach the state every read checks for. The bytes are *moved*: the new
    // buffer has them and the old one has nothing.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); v.setUint8(0, 7);              var c = b.transfer(); new DataView(c).getUint8(0) + ',' + c.byteLength"
        ),
        "7,4"
    );
    // §25.1.5.3 — `detached` is the only way to ask, because `byteLength` cannot: a detached buffer
    // and an empty one both answer 0, deliberately.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var was = b.detached; b.transfer();              was + ',' + b.detached + ',' + b.byteLength"
        ),
        "false,true,0"
    );
    assert_eq!(run("new ArrayBuffer(0).detached"), "false");
    // Every view onto it starts throwing from here — which is the whole reason a read asks again
    // rather than trusting that its buffer was there when the view was made.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); b.transfer();              try { v.getUint8(0); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); b.transfer();              try { v.setUint8(0, 1); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …including the three accessors, which **throw** where `ArrayBuffer.prototype.byteLength`
    // answers 0. A view onto nothing is not a view of length nothing; it is an error.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); b.transfer();              try { v.byteLength; } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); b.transfer();              try { v.byteOffset; } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // `buffer` still answers, because it is about which buffer and not about its bytes.
    assert_eq!(
        run("var b = new ArrayBuffer(4); var v = new DataView(b); b.transfer(); v.buffer === b"),
        "true"
    );
    // A buffer that is already detached has nothing to transfer, to slice, or to make a view over.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); b.transfer();              try { b.transfer(); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); b.transfer();              try { b.slice(0, 1); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); b.transfer();              try { new DataView(b); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // Step 5 — an explicit length truncates or zero-extends, so `transfer` is also how a buffer is
    // resized. The bytes that survive keep their places and the new ones are zero.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4); var v = new DataView(b); v.setUint8(3, 9);              var c = b.transfer(8); c.byteLength + ',' + new DataView(c).getUint8(3) + ','                + new DataView(c).getUint8(7)"
        ),
        "8,9,0"
    );
    assert_eq!(run("new ArrayBuffer(4).transfer(2).byteLength"), "2");
}

#[test]
fn a_window_may_sit_exactly_at_the_end_of_its_buffer() {
    // §25.3.2.1 steps 6 and 10 compare with `>`, not `>=`: an offset *equal* to the length is a
    // legal empty window, and one past it is a RangeError. The difference is one byte and it is
    // the difference between a program that works and one that does not, because a loop that
    // slices to the end naturally lands on it.
    assert_eq!(run("new DataView(new ArrayBuffer(4), 4).byteLength"), "0");
    assert_eq!(
        run("new DataView(new ArrayBuffer(4), 2, 2).byteLength"),
        "2"
    );
    assert_eq!(run("new DataView(new ArrayBuffer(0), 0).byteLength"), "0");
    assert_eq!(
        run("try { new DataView(new ArrayBuffer(4), 5); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(
        run("try { new DataView(new ArrayBuffer(4), 2, 3); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // §25.3.2.1 step 1 — a plain call has no `new.target` to take a prototype from.
    assert_eq!(
        run("try { DataView(new ArrayBuffer(4)); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn the_accessors_have_the_attributes_their_clauses_give_them() {
    // §17's convention for an accessor: not enumerable, configurable. Each is a thing a program can
    // detect, and `configurable` is what makes each replaceable — the only reason to say so.
    for (object, name) in [
        ("ArrayBuffer.prototype", "byteLength"),
        ("ArrayBuffer.prototype", "detached"),
        ("DataView.prototype", "buffer"),
        ("DataView.prototype", "byteLength"),
        ("DataView.prototype", "byteOffset"),
    ] {
        assert_eq!(
            run(&format!(
                "var d = Object.getOwnPropertyDescriptor({object}, '{name}');                  (typeof d.get) + ',' + d.enumerable + ',' + d.configurable"
            )),
            "function,false,true",
            "{object}.{name}"
        );
    }
    // §25.1.4.3 — species is an accessor too, and answers the receiver.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(ArrayBuffer, Symbol.species);              d.enumerable + ',' + d.configurable + ',' + (ArrayBuffer[Symbol.species] === ArrayBuffer)"
        ),
        "false,true,true"
    );
}

#[test]
fn a_buffer_counts_against_the_heaps_budget_before_it_is_allocated() {
    // DR-0013 — a buffer is the easiest thing in the language to ask too much of, and it is the one
    // allocation whose size a *program* chooses. Refused before the memory is taken rather than
    // reported once it has been, which is the difference between a RangeError and a dead process.
    assert_eq!(
        run("try { new ArrayBuffer(2 ** 40); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // …and the bytes count towards the footprint from then on, so a second buffer is measured
    // against a heap that knows about the first. Without that every buffer is measured against an
    // allowance that never moved, and a loop allocating them runs until the process dies.
    assert_eq!(
        run(
            "var kept = []; var caught = 'none';              try { for (var i = 0; i < 64; i++) { kept.push(new ArrayBuffer(4 * 1024 * 1024)); } }              catch (e) { caught = e.constructor.name; } caught"
        ),
        "RangeError"
    );
}

#[test]
fn a_conversion_may_detach_the_buffer_underneath_the_thing_that_is_using_it() {
    // Why §25.3 checks for a detached buffer *twice* around one conversion, and why every read
    // checks again rather than trusting the view. Converting an argument runs `valueOf`, and
    // `valueOf` is a program: it can transfer the buffer out from under the operation that is
    // half-way through reading its own arguments.
    //
    // Each of these throws only because of the check *after* the conversion. Without it a view
    // would be built over bytes that are gone, or a write would go into them.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8);              var len = { valueOf: function () { b.transfer(); return 4; } };              try { new DataView(b, 0, len); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8);              var at = { valueOf: function () { b.transfer(); return 0; } };              try { new DataView(b, at); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and the same for a *write*, whose value is converted before the bounds are looked at.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var v = new DataView(b);              var n = { valueOf: function () { b.transfer(); return 1; } };              try { v.setUint8(0, n); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and for a read, whose index is converted first.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var v = new DataView(b);              var at = { valueOf: function () { b.transfer(); return 0; } };              try { v.getUint8(at); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // §25.3.1.1 checks for a detached buffer at step 5 and the bounds at step 8, in that order, and
    // the order is observable: an index that is *both* past the end and arrived by detaching the
    // buffer earns a TypeError and not a RangeError. Without the check the read would fail anyway,
    // one step later and with the wrong complaint — which is the difference between "your data is
    // gone" and "you asked for the wrong byte".
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var v = new DataView(b);              var at = { valueOf: function () { b.transfer(); return 99; } };              try { v.getUint8(at); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var v = new DataView(b);              var at = { valueOf: function () { b.transfer(); return 99; } };              try { v.setUint8(at, 1); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // A buffer that is *already* detached is refused before any conversion happens, which is the
    // first of the two checks and answers a different question: `TypeError` rather than the
    // `RangeError` an offset past a zero-length buffer would earn.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); b.transfer();              try { new DataView(b, 5); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}
