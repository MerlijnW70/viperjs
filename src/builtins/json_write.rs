//! §25.5.2 `JSON.stringify` — turning a value into text that parses back.
//!
//! That promise is what decides every awkward case. An unpaired surrogate is escaped rather than
//! written through (§25.5.2.2, the "well-formed" rule ES2019 added), because a lone surrogate does
//! not survive a trip through UTF-8 and an escape does. `NaN` and the infinities become `null`,
//! because JSON has no spelling for either and text that did not parse back would be worse than a
//! number that is not the one you had. And a cycle is a TypeError rather than a stack that runs
//! out.
//!
//! `undefined`, a function and a Symbol have no JSON at all. In an object they are *omitted*; in
//! an array they become `null`, because an array's shape is its indices and dropping one would
//! move every element after it. The two are opposite, which is why writing an array and writing an
//! object are separate operations here rather than one with a flag.
//!
//! The reading half is [`super::json`]; the two share only the object they are installed on.

use super::key;
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, PropertyKey};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// §25.5.2 `JSON.stringify(value[, replacer[, space]])`.
pub(super) fn stringify(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let replacer = call.argument(1);
    let allowed = property_list(vm, heap, replacer)?;
    let function = callable(heap, replacer);
    let gap = indent(vm, heap, call.argument(2))?;
    let mut writer = Writer {
        allowed,
        function,
        gap,
        depth: 0,
        open: Vec::new(),
    };
    // Step 10 — the value is wrapped so the replacer sees it under the empty key, exactly as the
    // reviver does. The two are symmetrical and this is where that shows.
    let holder = heap.new_object(Some(vm.realm().object_prototype()));
    let empty = key(heap, "");
    let _ = heap.define_own_property(holder, empty, &PropertyDescriptor::data(call.argument(0)));
    let Some(text) = writer.property(vm, heap, holder, empty)? else {
        // §25.5.2 — `undefined`, a function and a Symbol have no JSON, and at the top level that
        // is the answer rather than an error or an empty string.
        return Ok(Value::Undefined);
    };
    Ok(Value::String(heap.intern(&text)))
}

/// §25.5.2 step 5 — the property list an array replacer names, in order and without repeats.
fn property_list(
    vm: &mut Vm,
    heap: &mut Heap,
    replacer: Value,
) -> Completion<Option<Vec<PropertyKey>>> {
    let Value::Object(object) = replacer else {
        return Ok(None);
    };
    if !heap.is_array_through(object)? {
        return Ok(None);
    }
    let name = key(heap, "length");
    let length = vm.get_property_key(replacer, name, heap)?;
    let length = super::array_methods::to_length(vm.to_number(length, heap)?);
    let mut allowed: Vec<PropertyKey> = Vec::new();
    for at in 0..length {
        let index = super::array_methods::index_key(heap, at);
        let item = vm.get_property_key(replacer, index, heap)?;
        // Step 5.b.ii — Strings and Numbers, and the wrappers of either. Anything else is skipped
        // rather than refused, so a replacer array may hold things that are not names.
        let name = match item {
            Value::String(_) | Value::Number(_) => vm.to_property_key(item, heap)?,
            Value::Object(found) => {
                match heap.object(found).and_then(crate::heap::Object::primitive) {
                    Some(Value::String(_) | Value::Number(_)) => vm.to_property_key(item, heap)?,
                    _ => continue,
                }
            }
            _ => continue,
        };
        if !allowed.contains(&name) {
            allowed.push(name);
        }
    }
    Ok(Some(allowed))
}

