//! What "done" means for an ikigai module, as one test.
//!
//! Scaffold: the check catalogue ([`Check`], [`Checks`]) lands first; the walk
//! that runs them against a kernel follows.
#![forbid(unsafe_code)]

mod checks;

pub use checks::{Check, Checks};
