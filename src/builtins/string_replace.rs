//! §22.1.3's five methods that hand the work to a *pattern* — `replace`, `replaceAll`, `match`,
//! `matchAll` and `search`.
//!
//! # Why these are one module and not five
//!
//! Each begins the same way: look for a well-known Symbol method on the argument and, if there is
//! one, hand the whole operation over to it. That is the extension point regular expressions use —
//! `RegExp.prototype[Symbol.replace]` is where a replacement involving a pattern actually happens —
//! and it is open to anything else that supplies the method. So all five are, in their own right,
//! *delegation*, and only `replace` and `replaceAll` do work of their own after it.
//!
//! # What is here without `RegExp`
//!
//! `replace` and `replaceAll` are complete: a plain string search value needs no pattern engine,
//! and neither does `$&`, `` $` ``, `$'`, `$$` or `$<name>` in the replacement template. `match`,
//! `matchAll` and `search` are complete up to the point where the specification says "make a
//! RegExp out of the argument" — there is nothing to make one with yet, so they refuse there and
//! say so. Everything before that point, including the delegation, is real: an object with a
//! `Symbol.match` method works today.

use super::{key, string};
use crate::heap::{Heap, NativeCall};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §7.3.11 `GetMethod` — a property that must be callable if it is there at all.
///
/// `undefined` **and** null both mean "absent", which is what lets `"a".replace(x, y)` fall through
/// to the string path when `x` has no `Symbol.replace`. Anything else that is not callable is a
/// TypeError rather than a silent fall through, so a misspelled method is reported rather than
/// ignored.
///
/// # Every caller asks "is it an Object" first, and that is a 2025 normative change
///
/// §22.1.3's six pattern-taking methods used to reach here for anything that was neither
/// `undefined` nor null; they now do so only for an **Object**. The difference is a primitive, and
/// it is observable: `GetMethod` on one goes through `ToObject`, so the lookup lands on
/// `Number.prototype` or `String.prototype` — which a script can install a getter on.
/// `"a1b".match(1)` must not call it, and test262 has a file per method per primitive kind saying
/// so. The guard is at each call site rather than here because two of the six do more inside it
/// than call this: `replaceAll` and `matchAll` also run §7.2.8's `IsRegExp`, which looks up
/// `%Symbol.match%` and would reach the same prototype.
pub(super) fn method_of(
    vm: &mut Vm,
    heap: &mut Heap,
    value: Value,
    symbol: &str,
) -> Completion<Option<Value>> {
    // As above: a Symbol the realm has not got names no property, so the answer is the one an
    // absent property gives.
    let found = match vm.realm().well_known(super::well_known_at(symbol)) {
        Some(id) => vm.get_property_key(value, crate::heap::PropertyKey::from_symbol(id), heap)?,
        None => Value::Undefined,
    };
    if matches!(found, Value::Undefined | Value::Null) {
        return Ok(None);
    }
    if !heap.is_callable(found) {
        return Err(Abrupt::type_error(
            "this pattern's method is not a function",
        ));
    }
    Ok(Some(found))
}

