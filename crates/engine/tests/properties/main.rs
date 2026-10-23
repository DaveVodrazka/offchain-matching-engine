//! Property tests: random command sequences checked against invariants on the events.

#![allow(
    clippy::expect_used,
    reason = "test crate: helpers outside #[test] fns build fixtures from known-valid values"
)]

mod invariants;
mod strategy;
