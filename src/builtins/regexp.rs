//! §22.2 — `RegExp`, its prototype, and `RegExpBuiltinExec`.
//!
//! # What a match has to be able to say
//!
//! §22.2.7.2 answers an *Array* with two extra properties, and the shape is load-bearing: index 0
//! is the whole match, later indices are the groups, `index` is where it began, `input` is what was
//! searched and `groups` is an object of the named captures or `undefined` when there are none.
//! Everything in §22.1.3 that takes a pattern reads that shape back, so it is built in one place.
//!
//! # `lastIndex`, and why it is read rather than trusted
//!
//! It is an ordinary writable property. A program may set it to a string, to a number past the end
//! of any subject, or to something with a `valueOf` that changes the regular expression — so
//! §22.2.7.2 reads it with `ToLength` on every call and writes it back on every call, and this does
//! the same rather than keeping a copy.
//!
//! Only a `g` or `y` pattern reads it or writes it. That is why `/a/.exec` finds the same match
//! twice and `/a/g.exec` walks: the flag is what makes the property mean anything.

use super::{create_data_property, define_method, define_value, key};
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, RegExp};
use crate::realm::Realm;
use crate::regexp::{Flags, Matcher, parse};
use crate::value::{Abrupt, Completion, ErrorKind, Value};
use crate::vm::Vm;

/// §22.2.3.1 `RegExpInitialize` — parse the pattern and flags onto `object`.
///
/// Both are converted first and `undefined` means "empty" for either, which is what makes
/// `new RegExp()` a regular expression matching everywhere rather than an error.
fn initialize(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    pattern: Value,
    flags: Value,
) -> Completion<()> {
    let source = match pattern {
        Value::Undefined => Vec::new(),
        given => {
            let text = vm.to_string(given, heap)?;
            heap.string(text).unwrap_or(&[]).to_vec()
        }
    };
    let spelled = match flags {
        Value::Undefined => Vec::new(),
        given => {
            let text = vm.to_string(given, heap)?;
            heap.string(text).unwrap_or(&[]).to_vec()
        }
    };
    let flags = Flags::parse(&String::from_utf16_lossy(&spelled))
        .map_err(|error| Abrupt::Raised(ErrorKind::Syntax, error.message))?;
    let parsed = parse(&String::from_utf16_lossy(&source), flags)
        .map_err(|error| Abrupt::Raised(ErrorKind::Syntax, error.message))?;
    let escaped = escape_source(&source);
    if let Some(found) = heap.object_mut(object) {
        found.set_regexp(RegExp::new(parsed, source.clone(), escaped, flags));
    }
    // §22.2.3.1 step 6 — `lastIndex` is a *property*, writable and neither enumerable nor
    // configurable, and it starts at zero however the object was made.
    let name = key(heap, "lastIndex");
    let _ = heap.define_own_property(
        object,
        name,
        &PropertyDescriptor {
            value: Some(Value::Number(0.0)),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(false),
            ..PropertyDescriptor::EMPTY
        },
    );
    Ok(())
}

/// §22.2.6.13's `EscapeRegExpPattern` — the source in a form that can be read back.
///
/// An empty pattern reads as `(?:)`, because `//` between slashes is a comment and would not parse.
/// A line terminator is escaped for the same reason: `RegExp.prototype.toString` puts this between
/// slashes, and a newline there ends the literal.
fn escape_source(source: &[u16]) -> Vec<u16> {
    if source.is_empty() {
        return "(?:)".encode_utf16().collect();
    }
    let mut out = Vec::with_capacity(source.len());
    let mut escaped = false;
    for unit in source {
        // A `/` that is not already escaped has to be, or the text would close the literal early.
        // One inside a class is safe in the grammar but escaping it changes nothing, so this does
        // not track classes.
        match (*unit, escaped) {
            (0x2F, false) => out.extend_from_slice(&[0x5C, 0x2F]),
            (0x0A, _) => out.extend_from_slice(&[0x5C, u16::from(b'n')]),
            (0x0D, _) => out.extend_from_slice(&[0x5C, u16::from(b'r')]),
            (0x2028, _) => out.extend_from_slice(&"\\u2028".encode_utf16().collect::<Vec<_>>()),
            (0x2029, _) => out.extend_from_slice(&"\\u2029".encode_utf16().collect::<Vec<_>>()),
            _ => out.push(*unit),
        }
        escaped = *unit == 0x5C && !escaped;
    }
    out
}

