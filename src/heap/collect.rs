//! Mark and sweep — the collector DR-0010 was shaped around.
//!
//! # Why not reference counting
//!
//! DR-0010's argument, and it is worth repeating where the alternative would go: `Rc` frees at
//! zero and never frees a cycle, and JavaScript makes cycles before any user code runs.
//! `f.prototype.constructor === f` is one, and every function has it — so a counting collector
//! would leak every function ever made. Marking does not care: a cycle nothing reaches is
//! unreachable, and unreachable is exactly what it looks for.
//!
//! # What a root is
//!
//! Everything a running program can still name. That is not something the heap can work out for
//! itself — the interpreter has the stack, the frames and the environment it is running in — so
//! the roots are handed in. A collector that guessed would be a collector that freed something
//! still in use, and no amount of testing finds that reliably.
//!
//! # The generation counter DR-0010 deferred, and why there is still none
//!
//! DR-0010 left it out and said the sweep would decide with evidence. This is that decision, and
//! the answer is that it is still not needed — because a freed slot is never *reused*. Sweeping
//! empties a slot and leaves the hole; the arena only grows. A stale handle therefore addresses
//! an empty slot and answers `None`, which is the same narrow promise every handle already makes.
//!
//! A free list would change that, and would need a generation the same day: without one, a reused
//! slot turns a stale handle into a use-after-free with the types intact — a wrong answer rather
//! than a crash, which is the worse of the two. Reusing slots is an M8 experiment, and this is the
//! note that says what it costs.
//!
//! # What is not here
//!
//! Any decision about *when* to collect. §9.10's note leaves that to the implementation entirely,
//! and picking a moment needs a measurement of what allocation costs — an M8 experiment. What is
//! here is the operation, and an embedder that calls it.

use crate::heap::{EnvironmentId, Heap, Object, ObjectId, PropertyKind, StringId, Weak};
use crate::value::Value;

/// Whether the walk reached this value, for the two questions weakness asks about a key.
///
/// A key that is neither an Object nor a Symbol cannot be held weakly at all — §7.2.10 refuses to
/// store one — so the last arm is a shape no weak collection contains. It answers "reachable",
/// which keeps the entry: of the two ways to be wrong about a value that cannot be there, keeping
/// something alive too long is the one that is not a use-after-free.
fn reachable(value: Value, marked: &Marked) -> bool {
    match value {
        Value::Object(id) => marked.objects.get(id.index()).copied().unwrap_or(false),
        Value::Symbol(id) => marked.symbols.get(id.index()).copied().unwrap_or(false),
        _ => true,
    }
}

/// Everything a running program can still reach, handed to the collector by its owner.
///
/// Deliberately explicit. The heap cannot see the interpreter's stack, and an interpreter that
/// forgot to mention it would have its values freed underneath it — so this is one struct with one
/// field per place a value can be, and adding a place is a change the compiler asks about.
#[derive(Debug, Default)]
pub struct Roots {
    /// Values on an operand stack, in a constant table, or held by an embedder.
    pub values: Vec<Value>,
    /// Environments a frame or a closure can still reach.
    pub environments: Vec<EnvironmentId>,
}

/// What a collection freed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collected {
    /// How many objects were unreachable.
    pub objects: usize,
    /// How many environments were unreachable.
    pub environments: usize,
    /// How many Strings were unreachable.
    pub strings: usize,
    /// How many Symbols were unreachable.
    pub symbols: usize,
    /// How many BigInts were unreachable.
    pub bigints: usize,
}

impl Heap {
    /// Free everything `roots` cannot reach, and answer how much that was.
    ///
    /// # What survives
    ///
    /// Whatever is reachable, by any path. From a root value to an object, from an object to its
    /// prototype and to every value in every property, from a function to the environment it was
    /// written in, from an environment to its parent and to every variable in it. A cycle among
    /// them survives if anything outside reaches it and is freed if nothing does, which is the
    /// whole point.
    ///
    /// The intern table is *not* a root. A property name nothing uses any more should go, and
    /// keeping the table strong would pin every name a program ever computed — which is the leak
    /// [`Heap::intern`] warned about.
    pub fn collect(&mut self, roots: &Roots) -> Collected {
        let mut marked = Marked {
            objects: vec![false; self.objects.len()],
            environments: vec![false; self.environments.len()],
            strings: vec![false; self.strings.len()],
            bigints: vec![false; self.bigints.len()],
            symbols: vec![false; self.symbols.len()],
        };
        for value in &roots.values {
            self.mark_value(*value, &mut marked);
        }
        // §20.4.2.2's registry holds its Symbols for as long as the process runs, so it is a root
        // and not a table to be pruned: `Symbol.for("a")` must answer the same Symbol however long
        // ago the last other holder of it went away.
        for (key, symbol) in &self.registry {
            if let Some(seen) = marked.symbols.get_mut(symbol.index()) {
                *seen = true;
            }
            if let Some(seen) = marked.strings.get_mut(key.index()) {
                *seen = true;
            }
        }
        for environment in &roots.environments {
            self.mark_environment(*environment, &mut marked);
        }
        // Last, because it can only be answered once everything else has been: a weak entry's
        // value is reachable exactly when its key is, and whether the key is reachable is what the
        // walk above was working out.
        self.mark_weak_entries(&mut marked);
        self.sweep(&marked)
    }

    /// Mark what §24.3's weak collections keep alive, which is less than what they hold.
    ///
    /// A `WeakMap` entry keeps its **value** alive for as long as its **key** is alive, and keeps
    /// neither alive on its own. That is an ephemeron, and it cannot be settled in one pass: the
    /// value of a live entry may itself be the key of an entry in another weak map, whose value is
    /// the key of a third, and marking the first is what makes the second live. So this repeats
    /// until a pass marks nothing new.
    ///
    /// It terminates because `settled` only ever grows and there are finitely many entries — a
    /// pass that adds nothing to it ends the loop.
    ///
    /// A `WeakSet` has nothing to mark, because its entry *is* its key and the key is exactly what
    /// must not be kept alive. It is walked all the same, and the marking is a no-op on a key that
    /// is already marked — one branch fewer than saying so, and the sweep is where a weak set's
    /// entries actually go.
    fn mark_weak_entries(&self, marked: &mut Marked) {
        let mut settled: std::collections::HashSet<(usize, usize)> =
            std::collections::HashSet::new();
        loop {
            let mut grew = false;
            for (slot, object) in self.objects.iter().enumerate() {
                let Some(collection) = object.as_ref().and_then(Object::collection) else {
                    continue;
                };
                if !collection.kind().weak() {
                    continue;
                }
                for (at, (key, value)) in collection.live_entries().enumerate() {
                    if settled.contains(&(slot, at)) || !reachable(key, marked) {
                        continue;
                    }
                    settled.insert((slot, at));
                    self.mark_value(value, marked);
                    grew = true;
                }
            }
            if !grew {
                return;
            }
        }
    }

