//! Objects from source — literals, properties, and the attributes they get.
//!
//! Every row runs *source* rather than asserting on a chunk: an instruction sequence is an
//! implementation detail and a value is not.

use super::*;

fn key_of(heap: &mut Heap, name: &str) -> Value {
    Value::String(heap.new_string(name.encode_utf16().collect()))
}

#[test]
fn an_object_literal_makes_properties_that_can_be_read_back() {
    assert_eq!(run("var o = {a: 1}; o.a;"), "1");
    assert_eq!(run("var o = {a: 1, b: 2}; o.a + o.b;"), "3");
    assert_eq!(run("var o = {}; o.missing;"), "undefined");
    // Every spelling of a key names the same property, because every one of them is the
    // String `ToString` writes: a quoted name, a bare name, a number, a computed expression.
    assert_eq!(run("var o = {'a': 1}; o.a;"), "1");
    assert_eq!(run("var o = {1: 'x'}; o[1];"), "x");
    assert_eq!(run("var o = {1: 'x'}; o['1'];"), "x");
    assert_eq!(run("var o = {1.0: 'x'}; o[1];"), "x");
    assert_eq!(run("var k = 'a'; var o = {[k]: 1}; o.a;"), "1");
    assert_eq!(run("var o = {1e21: 'x'}; o['1e+21'];"), "x");
    // A later property wins, and it is one property rather than two.
    assert_eq!(run("var o = {a: 1, a: 2}; o.a;"), "2");
}

#[test]
fn a_property_is_written_read_and_deleted_through_the_prototype_chain() {
    assert_eq!(run("var o = {}; o.a = 1; o.a;"), "1");
    assert_eq!(run("var o = {}; o['a'] = 1; o.a;"), "1");
    assert_eq!(run("var o = {a: 1}; o.a = 2; o.a;"), "2");
    // Assignment is an expression whose value is the value assigned.
    assert_eq!(run("var o = {}; o.a = 5;"), "5");
    assert_eq!(run("var o = {}; var x = o.a = 5; x;"), "5");
    // Compound assignment reads and writes the same property.
    assert_eq!(run("var o = {a: 1}; o.a += 2; o.a;"), "3");
    assert_eq!(run("var o = {a: 'x'}; o.a += 'y'; o.a;"), "xy");
    assert_eq!(run("var o = {a: 8}; o['a'] /= 2; o.a;"), "4");
    // `delete` answers whether the property is gone, which is true even when there was none.
    assert_eq!(run("var o = {a: 1}; delete o.a;"), "true");
    assert_eq!(run("var o = {a: 1}; delete o.a; o.a;"), "undefined");
    assert_eq!(run("var o = {}; delete o.nothing;"), "true");
    // …and `in` asks about the chain rather than about own properties.
    assert_eq!(run("var o = {a: 1}; 'a' in o;"), "true");
    assert_eq!(run("var o = {a: 1}; 'b' in o;"), "false");
    assert_eq!(run("var o = {a: 1}; delete o.a; 'a' in o;"), "false");
}

#[test]
fn a_key_is_evaluated_once_even_when_the_property_is_read_and_written() {
    // §13.15.2 — `o[k] += 1` evaluates the key once. With no function calls yet the only way
    // to see that is a key expression with a side effect, and assignment is one: if the key
    // were evaluated twice, `i` would end at 2 and the property written would be `o[1]`.
    assert_eq!(run("var o = {}; var i = 0; o[i = i + 1] = 5; i;"), "1");
    assert_eq!(
        run("var o = {0: 10}; var i = 0; o[i = i + 1] = 5; o[1];"),
        "5"
    );
    assert_eq!(
        run("var o = {1: 10}; var i = 0; o[i = i + 1] += 5; o[1];"),
        "15"
    );
    assert_eq!(
        run("var o = {1: 10}; var i = 0; o[i = i + 1] += 5; i;"),
        "1"
    );
}

#[test]
fn reading_a_property_of_something_that_is_not_an_object_is_a_type_error() {
    // Right for `null` and `undefined`, and temporary for the rest: §7.3.2 wraps a primitive
    // in its own object first, and there is no `String.prototype` to wrap one in yet.
    assert_eq!(run("try { null.a; } catch (e) { e.name; }"), "TypeError");
    assert_eq!(
        run("try { (void 0).a; } catch (e) { e.name; }"),
        "TypeError"
    );
    assert_eq!(
        run("try { null.a = 1; } catch (e) { e.name; }"),
        "TypeError"
    );
    assert_eq!(run("try { 1 in 2; } catch (e) { e.name; }"), "TypeError");
    // The error is an object with a message of its own and a name from its prototype, which
    // is the seam between the value layer and the realm made visible.
    assert_eq!(run("try { null.a; } catch (e) { typeof e; }"), "object");
    // An ordinary object *does* convert now — §7.1.1 asks it, and `Object.prototype.toString`
    // answers. The failure needs an object with nothing to ask, which is what
    // `Object.create(null)` is: no prototype, so no `valueOf` and no `toString`.
    assert_eq!(run("({}) + 1"), "[object Object]1");
    assert_eq!(
        run("try { Object.create(null) + 1; } catch (e) { e.name + ': ' + e.message; }"),
        "TypeError: cannot convert an object to a primitive value"
    );
}