/// §22.2.4.1 `RegExp(pattern, flags)`.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let pattern = call.argument(0);
    let flags = call.argument(1);
    // Step 1 — a plain call on something that is already a regular expression with the same
    // constructor and no new flags hands the *same object* back. That is why `RegExp(re) === re`,
    // and it is the one constructor in the language that does not always make something.
    if !call.constructing()
        && let Value::Object(given) = pattern
        && heap
            .object(given)
            .and_then(crate::heap::Object::regexp)
            .is_some()
        && matches!(flags, Value::Undefined)
    {
        return Ok(pattern);
    }
    // §22.2.3.1 step 1 — a regular expression argument contributes its *source and flags*, not its
    // string form, so `new RegExp(/a/g)` keeps the `g` and `new RegExp(/a/g, "i")` replaces it.
    let held = match pattern {
        Value::Object(given) => heap
            .object(given)
            .and_then(crate::heap::Object::regexp)
            .map(|found| (found.source().to_vec(), found.flags().spelled())),
        _ => None,
    };
    let (pattern, flags) = match held {
        Some((source, spelled)) => {
            let source = Value::String(heap.intern(&source));
            let carried = match flags {
                Value::Undefined => {
                    Value::String(heap.intern(&spelled.encode_utf16().collect::<Vec<_>>()))
                }
                given => given,
            };
            (source, carried)
        }
        None => (pattern, flags),
    };
    let prototype = super::prototype_from(heap, call, vm.realm().regexp_prototype());
    let object = heap.new_object(Some(prototype));
    initialize(vm, heap, object, pattern, flags)?;
    Ok(Value::Object(object))
}

/// The regular expression a receiver *is*, or the TypeError §22.2.6 gives.
fn this_regexp(heap: &Heap, receiver: Value) -> Completion<ObjectId> {
    match receiver {
        Value::Object(object)
            if heap
                .object(object)
                .and_then(crate::heap::Object::regexp)
                .is_some() =>
        {
            Ok(object)
        }
        _ => Err(Abrupt::type_error(
            "this method requires a regular expression",
        )),
    }
}

/// §22.2.7.2 `RegExpBuiltinExec`.
#[allow(clippy::manual_clamp)] // `clamp` answers NaN for NaN; §7.1.20 says a NaN index is 0
pub(super) fn builtin_exec(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    subject: &[u16],
) -> Completion<Value> {
    let Some(found) = heap.object(object).and_then(crate::heap::Object::regexp) else {
        return Err(Abrupt::type_error("this is not a regular expression"));
    };
    let flags = found.flags();
    // Steps 4 and 5 — `lastIndex` is only consulted when one of the two flags gives it a meaning.
    // Without them a search always starts at zero, which is why `/a/.exec` finds the same match
    // however many times it is called.
    let stateful = flags.global || flags.sticky;
    let name = key(heap, "lastIndex");
    let start = match stateful {
        true => {
            let held = vm.get_property_key(Value::Object(object), name, heap)?;
            // §7.1.20 `ToLength` — a negative or NaN `lastIndex` is zero, and one past what an
            // index can be is clamped rather than refused. A program may write anything here.
            let number = vm.to_number(held, heap)?;
            // §7.1.20 `ToLength` — a negative or NaN `lastIndex` is zero and one past what an
            // index can be is clamped. `max` before `min`, because `f64::max` answers the other
            // operand for NaN: written as a branch it would be a branch whose two sides agree.
            let clamped = number.max(0.0).min(9_007_199_254_740_991.0);
            usize::try_from(clamped as u64).unwrap_or(usize::MAX)
        }
        false => 0,
    };
    let pattern = heap
        .object(object)
        .and_then(crate::heap::Object::regexp)
        .map(|found| found.pattern().clone());
    let Some(pattern) = pattern else {
        return Err(Abrupt::type_error("this is not a regular expression"));
    };
    let found = Matcher::new(&pattern, subject).find(start);
    let Some(found) = found else {
        // Step 6.a.ii — a failed search resets `lastIndex`, so the next call starts over rather
        // than being stuck past the end.
        if stateful {
            super::set_or_throw(vm, heap, object, name, Value::Number(0.0))?;
        }
        return Ok(Value::Null);
    };
    if stateful {
        let end = f64::from(u32::try_from(found.span.1).unwrap_or(u32::MAX));
        super::set_or_throw(vm, heap, object, name, Value::Number(end))?;
    }
    let result = build_result(vm, heap, &pattern, subject, &found)?;
    Ok(result)
}

