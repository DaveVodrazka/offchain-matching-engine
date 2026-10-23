//! The matching engine for one market: a deterministic state machine that applies
//! [`Command`]s to an order book and reports every outcome as [`Event`]s.

mod book;
mod command;
mod engine;
mod event;

pub use command::Command;
pub use engine::Engine;
pub use event::{Event, RejectionReason};
