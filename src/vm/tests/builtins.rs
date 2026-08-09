//! §19 through §28 as a script sees them.
//!
//! Every row runs source. What a built-in *is* — a Rust body behind an ordinary object — is an
//! implementation detail; what it answers is not.

use super::*;

#[test]
fn a_built_in_is_an_ordinary_object_that_happens_to_be_callable() {
    // The whole of what makes it a function, from a script's side: `typeof` says so, and calling
    // it works. There is no frame and no environment behind it, and nothing here can tell.
    assert_eq!(run("typeof Error"), "function");
    assert_eq!(run("typeof TypeError"), "function");
    assert_eq!(run("typeof Error.prototype.toString"), "function");
    // …and it is an object like any other, with properties and a prototype.
    assert_eq!(run("typeof Error.prototype"), "object");
    assert_eq!(run("typeof Error.name"), "string");
}

#[test]
fn a_built_in_function_carries_the_name_and_length_10_3_3_gives_it() {
    // `assert.throws` reads `name` off a constructor to say which error it wanted, so this is
    // load-bearing for the suite rather than decoration.
    assert_eq!(run("Error.name"), "Error");
    assert_eq!(run("TypeError.name"), "TypeError");
    assert_eq!(run("Error.prototype.toString.name"), "toString");
    // `length` is how many arguments the specification *writes*, not how many it accepts.
    assert_eq!(run("Error.length"), "1");
    assert_eq!(run("Error.prototype.toString.length"), "0");
    // Both are non-writable, so a sloppy assignment is ignored rather than an error.
    assert_eq!(run("Error.name = 'Other'; Error.name"), "Error");
    // …and both are configurable, said as `delete` answering `true` rather than as the property
    // being gone afterwards. §13.5.1.2 makes the answer mean exactly "it was configurable, or it
    // was not there"; what is *left* behind depends on what `Function.prototype` carries, which
    // is a different slice's business and would make this row wrong the day it lands.
    assert_eq!(run("delete Error.name"), "true");
    assert_eq!(run("delete Error.prototype.toString.length"), "true");
    // The contrast that gives the rows above their meaning: §20.5.2's `prototype` is not
    // configurable, and `delete` says so.
    assert_eq!(run("delete Error.prototype"), "false");
}

#[test]
fn a_built_in_sees_the_receiver_the_call_passed_and_never_a_substitute() {
    // §10.3.1 does no substitution where §10.2.1.2 does. A sloppy-mode JavaScript function called
    // with no receiver is handed the global object; a built-in is handed `undefined` — and
    // `Error.prototype.toString` refusing is how that difference is observable.
    assert_eq!(
        run("var f = Error.prototype.toString; try { f() } catch (e) { e.name }"),
        "TypeError"
    );
    // …while the same function reached as a method sees the object it was found on.
    assert_eq!(run("var e = new Error('x'); e.toString()"), "Error: x");
}

#[test]
fn an_error_is_the_same_thing_whether_or_not_new_was_written() {
    // §20.5.1.1 mentions `NewTarget` only to choose a prototype, never to refuse — which makes
    // `Error` almost the only constructor in the language that does not care.
    assert_eq!(run("Error('x').message"), "x");
    assert_eq!(run("new Error('x').message"), "x");
    assert_eq!(run("TypeError('x') instanceof TypeError"), "true");
    assert_eq!(run("Error('x') instanceof Error"), "true");
}

#[test]
fn an_absent_message_is_not_an_empty_one() {
    // §20.5.1.1 step 3 — `undefined` means *absent*, and an absent message leaves no own
    // property at all, so the empty string comes from `Error.prototype` instead. An empty
    // *string* argument does make one. Nothing prints differently; `hasOwnProperty` would.
    assert_eq!(run("new Error().message"), "");
    assert_eq!(run("new Error(undefined).message"), "");
    assert_eq!(run("new Error('').message"), "");
    // The message is coerced, because §20.5.1.1 step 3 says `ToString`.
    assert_eq!(run("new Error(1).message"), "1");
    assert_eq!(run("new Error(null).message"), "null");
}

#[test]
fn a_native_error_inherits_twice_and_the_second_one_is_the_forgotten_half() {
    // §20.5.6.3 — the *prototype* chain, which is what `instanceof` walks.
    assert_eq!(run("new TypeError('x') instanceof TypeError"), "true");
    assert_eq!(run("new TypeError('x') instanceof Error"), "true");
    assert_eq!(run("new Error('x') instanceof TypeError"), "false");
    // §20.5.6.2 — the *constructor* chain. `TypeError`'s prototype is `Error` itself, so it
    // inherits `Error`'s own properties; `Error`'s is `%Function.prototype%`, which has none yet.
    assert_eq!(run("TypeError.length"), "1");
    // …and the way back, which `assert.throws` compares.
    assert_eq!(run("new TypeError('x').constructor === TypeError"), "true");
    assert_eq!(run("new Error('x').constructor === Error"), "true");
    assert_eq!(run("TypeError.prototype.constructor.name"), "TypeError");
}

