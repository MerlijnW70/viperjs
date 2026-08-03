//! Annex B §B.2.3 — the thirteen methods that wrap a string in a tag.
//!
//! `"x".bold()` is `"<b>x</b>"`, and twelve more of the same shape. They are here for the reason
//! §B.2.2's accessors are: DR-0008's line is that an Annex B rule is implemented when strictness
//! alone decides it, and nothing here is conditioned on anything at all. These are ordinary
//! methods that change no grammar, that every engine has, and that a page written in 1997 calls.
//!
//! # One operation, thirteen tables of two strings
//!
//! §B.2.3.2.1 `CreateHTML(string, tag, attribute, value)` is the whole of it, and each method is
//! that operation with a tag and an attribute name filled in. So this file is one function and a
//! table, rather than thirteen near-copies — the difference matters because the *order* of the two
//! conversions is observable and would otherwise have to be got right thirteen times.
//!
//! # What is escaped, and what deliberately is not
//!
//! Step 4.b replaces `"` with `&quot;` in the attribute value **and nothing else**. Not `<`, not
//! `&`, not `>`, and not in the element's content at all: `"<".bold()` is `"<b><</b>"`, which is
//! not valid HTML and is what the specification says. These methods have never been safe to build
//! markup with, and making them safe here would be a divergence dressed up as a kindness —
//! test262 asserts the unescaped `<` in three separate files.

use super::string::{argument_string, characters};
use crate::heap::{Heap, NativeCall};
use crate::value::{Completion, Value};
use crate::vm::Vm;

/// §B.2.3.2.1 `CreateHTML(string, tag, attribute, value)`.
///
/// `attribute` empty is the clause's own test for "this method takes no argument", which is why
/// the nine that do not are the same code path as the four that do rather than a second one.
///
/// The order of the two conversions is the part with teeth. Step 2 converts the **receiver** and
/// step 4.a the **attribute value**, so a receiver whose `toString` throws throws before the
/// argument is looked at — and each of the four attribute-taking methods has a test262 file for
/// each direction.
fn create_html(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    tag: &str,
    attribute: &str,
) -> Completion<Value> {
    // Steps 1 and 2 — `RequireObjectCoercible` and then `ToString`, which is what `characters`
    // does and is why `String.prototype.bold.call(undefined)` is a TypeError.
    let content = characters(vm, heap, call)?;
    let mut html: Vec<u16> = Vec::new();
    html.push(u16::from(b'<'));
    html.extend(tag.encode_utf16());
    if !attribute.is_empty() {
        // Step 4.a — and only now, after the receiver has been converted.
        let value = argument_string(vm, heap, call, 0)?;
        html.push(u16::from(b' '));
        html.extend(attribute.encode_utf16());
        html.push(u16::from(b'='));
        html.push(u16::from(b'"'));
        for unit in value {
            // Step 4.b, the whole of the escaping: a quotation mark becomes `&quot;` so that it
            // cannot close the attribute it is inside. Everything else is copied.
            match unit == u16::from(b'"') {
                true => html.extend("&quot;".encode_utf16()),
                false => html.push(unit),
            }
        }
        html.push(u16::from(b'"'));
    }
    html.push(u16::from(b'>'));
    html.extend(content);
    html.extend("</".encode_utf16());
    html.extend(tag.encode_utf16());
    html.push(u16::from(b'>'));
    Ok(Value::String(heap.intern(&html)))
}

/// Each method, as the tag and attribute name §B.2.3 gives it.
///
/// The tag is not the method's name in nine of the thirteen — `bold` is `b`, `fixed` is `tt`,
/// `italics` is `i`, `anchor` and `link` are both `a` — so this table is the mapping rather than a
/// decoration of one.
macro_rules! html_methods {
    ($(($name:ident, $tag:literal, $attribute:literal)),* $(,)?) => {
        $(
            fn $name(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
                create_html(vm, heap, call, $tag, $attribute)
            }
        )*

        /// Every method this module defines, with the `length` §B.2.3 gives it.
        ///
        /// The length is one for a method that takes an attribute value and zero for one that does
        /// not, which is the same fact the empty attribute name carries — so it is computed from it
        /// rather than written twice and able to disagree.
        pub(super) const METHODS: [(&str, u32, crate::heap::Native); 13] = [
            $((stringify!($name), ($attribute.len() != 0) as u32, $name as crate::heap::Native),)*
        ];
    };
}

html_methods![
    (anchor, "a", "name"),
    (big, "big", ""),
    (blink, "blink", ""),
    (bold, "b", ""),
    (fixed, "tt", ""),
    (fontcolor, "font", "color"),
    (fontsize, "font", "size"),
    (italics, "i", ""),
    (link, "a", "href"),
    (small, "small", ""),
    (strike, "strike", ""),
    (sub, "sub", ""),
    (sup, "sup", ""),
];
