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
        "[object Atomics],object,function"
    );
    // Three of the four are here; `pause` is a separate proposal and is not.
    assert_eq!(
        run("typeof Atomics.waitAsync + ',' + typeof Atomics.pause"),
        "function,undefined"
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

#[test]
fn notify_wakes_this_agents_own_waiters_and_answers_how_many() {
    // Nothing parked, so nothing to wake — but this is `+0` by counting an empty list, not by the
    // operation being decorative. The rows below park first and get a different number.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); Atomics.notify(a, 0, 1)"),
        "0"
    );
    // Step 7 — an ordinary ArrayBuffer is a `0` and *not* an error, which is where this parts
    // company with `wait`. An engine that refused here would look stricter and be wrong.
    assert_eq!(run("Atomics.notify(new Int32Array(4), 0, 1)"), "0");
    // §25.4.3.4's waitable check: `Int32Array` and `BigInt64Array` alone. A `Uint8Array` is a
    // perfectly good target for `Atomics.add` and cannot key a waiter list, so the two checks are
    // different questions and this one has to be asked separately.
    assert_eq!(
        run(
            "try { Atomics.notify(new Uint8Array(new SharedArrayBuffer(8)), 0, 1) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run("var a = new BigInt64Array(new SharedArrayBuffer(16)); Atomics.notify(a, 0, 1)"),
        "0"
    );
    // The index is still validated, and out of range is a RangeError rather than a silent zero.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             try { Atomics.notify(a, 99, 1) } catch (e) { e.constructor.name }"),
        "RangeError"
    );
    // Step 3 — the count's conversion runs the program's own code, and its throw is what the
    // program sees. This is the row that fails if the count is discarded without being computed:
    // the answer is `0` either way, and only the `valueOf` running tells the two apart.
    assert_eq!(
        run(
            "var a = new Int32Array(new SharedArrayBuffer(16)); var ran = 0; \
             Atomics.notify(a, 0, { valueOf: function () { ran++; return 1; } }); ran"
        ),
        "1"
    );
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             try { Atomics.notify(a, 0, { valueOf: function () { throw new RangeError('mine') } }) } \
             catch (e) { e.message }"),
        "mine"
    );
    // …and a missing count is accepted rather than being a TypeError for a missing argument. The
    // clause makes it +∞ where an explicit number is itself; both are discarded here, so what this
    // row pins is only that neither spelling refuses. It is deliberately *not* a claim that the
    // two take different paths — they do not, because no program could tell.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             Atomics.notify(a, 0) + ',' + Atomics.notify(a, 0, undefined)"),
        "0,0"
    );
}

