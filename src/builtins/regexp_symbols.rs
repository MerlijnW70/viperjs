//! §22.2.6's four `Symbol` methods — what `String.prototype` calls when it is handed a pattern.
//!
//! # Why these are on `RegExp` and not in `String`
//!
//! `"a".replace(x, y)` asks `x` for its `Symbol.replace` and hands the whole operation over. That
//! is the extension point: a regular expression supplies these, and so may anything else. So the
//! String methods know nothing about patterns and these know nothing about how they were reached,
//! and the two meet at a well-known Symbol.
//!
//! # `RegExpExec` and why every one of them goes through it
//!
//! §22.2.7.1 reads `exec` **off the object** and calls it if it is callable, falling back to the
//! built-in only when it is not. So a program may override `exec` on one regular expression and
//! every one of these obeys it — which is the whole reason they are written in terms of an
//! operation rather than calling the built-in directly.

use super::{key, regexp::builtin_exec};
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §22.2.7.1 `RegExpExec` — the object's own `exec` if it has a callable one.
fn regexp_exec(
    vm: &mut Vm,
    heap: &mut Heap,
    object: ObjectId,
    subject: &[u16],
) -> Completion<Value> {
    let name = key(heap, "exec");
    let found = vm.get_property_key(Value::Object(object), name, heap)?;
    if heap.is_callable(found) {
        let text = Value::String(heap.intern(subject));
        let answer = vm.call_value(found, Value::Object(object), &[text], heap)?;
        // Step 3.c — an overriding `exec` must answer an Object or null. Anything else is a
        // TypeError rather than something the caller has to guess at.
        if !matches!(answer, Value::Object(_) | Value::Null) {
            return Err(Abrupt::type_error(
                "a regular expression's exec must answer an object or null",
            ));
        }
        return Ok(answer);
    }
    builtin_exec(vm, heap, object, subject)
}

/// The receiver as an object, which every one of these requires — §22.2.6.
fn this_object(receiver: Value) -> Completion<ObjectId> {
    match receiver {
        Value::Object(object) => Ok(object),
        _ => Err(Abrupt::type_error(
            "this method requires a regular expression",
        )),
    }
}

/// The subject as code units, after `ToString`.
fn subject_of(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<Vec<u16>> {
    let text = vm.to_string(value, heap)?;
    Ok(heap.string(text).unwrap_or(&[]).to_vec())
}

/// Whether the receiver's `flags` contain a letter — read as a *property*, per §22.2.6.
///
/// Every one of these clauses reads `flags` with `Get` rather than the slot, so a subclass that
/// overrides it is obeyed. That is observable and it is why this is not `found.flags().global`.
fn has_flag(vm: &mut Vm, heap: &mut Heap, object: ObjectId, letter: u8) -> Completion<bool> {
    let name = key(heap, "flags");
    let held = vm.get_property_key(Value::Object(object), name, heap)?;
    let text = vm.to_string(held, heap)?;
    Ok(heap
        .string(text)
        .unwrap_or(&[])
        .contains(&u16::from(letter)))
}

/// Read `lastIndex`, and write it — §22.2.6's own bookkeeping between matches.
fn set_last_index(vm: &mut Vm, heap: &mut Heap, object: ObjectId, at: usize) -> Completion<()> {
    let name = key(heap, "lastIndex");
    let value = Value::Number(f64::from(u32::try_from(at).unwrap_or(u32::MAX)));
    super::set_or_throw(vm, heap, object, name, value)?;
    Ok(())
}

/// §22.2.6.6 `RegExp.prototype[Symbol.match]`.
fn symbol_match(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call.this_value)?;
    let subject = subject_of(vm, heap, call.argument(0))?;
    // Step 5 — a pattern that is not global answers *one match*, in `exec`'s shape. A global one
    // answers an array of the matched **strings** and no captures at all, which is the one place
    // the two differ in kind rather than in count.
    if !has_flag(vm, heap, object, b'g')? {
        return regexp_exec(vm, heap, object, &subject);
    }
    set_last_index(vm, heap, object, 0)?;
    let mut found = Vec::new();
    loop {
        let result = regexp_exec(vm, heap, object, &subject)?;
        let Value::Object(result) = result else {
            break;
        };
        let zero = heap.index_key(0);
        let text = vm.get_property_key(Value::Object(result), zero, heap)?;
        let text = Value::String(vm.to_string(text, heap)?);
        // Step 8.d.iii.2 — an *empty* match would leave `lastIndex` where it was, so it is
        // advanced by hand. Without this the loop never ends for a pattern like `/(?:)/g`.
        if matches!(text, Value::String(id) if heap.string(id).is_none_or(<[u16]>::is_empty)) {
            let name = key(heap, "lastIndex");
            let held = vm.get_property_key(Value::Object(object), name, heap)?;
            let at = vm.to_number(held, heap)?;
            let next = usize::try_from(at.max(0.0) as u64).unwrap_or(usize::MAX);
            set_last_index(vm, heap, object, next.saturating_add(1))?;
        }
        found.push(text);
    }
    // Step 8.a — no match at all is **null**, not an empty array, so a caller can tell "found
    // nothing" from "found nothing yet".
    if found.is_empty() {
        return Ok(Value::Null);
    }
    super::array::from_values(vm, heap, &found)
}

