//! §10.5.12 and §10.5.13 — the two internal methods a proxy has only *sometimes*.
//!
//! # Why callability is decided once, at construction
//!
//! §10.5 gives a proxy a `[[Call]]` only if the initial value of `[[ProxyTarget]]` had one, and the
//! same for `[[Construct]]`. So `typeof new Proxy(function () {}, {})` is `"function"` and
//! `typeof new Proxy({}, {})` is `"object"` — decided when the proxy is made and never revisited,
//! which is why an `apply` trap on a handler whose target is a plain object does nothing at all.
//!
//! That is also what lets a proxy be an ordinary callable to the rest of the engine: the object
//! carries a `[[Call]]` like any other function, and the call machinery needs to know nothing about
//! proxies. Only the body it runs is different.
//!
//! # What a revoked callable proxy does
//!
//! Keeps its `[[Call]]` and throws from it. Revocation empties the target and handler; it does not
//! make the object stop being a function, so `typeof` still says `"function"` and calling it is a
//! TypeError. Both halves of that are observable and neither is an accident.

use crate::heap::{Heap, ObjectId};
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;
use crate::vm::proxy::Trapped;

impl Vm {
    /// §10.5.12 `[[Call]]` — the `apply` trap, or the target called directly.
    pub(crate) fn proxy_call(
        &mut self,
        object: ObjectId,
        this_value: Value,
        arguments: &[Value],
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Some(trapped) = self.proxy_trap(object, "apply", heap)? else {
            // Only reachable if this object stopped being a proxy, which nothing does. Calling the
            // target is the answer that stays true if it ever became reachable.
            return Err(Abrupt::type_error("this proxy has no target to call"));
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                return self.call_value(Value::Object(target), this_value, arguments, heap);
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        // Step 7 — the arguments arrive as an **array**, not as an argument list. That is what
        // makes an `apply` trap writable as `function (t, self, args) { … }` whatever the arity of
        // what it stands in front of.
        let listed = crate::builtins::array::from_values(self, heap, arguments)?;
        self.call_value(
            trap,
            handler,
            &[Value::Object(target), this_value, listed],
            heap,
        )
    }

    /// §10.5.13 `[[Construct]]` — the `construct` trap, or the target constructed directly.
    pub(crate) fn proxy_construct(
        &mut self,
        object: ObjectId,
        arguments: &[Value],
        new_target: Value,
        heap: &mut Heap,
    ) -> Completion<Value> {
        let Some(trapped) = self.proxy_trap(object, "construct", heap)? else {
            return Err(Abrupt::type_error("this proxy has no target to construct"));
        };
        let (trap, handler, target) = match trapped {
            Trapped::Target(target) => {
                // The `new.target` is passed through unchanged, so a proxy in front of a class
                // still builds an instance of whatever the `new` actually named. Where the proxy
                // *is* what was named, that is the proxy — and the target then reads `prototype`
                // off it, which §10.5.8 answers from the target. So the ordinary case works
                // without the proxy having a `prototype` property of its own.
                return self.construct_with_target(
                    Value::Object(target),
                    new_target,
                    arguments,
                    heap,
                );
            }
            Trapped::Handler {
                trap,
                handler,
                target,
            } => (trap, handler, target),
        };
        let listed = crate::builtins::array::from_values(self, heap, arguments)?;
        let answer = self.call_value(
            trap,
            handler,
            &[Value::Object(target), listed, new_target],
            heap,
        )?;
        // Step 9 — a `construct` trap must answer an object. `new` evaluates to whatever this
        // says, and a primitive there would make `new p()` a number, which no other construction
        // in the language can be. A trap that forgets to return is the common case and is caught
        // here rather than silently answering `undefined`.
        if !matches!(answer, Value::Object(_)) {
            return Err(Abrupt::type_error(
                "a proxy construct trap answered something that is not an object",
            ));
        }
        Ok(answer)
    }
}
