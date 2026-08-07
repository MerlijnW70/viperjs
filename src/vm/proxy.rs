//! §10.5's internal methods — the four a property access goes through.
//!
//! # Why these live here and not in the heap
//!
//! Every other exotic object in ViperJS is exotic in the *heap*: a String object's characters and a
//! TypedArray's elements are answered without an interpreter, so `Heap::own_property` can
//! synthesise them. A Proxy cannot be. Its answer to `[[Get]]` is whatever a JavaScript function
//! says, so the operation needs the machine that runs JavaScript — and that is why the thirteen
//! internal methods a proxy overrides are the only ones that could not simply become another arm
//! of the heap's dispatch.
//!
//! # The shape every trap shares
//!
//! §10.5.1 through §10.5.13 all begin the same way: take the handler, throw if the proxy has been
//! revoked, look the trap up on the handler, and — if there is none — do the operation on the
//! target instead. Only then does the trap-specific part begin. [`Trapped`] is those four steps,
//! written once.
//!
//! # The invariants, and why they are not optional
//!
//! A trap may lie, and §10.5 lets it right up to the point where the lie would break something a
//! program is entitled to rely on. `[[Get]]` on a non-writable, non-configurable data property of
//! the target **must** answer that property's value: a program that has checked the descriptor is
//! allowed to believe it. Those checks are the difference between a Proxy and an object with
//! callbacks, and they are where a naive implementation is wrong in ways nothing but the suite
//! notices.

use crate::heap::{Heap, ObjectId, PropertyKey, PropertyKind};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// What a trap lookup found: either the function to call, or the target to fall back to.
pub(crate) enum Trapped {
    /// The handler has this trap — call it.
    Handler {
        /// The trap function itself.
        trap: Value,
        /// The handler, which the trap is called *on*.
        handler: Value,
        /// The target, which every trap is handed as its first argument.
        target: ObjectId,
    },
    /// The handler has none, so the operation goes to the target unchanged.
    Target(ObjectId),
}

impl Vm {
    /// §10.5's opening four steps — the trap, or the target to use instead.
    ///
    /// Answers `None` when the object is not a proxy at all, which is what lets every caller ask
    /// unconditionally and pay one `Option` check on the ordinary path.
    ///
    /// # Why this loops
    ///
    /// A proxy's target may be another proxy, and §10.5 says the operation is performed on it — so
    /// `new Proxy(new Proxy(t, {}), {})` reaches `t` through two hops. Written recursively that is
    /// one Rust frame per hop and a program chooses how many, which DR-0002 does not allow. Written
    /// as a loop it is one frame however long the chain, and the answer is identical: each hop
    /// checks revocation and looks for the trap exactly as a recursive call would. A hop that *has*
    /// a trap ends the loop, and any further nesting then happens through a real JavaScript call,
    /// which the call-depth guard already counts.
    pub(crate) fn proxy_trap(
        &mut self,
        object: ObjectId,
        name: &str,
        heap: &mut Heap,
    ) -> Completion<Option<Trapped>> {
        let mut walk = object;
        let mut found_any = false;
        loop {
            let Some(proxy) = heap.object(walk).and_then(crate::heap::Object::proxy) else {
                // The chain ended at an ordinary object. If there was at least one proxy on the
                // way, this is the target the operation goes to; if there was none, the caller was
                // never asking about a proxy at all.
                return Ok(if found_any {
                    Some(Trapped::Target(walk))
                } else {
                    None
                });
            };
            found_any = true;
            // Step 2 — a revoked proxy has neither target nor handler, and every one of its
            // internal methods is a TypeError. This is the check that makes revocation mean
            // something, and it applies at every hop rather than only the first.
            let Some((target, handler)) = proxy.parts() else {
                return Err(Abrupt::type_error("this proxy has been revoked"));
            };
            let key = crate::builtins::key(heap, name);
            let trap = self.get_property_key(Value::Object(handler), key, heap)?;
            // Step 5 — `undefined` **and null** both mean "no trap", and anything else that is not
            // callable is a TypeError rather than a silent fall through to the target.
            if matches!(trap, Value::Undefined | Value::Null) {
                walk = target;
                continue;
            }
            if !heap.is_callable(trap) {
                return Err(Abrupt::type_error("this proxy trap is not a function"));
            }
            return Ok(Some(Trapped::Handler {
                trap,
                handler: Value::Object(handler),
                target,
            }));
        }
    }

