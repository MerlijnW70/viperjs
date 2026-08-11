//! §6.1.5 and §20.4 — the one primitive whose identity is itself.
//!
//! Checked against V8 first. Nearly every row here is about a Symbol *not* doing something: not
//! being equal to another Symbol, not turning into text, not appearing in a list of keys. That is
//! the type — it is useful precisely because of what it refuses.

use super::*;

#[test]
fn two_symbols_are_never_equal_however_alike_they_look() {
    // §7.2.12 — a Symbol is compared by identity and its description takes no part. This is the
    // whole type in one row: it is what makes a Symbol a key nothing else can collide with.
    assert_eq!(run("Symbol('a') === Symbol('a')"), "false");
    assert_eq!(run("Symbol() == Symbol()"), "false");
    assert_eq!(
        run("(function () { var s = Symbol('a'); return s === s; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var s = Symbol(); return s == s; })()"),
        "true"
    );
    assert_eq!(run("typeof Symbol()"), "symbol");
    // §7.2.14 — a Symbol is equal to nothing of another type, and `==` does not throw finding out.
    for other in ["1", "'a'", "null", "undefined", "true"] {
        assert_eq!(run(&format!("Symbol() == {other}")), "false");
    }
    // …but a Symbol *wrapper* is, because steps 13 and 14 name Symbol among the types an object
    // is converted for. `Object(s).valueOf()` is `s`, so the two ends meet.
    assert_eq!(
        run("(function () { var s = Symbol(); return Object(s) == s; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var s = Symbol(); return Object(s) === s; })()"),
        "false"
    );
}