/// §22.2.7.2 steps 16 to 34 — the Array a match answers, with its four extra properties.
fn build_result(
    vm: &mut Vm,
    heap: &mut Heap,
    pattern: &crate::regexp::Pattern,
    subject: &[u16],
    found: &crate::regexp::Match,
) -> Completion<Value> {
    let realm = vm.realm();
    let array = heap.new_array(realm.array_prototype(), 0);
    let slice = |heap: &mut Heap, span: (usize, usize)| {
        Value::String(heap.intern(&subject[span.0.min(subject.len())..span.1.min(subject.len())]))
    };
    let whole = slice(heap, found.span);
    let zero = heap.index_key(0);
    let _ = heap.define_own_property(array, zero, &PropertyDescriptor::data(whole));
    for (at, capture) in found.captures.iter().enumerate() {
        // A group that did not participate is `undefined` — not the empty string, which is what a
        // group that matched emptily is. Every consumer of a match relies on telling those apart.
        let value = match capture {
            Some(span) => slice(heap, *span),
            None => Value::Undefined,
        };
        let index = u32::try_from(at + 1).unwrap_or(u32::MAX);
        let slot = heap.index_key(index);
        let _ = heap.define_own_property(array, slot, &PropertyDescriptor::data(value));
    }
    let index = f64::from(u32::try_from(found.span.0).unwrap_or(u32::MAX));
    create_data_property(heap, array, "index", Value::Number(index));
    let input = Value::String(heap.intern(subject));
    create_data_property(heap, array, "input", input);
    // Step 34 — `groups` is `undefined` when the pattern has no named groups at all, and an object
    // with a **null** prototype when it has. The null prototype is deliberate: a group called
    // `toString` must not read as `Object.prototype`'s.
    let groups = match pattern.names.is_empty() {
        true => Value::Undefined,
        false => {
            let holder = heap.new_object(None);
            let capture = |number: &u32| {
                usize::try_from(*number)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|index| found.captures.get(index).copied())
                    .flatten()
            };
            for (name, _) in &pattern.names {
                // §22.2.1.1 lets several groups share a name when no match could fill in more than
                // one of them, so the object has **one** property per distinct name — at the
                // position the name is first written, which is the enumeration order §22.2.7.2
                // step 34 produces and a test measures.
                //
                // Nothing here skips the second group of a shared name, and nothing needs to: the
                // value is looked up **by name** across every group wearing it, so a repeated
                // define writes the same value to a key that already holds it and keeps the place
                // it already had. A guard in front of it was a branch no program could tell from
                // its absence, and mutation coverage said so.
                let value = match pattern
                    .names
                    .iter()
                    .filter(|(had, _)| had == name)
                    .find_map(|(_, number)| capture(number))
                {
                    Some(span) => slice(heap, span),
                    None => Value::Undefined,
                };
                let units: Vec<u16> = name.encode_utf16().collect();
                let slot = crate::heap::PropertyKey::from_units(heap, &units);
                let _ = heap.define_own_property(holder, slot, &PropertyDescriptor::data(value));
            }
            Value::Object(holder)
        }
    };
    create_data_property(heap, array, "groups", groups);
    // §22.2.7.2 step 34 — the `d` flag adds one more property, and only then. A pattern without it
    // pays nothing: the array is not built, and `match.indices` is `undefined` rather than absent,
    // which is what `'indices' in match` being false means for every other pattern.
    if pattern.flags.indices {
        let pairs = match_indices(vm, heap, pattern, found);
        create_data_property(heap, array, "indices", pairs);
    }
    Ok(Value::Object(array))
}

