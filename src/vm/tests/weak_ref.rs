//! §26.1's `WeakRef` and §26.2's `FinalizationRegistry` as a script sees them.
//!
//! Which, again, is not weakly: praxis collects when its embedder says to, so `deref` answers the
//! same thing throughout any one script. What is testable here is the surface — which values may
//! be held, which arguments are refused, and the brands. The rows about a target actually going
//! away are in `heap::collect`.

use super::*;

#[test]
fn a_weak_ref_answers_its_target_for_as_long_as_it_is_there() {
    // §26.1.3.2 — `deref` is the whole of the interface, and within one script nothing has been
    // collected, so it answers the target every time. That "every time" is itself the rule:
    // §9.10.4 does not let an engine answer the target once and `undefined` the next line.
    assert_eq!(
        run("var o = {}; var r = new WeakRef(o); (r.deref() === o) + ',' + (r.deref() === o)"),
        "true,true"
    );
    // §7.2.10 again — an unregistered Symbol may be held, and it comes back as itself.
    assert_eq!(
        run("var s = Symbol('s'); var r = new WeakRef(s); r.deref() === s"),
        "true"
    );
    // …including one with no description at all, which is alive and has none. An engine reading
    // "no description" as "collected" answers `undefined` here.
    assert_eq!(
        run("var s = Symbol(); new WeakRef(s).deref() === s"),
        "true"
    );
    // §26.1.1.1 step 2 — everything that could never become stale is refused at construction.
    for bad in ["1", "'s'", "true", "null", "undefined", "Symbol.for('r')"] {
        assert_eq!(
            run(&format!(
                "try {{ new WeakRef({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{bad} cannot be held weakly"
        );
    }
    assert_eq!(
        run("try { WeakRef({}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Two references to the same target are two objects holding one thing.
    assert_eq!(
        run(
            "var o = {}; var a = new WeakRef(o); var b = new WeakRef(o); \
             (a === b) + ',' + (a.deref() === b.deref())"
        ),
        "false,true"
    );
}

#[test]
fn a_registry_refuses_the_registrations_that_could_never_be_cleaned_up() {
    // §26.2.3.1 step 5 — the held value may not be the target. It is held *strongly*, so such a
    // registration would keep the target alive through its own cell and the callback could never
    // be reached. The specification refusing it is the specification noticing that.
    assert_eq!(
        run(
            "var o = {}; var f = new FinalizationRegistry(function () {}); \
             try { f.register(o, o); } catch (e) { e.constructor.name + ':' + e.message }"
        ),
        "TypeError:a FinalizationRegistry cannot hold its own target"
    );
    // …but the held value may be anything else at all, including a primitive, and registering
    // answers `undefined`.
    assert_eq!(
        run("var f = new FinalizationRegistry(function () {}); \
             typeof f.register({}, 1) + ',' + typeof f.register({}, undefined)"),
        "undefined,undefined"
    );
    // §26.2.1.1 step 2 — a registry with nothing to call is refused at construction, which is the
    // only moment at which refusing it is any use.
    for bad in ["", "1", "'f'", "undefined", "null", "{}"] {
        assert_eq!(
            run(&format!(
                "try {{ new FinalizationRegistry({bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "cleanup callback {bad}"
        );
    }
    assert_eq!(
        run("try { FinalizationRegistry(function () {}); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 3 — the target must be holdable, on the same terms as everything else in §26.
    assert_eq!(
        run("var f = new FinalizationRegistry(function () {}); \
             try { f.register(1, 'h'); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn an_unregister_token_is_absent_or_holdable_and_nothing_between() {
    // §26.2.3.1 step 6 — `undefined` means "no token", and every other unholdable value is a
    // TypeError. `null` is the interesting one: it is the usual stand-in for absence and it is not
    // one here, because the check is about what can be held rather than about emptiness.
    assert_eq!(
        run("var f = new FinalizationRegistry(function () {}); \
             typeof f.register({}, 'h', undefined)"),
        "undefined"
    );
    for bad in ["null", "1", "'t'", "Symbol.for('r')"] {
        assert_eq!(
            run(&format!(
                "var f = new FinalizationRegistry(function () {{}}); \
                 try {{ f.register({{}}, 'h', {bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "token {bad}"
        );
    }
    // §26.2.3.4 — `unregister` answers whether anything went, so a second call is false. Note it
    // *throws* for a token that could never have been stored, where §24.3.3.3's `get` answers a
    // miss: this method is being told something rather than asked.
    assert_eq!(
        run(
            "var f = new FinalizationRegistry(function () {}); var t = {}; \
             f.register({}, 'h', t); f.unregister(t) + ',' + f.unregister(t) + ',' + f.unregister({})"
        ),
        "true,false,false"
    );
    assert_eq!(
        run("var f = new FinalizationRegistry(function () {}); \
             try { f.unregister(1); } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // One token may be given to any number of registrations, and step 5 removes **all** of them —
    // so the second call finds nothing left rather than finding the others.
    assert_eq!(
        run(
            "var f = new FinalizationRegistry(function () {}); var t = {}; \
             f.register({}, 1, t); f.register({}, 2, t); f.register({}, 3, t); \
             f.unregister(t) + ',' + f.unregister(t)"
        ),
        "true,false"
    );
    // A registration made without a token cannot be removed by any token, which is what makes the
    // third argument worth passing.
    assert_eq!(
        run(
            "var f = new FinalizationRegistry(function () {}); var t = {}; \
             f.register({}, 'h'); f.unregister(t)"
        ),
        "false"
    );
    // A token of one kind never matches one of the other, however the two are stored — the
    // comparison is `SameValue` and an Object is not a Symbol.
    assert_eq!(
        run(
            "var f = new FinalizationRegistry(function () {}); var o = {}; var s = Symbol('t'); \
             f.register({}, 'h', o); f.unregister(s) + ',' + f.unregister(o)"
        ),
        "false,true"
    );
    // A Symbol works as a token, on the same terms as an object.
    assert_eq!(
        run(
            "var f = new FinalizationRegistry(function () {}); var t = Symbol('t'); \
             f.register({}, 'h', t); f.unregister(t) + ',' + f.unregister(Symbol('t'))"
        ),
        "true,false"
    );
}

#[test]
fn each_of_the_two_is_its_own_brand() {
    // §26.1.3.2 requires a `[[WeakRefTarget]]` and §26.2.3 a `[[Cells]]`, and neither answers for
    // the other however alike they are underneath — both are the same slot on an object here,
    // which is exactly why the check has to look at *which* it is.
    for borrowed in [
        "WeakRef.prototype.deref.call(new FinalizationRegistry(function () {}))",
        "FinalizationRegistry.prototype.register.call(new WeakRef({}), {}, 1)",
        "FinalizationRegistry.prototype.unregister.call(new WeakRef({}), {})",
        "WeakRef.prototype.deref.call(new WeakMap())",
        "WeakRef.prototype.deref.call({})",
        "WeakRef.prototype.deref.call(1)",
        "FinalizationRegistry.prototype.register.call({}, {}, 1)",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {borrowed}; }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{borrowed}"
        );
    }
    // §17's `[@@toStringTag]` and §10.3.3's `name` and `length`.
    assert_eq!(
        run("Object.prototype.toString.call(new WeakRef({})) + ' ' \
             + Object.prototype.toString.call(new FinalizationRegistry(function () {}))"),
        "[object WeakRef] [object FinalizationRegistry]"
    );
    assert_eq!(
        run("WeakRef.length + ',' + FinalizationRegistry.length + ',' \
             + WeakRef.prototype.deref.length + ',' + FinalizationRegistry.prototype.register.length \
             + ',' + FinalizationRegistry.prototype.unregister.length"),
        "1,1,0,2,1"
    );
    // Both take their prototype from `new.target`, so a subclass keeps working.
    assert_eq!(
        run("class R extends WeakRef {} var o = {}; var r = new R(o); \
             (r instanceof R) + ',' + (r.deref() === o)"),
        "true,true"
    );
    assert_eq!(
        run(
            "class F extends FinalizationRegistry {} var f = new F(function () {}); var t = {}; \
             f.register({}, 'h', t); (f instanceof F) + ',' + f.unregister(t)"
        ),
        "true,true"
    );
}
