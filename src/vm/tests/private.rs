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
    // §10.1.4 — a private field may be added to an object that is not extensible, because it is not
    // a property and extensibility is about properties. `Object.freeze` in a field initialiser is the
    // only way to reach it, since the fields run before the constructor body.
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
