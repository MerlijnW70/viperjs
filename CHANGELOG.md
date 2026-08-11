# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) — with the pre-1.0 caveat that the
public API is not stable and may change in any release.

## [Unreleased]

**86.35% of test262** — 80,443 of 93,161 runs, measured three times on an idle machine against the
suite revision the expectations file names, up from 86.01% at 0.3.0.

### Added

- **`String.prototype.normalize`** — UAX #15's four forms over tables generated from UCD 17.0.0, the
  same edition the identifier, property and case tables pin. Absent until now because the
  decomposition data was missing, and stubbing it would have been a silently wrong answer.
- **`Function.prototype.toString` answers the source text** a function was written as, rather than
  `function f() { [native code] }` for everything. §20.2.3.5 step 2 needs the text to outlive the
  parse, which costs one `Rc<str>` per compilation unit shared by every body compiled out of it. A
  method's text starts at `get`, `set`, `async` or `*` and otherwise at its name, with `static`
  outside it; a class constructor's is the whole class. A built-in, a bound function and a Proxy
  still answer step 3's form.
- **A regular expression reads its pattern as code units** unless `u` or `v` says otherwise, which
  is §22.2.1 and which it did not: a literal astral character became one atom and could never match
  a code unit, so `new RegExp(emoji).test(emoji)` was `false` and a `replace` with one did nothing.
  A lone surrogate in a pattern survives now too — the source had been arriving through a lossy
  conversion — and a group name may hold an astral letter, §22.2.1's `RegExpIdentifierStart`
  admitting a surrogate pair as one identifier character without asking for the flag.
- **Case-insensitive matching past ASCII** — §22.2.2.9's `Canonicalize` was `a`-`z` and nothing
  else, so `/café/i` did not match `CAFÉ` in any script or direction. Both of the clause's branches
  are built: folding under `u` and `v`, uppercasing without them.
- `$262.createRealm`, `$262.agent` and the blocking `Atomics` (0.3.0 shipped the first two; this
  records that `SharedArrayBuffer` and `Atomics.wait`/`notify` now work between engines on separate
  threads — the engine still starts none of its own).

### Fixed

- **A step under `u` is a code point in all four places it is taken.** §22.2.7.8
  `AdvanceStringIndex` was written once and three callers ignored it, so an empty match under `u`
  put its replacement *between* the halves of a surrogate pair, and the matcher's own scan retried
  one unit into a character it had just declined and reported that as a match. Separately, §22.2.7.2
  step 11 runs the matcher over code points, so a `lastIndex` inside a pair names the character
  containing it and the attempt begins at its start.
- **A proxy could report deleting what its target can never regain** — §10.5.10 step 13, the one
  invariant of the eleven that turns on the target's extensibility rather than the property's
  attributes.
- **A `for`-`in` over a primitive enumerated nothing.** §14.7.5.9 step 6's `ToObject` was missing,
  so `for (var k in "ab")` saw no indices and an enumerable property on `Number.prototype` or any
  of the other four was invisible to a `for`-`in` over a primitive of that type.
- **A `finally` that leaves abruptly keeps its own completion value**, and an empty one answers
  `undefined` — §14.15.3 step 3. `1; try { 2 } finally { 3 }` answered 3.
- **A collection reads its iterable one element at a time.** §24.1.1.2 `AddEntriesFromIterable`
  gathered the whole thing first, so `new Map([0, 1])` drew every value before refusing and never
  closed the iterator. The weak collections held a verbatim copy of the same loop and needed the
  same fix; they share one now.
- **`toExponential` took its digits from one spelling of the number and its exponent from another**,
  which put 13 of the first 30 negative powers of ten one place out.
- **A BigInt comparison never ran `ToNumeric`** — §7.2.13 steps 3.d and 3.e — so `1n <= true`,
  `0n <= false` and `1n >= null` were all `false`, and `1n < Symbol()` was `false` where the
  conversion refuses.
- **`Function.prototype.bind` read the property table where §20.2.3.2 says `Get`**, so an accessor
  decided nothing, a throwing getter was swallowed, and a Proxy target never saw its trap. Step
  6.b.i's `ToIntegerOrInfinity` was missing too, so a `length` of 3.7 produced a function object
  whose `length` was 3.7.