/// §22.2.6.11 `RegExp.prototype[Symbol.search]`.
fn symbol_search(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call.this_value)?;
    let subject = subject_of(vm, heap, call.argument(0))?;
    // Steps 4 and 8 — `lastIndex` is saved and *put back*, because a search is not supposed to
    // move it. That is the difference from `exec`, and the only reason this is not two lines.
    let name = key(heap, "lastIndex");
    let held = vm.get_property_key(Value::Object(object), name, heap)?;
    set_last_index(vm, heap, object, 0)?;
    let result = regexp_exec(vm, heap, object, &subject);
    super::set_or_throw(vm, heap, object, name, held)?;
    let Value::Object(result) = result? else {
        return Ok(Value::Number(-1.0));
    };
    let index = key(heap, "index");
    vm.get_property_key(Value::Object(result), index, heap)
}

/// §22.2.6.9 `RegExp.prototype[Symbol.replace]`.
fn symbol_replace(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call.this_value)?;
    let subject = subject_of(vm, heap, call.argument(0))?;
    let with = call.argument(1);
    let functional = heap.is_callable(with);
    let with = match functional {
        true => with,
        // Step 5 — a non-callable replacement is converted to a String *now*, before any matching,
        // which is observable when it has a `toString` that throws.
        false => Value::String(vm.to_string(with, heap)?),
    };
    let global = has_flag(vm, heap, object, b'g')?;
    if global {
        set_last_index(vm, heap, object, 0)?;
    }
    // Step 8 — every match is collected *first* and the replacements applied afterwards. The
    // ordering is observable: a replacement function may change `lastIndex`, and the matches it
    // sees must be the ones found before it ran.
    let mut results = Vec::new();
    loop {
        let result = regexp_exec(vm, heap, object, &subject)?;
        let Value::Object(result) = result else {
            break;
        };
        results.push(result);
        if !global {
            break;
        }
        let zero = heap.index_key(0);
        let text = vm.get_property_key(Value::Object(result), zero, heap)?;
        let text = vm.to_string(text, heap)?;
        if heap.string(text).is_none_or(<[u16]>::is_empty) {
            let name = key(heap, "lastIndex");
            let held = vm.get_property_key(Value::Object(object), name, heap)?;
            let at = vm.to_number(held, heap)?;
            let next = usize::try_from(at.max(0.0) as u64).unwrap_or(usize::MAX);
            set_last_index(vm, heap, object, next.saturating_add(1))?;
        }
    }
    let mut built: Vec<u16> = Vec::with_capacity(subject.len());
    let mut copied = 0;
    for result in results {
        let found = super::string_replace::parts_of(vm, heap, result, &subject)?;
        let position = found.position.min(subject.len());
        let matched = found.matched.clone();
        let text = match functional {
            true => super::string_replace::from_function(vm, heap, with, &found, &subject)?,
            false => {
                let template = match with {
                    Value::String(id) => heap.string(id).unwrap_or(&[]).to_vec(),
                    _ => Vec::new(),
                };
                super::string_replace::fill_in(
                    &matched,
                    &subject,
                    position,
                    &found.captures,
                    found.named.as_deref(),
                    &template,
                )
            }
        };
        // Step 14.n — a match that begins before what has already been copied is *skipped*, which
        // is how an `exec` that moves backwards cannot make the answer nonsense.
        if position >= copied {
            built.extend_from_slice(&subject[copied..position]);
            built.extend_from_slice(&text);
            copied = position + matched.len();
        }
    }
    built.extend_from_slice(&subject[copied.min(subject.len())..]);
    Ok(Value::String(heap.intern(&built)))
}