#[test]
fn an_object_inherits_from_its_prototype_and_shadows_what_it_writes() {
    // Everything a literal makes inherits from `Object.prototype`, so a property put there is
    // visible from every object — and writing the same name makes an own property that hides
    // it rather than changing it.
    assert_eq!(run("var o = {}; 'nothing' in o;"), "false");
    assert_eq!(run("var o = {a: 1}; var p = {a: 2}; o.a + p.a;"), "3");
    // A property that does not exist reads as `undefined` rather than throwing, which is the
    // difference between a property and a name.
    assert_eq!(run("var o = {}; typeof o.missing;"), "undefined");
    assert_eq!(run("var o = {a: void 0}; 'a' in o;"), "true");
}

#[test]
fn an_object_literal_makes_the_same_ordinary_properties_assignment_does() {
    // §13.2.5's `CreateDataPropertyOrThrow` gives all three attributes, and they are *not*
    // §6.1.7.1's defaults: a property a program writes is writable, enumerable and
    // configurable, where one `Object.defineProperty` makes is none of those.
    //
    // No source can see this yet — that needs `for...in` or `getOwnPropertyDescriptor` — but
    // the object itself is the script's completion value, so the heap can be asked directly.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let script = parse_script("({a: 1})").expect("parses"); // the test is about the object
    let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
    let outcome = vm.run(&chunk, &mut heap).expect("well formed"); // same
    let Outcome::Value(Value::Object(object)) = outcome else {
        panic!("an object literal evaluates to an object")
    };
    let property = own(&mut heap, object, "a").expect("just defined"); // same
    assert!(property.enumerable);
    assert!(property.configurable);
    assert!(matches!(
        property.kind,
        PropertyKind::Data {
            value: Value::Number(value),
            writable: true
        } if value == 1.0
    ));
}

#[test]
fn assignment_and_a_literal_both_make_an_ordinary_property() {
    // §10.1.9's `CreateDataProperty` and §13.2.5's define give the same three attributes, and
    // they are *not* §6.1.7.1's defaults: a property a program makes is writable, enumerable
    // and configurable, where one `Object.defineProperty` makes is none of those.
    //
    // Nothing in the language can see this yet — that needs `for...in` and
    // `getOwnPropertyDescriptor` — so it is checked where it is decided.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let object = heap.new_object(None);
    let key = key_of(&mut heap, "a");
    let base = Value::Object(object);
    assert!(matches!(
        vm.set_property(base, key, Value::Number(1.0), &mut heap),
        Ok(Value::Boolean(true))
    ));
    let property = own(&mut heap, object, "a").expect("just assigned"); // the test is about it
    assert!(property.enumerable);
    assert!(property.configurable);
    assert!(matches!(
        property.kind,
        PropertyKind::Data { writable: true, .. }
    ));
}

#[test]
fn assignment_keeps_the_attributes_an_existing_own_property_had() {
    // §10.1.9.2 — writing to an own property changes its value and nothing else. A property
    // that was hidden stays hidden, which is why assigning to a built-in does not suddenly
    // make it turn up in `for...in`.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let object = heap.new_object(None);
    let hidden = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
    let descriptor = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(true),
        enumerable: Some(false),
        configurable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, hidden, &descriptor));

    let key = key_of(&mut heap, "a");
    let base = Value::Object(object);
    assert!(matches!(
        vm.set_property(base, key, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(true))
    ));
    let property = own(&mut heap, object, "a").expect("still there"); // same
    assert!(!property.enumerable);
    assert!(!property.configurable);
    assert!(matches!(
        property.kind,
        PropertyKind::Data { value: Value::Number(value), .. } if value == 2.0
    ));
}

#[test]
fn a_write_is_refused_by_what_it_would_have_to_go_through() {
    // The three ways §10.1.9 answers `false`, and none of them throws: a plain assignment in
    // sloppy code swallows the answer, which is why `o.frozen = 1` is silent.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let prototype = heap.new_object(None);
    let object = heap.new_object(Some(prototype));
    let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());

    // A non-writable *inherited* data property refuses the write on the receiver too.
    let frozen = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(false),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(prototype, name, &frozen));
    let key = key_of(&mut heap, "a");
    let base = Value::Object(object);
    assert!(matches!(
        vm.set_property(base, key, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(false))
    ));
    assert!(own(&mut heap, object, "a").is_none());

    // An accessor with no setter refuses as well…
    let setterless = PropertyKey::from_units(&mut heap, &"b".encode_utf16().collect::<Vec<_>>());
    let accessor = PropertyDescriptor {
        getter: Some(Value::Undefined),
        setter: Some(Value::Undefined),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(prototype, setterless, &accessor));
    let key = key_of(&mut heap, "b");
    assert!(matches!(
        vm.set_property(base, key, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(false))
    ));
    // …and one whose setter is not callable is a TypeError rather than a refusal, because
    // §10.1.9.2 calls it and calling a non-function throws.
    let uncallable = PropertyKey::from_units(&mut heap, &"c".encode_utf16().collect::<Vec<_>>());
    let accessor = PropertyDescriptor {
        setter: Some(Value::Number(0.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(prototype, uncallable, &accessor));
    let key = key_of(&mut heap, "c");
    assert!(
        vm.set_property(base, key, Value::Number(2.0), &mut heap)
            .is_err()
    );
}

#[test]
fn a_writable_inherited_property_is_shadowed_rather_than_changed() {
    // §10.1.9.2 again, and the case that makes prototypes useful: writing a name the
    // prototype has puts an *own* property on the receiver and leaves the prototype's alone.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let prototype = heap.new_object(None);
    let object = heap.new_object(Some(prototype));
    let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
    let inherited = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(true),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(prototype, name, &inherited));

    let key = key_of(&mut heap, "a");
    let base = Value::Object(object);
    assert!(matches!(
        vm.set_property(base, key, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(true))
    ));
    assert!(matches!(
        own(&mut heap, object, "a").expect("shadowed").kind, // the test is about it
        PropertyKind::Data { value: Value::Number(value), .. } if value == 2.0
    ));
    assert!(matches!(
        own(&mut heap, prototype, "a").expect("untouched").kind, // same
        PropertyKind::Data { value: Value::Number(value), .. } if value == 1.0
    ));
}