- **Which built-in is running was read off its own `name`.** `Int8Array` renamed to
  `"Float64Array"` allocated sixteen bytes per element and deleting the name made every
  construction a RangeError; the six `NativeError` constructors chose §20.5.6.2's intrinsic default
  the same way. Both answer by object identity now.
- **`%TypedArray%.prototype.toString` is `%Array.prototype.toString%`**, not a copy — §23.2.3.32 —
  which a program compares and which the copy got wrong in a second way: it threw where §23.1.3.36
  step 3 falls back to `%Object.prototype.toString%`.
- **`Math[@@toStringTag]`** (§21.3.1.9) and an own **`message`** on each of the six `NativeError`
  prototypes (§20.5.6.3.2), both simply absent.
- **`Symbol` and `BigInt` have a `[[Construct]]` that throws**, rather than none at all — so
  `class A extends Symbol {}` is legal and the exception belongs to the `super` call, which is what
  both clauses say.
- **Four early errors an arrow was let off.** A rest element is one of the `BoundNames`, and the
  duplicate-parameter walk existed twice with one copy stopping at the list — so `(x, ...x) => 1`
  was accepted where `function f(x, ...x) {}` was refused, and `async(...await) => {}` where
  `async(await) => {}` was refused. An arrow's *block* body was treated as a boundary for
  §15.7.9's `ContainsArguments` and is not. And an async arrow's head refuses `await` as a **name**,
  which in a Script is the only thing it ever is.
- **A sloppy direct `eval`'s `var` reaches the caller's scope** (§19.2.1.1), and such a binding is
  the one `delete` may take away.
- **`Array.prototype.reverse` interleaves `HasProperty` and `Get` per end**, and does not read an
  absent index.
- **A class field's initialiser is a function of its own** — DR-0030 — which four bugs were under.
- The nesting bound was lower than real code asks for; eight repositories said so.

### Changed

- **The no-panic invariant has an instrument.** `cargo run --release -p conformance -- --fuzz <n>`
  mutates test262 rather than random bytes, because random bytes are refused in the first few
  characters and measure the same refusal every time. Four mutations: truncation, a flipped code
  unit, a deleted run, and a splice of two files. Only a panic is a failure. The seed reproduces the
  *inputs* and not the run — `Math.random` is seeded from the clock at every `Engine::new`.
- `--summary` writes the badge's JSON, and the conformance workflow reds the build when the
  committed figure has drifted from the one it just measured.
- The per-test budget is 30 seconds, so no entry in the expectations file names a time.

## [0.3.0] — 2026-08-09

**86.01% of test262** — 80,126 of 93,161 runs, measured three times on an idle machine against the
suite revision the expectations file names.

### Added

- **More than one agent — §9.7, `SharedArrayBuffer` and the blocking `Atomics`.** A shared buffer's
  bytes are a `heap::Block` now: one allocation behind an `Arc` that any number of heaps may hold,
  with §25.4.1's waiter list under the same lock, so a blocking `Atomics.wait` compares the slot and
  joins the list in one step rather than in two a notify could land between. §9.7's `[[CanBlock]]` is
  a property of the *agent* and is the host's to answer — `Engine::set_can_block`, `false` by default
  for an engine embedded on its own. `Engine::shared_block` and `Engine::new_shared_buffer` are what
  a host hands the same memory to a second engine with, and `Engine::realm` names the realm to build
  it in.
- **§9.13 `HostPromiseRejectionTracker`, and §27.2.6's `[[PromiseIsHandled]]` with it.**
  `Engine::unhandled_rejections` answers every rejection of the last run that nothing ever asked
  for, and `viper` prints each to standard error as a warning. Usually that is a program's own bug.
  It is also the **only** signal the engine gives when a job drain has stopped early: a refusal
  thrown inside a job is discarded by §9.5 step 3, so the queue empties, the run answers normally and
  the exit status is zero. See DR-0029.
- **`Function.prototype.caller` and `arguments` on a sloppy function**, which ECMA-262 does not
  describe and which the web depends on. Deliberately outside the specification, built on request,
  and bounded: a strict function, an arrow, a method and a class body are untouched, and DR-0028 says
  exactly where it stops and how a program gets the standard behaviour back.
