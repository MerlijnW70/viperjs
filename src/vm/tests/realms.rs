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
