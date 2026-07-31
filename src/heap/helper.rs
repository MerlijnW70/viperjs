//! §27.1.5's Iterator Helper — a `map`, `filter`, `take`, `drop` or `flatMap` part-way through.
//!
//! # Why these are objects rather than closures
//!
//! `[1, 2, 3].values().map(f)` returns *immediately*, having called `f` not at all. What comes back
//! is an iterator that will call it once per `next`, and the specification models that as a
//! generator with internal slots. praxis has no generators yet, so the state a generator would keep
//! in its frame is kept here explicitly: what it is drawing from, what it does to what it draws,
//! and how far it has got.
//!
//! That is not a workaround for the missing generators — it is what the slots hold either way. When
//! the interpreter can suspend a frame this could become one, and nothing a program can see would
//! change.
//!
//! # Why `done` is remembered rather than re-derived
//!
//! §27.1.5.1's `[[GeneratorState]]` becomes `completed` and stays there. Once a helper has said it
//! is done it must go on saying so, even if the iterator underneath it starts answering again —
//! `take(2)` that has yielded two values is finished whatever the source does next.

use crate::value::Value;

/// What a helper does to each value it draws.
#[derive(Debug, Clone)]
pub enum Step {
    /// §27.1.4.8 — hand each value to the callback and yield what comes back.
    Map(Value),
    /// §27.1.4.5 — yield only the values the callback likes.
    Filter(Value),
    /// §27.1.4.12 — yield this many and then stop, closing the source.
    Take(u64),
    /// §27.1.4.4 — skip this many, then yield the rest.
    Drop(u64),
    /// §27.1.4.3 — the callback answers an iterable, and its values are yielded in its place.
    FlatMap(Value),
}

/// §27.1.5.3's internal slots — the underlying iterator, the operation, and where it has got to.
#[derive(Debug)]
pub struct Helper {
    /// `[[UnderlyingIterator]]`'s iterator, which `next` is called on.
    pub source: Value,
    /// …and its `next`, read once when the helper was made — §7.4.10 `GetIteratorDirect`.
    ///
    /// Read once rather than per step, so replacing the source's `next` half-way through a walk
    /// does not change the walk. That is what an Iterator Record is for.
    pub next: Value,
    /// What this helper is.
    pub what: Step,
    /// How many values have been drawn, which `map` and `filter` hand to their callback and which
    /// `take` and `drop` count against.
    pub counter: u64,
    /// The iterator a `flatMap` is currently drawing from, and its `next`.
    ///
    /// `None` between inner iterators. Only `flatMap` ever sets it, and it is here rather than
    /// inside [`Step::FlatMap`] because it changes as the walk proceeds while the callback does
    /// not — a field that is rewritten does not belong beside one that is read.
    pub inner: Option<(Value, Value)>,
    /// Whether it has finished for good — see the module documentation.
    pub done: bool,
}

impl Helper {
    /// A helper drawing from `source`, whose `next` has already been read.
    #[must_use]
    pub fn new(source: Value, next: Value, what: Step) -> Self {
        Self {
            source,
            next,
            what,
            counter: 0,
            inner: None,
            done: false,
        }
    }
}