- **A syntax error from `viper` puts a caret under the token it means**, with the line and column.
- `Engine::set_heap_budget`, `Engine::eval_script`, `Engine::bind_namespace` and `Host::number`.
- **A second ECMAScript realm.** `Vm::create_realm` builds a whole new set of §9.3 intrinsics on the
  same heap — its own global, its own `Object.prototype`, its own constructors — and a function now
  records the realm it was made in. §10.1.14 `GetFunctionRealm` answers for a bound function and a
  `Proxy` by recursing into their targets, and a call runs in the **callee's** realm as §10.3.1
  step 3 requires. An embedder wanting a sandbox per tenant wants exactly this. See DR-0025.
- **`atob` and `btoa` on the command line**, the HTML standard's forgiving-base64 pair. Not
  ECMAScript, and here on the same terms as `console`: in the Minimum Common API, pure arithmetic,
  and a real blocker — `entities`, which `htmlparser2` and `cheerio` are built on, decodes its own
  tables with `atob` while it loads.
- The well-known Symbols moved to the heap, so every realm shares one `Symbol.iterator` as
  §6.1.5.1 requires.
- **`examples/agent_loop.rs`** — the embedding surface used as a sandbox for code nobody has read.
  One draft goes in and the loop **patches it from the failures**, converging in four rounds: the
  parser names the delimiter it wanted and the repair is confirmed by parsing again; a `TypeError`
  is repaired by asking the *data* which link no order has; and an interrupt is repaired by finding
  the name the loop condition tests and the body never writes. Shows why `Error` is an enum — each
  case is a different repair — the time and heap budgets that bound an untrusted run, a fresh realm
  per run so an interrupted script cannot poison the next, and the one failure no engine can report:
  a program that runs perfectly and answers the wrong question, which a second draft reaches and the
  loop honestly gives up on.

### Fixed

- **The collector never ran during a job drain, so a promise-driven program leaked everything it
  allocated.** A job runs its handler through a nested execution, and the schedule in the
  interpreter loop is guarded on there being no native re-entry — a correct guard, written for
  `Array.prototype.sort`, which silently covered the whole of §9.5's queue. For a program whose work
  is promises that is the entire run. A chain that re-arms itself — `p.then(step)` from inside
  `step`, which is what every polling loop in JavaScript is — reached the heap budget at 38,174
  turns and threw a `RangeError` **inside a job**, where §9.5 step 3 discards the completion: the
  queue emptied, the run answered normally and the exit status was zero. The check now also happens
  at the boundary between two jobs, which owns every live value for the same reason the guard was
  asking about. See DR-0023's amendment.
- **`Atomics.add`, `and`, `or`, `sub`, `xor`, `exchange` and `compareExchange` were not atomic.**
  Each read, released the lock and wrote back, so two agents could interleave between the two
  halves. Measured on a two-agent CAS-increment loop, 407 of 40,000 increments were lost. They go
  through one read-modify-write under a single lock now. No single-agent test could ever have said
  so, and the symptom was a suite that **hung** rather than a number that was wrong.
- **`Atomics.waitAsync` with a finite timeout settles `"timed-out"`.** DR-0024 recorded this as
  needing a timer the engine does not have and refused three ways to fake one; the question was
  wrong. §25.4.1.6's `TriggerTimeout` only has to run before anything can observe that it has not,
  and a job boundary is such a point. Nothing settles early, nothing spins, and nothing blocks a
  queue that has work in it — measured at 200.29 ms for a 200 ms wait.
- **A strict assignment to an unqualified name the global object refuses now throws.** §9.1.1.4.5
  ends in a `Set` whose `false` a strict store has to turn into a `TypeError`, and the store
  discarded it — so `"use strict"; undefined = 1` succeeded silently against a non-writable
  property.
- **Five of §20.1.3's `ToObject`s were being read as type checks**, so `Object.prototype.valueOf`
  and its neighbours refused a primitive receiver instead of converting it.

- **A TypedArray built from an object no longer swallows the object's own error.** §23.2.5.1 step
  6.b, §23.2.2.1 step 4 and §23.2.3.26 decide between an iterable reading and an array-like one by
  asking `GetMethod(object, @@iterator)` and branching on whether there **is** one; all three tried
  the iterable reading and fell back when it *failed*, catching every error the walk could raise. A
  function has a `length` of 0, so `new Float64Array(obj)` where `obj`'s `@@iterator` is a getter
  that throws built an empty array and discarded the throw. The `@@iterator` method is now read once
  and the branch is on its absence.