#[test]
fn an_accessor_answers_undefined_without_a_getter_and_throws_with_one() {
    // §10.1.8.1 steps 5 and 6. Nothing is callable yet, so the second is a TypeError for
    // whatever was put there — and both are reachable by defining the property directly,
    // which is why neither is a branch nothing can take.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let object = heap.new_object(None);
    let getterless = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
    let accessor = PropertyDescriptor {
        getter: Some(Value::Undefined),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, getterless, &accessor));
    let base = Value::Object(object);
    let key = key_of(&mut heap, "a");
    assert!(matches!(
        vm.get_property(base, key, &mut heap),
        Ok(Value::Undefined)
    ));

    let uncallable = PropertyKey::from_units(&mut heap, &"b".encode_utf16().collect::<Vec<_>>());
    let accessor = PropertyDescriptor {
        getter: Some(Value::Number(0.0)),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, uncallable, &accessor));
    let key = key_of(&mut heap, "b");
    assert!(vm.get_property(base, key, &mut heap).is_err());
}

#[test]
fn delete_reaches_only_own_properties_and_a_non_reference_is_always_gone() {
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let prototype = heap.new_object(None);
    let object = heap.new_object(Some(prototype));
    let name = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
    let descriptor = PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        configurable: Some(true),
        ..PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(prototype, name, &descriptor));
    // Deleting an inherited property answers `true` and leaves it exactly where it was, which
    // is the trap: `delete o.inherited` looks like it worked and `o.inherited` still reads.
    let base = Value::Object(object);
    let key = key_of(&mut heap, "a");
    assert!(matches!(
        vm.delete_property(base, key, &mut heap),
        Ok(Value::Boolean(true))
    ));
    assert!(own(&mut heap, prototype, "a").is_some());
    assert!(matches!(
        vm.get_property(base, key, &mut heap),
        Ok(Value::Number(value)) if value == 1.0
    ));
    // …and deleting something that is not a property reference at all is `true` too, which is
    // why `delete 1` is legal outside strict mode.
    assert_eq!(run("delete (1 + 1);"), "true");
    assert_eq!(run("var n = 0; delete (n = 1); n;"), "1");
}

#[test]
fn deleting_a_name_answers_for_the_binding_and_only_a_global_property_ever_goes() {
    // §13.5.1.2 step 5 — `delete x` is not `delete o.x` with the base left out. The name resolves to
    // an environment record first, and *which* record it lands in decides the answer before anything
    // is looked at: a declarative binding is non-deletable by §9.1.1.1.5, and a property of the
    // global object is deletable exactly when it is configurable.
    //
    // A `var` in a function and a `var` at the top level of a script both answer `false`, and that
    // agreement is a coincidence of two different rules — the first is a binding that cannot be
    // deleted, the second is a global property §9.1.1.4.5 makes non-configurable. Both rows are
    // here because an implementation that gets one right by the other's reasoning still answers
    // wrongly for `delete Math`.
    assert_eq!(
        run("function f() { var a = 1; return delete a; } f()"),
        "false"
    );
    assert_eq!(
        run("function f() { var a = 1; delete a; return a; } f()"),
        "1"
    );
    assert_eq!(run("function f(p) { return delete p; } f(1)"), "false");
    assert_eq!(
        run("function f() { return delete arguments; } f()"),
        "false"
    );
    assert_eq!(run("var y = 1; delete y"), "false");
    assert_eq!(run("var y = 1; delete y; y"), "1");
    assert_eq!(run("function g() {} delete g"), "false");
    assert_eq!(run("let l = 1; delete l"), "false");
    assert_eq!(run("const k = 1; delete k"), "false");
    // A configurable global really goes, and this is the only shape of `delete x` that changes
    // anything at all. `Math` is the row that matters: §19's built-ins are `{ writable: true,
    // enumerable: false, configurable: true }`, so deleting one is legal and permanent.
    assert_eq!(run("delete Math"), "true");
    assert_eq!(run("delete Math; typeof Math"), "undefined");
    assert_eq!(
        run("this.z = 1; var r = delete z; r + ',' + (typeof z)"),
        "true,undefined"
    );
    // …and `undefined` is the built-in that is *not* configurable, so it stays.
    assert_eq!(run("delete undefined"), "false");
    // §13.5.1.2 step 3 — a name that resolves nowhere answers `true`, and so does deleting the same
    // global twice. §10.1.10.1 step 2 gives the second one for free: there is nothing there to refuse.
    assert_eq!(run("delete notdeclared"), "true");
    assert_eq!(run("this.z = 1; delete z; delete z"), "true");
    // Own only, exactly as for a property reference — `toString` resolves as a bare name because
    // §9.1.1.2.1 walks the prototype chain, and `[[Delete]]` does not. So the answer is `true` and
    // `Object.prototype.toString` is untouched, which is the same trap as the inherited property
    // above wearing different syntax.
    assert_eq!(run("delete toString; typeof toString"), "function");
    // Strict code cannot say any of this at all: §13.5.1.1 makes `delete` of a name an early error
    // whatever the name turns out to be, which is why nothing above needs a strict counterpart and
    // why the instruction has no strict path. The refusal itself is the parser's, and pinned there.
}

