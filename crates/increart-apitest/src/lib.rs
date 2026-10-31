//! `increart-apitest`: validates the servers of an IncreArt config
//! (DESIGN.md, Validating servers). Each server preset's servers are
//! opened together, as the app opens them (started ones by the suite
//! itself), then tested case by case: each image server, each chat server
//! prompt enhance uses, and the servers together (routing, two at once,
//! the real engine end to end, one going away mid-run). Every request
//! and its answer is logged per case; the report (`report.md` for a
//! person or an LLM, `report.json`, the traffic and the started servers'
//! logs) is zipped to hand on.
//!
//! The protocol the app's window reads is [`increart_core::apitest`].

mod catalogue;
mod chat;
mod ctx;
mod image;
pub mod report;
mod runner;
mod together;

pub use catalogue::plan;
pub use runner::{Options, run};

/// The program's version line.
pub fn version() -> String {
    format!("increart-apitest {}", env!("CARGO_PKG_VERSION"))
}
