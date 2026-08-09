//! §20.1 as a script sees it — `Object`, and a property descriptor as a value.

use super::*;

#[test]
fn a_descriptor_read_back_is_complete_where_the_one_written_was_partial() {
    // §6.2.6.4 fills every field in, and §6.2.6.5 leaves absent ones absent. That asymmetry is
    // the whole reason both functions exist: `{value: 1}` defines a property whose other three
    // attributes are `false`, and reading it back says so rather than staying silent.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {value: 1}); var d = Object.getOwnPropertyDescriptor(o, 'a'); d.value + '|' + d.writable + '|' + d.enumerable + '|' + d.configurable"
        ),
        "1|false|false|false"
    );
    // …where a property an *assignment* made has §6.1.7.1's three defaults instead.
    assert_eq!(
        run(
            "var o = {a: 1}; var d = Object.getOwnPropertyDescriptor(o, 'a'); d.writable + '|' + d.enumerable + '|' + d.configurable"
        ),
        "true|true|true"
    );
    // A property that is not there is `undefined` rather than an empty descriptor, which is how
    // a caller tells absent from present-and-undefined.
    assert_eq!(
        run("typeof Object.getOwnPropertyDescriptor({}, 'nope')"),
        "undefined"
    );
    assert_eq!(
        run("var o = {a: undefined}; typeof Object.getOwnPropertyDescriptor(o, 'a')"),
        "object"
    );
}

#[test]
fn an_empty_descriptor_changes_nothing_because_absent_is_not_undefined() {
    // §6.2.6.5 — `{}` sets no field at all, so redefining with it leaves every attribute as it
    // was. `{value: undefined}` is a different thing and does set the value.
    assert_eq!(
        run("var o = {a: 1}; Object.defineProperty(o, 'a', {}); o.a"),
        "1"
    );
    assert_eq!(
        run("var o = {a: 1}; Object.defineProperty(o, 'a', {value: undefined}); typeof o.a"),
        "undefined"
    );
    // …and an attribute left out of a *new* property's descriptor is `false`, not unchanged,
    // because there was nothing to leave alone.
    assert_eq!(
        run("var o = {}; Object.defineProperty(o, 'a', {value: 1}); delete o.a"),
        "false"
    );
}