/// §25.5.2 steps 6 to 8 — the indent one level of nesting adds.
///
/// A Number is that many spaces, clamped to ten; a String is its first ten characters. A wrapper of
/// either is converted with `ToNumber`/`ToString` *on the object*, so its own `valueOf` or
/// `toString` decides and an abrupt one propagates. Anything else indents nothing, which is why
/// `JSON.stringify(v, null, {})` is the compact form.
fn indent(vm: &mut Vm, heap: &mut Heap, space: Value) -> Completion<Vec<u16>> {
    let space = match space {
        Value::Object(object) => {
            // §25.5.2 step 5 — the slot only chooses *which* conversion runs. The conversion is
            // then applied to the object, so an overridden `valueOf` or `toString` is what answers
            // and may throw. Reading the slot directly would ignore both. Naming the slot in its
            // own binding ends the borrow of `heap` before the conversion needs it back.
            let slot = heap.object(object).and_then(crate::heap::Object::primitive);
            match slot {
                Some(Value::Number(_)) => Value::Number(vm.to_number(space, heap)?),
                Some(Value::String(_)) => Value::String(vm.to_string(space, heap)?),
                _ => return Ok(Vec::new()),
            }
        }
        space => space,
    };
    match space {
        Value::Number(_) => {
            let count = vm.to_number(space, heap)?;
            // `clamp` cannot panic here — both bounds are literals and ordered. A NaN clamps to
            // NaN and then truncates to zero, which is the compact form and what §25.5.2 step 6's
            // `ToIntegerOrInfinity` reaches for a NaN too.
            let count = count.clamp(0.0, 10.0) as usize;
            Ok(vec![u16::from(b' '); count])
        }
        Value::String(id) => {
            let units = heap.string(id).unwrap_or(&[]);
            Ok(units.iter().take(10).copied().collect())
        }
        _ => Ok(Vec::new()),
    }
}

/// What `stringify` is carrying while it walks.
struct Writer {
    /// The names an array replacer allows, if there was one.
    allowed: Option<Vec<PropertyKey>>,
    /// The replacer function, if there was one.
    function: Option<Value>,
    /// One level of indent.
    gap: Vec<u16>,
    /// How deep the walk is, for that indent.
    depth: usize,
    /// The objects being serialised right now — §25.5.2.1 step 4's cycle check.
    open: Vec<ObjectId>,
}

impl Writer {
    /// §25.5.2.1 `SerializeJSONProperty` — the text for one property, or nothing.
    fn property(
        &mut self,
        vm: &mut Vm,
        heap: &mut Heap,
        holder: ObjectId,
        name: PropertyKey,
    ) -> Completion<Option<Vec<u16>>> {
        let mut value = vm.get_property_key(Value::Object(holder), name, heap)?;
        let spelled = Value::String(name.as_string().unwrap_or_else(|| heap.intern(&[])));
        // Step 2 — `toJSON` is asked *before* the replacer, and is given the key. An object that
        // knows how to describe itself gets the first word.
        if let Value::Object(_) = value {
            let method = key(heap, "toJSON");
            let method = vm.get_property_key(value, method, heap)?;
            if let Some(method) = callable(heap, method) {
                value = vm.call_value(method, value, &[spelled], heap)?;
            }
        }
        if let Some(replacer) = self.function {
            value = vm.call_value(replacer, Value::Object(holder), &[spelled, value], heap)?;
        }
        // Steps 4 and 5 — a wrapper is unwrapped before anything is decided about it, so
        // `new Number(1)` writes as `1` and `new Boolean(true)` as `true`.
        if let Value::Object(object) = value
            && let Some(primitive) = heap.object(object).and_then(crate::heap::Object::primitive)
        {
            value = match primitive {
                Value::Number(_) => Value::Number(vm.to_number(value, heap)?),
                Value::String(_) => Value::String(vm.to_string(value, heap)?),
                other @ Value::Boolean(_) => other,
                _ => value,
            };
        }
        self.value(vm, heap, value)
    }