#[test]
fn a_symbol_will_not_become_text_by_accident() {
    // §7.1.17 step 2 and §7.1.4 step 3 — both conversions **throw**. That is the point: a Symbol
    // in a template or an addition is always a mistake, and the specification says so rather than
    // producing something unhelpful in the middle of a string.
    for attempt in ["'' + Symbol()", "Symbol() + 1", "+Symbol()", "Symbol() * 2"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return {attempt}; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // §22.1.1.1 step 2 is the one door out, and `Symbol.prototype.toString` is the other. Both
    // have to be asked for by name.
    assert_eq!(run("String(Symbol('a'))"), "Symbol(a)");
    assert_eq!(run("Symbol('a').toString()"), "Symbol(a)");
    // A Symbol with no description and one described as the empty String print alike and are not
    // the same thing — which is what `description` keeps and `toString` throws away.
    assert_eq!(run("Symbol().toString()"), "Symbol()");
    assert_eq!(run("Symbol('').toString()"), "Symbol()");
    assert_eq!(run("typeof Symbol().description"), "undefined");
    assert_eq!(run("Symbol('').description === ''"), "true");
    assert_eq!(run("Symbol('a').description"), "a");
    // §7.1.2 — always true, description or not. There is no empty Symbol for a rule to be about.
    assert_eq!(run("Boolean(Symbol())"), "true");
    assert_eq!(run("!Symbol()"), "false");
}

#[test]
fn a_symbol_key_is_a_property_that_no_listing_of_names_shows() {
    assert_eq!(
        run("(function () { var o = {}, s = Symbol('k'); o[s] = 1; return o[s]; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var o = {}, s = Symbol('k'); o[s] = 1; return s in o; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var o = {}, s = Symbol('k'); o[s] = 1; return delete o[s]; })()"),
        "true"
    );
    // Not hidden — `getOwnPropertySymbols` lists it — but not among the *names*, and not walked
    // by `for`-`in` or `Object.keys`. Every operation that lists keys picks one of the two lists.
    let hidden = "(function () { var o = {}, s = Symbol('k'); o[s] = 1; return ";
    assert_eq!(run(&format!("{hidden}Object.keys(o).length; }})()")), "0");
    assert_eq!(
        run(&format!(
            "{hidden}Object.getOwnPropertyNames(o).length; }})()"
        )),
        "0"
    );
    assert_eq!(
        run(&format!(
            "{hidden}Object.getOwnPropertySymbols(o).length; }})()"
        )),
        "1"
    );
    assert_eq!(
        run(&format!(
            "{hidden}Object.getOwnPropertySymbols(o)[0] === s; }})()"
        )),
        "true"
    );
    assert_eq!(
        run(
            "(function () { var o = {}, s = Symbol('k'); o[s] = 1; var r = ''; \
             for (var k in o) { r += k; } return r; })()"
        ),
        ""
    );
    // …and `getOwnPropertyDescriptor` answers about it, because that one is asked about a key
    // rather than for a list. `ToPropertyKey` of a Symbol is the Symbol, never its text.
    assert_eq!(
        run("(function () { var s = Symbol('k'); var o = {}; o[s] = 1; \
             return Object.getOwnPropertyDescriptor(o, s).value; })()"),
        "1"
    );
    // §10.1.11 step 4 — Symbol keys come after every String key, whatever order they were added.
    assert_eq!(
        run(
            "(function () { var o = {a: 1}; var s = Symbol('s'); o[s] = 2; o.b = 3; o[1] = 4; \
             return Object.getOwnPropertyNames(o).join(','); })()"
        ),
        "1,a,b"
    );
    // §20.1.2.1 copies a Symbol-keyed property, because `assign` walks own *enumerable* keys and
    // that is a question about the attribute rather than about the kind of key.
    assert_eq!(
        run("(function () { var o = {}; var s = Symbol(); o[s] = 1; \
             return Object.assign({}, o)[s]; })()"),
        "1"
    );
}

#[test]
fn the_registry_is_the_one_way_to_ask_for_a_symbol_that_already_exists() {
    assert_eq!(run("Symbol.for('k') === Symbol.for('k')"), "true");
    assert_eq!(run("Symbol.for('') === Symbol.for('')"), "true");
    assert_eq!(run("Symbol.for('a') === Symbol.for('b')"), "false");
    assert_eq!(run("Symbol.for('k') === Symbol('k')"), "false");
    assert_eq!(run("Symbol.keyFor(Symbol.for('k'))"), "k");
    // §20.4.2.7 — `undefined` for a Symbol that was not registered, which is the only way to tell
    // a registered Symbol from an ordinary one with the same description.
    assert_eq!(run("typeof Symbol.keyFor(Symbol('k'))"), "undefined");
    // The key is `ToString`ed, so `Symbol.for(1)` and `Symbol.for("1")` are one Symbol.
    assert_eq!(run("Symbol.for(1) === Symbol.for('1')"), "true");
    assert_eq!(run("Symbol.keyFor(Symbol.for('1'))"), "1");
}

#[test]
fn the_well_known_symbols_exist_and_are_the_same_ones_every_time() {
    for name in [
        "asyncIterator",
        "hasInstance",
        "isConcatSpreadable",
        "iterator",
        "match",
        "matchAll",
        "replace",
        "search",
        "species",
        "split",
        "toPrimitive",
        "toStringTag",
        "unscopables",
    ] {
        assert_eq!(run(&format!("typeof Symbol.{name}")), "symbol");
        assert_eq!(run(&format!("Symbol.{name} === Symbol.{name}")), "true");
    }
    // §6.1.5.1 gives each a description, which is how one shows up in a message.
    assert_eq!(run("String(Symbol.iterator)"), "Symbol(Symbol.iterator)");
    // §20.4.2 — not writable and not configurable, so a script cannot move what the engine will
    // reach for when `for`-`of` arrives.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol, 'iterator').writable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol, 'iterator').configurable"),
        "false"
    );
    // …and none of them is in the registry: a well-known Symbol is not one `Symbol.for` made.
    assert_eq!(run("typeof Symbol.keyFor(Symbol.iterator)"), "undefined");
}