#[test]
fn the_error_the_engine_throws_is_the_one_a_script_can_name() {
    // The point of the whole slice. Until `TypeError` was a value a script could reach, a program
    // could catch an error and had no way to ask what it was.
    assert_eq!(
        run("try { null.x } catch (e) { e instanceof TypeError }"),
        "true"
    );
    assert_eq!(
        run("try { nowhere } catch (e) { e instanceof ReferenceError }"),
        "true"
    );
    assert_eq!(
        run("try { nowhere } catch (e) { e.constructor.name }"),
        "ReferenceError"
    );
    // …and the three that no part of the engine throws yet are reachable all the same, because a
    // script throws them.
    for name in ["SyntaxError", "EvalError", "URIError"] {
        assert_eq!(run(&format!("new {name}('x').name")), name);
        assert_eq!(run(&format!("new {name}('x') instanceof Error")), "true");
    }
}

#[test]
fn to_string_joins_the_name_and_the_message_and_drops_whichever_is_empty() {
    // §20.5.3.4 steps 8–10, which are three cases rather than one concatenation.
    assert_eq!(run("new Error('m').toString()"), "Error: m");
    assert_eq!(run("new TypeError('m').toString()"), "TypeError: m");
    assert_eq!(run("new Error().toString()"), "Error");
    assert_eq!(
        run("var e = new Error('m'); e.name = ''; e.toString()"),
        "m"
    );
    assert_eq!(run("var e = new Error(); e.name = ''; e.toString()"), "");
    // It reads through the prototype chain rather than off the error, which is why assigning
    // `name` changes what it prints.
    assert_eq!(
        run("var e = new TypeError('m'); e.name = 'Mine'; e.toString()"),
        "Mine: m"
    );
    // §20.5.3.4 steps 3 and 5 — an absent name reads as "Error" and an absent message as "".
    assert_eq!(
        run(
            "var e = new Error('m'); e.name = undefined; Error.prototype.name = undefined; e.toString()"
        ),
        "Error: m"
    );
}

#[test]
fn a_constructors_prototype_may_not_be_replaced_where_a_functions_may() {
    // §20.5.2 — `Error.prototype` is neither writable nor configurable, which is not how a
    // JavaScript function's `prototype` behaves. The difference is observable from source and is
    // the reason a program can rely on `e instanceof Error`.
    assert_eq!(
        run("Error.prototype = {}; typeof Error.prototype.toString"),
        "function"
    );
    assert_eq!(
        run("delete Error.prototype; typeof Error.prototype"),
        "object"
    );
    // …where a plain function's `prototype` is writable, and what a script puts there is what
    // `new` reads — falling back to `%Object.prototype%` when it is not an object (§10.1.13).
    assert_eq!(run("function f() {} f.prototype = 1; f.prototype"), "1");
    assert_eq!(
        run("function f() {} f.prototype = 1; typeof new f()"),
        "object"
    );
}

#[test]
fn every_built_in_property_follows_17s_convention_about_attributes() {
    // §17: a built-in property is **never** enumerable. Nothing in the language can see that yet
    // — `for...in` and `Object.keys` are both still to come — so it is read off the objects
    // rather than left as a claim in a comment. The other two attributes differ per property and
    // are checked with it, since the three are one decision each time they are written.
    let mut heap = Heap::new();
    let vm = Vm::new(&mut heap);
    let realm = vm.realm;
    let global = realm.global();
    let Some(Value::Object(error)) = own(&mut heap, global, "Error").map(kind_value) else {
        panic!("Error is a global"); // the test is about it
    };

    // §10.3.3's pair on a built-in function: not writable, not enumerable, configurable.
    for name in ["name", "length"] {
        let property = own(&mut heap, error, name).unwrap_or_else(|| panic!("Error.{name}")); // same
        let PropertyKind::Data { writable, .. } = property.kind else {
            panic!("Error.{name} is a data property"); // same
        };
        assert!(!writable, "Error.{name} is not writable");
        assert!(!property.enumerable, "Error.{name} is not enumerable");
        assert!(property.configurable, "Error.{name} is configurable");
    }

    // §20.5.2 — the constructor's `prototype` is none of the three, which is what stops a script
    // replacing it and is why `e instanceof Error` can be relied on.
    let property = own(&mut heap, error, "prototype").expect("Error.prototype"); // same
    let PropertyKind::Data { writable, .. } = property.kind else {
        panic!("Error.prototype is a data property"); // same
    };
    assert!(!writable);
    assert!(!property.enumerable);
    assert!(!property.configurable);

    // …and §17's ordinary shape, which `constructor` and the methods have: writable and
    // configurable, and still not enumerable.
    let property = own(&mut heap, realm.error_prototype(), "toString").expect("toString"); // same
    assert!(!property.enumerable);
    assert!(property.configurable);
}

/// The value a data property holds, for a row that only cares about the value.
fn kind_value(property: crate::heap::Property) -> Value {
    match property.kind {
        PropertyKind::Data { value, .. } => value,
        PropertyKind::Accessor { .. } => Value::Undefined,
    }
}