/// §22.2.6.14 `RegExp.prototype[Symbol.split]`.
fn symbol_split(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call.this_value)?;
    let subject = subject_of(vm, heap, call.argument(0))?;
    let limit = match call.argument(1) {
        Value::Undefined => u32::MAX,
        given => {
            let number = vm.to_number(given, heap)?;
            // §7.1.6 `ToUint32` — a limit of zero means an empty answer, and a negative one wraps.
            (number as i64).rem_euclid(0x1_0000_0000) as u32
        }
    };
    let mut pieces: Vec<Value> = Vec::new();
    if limit == 0 {
        return super::array::from_values(vm, heap, &pieces);
    }
    // §22.2.6.14 makes a **sticky** copy of the pattern, so each attempt is anchored where the last
    // piece ended rather than searching forward. That is what lets a split tell `"a,b"` on `/,/`
    // from one on `/x/`, and it is why the receiver's own flags are not used directly.
    let flags_name = key(heap, "flags");
    let held = vm.get_property_key(Value::Object(object), flags_name, heap)?;
    let spelled = vm.to_string(held, heap)?;
    let mut letters = String::from_utf16_lossy(heap.string(spelled).unwrap_or(&[]));
    if !letters.contains('y') {
        letters.push('y');
    }
    let source_name = key(heap, "source");
    let source = vm.get_property_key(Value::Object(object), source_name, heap)?;
    let splitter = {
        let constructor = vm.realm().regexp_prototype();
        let _ = constructor;
        let pattern = Value::String(vm.to_string(source, heap)?);
        let spelled = Value::String(heap.intern(&letters.encode_utf16().collect::<Vec<_>>()));
        super::regexp::make(vm, heap, pattern, spelled)?
    };
    if subject.is_empty() {
        // Step 15 — an empty subject answers one empty piece, unless the pattern matches it, in
        // which case it answers none at all.
        let found = regexp_exec(vm, heap, splitter, &subject)?;
        if matches!(found, Value::Null) {
            let whole = Value::String(heap.intern(&subject));
            pieces.push(whole);
        }
        return super::array::from_values(vm, heap, &pieces);
    }
    let mut piece_start = 0;
    let mut at = 0;
    while at < subject.len() {
        set_last_index(vm, heap, splitter, at)?;
        let found = regexp_exec(vm, heap, splitter, &subject)?;
        let Value::Object(found) = found else {
            at += 1;
            continue;
        };
        let name = key(heap, "lastIndex");
        let held = vm.get_property_key(Value::Object(splitter), name, heap)?;
        let end = vm.to_number(held, heap)?;
        let end = usize::try_from(end.max(0.0) as u64)
            .unwrap_or(usize::MAX)
            .min(subject.len());
        // Step 19.c.iii — a match that consumed nothing at the same place would split forever, so
        // it is stepped over rather than acted on.
        if end == piece_start {
            at += 1;
            continue;
        }
        let piece = Value::String(heap.intern(&subject[piece_start..at]));
        pieces.push(piece);
        if pieces.len() >= limit as usize {
            return super::array::from_values(vm, heap, &pieces);
        }
        // The captures go into the answer too, which is what makes `"a1b".split(/(\d)/)` three
        // pieces and not two.
        let length = key(heap, "length");
        let count = vm.get_property_key(Value::Object(found), length, heap)?;
        let count = vm.to_number(count, heap)? as u32;
        for index in 1..count {
            let slot = heap.index_key(index);
            let capture = vm.get_property_key(Value::Object(found), slot, heap)?;
            pieces.push(capture);
            if pieces.len() >= limit as usize {
                return super::array::from_values(vm, heap, &pieces);
            }
        }
        piece_start = end;
        at = end;
    }
    let last = Value::String(heap.intern(&subject[piece_start.min(subject.len())..]));
    pieces.push(last);
    super::array::from_values(vm, heap, &pieces)
}