#[test]
fn to_string_tag_renames_what_object_prototype_to_string_reports() {
    // §20.1.3.6 step 15 — the first well-known Symbol ViperJS acts on, and the supported way for a
    // script to name its own type here.
    assert_eq!(
        run(
            "(function () { var o = {}; o[Symbol.toStringTag] = 'Mine'; \
             return Object.prototype.toString.call(o); })()"
        ),
        "[object Mine]"
    );
    // A tag that is not a String is ignored, and the table's answer stands.
    assert_eq!(
        run("(function () { var o = {}; o[Symbol.toStringTag] = 5; \
             return Object.prototype.toString.call(o); })()"),
        "[object Object]"
    );
    // A get and not an own-property read, so an inherited tag counts — which is how one property
    // on `Symbol.prototype` tags every Symbol without anything being put on each of them.
    assert_eq!(
        run(
            "(function () { function F() {} F.prototype[Symbol.toStringTag] = 'F'; \
             return Object.prototype.toString.call(new F()); })()"
        ),
        "[object F]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(Symbol())"),
        "[object Symbol]"
    );
    // …which is a property on `Symbol.prototype` and not a row in the table, so it has the three
    // attributes §20.4.3.5 gives it — and being *configurable* is the one that shows: deleting it
    // makes a Symbol tag as an ordinary object again.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, Symbol.toStringTag).value"),
        "Symbol"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, Symbol.toStringTag).writable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, Symbol.toStringTag).enumerable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, Symbol.toStringTag).configurable"),
        "true"
    );
    assert_eq!(
        run(
            "(function () { delete Symbol.prototype[Symbol.toStringTag];              return Object.prototype.toString.call(Symbol()); })()"
        ),
        "[object Object]"
    );
    // …and the rows that were already there still answer from the table.
    assert_eq!(run("Object.prototype.toString.call({})"), "[object Object]");
    assert_eq!(run("Object.prototype.toString.call([])"), "[object Array]");
    assert_eq!(run("Object.prototype.toString.call(null)"), "[object Null]");
}

#[test]
fn symbol_is_the_one_constructor_that_refuses_itself() {
    // §20.4.1 step 1. A Symbol wrapper would be an object, and an object is equal to nothing but
    // itself — so `new Symbol("a")` would look like a Symbol and silently fail to be usable as
    // the key it was made to be. The specification refuses rather than hand that back.
    assert_eq!(
        run("(function () { try { new Symbol(); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // …and it refuses **as a constructor**, which is the distinction the row above cannot see.
    // §20.4.1 says `Symbol` "may be used as the value of an `extends` clause of a class definition
    // but a `super` call to it will cause an exception" — so it has a `[[Construct]]` that throws,
    // not an absent one. Answering the same TypeError by having none at all moved the refusal to
    // the class definition, where the clause allows the definition and refuses the `super` call.
    assert_eq!(
        run(
            "(function () { try { Reflect.construct(function () {}, [], Symbol); return true; } \
             catch (e) { return false; } })()"
        ),
        "true"
    );
    assert_eq!(
        run("(function () { class A extends Symbol {} return typeof A; })()"),
        "function"
    );
    assert_eq!(
        run("(function () { class A extends Symbol {} \
             try { new A(); return 'ok'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // The check is step 1 and therefore before step 2's coercion: a description that would run
    // JavaScript never gets to.
    assert_eq!(
        run("(function () { var ran = false; \
             try { new Symbol({ toString: function () { ran = true; return 'x'; } }); } \
             catch (e) {} return ran; })()"),
        "false"
    );
    // §20.4.3.2 — `description` is an accessor, not enumerable and *configurable*: a script may
    // delete it, which is the one of the three attributes that surprises.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, 'description').enumerable"),
        "false"
    );
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Symbol.prototype, 'description').configurable"),
        "true"
    );
    assert_eq!(
        run("typeof Object.getOwnPropertyDescriptor(Symbol.prototype, 'description').get"),
        "function"
    );
    // The wrapper still exists, because `ToObject` has to answer something. It just cannot be
    // asked for directly.
    assert_eq!(run("typeof Object(Symbol())"), "object");
    assert_eq!(run("Object(Symbol('a')).description"), "a");
    assert_eq!(
        run("(function () { var s = Symbol(); \
             return Symbol.prototype.valueOf.call(Object(s)) === s; })()"),
        "true"
    );
    // `thisSymbolValue` refuses anything else, which is what keeps these two methods honest.
    assert_eq!(
        run(
            "(function () { try { return Symbol.prototype.toString.call('x'); } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { try { return Symbol.prototype.description; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}
