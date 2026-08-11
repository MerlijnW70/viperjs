//! §9.3's realm, when there is more than one of them — DR-0025.
//!
//! A realm is the set of intrinsics a script reaches without making them, and until `create_realm`
//! there was exactly one, so nothing in the engine had to say *whose* intrinsics it meant. These
//! are the three questions a second realm makes askable, and each of them has a wrong answer that
//! no single-realm test can see.

use super::*;

/// The Symbol a realm's `Symbol` constructor holds under `name`, as the heap knows it.
///
/// Reached through the *global object* rather than through the heap's own table, because what is
/// under test is what a script would find: `heap.well_known` answering consistently proves nothing
/// if the realm's `Symbol.iterator` property points somewhere else.
fn symbol_property(heap: &mut Heap, realm: &Realm, name: &str) -> Option<crate::heap::SymbolId> {
    let constructor = crate::builtins::global_object(heap, realm, "Symbol")?;
    match crate::builtins::own_value(heap, constructor, name)? {
        Value::Symbol(id) => Some(id),
        _ => None,
    }
}

#[test]
fn a_second_realm_gets_its_own_intrinsics_and_its_own_global() {
    // The whole point of §9.3: two sets of objects, so `other.Array` and `Array` are different
    // constructors and an array made in one is not an instance of the other's.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm();
    let second = vm.create_realm(&mut heap);

    assert_ne!(first.global(), second.global());
    assert_ne!(first.object_prototype(), second.object_prototype());
    assert_ne!(first.array_prototype(), second.array_prototype());
    assert_ne!(
        crate::builtins::global_object(&mut heap, &first, "Array"),
        crate::builtins::global_object(&mut heap, &second, "Array"),
    );
}

#[test]
fn the_well_known_symbols_are_one_set_that_every_realm_shares() {
    // §6.1.5.1: "unless otherwise specified, well-known symbols values are shared by all realms."
    // Built per realm they would be *different* Symbols wearing the same description, and the
    // failure would be silent — an object carrying one realm's `@@iterator` would simply not be
    // iterable in the other, because the key it is filed under is not the key the walk looks for.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm();
    let second = vm.create_realm(&mut heap);

    for name in [
        "iterator",
        "asyncIterator",
        "toPrimitive",
        "toStringTag",
        "species",
    ] {
        assert_eq!(
            symbol_property(&mut heap, &first, name),
            symbol_property(&mut heap, &second, name),
            "Symbol.{name} must be one value in both realms",
        );
        assert!(symbol_property(&mut heap, &first, name).is_some());
    }
}

#[test]
fn building_a_second_realm_makes_no_new_well_known_symbols() {
    // The same fact counted rather than compared, and it is worth both: the comparison above would
    // still pass if the second realm made thirteen Symbols and then found the first realm's through
    // a property it had copied. This says none were made at all.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let before = heap.symbol_count();
    vm.create_realm(&mut heap);

    assert_eq!(heap.symbol_count(), before);
}

#[test]
fn a_realm_roots_what_it_built_and_nothing_that_came_before_it() {
    // `Realm::intrinsics` is the collector's root set for a realm, and it used to be a *ceiling* —
    // every object below the count taken when the realm was sealed. Taken by a realm built second
    // that swallows the first realm's objects and everything a program allocated in between, so the
    // collector would stay sound and go blind. The floor is what stops it.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm();

    // Something allocated *between* the two realms, which belongs to neither.
    let between = heap.new_object(None);

    let second = vm.create_realm(&mut heap);
    let rooted: Vec<_> = second.intrinsics().collect();

    assert!(!rooted.contains(&first.global()));
    assert!(!rooted.contains(&between));
    assert!(rooted.contains(&second.global()));
    // …and the first realm's range still starts at zero, so nothing about the one-realm case moved.
    assert!(first.intrinsics().any(|id| id == first.global()));
}

#[test]
fn a_second_realms_intrinsics_survive_a_collection() {
    // A realm the machine built but does not run in is still reachable from whatever the host was
    // handed, so the collector has to be told about it. Forgetting costs an intrinsic that a script
    // is still inheriting from, which reads as a wrong answer rather than as a leak.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let second = vm.create_realm(&mut heap);
    let array =
        crate::builtins::global_object(&mut heap, &second, "Array").expect("a realm has Array"); // the test is what survives

    let script = parse_script("1").expect("the source parses"); // same
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    vm.run(&chunk, &mut heap).expect("the chunk is well formed"); // same
    vm.collect(&chunk, &mut heap);

    assert!(heap.object(second.global()).is_some());
    assert!(heap.object(array).is_some());
    assert!(heap.object(second.object_prototype()).is_some());
}