/// §22.2.6.8 `RegExp.prototype[Symbol.matchAll]`.
fn symbol_match_all(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let object = this_object(call.this_value)?;
    let subject = subject_of(vm, heap, call.argument(0))?;
    // Steps 4 and 5 — the iterator walks a **copy**, so a program that moves the original's
    // `lastIndex` half-way through a `for`-`of` does not disturb the walk. Both objects are
    // reachable and both are observable, which is why this is a copy and not a borrow.
    let flags_name = key(heap, "flags");
    let held = vm.get_property_key(Value::Object(object), flags_name, heap)?;
    let spelled = vm.to_string(held, heap)?;
    let letters = heap.string(spelled).unwrap_or(&[]).to_vec();
    let source_name = key(heap, "source");
    let source = vm.get_property_key(Value::Object(object), source_name, heap)?;
    let source = Value::String(vm.to_string(source, heap)?);
    let copy = super::regexp::make(vm, heap, source, Value::String(spelled))?;
    // Step 6 — the copy starts where the original had got to, so `matchAll` on a pattern already
    // part-way through a subject continues rather than starting over.
    let index_name = key(heap, "lastIndex");
    let at = vm.get_property_key(Value::Object(object), index_name, heap)?;
    let at = vm.to_number(at, heap)?;
    let at = usize::try_from(at.max(0.0) as u64).unwrap_or(usize::MAX);
    set_last_index(vm, heap, copy, at)?;
    let text = heap.intern(&subject);
    let iterator = heap.new_object(Some(vm.realm().regexp_string_iterator_prototype()));
    if let Some(found) = heap.object_mut(iterator) {
        found.set_matches(crate::heap::Matches {
            regexp: copy,
            subject: text,
            // Steps 8 and 9 — read **once**. A `flags` getter cannot change what the walk does
            // between one step and the next.
            global: letters.contains(&u16::from(b'g')),
            unicode: letters.contains(&u16::from(b'u')) || letters.contains(&u16::from(b'v')),
            done: false,
        });
    }
    Ok(Value::Object(iterator))
}

/// §22.2.9.2.1 — the iterator's `next`.
fn iterator_next(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let Value::Object(iterator) = call.this_value else {
        return Err(Abrupt::type_error(
            "this method requires a RegExp String Iterator",
        ));
    };
    let Some(state) = heap.object(iterator).and_then(crate::heap::Object::matches) else {
        return Err(Abrupt::type_error(
            "this method requires a RegExp String Iterator",
        ));
    };
    if state.done {
        return finished(vm, heap);
    }
    let subject = heap.string(state.subject).unwrap_or(&[]).to_vec();
    let found = regexp_exec(vm, heap, state.regexp, &subject)?;
    let Value::Object(found) = found else {
        // Step 6.a — nothing more. The walk is marked finished so a second `next` costs no call at
        // all, which a `return` on the regular expression could otherwise observe.
        stop(heap, iterator);
        return finished(vm, heap);
    };
    // Step 7 — a non-global pattern yields exactly one result and is done, whatever the subject
    // still holds. That is the difference the flag makes here, and it is read from the copy taken
    // when the iterator was made.
    if !state.global {
        stop(heap, iterator);
        return step(vm, heap, Value::Object(found));
    }
    // Step 8.e — an empty match would leave `lastIndex` where it is, so it is stepped over. Under
    // `u` or `v` the step is a whole code point, or the walk would stop inside a surrogate pair.
    let zero = heap.index_key(0);
    let text = vm.get_property_key(Value::Object(found), zero, heap)?;
    let text = vm.to_string(text, heap)?;
    if heap.string(text).is_none_or(<[u16]>::is_empty) {
        let name = key(heap, "lastIndex");
        let held = vm.get_property_key(Value::Object(state.regexp), name, heap)?;
        let at = vm.to_number(held, heap)?;
        let at = usize::try_from(at.max(0.0) as u64).unwrap_or(usize::MAX);
        set_last_index(
            vm,
            heap,
            state.regexp,
            advanced(&subject, at, state.unicode),
        )?;
    }
    step(vm, heap, Value::Object(found))
}