#[test]
fn is_error_asks_about_the_slot_and_nothing_else() {
    // §20.5.2.1 — the `[[ErrorData]]` slot, which is not `instanceof` and not the `@@toStringTag`.
    // A plain object with `Error.prototype` behind it answers `false` where `instanceof` says
    // true, and that is the whole reason the function exists.
    assert_eq!(
        run(
            "[Error.isError(new TypeError('x')), Error.isError(new Error()), \
             Error.isError(Object.create(Error.prototype)), Error.isError({}), \
             Error.isError(1), Error.isError(null), Error.isError(undefined)].join(',')"
        ),
        "true,true,false,false,false,false,false"
    );
    // It never throws and never reads a property, so a Proxy over an error answers `false`: the
    // slot is on the target and a Proxy has none of its own.
    assert_eq!(
        run("var reads = 0; \
             var p = new Proxy(new Error(), { get: function (t, k) { reads += 1; return t[k] } }); \
             Error.isError(p) + ',' + reads"),
        "false,0"
    );
    assert_eq!(
        run("[Error.isError.length, Error.isError.name].join(',')"),
        "1,isError"
    );
}

#[test]
fn an_error_message_and_a_typed_array_separator_coerce_through_the_machine() {
    // The same fault as `builtins::math`'s, in the two other places a heap-only conversion was
    // reachable from a value a program controls. §20.5.1.1 step 3 and §23.2.3.16 step 3 are both
    // `ToString`, which for an object calls its `toString` and therefore needs the interpreter.
    assert_eq!(
        run("new Error({ toString: function () { return 'said' } }).message"),
        "said"
    );
    assert_eq!(run("new TypeError(new String('boxed')).message"), "boxed");
    // §20.5.7.1 — `AggregateError` takes its message second, and had the same conversion.
    assert_eq!(
        run("new AggregateError([], { toString: function () { return 'agg' } }).message"),
        "agg"
    );
    // …and `undefined` is still *absent* rather than the text "undefined", which is the row that
    // fails if the conversion is moved in front of the check.
    assert_eq!(run("new Error().hasOwnProperty('message')"), "false");
    // §23.2.3.16's separator, which is the one argument to `join` a program controls.
    assert_eq!(
        run("new Int8Array([1, 2, 3]).join({ toString: function () { return '-' } })"),
        "1-2-3"
    );
    // A `toString` that throws reaches the program, so the conversion is really happening.
    assert_eq!(
        run("var said = 'none'; \
             try { new Error({ toString: function () { throw 'from toString' } }) } \
             catch (e) { said = e } said"),
        "from toString"
    );
}

#[test]
fn an_error_takes_a_cause_from_its_options_bag() {
    // §20.5.8.1 `InstallErrorCause`, which ViperJS did not have at all: `new Error('m', { cause })`
    // built an error with no `cause` and no complaint. ES2022, and shipped everywhere.
    assert_eq!(run("new Error('m', { cause: 42 }).cause"), "42");
    assert_eq!(run("new TypeError('m', { cause: 'why' }).cause"), "why");
    // §20.5.7.1.1 — `AggregateError` takes it third, after the errors and the message.
    assert_eq!(run("new AggregateError([], 'm', { cause: 7 }).cause"), "7");
    // `HasProperty` and not a truthiness test, which is the whole reason the clause is two steps:
    // a `cause` of `undefined` was *given*, and an absent one was not. Nothing but
    // `hasOwnProperty` can tell those apart, and a program asking whether a cause was supplied
    // uses exactly that.
    assert_eq!(
        run("new Error('m', { cause: undefined }).hasOwnProperty('cause')"),
        "true"
    );
    assert_eq!(run("new Error('m', {}).hasOwnProperty('cause')"), "false");
    assert_eq!(run("new Error('m').hasOwnProperty('cause')"), "false");
    // …and `HasProperty` rather than `HasOwnProperty`, so a bag made from a shared default works.
    assert_eq!(
        run("var base = { cause: 'inherited' }; \
             new Error('m', Object.create(base)).cause"),
        "inherited"
    );
    // An options argument that is not an object is absent rather than an error.
    assert_eq!(run("new Error('m', null).hasOwnProperty('cause')"), "false");
    assert_eq!(run("new Error('m', 1).hasOwnProperty('cause')"), "false");
    // Non-enumerable, like `message`, so logging an error whole does not spill it.
    assert_eq!(
        run("JSON.stringify(Object.keys(new Error('m', { cause: 1 })))"),
        "[]"
    );
    // The getter runs, and once — which is what says this is a `Get` and not a slot read.
    assert_eq!(
        run("var calls = 0; \
             var bag = { get cause() { calls++; return 'c' } }; \
             new Error('m', bag).cause + ',' + calls"),
        "c,1"
    );
    // …and a getter that throws reaches the program, after the message is already in place.
    assert_eq!(
        run("var said = 'none'; \
             try { new Error('m', { get cause() { throw 'from cause' } }) } \
             catch (e) { said = e } said"),
        "from cause"
    );
}