- **`%TypedArray%.prototype.set` reads an array-like and never an iterator.** §23.2.3.26.2 is
  `ToObject`, `LengthOfArrayLike` and a loop of `Get` — there is no `GetMethod` in it — so a source
  carrying both a `length` and an `@@iterator` was written from the iterator, which is a wrong value
  rather than a missing error.

- **§10.1.13 `GetPrototypeFromConstructor` now performs a real `Get`.** Every built-in constructor
  read `new.target`'s `prototype` as an own data property, so a throwing accessor there did not
  propagate and a `Proxy`'s `get` trap was never consulted — and the value a non-throwing accessor
  answered was discarded, giving the instance the wrong prototype. Its intrinsic default also comes
  from the constructor's realm now, and is the *concrete* one: a `Float64Array` falls back to
  `Float64Array.prototype` and a `URIError` to `URIError.prototype`.
- **A lexical `for` head binds its names around the expression it iterates**, uninitialised — so
  `let x = 1; for (let x in { x })` is the ReferenceError §14.7.5.6 step 2 asks for.
- `arguments` is refused in a direct `eval` written in a static class field's initialiser (§15.7.1).
- **`Array.of` and `Array.from` build into their `this` when it is a constructor** (§23.1.2.3 step 4
  and §23.1.2.1 step 5), refuse per element with `CreateDataPropertyOrThrow`, and set `length` with
  a throwing write. `Array.from` also closes the iterator when its own mapper or write throws, which
  is only possible because the write happens inside the walk.
- **The eleven arithmetic and bitwise operators convert their left operand whole before touching
  the right** (§13.15.3 steps 3 and 4). `ToNumeric` is `ToPrimitive` *followed by* the numeric
  conversion, so a left operand whose `valueOf` answers a Symbol is refused before the right
  operand's `valueOf` runs at all. `+` keeps step 1's shape, which asks both for a primitive first,
  and the relational operators keep theirs — §7.2.13 compares two Strings lexicographically.
- **An Iterator Helper that stops early reports what closing the source found.** §27.1.4's `every`,
  `find`, `some` and `take` close with a `NormalCompletion`, so §7.4.9 step 4 has nothing to keep
  and a `return` that throws — or a `return` getter that throws — reaches the program. A helper
  whose *callback* throws still discards the close's own trouble, which is the same clause read
  from the other side.
- **`%TypedArray%.prototype.set` reads its buffer again after converting the offset** (§23.2.3.26
  steps 8 and 9). A `valueOf` in the offset may detach or resize the buffer, and everything learned
  before the conversion described a buffer that may no longer exist.
- Three ordering faults that only a real `Get` could expose: `AllocateArrayBuffer`'s length check,
  §23.2.5.1's two branches, and §10.2.2's steps 14 and 15 belonging to the caller.

- **Proper tail calls — §15.10.** A `return` whose argument is a call no longer grows the call
  stack in strict code, so `function f(n) { "use strict"; return n === 0 ? "done" : f(n - 1) }` runs
  at any depth instead of throwing a RangeError at ten thousand. The frame the call is made from is
  taken down before the call and the callee inherits its return target whole, so `frames` gains one
  entry and loses one.

  Where the compiler says it is safe: strict code, not a derived constructor, and no `finally`,
  iterator close or **armed** handler left to cross — so a `catch` block and a `finally` block are
  tail positions and a `try` block is not. Where the machine declines it, because the same body runs
  for `f(1)` and `new f(1)`: a construction, whose frame is holding the object it made, and a
  generator, `async` function or async generator, which answer with something other than what they
  returned. Those degrade to an ordinary call. See DR-0027, and its list of what is deliberately
  left out: a call written as the bare name `eval`, a spread argument, and the tail positions inside
  `?:`, `&&`, `||`, `??` and a comma.

### Changed

- **Breaking: `api::Error::Syntax` is a struct variant carrying a span.** It was
  `Syntax(String)` and is now `Syntax { message: String, span: Span }`, because a host that is
  handed a syntax error almost always wants to point at it — `viper` prints a caret under the
  token, and it could not before. A `match Error::Syntax(said)` becomes
  `match Error::Syntax { message: said, .. }`.
