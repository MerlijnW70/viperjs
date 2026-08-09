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
/// inside. 64 is the number the parser's caps use — `MAX_NESTING_DEPTH` and
/// `MAX_EXPRESSION_DEPTH` — and it sits at a quarter of the narrowest of those, which is the margin
/// they do not have and this one can afford. `MAX_REENTRY_DEPTH` was in that list and is **32**: it
/// came down when a macOS runner overflowed at 64, and this comment went on naming it as one of the
/// three for some time afterwards.
///
/// It costs nothing measurable: no JSON in test262 nests past a handful, and data that nests past
/// 64 is machine-generated. Like `MAX_NESTING_DEPTH` this should become an embedder's number, when
/// there is somebody who knows how much stack there actually is.
pub(super) const MAX_JSON_DEPTH: u32 = 64;

/// The refusal all three share — §25.5 has no error for this, so ViperJS picks one and explains it.
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
    let Some(symbol) = heap.well_known(super::well_known_at("toStringTag")) else {
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
/// walked ViperJS off the end of the stack.
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
        // §25.5.1.1 step 2.a — `IsArray`, which for a proxy asks its target through the chain
        // rather than looking at the object in front of it.
        let names = match heap.is_array_through(object)? {
            // Step 2.b — an array is walked by **index**, from `0` to
            // `ToLength(Get(val, "length"))`. Not by its own keys: the two differ on a sparse
            // array, whose holes are visited and revived as `undefined`, and on an array whose
            // `length` a getter or a proxy answers for — reading it is an observable step that
            // may throw, and step 2.b.ii is where that throw comes from.
            true => {
                let length = key(heap, "length");
                let length = vm.get_property_key(Value::Object(object), length, heap)?;
                let length = super::array_methods::to_length(vm.to_number(length, heap)?);
                (0..length)
                    .map(|at| super::array_methods::index_key(heap, at))
                    .collect()
            }
            // Step 2.c — everything else by `EnumerableOwnPropertyNames(val, key)`, which is two
            // conditions and not one: a Symbol key is not a name the document could have had, and
            // a **non-enumerable** property is not the walk's to visit. Asking for every own key
            // instead is what sent this into a function's `length` and `name`.
            false => {
                let mut listed = Vec::new();
                for found in vm.own_keys_through(object, heap)? {
                    if !found.is_spellable() {
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
        for key in names {
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
            // The walk visits as many elements as `length` said, and a document is not bounded by
            // anything else — DR-0013's budget is what stops `{length: 2 ** 53}` from being a hang
            // rather than a refusal.
            super::array_methods::within_budget(vm, heap)?;
        }
    }
    let name = Value::String(name.spelling(heap).unwrap_or_else(|| heap.intern(&[])));
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
