//! §15.7 and §7.3.28 — a private field, which is not a property by any test a program can make.
//!
//! Two things here are worth more than the rest. A private element must be **invisible** to every
//! property walk, because putting one in the property table would mean teaching each of `Object.keys`,
//! `getOwnPropertyNames`, `getOwnPropertySymbols` and `for...in` to skip it — and the one that was
//! forgotten would leak `#x` to a script. And a Private Name is **fresh per evaluation** of the class,
//! so an instance of one evaluation is not a brand of another; a name baked into the compiled chunk
//! would make every test in this file pass and be wrong.

use super::*;

#[test]
fn a_private_field_is_read_and_written_through_the_name_its_class_minted() {
    assert_eq!(
        run(
            "(function () { class C { #x = 1; read() { return this.#x; } } \
             return new C().read(); })()"
        ),
        "1"
    );
    assert_eq!(
        run(
            "(function () { class C { #x = 1; write(v) { this.#x = v; return this.#x; } } \
             return new C().write(9); })()"
        ),
        "9"
    );
    // §15.7.14 — written without an initialiser it is `undefined`, and it is *there*: the difference
    // from being absent is what `#x in o` below can see.
    assert_eq!(
        run(
            "(function () { class C { #x; read() { return String(this.#x); } } \
             return new C().read(); })()"
        ),
        "undefined"
    );
    // Two names on one object do not collide, which is what says the list is keyed and not a slot.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; #y = 2; sum() { return this.#x + this.#y; } } \
             return new C().sum(); })()"
        ),
        "3"
    );
    // An initialiser runs per construction and may read what was set before it, exactly as a public
    // field's does — the two go through one path and differ only in where the value lands.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; #y = this.#x + 1; sum() { return this.#x + this.#y; } } \
             return new C().sum(); })()"
        ),
        "3"
    );
    // An arrow inside a method reaches the name through the scope chain, having no scope of its own.
    assert_eq!(
        run(
            "(function () { class C { #x = 4; peek() { var f = () => this.#x; return f(); } } \
             return new C().peek(); })()"
        ),
        "4"
    );
    // …and so does a nested plain function, because the *name* is lexical even where `this` is not.
    assert_eq!(
        run(
            "(function () { class C { #x = 5; peek() { var self = this; \
             var f = function () { return self.#x; }; return f(); } } \
             return new C().peek(); })()"
        ),
        "5"
    );
}

#[test]
fn a_private_field_is_invisible_to_every_way_of_asking_what_an_object_has() {
    // The reason `[[PrivateElements]]` is a list of its own rather than rows in the property table.
    // Each of these would need to be taught to skip a private key, and the one that was forgotten
    // would hand `#x` to a script.
    assert_eq!(
        run("(function () { class C { #x = 1; } var o = new C(); \
             return Object.keys(o).length + ',' + Object.getOwnPropertyNames(o).length \
                  + ',' + Object.getOwnPropertySymbols(o).length; })()"),
        "0,0,0"
    );
    assert_eq!(
        run("(function () { class C { #x = 1; } var seen = 0; \
             for (var k in new C()) seen++; return seen; })()"),
        "0"
    );
    // Nor by the name it happens to spell: `#x` and `'x'` are not the same key, and an object with a
    // private `#x` has no property `x` at all.
    assert_eq!(
        run("(function () { class C { #x = 1; } var o = new C(); \
             return o.hasOwnProperty('x') + ',' + ('x' in o) + ',' + String(o.x); })()"),
        "false,false,undefined"
    );
    // A public field of the same spelling is a *second*, separate thing.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; x = 2; both() { return this.#x + ',' + this.x; } } \
             return new C().both(); })()"
        ),
        "1,2"
    );
    // §7.3.29 — a private field may be added to an object that is not extensible, because it is not a
    // property and extensibility is about properties: `PrivateFieldAdd` has no extensibility step, only
    // the duplicate-name check and a host hook for web browsers. `Object.freeze` in a field initialiser
    // is the only way to reach it, since the fields run before the constructor body.
    //
    // **test262 asserts the opposite**, in tests flagged `nonextensible-applies-to-private` — which is
    // a *proposal* (`tc39/proposal-nonextensible-applies-to-private`) and not ES2023. Those tests are
    // expectations entries, and this row is why: do not "fix" the engine to match them without
    // checking that the proposal has landed in the specification first.
    assert_eq!(
        run(
            "(function () { class C { a = Object.freeze(this); #x = 1; read() { return this.#x; } } \
             return new C().read(); })()"
        ),
        "1"
    );
}