#[test]
fn optional_chaining_gives_up_on_the_whole_chain_and_stops_at_the_parenthesis() {
    // §13.3.9 — the short circuit ends at the **chain**, not at the link, and that boundary is the
    // whole of what is hard about it. `a?.b.c` gives up on all of it when `a` is nullish; `(a?.b).c`
    // gives up only on the part inside the parentheses and then reads `.c` of `undefined`, which
    // throws. An implementation that short-circuited per link would answer `undefined` for both.
    assert_eq!(
        run("(function () { var a = null; return String(a?.b.c.d); })()"),
        "undefined"
    );
    assert_eq!(
        run("(function () { var a = { b: { c: 5 } }; return a?.b.c; })()"),
        "5"
    );
    assert_eq!(
        run("(function () { var a = { b: { c: 1 } }; return String((a?.b).c); })()"),
        "1"
    );
    assert_eq!(
        run(
            "(function () { var a = null;              try { return String((a?.b).c); } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // Nothing after the link is *evaluated* either, which is the observable half: a computed key
    // with a side effect does not run.
    assert_eq!(
        run("(function () { var n = 0; var o = null; String(o?.[n++]); return n; })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var n = 0; var o = null; String(o?.m(n++)); return n; })()"),
        "0"
    );
    // `delete` through a chain short-circuits to `true`, which is §13.5.1.2's answer for a reference
    // that is not a property reference at all.
    assert_eq!(
        run("(function () { var o = null; return delete o?.a; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { var o = { a: 1 }; return delete o?.a; })()"),
        "true"
    );
}

#[test]
fn a_script_var_gets_the_three_attributes_9_1_1_4_17_gives_it() {
    // §9.1.1.4.17 `CreateGlobalVarBinding`: writable and enumerable, and **not** configurable.
    // The last is what `delete` observes; the other two have nothing in the language to observe
    // them yet — `for...in` and `Object.keys` are both still to come — so they are read off the
    // object directly rather than left as a claim in a comment nobody checks.
    let mut heap = Heap::new();
    let script = parse_script("var declared = 1; function fn() {}").expect("it parses"); // the test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("it compiles"); // same
    let mut vm = Vm::new(&mut heap);
    vm.run(&chunk, &mut heap).expect("it runs"); // same
    let global = vm.realm.global();

    for name in ["declared", "fn"] {
        let property = own(&mut heap, global, name).unwrap_or_else(|| panic!("{name} is a global")); // the test is about it
        let PropertyKind::Data { writable, .. } = property.kind else {
            panic!("{name} is a data property"); // same
        };
        assert!(writable, "{name} is writable");
        assert!(property.enumerable, "{name} is enumerable");
        assert!(!property.configurable, "{name} is not configurable");
    }

    // An ordinary assignment gives the three §6.1.7.1 defaults instead, and the difference
    // between the two is the whole reason a `var` cannot be deleted.
    let assigned = own(&mut heap, global, "globalThis").expect("globalThis is a global"); // same
    assert!(!assigned.enumerable, "a built-in is not enumerable");
    assert!(assigned.configurable, "globalThis is configurable");

    // §19.1.2–4 give `undefined`, `NaN` and `Infinity` all three attributes off. Two of them are
    // observable from source — a write is ignored and a delete answers `false` — and the third
    // is not, so it is read here rather than assumed. §17 is why: a built-in never shows up in
    // an enumeration, and nothing enumerates yet.
    for name in ["undefined", "NaN", "Infinity"] {
        let property = own(&mut heap, global, name).unwrap_or_else(|| panic!("{name} is a global")); // the test is about it
        let PropertyKind::Data { writable, .. } = property.kind else {
            panic!("{name} is a data property"); // same
        };
        assert!(!writable, "{name} is not writable");
        assert!(!property.enumerable, "{name} is not enumerable");
        assert!(!property.configurable, "{name} is not configurable");
    }
}

#[test]
fn set_answers_whether_the_write_was_allowed_even_though_nothing_reads_it_yet() {
    // §10.1.9 answers a Boolean, and sloppy code throws it away — so today nothing in the
    // language can see the difference between a write that worked and one that was refused.
    // Strict mode is what turns a `false` into a TypeError, and until it exists this is the only
    // place the answer is observable at all. Asked here rather than left as a claim in a comment.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let object = heap.new_object(None);
    let key = PropertyKey::from_units(&mut heap, &"a".encode_utf16().collect::<Vec<_>>());
    let descriptor = crate::heap::PropertyDescriptor {
        value: Some(Value::Number(1.0)),
        writable: Some(false),
        enumerable: Some(true),
        configurable: Some(false),
        ..crate::heap::PropertyDescriptor::EMPTY
    };
    assert!(heap.define_own_property(object, key, &descriptor));

    let base = Value::Object(object);
    let name = key_of(&mut heap, "a");
    // A non-writable property refuses the write and says so.
    assert!(matches!(
        vm.set_property(base, name, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(false))
    ));
    // …and a writable one allows it and says that.
    let other = key_of(&mut heap, "b");
    assert!(matches!(
        vm.set_property(base, other, Value::Number(2.0), &mut heap),
        Ok(Value::Boolean(true))
    ));
    // A *new* property on an object that is not extensible is the other way a write is refused,
    // and it takes a different path: §10.1.9.2 refuses the first one before a define is even
    // attempted, where this one is the define itself answering.
    let sealed = heap.new_object(None);
    if let Some(found) = heap.object_mut(sealed) {
        found.prevent_extensions();
    }
    let fresh = key_of(&mut heap, "c");
    assert!(matches!(
        vm.set_property(Value::Object(sealed), fresh, Value::Number(1.0), &mut heap),
        Ok(Value::Boolean(false))
    ));
    // §10.1.9.2 step 5 — a setter that ran answers `true`, and an accessor with no setter
    // answers `false`. Sloppy code sees neither, and strict mode will turn the second into a
    // TypeError, so this is the only place the pair can be told apart today.
    let accessors = heap.new_object(None);
    let with_setter = PropertyKey::from_units(&mut heap, &"w".encode_utf16().collect::<Vec<_>>());
    let read_only = PropertyKey::from_units(&mut heap, &"r".encode_utf16().collect::<Vec<_>>());
    let setter = {
        let script = parse_script("(function (v) {})").expect("parses"); // the test needs a function
        let chunk = compile_script(&script, &mut heap).expect("compiles"); // same
        let outcome = Vm::new(&mut heap).run(&chunk, &mut heap).expect("runs"); // same
        let Outcome::Value(function) = outcome else {
            panic!("a function expression evaluates to a function") // same
        };
        function
    };
    for (key, half) in [(with_setter, Some(setter)), (read_only, None)] {
        let descriptor = crate::heap::PropertyDescriptor {
            getter: Some(Value::Undefined),
            setter: Some(half.unwrap_or(Value::Undefined)),
            enumerable: Some(true),
            configurable: Some(true),
            ..crate::heap::PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(accessors, key, &descriptor));
    }
    let holder = Value::Object(accessors);
    let writable = key_of(&mut heap, "w");
    assert!(matches!(
        vm.set_property(holder, writable, Value::Number(1.0), &mut heap),
        Ok(Value::Boolean(true))
    ));
    let unwritable = key_of(&mut heap, "r");
    assert!(matches!(
        vm.set_property(holder, unwritable, Value::Number(1.0), &mut heap),
        Ok(Value::Boolean(false))
    ));

    // The value the refusal left behind is the one that was there, which is the part sloppy code
    // *can* see.
    assert_eq!(
        run("var o = {}; Object.defineProperty(o, 'a', {value: 1}); o.a = 2; o.a"),
        "1"
    );
}

#[test]
fn an_object_with_many_properties_answers_the_same_as_one_with_few() {
    // An object stops scanning its properties and starts indexing them once there are enough of
    // them (see `heap::object`), and §10.1.11's answers must not notice. Every row here is over
    // that threshold and asks something the index could get wrong while the scan got it right.
    let many = "var o = {}; var i = 0; while (i < 30) { o['k' + i] = i; i = i + 1; } ";

    // Creation order, all thirty of them, with nothing lost and nothing repeated.
    assert_eq!(
        run(&format!("{many} Object.getOwnPropertyNames(o).length")),
        "30"
    );
    assert_eq!(
        run(&format!(
            "{many} Object.getOwnPropertyNames(o).slice(0, 4).join(',')"
        )),
        "k0,k1,k2,k3"
    );
    assert_eq!(run(&format!("{many} o.k0 + ',' + o.k29")), "0,29");

    // Writing to a key that is already there keeps its place — §10.1.11 is *creation* order, so a
    // second write must not move it to the end.
    assert_eq!(
        run(&format!(
            "{many} o.k1 = 'again'; Object.getOwnPropertyNames(o).slice(0, 3).join(',') + '|' + o.k1"
        )),
        "k0,k1,k2|again"
    );

    // A delete shifts every property after it. An index that recorded the old positions and was
    // not rebuilt would answer a *neighbouring* property here rather than none, which reads as a
    // plausible value rather than as a bug.
    let after_delete = format!("{many} delete o.k10; ");
    assert_eq!(run(&format!("{after_delete} typeof o.k10")), "undefined");
    assert_eq!(
        run(&format!("{after_delete} o.k9 + ',' + o.k11 + ',' + o.k29")),
        "9,11,29"
    );
    assert_eq!(
        run(&format!(
            "{after_delete} Object.getOwnPropertyNames(o).length"
        )),
        "29"
    );
    // …and the object still works afterwards: a key added after a delete goes on the end, and a
    // key deleted twice is still gone.
    assert_eq!(
        run(&format!(
            "{after_delete} o.later = 1; delete o.k10; o.later + ',' + o.k11"
        )),
        "1,11"
    );

    // §10.1.11's two-part order survives the crossing too: indices ascending first, then strings
    // in creation order, however many of each there are.
    let mixed = "var o = {}; o.z = 1; var i = 20; while (i > 0) { o[i] = i; i = i - 1; } o.a = 2; ";
    assert_eq!(
        run(&format!(
            "{mixed} Object.getOwnPropertyNames(o).slice(0, 3).join(',')"
        )),
        "1,2,3"
    );
    assert_eq!(
        run(&format!(
            "{mixed} Object.getOwnPropertyNames(o).slice(-2).join(',')"
        )),
        "z,a"
    );
}

#[test]
fn a_spread_in_an_object_literal_copies_own_enumerable_properties() {
    assert_eq!(
        run("(function () { var o = {a: 1, b: 2}; var c = {...o}; return c.a + ',' + c.b; })()"),
        "1,2"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; var c = {...o, b: 2}; return c.a + ',' + c.b; })()"),
        "1,2"
    );
    // Later wins, whichever side it is written on — a spread is not special in that.
    assert_eq!(
        run("(function () { var o = {a: 1}; return {a: 9, ...o}.a; })()"),
        "1"
    );
    assert_eq!(
        run("(function () { var o = {a: 1}; return {...o, a: 9}.a; })()"),
        "9"
    );
    // §7.3.25 step 3 — `undefined` and `null` are skipped rather than refused. That is the difference
    // from object *rest* in a pattern, where `var {...a} = null` is a TypeError: the pattern emits a
    // coercibility check and a literal does not.
    assert_eq!(
        run(
            "(function () { return JSON.stringify({...null}) + ',' + JSON.stringify({...undefined}); })()"
        ),
        "{},{}"
    );
    // Own and enumerable, both conditions. A non-enumerable property is not copied…
    assert_eq!(
        run("(function () { var o = {}; \
             Object.defineProperty(o, 'h', {value: 1, enumerable: false}); o.v = 2; \
             var c = {...o}; return ('h' in c) + ',' + c.v; })()"),
        "false,2"
    );
    // …and an inherited one is not either.
    assert_eq!(
        run(
            "(function () { var base = {inherited: 1}; var o = Object.create(base); o.own = 2; \
             var c = {...o}; return ('inherited' in c) + ',' + c.own; })()"
        ),
        "false,2"
    );
    // A getter is *called*, once, and what it answered is copied as a plain data property — the
    // accessor itself does not come along.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {get g() { n++; return 3; }}; var c = {...o}; \
             return c.g + ',' + n + ',' \
             + (Object.getOwnPropertyDescriptor(c, 'g').get === undefined); })()"
        ),
        "3,1,true"
    );
    // Anything with own enumerable properties, not only an object: a String has one per index.
    assert_eq!(
        run("(function () { var c = {...'ab'}; return c[0] + c[1]; })()"),
        "ab"
    );
    // Two spreads, each contributing.
    assert_eq!(
        run(
            "(function () { var a = {a: 1}, b = {b: 2}; var c = {...a, ...b}; \
             return c.a + ',' + c.b; })()"
        ),
        "1,2"
    );
    // The copies are ordinary data properties, whatever the source's attributes were.
    assert_eq!(
        run("(function () { var o = {}; \
             Object.defineProperty(o, 'a', {value: 1, enumerable: true, writable: false, \
                                            configurable: false}); \
             var d = Object.getOwnPropertyDescriptor({...o}, 'a'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"),
        "true,true,true"
    );
}

