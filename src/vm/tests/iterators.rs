//! §27.1, §23.1.5 and §22.1.5 — the iterator objects and the prototype chain they share.
//!
//! Checked against V8 first. The rows that matter are the ones about *state*: an iterator's
//! position is an internal slot, so nothing in the language can move it, and it re-reads its
//! target's `length` at every step rather than counting once at the start.

use super::*;

#[test]
fn an_array_hands_out_three_iterators_and_one_of_them_is_its_own_symbol() {
    assert_eq!(run("typeof [].values"), "function");
    assert_eq!(run("typeof [].keys"), "function");
    assert_eq!(run("typeof [].entries"), "function");
    // §23.1.3.38 — `[@@iterator]` **is** `values`, the same function object rather than a second
    // one that behaves alike. A second native would fail this row and pass every other.
    assert_eq!(
        run("Array.prototype[Symbol.iterator] === Array.prototype.values"),
        "true"
    );
    assert_eq!(
        run("(function () { var i = [1, 2].values(); var r = i.next(); \
             return r.value + ',' + r.done; })()"),
        "1,false"
    );
    assert_eq!(
        run(
            "(function () { var i = [1].values(); i.next(); var r = i.next(); \
             return r.value + ',' + r.done; })()"
        ),
        "undefined,true"
    );
    assert_eq!(
        run("(function () { var i = [].values(); var r = i.next(); \
             return typeof r.value + ',' + r.done; })()"),
        "undefined,true"
    );
    assert_eq!(
        run(
            "(function () { var i = [1, 2].keys(); return i.next().value + ',' + i.next().value; })()"
        ),
        "0,1"
    );
    assert_eq!(
        run(
            "(function () { var i = ['a'].entries(); var e = i.next().value; \
             return e[0] + ':' + e[1]; })()"
        ),
        "0:a"
    );
    // §23.1.5.1 step 1 is `ToObject`, so these work on anything array-like and not only on Arrays.
    assert_eq!(
        run(
            "(function () { var i = Array.prototype.values.call({length: 2, 0: 'x', 1: 'y'}); \
             return i.next().value + i.next().value; })()"
        ),
        "xy"
    );
    assert_eq!(
        run(
            "(function () { var i = Array.prototype.values.call('ab'); return i.next().value; })()"
        ),
        "a"
    );
}

