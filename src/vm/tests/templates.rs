//! §13.2.8 — a template literal, and the two things about it that look like `+` and are not.
//!
//! Checked against V8 first. A template is its components and its substitutions joined in written
//! order, and the only subtle parts are *which* conversion each substitution gets and *when*.

use super::*;

#[test]
fn a_template_is_its_pieces_joined_in_the_order_they_are_written() {
    assert_eq!(run("`hello`"), "hello");
    assert_eq!(run("`a${1}b`"), "a1b");
    assert_eq!(run("`${1}`"), "1");
    assert_eq!(run("`a${1}b${2}c`"), "a1b2c");
    assert_eq!(run("`${1}${2}`"), "12");
    assert_eq!(run("typeof `x`"), "string");
    assert_eq!(run("`${'a'}` === 'a'"), "true");
    // A template with nothing in it is one empty component and no substitutions — the count is
    // always one more than the other, which is what makes the loop that joins them terminate.
    assert_eq!(run("``.length"), "0");
    assert_eq!(run("`a${1}b`.length"), "3");
    // The substitution is a full expression, not just a name.
    assert_eq!(run("`${1 + 2}`"), "3");
    assert_eq!(
        run("(function () { var x = 5; return `x is ${x}`; })()"),
        "x is 5"
    );
    // Escapes are the lexer's and are cooked before the compiler ever sees them; a `$` that is
    // not followed by a brace is an ordinary character, and a brace inside a substitution closes
    // nothing.
    assert_eq!(run("`a\\nb`.length"), "3");
    assert_eq!(run("`a\\tb`.charCodeAt(1)"), "9");
    assert_eq!(run("`\\u0041`"), "A");
    assert_eq!(run("`$`"), "$");
    assert_eq!(run("`${'}'}`"), "}");
}