#[test]
fn the_proto_key_in_a_literal_sets_the_prototype_and_only_in_one_spelling() {
    // B.3.1 — the one Annex B rule ViperJS implements, and DR-0008 says why: it is not conditioned on
    // strictness, and leaving it out was a *silent wrong answer* rather than a refusal, the grammar
    // already accepting `__proto__: x` as an ordinary property definition.
    assert_eq!(
        run(
            "(function () { var p = { m() { return 'p'; } }; var o = { __proto__: p }; \
             return o.m() + ',' + o.hasOwnProperty('__proto__'); })()"
        ),
        "p,false"
    );
    // A String literal key has the same StringValue, and B.3.1 asks about that rather than about the
    // production the key was written as.
    assert_eq!(
        run(
            "(function () { var p = { m() { return 'q'; } }; var o = { '__proto__': p }; \
             return o.m(); })()"
        ),
        "q"
    );
    assert_eq!(
        run("(function () { var o = { __proto__: null }; \
             return Object.getPrototypeOf(o) === null; })()"),
        "true"
    );
    // The shape that surprises people: a value that is neither an Object nor `null` is **ignored** —
    // no prototype change *and* no property. An implementation that fell through to defining a
    // property would answer `true` for the second half.
    assert_eq!(
        run("(function () { var o = { __proto__: 1 }; \
             return (Object.getPrototypeOf(o) === Object.prototype) \
                  + ',' + o.hasOwnProperty('__proto__'); })()"),
        "true,false"
    );
    // Every other way of writing the same spelling is an ordinary property, because B.3.1 covers
    // exactly `PropertyName : AssignmentExpression`. These three exclusions are the whole difficulty,
    // and an implementation that matched on the spelling alone would get all three wrong.
    assert_eq!(
        run(
            "(function () { var p = { a: 1 }; var o = { ['__proto__']: p }; \
             return o.hasOwnProperty('__proto__') \
                  + ',' + (Object.getPrototypeOf(o) === Object.prototype); })()"
        ),
        "true,true"
    );
    assert_eq!(
        run(
            "(function () { var __proto__ = { a: 1 }; var o = { __proto__ }; \
             return o.hasOwnProperty('__proto__'); })()"
        ),
        "true"
    );
    assert_eq!(
        run("(function () { var o = { __proto__() { return 1; } }; \
             return o.hasOwnProperty('__proto__') + ',' + o.__proto__(); })()"),
        "true,1"
    );
    // …and it reaches a prototype's methods through the chain like any other, which is what makes the
    // rule worth having rather than a curiosity.
    assert_eq!(
        run("(function () { var p = { get v() { return this.n; } }; \
             var o = { __proto__: p, n: 7 }; return o.v; })()"),
        "7"
    );
}