#[test]
fn wait_refuses_because_this_agent_cannot_suspend_and_converts_first() {
    // DoWait step 12 — `AgentCanSuspend()` is false here, so the TypeError *is* the conformant
    // answer. A browser's main thread says the same; test262 spells the condition `CanBlockIsFalse`.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             try { Atomics.wait(a, 0, 0, 0) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 1 wants a **shared** buffer, where §25.4.3.7's `notify` takes either. Both are a
    // TypeError, so the messages are what tell them apart — and the buffer is refused before the
    // suspend check, which is what the next row proves.
    assert_eq!(
        run("try { Atomics.wait(new Int32Array(4), 0, 0, 0) } catch (e) { e.message }"),
        "this is not a SharedArrayBuffer"
    );
    // …and that refusal is inside step 1, so it lands **before** the index is converted. This is
    // the row that distinguishes the two orders: both answer TypeError, and only a poisoned index
    // says which one ran first. test262 spells the same check `non-shared-bufferdata-throws.js`,
    // where the getter throws a Test262Error precisely so that running it is not a TypeError.
    assert_eq!(
        run(
            "var poisoned = { valueOf: function () { throw new RangeError('ran') } }; \
             try { Atomics.wait(new Int32Array(4), poisoned, poisoned, poisoned) } \
             catch (e) { e.constructor.name + ':' + e.message }"
        ),
        "TypeError:this is not a SharedArrayBuffer"
    );
    // The same ordering the other way round: `notify` accepts an unshared buffer, so for it the
    // index *is* reached and the poisoned getter does run.
    assert_eq!(
        run(
            "var poisoned = { valueOf: function () { throw new RangeError('ran') } }; \
             try { Atomics.notify(new Int32Array(4), poisoned, 1) } catch (e) { e.message }"
        ),
        "ran"
    );
    // Every conversion runs before the refusal, in the clause's order, and the program's own error
    // wins. Written with a throw rather than a counter because a counter would also pass if the
    // conversion ran *after* the TypeError — which it cannot, since nothing runs after it.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             try { Atomics.wait(a, 0, 0, { valueOf: function () { throw new RangeError('t') } }) } \
             catch (e) { e.constructor.name + ':' + e.message }"),
        "RangeError:t"
    );
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(16)); \
             try { Atomics.wait(a, 0, { valueOf: function () { throw new RangeError('v') } }, 0) } \
             catch (e) { e.message }"),
        "v"
    );
    // The kind is checked before the index, so a Float64Array never reaches an index that would
    // itself have thrown — the error names the array and not the index.
    assert_eq!(
        run(
            "try { Atomics.wait(new Float64Array(new SharedArrayBuffer(32)), 99, 0, 0) } \
             catch (e) { e.message }"
        ),
        "this is not an integer TypedArray"
    );
    // A BigInt64Array is waitable and takes a BigInt value; a Number there is §7.1.13's TypeError
    // and not a silent conversion.
    assert_eq!(
        run("var a = new BigInt64Array(new SharedArrayBuffer(16)); \
             try { Atomics.wait(a, 0, 0n, 0) } catch (e) { e.message }"),
        "this agent cannot be suspended"
    );
    assert_eq!(
        run("var a = new BigInt64Array(new SharedArrayBuffer(16)); \
             try { Atomics.wait(a, 0, 0, 0) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Both carry §10.3's own metadata, which `descriptor.js` and `length.js` check.
    assert_eq!(
        run("Atomics.wait.length + ',' + Atomics.wait.name + ',' \
             + Atomics.notify.length + ',' + Atomics.notify.name"),
        "4,wait,3,notify"
    );
}

#[test]
fn wait_async_parks_a_promise_that_this_agents_own_notify_wakes() {
    // The two answers that settle before returning carry `async: false` and a **String**, because
    // there is nothing left to wait for: the value has already changed, or the timeout is zero.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             var r = Atomics.waitAsync(a, 0, 0, 0); r.async + ',' + r.value"),
        "false,timed-out"
    );
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             var r = Atomics.waitAsync(a, 0, 42); r.async + ',' + r.value"),
        "false,not-equal"
    );
    // A negative timeout is `max(q, 0)` and not an error — it is a duration, where an index would
    // have been a RangeError. So -1 answers at once rather than waiting.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             Atomics.waitAsync(a, 0, 0, -1).value"),
        "timed-out"
    );
    // …and the third answer parks, with `%Promise%` itself rather than whatever `Promise` names.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             var r = Atomics.waitAsync(a, 0, 0, 1000); \
             r.async + ',' + (r.value instanceof Promise) \
             + ',' + (Object.getPrototypeOf(r.value) === Promise.prototype)"),
        "true,true,true"
    );
    // The whole point of the list with one agent: the agent that parked carries on and wakes its
    // own waiter. A blocking `Atomics.wait` could never reach the next statement.
    assert_eq!(
        run(
            "var a = new Int32Array(new SharedArrayBuffer(32)); var said = 'nothing'; \
             Atomics.waitAsync(a, 0, 0).value.then(function (o) { said = o; }); \
             var woke = Atomics.notify(a, 0); \
             woke + ',' + said"
        ),
        // `said` is still untouched here: settling queues a job and §9.5 runs it after this script.
        "1,nothing"
    );
    // The promise really does settle with "ok", which the row above deliberately cannot show —
    // `run_settled` drains §9.5's queue first and only then reads the expression.
    assert_eq!(
        run_settled(
            "var a = new Int32Array(new SharedArrayBuffer(32)); var said = 'nothing'; \
             Atomics.waitAsync(a, 0, 0).value.then(function (o) { said = o; }); \
             Atomics.notify(a, 0);",
            "said"
        ),
        "ok"
    );
    // …and a waiter nothing notifies stays parked rather than settling, because there is no timer
    // to time it out. This is the recorded divergence, pinned here so that building a timer has to
    // change this row deliberately rather than quietly.
    assert_eq!(
        run_settled(
            "var a = new Int32Array(new SharedArrayBuffer(32)); var said = 'nothing'; \
             Atomics.waitAsync(a, 0, 0, 1).value.then(function (o) { said = o; });",
            "said"
        ),
        "nothing"
    );
    // A count smaller than the list wakes that many and leaves the rest parked — and §25.4.1.5
    // appends, so it is the *earliest* waiters that go.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             Atomics.waitAsync(a, 0, 0); Atomics.waitAsync(a, 0, 0); Atomics.waitAsync(a, 0, 0); \
             Atomics.notify(a, 0, 2) + ',' + Atomics.notify(a, 0)"),
        "2,1"
    );
    // A missing count is +∞ and not zero, which is the row that fails if `undefined` is read as
    // `ToIntegerOrInfinity(undefined)`: two waiters, one notify with no count, both woken.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             Atomics.waitAsync(a, 0, 0); Atomics.waitAsync(a, 0, 0); Atomics.notify(a, 0)"),
        "2"
    );
    // The list is keyed on a **byte** position, so a waiter at index 1 is not woken by a notify at
    // index 0 — and a notify at the wrong index answers 0 rather than waking the wrong promise.
    assert_eq!(
        run("var a = new Int32Array(new SharedArrayBuffer(32)); \
             Atomics.waitAsync(a, 1, 0); Atomics.notify(a, 0) + ',' + Atomics.notify(a, 1)"),
        "0,1"
    );
    // Two views of different widths over one buffer agree about the position, because the key is a
    // byte offset: a `BigInt64Array`'s slot 0 and an `Int32Array`'s slot 0 are the same eight-byte
    // start. An element-index key would make these two different lists and the notify would miss.
    assert_eq!(
        run("var b = new SharedArrayBuffer(32); \
             var w = new BigInt64Array(b); var n = new Int32Array(b); \
             Atomics.waitAsync(w, 0, 0n); Atomics.notify(n, 0)"),
        "1"
    );
    // An unshared buffer is refused by `waitAsync` exactly as by `wait` — nothing else can reach
    // it, so a parked promise there could only ever be woken by the parking agent itself, and the
    // clause does not allow it.
    assert_eq!(
        run("try { Atomics.waitAsync(new Int32Array(4), 0, 0) } catch (e) { e.message }"),
        "this is not a SharedArrayBuffer"
    );
    // Step 17 compares against the value **as the element kind stores it**, and storing wraps
    // rather than clamps: 2**31 lands in an `Int32Array` as -2147483648, so a cell holding that is
    // a match and the wait parks. Clamping would make it 2147483647, find no match, and answer
    // "not-equal" — the same shape `compareExchange` has, and the only row that tells the two
    // apart, since `Uint8ClampedArray` is not a waitable kind and can never reach this.
    assert_eq!(
        run(
            "var a = new Int32Array(new SharedArrayBuffer(32)); a[0] = -2147483648; \
             var r = Atomics.waitAsync(a, 0, 2147483648); r.async + ',' + r.value"
        ),
        "true,[object Promise]"
    );
    assert_eq!(
        run("Atomics.waitAsync.length + ',' + Atomics.waitAsync.name"),
        "4,waitAsync"
    );
}