/// §7.2.8 `IsRegExp` — whether something claims to be a pattern.
///
/// Asks `Symbol.match` **first** and believes whatever it finds, so an ordinary object saying
/// `{[Symbol.match]: true}` counts as one. That is deliberate in the specification: the question is
/// "does this behave as a pattern", not "was this made by `RegExp`", and `replaceAll` uses the
/// answer to decide whether to demand a `g` flag.
pub(super) fn is_pattern(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<bool> {
    let Value::Object(object) = value else {
        return Ok(false);
    };
    // A well-known Symbol the realm does not have is one nothing can be keyed by, so the lookup
    // answers exactly as an absent property does.
    let matcher = match vm.realm().well_known(super::well_known_at("match")) {
        Some(id) => vm.get_property_key(value, crate::heap::PropertyKey::from_symbol(id), heap)?,
        None => Value::Undefined,
    };
    // Step 3 — the property wins whenever it is **present**, which is not the same as being
    // truthy: a regular expression whose `@@match` has been set to `undefined` falls through to
    // step 4 and is a pattern anyway, while one set to `false` is not.
    if !matches!(matcher, Value::Undefined) {
        return Ok(matcher.to_boolean(heap));
    }
    // Step 4 — a real `[[RegExpMatcher]]`. This was written as `ToBoolean(undefined)` above a
    // comment saying it would "become a real question when `RegExp` arrives"; it had arrived, and
    // the two answers differ for exactly one input, which no test happened to ask about.
    Ok(heap
        .object(object)
        .and_then(crate::heap::Object::regexp)
        .is_some())
}

/// The receiver as characters, after `RequireObjectCoercible` — §22.1.3's opening step.
fn receiver(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Vec<u16>> {
    string::characters(vm, heap, call)
}

/// `StringIndexOf` (§6.1.4.1) — the first position at or after `from` where `needle` sits.
///
/// An **empty** needle is found at `from` itself rather than nowhere, which is what makes
/// `"abc".replaceAll("", "-")` produce `-a-b-c-` and not loop forever: the caller advances past a
/// zero-length match by one unit, and this reports each position exactly once.
fn index_of(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    // No guard for a `from` past the end: the range below is then empty and the search answers
    // `None` of its own accord, which is the same answer one more branch would have given.
    (from..=haystack.len().saturating_sub(needle.len()))
        .find(|at| haystack[*at..].starts_with(needle))
}

/// The named capture groups a match produced, in the order the pattern declares them.
///
/// A name and what it captured, where `None` is a group that did not participate — which reads as
/// the empty string in a replacement and as `undefined` everywhere else. A list rather than a map
/// because §22.2.7.2 builds one per match and the counts here are single digits: a hash of three
/// entries costs more to build than it saves.
type Named<'a> = &'a [(Vec<u16>, Option<Vec<u16>>)];

/// The same list, owned — what reading one back out of a match produces.
type OwnedNamed = Vec<(Vec<u16>, Option<Vec<u16>>)>;

/// Everything §22.2.6.9 reads out of one `exec` result.
///
/// A struct rather than four parameters, because they travel together everywhere: what matched,
/// where it began, its numbered groups and its named ones. Splitting them apart is what made the
/// replacement path take eight arguments and read like a coincidence.
pub(super) struct Found {
    /// What matched.
    pub matched: Vec<u16>,
    /// Where it began, in code units, clamped into the subject.
    pub position: usize,
    /// Each numbered group, `None` for one that did not participate.
    pub captures: Vec<Option<Vec<u16>>>,
    /// The named groups, or `None` when the pattern has none at all.
    pub named: Option<OwnedNamed>,
}

/// §22.1.3.19.1 `GetSubstitution` — a replacement template with its `$` forms filled in.
///
/// `captures` is empty and `named` absent for a string search value: there is nothing to capture
/// without a pattern. Both are taken anyway, because this is the operation `RegExp`'s own
/// `Symbol.replace` will call, and writing it twice is how the two spellings of `$1` come to
/// disagree.
///
/// Anything after `$` that is not one of the forms below is left alone, `$` included: `"$x"`
/// replaces as `$x` and a trailing `$` as `$`. That is a rule about *not* erroring, and it is the
/// part a hand-rolled version usually gets wrong.
pub(super) fn fill_in(
    matched: &[u16],
    string: &[u16],
    position: usize,
    captures: &[Option<Vec<u16>>],
    named: Option<Named<'_>>,
    template: &[u16],
) -> Vec<u16> {
    const DOLLAR: u16 = b'$' as u16;
    let tail = position.saturating_add(matched.len()).min(string.len());
    let mut out = Vec::with_capacity(template.len());
    let mut at = 0;
    while at < template.len() {
        if template[at] != DOLLAR || at + 1 >= template.len() {
            out.push(template[at]);
            at += 1;
            continue;
        }
        let next = template[at + 1];
        match next {
            // `$$` is the only way to write a literal dollar before one of the forms below.
            DOLLAR => {
                out.push(DOLLAR);
                at += 2;
            }
            // `$&` — what matched.
            0x26 => {
                out.extend_from_slice(matched);
                at += 2;
            }
            // `` $` `` — everything before the match.
            0x60 => {
                out.extend_from_slice(&string[..position.min(string.len())]);
                at += 2;
            }
            // `$'` — everything after it.
            0x27 => {
                out.extend_from_slice(&string[tail..]);
                at += 2;
            }
            // `$<name>` — a named capture, and *only* when there are named captures at all. With
            // none, §22.1.3.19.1 leaves the four characters alone rather than reading ahead, so
            // `"a".replace("a", "$<x>")` is the literal `$<x>`.
            0x3C if named.is_some() => {
                let groups = named.unwrap_or(&[]);
                let Some(end) = template[at + 2..].iter().position(|unit| *unit == 0x3E) else {
                    // No closing `>`: the whole rest is literal.
                    out.push(template[at]);
                    at += 1;
                    continue;
                };
                let name = &template[at + 2..at + 2 + end];
                if let Some((_, value)) = groups.iter().find(|(had, _)| had == name) {
                    // A group that did not participate reads as the empty string, not `undefined`.
                    out.extend_from_slice(value.as_deref().unwrap_or(&[]));
                }
                at += 2 + end + 1;
            }
            _ => {
                // `$n` and `$nn` — one or two digits, and the two-digit reading is preferred only
                // when it names a group that exists. `$12` with one capture is capture 1 followed
                // by a literal `2`, which is why this cannot simply take both digits.
                let Some(first) = digit(next) else {
                    out.push(template[at]);
                    at += 1;
                    continue;
                };
                let second = template.get(at + 2).copied().and_then(digit);
                let two = second.map(|low| first * 10 + low);
                let (number, width) = match two {
                    Some(both) if both >= 1 && both as usize <= captures.len() => (both, 3),
                    _ => (first, 2),
                };
                let Some(capture) = usize::from(number)
                    .checked_sub(1)
                    .and_then(|index| captures.get(index))
                else {
                    // `$0`, or a number past the last group: left alone, dollar and all.
                    out.push(template[at]);
                    at += 1;
                    continue;
                };
                out.extend_from_slice(capture.as_deref().unwrap_or(&[]));
                at += width;
            }
        }
    }
    out
}

/// A decimal digit's value, for the `$n` forms.
fn digit(unit: u16) -> Option<u8> {
    (0x30..=0x39)
        .contains(&unit)
        .then(|| u8::try_from(unit - 0x30).unwrap_or(0))
}

/// One replacement's text — a function's answer, or the template with its `$` forms filled in.
fn replacement(
    vm: &mut Vm,
    heap: &mut Heap,
    with: Value,
    matched: &[u16],
    string: &[u16],
    position: usize,
) -> Completion<Vec<u16>> {
    if heap.is_callable(with) {
        // §22.1.3.19 step 14.a — the function is handed the match, where it was, and the whole
        // string, and its answer is used as-is: no `$` form is interpreted in it.
        let text = heap.intern(matched);
        let whole = heap.intern(string);
        let position = f64::from(u32::try_from(position).unwrap_or(u32::MAX));
        let answered = vm.call_value(
            with,
            Value::Undefined,
            &[
                Value::String(text),
                Value::Number(position),
                Value::String(whole),
            ],
            heap,
        )?;
        let text = vm.to_string(answered, heap)?;
        return Ok(heap.string(text).unwrap_or(&[]).to_vec());
    }
    let template = vm.to_string(with, heap)?;
    let template = heap.string(template).unwrap_or(&[]).to_vec();
    Ok(fill_in(matched, string, position, &[], None, &template))
}

/// The four things §22.2.6.9 reads back out of an `exec` result.
///
/// A match is an *Array* with extra properties, and every one of them is read with `Get` — so an
/// overriding `exec` that answers a hand-made object works, which is the whole reason the shape is
/// a convention rather than a slot.
pub(super) fn parts_of(
    vm: &mut Vm,
    heap: &mut Heap,
    result: crate::heap::ObjectId,
    subject: &[u16],
) -> Completion<Found> {
    let zero = heap.index_key(0);
    let matched = vm.get_property_key(Value::Object(result), zero, heap)?;
    let matched = vm.to_string(matched, heap)?;
    let matched = heap.string(matched).unwrap_or(&[]).to_vec();
    let index = key(heap, "index");
    let position = vm.get_property_key(Value::Object(result), index, heap)?;
    let position = vm.to_number(position, heap)?;
    // §22.2.6.9 step 12 clamps the position into the subject, because an overriding `exec` may
    // answer any number at all and the slicing below must not be asked to reach past the end.
    let position = usize::try_from(position.max(0.0) as u64)
        .unwrap_or(usize::MAX)
        .min(subject.len());
    let length = key(heap, "length");
    let count = vm.get_property_key(Value::Object(result), length, heap)?;
    let count = vm.to_number(count, heap)?;
    let count = (count.max(1.0) as u64).min(1000) as u32;
    let mut captures = Vec::new();
    for at in 1..count {
        let slot = heap.index_key(at);
        let held = vm.get_property_key(Value::Object(result), slot, heap)?;
        captures.push(match held {
            Value::Undefined => None,
            given => {
                let text = vm.to_string(given, heap)?;
                Some(heap.string(text).unwrap_or(&[]).to_vec())
            }
        });
    }
    let groups_key = key(heap, "groups");
    let groups = vm.get_property_key(Value::Object(result), groups_key, heap)?;
    let named = match groups {
        Value::Object(holder) => {
            let mut listed = Vec::new();
            for found in vm.own_keys_through(holder, heap)? {
                let Some(text) = found.as_string() else {
                    continue;
                };
                let name = heap.string(text).unwrap_or(&[]).to_vec();
                let held = vm.get_property_key(Value::Object(holder), found, heap)?;
                let value = match held {
                    Value::Undefined => None,
                    given => {
                        let text = vm.to_string(given, heap)?;
                        Some(heap.string(text).unwrap_or(&[]).to_vec())
                    }
                };
                listed.push((name, value));
            }
            Some(listed)
        }
        _ => None,
    };
    Ok(Found {
        matched,
        position,
        captures,
        named,
    })
}

/// §22.2.6.9 step 14.l — a replacement *function*, handed the match, its groups, where it was, the
/// subject, and the named groups as an object when there are any.
pub(super) fn from_function(
    vm: &mut Vm,
    heap: &mut Heap,
    with: Value,
    found: &Found,
    subject: &[u16],
) -> Completion<Vec<u16>> {
    let mut arguments = vec![Value::String(heap.intern(&found.matched))];
    for capture in &found.captures {
        arguments.push(match capture {
            Some(text) => Value::String(heap.intern(text)),
            None => Value::Undefined,
        });
    }
    arguments.push(Value::Number(f64::from(
        u32::try_from(found.position).unwrap_or(u32::MAX),
    )));
    arguments.push(Value::String(heap.intern(subject)));
    // The named groups arrive as a *last* argument and only when the pattern has any, which is why
    // a function written for a pattern without them sees the arity it expects.
    if let Some(listed) = &found.named {
        let holder = heap.new_object(Some(vm.realm().object_prototype()));
        for (name, value) in listed.clone() {
            let slot = crate::heap::PropertyKey::from_units(heap, &name);
            let held = match value {
                Some(text) => Value::String(heap.intern(&text)),
                None => Value::Undefined,
            };
            let _ = heap.define_own_property(
                holder,
                slot,
                &crate::heap::PropertyDescriptor::data(held),
            );
        }
        arguments.push(Value::Object(holder));
    }
    let answered = vm.call_value(with, Value::Undefined, &arguments, heap)?;
    let text = vm.to_string(answered, heap)?;
    Ok(heap.string(text).unwrap_or(&[]).to_vec())
}

/// §22.1.3.19 `String.prototype.replace`.
fn replace(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `RequireObjectCoercible` comes **before** the pattern is asked for its Symbol, so
    // a nullish receiver is refused whatever the argument is. The conversion to a String stays
    // where it was, below the dispatch, because the clause does not run it on the handed-over path.
    string::require_coercible(call.this_value)?;
    let pattern = call.argument(0);
    let with = call.argument(1);
    // Step 2 — the Symbol method is looked for **before** the receiver is converted, so a pattern
    // that handles the whole operation sees an unconverted `this`.
    if matches!(pattern, Value::Object(_))
        && let Some(replacer) = method_of(vm, heap, pattern, "replace")?
    {
        return vm.call_value(replacer, pattern, &[call.this_value, with], heap);
    }
    let string = receiver(vm, heap, call)?;
    let needle = vm.to_string(pattern, heap)?;
    let needle = heap.string(needle).unwrap_or(&[]).to_vec();
    // Step 6 — a non-callable replacement is converted to a String *now*, before the search, which
    // is observable when it has a `toString` that throws.
    let with = match heap.is_callable(with) {
        true => with,
        false => Value::String(vm.to_string(with, heap)?),
    };
    let Some(position) = index_of(&string, &needle, 0) else {
        return Ok(Value::String(heap.intern(&string)));
    };
    let text = replacement(vm, heap, with, &needle, &string, position)?;
    let mut built = string[..position].to_vec();
    built.extend_from_slice(&text);
    built.extend_from_slice(&string[position + needle.len()..]);
    Ok(Value::String(heap.intern(&built)))
}

/// §22.1.3.20 `String.prototype.replaceAll`.
fn replace_all(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `RequireObjectCoercible` comes **before** the pattern is asked for its Symbol, so
    // a nullish receiver is refused whatever the argument is. The conversion to a String stays
    // where it was, below the dispatch, because the clause does not run it on the handed-over path.
    string::require_coercible(call.this_value)?;
    let pattern = call.argument(0);
    let with = call.argument(1);
    if matches!(pattern, Value::Object(_)) {
        // Step 2.b — a pattern that is *not* global is refused outright, because replacing all of
        // something with a pattern that stops at the first match could not do what was asked. The
        // check is here rather than in the delegate so that it happens whatever the delegate does.
        if is_pattern(vm, heap, pattern)? {
            let flags = vm.get_property_key(pattern, key(heap, "flags"), heap)?;
            if matches!(flags, Value::Undefined | Value::Null) {
                return Err(Abrupt::type_error(
                    "a pattern given to replaceAll must have flags",
                ));
            }
            let spelled = vm.to_string(flags, heap)?;
            if !heap
                .string(spelled)
                .unwrap_or(&[])
                .contains(&u16::from(b'g'))
            {
                return Err(Abrupt::type_error(
                    "a pattern given to replaceAll must be global",
                ));
            }
        }
        if let Some(replacer) = method_of(vm, heap, pattern, "replace")? {
            return vm.call_value(replacer, pattern, &[call.this_value, with], heap);
        }
    }
    let string = receiver(vm, heap, call)?;
    let needle = vm.to_string(pattern, heap)?;
    let needle = heap.string(needle).unwrap_or(&[]).to_vec();
    let with = match heap.is_callable(with) {
        true => with,
        false => Value::String(vm.to_string(with, heap)?),
    };
    // Step 11's advance — one past the match, or one *unit* when the match was empty. Without the
    // second, an empty needle would be found at the same position forever.
    let step = needle.len().max(1);
    let mut built = Vec::with_capacity(string.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(position) = index_of(&string, &needle, from) {
        let text = replacement(vm, heap, with, &needle, &string, position)?;
        built.extend_from_slice(&string[copied..position]);
        built.extend_from_slice(&text);
        copied = position + needle.len();
        from = position + step;
    }
    built.extend_from_slice(&string[copied.min(string.len())..]);
    Ok(Value::String(heap.intern(&built)))
}

/// The three that have nothing to do once there is no Symbol method — §22.1.3.14, .15 and .21.
///
/// Each ends "let `rx` be `RegExpCreate(argument)`, and invoke its Symbol method". There is nothing
/// to create one with, so this refuses and says which method wanted it, rather than answering
/// something wrong. Everything before it is real: the delegation above is what a pattern uses.
fn pattern_from(vm: &mut Vm, heap: &mut Heap, given: Value) -> Completion<crate::heap::ObjectId> {
    let empty = Value::String(heap.intern(&[]));
    let source = match given {
        Value::Undefined => empty,
        held => held,
    };
    super::regexp::make(vm, heap, source, Value::Undefined)
}

/// §22.1.3.14 `String.prototype.match`.
fn string_match(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `RequireObjectCoercible` comes **before** the pattern is asked for its Symbol, so
    // a nullish receiver is refused whatever the argument is. The conversion to a String stays
    // where it was, below the dispatch, because the clause does not run it on the handed-over path.
    string::require_coercible(call.this_value)?;
    let pattern = call.argument(0);
    if matches!(pattern, Value::Object(_))
        && let Some(matcher) = method_of(vm, heap, pattern, "match")?
    {
        return vm.call_value(matcher, pattern, &[call.this_value], heap);
    }
    let subject = receiver(vm, heap, call)?;
    // Step 4 — `RegExpCreate(regexp, undefined)`, then its own `Symbol.match`. So `"ab".match("b")`
    // makes a pattern out of the string rather than searching for it as text.
    let made = pattern_from(vm, heap, pattern)?;
    invoke_symbol(vm, heap, made, "match", &subject, None)
}

/// Call a well-known Symbol method on a freshly made pattern — the last step of all three.
fn invoke_symbol(
    vm: &mut Vm,
    heap: &mut Heap,
    pattern: crate::heap::ObjectId,
    symbol: &str,
    subject: &[u16],
    extra: Option<Value>,
) -> Completion<Value> {
    let Some(method) = method_of(vm, heap, Value::Object(pattern), symbol)? else {
        return Err(Abrupt::type_error("a pattern is missing its method"));
    };
    let text = Value::String(heap.intern(subject));
    let mut arguments = vec![text];
    if let Some(given) = extra {
        arguments.push(given);
    }
    vm.call_value(method, Value::Object(pattern), &arguments, heap)
}

/// §22.1.3.15 `String.prototype.matchAll`.
fn match_all(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `RequireObjectCoercible` comes **before** the pattern is asked for its Symbol, so
    // a nullish receiver is refused whatever the argument is. The conversion to a String stays
    // where it was, below the dispatch, because the clause does not run it on the handed-over path.
    string::require_coercible(call.this_value)?;
    let pattern = call.argument(0);
    if matches!(pattern, Value::Object(_)) {
        // Step 2.b — the same global-flag demand `replaceAll` makes, and for the same reason:
        // iterating every match with a pattern that stops at the first is not a thing to allow
        // quietly.
        if is_pattern(vm, heap, pattern)? {
            let flags = vm.get_property_key(pattern, key(heap, "flags"), heap)?;
            if matches!(flags, Value::Undefined | Value::Null) {
                return Err(Abrupt::type_error(
                    "a pattern given to matchAll must have flags",
                ));
            }
            let spelled = vm.to_string(flags, heap)?;
            if !heap
                .string(spelled)
                .unwrap_or(&[])
                .contains(&u16::from(b'g'))
            {
                return Err(Abrupt::type_error(
                    "a pattern given to matchAll must be global",
                ));
            }
        }
        if let Some(matcher) = method_of(vm, heap, pattern, "matchAll")? {
            return vm.call_value(matcher, pattern, &[call.this_value], heap);
        }
    }
    let subject = receiver(vm, heap, call)?;
    // Step 3.c — the pattern this one makes is **global**, whatever it was given, because
    // iterating every match is what the method is for.
    let source = match pattern {
        Value::Undefined => Value::String(heap.intern(&[])),
        held => held,
    };
    let global = Value::String(heap.intern(&[u16::from(b'g')]));
    let made = super::regexp::make(vm, heap, source, global)?;
    invoke_symbol(vm, heap, made, "matchAll", &subject, None)
}

/// §22.1.3.21 `String.prototype.search`.
fn search(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 1 — `RequireObjectCoercible` comes **before** the pattern is asked for its Symbol, so
    // a nullish receiver is refused whatever the argument is. The conversion to a String stays
    // where it was, below the dispatch, because the clause does not run it on the handed-over path.
    string::require_coercible(call.this_value)?;
    let pattern = call.argument(0);
    if matches!(pattern, Value::Object(_))
        && let Some(searcher) = method_of(vm, heap, pattern, "search")?
    {
        return vm.call_value(searcher, pattern, &[call.this_value], heap);
    }
    let subject = receiver(vm, heap, call)?;
    let made = pattern_from(vm, heap, pattern)?;
    invoke_symbol(vm, heap, made, "search", &subject, None)
}

/// Every method this module defines, with the `length` §22.1.3 gives it.
pub(super) const METHODS: [(&str, u32, crate::heap::Native); 5] = [
    ("match", 1, string_match),
    ("matchAll", 1, match_all),
    ("replace", 2, replace),
    ("replaceAll", 2, replace_all),
    ("search", 1, search),
];

#[cfg(test)]
mod pieces {
    use super::{fill_in, index_of};

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn filled(matched: &str, string: &str, position: usize, template: &str) -> String {
        String::from_utf16_lossy(&fill_in(
            &units(matched),
            &units(string),
            position,
            &[],
            None,
            &units(template),
        ))
    }

    #[test]
    fn an_empty_needle_is_found_at_every_position_including_the_end() {
        // §6.1.4.1 — and it is why `replaceAll` advances by one *unit* rather than by the match:
        // reporting the same position twice would not terminate.
        assert_eq!(index_of(&units("abc"), &units(""), 0), Some(0));
        assert_eq!(index_of(&units("abc"), &units(""), 3), Some(3));
        assert_eq!(index_of(&units("abc"), &units(""), 4), None);
    }

    #[test]
    fn a_needle_longer_than_what_is_left_is_not_found() {
        assert_eq!(index_of(&units("abc"), &units("bcd"), 0), None);
        assert_eq!(index_of(&units("abc"), &units("bc"), 1), Some(1));
        assert_eq!(index_of(&units("abc"), &units("bc"), 2), None);
        assert_eq!(index_of(&units("abcabc"), &units("bc"), 2), Some(4));
    }

    #[test]
    fn the_four_positional_dollar_forms_read_around_the_match() {
        assert_eq!(filled("b", "abc", 1, "[$&]"), "[b]");
        assert_eq!(filled("b", "abc", 1, "[$`]"), "[a]");
        assert_eq!(filled("b", "abc", 1, "[$']"), "[c]");
        assert_eq!(filled("b", "abc", 1, "[$$]"), "[$]");
    }

    #[test]
    fn a_dollar_that_names_nothing_is_left_exactly_as_written() {
        // §22.1.3.19.1's last step — this is a rule about *not* erroring, and the one a
        // hand-rolled substitution usually gets wrong.
        assert_eq!(filled("b", "abc", 1, "$x"), "$x");
        assert_eq!(filled("b", "abc", 1, "$"), "$");
        assert_eq!(filled("b", "abc", 1, "$0"), "$0");
        // With no named captures at all, `$<` is four literal characters rather than the start of
        // a name — the specification does not read ahead for a `>` in that case.
        assert_eq!(filled("b", "abc", 1, "$<x>"), "$<x>");
        // …and a capture number past the last group is left alone too, there being none here.
        assert_eq!(filled("b", "abc", 1, "$1"), "$1");
    }

    #[test]
    fn a_two_digit_group_is_read_as_two_digits_only_when_that_group_exists() {
        // `$12` with one capture is capture 1 followed by a literal `2`, and with twelve it is
        // capture 12. Taking both digits unconditionally is the obvious implementation and is
        // wrong for every pattern with fewer than twelve groups.
        let one = vec![Some(units("X"))];
        let filled = |captures: &[Option<Vec<u16>>], template: &str| {
            String::from_utf16_lossy(&fill_in(
                &units("b"),
                &units("abc"),
                1,
                captures,
                None,
                &units(template),
            ))
        };
        assert_eq!(filled(&one, "$12"), "X2");
        // Lettered rather than numbered, because with capture 1 holding `"1"` the wrong reading of
        // `$12` also spells `12` and the test would pass either way.
        let twelve: Vec<Option<Vec<u16>>> =
            ('a'..='l').map(|c| Some(units(&c.to_string()))).collect();
        assert_eq!(filled(&twelve, "$12"), "l");
        assert_eq!(filled(&twelve, "$1"), "a");
        // `$01` is the two-digit form too — §22.1.3.19.1 says 01 to 99, so a leading zero names
        // group 1 and does not fall back to the one-digit reading of `$0`, which names nothing.
        assert_eq!(filled(&one, "$01"), "X");
        assert_eq!(filled(&one, "$99"), "$99");
        // A group that did not participate reads as the empty string rather than `undefined`.
        assert_eq!(filled(&[None], "[$1]"), "[]");
    }

    #[test]
    fn a_named_capture_is_read_when_there_are_named_captures_to_read() {
        let named = vec![(units("who"), Some(units("world")))];
        let filled = |template: &str| {
            String::from_utf16_lossy(&fill_in(
                &units("b"),
                &units("abc"),
                1,
                &[],
                Some(&named),
                &units(template),
            ))
        };
        assert_eq!(filled("hello $<who>"), "hello world");
        // A name that is not among them contributes nothing at all, and is not left literal.
        assert_eq!(filled("[$<missing>]"), "[]");
        // An unterminated `$<` is literal, because there is no name to have read.
        assert_eq!(filled("$<who"), "$<who");
    }
}