/// §22.2.7.8 `MakeMatchIndicesIndexPairArray` — where each capture began and ended.
///
/// The same shape as the match array beside it, and built from the same spans: one element per
/// capture plus the whole match at zero, and a `groups` object when the pattern names any. What
/// differs is what an element *is* — a two-element Array of `[start, end]` rather than the text —
/// so nothing here reads the subject at all.
///
/// A capture that did not participate is `undefined`, exactly as it is in the match array. That is
/// the one thing a caller of this cannot work out for itself: an empty match and an absent one both
/// have a zero-length span, and only the record knows which happened.
fn match_indices(
    vm: &mut Vm,
    heap: &mut Heap,
    pattern: &crate::regexp::Pattern,
    found: &crate::regexp::Match,
) -> Value {
    let realm = vm.realm();
    let array = heap.new_array(realm.array_prototype(), 0);
    // §22.2.7.9 `GetMatchIndexPair` — a two-element Array, and an ordinary one: its prototype is
    // `Array.prototype`, so a script may `map` over it like anything else.
    let mut pair = |heap: &mut Heap, span: (usize, usize)| {
        let made = heap.new_array(realm.array_prototype(), 0);
        for (at, end) in [(0_u32, span.0), (1, span.1)] {
            let slot = heap.index_key(at);
            let value = Value::Number(f64::from(u32::try_from(end).unwrap_or(u32::MAX)));
            let _ = heap.define_own_property(made, slot, &PropertyDescriptor::data(value));
        }
        Value::Object(made)
    };
    let whole = pair(heap, found.span);
    let zero = heap.index_key(0);
    let _ = heap.define_own_property(array, zero, &PropertyDescriptor::data(whole));
    let mut placed: Vec<Value> = Vec::with_capacity(found.captures.len());
    for (at, capture) in found.captures.iter().enumerate() {
        let value = match capture {
            Some(span) => pair(heap, *span),
            None => Value::Undefined,
        };
        placed.push(value);
        let index = u32::try_from(at + 1).unwrap_or(u32::MAX);
        let slot = heap.index_key(index);
        let _ = heap.define_own_property(array, slot, &PropertyDescriptor::data(value));
    }
    // Step 5 and step 6 — `groups` is on the indices array whether or not the pattern names
    // anything, and is `undefined` when it names nothing. So `'groups' in match.indices` is true
    // for every pattern, which is the same promise the match array makes.
    let groups = match pattern.names.is_empty() {
        true => Value::Undefined,
        false => {
            let holder = heap.new_object(None);
            for (name, number) in &pattern.names {
                // §22.2.1.1 lets several groups share a name, so the value is whichever of them
                // took part — the same rule the match array's `groups` follows, and asked the same
                // way rather than a second time.
                let value = pattern
                    .names
                    .iter()
                    .filter(|(had, _)| had == name)
                    .find_map(|(_, number)| {
                        usize::try_from(*number)
                            .ok()
                            .and_then(|n| n.checked_sub(1))
                            .and_then(|index| placed.get(index).copied())
                            .filter(|found| !matches!(found, Value::Undefined))
                    })
                    .unwrap_or(Value::Undefined);
                let _ = number;
                let units: Vec<u16> = name.encode_utf16().collect();
                let slot = crate::heap::PropertyKey::from_units(heap, &units);
                let _ = heap.define_own_property(holder, slot, &PropertyDescriptor::data(value));
            }
            Value::Object(holder)
        }
    };
    create_data_property(heap, array, "groups", groups);
    Value::Object(array)
}

