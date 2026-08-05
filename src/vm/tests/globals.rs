//! §19.1 and §19.2 — the properties of the global object that belong to nothing.
//!
//! Checked against V8 first. The rows worth reading are the ones where `parseInt` and `Number`
//! disagree: one asks whether the whole string is a number and the other reads as far as it can.

use super::*;

#[test]
fn the_two_predicates_ask_about_what_the_argument_becomes() {
    assert_eq!(run("isNaN(NaN)"), "true");
    assert_eq!(run("isNaN(12)"), "false");
    assert_eq!(run("isFinite(1)"), "true");
    assert_eq!(run("isFinite(Infinity)"), "false");
    assert_eq!(run("isFinite(NaN)"), "false");
    // §19.2.2 converts first, so these answer about the *number* the argument becomes — which is
    // what makes them differ from `Number.isNaN` and `Number.isFinite` on every non-number.
    assert_eq!(run("isNaN('abc')"), "true");
    assert_eq!(run("isNaN('12')"), "false");
    assert_eq!(run("isNaN(undefined)"), "true");
    assert_eq!(run("isFinite('12')"), "true");
    assert_eq!(run("Number.isNaN('abc')"), "false");
    // §19.1 — `undefined`, `NaN` and `Infinity` are *properties* and not keywords, which is why
    // `typeof undefined` works at all. All three are fixed in place.
    assert_eq!(run("Infinity"), "Infinity");
    assert_eq!(run("typeof undefined"), "undefined");
    assert_eq!(run("isNaN(NaN + 1)"), "true");
}

#[test]
fn parse_int_reads_as_far_as_it_can_where_number_asks_about_the_whole_string() {
    // The distinction the function exists for. A program that reaches for the wrong one is wrong
    // on odd input only, which is the worst place to be wrong.
    assert_eq!(run("parseInt('12abc')"), "12");
    assert_eq!(run("isNaN(Number('12abc'))"), "true");
    assert_eq!(run("parseInt('12')"), "12");
    assert_eq!(run("parseInt('  42  ')"), "42");
    assert_eq!(run("parseInt('-7')"), "-7");
    assert_eq!(run("parseInt('+7')"), "7");
    assert_eq!(run("parseInt(15.99)"), "15");
    // No digits at all is the one failure, and it is a NaN rather than a throw.
    assert_eq!(run("isNaN(parseInt('abc'))"), "true");
    assert_eq!(run("isNaN(parseInt(''))"), "true");
    assert_eq!(run("isNaN(parseInt(null))"), "true");
    // §19.2.5's radix rules, which are three separate ones. Absent means ten *except* that `0x`
    // then means sixteen; an explicit sixteen permits the prefix and does not need it; and every
    // other explicit radix forbids it, which is why `0b11` reads as a single zero.
    assert_eq!(run("parseInt('0x1f')"), "31");
    assert_eq!(run("parseInt('0x1f', 16)"), "31");
    assert_eq!(run("parseInt('1f', 16)"), "31");
    assert_eq!(run("parseInt('10', 2)"), "2");
    assert_eq!(run("parseInt('z', 36)"), "35");
    assert_eq!(run("parseInt('10', 0)"), "10");
    assert_eq!(run("parseInt('0b11')"), "0");
    // Out of range is a NaN and not a quiet fallback to ten — a radix of 37 is a mistake, and
    // answering 10 would hide it.
    assert_eq!(run("isNaN(parseInt('10', 37))"), "true");
    assert_eq!(run("isNaN(parseInt('10', 1))"), "true");
    // …and the radix is `ToInt32`, so one past 2^32 wraps rather than being refused.
    assert_eq!(run("parseInt('10', 4294967298)"), "2");
    // §19.2.5 step 19 rounds the mathematical value *once*. Accumulating digit by digit in an
    // `f64` rounds at every one, and thirty digits is where the two answers part.
    assert_eq!(
        run("parseInt('123456789012345678901234567890')"),
        "1.2345678901234568e+29"
    );
}

#[test]
fn parse_float_takes_the_longest_prefix_that_is_a_decimal_literal() {
    assert_eq!(run("parseFloat('1.5')"), "1.5");
    assert_eq!(run("parseFloat('  3.14xyz')"), "3.14");
    assert_eq!(run("parseFloat('-.5')"), "-0.5");
    assert_eq!(run("parseFloat('.5e2')"), "50");
    assert_eq!(run("parseFloat('1e3')"), "1000");
    // The *longest* prefix, so a second point ends the literal and an `e` with no exponent after
    // it is not part of one.
    assert_eq!(run("parseFloat('1.5.2')"), "1.5");
    assert_eq!(run("parseFloat('1e')"), "1");
    // `Infinity` is a decimal literal, which is the one word-shaped thing this reads.
    assert_eq!(run("parseFloat('Infinity')"), "Infinity");
    assert_eq!(run("parseFloat('-Infinity')"), "-Infinity");
    // …and `0x10` is *not*: a hex literal is a number to `Number` and is not a decimal literal,
    // so this reads the leading zero and stops.
    assert_eq!(run("parseFloat('0x10')"), "0");
    assert_eq!(run("Number('0x10')"), "16");
    assert_eq!(run("isNaN(parseFloat('abc'))"), "true");
    assert_eq!(run("isNaN(parseFloat(''))"), "true");
    // §17 — the `length` each of these declares, which is what a caller is told to supply.
    assert_eq!(run("parseInt.length"), "2");
    assert_eq!(run("parseFloat.length"), "1");
    assert_eq!(run("isNaN.length"), "1");
    assert_eq!(run("typeof parseInt"), "function");
}

