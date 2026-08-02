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

#[test]
fn a_buffer_given_a_maximum_may_be_resized_up_to_it_and_no_further() {
    // §25.1.3.1's `maxByteLength` option is the whole of what makes a buffer resizable — §25.1.6.4
    // step 2 asks for the slot, not for a flag — so a buffer made without one has no `resize` to
    // offer and says so with a TypeError rather than a RangeError about the length.
    assert_eq!(run("new ArrayBuffer(8).resizable"), "false");
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: 8 }).resizable"),
        "true"
    );
    // §25.1.6.2 step 5 — a fixed buffer answers its *current* length rather than `undefined`,
    // because a buffer that cannot be resized is already as long as it will ever be.
    assert_eq!(run("new ArrayBuffer(8).maxByteLength"), "8");
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: 8 }).maxByteLength"),
        "8"
    );
    // `undefined` under the key is "no opinion" and not a maximum of zero, so it makes the same
    // fixed buffer as no options bag at all — which is what lets an option be passed through.
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: undefined }).resizable"),
        "false"
    );
    assert_eq!(run("new ArrayBuffer(4, null).resizable"), "false");
    // Both directions, and the two refusals either side of them.
    assert_eq!(
        run("var b = new ArrayBuffer(4, { maxByteLength: 8 }); b.resize(6); b.byteLength"),
        "6"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(4, { maxByteLength: 8 }); b.resize(1); b.byteLength"),
        "1"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); try { b.resize(9); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run("try { new ArrayBuffer(4).resize(2); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §25.1.3.1 step 4 — and a buffer cannot start out longer than it may ever be.
    assert_eq!(
        run("try { new ArrayBuffer(9, { maxByteLength: 8 }); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
}

#[test]
fn bytes_a_resize_uncovers_are_zero_and_never_what_used_to_be_there() {
    // §25.1.3.1's rule that a program may read every byte of a buffer and find 0 does not stop
    // applying because the byte arrived by growing. Shrinking and re-growing is the case that would
    // give it away: the old bytes are still in the allocation right up until this says otherwise.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(1, { maxByteLength: 4 }); var v = new Uint8Array(b); v[0] = 9; b.resize(3); new Uint8Array(b)[2]"
        ),
        "0"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 4 }); new Uint8Array(b)[3] = 7; b.resize(1); b.resize(4); new Uint8Array(b)[3]"
        ),
        "0"
    );
}

#[test]
fn a_view_made_without_a_length_over_a_resizable_buffer_follows_it() {
    // §10.4.5's `auto`. The two halves that decide it are both necessary and each is a row here: an
    // explicit length pins the window however the buffer moves, and a fixed buffer has nothing to
    // follow.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(2, { maxByteLength: 8 }); var v = new Uint8Array(b); b.resize(6); v.length"
        ),
        "6"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(6, { maxByteLength: 8 }); var v = new Uint8Array(b); b.resize(2); v.length"
        ),
        "2"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(6, { maxByteLength: 8 }); var v = new Uint8Array(b, 0, 3); b.resize(8); v.length"
        ),
        "3"
    );
    assert_eq!(
        run("var b = new ArrayBuffer(4); var v = new Uint8Array(b); v.length"),
        "4"
    );
    // The offset is kept, so a tracking view starting part way along follows the *remainder*.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8, { maxByteLength: 16 }); var v = new Uint8Array(b, 4); b.resize(12); v.length"
        ),
        "8"
    );
    // …rounded down to a whole element, because a partial one at the end is not an element.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8, { maxByteLength: 16 }); var v = new Int32Array(b); b.resize(11); v.length"
        ),
        "2"
    );
    // An element that the buffer no longer covers is absent rather than stale, and one it has just
    // come to cover is writable.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 4 }); var v = new Uint8Array(b); v[3] = 9; b.resize(1); typeof v[3]"
        ),
        "undefined"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(1, { maxByteLength: 4 }); var v = new Uint8Array(b); b.resize(3); v[2] = 5; v[2]"
        ),
        "5"
    );
    // A `DataView` tracks on the same terms, and keeps every byte rather than whole elements.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(2, { maxByteLength: 8 }); var d = new DataView(b); b.resize(5); d.byteLength"
        ),
        "5"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(6, { maxByteLength: 8 }); var d = new DataView(b, 0, 2); b.resize(8); d.byteLength"
        ),
        "2"
    );
}

#[test]
fn a_fixed_view_whose_buffer_shrank_under_it_is_refused_like_a_detached_one() {
    // §10.4.5.2 `IsTypedArrayOutOfBounds`. `new Uint8Array(rab, 0, 4)` still names four elements
    // after `rab.resize(2)` and only two of them exist, so every method that begins with
    // `ValidateTypedArray` throws — where an array that merely *has* no elements walks nothing and
    // answers. Only a fixed-length view can be in this state: a tracking one follows the buffer.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var v = new Uint8Array(b, 0, 4); b.resize(2); try { v.fill(1); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var v = new Uint8Array(b, 0, 4); b.resize(2); v.length"
        ),
        "0"
    );
    // …and it comes back when the buffer does, because nothing about the view was changed.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var v = new Uint8Array(b, 0, 4); b.resize(2); b.resize(4); v.length"
        ),
        "4"
    );
    // A tracking view over the same buffer is never out of bounds, however far it shrinks.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var v = new Uint8Array(b); b.resize(0); v.fill(1); v.length"
        ),
        "0"
    );
}

