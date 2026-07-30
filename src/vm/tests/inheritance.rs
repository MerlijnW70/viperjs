//! §15.7 — `extends`, `super` and a derived constructor, which is where a class stops being an
//! object literal with different attributes.
//!
//! Split from [`super::classes`], which had grown to hold both and whose own heading said it covered
//! "the part that does not need `extends`" while half its rows were exactly that. The rows here are
//! about the two things a derived class has that nothing else does: an object made by its *parent*
//! and answered for by the child, and a `this` that does not exist until `super()` has returned.
//!
//! Where an implementation goes wrong and still looks right: wiring the prototype chain and not the
//! constructor chain (every method test passes and `D.s()` is `undefined`); reading the parent's
//! `prototype` instead of new.target's (`new D()` answers a `B`); and passing the super *base* as the
//! receiver instead of `this`, which only a parent's getter can tell.

use super::*;

#[test]
fn extends_points_both_halves_of_a_class_at_both_halves_of_its_parent() {
    // §15.7.14 steps 12 to 14 — two edges, not one, and each carries something different. The
    // prototype chain is what makes an inherited *method* reachable; the constructor chain is what
    // makes an inherited *static* reachable. An implementation that wired only the first would pass
    // every method test and answer `undefined` for `D.s()`.
    assert_eq!(
        run(
            "(function () { class B { m() { return 'm'; } static s() { return 's'; } } \
             class D extends B {} return new D().m() + D.s(); })()"
        ),
        "ms"
    );
    assert_eq!(
        run("(function () { class B {} class D extends B {} \
             return (Object.getPrototypeOf(D.prototype) === B.prototype) + ',' \
                  + (Object.getPrototypeOf(D) === B); })()"),
        "true,true"
    );
    // …and an instance is an instance of every class in the chain, which is the same two edges read
    // by §7.3.20 rather than by a call.
    assert_eq!(
        run(
            "(function () { class B {} class D extends B {} class E extends D {} var e = new E(); \
             return (e instanceof E) + ',' + (e instanceof D) + ',' + (e instanceof B); })()"
        ),
        "true,true,true"
    );
}

#[test]
fn a_heritage_that_is_not_a_constructor_is_a_type_error_and_null_is_not() {
    // §15.7.14 steps 9 to 11 read the value three ways, and the middle case is about `[[Construct]]`
    // rather than about being an object: `Math.max` is a function and is not a constructor, so it
    // fails here where reading its `prototype` would simply have found `undefined`.
    for heritage in ["1", "'a'", "{}", "Math.max", "(() => {})", "undefined"] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ class D extends {heritage} {{}} return 'no'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "extends {heritage}"
        );
    }
    // §15.7.14 step 9 — `extends null` is *not* an error. The class is made, its instances would
    // inherit from nothing, and it is still derived: so the error arrives per construction, when
    // `super()` looks for a constructor and finds `Function.prototype`.
    assert_eq!(
        run("(function () { class D extends null {} return typeof D; })()"),
        "function"
    );
    assert_eq!(
        run("(function () { class D extends null {} \
             return Object.getPrototypeOf(D.prototype) === null; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class D extends null {} \
             try { new D(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // A parent whose `prototype` was replaced with a primitive is step 11's other TypeError, and it
    // is a different check from the one above: `B` here *is* a constructor.
    assert_eq!(
        run("(function () { function B() {} B.prototype = 1; \
             try { class D extends B {} return 'no'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn a_derived_constructor_with_none_written_forwards_every_argument() {
    // §15.7.14 step 15 — the implicit one is `constructor(...args) { super(...args); }`, so the
    // arguments reach the parent unchanged and however many there are. An implementation that
    // synthesised an *empty* constructor would construct successfully and lose every argument.
    assert_eq!(
        run(
            "(function () { class B { constructor(a, b) { this.sum = a + b; } } \
             class D extends B {} return new D(1, 2).sum; })()"
        ),
        "3"
    );
    // Through two levels, each of which forwards, and with a count neither one names.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B {} class E extends D {} return new E(1, 2, 3, 4).n; })()"
        ),
        "4"
    );
}

#[test]
fn a_derived_instance_inherits_from_the_class_that_was_written_after_new() {
    // §10.2.2 — `super()` inherits `new.target` rather than replacing it with the parent, and this is
    // the single most consequential thing about a derived construction: the *parent* makes the
    // object, so if it made one from its own `prototype` then `new D()` would answer a `B` and
    // `d instanceof D` would be false. Read from inside the parent, where the object is made.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.p = Object.getPrototypeOf(this); } } \
             class D extends B {} return new D().p === D.prototype; })()"
        ),
        "true"
    );
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.t = new.target; } } \
             class D extends B {} class E extends D {} return new E().t === E; })()"
        ),
        "true"
    );
    // The running function is read at `super()` time, not captured when the class was defined — so
    // moving `D`'s prototype moves what `super()` reaches. A definition that had recorded the answer
    // would go on calling `B`.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.who = 'B'; } } \
             class C { constructor() { this.who = 'C'; } } \
             class D extends B {} Object.setPrototypeOf(D, C); \
             return new D().who; })()"
        ),
        "C"
    );
}

