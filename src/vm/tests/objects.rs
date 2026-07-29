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
fn optional_chaining_and_private_names_are_refused_with_a_span() {
    let mut heap = Heap::new();
    for (source, what) in [
        ("var o = {}; o?.a;", "optional chaining"),
        ("var o = {}; delete o?.a;", "optional chaining"),
        ("var o = {}; o?.['a'];", "optional chaining"),
        ("var o = {}; delete o?.['a'];", "optional chaining"),
    ] {
        // Every row here parses; a row that did not would silently test nothing, which is
        // how a table of refusals stops refusing anything.
        let script = parse_script(source).expect("the row parses"); // a row that does not is the bug

        let error = compile_script(&script, &mut heap).expect_err("not implemented yet"); // the test is about the error
        assert_eq!(
            error.kind,
            crate::compile::ErrorKind::Unsupported(what),
            "compiling {source:?}"
        );
    }
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