#[test]
fn a_function_declaration_may_not_name_a_global_property_it_could_not_write() {
    // §9.1.1.4.16 `CanDeclareGlobalFunction` against §9.1.1.4.15's `var` question, and the pair is
    // the point: a `var` that names an existing property leaves it exactly as it is, where a
    // function declaration has to *put its function in* — so a property it could not write is a
    // property it cannot declare over. §19.1 fixes `NaN` in place, which makes it the case that
    // separates the two.
    assert_eq!(
        run("try { (0, eval)('var NaN;'); 'allowed' } catch (e) { e.constructor.name }"),
        "allowed"
    );
    assert_eq!(
        run("try { (0, eval)('function NaN() {}'); 'allowed' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A property that is configurable may be declared over, whatever else it is.
    assert_eq!(
        run("globalThis.cfgName = 1; \
             try { (0, eval)('function cfgName() {}'); typeof cfgName } catch (e) { e.constructor.name }"),
        "function"
    );
    // …and so may a non-configurable one that is an ordinary, visible data property, because the
    // declaration redefines it in place rather than replacing it. `var` makes exactly that shape.
    assert_eq!(
        run("(0, eval)('var varName;'); \
             try { (0, eval)('function varName() {}'); typeof varName } catch (e) { e.constructor.name }"),
        "function"
    );
    // Configurable is enough **on its own**, and this is the row that says so: a property that is
    // configurable but neither writable nor enumerable is allowed by step 5 and would be refused
    // by step 6. `globalThis.x = 1` cannot show that — it makes a property both tests accept.
    //
    // Asserted as "was it allowed" rather than by reading the binding afterwards, because
    // §9.1.1.4.16's *storage* is a separate gap: it redefines the property where ViperJS still
    // assigns to it, so a non-writable one keeps its old value. That is a bug about
    // `CreateGlobalFunctionBinding` and not about `CanDeclareGlobalFunction`, and a row that
    // conflated the two would go green when either was fixed.
    assert_eq!(
        run("Object.defineProperty(globalThis, 'hiddenCfg', \
             { value: 1, configurable: true, writable: false, enumerable: false }); \
             try { (0, eval)('function hiddenCfg() {}'); 'allowed' } catch (e) { e.constructor.name }"),
        "allowed"
    );
    // …and step 6 needs **both** of its halves. A non-configurable property that is writable but
    // hidden from enumeration is refused, which either half alone would allow.
    assert_eq!(
        run("Object.defineProperty(globalThis, 'hiddenVar', \
             { value: 1, configurable: false, writable: true, enumerable: false }); \
             try { (0, eval)('function hiddenVar() {}'); 'allowed' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // The mirror of it: enumerable but not writable, and also refused.
    assert_eq!(
        run("Object.defineProperty(globalThis, 'frozenVar', \
             { value: 1, configurable: false, writable: false, enumerable: true }); \
             try { (0, eval)('function frozenVar() {}'); 'allowed' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // An accessor is refused, by the same rule and a different route: not configurable, and not a
    // writable data property either.
    assert_eq!(
        run(
            "Object.defineProperty(globalThis, 'accName', { get: function () { return 1; }, configurable: false }); \
             try { (0, eval)('function accName() {}'); 'allowed' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn a_script_that_cannot_declare_one_name_declares_none_of_them() {
    // §16.1.7 asks about every declaration before it creates any, and the order is the whole of
    // what a program can see. `shouldNotBeDefined` precedes the offending declaration in the
    // source, so a check folded into each creation would leave it standing — the global object
    // would be half-instantiated by an operation that threw.
    assert_eq!(
        run(
            "try { (0, eval)('var shouldNotBeDefined; function NaN() {}'); } catch (e) {} \
             typeof Object.getOwnPropertyDescriptor(globalThis, 'shouldNotBeDefined')"
        ),
        "undefined"
    );
    // The same when the refusal is the *first* declaration, so that the check is not merely
    // happening to run early.
    assert_eq!(
        run(
            "try { (0, eval)('function NaN() {} var alsoNotDefined;'); } catch (e) {} \
             typeof Object.getOwnPropertyDescriptor(globalThis, 'alsoNotDefined')"
        ),
        "undefined"
    );
    // …and a script with nothing to refuse still declares everything it named.
    assert_eq!(
        run("(0, eval)('var fine1; function fine2() {}'); \
             typeof fine1 + ',' + typeof fine2"),
        "undefined,function"
    );
}

#[test]
fn a_global_object_that_takes_no_more_properties_refuses_a_new_var() {
    // §9.1.1.4.15's other half, and the only way to refuse a `var` at all: a name that is not
    // there already, on an object that will accept nothing new. Done last in its own script,
    // because it cannot be undone.
    assert_eq!(
        run("Object.preventExtensions(globalThis); \
             try { (0, eval)('var brandNewName;'); 'allowed' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // A `var` naming something the global object *already* has is still allowed, because it
    // creates nothing — which is why the extensibility question never reaches it.
    assert_eq!(
        run(
            "(0, eval)('var already;'); Object.preventExtensions(globalThis); \
             try { (0, eval)('var already;'); 'allowed' } catch (e) { e.constructor.name }"
        ),
        "allowed"
    );
}