/// §22.2.6.8 `RegExp.prototype.exec`.
fn exec(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_regexp(heap, call.this_value)?;
    let subject = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(subject).unwrap_or(&[]).to_vec();
    builtin_exec(vm, heap, object, &units)
}

/// §22.2.6.16 `RegExp.prototype.test`.
fn test(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_regexp(heap, call.this_value)?;
    let subject = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(subject).unwrap_or(&[]).to_vec();
    let found = builtin_exec(vm, heap, object, &units)?;
    Ok(Value::Boolean(!matches!(found, Value::Null)))
}

/// §B.2.4.1 `RegExp.prototype.compile` — re-initialise a regular expression in place.
///
/// # Why this is here when DR-0008 leaves Annex B out
///
/// DR-0008 is about Annex B's *syntactic* extensions — the grammar praxis will not grow. This is a
/// **built-in**, and a built-in is a property on an object rather than a way of writing a program:
/// leaving it out would refuse a method the web depends on for no reason the decision record gives.
///
/// It is the one thing in the language that changes what a regular expression *is* after it has
/// been made, which is why [`crate::heap::Object::set_regexp`] replaces rather than only sets.
fn compile(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_regexp(heap, call.this_value)?;
    let given = call.argument(0);
    let flags = call.argument(1);
    // Step 2 — a regular expression argument brings its own flags, so a second argument beside it
    // would be two answers to one question and is refused rather than one of them being preferred.
    let held = match given {
        Value::Object(other) => heap
            .object(other)
            .and_then(crate::heap::Object::regexp)
            .map(|found| (found.source().to_vec(), found.flags().spelled())),
        _ => None,
    };
    let (pattern, flags) = match held {
        Some((source, spelled)) => {
            if !matches!(flags, Value::Undefined) {
                return Err(Abrupt::type_error(
                    "compile cannot take flags beside a regular expression",
                ));
            }
            (
                Value::String(heap.intern(&source)),
                Value::String(heap.intern(&spelled.encode_utf16().collect::<Vec<_>>())),
            )
        }
        None => (given, flags),
    };
    initialize(vm, heap, object, pattern, flags)?;
    Ok(call.this_value)
}

/// §22.2.6.13 `RegExp.prototype.source`.
fn source(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Some(object) = regexp_or_prototype(vm, heap, call.this_value)? else {
        // §22.2.6.13 step 3.a — the prototype itself answers `(?:)`, which is an empty
        // non-capturing group: the source text of a pattern that matches the empty string, so
        // that `RegExp.prototype.toString()` is `/(?:)/` and parses back.
        let text: Vec<u16> = "(?:)".encode_utf16().collect();
        return Ok(Value::String(heap.intern(&text)));
    };
    let Some(found) = heap.object(object).and_then(crate::heap::Object::regexp) else {
        return Err(Abrupt::type_error("source requires a regular expression"));
    };
    let text = found.escaped().to_vec();
    Ok(Value::String(heap.intern(&text)))
}