#[test]
fn a_substitution_is_to_string_and_not_addition() {
    // The row this is all about. §13.2.8.6 specifies `ToString`, which asks an object with the
    // **string** hint and reaches `toString`; `+` asks with the default hint and reaches
    // `valueOf`. An object with both answers differently in the two places, and writing a
    // template as `"" + x` would quietly get this wrong for every such object.
    let both = "{toString: function () { return 'a'; }, valueOf: function () { return 'b'; }}";
    assert_eq!(run(&format!("`${{{both}}}`")), "a");
    assert_eq!(run(&format!("'' + {both}")), "b");
    // The ordinary conversions, which agree either way and are worth pinning so the row above is
    // read as the exception it is.
    assert_eq!(run("`${null}`"), "null");
    assert_eq!(run("`${undefined}`"), "undefined");
    assert_eq!(run("`${true}`"), "true");
    assert_eq!(run("`${[1, 2]}`"), "1,2");
    assert_eq!(run("`${{}}`"), "[object Object]");
    // §7.1.17 throws for a Symbol, and a template is one of the places that reaches it — so a
    // Symbol in a template is a TypeError rather than something unhelpful in the middle of a
    // string. That is the whole reason the conversion throws.
    assert_eq!(
        run("(function () { try { return `${Symbol()}`; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
}

#[test]
fn each_substitution_is_converted_before_the_next_one_is_evaluated() {
    // Not at the end, over a list of collected values. The difference shows twice: a side effect
    // in one `toString` must be visible to the expression after it, and a `toString` that throws
    // must stop the ones that would have followed.
    assert_eq!(
        run(
            "(function () { var r = []; var s = `${r.push(1)}${r.push(2)}`; return r.join(','); })()"
        ),
        "1,2"
    );
    assert_eq!(
        run("(function () { var r = ''; \
             try { `${{toString: function () { r += 'a'; throw new Error('x'); }}}\
             ${(function () { r += 'b'; return 1; })()}`; } catch (e) {} return r; })()"),
        "a"
    );
    // …and each substitution is converted once, not once per place it is joined.
    assert_eq!(
        run(
            "(function () { var n = 0; var o = {toString: function () { n++; return 'z'; }}; \
             `${o}${o}`; return n; })()"
        ),
        "2"
    );
}

#[test]
fn a_tagged_template_calls_its_tag_with_the_template_object_and_the_substitutions() {
    // §13.3.11 — `` f`a${b}c` `` is `f(templateObject, b)`, so a tag with no substitutions still
    // takes one argument and the cooked strings are always one more than the substitutions.
    assert_eq!(
        run(
            "(function () { function tag(s, x, y) { return s.join(',') + '|' + x + '|' + y; } \
             return tag`a${1}b${2}c`; })()"
        ),
        "a,b,c|1|2"
    );
    assert_eq!(
        run(
            "(function () { function tag(s) { return s.length + ',' + arguments.length; } \
             return tag`only`; })()"
        ),
        "1,1"
    );
    // §13.2.8.3 — the cooked strings and a `raw` beside them, which is the whole reason a tag is
    // handed an object rather than a string: `raw` has the escapes exactly as written.
    assert_eq!(
        run(
            "(function () { function tag(s) { return (s[0] === 'a\\nb') + ',' + s.raw[0]; } \
             return tag`a\\nb`; })()"
        ),
        "true,a\\nb"
    );
    // §12.9.6 leaves `TV` **undefined** for an escape that is not one, which only a *tagged* template
    // may contain — an untagged one is a Syntax Error. So `cooked` has a hole and `raw` does not.
    assert_eq!(
        run(
            "(function () { function tag(s) { return String(s[0]) + ',' + s.raw[0]; } \
             return tag`\\unicode`; })()"
        ),
        "undefined,\\unicode"
    );
    // The tag is called as a *method* when it is written as one, exactly as `o.m()` would be.
    assert_eq!(
        run("(function () { var o = { m(s) { return this === o; } }; return o.m`x`; })()"),
        "true"
    );
}

#[test]
fn the_template_object_is_the_same_one_every_time_that_site_is_evaluated() {
    // §13.2.8.3 caches per *Parse Node*, and that identity is the only thing about the object a
    // program can detect which its contents do not already say — it is what lets a tag use it as a key
    // into a table of its own. Building a fresh object each time passes every test about the contents
    // and fails every test about this.
    assert_eq!(
        run("(function () { function tag(s) { return s; } \
             function twice() { return tag`x`; } return twice() === twice(); })()"),
        "true"
    );
    // Two *sites* that happen to spell the same thing are two objects, which is the other half: the
    // key is the site and not the text.
    assert_eq!(
        run("(function () { function tag(s) { return s; } \
             function a() { return tag`x`; } function b() { return tag`x`; } \
             return a() === b(); })()"),
        "false"
    );
    // §13.2.8.3 steps 10 and 11 — both arrays are frozen, so a tag cannot change what a later
    // evaluation of the same site will see. That is what makes handing the same object out safe.
    assert_eq!(
        run(
            "(function () { function tag(s) { return Object.isFrozen(s) + ',' + Object.isFrozen(s.raw); } \
             return tag`x`; })()"
        ),
        "true,true"
    );
    assert_eq!(
        run(
            "(function () { function tag(s) { return s; } function once() { return tag`x`; } \
             var first = once(); first[0] = 'changed'; \
             return once()[0]; })()"
        ),
        "x"
    );
    // …and `raw` is **not enumerable**, which is the one of its three attributes the freeze does not
    // set for it: `Object.keys` of the object is its indices and nothing else.
    assert_eq!(
        run(
            "(function () { function tag(s) { return Object.keys(s).join(',') \
             + '|' + s.propertyIsEnumerable('raw'); } return tag`a${1}b`; })()"
        ),
        "0,1|false"
    );
}

#[test]
fn a_site_hands_out_one_object_and_no_other_site_shares_it_however_the_code_was_compiled() {
    // §13.2.8.3 keys `[[TemplateMap]]` by Parse Node, so the rows are the ways ViperJS makes one: a
    // script, a function, a direct and an indirect `eval`, `new Function`, and a module. Each row asks
    // whether one site gives one object and whether two sites spelled alike give two, and every
    // answer was computed once with V8 (Node 22.22.1). An `eval` of the same text twice is two
    // parses and so two sites, which is the row a cache keyed by *text* would get wrong.
    assert_eq!(
        run(
            "function tag(s) { return s; } function f() { return tag`x`; } var a = []; \
             for (var i = 0; i < 3; i++) a.push(tag`x`); \
             [a[0] === a[1] && a[1] === a[2], f() === f(), a[0] !== f(), tag`x` !== a[0]].join()"
        ),
        "true,true,true,true"
    );
    assert_eq!(
        run(
            "(function () { function tag(s) { return s; } function site() { return tag`x`; } \
             var first = site(); var again = site(); \
             var other = (function () { return tag`x`; })(); \
             return [first === again, first !== other, first[0] === other[0]].join(); })()"
        ),
        "true,true,true"
    );
    assert_eq!(
        run("(function () { function tag(s) { return s; } \
             var g = eval('(function () { return tag`x`; })'); \
             var a = eval('tag`x`'), b = eval('tag`x`'); var loop = []; \
             for (var i = 0; i < 2; i++) loop.push(eval('tag`x`')); \
             return [g() === g(), a !== b, loop[0] !== loop[1], a[0] === b[0]].join(); })()"),
        "true,true,true,true"
    );
    assert_eq!(
        run("function tag(s) { return s; } \
             var g = (0, eval)('(function () { return tag`x`; })'); \
             var a = (0, eval)('tag`x`'), b = (0, eval)('tag`x`'); var loop = []; \
             for (var i = 0; i < 2; i++) loop.push((0, eval)('tag`x`')); \
             [g() === g(), a !== b, loop[0] !== loop[1], a[0] === b[0]].join()"),
        "true,true,true,true"
    );
    assert_eq!(
        run("function tag(s) { return s; } \
             var f = new Function('tag', 'return tag`x`;'); \
             var g = new Function('tag', 'return tag`x`;'); \
             [f(tag) === f(tag), f(tag) !== g(tag), f(tag)[0] === g(tag)[0]].join()"),
        "true,true,true"
    );
    assert_eq!(
        run_module_source(
            "function tag(s) { return s; } function f() { return tag`x`; } var a = []; \
             for (var i = 0; i < 3; i++) a.push(tag`x`); \
             [a[0] === a[1] && a[1] === a[2], f() === f(), a[0] !== f(), tag`x` !== a[0]].join()"
        ),
        "true,true,true,true"
    );
}

#[test]
fn a_site_compiled_where_a_freed_one_was_gets_its_own_object_and_its_own_strings() {
    // The bug this is about. The cache was keyed by the running chunk's **address**, and an `eval`'s
    // chunk is freed when it returns — so the next one, allocated into the same block, was handed
    // the previous site's object: 2,999 of 3,000 `eval`s came back with the wrong strings. Every
    // iteration here drops a direct `eval`, a `new Function` and an indirect `eval`, which is as
    // likely as an allocator gets to reuse an address. The counts are wrong strings, an object shared
    // with the previous iteration's site, and re-evaluations of one site that kept its object.
    // Computed once with V8 (Node 22.22.1).
    assert_eq!(
        run("function tag(s) { return s; } \
             var wrong = 0, shared = 0, same = 0, e, f, i, p, last = []; \
             for (var n = 0; n < 2000; n++) { \
               e = eval('tag`e' + n + '`'); \
               f = new Function('tag', 'return tag`f' + n + '`;'); p = f(tag); \
               i = (0, eval)('tag`i' + n + '`'); \
               if (e[0] !== 'e' + n || e.raw[0] !== 'e' + n || p[0] !== 'f' + n \
                   || i[0] !== 'i' + n) wrong++; \
               if (e === last[0] || p === last[1] || i === last[2]) shared++; \
               if (f(tag) === p) same++; \
               last = [e, p, i]; \
             } \
             [wrong, shared, same].join()"),
        "0,0,2000"
    );
}

#[test]
fn a_host_that_runs_one_script_after_another_gets_each_ones_own_strings() {
    // The same fault from the embedder's side, and the one place reuse is all but certain: each
    // chunk is a local of one loop body, so the next is compiled into the bytes the last one
    // occupied. Keyed by address, the second script was told `first`.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let mut answers = Vec::new();
    for text in ["first", "second", "third"] {
        let source = format!("(function (s) {{ return s[0]; }})`{text}`");
        let script = parse_script(&source).expect("the source parses"); // a VM test needs a chunk
        let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
        answers.push(describe_run(&chunk, &mut vm, &mut heap));
    }
    assert_eq!(answers, ["first", "second", "third"]);
}

/// Compile `source` as a script and run it on a machine that already exists.
fn run_on(source: &str, vm: &mut Vm, heap: &mut Heap) -> (Chunk, String) {
    let script = parse_script(source).expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, heap).expect("the source compiles"); // same
    let answer = describe_run(&chunk, vm, heap);
    (chunk, answer)
}

#[test]
fn the_template_objects_of_code_that_can_never_run_again_are_collected_with_it() {
    // DR-0032. A site's object is held by the code the site is in, so once an `eval` has returned
    // or a `new Function` is unreachable, nothing holds the object either. Measured as the live
    // object count after a collection, which must not depend on how many sites have come and gone:
    // a cache on the machine kept two Arrays a site for ever, six an iteration here.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    run_on(
        "function tag(s) { return s; } \
         function churn(n) { for (var k = 0; k < n; k++) { \
           eval('tag`e' + k + '`'); \
           new Function('tag', 'return tag`f' + k + '`;')(tag); \
           (0, eval)('tag`i' + k + '`'); } }",
        &mut vm,
        &mut heap,
    );
    let mut live = Vec::new();
    for turns in [100, 1000, 3000] {
        let (churn, answer) = run_on(&format!("churn({turns})"), &mut vm, &mut heap);
        assert_eq!(answer, "undefined");
        vm.collect(&churn, &mut heap);
        live.push(heap.object_count());
    }
    assert_eq!(live[0], live[1], "{live:?}");
    assert_eq!(live[1], live[2], "{live:?}");
    // …and a site whose code is still reachable keeps its object through every collection the
    // churn forces, which is the other half: collected *with* its code, not instead of it. Nothing
    // but the code holds the object here — the program has only a `WeakRef` to it — so a chunk
    // that stopped naming what it cached would hand back a swept handle and a dead `WeakRef`.
    vm.set_collection_growth(Some(0));
    let (_, answer) = run_on(
        "var keep = new Function('tag', 'return tag`k`;'); var weak = new WeakRef(keep(tag)); \
         churn(500); keep(tag) === weak.deref()",
        &mut vm,
        &mut heap,
    );
    assert_eq!(answer, "true");
    let (asked, answer) = run_on("keep(tag)[0]", &mut vm, &mut heap);
    assert_eq!(answer, "k");
    vm.collect(&asked, &mut heap);
    let (_, answer) = run_on(
        "keep(tag) === weak.deref() && weak.deref().raw[0]",
        &mut vm,
        &mut heap,
    );
    assert_eq!(answer, "k");
}