#[test]
fn reading_a_private_field_an_object_does_not_have_is_a_type_error() {
    // §7.3.31 `PrivateGet` — and this is what makes a private name a *brand* rather than a hidden
    // property: there is no way to ask that answers `undefined`.
    for target in ["{}", "[]", "1", "'a'", "null", "undefined", "new Object()"] {
        assert_eq!(
            run(&format!(
                "(function () {{ class C {{ #x = 1; read(o) {{ return o.#x; }} }} \
                 try {{ new C().read({target}); return 'no'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "reading #x of {target}"
        );
    }
    // §7.3.32 `PrivateSet` does **not** create one, which is the same rule from the writing side and
    // is what fixes an object's set of private names at construction.
    assert_eq!(
        run("(function () { class C { #x = 1; write(o) { o.#x = 2; } } \
             try { new C().write({}); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // Not found in a list that *has* other names is a separate path from having no list at all, and
    // the answer is the same TypeError: an instance of one class carries its own names and none of
    // another's.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; } class E { #y = 1; write(o) { o.#y = 2; } }              try { new E().write(new C()); return 'no'; }              catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // …and a primitive target fails the write for the same reason it fails the read: it carries no
    // private elements and no wrapper is made to carry any.
    for target in ["7", "'a'", "true", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ class C {{ #x = 1; write(o) {{ o.#x = 2; }} }}                  try {{ new C().write({target}); return 'no'; }}                  catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "writing #x of {target}"
        );
    }
    // §7.3.29 step 3 — adding a name an object *already* carries is a TypeError, and it is reachable:
    // a parent constructor that answers with an object it made earlier has that object initialised
    // twice by the derived class's field list.
    assert_eq!(
        run(
            "(function () { var first;              class B { constructor() { if (first) return first; first = this; } }              class D extends B { #y = 1; }              new D(); try { new D(); return 'no'; }              catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // §13.15.5 and §14.7.5 — a private field is a legal assignment *target*, and not only on the
    // right of an `=`. Each of these went through a path that wrote with `SetProperty`, and a Private
    // Name is a valid property key — so instead of throwing they quietly made a Symbol-keyed property
    // on the object, which is the worst shape a bug can have.
    for target in [
        "for (o.#x of [1]) ;",
        "[o.#x] = [1];",
        "({ a: o.#x } = { a: 1 });",
        "for (o.#x in { a: 1 }) ;",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ class C {{ #x = 1; write(o) {{ {target} }} }}                  try {{ new C().write({{}}); return 'no'; }}                  catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "{target}"
        );
    }
    // …and each writes the field when the object *does* carry it, so the rows above are about the
    // missing name and not about the target form being refused outright.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; take() { for (this.#x of [7]) ; return this.#x; } }              return new C().take(); })()"
        ),
        "7"
    );
    assert_eq!(
        run(
            "(function () { class C { #x = 1; take() { [this.#x] = [8]; return this.#x; } }              return new C().take(); })()"
        ),
        "8"
    );
    // An instance of a *subclass* has the field, because the parent's initialisers ran on it.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; read(o) { return o.#x; } } class D extends C {} \
             return new C().read(new D()); })()"
        ),
        "1"
    );
}

#[test]
fn hash_in_asks_without_risking_the_throw() {
    // §13.10.1 — the production exists precisely because §7.3.31 throws. Without it, asking whether
    // an object is one of yours would mean catching a TypeError.
    assert_eq!(
        run(
            "(function () { class C { #x; static has(o) { return #x in o; } } \
             return C.has(new C()) + ',' + C.has({}); })()"
        ),
        "true,false"
    );
    // §13.10.1 **step 3** — a non-object right-hand side is a TypeError, exactly as for an ordinary
    // `in`. This file asserted the opposite for one commit, on the guess that the production existed
    // to make the question always safe; it exists to make it safe for an *object* that lacks the name,
    // where §7.3.31 would throw. Read the clause.
    for target in ["1", "'a'", "null", "undefined", "true"] {
        assert_eq!(
            run(&format!(
                "(function () {{ class C {{ #x; static has(o) {{ return #x in o; }} }}                  try {{ C.has({target}); return 'no'; }}                  catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "#x in {target}"
        );
    }
    assert_eq!(
        run(
            "(function () { try { return 'x' in 1; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // A subclass instance carries it and an unrelated class's does not, which is the brand check the
    // production was added for.
    assert_eq!(
        run(
            "(function () { class C { #x; static has(o) { return #x in o; } } \
             class D extends C {} class E { #x; } \
             return C.has(new D()) + ',' + C.has(new E()); })()"
        ),
        "true,false"
    );
}

#[test]
fn each_evaluation_of_a_class_mints_its_own_private_names() {
    // §9.2 — the `PrivateEnvironment` is created per *evaluation*, so two classes from one piece of
    // source have two sets of names and an instance of one is not a brand of the other. A Private
    // Name compiled into the chunk as a constant would make every row in this file pass and this one
    // answer `true,true`.
    assert_eq!(
        run(
            "(function () { function make() { return class { #x = 1; static has(o) { return #x in o; } }; } \
             var A = make(), B = make(); \
             return A.has(new A()) + ',' + A.has(new B()); })()"
        ),
        "true,false"
    );
    // …and the read throws across the two, rather than answering the other one's value.
    assert_eq!(
        run(
            "(function () { function make() { return class { #x = 1; read(o) { return o.#x; } }; } \
             var A = make(), B = make(); \
             try { new A().read(new B()); return 'no'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // The same holds for a class *declaration* in a function called twice, which is the shape a
    // scope-per-evaluation gets right and a scope-per-source-position would not.
    assert_eq!(
        run(
            "(function () { function make() { class C { #x = 1; static has(o) { return #x in o; } } \
             return C; } var A = make(), B = make(); \
             return A.has(new A()) + ',' + A.has(new B()); })()"
        ),
        "true,false"
    );
}

#[test]
fn a_private_method_is_one_function_every_instance_carries_an_entry_for() {
    // §15.7.14 and §7.3.30 — the method is made **once**, at the class definition, and each instance
    // gets an *entry* pointing at it. That is what makes `#m in o` a brand rather than a lookup up a
    // prototype chain, and it is why the function is shared rather than copied.
    assert_eq!(
        run(
            "(function () { class C { #m() { return 1; } call() { return this.#m(); } } \
             return new C().call(); })()"
        ),
        "1"
    );
    assert_eq!(
        run(
            "(function () { class C { #m() { return 1; } same(o) { return this.#m === o.#m; } } \
             return new C().same(new C()); })()"
        ),
        "true"
    );
    // Not on the prototype and not on the instance, by any way of asking: a private method is not a
    // property at all, which is the same rule its field siblings follow.
    assert_eq!(
        run("(function () { class C { #m() {} } \
             return Object.getOwnPropertyNames(C.prototype).length + ',' \
                  + Object.getOwnPropertyNames(new C()).length; })()"),
        "1,0"
    );
    // An *instance* method's entry goes on the instance and nowhere else — in particular not on the
    // constructor, which is where a static one goes. The two lists are separate, and adding an
    // instance method to the constructor as well would make `#m in C` true.
    assert_eq!(
        run(
            "(function () { class C { #m() {} static has(o) { return #m in o; } } \
             return C.has(C) + ',' + C.has(new C()); })()"
        ),
        "false,true"
    );
    // …and a class may have both kinds at once, each with a function of its own.
    assert_eq!(
        run(
            "(function () { class C { #i() { return 'i'; } static #s() { return 's'; } \
             static both() { return C.#s(); } inst() { return this.#i(); } } \
             return C.both() + ',' + new C().inst(); })()"
        ),
        "s,i"
    );
    // §7.3.30 step 2 — adding a name an object already carries is a TypeError for a method exactly as
    // for a field, and it is reachable the same way: a parent constructor that answers with an object
    // it made earlier has the derived class's methods added to it twice.
    assert_eq!(
        run("(function () { var first; \
             class B { constructor() { if (first) return first; first = this; } } \
             class D extends B { #m() {} } \
             new D(); try { new D(); return 'no'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // A private method reads private fields of the same instance, which is most of what one is for.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; #m() { return this.#x + 1; } call() { return this.#m(); } } \
             return new C().call(); })()"
        ),
        "2"
    );
    // §7.3.32 step 3 — a **method** refuses assignment, which is what makes it unlike a field that
    // happens to hold a function.
    assert_eq!(
        run("(function () { class C { #m() {} bad() { this.#m = 1; } } \
             try { new C().bad(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    assert_eq!(
        run(
            "(function () { class C { #m = function () {}; fine() { this.#m = 1; return this.#m; } } \
             return new C().fine(); })()"
        ),
        "1"
    );
}

#[test]
fn the_methods_are_added_before_any_field_initialiser_runs() {
    // §15.7.14's `InitializeInstanceElements` adds every private method *first* and only then
    // evaluates the fields. The order is observable exactly here: a field initialiser may call one.
    assert_eq!(
        run(
            "(function () { class C { #f = this.#m(); #m() { return 3; } read() { return this.#f; } } \
             return new C().read(); })()"
        ),
        "3"
    );
    // …including when the method is written *after* the field, which is the row that says the adds
    // are not simply interleaved in source order with the fields.
    assert_eq!(
        run(
            "(function () { class C { #m() { return 4; } #f = this.#m(); read() { return this.#f; } } \
             return new C().read(); })()"
        ),
        "4"
    );
    // A derived class runs its fields from `super()`, so the methods have to be added there too.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { #f = this.#m(); #m() { return 5; } read() { return this.#f; } } \
             return new D().read(); })()"),
        "5"
    );
}

#[test]
fn a_private_accessor_is_one_element_with_two_halves() {
    // §7.3.30 — `get #a` and `set #a` are two class elements and **one** private element, so the
    // second has to join the first rather than be refused as a duplicate name. Written the other way
    // round it would be either a TypeError or a lost half.
    assert_eq!(
        run(
            "(function () { class C { #v = 0; get #a() { return this.#v; } set #a(x) { this.#v = x * 2; } \
             run() { this.#a = 4; return this.#a; } } return new C().run(); })()"
        ),
        "8"
    );
    // …and in the other written order, because the merge has to work from either side.
    assert_eq!(
        run(
            "(function () { class C { #v = 0; set #a(x) { this.#v = x * 2; } get #a() { return this.#v; } \
             run() { this.#a = 4; return this.#a; } } return new C().run(); })()"
        ),
        "8"
    );
    // The getter is called with the instance as its receiver, which is what lets it read a field.
    assert_eq!(
        run(
            "(function () { class C { #x = 6; get #a() { return this.#x; } read() { return this.#a; } } \
             return new C().read(); })()"
        ),
        "6"
    );
    // A half that was not written is a **TypeError** in that direction, where a public accessor would
    // answer `undefined` for a missing getter and silently do nothing for a missing setter.
    //
    // The *message* and not only the constructor, because calling `undefined` is a TypeError as well:
    // asserting the kind alone cannot tell the guard from its absence, and mutation coverage said so
    // by surviving the removal of both.
    assert_eq!(
        run(
            "(function () { class C { get #a() { return 1; } bad() { this.#a = 2; } } \
             try { new C().bad(); return 'no'; } catch (e) { return e.message; } })()"
        ),
        "this private accessor has no setter"
    );
    assert_eq!(
        run(
            "(function () { class C { set #a(v) {} bad() { return this.#a; } } \
             try { new C().bad(); return 'no'; } catch (e) { return e.message; } })()"
        ),
        "this private accessor has no getter"
    );
}

#[test]
fn a_static_private_element_belongs_to_the_constructor_and_to_nothing_else() {
    // §15.7.14 — a static private method or field is added to the *constructor* when the class is
    // defined, and no instance ever carries it. So the brand check is about `C` itself.
    assert_eq!(
        run(
            "(function () { class C { static #s() { return 's'; } static call() { return C.#s(); } } \
             return C.call(); })()"
        ),
        "s"
    );
    assert_eq!(
        run(
            "(function () { class C { static #x = 7; static read() { return C.#x; } } \
             return C.read(); })()"
        ),
        "7"
    );
    assert_eq!(
        run(
            "(function () { class C { static #x = 1; static has(o) { return #x in o; } } \
             return C.has(C) + ',' + C.has(new C()); })()"
        ),
        "true,false"
    );
    // Reading one off an instance is the ordinary TypeError, which is what that `false` means.
    assert_eq!(
        run(
            "(function () { class C { static #x = 1; static read(o) { return o.#x; } } \
             try { C.read(new C()); return 'no'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // A static accessor is one element on the constructor, merged from its two halves like any other.
    assert_eq!(
        run(
            "(function () { class C { static #v = 0; static get #a() { return C.#v; } \
             static set #a(x) { C.#v = x + 1; } static run() { C.#a = 4; return C.#a; } } \
             return C.run(); })()"
        ),
        "5"
    );
    // A static private field written beside a public one does not collide with it.
    assert_eq!(
        run(
            "(function () { class C { static #x = 1; static x = 2; static both() { return C.#x + ',' + C.x; } } \
             return C.both(); })()"
        ),
        "1,2"
    );
}

#[test]
fn a_private_method_is_fresh_per_evaluation_like_the_names_are() {
    // The same property the fields have, and for the same reason: the Private Name lives in the class
    // body's scope, which is a new environment each time the definition runs. Two evaluations give two
    // brands, so an instance of one cannot be read by the other's method.
    assert_eq!(
        run(
            "(function () { function make() { return class { #m() { return 1; } \
             static has(o) { return #m in o; } }; } \
             var A = make(), B = make(); \
             return A.has(new A()) + ',' + A.has(new B()); })()"
        ),
        "true,false"
    );
    assert_eq!(
        run(
            "(function () { function make() { return class { #m() { return 1; } \
             read(o) { return o.#m(); } }; } var A = make(), B = make(); \
             try { new A().read(new B()); return 'no'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    // …and the two classes' methods are different function objects, which is the same fact seen from
    // the other side.
    assert_eq!(
        run(
            "(function () { function make() { return class { #m() {} take() { return this.#m; } }; } \
             var A = make(), B = make(); \
             return new A().take() === new B().take(); })()"
        ),
        "false"
    );
}

#[test]
fn a_private_methods_home_object_is_where_super_starts_and_not_where_it_lives() {
    // §15.7.14 gives a private method a `[[HomeObject]]` like any other, and it is the object the
    // method is *conceptually* defined on: the **prototype** for an instance method, the constructor
    // for a static one. A private method lives on neither — it is not a property at all — and that is
    // exactly why the wrong answer sounded reasonable. What a home decides is where `super` starts.
    assert_eq!(
        run("(function () { class B { method() { return 'Base'; } } \
             class C extends B { #m() { return super.method(); } \
                 access(o) { return this.#m.call(o); } } \
             var c = new C(); return c.access(c) + ',' + c.access({}); })()"),
        "Base,Base"
    );
    // …including from a private accessor, which is the same home by a different element kind.
    assert_eq!(
        run("(function () { class B { method() { return 'B'; } } \
             class C extends B { get #p() { return super.method(); } read() { return this.#p; } } \
             return new C().read(); })()"),
        "B"
    );
    // A *static* private method's home is the constructor, so its `super` reaches the parent class's
    // statics and not its prototype's methods. Both halves of that are stated, because a home set to
    // the wrong one of the two objects would pass whichever row was written alone.
    assert_eq!(
        run("(function () { class B { static s() { return 'Bs'; } } \
             class C extends B { static #p() { return super.s(); } static go() { return C.#p(); } } \
             return C.go(); })()"),
        "Bs"
    );
    assert_eq!(
        run("(function () { class B { m() { return 1; } } \
             class C extends B { #g() { return super.m(); } \
                 static #s() { return typeof super.m; } \
                 static go() { return C.#s(); } run() { return this.#g(); } } \
             return new C().run() + ',' + C.go(); })()"),
        "1,undefined"
    );
}

#[test]
fn adding_the_same_private_element_to_one_object_twice_is_refused() {
    // §7.3.30 step 2 — an existing name is a TypeError, and there is no exception for an accessor. Its
    // two halves are **one** element, built at the class definition; merging two adds at run time
    // instead let the same accessor be added to one object twice, and the specification refuses that.
    //
    // Reachable through a parent constructor that answers with an object it made earlier, which is
    // then initialised by the derived class a second time.
    for element in [
        "get #p() {}",
        "set #p(v) {}",
        "get #p() {} set #p(v) {}",
        "#p() {}",
        "#p = 1",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ class Base {{ constructor(o) {{ return o; }} }} \
                 class C extends Base {{ {element} }} \
                 var obj = {{}}; new C(obj); \
                 try {{ new C(obj); return 'no'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "twice: {element}"
        );
    }
}

#[test]
fn a_compound_assignment_and_an_update_work_through_a_private_name() {
    // §13.15.2 evaluates the reference **once** and then reads it back before writing, which needs the
    // whole reference copied — and how much that is depends on which reference it is. Both of these
    // were refused until the copy took a count: a private reference's two values are a base and a
    // *name*, so a read through `GetProperty` would have looked for a property under a Symbol.
    assert_eq!(
        run(
            "(function () { class C { #x = 1; bump() { this.#x += 5; return this.#x; } } \
             return new C().bump(); })()"
        ),
        "6"
    );
    // §13.4.4.1 — `++` answers the old value and stores the new one, and the coercion is of the *old*
    // one: the answer is a number even where the field held a string.
    assert_eq!(
        run(
            "(function () { class C { #x = '1'; bump() { return this.#x++; } read() { return this.#x; } } \
             var c = new C(); return typeof c.bump() + ',' + c.read(); })()"
        ),
        "number,2"
    );
    // A logical assignment does not write at all when it short-circuits, and its stack has to balance
    // on both paths — the reference is under the old value on the one where the circuit fires.
    assert_eq!(
        run(
            "(function () { class C { #x = null; or() { this.#x ||= 7; return this.#x; } } \
             return new C().or(); })()"
        ),
        "7"
    );
    assert_eq!(
        run(
            "(function () { class C { #x = 3; keep() { this.#x ??= 9; return this.#x; } } \
             return new C().keep(); })()"
        ),
        "3"
    );
    // …and a private *method* still refuses the write, which is the rule the compound form must not
    // have found a way around.
    assert_eq!(
        run(
            "(function () { class C { #m() {} bad() { this.#m += 1; } } \
             try { new C().bad(); return 'no'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}
