//! §10.5 and §28.2 — `Proxy`, the one object whose behaviour is written in JavaScript.
//!
//! Two kinds of test here, and the second is the one that matters. The first says a trap is
//! reached and its answer used, which is the easy half. The second says a trap that *lies* is
//! caught: §10.5's invariants are what separate a proxy from an object with callbacks on it, and
//! they are precisely the part an implementation gets wrong quietly, because a program only finds
//! out when a promise it relied on turns out not to hold.

use super::*;

#[test]
fn a_proxy_with_an_empty_handler_is_its_target_in_every_way() {
    // §10.5 — every internal method falls through to the target when the handler has no trap, so
    // a proxy with `{}` is indistinguishable from what it stands in front of. This is the base
    // case each trap below departs from.
    assert_eq!(run("new Proxy({a: 1}, {}).a"), "1");
    assert_eq!(run("'a' in new Proxy({a: 1}, {})"), "true");
    assert_eq!(run("delete new Proxy({a: 1}, {}).a"), "true");
    assert_eq!(
        run("Object.keys(new Proxy({a: 1, b: 2}, {})).join()"),
        "a,b"
    );
    assert_eq!(run("new Proxy({}, {}) instanceof Object"), "true");
    assert_eq!(run("JSON.stringify(new Proxy({a: 1}, {}))"), r#"{"a":1}"#);
}

#[test]
fn a_target_that_is_itself_a_proxy_costs_no_rust_stack() {
    // §10.5's fall-through is "perform the operation on the target", and the target may be another
    // proxy. Written recursively that is one Rust frame per link and a program picks how many,
    // which DR-0002 forbids — so a chain this long is the test that it is not.
    assert_eq!(
        run(
            "var p = {a: 1}; for (var i = 0; i < 20000; i++) { p = new Proxy(p, {}); } \
             p.a + ',' + ('a' in p)"
        ),
        "1,true"
    );
    // …and a trap found part-way down the chain is still the one that answers.
    assert_eq!(
        run(
            "var p = new Proxy({}, {get: function () { return 'deep'; }}); \
             for (var i = 0; i < 100; i++) { p = new Proxy(p, {}); } p.anything"
        ),
        "deep"
    );
}

#[test]
fn the_four_property_traps_are_handed_the_target_and_the_key() {
    assert_eq!(
        run("new Proxy({}, {get: function (t, k) { return 'got ' + k; }}).x"),
        "got x"
    );
    // A Symbol key arrives as a Symbol rather than as its description: §10.5.8 hands the key
    // itself, and `String(k)` is the only way a trap can spell one.
    assert_eq!(
        run(
            "var p = new Proxy({}, {get: function (t, k) { return typeof k; }}); \
             p[Symbol.iterator]"
        ),
        "symbol"
    );
    assert_eq!(
        run(
            "var log = []; var p = new Proxy({}, {set: function (t, k, v) { \
             log.push(k + '=' + v); return true; }}); p.a = 1; log.join()"
        ),
        "a=1"
    );
    assert_eq!(
        run("'anything' in new Proxy({}, {has: function () { return true; }})"),
        "true"
    );
    assert_eq!(
        run(
            "var log = []; var p = new Proxy({}, {deleteProperty: function (t, k) { \
             log.push(k); return true; }}); delete p.gone; log.join()"
        ),
        "gone"
    );
}

#[test]
fn the_receiver_a_get_trap_is_handed_is_the_object_the_lookup_started_from() {
    // §10.5.8 step 8 passes the *receiver*, which is what makes a proxy usable as a prototype: an
    // inherited access reports the object it was written on, not the proxy.
    assert_eq!(
        run(
            "var p = new Proxy({}, {get: function (t, k, r) { return r === child; }}); \
             var child = Object.create(p); child.probe"
        ),
        "true"
    );
}

#[test]
fn a_trap_that_is_not_a_function_is_refused_and_null_means_there_is_none() {
    // §10.5's step 5 — `undefined` **and** null both mean "no trap". Anything else that is not
    // callable is a TypeError rather than a quiet fall through, which is what stops a misspelled
    // handler property from silently doing nothing.
    assert_eq!(run("new Proxy({a: 1}, {get: null}).a"), "1");
    assert_eq!(run("new Proxy({a: 1}, {get: undefined}).a"), "1");
    assert_eq!(
        run("try { new Proxy({}, {get: 1}).a } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_get_trap_may_not_contradict_a_property_the_target_has_fixed() {
    // §10.5.8 step 10 — the invariant that makes a descriptor worth reading. A program that has
    // checked `writable: false, configurable: false` is entitled to believe the value it saw.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { new Proxy(t, {get: function () { return 2; }}).x } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Answering the *same* value is fine — the rule is about contradiction, not about trapping.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             new Proxy(t, {get: function () { return 1; }}).x"),
        "1"
    );
    // A writable fixed property may be reported as anything: it could genuinely change.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1, writable: true}); \
             new Proxy(t, {get: function () { return 2; }}).x"
        ),
        "2"
    );
    // An accessor with no getter reads as `undefined` however the trap answers.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {set: function () {}}); \
             try { new Proxy(t, {get: function () { return 1; }}).x } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_set_or_has_or_delete_trap_may_not_contradict_one_either() {
    // §10.5.9 step 9, §10.5.7 step 9, §10.5.10 step 11 — the same rule from three directions.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { new Proxy(t, {set: function () { return true; }}).x = 2 } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { 'x' in new Proxy(t, {has: function () { return false; }}) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { delete new Proxy(t, {deleteProperty: function () { return true; }}).x } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A trap that reports *failure* is always allowed: refusing to do something breaks no promise.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             delete new Proxy(t, {deleteProperty: function () { return false; }}).x"),
        "false"
    );
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             'x' in new Proxy(t, {has: function () { return true; }})"),
        "true"
    );
}