#[test]
fn a_derived_constructors_this_does_not_exist_until_super_has_returned() {
    // §10.2.2 and DR-0015 — the whole reason `this` is a binding there. Every one of these is a
    // ReferenceError, and each reaches the binding by a different route.
    let unbound = [
        // Read directly, above the call.
        "class D extends B { constructor() { this.x = 1; super(); } }",
        // Read by a parameter default, which runs before the body — so the binding has to exist
        // before the defaults do.
        "class D extends B { constructor(a = this) { super(); } }",
        // Never called at all, so the *return* is what finds the binding empty.
        "class D extends B { constructor() {} }",
        // Returned `undefined` explicitly, which is the same step by the other path.
        "class D extends B { constructor() { return undefined; } }",
        // Called twice: the second is §10.2.2's `BindThisValue` refusing an already-bound binding.
        "class D extends B { constructor() { super(); super(); } }",
    ];
    for source in unbound {
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{}} {source} \
                 try {{ new D(); return 'no'; }} catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "ReferenceError",
            "{source}"
        );
    }
    // …and after `super()` it is there, which is what makes the rows above about *timing* rather
    // than about `this` being broken.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); this.x = 1; } } \
             return new D().x; })()"),
        "1"
    );
    // A `try` around the read proves the throw is an ordinary abrupt completion and not a fault: the
    // constructor recovers and goes on to call `super()`.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { try { this; } \
                 catch (e) { super(); this.caught = 1; } } } \
             return new D().caught; })()"),
        "1"
    );
}

#[test]
fn an_arrow_written_above_a_super_call_still_sees_the_instance() {
    // The case DR-0015 exists for, and the reason `this` is a binding rather than a flag beside the
    // register. An arrow captures its `this` as a *value* where it is written, so an arrow written
    // above the `super()` would have captured the placeholder and answered `undefined` forever.
    // Reading the binding instead means it sees the `super()` that ran after it was made.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => this; super(); \
                 this.ok = f() === this; } } \
             return new D().ok; })()"),
        "true"
    );
    // Called before the `super()`, the same arrow throws — so it is reading the binding each time
    // rather than having been repaired once.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => this; \
                 try { f(); } catch (e) { super(); this.e = e.constructor.name; } } } \
             return new D().e; })()"),
        "ReferenceError"
    );
    // Two levels of arrow, because the binding is reached by counting environments outward and one
    // level is where an off-by-one would still pass.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { var f = () => () => this; super(); \
                 this.ok = f()() === this; } } \
             return new D().ok; })()"),
        "true"
    );
    // The dangerous direction: a body that binds `this` itself must *not* reach the binding. An
    // object-literal method and a function expression both get their own receiver, and a permissive
    // propagation rule would hand them the enclosing instance instead.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); \
                 var o = { m() { return this; } }; this.ok = o.m() === o; } } \
             return new D().ok; })()"),
        "true"
    );
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); \
                 this.m = function () { return this; }; } } \
             var d = new D(); return d.m() === d; })()"),
        "true"
    );
}