    /// Mark a value and everything it leads to.
    fn mark_value(&self, value: Value, marked: &mut Marked) {
        match value {
            Value::String(id) => {
                if let Some(seen) = marked.strings.get_mut(id.index()) {
                    *seen = true;
                }
            }
            // §6.1.6.2's magnitude is the program's to size, so a BigInt nothing names is worth
            // reclaiming for the same reason a String is — and unlike a String it is never
            // interned, so nothing else is holding it.
            Value::BigInt(id) => {
                if let Some(seen) = marked.bigints.get_mut(id.index()) {
                    *seen = true;
                }
            }
            Value::Symbol(id) => {
                if let Some(seen) = marked.symbols.get_mut(id.index()) {
                    *seen = true;
                }
                // …and its description, which is a String nothing else may be holding.
                if let Some(description) = self.symbol_description(id)
                    && let Some(seen) = marked.strings.get_mut(description.index())
                {
                    *seen = true;
                }
            }
            Value::Object(id) => self.mark_object(id, marked),
            // A primitive that is neither a String nor a Symbol leads nowhere: it *is* its value.
            Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => {}
        }
    }

    /// Mark an object, its prototype, its properties and the environment it closed over.
    ///
    /// Iterative rather than recursive. An object graph is as deep as a program makes it — a list
    /// of a million links is a chain of a million objects — and recursing would run out of Rust
    /// stack on data rather than on nesting. DR-0002 again: a collector that crashes on a long
    /// list is not a collector.
    fn mark_object(&self, from: ObjectId, marked: &mut Marked) {
        let mut pending = vec![from];
        while let Some(id) = pending.pop() {
            match marked.objects.get_mut(id.index()) {
                // Already marked, so its edges have been walked. This is also what makes a cycle
                // terminate rather than spin.
                Some(true) | None => continue,
                Some(seen) => *seen = true,
            }
            let Some(object) = self.object(id) else {
                continue;
            };
            if let Some(prototype) = object.prototype() {
                pending.push(prototype);
            }
            if let Some(environment) = object.environment() {
                self.mark_environment(environment, marked);
            }
            // An arrow's captured `this` is reachable *through the arrow*, and nothing else may
            // be holding it: `function F() { return () => this; }` leaves the constructed object
            // alive only because the arrow it returned points at it.
            // An arguments object is the one thing that can outlive its call and still be reading
            // its variables: `function f(a) { return arguments; }` hands back an object whose
            // `[0]` *is* `a`, so the environment has to survive as long as the object does.
            if let Some(map) = object.arguments_map() {
                self.mark_environment(map.environment(), marked);
            }
            // What this object is callable *as*, which four of the five shapes make reachable and
            // nothing else does. A bound function is the plain one: `f.bind(o, x)` leaves `f`, `o`
            // and `x` named by the bound function alone, so a collector that skipped this would
            // free the target of a function still sitting in a variable.
            //
            // The compiled body is the one that is easy to miss, because what it names is not
            // values on the object but Strings in its *constant table* — every literal the body
            // mentions, its own name, and both halves of every tagged template. See
            // [`Chunk::names`], which is in `compile` so that a field added to a chunk cannot be
            // forgotten here.
            match object.call() {
                Some(crate::heap::Callable::Bytecode(chunk)) => {
                    let mut named = Vec::new();
                    chunk.names(&mut named);
                    for value in named {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                Some(crate::heap::Callable::Bound(bound)) => {
                    pending.push(bound.target);
                    for value in std::iter::once(bound.this_value).chain(bound.arguments.clone()) {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                // §27.7.5.3's two closures name the execution they revive, and that context object
                // is where the parked body and the promise it settles live.
                Some(crate::heap::Callable::Revive { context, .. }) => pending.push(*context),
                // A native holds a function pointer and a resumption holds a kind; neither names
                // anything on the heap. Listed rather than swept into a catch-all, so that a sixth
                // shape carrying a value cannot arrive here unnoticed.
                Some(
                    crate::heap::Callable::Native { .. } | crate::heap::Callable::Resume { .. },
                )
                | None => {}
            }
            // A wrapper's primitive can be a String, and nothing else need be holding it: the
            // only reference to `new String('x')`'s contents is the wrapper itself.
            match object.primitive() {
                Some(Value::Object(reached)) => pending.push(reached),
                Some(other) => self.mark_value(other, marked),
                None => {}
            }
            // A private field's value and the Private Name it is filed under. Neither is reachable
            // any other way: the name lives in a compiler slot no script can spell, and the value is
            // in a list that no property walk visits — so a collector that skipped this would free
            // what `this.#x` is about to answer with.
            for (name, element) in object.private_elements() {
                if let Some(seen) = marked.symbols.get_mut(name.index()) {
                    *seen = true;
                }
                // Both halves of an accessor, because either may be the only reference to its
                // function: a private accessor's getter is reachable through nothing else at all.
                let held = match element {
                    crate::heap::PrivateElement::Field(value)
                    | crate::heap::PrivateElement::Method(value) => [*value, Value::Undefined],
                    crate::heap::PrivateElement::Accessor { getter, setter } => [*getter, *setter],
                };
                for value in held {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
            // The buffer a view is a window onto. Nothing else need be holding it: `new
            // DataView(new ArrayBuffer(8))` leaves the buffer named by the view and by nothing at
            // all, and collecting it would leave a window onto bytes that are gone.
            if let Some(view) = object.view() {
                pending.push(view.buffer);
            }
            // Every key and value a `Map` or a `Set` holds. Nothing else need be holding them: a
            // collection is precisely a thing that keeps values alive on purpose.
            //
            // …and a `WeakMap` or a `WeakSet` is precisely a thing that does not, so its entries
            // are skipped here and settled afterwards by [`Heap::mark_weak_entries`]. Marking them
            // here instead would make the weak collections strong ones with a different name —
            // every test would still pass, and a program that used one as a cache would never free
            // anything.
            // §26.1's target is *not* marked, which is the whole of what a `WeakRef` is. A
            // registry's callback and its held values are, because it will hand them over: the
            // targets and the unregister tokens beside them are the weak half, and §26.2.3.1
            // step 5 refuses a held value that is the target for exactly this reason — holding it
            // strongly would keep the target alive through its own registration.
            // §10.5's target and handler. A proxy is very often the only thing naming either —
            // `new Proxy({}, {})` leaves both reachable through it and nowhere else — and a
            // *revoked* proxy names neither, which is what lets both be collected once it is.
            // §22.2.9 — an iterator holds the regular expression it is walking with, which
            // nothing else may be pointing at once the `for`-`of` owns it.
            if let Some(matches) = object.matches() {
                pending.push(matches.regexp);
            }
            // §27.5.1's parked execution, which is the one place a value can be *on a stack* and
            // still be nowhere the roots reach: the operands a generator had half-built are not on
            // the machine's stack any more, and its environment is named by no frame. Collecting
            // either would leave the generator to resume into rubbish.
            if let Some(parked) = object.suspension() {
                for value in parked.reachable() {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
                self.mark_environment(parked.environment(), marked);
            }
            if let Some(proxy) = object.proxy()
                && let Some((target, handler)) = proxy.parts()
            {
                pending.push(target);
                pending.push(handler);
            }
            // §27.1.5's helper holds its source iterator, its callback and — part-way through a
            // `flatMap` — the inner iterator it is drawing from. Nothing else may be holding any
            // of them: `[1, 2].values().map(f)` leaves both the array's iterator and `f` named by
            // the helper alone.
            if let Some(helper) = object.helper() {
                for held in [Some(helper.source), Some(helper.next)]
                    .into_iter()
                    .chain(
                        helper
                            .inner
                            .map(|(iterator, next)| [iterator, next])
                            .into_iter()
                            .flatten()
                            .map(Some),
                    )
                    .flatten()
                {
                    match held {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
                let callback = match &helper.what {
                    crate::heap::Step::Map(function)
                    | crate::heap::Step::Filter(function)
                    | crate::heap::Step::FlatMap(function) => Some(*function),
                    crate::heap::Step::Take(_) | crate::heap::Step::Drop(_) => None,
                };
                if let Some(Value::Object(reached)) = callback {
                    pending.push(reached);
                }
            }
            if let Some(Weak::Registry(registry)) = object.weak() {
                self.mark_value(registry.cleanup, marked);
                for cell in &registry.cells {
                    match cell.held {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
            if let Some(collection) = object.collection()
                && !collection.kind().weak()
            {
                for value in collection
                    .live_entries()
                    .flat_map(|(key, value)| [key, value])
                {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
            // Everything a promise is holding on behalf of something that has not happened yet, and
            // it is the *only* thing holding most of it. `Promise.resolve(o).then(f)` leaves `o`
            // named by nothing but the promise's result and `f` named by nothing but its reaction
            // list — and a reaction also holds the capability whose promise is settled afterwards,
            // which is the object a program is usually still waiting on.
            if let Some(promise) = object.promise() {
                let reactions = promise.fulfil.iter().chain(promise.reject.iter());
                let held = std::iter::once(promise.result).chain(reactions.flat_map(|reaction| {
                    let capability = reaction.capability;
                    [
                        reaction.handler.unwrap_or(Value::Undefined),
                        capability.map_or(Value::Undefined, |it| it.promise),
                        capability.map_or(Value::Undefined, |it| it.resolve),
                        capability.map_or(Value::Undefined, |it| it.reject),
                    ]
                }));
                for value in held {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
            // …and what a resolving function settles, which is the other direction of the same
            // pair: a program that keeps only the `resolve` it was handed keeps the promise alive
            // through it, and that is the whole point of holding one.
            match object.role() {
                // §27.7.5.1's capability: the promise an `async` execution will settle, and the two
                // functions that settle it. Nothing else names the pair once the body is parked —
                // the caller holds the promise, and the resolve and reject functions are reachable
                // through here and nowhere at all.
                Some(crate::heap::Role::Await(capability)) => {
                    for value in [capability.promise, capability.resolve, capability.reject] {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                // §27.6.1's `[[AsyncGeneratorQueue]]`. Every request holds a promise its caller is
                // waiting on and the two functions that settle it, and while the generator is
                // parked there is no other path to any of them — a caller that dropped its promise
                // still gets it settled, so the queue is what keeps it alive.
                Some(crate::heap::Role::Requests(requests)) => {
                    for request in requests {
                        let capability = request.capability;
                        for value in [
                            request.value,
                            capability.promise,
                            capability.resolve,
                            capability.reject,
                        ] {
                            match value {
                                Value::Object(reached) => pending.push(reached),
                                other => self.mark_value(other, marked),
                            }
                        }
                    }
                }
                // §27.1.4's wrapper keeps the sync iterator it stands in front of, and its `next`
                // read once. Nothing else names either: the loop only ever holds the wrapper.
                Some(crate::heap::Role::SyncIterator { iterator, next }) => {
                    for value in [*iterator, *next] {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                Some(crate::heap::Role::Resolve(settler) | crate::heap::Role::Reject(settler)) => {
                    pending.push(settler.promise);
                }
                // §28.2.2.1.1 — a revocation function keeps its proxy alive, because it is the only
                // thing that can still do anything to it: `Proxy.revocable(t, h).revoke` handed out
                // on its own is a function whose whole purpose is that one object.
                Some(crate::heap::Role::Revoke(proxy)) => pending.push(*proxy),
                Some(crate::heap::Role::Finally {
                    handler: first,
                    constructor: second,
                })
                | Some(crate::heap::Role::Executor {
                    resolve: first,
                    reject: second,
                }) => {
                    for value in [*first, *second] {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                Some(crate::heap::Role::Thunk(value) | crate::heap::Role::Thrower(value)) => {
                    match *value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
                // A combinator's shared record, which holds the values gathered so far and the
                // capability they will resolve. Nothing else need be holding any of it: the group's
                // own promise is what a program keeps, and everything above hangs off this.
                Some(crate::heap::Role::Element { gather, .. }) => {
                    let state = gather.borrow();
                    let held = state.values.iter().copied().chain([
                        state.capability.promise,
                        state.capability.resolve,
                        state.capability.reject,
                    ]);
                    for value in held {
                        match value {
                            Value::Object(reached) => pending.push(reached),
                            other => self.mark_value(other, marked),
                        }
                    }
                }
                None => {}
            }
            // A method's home object, which nothing else need be holding: a method taken off a class
            // and stored on its own still reads `super.x` through it, so the class's prototype is
            // reachable through the method and by no other path.
            if let Some(home) = object.home_object() {
                pending.push(home);
            }
            // Both halves of what an arrow captured, because either can be an object and the arrow
            // may be the only thing left holding it: `function F() { return () => new.target }`
            // hands back an arrow whose `new.target` is a constructor nothing else need name.
            for captured in object
                .lexical()
                .into_iter()
                .flat_map(|lexical| [lexical.this_value, lexical.new_target])
            {
                match captured {
                    Value::Object(reached) => pending.push(reached),
                    other => self.mark_value(other, marked),
                }
            }
            // What an iterator is walking, which nothing else need be holding: after
            // `var i = [1, 2].values()` the array has no other name, and collecting it would
            // leave an iterator that steps into a slot something else has since been given.
            match object.iteration().map(|found| found.over) {
                Some(Value::Object(reached)) => pending.push(reached),
                Some(other) => self.mark_value(other, marked),
                None => {}
            }
            for key in object.own_property_keys(self) {
                // A key is reachable *because* it is a key: a property nothing else names still
                // has its name. Both kinds — a Symbol key is the only reference to that Symbol
                // once the code that made it has gone, and collecting it would leave a property
                // nothing could ever ask for again.
                if let Some(id) = key.as_string()
                    && let Some(seen) = marked.strings.get_mut(id.index())
                {
                    *seen = true;
                }
                if let Some(id) = key.as_symbol()
                    && let Some(seen) = marked.symbols.get_mut(id.index())
                {
                    *seen = true;
                }
                let Some(property) = object.get_own_property(key) else {
                    continue;
                };
                let values = match property.kind {
                    PropertyKind::Data { value, .. } => [value, Value::Undefined],
                    PropertyKind::Accessor { getter, setter } => [getter, setter],
                };
                for value in values {
                    match value {
                        Value::Object(reached) => pending.push(reached),
                        other => self.mark_value(other, marked),
                    }
                }
            }
        }
    }

    /// Mark an environment, its parent chain, and every variable along it.
    fn mark_environment(&self, from: EnvironmentId, marked: &mut Marked) {
        let mut next = Some(from);
        while let Some(id) = next {
            match marked.environments.get_mut(id.index()) {
                Some(true) | None => return,
                Some(seen) => *seen = true,
            }
            for value in self.environment_slots(id) {
                self.mark_value(value, marked);
            }
            // §9.1.1.2's binding object, which a `with` scope's names live on. Reached from the
            // environment and from nowhere else once the statement has been entered — the value
            // the header evaluated is off the stack by then — so missing it here frees an object
            // the body is still reading names from.
            if let Some(object) = self.environment_binding_object(id) {
                self.mark_object(object, marked);
            }
            // §16.2.1.5.2's import bindings, which reach *sideways* rather than outwards: an
            // importing module names slots of a module its parent chain never touches, so a walk
            // that followed only parents would free the exporter's environment under it.
            for (from, _) in self.environment_aliases(id) {
                self.mark_environment(from, marked);
            }
            next = self.environment_parent(id);
        }
    }

    /// Free everything unmarked, leaving a hole where it was.
    ///
    /// A hole rather than a compaction: moving an object would mean finding every handle to it,
    /// and a handle is a plain index that anything may hold — including an embedder. So a slot is
    /// emptied and its generation moves on, which is what makes a stale handle answer `None`
    /// instead of addressing whatever is put there next.
    fn sweep(&mut self, marked: &Marked) -> Collected {
        let mut freed = Collected {
            objects: 0,
            environments: 0,
            strings: 0,
            symbols: 0,
            bigints: 0,
        };
        // Before anything is freed, because a weak collection that *survives* has to lose the
        // entries whose keys do not — that is the whole observable effect of weakness, and it is
        // observable only as memory that comes back.
        //
        // A collection that is itself about to be freed is pruned too, and skipping those would be
        // an optimisation with no test behind it: the entries are dropped either way a moment
        // later, so no input could tell the guard from its absence. Cheaper to walk them than to
        // carry a branch nothing can justify.
        for object in self.objects.iter_mut() {
            if let Some(collection) = object.as_mut().and_then(Object::collection_mut)
                && collection.kind().weak()
            {
                collection.retain_keys(|key| reachable(key, marked));
            }
            // §26.2's cells go the same way and for the same reason: a cell whose target the walk
            // could not reach is a cell nothing can ask about again. A `WeakRef`'s target needs no
            // pruning — DR-0010 leaves the freed slot empty and never reuses it, so the handle
            // itself becomes the answer `deref` gives.
            if let Some(Weak::Registry(registry)) = object.as_mut().and_then(Object::weak_mut) {
                registry.retain_cells(|target| reachable(target.as_value(), marked));
            }
        }
        // Zipped rather than indexed. The marks were sized from the arenas and nothing allocates
        // between, so the two are the same length — and `zip` says that rather than an index with
        // a default for a case that cannot happen.
        for (object, marked) in self.objects.iter_mut().zip(&marked.objects) {
            if *marked || object.is_none() {
                continue;
            }
            *object = None;
            freed.objects += 1;
        }
        for (environment, marked) in self.environments.iter_mut().zip(&marked.environments) {
            if *marked || environment.is_none() {
                continue;
            }
            *environment = None;
            freed.environments += 1;
        }
        for (string, marked) in self.strings.iter_mut().zip(&marked.strings) {
            if *marked || string.is_none() {
                continue;
            }
            // The units go back to the budget DR-0013 keeps, because they are genuinely given
            // back: the `Box` is dropped here. The *slot* is not — it stays as a `None` for as
            // long as the arena does, which is the cost DR-0010 accepted in exchange for a handle
            // that can never dangle, and which `Heap::footprint` therefore goes on counting.
            self.string_units -= string.as_ref().map_or(0, |units| units.len());
            *string = None;
            freed.strings += 1;
        }
        for (value, marked) in self.bigints.iter_mut().zip(&marked.bigints) {
            if *marked || value.is_none() {
                continue;
            }
            *value = None;
            freed.bigints += 1;
        }
        for (symbol, marked) in self.symbols.iter_mut().zip(&marked.symbols) {
            if *marked || symbol.is_none() {
                continue;
            }
            *symbol = None;
            freed.symbols += 1;
        }
        // §20.4.2.2's registry is a **strong** reference and is deliberately not swept: a
        // registered Symbol outlives every realm, because `Symbol.for("a")` must answer the same
        // Symbol however long ago the last holder of it was collected. It was marked as a root
        // above rather than pruned here.
        //
        // The intern table would otherwise keep pointing at freed Strings, and a later `intern`
        // of the same text would hand back a handle to nothing.
        let strings = &self.strings;
        self.interned
            .retain(|_, id| strings.get(id.index()).is_some_and(Option::is_some));
        freed
    }

    /// The values in an environment's slots, copied out so the walk may borrow the heap.
    ///
    /// A slot in §9.1.1.1's uninitialised state holds no value and so reaches nothing — a `let`
    /// above its declaration is a binding with nothing behind it to keep alive.
    fn environment_slots(&self, id: EnvironmentId) -> Vec<Value> {
        self.environments
            .get(id.index())
            .and_then(Option::as_ref)
            .map(|found| found.slots().iter().flatten().copied().collect())
            .unwrap_or_default()
    }

    /// The environments an environment's import bindings reach into — §16.2.1.5.2.
    fn environment_aliases(&self, id: EnvironmentId) -> Vec<(EnvironmentId, u32)> {
        self.imports
            .iter()
            .filter(|((importer, _), _)| *importer == id)
            .map(|(_, target)| *target)
            .collect()
    }

    /// An environment's parent, if it has one and exists.
    fn environment_parent(&self, id: EnvironmentId) -> Option<EnvironmentId> {
        self.environments
            .get(id.index())
            .and_then(Option::as_ref)
            .and_then(|found| found.parent())
    }
}

/// Which slots the mark phase reached.
///
/// A bit per slot rather than a flag on each object, so that a collection leaves no trace behind
/// it: the marks are gone the moment it returns, and nothing has to be cleared for the next one.
struct Marked {
    objects: Vec<bool>,
    environments: Vec<bool>,
    strings: Vec<bool>,
    symbols: Vec<bool>,
    bigints: Vec<bool>,
}

/// The index inside a handle, for the collector's own use.
pub(super) trait Slot {
    /// Which slot of its arena this handle names.
    fn index(&self) -> usize;
}

impl Slot for crate::heap::BigIntId {
    fn index(&self) -> usize {
        self.0
    }
}

impl Slot for StringId {
    fn index(&self) -> usize {
        self.0
    }
}

impl Slot for ObjectId {
    fn index(&self) -> usize {
        self.0
    }
}

impl Slot for EnvironmentId {
    fn index(&self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{
        Cell, Collection, CollectionKind, Holdable, PropertyDescriptor, PropertyKey, Registry,
    };

    fn define(heap: &mut Heap, object: ObjectId, name: &str, value: Value) {
        let key = PropertyKey::from_units(heap, &name.encode_utf16().collect::<Vec<_>>());
        let descriptor = PropertyDescriptor {
            value: Some(value),
            writable: Some(true),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::EMPTY
        };
        assert!(heap.define_own_property(object, key, &descriptor));
    }

    #[test]
    fn an_import_binding_keeps_the_module_it_reaches_into() {
        // §16.2.1.5.2's bindings reach **sideways** rather than outwards: an importing module names
        // slots of a module its parent chain never touches. A walk that followed only parents would
        // free the exporter's environment under it, and the importer would then read a slot that is
        // not there — which is a wrong value rather than a crash, and the worst kind.
        let mut heap = Heap::new();
        let exporter = heap.new_environment(None, 1);
        let importer = heap.new_environment(None, 1);
        let held = heap.new_object(None);
        assert!(heap.set_variable(exporter, 0, Value::Object(held)));
        assert!(heap.bind_import(importer, 0, exporter, 0));
        // Only the *importer* is rooted. Everything the exporter holds has to survive through the
        // binding alone.
        let roots = Roots {
            environments: vec![importer],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(held).is_some(), "the exported value survives");
        assert!(matches!(
            heap.variable(importer, 0),
            Some(Some(Value::Object(_)))
        ));
        // …and an environment nothing reaches, by a parent or a binding, is still freed — or the
        // test above would pass with a collector that frees nothing.
        let mut heap = Heap::new();
        let unreached = heap.new_environment(None, 1);
        let orphan = heap.new_object(None);
        assert!(heap.set_variable(unreached, 0, Value::Object(orphan)));
        heap.collect(&Roots::default());
        assert!(heap.object(orphan).is_none());
    }

    #[test]
    fn what_nothing_reaches_is_freed_and_what_something_reaches_is_not() {
        let mut heap = Heap::new();
        let kept = heap.new_object(None);
        let dropped = heap.new_object(None);
        let roots = Roots {
            values: vec![Value::Object(kept)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.objects, 1);
        assert!(heap.object(kept).is_some());
        // The handle survives the object; it addresses an empty slot and says so, which is the
        // same narrow promise a handle from another heap already makes.
        assert!(heap.object(dropped).is_none());
        assert_eq!(heap.object_count(), 1);
    }

    #[test]
    fn an_iterator_keeps_what_it_is_walking() {
        // The array has no other name once the expression that made it is gone, so the iterator
        // is the only thing reaching it. Collecting it would leave the iterator stepping into a
        // slot that something else has since been given.
        let mut heap = Heap::new();
        let walked = heap.new_object(None);
        let iterator = heap.new_iterator(
            walked,
            crate::heap::Iteration {
                over: Value::Object(walked),
                at: 0,
                kind: crate::heap::Iterated::Values,
                done: false,
            },
        );
        let elsewhere = heap.new_object(None);
        let roots = Roots {
            values: vec![Value::Object(iterator)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.objects, 1);
        assert!(heap.object(walked).is_some());
        assert!(heap.object(elsewhere).is_none());
    }

    #[test]
    fn a_symbol_is_kept_by_whatever_can_still_reach_it() {
        let mut heap = Heap::new();
        let described = heap.intern(&"kept".encode_utf16().collect::<Vec<_>>());
        let kept = heap.new_symbol(Some(described));
        let dropped = heap.new_symbol(None);
        let roots = Roots {
            values: vec![Value::Symbol(kept)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.symbols, 1);
        assert!(heap.symbol(kept).is_some());
        assert!(heap.symbol(dropped).is_none());
        assert_eq!(heap.symbol_count(), 1);
        // …and its description with it. A Symbol's description is a String nothing else may be
        // holding, so a collector that marked the Symbol and not its text would leave
        // `sym.description` reading from a freed slot.
        assert_eq!(heap.symbol_description(kept), Some(described));
        assert!(heap.string(described).is_some());
    }

    #[test]
    fn a_symbol_used_as_a_key_is_reached_through_the_object_it_keys() {
        // The case that makes this worth marking at all: nothing holds the Symbol except the
        // property it names. Collecting it would leave a property no operation could ever ask
        // for again — reachable in the heap and unreachable in the language.
        let mut heap = Heap::new();
        let object = heap.new_object(None);
        let symbol = heap.new_symbol(None);
        let key = PropertyKey::from_symbol(symbol);
        let descriptor = PropertyDescriptor::data(Value::Number(1.0));
        assert!(heap.define_own_property(object, key, &descriptor));
        let roots = Roots {
            values: vec![Value::Object(object)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.symbols, 0);
        assert!(heap.symbol(symbol).is_some());
    }

    #[test]
    fn a_registered_symbol_outlives_everything_that_was_holding_it() {
        // §20.4.2.2 — the registry is a *strong* reference and deliberately not swept.
        // `Symbol.for("a")` must answer the same Symbol however long ago the last other holder of
        // it went away, so this is a root and not a table to be pruned after the fact.
        let mut heap = Heap::new();
        let key = heap.intern(&"a".encode_utf16().collect::<Vec<_>>());
        let registered = heap.registered_symbol(key);
        let ordinary = heap.new_symbol(None);
        let freed = heap.collect(&Roots::default());
        assert_eq!(freed.symbols, 1);
        assert!(heap.symbol(registered).is_some());
        assert!(heap.symbol(ordinary).is_none());
        // …and it is still the one the registry answers with, which is the property the strong
        // reference exists to keep.
        assert_eq!(heap.registered_symbol(key), registered);
        assert_eq!(heap.symbol_registry_key(registered), Some(key));
    }

    #[test]
    fn a_cycle_nothing_reaches_is_freed_which_is_the_whole_reason_for_marking() {
        // The case reference counting cannot do, and it is not a corner: every function in a
        // program is in one, because §10.2.5 gives it a `prototype` whose `constructor` points
        // back. A counting collector would leak all of them.
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(None);
        define(&mut heap, first, "other", Value::Object(second));
        define(&mut heap, second, "other", Value::Object(first));
        let before = heap.object_count();

        let freed = heap.collect(&Roots::default());
        assert_eq!(freed.objects, before);
        assert!(heap.object(first).is_none());
        assert!(heap.object(second).is_none());

        // …and the same cycle survives whole when anything at all reaches into it.
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(None);
        define(&mut heap, first, "other", Value::Object(second));
        define(&mut heap, second, "other", Value::Object(first));
        let roots = Roots {
            values: vec![Value::Object(first)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).objects, 0);
        assert!(heap.object(second).is_some());
    }

    #[test]
    fn everything_an_object_leads_to_survives_with_it() {
        let mut heap = Heap::new();
        let prototype = heap.new_object(None);
        let object = heap.new_object(Some(prototype));
        let held = heap.new_object(None);
        let text = heap.new_string("kept".encode_utf16().collect());
        define(&mut heap, object, "child", Value::Object(held));
        define(&mut heap, object, "text", Value::String(text));
        let orphan = heap.new_object(None);
        let forgotten = heap.new_string("gone".encode_utf16().collect());

        let roots = Roots {
            values: vec![Value::Object(object)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.objects, 1);
        assert!(heap.object(prototype).is_some());
        assert!(heap.object(held).is_some());
        assert!(heap.string(text).is_some());
        assert!(heap.object(orphan).is_none());
        assert!(heap.string(forgotten).is_none());
        // A property's *name* is reachable because it is a name, so the keys survive too — a
        // property nobody else mentions still has one.
        assert!(
            heap.object(object)
                .is_some_and(|found| found.property_count() == 2)
        );

        // …and the names themselves, which are Strings like any other and are reachable *because*
        // they are names. Without that, a surviving object would have properties whose keys had
        // been freed underneath it.
        let names: Vec<String> = heap
            .object(object)
            .map_or_else(Vec::new, |found| found.own_property_keys(&heap))
            .into_iter()
            .map(|key| {
                String::from_utf16_lossy(
                    key.as_string()
                        .and_then(|id| heap.string(id))
                        .unwrap_or(&[]),
                )
            })
            .collect();
        assert_eq!(names, ["child", "text"]);
    }

    #[test]
    fn an_environment_keeps_its_parents_and_its_variables() {
        // What a closure is, from the collector's side: a function reaches the environment it was
        // written in, that environment reaches its parent, and every variable along the way is
        // kept because something can still name it.
        let mut heap = Heap::new();
        let outer = heap.new_environment(None, 1);
        let inner = heap.new_environment(Some(outer), 1);
        let held = heap.new_object(None);
        assert!(heap.set_variable(outer, 0, Value::Object(held)));
        let unreachable = heap.new_environment(None, 1);

        let roots = Roots {
            environments: vec![inner],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.environments, 1);
        assert!(heap.environment_at(inner, 1).is_some());
        assert!(heap.object(held).is_some());
        assert!(heap.variable(unreachable, 0).is_none());
    }

    #[test]
    fn a_function_keeps_the_environment_it_closed_over() {
        let mut heap = Heap::new();
        let captured = heap.new_environment(None, 1);
        let held = heap.new_object(None);
        assert!(heap.set_variable(captured, 0, Value::Object(held)));
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let function = heap.new_function(prototype, body, captured, None);

        let roots = Roots {
            values: vec![Value::Object(function)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).environments, 0);
        // The variable the closure can still read is still there, which is the property that
        // makes a closure work at all.
        assert!(heap.object(held).is_some());
    }

    #[test]
    fn an_arrow_keeps_what_it_closed_over() {
        // §15.3's captured `this` is an edge in the object graph like any other, and it is the
        // *only* one holding the receiver: `function F() { return () => this; }` leaves nothing
        // else pointing at the constructed object. A collector that walked the environment but
        // not this field would free the object the arrow is about to answer with — a
        // use-after-free with the types intact, which is the wrong kind of failure.
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 0);
        let receiver = heap.new_object(None);
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let arrow = heap.new_function(
            prototype,
            body,
            environment,
            Some(crate::heap::Lexical {
                this_value: Value::Object(receiver),
                new_target: Value::Undefined,
                home: None,
            }),
        );

        let roots = Roots {
            values: vec![Value::Object(arrow)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(receiver).is_some());
        // The captured `new.target` is the same edge and needs walking for the same reason, and it
        // is a *separate* one: `function F() { return () => new.target; }` leaves the constructor
        // reachable only through the arrow, and the object graph does not join it to the `this`.
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 0);
        let target = heap.new_object(None);
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let arrow = heap.new_function(
            prototype,
            body,
            environment,
            Some(crate::heap::Lexical {
                this_value: Value::Undefined,
                new_target: Value::Object(target),
                home: None,
            }),
        );
        let roots = Roots {
            values: vec![Value::Object(arrow)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(target).is_some());
        // …and a captured String is kept for the same reason, a primitive `this` being reachable
        // exactly as far as the arrow is.
        let mut heap = Heap::new();
        let environment = heap.new_environment(None, 0);
        let text = heap.intern(&"held".encode_utf16().collect::<Vec<_>>());
        let body = std::rc::Rc::new(crate::compile::Chunk::from_parts(Vec::new(), Vec::new()));
        let prototype = heap.new_object(None);
        let arrow = heap.new_function(
            prototype,
            body,
            environment,
            Some(crate::heap::Lexical {
                this_value: Value::String(text),
                new_target: Value::Undefined,
                home: None,
            }),
        );
        let roots = Roots {
            values: vec![Value::Object(arrow)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).strings, 0);
        assert!(heap.string(text).is_some());
    }

    #[test]
    fn a_private_field_keeps_its_name_and_its_value() {
        // §7.3.28's list is an edge in the object graph that no property walk visits, and both halves
        // of each entry are reachable *only* through it: the Private Name lives in a compiler slot no
        // script can spell, and the value is in the list. A collector that skipped this would free
        // what `this.#x` is about to answer with, and free the Symbol that finds it — a
        // use-after-free with the types intact, which is the wrong kind of failure.
        let mut heap = Heap::new();
        let instance = heap.new_object(None);
        let name = heap.new_symbol(None);
        let held = heap.new_object(None);
        assert!(heap.add_private_field(instance, name, Value::Object(held)));

        let roots = Roots {
            values: vec![Value::Object(instance)],
            ..Roots::default()
        };
        let freed = heap.collect(&roots);
        assert_eq!(freed.symbols, 0);
        assert!(heap.object(held).is_some());
        assert!(heap.symbol(name).is_some());
        // …and the field still answers, which is the property the two assertions above are for.
        let found = heap
            .object(instance)
            .and_then(|object| object.private_element(name));
        assert!(matches!(
            found.and_then(crate::heap::PrivateElement::value),
            Some(Value::Object(reached)) if reached == held
        ));
    }

    #[test]
    fn sweeping_gives_back_a_freed_strings_units_but_not_its_slot() {
        // The two halves of DR-0010's bargain, told apart by DR-0013's number. A swept String's
        // units are genuinely returned — the `Box` is dropped here — so the budget must see them
        // come back, or a program that collects would be charged forever for memory it no longer
        // holds. Its *slot* is not returned and never will be, which is the price of a handle
        // that cannot dangle, and the footprint goes on counting it.
        let mut heap = Heap::new();
        let kept = heap.new_string("kept".encode_utf16().collect());
        heap.new_string("gone".encode_utf16().collect());
        let both = heap.footprint();

        let roots = Roots {
            values: vec![Value::String(kept)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).strings, 1);
        // Four units at two bytes each, and the slot left behind — so the drop is exactly the
        // units and nothing more.
        assert_eq!(heap.footprint(), both - 4 * size_of::<u16>());
        assert_eq!(
            heap.string(kept),
            Some(&"kept".encode_utf16().collect::<Vec<_>>()[..])
        );
        // Collecting again frees nothing, so the number does not move — a decrement applied to an
        // already-empty slot would take the budget below what is really held.
        assert_eq!(heap.collect(&roots).strings, 0);
        assert_eq!(heap.footprint(), both - 4 * size_of::<u16>());
    }

    #[test]
    fn the_intern_table_is_not_a_root_and_forgets_a_freed_name() {
        // `Heap::intern` warned that an interned key lives as long as the heap. It does not any
        // more: a name nothing uses is freed, and the table forgets it — so a later `intern` of
        // the same text makes a *new* String rather than handing back a handle to nothing.
        let mut heap = Heap::new();
        let name = heap.intern(&"gone".encode_utf16().collect::<Vec<_>>());
        assert_eq!(heap.collect(&Roots::default()).strings, 1);
        assert!(heap.string(name).is_none());
        let again = heap.intern(&"gone".encode_utf16().collect::<Vec<_>>());
        assert_ne!(again, name);
        assert!(heap.string(again).is_some());
    }

    /// A collection of `kind` on the heap, with its entries already in it.
    fn holding(heap: &mut Heap, kind: CollectionKind, entries: &[(Value, Value)]) -> ObjectId {
        let object = heap.new_object(None);
        let mut collection = Collection::new(kind);
        for (key, value) in entries {
            collection.push(*key, *value);
        }
        if let Some(found) = heap.object_mut(object) {
            found.set_collection(collection);
        }
        object
    }

    /// How many entries a collection still has, for the rows that check one was pruned.
    fn entries_of(heap: &Heap, object: ObjectId) -> usize {
        heap.object(object)
            .and_then(Object::collection)
            .map_or(0, Collection::size)
    }

    #[test]
    fn a_weak_map_does_not_keep_its_key_alive_and_a_map_does() {
        // The one difference between §24.1 and §24.3, and it is invisible to a program: an entry
        // whose key nothing else can name goes away. The same arrangement with a `Map` keeps both,
        // which is what says the collector reads the kind rather than freeing indiscriminately.
        for (kind, expected) in [(CollectionKind::Map, 1), (CollectionKind::WeakMap, 0)] {
            let mut heap = Heap::new();
            let key = heap.new_object(None);
            let value = heap.new_object(None);
            let map = holding(
                &mut heap,
                kind,
                &[(Value::Object(key), Value::Object(value))],
            );
            // Only the collection is a root. Nothing else names the key, so a weak map is the one
            // of the two that may let go of it.
            let roots = Roots {
                values: vec![Value::Object(map)],
                ..Roots::default()
            };
            heap.collect(&roots);
            assert_eq!(
                usize::from(heap.object(key).is_some()),
                expected,
                "{kind:?} key"
            );
            // ...and the value goes with it, because a weak entry keeps its value only while its
            // key lives. It does not keep it on its own.
            assert_eq!(
                usize::from(heap.object(value).is_some()),
                expected,
                "{kind:?} value"
            );
            // The entry is gone from the surviving collection rather than left as a pair of
            // handles addressing empty slots.
            assert_eq!(entries_of(&heap, map), expected, "{kind:?} size");
        }
    }

    #[test]
    fn a_weak_entry_whose_key_is_still_named_keeps_its_value() {
        // The other half of the rule. The key is a root, so the entry is live and its value is
        // reachable *through* it -- nothing else names the value at all, which is exactly the case
        // a collector that skipped weak entries altogether would get wrong.
        let mut heap = Heap::new();
        let key = heap.new_object(None);
        let value = heap.new_object(None);
        let map = holding(
            &mut heap,
            CollectionKind::WeakMap,
            &[(Value::Object(key), Value::Object(value))],
        );
        let roots = Roots {
            values: vec![Value::Object(map), Value::Object(key)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(key).is_some());
        assert!(heap.object(value).is_some());
        assert_eq!(entries_of(&heap, map), 1);
    }

    #[test]
    fn a_weak_value_that_is_another_weak_key_is_settled_by_repeating() {
        // Why one pass is not enough. The near map's entry is live, and its *value* is the far
        // map's **key** -- so marking it is what makes the far entry live. A collector that walked
        // the weak entries once would free the far end of that chain, and a program would find
        // `get` answering `undefined` for a key it was still holding.
        //
        // The far map is built first so that it sits earlier in the arena than the entry that
        // makes it live: the pass reaches it before it can be settled, which is the ordering that
        // makes a single pass visibly wrong rather than accidentally right.
        let mut heap = Heap::new();
        let first = heap.new_object(None);
        let second = heap.new_object(None);
        let third = heap.new_object(None);
        let far = holding(
            &mut heap,
            CollectionKind::WeakMap,
            &[(Value::Object(second), Value::Object(third))],
        );
        let near = holding(
            &mut heap,
            CollectionKind::WeakMap,
            &[(Value::Object(first), Value::Object(second))],
        );
        let roots = Roots {
            values: vec![
                Value::Object(near),
                Value::Object(far),
                Value::Object(first),
            ],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(first).is_some(), "the root itself");
        assert!(heap.object(second).is_some(), "reached through one entry");
        assert!(heap.object(third).is_some(), "reached through two");
        assert_eq!(entries_of(&heap, near), 1, "near");
        assert_eq!(entries_of(&heap, far), 1, "far");
    }

    #[test]
    fn a_weak_set_lets_go_of_a_value_nothing_else_names() {
        // A weak set's entry *is* its key, so there is nothing for the repeating pass to mark and
        // the whole of its weakness is the sweep. One value is a root and one is not, so this also
        // says the pruning is per entry rather than per collection.
        let mut heap = Heap::new();
        let kept = heap.new_object(None);
        let dropped = heap.new_object(None);
        let set = holding(
            &mut heap,
            CollectionKind::WeakSet,
            &[
                (Value::Object(kept), Value::Object(kept)),
                (Value::Object(dropped), Value::Object(dropped)),
            ],
        );
        let roots = Roots {
            values: vec![Value::Object(set), Value::Object(kept)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(kept).is_some());
        assert!(heap.object(dropped).is_none());
        assert_eq!(entries_of(&heap, set), 1);
    }

    #[test]
    fn the_repeating_pass_leaves_a_strong_collection_alone() {
        // The guard that keeps the ephemeron pass to weak collections, and it is not merely an
        // optimisation. A `Map` that is itself unreachable is never walked by the mark phase, so
        // its entries are dead however live their keys are elsewhere -- and a pass that looked at
        // it would see a marked key, mark the value, and keep alive an object whose only path ran
        // through a map nothing can name.
        let mut heap = Heap::new();
        let key = heap.new_object(None);
        let value = heap.new_object(None);
        let orphan = holding(
            &mut heap,
            CollectionKind::Map,
            &[(Value::Object(key), Value::Object(value))],
        );
        // The key is a root; the map is not, and the value is reachable only through it.
        let roots = Roots {
            values: vec![Value::Object(key)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(key).is_some(), "the root itself");
        assert!(heap.object(orphan).is_none(), "nothing names the map");
        assert!(
            heap.object(value).is_none(),
            "its only path ran through the map"
        );
    }

    #[test]
    fn a_weak_key_no_builtin_could_have_stored_keeps_its_entry() {
        // §7.2.10 lets only an Object or an unregistered Symbol be a weak key, so `set` refuses
        // everything else and a collection built by running JavaScript can never hold one. A
        // collection built *here* can, and the collector still has to answer about it -- the two
        // ways to be wrong are dropping a live entry and keeping a dead one, and only the first is
        // a use-after-free. So a key it cannot reason about is treated as reachable.
        let mut heap = Heap::new();
        let value = heap.new_object(None);
        let map = holding(
            &mut heap,
            CollectionKind::WeakMap,
            &[(Value::Number(1.0), Value::Object(value))],
        );
        let roots = Roots {
            values: vec![Value::Object(map)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert_eq!(entries_of(&heap, map), 1, "the entry is kept");
        assert!(heap.object(value).is_some(), "and so is what it holds");
    }

    #[test]
    fn a_weak_key_this_heap_never_issued_is_not_reachable() {
        // The same narrow promise every handle here makes (DR-0010): a handle from another heap
        // addresses nothing, and asking about it answers rather than panicking. For a weak key the
        // answer has to be "not reachable" -- a foreign index is past the end of the marks, and
        // reading past the end as *reached* would pin every such entry for ever.
        //
        // Both arenas are asked, because an object and a Symbol are marked in different vectors and
        // an implementation can get one right while getting the other wrong.
        let mut other = Heap::new();
        for _ in 0..8 {
            other.new_object(None);
            other.new_symbol(None);
        }
        let foreign_object = other.new_object(None);
        let foreign_symbol = other.new_symbol(None);

        let mut heap = Heap::new();
        let kept = heap.new_object(None);
        let map = holding(
            &mut heap,
            CollectionKind::WeakMap,
            &[
                (Value::Object(kept), Value::Undefined),
                (Value::Object(foreign_object), Value::Undefined),
                (Value::Symbol(foreign_symbol), Value::Undefined),
            ],
        );
        let roots = Roots {
            values: vec![Value::Object(map), Value::Object(kept)],
            ..Roots::default()
        };
        heap.collect(&roots);
        // Only the entry this heap issued a key for survives, which says both arenas were read as
        // "past the end is not reached" rather than one of them defaulting the other way.
        assert_eq!(entries_of(&heap, map), 1);
    }

    #[test]
    fn a_weak_ref_does_not_keep_its_target_and_leaves_a_slot_that_answers() {
        // §26.1 -- the target is not marked, so a `WeakRef` is the only thing naming it and it goes.
        // What is left is the handle addressing an empty slot, which is what `deref` reads: DR-0010
        // never reuses a slot, so an empty one means collected and can never come to mean anything
        // else. A collector that marked the target would make `deref` an ordinary reference.
        let mut heap = Heap::new();
        let target = heap.new_object(None);
        let reference = heap.new_object(None);
        if let Some(found) = heap.object_mut(reference) {
            found.set_weak(Weak::Ref(Holdable::Object(target)));
        }
        let roots = Roots {
            values: vec![Value::Object(reference)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(reference).is_some(), "the reference itself");
        assert!(
            heap.object(target).is_none(),
            "nothing else named the target"
        );

        // ...and while something else does name it, it stays -- which is the other half, and says
        // the reference is not somehow *preventing* the target from being marked.
        let mut heap = Heap::new();
        let target = heap.new_object(None);
        let reference = heap.new_object(None);
        if let Some(found) = heap.object_mut(reference) {
            found.set_weak(Weak::Ref(Holdable::Object(target)));
        }
        let roots = Roots {
            values: vec![Value::Object(reference), Value::Object(target)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(target).is_some());
    }

    #[test]
    fn a_registry_holds_its_callback_and_what_it_would_hand_over_but_not_its_target() {
        // §26.2's three kinds of reference in one arrangement. The callback and the held value are
        // strong -- the registry will hand them over, so it must still have them -- and the target
        // and the unregister token are weak. Each is named by nothing else, so each row is about
        // that one edge and no other.
        let mut heap = Heap::new();
        let cleanup = heap.new_object(None);
        let held = heap.new_object(None);
        let target = heap.new_object(None);
        let token = heap.new_object(None);
        let registry = heap.new_object(None);
        if let Some(found) = heap.object_mut(registry) {
            found.set_weak(Weak::Registry(Registry {
                cleanup: Value::Object(cleanup),
                cells: vec![Cell {
                    target: Holdable::Object(target),
                    held: Value::Object(held),
                    token: Some(Holdable::Object(token)),
                }],
            }));
        }
        let roots = Roots {
            values: vec![Value::Object(registry)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(cleanup).is_some(), "the callback is strong");
        assert!(heap.object(held).is_some(), "the held value is strong");
        assert!(heap.object(target).is_none(), "the target is weak");
        assert!(heap.object(token).is_none(), "the token is weak");
        // §26.2's liveness rule -- the cell went with its target, because nothing can ask about it
        // again: a program that could still name the target is a program the cell was kept for.
        let cells = match heap.object(registry).and_then(Object::weak) {
            Some(Weak::Registry(found)) => found.cells.len(),
            _ => usize::MAX,
        };
        assert_eq!(cells, 0);
    }

    #[test]
    fn a_registry_keeps_the_cell_of_a_target_something_still_names() {
        // The other half again, and the row that says the pruning is per cell rather than per
        // registry: one target is a root and one is not.
        let mut heap = Heap::new();
        let cleanup = heap.new_object(None);
        let kept = heap.new_object(None);
        let dropped = heap.new_object(None);
        let registry = heap.new_object(None);
        if let Some(found) = heap.object_mut(registry) {
            found.set_weak(Weak::Registry(Registry {
                cleanup: Value::Object(cleanup),
                cells: vec![
                    Cell {
                        target: Holdable::Object(kept),
                        held: Value::Number(1.0),
                        token: None,
                    },
                    Cell {
                        target: Holdable::Object(dropped),
                        held: Value::Number(2.0),
                        token: None,
                    },
                ],
            }));
        }
        let roots = Roots {
            values: vec![Value::Object(registry), Value::Object(kept)],
            ..Roots::default()
        };
        heap.collect(&roots);
        assert!(heap.object(kept).is_some());
        assert!(heap.object(dropped).is_none());
        let remaining = match heap.object(registry).and_then(Object::weak) {
            Some(Weak::Registry(found)) => found.cells.len(),
            _ => usize::MAX,
        };
        assert_eq!(remaining, 1, "one cell went and one stayed");
    }

    #[test]
    fn a_long_chain_of_objects_does_not_run_out_of_stack() {
        // The mark phase walks the graph, and a graph is as deep as a program makes it: a list of
        // a hundred thousand links is a chain of a hundred thousand objects. Recursing would run
        // out of Rust stack on *data* rather than on nesting, which DR-0002 does not allow.
        let mut heap = Heap::new();
        let mut previous = heap.new_object(None);
        let head = previous;
        for _ in 0..100_000 {
            let next = heap.new_object(None);
            define(&mut heap, previous, "next", Value::Object(next));
            previous = next;
        }
        let roots = Roots {
            values: vec![Value::Object(head)],
            ..Roots::default()
        };
        assert_eq!(heap.collect(&roots).objects, 0);
        assert!(heap.object(previous).is_some());
    }

    #[test]
    fn collecting_twice_frees_nothing_the_second_time() {
        // The marks live for one collection and are gone when it returns, so nothing has to be
        // cleared and a second pass over the same heap is a no-op rather than a double free.
        let mut heap = Heap::new();
        heap.new_object(None);
        heap.new_string("gone".encode_utf16().collect());
        let first = heap.collect(&Roots::default());
        assert_eq!((first.objects, first.strings), (1, 1));
        let second = heap.collect(&Roots::default());
        assert_eq!((second.objects, second.strings), (0, 0));
    }
}