    /// The target's own property, as the invariant checks need to see it.
    ///
    /// Only a **non-configurable** property can constrain a trap, so anything else answers `None`
    /// and the trap is believed. That is not a shortcut: §10.5.8's invariants are written about
    /// exactly the properties a program could have observed and been promised would not move.
    fn fixed_own(
        &mut self,
        target: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Option<crate::heap::Property>> {
        // `[[GetOwnProperty]]` and not the heap's table, because a trap's target may itself be a
        // proxy — `new Proxy(new Proxy(t, h), {get: …})` reports the inner proxy as its target,
        // and asking the table would find nothing and check no invariant at all.
        Ok(self
            .own_property_through(target, key, heap)?
            .filter(|found| !found.configurable))
    }

    /// §10.5.8 `[[Get]]` through a proxy.
    pub(crate) fn proxy_get(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        receiver: Value,
        heap: &mut Heap,
    ) -> Completion<Option<Value>> {
        let Some(trapped) = self.proxy_trap(object, "get", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return self
                    .get_through(Value::Object(target), key, receiver, heap)
                    .map(Some);
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        let answer = self.call_value(
            trap,
            handler,
            &[Value::Object(target), named, receiver],
            heap,
        )?;
        // Step 10 — the trap may lie, but not about a property the target has promised will not
        // move. A non-writable, non-configurable data property must be reported as it is, because
        // a program that read the descriptor is entitled to believe it.
        if let Some(found) = self.fixed_own(target, key, heap)? {
            match found.kind {
                PropertyKind::Data {
                    value,
                    writable: false,
                } if !answer.same_value(&value, heap) => {
                    return Err(Abrupt::type_error(
                        "a proxy get trap answered something other than the target's fixed value",
                    ));
                }
                // …and an accessor with no getter reads as `undefined` however the trap answers.
                PropertyKind::Accessor { getter, .. }
                    if matches!(getter, Value::Undefined)
                        && !matches!(answer, Value::Undefined) =>
                {
                    return Err(Abrupt::type_error(
                        "a proxy get trap answered a value for a property that has no getter",
                    ));
                }
                _ => {}
            }
        }
        Ok(Some(answer))
    }

    /// §10.5.9 `[[Set]]` through a proxy.
    pub(crate) fn proxy_set(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        value: Value,
        receiver: Value,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "set", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            // §10.5.9 step 6 — `Return ? target.[[Set]](P, V, Receiver)`, and the `Return` is the
            // whole of it. A proxy with no `set` trap is not a proxy that always succeeds; it is one
            // whose answer is the target's. ViperJS ran the write and reported `true` regardless, so
            // `Reflect.set(new Proxy(sealed, {}), 'x', 1)` said the write happened when nothing had
            // been written anywhere.
            //
            // Every sibling here already forwards its answer — `[[Delete]]` below is the model.
            // This was the one that did not, and it is invisible until the target refuses.
            Trapped::Target(target) => {
                let answered =
                    self.set_through(Value::Object(target), key, value, receiver, heap)?;
                return Ok(Some(answered.to_boolean(heap)));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        let answer = self.call_value(
            trap,
            handler,
            &[Value::Object(target), named, value, receiver],
            heap,
        )?;
        if !answer.to_boolean(heap) {
            return Ok(Some(false));
        }
        // Step 9 — a trap that claims a write succeeded may not claim it about a property the
        // target has fixed at another value.
        if let Some(found) = self.fixed_own(target, key, heap)? {
            match found.kind {
                PropertyKind::Data {
                    value: held,
                    writable: false,
                } if !value.same_value(&held, heap) => {
                    return Err(Abrupt::type_error(
                        "a proxy set trap reported success for a value the target fixes otherwise",
                    ));
                }
                PropertyKind::Accessor {
                    setter: Value::Undefined,
                    ..
                } => {
                    return Err(Abrupt::type_error(
                        "a proxy set trap reported success for a property that has no setter",
                    ));
                }
                _ => {}
            }
        }
        Ok(Some(true))
    }

    /// §10.5.7 `[[HasProperty]]` through a proxy.
    pub(crate) fn proxy_has(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "has", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return self
                    .has_property_key(Value::Object(target), key, heap)
                    .map(Some);
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        let answer = self.call_value(trap, handler, &[Value::Object(target), named], heap)?;
        if answer.to_boolean(heap) {
            return Ok(Some(true));
        }
        // Step 11.c — denying a property the target has and cannot lose is a lie a program could
        // catch, so the specification catches it first. **Two ways to be unlosable**, and only the
        // first was here: step 11.c.i is a non-configurable own property, and step 11.c.iv is a
        // target that is not extensible at all — where even a *configurable* property cannot be
        // removed and put back, so denying it is the same lie.
        //
        // Asked in that order because 11.c.i needs no second call, and because `Object.freeze`
        // reaches both: a frozen target's properties are non-configurable *and* the object is not
        // extensible, so a check for one of the two looks complete against the obvious test.
        let Some(found) = self.own_property_through(target, key, heap)? else {
            return Ok(Some(false));
        };
        if !found.configurable {
            return Err(Abrupt::type_error(
                "a proxy has trap denied a property the target cannot remove",
            ));
        }
        if !self.extensible_through(target, heap)? {
            return Err(Abrupt::type_error(
                "a proxy has trap denied a property of a target that is not extensible",
            ));
        }
        Ok(Some(false))
    }

    /// §10.5.10 `[[Delete]]` through a proxy.
    pub(crate) fn proxy_delete(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        heap: &mut Heap,
    ) -> Completion<Option<bool>> {
        let Some(trapped) = self.proxy_trap(object, "deleteProperty", heap)? else {
            return Ok(None);
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                let answer = self.delete_property_key(Value::Object(target), key, heap)?;
                return Ok(Some(answer.to_boolean(heap)));
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let named = Self::key_as_value(key);
        let answer = self.call_value(trap, handler, &[Value::Object(target), named], heap)?;
        if !answer.to_boolean(heap) {
            return Ok(Some(false));
        }
        // Step 11 — a property the target cannot lose cannot be reported as deleted.
        if self.fixed_own(target, key, heap)?.is_some() {
            return Err(Abrupt::type_error(
                "a proxy deleteProperty trap removed a property the target cannot lose",
            ));
        }
        Ok(Some(true))
    }

    /// A property key as the value a trap is handed — a String or a Symbol, never anything else.
    ///
    /// The last arm is a key that is neither, which §6.2.2 does not have: a `PropertyKey` is one or
    /// the other by construction. Answering `undefined` keeps the promise that nothing here panics
    /// on a shape the types cannot rule out.
    pub(super) fn key_as_value(key: PropertyKey) -> Value {
        match (key.as_symbol(), key.as_string()) {
            (Some(symbol), _) => Value::Symbol(symbol),
            (None, Some(text)) => Value::String(text),
            (None, None) => Value::Undefined,
        }
    }
}