#[test]
fn a_function_answers_the_realm_it_was_made_in() {
    // §10.1.14 step 2 — the `[[Realm]]` slot, which here is recorded on the `Callable` when the
    // function object is built. Every intrinsic of a realm is one of its functions, so this is the
    // question `other.Array` has to answer differently from `Array`.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm();
    let second = vm.create_realm(&mut heap);

    let ours =
        crate::builtins::global_object(&mut heap, &first, "Array").expect("a realm has Array"); // the test is which realm answers
    let theirs =
        crate::builtins::global_object(&mut heap, &second, "Array").expect("a realm has Array"); // same

    assert_eq!(vm.realm_of(ours, &heap).global(), first.global());
    assert_eq!(vm.realm_of(theirs, &heap).global(), second.global());
}

#[test]
fn a_bound_function_answers_the_realm_of_what_it_is_bound_to() {
    // §10.1.14 step 3 — a bound function exotic object has no `[[Realm]]` of its own and recurses
    // into its `[[BoundTargetFunction]]`. Binding one of the *other* realm's functions here in this
    // one is exactly the case a slot copied at binding time would get wrong.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let second = vm.create_realm(&mut heap);
    let theirs =
        crate::builtins::global_object(&mut heap, &second, "Array").expect("a realm has Array"); // the test is which realm answers

    let bound = heap.new_bound_function(
        Some(vm.realm().function_prototype()),
        crate::heap::Bound {
            constructs: true,
            target: theirs,
            this_value: Value::Undefined,
            arguments: Vec::new(),
        },
    );
    assert_eq!(vm.realm_of(bound, &heap).global(), second.global());

    // …and a chain of them, because the clause recurses and this is written as a loop.
    let twice = heap.new_bound_function(
        Some(vm.realm().function_prototype()),
        crate::heap::Bound {
            constructs: true,
            target: bound,
            this_value: Value::Undefined,
            arguments: Vec::new(),
        },
    );
    assert_eq!(vm.realm_of(twice, &heap).global(), second.global());
}

#[test]
fn a_proxy_answers_the_realm_of_its_target_and_a_revoked_one_answers_the_running_realm() {
    // §10.1.14 step 4 — a Proxy recurses into its `[[ProxyTarget]]`. Asked *before* the `[[Call]]`,
    // because §10.5 gives a proxy its call through `make_callable` and so it does carry a Callable
    // with a realm on it — the realm of whoever built the proxy, which the clause never asks for.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let second = vm.create_realm(&mut heap);
    let theirs =
        crate::builtins::global_object(&mut heap, &second, "Array").expect("a realm has Array"); // the test is which realm answers
    let handler = heap.new_object(None);

    let proxy = heap.new_object(None);
    if let Some(object) = heap.object_mut(proxy) {
        object.set_proxy(crate::heap::Proxy::new(theirs, handler));
    }
    assert_eq!(vm.realm_of(proxy, &heap).global(), second.global());

    // Revoked, it has no target to ask. The clause throws; this answers the running realm, because
    // every caller wants a default prototype and a revoked proxy is refused by `Construct` long
    // before it could reach one — see `Vm::realm_of`.
    if let Some(found) = heap
        .object_mut(proxy)
        .and_then(crate::heap::Object::proxy_mut)
    {
        found.revoke();
    }
    assert_eq!(vm.realm_of(proxy, &heap).global(), vm.realm().global());
}

#[test]
fn something_with_no_realm_of_its_own_answers_the_running_realm() {
    // §10.1.14 step 5. Reached by every callable that is not one of the four above — §27.5.1's
    // resumption methods and §27.7.5.3's revive closures — and by an object that is not callable at
    // all, which is what a caller holding a `new.target` that is not a constructor would have.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    vm.create_realm(&mut heap);
    let plain = heap.new_object(None);

    assert_eq!(vm.realm_of(plain, &heap).global(), vm.realm().global());
}

#[test]
fn a_proxy_has_no_realm_of_its_own_however_callable_it_is() {
    // The distinction `own_realm` exists for, and it decides where a *call* runs. §10.5.12 is an
    // **internal method**: calling through a proxy pushes no execution context, so the running realm
    // stays the caller's and the trap's arguments array is made in it. A built-in's `[[Call]]` is
    // the opposite — §10.3.1 step 3 makes the callee's realm the running one.
    //
    // ViperJS gives a proxy its `[[Call]]` through `Heap::make_callable`, so it really does hold a
    // `Callable::Native` carrying a realm — whoever built the proxy. Answering `Some` for that would
    // switch the running realm on every call through a proxy, and
    // `Proxy/apply/arguments-realm.js` is what notices.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let second = vm.create_realm(&mut heap);
    let theirs =
        crate::builtins::global_object(&mut heap, &second, "Array").expect("a realm has Array"); // the test is which realm answers
    let handler = heap.new_object(None);

    // A real function has one, and it is the realm that made it.
    assert_eq!(
        vm.own_realm(theirs, &heap).map(|realm| realm.global()),
        Some(second.global())
    );

    let proxy = heap.new_object(None);
    if let Some(object) = heap.object_mut(proxy) {
        object.set_proxy(crate::heap::Proxy::new(theirs, handler));
    }
    heap.make_callable(proxy, refuses, false, vm.realm().id());

    // Callable, carrying a realm on its `Callable::Native`, and still `None` — because the clause
    // gives it no slot. `realm_of` above answers `second` for the same object by recursing into the
    // target, which is the other half of the same distinction.
    assert!(heap.is_callable(Value::Object(proxy)));
    assert!(vm.own_realm(proxy, &heap).is_none());
    assert_eq!(vm.realm_of(proxy, &heap).global(), second.global());
}