/// §22.2.6.4 `RegExp.prototype.flags`.
///
/// **Reads the eight accessors as properties**, in the order the clause lists them, rather than
/// the flags the receiver was built with. That is not a nicety: step 2 is the only receiver check
/// there is — no `[[OriginalFlags]]` is required — so this works on any object at all, and a
/// subclass overriding `global` is obeyed. Which is also why `RegExp.prototype.flags` is `""`
/// rather than a TypeError: each `Get` reaches the accessor below, which answers `undefined` for
/// the prototype, and eight `undefined`s spell nothing.
fn flags(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 2, and the whole of it.
    if !matches!(call.this_value, Value::Object(_)) {
        return Err(Abrupt::type_error("this method requires an object"));
    }
    let mut spelled: Vec<u16> = Vec::new();
    // Steps 4 to 19, and the order is observable: each `Get` may run a getter that throws or that
    // watches, and `flags/get-order.js` asserts this exact sequence.
    for (name, letter) in [
        ("hasIndices", b'd'),
        ("global", b'g'),
        ("ignoreCase", b'i'),
        ("multiline", b'm'),
        ("dotAll", b's'),
        ("unicode", b'u'),
        ("unicodeSets", b'v'),
        ("sticky", b'y'),
    ] {
        let slot = key(heap, name);
        let held = vm.get_property_key(call.this_value, slot, heap)?;
        if held.to_boolean(heap) {
            spelled.push(u16::from(letter));
        }
    }
    Ok(Value::String(heap.intern(&spelled)))
}

/// The regular expression a receiver is, `None` for `%RegExp.prototype%`, or a TypeError.
///
/// §22.2.6's accessors all share step 3: an object with no `[[OriginalSource]]` is a TypeError
/// **unless it is the prototype itself**, which answers a default instead. The prototype is an
/// ordinary object rather than a regular expression — §22.2.6 says so in as many words, unlike
/// §21.1.3's `Number.prototype` — so without this carve-out reading `RegExp.prototype.source`
/// would throw, and `RegExp.prototype.toString()` with it.
fn regexp_or_prototype(
    vm: &Vm,
    heap: &Heap,
    receiver: Value,
) -> Completion<Option<crate::heap::ObjectId>> {
    let Value::Object(object) = receiver else {
        return Err(Abrupt::type_error(
            "this method requires a regular expression",
        ));
    };
    if heap
        .object(object)
        .and_then(crate::heap::Object::regexp)
        .is_some()
    {
        return Ok(Some(object));
    }
    match object == vm.realm().regexp_prototype() {
        true => Ok(None),
        false => Err(Abrupt::type_error(
            "this method requires a regular expression",
        )),
    }
}

/// §22.2.6.14 `RegExp.prototype.toString`.
fn to_string(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // §22.2.6.14 reads `source` and `flags` as *properties*, so a subclass overriding either is
    // obeyed. That is why this goes through `Get` rather than reading the slots.
    let source_key = key(heap, "source");
    let flags_key = key(heap, "flags");
    let source = vm.get_property_key(call.this_value, source_key, heap)?;
    let source = vm.to_string(source, heap)?;
    let spelled = vm.get_property_key(call.this_value, flags_key, heap)?;
    let spelled = vm.to_string(spelled, heap)?;
    let mut text = vec![0x2F];
    text.extend_from_slice(heap.string(source).unwrap_or(&[]));
    text.push(0x2F);
    text.extend_from_slice(heap.string(spelled).unwrap_or(&[]));
    Ok(Value::String(heap.intern(&text)))
}

/// One of the eight flag accessors — §22.2.6.5 through §22.2.6.18.
///
/// Each is the same question about a different letter, so they share a body and differ in a
/// closure's worth of state that a `fn` pointer cannot hold. Hence eight small functions and one
/// helper, rather than eight copies.
fn flag_of(vm: &Vm, heap: &Heap, receiver: Value, read: fn(Flags) -> bool) -> Completion<Value> {
    let Some(object) = regexp_or_prototype(vm, heap, receiver)? else {
        // §22.2.6.6 step 3.a and its seven siblings — the prototype answers **`undefined`**, and
        // not `false`. The difference is what `RegExp.prototype.flags` is built out of: `undefined`
        // is falsy, so the letter is left out, and a program asking `"global" in RegExp.prototype`
        // still finds the accessor.
        return Ok(Value::Undefined);
    };
    let answer = heap
        .object(object)
        .and_then(crate::heap::Object::regexp)
        .is_some_and(|found| read(found.flags()));
    Ok(Value::Boolean(answer))
}

