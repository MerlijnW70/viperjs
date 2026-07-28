//! Running test262 against praxis, and the number that may only go up.
//!
//! # What this is for
//!
//! Everything before it was built because the specification said so. From here the *number* says
//! what to build: a failure bucket with four thousand tests in it is the next milestone, and one
//! with three is not. That is why M5 comes before M4 in the charter — the harness is what stops
//! the work being chosen by whatever seems interesting.
//!
//! # The expectations file may only shrink
//!
//! A test that fails is written down. A test that starts passing is *removed*, and the run fails
//! if a listed test passes without being removed — because a stale entry is a test nobody is
//! watching. A test that starts failing and is not listed is a regression and fails the run.
//!
//! The file is not a list of excuses. Every entry says what went wrong, so that reading it is
//! reading a work list — and so that an entry which stops being true is visible rather than
//! quietly right for the wrong reason.
//!
//! # What "passing" means here
//!
//! Exactly what `INTERPRETING.md` says, and no more. A positive test passes when it runs to the
//! end without throwing. A negative test passes when it fails in the phase it named, with the
//! error it named — a `parse` test that fails at run time has *not* passed, however loudly it
//! failed.

pub mod drive;
pub mod expectations;
pub mod frontmatter;
pub mod runner;

pub use self::expectations::{Expectations, Judgement};
pub use self::frontmatter::{Frontmatter, Negative};
pub use self::runner::{Outcome, Runner, Verdict};