/// A body for a proxy's `[[Call]]` that the test never runs — `own_realm` reads the slot, not this.
fn refuses(
    _vm: &mut Vm,
    _heap: &mut Heap,
    _call: &crate::heap::NativeCall<'_>,
) -> crate::value::Completion<Value> {
    Ok(Value::Undefined)
}

#[test]
fn a_default_prototype_comes_from_the_constructors_realm_and_not_the_running_one() {
    // §10.1.13 `GetPrototypeFromConstructor` step 4.a. Step 3's `Get(constructor, "prototype")`
    // decides on its own whenever it answers an object, so the realm in step 4 is reachable by
    // exactly one route: a constructor whose `prototype` is **not** an object, which a script
    // makes by assigning one. `C.prototype = null; new C()` is the whole shape, and without it
    // nothing distinguishes the two realms' `%Object.prototype%` from each other.
    //
    // Every row here answered `here` before, including the two that go through a built-in: those
    // reach `Construct` with the foreign `C` as new.target, so they ask the same question one
    // clause further along.
    for construct in [
        "new C()",
        "Reflect.construct(function () {}, [], C)",
        "Array.from.call(C, [])",
        "Array.of.call(C, 1)",
    ] {
        assert_eq!(
            run_with_other_realm(&format!(
                "var C = other.eval('(function C() {{}})'); C.prototype = null; \
                 Object.getPrototypeOf({construct}) === other.Object.prototype"
            )),
            "true",
            "{construct} should inherit from the other realm's Object.prototype"
        );
    }
    // A function made by the other realm's `Function` constructor rather than by its `eval`, which
    // is the shape `built-ins/Function/proto-from-ctor-realm-prototype.js` uses and a different
    // path to the same `[[Realm]]`.
    assert_eq!(
        run_with_other_realm(
            "var C = new other.Function(); C.prototype = null; \
             Object.getPrototypeOf(new C()) === other.Object.prototype"
        ),
        "true"
    );
    // …and the running realm still decides when the constructor belongs to it, which is what says
    // the fallback follows the constructor rather than simply always answering `other`.
    assert_eq!(
        run_with_other_realm(
            "function C() {} C.prototype = null; \
             Object.getPrototypeOf(new C()) === Object.prototype"
        ),
        "true"
    );
}

#[test]
fn a_built_in_reached_from_rust_runs_in_its_own_realm_as_one_reached_from_an_instruction_does() {
    // §10.3.1 step 3 — the running realm becomes the callee's for the duration of a built-in.
    // There are two ways into a built-in and only one of them did it: `Vm::enter`, which an
    // instruction goes through, and `Vm::reach`, which is how Rust calls one. So the realm a
    // built-in ran in depended on who reached it, and `Reflect` is the plainest way for a program
    // to tell the difference — `Reflect.construct(F, args)` and `new F(...args)` are the same
    // construction by the specification and were not the same here.
    //
    // §20.2.1.1 is what notices, because a dynamic function is stamped with the **running** realm
    // rather than built from `new.target`'s. Most built-ins take everything they need from
    // `new.target`, which is why `Reflect.construct(other.Array, [])` was right throughout and hid
    // this for as long as it existed.
    //
    // The body's free names are the sharper half of the assertion: step 30 gives the function its
    // *own* realm's global environment, so a function stamped with the wrong realm does not merely
    // look wrong, it reads a different global — and reads it as a `ReferenceError` when the name
    // is only defined on one side.
    for construct in [
        "new other.Function('return marker;')",
        "Reflect.construct(other.Function, ['return marker;'])",
        "Reflect.construct(other.Function, ['return marker;'], other.Function)",
    ] {
        assert_eq!(
            run_with_other_realm(&format!(
                "other.marker = 'theirs'; var marker = 'ours'; \
                 ({construct})()"
            )),
            "theirs",
            "{construct} should read the other realm's global"
        );
    }
    // The same question for §20.2.1.1's other three kinds, asked through the `prototype` object a
    // generator function is given — §15.5.4's, which inherits from that realm's
    // `%GeneratorPrototype%` rather than from `%Object.prototype%`.
    for kind in ["function*", "async function*"] {
        assert_eq!(
            run_with_other_realm(&format!(
                "var Ctor = other.eval('({kind} () {{}})').constructor; \
                 var theirs = Object.getPrototypeOf(other.eval('({kind} () {{}})').prototype); \
                 var made = Reflect.construct(Ctor, ['']); \
                 Object.getPrototypeOf(made.prototype) === theirs"
            )),
            "true",
            "a dynamic {kind} built through Reflect.construct"
        );
    }
}