/// How many Strings a script leaves on the heap once it has run.
///
/// A fresh heap each time, so the answer is about the script and not about what ran before it.
fn strings_left_by(source: &str) -> usize {
    let mut heap = Heap::new();
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    Vm::new(&mut heap)
        .run(&chunk, &mut heap)
        .expect("the chunk is well formed"); // same
    heap.string_count()
}

#[test]
fn writing_one_element_a_hundred_times_does_not_leave_a_hundred_copies_of_its_name() {
    // §7.1.19 turns the index `0` into the key `"0"`, and a key is *interned* — so the second
    // write finds the name the first one filed and nothing new is allocated. Reaching that through
    // `ToString` first did allocate one, handed it to the intern table, got the earlier copy back
    // and abandoned the new one — a dead String per property access, which DR-0010 never gives
    // back and DR-0013's budget goes on counting. A million writes to a single element cost 17 MiB
    // of names for one value.
    //
    // Asked as "does it grow with the number of accesses" rather than as an absolute count,
    // because what a script allocates otherwise is not this test's business.
    let few = strings_left_by("var a = []; for (var i = 0; i < 4; i++) { a[0] = i; }");
    let many = strings_left_by("var a = []; for (var i = 0; i < 400; i++) { a[0] = i; }");
    assert_eq!(
        few, many,
        "a hundred times the accesses left {many} names against {few}"
    );
}