#[test]
fn define_property_refuses_what_10_1_6_3_refuses_rather_than_doing_nothing() {
    // §20.1.2.4 step 4 is `DefinePropertyOrThrow`, and the throw is the difference between it
    // and `Reflect.defineProperty` — a silent `false` would leave a program believing it had
    // changed something.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {value: 1}); try { Object.defineProperty(o, 'a', {value: 2}) } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // A non-configurable but *writable* property may still have its value changed, which is the
    // rule people are surprised by.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {value: 1, writable: true}); Object.defineProperty(o, 'a', {value: 2}); o.a"
        ),
        "2"
    );
    // §6.2.6.5 step 21 — a descriptor may not be both kinds at once.
    assert_eq!(
        run(
            "try { Object.defineProperty({}, 'a', {value: 1, get: function () {}}) } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // …nor may an accessor be something that cannot be called.
    assert_eq!(
        run("try { Object.defineProperty({}, 'a', {get: 1}) } catch (e) { e.name }"),
        "TypeError"
    );
    // The target and the descriptor must both be objects.
    assert_eq!(
        run("try { Object.defineProperty(1, 'a', {}) } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("try { Object.defineProperty({}, 'a', 1) } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn a_descriptor_field_may_be_inherited_because_6_2_6_5_asks_has_property() {
    // Not a curiosity: it is what lets a program build one descriptor out of another, and
    // reading only own properties would silently drop the inherited half.
    assert_eq!(
        run(
            "var base = {writable: true}; var d = Object.create(base); d.value = 1; var o = {}; Object.defineProperty(o, 'a', d); o.a = 2; o.a"
        ),
        "2"
    );
}

#[test]
fn a_descriptor_field_may_be_a_getter_because_6_2_6_5_asks_get() {
    // The other half of the same sentence. §6.2.6.5 reads each field with `HasProperty` and then
    // **`Get`**, and `Get` calls an accessor — so a descriptor may compute its own fields. Reading
    // the property table instead finds the accessor and has nothing to do with it; ViperJS used to
    // refuse outright, which turned a legal descriptor into a TypeError.
    assert_eq!(
        run("var o = {}; Object.defineProperty(o, 'x', { get value() { return 5; } }); o.x"),
        "5"
    );
    // …including through a descriptor *list*, where §20.1.2.3.1 step 3.b.i is the same `Get`.
    assert_eq!(
        run("var o = {}; Object.defineProperties(o, { a: { get value() { return 1; } } }); o.a"),
        "1"
    );
    // The fields are read in §6.2.6.5's order, which is **not** the order they are written in
    // anywhere else: `value` (step 7) comes before `writable` (step 8). Nothing could see that
    // until a field was allowed to be a getter, and now two of them with side effects can see it
    // exactly. `enumerable` and `configurable` come before both.
    assert_eq!(
        run("var log = ''; Object.defineProperty({}, 'x', { \
             get writable() { log += 'w'; return true; }, \
             get value() { log += 'v'; return 1; }, \
             get configurable() { log += 'c'; return true; }, \
             get enumerable() { log += 'e'; return true; } }); log"),
        "ecvw"
    );
    // A getter that throws throws from here, which is the point of calling it rather than reading
    // around it — and it throws before the property is defined.
    assert_eq!(
        run("var o = {}; try { Object.defineProperty(o, 'x', \
             { get value() { throw new RangeError('no'); } }); } \
             catch (e) { e.constructor.name + ',' + ('x' in o); }"),
        "RangeError,false"
    );
}

#[test]
fn object_create_is_the_only_way_to_make_an_object_with_no_prototype() {
    assert_eq!(run("Object.getPrototypeOf(Object.create(null))"), "null");
    assert_eq!(run("var p = {x: 1}; Object.create(p).x"), "1");
    assert_eq!(
        run("var p = {}; Object.getPrototypeOf(Object.create(p)) === p"),
        "true"
    );
    // The second argument is a descriptor list, exactly as `defineProperties` takes one.
    assert_eq!(
        run(
            "var o = Object.create(null, {a: {value: 1, enumerable: true}}); o.a + '|' + Object.keys(o).length"
        ),
        "1|1"
    );
    // A prototype that is neither an object nor null is refused rather than ignored.
    assert_eq!(
        run("try { Object.create(1) } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn define_properties_applies_every_descriptor_or_none_of_them() {
    // §20.1.2.3.1 step 4 reads them all before applying any, so a malformed second descriptor
    // must not leave the first one applied. Without that, a failed call would leave the object
    // half-changed with no way to tell how far it got.
    assert_eq!(
        run(
            "var o = {}; try { Object.defineProperties(o, {a: {value: 1}, b: 1}) } catch (e) { typeof o.a }"
        ),
        "undefined"
    );
    assert_eq!(
        run(
            "var o = {}; Object.defineProperties(o, {a: {value: 1}, b: {value: 2, enumerable: true}}); o.a + '|' + o.b + '|' + Object.keys(o).length"
        ),
        "1|2|1"
    );
    // A non-enumerable property of the *list* is skipped, because §20.1.2.3.1 walks own
    // enumerable keys.
    assert_eq!(
        run(
            "var list = {}; Object.defineProperty(list, 'a', {value: {value: 1}}); var o = {}; Object.defineProperties(o, list); typeof o.a"
        ),
        "undefined"
    );
}

#[test]
fn keys_and_get_own_property_names_differ_in_one_filter_and_in_nothing_else() {
    // §10.1.11's order is creation order for string keys, and both functions answer in it.
    assert_eq!(
        run("var o = {b: 1, a: 2}; Object.keys(o)[0] + Object.keys(o)[1]"),
        "ba"
    );
    assert_eq!(
        run(
            "var o = {a: 1}; Object.defineProperty(o, 'hidden', {value: 2}); Object.keys(o).length + '|' + Object.getOwnPropertyNames(o).length"
        ),
        "1|2"
    );
    // Own only — a prototype's properties are not listed, which is what makes `Object.keys` safe
    // where `for...in` is not.
    assert_eq!(
        run("var o = Object.create({inherited: 1}); Object.keys(o).length"),
        "0"
    );
}

#[test]
fn the_three_questions_an_object_answers_about_its_own_properties() {
    // `hasOwnProperty` asks whether it is there, `propertyIsEnumerable` whether it shows, and
    // `isPrototypeOf` whether this object is on another one's chain. Two are own-only and one
    // walks the chain, and confusing them is a classic bug.
    assert_eq!(
        run("var o = {a: 1}; o.hasOwnProperty('a') + '|' + o.hasOwnProperty('b')"),
        "true|false"
    );
    assert_eq!(run("Object.create({a: 1}).hasOwnProperty('a')"), "false");
    assert_eq!(run("Object.hasOwn({a: 1}, 'a')"), "true");
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {value: 1}); o.propertyIsEnumerable('a') + '|' + o.hasOwnProperty('a')"
        ),
        "false|true"
    );
    let chain = "var p = {}; var o = Object.create(p); var far = Object.create(o);";
    assert_eq!(run(&format!("{chain} p.isPrototypeOf(far)")), "true");
    assert_eq!(run(&format!("{chain} far.isPrototypeOf(p)")), "false");
    // An object is not its own prototype, which is step 3's loop starting from the *next* link.
    assert_eq!(run("var o = {}; o.isPrototypeOf(o)"), "false");
    assert_eq!(run("var o = {}; o.isPrototypeOf(1)"), "false");
}

#[test]
fn every_to_object_in_section_20_1_3_is_a_conversion_and_not_a_check() {
    // Four of §20.1.3's methods begin `Let O be ? ToObject(this value)`, and ViperJS read all four
    // as "if this is not an Object, throw". The difference is only ever visible with a **primitive**
    // receiver, which is a shape nothing writes by hand and every generic helper produces — so it
    // sat there. `undefined` and `null` are the only receivers with no object at all, and they are
    // where the TypeError belongs.
    //
    // §20.1.3.7 — the answer is a *wrapper*, so `typeof` of it is "object". This is the row that
    // says the conversion happened rather than being skipped over.
    assert_eq!(run("typeof Object.prototype.valueOf.call(true)"), "object");
    assert_eq!(
        run(
            "var v = Object.prototype.valueOf.call('ab'); typeof v + ',' + v.length + ',' + (v === 'ab')"
        ),
        "object,2,false"
    );
    assert_eq!(
        run("try { Object.prototype.valueOf.call(null) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // §20.1.3.2 — asked of the wrapper, which has the index properties a String object has and
    // none of the ones its prototype carries.
    assert_eq!(
        run("Object.prototype.hasOwnProperty.call('ab', 0) + ',' \
             + Object.prototype.hasOwnProperty.call('ab', 'length') + ',' \
             + Object.prototype.hasOwnProperty.call('ab', 'charAt')"),
        "true,true,false"
    );
    // The one that found it: `description` lives on `Symbol.prototype`, so the object a Symbol
    // stands for does not have it — an answer, where this used to be a TypeError.
    assert_eq!(run("Symbol().hasOwnProperty('description')"), "false");
    // §20.1.3.4, which no test262 file distinguishes — see the comment at the call site.
    assert_eq!(
        run("Object.prototype.propertyIsEnumerable.call('ab', 0) + ',' \
             + Object.prototype.propertyIsEnumerable.call('ab', 'length')"),
        "true,false"
    );
    // §20.1.3.3, and here the *order* is the whole point. Step 1 settles a primitive argument
    // before step 2 can convert the receiver, so these two disagree about the same `null`.
    assert_eq!(
        run("Object.prototype.isPrototypeOf.call(null, 10) + ',' \
             + Object.prototype.isPrototypeOf.call(undefined, '')"),
        "false,false"
    );
    assert_eq!(
        run(
            "try { Object.prototype.isPrototypeOf.call(null, {}) } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    // A primitive receiver that *does* convert answers by walking: the wrapper is a fresh object
    // and no chain contains it, so `false` — arrived at rather than refused.
    assert_eq!(
        run("Object.prototype.isPrototypeOf.call('ab', {})"),
        "false"
    );
}

#[test]
fn a_property_descriptor_list_is_converted_where_the_target_is_refused() {
    // §20.1.2.3.1 `ObjectDefineProperties` begins `ToObject(Properties)`, so a primitive list is a
    // list with no own enumerable keys rather than an error — `Object.create(proto, 1)` makes an
    // object with the prototype and nothing on it. ViperJS refused, which is a TypeError where the
    // clause has an answer.
    assert_eq!(
        run("var p = {}; var o = Object.create(p, 1); \
             (Object.getPrototypeOf(o) === p) + ',' + Object.getOwnPropertyNames(o).length"),
        "true,0"
    );
    for list in ["true", "false", "NaN", "''", "Symbol('s')"] {
        assert_eq!(
            run(&format!(
                "Object.getOwnPropertyNames(Object.create({{}}, {list})).length"
            )),
            "0",
            "a {list} list has no own enumerable keys"
        );
    }
    // A *non-empty* String is the one that shows the conversion really happened rather than being
    // skipped past: its wrapper has own enumerable `"0"` and `"1"`, so step 3.b reads them and
    // `ToPropertyDescriptor` refuses the character `"a"` for not being an object. A TypeError
    // again, and from three steps further in — which is why the empty string above is the row that
    // distinguishes converting from refusing, and this one is the row that says it was *read*.
    assert_eq!(
        run("try { Object.defineProperties({}, 'ab') } catch (e) { e.message }"),
        "a property descriptor must be an object"
    );
    assert_eq!(
        run("var o = {}; Object.defineProperties(o, ''); Object.getOwnPropertyNames(o).length"),
        "0"
    );
    // …and `undefined` and `null` still refuse, because they are the two values `ToObject` has no
    // answer for. `Object.create(p)` with no second argument does **not** reach this at all —
    // §20.1.2.2 step 3 asks whether `Properties` is present before converting anything.
    assert_eq!(
        run("try { Object.create({}, null) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Object.getOwnPropertyNames(Object.create({})).length"),
        "0"
    );
    // The **target** is the argument that still refuses, and that asymmetry is the point: a define
    // against a throwaway wrapper would report success and change nothing anybody can see.
    assert_eq!(
        run("try { Object.defineProperties(1, {}) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn object_prototype_to_string_is_the_type_test_that_answers_for_null() {
    // §20.1.3.6 steps 1 and 2 — it is the idiomatic type test precisely because it does not
    // throw on the two values that have no object to ask.
    assert_eq!(run("({}).toString()"), "[object Object]");
    // An object with no prototype has no `toString` to reach, which is the other half of what
    // `Object.create(null)` is for.
    assert_eq!(
        run("var o = Object.create(null); typeof o.toString"),
        "undefined"
    );
    // `valueOf` on an object is the object itself, which is what makes it invisible in coercion.
    assert_eq!(run("var o = {}; o.valueOf() === o"), "true");
    // Detached, because `Object.prototype.valueOf()` is a *method* call whose receiver is
    // `Object.prototype` itself — and that answers rather than throwing.
    assert_eq!(
        run("var f = Object.prototype.valueOf; try { f() } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        run("Object.prototype.valueOf() === Object.prototype"),
        "true"
    );
}

#[test]
fn extensibility_is_one_way_and_a_primitive_was_never_extensible() {
    assert_eq!(run("var o = {}; Object.isExtensible(o)"), "true");
    assert_eq!(
        run("var o = {}; Object.preventExtensions(o); Object.isExtensible(o)"),
        "false"
    );
    // §20.1.2.19 step 1 — a primitive is handed back rather than refused, because the request is
    // already satisfied; and §20.1.2.16 answers `false` for the same reason.
    assert_eq!(run("Object.preventExtensions(1)"), "1");
    assert_eq!(run("Object.isExtensible(1)"), "false");
    // Existing properties are untouched: this stops *additions* and nothing else.
    assert_eq!(
        run(
            "var o = {a: 1}; Object.preventExtensions(o); o.a = 2; o.b = 3; o.a + '|' + typeof o.b"
        ),
        "2|undefined"
    );
}

#[test]
fn object_hands_back_what_it_was_given_and_makes_one_out_of_nothing() {
    // §20.1.1.1 step 3 — `Object(o) === o`, which is why it is useless as a copy and useful as a
    // coercion.
    assert_eq!(run("var o = {}; Object(o) === o"), "true");
    assert_eq!(run("typeof Object()"), "object");
    assert_eq!(run("typeof Object(null)"), "object");
    assert_eq!(run("typeof Object(undefined)"), "object");
    assert_eq!(run("Object.prototype.constructor === Object"), "true");
    assert_eq!(run("Object.name + '|' + Object.length"), "Object|1");
    // §20.1.2.20 — `Object.prototype` is none of the three, for the same reason
    // `Error.prototype` is not: every object in the realm inherits from it.
    assert_eq!(run("delete Object.prototype"), "false");
    assert_eq!(
        run("var p = Object.prototype; Object.prototype = {}; Object.prototype === p"),
        "true"
    );
    let attributes = "var d = Object.getOwnPropertyDescriptor(Object, 'prototype'); \
                      d.writable + '|' + d.enumerable + '|' + d.configurable";
    assert_eq!(run(attributes), "false|false|false");
}

#[test]
fn everything_this_slice_hands_back_is_an_ordinary_object_with_ordinary_properties() {
    // Two things here make objects out of nothing — the descriptor `getOwnPropertyDescriptor`
    // returns, and the list `keys` returns — and both are specified to be *ordinary*. Nothing
    // else in the language can see those attributes, so this is where they are checked.
    //
    // §6.2.6.4 builds its descriptor with `CreateDataPropertyOrThrow`, which is §6.1.7.1's three
    // defaults: writable, enumerable, configurable. A descriptor a caller cannot edit would be
    // useless as a template, which is what `Object.defineProperty(b, k, Object
    // .getOwnPropertyDescriptor(a, k))` relies on.
    let of_descriptor = "var o = {a: 1}; var d = Object.getOwnPropertyDescriptor(o, 'a'); \
                         var f = Object.getOwnPropertyDescriptor(d, 'value'); \
                         f.writable + '|' + f.enumerable + '|' + f.configurable";
    assert_eq!(run(of_descriptor), "true|true|true");

    // The same for a name in the list `keys` answers with.
    let of_list = "var o = {a: 1}; var list = Object.keys(o); \
                   var f = Object.getOwnPropertyDescriptor(list, '0'); \
                   f.value + '|' + f.writable + '|' + f.enumerable + '|' + f.configurable";
    assert_eq!(run(of_list), "a|true|true|true");
}

#[test]
fn an_accessor_descriptor_is_accepted_when_its_halves_are_callable_or_absent() {
    // §6.2.6.5 steps 17 and 20 refuse anything else, and the rows that check the refusal say
    // nothing about what is *allowed* — a check that always threw would pass them all.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {get: function () {}}); typeof Object.getOwnPropertyDescriptor(o, 'a').get"
        ),
        "function"
    );
    // `undefined` is the way a descriptor says "this half has no function", and is not the same
    // as leaving the field out — it makes the property an accessor either way.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {get: undefined}); var d = Object.getOwnPropertyDescriptor(o, 'a'); typeof d.get + '|' + typeof d.set + '|' + ('value' in d)"
        ),
        "undefined|undefined|false"
    );
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'a', {set: function () {}}); typeof Object.getOwnPropertyDescriptor(o, 'a').set"
        ),
        "function"
    );
}

#[test]
fn the_builtin_tag_is_read_from_an_internal_slot_and_not_from_the_prototype_chain() {
    // §20.1.3.6 steps 4 to 14 are a table of internal slots, and the whole point of asking a slot
    // is that a prototype cannot lie about it. Every row here has a twin below made with
    // `Object.create`, and the twins are all `[object Object]` — which is what `instanceof` would
    // get wrong and this does not.
    let tag = |source: &str| run(&format!("Object.prototype.toString.call({source})"));
    assert_eq!(tag("new Error('x')"), "[object Error]");
    assert_eq!(tag("new TypeError('x')"), "[object Error]");
    assert_eq!(tag("new AggregateError([])"), "[object Error]");
    assert_eq!(tag("/x/"), "[object RegExp]");
    assert_eq!(tag("new RegExp('x')"), "[object RegExp]");
    assert_eq!(tag("new Date()"), "[object Date]");
    assert_eq!(tag("[]"), "[object Array]");
    // …and an object merely *given* one of those prototypes has none of their slots.
    for prototype in ["Error", "TypeError", "RegExp", "Date", "Array"] {
        assert_eq!(
            tag(&format!("Object.create({prototype}.prototype)")),
            "[object Object]",
            "an object given {prototype}.prototype has no slot of its own"
        );
    }
    // An error the *engine* threw is an error like any other — there is no second kind, and
    // nothing a program can ask tells the two apart.
    assert_eq!(
        run("try { null.x; 'none' } catch (e) { Object.prototype.toString.call(e) }"),
        "[object Error]"
    );
    assert_eq!(
        run("try { undeclared_name_xyz; 'none' } catch (e) { \
             Object.prototype.toString.call(e) }"),
        "[object Error]"
    );
    // Step 15's `@@toStringTag` still wins over the table when it is a String, which is how a
    // subclass renames itself — and is ignored when it is not.
    assert_eq!(
        run("var e = new Error('x'); e[Symbol.toStringTag] = 'Mine'; \
             Object.prototype.toString.call(e)"),
        "[object Mine]"
    );
    assert_eq!(
        run("var e = new Error('x'); e[Symbol.toStringTag] = 7; \
             Object.prototype.toString.call(e)"),
        "[object Error]"
    );
}

#[test]
fn a_property_key_argument_is_converted_by_running_the_objects_own_methods() {
    // §7.1.19 `ToPropertyKey` is `ToPrimitive` with the **string** hint and then `ToString`, and
    // `ToPrimitive` of an object *calls a method*. Every function taking a key by argument runs
    // that operation, so `Object.defineProperty(o, [1, 2], …)` defines `"1,2"` — `Array.prototype
    // .join` is what produces it, and it is ordinary JavaScript running inside the conversion.
    assert_eq!(
        run("var o = {}; Object.defineProperty(o, [1, 2], {value: 7}); o['1,2']"),
        "7"
    );
    assert_eq!(
        run("var o = {}; o[[1, 2]] = 7; Object.getOwnPropertyDescriptor(o, [1, 2]).value"),
        "7"
    );
    assert_eq!(
        run("var o = {}; o[[1, 2]] = 7; Reflect.has(o, [1, 2])"),
        "true"
    );
    assert_eq!(
        run("var o = {}; o[[1, 2]] = 7; Reflect.get(o, [1, 2])"),
        "7"
    );
    assert_eq!(run("({'3,4': 1}).hasOwnProperty([3, 4])"), "true");
    assert_eq!(run("({'3,4': 1}).propertyIsEnumerable([3, 4])"), "true");
    assert_eq!(run("Object.hasOwn({'3,4': 1}, [3, 4])"), "true");
    // §B.2.2.1's key is converted the same way — it is the same abstract operation, and the only
    // reason to mention Annex B separately is that its functions were written before the operation
    // was factored out and are easy to leave behind.
    assert_eq!(
        run("var o = {}; o.__defineGetter__([5, 6], function () { return 9 }); o['5,6']"),
        "9"
    );
    assert_eq!(
        run("var o = {a: 1}; typeof o.__lookupGetter__({toString: function () { return 'a' }})"),
        "undefined"
    );
    // What the conversion *throws* reaches the caller unchanged. A key whose `toString` fails is
    // the program's error and not a TypeError about a key that could not be spelled — which is
    // exactly what a conversion that cannot run code has to say instead.
    assert_eq!(
        run(
            "try { Object.defineProperty({}, {toString: function () { throw new Error('boom') }}, {}) } \
             catch (e) { e.message }"
        ),
        "boom"
    );
}

#[test]
fn a_key_argument_reaches_symbol_to_primitive_and_may_answer_a_symbol() {
    // §7.1.19 step 3 — the Symbol check is **after** `ToPrimitive`, so an object with an
    // `@@toPrimitive` answering a Symbol is used as that Symbol rather than spelled. Doing the
    // check first would take the wrapper for a non-Symbol and reach `ToString`, which throws for
    // the Symbol it hands back.
    assert_eq!(
        run("var s = Symbol(), o = {}; o[s] = 4; \
             var w = {}; w[Symbol.toPrimitive] = function () { return s }; \
             Object.getOwnPropertyDescriptor(o, w).value"),
        "4"
    );
    // …and exactly once, which is the observable difference between running the operation and
    // running it again for a second look at the key.
    assert_eq!(
        run("var s = Symbol(), o = {}, n = 0; o[s] = 0; \
             var w = {}; w[Symbol.toPrimitive] = function () { n += 1; return s }; \
             Object.hasOwn(o, w) + '|' + n"),
        "true|1"
    );
}
