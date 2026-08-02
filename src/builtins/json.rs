//! §25.5 — `JSON.parse` and `JSON.stringify`.
//!
//! # Why the grammar is written out rather than reused
//!
//! JSON looks like a subset of JavaScript and is not one, in both directions. It has no trailing
//! commas, no comments, no single quotes, no unquoted keys, no leading `+`, no `.5`, and no
//! hexadecimal — and it *does* allow a lone surrogate in a string, which the lexer refuses. So the
//! parser here reads §25.5.1's grammar and nothing else. Reaching for `crate::lexer` would accept
//! programs that are not JSON, which is the failure mode nobody notices until data written by one
//! implementation is refused by another.
//!
//! The writing half is [`super::json_write`], and what it promises is written up there. The two
//! meet only on the object they are installed on — and on [`MAX_JSON_DEPTH`], because all three of
//! §25.5's walks recurse and a stack overflow is not a thing a `Result` can carry.

use super::{define_method, key};
use crate::heap::{Heap, NativeCall, ObjectId, PropertyDescriptor, PropertyKey, StringId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// How deeply §25.5's three walks will nest before refusing — DR-0002 and DR-0006's rule.
///
/// All three recurse in Rust over something a script chooses: the reader over the text's nesting,
/// [`revive`] over the object graph the reviver hands back, and [`super::json_write`]'s serialiser
/// over the graph it is given. §25.5 puts no limit on any of them, and every engine has one — the
/// alternative is a stack overflow, which DR-0002 says no `Result` can rescue and which takes the
/// embedder's process with it.
///
/// **A count and not a stack measurement**, for DR-0006's reason: measuring would make which
/// documents parse depend on how the engine was compiled. Measured on the 1 MiB stack Windows gives
/// a program, in a debug build, whose frames are largest — the reader dies between 750 and 800, a
/// `revive` past 400, and the **serialiser between 250 and 300**, which is the one this has to fit
/// inside. 64 is the number every other cap in the engine uses (`MAX_NESTING_DEPTH`,
/// `MAX_EXPRESSION_DEPTH`, `MAX_REENTRY_DEPTH`) and it sits at a quarter of the narrowest of those
/// three, which is the margin the others do not have and this one can afford.
///
/// It costs nothing measurable: no JSON in test262 nests past a handful, and data that nests past
/// 64 is machine-generated. Like `MAX_NESTING_DEPTH` this should become an embedder's number, when
/// there is somebody who knows how much stack there actually is.
pub(super) const MAX_JSON_DEPTH: u32 = 64;

/// The refusal all three share — §25.5 has no error for this, so praxis picks one and explains it.
///
/// A **RangeError** rather than a SyntaxError, even from the reader. The text is perfectly good
/// JSON; what ran out is this engine's willingness to descend, which is a resource question and is
/// what `RangeError` means everywhere else here. A SyntaxError would also be indistinguishable from
/// [`bad_json`], and a program cannot fix malformed text by making it shallower.
pub(super) fn too_deep() -> Abrupt {
    Abrupt::Raised(
        crate::value::ErrorKind::Range,
        "JSON nested too deeply for this engine",
    )
}

/// Build the `JSON` object into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    // §25.5 — an ordinary object and not a constructor: `new JSON()` is a TypeError, and there is
    // nothing to construct because it holds two functions and no state.
    let json = heap.new_object(Some(realm.object_prototype()));
    super::define_value(heap, global, "JSON", Value::Object(json));
    define_method(heap, realm, json, "parse", 2, parse);
    define_method(
        heap,
        realm,
        json,
        "stringify",
        3,
        super::json_write::stringify,
    );
    // §25.5.3 — the tag, which is what makes `Object.prototype.toString.call(JSON)` say `JSON`
    // rather than `Object`.
    let Some(symbol) = realm.well_known(super::well_known_at("toStringTag")) else {
        return;
    };
    let units: Vec<u16> = "JSON".encode_utf16().collect();
    let value = Value::String(heap.intern(&units));
    let _ = heap.define_own_property(
        json,
        PropertyKey::from_symbol(symbol),
        &PropertyDescriptor {
            value: Some(value),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}

/// §25.5.1 `JSON.parse(text[, reviver])`.
fn parse(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    let mut reader = Reader {
        units: &units,
        at: 0,
        depth: 0,
    };
    reader.spaces();
    let value = reader.value(vm, heap)?;
    reader.spaces();
    // Step 2 — the whole text has to be one value. Trailing anything is a SyntaxError, which is
    // what stops `JSON.parse("1 2")` from quietly answering 1.
    if reader.at < units.len() {
        return Err(bad_json());
    }
    let Value::Object(reviver) = call.argument(1) else {
        return Ok(value);
    };
    if heap
        .object(reviver)
        .is_none_or(|found| found.call().is_none())
    {
        return Ok(value);
    }
    // §25.5.1.1 `InternalizeJSONProperty` — the reviver walks the result bottom-up, in a wrapper
    // object under the empty key, which is what lets it replace the root as readily as a leaf.
    let root = heap.new_object(Some(vm.realm().object_prototype()));
    let empty = key(heap, "");
    let _ = heap.define_own_property(root, empty, &PropertyDescriptor::data(value));
    revive(vm, heap, root, empty, Value::Object(reviver), 0)
}

/// §25.5.1.1 — hand each value to the reviver, innermost first.
///
/// `depth` is the walk's own, and it is **not** the depth of the text that was parsed. The reviver
/// runs at every node and may put anything it likes where it was called, so what this descends
/// through is a graph the script builds as the walk goes — see [`MAX_JSON_DEPTH`]. That is not a
/// hypothetical: test262's `reviver-array-length-coerce-err.js` hands back a Proxy on every call and
/// walked praxis off the end of the stack.
///
/// **Level 0 is the wrapper, which is not part of the document**, so the document's own root is
/// level 1 and the cap is reached one level later than the reader's. That is what makes a text the
/// reader accepts one the reviver can also walk: counted the same way, adding a reviver to a
/// working `JSON.parse` would start refusing it at the deepest level the reader allows.
fn revive(
    vm: &mut Vm,
    heap: &mut Heap,
    holder: ObjectId,
    name: PropertyKey,
    reviver: Value,
    depth: u32,
) -> Completion<Value> {
    if depth > MAX_JSON_DEPTH {
        return Err(too_deep());
    }
    let value = vm.get_property_key(Value::Object(holder), name, heap)?;
    if let Value::Object(object) = value {
        // §25.5.1.1 step 2.b — `EnumerableOwnPropertyNames` on the holder, which for a proxy is
        // its `ownKeys` trap.
        for key in vm.own_keys_through(object, heap)? {
            let revived = revive(vm, heap, object, key, reviver, depth + 1)?;
            // Step 2.b.ii.2 — a reviver answering `undefined` *deletes* the property rather than
            // setting it to `undefined`, which is the only way it can remove one.
            match revived {
                Value::Undefined => {
                    heap.delete_own_property(object, key);
                }
                revived => {
                    let _ =
                        heap.define_own_property(object, key, &PropertyDescriptor::data(revived));
                }
            }
        }
    }
    let name = Value::String(name.as_string().unwrap_or_else(|| heap.intern(&[])));
    vm.call_value(reviver, Value::Object(holder), &[name, value], heap)
}

/// A SyntaxError with the one message every malformed input gets.
fn bad_json() -> Abrupt {
    Abrupt::Raised(
        crate::value::ErrorKind::Syntax,
        "the text is not valid JSON",
    )
}

/// §25.5.1's grammar, read left to right over UTF-16 units.
struct Reader<'a> {
    units: &'a [u16],
    at: usize,
    /// How many `{` and `[` are open — see [`MAX_JSON_DEPTH`].
    ///
    /// Counted on the reader rather than passed down, because `value` is reached from three places
    /// and a parameter is three chances to forget one. Raised where a container opens and lowered
    /// where it closes, including on the paths that return early: a reader that only lowered on the
    /// way out of the loop would refuse a *wide* document — `[[],[],[]…]` — for being deep.
    depth: u32,
}

impl Reader<'_> {
    /// The unit at the cursor, if there is one.
    fn peek(&self) -> Option<u16> {
        self.units.get(self.at).copied()
    }

    /// Skip §25.5.1's whitespace — four characters, and not §12.2's much longer list.
    fn spaces(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0A | 0x0D)) {
            self.at += 1;
        }
    }

    /// Whether the text at the cursor begins with `word`, and step past it if so.
    fn word(&mut self, word: &str) -> bool {
        let units: Vec<u16> = word.encode_utf16().collect();
        let matched = self.units.get(self.at..self.at + units.len()) == Some(&units[..]);
        if matched {
            self.at += units.len();
        }
        matched
    }

    /// One `JSONValue`.
    fn value(&mut self, vm: &mut Vm, heap: &mut Heap) -> Completion<Value> {
        match self.peek() {
            Some(0x7B) => self.nested(Self::object, vm, heap),
            Some(0x5B) => self.nested(Self::array, vm, heap),
            Some(0x22) => Ok(Value::String(self.text(heap)?)),
            Some(_) if self.word("true") => Ok(Value::Boolean(true)),
            Some(_) if self.word("false") => Ok(Value::Boolean(false)),
            Some(_) if self.word("null") => Ok(Value::Null),
            Some(_) => self.number(),
            None => Err(bad_json()),
        }
    }

    /// Read one container, counting it against [`MAX_JSON_DEPTH`].
    ///
    /// The count comes back down however the container ended, including by refusal — a reader that
    /// leaked a level per failed parse would answer differently for the second `JSON.parse` in a
    /// program than for the first.
    fn nested(
        &mut self,
        read: fn(&mut Self, &mut Vm, &mut Heap) -> Completion<Value>,
        vm: &mut Vm,
        heap: &mut Heap,
    ) -> Completion<Value> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(too_deep());
        }
        self.depth += 1;
        let read = read(self, vm, heap);
        self.depth -= 1;
        read
    }

    /// `JSONObject`.
    fn object(&mut self, vm: &mut Vm, heap: &mut Heap) -> Completion<Value> {
        self.at += 1;
        let object = heap.new_object(Some(vm.realm().object_prototype()));
        self.spaces();
        if self.peek() == Some(0x7D) {
            self.at += 1;
            return Ok(Value::Object(object));
        }
        loop {
            self.spaces();
            // A key is a *string*, always quoted — an unquoted one is JavaScript and not JSON.
            // `text` is what insists on the quote, so there is no second check here to disagree
            // with it.
            let name = self.text(heap)?;
            let name = PropertyKey::from_string(heap, name);
            self.spaces();
            if self.peek() != Some(0x3A) {
                return Err(bad_json());
            }
            self.at += 1;
            self.spaces();
            let value = self.value(vm, heap)?;
            let _ = heap.define_own_property(object, name, &PropertyDescriptor::data(value));
            self.spaces();
            match self.peek() {
                Some(0x2C) => self.at += 1,
                Some(0x7D) => {
                    self.at += 1;
                    return Ok(Value::Object(object));
                }
                // A trailing comma lands here, which is where JSON and JavaScript part.
                _ => return Err(bad_json()),
            }
        }
    }

    /// `JSONArray`.
    fn array(&mut self, vm: &mut Vm, heap: &mut Heap) -> Completion<Value> {
        self.at += 1;
        let array = heap.new_array(vm.realm().array_prototype(), 0);
        self.spaces();
        if self.peek() == Some(0x5D) {
            self.at += 1;
            return Ok(Value::Object(array));
        }
        let mut index = 0;
        loop {
            self.spaces();
            let value = self.value(vm, heap)?;
            super::array_methods::set_index(heap, array, index, value);
            index += 1;
            self.spaces();
            match self.peek() {
                Some(0x2C) => self.at += 1,
                Some(0x5D) => {
                    self.at += 1;
                    return Ok(Value::Object(array));
                }
                _ => return Err(bad_json()),
            }
        }
    }

    /// `JSONString` — double quotes, and a shorter escape list than JavaScript's.
    fn text(&mut self, heap: &mut Heap) -> Completion<StringId> {
        if self.peek() != Some(0x22) {
            return Err(bad_json());
        }
        self.at += 1;
        let mut units = Vec::new();
        loop {
            let Some(unit) = self.peek() else {
                return Err(bad_json());
            };
            self.at += 1;
            match unit {
                0x22 => return Ok(heap.intern(&units)),
                // A control character has to be escaped — writing one through is what §25.5.1
                // refuses and what makes JSON safe to embed in a line-oriented format.
                0x00..=0x1F => return Err(bad_json()),
                0x5C => units.push(self.escape()?),
                unit => units.push(unit),
            }
        }
    }

    /// One escape sequence, after the backslash.
    fn escape(&mut self) -> Completion<u16> {
        let Some(kind) = self.peek() else {
            return Err(bad_json());
        };
        self.at += 1;
        let unit = match kind {
            0x22 | 0x5C | 0x2F => kind,
            0x62 => 0x08,
            0x66 => 0x0C,
            0x6E => 0x0A,
            0x72 => 0x0D,
            0x74 => 0x09,
            0x75 => return self.hex(),
            // No `\x`, no `\0`, no line continuation: JSON's list is shorter than JavaScript's.
            _ => return Err(bad_json()),
        };
        Ok(unit)
    }

    /// The four hexadecimal digits of a `\u` escape.
    fn hex(&mut self) -> Completion<u16> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let Some(unit) = self.peek() else {
                return Err(bad_json());
            };
            let Some(digit) = char::from_u32(u32::from(unit)).and_then(|c| c.to_digit(16)) else {
                return Err(bad_json());
            };
            self.at += 1;
            value = value * 16 + digit;
        }
        // Four hexadecimal digits cannot exceed `u16`, and a lone surrogate is *allowed* here —
        // §25.5.1 has no pairing rule, which is why a JSON string may hold one.
        Ok(value as u16)
    }

    /// `JSONNumber`, which is a narrower grammar than a JavaScript numeric literal.
    fn number(&mut self) -> Completion<Value> {
        let start = self.at;
        if self.peek() == Some(0x2D) {
            self.at += 1;
        }
        // A leading zero may not be followed by another digit: `01` is not JSON.
        match self.peek() {
            Some(0x30) => self.at += 1,
            Some(0x31..=0x39) => self.digits(),
            _ => return Err(bad_json()),
        }
        if self.peek() == Some(0x2E) {
            self.at += 1;
            // …and a point must be followed by at least one digit, so `1.` is not JSON either.
            if !matches!(self.peek(), Some(0x30..=0x39)) {
                return Err(bad_json());
            }
            self.digits();
        }
        if matches!(self.peek(), Some(0x65 | 0x45)) {
            self.at += 1;
            if matches!(self.peek(), Some(0x2B | 0x2D)) {
                self.at += 1;
            }
            if !matches!(self.peek(), Some(0x30..=0x39)) {
                return Err(bad_json());
            }
            self.digits();
        }
        // The units read are a decimal literal by construction, so §7.1.4.1 reads them exactly —
        // and reading them there rather than here is what keeps one rounding in the engine.
        let text = &self.units[start..self.at];
        Ok(Value::Number(crate::value::string_to_number(text)))
    }

    /// Step past a run of decimal digits.
    fn digits(&mut self) {
        while matches!(self.peek(), Some(0x30..=0x39)) {
            self.at += 1;
        }
    }
}