- **A `RegExp` shares its compiled pattern instead of copying it.** Every operation — `exec`,
  `test`, and each of the four `Symbol` methods — deep-copied the whole pattern tree before reading
  a single character. `he.decode`, the HTML entity table under `htmlparser2` and `cheerio`, went
  **776 ms → 97 ms**, which is node to the millisecond.
- **An alternation skips branches the character at hand cannot start.** Each branch carries the
  ASCII code points it may begin with as a bitmap plus one bit for "anything non-ASCII", computed
  when the pattern is parsed and folded on both sides so one summary is right with `i` and without.
  It is a prefilter and never a decision: it may admit a branch that cannot match, never refuse one
  that can. Measured on an alternation the matcher enters, 2,000 branches go 10.60 → 4.70 µs.

- **The conformance suite runs in CI, and the badge is measured rather than typed.** A new
  `conformance` workflow checks out the **exact** test262 revision the expectations file names — a
  conformance number without a suite revision is not a number — runs the suite, and judges the
  ratchet, so a regression from anyone reds the build. `cargo run -p conformance -- --summary <path>`
  writes the headline figure as shields.io endpoint JSON; `conformance/summary.json` is committed and
  the README badge reads it, and the workflow refuses a build whose committed figure has drifted from
  the one it just measured.
- **The default per-test budget is thirty seconds, up from ten.** Four scenarios sat exactly on ten
  and crossed it in both directions between consecutive runs of one unchanged commit; they were the
  only timing-bound entries left in the expectations file and all four pass at thirty. The file now
  contains no entry that names a time, which is what a ratchet has to be before a machine can
  enforce it.

- **An array index is now a kind of property key.** `PropertyKey` gained an `Index(u32)` variant,
  and it is **canonical** — a key is `Index` exactly when §6.1.7 says its spelling is an array
  index, so `a[0]` and `a["0"]` are one key by construction while `a["01"]` stays a named property.
  `a[i]` used to turn the Number into decimal text, encode it to UTF-16 and intern it, then decode
  the units back at the access; both halves are gone. Measured on the same machine and benchmark as
  before: a varying indexed read **412 → 212 ns**, a write **895 → 439**, and a TypedArray element
  read **958 → 217** — the last because §7.1.21 `CanonicalNumericIndexString` was parsing and
  re-formatting a float per element access. See DR-0026.

  Three signatures on the public heap surface changed with it: `PropertyKey::as_string` is replaced
  by `spelling`, `spells`, `describe` and `is_spellable` (an index has no text until something asks
  for it, so spelling one takes a `&mut Heap`); `PropertyKey::to_value` and `Heap::key_value` take
  the heap for the same reason; and `PropertyKey::as_array_index` and `Object::own_property_keys`
  no longer take a heap at all, because the key now knows.

## [0.2.2] — 2026-08-06

### Security

