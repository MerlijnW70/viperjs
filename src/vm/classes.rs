//! §15.7's classes, and the private names that only a class can declare.
//!
//! Split from [`super::execute`] because the loop and this are two different readings. The loop is
//! one `match` over instructions; these are the several hundred lines that one of its arms — the
//! one that builds a class — needs, together with the three private-name operations, which are not
//! property lookups however much `o.#x` looks like one.
//!
//! `Vm`'s fields are private to `vm` and this is a module inside it, so these reach them directly.

use super::execute::property_name;
use super::{Fault, Vm};
use crate::compile::{Chunk, Instruction};
use crate::heap::{Heap, PropertyDescriptor};
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    pub(super) fn make_class(
        &mut self,
        body: &Rc<Chunk>,
        derived: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<Option<crate::heap::ObjectId>, Fault> {
        // §15.7.14 steps 9 to 11 — the heritage read three ways. `extends null` is a
        // class whose instances inherit from nothing, and whose constructor still
        // inherits from `Function.prototype`; anything that is not a constructor is a
        // TypeError, and `extends {}` is caught by that rather than by the step below.
        let inheritance = match derived {
            false => Inheritance {
                prototype: Some(self.realm.object_prototype()),
                constructor: self.realm.function_prototype(),
            },
            true => match self.inheritance(heap) {
                Ok(found) => found,
                Err(error) => {
                    self.raise(error, heap, root, current, at)?;
                    return Ok(None);
                }
            },
        };
        let object = heap.new_function(
            inheritance.constructor,
            body.clone(),
            self.environment,
            None,
            self.realm.id(),
        );
        let key = property_name(heap, "length");
        heap.define_own_property(
            object,
            key,
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Number(body.length() as f64)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
        // §10.2.9 `SetFunctionName` — not writable, not enumerable, and *configurable*,
        // which is the set §10.3.3 gives `length` beside it. An unnamed function gets the
        // empty string rather than no property at all: `(function () {}).name` is `""`, and
        // `'name' in f` is true for every function.
        let named = match body.name() {
            Some(text) => Value::String(text),
            None => Value::String(heap.intern(&[])),
        };
        let key = property_name(heap, "name");
        heap.define_own_property(
            object,
            key,
            &PropertyDescriptor {
                value: Some(named),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..PropertyDescriptor::EMPTY
            },
        );
        // DR-0028, and here it is always a no-op: §11.2.2 makes every class body strict, so a class
        // constructor is never the kind that shadows the pair. Called anyway, so that all three
        // sites read alike and a fourth cannot be added without meeting the question.
        crate::builtins::function::reflect_legacy(heap, &self.realm, object);
        // §15.7.14 steps 12 to 14 — the prototype, and the pair of references that make
        // `new C() instanceof C` true. `prototype` is **not writable** here, which is the
        // difference from §10.2.5's `MakeConstructor` for an ordinary function: a class
        // may not be pointed at a different prototype after the fact.
        let prototype = heap.new_object(inheritance.prototype);
        // §15.7.14 step 17 `MakeMethod(F, proto)` — the constructor is a method of the
        // prototype, which is what lets `super.x` be written in it. Set here rather than
        // by an instruction because both objects are only in one place at this moment.
        heap.set_home_object(object, prototype);
        let key = property_name(heap, "constructor");
        heap.define_own_property(
            prototype,
            key,
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(object)),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
        let key = property_name(heap, "prototype");
        heap.define_own_property(
            object,
            key,
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(prototype)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(false),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
        Ok(Some(object))
    }

    /// §15.7.14 — put one method on a class or on its prototype.
    ///
    /// Writable and configurable, and *not* enumerable. That last one is the whole run-time
    /// difference between a class method and the same syntax in an object literal.
    pub(super) fn define_class_method(
        &mut self,
        kind: crate::ast::MethodKind,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let key = self.pop()?;
        let Value::Object(target) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let key = match self.property_key(key, heap) {
            Ok(key) => key,
            Err(error) => {
                self.raise(error, heap, root, current, at)?;
                return Ok(());
            }
        };
        // §15.7.14 — writable and configurable, and *not* enumerable. The last of those
        // is the whole runtime difference from an object literal's method.
        let descriptor = match kind {
            crate::ast::MethodKind::Get => crate::heap::PropertyDescriptor {
                getter: Some(value),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
            crate::ast::MethodKind::Set => crate::heap::PropertyDescriptor {
                setter: Some(value),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
            crate::ast::MethodKind::Normal => crate::heap::PropertyDescriptor {
                value: Some(value),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        };
        // §15.4.5's `DefinePropertyOrThrow`, and the throw is reachable for the same reason
        // §15.7.10's is: a **static** method defines onto the constructor, whose `prototype` is
        // neither writable nor configurable. `static ["prototype"]() {}` is refused here.
        if !heap.define_own_property(target, key, &descriptor) {
            let error = Abrupt::type_error("this property cannot be redefined");
            self.raise(error, heap, root, current, at)?;
        }
        Ok(())
    }

    /// §15.4.5 — put one half of an accessor on the object under construction.
    ///
    /// Only the half that was written: §10.1.6.3 leaves an absent field alone, so a getter defined
    /// after a setter joins it rather than replacing it — which is what makes `{get a() {}, set
    /// a(v) {}}` one property with both halves.
    pub(super) fn define_accessor(
        &mut self,
        getter: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let function = self.pop()?;
        let key = self.pop()?;
        let base = *self.stack.last().ok_or(Fault::StackUnderflow)?;
        let Value::Object(base) = base else {
            return Err(Fault::NotAnObject);
        };
        let key = match self.property_key(key, heap) {
            Ok(key) => key,
            Err(error) => {
                self.raise(error, heap, root, current, at)?;
                return Ok(());
            }
        };
        // Only the half that was written. §10.1.6.3 leaves an absent field alone, so
        // a getter defined after a setter joins it rather than replacing it — which
        // is what makes `{get a() {}, set a(v) {}}` one property with both.
        let half = match getter {
            true => PropertyDescriptor {
                getter: Some(function),
                ..PropertyDescriptor::EMPTY
            },
            false => PropertyDescriptor {
                setter: Some(function),
                ..PropertyDescriptor::EMPTY
            },
        };
        // §15.4.5 gives an accessor made this way `[[Enumerable]]` and
        // `[[Configurable]]`, the same two an ordinary literal property gets.
        let descriptor = PropertyDescriptor {
            enumerable: Some(true),
            configurable: Some(true),
            ..half
        };
        let _ = heap.define_own_property(base, key, &descriptor);
        Ok(())
    }

    /// §7.3.30 `PrivateMethodOrAccessorAdd` — give an object a private method or accessor.
    ///
    /// Out of line, and every method in this block is out of line for one reason: [`Vm::execute`] is a
    /// single `match`, so its Rust frame is the sum of every arm's locals — and §7.1.1's conversions
    /// re-enter the interpreter, paying that frame again per level. `MAX_REENTRY_DEPTH` is a
    /// *measured* number against a one-mebibyte stack, and writing these three inline was enough to
    /// break its margin: `a_conversion_at_the_cap_fits_in_the_stack_it_claims_to_need` found it by
    /// overflowing, which is exactly what that guard is for. `inline` is refused for the same reason,
    /// because a release build that folded them back in would put the frame back with them.
    #[inline(never)]
    pub(super) fn add_private(
        &mut self,
        instruction: Instruction,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let element = match instruction {
            Instruction::AddPrivateAccessor => {
                let setter = self.pop()?;
                let getter = self.pop()?;
                crate::heap::PrivateElement::Accessor { getter, setter }
            }
            // Listed rather than defaulted, so a third kind cannot arrive here unnoticed.
            _ => crate::heap::PrivateElement::Method(self.pop()?),
        };
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        // Peeked, so one target takes element after element.
        let Some(&Value::Object(target)) = self.stack.last() else {
            return Err(Fault::NotAnObject);
        };
        // §7.3.30 step 2 — an existing name is a TypeError, with no exception for an accessor. Its two
        // halves are **one** element built by §15.7.14 at the class definition, so by the time this
        // runs there is one add per name; merging here instead let the same accessor be added to one
        // object twice, which the specification refuses and a re-entered constructor reaches.
        if !heap.add_private_element(target, name, element) {
            self.raise(
                Abrupt::type_error("this object already has that private element"),
                heap,
                root,
                current,
                at,
            )?;
        }
        Ok(())
    }

    /// §7.3.31 `PrivateGet` — read a private field, method or accessor, or throw.
    ///
    /// Out of line; see [`Vm::add_private`].
    #[inline(never)]
    pub(super) fn get_private(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let target = self.pop()?;
        // §7.3.31 step 1 — a primitive carries no private elements, so it fails the same way an
        // object without the name does. No wrapper is made: a wrapper would have none either.
        let found = match target {
            Value::Object(object) => heap
                .object(object)
                .and_then(|held| held.private_element(name)),
            _ => None,
        };
        let Some(element) = found else {
            self.raise(
                Abrupt::type_error("this object has no such private field"),
                heap,
                root,
                current,
                at,
            )?;
            return Ok(());
        };
        // §7.3.31 step 4 — a field or a method answers directly; an accessor's getter is **called**,
        // with the object as its receiver, which is why this cannot be the heap's business alone. A
        // getter-less accessor is a TypeError where a public one would have answered `undefined`.
        let value = match element {
            crate::heap::PrivateElement::Accessor { getter, .. } => {
                if matches!(getter, Value::Undefined) {
                    self.raise(
                        Abrupt::type_error("this private accessor has no getter"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    return Ok(());
                }
                match self.call_value(getter, target, &[], heap) {
                    Ok(value) => value,
                    Err(error) => {
                        self.raise(error, heap, root, current, at)?;
                        return Ok(());
                    }
                }
            }
            // A field and a method both hold one value, and an accessor was answered above.
            held => match held.value() {
                Some(value) => value,
                None => return Err(Fault::NotAnObject),
            },
        };
        self.stack.push(value);
        Ok(())
    }

    /// §7.3.32 `PrivateSet` — write a private field or call a private setter, or throw.
    ///
    /// Out of line; see [`Vm::add_private`].
    #[inline(never)]
    pub(super) fn set_private(
        &mut self,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let value = self.pop()?;
        let Value::Symbol(name) = self.pop()? else {
            return Err(Fault::NotAnObject);
        };
        let target = self.pop()?;
        let element = match target {
            Value::Object(object) => heap
                .object(object)
                .and_then(|held| held.private_element(name)),
            _ => None,
        };
        // §7.3.32 reads the kind before it writes anything, and two of the three never reach the heap:
        // a **method** refuses assignment outright, which is what makes `#m` unlike a field holding a
        // function, and an accessor's setter is called.
        match element {
            Some(crate::heap::PrivateElement::Accessor { setter, .. }) => {
                if matches!(setter, Value::Undefined) {
                    self.raise(
                        Abrupt::type_error("this private accessor has no setter"),
                        heap,
                        root,
                        current,
                        at,
                    )?;
                    return Ok(());
                }
                if let Err(error) = self.call_value(setter, target, &[value], heap) {
                    self.raise(error, heap, root, current, at)?;
                    return Ok(());
                }
            }
            Some(crate::heap::PrivateElement::Method(_)) => {
                self.raise(
                    Abrupt::type_error("a private method cannot be assigned to"),
                    heap,
                    root,
                    current,
                    at,
                )?;
                return Ok(());
            }
            Some(crate::heap::PrivateElement::Field(_)) => {
                let Value::Object(object) = target else {
                    return Err(Fault::NotAnObject);
                };
                if !heap.set_private_field(object, name, value) {
                    return Err(Fault::NotAnObject);
                }
            }
            None => {
                self.raise(
                    Abrupt::type_error("this object has no such private field"),
                    heap,
                    root,
                    current,
                    at,
                )?;
                return Ok(());
            }
        }
        self.stack.push(value);
        Ok(())
    }

    /// §10.2.2's `GetSuperConstructor` — the running function's `[[Prototype]]`.
    ///
    /// Read now rather than captured when the class was defined, because it is *mutable*:
    /// `Object.setPrototypeOf(D, Other)` changes which constructor `super()` reaches, and a class
    /// definition that had recorded the answer would go on calling the old one.
    pub(super) fn super_constructor(&mut self, heap: &Heap) -> Result<Value, crate::value::Abrupt> {
        let running = self.frames.last().and_then(|frame| frame.function);
        // Unreachable from source: the parser makes `super(…)` outside a derived constructor a
        // Syntax Error, and a constructor is always entered through a frame. A hand-written chunk
        // can ask, and this is the honest answer rather than a panic.
        let Some(running) = running else {
            return Err(crate::value::Abrupt::type_error(
                "`super` was called outside a constructor",
            ));
        };
        let parent = heap
            .object(running)
            .and_then(crate::heap::Object::prototype);
        // §10.2.2 step 3 — the parent must be a constructor. `class D extends null {}` arrives here
        // with `Function.prototype`, which is callable and *not* a constructor, so this is where
        // `new D()` on such a class becomes the TypeError §15.7.14 promised at step 9.
        let constructs = parent.is_some_and(|parent| {
            heap.object(parent)
                .and_then(crate::heap::Object::call)
                .is_some_and(crate::heap::Callable::constructs)
        });
        match (parent, constructs) {
            (Some(parent), true) => Ok(Value::Object(parent)),
            _ => Err(crate::value::Abrupt::type_error(
                "the superclass is not a constructor",
            )),
        }
    }

    /// Read the `extends` value on top of the stack as §15.7.14 steps 9 to 11 read it.
    ///
    /// Three cases, and the middle one is the reason this is not a property access: `extends {}` and
    /// `extends 1` are TypeErrors because the value is not a **constructor**, which is a question
    /// about `[[Construct]]` and not about being callable — so `extends Math.max` fails here too,
    /// where `Math.max.prototype` would simply have been `undefined`.
    fn inheritance(&mut self, heap: &mut Heap) -> Result<Inheritance, crate::value::Abrupt> {
        // A missing operand is a chunk that does not make sense rather than a throw, and there is
        // nothing to inherit from either way — the compiler emits the heritage before this.
        let heritage = self.stack.pop().unwrap_or(Value::Undefined);
        // §15.7.14 step 9 — `extends null` is not an error and not the same as no `extends` at all:
        // the class is still *derived*, so its constructor must call `super()`, and `super()` will
        // then find `null` where a constructor should be. That is a run-time TypeError per
        // construction rather than a definition-time one, which is what the specification says.
        if matches!(heritage, Value::Null) {
            return Ok(Inheritance {
                prototype: None,
                constructor: self.realm.function_prototype(),
            });
        }
        // §15.7.14 step 10 — one refusal and not two. "Not an object" and "an object that does not
        // construct" were written as separate questions, and the first of them was not a question:
        // the destructuring that followed already required an Object, so the arm answering `false`
        // for a number could be made to answer `true` without any input noticing. Found by probing
        // this code for the first time — it moved here unchanged from `execute.rs`, where it had
        // been correct and untouched since it was written, and a diff-scoped ratchet never mutates
        // a line that does not change.
        let Value::Object(parent) = heritage else {
            return Err(crate::value::Abrupt::type_error(
                "a class may only extend a constructor or null",
            ));
        };
        if !heap
            .object(parent)
            .and_then(crate::heap::Object::call)
            .is_some_and(crate::heap::Callable::constructs)
        {
            return Err(crate::value::Abrupt::type_error(
                "a class may only extend a constructor or null",
            ));
        }
        // §15.7.14 step 11 — the parent's `prototype` is read with `[[Get]]`, so a getter runs and a
        // Proxy would be consulted. It must be an Object or null; a parent whose `prototype` was
        // replaced with a number is a TypeError, and this is the one place that check lives.
        let key = property_name(heap, "prototype");
        let found = self.get_property_key(Value::Object(parent), key, heap)?;
        let prototype = match found {
            Value::Object(prototype) => Some(prototype),
            Value::Null => None,
            _ => {
                return Err(crate::value::Abrupt::type_error(
                    "the `prototype` of an extended constructor is neither an object nor null",
                ));
            }
        };
        Ok(Inheritance {
            prototype,
            constructor: parent,
        })
    }
}

/// The two prototypes a class definition points its halves at — §15.7.14 steps 9 to 11.
///
/// A pair rather than two values because the three cases decide them *together*: `extends null` sets
/// one to nothing and the other to `Function.prototype`, and nothing sets only one of them.
struct Inheritance {
    /// What instances inherit from — `[[Prototype]]` of the class's `prototype` object.
    ///
    /// `None` for `extends null`, which is the whole reason it is an `Option`: a class whose
    /// instances inherit from nothing at all is legal, and its instances have no `toString`.
    prototype: Option<crate::heap::ObjectId>,
    /// What the constructor itself inherits from, which is how a static method is inherited.
    constructor: crate::heap::ObjectId,
}

/// §10.2.9 `SetFunctionName(F, name, prefix)` — what a computed key calls the function it names.
///
/// The compile-time half of this bakes the name into the body, which a literal key allows and a
/// computed one does not. So this is the same clause reached the other way, and the parts that are
/// *not* a string copy are the reason it is worth writing out:
///
/// - **A Symbol key names the function after its description in brackets** — step 2. `Symbol("t")`
///   gives `"[t]"`, and a Symbol with no description gives the **empty string** rather than `"[]"`,
///   because §20.4's `[[Description]]` distinguishes absent from empty and step 2.b says so.
/// - **The prefix is joined with a space** — step 5 concatenates prefix, U+0020 and the name, so an
///   accessor is `"get x"`. A getter on a Symbol key is `"get [t]"`, brackets and all.
/// - **The property is not writable, not enumerable and configurable**, which is §10.3.3's set and
///   the same one `length` beside it has.
///
/// Cannot fail and does not run any code: the key has already been settled by `SettleKey`, so
/// nothing here calls a `toString`.
pub(super) fn name_function(
    vm: &mut Vm,
    function: Value,
    key: Value,
    prefix: crate::compile::NamePrefix,
    heap: &mut Heap,
) {
    let Value::Object(function) = function else {
        // The compiler emits this only after a function it has just made, so there is nothing else
        // this can be — and nothing to say if a hand-written chunk arranges otherwise.
        return;
    };
    let mut name: Vec<u16> = match key {
        // Step 2 — a Symbol's description in brackets, or nothing at all when it has none.
        Value::Symbol(id) => match heap.symbol(id).and_then(|symbol| symbol.description()) {
            Some(text) => {
                let mut units = vec![u16::from(b'[')];
                units.extend_from_slice(heap.string(text).unwrap_or(&[]));
                units.push(u16::from(b']'));
                units
            }
            None => Vec::new(),
        },
        Value::String(id) => heap.string(id).unwrap_or(&[]).to_vec(),
        // `SettleKey` leaves a String or a Symbol and nothing else can arrive here.
        _ => Vec::new(),
    };
    // Step 5, and the space belongs to the clause rather than to the caller.
    if let Some(word) = match prefix {
        crate::compile::NamePrefix::Plain => None,
        crate::compile::NamePrefix::Get => Some("get "),
        crate::compile::NamePrefix::Set => Some("set "),
    } {
        let mut prefixed: Vec<u16> = word.encode_utf16().collect();
        prefixed.append(&mut name);
        name = prefixed;
    }
    let named = Value::String(heap.intern(&name));
    let slot = property_name(heap, "name");
    let _ = vm;
    heap.define_own_property(
        function,
        slot,
        &PropertyDescriptor {
            value: Some(named),
            writable: Some(false),
            enumerable: Some(false),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        },
    );
}