#[test]
fn an_iterator_is_itself_iterable_and_says_what_kind_it_is() {
    // §27.1.2.1 — one method on %IteratorPrototype%, and it answers the receiver. That is the
    // whole of what makes an iterator usable wherever an iterable is wanted.
    assert_eq!(
        run("(function () { var i = [1].values(); return i[Symbol.iterator]() === i; })()"),
        "true"
    );
    // Both kinds inherit from that one object, which is what "the iterator prototype" means.
    assert_eq!(
        run(
            "Object.getPrototypeOf(Object.getPrototypeOf([].values())) === \
             Object.getPrototypeOf(Object.getPrototypeOf(''[Symbol.iterator]()))"
        ),
        "true"
    );
    // …and each adds its own `next`, so replacing one leaves the other alone.
    assert_eq!(
        run(
            "(function () { var p = Object.getPrototypeOf([].values()); return typeof p.next; })()"
        ),
        "function"
    );
    assert_eq!(
        run("Object.getPrototypeOf([].values()) === Object.getPrototypeOf(''[Symbol.iterator]())"),
        "false"
    );
    // §23.1.5.2.2 and §22.1.5.2.2 — the tag is the only thing that tells them apart in a message.
    assert_eq!(
        run("Object.prototype.toString.call([].values())"),
        "[object Array Iterator]"
    );
    assert_eq!(
        run("Object.prototype.toString.call(''[Symbol.iterator]())"),
        "[object String Iterator]"
    );
    // §17 attributes on the Symbol-keyed method: writable, not enumerable, configurable.
    assert_eq!(
        run(
            "(function () { var d = Object.getOwnPropertyDescriptor(Array.prototype, Symbol.iterator); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "true,false,true"
    );
    // §22.1.3.34's method has a Symbol key and no String one — the name it was built under is
    // not a property anybody can reach.
    assert_eq!(
        run("typeof String.prototype['[Symbol.iterator]']"),
        "undefined"
    );
    // §27.1.2.1, §23.1.5.2.2 and §7.4.13 each give their property a different set of attributes,
    // and the three differ in ways nothing else would reveal: the shared `[@@iterator]` is
    // writable, the tag is not, and an iterator result's two properties are ordinary in every way.
    assert_eq!(
        run(
            "(function () { var p = Object.getPrototypeOf(Object.getPrototypeOf([].values()));              var d = Object.getOwnPropertyDescriptor(p, Symbol.iterator);              return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "true,false,true"
    );
    assert_eq!(
        run(
            "(function () { var p = Object.getPrototypeOf([].values());              var d = Object.getOwnPropertyDescriptor(p, Symbol.toStringTag);              return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "false,false,true"
    );
    assert_eq!(
        run(
            "(function () { var p = Object.getPrototypeOf([].values());              return Object.getOwnPropertyDescriptor(p, Symbol.toStringTag).value; })()"
        ),
        "Array Iterator"
    );
    assert_eq!(
        run(
            "(function () { var r = [1].values().next();              var d = Object.getOwnPropertyDescriptor(r, 'value');              return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "true,true,true"
    );
    assert_eq!(
        run("(function () { var r = [1].values().next(); return Object.keys(r).join(','); })()"),
        "value,done"
    );
    assert_eq!(run("Array.prototype.values.length"), "0");
    assert_eq!(run("[].values().next.length"), "0");
}

#[test]
fn a_string_iterates_by_code_point_and_not_by_unit() {
    assert_eq!(
        run("(function () { var i = 'ab'[Symbol.iterator](); \
             return i.next().value + i.next().value; })()"),
        "ab"
    );
    assert_eq!(
        run("(function () { var i = ''[Symbol.iterator](); return i.next().done; })()"),
        "true"
    );
    // §22.1.3.34 begins with `RequireObjectCoercible`, so these two are refused before anything
    // is converted — and everything else is `ToString`ed and walked.
    for receiver in ["null", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return String.prototype[Symbol.iterator].call({receiver}); }}                  catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    assert_eq!(
        run(
            "(function () { var i = String.prototype[Symbol.iterator].call(5);              return i.next().value; })()"
        ),
        "5"
    );
    // §22.1.5.1 — a surrogate pair is *one* step, so an astral character iterates once where
    // `.length` says two. This is the whole reason a String iterator is not an Array Iterator
    // with a String inside it.
    assert_eq!(
        run(
            "(function () { var i = '\\ud83d\\ude00'[Symbol.iterator](); var r = i.next(); \
             return r.value.length + ',' + i.next().done; })()"
        ),
        "2,true"
    );
    // …and a *lone* surrogate is one step of one unit, passed through rather than replaced.
    assert_eq!(
        run(
            "(function () { var i = '\\ud800'[Symbol.iterator](); var r = i.next(); \
             return r.value.length + ',' + r.value.charCodeAt(0); })()"
        ),
        "1,55296"
    );
}

#[test]
fn where_an_iterator_has_got_to_is_not_something_a_script_can_reach() {
    // §23.1.5.2.1 step 2 is a `RequireInternalSlot`, so `next` refuses anything that is not one
    // of its own iterators — an object with the right prototype is not one, and neither is the
    // array it came from.
    for receiver in [
        "{}",
        "[]",
        "null",
        "Object.create(Object.getPrototypeOf([].values()))",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ return [].values().next.call({receiver}); }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError"
        );
    }
    // §23.1.5.2.1 step 6 reads `length` at *every* step rather than counting once, so an array
    // that grows while being walked keeps going and one that shrinks stops early.
    assert_eq!(
        run(
            "(function () { var a = [1]; var i = a.values(); a.push(2); \
             return i.next().value + ',' + i.next().value; })()"
        ),
        "1,2"
    );
    assert_eq!(
        run(
            "(function () { var a = [1, 2]; var i = a.values(); i.next(); a.length = 0; \
             return i.next().done; })()"
        ),
        "true"
    );
    // Once done, done. An iterator that ran off the end does not start finding things again
    // because the array grew back — §23.1.5.2.1 step 4.b clears the kind, and this is that.
    assert_eq!(
        run(
            "(function () { var a = [1]; var i = a.values(); i.next(); i.next(); \
             a.push(2); return i.next().done; })()"
        ),
        "true"
    );
    assert_eq!(
        run("(function () { var i = [1].values(); i.next(); i.next(); return i.next().done; })()"),
        "true"
    );
}