#[test]
fn revoking_turns_every_internal_method_into_a_type_error() {
    // §28.2.2.1.1 — the target and handler go together, so there is no half-revoked state in which
    // some operations still work.
    assert_eq!(
        run(
            "var r = Proxy.revocable({a: 1}, {}); var before = r.proxy.a; r.revoke(); \
             before + ',' + (function () { try { r.proxy.a } catch (e) { return e.constructor.name } })()"
        ),
        "1,TypeError"
    );
    for source in [
        "'a' in r.proxy",
        "r.proxy.a = 1",
        "delete r.proxy.a",
        "Object.keys(r.proxy)",
        "Object.getPrototypeOf(r.proxy)",
        "Object.isExtensible(r.proxy)",
        "Array.isArray(r.proxy)",
    ] {
        assert_eq!(
            run(&format!(
                "var r = Proxy.revocable([], {{}}); r.revoke(); \
                 try {{ {source}; 'no throw' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source} should refuse a revoked proxy"
        );
    }
    // Step 2 — revoking twice is not an error, because the second call finds the slot already
    // empty and has nothing to do.
    assert_eq!(
        run("var r = Proxy.revocable({}, {}); r.revoke(); r.revoke(); typeof r.revoke()"),
        "undefined"
    );
}

#[test]
fn the_constructor_needs_new_and_two_objects_and_has_no_prototype_property() {
    assert_eq!(
        run("try { Proxy({}, {}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    for source in ["new Proxy(1, {})", "new Proxy({}, 1)", "new Proxy({})"] {
        assert_eq!(
            run(&format!(
                "try {{ {source} }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source} should be refused"
        );
    }
    // §28.2.2 — the only constructor in the language with no `prototype` property at all, because
    // a proxy's prototype is its target's and there is nothing for `new.target` to read.
    assert_eq!(run("typeof Proxy.prototype"), "undefined");
    assert_eq!(run("Proxy.name + ',' + Proxy.length"), "Proxy,2");
    assert_eq!(
        run("var r = Proxy.revocable({}, {}); typeof r.proxy + ',' + typeof r.revoke"),
        "object,function"
    );
    // §28.2.2.1.1 — the revocation function is anonymous and takes nothing.
    assert_eq!(
        run("var r = Proxy.revocable({}, {}); r.revoke.name + '|' + r.revoke.length"),
        "|0"
    );
}

#[test]
fn a_prototype_trap_answers_instanceof_and_the_walk_that_reads_it() {
    // §7.3.22 step 4 is `[[GetPrototypeOf]]`, not a field read — which is why a proxy can stand in
    // for a whole prototype chain.
    assert_eq!(
        run(
            "var p = new Proxy({}, {getPrototypeOf: function () { return Array.prototype; }}); \
             (p instanceof Array) + ',' + (Object.getPrototypeOf(p) === Array.prototype)"
        ),
        "true,true"
    );
    assert_eq!(
        run(
            "var p = new Proxy({}, {getPrototypeOf: function () { return Array.prototype; }}); \
             Array.prototype.isPrototypeOf(p)"
        ),
        "true"
    );
    // Step 7 — a prototype is an object or null. `undefined`, the answer a trap that forgot to
    // return gives, is refused rather than read as null.
    assert_eq!(
        run(
            "try { Object.getPrototypeOf(new Proxy({}, {getPrototypeOf: function () {}})) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // Step 8 — a non-extensible target's prototype cannot move, so the trap may not say it has.
    assert_eq!(
        run("var t = Object.preventExtensions({}); \
             try { Object.getPrototypeOf(new Proxy(t, \
             {getPrototypeOf: function () { return Array.prototype; }})) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var t = Object.preventExtensions({}); \
             Object.getPrototypeOf(new Proxy(t, \
             {getPrototypeOf: function () { return Object.prototype; }})) === Object.prototype"),
        "true"
    );
}

#[test]
fn a_set_prototype_trap_reports_whether_it_was_allowed() {
    assert_eq!(
        run("Reflect.setPrototypeOf(new Proxy({}, \
             {setPrototypeOf: function () { return false; }}), null)"),
        "false"
    );
    assert_eq!(
        run(
            "var log = []; var p = new Proxy({}, {setPrototypeOf: function (t, v) { \
             log.push(v === null ? 'null' : 'object'); return true; }}); \
             Object.setPrototypeOf(p, null); log.join()"
        ),
        "null"
    );
    // §10.5.2 step 8 — the same fixed-prototype rule as the getter's, from the other side.
    assert_eq!(
        run("var t = Object.preventExtensions({}); \
             try { Object.setPrototypeOf(new Proxy(t, \
             {setPrototypeOf: function () { return true; }}), Array.prototype) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn an_extensibility_trap_has_no_freedom_at_all() {
    // §10.5.3 step 9 — this trap must agree with the target. It exists so a program can *observe*
    // the question, not so it can answer it differently, and that is the strictest invariant in
    // §10.5.
    assert_eq!(
        run("Object.isExtensible(new Proxy({}, {isExtensible: function () { return true; }}))"),
        "true"
    );
    assert_eq!(
        run("try { Object.isExtensible(new Proxy({}, \
             {isExtensible: function () { return false; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §10.5.4 step 8 — claiming to have prevented extensions on a target that is still extensible
    // would make `isExtensible` and `preventExtensions` disagree about the same object.
    assert_eq!(
        run("try { Object.preventExtensions(new Proxy({}, \
             {preventExtensions: function () { return true; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Reflect.preventExtensions(new Proxy({}, \
             {preventExtensions: function () { return false; }}))"),
        "false"
    );
    // …and `Object.preventExtensions` throws where `Reflect`'s reports, which is the whole
    // difference between the two clauses.
    assert_eq!(
        run("try { Object.preventExtensions(new Proxy({}, \
             {preventExtensions: function () { return false; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // With no trap, preventing extensions on a proxy reaches the *target*.
    assert_eq!(
        run("var t = {}; Object.preventExtensions(new Proxy(t, {})); Object.isExtensible(t)"),
        "false"
    );
}

#[test]
fn an_own_keys_trap_supplies_every_listing_there_is() {
    let listings = [
        ("Object.keys(p).join()", "a"),
        ("Object.getOwnPropertyNames(p).join()", "a"),
        ("Reflect.ownKeys(p).join()", "a"),
        ("Object.values(p).join()", "1"),
        ("Object.entries(p).join()", "a,1"),
        ("JSON.stringify(p)", r#"{"a":1}"#),
        ("JSON.stringify(Object.assign({}, p))", r#"{"a":1}"#),
        ("JSON.stringify({...p})", r#"{"a":1}"#),
    ];
    for (source, expected) in listings {
        assert_eq!(
            run(&format!(
                "var p = new Proxy({{a: 1, b: 2}}, {{ownKeys: function () {{ return ['a']; }}}}); \
                 {source}"
            )),
            expected,
            "{source} should see only the key the trap listed"
        );
    }
    // §7.3.24 asks `[[GetOwnProperty]]` for each listed key as well, so a key the `ownKeys` trap
    // names and the descriptor trap then hides does not appear.
    assert_eq!(
        run(
            "Object.keys(new Proxy({}, {ownKeys: function () { return ['a']; }, \
             getOwnPropertyDescriptor: function () {}})).length"
        ),
        "0"
    );
}

#[test]
fn a_for_in_loop_walks_a_proxy_with_its_traps() {
    // §14.7.5.10 — the names come from `[[OwnPropertyKeys]]` and `[[GetOwnProperty]]`, and each is
    // re-checked with `[[HasProperty]]` before the body sees it. All three may be traps.
    assert_eq!(
        run("var out = []; for (var k in new Proxy({a: 1, b: 2}, {})) out.push(k); out.join()"),
        "a,b"
    );
    assert_eq!(
        run("var out = []; for (var k in new Proxy({}, {\
             ownKeys: function () { return ['x', 'y']; }, \
             getOwnPropertyDescriptor: function () { \
             return {value: 1, enumerable: true, configurable: true}; }, \
             has: function () { return true; }})) out.push(k); out.join()"),
        "x,y"
    );
    // A non-enumerable descriptor keeps a listed key out of the loop.
    assert_eq!(
        run("var out = []; for (var k in new Proxy({}, {\
             ownKeys: function () { return ['x']; }, \
             getOwnPropertyDescriptor: function () { \
             return {value: 1, enumerable: false, configurable: true}; }})) out.push(k); \
             out.length"),
        "0"
    );
    // And a proxy's *prototype* is walked too, because the chain is read with the same trap.
    assert_eq!(
        run("var out = []; var base = {inherited: 1}; \
             for (var k in new Proxy(Object.create(base), {})) out.push(k); out.join()"),
        "inherited"
    );
}

#[test]
fn an_own_keys_trap_may_not_lie_about_a_shape_that_is_fixed() {
    // §10.5.11 step 7 — duplicates are refused outright, because a caller iterating the list would
    // otherwise see the same property twice.
    assert_eq!(
        run("try { Reflect.ownKeys(new Proxy({}, \
             {ownKeys: function () { return ['x', 'x']; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 6 — the list holds Strings and Symbols. A number in it is a mistake, not a key to be
    // coerced.
    assert_eq!(
        run("try { Reflect.ownKeys(new Proxy({}, \
             {ownKeys: function () { return [0]; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 17 — a key the target cannot lose must be listed.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'fixed', {value: 1}); \
             try { Reflect.ownKeys(new Proxy(t, {ownKeys: function () { return []; }})) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // Steps 19 and 20 — a non-extensible target's keys are exactly its own: none may be left out…
    assert_eq!(
        run("var t = Object.preventExtensions({a: 1}); \
             try { Reflect.ownKeys(new Proxy(t, {ownKeys: function () { return []; }})) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and none invented.
    assert_eq!(
        run("var t = Object.preventExtensions({a: 1}); \
             try { Reflect.ownKeys(new Proxy(t, \
             {ownKeys: function () { return ['a', 'b']; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("var t = Object.preventExtensions({a: 1}); \
             Reflect.ownKeys(new Proxy(t, {ownKeys: function () { return ['a']; }})).join()"),
        "a"
    );
    // An extensible target with nothing permanent constrains nothing at all — this is the ordinary
    // case, and it is why the arithmetic above almost never runs.
    assert_eq!(
        run("Reflect.ownKeys(new Proxy({a: 1}, \
             {ownKeys: function () { return ['q']; }})).join()"),
        "q"
    );
}

#[test]
fn a_descriptor_trap_answers_get_own_property_descriptor_and_is_completed_first() {
    // §6.2.6.6 `CompletePropertyDescriptor` — a partial answer is filled in with what a *fresh*
    // property would have, not with the target's attributes.
    assert_eq!(
        run(
            "JSON.stringify(Object.getOwnPropertyDescriptor(new Proxy({}, \
             {getOwnPropertyDescriptor: function () { return {value: 5, configurable: true}; }}), 'q'))"
        ),
        r#"{"value":5,"writable":false,"enumerable":false,"configurable":true}"#
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(new Proxy({a: 1}, \
             {getOwnPropertyDescriptor: function () {}}), 'a')"),
        "undefined"
    );
    // §10.5.5 step 9 — a property the target cannot delete may not be reported absent.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { Object.getOwnPropertyDescriptor(new Proxy(t, \
             {getOwnPropertyDescriptor: function () {}}), 'x') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 17 — nor may a trap invent a permanent property the target does not have.
    assert_eq!(
        run("try { Object.getOwnPropertyDescriptor(new Proxy({}, \
             {getOwnPropertyDescriptor: function () { return {value: 1, configurable: false}; }}), 'x') } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 15 — nor describe something the target could not have held.
    assert_eq!(
        run("var t = Object.preventExtensions({}); \
             try { Object.getOwnPropertyDescriptor(new Proxy(t, \
             {getOwnPropertyDescriptor: function () { return {value: 1}; }}), 'x') } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // The answer must be a descriptor or `undefined` and nothing else.
    assert_eq!(
        run("try { Object.getOwnPropertyDescriptor(new Proxy({}, \
             {getOwnPropertyDescriptor: function () { return 1; }}), 'x') } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn an_invariant_is_checked_against_the_target_through_its_own_traps() {
    // §10.5.8 step 9 asks `target.[[GetOwnProperty]]`, not the target's property table — and the
    // target of a proxy may be another proxy. Reading the table instead finds nothing on the inner
    // proxy and checks no invariant at all, which is a lie that gets through silently.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1});              try { new Proxy(new Proxy(t, {}), {get: function () { return 2; }}).x }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1});              try { 'x' in new Proxy(new Proxy(t, {}), {has: function () { return false; }}) }              catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_descriptor_trap_is_judged_in_the_order_the_steps_are_written() {
    // §10.5.5 step 6 judges the answer's *type* before step 7 reads the target, and `IsExtensible`
    // is not asked until step 8.c — after the absent case has been settled two cheaper ways.
    //
    // Both are only observable when the *target* is itself a proxy, because the invariants are
    // asked of the target and not of the object the trap is on. That is also why getting the order
    // wrong is easy: with an ordinary target nothing distinguishes it.
    assert_eq!(
        run(
            "var asked = false;              var inner = new Proxy({}, {getOwnPropertyDescriptor: function () { asked = true; }});              var p = new Proxy(inner, {getOwnPropertyDescriptor: function () { return 1; }});              try { Object.getOwnPropertyDescriptor(p, 'x') } catch (e) {} asked"
        ),
        "false"
    );
    assert_eq!(
        run(
            "var asked = false;              var inner = new Proxy({}, {isExtensible: function () { asked = true; return true; }});              var p = new Proxy(inner, {getOwnPropertyDescriptor: function () {}});              Object.getOwnPropertyDescriptor(p, 'absent'); asked"
        ),
        "false"
    );
    // …and it *is* asked once the target really has the property the trap is hiding.
    assert_eq!(
        run(
            "var asked = false;              var inner = new Proxy({a: 1},              {isExtensible: function () { asked = true; return true; }});              var p = new Proxy(inner, {getOwnPropertyDescriptor: function () {}});              Object.getOwnPropertyDescriptor(p, 'a'); asked"
        ),
        "true"
    );
}

#[test]
fn a_define_trap_is_handed_the_descriptor_the_caller_actually_wrote() {
    // §10.5.6 step 7 — `FromPropertyDescriptor` of the *partial* descriptor, so a handler can tell
    // `{value: 1}` from `{value: 1, enumerable: false}`. Completing it first would make those two
    // indistinguishable to every trap ever written.
    assert_eq!(
        run(
            "var seen; var p = new Proxy({}, {defineProperty: function (t, k, d) { \
             seen = Object.keys(d).join(); return true; }}); \
             Object.defineProperty(p, 'x', {value: 1}); seen"
        ),
        "value"
    );
    assert_eq!(
        run("Reflect.defineProperty(new Proxy({}, \
             {defineProperty: function () { return false; }}), 'x', {value: 1})"),
        "false"
    );
    // …and `Object.defineProperty` throws where `Reflect.defineProperty` reports.
    assert_eq!(
        run("try { Object.defineProperty(new Proxy({}, \
             {defineProperty: function () { return false; }}), 'x', {value: 1}) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §10.5.6 step 16 — nothing may be added to a non-extensible target.
    assert_eq!(
        run("var t = Object.preventExtensions({}); \
             try { Object.defineProperty(new Proxy(t, \
             {defineProperty: function () { return true; }}), 'x', {value: 1}) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a property that does not exist cannot be made permanent, because there would be
    // nothing for the promise to be about.
    assert_eq!(
        run("try { Object.defineProperty(new Proxy({}, \
             {defineProperty: function () { return true; }}), 'x', \
             {value: 1, configurable: false}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 17 — a configurable property may not be reported as having become permanent.
    assert_eq!(
        run("try { Object.defineProperty(new Proxy({x: 1}, \
             {defineProperty: function () { return true; }}), 'x', \
             {value: 1, configurable: false}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn freezing_a_proxy_runs_every_step_of_it_through_the_traps() {
    // §7.3.14 `SetIntegrityLevel` is `[[PreventExtensions]]`, then `[[OwnPropertyKeys]]`, then a
    // `[[DefineOwnProperty]]` per key — three different traps for one call.
    assert_eq!(
        run("var t = {a: 1}; Object.freeze(new Proxy(t, {})); Object.isFrozen(t)"),
        "true"
    );
    assert_eq!(
        run("Object.isFrozen(new Proxy(Object.freeze({}), {}))"),
        "true"
    );
    assert_eq!(
        run("Object.isSealed(new Proxy(Object.seal({}), {}))"),
        "true"
    );
    assert_eq!(run("Object.isFrozen(new Proxy({a: 1}, {}))"), "false");
    // Step 3 — `[[PreventExtensions]]` answering `false` stops the whole operation, and
    // `Object.freeze` then throws rather than returning a half-frozen object.
    assert_eq!(
        run("try { Object.freeze(new Proxy({}, \
             {preventExtensions: function () { return false; }})) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §7.3.15 step 3 — an extensible object is frozen to no level, whatever its properties say.
    assert_eq!(
        run(
            "Object.isFrozen(new Proxy({}, {isExtensible: function () { return true; }, \
             ownKeys: function () { return []; }}))"
        ),
        "false"
    );
}

#[test]
fn is_array_looks_through_a_proxy_rather_than_asking_it() {
    // §7.2.2 — the one question about a proxy that consults no handler at all. It reads
    // `[[ProxyTarget]]` directly, so no trap can change the answer, and that is what lets
    // `JSON.stringify` tell an array from an object safely.
    assert_eq!(run("Array.isArray(new Proxy([], {}))"), "true");
    assert_eq!(run("Array.isArray(new Proxy({}, {}))"), "false");
    assert_eq!(
        run("Array.isArray(new Proxy([], {get: function () { return 1; }}))"),
        "true"
    );
    assert_eq!(
        run("Array.isArray(new Proxy(new Proxy([], {}), {}))"),
        "true"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Proxy([], {}))"),
        "[object Array]"
    );
    assert_eq!(run("JSON.stringify(new Proxy([1, 2], {}))"), "[1,2]");
    assert_eq!(run("[0].concat(new Proxy([1, 2], {})).join()"), "0,1,2");
    assert_eq!(run("[[1], new Proxy([2], {})].flat().join()"), "1,2");
    // …and a revoked proxy has no target to look through to, so the question throws.
    assert_eq!(
        run("var r = Proxy.revocable([], {}); r.revoke(); \
             try { Array.isArray(r.proxy) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_trap_that_throws_carries_its_error_out_through_whatever_asked() {
    // The traps are JavaScript, so every operation that consults one becomes an operation that can
    // throw — including several that never could before.
    for source in [
        "p.x",
        "p.x = 1",
        "'x' in p",
        "delete p.x",
        "Object.keys(p)",
        "for (var k in p) {}",
        "Object.getPrototypeOf(p)",
        "Object.isExtensible(p)",
        "JSON.stringify(p)",
        "({...p})",
    ] {
        assert_eq!(
            run(&format!(
                "var boom = function () {{ throw new RangeError('trap'); }}; \
                 var p = new Proxy({{}}, {{get: boom, set: boom, has: boom, deleteProperty: boom, \
                 ownKeys: boom, getPrototypeOf: boom, isExtensible: boom}}); \
                 try {{ {source}; 'no throw' }} catch (e) {{ e.constructor.name }}"
            )),
            "RangeError",
            "{source} should carry a trap's throw out"
        );
    }
}

#[test]
fn a_trap_that_is_present_but_not_callable_is_refused_before_it_is_called() {
    // §10.5's step 5 checks `IsCallable` and throws — it does not call and let the call fail.
    // Both end in a TypeError, so the *message* is the only thing that says which happened, and
    // getting this wrong means a handler with a typo reports the wrong problem.
    assert_eq!(
        run("try { new Proxy({}, {get: 1}).a } catch (e) { e.message }"),
        "this proxy trap is not a function"
    );
    assert_eq!(
        run("try { Object.keys(new Proxy({}, {ownKeys: {}})) } catch (e) { e.message }"),
        "this proxy trap is not a function"
    );
}

#[test]
fn a_fixed_accessor_that_has_a_getter_constrains_a_get_trap_not_at_all() {
    // §10.5.8 step 10.b — the rule is about an accessor with **no** getter, whose value is
    // `undefined` however the trap answers. One that has a getter could return anything, so the
    // trap may too, and answering `undefined` is not a contradiction.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {get: function () { return 1; }}); \
             typeof new Proxy(t, {get: function () {}}).x"
        ),
        "undefined"
    );
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {get: function () { return 1; }}); \
             new Proxy(t, {get: function () { return 2; }}).x"
        ),
        "2"
    );
}

#[test]
fn every_trap_answers_the_boolean_its_internal_method_reports() {
    // §10.5.7, §10.5.9 and §10.5.10 each answer a Boolean, and `Reflect` is where it is visible:
    // `delete` and `in` show it directly, and a `[[Set]]` refusal is silent except through
    // `Reflect.set` or strict mode.
    assert_eq!(run("Reflect.set(new Proxy({}, {}), 'x', 1)"), "true");
    assert_eq!(
        run("Reflect.set(new Proxy({}, {set: function () { return true; }}), 'x', 1)"),
        "true"
    );
    assert_eq!(
        run("Reflect.set(new Proxy({}, {set: function () { return false; }}), 'x', 1)"),
        "false"
    );
    assert_eq!(
        run("'x' in new Proxy({}, {has: function () { return false; }})"),
        "false"
    );
    assert_eq!(
        run("delete new Proxy({x: 1}, {deleteProperty: function () { return true; }}).x"),
        "true"
    );
    assert_eq!(
        run("delete new Proxy({}, {deleteProperty: function () { return false; }}).x"),
        "false"
    );
    // …and a refused write is a TypeError in strict code, which is the only place assignment
    // reports it.
    assert_eq!(
        run(
            "'use strict'; try { new Proxy({}, {set: function () { return false; }}).x = 1 } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_prototype_or_extensibility_trap_that_agrees_with_its_target_is_allowed_through() {
    // Each of these is the *other* side of an invariant tested above: the throw says the rule is
    // there, and this says the rule does not fire on the operation it is meant to permit.
    assert_eq!(
        run("var t = Object.preventExtensions(Object.create(null)); \
             Reflect.setPrototypeOf(new Proxy(t, \
             {setPrototypeOf: function () { return true; }}), null)"),
        "true"
    );
    // §10.5.4 — preventing extensions on a target that already has none is the one case the trap
    // may report success for.
    assert_eq!(
        run(
            "Reflect.preventExtensions(new Proxy(Object.preventExtensions({}), \
             {preventExtensions: function () { return true; }}))"
        ),
        "true"
    );
}

#[test]
fn a_descriptor_trap_may_not_describe_a_property_the_target_could_not_have_held() {
    // §10.5.5 step 15 — `IsCompatiblePropertyDescriptor`. A fixed value may not be re-described
    // as another one, and this is a *different* rule from step 17's about configurability: it
    // fires even when the trap agrees the property is permanent.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { Object.getOwnPropertyDescriptor(new Proxy(t, {getOwnPropertyDescriptor: \
             function () { return {value: 2, writable: false, enumerable: false, \
             configurable: false}; }}), 'x') } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // Step 17.b — a non-configurable *writable* property may not be reported as non-writable,
    // because non-configurable and non-writable together is the one state nothing can undo.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1, writable: true}); \
             try { Object.getOwnPropertyDescriptor(new Proxy(t, {getOwnPropertyDescriptor: \
             function () { return {value: 1, writable: false, enumerable: false, \
             configurable: false}; }}), 'x') } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …and reporting it as writable, which is what it is, is fine.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1, writable: true}); \
             Object.getOwnPropertyDescriptor(new Proxy(t, {getOwnPropertyDescriptor: \
             function () { return {value: 1, writable: true, enumerable: false, \
             configurable: false}; }}), 'x').writable"
        ),
        "true"
    );
    // §6.2.6.6 — a descriptor with a getter is an *accessor*, and one with a setter alone is too.
    // Completing it as a data property instead would answer `value: undefined` for both.
    assert_eq!(
        run(
            "typeof Object.getOwnPropertyDescriptor(new Proxy({}, {getOwnPropertyDescriptor: \
             function () { return {get: function () { return 1; }, configurable: true}; }}), \
             'x').get"
        ),
        "function"
    );
    assert_eq!(
        run(
            "typeof Object.getOwnPropertyDescriptor(new Proxy({}, {getOwnPropertyDescriptor: \
             function () { return {set: function () {}, configurable: true}; }}), 'x').set"
        ),
        "function"
    );
    // …and an absent `configurable` completes to **false**, which then trips step 17: a trap
    // cannot report a permanent property the target does not have, even by omission.
    assert_eq!(
        run(
            "try { Object.getOwnPropertyDescriptor(new Proxy({}, {getOwnPropertyDescriptor: \
             function () { return {value: 5}; }}), 'x') } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_define_trap_may_not_accept_a_change_the_target_could_not_have_made() {
    // §10.5.6 step 16.b — `IsCompatiblePropertyDescriptor` again, on the way in rather than out.
    assert_eq!(
        run("var t = {}; Object.defineProperty(t, 'x', {value: 1}); \
             try { Object.defineProperty(new Proxy(t, \
             {defineProperty: function () { return true; }}), 'x', {value: 2}) } \
             catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // …and a change that *is* compatible goes through, which is what says the rule is a rule and
    // not a refusal to define anything at all.
    assert_eq!(
        run("Reflect.defineProperty(new Proxy({x: 1}, \
             {defineProperty: function () { return true; }}), 'x', {value: 2})"),
        "true"
    );
    assert_eq!(
        run("Reflect.defineProperty(new Proxy({}, \
             {defineProperty: function () { return true; }}), 'fresh', {value: 2})"),
        "true"
    );
    // Step 17.c — a permanent writable property may not be reported as having become non-writable.
    assert_eq!(
        run(
            "var t = {}; Object.defineProperty(t, 'x', {value: 1, writable: true}); \
             try { Object.defineProperty(new Proxy(t, \
             {defineProperty: function () { return true; }}), 'x', {writable: false}) } \
             catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // …but a *configurable* one may: it can be changed back.
    assert_eq!(
        run("Reflect.defineProperty(new Proxy({x: 1}, \
             {defineProperty: function () { return true; }}), 'x', {writable: false})"),
        "true"
    );
    // Step 7 — the descriptor a trap is handed has the ordinary attributes of a fresh object's
    // properties, so a handler can rewrite it before passing it on.
    assert_eq!(
        run(
            "var seen; var p = new Proxy({}, {defineProperty: function (t, k, d) { \
             var inner = Object.getOwnPropertyDescriptor(d, 'value'); \
             seen = inner.writable + ',' + inner.enumerable + ',' + inner.configurable; \
             return true; }}); Object.defineProperty(p, 'x', {value: 1}); seen"
        ),
        "true,true,true"
    );
}

#[test]
fn an_own_keys_trap_on_an_extensible_target_may_add_but_not_omit() {
    // §10.5.11 steps 17 and 18 — the case between the two extremes: an extensible target with a
    // permanent key. That key must appear, and anything else may, because the target could still
    // grow the rest.
    assert_eq!(
        run(
            "var t = {a: 1}; Object.defineProperty(t, 'fixed', {value: 1}); \
             Reflect.ownKeys(new Proxy(t, \
             {ownKeys: function () { return ['fixed', 'invented']; }})).join()"
        ),
        "fixed,invented"
    );
    // …and the configurable key it left out is not missed, because an extensible target could
    // have lost it.
    assert_eq!(
        run(
            "var t = {a: 1}; Object.defineProperty(t, 'fixed', {value: 1}); \
             try { Reflect.ownKeys(new Proxy(t, \
             {ownKeys: function () { return ['invented']; }})) } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_for_in_walk_does_not_ask_about_a_key_it_is_going_to_discard() {
    // §14.7.5.10 filters to String keys *before* asking `[[GetOwnProperty]]`, and with a proxy in
    // the chain that ordering is a trap call that either happens or does not. A Symbol listed by
    // an `ownKeys` trap must cost nothing at all.
    assert_eq!(
        run("var asked = []; var s = Symbol('s'); \
             var p = new Proxy({}, {ownKeys: function () { return ['a', s]; }, \
             getOwnPropertyDescriptor: function (t, k) { asked.push(typeof k); \
             return {value: 1, enumerable: true, configurable: true}; }, \
             has: function () { return true; }}); \
             var out = []; for (var k in p) out.push(k); out.join() + '|' + asked.join()"),
        "a|string"
    );
}

#[test]
fn the_listings_that_ask_for_a_descriptor_and_the_ones_that_do_not() {
    // §20.1.2.11.1 `GetOwnPropertyKeys` filters by *type* alone, so `getOwnPropertyNames` reports
    // whatever `ownKeys` said. §7.3.24 `EnumerableOwnProperties` asks for each key's descriptor,
    // so `Object.keys` can end up shorter. The two used to be one loop, and were wrong for it.
    assert_eq!(
        run(
            "var p = new Proxy({}, {ownKeys: function () { return ['a']; }, \
             getOwnPropertyDescriptor: function () {}}); \
             Object.getOwnPropertyNames(p).join() + '|' + Object.keys(p).length"
        ),
        "a|0"
    );
}

#[test]
fn a_proxy_is_callable_exactly_when_the_target_it_was_made_with_was() {
    // §10.5 — a proxy has a `[[Call]]` only if the *initial* target had one, and a `[[Construct]]`
    // only if the target was a constructor. Decided when the proxy is made and never revisited,
    // which is why an `apply` trap in front of a plain object does nothing at all: there is no
    // `[[Call]]` for it to be the body of.
    assert_eq!(run("typeof new Proxy(function () {}, {})"), "function");
    assert_eq!(run("typeof new Proxy({}, {})"), "object");
    assert_eq!(
        run("typeof new Proxy({}, {apply: function () { return 1; }})"),
        "object"
    );
    assert_eq!(
        run("Object.prototype.toString.call(new Proxy(function () {}, {}))"),
        "[object Function]"
    );
    // An arrow has a `[[Call]]` and no `[[Construct]]`, and a proxy over one answers the same.
    assert_eq!(
        run(
            "try { new (new Proxy(function () {}, {}))(); 'constructed' } \
             catch (e) { e.constructor.name }"
        ),
        "constructed"
    );
    assert_eq!(
        run("try { new (new Proxy(Math.max, {}))() } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn calling_a_proxy_with_no_apply_trap_calls_its_target() {
    assert_eq!(
        run("new Proxy(function (a, b) { return a + b; }, {})(2, 3)"),
        "5"
    );
    assert_eq!(
        run("[3, 1, 2].sort(new Proxy(function (a, b) { return a - b; }, {})).join()"),
        "1,2,3"
    );
    // A built-in behind a proxy is reached the same way, which is what says the fall-through is
    // `Call` and not a re-implementation of one kind of function.
    assert_eq!(run("new Proxy(Math.max, {})(1, 5, 3)"), "5");
    // The receiver goes through untouched — §10.5.12 passes `thisArgument` along.
    assert_eq!(
        run("new Proxy(function () { return this.tag; }, {}).call({tag: 'here'})"),
        "here"
    );
}

#[test]
fn an_apply_trap_is_handed_the_target_the_receiver_and_the_arguments_as_an_array() {
    // §10.5.12 step 7 — an **array**, not an argument list, which is what lets one trap stand in
    // front of functions of any arity.
    assert_eq!(
        run(
            "new Proxy(function () {}, {apply: function (t, self, args) { \
             return Array.isArray(args) + ':' + args.join('-'); }})(1, 2, 3)"
        ),
        "true:1-2-3"
    );
    assert_eq!(
        run(
            "new Proxy(function () {}, {apply: function (t, self) { return self.tag; }})\
             .call({tag: 'given'})"
        ),
        "given"
    );
    assert_eq!(
        run("var f = function () { return 'target'; }; \
             new Proxy(f, {apply: function (t) { return t === f; }})()"),
        "true"
    );
    // A call with no arguments hands the trap an empty array rather than `undefined`.
    assert_eq!(
        run(
            "new Proxy(function () {}, {apply: function (t, self, args) { \
             return args.length; }})()"
        ),
        "0"
    );
}

#[test]
fn constructing_through_a_proxy_keeps_new_target_pointing_at_the_proxy() {
    // §10.5.13 with no trap is `Construct(target, args, newTarget)`, and `newTarget` is the proxy
    // — so §10.1.13 reads `prototype` off *it*, which §10.5.8 answers from the target. Reading the
    // property table instead finds nothing on a proxy, and every instance then inherited from
    // `Object.prototype` rather than from the constructor.
    assert_eq!(
        run(
            "function C() { this.made = 1; } var p = new Proxy(C, {}); var o = new p(); \
             (o instanceof C) + ',' + (Object.getPrototypeOf(o) === C.prototype) + ',' + o.made"
        ),
        "true,true,1"
    );
    // …and a `get` trap that answers a different `prototype` decides what the instance inherits
    // from, which is the observable half of that being a `[[Get]]`.
    assert_eq!(
        run("function C() {} \
             var p = new Proxy(C, {get: function (t, k) { \
             return k === 'prototype' ? Array.prototype : t[k]; }}); \
             Object.getPrototypeOf(new p()) === Array.prototype"),
        "true"
    );
    assert_eq!(
        run("class B { constructor() { this.b = 1; } } new (new Proxy(B, {}))().b"),
        "1"
    );
}

#[test]
fn a_construct_trap_decides_what_new_evaluates_to_and_must_answer_an_object() {
    assert_eq!(
        run(
            "new (new Proxy(function () {}, {construct: function (t, args) { \
             return {made: args[0]}; }}))(5).made"
        ),
        "5"
    );
    // Step 9 — a primitive is refused. `new` evaluating to a number is something no other
    // construction in the language can do, and a trap that forgets to return is the common case.
    for answer in ["1", "undefined", "'text'", "null"] {
        assert_eq!(
            run(&format!(
                "try {{ new (new Proxy(function () {{}}, \
                 {{construct: function () {{ return {answer}; }}}}))() }} \
                 catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "a construct trap answering {answer} should be refused"
        );
    }
    // Step 8 — the trap is handed the target, the arguments as an array, and `new.target`.
    assert_eq!(
        run("var f = function () {}; \
             var p = new Proxy(f, {construct: function (t, args, nt) { \
             return {seen: (t === f) + ',' + Array.isArray(args) + ',' + (nt === p)}; }}); \
             new p(1).seen"),
        "true,true,true"
    );
}

#[test]
fn a_revoked_callable_proxy_is_still_a_function_and_refuses_to_run() {
    // Revocation empties the target and handler; it does not take the `[[Call]]` away. So `typeof`
    // still says `"function"` and calling it is a TypeError, and both halves are observable.
    assert_eq!(
        run("var r = Proxy.revocable(function () { return 1; }, {}); \
             var before = r.proxy(); r.revoke(); \
             typeof r.proxy + ',' + before + ',' + \
             (function () { try { r.proxy() } catch (e) { return e.constructor.name } })()"),
        "function,1,TypeError"
    );
    assert_eq!(
        run("var r = Proxy.revocable(function () {}, {}); r.revoke(); \
             try { new r.proxy() } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn a_plain_call_never_reads_the_callee_s_prototype() {
    // §10.2.1 does not make a receiver, so it has no reason to ask for `prototype` — only
    // §10.2.2's construction does. An ordinary function's own `prototype` is a plain data property
    // and reading it costs nothing visible, so the read is caught where there is no own one to
    // find: an arrow has none, the lookup walks to `Function.prototype`, and an accessor put there
    // records every read that happens.
    assert_eq!(
        run("var log = []; \
             Object.defineProperty(Function.prototype, 'prototype', \
             {get: function () { log.push('read'); return {}; }, configurable: true}); \
             var f = () => 1; f(); log.length"),
        "0"
    );
    // …and a construction through a proxy reads it exactly once, which is what says the read
    // belongs to the construction rather than having been removed altogether.
    assert_eq!(
        run("var log = []; \
             var p = new Proxy(function () {}, \
             {get: function (t, k) { log.push(k); return t[k]; }}); \
             new p(); log.join()"),
        "prototype"
    );
}