    /// The text one value serialises to, or nothing when it has no JSON.
    fn value(
        &mut self,
        vm: &mut Vm,
        heap: &mut Heap,
        value: Value,
    ) -> Completion<Option<Vec<u16>>> {
        match value {
            Value::Null => Ok(Some("null".encode_utf16().collect())),
            Value::Boolean(true) => Ok(Some("true".encode_utf16().collect())),
            Value::Boolean(false) => Ok(Some("false".encode_utf16().collect())),
            Value::String(id) => Ok(Some(quote(heap.string(id).unwrap_or(&[])))),
            // §25.5.2.2 step 10 — a **TypeError**, and the only value JSON refuses outright. JSON
            // has no integer syntax that survives a round trip past 2^53, so writing `1n` as `1`
            // would produce text that parses back as a different value.
            Value::BigInt(_) => Err(Abrupt::type_error("a BigInt cannot be serialised to JSON")),
            // §25.5.2.1 step 9 — a number JSON cannot write is `null`, because JSON has no NaN and
            // no infinities and the alternative would be text that does not parse back.
            Value::Number(number) if number.is_finite() => Ok(Some(
                crate::value::number_to_string(number)
                    .encode_utf16()
                    .collect(),
            )),
            Value::Number(_) => Ok(Some("null".encode_utf16().collect())),
            // §25.5.2.1 step 10 — a *callable* object has no JSON, which is why a function in an
            // object is omitted and one in an array becomes `null`.
            Value::Object(object) if heap.object(object).is_some_and(|f| f.call().is_some()) => {
                Ok(None)
            }
            Value::Object(object) => self.structure(vm, heap, object).map(Some),
            // `undefined` and a Symbol have no JSON at all.
            Value::Undefined | Value::Symbol(_) => Ok(None),
        }
    }

    /// An array or an object, with its cycle check and its indenting.
    fn structure(
        &mut self,
        vm: &mut Vm,
        heap: &mut Heap,
        object: ObjectId,
    ) -> Completion<Vec<u16>> {
        // §25.5.2.1 step 4 — a value that is already being written is a cycle, and a TypeError is
        // the answer rather than a stack that runs out. The list is the path from the root, not
        // everything seen: the same object twice in *different* branches is not a cycle.
        if self.open.contains(&object) {
            return Err(Abrupt::type_error("a circular structure has no JSON"));
        }
        // …and a structure that is merely deep is not a cycle, so the check above never ends it.
        // This walk has the fattest frames of §25.5's three — see `super::json::MAX_JSON_DEPTH` —
        // so it is the one that decides the number, and it is guarded *after* the cycle check
        // because a cycle has an answer of its own and reaching this first would rename it.
        if self.depth >= super::json::MAX_JSON_DEPTH as usize {
            return Err(super::json::too_deep());
        }
        self.open.push(object);
        self.depth += 1;
        let written = match heap.is_array_through(object)? {
            true => self.list(vm, heap, object),
            false => self.members(vm, heap, object),
        };
        self.depth -= 1;
        self.open.pop();
        written
    }

    /// §25.5.2.4 `SerializeJSONArray`.
    fn list(&mut self, vm: &mut Vm, heap: &mut Heap, object: ObjectId) -> Completion<Vec<u16>> {
        let name = key(heap, "length");
        let length = vm.get_property_key(Value::Object(object), name, heap)?;
        let length = super::array_methods::to_length(vm.to_number(length, heap)?);
        let mut parts = Vec::new();
        for at in 0..length {
            let index = super::array_methods::index_key(heap, at);
            // Step 8.b — an element with no JSON is `null` and not omitted, because an array's
            // shape is its indices and dropping one would move everything after it.
            let part = self
                .property(vm, heap, object, index)?
                .unwrap_or_else(|| "null".encode_utf16().collect());
            parts.push(part);
            super::array_methods::within_budget(heap)?;
        }
        Ok(self.join(parts, 0x5B, 0x5D))
    }