#[test]
fn a_program_that_keeps_compiling_new_sites_runs_inside_the_heap_budget() {
    // The budget's side of the same fact: a hundred thousand sites, none kept, under four MiB. A
    // cache that never let go spent it some seventy thousand sites in and threw DR-0013's
    // RangeError. The sum of the strings' lengths was computed once with V8 (Node 22.22.1).
    let mut engine = crate::api::Engine::new();
    engine.set_heap_budget(1 << 22);
    let answer = engine
        .eval(
            "function tag(s) { return s; } var n = 0; \
             for (var k = 0; k < 100000; k++) n += eval('tag`s' + k + '`')[0].length; n",
        )
        .expect("it runs inside the budget");
    assert_eq!(engine.text(answer).as_deref(), Ok("588890"));
}

#[test]
fn one_chunk_run_in_two_realms_hands_each_realm_its_own_object() {
    // §13.2.8.3 step 1 — `[[TemplateMap]]` belongs to the *realm*, so the cache on a chunk is keyed
    // by realm as well as site. No program can reach this today: a function's code runs in the realm
    // that made the function, and `evalScript` compiles afresh. A host can, by running one compiled
    // script in each of its realms, which is what switching the realm underneath the machine does.
    let mut heap = Heap::new();
    let mut vm = Vm::new(&mut heap);
    let first = vm.realm;
    let second = vm.create_realm(&mut heap);
    let script = parse_script("(function (s) { return s; })`x`").expect("the source parses"); // a VM test needs a chunk
    let chunk = compile_script(&script, &mut heap).expect("the source compiles"); // same
    let mut object_in = |realm: crate::realm::Realm, heap: &mut Heap| {
        vm.realm = realm;
        let Ok(Outcome::Value(Value::Object(object))) = vm.run(&chunk, heap) else {
            panic!("the tag hands back the template object");
        };
        object
    };
    let ours = object_in(first, &mut heap);
    let theirs = object_in(second, &mut heap);
    assert_ne!(ours, theirs);
    assert_eq!(object_in(first, &mut heap), ours);
    assert_eq!(object_in(second, &mut heap), theirs);
    // …and each is an Array of the realm it was made in, which is why sharing one would be wrong
    // even where nothing compared the two.
    let prototype = |heap: &Heap, object| heap.object(object).and_then(|held| held.prototype());
    assert_eq!(prototype(&heap, ours), Some(first.array_prototype()));
    assert_eq!(prototype(&heap, theirs), Some(second.array_prototype()));
}