/// §22.2.7.3 `AdvanceStringIndex` — one unit, or one code point under `u`.
fn advanced(subject: &[u16], at: usize, unicode: bool) -> usize {
    if !unicode {
        return at.saturating_add(1);
    }
    let leading = subject.get(at).copied().unwrap_or(0);
    let trailing = subject.get(at + 1).copied().unwrap_or(0);
    match (0xD800..=0xDBFF).contains(&leading) && (0xDC00..=0xDFFF).contains(&trailing) {
        true => at.saturating_add(2),
        false => at.saturating_add(1),
    }
}

/// Mark a walk finished, which nothing undoes.
fn stop(heap: &mut Heap, iterator: ObjectId) {
    if let Some(state) = heap
        .object_mut(iterator)
        .and_then(crate::heap::Object::matches_mut)
    {
        state.done = true;
    }
}

/// §7.4.14 `CreateIterResultObject` with `done` true and nothing to hand over.
fn finished(vm: &mut Vm, heap: &mut Heap) -> Completion<Value> {
    result(vm, heap, Value::Undefined, true)
}

/// The same, with a value and `done` false.
fn step(vm: &mut Vm, heap: &mut Heap, value: Value) -> Completion<Value> {
    result(vm, heap, value, false)
}

/// §7.4.14 `CreateIterResultObject`.
fn result(vm: &mut Vm, heap: &mut Heap, value: Value, done: bool) -> Completion<Value> {
    let object = heap.new_object(Some(vm.realm().object_prototype()));
    super::define_value(heap, object, "value", value);
    super::define_value(heap, object, "done", Value::Boolean(done));
    Ok(Value::Object(object))
}

/// Put the four onto `RegExp.prototype`.
pub(super) fn install(heap: &mut Heap, realm: &Realm) {
    let prototype = realm.regexp_prototype();
    let methods: [(&str, u32, crate::heap::Native); 5] = [
        ("match", 1, symbol_match),
        ("matchAll", 1, symbol_match_all),
        ("replace", 2, symbol_replace),
        ("search", 1, symbol_search),
        ("split", 2, symbol_split),
    ];
    // §22.2.9.3 — the iterator's own prototype, which inherits `[Symbol.iterator]` from
    // `%IteratorPrototype%` and so is iterable without saying so itself.
    let iterating = realm.regexp_string_iterator_prototype();
    super::define_method(heap, realm, iterating, "next", 0, iterator_next);
    super::collection::tag_with(heap, realm, iterating, "RegExp String Iterator");
    for (name, length, native) in methods {
        let Some(symbol) = realm.well_known(super::well_known_at(name)) else {
            continue;
        };
        let function = heap.new_native_function(realm.function_prototype(), native);
        super::define_function_metadata(heap, function, &format!("[Symbol.{name}]"), length);
        let slot = crate::heap::PropertyKey::from_symbol(symbol);
        let _ = heap.define_own_property(
            prototype,
            slot,
            &PropertyDescriptor {
                value: Some(Value::Object(function)),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
    }
}