#[test]
fn a_derived_constructor_may_only_answer_with_an_object_or_undefined() {
    // §10.2.2 step 13, which is *stricter* than a base constructor's: there a primitive `return` is
    // ignored and the constructed object is answered with anyway. Here it is a TypeError, and that
    // difference is the whole reason the two returns cannot share one instruction.
    for value in ["1", "'a'", "true", "null"] {
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{}} \
                 class D extends B {{ constructor() {{ super(); return {value}; }} }} \
                 try {{ new D(); return 'no'; }} catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "return {value}"
        );
        // The same value from a *base* constructor is ignored, not an error.
        assert_eq!(
            run(&format!(
                "(function () {{ class B {{ constructor() {{ return {value}; }} }} \
                 return new B() instanceof B; }})()"
            )),
            "true",
            "base return {value}"
        );
    }
    // An object return wins, exactly as in a base constructor — and it does not have to be the
    // instance.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); return { z: 9 }; } } \
             return new D().z; })()"),
        "9"
    );
    // `return;` with nothing is `return undefined`, which is answered with the bound `this`.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { constructor() { super(); this.x = 1; return; } } \
             return new D().x; })()"),
        "1"
    );
}

#[test]
fn a_derived_classs_fields_are_initialised_by_super_and_not_on_entry() {
    // §15.7.14 — `InitializeInstanceElements` runs at step 7 of `SuperCall`, after the parent has made
    // the object, because until then there is nothing to define a property on. So a field initialiser
    // can read what the parent wrote, which is what makes the ordering observable rather than
    // internal.
    assert_eq!(
        run("(function () { class B { constructor() { this.x = 10; } } \
             class D extends B { y = this.x + 1; } return new D().y; })()"),
        "11"
    );
    // …and the parent cannot see the field, which is the same ordering from the other side.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.seen = this.y; } } \
             class D extends B { y = 1; } return String(new D().seen); })()"
        ),
        "undefined"
    );
    // Fields go in source order, and after the parent's work in both cases.
    assert_eq!(
        run("(function () { var order = []; \
             class B { constructor() { order.push('B'); } } \
             class D extends B { a = order.push('a'); b = order.push('b'); \
               constructor() { super(); order.push('body'); } } \
             new D(); return order.join(','); })()"),
        "B,a,b,body"
    );
    // A computed field name in a derived class is still evaluated once, at definition time — the slot
    // it was left in is reached from the field initialiser body, which is one environment further out
    // than in a base class because that body is nested inside the constructor.
    assert_eq!(
        run("(function () { var n = 0; class B {} \
             class D extends B { [(n++, 'k')] = 1; } \
             new D(); new D(); return new D().k + ',' + n; })()"),
        "1,1"
    );
}

#[test]
fn super_forwards_a_spread_and_the_arguments_a_written_constructor_chooses() {
    // §13.3.8 through §13.3.7 — a spread in a `super()` has no count until it is iterated, exactly as
    // in any other call, and it goes through the same array-building path. The implicit constructor
    // uses it too, which is why that path had to exist before `extends` could.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B { constructor(list) { super(...list, 9); } } \
             return new D([1, 2, 3]).n; })()"
        ),
        "4"
    );
    // A written constructor may pass whatever it likes, which is the difference from the implicit one.
    assert_eq!(
        run("(function () { class B { constructor(a) { this.a = a; } } \
             class D extends B { constructor(a) { super(a * 2); } } \
             return new D(21).a; })()"),
        "42"
    );
    // A spread whose iterator is exhausted contributes nothing, and `super()` with no arguments is
    // the same call with a count of zero.
    assert_eq!(
        run(
            "(function () { class B { constructor() { this.n = arguments.length; } } \
             class D extends B { constructor() { super(...[]); } } \
             return new D().n; })()"
        ),
        "0"
    );
}

