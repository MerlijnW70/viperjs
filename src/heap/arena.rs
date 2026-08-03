//! DR-0019's arena — a slot table that hands freed slots out again, safely.
//!
//! # Why this is one type and not five copies
//!
//! Five arenas need the same three things: allocate into a freed slot when there is one, refuse a
//! handle whose generation has moved on, and put a slot back when a sweep finds it unreachable.
//! Written per arena that is five copies of a rule whose whole purpose is to be exactly right, and
//! the copy nobody reads is the one that would forget the generation check.
//!
//! More than tidiness: [`Arena::get`] is the **only** way to reach a value, and it takes the whole
//! handle rather than an index. There is no method here that takes a bare `usize`, so a caller
//! cannot skip the check by accident — it would have to construct a handle to do it. That is the
//! property this module exists for.
//!
//! # What a generation is for
//!
//! Nothing reused a slot before DR-0019, so a root the collector failed to trace was *invisible*:
//! the slot was freed, nothing overwrote it, and a later read found the same value still sitting
//! there. With reuse that bug hands back a different value of the same type, silently. A
//! generation turns it into `None` — which is what an index past the end has always answered, so
//! no caller gains a case it did not already have.

use super::Handle;

/// One type's slots, their generations, and the ones a sweep has freed.
#[derive(Debug)]
pub(super) struct Arena<T> {
    /// The values, by index. A `None` is a slot that is free or has never been used.
    slots: Vec<Option<T>>,
    /// Which use each slot is on.
    ///
    /// Parallel to `slots` rather than inside the `Option`, because it has to outlive the value:
    /// the question a stale handle asks is about a slot whose contents are gone.
    generations: Vec<u32>,
    /// Slots a sweep freed, waiting to be handed out again.
    ///
    /// A slot whose generation would wrap is **not** put back here. It is retired for the life of
    /// the arena, which is what makes "a handle names its own value or nothing" true with no
    /// exception — see [`Arena::sweep`].
    free: Vec<usize>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            generations: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Arena<T> {
    /// Put `value` in a slot and answer the handle that names it.
    ///
    /// A freed slot if there is one, and a fresh one otherwise. This is the only way a handle of
    /// this arena is ever made, which is what lets the free list be trusted: a construction that
    /// pushed directly would issue a handle whose generation was whatever the last use left.
    pub(super) fn place<H: Handle>(&mut self, value: T) -> H {
        match self.free.pop() {
            Some(index) => {
                let generation = self.generations.get(index).copied().unwrap_or_default();
                if let Some(slot) = self.slots.get_mut(index) {
                    *slot = Some(value);
                }
                H::at(index, generation)
            }
            None => {
                let index = self.slots.len();
                self.slots.push(Some(value));
                self.generations.push(0);
                H::at(index, 0)
            }
        }
    }

    /// The value a handle names, or `None` because it names none.
    ///
    /// Three ways to answer `None` and deliberately one answer: an index past the end, a slot a
    /// sweep emptied, and a slot handed out again since this handle was issued.
    pub(super) fn get<H: Handle>(&self, handle: H) -> Option<&T> {
        let index = handle.index();
        if self.generations.get(index).copied()? != handle.generation() {
            return None;
        }
        self.slots.get(index)?.as_ref()
    }

    /// The same, for the operations that write.
    pub(super) fn get_mut<H: Handle>(&mut self, handle: H) -> Option<&mut T> {
        let index = handle.index();
        if self.generations.get(index).copied()? != handle.generation() {
            return None;
        }
        self.slots.get_mut(index)?.as_mut()
    }

    /// How many slots hold a value — what a program can still reach, rather than what was paid for.
    pub(super) fn live(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// How many slots exist, used and free — what DR-0013's footprint is charged for.
    ///
    /// Not the live count: a free slot still occupies its place in the `Vec`. What DR-0019 changes
    /// is that this stops *growing*, not that it falls.
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Free every slot `marked` did not reach, and answer how many that was.
    ///
    /// `farewell` is run on each value before it goes, for the one arena that has to give
    /// something back besides the slot — a String's code units are charged to DR-0013's budget and
    /// are genuinely returned here.
    pub(super) fn sweep(&mut self, marked: &[bool], mut farewell: impl FnMut(&T)) -> usize {
        let mut freed = 0;
        let mut reusable = Vec::new();
        // Walked in lockstep rather than by index: the marks are built at this arena's length, so
        // a `get` on either would need a default no index could reach. `zip` says so structurally.
        for (index, ((slot, generation), seen)) in self
            .slots
            .iter_mut()
            .zip(self.generations.iter_mut())
            .zip(marked)
            .enumerate()
        {
            if *seen {
                continue;
            }
            let Some(value) = slot.take() else {
                continue;
            };
            farewell(&value);
            freed += 1;
            // The wrap is declined rather than asserted against: put back, this slot would one day
            // issue a handle indistinguishable from one that named an older value. Nothing reaches
            // it — four billion reuses of one slot is more collections than DR-0013's budget can
            // drive — and one comparison is what makes the sentence above true without an "unless".
            if let Some(next) = generation.checked_add(1) {
                *generation = next;
                reusable.push(index);
            }
        }
        // Appended after, because the loop holds the slots mutably.
        self.free.append(&mut reusable);
        freed
    }
}