- **A `BigInt` division could panic in the embedder's process** — GHSA-6976-qm5m-7mcj,
  reported privately by [@Zniece](https://github.com/Zniece). `1n / ((1n << 33554399n) * 2n)`
  ended the process with an index out of bounds, which for an engine whose whole promise is that
  no input may panic is the invariant itself failing. Affects every release up to and including
  0.2.1.

  The cause was one comparison against the wrong number: a left shift reserved a limb for what it
  might push off the top, measured *that* against the size ceiling, and trimmed it away afterwards
  — so a magnitude landing exactly on the ceiling was refused for room it never keeps. The refusal
  was then read as impossible by the division and discarded, leaving an empty divisor to be
  indexed at `usize::MAX`.

  Two silent wrong answers came from the same line and are fixed with it: `d % 7n` answered `0n`
  where the true remainder is `1n`, and `String(d)` answered `"0"`. Both now answer correctly.

### Changed

- `BigInt::to_digits` returns a `Result`. A magnitude this engine cannot divide has no decimal
  form, and §6.1.4 requires an implementation that imposes a limit to throw rather than to answer
  something else — so `String(x)` and `x.toString()` raise a RangeError where they used to spell
  `"0"`. Pre-1.0 and breaking, as the note at the top of this file allows.
- A left shift that lands **exactly** on the size ceiling is now allowed, where it was refused for
  the transient limb. `1n << n` therefore succeeds for one more value of `n` than before.


## [0.2.1] — 2026-08-05

### Fixed

- **The crate-level documentation described a crate that no longer exists.** It called the public
  surface "deliberately tiny" directly above the fourteen public modules docs.rs lists, and pointed
  readers at `AGENTS.md` — a file the packaged crate deliberately does not ship, so the reference
  resolved to nothing for anyone arriving from crates.io or docs.rs. It now names [`api`] as the
  entry point, says the wide surface is deliberate, and links only to things inside the crate.

## [0.2.0] — 2026-08-05

The first released version. `0.1.0` was never published; this is what the crate has grown into
since, and the headline is that the engine **runs JavaScript** rather than merely reading it.

### Added

- **Lexer.** Every token form ECMA-262 §12 defines: identifiers over the real Unicode
  `ID_Start`/`ID_Continue` sets, all numeric literals including Annex B's legacy forms and
  `BigInt`, strings and templates as UTF-16 code units, regular-expression literals under a goal
  symbol supplied by the parser, and all four line terminators. Every token knows its exact span,
  and the token spans plus the trivia between them reconstruct the source byte for byte.
- **Parser.** Statements and declarations, the full expression grammar with correct precedence
  and associativity, automatic semicolon insertion, functions, classes with fields, static blocks
  and private names, generators, `async`/`await`, arrow functions, destructuring and the three
  cover grammars, template literals, optional chaining, and modules — each with the early errors
  the spec demands. Errors are values carrying spans, and read like a good compiler's.
- **Values, objects and a garbage collector.** The object model with prototypes, property
  descriptors, getters and setters and the ordinary internal methods; a mark-sweep collector with
  slot reuse behind generation-tagged handles, scheduled by the interpreter itself.
- **A bytecode compiler and interpreter.** Block scoping with the temporal dead zone, closures,
  `this`, `try`/`catch`/`finally`, and the abstract operations spelled out as such — `ToNumber`,
  `ToString`, `ToPrimitive`, `ToPropertyKey`.
- **The ES5 library, complete.** `Object`, `Function`, `Array`, `String`, `Number`, `Boolean`,
  `Math`, `JSON`, `Date`, the `Error` hierarchy, and **our own backtracking `RegExp` engine** —
  no dependency — including named groups, lookbehind, the `u` and `v` flags and Annex B's §B.1.2
  grammar.
- **ES2015 and beyond.** Classes, iterators and generators, `Symbol`, `Map`/`Set`/`WeakMap`/
  `WeakSet`, `Promise` with §9.5's job queue, `Proxy` and `Reflect`, `ArrayBuffer`, `DataView`
  and the TypedArrays including the resizable-buffer semantics, `async` functions and `await`,
  async generators, `BigInt`, and modules — live bindings, namespace objects, `export *` with its
  ambiguity rule, dynamic `import()` and top-level `await`.
- **`eval` in both modes, `with`, and Annex B.** Direct `eval` resolves into the scopes its
  caller is running in; §B.3's block-level function declarations and §B.2's library additions are
  implemented for sloppy code.
- **An embedding surface** (`api.rs`): `Engine` owns the heap and the machine together, host
  functions can be bound from Rust, and a stale handle is refused rather than read as `undefined`.
  `examples/embed.rs` is the tour.
- **An interruptible run.** `Engine::set_time_budget` stops a script that will not stop, and
  because a budget a script can `catch` is not a budget, exceeding it is not a throw.
- **A `viper` command line** — run a file, a string with `-e`, or standard input.
- **A test262 harness** (`conformance/`) with an expectations file that may only shrink.

### Conformance

About **84% of test262** at this release — 78,222 of 93,161 runs. Treat that number as
perishable and re-measure rather than quoting it. The largest remaining gaps are proposals rather
than the standard: `Temporal`, explicit resource management, decorators, and `Atomics` tests
needing multiple agents.

### Guarantees held throughout

- Zero runtime dependencies; the dependency table is empty and checked in CI.
- `#![forbid(unsafe_code)]`, crate-wide.
- No input panics. Nesting is bounded by an explicit count rather than by hitting the OS stack
  guard, and a full-depth parse is asserted to survive one mebibyte of stack.

[Unreleased]: https://github.com/MerlijnW70/viperjs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.3.0
[0.2.2]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.2
[0.2.1]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.1
[0.2.0]: https://github.com/MerlijnW70/viperjs/releases/tag/v0.2.0
