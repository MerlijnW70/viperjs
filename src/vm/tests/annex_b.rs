//! The three clauses of Annex B praxis implements, which have nothing to do with each other.
//!
//! - **§B.2.2** — `Object.prototype`'s four accessor methods.
//! - **§B.2.3** — `String.prototype`'s thirteen that wrap a string in an HTML tag.
//! - **§B.3.3** — the extra `var` binding a block-level function declaration makes.
//!
//! They are together because DR-0008 is about where the line through Annex B falls and all three
//! are on the near side of it: the first two change no grammar and are conditioned on nothing at
//! all, and the third is conditioned on strictness, which the compiler already knows. See
//! [`crate::compile`]'s `annex_b` for the rules the last one turns on.

use super::*;

#[test]
fn defining_an_accessor_the_old_way_makes_an_enumerable_property() {
    // §B.2.2.1 step 3 — **enumerable and configurable**, where a descriptor with those fields
    // absent gets `false` for both. That is the whole difference from `defineProperty`, and it is
    // visible from `Object.keys` rather than only from a descriptor.
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 1; }); \
             var d = Object.getOwnPropertyDescriptor(o, 'x'); \
             o.x + ',' + d.enumerable + ',' + d.configurable + ',' + (typeof d.get) \
             + ',' + (d.set === undefined)"
        ),
        "1,true,true,function,true"
    );
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 1; }); \
             Object.keys(o).join(',')"
        ),
        "x"
    );
    // …and the contrast, which is what says the attributes are being set rather than defaulted.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'x', {get: function () { return 1; }}); \
             Object.getOwnPropertyDescriptor(o, 'x').enumerable"
        ),
        "false"
    );
    // §B.2.2.2 — the setter half, and it runs.
    assert_eq!(
        run(
            "var o = {}; var seen; o.__defineSetter__('x', function (v) { seen = v; }); \
             o.x = 7; seen + ',' + (o.x === undefined)"
        ),
        "7,true"
    );
    // Both answer `undefined`, which is worth pinning because "returns the object" would be the
    // obvious guess and would let them chain.
    assert_eq!(
        run(
            "var o = {}; typeof o.__defineGetter__('x', function () {}) + ',' \
             + typeof o.__defineSetter__('y', function () {})"
        ),
        "undefined,undefined"
    );
    // Defining one half then the other leaves an accessor with both, because the second call
    // redefines rather than replacing — a configurable property, so it is allowed to.
    assert_eq!(
        run(
            "var o = {}; o.__defineGetter__('x', function () { return 'g'; }); \
             o.__defineSetter__('x', function () {}); \
             var d = Object.getOwnPropertyDescriptor(o, 'x'); \
             o.x + ',' + (typeof d.get) + ',' + (typeof d.set)"
        ),
        "g,function,function"
    );
    // The key goes through `ToPropertyKey`, so a number and a Symbol both work.
    assert_eq!(
        run("var o = {}; o.__defineGetter__(1, function () { return 'one'; }); o[1]"),
        "one"
    );
    assert_eq!(
        run(
            "var o = {}; var s = Symbol('s'); o.__defineGetter__(s, function () { return 'sym'; }); \
             o[s]"
        ),
        "sym"
    );
}