#[test]
fn a_shared_buffer_grows_and_will_not_shrink() {
    // §25.2.5.4 is §25.1.6.4 with one rule added and one removed. The addition is the interesting
    // one: a shrink would pull memory out from under a view another agent is reading through, and
    // §25.2 exists so that memory can be shared without that being possible. It is a RangeError and
    // not a silent no-op, so a program that believed it shrank one finds out.
    assert_eq!(run("new SharedArrayBuffer(4).growable"), "false");
    assert_eq!(
        run("new SharedArrayBuffer(4, { maxByteLength: 8 }).growable"),
        "true"
    );
    assert_eq!(
        run("var b = new SharedArrayBuffer(4, { maxByteLength: 8 }); b.grow(7); b.byteLength"),
        "7"
    );
    assert_eq!(
        run(
            "var b = new SharedArrayBuffer(4, { maxByteLength: 8 }); try { b.grow(2); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    assert_eq!(
        run(
            "var b = new SharedArrayBuffer(4, { maxByteLength: 8 }); try { b.grow(9); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // Neither name works on the other's kind of buffer, which is what keeps the two brands apart.
    assert_eq!(
        run(
            "try { ArrayBuffer.prototype.resize.call(new SharedArrayBuffer(4, { maxByteLength: 8 }), 5); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run("typeof new SharedArrayBuffer(4, { maxByteLength: 8 }).resize"),
        "undefined"
    );
}

#[test]
fn transferring_keeps_the_ceiling_and_transferring_to_fixed_length_drops_it() {
    // §25.1.5.5 passes `preserve-resizability` and §25.1.5.6 passes `fixed-length`, which is the
    // only difference between the two methods — and the only way in the language to turn a
    // resizable buffer into a fixed one.
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: 8 }).transfer().maxByteLength"),
        "8"
    );
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: 8 }).transfer().resizable"),
        "true"
    );
    assert_eq!(
        run("new ArrayBuffer(4, { maxByteLength: 8 }).transferToFixedLength().resizable"),
        "false"
    );
    // A fixed buffer transfers into a fixed one, so `transfer` does not *make* anything resizable.
    assert_eq!(run("new ArrayBuffer(4).transfer().resizable"), "false");
}

#[test]
fn an_array_like_longer_than_the_heap_is_refused_before_it_is_walked() {
    // A loop counted by a number the program chose. `new Int8Array({ length: 2 ** 53 })` read
    // absent properties into a Rust list that DR-0013's budget does not measure, so the check
    // inside the loop never fired and nothing could stop it. The bound has to be on what is about
    // to be produced, which is the same lesson `String.prototype.repeat` taught.
    assert_eq!(
        run("try { new Int8Array({ length: Math.pow(2, 53) }); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // …while an array-like that could plausibly exist is still built.
    assert_eq!(run("new Int8Array({ length: 3, 0: 7 })[0]"), "7");
}

#[test]
fn resizing_a_buffer_whose_bytes_have_gone_is_refused_rather_than_reviving_it() {
    // §25.1.6.4 step 4. A resizable buffer can still be detached — `transfer` does it — and the
    // resize that follows must not put a `Vec` back where §25.1.3.3 took one away. It is the one
    // ordering in this method that cannot be seen from the answer, only from what the buffer is
    // afterwards, so both are asserted.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); b.transfer(); try { b.resize(6); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); b.transfer(); try { b.resize(6); } catch (e) {} b.byteLength + ',' + b.detached"
        ),
        "0,true"
    );
}