#[test]
fn a_class_may_extend_an_ordinary_function_and_be_called_only_with_new() {
    // §15.7.14 does not require the parent to be a class. An ordinary function is a constructor, so
    // it is a legal heritage, and `super()` constructs it — which is how a subclass of a
    // pre-class-syntax constructor works.
    assert_eq!(
        run("(function () { function B(x) { this.x = x; } \
             B.prototype.m = function () { return this.x; }; \
             class D extends B { constructor() { super(7); } } return new D().m(); })()"),
        "7"
    );
    // …and a derived constructor is still a class constructor, so calling it without `new` is a
    // TypeError before anything in its body runs.
    assert_eq!(
        run("(function () { class B {} class D extends B {} \
             try { D(); return 'no'; } catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn super_reads_from_one_level_above_where_the_method_was_defined() {
    // §9.1.1.3 `GetSuperBase` — the home object's *prototype*, not the home object. A method that
    // read its own home would find itself, and `super.m()` would be infinite recursion rather than a
    // call to the parent.
    assert_eq!(
        run("(function () { class B { m() { return 1; } } \
             class D extends B { m() { return super.m() + 1; } } return new D().m(); })()"),
        "2"
    );
    // Three deep, so the base is where the method was *defined* and not where it was found: `D`'s
    // `m` reads `C`'s however it was reached, which is what makes the chain terminate.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class C extends B { m() { return super.m() + 'C'; } } \
             class D extends C { m() { return super.m() + 'D'; } } return new D().m(); })()"),
        "BCD"
    );
    // A computed key is the same reference with the key evaluated at run time.
    assert_eq!(
        run("(function () { class B {} B.prototype.v = 5; \
             class D extends B { m() { return super['v'] + super['v' + '']; } } \
             return new D().m(); })()"),
        "10"
    );
    // Absent is `undefined` rather than an error, as any read is.
    assert_eq!(
        run(
            "(function () { class B {} class D extends B { m() { return String(super.nothing); } } \
             return new D().m(); })()"
        ),
        "undefined"
    );
    // A base class's method has a home too — its prototype's prototype is `Object.prototype`, so
    // this is not a special case for derived classes.
    assert_eq!(
        run(
            "(function () { class C { m() { return typeof super.hasOwnProperty; } } \
             return new C().m(); })()"
        ),
        "function"
    );
}

#[test]
fn super_keeps_this_as_the_receiver_and_not_the_object_it_looked_on() {
    // §13.3.7.1 — the reference has two objects, and this is the row that tells them apart. The
    // parent's getter is *found* on `B.prototype` and called with the instance, so it can read a
    // field the instance has and the prototype does not. An implementation that passed the base for
    // both would answer `undefined` here and pass every other row in this file.
    assert_eq!(
        run("(function () { class B { get g() { return this.x; } } \
             class D extends B { constructor() { super(); this.x = 7; } read() { return super.g; } } \
             return new D().read(); })()"),
        "7"
    );
    // The same for a method call, which is the common case: `super.m()` is `this.m()` with the
    // lookup started higher.
    assert_eq!(
        run("(function () { class B { m() { return this.tag; } } \
             class D extends B { m() { return super.m(); } } \
             var d = new D(); d.tag = 'inst'; return d.m(); })()"),
        "inst"
    );
    // A getter with no getter half answers `undefined` rather than throwing, which is a different
    // route to the same answer as an absent property.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 'w', { set: function (v) {} }); \
             class D extends B { m() { return String(super.w); } } return new D().m(); })()"),
        "undefined"
    );
}

#[test]
fn a_static_methods_super_reads_the_parent_class_and_not_its_prototype() {
    // §15.7.14 gives a static method the *constructor* as its home, so its super base is the parent
    // constructor — which is how a static method is inherited and overridden. Getting the home wrong
    // here would look for a static on the parent's prototype and find nothing.
    assert_eq!(
        run("(function () { class B { static s() { return 's'; } } \
             class D extends B { static s() { return super.s() + 't'; } } return D.s(); })()"),
        "st"
    );
    assert_eq!(
        run(
            "(function () { class B { static get g() { return 'bg'; } } \
             class D extends B { static read() { return super.g; } } return D.read(); })()"
        ),
        "bg"
    );
}