#[test]
fn a_computed_name_is_filed_once_however_many_objects_wear_it() {
    // The same rule for a name rather than an index, and through a different conversion: the key
    // here is spelled by `ToString` of a Number that is not an index at all. One entry in the
    // intern table serves every write, so the count does not move with the number of them.
    //
    // The two sources differ *only* in the loop's bound, which is a Number constant. Written with
    // one of them as a straight-line `o[1.5] = 1`, the other declares an `i` the first does not
    // and the count is one higher for a reason that has nothing to do with the subject.
    let once = strings_left_by("var o = {}; for (var i = 0; i < 1; i++) { o[1.5] = i; }");
    let many = strings_left_by("var o = {}; for (var i = 0; i < 200; i++) { o[1.5] = i; }");
    assert_eq!(once, many);
}

#[test]
fn a_reference_that_is_read_and_written_settles_its_key_once_and_after_the_base() {
    // §6.2.5.5's `GetValue` and `PutValue` both convert a property reference's key — and both
    // *write the converted key back into the Reference Record*, so a compound assignment, which
    // reads and then writes one reference, converts it once. Without that, `o[p] += 1` calls
    // `p.toString()` twice.
    assert_eq!(
        run(
            "var n = 0; var o = {}; var p = {toString: function () { n++; return 'k' }}; \
             o[p] ^= 0; n"
        ),
        "1"
    );
    assert_eq!(
        run(
            "var n = 0; var o = {k: 1}; var p = {toString: function () { n++; return 'k' }}; \
             o[p]++; n + ',' + o.k"
        ),
        "1,2"
    );
    assert_eq!(
        run(
            "var n = 0; var o = {}; var p = {toString: function () { n++; return 'k' }}; \
             o[p] ||= 5; n + ',' + o.k"
        ),
        "1,5"
    );
    assert_eq!(
        run(
            "var n = 0; var o = {k: 1}; var p = {toString: function () { n++; return 'k' }}; \
             o[p] &&= 5; n + ',' + o.k"
        ),
        "1,5"
    );
    // …and the order within one: step 3.a's `ToObject` of the **base** comes before step 3.b's
    // `ToPropertyKey`, so a nullish base throws before the key's `toString` is called at all —
    // and before the right-hand side is evaluated.
    assert_eq!(
        run("var log = []; var base = null; \
             var p = {toString: function () { log.push('key'); return 'k' }}; \
             var rhs = function () { log.push('rhs'); return 0 }; \
             try { base[p] ^= rhs() } catch (e) { log.push(e.constructor.name) } log.join(',')"),
        "TypeError"
    );
    assert_eq!(
        run("var log = []; var base; \
             var p = {toString: function () { log.push('key'); return 'k' }}; \
             try { base[p]++ } catch (e) { log.push(e.constructor.name) } log.join(',')"),
        "TypeError"
    );
    // The whole sequence for a base that is fine: the key once, then the right-hand side, and no
    // second conversion after it.
    assert_eq!(
        run("var log = []; var o = {}; \
             var p = {toString: function () { log.push('key'); return 'k' }}; \
             var rhs = function () { log.push('rhs'); return 1 }; \
             o[p] ^= rhs(); log.join(',')"),
        "key,rhs"
    );
    // A plain `=` reads nothing, so it converts once anyway — and it converts **after** the
    // right-hand side, because `PutValue` is step 8 and nothing before it touches the key.
    assert_eq!(
        run("var log = []; var o = {}; \
             var p = {toString: function () { log.push('key'); return 'k' }}; \
             o[p] = (log.push('rhs'), 1); log.join(',')"),
        "rhs,key"
    );
}

