//! Resolving a name — §9.4.2's `ResolveBinding`, as the loop performs it.
//!
//! Split from [`super::execute`] because these seven answer one question between them and the loop
//! answers a hundred. What they have in common is that a *name* is not a property: it is looked up
//! through the environment chain, it may be unresolvable, and whether that is a `ReferenceError` or
//! an `undefined` depends on which of these was asked. `typeof` is the one that tolerates absence,
//! which is why it cannot share a path with the others.
//!
//! `super::dynamic` is the same question when a `with` is in the chain, and `super::global` is
//! where a name falls when it falls off the end of it.
//!
//! `Vm`'s fields are private to `vm` and this is a module inside it, so these reach them directly.

use super::{Fault, Vm};
use crate::compile::Chunk;
use crate::heap::Heap;
use crate::realm::NativeError;
use crate::value::{Abrupt, Value};
use std::rc::Rc;

impl Vm {
    /// [`Instruction::TypeofGlobal`] — §13.5.1.1 step 2, where no such global is `"undefined"`.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    pub(super) fn typeof_global(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let read = match self.global_binding(key, heap) {
            Some(read) => read,
            None => Ok(Value::Undefined),
        };
        let Some(value) = self.settle(read, heap, root, current, at)? else {
            return Ok(());
        };
        let answer = value.type_of(heap);
        let id = heap.new_string(answer.encode_utf16().collect());
        self.stack.push(Value::String(id));
        Ok(())
    }

    /// [`Instruction::TypeofName`] — §13.5.1.1, the one read that answers for a name that is
    /// nowhere.
    ///
    /// Step 2 makes an unresolvable name `"undefined"` rather than the ReferenceError every other
    /// read gives, which is the whole of what distinguishes this from [`Vm::load_name`]. Out of
    /// line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    pub(super) fn typeof_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        let read = self.read_resolved(found, key, strict, heap);
        let settled = self.settle(
            read.map(|found| found.unwrap_or(Value::Undefined)),
            heap,
            root,
            current,
            at,
        )?;
        let Some(value) = settled else {
            return Ok(());
        };
        let answer = value.type_of(heap);
        let id = heap.new_string(answer.encode_utf16().collect());
        self.stack.push(Value::String(id));
        Ok(())
    }

    /// [`Instruction::ResolveName`] — §9.4.2 `ResolveBinding`, once.
    ///
    /// Everything after this reads and writes through what it found, which is the whole of what
    /// §13.15.2 means by "the same reference": a getter that deletes the property between the read
    /// and the write must not send the write to whatever the name would resolve to now.
    ///
    /// Out of line for [`Vm::store_through`]'s reason.
    #[inline(never)]
    pub(super) fn resolve_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        if let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? {
            self.references.push(found);
        }
        Ok(())
    }

    /// [`Instruction::LoadName`] and [`Instruction::LoadNameForCall`] — read a name through the
    /// scope chain, and for the call form push §9.1.1.2.10's `WithBaseObject` under it.
    ///
    /// Out of line for [`Vm::store_through`]'s reason. `for_call` rather than the instruction,
    /// because the two spellings differ in exactly that and passing the enum would make this frame
    /// hold a copy of it.
    #[inline(never)]
    pub(super) fn load_name(
        &mut self,
        index: u32,
        for_call: bool,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        // Read before `settle_resolution`, which borrows the `current` that `running` came from —
        // the same hoist the two store sites make.
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        // Pushed *first* so the stack reads as a method call's — receiver, callee, arguments — and
        // only for the call form.
        if for_call {
            self.stack.push(Vm::with_base(found));
        }
        match self.read_resolved(found, key, strict, heap) {
            Ok(Some(value)) => self.stack.push(value),
            // §6.2.5.5 — nothing anywhere is the ReferenceError an ordinary unresolvable name
            // gets, said in the same words by the same code.
            Ok(None) => {
                let message = self.missing_global(key, heap);
                let thrown = self.realm.error(heap, NativeError::Reference, &message);
                self.unwind(thrown, root, current, at)?;
            }
            Err(error) => {
                let thrown = self.thrown_value(error, heap);
                self.unwind(thrown, root, current, at)?;
            }
        }
        Ok(())
    }

    /// [`Instruction::StoreName`] — a store to a name resolved through the scope chain.
    ///
    /// Out of line for [`Vm::store_through`]'s reason, which is written out there.
    #[inline(never)]
    pub(super) fn store_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let strict = running.is_strict();
        let Some(found) = self.settle_resolution(&name, key, heap, root, current, at)? else {
            return Ok(());
        };
        let stored = self.store_dynamic(found, key, value, strict, heap);
        if let Some(Value::Boolean(false)) =
            self.settle(stored.map(Value::Boolean), heap, root, current, at)?
        {
            // Nowhere in the chain: §6.2.5.6 puts it on the global object, which is what
            // `StoreGlobal` does and is reached here by the same call.
            let global = Value::Object(self.realm.global());
            let stored = self.set_property_key(global, key, value, heap);
            self.settle(stored, heap, root, current, at)?;
        }
        Ok(())
    }

    /// §9.1.1.1.5 `SetMutableBinding` — assign to a slot the compiler placed.
    ///
    /// Step 2's dead zone is the whole of what is left for run time: assigning to a binding that is
    /// not initialised yet is a ReferenceError and not a way to initialise it. `let x = x` reads the
    /// dead zone and `x = 1; let x;` writes to it; both are errors, and only `Initialise` may fill
    /// an empty slot.
    pub(super) fn store_variable(
        &mut self,
        depth: u32,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        // Peeked, not popped: assignment is an expression, and `a = (b = 1)` needs
        // the inner one to leave its value behind.
        let value = *self.stack.last().ok_or(Fault::StackUnderflow)?;
        let target = heap
            .environment_at(self.environment, depth)
            .ok_or(Fault::MissingLocal)?;
        // §9.1.1.1.5 `SetMutableBinding` step 2 — assigning to a binding that is not
        // initialised yet is a ReferenceError, not a way to initialise it. `let x = x`
        // reads the dead zone; `x = 1; let x;` writes to it, and both are errors.
        // Only `Initialise` above may fill an empty slot.
        if heap
            .variable(target, index)
            .ok_or(Fault::MissingLocal)?
            .is_none()
        {
            self.raise(
                Abrupt::reference_error(
                    "a `let` or `const` was assigned to before its declaration ran",
                ),
                heap,
                root,
                current,
                at,
            )?;
            return Ok(());
        }
        if !heap.set_variable(target, index, value) {
            return Err(Fault::MissingLocal);
        }
        Ok(())
    }

    /// §13.5.1.2 — delete a bare name, when where it lives is only known at run time.
    ///
    /// The same walk a read of the name makes, and one of three answers depending on where it
    /// lands. Emitted only inside a `with`: everywhere else the compiler already knows which of the
    /// three applies and emits that answer as a constant.
    pub(super) fn delete_name(
        &mut self,
        index: u32,
        heap: &mut Heap,
        root: &Chunk,
        current: &mut Option<Rc<Chunk>>,
        at: &mut usize,
    ) -> Result<(), Fault> {
        let running: &Chunk = current.as_deref().unwrap_or(root);
        let key = self.global_name(running, index, heap)?;
        let name = self.name_text(running, index, heap)?;
        let found = match self.settle_resolution(&name, key, heap, root, current, at)? {
            Some(found) => found,
            None => return Ok(()),
        };
        let answer = match found {
            // §9.1.1.1.7 `DeleteBinding` — a declarative binding is not deletable whatever it
            // is (a `var`, a parameter, a `let`, a function's own slot) **except** the one
            // §19.2.1.1 step 16.b.ii.1 creates with `D` true, which is a direct eval's `var`.
            // `Heap::delete_in` is the clause and answers for both cases; this said ViperJS
            // did not implement the exception, which was true until it was.
            crate::vm::dynamic::Resolved::Slot {
                environment, index, ..
            } => heap.delete_in(environment, index),
            // §9.1.1.2.7 `DeleteBinding` — `[[Delete]]` of the `with` object, which may
            // run a proxy's trap and so may throw. Own-only, like every `[[Delete]]`:
            // `with (o) { delete toString }` answers true and leaves
            // `Object.prototype.toString` where it is, because `o` never had it.
            crate::vm::dynamic::Resolved::Property(object) => {
                let gone = self.delete_property_key(Value::Object(object), key, heap);
                match self.settle(gone, heap, root, current, at)? {
                    Some(value) => value.to_boolean(heap),
                    None => return Ok(()),
                }
            }
            // §13.5.1.2 step 3 — nowhere in the chain, so the global object answers, and
            // §10.1.10.1 step 2 makes a property that is not there **true**.
            crate::vm::dynamic::Resolved::Global => {
                heap.delete_own_property(self.realm.global(), key)
            }
        };
        self.stack.push(Value::Boolean(answer));
        Ok(())
    }
}
