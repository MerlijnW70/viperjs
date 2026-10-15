//! What the engine knows of time, and the one place it asks the platform.
//!
//! Two questions, and every reader of the time asks one of them here: how far along a *run* is,
//! which is a monotonic `Instant` and serves DR-0022's budget and §25.4's waits, and what time it
//! *is*, which is the epoch and serves `Date` and the generator's seed.
//!
//! A platform may have no answer to either. `wasm32-unknown-unknown` with no imports is the case
//! that asked: its standard library panics on both `Instant::now` and `SystemTime::now`, so an
//! engine that read the clock directly could not even be built there — `Math` seeds itself as the
//! realm is made. Here both questions answer `None` on such a target, and every caller already has
//! a meaning for "no clock": no deadline, an invalid date, the fixed seed.
//!
//! A host may also answer the second question itself with [`set_clock`], which is how a run is made
//! to see the same time on every machine — DR-0031's reproducibility asks for it, since a program
//! that branches on `Date.now()` is reproducible under no budget.

use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    /// The time a host has fixed, in milliseconds since the epoch, or `None` to read the platform.
    ///
    /// Thread-local for the reason DR-0014's offset is: it is host state, not realm state.
    static FIXED: Cell<Option<f64>> = const { Cell::new(None) };
}

/// Fix the time `Date` reads on this thread, in milliseconds since the epoch — `None` to read the
/// platform's clock again.
///
/// Per thread, like [`crate::set_local_offset`]. A non-finite time is refused and leaves the clock
/// as it was, because a NaN here would make every `Date` invalid and look like a broken platform
/// rather than a bad argument. What a host fixes is what `Date` sees; the run's own budget and a
/// wait's timeout still measure real time, because they bound the host's wait rather than tell the
/// program anything.
pub fn set_clock(milliseconds: Option<f64>) {
    if milliseconds.is_none_or(f64::is_finite) {
        FIXED.with(|fixed| fixed.set(milliseconds.map(f64::trunc)));
    }
}

/// Whether this target has a clock to ask at all.
const HAS_CLOCK: bool = !cfg!(all(target_family = "wasm", target_os = "unknown"));

/// The time `Date` reads, in milliseconds since the epoch: the host's fixed one, or the platform's.
pub(crate) fn epoch_ms() -> Option<f64> {
    FIXED
        .with(Cell::get)
        .or_else(|| platform_since_epoch().map(|since| since.as_millis() as f64))
}

/// The platform's time since the epoch, whatever a host fixed — what the generator is seeded from,
/// since a fixed `Date` is no reason to fix the sequence; [`crate::api::Engine::set_random_seed`]
/// is how a host does that.
pub(crate) fn platform_since_epoch() -> Option<Duration> {
    HAS_CLOCK
        .then(std::time::SystemTime::now)
        .and_then(|now| now.duration_since(std::time::UNIX_EPOCH).ok())
}

/// Now, on the monotonic clock — or `None` where the platform has none.
pub(crate) fn now() -> Option<Instant> {
    HAS_CLOCK.then(Instant::now)
}