#[test]
fn growing_a_shared_buffer_to_the_length_it_already_has_is_allowed() {
    // §25.2.5.4's refusal is on *shrinking*, so the equal case is the boundary and it goes the
    // permissive way: `grow(byteLength)` is a no-op and not a RangeError. Written the other way
    // round it would refuse a program that grows to a length it computed and happened to already
    // have, which is what a loop stepping towards a maximum does on its last turn.
    assert_eq!(
        run("var b = new SharedArrayBuffer(4, { maxByteLength: 8 }); b.grow(4); b.byteLength"),
        "4"
    );
    // …and the same length exactly at the maximum, which is the other boundary and also allowed.
    assert_eq!(
        run("var b = new SharedArrayBuffer(4, { maxByteLength: 8 }); b.grow(8); b.byteLength"),
        "8"
    );
    // A buffer with no maximum has no `grow` to offer, and says so with a TypeError about the
    // buffer rather than a RangeError about the length.
    assert_eq!(
        run("try { new SharedArrayBuffer(4).grow(8); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_shared_buffer_may_start_exactly_at_its_maximum_and_no_longer() {
    // §25.2.2.1's step 2 check, and its boundary. Equal is allowed — a buffer that starts full is
    // a perfectly ordinary thing to ask for — and one byte more is refused.
    assert_eq!(
        run("new SharedArrayBuffer(8, { maxByteLength: 8 }).byteLength"),
        "8"
    );
    assert_eq!(
        run(
            "try { new SharedArrayBuffer(9, { maxByteLength: 8 }); } catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // The same boundary for §25.1.3.1, so the two constructors cannot drift apart.
    assert_eq!(
        run("new ArrayBuffer(8, { maxByteLength: 8 }).byteLength"),
        "8"
    );
}

#[test]
fn a_subarray_is_told_how_long_it_is_unless_the_array_it_came_from_tracks() {
    // §23.2.3.30 step 16 — the *number of arguments* the species is constructed with is the whole
    // of how "keep tracking" is said, and a program can count them. Three cases, and each is a
    // different reason for the third argument to be there or not: a tracking array asked for
    // everything to the end passes two, the same array given an explicit end passes three, and a
    // fixed-length array always passes three however it is asked.
    let counter = "var seen; \
         function Spy(...args) { seen = args.length; return new Uint8Array(args[0], args[1], args[2]); } \
         Spy[Symbol.species] = Spy; ";
    assert_eq!(
        run(&format!(
            "{counter} var b = new ArrayBuffer(4, {{ maxByteLength: 8 }}); \
             var v = new Uint8Array(b); v.constructor = Spy; v.subarray(1); seen"
        )),
        "2"
    );
    assert_eq!(
        run(&format!(
            "{counter} var b = new ArrayBuffer(4, {{ maxByteLength: 8 }}); \
             var v = new Uint8Array(b); v.constructor = Spy; v.subarray(1, 3); seen"
        )),
        "3"
    );
    assert_eq!(
        run(&format!(
            "{counter} var v = new Uint8Array(4); v.constructor = Spy; v.subarray(1); seen"
        )),
        "3"
    );
    // A view over a resizable buffer that was given an explicit length does not track either, so
    // it is the *view* being asked and not the buffer it sits on.
    assert_eq!(
        run(&format!(
            "{counter} var b = new ArrayBuffer(4, {{ maxByteLength: 8 }}); \
             var v = new Uint8Array(b, 0, 4); v.constructor = Spy; v.subarray(1); seen"
        )),
        "3"
    );
}

#[test]
fn a_data_view_that_no_longer_fits_its_buffer_says_so_before_it_says_anything_else() {
    // §25.3.1.2, the `DataView` half of §10.4.5.2, and the two errors it has to keep apart. An
    // out-of-bounds view is a TypeError about the *bounds*; something that was never a `DataView`
    // is a TypeError about the receiver. Both are TypeErrors, so the message is the only thing that
    // distinguishes them and it is what is asserted.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var d = new DataView(b, 0, 4); b.resize(2); try { d.getInt8(0); } catch (e) { e.message }"
        ),
        "this DataView is outside the bounds of its buffer"
    );
    assert_eq!(
        run("try { DataView.prototype.getInt8.call({}, 0); } catch (e) { e.message }"),
        "this is not a DataView"
    );
    // A tracking `DataView` over the same shrinking buffer is never out of bounds, so it reads.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(4, { maxByteLength: 8 }); var d = new DataView(b); b.resize(2); d.byteLength"
        ),
        "2"
    );
}

#[test]
fn an_array_like_is_refused_against_the_room_that_is_left_and_not_a_fixed_number() {
    // The bound is DR-0013's *remaining* allowance, so what counts as too long depends on what the
    // heap already holds. Asserted by taking most of the budget first and then asking for a length
    // that would have been perfectly ordinary on an empty heap: 100,000 elements is nothing, and
    // there is no longer room to read them into.
    //
    // This is also the row that pins the guard. `{ length: 2 ** 53 }` above proves it refuses, but
    // removing the guard makes *that* case run for ever rather than answer wrongly — and a test
    // that hangs is not a test that fails. Here the unguarded path finishes in a moment and
    // finishes with an array, so the two behaviours are told apart by an answer instead of by a
    // clock.
    assert_eq!(
        run("var hog = new ArrayBuffer(63 * 1024 * 1024); \
             try { new Int8Array({ length: 100000 }); } catch (e) { e.message }"),
        "this array-like is longer than this engine will allocate"
    );
    // …and on a heap that has not been filled, the very same length is built without complaint.
    // Two `run`s rather than one script, because a detached buffer's bytes are *not* given back to
    // the budget — `transfer` moves them and charges the new buffer, so a script cannot undo the
    // first line and the contrast has to be drawn between two heaps.
    assert_eq!(run("new Int8Array({ length: 100000 }).length"), "100000");
}
