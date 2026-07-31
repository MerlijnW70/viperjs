//! §B.2.2's four accessor methods, and the two ways they are not `Object.defineProperty`.

use super::*;

#[test]
fn defining_an_accessor_the_old_way_makes_an_enumerable_property() {
    // §B.2.2.1 step 3 — **enumerable and configurable**, where a descriptor with those fields
    // absent gets `false` for both. That is the whole difference from `defineProperty`, and it is
    // visible from `Object.keys` rather than only from a descriptor.
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 1; }); \
             var d = Object.getOwnPropertyDescriptor(o, 'x'); \
             o.x + ',' + d.enumerable + ',' + d.configurable + ',' + (typeof d.get) \
             + ',' + (d.set === undefined)"
        ),
        "1,true,true,function,true"
    );
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 1; }); \
             Object.keys(o).join(',')"
        ),
        "x"
    );
    // …and the contrast, which is what says the attributes are being set rather than defaulted.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'x', {get: function () { return 1; }}); \
             Object.getOwnPropertyDescriptor(o, 'x').enumerable"
        ),
        "false"
    );
    // §B.2.2.2 — the setter half, and it runs.
    assert_eq!(
        run(
            "var o = {}; var seen; o.__defineSetter__('x', function (v) { seen = v; }); \
             o.x = 7; seen + ',' + (o.x === undefined)"
        ),
        "7,true"
    );
    // Both answer `undefined`, which is worth pinning because "returns the object" would be the
    // obvious guess and would let them chain.
    assert_eq!(
        run(
            "var o = {}; typeof o.__defineGetter__('x', function () {}) + ',' \
             + typeof o.__defineSetter__('y', function () {})"
        ),
        "undefined,undefined"
    );
    // Defining one half then the other leaves an accessor with both, because the second call
    // redefines rather than replacing — a configurable property, so it is allowed to.
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 'g'; }); \
             o.__defineSetter__('x', function () {}); \
             var d = Object.getOwnPropertyDescriptor(o, 'x'); \
             o.x + ',' + (typeof d.get) + ',' + (typeof d.set)"
        ),
        "g,function,function"
    );
    // The key goes through `ToPropertyKey`, so a number and a Symbol both work.
    assert_eq!(
        run("var o = {}; o.__defineGetter__(1, function () { return 'one'; }); o[1]"),
        "one"
    );
    assert_eq!(
        run(
            "var o = {}; var s = Symbol('s'); o.__defineGetter__(s, function () { return 'sym'; }); \
             o[s]"
        ),
        "sym"
    );
}

#[test]
fn a_half_that_is_not_a_function_is_refused_before_the_key_is_read() {
    // §B.2.2.1 step 2 comes before the key conversion, so a bad getter is reported as a bad getter
    // even when the key would also have thrown. An engine converting the key first says the wrong
    // thing here, and this is the only row that can tell the two orders apart.
    assert_eq!(
        run(
            "var o = {}; try { o.__defineGetter__({toString: function () { throw new RangeError('key'); }}, 1); } \
             catch (e) { e.constructor.name + ':' + e.message }"
        ),
        "TypeError:the getter is not a function"
    );
    assert_eq!(
        run("var o = {}; try { o.__defineSetter__('x', 1); } catch (e) { e.message }"),
        "the setter is not a function"
    );
    for bad in ["1", "'f'", "undefined", "null", "{}"] {
        assert_eq!(
            run(&format!(
                "var o = {{}}; try {{ o.__defineGetter__('x', {bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "getter {bad}"
        );
    }
    // A property that cannot be redefined is a TypeError rather than a silent nothing — this is
    // `DefinePropertyOrThrow` and not `[[DefineOwnProperty]]`.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'x', {value: 1, configurable: false}); \
             try { o.__defineGetter__('x', function () {}); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn looking_one_up_walks_the_chain_and_stops_at_the_first_property_of_any_kind() {
    // §B.2.2.3 step 3 — the walk finds the accessor a program would actually *reach*, which is
    // what makes these different from `getOwnPropertyDescriptor`.
    assert_eq!(
        run("var base = {}; var get = function () { return 1; }; \
             base.__defineGetter__('x', get); \
             var derived = Object.create(base); \
             (derived.__lookupGetter__('x') === get) + ',' \
             + (Object.getOwnPropertyDescriptor(derived, 'x') === undefined)"),
        "true,true"
    );
    // …and it stops at the **first** object that has the property at all. A data property part-way
    // up answers `undefined` rather than being stepped over — because that data property is what
    // the program would reach, and it has no getter.
    //
    // Defined rather than assigned: `middle.x = 2` reaches the inherited accessor, which has no
    // setter, and a sloppy-mode assignment to one of those is silently ignored — so `middle` would
    // have had no own property at all and the row would have proved nothing.
    assert_eq!(
        run(
            "var base = {}; base.__defineGetter__('x', function () { return 1; }); \
             var middle = Object.create(base); \
             Object.defineProperty(middle, 'x', {value: 2}); \
             var derived = Object.create(middle); \
             derived.__lookupGetter__('x') === undefined"
        ),
        "true"
    );
    // An accessor with only a setter answers `undefined` for the getter, and the other way about.
    assert_eq!(
        run(
            "var o = {}; var set = function () {}; o.__defineSetter__('x', set); \
             (o.__lookupGetter__('x') === undefined) + ',' + (o.__lookupSetter__('x') === set)"
        ),
        "true,true"
    );
    // A name nothing in the chain has answers `undefined` rather than throwing, and the walk ends
    // at a prototype of `null` rather than running on.
    assert_eq!(
        run("var bare = Object.create(null); bare.__proto__x = 1; \
             typeof Object.prototype.__lookupGetter__.call(bare, 'nothing')"),
        "undefined"
    );
    assert_eq!(run("typeof ({}).__lookupSetter__('nothing')"), "undefined");
    // The key is converted the same way, so a Symbol is looked up as a Symbol.
    assert_eq!(
        run(
            "var o = {}; var s = Symbol('s'); var get = function () { return 1; }; \
             o.__defineGetter__(s, get); o.__lookupGetter__(s) === get"
        ),
        "true"
    );
    // §10.3.3's `length` and `name` — two arguments for the pair that define, one for the pair
    // that look up.
    assert_eq!(
        run(
            "[Object.prototype.__defineGetter__.length, Object.prototype.__defineSetter__.length, \
             Object.prototype.__lookupGetter__.length, Object.prototype.__lookupSetter__.length] \
             .join(',')"
        ),
        "2,2,1,1"
    );
    // …and they are not enumerable, like every other built-in.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Object.prototype, '__defineGetter__').enumerable"),
        "false"
    );
}