#[test]
fn super_survives_being_taken_off_the_class_it_was_written_in() {
    // `[[HomeObject]]` is fixed where the method was *written* and has nothing to do with how it is
    // called — which is the whole reason it is a field on the function rather than something derived
    // from `this`. A method borrowed by an unrelated object still reads the original parent.
    assert_eq!(
        run("(function () { class B { m() { return 1; } } \
             class D extends B { m() { return super.m() + 1; } } \
             var taken = new D().m; return taken.call({}); })()"),
        "2"
    );
    // …and an arrow written inside a method reaches the enclosing method's home, because §15.3 gives
    // it none of its own — the same outward reach as `this`, captured at the same moment and in the
    // same field, so the two cannot disagree about which method the arrow was written in.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { m() { var f = () => super.m(); return f() + 'D'; } } \
             return new D().m(); })()"),
        "BD"
    );
    // Two levels deep, where a capture that reached only one would still have passed.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { m() { var f = () => () => super.m(); return f()(); } } \
             return new D().m(); })()"),
        "B"
    );
}

#[test]
fn a_write_through_super_lands_on_the_receiver_and_not_on_the_base() {
    // §13.3.7.1 with `[[Set]]` — the receiver decides where the value goes, so `super.x = 1` makes an
    // own property of the *instance* and leaves the parent prototype alone. That reads oddly and is
    // the same rule an ordinary assignment through a prototype follows.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { m() { super.q = 3; \
                 return this.q + ',' + B.prototype.hasOwnProperty('q') \
                      + ',' + this.hasOwnProperty('q'); } } \
             return new D().m(); })()"),
        "3,false,true"
    );
    // …and the same when the base *does* have the property already, which is the case that says the
    // write is not simply falling through to an ordinary assignment on the base: the parent's
    // property is untouched and the instance shadows it.
    assert_eq!(
        run("(function () { class B {} B.prototype.p = 1; \
             class D extends B { m() { super.p = 2; \
                 return this.p + ',' + B.prototype.p + ',' + this.hasOwnProperty('p'); } } \
             return new D().m(); })()"),
        "2,1,true"
    );
    // An inherited setter is called instead, with `this` as its receiver — so it writes wherever it
    // means to, and nothing is defined on the instance by this instruction.
    assert_eq!(
        run(
            "(function () { class B { set s(v) { this.taken = v * 2; } } \
             class D extends B { m() { super.s = 4; return this.taken; } } \
             return new D().m(); })()"
        ),
        "8"
    );
    // A setter-less accessor and a non-writable data property both refuse the write, silently in
    // sloppy code, which is what an ordinary assignment does too.
    assert_eq!(
        run("(function () { class B {} \
             Object.defineProperty(B.prototype, 'r', { get: function () { return 1; } }); \
             class D extends B { m() { super.r = 9; return String(this.r); } } \
             return new D().m(); })()"),
        "1"
    );
    // The assignment is an expression, so its value is what was written and not what was read back.
    assert_eq!(
        run(
            "(function () { class B {} class D extends B { m() { return (super.z = 5); } } \
             return new D().m(); })()"
        ),
        "5"
    );
}

#[test]
fn an_object_literal_method_has_a_home_and_a_function_written_as_a_value_does_not() {
    // §15.4.5 calls `MakeMethod` for a `MethodDefinition` and not for a property whose value happens
    // to be a function. That is the only difference between the two shapes, and `super` is the only
    // thing that can see it — which is why the parser makes `super` in the second a Syntax Error.
    assert_eq!(
        run("(function () { var parent = { m() { return 'p'; } }; \
             var child = { m() { return super.m() + 'c'; } }; \
             Object.setPrototypeOf(child, parent); return child.m(); })()"),
        "pc"
    );
    assert_eq!(
        run("(function () { var parent = { get g() { return 'pg'; } }; \
             var child = { read() { return super.g; } }; \
             Object.setPrototypeOf(child, parent); return child.read(); })()"),
        "pg"
    );
    // An accessor in a literal is a method definition too, so it has a home.
    assert_eq!(
        run("(function () { var parent = { m() { return 'p'; } }; \
             var child = { get g() { return super.m(); } }; \
             Object.setPrototypeOf(child, parent); return child.g; })()"),
        "p"
    );
}