#[test]
fn settling_a_key_changes_nothing_a_program_could_otherwise_see() {
    // The settled key is left on the stack as the String or Symbol it became, and `ToPropertyKey`
    // of that is itself — so every kind of key still reaches the property it names.
    assert_eq!(run("var o = {k: 1}; o['k'] += 2; o.k"), "3");
    assert_eq!(run("var o = {}; o[0] = 1; o[0] += 2; o[0]"), "3");
    assert_eq!(run("var a = [1, 2]; a[1] += 10; a.join(',')"), "1,12");
    assert_eq!(
        run("var s = Symbol('s'); var o = {}; o[s] = 1; o[s] += 1; o[s]"),
        "2"
    );
    assert_eq!(
        run("var o = {}; o[undefined] = 1; o[undefined] += 1; o['undefined']"),
        "2"
    );
    // A Symbol key still cannot be converted with `ToString`, which is what makes the Symbol
    // branch of the conversion load-bearing rather than a shortcut.
    assert_eq!(
        run("var s = Symbol('s'); var o = {}; o[s] ||= 7; String(o[s])"),
        "7"
    );
    // A `super[p]` reference is one value wider — §13.3.7.1 pushes the receiver under the base —
    // so the base this checks sits one deeper. Both the conversion count and the reference's own
    // shape have to be right, or the stack unbalances.
    //
    // The answer also says which object each end reached: `super[p]` *reads* the home object's
    // prototype, where `k` is 0 and falsy, and *writes* the receiver — so the instance gains a `k`
    // of 9 and `A.prototype.k` is left as it was.
    assert_eq!(
        run(
            "var n = 0; var p = {toString: function () { n++; return 'k' }}; \
             class A {} A.prototype.k = 0; \
             class B extends A { go() { super[p] ||= 9; \
                return n + ',' + this.k + ',' + A.prototype.k } } \
             new B().go()"
        ),
        "1,9,0"
    );
    // A written-down key needs no settling at all and is unaffected — including a private name,
    // which is not a property key and must never be handed to `ToPropertyKey`.
    assert_eq!(
        run("class C { #x = 1; go() { this.#x += 2; return this.#x } } new C().go()"),
        "3"
    );
    assert_eq!(
        run("class C { #x = 1; go() { this.#x++; return this.#x } } new C().go()"),
        "2"
    );
    // A getter and a setter on the same key are both reached through the one settled key, which
    // is the row that would notice a key converted once and then *used* twice differently.
    assert_eq!(
        run(
            "var seen = []; var o = {get k() { seen.push('get'); return 1 }, \
             set k(v) { seen.push('set ' + v) }}; \
             var p = {toString: function () { seen.push('key'); return 'k' }}; \
             o[p] += 4; seen.join(',')"
        ),
        "key,get,set 5"
    );
}

#[test]
fn a_write_through_a_primitive_wraps_the_base_and_keeps_the_primitive_as_receiver() {
    // §6.2.5.6 step 3.a wraps the base with `ToObject`; step 3.c hands `GetThisValue(V)` as the
    // receiver, which is the **primitive**. So §10.1.9.2's accessor branch calls an inherited
    // setter with the primitive as `this` — a setter on `Number.prototype` really runs for a write
    // through a number, and this was never reached before: the base was refused outright.
    assert_eq!(
        run(
            "(function () { var seen;              Object.defineProperty(Number.prototype, 'probe', {                set: function (v) { seen = typeof this + ':' + this + ':' + v; }, configurable: true });              var n = 1; n.probe = 5; return seen; })()"
        ),
        "number:1:5"
    );
    // A String's is reached the same way, which is the row that says this is about the clause and
    // not about numbers.
    assert_eq!(
        run(
            "(function () { var seen;              Object.defineProperty(String.prototype, 'probe', {                set: function () { seen = this.length; }, configurable: true });              var s = 'abc'; s.probe = 1; return String(seen); })()"
        ),
        "3"
    );
    // And a Proxy on the wrapper's prototype chain sees the write, because the walk reaches it
    // through the wrapper rather than stopping at the primitive.
    assert_eq!(
        run(
            "(function () { var count = 0;              var spy = new Proxy({}, { set: function () { count += 1; return true; } });              var was = Object.getPrototypeOf(Boolean.prototype);              Object.setPrototypeOf(Boolean.prototype, spy);              true.anything = 1;              Object.setPrototypeOf(Boolean.prototype, was);              return count; })()"
        ),
        "1"
    );
}

#[test]
fn an_ordinary_write_through_a_primitive_is_silent_in_sloppy_code_and_throws_in_strict() {
    // §10.1.9.2 step 3.b — a receiver that is not an Object answers `false`, and §6.2.5.6 step 3.d
    // turns that into a TypeError **only** for strict code. The value goes nowhere either way:
    // there is no object to keep it on, because the wrapper is discarded the moment the write ends.
    assert_eq!(
        run("(function () { var n = 1; n.foo = 2; return String(n.foo); })()"),
        "undefined"
    );
    for spelling in [
        "var n = 1; n.foo = 2",
        "var s = 'a'; s.foo = 2",
        "var b = true; b.foo = 2",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ 'use strict';                  try {{ {spelling}; return 'no throw' }} catch (e) {{ return e.constructor.name }} }})()"
            )),
            "TypeError",
            "for `{spelling}`"
        );
        assert_eq!(
            run(&format!(
                "(function () {{ {spelling}; return 'silent' }})()"
            )),
            "silent",
            "for `{spelling}`"
        );
    }
    // A String's index is the same rule and not a special case: the character does not change.
    assert_eq!(
        run("(function () { var s = 'a'; s[0] = 'b'; return s; })()"),
        "a"
    );
    // `undefined` and `null` still throw in **both** modes, because there is no object to wrap
    // them in — that is step 3.a failing rather than step 3.d firing.
    assert_eq!(
        run(
            "(function () { var x; try { x.a = 1; return 'no throw' }              catch (e) { return e.constructor.name } })()"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { var x = null; try { x.a = 1; return 'no throw' }              catch (e) { return e.constructor.name } })()"
        ),
        "TypeError"
    );
}