    /// §25.5.2.5 `SerializeJSONObject`.
    fn members(&mut self, vm: &mut Vm, heap: &mut Heap, object: ObjectId) -> Completion<Vec<u16>> {
        let names = match &self.allowed {
            Some(allowed) => allowed.clone(),
            // §25.5.2.5 step 5 `EnumerableOwnProperties(value, key)` — two conditions that exclude
            // different properties. A Symbol key has no name JSON could write, and a
            // non-enumerable one is not the caller's to see.
            None => {
                let mut listed = Vec::new();
                for found in vm.own_keys_through(object, heap)? {
                    if found.as_string().is_none() {
                        continue;
                    }
                    if vm
                        .own_property_through(object, found, heap)?
                        .is_some_and(|property| property.enumerable)
                    {
                        listed.push(found);
                    }
                }
                listed
            }
        };
        let mut parts = Vec::new();
        for name in names {
            // Step 6.b — a property with no JSON is *omitted*, which is the opposite of what an
            // array does with one and is why the two are separate operations.
            let Some(part) = self.property(vm, heap, object, name)? else {
                continue;
            };
            let mut written = quote(
                name.as_string()
                    .and_then(|id| heap.string(id))
                    .unwrap_or(&[]),
            );
            written.push(u16::from(b':'));
            if !self.gap.is_empty() {
                written.push(u16::from(b' '));
            }
            written.extend(part);
            parts.push(written);
        }
        Ok(self.join(parts, 0x7B, 0x7D))
    }

    /// Put the parts between their brackets, indented if there is a gap.
    fn join(&self, parts: Vec<Vec<u16>>, open: u16, close: u16) -> Vec<u16> {
        let mut written = vec![open];
        if parts.is_empty() {
            written.push(close);
            return written;
        }
        let inner: Vec<u16> = self.gap.repeat(self.depth);
        let outer: Vec<u16> = self.gap.repeat(self.depth.saturating_sub(1));
        for (at, part) in parts.into_iter().enumerate() {
            if at > 0 {
                written.push(u16::from(b','));
            }
            if !self.gap.is_empty() {
                written.push(0x0A);
                written.extend_from_slice(&inner);
            }
            written.extend(part);
        }
        if !self.gap.is_empty() {
            written.push(0x0A);
            written.extend_from_slice(&outer);
        }
        written.push(close);
        written
    }
}

/// The value if it is something a call may reach, and nothing otherwise — §7.2.3 `IsCallable`.
///
/// Answers the value rather than a Boolean so that the caller has no second condition to write:
/// a `toJSON` that is not callable is an ordinary property, and one test decides both.
fn callable(heap: &Heap, value: Value) -> Option<Value> {
    let Value::Object(object) = value else {
        return None;
    };
    heap.object(object)
        .and_then(|found| found.call().map(|_| value))
}

/// §25.5.2.2 `QuoteJSONString` — the text as a JSON string literal.
///
/// An unpaired surrogate is escaped rather than written through. That is the "well-formed" rule,
/// and it is what makes the promise `stringify` exists to keep: the output parses back, and a lone
/// surrogate in the middle of a UTF-8 stream does not survive the trip.
fn quote(units: &[u16]) -> Vec<u16> {
    let mut written = vec![0x22];
    let mut at = 0;
    while at < units.len() {
        let unit = units[at];
        match unit {
            0x22 | 0x5C => {
                written.push(0x5C);
                written.push(unit);
            }
            0x08 => written.extend("\\b".encode_utf16()),
            0x0C => written.extend("\\f".encode_utf16()),
            0x0A => written.extend("\\n".encode_utf16()),
            0x0D => written.extend("\\r".encode_utf16()),
            0x09 => written.extend("\\t".encode_utf16()),
            0x00..=0x1F => written.extend(escaped(unit)),
            0xD800..=0xDBFF => {
                let paired = units
                    .get(at + 1)
                    .is_some_and(|next| (0xDC00..0xE000).contains(next));
                match paired {
                    true => {
                        written.push(unit);
                        written.push(units[at + 1]);
                        at += 1;
                    }
                    false => written.extend(escaped(unit)),
                }
            }
            0xDC00..=0xDFFF => written.extend(escaped(unit)),
            unit => written.push(unit),
        }
        at += 1;
    }
    written.push(0x22);
    written
}

/// One unit as `\uXXXX`, lower case as §25.5.2.2 writes it.
fn escaped(unit: u16) -> Vec<u16> {
    format!("\\u{unit:04x}").encode_utf16().collect()
}