/// Build `RegExp` and `RegExp.prototype` into `heap`.
pub(super) fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.regexp_prototype();
    let constructor = heap.new_native_constructor(realm.function_prototype(), construct);
    super::define_function_metadata(heap, constructor, "RegExp", 2);
    define_value(heap, global, "RegExp", Value::Object(constructor));
    super::define_fixed(heap, constructor, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(constructor));

    define_method(heap, realm, prototype, "exec", 1, exec);
    define_method(heap, realm, prototype, "test", 1, test);
    define_method(heap, realm, prototype, "toString", 0, to_string);
    define_method(heap, realm, prototype, "compile", 2, compile);

    // The accessors. §22.2.6 makes every one of these a getter with no setter, so `re.global = true`
    // is silently ignored in sloppy code and a TypeError in strict — which is a different thing
    // from a data property that happens to hold a Boolean.
    let accessors: [(&str, crate::heap::Native); 10] = [
        ("source", source),
        ("flags", flags),
        ("hasIndices", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.indices)
        }),
        ("global", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.global)
        }),
        ("ignoreCase", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.ignore_case)
        }),
        ("multiline", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.multiline)
        }),
        ("dotAll", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.dot_all)
        }),
        ("unicode", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.unicode)
        }),
        ("unicodeSets", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.unicode_sets)
        }),
        ("sticky", |vm, heap, call| {
            flag_of(vm, heap, call.this_value, |flags| flags.sticky)
        }),
    ];
    for (name, native) in accessors {
        let getter = heap.new_native_function(realm.function_prototype(), native);
        super::define_function_metadata(heap, getter, &format!("get {name}"), 0);
        let slot = key(heap, name);
        let _ = heap.define_own_property(
            prototype,
            slot,
            &PropertyDescriptor {
                getter: Some(Value::Object(getter)),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}

/// §22.2.3.2 `RegExpCreate` — a regular expression out of a source and flags.
///
/// For §22.1.3's methods, which are given something that is not a pattern and have to make one, and
/// for §22.2.6.14's splitter. A program cannot reach this: `new RegExp` goes through the
/// constructor above so that `new.target` is honoured.
///
/// # Errors
///
/// A `SyntaxError` if the pattern or flags do not parse.
pub(crate) fn make(
    vm: &mut Vm,
    heap: &mut Heap,
    pattern: Value,
    flags: Value,
) -> Completion<ObjectId> {
    let object = heap.new_object(Some(vm.realm().regexp_prototype()));
    initialize(vm, heap, object, pattern, flags)?;
    Ok(object)
}

/// Make a `RegExp` object for a literal — §13.2.7.3's `InstantiateRegExpLiteral`.
///
/// A **new** object every time the literal is evaluated, which is why a regular expression in a
/// loop does not carry `lastIndex` from one turn to the next. That was not always so: ES3 shared
/// one object per literal, and the change is the reason this is a function rather than a constant.
///
/// # Errors
///
/// A `SyntaxError` if the pattern or flags do not parse. The parser accepted the literal's *shape*
/// (§12.9.5) without reading it as a pattern, so this is the first place a bad one is noticed.
pub fn from_literal(
    vm: &mut Vm,
    heap: &mut Heap,
    source: &str,
    spelled: &str,
) -> Completion<ObjectId> {
    let object = heap.new_object(Some(vm.realm().regexp_prototype()));
    let pattern = Value::String(heap.intern(&source.encode_utf16().collect::<Vec<_>>()));
    let flags = Value::String(heap.intern(&spelled.encode_utf16().collect::<Vec<_>>()));
    initialize(vm, heap, object, pattern, flags)?;
    Ok(object)
}