#[test]
fn a_half_that_is_not_a_function_is_refused_before_the_key_is_read() {
    // §B.2.2.1 step 2 comes before the key conversion, so a bad getter is reported as a bad getter
    // even when the key would also have thrown. An engine converting the key first says the wrong
    // thing here, and this is the only row that can tell the two orders apart.
    assert_eq!(
        run(
            "var o = {}; try { o.__defineGetter__({toString: function () { throw new RangeError('key'); }}, 1); } \
             catch (e) { e.constructor.name + ':' + e.message }"
        ),
        "TypeError:the getter is not a function"
    );
    assert_eq!(
        run("var o = {}; try { o.__defineSetter__('x', 1); } catch (e) { e.message }"),
        "the setter is not a function"
    );
    for bad in ["1", "'f'", "undefined", "null", "{}"] {
        assert_eq!(
            run(&format!(
                "var o = {{}}; try {{ o.__defineGetter__('x', {bad}); }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "getter {bad}"
        );
    }
    // A property that cannot be redefined is a TypeError rather than a silent nothing — this is
    // `DefinePropertyOrThrow` and not `[[DefineOwnProperty]]`.
    assert_eq!(
        run(
            "var o = {}; Object.defineProperty(o, 'x', {value: 1, configurable: false}); \
             try { o.__defineGetter__('x', function () {}); } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn looking_one_up_walks_the_chain_and_stops_at_the_first_property_of_any_kind() {
    // §B.2.2.3 step 3 — the walk finds the accessor a program would actually *reach*, which is
    // what makes these different from `getOwnPropertyDescriptor`.
    assert_eq!(
        run("var base = {}; var get = function () { return 1; }; \
             base.__defineGetter__('x', get); \
             var derived = Object.create(base); \
             (derived.__lookupGetter__('x') === get) + ',' \
             + (Object.getOwnPropertyDescriptor(derived, 'x') === undefined)"),
        "true,true"
    );
    // …and it stops at the **first** object that has the property at all. A data property part-way
    // up answers `undefined` rather than being stepped over — because that data property is what
    // the program would reach, and it has no getter.
    //
    // Defined rather than assigned: `middle.x = 2` reaches the inherited accessor, which has no
    // setter, and a sloppy-mode assignment to one of those is silently ignored — so `middle` would
    // have had no own property at all and the row would have proved nothing.
    assert_eq!(
        run(
            "var base = {}; base.__defineGetter__('x', function () { return 1; }); \
             var middle = Object.create(base); \
             Object.defineProperty(middle, 'x', {value: 2}); \
             var derived = Object.create(middle); \
             derived.__lookupGetter__('x') === undefined"
        ),
        "true"
    );
    // An accessor with only a setter answers `undefined` for the getter, and the other way about.
    assert_eq!(
        run(
            "var o = {}; var set = function () {}; o.__defineSetter__('x', set); \
             (o.__lookupGetter__('x') === undefined) + ',' + (o.__lookupSetter__('x') === set)"
        ),
        "true,true"
    );
    // A name nothing in the chain has answers `undefined` rather than throwing, and the walk ends
    // at a prototype of `null` rather than running on.
    assert_eq!(
        run("var bare = Object.create(null); bare.__proto__x = 1; \
             typeof Object.prototype.__lookupGetter__.call(bare, 'nothing')"),
        "undefined"
    );
    assert_eq!(run("typeof ({}).__lookupSetter__('nothing')"), "undefined");
    // The key is converted the same way, so a Symbol is looked up as a Symbol.
    assert_eq!(
        run(
            "var o = {}; var s = Symbol('s'); var get = function () { return 1; }; \
             o.__defineGetter__(s, get); o.__lookupGetter__(s) === get"
        ),
        "true"
    );
    // §10.3.3's `length` and `name` — two arguments for the pair that define, one for the pair
    // that look up.
    assert_eq!(
        run(
            "[Object.prototype.__defineGetter__.length, Object.prototype.__defineSetter__.length, \
             Object.prototype.__lookupGetter__.length, Object.prototype.__lookupSetter__.length] \
             .join(',')"
        ),
        "2,2,1,1"
    );
    // …and they are not enumerable, like every other built-in.
    assert_eq!(
        run("Object.getOwnPropertyDescriptor(Object.prototype, '__defineGetter__').enumerable"),
        "false"
    );
}

#[test]
fn a_block_level_function_also_gets_a_var_binding_in_sloppy_code() {
    // §B.3.3.1 step 3 — the block's binding is copied into the variable scope's when the
    // declaration is *evaluated*, which is what makes the name escape a block it belongs to.
    assert_eq!(
        run("function f() { { function g() { return 'inner' } } return g() } f()"),
        "inner"
    );
    // §B.3.3.2, the same for a script — where the variable scope is the global object rather than
    // a slot, so the copy is a property write. Asserted separately because the two are two
    // different instructions, and getting the second from the first is what a first attempt did
    // wrong: it asked whether the compiler was at the global *scope*, which inside a block is
    // false, and stored into a slot a script does not have.
    assert_eq!(run("{ function g() { return 'script' } } g()"), "script");
    assert_eq!(run("{ function g() {} } typeof g"), "function");
    // §16.1.7 makes it a property with a `var`'s attributes, which is what says the binding was
    // created by `CreateGlobalVarBinding` rather than by an ordinary assignment — one of those
    // would have made it configurable.
    assert_eq!(
        run("{ function g() {} } \
             var d = Object.getOwnPropertyDescriptor(globalThis, 'g'); \
             [d.writable, d.enumerable, d.configurable].join(',')"),
        "true,true,false"
    );
    // Step 2 — the binding exists, holding `undefined`, before the block runs. Both halves are
    // asserted: `undefined` rather than absent, and mutable rather than a dead zone.
    assert_eq!(
        run("function f() { var a = g; g = 1; { function g() {} } return a + ',' + typeof g } f()"),
        "undefined,function"
    );
    assert_eq!(
        run("function f() { var a = typeof g; { function g() {} } return a + ',' + typeof g } f()"),
        "undefined,function"
    );
    // …and the copy happens where the declaration stands rather than when the block is entered,
    // which is the difference between step 3 and hoisting. `h` reads the variable scope's binding
    // from outside the block, so what it sees is what has been copied so far.
    assert_eq!(
        run("function f() { var seen; { seen = h(); function g() {} } \
             function h() { return typeof g } return seen + ',' + typeof g } f()"),
        "undefined,function"
    );
    // §11.2.2 — strict code gets none of it, which is the only condition DR-0008's amendment left.
    assert_eq!(
        run("'use strict'; function f() { { function g() {} } return typeof g } f()"),
        "undefined"
    );
    assert_eq!(
        run("function f() { 'use strict'; { function g() {} } return typeof g } f()"),
        "undefined"
    );
    // A `GeneratorDeclaration` is not a `FunctionDeclaration`, so §B.3.3 does not name it.
    assert_eq!(
        run("function f() { { function* g() {} } return typeof g } f()"),
        "undefined"
    );
    assert_eq!(
        run("function f() { { async function g() {} } return typeof g } f()"),
        "undefined"
    );
}

#[test]
fn the_extension_is_skipped_wherever_a_var_of_that_name_would_not_have_parsed() {
    // §B.3.3.1 step 1.a.ii, which is a hypothetical about a program nobody wrote: the extension
    // applies only where writing `var g` in the declaration's place would have been legal. Every
    // one of these has a lexical binding of `g` between the block and the variable scope, so
    // `var g` would be §14.2.1's second rule and the extension is off.
    for source in [
        "function f() { let g = 1; { function g() {} } return g } f()",
        "function f() { const g = 1; { function g() {} } return g } f()",
        "function f() { { let g = 1; { function g() {} } return g } } f()",
        "function f() { for (let g = 1; ; ) { { function g() {} } return g } } f()",
        "function f() { switch (0) { default: let g = 1; { function g() {} } return g } } f()",
    ] {
        assert_eq!(run(source), "1", "{source}");
    }
    assert_eq!(
        run("function f() { try { throw 0 } catch ({g}) { { function g() {} } } return 1 } f()"),
        "1"
    );
    // …and in every one of them the *outer* name was never created, which is the other half of
    // "skipped": a binding holding `undefined` would be as wrong as one holding the function.
    assert_eq!(
        run(
            "function f() { var seen = typeof g; { let g = 1; { function g() {} } } return seen } f()"
        ),
        "undefined"
    );
    // A **simple** catch parameter does not skip it, and this is the one place the hypothetical
    // needs Annex B to answer itself: §14.15.1 refuses a `var` naming a catch parameter, and
    // B.3.4 puts the `BindingIdentifier` form back. So this pair differs only in the brackets.
    assert_eq!(
        run(
            "function f() { try { throw 0 } catch (g) { { function g() { return 9 } } } \
             return g() } f()"
        ),
        "9"
    );
    // The two conditions stated outside the hypothetical, neither of which is an early error: a
    // parameter name, and `arguments`.
    assert_eq!(
        run("function f(g) { { function g() {} } return g } f(1)"),
        "1"
    );
    assert_eq!(
        run("function f(g = 1) { { function g() {} } return g } f()"),
        "1"
    );
    assert_eq!(
        run("function f([g]) { { function g() {} } return g } f([1])"),
        "1"
    );
    assert_eq!(
        run("function f() { { function arguments() {} } return arguments.length } f(1, 2)"),
        "2"
    );
    // Two declarations of one name in **one** block, which §B.3.3.5 lets parse. Replacing either
    // with `var g` leaves the other lexically declaring `g` in the same list, which is §14.2.1's
    // second rule — relaxed for duplicates and not for this — so neither is eligible and nothing
    // escapes. Every browser answers with the second function instead; see the module doc of
    // `crate::compile`'s `annex_b` for why the letter is what is implemented, and note that no
    // test262 file measures this.
    assert_eq!(
        run(
            "function f() { { function g() { return 1 } function g() { return 2 } } \
             return typeof g } f()"
        ),
        "undefined"
    );
    assert_eq!(
        run("{ function g() { return 1 } function g() { return 2 } } typeof g"),
        "undefined"
    );
    // One of the two being a `let` is still the Syntax Error it always was, which is what says the
    // pair above is about §B.3.3.5's carve-out rather than about duplicates in general — that half
    // never reaches a VM and is
    // `crate::parser::function::tests::a_function_is_var_scoped_at_a_top_level_and_lexical_anywhere_else`.
    //
    // §14.2.1 read against the *block*: the inner declaration of two nested blocks is skipped,
    // because `var g` there would collide with the outer block's own lexical binding of `g`. So
    // the name that escapes is the outer one, which is `nested-blocks-with-fun-decl.js`.
    assert_eq!(
        run(
            "function f() { { function g() { return 1 } { function g() { return 2 } } } \
             return g() } f()"
        ),
        "1"
    );
}

#[test]
fn a_name_the_variable_scope_already_has_gets_no_second_binding() {
    // §B.3.3.1 step 2's note — "a var binding for F is only instantiated here if it is neither a
    // VarDeclaredName nor the name of another FunctionDeclaration". A second binding is not a
    // harmless duplicate: reads would resolve to one of the two and the copy would write the
    // other, which is what the `existing-fn-update` tests see.
    assert_eq!(
        run(
            "function f() { { function g() { return 'inner' } } var seen = g(); \
             function g() { return 'outer' } return seen } f()"
        ),
        "inner"
    );
    assert_eq!(
        run(
            "function f() { { function g() { return 'inner' } } var seen = g(); var g = 1; \
             return seen + ',' + g } f()"
        ),
        "inner,1"
    );
    // …and the binding is not re-initialised to `undefined` either, which a second
    // `CreateMutableBinding` would have done: the top-level function is still there before the
    // block runs.
    assert_eq!(
        run(
            "function f() { var seen = g(); { function g() { return 'inner' } } \
             function g() { return 'outer' } return seen } f()"
        ),
        "outer"
    );
    // Two sibling blocks declaring one name share the binding, and the last one evaluated wins.
    assert_eq!(
        run(
            "function f() { { function g() { return 1 } } { function g() { return 2 } } \
             return g() } f()"
        ),
        "2"
    );
    assert_eq!(
        run(
            "function f() { var seen = typeof g; { function g() {} } { function g() {} } \
             return seen } f()"
        ),
        "undefined"
    );
}

#[test]
fn annex_b_reaches_an_if_clause_a_label_and_a_switch_case() {
    // §B.3.4 — `if ( Expression ) FunctionDeclaration`, "evaluated as if it were
    // `if ( Expression ) { FunctionDeclaration }`". So the block is real, and everything §B.3.3
    // says about a block applies to it.
    assert_eq!(run("if (true) function g() { return 1 } g()"), "1");
    assert_eq!(run("if (false) function g() {} typeof g"), "undefined");
    assert_eq!(
        run("if (false) function g() {} else function h() { return 2 } h()"),
        "2"
    );
    assert_eq!(
        run("function f() { if (true) function g() { return 3 } return g() } f()"),
        "3"
    );
    // The declaration is the block's, so a name a `let` claims is still skipped.
    assert_eq!(
        run("function f() { let g = 1; if (true) function g() {} return g } f()"),
        "1"
    );
    // §B.3.2 — a labelled declaration, which is *not* wrapped: §8.2.12 hands a `LabelledStatement`
    // to `TopLevelVarDeclaredNames`, so one at a body's top level is var-scoped already and needs
    // no extension at all.
    assert_eq!(run("L: function g() { return 4 } g()"), "4");
    assert_eq!(
        run("function f() { L: function g() { return 5 } return g() } f()"),
        "5"
    );
    assert_eq!(
        run("function f() { var seen = typeof g; L: function g() {} return seen } f()"),
        "function",
        "var-scoped and hoisted, where a block's would have been `undefined` here"
    );
    // …and inside a block it is the block's, which is where §B.3.3 has something to do.
    assert_eq!(
        run("function f() { { L: function g() { return 6 } } return g() } f()"),
        "6"
    );
    assert_eq!(
        run("function f() { var seen = typeof g; { L: function g() {} } return seen } f()"),
        "undefined"
    );
    assert_eq!(
        run("function f() { { a: b: function g() { return 7 } } return g() } f()"),
        "7"
    );
    // §14.12.4 step 3 hands the whole `CaseBlock` to `BlockDeclarationInstantiation`, so a
    // declaration in one clause is initialised for all of them — and §B.3.3 names a `CaseClause`
    // and a `DefaultClause` beside a `Block`.
    assert_eq!(
        run("function f() { switch (1) { case 1: return typeof g; case 2: function g() {} } } f()"),
        "function"
    );
    assert_eq!(
        run("function f() { switch (1) { case 1: function g() { return 8 } } return g() } f()"),
        "8"
    );
    assert_eq!(
        run("function f() { switch (1) { default: function g() { return 9 } } return g() } f()"),
        "9"
    );
    assert_eq!(
        run("function f() { let g = 1; switch (1) { case 1: { function g() {} } } return g } f()"),
        "1"
    );
}

#[test]
fn the_extension_reaches_out_of_however_many_scopes_are_in_the_way() {
    // The copy is a store into the variable scope, counted in environments — so every block,
    // every `with` and every loop pass between here and there is one more hop. A depth that is
    // one out does not fail; it writes a different variable.
    assert_eq!(
        run("function f() { { { { function g() { return 1 } } } } return g() } f()"),
        "1"
    );
    assert_eq!(
        run(
            "function f() { { let a = 1; { let b = 2; { function g() { return a + b } } } } \
             return g() } f()"
        ),
        "3"
    );
    assert_eq!(
        run("function f() { with ({}) { { function g() { return 2 } } } return g() } f()"),
        "2"
    );
    assert_eq!(
        run(
            "function f() { for (let i = 0; i < 2; i++) { { function g() { return i } } } \
             return g() } f()"
        ),
        "1"
    );
    assert_eq!(
        run(
            "function f() { try { throw 0 } catch (e) { { function g() { return 3 } } } \
             return g() } f()"
        ),
        "3"
    );
    assert_eq!(
        run("function f() { try {} finally { { function g() { return 4 } } } return g() } f()"),
        "4"
    );
    // …and the same from a script, where the store goes to the global object instead.
    assert_eq!(run("{ { { function g() { return 5 } } } } g()"), "5");
    assert_eq!(run("with ({}) { { function g() { return 6 } } } g()"), "6");
}

#[test]
fn a_direct_eval_gets_the_extension_where_its_variable_scope_can_take_one() {
    // §B.3.3.3's `EvalDeclarationInstantiation`. A sloppy direct eval at the top level of a script
    // has the global object for its variable scope, so the binding goes there and outlives the
    // eval — the same claim `eval("var x = 1")` makes.
    assert_eq!(run("eval('{ function g() { return 1 } }'); g()"), "1");
    assert_eq!(run("eval('{ function g() {} } typeof g')"), "function");
    // A strict eval gets no extension, §B.3.3 being conditioned on sloppiness — asked from inside
    // the eval as well as from outside it. The two rows are not the same claim: a strict eval's
    // variable scope is its own and goes away with it, so the outer row would still say
    // ReferenceError if the binding had been made and discarded. Only the inner one can tell that
    // no binding was made at all.
    assert_eq!(
        run("eval('\\'use strict\\'; { function g() {} } typeof g')"),
        "undefined"
    );
    assert_eq!(
        run(
            "var e = 'none'; eval('\\'use strict\\'; { function g() {} }'); \
             try { g } catch (x) { e = x.constructor.name } e"
        ),
        "ReferenceError"
    );
    // …and an indirect eval is a Script of its own, which is the global path again.
    assert_eq!(run("(0, eval)('{ function g() { return 2 } }'); g()"), "2");
}

#[test]
fn the_thirteen_html_methods_wrap_a_string_in_the_tag_the_clause_names() {
    // §B.2.3, and the tag is not the method's name in nine of the thirteen — which is the whole
    // reason the mapping is a table rather than something derived from the name.
    assert_eq!(run("'x'.big()"), "<big>x</big>");
    assert_eq!(run("'x'.blink()"), "<blink>x</blink>");
    assert_eq!(run("'x'.bold()"), "<b>x</b>");
    assert_eq!(run("'x'.fixed()"), "<tt>x</tt>");
    assert_eq!(run("'x'.italics()"), "<i>x</i>");
    assert_eq!(run("'x'.small()"), "<small>x</small>");
    assert_eq!(run("'x'.strike()"), "<strike>x</strike>");
    assert_eq!(run("'x'.sub()"), "<sub>x</sub>");
    assert_eq!(run("'x'.sup()"), "<sup>x</sup>");
    // The four that take an attribute, two of which share a tag with something else: `anchor` and
    // `link` are both `a`, and `fontcolor` and `fontsize` are both `font`.
    assert_eq!(run("'x'.anchor('n')"), "<a name=\"n\">x</a>");
    assert_eq!(run("'x'.link('u')"), "<a href=\"u\">x</a>");
    assert_eq!(run("'x'.fontcolor('c')"), "<font color=\"c\">x</font>");
    assert_eq!(run("'x'.fontsize(3)"), "<font size=\"3\">x</font>");
    // §B.2.3.2.1 step 2 — the receiver goes through `ToString`, so a number receiver works and a
    // `toString` of the caller's is called.
    assert_eq!(run("String.prototype.big.call(42)"), "<big>42</big>");
    assert_eq!(
        run("String.prototype.anchor.call(42, 42)"),
        "<a name=\"42\">42</a>"
    );
    assert_eq!(
        run("'x'.anchor({toString: function () { return 'q' }})"),
        "<a name=\"q\">x</a>"
    );
    // Step 1's `RequireObjectCoercible`, which is what makes these TypeErrors rather than the
    // string `"undefined"` wrapped in a tag.
    for source in [
        "String.prototype.big.call(undefined)",
        "String.prototype.big.call(null)",
        "String.prototype.anchor.call(undefined)",
        "String.prototype.anchor.call(null, 'n')",
    ] {
        assert_eq!(
            run(&format!(
                "try {{ {source}; 'no error' }} catch (e) {{ e.constructor.name }}"
            )),
            "TypeError",
            "{source}"
        );
    }
}

#[test]
fn only_a_quotation_mark_is_escaped_and_only_in_the_attribute() {
    // §B.2.3.2.1 step 4.b escapes `"` and nothing else, so that a quotation mark cannot close the
    // attribute it is inside. That is the whole of the escaping.
    assert_eq!(
        run("'x'.anchor(String.fromCharCode(34))"),
        "<a name=\"&quot;\">x</a>"
    );
    assert_eq!(
        run("'x'.anchor('a' + String.fromCharCode(34) + 'b' + String.fromCharCode(34) + 'c')"),
        "<a name=\"a&quot;b&quot;c\">x</a>"
    );
    // …and `<`, `&` and `>` are **not** escaped, in the attribute or in the content. The output is
    // not valid HTML and that is what the clause says — a kinder answer here would be a divergence
    // wearing a safety argument, and test262 asserts the bare `<` in three files.
    assert_eq!(run("'<'.big()"), "<big><</big>");
    assert_eq!(run("'<'.anchor('<')"), "<a name=\"<\"><</a>");
    assert_eq!(run("'&'.bold()"), "<b>&</b>");
    assert_eq!(run("'x'.anchor('a&b')"), "<a name=\"a&b\">x</a>");
    // A quotation mark in the *content* is left alone, which is the other half of "only in the
    // attribute" and is what an escape applied to the whole result would get wrong.
    assert_eq!(run("String.fromCharCode(34).big()"), "<big>\"</big>");
}

#[test]
fn the_receiver_is_converted_before_the_attribute_value_is_looked_at() {
    // §B.2.3.2.1's steps are numbered and the order is observable: step 2 converts the receiver
    // and step 4.a the attribute. With both throwing, the receiver's error is the one that
    // escapes — an implementation that read its argument first would answer the other.
    assert_eq!(
        run(
            "var receiver = {toString: function () { throw new RangeError('r') }}; \
             var attribute = {toString: function () { throw new EvalError('a') }}; \
             try { String.prototype.anchor.call(receiver, attribute) } \
             catch (e) { e.constructor.name }"
        ),
        "RangeError"
    );
    // …and with only the attribute throwing, it is the attribute's.
    assert_eq!(
        run(
            "try { ''.anchor({toString: function () { throw new EvalError('a') }}) } \
             catch (e) { e.constructor.name }"
        ),
        "EvalError"
    );
    // A method that takes no attribute never looks at an argument at all, so one that would throw
    // is simply ignored — which is what the empty attribute name in the table decides.
    assert_eq!(
        run("'x'.big({toString: function () { throw new EvalError('a') }})"),
        "<big>x</big>"
    );
}

#[test]
fn each_of_the_thirteen_is_an_ordinary_method_of_string_prototype() {
    // §10.3's shape, which every test262 file for these checks: a `length` of one where an
    // attribute is taken and zero where none is, the method's own name, not enumerable, and not a
    // constructor.
    assert_eq!(
        run("['anchor', 'link', 'fontcolor', 'fontsize'] \
             .map(function (n) { return String.prototype[n].length }).join(',')"),
        "1,1,1,1"
    );
    assert_eq!(
        run(
            "['big', 'blink', 'bold', 'fixed', 'italics', 'small', 'strike', 'sub', 'sup'] \
             .map(function (n) { return String.prototype[n].length }).join(',')"
        ),
        "0,0,0,0,0,0,0,0,0"
    );
    assert_eq!(
        run("String.prototype.bold.name + ',' + String.prototype.fontcolor.name"),
        "bold,fontcolor"
    );
    assert_eq!(
        run(
            "var d = Object.getOwnPropertyDescriptor(String.prototype, 'big'); \
             [d.writable, d.enumerable, d.configurable].join(',')"
        ),
        "true,false,true"
    );
    // §10.3 — a built-in method has no `[[Construct]]`, so `new` is a TypeError rather than an
    // object wrapping a tag.
    assert_eq!(
        run("try { new String.prototype.big(); 'no error' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    // All thirteen are there and are functions, which is the one row that would notice a table
    // entry left out — the reason they are installed from a table at all.
    assert_eq!(
        run(
            "['anchor', 'big', 'blink', 'bold', 'fixed', 'fontcolor', 'fontsize', 'italics', \
              'link', 'small', 'strike', 'sub', 'sup'] \
             .filter(function (n) { return typeof String.prototype[n] === 'function' }).length"
        ),
        "13"
    );
}