#[test]
fn super_in_a_class_that_extends_null_refuses_the_read_rather_than_faulting() {
    // §9.1.1.3 — the home object exists and its prototype is `null`, so the base is `null` and the
    // read is a TypeError. Not a fault and not `undefined`: the class was made, and it is the *read*
    // that has nowhere to go.
    assert_eq!(
        run(
            "(function () { class D extends null { m() { return super.anything; } } \
             var d = Object.create(D.prototype); \
             try { d.m(); return 'no'; } catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
}

#[test]
fn deleting_a_property_of_super_is_a_reference_error_after_the_key_has_run() {
    // §13.5.1.1 step 3 — there is no super reference `delete` is legal for, so this is unconditional.
    // It was a *silent* wrong answer the moment `super` began to compile: the reference resolves, and
    // an implementation that let it through would delete a property of the parent prototype.
    assert_eq!(
        run("(function () { class B {} \
             class D extends B { m() { try { delete super.x; return 'no'; } \
                 catch (e) { return e.constructor.name; } } } \
             return new D().m(); })()"),
        "ReferenceError"
    );
    // A run-time throw and not an early error, which is observable: step 1 evaluates the reference,
    // so `ToPropertyKey` has already run its side effect by the time step 3 refuses.
    assert_eq!(
        run("(function () { var order = []; class B {} \
             class D extends B { m() { \
                 try { delete super[(order.push('key'), 'k')]; } \
                 catch (e) { order.push(e.constructor.name); } \
                 return order.join(','); } } \
             return new D().m(); })()"),
        "key,ReferenceError"
    );
    // An ordinary delete is untouched, which is the row that says the refusal is about `super` and
    // not about member deletion.
    assert_eq!(
        run("(function () { var o = { x: 1 }; return delete o.x; })()"),
        "true"
    );
}

#[test]
fn super_reaches_the_right_home_from_every_synthesised_body_in_a_class() {
    // praxis compiles four things as bodies of their own that the specification writes as inline
    // code: a static block, a static field's initialiser, and a derived class's instance field
    // initialisers. Each therefore needs a `[[HomeObject]]` it did not get from being defined on
    // anything, and each needs a *different* one — which is why they are four rows and not one.
    //
    // A static block and a static field belong to the **constructor**, so `super` in either reads the
    // parent class rather than its prototype.
    assert_eq!(
        run("(function () { class B { static s() { return 'S'; } } \
             class D extends B { static { D.got = super.s(); } } return D.got; })()"),
        "S"
    );
    assert_eq!(
        run("(function () { class B { static s() { return 'S'; } } \
             class D extends B { static f = super.s(); } return D.f; })()"),
        "S"
    );
    // An instance field initialiser belongs to the **prototype**, and in a derived class it runs from
    // `super()` inside a body of its own — so it takes the constructor's home rather than being told
    // a prototype it has no way to name from there.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { f = super.m(); } return new D().f; })()"),
        "B"
    );
    // …and an arrow inside such an initialiser reaches through it, which is two captures deep.
    assert_eq!(
        run("(function () { class B { m() { return 'B'; } } \
             class D extends B { f = () => super.m(); } return new D().f(); })()"),
        "B"
    );
    // A *base* class's field initialiser is inline in the constructor, so it uses the constructor's
    // home directly — the same answer by a different path, which is worth pinning separately.
    assert_eq!(
        run("(function () { class C { f = typeof super.hasOwnProperty; } return new C().f; })()"),
        "function"
    );
}
