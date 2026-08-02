//! §25.2's `SharedArrayBuffer` and §25.4's `Atomics`.

use super::*;

/// The ten `Atomics` operations, for the rows that ask the same thing of all of them.
const ATOMICS: [&str; 6] = ["add", "and", "or", "sub", "xor", "exchange"];

#[test]
fn a_shared_buffer_is_a_different_brand_from_an_ordinary_one() {
    assert_eq!(
        run("var s = new SharedArrayBuffer(8); \
             s.byteLength + ',' + Object.prototype.toString.call(s) \
             + ',' + (s instanceof SharedArrayBuffer)"),
        "8,[object SharedArrayBuffer],true"
    );
    // §25.1.5's methods want an unshared buffer and §25.2.4's a shared one, so neither answers
    // about the other. Without that check `ArrayBuffer.prototype.slice` would copy a shared buffer
    // and hand back something of the wrong kind — which reads correctly and is not.
    assert_eq!(
        run(
            "try { ArrayBuffer.prototype.slice.call(new SharedArrayBuffer(8)); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { SharedArrayBuffer.prototype.slice.call(new ArrayBuffer(8)); } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var get = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get; \
             try { get.call(new SharedArrayBuffer(8)); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // §25.2 gives a shared buffer no `[[ArrayBufferDetachKey]]` and no `transfer`, so its bytes
    // cannot be taken away — which is the whole of what "shared" means to one agent.
    assert_eq!(
        run("try { new SharedArrayBuffer(8).transfer(); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("typeof SharedArrayBuffer.prototype.transfer + ',' \
             + typeof SharedArrayBuffer.prototype.detached"),
        "undefined,undefined"
    );
    // Each `slice` answers its own kind, and the arithmetic is the same for both.
    assert_eq!(
        run(
            "new ArrayBuffer(8).slice(2).byteLength + ',' + new SharedArrayBuffer(8).slice(2).byteLength \
             + ',' + Object.prototype.toString.call(new SharedArrayBuffer(8).slice(2, 6)) \
             + ',' + new SharedArrayBuffer(8).slice(6, 2).byteLength"
        ),
        // A backwards range is **empty** rather than reversed, the same as every other relative
        // range in the library.
        "6,6,[object SharedArrayBuffer],0"
    );
    assert_eq!(
        run("try { SharedArrayBuffer(8); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // DR-0013 — refused before the bytes are taken, the same as §25.1.3.1's buffer.
    assert_eq!(
        run("try { new SharedArrayBuffer(2 ** 40); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // §25.2.4.1 is an accessor with §17's attributes, so a program may replace it and may not
    // simply assign over it.
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'byteLength');              (typeof d.get) + ',' + (d.set === undefined) + ',' + d.enumerable + ',' + d.configurable"
        ),
        "function,true,false,true"
    );
    // A TypedArray over a shared buffer is an ordinary TypedArray — the sharing is the buffer's.
    assert_eq!(
        run(
            "var a = new Int32Array(new SharedArrayBuffer(16)); a[2] = 7; \
             a.length + ',' + a[2] + ',' + (a.buffer instanceof SharedArrayBuffer)"
        ),
        "4,7,true"
    );
}

#[test]
fn every_atomic_answers_what_was_there_and_leaves_what_it_computed() {
    // §25.4.3's read-modify-writes each answer the **old** value, which is what makes them
    // read-modify-writes rather than writes. An implementation answering the new one agrees about
    // the array and disagrees about every one of these.
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 12; \
             Atomics.add(a, 0, 3) + ':' + a[0] + '|' \
             + Atomics.sub(a, 0, 5) + ':' + a[0]"),
        "12:15|15:10"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 12; \
             Atomics.and(a, 0, 10) + ':' + a[0]"),
        "12:8"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 9; \
             Atomics.or(a, 0, 5) + ':' + a[0] + '|' + Atomics.xor(a, 0, 3) + ':' + a[0]"),
        "9:13|13:14"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 1; Atomics.exchange(a, 0, 9) + ':' + a[0]"),
        "1:9"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 7; Atomics.load(a, 0) + ',' + a[0]"),
        "7,7"
    );
    // §25.4.3.13 — `store` is the odd one: it answers what it was **given**, not what it wrote and
    // not what was there. Storing 300 into a `Uint8Array` writes 44 and answers 300.
    assert_eq!(
        run("var a = new Uint8Array(4); Atomics.store(a, 0, 300) + ',' + a[0]"),
        "300,44"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 1; Atomics.store(a, 0, 2.7) + ',' + a[0]"),
        "2,2"
    );
    // §25.4.3.3 — the comparison is against the value **as the element kind stores it**. Expecting
    // 300 of a `Uint8Array` holding 44 matches, because 300 stored there *is* 44; comparing the
    // raw arguments would never match and the write would never happen.
    assert_eq!(
        run("var a = new Uint8Array(4); a[0] = 44; \
             Atomics.compareExchange(a, 0, 300, 9) + ':' + a[0]"),
        "44:9"
    );
    assert_eq!(
        run("var a = new Int32Array(4); a[0] = 5; \
             Atomics.compareExchange(a, 0, 5, 8) + ':' + a[0] + '|' \
             + Atomics.compareExchange(a, 0, 99, 1) + ':' + a[0]"),
        "5:8|8:8"
    );
    // …and they all work over a `SharedArrayBuffer` too, which is what they are named for.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             Atomics.add(a, 1, 4) + ',' + Atomics.load(a, 1)"),
        "0,4"
    );
}

#[test]
fn an_atomic_refuses_the_kinds_and_indices_an_ordinary_element_read_would_allow() {
    // §25.4.2.1 `ValidateIntegerTypedArray` — `IsUnclampedIntegerElementType` or
    // `IsBigIntElementType`, and the three kinds that are neither are refused outright. The floats
    // because atomics are about bit patterns a CPU can exchange and a double is not one however
    // well it holds an integer; `Uint8ClampedArray` because §7.1.11's saturation is not one either.
    // test262 spells the same list out as `nonAtomicsFriendlyTypedArrayConstructors` in
    // `harness/testTypedArray.js`, which is the float kinds *concatenated with* `Uint8ClampedArray`.
    for kind in ["Float32Array", "Float64Array", "Uint8ClampedArray"] {
        assert_eq!(
            run(&format!(
                "try {{ Atomics.load(new {kind}(4), 0); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "Atomics.load on a {kind}"
        );
        // Refused by `store` too, whose validation is the same operation and could have drifted:
        // a clamped array reaching the write is where the saturation would have been silent.
        assert_eq!(
            run(&format!(
                "try {{ Atomics.store(new {kind}(4), 0, 3); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "Atomics.store on a {kind}"
        );
    }
    // …and every *unclamped* integer kind is accepted.
    for kind in [
        "Int8Array",
        "Uint8Array",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
    ] {
        assert_eq!(
            run(&format!("Atomics.store(new {kind}(4), 0, 3)")),
            "3",
            "Atomics.store on a {kind}"
        );
    }
    // The two BigInt kinds are accepted as well, and take a BigInt where the six above take a
    // Number — §25.4.3.13 step 3 chooses the conversion by `[[ContentType]]`.
    for kind in ["BigInt64Array", "BigUint64Array"] {
        assert_eq!(
            run(&format!("String(Atomics.store(new {kind}(4), 0, 3n))")),
            "3",
            "Atomics.store on a {kind}"
        );
        assert_eq!(
            run(&format!(
                "try {{ Atomics.store(new {kind}(4), 0, 3); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "Atomics.store of a Number into a {kind}"
        );
    }
    // Anything that is not a TypedArray at all is refused before the index is looked at.
    for bad in ["[1, 2]", "{}", "1", "null", "new ArrayBuffer(8)"] {
        assert_eq!(
            run(&format!(
                "try {{ Atomics.load({bad}, 0); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "Atomics.load on {bad}"
        );
    }
    // §25.4.3.3 step 3 — an index outside the array is a **RangeError**, where an ordinary `a[9]`
    // is silently `undefined`. An atomic write that went nowhere is worse than an error.
    for method in ATOMICS {
        assert_eq!(
            run(&format!(
                "try {{ Atomics.{method}(new Int32Array(4), 9, 1); }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "Atomics.{method} past the end"
        );
    }
    assert_eq!(
        run("try { Atomics.load(new Int32Array(4), -1); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // Exactly *at* the count is outside, and one below it is inside — the boundary itself, which
    // an index of nine cannot tell from an index of five.
    assert_eq!(
        run("try { Atomics.load(new Int32Array(4), 4); } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    assert_eq!(run("Atomics.load(new Int32Array(4), 3)"), "0");
    // A detached buffer is refused too, which an ordinary read would answer `undefined` for.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var a = new Int32Array(b); b.transfer(); \
             try { Atomics.load(a, 0); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // §25.4.3.8 — `isLockFree` answers about a *width*, and gives the same answer every time it is
    // asked about the same one.
    assert_eq!(
        run(
            "[Atomics.isLockFree(1), Atomics.isLockFree(2), Atomics.isLockFree(4), \
             Atomics.isLockFree(8), Atomics.isLockFree(3), Atomics.isLockFree(0)].join(',')"
        ),
        "true,true,true,true,false,false"
    );
    assert_eq!(
        run("Atomics.isLockFree(4) === Atomics.isLockFree(4)"),
        "true"
    );
    // §25.4 is an ordinary object, like `Math` and `JSON` — not a constructor.
    assert_eq!(
        run(
            "Object.prototype.toString.call(Atomics) + ',' + (typeof Atomics) \
             + ',' + (typeof Atomics.wait)"
        ),
        "[object Atomics],object,undefined"
    );
}

#[test]
fn the_bitwise_atomics_on_a_bigint_array_are_the_operations_they_are_named_for() {
    // §25.4.3.2, §25.4.3.11 and §25.4.3.15 on a sixty-four bit slot. Each pair below is chosen so
    // that the *other* two operations would give a different answer — `0b1100` against `0b1010` is
    // 8, 14 and 6 for `and`, `or` and `xor` — because three operations over the same bits are the
    // easiest three in the engine to write in place of one another and have every round number
    // still agree.
    for (name, answer) in [("and", "8"), ("or", "14"), ("xor", "6")] {
        assert_eq!(
            run(&format!(
                "var a = new BigUint64Array(1); a[0] = 12n; \
                 String(Atomics.{name}(a, 0, 10n)) + ',' + String(a[0])"
            )),
            format!("12,{answer}"),
            "Atomics.{name} on a BigUint64Array"
        );
    }
    // The arithmetic two, and both **wrap** at sixty-four bits rather than growing: an element is
    // a fixed-width slot however unbounded §6.1.6.2's type is. `0n - 1n` in one is the largest
    // unsigned value, which is also what says the subtraction happened on the bits.
    assert_eq!(
        run("var a = new BigUint64Array(1); Atomics.sub(a, 0, 1n); String(a[0])"),
        "18446744073709551615"
    );
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = 2n ** 63n - 1n; \
             Atomics.add(a, 0, 1n); String(a[0])"),
        "-9223372036854775808"
    );
    // `exchange` keeps the new value and answers the old, which is what makes it the one operation
    // here that ignores what was there.
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = 5n; \
             String(Atomics.exchange(a, 0, -2n)) + ',' + String(a[0])"),
        "5,-2"
    );
    // A signed and an unsigned array over one buffer see the same bits, which is the whole of the
    // difference between the two kinds and is decided by the *read* rather than by the write.
    assert_eq!(
        run(
            "var b = new ArrayBuffer(8); var s = new BigInt64Array(b); var u = new BigUint64Array(b); \
             Atomics.sub(s, 0, 1n); String(s[0]) + ',' + String(u[0])"
        ),
        "-1,18446744073709551615"
    );
}

#[test]
fn compare_exchange_compares_what_the_slot_would_hold_and_never_a_clamped_form_of_it() {
    // §25.4.3.3 step 9 compares **bytes**: the expected value is put through
    // `NumericToRawBytes` for the array's own kind and matched against the bytes that are there.
    // So expecting 300 of a `Uint8Array` holding 44 is a match, because 300 stored there *is* 44 —
    // and a comparison against the raw argument would never match and the write would never
    // happen at all.
    assert_eq!(
        run("var a = new Uint8Array(1); a[0] = 300; \
             String(Atomics.compareExchange(a, 0, 300, 7)) + ',' + a[0]"),
        "44,7"
    );
    // …and §7.1.11's *clamping* is not what forms those bytes, which is a distinction only a
    // `Uint8Array` can show: 300 wraps to 44 and clamps to 255, and the cell holds 44. Clamping
    // here would make the expectation 255, find no match, and leave the cell alone — a
    // `compareExchange` that silently did nothing.
    assert_eq!(
        run(
            "var a = new Uint8Array(1); a[0] = 300; Atomics.compareExchange(a, 0, 255, 7) + ',' + a[0]"
        ),
        "44,44"
    );
    // A mismatch answers what was there and writes nothing, which is the other half of the pair.
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = 5n; \
             String(Atomics.compareExchange(a, 0, 4n, 9n)) + ',' + String(a[0])"),
        "5,5"
    );
    assert_eq!(
        run("var a = new BigInt64Array(1); a[0] = 5n; \
             String(Atomics.compareExchange(a, 0, 5n, 9n)) + ',' + String(a[0])"),
        "5,9"
    );
    // The same wrapping the Number kinds get: an expectation past the width matches the cell that
    // holds its low bits, because that is what storing it there would have produced.
    assert_eq!(
        run("var a = new BigUint64Array(1); a[0] = 7n; \
             String(Atomics.compareExchange(a, 0, 2n ** 64n + 7n, 1n)) + ',' + String(a[0])"),
        "7,1"
    );
}
