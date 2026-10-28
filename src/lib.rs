//! Airlock: local-first multi-agent workflow coordinator and task management hub.

pub mod cli;
pub mod config;
pub mod core;
pub mod snapshot;

/// Airlock crate version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
